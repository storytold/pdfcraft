//! The PDF/A gate in tool results ([`pdfcraft_engine::Session::pdfa_check`]).
//!
//! Every tool call that changes or saves an open document that declares PDF/A says what the
//! document's changes break, the same way whatever the tool: a `pdfa` list with one entry per such
//! document (what it declares, the level it was checked as, every rule its changes break so far),
//! and a sentence in `warnings` for each rule a change breaks that no earlier result named. A save
//! that writes a file breaking rules says so once more, all of them. Documents that declare no
//! PDF/A part get neither, so their results are as before.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_engine::{DocId, PdfaCheck};
use serde_json::{Value, json};

use crate::{Automation, Content};

/// An open document before a tool call: what changed it, and what saved it.
pub(crate) struct Before {
    id: DocId,
    generation: u64,
    saved: Arc<Vec<u8>>,
}

/// What the gate remembers between calls about a document that declares PDF/A.
#[derive(Default)]
pub(crate) struct Reported {
    /// The last check a result named the breaks of, for each document.
    by_doc: HashMap<DocId, PdfaCheck>,
}

impl Automation {
    /// The open documents before a tool call ([`Self::report_pdfa`] compares).
    pub(crate) fn before_call(&self) -> Vec<Before> {
        self.session.docs().iter().map(|d| Before { id: d.id, generation: d.edit_generation(), saved: d.saved_bytes() }).collect()
    }

    /// Adds the gate's findings to `content`, the result of a call that started with `before`:
    /// for every document the call changed, opened or saved that declares PDF/A.
    pub(crate) fn report_pdfa(&mut self, before: &[Before], content: &mut Vec<Content>) {
        let open: Vec<(DocId, u64, Arc<Vec<u8>>, bool)> =
            self.session.docs().iter().map(|d| (d.id, d.edit_generation(), d.saved_bytes(), d.dirty)).collect();
        self.reported.by_doc.retain(|id, _| open.iter().any(|(o, _, _, _)| o == id));
        let (mut entries, mut warnings) = (Vec::new(), Vec::new());
        for (id, generation, saved, dirty) in open {
            let was = before.iter().find(|b| b.id == id);
            if was.is_some_and(|b| b.generation == generation) {
                continue;
            }
            let Some(check) = self.session.pdfa_check(id) else {
                self.reported.by_doc.remove(&id);
                continue;
            };
            let earlier = self.reported.by_doc.get(&id);
            let label = check.claim.label();
            match check.checked_as {
                None if earlier.is_none() => warnings
                    .push(format!("the document declares {label}, which PdfCraft doesn't check, so its changes may break its conformance unnoticed")),
                None => {}
                Some(_) => {
                    let new = earlier.map_or_else(|| check.breaks.clone(), |e| check.breaks_since(e));
                    for b in new {
                        warnings.push(format!("{label}: this change breaks ISO 19005-{} {}: {}", check.claim.part, b.clause, b.message));
                    }
                }
            }
            // A save writes them into the file: said once more, all of them. The file written is
            // the document as it is now unless the call changed it again after saving (a batch
            // that saves and then edits, or undoes): then the file written is checked, with the
            // pages it was written with.
            let saved_now = was.is_some_and(|b| !Arc::ptr_eq(&b.saved, &saved));
            if saved_now {
                let written = if dirty { self.session.pdfa_check_saved(id) } else { Some(check.clone()) };
                if let Some(written) = written.filter(|w| !w.breaks.is_empty()) {
                    warnings.push(saved_breaking(&written, "the saved file"));
                }
            }
            entries.push(entry(id, &check, None));
            self.reported.by_doc.insert(id, check);
        }
        add(content, entries, warnings);
    }

    /// Adds to `out`, the result of writing `bytes` as a copy of document `id` to `path` (an
    /// optimized or reduced copy), what the copy breaks of the PDF/A part it declares.
    pub(crate) fn report_pdfa_copy(&self, id: DocId, bytes: &Arc<Vec<u8>>, path: &str, out: &mut Value) {
        let Some(check) = self.session.pdfa_check_copy(id, bytes) else { return };
        let mut warnings = Vec::new();
        if !check.breaks.is_empty() {
            warnings.push(saved_breaking(&check, "the copy"));
        }
        let mut content = vec![Content::Json(std::mem::take(out))];
        add(&mut content, vec![entry(id, &check, Some(path))], warnings);
        if let Some(Content::Json(v)) = content.into_iter().next() {
            *out = v;
        }
    }
}

/// One `pdfa` entry: the document, what it declares, how it was checked and what breaks.
fn entry(id: DocId, check: &PdfaCheck, copy: Option<&str>) -> Value {
    let breaks: Vec<Value> =
        check.breaks.iter().map(|b| json!({ "clause": b.clause, "message": b.message, "page": b.page.map(|p| p.saturating_add(1)) })).collect();
    let mut e = json!({
        "doc": id.0,
        "declares": check.claim.label(),
        "checked_as": check.checked_as.map(|l| l.label()),
        "breaks": breaks,
    });
    if let Some(path) = copy {
        e["copy"] = json!(path);
    }
    e
}

/// The sentence for a written file (`what`) that breaks rules of the part it declares.
fn saved_breaking(check: &PdfaCheck, what: &str) -> String {
    let mut clauses: Vec<&str> = Vec::new();
    for b in &check.breaks {
        if !clauses.contains(&b.clause) {
            clauses.push(b.clause);
        }
    }
    let n = check.breaks.len();
    format!(
        "{what} declares {} but breaks {n} of its rules that it didn't break before this session's changes (ISO 19005-{} {})",
        check.claim.label(),
        check.claim.part,
        clauses.join(", ")
    )
}

/// Puts `entries` under `pdfa` and `warnings` after any the result has, in its first JSON
/// object (or in one of their own after a result that has none).
fn add(content: &mut Vec<Content>, entries: Vec<Value>, warnings: Vec<String>) {
    if entries.is_empty() {
        return;
    }
    let object = content.iter_mut().find_map(|c| match c {
        Content::Json(v) => v.as_object_mut(),
        Content::Png { .. } => None,
    });
    let mut own = serde_json::Map::new();
    let object = match object {
        Some(o) => o,
        None => &mut own,
    };
    object.insert("pdfa".into(), Value::Array(entries));
    if !warnings.is_empty() {
        let mut all = match object.remove("warnings") {
            Some(Value::Array(old)) => old,
            Some(Value::Null) | None => Vec::new(),
            Some(old) => vec![old],
        };
        all.extend(warnings.into_iter().map(Value::String));
        object.insert("warnings".into(), Value::Array(all));
    }
    if !own.is_empty() {
        content.push(Content::Json(Value::Object(own)));
    }
}
