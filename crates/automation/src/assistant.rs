//! The AI assistant's tools (M13.A5): the personal library, and `ai_ask` with `dry_run`, which
//! returns the exact request a question would send. Nothing is sent: this build has no transport.
//!
//! A library lives in memory for the session, or in a JSON file named by `library` (inside
//! `--root`), read on first use and rewritten atomically after each change. A file that exists
//! but can't be read is refused, never replaced.

use std::path::PathBuf;
use std::sync::Arc;

use pdfcraft_ai::claude::{self, Options, Passage, WebSearch};
use pdfcraft_ai::library::{Added, Entry, Library, Status};
use pdfcraft_ai::model::{self, Effort};
use pdfcraft_ai::source::{Sha256, SourceId};
use pdfcraft_ai::transport::Secret;
use pdfcraft_engine::assistant::{self, Selected};
use serde_json::{Value, json};

use super::error::{Result, ToolError, failed};
use super::json::Args;
use super::paths::{read_input, write_atomic};
use super::{Automation, DocId, file_access};

/// Files and documents one `ai_library_add` takes.
const MAX_ADD: usize = 1_000;

/// The library the tools work on, the file it was read from, and that file's size and
/// modification time then: another process that changes the file is noticed, so its additions are
/// read rather than written over.
#[derive(Default)]
pub(crate) struct Loaded {
    file: Option<PathBuf>,
    library: Library,
    stamp: Option<Stamp>,
}

type Stamp = (u64, Option<std::time::SystemTime>);

/// The size and modification time of `path`, which the caller resolved (the library file).
fn stamp_of(path: &std::path::Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

/// Whether a tool only reads the library or also changes it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Read,
    Change,
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Indexed => "indexed",
        Status::NeedsPassword => "needs_password",
        Status::NotPermitted => "not_permitted",
        Status::NoText => "no_text",
    }
}

fn entry_json(e: &Entry) -> Value {
    json!({
        "sha256": e.doc.sha256.to_hex(),
        "size": e.doc.size,
        "title": e.title,
        "paths": e.paths,
        "pages": e.pages,
        "pages_without_text": e.pages_without_text,
        "passages": e.passages,
        "status": status_name(e.status),
        "added_at": e.added_at,
    })
}

fn added_json(a: Added) -> Value {
    match a {
        Added::New { status, passages } => json!({"outcome": "added", "status": status_name(status), "passages": passages}),
        Added::Known { status, passages } => json!({"outcome": "already_in_library", "status": status_name(status), "passages": passages}),
    }
}

fn source_json(id: &SourceId) -> Value {
    match id {
        SourceId::ThisDocument { page } => json!({"kind": "this_document", "page": u64::from(*page) + 1}),
        SourceId::Library { sha256, page } => json!({"kind": "library", "sha256": sha256.to_hex(), "page": u64::from(*page) + 1}),
    }
}

impl Automation {
    /// The library named by `library`, read on first use and again whenever its file changed.
    /// Without `library`, the last library named, or one kept in memory for the session. Reading
    /// a file needs read access to it, and a tool that changes the library write access too.
    fn library(&mut self, a: &Args, need: Use) -> Result<&mut Library> {
        let file = match a.opt_str("library")? {
            Some(p) => {
                let access = if need == Use::Change { file_access::Access::Write } else { file_access::Access::Read };
                Some(self.resolve_for(p, access, true)?)
            }
            None => self.ai.as_ref().and_then(|l| l.file.clone()),
        };
        // A file first named by a tool that only reads must not be written through another one.
        if need == Use::Change
            && let Some(path) = &file
        {
            self.check_roots(&path.to_string_lossy(), path, file_access::Access::Write)?;
        }
        let fresh = self.ai.as_ref().is_some_and(|l| l.file == file && file.as_deref().is_none_or(|path| l.stamp == stamp_of(path)));
        if !fresh {
            let (library, stamp) = match &file {
                Some(path) if path.exists() => {
                    // A library in the write root alone is not readable through it.
                    self.check_roots(&path.to_string_lossy(), path, file_access::Access::Read)?;
                    let stamp = stamp_of(path);
                    let bytes = read_input(path)?;
                    let (library, _warnings) = Library::from_json(&bytes)
                        .map_err(|e| failed(format!("{}: {e}. Move it away or name another library file; it was not changed.", path.display())))?;
                    (library, stamp)
                }
                _ => (Library::new(), None),
            };
            self.ai = Some(Loaded { file, library, stamp });
        }
        match self.ai.as_mut() {
            Some(l) => Ok(&mut l.library),
            None => Err(failed("the library could not be loaded")),
        }
    }

    /// Write the library back to its file, if it has one. If the file changed since it was read
    /// (another process saved to it), nothing is written: the change is refused rather than
    /// written over theirs. If the write fails the file still holds the library as it was. Either
    /// way it is read again next time rather than trusting the changed copy.
    fn save_library(&mut self) -> Result<()> {
        let Some(Loaded { file: Some(path), library, stamp }) = self.ai.as_ref() else { return Ok(()) };
        let written = if stamp_of(path) == *stamp {
            library.to_json().map_err(failed).and_then(|bytes| write_atomic(path, &bytes))
        } else {
            Err(failed(format!("{}: the library file changed while this change was made, so nothing was written. Try again.", path.display())))
        };
        match (&written, self.ai.as_mut()) {
            (Ok(()), Some(l)) => l.stamp = l.file.as_deref().and_then(stamp_of),
            _ => self.ai = None,
        }
        written
    }

    pub(crate) fn ai_library_add(&mut self, a: &Args) -> Result<Value> {
        // The library file first: one outside the root, or unreadable, refuses the whole call.
        self.library(a, Use::Change)?;
        let paths: Vec<String> = match a.get("paths") {
            Some(_) => a.strs("paths")?.into_iter().map(str::to_owned).collect(),
            None => Vec::new(),
        };
        let docs = a.opt_ints("docs")?.unwrap_or_default();
        if paths.is_empty() && docs.is_empty() {
            return Err(ToolError::InvalidArgs("give paths, docs or both".into()));
        }
        if paths.len().saturating_add(docs.len()) > MAX_ADD {
            return Err(ToolError::InvalidArgs(format!("at most {MAX_ADD} files and documents at once")));
        }
        // Every path is resolved before any is read: one outside the root refuses the call, while
        // a missing file is reported with the others' results.
        let resolved = paths.iter().map(|p| Ok((p.as_str(), self.resolve_for(p, file_access::Access::Read, true)?))).collect::<Result<Vec<_>>>()?;
        let now = self.session.now_secs();
        // One document at a time, each added before the next is read, so only one document's
        // text is held at once. The library is taken out meanwhile (`self.session` reads open
        // documents) and put back before anything can return.
        let Some(mut loaded) = self.ai.take() else { return Err(failed("the library could not be loaded")) };
        let mut results = Vec::new();
        let mut changed = false;
        let mut record = |mut item: Value, read: Result<pdfcraft_ai::library::NewDocument>| {
            let outcome = read.and_then(|new| {
                let hash = new.doc.sha256;
                loaded.library.add(new).map(|added| (hash, added)).map_err(failed)
            });
            match (item.as_object_mut(), outcome) {
                (Some(obj), Ok((hash, added))) => {
                    changed = true;
                    obj.insert("sha256".into(), json!(hash.to_hex()));
                    if let Value::Object(extra) = added_json(added) {
                        obj.extend(extra);
                    }
                }
                (Some(obj), Err(e)) => {
                    obj.insert("error".into(), json!(e.to_string()));
                }
                (None, _) => {}
            }
            results.push(item);
        };
        for (p, path) in resolved {
            let read = read_input(&path).and_then(|bytes| {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                assistant::library_document_from_file(&path.to_string_lossy(), &name, Arc::new(bytes), now).map_err(failed)
            });
            record(json!({"path": p}), read);
        }
        for d in &docs {
            let read = u64::try_from(*d)
                .map_err(|_| ToolError::InvalidArgs("doc ids are positive".into()))
                .and_then(|id| self.session.library_document(DocId(id), now).map_err(failed));
            record(json!({"doc": d}), read);
        }
        self.ai = Some(loaded);
        if changed {
            self.save_library()?;
        }
        Ok(json!({ "results": results }))
    }

    pub(crate) fn ai_library_remove(&mut self, a: &Args) -> Result<Value> {
        self.library(a, Use::Change)?;
        let hash = Sha256::from_hex(a.str("sha256")?).ok_or_else(|| ToolError::InvalidArgs("sha256 must be 64 hex digits".into()))?;
        let removed = self.library(a, Use::Change)?.remove(&hash);
        if removed {
            self.save_library()?;
        }
        Ok(json!({ "removed": removed }))
    }

    pub(crate) fn ai_library_list(&mut self, a: &Args) -> Result<Value> {
        let library = self.library(a, Use::Read)?;
        Ok(json!({
            "documents": library.entries().iter().map(entry_json).collect::<Vec<_>>(),
            "passages": library.passage_count(),
        }))
    }

    pub(crate) fn ai_library_search(&mut self, a: &Args) -> Result<Value> {
        let query = a.str("query")?.to_owned();
        let k = result_count(a, "k", pdfcraft_ai::library::DEFAULT_RESULTS)?;
        let library = self.library(a, Use::Read)?;
        let results: Vec<Value> = library
            .search(&query, k, &[])
            .into_iter()
            .map(|h| {
                let title = library.entry(&h.doc).map(|e| e.title.clone()).unwrap_or_default();
                json!({"sha256": h.doc.to_hex(), "title": title, "page": u64::from(h.page) + 1, "text": h.text, "score": h.score})
            })
            .collect();
        Ok(json!({ "results": results }))
    }

    pub(crate) fn ai_ask(&mut self, a: &Args) -> Result<Value> {
        if a.opt_str("library")?.is_some() {
            self.library(a, Use::Read)?;
        }
        if !a.opt_bool("dry_run")?.unwrap_or(false) {
            return Err(failed(
                "this build sends nothing: outbound network is not enabled. Pass dry_run: true to see the exact request a question would send.",
            ));
        }
        let id = self.doc(a)?.id;
        let page = self.page(a)?;
        let question = a.str("question")?.to_owned();
        let selected = match a.nums::<4>("rect")? {
            Some(r) => Selected::Region(r.map(|v| v as f32)),
            None => Selected::Page,
        };
        let model = match a.opt_str("model")? {
            Some(m) => model::find(m).ok_or_else(|| ToolError::InvalidArgs(format!("unknown model {m:?} (expected one of: {})", model_ids())))?,
            None => model::default_model(),
        };
        let effort = match a.opt_str("effort")? {
            None => Effort::default(),
            Some("low") => Effort::Low,
            Some("medium") => Effort::Medium,
            Some("high") => Effort::High,
            Some("xhigh") => Effort::Xhigh,
            Some("max") => Effort::Max,
            Some(other) => return Err(ToolError::InvalidArgs(format!("unknown effort {other:?} (low, medium, high, xhigh, max)"))),
        };
        let k = result_count(a, "k", pdfcraft_ai::library::DEFAULT_RESULTS)?;
        let ctx = self.session.assistant_context(id, page, &selected).map_err(failed)?;
        let mut passages: Vec<Passage> = assistant::context_passages(&ctx);
        let uses_library = a.opt_str("library")?.is_some() || self.ai.is_some();
        if k > 0 && uses_library {
            self.library(a, Use::Read)?;
            // The open document's own bytes, and any copy of it indexed from the same path before
            // an edit (a redaction mark, say), are left out.
            let library = self.ai.as_ref().map(|l| &l.library).ok_or_else(|| failed("the library could not be loaded"))?;
            let exclude = self.session.library_exclusions(id, library);
            passages.extend(assistant::library_passages(library, &question, Some(&ctx), &exclude, k));
        }
        passages.truncate(claude::MAX_PASSAGES);
        let turn = claude::user_turn(&passages, None, &question).map_err(failed)?;
        let search = WebSearch { max_uses: 3, ..WebSearch::default() };
        let web = a.opt_bool("web_search")?.unwrap_or(false);
        let opts = Options { effort, web_search: web.then_some(&search), ..Options::new(model) };
        // The preview masks the key, so the dry run needs none.
        let request = claude::request(&opts, &Secret::new(""), &[], &turn).map_err(failed)?;
        Ok(json!({
            "sent": false,
            "request": request.preview(),
            "model": model.id,
            "passages": passages.iter().map(|p| json!({"source": source_json(&p.id), "title": p.title})).collect::<Vec<_>>(),
            "redacted_glyphs_left_out": ctx.redacted_glyphs,
            "encrypted": ctx.encrypted,
        }))
    }
}

fn model_ids() -> String {
    model::MODELS.iter().map(|m| m.id).collect::<Vec<_>>().join(", ")
}

fn result_count(a: &Args, key: &str, default: usize) -> Result<usize> {
    match a.opt_int(key)? {
        None => Ok(default),
        Some(n) => usize::try_from(n)
            .ok()
            .filter(|n| *n <= pdfcraft_ai::library::MAX_RESULTS)
            .ok_or_else(|| ToolError::InvalidArgs(format!("{key} must be 0 to {}", pdfcraft_ai::library::MAX_RESULTS))),
    }
}
