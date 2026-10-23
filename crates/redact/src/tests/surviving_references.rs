//! Redacted content must leave the file: no other reference may keep it reachable by a full save.

use super::*;

/// Whether `secret` survives a full save of `doc`, in the file's bytes or in a decoded stream.
pub(super) fn full_save_holds(doc: &Document, secret: &[u8]) -> bool {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let found = |data: &[u8]| data.windows(secret.len()).any(|w| w == secret);
    if found(&bytes) {
        return true;
    }
    let saved = Document::open(Arc::new(bytes)).unwrap();
    saved.scan_objects().into_iter().any(|(_, o)| matches!(&*o, Object::Stream(s) if s.decoded().is_ok_and(|d| found(&d))))
}

#[test]
fn a_redacted_field_listed_in_the_calculation_order_leaves_the_saved_file() {
    let mut doc = one_page(b"", "", vec![]);
    pdfcraft_forms::add_field(&mut doc, 0, [10.0, 50.0, 100.0, 70.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("ssn")).unwrap();
    pdfcraft_forms::set_value(&mut doc, "ssn", &pdfcraft_forms::FieldValue::Text("SSN-078-05-1120".into())).unwrap();
    let field = pdfcraft_forms::fields(&doc).swap_remove(0).obj;
    let root = doc.root().unwrap();
    let af = doc.get(root).as_dict().unwrap().reference(b"AcroForm").unwrap();
    doc.update_dict(af, |d| d.set(b"CO".to_vec(), Object::Array(vec![Object::Ref(field)]))).unwrap();
    mark(&mut doc, 0, &[[0.0, 40.0, 120.0, 80.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().fields, 1);
    assert!(!full_save_holds(&doc, b"SSN-078-05-1120"), "the field's value is still reachable");
}

/// A surviving form shares the page's resource dictionary, which lists the image the mark
/// removes; the form never draws it.
#[test]
fn a_redacted_image_does_not_survive_in_a_surviving_forms_resources() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Font << /F1 5 0 R >> /XObject << /Im 8 0 R /Fm 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 7 0 R", b"BT /F1 10 Tf 10 200 Td (Public) Tj ET"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(content(&doc, 0).contains("Do"), "the form is still drawn");
    assert!(!full_save_holds(&doc, b"IMAGESECRET"), "the removed image is still reachable");
}

/// The same form drawing the image itself keeps it: the page still shows it there.
#[test]
fn an_image_a_surviving_form_draws_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Font << /F1 5 0 R >> /XObject << /Im 8 0 R /Fm 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 7 0 R", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"IMAGESECRET"), "the image the form still draws outside the mark");
}
