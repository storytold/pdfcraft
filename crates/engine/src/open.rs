//! Opening a document: preparing it away from the session, then registering it.

use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_render::{DocInfo, OpenError, RenderConfig, RenderPool, inspect};

use crate::history::{Editor, Keys, TextBlocksCache};
use crate::{
    AppearanceCache, DocId, Document, EnvelopeInfo, Session, TrustStore, XFA_SCRIPTS_OFF, XfaLayout, display_bytes, display_config, guard,
    info_fields, js, note_warnings, recovered_render_password, render_threads, same_pages, signatures_of, xfa, xfa_layout, xfa_values_from_datasets,
};

/// Immutable session state needed while a document is prepared away from its owning thread.
#[derive(Clone)]
pub struct OpenContext {
    now: Option<i64>,
    js_off: bool,
    trust: Arc<TrustStore>,
    xfa_time_scale: u32,
    /// The session's [`FileReadGuard`], which opens a file read besides the one the host
    /// handed over.
    pub(crate) file_read_guard: Option<FileReadGuard>,
}

/// Confinement for a file the engine reads on its own besides the one it was given: today the
/// document beside a detached signature (`.p7s`, #2129). The guard performs the open itself,
/// under the host's rules, and hands back the handle; the engine reads only from that, so the
/// path is never resolved again after the host's check — a sibling (or a folder above it)
/// swapped for a link after the guard returns cannot steer the engine's read to a file the
/// host would refuse. What remains is the window inside the guard, between its check and its
/// open; closing that needs no-follow opens, which the workspace does not carry a crate for.
/// Hosts that hold every path to a root install one (AGENTS.md §3); the desktop UI, where the
/// user picked the file themselves, has none.
pub type FileReadGuard = Arc<dyn Fn(&std::path::Path) -> std::result::Result<std::fs::File, FileReadRefusal> + Send + Sync>;

/// Why a [`FileReadGuard`] handed back no file. `Unreadable` carries no detail on purpose:
/// every cause reads the same to whoever asked for the file, so the engine's caller cannot
/// use it to probe the directory around the file it was given.
#[derive(Debug)]
pub enum FileReadRefusal {
    /// The host's rules refuse this path; the message says why and becomes the open error.
    Refused(String),
    /// The file could not be opened at all: missing, unreadable, not a file.
    Unreadable,
}

/// A fully prepared document that has not yet been registered in a session.
pub struct PreparedDocument {
    document: Document,
}

struct DocumentOpenData<'a> {
    name: String,
    path: Option<String>,
    bytes: Arc<Vec<u8>>,
    info: DocInfo,
    structure: Result<Result<pdfcraft_cos::Document, pdfcraft_cos::CosError>, Box<dyn std::any::Any + Send>>,
    render_password: Option<String>,
    password: Option<&'a str>,
    xfa: Option<XfaLayout>,
}

impl OpenContext {
    /// Prepare a document without mutating the session that supplied this context.
    pub fn prepare_open(
        &self,
        name: impl Into<String>,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let name = name.into();
        // A CAdES envelope (.p7m, issue #1449) opens as the document it wraps, read-only. The
        // DER object is structurally disjoint from any PDF (a PDF starts with `%`, DER with a
        // SEQUENCE), so this one sniff decides which reader sees the bytes.
        if pdfcraft_sign::p7m::is_cms_envelope(&bytes) {
            return guard(|| Session::prepare_envelope(self, name, path, bytes, password))
                .unwrap_or_else(|m| Err(OpenError::Invalid(format!("reading it failed unexpectedly ({m})"))));
        }
        guard(|| Session::prepare_open_unguarded(self, name, path, bytes, password))
            .unwrap_or_else(|m| Err(OpenError::Invalid(format!("reading it failed unexpectedly ({m})"))))
    }
}

impl Session {
    /// Snapshot the session state that affects open-time scripts, dates and signature trust.
    pub fn open_context(&self) -> OpenContext {
        OpenContext { now: self.now(), js_off: self.js_off, trust: self.trust.clone(), xfa_time_scale: self.xfa_time_scale.unwrap_or(1), file_read_guard: self.file_read_guard.clone() }
    }

    /// Prepare a document without adding it to this session.
    pub fn prepare_open(
        &self,
        name: impl Into<String>,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        self.open_context().prepare_open(name, path, bytes, password)
    }

    /// Register a prepared document. This is the only open step that mutates the session.
    pub fn commit_open(&mut self, prepared: PreparedDocument) -> DocId {
        self.next_id += 1;
        let id = DocId(self.next_id);
        let mut document = prepared.document;
        document.id = id;
        self.docs.push(document);
        id
    }

    /// Synchronous convenience API used by the CLI and automation.
    pub fn open(&mut self, name: impl Into<String>, path: Option<String>, bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocId, OpenError> {
        let prepared = self.prepare_open(name, path, bytes, password)?;
        Ok(self.commit_open(prepared))
    }

    /// Prepare the document a CMS envelope signs: read-only, with the signer identified. An
    /// attached envelope (`.p7m`) carries the document inside; a detached one (`.p7s`) opened
    /// from disk has it beside it, and the signature is verified over those bytes. A detached
    /// signature without its document and non-PDF content are actionable errors (never a repair).
    fn prepare_envelope(
        context: &OpenContext,
        name: String,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let mut env = pdfcraft_sign::p7m::read(&bytes).map_err(|e| OpenError::Invalid(e.to_string()))?;
        let mut detached = false;
        let (content, doc_name, reason, envelope_file) = match env.content.take() {
            Some(content) => {
                if !content.starts_with(b"%PDF-") {
                    let kind = if content.starts_with(b"<?xml") || content.starts_with(b"<") { "XML data" } else { "non-PDF data" };
                    return Err(OpenError::Unsupported(format!("the CAdES envelope contains {kind}, not a PDF")));
                }
                (
                    content,
                    strip_p7m(&name),
                    "This document is inside a CAdES signature envelope (.p7m); extracting the original PDF makes an editable copy.".to_string(),
                    Some(bytes.clone()),
                )
            }
            // A detached signature has its document beside it, not inside: opened from disk as
            // a `.p7s`, that document is the like-named file in the same directory, and the
            // signature is verified over its bytes.
            None => {
                let Some((sibling_path, doc_name, sig_name)) = path.as_deref().and_then(detached_sibling) else {
                    return Err(OpenError::Unsupported(DETACHED_ALONE.into()));
                };
                // The sibling is a file of its own, one the host never resolved when it
                // confined the `.p7s`: the host's guard opens it under its rules and hands
                // back the handle, and the engine reads only from that. Nothing resolves
                // the path a second time after the check (#2202 review), so a sibling or a
                // folder above it swapped for a link once the guard has opened cannot move
                // the bytes the engine verifies; the window that remains is inside the
                // guard, between its check and its open. Without a guard — the desktop UI,
                // where the user picked the file themselves — the engine opens the sibling
                // itself. Every way the open or the read can fail — missing, unreadable,
                // not a file — says the same thing, so opening a `.p7s` cannot be used to
                // probe the directory around it.
                let not_found = || OpenError::Unsupported(format!("{sig_name} signs {doc_name}, which was not found beside it"));
                let mut sibling = Vec::new();
                let mut opened = match &context.file_read_guard {
                    Some(guard) => match guard(&sibling_path) {
                        Ok(handle) => handle,
                        Err(FileReadRefusal::Refused(message)) => return Err(OpenError::Unsupported(message)),
                        Err(FileReadRefusal::Unreadable) => return Err(not_found()),
                    },
                    None => std::fs::File::open(&sibling_path).map_err(|_| not_found())?,
                };
                use std::io::Read as _;
                opened.read_to_end(&mut sibling).map_err(|_| not_found())?;
                if !sibling.starts_with(b"%PDF-") {
                    let kind = if sibling.starts_with(b"<?xml") || sibling.starts_with(b"<") { "XML data" } else { "non-PDF data" };
                    return Err(OpenError::Unsupported(format!("the detached signature's document is {kind}, not a PDF")));
                }
                env = pdfcraft_sign::p7m::verify_detached(&bytes, &sibling).map_err(|e| OpenError::Invalid(e.to_string()))?;
                detached = true;
                // The signature's own bytes are what Save writes: saving over the `.p7s` leaves
                // it as it is, and there is no carried content to extract.
                (sibling, doc_name, "This document was opened from a detached CAdES signature (.p7s); open the PDF itself to edit it.".to_string(), Some(bytes))
            }
        };
        let content = Arc::new(content);
        // `path` stays None for the document: the user's file is the envelope or the
        // signature, which is never rewritten.
        let mut prepared = Self::prepare_open_unguarded(context, doc_name, None, content.clone(), password)?;
        // The user's file is not the PDF (it is the envelope, or the signature beside it): the
        // document keeps the opened file's bytes and no editor of its own; only an attached
        // envelope carries the exact signed PDF that extraction hands out.
        prepared.document.editor = None;
        prepared.document.read_only_reason = Some(reason);
        prepared.document.envelope = Some(EnvelopeInfo {
            signer: env.signer_name,
            signing_time: env.signing_time.map(|t| format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", t.year, t.month, t.day, t.hour, t.minute)),
            digest_ok: env.digest_ok,
            signers: env.signers,
            quirks: env.quirks.iter().map(|q| (*q).to_string()).collect(),
            detached,
            source: path,
        });
        prepared.document.envelope_file = envelope_file;
        prepared.document.envelope_content = if detached { None } else { Some(content) };
        prepared.document.dirty = false;
        Ok(prepared)
    }

    fn prepare_open_unguarded(
        context: &OpenContext,
        name: String,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let cos = std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password));
        // The renderer authenticates on its own. It cannot use the owner password of R2–R4
        // files, so give it the user password that owner authentication recovers.
        let (info, render_password) = match inspect(bytes.clone(), password) {
            Ok(info) => (info, password.map(str::to_owned)),
            Err(OpenError::WrongPassword) => {
                let user = match &cos {
                    Ok(Ok(d)) => recovered_render_password(d),
                    _ => None,
                };
                let user = user.ok_or(OpenError::WrongPassword)?;
                (inspect(bytes.clone(), Some(&user))?, Some(user))
            }
            Err(e) => return Err(e),
        };
        // A dynamic XFA form is a shell around an XML template; lay the template out into real
        // pages and fields so the rest of the engine works on it. The original bytes stay: the
        // laid-out form is one appended revision.
        let opts = SaveOptions { mod_date: context.now.map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        // Write `work` as one more revision and reopen it: what the document then is.
        let rebase = |work: &pdfcraft_cos::Document| -> Result<(Arc<Vec<u8>>, DocInfo, pdfcraft_cos::Document), String> {
            let new_bytes = write_incremental(work, &opts).map(Arc::new).map_err(|e| e.to_string())?;
            let new_info = inspect(new_bytes.clone(), render_password.as_deref()).map_err(|e| e.to_string())?;
            let new_cos = pdfcraft_cos::Document::open_with_password(new_bytes.clone(), password).map_err(|e| e.to_string())?;
            Ok((new_bytes, new_info, new_cos))
        };
        let (bytes, info, cos, xfa) = match (info.xfa, cos) {
            (Some(pdfcraft_render::Xfa::Dynamic), Ok(Ok(cos))) if pdfcraft_xfa::existing_layout(&cos).is_none() => {
                let mut work = cos.clone();
                let laid_out = guard(|| xfa_layout(&mut work))
                    .unwrap_or_else(|m| Err(format!("laying it out failed unexpectedly ({m})")))
                    .and_then(|report| rebase(&work).map(|(b, i, c)| (b, i, c, report)));
                match laid_out {
                    Ok((b, i, c, report)) => (b, i, Ok(Ok(c)), Some(report)),
                    Err(e) => {
                        let mut info = info;
                        info.warnings.push(format!("This dynamic XFA form could not be laid out: {e}"));
                        (bytes, info, Ok(Ok(cos)), None)
                    }
                }
            }
            // A static XFA form, or a dynamic one laid out earlier: its datasets may hold values
            // filled in by another viewer since; give the fields those values.
            (Some(_), Ok(Ok(cos))) => {
                let report = pdfcraft_xfa::existing_layout(&cos);
                let mut work = cos.clone();
                let synced =
                    guard(|| xfa_values_from_datasets(&mut work)).unwrap_or_else(|m| Err(format!("reading its data failed unexpectedly ({m})")));
                match synced {
                    Ok(names) if names.is_empty() => (bytes, info, Ok(Ok(cos)), report),
                    Ok(names) => match rebase(&work) {
                        Ok((b, mut i, c)) => {
                            // The fields were rewritten from the XFA data: say so, and which.
                            let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
                            let more = if names.len() > shown.len() { format!(" and {} more", names.len() - shown.len()) } else { String::new() };
                            i.warnings.push(format!(
                                "{} form field value{} were taken from this form's XFA data (filled in by another viewer): {}{more}",
                                names.len(),
                                if names.len() == 1 { "" } else { "s" },
                                shown.join(", ")
                            ));
                            (b, i, Ok(Ok(c)), report)
                        }
                        Err(e) => {
                            let mut info = info;
                            info.warnings.push(format!("The values in this form's XFA data could not be applied: {e}"));
                            (bytes, info, Ok(Ok(cos)), report)
                        }
                    },
                    Err(e) => {
                        let mut info = info;
                        info.warnings.push(format!("The values in this form's XFA data could not be applied: {e}"));
                        (bytes, info, Ok(Ok(cos)), report)
                    }
                }
            }
            (_, cos) => (bytes, info, cos, None),
        };
        // A laid-out XFA form: its initialize and calculate scripts run now, as on opening in
        // Acrobat; what they change is one more revision, still nothing to save.
        let mut script_output = js::JsOutput::default();
        let mut ran_away = false;
        let xfa_template = match (&xfa, &cos) {
            (Some(_), Ok(Ok(c))) => xfa::template(c),
            _ => None,
        };
        let (bytes, info, cos) = match (&xfa, cos) {
            (Some(_), Ok(Ok(cos))) if !context.js_off => match xfa_template.clone() {
                Some(tpl) => {
                    let mut work = cos.clone();
                    let pages = info.pages.len();
                    let ran = guard(|| {
                        let mut run = xfa::XfaRun {
                            page_count: pages,
                            out: &mut script_output,
                            datasets: None,
                            ran_away: false,
                            time_scale: context.xfa_time_scale,
                        };
                        let changes = xfa::on_open(&mut work, &tpl, &mut run);
                        if run.ran_away {
                            ran_away = true;
                        }
                        changes
                    })
                    .unwrap_or_else(|m| Err(format!("its scripts failed unexpectedly ({m})")));
                    match ran {
                        Ok(changes) if work.is_modified() => match rebase(&work) {
                            Ok((b, mut i, c)) => {
                                // The file as saved differs from what opening shows: say so.
                                if !changes.is_empty() {
                                    i.warnings.push(format!(
                                        "This form's scripts changed it on opening ({}); saving keeps those changes",
                                        changes.describe()
                                    ));
                                }
                                (b, i, Ok(Ok(c)))
                            }
                            Err(e) => {
                                script_output.errors.push(format!("The form's scripts could not be applied: {e}"));
                                (bytes, info, Ok(Ok(cos)))
                            }
                        },
                        Ok(_) => (bytes, info, Ok(Ok(cos))),
                        Err(e) => {
                            script_output.errors.push(format!("The form's scripts could not run: {e}"));
                            (bytes, info, Ok(Ok(cos)))
                        }
                    }
                }
                None => (bytes, info, Ok(Ok(cos))),
            },
            (_, cos) => (bytes, info, cos),
        };
        // The catalog leads to no page, but the reader found pages by scanning: gather the page
        // objects into a page tree as one more revision, or page edits address none of them
        // (#1656). Kept only when the reader then shows the same pages, and never on a signed
        // document, where writing the repair could rewrite the file under its signatures.
        let (bytes, info, cos) = match cos {
            Ok(Ok(cos)) if !info.pages.is_empty() && pdfcraft_organize::page_count(&cos).unwrap_or(0) == 0 => {
                let mut work = cos.clone();
                let signed = || pdfcraft_sign::pdf::list(&cos, &bytes, &context.trust).iter().any(|s| s.signed);
                let rebuilt = match pdfcraft_organize::recover_page_tree(&mut work) {
                    Ok(n) if n > 0 && !signed() => rebase(&work).ok().filter(|(_, i, _)| same_pages(&i.pages, &info.pages)),
                    _ => None,
                };
                match rebuilt {
                    Some((b, i, mut c)) => {
                        for line in cos.repair_log() {
                            c.note_repair(line.clone());
                        }
                        c.note_repair(format!("the page tree is missing; its {} pages were found by scanning the file", i.pages.len()));
                        (b, i, Ok(Ok(c)))
                    }
                    None => (bytes, info, Ok(Ok(cos))),
                }
            }
            cos => (bytes, info, cos),
        };
        // XFA notes survive the document being re-read after edits.
        let xfa_warnings: Vec<String> = if info.xfa.is_some() { info.warnings.clone() } else { Vec::new() };
        let mut document =
            Self::build_document(&context.trust, DocumentOpenData { name, path, bytes, info, structure: cos, render_password, password, xfa });
        // A script that ran away on open turns the form's scripts off for this document.
        document.xfa_template = if ran_away { None } else { xfa_template };
        document.js_output.append(script_output);
        if ran_away {
            document.xfa_scripts_off = true;
            document.js_output.errors.push(XFA_SCRIPTS_OFF.into());
        }
        note_warnings(&mut document.xfa_warnings, &xfa_warnings);
        Ok(PreparedDocument { document })
    }

    /// The last step of opening: build the document record and register it.
    fn build_document(trust: &Arc<TrustStore>, data: DocumentOpenData<'_>) -> Document {
        let DocumentOpenData { name, path, bytes, mut info, structure: cos, render_password, password, xfa } = data;
        let config = RenderConfig { password: render_password.as_deref().map(Arc::from), ..Default::default() };
        let (mut editor, read_only_reason) = match cos {
            Ok(Ok(cos)) => {
                let keys = Keys { render: render_password.clone(), reopen: password.map(str::to_owned) };
                let saved = cos.bytes().clone();
                (
                    Some(Editor {
                        cos,
                        saved,
                        saved_keys: keys.clone(),
                        unsaved_file: false,
                        undo: Vec::new(),
                        redo: Vec::new(),
                        keys,
                        appearances: AppearanceCache::default(),
                        field_appearances: pdfcraft_forms::FieldAppearanceCache::default(),
                        text_blocks: TextBlocksCache::default(),
                    }),
                    None,
                )
            }
            Ok(Err(e)) => (None, Some(e.to_string())),
            Err(_) => (None, Some("the document structure could not be read for editing".into())),
        };
        let sig_cache = Arc::new(pdfcraft_sign::DigestCache::default());
        let signatures = editor.as_ref().map(|e| signatures_of(&e.cos, &bytes, trust, &sig_cache)).unwrap_or_default();
        // The inspection reads only `/Info`; a file stating its title or author only in XMP still
        // shows them (#2373).
        if let Some(e) = &editor {
            for (field, value) in info_fields(&mut info, &e.cos) {
                if field.is_none() {
                    *field = value;
                }
            }
        }
        let display = display_bytes(editor.as_mut(), &bytes);
        let renderer = RenderPool::new(display.clone(), render_threads(), display_config(&config));
        let (form, adopted) = editor
            .as_ref()
            .map(|e| {
                let mut fields = pdfcraft_forms::fields_with_adopted(&e.cos);
                xfa::mark_script_buttons(&e.cos, &mut fields.items);
                (fields.items, fields.adopted_page_fields)
            })
            .unwrap_or_default();
        // Fields the file lists only as page widgets are adopted leniently; the file says so.
        if adopted > 0
            && let Some(e) = &mut editor
        {
            let line = if adopted == 1 {
                "the form's /Fields list omits 1 field; it was read from the page annotations".to_string()
            } else {
                format!("the form's /Fields list omits {adopted} fields; they were read from the page annotations")
            };
            e.cos.note_repair(line);
        }
        Document {
            id: DocId(0),
            name,
            path,
            bytes,
            display,
            info,
            renderer,
            password: render_password,
            reopen_password: password.map(str::to_owned),
            dirty: false,
            generation: 0,
            snapshot_generation: 0,
            read_only_reason,
            envelope: None,
            envelope_file: None,
            envelope_content: None,
            form: Arc::new(form),
            marker_scan: OnceLock::new(),
            links: OnceLock::new(),
            signatures: Arc::new(signatures),
            trust: trust.clone(),
            sig_cache,
            editor,
            config,
            js_output: Default::default(),
            xfa,
            xfa_template: None,
            xfa_scripts_off: false,
            fonts_stale: false,
            xfa_warnings: Vec::new(),
            edit_notices: Vec::new(),
        }
    }
}

/// A display name without one trailing `.p7m` (any case): what the PDF inside the envelope is
/// called. Anything else is kept as it is.
fn strip_p7m(name: &str) -> String {
    // The matched suffix is ASCII, so cutting it off stays on a char boundary.
    if name.len() >= 4 && name.as_bytes()[name.len() - 4..].eq_ignore_ascii_case(b".p7m") {
        name[..name.len() - 4].to_string()
    } else {
        name.to_string()
    }
}

/// The document a detached signature (a `.p7s`) signs: the file name without one trailing
/// `.p7s` (any case). `None` when the name does not end in `.p7s`, so no sibling is looked
/// for at all — a detached signature is only a pair under that name.
fn strip_p7s(name: &str) -> Option<String> {
    // The matched suffix is ASCII, so cutting it off stays on a char boundary.
    if name.len() >= 4 && name.as_bytes()[name.len() - 4..].eq_ignore_ascii_case(b".p7s") {
        // A file literally called ".p7s" signs no document.
        let stripped = &name[..name.len() - 4];
        (!stripped.is_empty()).then(|| stripped.to_string())
    } else {
        None
    }
}

/// The plain error for a detached signature whose document is not at hand (issue #2129):
/// nothing is carried inside the file, and without a path there is nowhere to look for it.
const DETACHED_ALONE: &str = "this is a detached CAdES signature; the signed document is not inside the file";

/// Where the document of a detached signature lives (issue #2129): beside the `.p7s` it was
/// opened from, under the same name without the suffix. `None` when `path` does not name a
/// `.p7s` file, so no sibling is looked for at all. Gives the sibling's path, its file name
/// (the document's display name) and the signature's file name (for the error messages).
fn detached_sibling(path: &str) -> Option<(std::path::PathBuf, String, String)> {
    let path = std::path::Path::new(path);
    let file_name = path.file_name()?.to_str()?;
    let doc_name = strip_p7s(file_name)?;
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    Some((dir.join(&doc_name), doc_name, file_name.to_string()))
}
