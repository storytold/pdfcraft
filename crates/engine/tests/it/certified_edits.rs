//! A certification named from the catalog's `/Perms /DocMDP` prevents the changes its DocMDP
//! permissions don't allow, as well as detecting them (ISO 32000-2 §12.8.2.2.1, Table 257): 1 no
//! changes, 2 filling in forms and signing, 3 comments as well. Disallowed edits are refused
//! before they change anything; allowed ones still apply, save and keep the certification valid.

use std::sync::Arc;

use pdfcraft_engine::sign::{self, Modification};
use pdfcraft_engine::{AddedText, DocId, Edit, EditError, FieldValue, NewAnnotation, NoteIcon, Session, Shape, SignOptions, Style};

/// One page holding an empty text field `name`.
fn form() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 250 180 270] /P 3 0 R >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn digital_id() -> sign::DigitalId {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    sign::pkcs12::open(&p12, "test").unwrap()
}

/// The form, certified with DocMDP `level` (None: an approval signature) by the session's own
/// signing, saved and reopened as the signed file.
fn certified(level: Option<u8>) -> (Session, DocId) {
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("form.pdf", None, Arc::new(form()), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), certify: level, ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    assert_eq!(s.get(id).unwrap().certification(), level);
    (s, id)
}

fn comment() -> Edit {
    let shape = Shape::Note { at: [100.0, 150.0], icon: NoteIcon::Comment };
    let style = Style::default_for(&shape);
    Edit::AddAnnotation(NewAnnotation { page: 0, style, shape, contents: "Looks good".into(), author: "Test".into() })
}

fn fill() -> Edit {
    Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Ada".into()) }
}

fn rotate() -> Edit {
    Edit::RotatePages { pages: vec![0], degrees: 90 }
}

fn add_text() -> Edit {
    Edit::AddText { page: 0, text: AddedText { rect: [20.0, 100.0, 180.0, 130.0], text: "Added".into(), ..Default::default() } }
}

fn blank_page() -> Edit {
    Edit::InsertBlankPage { at: 1, width: 200.0, height: 300.0 }
}

/// The certification's verdict on the changes made since it was signed, after saving them.
fn saved_verdict(s: &mut Session, id: DocId) -> Modification {
    let bytes = s.save_bytes(id).unwrap();
    s.check_save(id, &bytes).unwrap();
    s.mark_saved(id, bytes, None).unwrap();
    let doc = s.get(id).unwrap();
    doc.signatures.iter().find(|x| x.certify.is_some()).expect("the certification").modification.clone()
}

fn refused(r: Result<(), EditError>) -> &'static str {
    match r {
        Err(EditError::Certified { what, .. }) => what,
        other => panic!("expected a refusal for the certification, got {other:?}"),
    }
}

#[test]
fn no_changes_allowed_refuses_every_edit_and_leaves_the_document_as_signed() {
    let (mut s, id) = certified(Some(1));
    let before = s.get(id).unwrap().bytes.clone();
    assert_eq!(refused(s.apply(id, comment())), "comments");
    assert_eq!(refused(s.apply(id, fill())), "filling in form fields");
    assert_eq!(refused(s.apply(id, rotate())), "page changes");
    assert_eq!(refused(s.apply(id, blank_page())), "page changes");
    assert_eq!(refused(s.apply(id, add_text())), "changes to the document");
    // A batch is refused as a whole when any edit in it is.
    let batch = Edit::Batch { label: "Two".into(), edits: vec![Edit::SetInfo { key: "Title".into(), value: "T".into() }, rotate()] };
    assert_eq!(refused(s.apply(id, batch)), "page changes");
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty, "nothing changed");
    assert!(doc.can_undo().is_none(), "no undo step");
    assert_eq!(doc.info.pages[0].rotation, 0);
    assert_eq!(s.save_bytes(id).unwrap(), before, "a save writes the signed file as it is");
    let message = EditError::Certified { level: 1, what: "comments" }.to_string();
    assert!(message.contains("certified with no changes allowed") && message.contains("comments"), "{message}");
}

#[test]
fn form_fill_and_signing_certification_allows_only_those() {
    let (mut s, id) = certified(Some(2));
    assert_eq!(refused(s.apply(id, comment())), "comments");
    assert_eq!(refused(s.apply(id, rotate())), "page changes");
    assert_eq!(refused(s.apply(id, add_text())), "changes to the document");
    s.apply(id, fill()).expect("filling in a form field is allowed");
    assert!(matches!(saved_verdict(&mut s, id), Modification::Allowed(_)), "the certification stays valid");
    // Signing is allowed too, and keeps it valid.
    let opts = SignOptions { page: 0, rect: Some([20.0, 60.0, 160.0, 90.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).expect("an approval signature is allowed");
    s.mark_signed(id, signed, None).unwrap();
    let doc = s.get(id).unwrap();
    let cert = doc.signatures.iter().find(|x| x.certify == Some(2)).unwrap();
    assert!(matches!(cert.modification, Modification::Allowed(_)), "{:?}", cert.modification);
    assert_eq!(doc.certification(), Some(2), "the approval signature changes nothing about what is allowed");
}

#[test]
fn comments_certification_allows_comments_and_form_fill_only() {
    let (mut s, id) = certified(Some(3));
    assert_eq!(refused(s.apply(id, rotate())), "page changes");
    assert_eq!(refused(s.apply(id, add_text())), "changes to the document");
    s.apply(id, comment()).expect("a comment is allowed");
    s.apply(id, fill()).expect("filling in a form field is allowed");
    s.apply(id, Edit::DeleteAnnotation { page: 0, index: 2 }).expect("deleting the comment is allowed");
    assert!(matches!(saved_verdict(&mut s, id), Modification::Allowed(_)), "the certification stays valid");
}

#[test]
fn document_information_and_undo_stay_available_under_a_certification() {
    let (mut s, id) = certified(Some(1));
    // Every save rewrites /Info; its entries are metadata, which validation allows at every level.
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Final".into() }).expect("metadata is allowed");
    s.undo(id).expect("undo");
    s.redo(id).expect("redo");
    assert!(matches!(saved_verdict(&mut s, id), Modification::Allowed(_)), "the certification stays valid");
}

#[test]
fn approval_signatures_restrict_nothing() {
    let (mut s, id) = certified(None);
    s.apply(id, comment()).expect("a comment");
    s.apply(id, rotate()).expect("a page change (validation reports it)");
}

const XFDF_FIELDS: &str =
    r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><fields><field name="name"><value>Ada</value></field></fields></xfdf>"#;
const XFDF_COMMENTS: &str = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><text page="0" name="c1" rect="100,150,120,170"><contents>Imported</contents></text></annots></xfdf>"#;
const XFDF_BOTH: &str = r#"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><text page="0" name="c1" rect="100,150,120,170"><contents>Imported</contents></text></annots><fields><field name="name"><value>Ada</value></field></fields></xfdf>"#;

fn import(name: &str, bytes: &[u8]) -> Edit {
    Edit::ImportData { name: name.into(), bytes: Arc::new(bytes.to_vec()) }
}

/// A PDF holding one comment, for Import comments from a PDF.
fn review_pdf() -> Vec<u8> {
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("review.pdf", None, Arc::new(form()), None).unwrap();
    s.apply(id, comment()).unwrap();
    s.save_bytes(id).unwrap().to_vec()
}

/// The field's value and the number of comments (annotations other than widgets).
fn state(s: &Session, id: DocId) -> (Vec<String>, usize) {
    let doc = s.get(id).unwrap();
    let value = doc.form.iter().find(|f| f.name == "name").map(|f| f.value.clone()).unwrap_or_default();
    (value, doc.info.annotations.iter().filter(|a| a.subtype != "Widget").count())
}

/// An import is judged by what it brings in: field values are filling in forms, comments need
/// a level 3 certification, and a file holding both is refused as a whole at level 2.
#[test]
fn imports_are_judged_by_what_they_bring_in() {
    let fields_xml = b"<?xml version=\"1.0\"?><fields><name>Ada</name></fields>";
    let review = review_pdf();

    // 1: no import at all.
    let (mut s, id) = certified(Some(1));
    let before = s.get(id).unwrap().bytes.clone();
    for (name, bytes) in [
        ("f.xfdf", XFDF_FIELDS.as_bytes()),
        ("f.xml", fields_xml),
        ("c.xfdf", XFDF_COMMENTS.as_bytes()),
        ("b.xfdf", XFDF_BOTH.as_bytes()),
        ("review.pdf", &review),
    ] {
        assert_eq!(refused(s.apply(id, import(name, bytes))), "importing data", "{name}");
    }
    assert!(!s.get(id).unwrap().dirty && s.get(id).unwrap().can_undo().is_none());
    assert_eq!(s.save_bytes(id).unwrap(), before, "a save writes the signed file as it is");

    // 2: field data only.
    let (mut s, id) = certified(Some(2));
    assert_eq!(refused(s.apply(id, import("c.xfdf", XFDF_COMMENTS.as_bytes()))), "comments");
    assert_eq!(refused(s.apply(id, import("review.pdf", &review))), "comments");
    assert_eq!(refused(s.apply(id, import("b.xfdf", XFDF_BOTH.as_bytes()))), "comments", "comments with the values: refused as a whole");
    assert_eq!(state(&s, id), (Vec::new(), 0), "nothing was imported");
    assert!(s.get(id).unwrap().can_undo().is_none());
    s.apply(id, import("f.xfdf", XFDF_FIELDS.as_bytes())).expect("XFDF field values are form filling");
    assert_eq!(state(&s, id), (vec!["Ada".to_string()], 0));
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text(String::new()) }).unwrap();
    s.apply(id, import("f.xml", fields_xml)).expect("XML field values are form filling");
    assert_eq!(state(&s, id), (vec!["Ada".to_string()], 0));
    assert!(matches!(saved_verdict(&mut s, id), Modification::Allowed(_)), "the certification stays valid");

    // 3: field data and comments.
    let (mut s, id) = certified(Some(3));
    s.apply(id, import("b.xfdf", XFDF_BOTH.as_bytes())).expect("values and comments are allowed");
    assert_eq!(state(&s, id), (vec!["Ada".to_string()], 1));
    s.apply(id, import("review.pdf", &review)).expect("comments from a PDF are allowed");
    assert_eq!(state(&s, id).1, 2);
    assert!(matches!(saved_verdict(&mut s, id), Modification::Allowed(_)), "the certification stays valid");
}

/// A certified XFA form whose filled text would grow its field keeps its signed pages: the
/// form is not laid out again (that would replace pages, which no certification allows), the
/// value is kept, the document says so, and the certification stays valid.
#[test]
fn a_growing_xfa_fill_under_a_certification_keeps_the_signed_pages() {
    // Without scripts the edit lays the form out itself; with them, the form's script engine does.
    for scripted in [false, true] {
        growing_xfa_fill_keeps_the_signed_pages(scripted);
    }
}

fn growing_xfa_fill_keeps_the_signed_pages(scripted: bool) {
    let template = pdfcraft_xfa::fixtures::growable_text_template(scripted);
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("growing.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), certify: Some(2), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    assert_eq!(s.get(id).unwrap().certification(), Some(2));
    let pages = s.get(id).unwrap().info.pages.len();
    let text: String = (0..150).map(|i| format!("Row{i:03} sample\n")).collect();
    s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text(text.clone()) }).expect("filling is allowed");
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), pages, "no page added (scripted: {scripted})");
    let value = doc.form.iter().find(|f| f.name == "ACTIVIDADES").map(|f| f.value.clone());
    assert_eq!(value, Some(vec![text.clone()]), "the value is kept");
    assert!(doc.xfa_warnings.iter().any(|w| w.contains("certification")), "{:?}", doc.xfa_warnings);
    // Importing the form's data is form filling too, and keeps the signed pages the same way.
    let longer: String = (0..200)
        .map(|i| {
            format!(
                "Line{i:03} imported
"
            )
        })
        .collect();
    let data = format!("<form><Page3><ACTIVIDADES>{longer}</ACTIVIDADES></Page3></form>");
    s.apply(id, import("data.xml", data.as_bytes())).expect("importing the form's data is allowed");
    let doc = s.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), pages, "no page added (scripted: {scripted})");
    let value = doc.form.iter().find(|f| f.name == "ACTIVIDADES").map(|f| f.value.clone());
    assert_eq!(value, Some(vec![longer.clone()]), "the imported value is kept");
    let verdict = saved_verdict(&mut s, id);
    assert!(matches!(verdict, Modification::Allowed(_)), "the certification stays valid: {verdict:?}");
}
