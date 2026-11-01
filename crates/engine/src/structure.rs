//! Validate structure: the document's object graph checked against the Arlington PDF Model
//! (`pdfcraft-arlington`), the way Preflight's syntax checks look at a file's structure.
//!
//! The validator never parses a file; this adapts the editor's object layer to it, so what is
//! checked is the document as it is now, unsaved edits included.

use std::collections::HashMap;

use pdfcraft_arlington::{Object, Ref, Source, Version};
use pdfcraft_cos::XrefEntry;

pub use pdfcraft_arlington::{Finding, MODEL_COMMIT, Options, Report, Rule, Severity, WORDINGS, Words, in_english};

/// Nesting followed when handing a direct object over; the object layer already caps what it
/// parses, so this only guards objects built in memory.
const MAX_DEPTH: usize = 256;

impl crate::Document {
    /// Validate structure against the Arlington PDF Model. `None` when the document has no
    /// object graph to check (it couldn't be read for editing).
    pub fn validate_structure(&self, options: &Options) -> Option<Report> {
        let cos = &self.editor.as_ref()?.cos;
        let pages: HashMap<u32, usize> = pdfcraft_model::pages(cos).iter().enumerate().map(|(i, p)| (p.obj.num, i)).collect();
        let source = CosSource { doc: cos, page_count: pages.len(), pages };
        Some(pdfcraft_arlington::validate(&source, options))
    }
}

struct CosSource<'a> {
    doc: &'a pdfcraft_cos::Document,
    pages: HashMap<u32, usize>,
    page_count: usize,
}

impl Source for CosSource<'_> {
    fn trailer(&self) -> Object {
        Object::Dict(entries(self.doc.trailer(), 0))
    }

    fn get(&self, r: Ref) -> Option<Object> {
        // An object number the cross-reference table doesn't hold (or marks free) is no object,
        // unless an unsaved edit made it; nor is one of another generation than the reference's
        // (ISO 32000-2 §7.3.10: such a reference is to the null object).
        let exists = self.doc.is_edited(r.num) || matches!(self.doc.xref_entry(r.num), Some(XrefEntry::InFile { .. } | XrefEntry::InStream { .. }));
        if !exists || self.doc.generation(r.num) != r.generation {
            return None;
        }
        self.doc.try_get(r.num).ok().map(|o| convert(&o, 0))
    }

    fn header_version(&self) -> Option<Version> {
        Version::parse(self.doc.version())
    }

    fn file_size(&self) -> Option<u64> {
        u64::try_from(self.doc.bytes().len()).ok()
    }

    fn page_index(&self, r: Ref) -> Option<usize> {
        self.pages.get(&r.num).copied()
    }

    fn page_count(&self) -> Option<usize> {
        Some(self.page_count)
    }
}

fn entries(d: &pdfcraft_cos::Dict, depth: usize) -> Vec<(Vec<u8>, Object)> {
    d.iter().map(|(k, v)| (k.clone(), convert(v, depth + 1))).collect()
}

fn convert(o: &pdfcraft_cos::Object, depth: usize) -> Object {
    use pdfcraft_cos::Object as C;
    if depth > MAX_DEPTH {
        return Object::Null;
    }
    match o {
        C::Null => Object::Null,
        C::Bool(b) => Object::Bool(*b),
        C::Int(i) => Object::Int(*i),
        C::Real(r) => Object::Real(*r),
        C::String(s) => Object::String { bytes: s.bytes.clone(), hex: s.hex },
        C::Name(n) => Object::Name(n.clone()),
        C::Array(a) => Object::Array(a.iter().map(|v| convert(v, depth + 1)).collect()),
        C::Dict(d) => Object::Dict(entries(d, depth)),
        C::Stream(s) => Object::Stream(entries(&s.dict, depth)),
        C::Ref(r) => Object::Ref(Ref { num: r.num, generation: r.generation }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::Session;

    /// A file as written, objects numbered from 1, with a classic cross-reference table.
    fn pdf(version: &str, objects: &[&str], trailer: &str) -> Vec<u8> {
        let mut out = format!("%PDF-{version}\n").into_bytes();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n{trailer}\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    fn validate(bytes: Vec<u8>) -> Report {
        let mut s = Session::new();
        let id = s.open("t.pdf", None, Arc::new(bytes), None).expect("opens");
        s.get(id).and_then(|d| d.validate_structure(&Options::default())).expect("has an object graph")
    }

    #[test]
    fn the_engine_fixture_is_valid() {
        let report = validate(crate::tests::fixture(3));
        let problems: Vec<String> = report.findings.iter().filter(|f| f.severity != Severity::Info).map(Finding::message).collect();
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(report.version.to_string(), "1.7");
        assert!(report.objects > 10, "{}", report.objects);
    }

    /// Findings point at their object and page, and an annotation's at its rectangle.
    #[test]
    fn findings_locate_their_object_page_and_annotation() {
        let bytes = pdf(
            "1.7",
            &[
                "<< /Type /Catalog /Pages 2 0 R /PageMode /Sideways >>",
                "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Annots [5 0 R] >>",
                "<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] /Border [0 0 1] /H /Glow >>",
            ],
            "<< /Size 6 /Root 1 0 R >>",
        );
        let report = validate(bytes);
        let said: Vec<String> = report.findings.iter().map(|f| format!("{} {}", f.path, f.message())).collect();
        let mode = report.findings.iter().find(|f| f.key == "PageMode").unwrap_or_else(|| panic!("PageMode has fixed values: {said:?}"));
        assert_eq!((mode.rule, mode.object.map(|r| r.num), mode.page), (Rule::Value, Some(1), None));
        let h = report.findings.iter().find(|f| f.key == "H").unwrap_or_else(|| panic!("H is N, I, O or P: {said:?}"));
        assert_eq!((h.object.map(|r| r.num), h.page), (Some(5), Some(1)));
        assert_eq!(h.rect, Some([10.0, 10.0, 50.0, 30.0]));
        assert_eq!(report.errors, 2, "{said:?}");
    }

    /// What is checked is the working state: an unsaved edit that breaks the structure shows.
    #[test]
    fn unsaved_edits_are_validated() {
        let mut s = Session::new();
        let id = s.open("t.pdf", None, Arc::new(crate::tests::fixture(1)), None).expect("opens");
        let check = |s: &Session| s.get(id).and_then(|d| d.validate_structure(&Options::default())).expect("checks");
        let before = check(&s);
        assert!(!before.findings.iter().any(|f| f.path.starts_with("trailer/Info")));
        s.apply(id, crate::Edit::SetInfo { key: "Title".into(), value: "Draft".into() }).expect("edits");
        let after = check(&s);
        assert!(after.objects > before.objects, "the new document information is walked: {} then {}", before.objects, after.objects);
        assert!(after.is_valid(), "{:?}", after.findings.iter().map(Finding::message).collect::<Vec<_>>());
    }

    #[test]
    fn a_dangling_root_is_reported_not_fatal() {
        let bytes = pdf(
            "1.4",
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] >>",
            ],
            "<< /Size 4 /Root 1 0 R /Info 9 0 R >>",
        );
        let report = validate(bytes);
        assert!(report.findings.iter().any(|f| f.rule == Rule::MissingObject && f.key == "Info"));
        assert!(report.findings.iter().any(|f| f.rule == Rule::Required && f.key == "Resources" && f.page == Some(0)));
    }

    /// `4 1 R` when object 4 is of generation 0 names no object, whatever object 4 holds.
    #[test]
    fn a_reference_of_another_generation_names_no_object() {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources << >> >>",
            "<< /Title (Report) >>",
        ];
        let info = |r: &str| validate(pdf("1.4", &objects, &format!("<< /Size 5 /Root 1 0 R /Info {r} >>")));
        let missing = |report: &Report| report.findings.iter().any(|f| f.rule == Rule::MissingObject && f.key == "Info");
        assert!(!missing(&info("4 0 R")));
        assert!(missing(&info("4 1 R")));
    }
}
