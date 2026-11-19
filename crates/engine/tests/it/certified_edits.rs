//! A certification named from the catalog's `/Perms /DocMDP` prevents the changes its DocMDP
//! permissions don't allow, as well as detecting them (ISO 32000-2 §12.8.2.2.1, Table 257): 1 no
//! changes, 2 filling in forms, instantiating page templates and signing, 3 comments as well.
//! Disallowed edits are refused before they change anything; allowed ones still apply, and the
//! form fill and comments save and keep the certification valid.

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
    // Each level names the document information, which stays open under every one of them.
    for level in [1, 2, 3] {
        let message = EditError::Certified { level, what: "page changes" }.to_string();
        assert!(message.contains("document information"), "{level}: {message}");
    }
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
fn instantiating_a_page_template_is_allowed_from_form_fill_up_and_hiding_one_never() {
    let spawn = Edit::SpawnTemplate { name: "More".into(), at: 1, rename: true };
    let hide = |hidden| Edit::SetTemplateHidden { name: "More".into(), hidden };
    let (s, id) = certified(Some(1));
    let doc = s.get(id).unwrap();
    assert_eq!(refused(doc.check_certification(&spawn)), "instantiating page templates");
    assert_eq!(refused(doc.check_certification(&hide(false))), "changes to the document");
    assert_eq!(refused(doc.check_certification(&hide(true))), "changes to the document");
    for level in [2, 3] {
        let (s, id) = certified(Some(level));
        let doc = s.get(id).unwrap();
        doc.check_certification(&spawn).unwrap_or_else(|e| panic!("P {level} permits instantiating a template: {e}"));
        // Showing or hiding a template moves its own page in or out of the document.
        assert_eq!(refused(doc.check_certification(&hide(false))), "changes to the document", "P {level}");
        assert_eq!(refused(doc.check_certification(&hide(true))), "changes to the document", "P {level}");
    }
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

/// A measurement scale is a viewport in the page dictionary (`/VP`), not a comment: no
/// certification allows it, while a measurement (a comment) is allowed at level 3.
#[test]
fn a_measurement_scale_is_a_page_change_under_a_certification() {
    let (mut s, id) = certified(Some(3));
    let scale = Edit::SetMeasurementScale {
        page: 0,
        bbox: [0.0, 0.0, 100.0, 100.0],
        name: "Detail".into(),
        scale: pdfcraft_engine::measure::Scale::default(),
    };
    assert_eq!(refused(s.apply(id, scale)), "page changes");
    assert!(s.get(id).unwrap().can_undo().is_none(), "nothing was changed");
}

/// A script can move or resize a field's widget while filling the form in (here the changes it
/// handed back), which changes its page: refused under a certification that allows filling in,
/// while the value it sets the same way goes ahead and keeps the certification valid.
#[test]
fn a_fill_that_moves_a_field_is_refused_under_a_certification() {
    let (mut s, id) = certified(Some(2));
    let moved = pdfcraft_engine::FieldChange { name: "name".into(), rect: Some([30.0, 240.0, 190.0, 260.0]), ..Default::default() };
    assert_eq!(refused(s.apply(id, Edit::ApplyScriptChanges { changes: vec![moved] })), "moving or resizing form fields");
    assert!(s.get(id).unwrap().can_undo().is_none(), "nothing was changed");
    let valued = pdfcraft_engine::FieldChange { name: "name".into(), value: Some(vec!["Ada".into()]), ..Default::default() };
    s.apply(id, Edit::ApplyScriptChanges { changes: vec![valued] }).expect("a value is filling in");
    let verdict = saved_verdict(&mut s, id);
    assert!(matches!(verdict, Modification::Allowed(_)), "the certification stays valid: {verdict:?}");
}

/// A script can hide or show a field's widget while filling the form in (its `/F`), which changes
/// what the page shows: refused under a certification that allows filling in, as the signature
/// check would count it as a page change.
#[test]
fn a_fill_that_hides_a_field_is_refused_under_a_certification() {
    let (mut s, id) = certified(Some(2));
    let hidden = pdfcraft_engine::FieldChange { name: "name".into(), display: Some(1), ..Default::default() };
    assert_eq!(refused(s.apply(id, Edit::ApplyScriptChanges { changes: vec![hidden] })), "hiding or showing form fields");
    assert!(s.get(id).unwrap().can_undo().is_none(), "nothing was changed");
    assert!(matches!(saved_verdict(&mut s, id), Modification::None), "the certification is as signed");
}

/// A form whose initialize script changes its field `flip` each time it is opened.
fn flipping_template() -> String {
    r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="master"><occur max="-1"/>
  <contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/>
 </pageArea></pageSet>
 <subform name="page1" layout="tb">
  <field name="flip" w="200pt" h="20pt"><ui><textEdit/></ui>
   <event activity="initialize"><script contentType="application/x-javascript">this.rawValue = (this.rawValue == "a") ? "b" : "a";</script></event>
  </field>
 </subform>
</subform></template>
</xdp:xdp>"#
        .to_string()
}

/// Opening a form certified with no changes allowed runs its scripts, but what they change on
/// opening isn't kept: the document stays the signed file, so saving it keeps the certification as
/// signed, and a warning says so. A certification that allows filling in keeps the values they set.
#[test]
fn opening_a_form_certified_with_no_changes_keeps_the_signed_file() {
    for level in [1, 2] {
        let mut s = Session::new().with_clock(|| 1_800_000_000);
        let id = s.open("flip.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&flipping_template())), None).unwrap();
        let flip = |s: &Session, id| s.get(id).unwrap().form.iter().find(|f| f.name.ends_with("flip")).map(|f| f.value.clone()).unwrap_or_default();
        let first = flip(&s, id);
        let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), certify: Some(level), ..SignOptions::default() };
        let signed = s.sign(id, &digital_id(), opts).unwrap();
        let mut again = Session::new().with_clock(|| 1_800_000_000);
        let reopened = again.open("flip.pdf", None, signed.clone(), None).unwrap();
        let doc = again.get(reopened).unwrap();
        if level == 1 {
            assert_eq!(flip(&again, reopened), first, "the value the script would set isn't kept");
            assert_eq!(doc.bytes, signed, "the document is the signed file");
            assert!(doc.info.warnings.iter().any(|w| w.contains("certification allows no changes")), "{:?}", doc.info.warnings);
            assert!(matches!(saved_verdict(&mut again, reopened), Modification::None), "the certification is as signed");
        } else {
            assert_ne!(flip(&again, reopened), first, "a certification that allows filling in keeps the value");
            assert!(matches!(saved_verdict(&mut again, reopened), Modification::Allowed(_)), "the certification stays valid");
        }
    }
}

/// A certified scripted XFA form, filled and signed with certification `level` 2, its text field
/// holding `text`: the session and the document.
fn certified_xfa(text: &str) -> (Session, DocId) {
    let template = pdfcraft_xfa::fixtures::growable_text_template(true);
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("growing.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), certify: Some(2), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text(text.into()) }).expect("filling is allowed");
    (s, id)
}

/// Data imported into a certified XFA form replaces the form's data, so a field the new data
/// doesn't hold is emptied, as on a form that isn't laid out again: the field and the data agree,
/// and the signed pages stay.
#[test]
fn a_certified_xfa_import_empties_the_fields_its_data_leaves_out() {
    let (mut s, id) = certified_xfa("Filled before");
    let pages = s.get(id).unwrap().info.pages.len();
    s.apply(id, import("data.xml", b"<form><Page3></Page3></form>")).expect("importing the form's data is allowed");
    let doc = s.get(id).unwrap();
    let value = doc.form.iter().find(|f| f.name == "ACTIVIDADES").map(|f| f.value.clone());
    assert!(value.as_ref().is_some_and(|v| v.iter().all(String::is_empty)), "{value:?}");
    assert_eq!(doc.info.pages.len(), pages);
    let verdict = saved_verdict(&mut s, id);
    assert!(matches!(verdict, Modification::Allowed(_)), "the certification stays valid: {verdict:?}");
}

/// A form whose initialize script hides the section `extra` (a second page) once `flag` reads
/// "hide": filling `flag` in changes nothing until the form is opened again.
fn hiding_template() -> String {
    r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="master"><occur max="-1"/>
  <contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/>
 </pageArea></pageSet>
 <subform name="page1" layout="tb">
  <event activity="initialize"><script contentType="application/x-javascript">extra.presence = (flag.rawValue == "hide") ? "hidden" : "visible";</script></event>
  <field name="flag" w="200pt" h="20pt"><ui><textEdit/></ui></field>
  <subform name="extra" layout="tb"><breakBefore targetType="pageArea"/>
   <draw name="more" w="500pt" h="20pt"><value><text>Second page</text></value></draw>
  </subform>
 </subform>
</subform></template>
</xdp:xdp>"#
        .to_string()
}

/// Reopening a certified XFA form runs its scripts but doesn't lay it out again: a value filled in
/// under the certification that makes an initialize script hide a page doesn't take the page away
/// on opening (that would replace the signed pages, which no level allows), and the
/// certification stays valid.
#[test]
fn reopening_a_certified_xfa_form_keeps_its_signed_pages() {
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("hiding.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&hiding_template())), None).unwrap();
    let pages = s.get(id).unwrap().info.pages.len();
    assert_eq!(pages, 2, "the fixture lays out the second page");
    let opts = SignOptions { page: 0, rect: Some([20.0, 20.0, 160.0, 50.0]), certify: Some(2), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "flag".into(), value: FieldValue::Text("hide".into()) }).expect("filling is allowed");
    let bytes = s.save_bytes(id).unwrap();
    let mut again = Session::new().with_clock(|| 1_800_000_000);
    let reopened = again.open("hiding.pdf", None, bytes, None).unwrap();
    let doc = again.get(reopened).unwrap();
    assert_eq!(doc.info.pages.len(), pages, "{:?}", doc.info.warnings);
    let verdict = saved_verdict(&mut again, reopened);
    assert!(matches!(verdict, Modification::Allowed(_)), "the certification stays valid: {verdict:?}");
}

/// `bytes` certified with DocMDP `level` as they are, signed without opening them in a session
/// first (which lays a dynamic XFA form out and gives a form the values of its XFA data): a form
/// as another program certifies it.
fn certified_as_is(bytes: Vec<u8>, level: u8) -> Arc<Vec<u8>> {
    let cos = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    let opts = SignOptions {
        page: 0,
        rect: Some([20.0, 20.0, 160.0, 50.0]),
        certify: Some(level),
        date: "D:20270115120000Z".into(),
        ..SignOptions::default()
    };
    Arc::new(sign::sign(&cos, &digital_id(), &opts).unwrap())
}

/// The refusal `r` holds for a certification that allows no changes.
fn refused_at_1<T: std::fmt::Debug>(r: Result<T, EditError>) -> &'static str {
    match r {
        Err(EditError::Certified { level: 1, what }) => what,
        other => panic!("expected a refusal for a no-changes certification, got {other:?}"),
    }
}

/// The document's certification reads as signed: nothing was changed since, and its signature
/// is not reported broken.
fn assert_as_signed(doc: &pdfcraft_engine::Document) {
    let cert = doc.signatures.iter().find(|x| x.certify == Some(1)).expect("the certification");
    assert!(matches!(cert.modification, Modification::None), "{:?}", cert.modification);
    assert_ne!(cert.status, sign::Status::Invalid, "{:?}", cert.status);
}

/// A dynamic XFA form certified with no changes allowed before it was ever laid out: opening it
/// shows it laid out from its template, as other viewers show it, but those pages are for display
/// only. Filling it in, its document information and saving or signing it are refused, as each
/// would write the laid-out pages into the signed file; the signed file is untouched.
#[test]
fn a_dynamic_xfa_shell_certified_with_no_changes_is_laid_out_for_display_only() {
    let signed = certified_as_is(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::growable_text_template(false)), 1);
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("shell.pdf", None, signed.clone(), None).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.certification(), Some(1));
    assert_as_signed(doc);
    assert!(doc.xfa.is_some() && doc.info.pages.len() >= 2, "the form is shown laid out: {:?}", doc.info.warnings);
    let value = doc.form.iter().find(|f| f.name == "ACTIVIDADES").map(|f| f.value.clone());
    assert_eq!(value, Some(vec!["Short".to_string()]), "its laid-out field shows the data");
    assert!(doc.info.warnings.iter().any(|w| w.contains("certification allows no changes") && w.contains("laid out")), "{:?}", doc.info.warnings);
    let fill = Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text("Changed".into()) };
    assert_eq!(refused(s.apply(id, fill)), "filling in form fields");
    let title = Edit::SetInfo { key: "Title".into(), value: "T".into() };
    assert_eq!(refused_at_1(s.apply(id, title)), "a change saved with the pages laid out from its XFA template");
    assert_eq!(refused_at_1(s.save_bytes(id)), "saving the pages laid out from its XFA template");
    assert_eq!(refused_at_1(s.save_full_bytes(id)), "saving the pages laid out from its XFA template");
    let opts = SignOptions { page: 0, rect: Some([20.0, 60.0, 160.0, 90.0]), ..SignOptions::default() };
    assert_eq!(refused_at_1(s.sign(id, &digital_id(), opts)), "saving the pages laid out from its XFA template");
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty && doc.can_undo().is_none(), "nothing changed");
    let message = EditError::Certified { level: 1, what: "saving the pages laid out from its XFA template" }.to_string();
    assert!(message.contains("certified with no changes allowed"), "{message}");
}

/// A static XFA form certified with no changes allowed whose XFA data holds a value its field
/// doesn't: opening it shows the data's value, as other viewers do, for display only. Filling it
/// in and saving are refused, so no revision carrying the value is ever written.
#[test]
fn a_static_xfa_form_certified_with_no_changes_shows_its_data_for_display_only() {
    let signed = certified_as_is(pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name></page1></form1>"), 1);
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("static.pdf", None, signed.clone(), None).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.certification(), Some(1));
    let value = doc.form.iter().find(|f| f.name == "form1[0].page1[0].name[0]").map(|f| f.value.clone());
    assert_eq!(value, Some(vec!["Ada".to_string()]), "the field shows the data's value");
    assert_as_signed(doc);
    assert!(doc.info.warnings.iter().any(|w| w.contains("certification allows no changes") && w.contains("XFA data")), "{:?}", doc.info.warnings);
    let fill = Edit::SetFieldValue { name: "form1[0].page1[0].name[0]".into(), value: FieldValue::Text("Grace".into()) };
    assert_eq!(refused(s.apply(id, fill)), "filling in form fields");
    assert_eq!(refused_at_1(s.save_bytes(id)), "saving the field values taken from its XFA data");
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty && doc.can_undo().is_none(), "nothing changed");
}

/// Under a certification that allows filling in, opening synchronizes and lays out an XFA form as
/// any other, and filling it in and saving go ahead.
#[test]
fn an_xfa_form_certified_for_form_fill_is_synchronized_and_filled_as_before() {
    let signed = certified_as_is(pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name></page1></form1>"), 2);
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("static.pdf", None, signed.clone(), None).unwrap();
    let value = s.get(id).unwrap().form.iter().find(|f| f.name == "form1[0].page1[0].name[0]").map(|f| f.value.clone());
    assert_eq!(value, Some(vec!["Ada".to_string()]));
    s.apply(id, Edit::SetFieldValue { name: "form1[0].page1[0].name[0]".into(), value: FieldValue::Text("Grace".into()) })
        .expect("filling in is allowed");
    let verdict = saved_verdict(&mut s, id);
    assert!(matches!(verdict, Modification::Allowed(_)), "the certification stays valid: {verdict:?}");

    let signed = certified_as_is(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::growable_text_template(false)), 2);
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("shell.pdf", None, signed, None).unwrap();
    assert!(s.get(id).unwrap().xfa.is_some(), "laid out");
    s.apply(id, Edit::SetFieldValue { name: "ACTIVIDADES".into(), value: FieldValue::Text("Filled".into()) }).expect("filling in is allowed");
    s.save_bytes(id).expect("saving is allowed");
}
