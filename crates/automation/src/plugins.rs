//! Plug-ins (RFC-0046): WebAssembly modules that work on an open document through the same tool
//! table as the CLI, MCP and `command_run`, inside the sandbox of the shared `craft-plugin` crate.
//! This module is PdfCraft's side of it: which tools a plug-in may call ([`ALLOWED`],
//! [`refusal`]), one undo step per run that is put back when the run fails
//! ([`pdfcraft_engine::EditMark`]), read-only access checked after every call, each plug-in's
//! settings and the installed plug-ins (the user's `Plug-ins` folder, or this session only). The
//! `plugin_*` tools are in `tools.rs`; see `docs/plugins.md`.
//!
//! A run works on one open document: every call is pinned to it. It never gets a file or a
//! folder, another document, the undo history, the clipboard, the printer, signing, attachments,
//! JavaScript, OCR or the person's preferences (see [`ALLOWED`]).

mod host;
mod runtime;

use std::sync::OnceLock;

pub use craft_plugin;
use craft_plugin::fetch::{self, Fetch};
use craft_plugin::{API_VERSION, AppInfo, Instance, KIND_COMMAND, Limits, Manifest, Policy, RunOutput, Source, UiChange, bundle, catalog, github};
use pdfcraft_engine::{DocId, Label, PLUGIN_RUN_LABEL};
use serde_json::{Map, Value, json};

use crate::error::{Result, ToolError, failed};
use crate::json::{Args, summary};
use crate::{Automation, tools};
use host::ToolHost;
pub use runtime::{InstalledPlugin, PluginPrefs, Runtime};

/// PdfCraft as manifests name it: command plug-ins of every shared ABI version, and only
/// manifests that name `pdfcraft` in `apps`.
pub const APP: AppInfo<'static> =
    AppInfo { id: "pdfcraft", version: env!("CARGO_PKG_VERSION"), api: 1..=API_VERSION, kinds: &[KIND_COMMAND], accept_legacy: false, kind_api: &[] };

/// What one plug-in may keep in its settings (keys and JSON values, in bytes), and its longest key.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
pub const MAX_SETTING_KEY: usize = 128;
/// Catalogs a person may add.
pub const MAX_CATALOGS: usize = 32;
/// Calls that change the document one run may make. The run is one undo step whatever it makes
/// (its document is set aside whole, [`pdfcraft_engine::EditMark`]); this keeps a run below the
/// history's own depth (100), so a plug-in can't do more in one step than a person could undo
/// one at a time.
pub const MAX_RUN_EDITS: u32 = 99;
/// Messages a run may show (`notify`), and their length in characters.
const MAX_NOTES: usize = 20;
const MAX_NOTE_CHARS: usize = 1000;
/// Least instruction budget `plugin_run {fuel}` may ask for.
const MIN_FUEL: u64 = 1000;
/// Most pages a run is told are selected.
const MAX_SELECTION: usize = 100_000;

/// The only tools a plug-in may call (by tool name, or by the registry command id a tool
/// automates): everything else is refused (`*` in [`policy`]), so a tool added to PdfCraft later
/// is refused until someone decides it is safe here.
///
/// What is on it reads the document the run was started on, or changes it through undoable edits:
/// pages, page labels and boxes, bookmarks, form fields and their values, redaction *marks*,
/// added text, links, measurements, comments, headers, footers, watermarks and backgrounds made
/// from text, printer marks, accessibility tags and text edits. What is not, and why:
/// - files and folders: opening, saving, closing, exporting, combining, splitting, extracting,
///   inserting or replacing pages from a file, importing or exporting data, bookmarks or
///   comments, batches, images placed from a file, and every tool that writes a report
///   (the person chooses what is written where; a plug-in never gets a path);
/// - other documents (`doc_list`, `doc_inspect`, `doc_compare*`, `object_copy` / `object_paste`,
///   `comments_copy` / `comments_paste`, which are also the clipboard) and new ones (`doc_create`);
/// - the undo history (`edit_undo`, `edit_redo`) and nesting (`command_run`, `command_batch`);
/// - applying redactions and sanitizing (`redact_apply`, `doc_remove_hidden`): they remove content
///   for good once saved, so the person applies the marks a plug-in left, after looking at them;
/// - security and signing: passwords, certificates, signatures and Fill & Sign, and the
///   attachments a document carries;
/// - JavaScript (`js_*`, `link_run_javascript`, form actions and scripts): a plug-in is already
///   code, and a document's scripts run only as filling a form runs them;
/// - OCR and its dictionaries, page enhancement and other long work over pixels, rendering to
///   images, printing and printers, standards conversions (`pdfa_convert`, `xfa_to_static`),
///   reducing and optimizing;
/// - the person's preferences and the view (`js_enabled`, `view_reading_colors`, `layer_set`,
///   `comments_hide`, `fill_sign_date_format`, trusted certificates).
pub const ALLOWED: &[&str] = &[
    // reading
    "doc_info",
    "object_list",
    "text_word_count",
    "text_extract",
    "text_find",
    "text_lines",
    "text_paragraphs",
    "text_edit_layout",
    "text_fonts",
    "text_styles",
    "page_images",
    "bookmark_list",
    "form_fields",
    "content_list",
    "link_list",
    "comment_list",
    "measure_info",
    "measure_list",
    "doc_hidden_info",
    "doc_audit_redactions",
    "doc_audit_space",
    "doc_backdrops",
    "accessibility_autotag_preview",
    "accessibility_check",
    "accessibility_find_untagged",
    "accessibility_figures",
    "accessibility_tags",
    "accessibility_tag_content",
    "pdfa_verify",
    "print_readiness",
    // pages
    "page_rotate",
    "page_delete",
    "page_move",
    "page_reverse",
    "page_insert_blank",
    "page_duplicate",
    "page_number",
    "page_transition_set",
    "page_set_box",
    // document properties
    "doc_set_info",
    "doc_initial_view",
    // bookmarks
    "bookmark_add",
    "bookmark_rename",
    "bookmark_delete",
    "bookmark_move",
    "bookmark_set_page",
    "bookmark_set_style",
    "bookmark_from_structure",
    "bookmark_from_text_styles",
    // forms
    "form_fill",
    "form_reset",
    "form_add_field",
    "form_add_field_table",
    "form_set_props",
    "form_tab_order",
    "form_calculation_order",
    "form_delete_field",
    // redaction marks (applying them is the person's)
    "redact_mark",
    "redact_clear",
    // content and text
    "page_add_text",
    "content_update",
    "content_delete",
    "object_move",
    "text_replace",
    "text_prepare",
    "text_edit",
    "text_script",
    // links
    "link_add",
    "link_edit",
    "link_delete",
    "links_from_urls",
    "links_remove",
    // measurements
    "measure_distance",
    "measure_perimeter",
    "measure_area",
    "measure_scale",
    // comments
    "comment_add",
    "comment_reply",
    "comment_set_status",
    "comment_mark",
    "comment_lock",
    "comment_edit",
    "comment_delete",
    "comment_popup",
    "doc_flatten",
    // page marks
    "doc_header_footer",
    "doc_watermark",
    "doc_background",
    "doc_printer_marks",
    "doc_remove_marks",
    "doc_fix_hairlines",
    "doc_convert_colours",
    // accessibility
    "accessibility_autotag_apply",
    "accessibility_autotag_form_fields",
    "accessibility_fix",
    "accessibility_set_alt",
    "accessibility_set_tag_property",
    "accessibility_mark_tag_artifact",
    "accessibility_layout_table_to_blocks",
    "accessibility_move_tag",
];

/// Parameter names that name a file or folder when their value is text (at any depth). Some
/// allowed tools take one optionally (a watermark from a PDF file, a comment's attached file, a
/// replacement image); a plug-in may use those tools without it. `path` holding numbers is a
/// bookmark's place in the outline, not a file, and stays allowed.
pub const FILE_KEYS: &[&str] = &["path", "paths", "file", "files", "folder", "dir", "out", "out_dir", "image", "input", "output"];

/// The rules every plug-in run follows: [`craft_plugin::ALWAYS_DENIED`], and every tool but
/// [`ALLOWED`] (and the registry ids those tools automate).
pub fn policy() -> &'static Policy {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let mut exceptions: Vec<&str> = ALLOWED.to_vec();
        for def in tools::tools() {
            if let Some(id) = def.command
                && tools::tool_for_command(id).is_some_and(|t| ALLOWED.contains(&t))
                && !exceptions.contains(&id)
            {
                exceptions.push(id);
            }
        }
        Policy::new(&["*"], &exceptions)
    })
}

/// The tool `command` names: a tool name, or a registry command id that a tool automates.
pub fn tool_named(command: &str) -> Option<&'static tools::ToolDef> {
    tools::find(command).or_else(|| tools::tool_for_command(command).and_then(tools::find))
}

/// Why plug-in `m` may not call `command` with `params`, before it runs: a refused tool, one
/// outside its manifest's `permissions.commands`, or parameters that name a file.
pub fn refusal(m: &Manifest, command: &str, params: &Value) -> Option<String> {
    if let Err(why) = craft_plugin::check_call(m, policy(), command) {
        return Some(why);
    }
    if file_value_inside(params, 0) {
        return Some(format!(
            "plug-ins can't name files or folders (`{command}` was given one of {}): work on the document the plug-in runs on",
            FILE_KEYS.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    None
}

/// Whether `v` holds text under one of [`FILE_KEYS`] at any depth.
fn file_value_inside(v: &Value, depth: usize) -> bool {
    fn has_text(v: &Value, depth: usize) -> bool {
        depth < 128
            && match v {
                Value::String(_) => true,
                Value::Array(a) => a.iter().any(|v| has_text(v, depth + 1)),
                Value::Object(m) => m.values().any(|v| has_text(v, depth + 1)),
                _ => false,
            }
    }
    // serde_json refuses nesting past 128, so nothing deeper reaches here
    depth < 128
        && match v {
            Value::Object(m) => m.iter().any(|(k, v)| (FILE_KEYS.contains(&k.as_str()) && has_text(v, depth + 1)) || file_value_inside(v, depth + 1)),
            Value::Array(a) => a.iter().any(|v| file_value_inside(v, depth + 1)),
            _ => false,
        }
}

/// Store (`Some`) or forget (`None`) plug-in `id`'s setting `key` in `all`, within
/// [`MAX_SETTINGS_BYTES`] and [`MAX_SETTING_KEY`].
fn set_setting(
    all: &mut std::collections::BTreeMap<String, Map<String, Value>>,
    id: &str,
    key: &str,
    value: Option<&str>,
) -> std::result::Result<(), String> {
    let Some(text) = value else {
        if let Some(mine) = all.get_mut(id) {
            mine.remove(key);
            if mine.is_empty() {
                all.remove(id);
            }
        }
        return Ok(());
    };
    if key.is_empty() || key.chars().count() > MAX_SETTING_KEY || key.chars().any(char::is_control) {
        return Err(format!("a setting's key must be 1–{MAX_SETTING_KEY} characters without control characters"));
    }
    let too_big = || format!("a plug-in's settings are limited to {} KB", MAX_SETTINGS_BYTES / 1024);
    if key.len().saturating_add(text.len()) > MAX_SETTINGS_BYTES {
        return Err(too_big());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("a setting must be JSON: {e}"))?;
    let others: usize = all.get(id).map_or(0, |mine| {
        mine.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len().saturating_add(v.to_string().len())).fold(0, usize::saturating_add)
    });
    if others.saturating_add(key.len()).saturating_add(v.to_string().len()) > MAX_SETTINGS_BYTES {
        return Err(too_big());
    }
    all.entry(id.to_string()).or_default().insert(key.to_string(), v);
    Ok(())
}

// ---------- installing ----------

/// Where a bundle to install comes from (`plugin_install`'s arguments).
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// A `.craftplugin` file on this computer.
    Path(String),
    /// The bundle itself (base64 in the arguments), with a name to remember it by.
    Data { bytes: Vec<u8>, name: String },
    /// A GitHub repository's release: `owner/repo` or `owner/repo@tag`.
    Repo(String),
    /// A catalog's package (its newest release for PdfCraft, or `version`).
    Catalog { url: String, id: String, version: Option<String> },
}

impl Request {
    /// The source `p` names: `path`, `data` (base64), `repo` or `catalog` + `id`.
    pub fn parse(p: &Value) -> std::result::Result<Request, String> {
        let text = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
        let given = ["path", "data", "repo", "catalog"].iter().filter(|k| text(k).is_some()).count();
        if given > 1 {
            return Err("pass one of `path`, `data`, `repo` or `catalog`".into());
        }
        if let Some(path) = text("path") {
            return Ok(Request::Path(path.into()));
        }
        if let Some(data) = text("data") {
            if data.len() > bundle::MAX_BUNDLE_BYTES / 3 * 4 + 4 {
                return Err(format!("the bundle is larger than {} MB", bundle::MAX_BUNDLE_BYTES >> 20));
            }
            let bytes = base64_decode(data).ok_or("`data` isn't base64")?;
            return Ok(Request::Data { bytes, name: text("name").unwrap_or("data").chars().take(200).collect() });
        }
        if let Some(spec) = text("repo") {
            return Ok(Request::Repo(spec.into()));
        }
        if let Some(url) = text("catalog") {
            let id = text("id").ok_or("`id` is required with `catalog` (a package of the catalog)")?;
            return Ok(Request::Catalog { url: url.into(), id: id.into(), version: text("version").map(str::to_string) });
        }
        Err("pass `path`, `data` (base64), `repo` (owner/repo or owner/repo@tag) or `catalog` with `id`".into())
    }
}

/// A bundle obtained for installing: its bytes, where it came from and whether a catalog in use
/// lists it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub bytes: Vec<u8>,
    pub source: Source,
    /// `Some(true)` from a catalog; for a repository, whether a catalog in use lists it; `None`
    /// for a file.
    pub in_catalog: Option<bool>,
}

/// A bundle checked and shown for review (`plugin_install {review: true}`), kept so the install
/// that follows doesn't download it again.
pub(crate) struct Pending {
    pub sha256: String,
    /// The `plugin_install` arguments that named it (without `review` and `sha256`).
    pub request: Value,
    pub candidate: Candidate,
}

/// The fetcher of sessions without network access: local files and `file://` catalogs still read.
pub struct NoNetwork;

impl Fetch for NoNetwork {
    fn get(&mut self, url: &str, _max: usize) -> std::result::Result<Option<Vec<u8>>, String> {
        let shown: String = url.chars().take(120).collect();
        Err(format!("there is no network access here to read {shown}: install the plug-in from a file"))
    }
}

/// A fetcher that reads no local (`file://`) catalogs, for sessions whose paths are confined
/// (`--root`, automation roots).
struct NoLocal<'f>(&'f mut dyn Fetch);

impl Fetch for NoLocal<'_> {
    fn get(&mut self, url: &str, max: usize) -> std::result::Result<Option<Vec<u8>>, String> {
        self.0.get(url, max)
    }

    fn read_local(&mut self, _path: &std::path::Path, _max: usize) -> std::result::Result<Vec<u8>, String> {
        Err("local (file://) catalogs aren't read while file access is confined (--root or automation roots)".into())
    }
}

/// Read the bundle `req` names: a file with `read_file`, a download over `f` (catalogs in use:
/// `catalogs`, to tell whether a repository is listed in one). Needs no session, so a frontend can
/// run it on a worker thread and hand the result to [`Runtime::hold`].
pub fn obtain_with(
    req: &Request,
    f: &mut dyn Fetch,
    catalogs: &[String],
    read_file: &mut dyn FnMut(&str) -> std::result::Result<Vec<u8>, String>,
) -> std::result::Result<Candidate, String> {
    match req {
        Request::Path(path) => {
            let bytes = read_file(path)?;
            if bytes.len() > bundle::MAX_BUNDLE_BYTES {
                return Err(format!("{path} is larger than {} MB", bundle::MAX_BUNDLE_BYTES >> 20));
            }
            Ok(Candidate { bytes, source: Source::File { path: path.clone() }, in_catalog: None })
        }
        Request::Data { bytes, name } => Ok(Candidate { bytes: bytes.clone(), source: Source::File { path: name.clone() }, in_catalog: None }),
        Request::Repo(spec) => {
            let repo = github::parse_repo_spec(spec)?;
            let plan = fetch::resolve_repo(f, &repo)?;
            let bytes = fetch::download_repo(f, &plan)?;
            let source = Source::Github { repo: plan.repo.slug(), tag: plan.repo.tag.clone().unwrap_or_default(), asset: plan.asset.clone() };
            let in_catalog = catalogs.iter().any(|url| {
                fetch::fetch_catalog(f, url)
                    .is_ok_and(|(cat, _)| cat.packages.iter().any(|pkg| pkg.repo_ref().is_some_and(|r| r.same_repo(&repo.owner, &repo.name))))
            });
            Ok(Candidate { bytes, source, in_catalog: Some(in_catalog) })
        }
        Request::Catalog { url, id, version } => {
            let (cat, _) = fetch::fetch_catalog(f, url)?;
            let package = cat.package(id).ok_or_else(|| format!("{url} has no package `{id}`"))?;
            let release = match version {
                Some(v) => package.release(v).ok_or_else(|| format!("{id} has no version {v} in {url}"))?,
                None => package.latest_for(&APP).ok_or_else(|| format!("{id} has no release for PdfCraft {}", APP.version))?,
            };
            release.target(&package.kind, &APP)?;
            let bytes = fetch::download_release(f, url, package, release)?;
            let (_, manifest) = bundle::open(&bytes)?;
            package.check_manifest(release, &manifest)?;
            Ok(Candidate { bytes, source: Source::Catalog { catalog: url.clone(), package: id.clone() }, in_catalog: Some(true) })
        }
    }
}

/// The catalog `url`'s packages PdfCraft can run, with their newest release for it. Needs no
/// session (`installed` is left out), so a frontend can run it on a worker thread.
pub fn catalog_packages_with(f: &mut dyn Fetch, url: &str) -> std::result::Result<(Vec<Value>, Vec<String>), String> {
    let (cat, warnings) = fetch::fetch_catalog(f, url)?;
    let packages = cat
        .packages
        .iter()
        .filter_map(|pkg| {
            let release = pkg.latest_for(&APP)?;
            Some(json!({
                "catalog": url, "id": pkg.id, "name": pkg.name, "version": release.version, "author": pkg.author,
                "description": pkg.description, "license": pkg.license, "repo": pkg.repo, "kind": pkg.kind,
                "sha256": release.sha256,
            }))
        })
        .collect();
    Ok((packages, warnings))
}

/// Where plug-in `p` has a newer release: the `plugin_install` arguments that get it and its
/// version (a tag, for a repository). `None` when it is up to date or came from a file.
pub fn update_for(f: &mut dyn Fetch, p: &InstalledPlugin) -> std::result::Result<Option<(Value, String)>, String> {
    match &p.source {
        Source::Catalog { catalog: url, package: id } => {
            let (cat, _) = fetch::fetch_catalog(f, url)?;
            let release = cat.package(id).and_then(|pkg| pkg.latest_for(&APP));
            Ok(release
                .filter(|r| bundle::newer(&r.version, &p.manifest.version))
                .map(|r| (json!({"catalog": url, "id": id, "version": r.version}), r.version.clone())))
        }
        Source::Github { repo, tag: installed, .. } => {
            let mut repo = github::parse_repo_spec(repo)?;
            repo.tag = None;
            let plan = fetch::resolve_repo(f, &repo)?;
            let tag = plan.repo.tag.clone().unwrap_or_default();
            Ok((tag != *installed).then(|| (json!({"repo": format!("{}@{tag}", plan.repo.slug())}), tag)))
        }
        Source::File { .. } | Source::Other { .. } => Ok(None),
    }
}

/// `plugin_install`'s arguments as a held review and the install after it compare them.
pub fn install_request(p: &Value) -> Value {
    let mut request: Map<String, Value> = p.as_object().cloned().unwrap_or_default();
    request.remove("review");
    request.remove("sha256");
    Value::Object(request)
}

/// Standard base64 (padding optional; whitespace ignored), or `None`.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut ended = false;
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                ended = true;
                continue;
            }
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        if ended {
            return None;
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from(acc >> bits).ok()?);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---------- the tools ----------

/// A plug-in error as a tool reports it.
fn plugin_failed(name: &str, e: impl std::fmt::Display) -> ToolError {
    failed(format!("{name}: {e}"))
}

impl Automation {
    /// Keep plug-ins in `<settings_dir>/Plug-ins` (and their settings beside it), and read the
    /// ones installed there. Without it, plug-ins are installed for this session only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_plugin_store(mut self, settings_dir: &std::path::Path) -> Self {
        self.plugins.open_store(settings_dir);
        self
    }

    /// Download plug-ins (catalogs, GitHub releases) with `fetch`. Without it, only files and
    /// `file://` catalogs install.
    pub fn with_plugin_fetch(mut self, fetch: Box<dyn Fetch + Send>) -> Self {
        self.plugins.fetch = Some(fetch);
        self
    }

    /// Use the plug-in runtime `rt` (installed plug-ins, settings, catalogs), as a frontend that
    /// keeps one across sessions does; [`Automation::take_plugin_runtime`] gives it back.
    pub fn with_plugin_runtime(mut self, rt: Runtime) -> Self {
        self.plugins = rt;
        self
    }

    /// Take the plug-in runtime out, leaving an empty one.
    pub fn take_plugin_runtime(&mut self) -> Runtime {
        std::mem::take(&mut self.plugins)
    }

    /// Whether a path tool's files are confined (`--root` or automation roots).
    fn confined(&self) -> bool {
        self.root.is_some() || self.roots.is_some()
    }

    /// The document a plug-in runs on: `doc`, or the only open one.
    fn plugin_doc(&self, a: &Args) -> Result<DocId> {
        if a.get("doc").is_some_and(|v| !v.is_null()) {
            return Ok(self.doc(a)?.id);
        }
        match self.session.docs() {
            [only] => Ok(only.id),
            [] => Err(failed("no document is open: open one first (doc_open)")),
            _ => Err(ToolError::InvalidArgs("several documents are open: pass `doc`".into())),
        }
    }

    pub(crate) fn plugin_list(&self) -> Value {
        let rt = &self.plugins;
        json!({
            "plugins": rt.list().iter().map(InstalledPlugin::describe).collect::<Vec<_>>(),
            "problems": rt.problems(),
            "catalogs": rt.prefs.catalogs,
            "stored": rt.has_store(),
            "network": rt.fetch.is_some(),
        })
    }

    /// Run `run` with the session's fetcher: the one [`Automation::with_plugin_fetch`] gave, or
    /// one without network access; refusing local (`file://`) catalogs when paths are confined.
    fn plugin_fetching<R>(&mut self, run: impl FnOnce(&Automation, &mut dyn Fetch) -> R) -> R {
        let confined = self.confined();
        let mut fetch = self.plugins.fetch.take();
        let mut none = NoNetwork;
        let inner: &mut dyn Fetch = match fetch.as_mut() {
            Some(f) => f.as_mut(),
            None => &mut none,
        };
        let r = if confined { run(self, &mut NoLocal(inner)) } else { run(self, inner) };
        self.plugins.fetch = fetch;
        r
    }

    /// Get the bundle a `plugin_install` call names (reading a file through the root rules).
    fn plugin_obtain(&mut self, req: &Request) -> Result<Candidate> {
        let catalogs = self.plugins.prefs.catalogs.clone();
        self.plugin_fetching(|a, f| {
            let mut read = |path: &str| -> std::result::Result<Vec<u8>, String> {
                let target = a.resolve(path, false).map_err(|e| e.to_string())?;
                crate::paths::read_input(&target).map_err(|e| e.to_string())
            };
            obtain_with(req, f, &catalogs, &mut read)
        })
        .map_err(failed)
    }

    pub(crate) fn plugin_install(&mut self, a: &Args) -> Result<Value> {
        let request = install_request(a.0);
        let review = a.opt_bool("review")?.unwrap_or(false);
        let sha256 = a.opt_str("sha256")?.map(str::to_ascii_lowercase);
        if review {
            let req = Request::parse(a.0).map_err(ToolError::InvalidArgs)?;
            let candidate = self.plugin_obtain(&req)?;
            let (_, mut v) = self.plugins.review(&candidate).map_err(failed)?;
            let sha = self.plugins.hold(request, candidate);
            v["sha256"] = json!(sha);
            v["review"] = json!(true);
            return Ok(v);
        }
        let candidate = match sha256.as_deref().and_then(|sha| self.plugins.take_held(sha, &request)) {
            Some(c) => c,
            None => {
                let req = Request::parse(a.0).map_err(ToolError::InvalidArgs)?;
                self.plugin_obtain(&req)?
            }
        };
        if let Some(sha) = &sha256 {
            let got = bundle::sha256_hex(&candidate.bytes);
            if !got.eq_ignore_ascii_case(sha) {
                return Err(failed(format!("the bundle is {got}, not the {sha} that was reviewed: review it again")));
            }
        }
        let p = self.plugins.install(candidate).map_err(failed)?;
        Ok(json!({ "installed": p.describe() }))
    }

    pub(crate) fn plugin_update(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("id")?;
        let p = self.plugins.get(id).cloned().ok_or_else(|| failed(format!("no plug-in `{id}` is installed (see plugin_list)")))?;
        let found = self.plugin_fetching(|_, f| update_for(f, &p));
        let Some((mut install, version)) = found.map_err(failed)? else {
            return Ok(json!({ "id": p.manifest.id, "version": p.manifest.version, "update": Value::Null }));
        };
        // Reviewed like any install: the person sees the permissions again when they changed.
        install["review"] = json!(true);
        let review = self.plugin_install(&Args(&install))?;
        if let Some(m) = install.as_object_mut() {
            m.remove("review");
        }
        Ok(json!({ "id": p.manifest.id, "version": p.manifest.version, "update": version, "review": review, "install": install }))
    }

    pub(crate) fn plugin_remove(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("id")?;
        let removed = self.plugins.remove(id).map_err(failed)?;
        Ok(json!({ "id": id, "removed": removed }))
    }

    pub(crate) fn plugin_catalog_list(&mut self, a: &Args) -> Result<Value> {
        let urls: Vec<String> = match a.opt_str("url")? {
            Some(url) => vec![url.to_string()],
            None => self.plugins.prefs.catalogs.clone(),
        };
        let (mut packages, warnings) = self.plugin_fetching(|_, f| {
            let (mut packages, mut warnings) = (Vec::new(), Vec::new());
            for url in &urls {
                match catalog_packages_with(f, url) {
                    Ok((p, w)) => {
                        packages.extend(p);
                        warnings.extend(w.into_iter().map(|w| format!("{url}: {w}")));
                    }
                    Err(e) => warnings.push(format!("{url}: {e}")),
                }
            }
            (packages, warnings)
        });
        for pkg in &mut packages {
            let installed = pkg["id"].as_str().and_then(|id| self.plugins.get(id)).map(|p| p.manifest.version.clone());
            pkg["installed"] = json!(installed);
        }
        Ok(json!({ "catalogs": urls, "packages": packages, "warnings": warnings }))
    }

    pub(crate) fn plugin_catalog_add(&mut self, a: &Args) -> Result<Value> {
        let url = a.str("url")?.trim().to_string();
        catalog::check_url(&url, None).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;
        if self.confined() && url.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("file://")) {
            return Err(failed("local (file://) catalogs aren't read while file access is confined (--root or automation roots)"));
        }
        let rt = &mut self.plugins;
        if !rt.prefs.catalogs.iter().any(|c| c == &url) {
            if rt.prefs.catalogs.len() >= MAX_CATALOGS {
                return Err(failed(format!("at most {MAX_CATALOGS} catalogs can be added")));
            }
            rt.prefs.catalogs.push(url);
            rt.save_prefs();
        }
        Ok(json!({ "catalogs": rt.prefs.catalogs }))
    }

    /// `plugin_run`: run plug-in `id`'s `command` (default: its first) with `params` on document
    /// `doc`, as one undo step named after the plug-in. A run that fails (a trap, an exhausted
    /// budget, a refusal it passes on, a non-zero status) leaves the document, its undo and redo
    /// history and the plug-in's settings as they were.
    pub(crate) fn plugin_run(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("id")?;
        let doc = self.plugin_doc(a)?;
        let p = self.plugins.get(id).cloned().ok_or_else(|| failed(format!("no plug-in `{id}` is installed (see plugin_list)")))?;
        let m = &p.manifest;
        let api = m.target(&APP).map_err(|e| failed(format!("{} can't run here: {e}", m.name)))?.api();
        let command = a.opt_str("command")?;
        let c = m.command(command).ok_or_else(|| failed(format!("{} has no command `{}`", m.name, command.unwrap_or_default())))?;
        let params = c.resolve_params(a.get("params").unwrap_or(&Value::Null)).map_err(ToolError::InvalidArgs)?;
        let mut limits = Limits::default();
        if let Some(fuel) = a.opt_int("fuel")? {
            limits.fuel = u64::try_from(fuel).unwrap_or(0).clamp(MIN_FUEL, limits.fuel);
        }
        let pages = self.session.get(doc).map_or(0, |d| d.info.pages.len());
        let selection: Vec<i64> = match a.opt_ints("selection")? {
            Some(s) if s.len() > MAX_SELECTION => return Err(ToolError::InvalidArgs(format!("at most {MAX_SELECTION} selected pages"))),
            Some(s) => {
                if let Some(bad) = s.iter().find(|&&p| !usize::try_from(p).is_ok_and(|p| (1..=pages).contains(&p))) {
                    return Err(ToolError::InvalidArgs(format!("selection: page {bad} isn't in the document (1–{pages})")));
                }
                s
            }
            None => Vec::new(),
        };
        let mut instance = Instance::new(&p.wasm, &limits).map_err(|e| plugin_failed(&m.name, e))?;
        let input = json!({"api": api, "command": c.id, "params": params, "selection": selection, "doc": doc.0, "pages": pages}).to_string();
        let label = if m.commands.len() > 1 { format!("{}: {}", m.name, c.label.trim_end_matches(['…', '.'])) } else { m.name.clone() };

        let mark = self.session.edit_mark(doc).map_err(|e| plugin_failed(&m.name, e))?;
        let mut rt = std::mem::take(&mut self.plugins);
        let settings = rt.prefs.settings.clone();
        let (ran, notes) = {
            let mut host = ToolHost::new(self, &mut rt, m, doc, &mark, selection);
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| instance.call(&input, &mut host)));
            (ran, host.finish())
        };
        let ran: std::result::Result<RunOutput, String> = match ran {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("the plug-in run stopped unexpectedly".into()),
        };
        let out = match ran {
            Ok(out) => out,
            Err(e) => {
                rt.prefs.settings = settings;
                self.plugins = rt;
                return Err(match self.session.restore_mark(&mark) {
                    Ok(()) => plugin_failed(&m.name, e),
                    Err(u) => plugin_failed(&m.name, format!("{e}; the document could not be put back as it was: {u}")),
                });
            }
        };
        if rt.prefs.settings != settings {
            rt.save_prefs();
        }
        self.plugins = rt;
        let folded = self.session.fold_edits(mark, Label::with(PLUGIN_RUN_LABEL, label)).map_err(|e| plugin_failed(&m.name, e))?;
        let output = serde_json::from_str::<Value>(&out.output).unwrap_or(Value::String(out.output));
        // Panels (`ui_show`) aren't shown in PdfCraft yet: say so rather than drop it silently.
        let panel = matches!(out.ui, Some(UiChange::Show(_))).then_some("PdfCraft doesn't show plug-in panels yet");
        let document = self.session.get(doc).map(summary).unwrap_or(Value::Null);
        Ok(json!({
            "plugin": m.id, "command": c.id, "output": output, "notes": notes, "logs": out.logs, "calls": out.calls,
            "fuel": out.fuel, "panel": panel, "edited": folded, "document": document,
        }))
    }
}

#[cfg(test)]
mod tests;
