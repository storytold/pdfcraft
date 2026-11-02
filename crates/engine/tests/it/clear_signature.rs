//! Remove Signature (`sign.clear-signature`) end to end through the session: list, refuse,
//! remove, save, reopen. Every document is synthetic and signed with the test IDs in
//! `crates/sign/tests/data` (they protect nothing).

use std::sync::Arc;

use pdfcraft_cos::Document as Cos;
use pdfcraft_engine::clear_signature::{Blocker, Effect, Kind, Target};
use pdfcraft_engine::sign::{self, Modification};
use pdfcraft_engine::{DocId, Edit, EditError, FieldValue, FillMark, NewAnnotation, Session, Shape, SignOptions, Style};
use pdfcraft_forms::NewField;

/// Two pages of text; the second also draws an image (a "scanned signature" in the content).
fn fixture() -> Vec<u8> {
    let image = [0u8, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0];
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 300 400] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
        stream("BT /F1 18 Tf 20 300 Td (Contract) Tj ET"),
        b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 8 0 R >> >> >>".to_vec(),
        stream("BT /F1 18 Tf 20 300 Td (Signed on paper) Tj ET q 120 0 0 40 20 200 cm /Im1 Do Q"),
        [
            b"<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >>\nstream\n".as_slice(),
            &image,
            b"\nendstream",
        ]
        .concat(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn stream(body: &str) -> Vec<u8> {
    format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes()
}

fn digital_id() -> sign::DigitalId {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    sign::pkcs12::open(&p12, "test").unwrap()
}

fn open() -> (Session, DocId) {
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("contract.pdf", Some("/tmp/contract.pdf".into()), Arc::new(fixture()), None).unwrap();
    (s, id)
}

/// Save what's unsaved, then sign a new field on `page` (signing always writes the file).
fn sign_on(s: &mut Session, id: DocId, page: usize, y: f64, certify: Option<u8>) -> String {
    if s.get(id).unwrap().dirty {
        let saved = s.save_bytes(id).unwrap();
        s.mark_saved(id, saved, None).unwrap();
    }
    let before: Vec<String> = s.get(id).unwrap().signatures.iter().map(|x| x.field.clone()).collect();
    let opts = SignOptions { page, rect: Some([20.0, y, 180.0, y + 40.0]), certify, ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    s.get(id).unwrap().signatures.iter().map(|x| x.field.clone()).find(|f| !before.contains(f)).unwrap()
}

fn page_texts(s: &Session, id: DocId) -> Vec<String> {
    let doc = s.get(id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig::default());
    (0..doc.info.pages.len())
        .map(|page| {
            let req = pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() };
            r.render(req).text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

fn reopen(bytes: Arc<Vec<u8>>) -> Cos {
    Cos::open(bytes).unwrap()
}

fn catalog_has(doc: &Cos, key: &[u8]) -> bool {
    doc.get(doc.root().unwrap()).as_dict().unwrap().contains(key)
}

fn sig_flags(doc: &Cos) -> Option<i64> {
    let root = doc.get(doc.root().unwrap());
    let af = doc.resolve(root.as_dict()?.get(b"AcroForm")?);
    af.as_dict()?.get(b"SigFlags").and_then(|f| doc.resolve(f).as_int())
}

/// How many entries the page's `/Annots` has.
fn page_annots(doc: &Cos, page: usize) -> usize {
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let kids = doc.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().clone();
    let p = doc.get(kids[page].as_ref().unwrap());
    p.as_dict().unwrap().get(b"Annots").map(|a| doc.resolve(a).as_array().map_or(0, Vec::len)).unwrap_or(0)
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}

// Case 1 — a PDF without signatures: nothing is listed and nothing changes.
#[test]
fn a_document_without_signatures_lists_nothing_and_stays_unchanged() {
    let (mut s, id) = open();
    assert!(s.removable_signatures(id).unwrap().is_empty());
    let err = s.apply(id, Edit::ClearSignature { field: "Signature1".into(), remove_field: true }).unwrap_err();
    assert!(matches!(&err, EditError::Sign(m) if m.contains("no signature field")), "{err:?}");
    let doc = s.get(id).unwrap();
    assert!(!doc.dirty && doc.can_undo().is_none());
    assert_eq!(*s.save_bytes(id).unwrap(), fixture(), "the file is written back as it was opened");
}

// Case 2 — one digital signature: the field goes, the rest stays, the result reopens, and a
// clean copy has no trace of it.
#[test]
fn removing_the_only_signature_removes_its_field_and_a_clean_copy_drops_it() {
    let (mut s, id) = open();
    let texts = page_texts(&s, id);
    let field = sign_on(&mut s, id, 0, 40.0, None);
    let list = s.removable_signatures(id).unwrap();
    assert_eq!(list.len(), 1);
    let r = &list[0];
    assert_eq!((&r.kind, &r.target, r.page, r.revision), (&Kind::Digital, &Target::Field(field.clone()), Some(0), 2));
    assert_eq!(r.signer.as_deref(), Some("Test Signer EC"));
    assert!(r.blocked.is_none() && r.effects.is_empty(), "{r:?}");

    s.apply(id, Edit::ClearSignature { field: field.clone(), remove_field: true }).unwrap();
    let doc = s.get(id).unwrap();
    assert!(doc.signatures.is_empty() && doc.form.is_empty() && !doc.is_signed());
    assert_eq!(page_annots(&reopen(s.save_bytes(id).unwrap()), 0), 0, "the widget left its page");
    assert_eq!(doc.info.pages.len(), 2);
    assert_eq!(page_texts(&s, id), texts, "page content is untouched");
    assert_eq!(doc.can_undo(), Some("Remove the signature Signature1"));

    // Saved in place it is incremental, so the signed revision is still inside the file.
    let incremental = s.save_bytes(id).unwrap();
    s.check_save(id, &incremental).unwrap();
    assert!(contains(&incremental, b"/ByteRange"), "an incremental save keeps the earlier, signed revision");
    // With no signature left, a full rewrite is allowed and leaves the old revision out.
    let clean = s.save_full_bytes(id).unwrap();
    s.check_save(id, &clean).unwrap();
    assert!(!contains(&clean, b"/ByteRange") && !contains(&clean, b"/Sig"), "the clean copy has no signature left");
    let cos = reopen(clean.clone());
    assert!(!catalog_has(&cos, b"Perms"));
    assert_eq!(sig_flags(&cos), None, "no signature field is left, so no flag holds");
    let mut s2 = Session::new();
    let id2 = s2.open("unsigned.pdf", None, clean, None).unwrap();
    assert!(s2.get(id2).unwrap().signatures.is_empty());
    assert_eq!(page_texts(&s2, id2), texts);
}

// Clearing without removing the field leaves an empty signature field that can be signed again.
#[test]
fn clearing_keeps_an_empty_field_that_can_be_signed_again() {
    let (mut s, id) = open();
    let field = sign_on(&mut s, id, 0, 40.0, None);
    s.apply(id, Edit::ClearSignature { field: field.clone(), remove_field: false }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.signatures.len(), 1);
    assert!(!doc.signatures[0].signed && doc.signatures[0].field == field);
    assert!(s.removable_signatures(id).unwrap().is_empty(), "an empty field has no signature to remove");
    let cos = reopen(s.save_full_bytes(id).unwrap());
    assert_eq!(sig_flags(&cos), Some(1), "the field exists, nothing is signed");

    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    let opts = SignOptions { field: Some(field.clone()), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id(), opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    let sig = &s.get(id).unwrap().signatures[0];
    assert!(sig.signed && sig.field == field && sig.modification == Modification::None, "{sig:?}");
}

// Case 3 and 10 — two signatures: only the chosen one goes, the other is told about and keeps
// reporting the change; the saved file still holds the removed one in its earlier revision.
#[test]
fn with_two_signatures_only_the_chosen_one_goes_and_the_other_reports_the_change() {
    let (mut s, id) = open();
    let first = sign_on(&mut s, id, 0, 40.0, None);
    let second = sign_on(&mut s, id, 1, 40.0, None);
    let list = s.removable_signatures(id).unwrap();
    let of = |f: &str| list.iter().find(|r| r.field.as_deref() == Some(f)).unwrap().clone();
    let a = of(&first);
    assert!(a.effects.contains(&Effect::OtherSignatures(vec![second.clone()])));
    assert!(a.effects.contains(&Effect::SignedAfter(vec![second.clone()])), "the second signature covers the first");
    assert!(a.effects.contains(&Effect::EarlierRevisionKept));
    let b = of(&second);
    assert!(b.effects.contains(&Effect::OtherSignatures(vec![first.clone()])));
    assert!(!b.effects.iter().any(|e| matches!(e, Effect::SignedAfter(_))));
    let signed_by_second = s.get(id).unwrap().bytes.clone();

    s.apply(id, Edit::ClearSignature { field: second.clone(), remove_field: true }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.signatures.iter().map(|x| x.field.as_str()).collect::<Vec<_>>(), [first.as_str()]);
    assert!(doc.signatures[0].signed);
    assert_ne!(doc.signatures[0].modification, Modification::None, "the remaining signature sees the change");
    assert_eq!(doc.form.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), [first.as_str()]);
    // A signature remains, so no full rewrite: the save appends, after the removed signature's revision.
    assert_eq!(s.save_full_bytes(id).unwrap_err(), EditError::Signed);
    let saved = s.save_bytes(id).unwrap();
    assert!(saved.starts_with(&signed_by_second), "the removed signature is still recoverable from the earlier revision");
    assert_eq!(sig_flags(&reopen(saved)), Some(3), "a signature remains");
}

// Case 4 — form fields that aren't the signature keep their values.
#[test]
fn other_form_fields_keep_their_values() {
    let (mut s, id) = open();
    let add = |kind: NewField, y: f64, name: &str| Edit::AddField { page: 0, rect: [20.0, y, 180.0, y + 22.0], kind, name: Some(name.into()) };
    s.apply(id, add(NewField::Text { multiline: false }, 250.0, "name")).unwrap();
    s.apply(id, add(NewField::CheckBox, 200.0, "agree")).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "name".into(), value: FieldValue::Text("Ada".into()) }).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "agree".into(), value: FieldValue::Check(true) }).unwrap();
    let field = sign_on(&mut s, id, 0, 40.0, None);
    s.apply(id, Edit::ClearSignature { field, remove_field: true }).unwrap();
    let saved = s.save_full_bytes(id).unwrap();
    let mut s2 = Session::new();
    let id2 = s2.open("f.pdf", None, saved, None).unwrap();
    let form = &s2.get(id2).unwrap().form;
    assert_eq!(
        form.iter().map(|f| (f.name.as_str(), f.value.clone())).collect::<Vec<_>>(),
        [("name", vec!["Ada".to_string()]), ("agree", vec!["Yes".to_string()])]
    );
    assert_eq!(page_annots(&reopen(s2.get(id2).unwrap().bytes.clone()), 0), 2, "both widgets are still on the page");
}

// Case 5 — a Fill & Sign signature is listed as visual; the other marks and comments are not.
#[test]
fn only_fill_and_sign_signatures_are_listed_as_visual_and_only_the_chosen_one_goes() {
    let (mut s, id) = open();
    let typed = Shape::TypedSignature { rect: [90.0, 40.0, 150.0, 70.0], contours: vec![vec![[0.1, 0.2], [0.5, 0.9], [0.9, 0.2]]] };
    let add = |shape: Shape, contents: &str| {
        let style = Style::default_for(&shape);
        Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: contents.into(), author: "Ada".into() })
    };
    s.apply(id, add(Shape::Mark { rect: [20.0, 100.0, 30.0, 110.0], mark: FillMark::Check }, "")).unwrap();
    s.apply(id, add(typed, "")).unwrap();
    s.apply(id, add(Shape::Rectangle { rect: [200.0, 200.0, 250.0, 250.0] }, "Look here")).unwrap();
    let list = s.removable_signatures(id).unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!((&list[0].kind, &list[0].target), (&Kind::Visual { initials: false }, &Target::Annotation { page: 0, index: 1 }));
    let Target::Annotation { page, index } = list[0].target else { unreachable!() };
    s.apply(id, Edit::DeleteAnnotation { page, index }).unwrap();
    let left: Vec<String> = s.get(id).unwrap().info.annotations.iter().map(|a| a.subtype.clone()).collect();
    assert_eq!(left.len(), 2, "the check mark and the comment stay: {left:?}");
    assert!(s.removable_signatures(id).unwrap().is_empty());
}

// Case 6 — a signature drawn into the page content is never listed, and nothing on the page is
// erased.
#[test]
fn a_signature_in_the_page_content_is_not_listed_and_the_page_is_untouched() {
    let (mut s, id) = open();
    assert!(s.removable_signatures(id).unwrap().is_empty());
    let field = sign_on(&mut s, id, 0, 40.0, None);
    s.apply(id, Edit::ClearSignature { field, remove_field: true }).unwrap();
    let clean = s.save_full_bytes(id).unwrap();
    assert!(contains(&clean, b"q 120 0 0 40 20 200 cm /Im1 Do Q"), "the drawn image is still in the page's content");
    let cos = reopen(clean);
    assert!(cos.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Image"))));
}

// Case 7 — a damaged or foreign file fails with an error, and never panics.
#[test]
fn a_damaged_file_fails_with_an_error() {
    let mut s = Session::new();
    assert!(s.open("broken.pdf", None, Arc::new(b"%PDF-1.7\nnot a pdf at all".to_vec()), None).is_err());
    let (s, id) = open();
    let bogus = DocId(u64::MAX);
    assert!(s.removable_signatures(bogus).is_err());
    assert!(s.removable_signatures(id).unwrap().is_empty());
    // Bytes that don't reopen are refused before anything is written.
    assert!(s.check_save(id, &Arc::new(b"%PDF-1.7\ntruncated".to_vec())).is_err());
}

// Case 9 — a certification: the certified document's later signature can't be removed (the
// certification doesn't permit it); the certification itself can, and says so.
#[test]
fn a_certification_blocks_removing_other_signatures_and_ends_when_removed() {
    let (mut s, id) = open();
    let certification = sign_on(&mut s, id, 0, 40.0, Some(2));
    let approval = sign_on(&mut s, id, 1, 40.0, None);
    let list = s.removable_signatures(id).unwrap();
    let of = |f: &str| list.iter().find(|r| r.field.as_deref() == Some(f)).unwrap().clone();
    assert_eq!(of(&approval).blocked, Some(Blocker::CertifiedBy(certification.clone())));
    let c = of(&certification);
    assert_eq!((c.kind.clone(), c.blocked.clone()), (Kind::Certification(2), None));
    assert!(c.effects.contains(&Effect::EndsCertification(2)));

    let before = s.get(id).unwrap().bytes.clone();
    let err = s.apply(id, Edit::ClearSignature { field: approval.clone(), remove_field: true }).unwrap_err();
    assert!(matches!(&err, EditError::Sign(m) if m.contains("certifies this document")), "{err:?}");
    assert!(!s.get(id).unwrap().dirty && Arc::ptr_eq(&s.get(id).unwrap().bytes, &before), "a refused removal changes nothing");

    s.apply(id, Edit::ClearSignature { field: certification.clone(), remove_field: true }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let cos = reopen(saved);
    let root = cos.get(cos.root().unwrap());
    let perms = root.as_dict().unwrap().get(b"Perms").map(|p| cos.resolve(p));
    assert!(perms.is_none_or(|p| !p.as_dict().unwrap().contains(b"DocMDP")), "no longer certified");
    let doc = s.get(id).unwrap();
    assert_eq!(doc.signatures.iter().map(|x| x.field.as_str()).collect::<Vec<_>>(), [approval.as_str()]);
}

// Undo brings the signature back exactly.
#[test]
fn undo_brings_the_signature_back() {
    let (mut s, id) = open();
    let field = sign_on(&mut s, id, 0, 40.0, None);
    let signed = s.get(id).unwrap().signatures[0].clone();
    s.apply(id, Edit::ClearSignature { field, remove_field: true }).unwrap();
    s.undo(id).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.signatures.len(), 1);
    assert_eq!((doc.signatures[0].status, &doc.signatures[0].modification), (signed.status, &signed.modification));
    assert!(!doc.dirty);
}
