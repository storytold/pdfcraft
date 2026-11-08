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

/// The surviving form draws the image through a nested form without /Resources of its own,
/// which draws through the outer form's: the image is kept.
#[test]
fn an_image_a_surviving_forms_nested_form_draws_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        b"<< /XObject << /Im 6 0 R /Fm 7 0 R /Inner 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 5 0 R", b"/Inner Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"IMAGESECRET"), "the image the nested form still draws outside the mark");
}

/// Two pages share one resource dictionary, and the second draws the image only through a form
/// without /Resources: redacting the image on the first page keeps it for the second.
#[test]
fn an_image_another_page_draws_through_a_form_without_resources_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources 7 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 6 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q"),
        stream("", b"/Inner Do"),
        b"<< /XObject << /Im 8 0 R /Inner 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"IMAGESECRET"), "the second page still draws the image through its form");
}

/// A Type 3 font whose displayed glyphs are redacted has its unused glyph programs unlinked from
/// /CharProcs and exclusive graphical resources pruned, so they do not survive in a full save (F-011).
#[test]
fn redacted_type3_glyph_and_exclusive_resources_do_not_survive_full_save() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        // Text showing AB with Type 3 font /T3
        stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        // Type 3 font with /CharProcs << /A 7 0 R /B 8 0 R >> and /Resources with images for both
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /ImA 9 0 R /ImB 10 0 R >> >> >>".to_vec(),
        stream("", b"/ImA Do % SECRET_GLYPH_A_PAYLOAD"),
        stream("", b"/ImB Do % SURVIVING_GLYPH_B_PAYLOAD"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_A"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBLIC_IMAGE_B"),
    ]);

    // Redaction mark covers only 'A' (x: 10..15, y: 95..110)
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");

    assert!(full_save_holds(&doc, b"SECRET_GLYPH_A_PAYLOAD"));
    assert!(full_save_holds(&doc, b"CONFIDENTIAL_IMAGE_A"));
    assert!(full_save_holds(&doc, b"SURVIVING_GLYPH_B_PAYLOAD"));
    assert!(full_save_holds(&doc, b"PUBLIC_IMAGE_B"));

    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.glyphs, 1);

    // After redaction, unreferenced glyph A and its exclusive image A must not survive full save
    assert!(!full_save_holds(&doc, b"SECRET_GLYPH_A_PAYLOAD"), "redacted glyph A program survived");
    assert!(!full_save_holds(&doc, b"CONFIDENTIAL_IMAGE_A"), "exclusive image for redacted glyph A survived");

    // Surviving glyph B and its image B must survive
    assert!(full_save_holds(&doc, b"SURVIVING_GLYPH_B_PAYLOAD"), "surviving glyph B was lost");
    assert!(full_save_holds(&doc, b"PUBLIC_IMAGE_B"), "surviving glyph B image was lost");

    // Reopen and inspect the Type 3 font dictionary
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let res = saved.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let fonts = saved.resolve(res.get(b"Font").unwrap()).as_dict().unwrap().clone();
    let t3 = saved.resolve(fonts.get(b"T3").unwrap()).as_dict().unwrap().clone();
    let procs = saved.resolve(t3.get(b"CharProcs").unwrap()).as_dict().unwrap().clone();
    assert!(!procs.contains(b"A"), "CharProcs still contains unreferenced glyph A");
    assert!(procs.contains(b"B"), "CharProcs lost surviving glyph B");

    let t3_res = saved.resolve(t3.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let t3_xo = saved.resolve(t3_res.get(b"XObject").unwrap()).as_dict().unwrap().clone();
    assert!(!t3_xo.contains(b"ImA"), "Type 3 font resources still contains exclusive image ImA");
    assert!(t3_xo.contains(b"ImB"), "Type 3 font resources lost surviving image ImB");
}

/// When all glyphs of a Type 3 font are redacted, the entire font is removed from Resources (F-011).
#[test]
fn all_redacted_type3_glyphs_cause_font_removal_from_resources() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 7 0 R >> /Resources << /XObject << /ImA 8 0 R >> >> >>".to_vec(),
        stream("", b"/ImA Do % SECRET_GLYPH_ONLY_PAYLOAD"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_ONLY"),
    ]);

    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");

    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.glyphs, 1);

    assert!(!full_save_holds(&doc, b"SECRET_GLYPH_ONLY_PAYLOAD"));
    assert!(!full_save_holds(&doc, b"CONFIDENTIAL_IMAGE_ONLY"));

    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    if let Some(res_val) = page.dict.get(b"Resources")
        && let Some(res) = saved.resolve(res_val).as_dict()
        && let Some(fonts_val) = res.get(b"Font")
    {
        let fonts = saved.resolve(fonts_val).as_dict().unwrap().clone();
        assert!(!fonts.contains(b"T3"), "Font /T3 was not removed from Resources");
    }
}

/// In shared Type 3 fonts across multiple pages, glyphs used on another page are preserved (F-011).
#[test]
fn shared_type3_font_preserves_glyphs_used_by_another_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        // Page 1: draws 'A' (redacted)
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources 7 0 R >>".to_vec(),
        // Page 2: draws 'B' (survives)
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 6 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET"),
        stream("", b"BT /T3 10 Tf 10 100 Td (B) Tj ET"),
        // Shared Resources 7 0 R
        b"<< /Font << /T3 8 0 R >> >>".to_vec(),
        // Shared Type 3 font 8 0 R
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 9 0 R /B 10 0 R >> >>".to_vec(),
        stream("", b"% SECRET_PAGE1_GLYPH_A"),
        stream("", b"% PUBLIC_PAGE2_GLYPH_B"),
    ]);

    // Redact only Page 1's 'A'
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");

    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.glyphs, 1);

    // Glyph A was removed from Page 1 and used nowhere else -> unlinked!
    assert!(!full_save_holds(&doc, b"SECRET_PAGE1_GLYPH_A"));

    // Glyph B is still used on Page 2 -> kept!
    assert!(full_save_holds(&doc, b"PUBLIC_PAGE2_GLYPH_B"));

    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let pages = pdfcraft_model::pages(&saved);
    let p2_res = saved.resolve(pages[1].dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    let fonts = saved.resolve(p2_res.get(b"Font").unwrap()).as_dict().unwrap().clone();
    let t3 = saved.resolve(fonts.get(b"T3").unwrap()).as_dict().unwrap().clone();
    let procs = saved.resolve(t3.get(b"CharProcs").unwrap()).as_dict().unwrap().clone();
    assert!(!procs.contains(b"A"));
    assert!(procs.contains(b"B"));
}

/// Fails closed when Type 3 font cleanup cannot be safely established (F-011).
#[test]
fn unparseable_type3_font_encoding_fails_closed() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        // Unsupported encoding /UnknownCustomEncoding without Differences
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding /UnknownCustomEncoding /CharProcs << /A 7 0 R >> >>".to_vec(),
        stream("", b"% SECRET_PAYLOAD_UNPARSEABLE"),
    ]);

    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");

    let res = apply(&mut doc, None);
    assert!(res.is_err(), "redaction must fail closed when Type 3 cleanup cannot be established: {res:?}");
}

/// A Type 3 font selected through an ExtGState /Font entry is a use of the font for pruning:
/// its redacted glyph program does not survive a full save (F-011).
#[test]
fn type3_font_set_through_extgstate_has_its_redacted_glyph_removed() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"/GS1 gs BT 10 100 Td (A) Tj ET"),
        b"<< /Font << /T3 6 0 R >> /ExtGState << /GS1 8 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 7 0 R >> >>".to_vec(),
        stream("", b"% SECRET_EXTGSTATE_GLYPH_PAYLOAD"),
        b"<< /Type /ExtGState /Font [6 0 R 10] >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.glyphs, 1);
    assert!(!full_save_holds(&doc, b"SECRET_EXTGSTATE_GLYPH_PAYLOAD"), "redacted glyph program survived");
}

/// A Type 3 font used only inside a Form XObject is found by the scan and its redacted glyph
/// program does not survive a full save (F-011).
#[test]
fn type3_font_used_in_a_form_xobject_has_its_redacted_glyph_removed() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"/Fm Do"),
        b"<< /XObject << /Fm 8 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 7 0 R >> >>".to_vec(),
        stream("", b"% SECRET_FORM_GLYPH_PAYLOAD"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 9 0 R", b"BT /T3 10 Tf 10 100 Td (A) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.glyphs, 1);
    assert!(!full_save_holds(&doc, b"SECRET_FORM_GLYPH_PAYLOAD"), "redacted glyph program survived");
}
