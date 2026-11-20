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

/// A signed catalog and text field whose strings a later save respells, given the catalog `extra`
/// entries and the field's `/T` as written in each revision.
fn respelled(signed_extra: &str, now_extra: &str, signed_t: &str, now_t: &str) -> (Document, Document) {
    let catalog = |extra: &str| format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) >> {extra} >>");
    let field = |t: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T {t} /V (Ann) /Rect [20 20 120 40] /P 3 0 R >>");
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, &catalog(signed_extra))),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page("/Annots [5 0 R]"))),
        (4, Row::Obj(0, 0, &page("/Annots []"))),
        (5, Row::Obj(0, 0, &field(signed_t))),
    ]);
    let now = signed.clone().revision(&[(1, Row::Obj(0, 0, &catalog(now_extra))), (5, Row::Obj(0, 0, &field(now_t)))]);
    (now.open(), signed.open())
}

/// A string is its bytes (ISO 32000-2 §7.3.4.1): a later save that writes the catalog's /Lang in
/// hexadecimal instead of as a literal changes nothing, under every permission level.
#[test]
fn a_string_respelled_in_hex_is_no_change() {
    for p in [Some(1), Some(2), Some(3), None] {
        let (doc, old) = respelled("/Lang (en-US)", "/Lang <656E2D5553>", "(Name)", "(Name)");
        assert_eq!(verdict(&doc, old, p), Modification::Allowed(Vec::new()), "P={p:?}");
    }
}

/// The same holds for strings nested in dictionaries and arrays (a name tree's keys), with the
/// entries also in another order, and for a signed field's /T.
#[test]
fn nested_strings_respelled_and_reordered_are_no_change() {
    let (doc, old) = respelled(
        "/Names << /Dests << /Names [(intro) [3 0 R /Fit]] >> >> /Lang (en-US)",
        "/Lang <656e2d5553> /Names << /Dests << /Names [<696E74726F> [3 0 R /Fit]] >> >>",
        "(Name)",
        "<4E616D65>",
    );
    assert_eq!(verdict(&doc, old, Some(1)), Modification::Allowed(Vec::new()));
}

/// Different bytes are a different string however they are written: a catalog whose /Lang (or a
/// string deep in its name tree) now says something else is still a structure change.
#[test]
fn a_string_whose_bytes_change_is_still_a_change() {
    for (signed, now) in [
        ("/Lang (en-US)", "/Lang <64652D4445>"),
        ("/Names << /Dests << /Names [(intro) [3 0 R /Fit]] >> >>", "/Names << /Dests << /Names [<6F75747472> [3 0 R /Fit]] >> >>"),
    ] {
        let (doc, old) = respelled(signed, now, "(Name)", "(Name)");
        assert_eq!(verdict(&doc, old, Some(2)), Modification::Disallowed(vec![super::kind::STRUCTURE]), "{now}");
    }
}

/// A save that adds a signature field and respells the catalog's and the form's strings on the
/// way is judged by the field it added: the respelled /Lang and /DA are not a structure or form
/// change.
#[test]
fn a_signature_added_with_respelled_strings_is_signing() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /Lang (en-US) /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) >> >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page("/Annots [5 0 R]"))),
        (4, Row::Obj(0, 0, &page("/Annots []"))),
        (5, Row::Obj(0, 0, TEXT_FIELD)),
    ]);
    let catalog =
        |lang: &str, da: &str| format!("<< /Type /Catalog /Pages 2 0 R /Lang {lang} /AcroForm << /DA {da} /Fields [5 0 R 6 0 R] /SigFlags 3 >> >>");
    let sig = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig1) /Rect [0 0 0 0] /P 4 0 R >>";
    let now = |lang: &str, da: &str| {
        signed.clone().revision(&[(1, Row::Obj(0, 0, &catalog(lang, da))), (4, Row::Obj(0, 0, &page("/Annots [6 0 R]"))), (6, Row::Obj(0, 0, sig))])
    };
    let respelled = now("<656E2D5553>", "<2F48656C76203020546620302067>");
    assert_eq!(verdict(&respelled.open(), signed.open(), Some(2)), Modification::Allowed(vec![super::kind::SIGNATURE]));
    // A /DA that now says something else is a form change, however it is spelled.
    let restyled = now("<656E2D5553>", "<2F48656C7620313220546620302067>");
    assert_eq!(verdict(&restyled.open(), signed.open(), None), Modification::Allowed(vec![super::kind::FORM_FILL, super::kind::SIGNATURE]));
}

/// A signed widget whose /Rect was written with reals, filled and saved by PdfCraft's own writer,
/// which writes a whole real as an integer: the rectangle is the same numbers (ISO 32000-2
/// §7.3.3, "wherever a real number is expected, an integer may be used instead"), so the save is a
/// form fill. A rectangle whose numbers did change is still a moved widget.
#[test]
fn a_widget_rect_saved_back_as_integers_is_a_form_fill() {
    let field = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ann) /Rect [20.0 20.0 120.0 40.0] /P 3 0 R >>";
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page("/Annots [5 0 R]"))),
        (4, Row::Obj(0, 0, &page("/Annots []"))),
        (5, Row::Obj(0, 0, field)),
    ]);
    let save = |rect: Option<f64>| {
        let mut doc = signed.open();
        doc.update_dict(r(5, 0), |w| {
            w.set(b"V".to_vec(), Object::String(pdfcraft_cos::PdfString::literal("Bob")));
            if let Some(x) = rect {
                w.set(b"Rect".to_vec(), Object::Array(vec![Object::Real(x), Object::Real(20.0), Object::Real(120.0), Object::Real(40.0)]));
            }
        })
        .unwrap();
        let bytes = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions { object_streams: false, ..Default::default() }).unwrap();
        Document::open(Arc::new(bytes)).unwrap()
    };
    let filled = save(None);
    let rect = filled.resolve(&Object::Ref(r(5, 0))).as_dict().and_then(|w| w.get(b"Rect").cloned());
    assert_eq!(
        rect,
        Some(Object::Array(vec![Object::Int(20), Object::Int(20), Object::Int(120), Object::Int(40)])),
        "the writer respells whole reals"
    );
    assert_eq!(verdict(&filled, signed.open(), Some(2)), Modification::Allowed(vec![super::kind::FORM_FILL]));
    let moved = save(Some(20.5));
    assert_eq!(verdict(&moved, signed.open(), Some(2)), Modification::Disallowed(vec![super::kind::PAGE_CONTENT]));
}

/// Append a revision indexed by a cross-reference stream: `plain` objects written on their own,
/// `members` packed into one object stream numbered `stream`, the cross-reference stream as
/// `stream + 1`. The file's size grows to cover both. Returns the file with this revision last.
fn packed_revision(mut file: File, plain: &[(u32, &str)], members: &[(u32, &str)], stream: u32) -> File {
    let xref_num = stream + 1;
    file.size = file.size.max(xref_num + 1);
    let mut rows: Vec<(u32, u8, u32, u16)> = Vec::new();
    for (num, body) in plain {
        rows.push((*num, 1, u32::try_from(file.bytes.len()).unwrap(), 0));
        file.bytes.extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let (mut header, mut data) = (String::new(), String::new());
    for (index, (num, body)) in members.iter().enumerate() {
        header.push_str(&format!("{num} {} ", data.len()));
        data.push_str(body);
        data.push('\n');
        rows.push((*num, 2, stream, u16::try_from(index).unwrap()));
    }
    rows.push((stream, 1, u32::try_from(file.bytes.len()).unwrap(), 0));
    let body = format!("{header}{data}");
    file.bytes.extend_from_slice(
        format!(
            "{stream} 0 obj\n<< /Type /ObjStm /N {} /First {} /Length {} >>\nstream\n{body}\nendstream\nendobj\n",
            members.len(),
            header.len(),
            body.len()
        )
        .as_bytes(),
    );
    let at = file.bytes.len();
    rows.push((xref_num, 1, u32::try_from(at).unwrap(), 0));
    if file.xref.is_none() {
        rows.push((0, 0, 0, 65535));
    }
    rows.sort_by_key(|row| row.0);
    let (mut index, mut table) = (String::new(), Vec::new());
    for (num, kind, field, last) in &rows {
        index.push_str(&format!("{num} 1 "));
        table.push(*kind);
        table.extend_from_slice(&field.to_be_bytes());
        table.extend_from_slice(&last.to_be_bytes());
    }
    let prev = file.xref.map(|p| format!(" /Prev {p}")).unwrap_or_default();
    file.bytes.extend_from_slice(
        format!(
            "{xref_num} 0 obj\n<< /Type /XRef /Size {} /Index [{index}] /W [1 4 2] /Root 1 0 R{prev} /Length {} >>\nstream\n",
            file.size,
            table.len()
        )
        .as_bytes(),
    );
    file.bytes.extend_from_slice(&table);
    file.bytes.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{at}\n%%EOF\n").as_bytes());
    file.xref = Some(at);
    file
}

/// A signed form whose text field, object 4, is a member of an object stream that the field list
/// and the page's /Annots reach as `4 {generation} R`.
fn packed_field(generation: u16) -> File {
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 {generation} R] >> >>");
    let page = page(&format!("/Annots [4 {generation} R]"));
    packed_revision(File::new(), &[(1, &catalog), (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"), (3, &page)], &[(4, TEXT_FIELD)], 5)
}

/// A form fill that writes a field signed inside an object stream on its own is still a form
/// fill: the signed references reach the new object as they reached the old.
#[test]
fn a_packed_field_filled_on_its_own_is_a_form_fill() {
    let signed = packed_field(0);
    assert!(signed.open().accepts_any_generation(4));
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let now = signed.clone().revision(&[(4, Row::Obj(0, 0, &filled))]);
    assert!(!now.open().accepts_any_generation(4));
    assert_eq!(verdict(&now.open(), signed.open(), Some(2)), Modification::Allowed(vec![super::kind::FORM_FILL]));
}

/// A member of an object stream reached as `4 1 R` when signed (ISO 32000-2 §7.5.7 gives it
/// generation 0, and readers follow the reference to it). Rewritten as `4 0 obj` with the same
/// content, the signed references no longer reach it, and the field leaves the page: a change.
#[test]
fn a_packed_field_signed_through_another_generation_cannot_be_unpacked_away() {
    let signed = packed_field(1);
    assert!(signed.open().get(r(4, 1)).as_dict().is_some(), "the signed field is shown");
    let now = signed.clone().revision(&[(4, Row::Obj(0, 0, TEXT_FIELD))]);
    assert_eq!(*now.open().get(r(4, 1)), Object::Null);
    for p in [Some(1), Some(2), Some(3), None] {
        assert!(matches!(verdict(&now.open(), signed.open(), p), Modification::Disallowed(_)), "P={p:?}");
    }
    // Filled and written as `4 1 obj`, the generation every signed reference uses, it stays on
    // the page: a form fill.
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let now = signed.clone().revision(&[(4, Row::Obj(1, 1, &filled))]);
    assert_eq!(verdict(&now.open(), signed.open(), Some(2)), Modification::Allowed(vec![super::kind::FORM_FILL]));
}

/// The other way round: a field written on its own that the signed revision lists as `4 1 R`,
/// a generation it doesn't have, is not on the page when signed. Packing it into an object
/// stream later makes the reference reach it: a change, and packing it with every signed
/// reference unchanged is none.
#[test]
fn packing_a_field_that_a_signed_reference_missed_is_a_change() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 1 R] >> >>")),
        (2, Row::Obj(0, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")),
        (3, Row::Obj(0, 0, &page("/Annots [4 1 R]"))),
        (4, Row::Obj(0, 0, TEXT_FIELD)),
    ]);
    assert_eq!(*signed.open().get(r(4, 1)), Object::Null, "nothing reaches the field when signed");
    let now = packed_revision(signed.clone(), &[], &[(4, TEXT_FIELD)], 5);
    assert!(now.open().get(r(4, 1)).as_dict().is_some());
    assert!(matches!(verdict(&now.open(), signed.open(), Some(3)), Modification::Disallowed(_)));
    let signed = form("/Annots [5 0 R]");
    let now = packed_revision(signed.clone(), &[], &[(5, TEXT_FIELD)], 7);
    assert!(now.open().accepts_any_generation(5));
    assert_eq!(verdict(&now.open(), signed.open(), Some(2)), Modification::Allowed(Vec::new()));
}
