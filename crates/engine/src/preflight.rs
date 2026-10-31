//! Preflight (Print production ▸ Preflight): run a profile over an open document, with the
//! Accessibility Checker as the check the preflight crate can't run itself, save the report as a
//! PDF, and apply fixups as undoable edits ([`crate::Edit::PreflightFix`]).

use std::sync::Arc;

pub use pdfcraft_preflight::profiles::*;

use crate::{EditError, Session};

/// What the engine adds to preflight: the Accessibility Checker, which sits above the preflight
/// crate. Its failed findings become hits, worded as the checker words them.
struct Engine;

impl Host for Engine {
    fn accessibility(&self, doc: &pdfcraft_cos::Document) -> Option<Vec<Hit>> {
        let report = pdfcraft_a11y::check(doc, &pdfcraft_a11y::Options::default());
        let hits = report
            .results
            .iter()
            .filter(|r| r.status == pdfcraft_a11y::Status::Failed)
            .flat_map(|r| {
                r.findings.iter().map(move |f| {
                    let mut hit = Hit::worded(f.pattern, f.values.clone())
                        .on(f.page)
                        .object(f.object.map(|o| o.num))
                        .bbox(f.bbox)
                        .detail("Rule", r.rule.id().to_owned());
                    hit.rule = Some(r.rule.id());
                    hit
                })
            })
            .collect();
        Some(hits)
    }
}

impl crate::Document {
    /// Preflight: run `profile` over the document. Nothing is changed. `None` when the document
    /// has no object graph to check (no results would claim it passed).
    pub fn preflight(&self, profile: &Profile) -> Option<Results> {
        self.editor.as_ref().map(|e| run(&e.cos, profile, &Engine))
    }

    /// What kind of object number `n` is, for a hit's details: its `/Type`, `/Subtype` and `/S`
    /// names as PDF writes them (`/Annot /Link`). `None` for an object that isn't a dictionary.
    pub fn object_names(&self, n: u32) -> Option<String> {
        let cos = &self.editor.as_ref()?.cos;
        let o = cos.get(pdfcraft_cos::ObjRef::new(n, cos.generation(n)));
        let d = match &*o {
            pdfcraft_cos::Object::Dict(d) => d,
            pdfcraft_cos::Object::Stream(s) => &s.dict,
            _ => return None,
        };
        let names: Vec<String> =
            [b"Type".as_slice(), b"Subtype", b"S"].iter().filter_map(|k| d.name(k)).map(|n| format!("/{}", String::from_utf8_lossy(n))).collect();
        Some(names.join(" "))
    }
}

impl Session {
    /// The preflight report as a PDF, written by PdfCraft's own text-to-PDF and said in the
    /// language `words` translates into.
    pub fn preflight_report_pdf(
        &self,
        results: &Results,
        document: &str,
        date: &str,
        words: pdfcraft_preflight::Words,
    ) -> Result<Arc<Vec<u8>>, EditError> {
        self.create_report(&words("Preflight report"), &text(results, document, date, words))
    }
}
