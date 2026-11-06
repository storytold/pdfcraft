//! Changes made after signing are judged against what a reader resolves and by the role each
//! object held when signed: a newer generation, a repaired generation alias, or a widget
//! replaced, removed or moved never passes as a permitted change.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object};

use super::Modification;
use super::changes::classify_changes;

/// One cross-reference row: an object written with header generation `.0` under row generation `.1`.
enum Row<'a> {
    Obj(u16, u16, &'a str),
}

/// A classic-xref file built one revision at a time; each later revision points back with /Prev.
#[derive(Clone)]
struct File {
    bytes: Vec<u8>,
    size: u32,
    xref: Option<usize>,
}

impl File {
    fn new() -> Self {
        File { bytes: b"%PDF-1.7\n".to_vec(), size: 1, xref: None }
    }

    fn revision(mut self, rows: &[(u32, Row<'_>)]) -> Self {
        let mut table = Vec::new();
        for (num, row) in rows {
            self.size = self.size.max(num + 1);
            let Row::Obj(header, generation, body) = row;
            table.push(format!("{num} 1\n{:010} {generation:05} n \n", self.bytes.len()));
            self.bytes.extend_from_slice(format!("{num} {header} obj\n{body}\nendobj\n").as_bytes());
        }
        let at = self.bytes.len();
        self.bytes.extend_from_slice(b"xref\n");
        if self.xref.is_none() {
            self.bytes.extend_from_slice(b"0 1\n0000000000 65535 f \n");
        }
        self.bytes.extend_from_slice(table.concat().as_bytes());
        let prev = self.xref.map(|p| format!(" /Prev {p}")).unwrap_or_default();
        self.bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{prev} >>\nstartxref\n{at}\n%%EOF\n", self.size).as_bytes());
        self.xref = Some(at);
        self
    }

    fn open(&self) -> Document {
        Document::open(Arc::new(self.bytes.clone())).unwrap()
    }
}

const PAGES: &str = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
const TEXT_FIELD: &str = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ann) /Rect [20 20 120 40] /P 3 0 R >>";
const COMMENT: &str = "<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (note) >>";

fn page(extra: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] {extra} >>")
}

fn r(num: u32, generation: u16) -> ObjRef {
    ObjRef { num, generation }
}

fn assert_refused(doc: &Document, old: Document, p: u8) {
    let verdict = classify_changes(doc, Some(old), Some(p));
    assert!(matches!(verdict, Modification::Disallowed(_)), "P={p}: main returns {verdict:?}");
}

/// Two pages and a text field on page 3, listed in /Fields and in the page's inline /Annots.
fn form(page3: &str) -> File {
    File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page(page3))),
        (4, Row::Obj(0, 0, &page("/Annots []"))),
        (5, Row::Obj(0, 0, TEXT_FIELD)),
        (6, Row::Obj(0, 0, "[5 0 R]")),
    ])
}

#[test]
fn content_stream_intermediate_generation_cover() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page(""))),
        (4, Row::Obj(0, 0, &page("/Contents 5 0 R"))),
        (5, Row::Obj(0, 0, "<< /Length 3 >>\nstream\nq Q\nendstream")),
    ]);
    let now = signed
        .clone()
        .revision(&[(5, Row::Obj(0, 0, "<< /Length 3 >>\nstream\nQ q\nendstream"))])
        .revision(&[(5, Row::Obj(1, 1, "<< /Length 3 >>\nstream\nq Q\nendstream"))]);
    let (doc, old) = (now.open(), signed.open());
    assert_eq!(doc.revisions().len(), 3);
    assert!(doc.xref_oddities().is_empty());
    assert_eq!(doc.get(r(5, 1)), old.get(r(5, 0)), "the newest generation holds the signed bytes");
    assert_refused(&doc, old, 2);
}

#[test]
fn widget_removed_from_inline_annots() {
    let signed = form("/Annots [5 0 R]");
    let mut doc = signed.open();
    doc.update_dict(r(3, 0), |p| p.set(b"Annots".to_vec(), Object::Array(Vec::new()))).unwrap();
    assert_refused(&doc, signed.open(), 3);
}

#[test]
fn widget_moved_between_pages() {
    let signed = form("/Annots [5 0 R]");
    let mut doc = signed.open();
    doc.update_dict(r(3, 0), |p| p.set(b"Annots".to_vec(), Object::Array(Vec::new()))).unwrap();
    doc.update_dict(r(4, 0), |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(r(5, 0))]))).unwrap();
    doc.update_dict(r(5, 0), |w| w.set(b"P".to_vec(), Object::Ref(r(4, 0)))).unwrap();
    assert_refused(&doc, signed.open(), 3);
}

#[test]
fn annots_moved_to_new_object_dropping_widget() {
    let signed = form("/Annots 6 0 R");
    let mut doc = signed.open();
    let empty = doc.add(Object::Array(Vec::new()));
    doc.update_dict(r(3, 0), |p| p.set(b"Annots".to_vec(), Object::Ref(empty))).unwrap();
    assert_refused(&doc, signed.open(), 3);
}

#[test]
fn repaired_page_relabelled_as_field() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R >>")),
        (2, Row::Obj(0, 0, "<< /Type /Pages /Kids [3 5 R 4 0 R] /Count 2 >>")),
        (3, Row::Obj(5, 0, &page(""))),
        (4, Row::Obj(0, 0, &page(""))),
    ]);
    let mut doc = signed.open();
    doc.update_dict(r(3, 0), |p| {
        p.remove(b"Type");
        p.set(b"FT".to_vec(), Object::name("Tx"));
        p.set(b"Rotate".to_vec(), Object::Int(180));
    })
    .unwrap();
    assert_refused(&doc, signed.open(), 2);
}

#[test]
fn resource_container_and_drawn_alias() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R >>")),
        (2, Row::Obj(0, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")),
        (3, Row::Obj(0, 0, &page("/Contents 6 0 R /Resources << /Font 4 0 R /ExtGState << /GS1 4 5 R >> >>"))),
        (4, Row::Obj(5, 0, "<< /F1 5 0 R /CA 1 >>")),
        (5, Row::Obj(0, 0, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")),
        (6, Row::Obj(0, 0, "<< /Length 3 >>\nstream\nq Q\nendstream")),
    ]);
    let mut doc = signed.open();
    doc.update_dict(r(4, 0), |d| d.set(b"FT".to_vec(), Object::name("Tx"))).unwrap();
    assert_refused(&doc, signed.open(), 2);
}

/// A field signed as `4 5 R` / `4 0 R` (header 5, row 0) that a later revision rewrites.
fn aliased_field(fields: &str, annots: &str) -> File {
    File::new().revision(&[
        (1, Row::Obj(0, 0, &format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{fields}] >> >>"))),
        (2, Row::Obj(0, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")),
        (3, Row::Obj(0, 0, &page(&format!("/Annots [{annots}]")))),
        (4, Row::Obj(5, 0, TEXT_FIELD)),
    ])
}

fn set_annots(doc: &mut Document, annot: ObjRef) {
    doc.update_dict(r(3, 0), |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annot)]))).unwrap();
}

#[test]
fn repaired_alias_set_changes_widget_to_comment() {
    let signed = aliased_field("4 0 R", "4 0 R");
    let now = signed.clone().revision(&[(4, Row::Obj(7, 0, COMMENT))]);
    let mut doc = now.open();
    set_annots(&mut doc, r(4, 7));
    assert_refused(&doc, signed.open(), 3);
}

#[test]
fn repaired_slot_becomes_disjoint_generation() {
    let signed = aliased_field("4 0 R", "4 0 R");
    let now = signed.clone().revision(&[(4, Row::Obj(7, 7, COMMENT))]);
    let mut doc = now.open();
    set_annots(&mut doc, r(4, 7));
    assert_refused(&doc, signed.open(), 3);
}

#[test]
fn hidden_shared_alias_form_fill() {
    let signed = aliased_field("4 5 R", "4 0 R");
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let now = signed.clone().revision(&[(4, Row::Obj(5, 1, &filled))]);
    let mut doc = now.open();
    set_annots(&mut doc, r(4, 1));
    // At P=3 a relisted annotation is permitted whatever it is: only the generations refuse it.
    for p in [2, 3] {
        assert_refused(&doc, signed.open(), p);
    }
}

fn verdict(doc: &Document, old: Document, p: Option<u8>) -> Modification {
    classify_changes(doc, Some(old), p)
}

/// A widget whose /Annots entry is rewritten to a generation its slot never had no longer
/// resolves: it is taken off the page, inline or through an indirect array.
#[test]
fn widget_dropped_through_a_dead_generation() {
    for (page3, num) in [("/Annots 6 0 R", 6), ("/Annots [5 0 R]", 3)] {
        let signed = form(page3);
        let body = if num == 6 { "[5 3 R]".to_owned() } else { page("/Annots [5 3 R]") };
        let now = signed.clone().revision(&[(num, Row::Obj(0, 0, &body))]);
        for p in [Some(2), Some(3), None] {
            let v = verdict(&now.open(), signed.open(), p);
            assert!(matches!(v, Modification::Disallowed(_)), "{page3} P={p:?}: {v:?}");
        }
    }
}

/// The same annotations, listed in a new array object, change nothing on the page; listed in
/// another order they change which is drawn on top.
#[test]
fn an_unchanged_annotation_list_is_no_change_and_a_reordered_one_is_a_comment_change() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R >>")),
        (2, Row::Obj(0, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")),
        (3, Row::Obj(0, 0, &page("/Annots [4 0 R 5 0 R]"))),
        (4, Row::Obj(0, 0, COMMENT)),
        (5, Row::Obj(0, 0, COMMENT)),
    ]);
    let moved = signed.clone().revision(&[(3, Row::Obj(0, 0, &page("/Annots 6 0 R"))), (6, Row::Obj(0, 0, "[4 0 R 5 0 R]"))]);
    assert_eq!(verdict(&moved.open(), signed.open(), Some(2)), Modification::Allowed(Vec::new()));
    let reordered = signed.clone().revision(&[(3, Row::Obj(0, 0, &page("/Annots [5 0 R 4 0 R]")))]);
    assert_eq!(verdict(&reordered.open(), signed.open(), Some(2)), Modification::Disallowed(vec![super::kind::COMMENTS]));
}

/// Many signed pages sharing one long /Annots array, all rewritten to list it through a new
/// object, are judged in bounded time.
#[test]
fn shared_annotation_arrays_are_judged_in_bounded_time() {
    const PAGES: u32 = 30_000;
    let first_page = 10;
    let kids: String = (0..PAGES).map(|i| format!("{} 0 R ", first_page + i)).collect();
    let annots: String = (0..PAGES).map(|i| format!("{} 0 R ", first_page + i)).collect();
    let mut rows = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
        (2, format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>")),
        (3, format!("[{annots}]")),
    ];
    rows.extend((0..PAGES).map(|i| (first_page + i, page("/Annots 3 0 R"))));
    let rows: Vec<(u32, Row<'_>)> = rows.iter().map(|(n, b)| (*n, Row::Obj(0, 0, b))).collect();
    let signed = File::new().revision(&rows);
    let repointed: Vec<String> = (0..PAGES).map(|_| page("/Annots 4 0 R")).collect();
    let mut later: Vec<(u32, Row<'_>)> = repointed.iter().enumerate().map(|(i, b)| (first_page + i as u32, Row::Obj(0, 0, b))).collect();
    let copy = format!("[{annots}]");
    later.push((4, Row::Obj(0, 0, &copy)));
    let now = signed.clone().revision(&later);
    let (doc, old) = (now.open(), signed.open());
    let started = std::time::Instant::now();
    let _ = verdict(&doc, old, Some(3));
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
}

/// A damaged signed slot (header 5, row 0) rewritten cleanly as `4 0 obj` keeps the generation
/// every signed reference uses: a form fill. Had anything signed referred to it as `4 5 R`, that
/// reference would no longer resolve, and the change is refused.
#[test]
fn a_repaired_slot_rewritten_cleanly_is_judged_by_its_references() {
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let signed = aliased_field("4 0 R", "4 0 R");
    let now = signed.clone().revision(&[(4, Row::Obj(0, 0, &filled))]);
    assert_eq!(verdict(&now.open(), signed.open(), Some(2)), Modification::Allowed(vec![super::kind::FORM_FILL]));
    let signed = aliased_field("4 5 R", "4 0 R");
    let now = signed.clone().revision(&[(4, Row::Obj(0, 0, &filled))]);
    assert!(matches!(verdict(&now.open(), signed.open(), Some(2)), Modification::Disallowed(_)));
}
