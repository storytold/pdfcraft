//! Changes made after signing are judged against what a reader resolves and by the role each
//! object held when signed: a newer generation, a repaired generation alias, or a widget
//! replaced, removed or moved never passes as a permitted change.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object};

use super::changes::classify_changes;
use super::{Modification, kind};

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

/// The AcroForm's `/Fields` written as an array object of its own (#4817). A signing tool may
/// append its field there and leave the AcroForm as it was: that is signing, not "other
/// changes", and an earlier signature stays intact. A field taken out of the array is still
/// refused, and so is a text field added: the rule is for signers.
#[test]
fn a_fields_array_of_its_own_that_gains_a_signature_field_is_signing() {
    const SIG_DICT: &str = "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached /ByteRange [0 0 0 0] /Contents <00> >>";
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page("/Annots [6 0 R]"))),
        (4, Row::Obj(0, 0, &page(""))),
        (5, Row::Obj(0, 0, "<< /Fields 8 0 R /SigFlags 3 >>")),
        (6, Row::Obj(0, 0, "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig1) /V 7 0 R /Rect [20 20 120 40] /P 3 0 R /F 4 >>")),
        (7, Row::Obj(0, 0, SIG_DICT)),
        (8, Row::Obj(0, 0, "[6 0 R]")),
    ]);
    let later = |fields: &str, field: &str| {
        let page4 = page("/Annots [9 0 R]");
        let mut rows = vec![(8, Row::Obj(0, 0, fields)), (4, Row::Obj(0, 0, &page4)), (9, Row::Obj(0, 0, field))];
        // A signature field brings its signature dictionary.
        if field.contains("/FT /Sig") {
            rows.push((10, Row::Obj(0, 0, SIG_DICT)));
        }
        signed.clone().revision(&rows)
    };
    let signature = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig2) /V 10 0 R /Rect [20 60 120 80] /P 4 0 R /F 4 >>";
    let text = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] /P 4 0 R /F 4 >>";
    assert_eq!(verdict(&later("[6 0 R 9 0 R]", signature).open(), signed.open(), None), Modification::Allowed(vec![kind::SIGNATURE]));
    assert!(
        matches!(verdict(&later("[9 0 R]", signature).open(), signed.open(), None), Modification::Disallowed(d) if d.contains(&kind::OTHER)),
        "the signed field taken out of the form"
    );
    let added_text = verdict(&later("[6 0 R 9 0 R]", text).open(), signed.open(), None);
    assert!(matches!(&added_text, Modification::Disallowed(d) if d.contains(&kind::OTHER)), "{added_text:?}");
}

/// Only what a signer does to a `/Fields` array of its own passes: the signed entries kept as
/// they were and in order, with new signature fields appended, each a single widget that joins
/// no field of the signed form. Anything else is refused at every DocMDP level, such as:
/// - an entry written in place taken out, rewritten or moved;
/// - the order changed, or an entry listed twice;
/// - an object that existed when signed appended, such as a widget no field listed, or the catalog;
/// - an entry written in place appended, something that isn't a field, or a text field;
/// - a new field with `/Kids`, which could take over a signed field;
/// - a new field with a `/Parent`, which grafts it onto one;
/// - a signature value that was there when signed, or one written in place;
/// - an action on the new widget;
/// - an object number something signed refers to, though it held nothing then: a signed entry
///   that resolved to nothing filled in, or a new field or value at such a number (review of
///   #4817).
#[test]
fn a_fields_array_of_its_own_only_accepts_new_signature_fields_appended() {
    const SIG_DICT: &str = "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached /ByteRange [0 0 0 0] /Contents <00> >>";
    const TIMESTAMP: &str = "<< /Type /DocTimeStamp /Filter /Adobe.PPKLite /SubFilter /ETSI.RFC3161 /ByteRange [0 0 0 0] /Contents <00> >>";
    const LISTED: &str = "[6 0 R 13 0 R]";
    const IN_PLACE: &str = "[<< /FT /Sig /T (Approval) /V 7 0 R >> 6 0 R 13 0 R]";
    let capture = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (ts) /Kids [13 0 R] /V 12 0 R /Rect [0 0 0 0] /F 2 >>";
    let (page3, page4) = (page("/Annots [6 0 R]"), page("/Annots [13 0 R]"));
    let signed = |fields: &str, extra: &[(u32, &str)]| {
        let mut rows = vec![
            (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>")),
            (2, Row::Obj(0, 0, PAGES)),
            (3, Row::Obj(0, 0, &page3)),
            (4, Row::Obj(0, 0, &page4)),
            (5, Row::Obj(0, 0, "<< /Fields 8 0 R /SigFlags 3 >>")),
            (6, Row::Obj(0, 0, "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig1) /V 7 0 R /Rect [20 20 120 40] /P 3 0 R /F 4 >>")),
            (7, Row::Obj(0, 0, SIG_DICT)),
            (8, Row::Obj(0, 0, fields)),
            (9, Row::Obj(0, 0, "<< /Type /Annot /Subtype /Widget /FT /Sig /T (unlisted) /Rect [0 0 100 40] >>")),
            (13, Row::Obj(0, 0, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] /P 4 0 R /F 4 >>")),
        ];
        rows.extend(extra.iter().map(|(n, body)| (*n, Row::Obj(0, 0, body))));
        File::new().revision(&rows)
    };
    /// A name, the signed `/Fields`, the later one, and objects the later revision adds.
    type Case<'a> = (&'a str, &'a str, &'a str, &'a [(u32, &'a str)]);
    let widget = |extra: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig2) /P 4 0 R /Rect [0 0 0 0] /F 2 {extra} >>");
    let (grafted, reused, in_place_value) = (widget("/Parent 6 0 R /V 12 0 R"), widget("/V 7 0 R"), widget(&format!("/V {TIMESTAMP}")));
    let (timestamp, timestamp2) = (widget("/V 12 0 R"), widget("/V 15 0 R"));
    let with_action = widget("/V 12 0 R /AA << /PO << /S /URI /URI (https://example.com) >> >>");
    // References by a generation their number doesn't have: null to this reader, the signed
    // object to one that ignores generations.
    let (parent_gen, kids_gen) = (widget("/Parent 6 1 R /V 12 0 R"), widget("/Kids 31 1 R /V 12 0 R"));
    let (actions_gen, value_gen) = (widget("/AA 30 1 R /V 12 0 R"), widget("/V 7 1 R"));
    let cases: [Case; 20] = [
        ("in-place entry taken out", IN_PLACE, LISTED, &[]),
        ("in-place entry rewritten", IN_PLACE, "[<< /FT /Sig /T (Other) /V 7 0 R >> 6 0 R 13 0 R]", &[]),
        ("in-place entry moved", IN_PLACE, "[6 0 R << /FT /Sig /T (Approval) /V 7 0 R >> 13 0 R]", &[]),
        ("order changed", LISTED, "[13 0 R 6 0 R]", &[]),
        ("entry listed twice", LISTED, "[6 0 R 13 0 R 6 0 R]", &[]),
        ("unlisted widget appended", LISTED, "[6 0 R 13 0 R 9 0 R]", &[]),
        ("catalog appended", LISTED, "[6 0 R 13 0 R 1 0 R]", &[]),
        ("in-place field appended", LISTED, "[6 0 R 13 0 R << /FT /Sig /T (sig2) /V 12 0 R >>]", &[(12, TIMESTAMP)]),
        ("not a field appended", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, "<< /Type /Foo >>")]),
        ("signed field captured", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, capture), (12, TIMESTAMP)]),
        (
            "text field appended",
            LISTED,
            "[6 0 R 13 0 R 11 0 R]",
            &[(11, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (t2) /P 4 0 R /Rect [20 100 120 120] /F 4 >>")],
        ),
        (
            "field with a widget under it",
            LISTED,
            "[6 0 R 13 0 R 11 0 R]",
            &[
                (11, "<< /FT /Sig /T (group) /Kids [14 0 R] >>"),
                (14, "<< /Type /Annot /Subtype /Widget /Parent 11 0 R /P 4 0 R /Rect [0 0 0 0] /F 2 >>"),
            ],
        ),
        ("grafted onto a signed field", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &grafted), (12, TIMESTAMP)]),
        ("signed signature value reused", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &reused)]),
        ("signature value written in place", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &in_place_value)]),
        ("action on the new widget", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &with_action), (12, TIMESTAMP)]),
        ("parent by another generation", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &parent_gen), (12, TIMESTAMP)]),
        ("kids by another generation", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &kids_gen), (12, TIMESTAMP), (31, "[13 0 R]")]),
        (
            "actions by another generation",
            LISTED,
            "[6 0 R 13 0 R 11 0 R]",
            &[(11, &actions_gen), (12, TIMESTAMP), (30, "<< /PO << /S /URI /URI (https://example.com) >> >>")],
        ),
        ("signed value by another generation", LISTED, "[6 0 R 13 0 R 11 0 R]", &[(11, &value_gen)]),
    ];
    /// A name, the signed `/Fields`, objects the signed revision holds besides, the later
    /// `/Fields`, and objects the later revision adds.
    type Signed<'a> = (&'a str, &'a str, &'a [(u32, &'a str)], &'a str, &'a [(u32, &'a str)]);
    let referred: [Signed; 3] = [
        (
            "signed entry that held nothing filled in",
            "[6 0 R 13 0 R 11 0 R]",
            &[],
            "[6 0 R 13 0 R 11 0 R 14 0 R]",
            &[(11, &timestamp), (12, TIMESTAMP), (14, &timestamp2), (15, TIMESTAMP)],
        ),
        (
            "new field at a number something signed refers to",
            LISTED,
            &[(11, "null"), (16, "[11 0 R]")],
            "[6 0 R 13 0 R 11 0 R]",
            &[(11, &timestamp), (12, TIMESTAMP)],
        ),
        (
            "new value at a number something signed refers to",
            LISTED,
            &[(12, "null"), (16, "[12 0 R]")],
            "[6 0 R 13 0 R 11 0 R]",
            &[(11, &timestamp), (12, TIMESTAMP)],
        ),
    ];
    let runs = cases.iter().map(|(name, was, now, extra)| (*name, signed(was, &[]), *now, *extra));
    let runs = runs.chain(referred.iter().map(|(name, was, before, now, extra)| (*name, signed(was, before), *now, *extra)));
    let mut passed = Vec::new();
    for (name, old, now, extra) in runs {
        let mut rows = vec![(8, Row::Obj(0, 0, now))];
        rows.extend(extra.iter().map(|(n, body)| (*n, Row::Obj(0, 0, body))));
        let later = old.clone().revision(&rows);
        for p in [None, Some(1), Some(2), Some(3)] {
            let v = verdict(&later.open(), old.open(), p);
            if !matches!(&v, Modification::Disallowed(d) if d.contains(&kind::OTHER)) {
                passed.push(format!("{name}, P={p:?}: {v:?}"));
            }
        }
    }
    assert!(passed.is_empty(), "{passed:#?}");

    // What a signer writes still passes: a signature on the page at P=2, and an invisible
    // document timestamp even at P=1, also at a number that held null and nothing referred to.
    let old = signed(LISTED, &[]);
    let page4 = page("/Annots [13 0 R 11 0 R]");
    let visible = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig2) /V 12 0 R /P 4 0 R /Rect [20 100 120 120] /F 4 >>";
    let later = old.clone().revision(&[
        (8, Row::Obj(0, 0, "[6 0 R 13 0 R 11 0 R]")),
        (11, Row::Obj(0, 0, visible)),
        (12, Row::Obj(0, 0, SIG_DICT)),
        (4, Row::Obj(0, 0, &page4)),
    ]);
    assert_eq!(verdict(&later.open(), old.open(), Some(2)), Modification::Allowed(vec![kind::SIGNATURE]));
    let appended = [(8, Row::Obj(0, 0, "[6 0 R 13 0 R 11 0 R]")), (11, Row::Obj(0, 0, &timestamp)), (12, Row::Obj(0, 0, TIMESTAMP))];
    let later = old.clone().revision(&appended);
    assert_eq!(verdict(&later.open(), old.open(), Some(1)), Modification::Allowed(vec![kind::SIGNATURE]));
    // An entry whose value is null, or a reference to nothing, is no entry (ISO 32000-1 §7.3.7,
    // §7.3.10): the field is still a signer's.
    for nulls in ["/Parent null /Kids null /AA null /A null", "/Parent 99 0 R"] {
        let field = widget(&format!("/V 12 0 R {nulls}"));
        let later =
            old.clone().revision(&[(8, Row::Obj(0, 0, "[6 0 R 13 0 R 11 0 R]")), (11, Row::Obj(0, 0, &field)), (12, Row::Obj(0, 0, TIMESTAMP))]);
        assert_eq!(verdict(&later.open(), old.open(), Some(1)), Modification::Allowed(vec![kind::SIGNATURE]), "{nulls}");
    }
    let empty = widget("/V null");
    let later = old.clone().revision(&[(8, Row::Obj(0, 0, "[6 0 R 13 0 R 11 0 R]")), (11, Row::Obj(0, 0, &empty))]);
    assert_eq!(verdict(&later.open(), old.open(), Some(2)), Modification::Allowed(vec![kind::SIGNATURE]));
    let old = signed(LISTED, &[(11, "null")]);
    let later = old.clone().revision(&appended);
    assert_eq!(verdict(&later.open(), old.open(), Some(1)), Modification::Allowed(vec![kind::SIGNATURE]));
}

/// A `/Fields` array that a signed page also draws as its `/Contents` is page content, not a
/// form listing: reordering it reorders what the page draws, and a hostile file must not pass
/// that off as signing (review of #4817).
#[test]
fn a_fields_array_that_is_also_page_contents_is_judged_as_page_content() {
    let stream = |data: &str| format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len());
    let (red, blue) = (stream("1 0 0 rg 0 0 100 100 re f"), stream("0 0 1 rg 0 0 100 100 re f"));
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page("/Contents 8 0 R"))),
        (4, Row::Obj(0, 0, &page(""))),
        (5, Row::Obj(0, 0, "<< /Fields 8 0 R /SigFlags 3 >>")),
        (6, Row::Obj(0, 0, &red)),
        (7, Row::Obj(0, 0, &blue)),
        (8, Row::Obj(0, 0, "[6 0 R 7 0 R]")),
    ]);
    let reordered = signed.clone().revision(&[(8, Row::Obj(0, 0, "[7 0 R 6 0 R]"))]);
    let verdict = verdict(&reordered.open(), signed.open(), None);
    assert!(matches!(&verdict, Modification::Disallowed(_)), "{verdict:?}");
}

/// A `/Fields` array that a signed page also uses as its `/MediaBox` is the page's geometry: a
/// later revision rewriting it resizes the page, which no signing permits (review of #4817).
#[test]
fn a_fields_array_that_is_also_a_page_box_is_not_judged_as_fields() {
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, "<< /Type /Page /Parent 2 0 R /MediaBox 8 0 R >>")),
        (4, Row::Obj(0, 0, &page(""))),
        (5, Row::Obj(0, 0, "<< /Fields 8 0 R /SigFlags 3 >>")),
        (8, Row::Obj(0, 0, "[0 0 300 300]")),
    ]);
    let resized = signed.clone().revision(&[(8, Row::Obj(0, 0, "[0 0 30 30]"))]);
    let verdict = verdict(&resized.open(), signed.open(), None);
    assert!(matches!(&verdict, Modification::Disallowed(_)), "{verdict:?}");
}

/// The signed form's `/Fields` array moved to another role in the later revision is not judged
/// as fields, even when it is still referred to only once: here the page lists it as its
/// `/Annots`, with an inline full-page annotation added, and the form's `/Fields` is written
/// inline (review of #4817).
#[test]
fn a_fields_array_moved_to_another_role_is_not_judged_as_fields() {
    const WIDGET: &str = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (sig1) /Rect [20 20 120 40] /P 3 0 R /F 4 >>";
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page("/Annots [6 0 R]"))),
        (4, Row::Obj(0, 0, &page(""))),
        (5, Row::Obj(0, 0, "<< /Fields 8 0 R /SigFlags 3 >>")),
        (6, Row::Obj(0, 0, WIDGET)),
        (8, Row::Obj(0, 0, "[6 0 R]")),
    ]);
    let swapped = signed.clone().revision(&[
        (3, Row::Obj(0, 0, &page("/Annots 8 0 R"))),
        (5, Row::Obj(0, 0, "<< /Fields [6 0 R] /SigFlags 3 >>")),
        (8, Row::Obj(0, 0, "[6 0 R << /Type /Annot /Subtype /Square /Rect [0 0 300 300] >>]")),
    ]);
    let verdict = verdict(&swapped.open(), signed.open(), Some(1));
    assert!(matches!(&verdict, Modification::Disallowed(_)), "{verdict:?}");
}
