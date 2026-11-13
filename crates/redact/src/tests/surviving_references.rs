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

/// The surviving glyph B draws a Form whose own resources also name A's image, which the Form
/// never draws. Pruning only the font's resources left that entry, so the image of the removed
/// glyph stayed in the file through the Form.
#[test]
fn a_redacted_type3_glyphs_image_does_not_survive_in_a_surviving_glyphs_form() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /ImA 9 0 R /Fm 10 0 R >> >> >>".to_vec(),
        stream("", b"/ImA Do"),
        stream("", b"/Fm Do"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_A"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 1000 1000] /Resources << /XObject << /ImA 9 0 R /ImB 11 0 R >> >>", b"/ImB Do"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBLIC_IMAGE_B"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"CONFIDENTIAL_IMAGE_A"), "the removed glyph's image survives through the surviving glyph's form");
    assert!(full_save_holds(&doc, b"PUBLIC_IMAGE_B"), "the image the surviving glyph's form draws was lost");
}

/// The same Form, its content behind a predictor whose `/DecodeParms` are given by reference: the
/// cleanup of its resources reads it through them, and keeps the image it draws. Read without
/// them, this content's bytes are all operators that use no resource, so nothing refuses and
/// the image would go.
#[test]
fn an_image_a_type3_glyphs_form_with_parameters_by_reference_draws_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /ImA 9 0 R /Fm 10 0 R >> >> >>".to_vec(),
        stream("", b"/ImA Do"),
        stream("", b"/Fm Do"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_A"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 1000 1000]", b""),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBLIC_IMAGE_B"),
    ]);
    let mut entries = super::form_entries([0, 0, 1000, 1000]);
    entries.push((
        "Resources",
        Object::Dict(Dict::from_iter([(
            b"XObject".to_vec(),
            Object::Dict(Dict::from_iter([(b"ImA".to_vec(), Object::Ref(ObjRef::new(9, 0))), (b"ImB".to_vec(), Object::Ref(ObjRef::new(11, 0)))])),
        )])),
    ));
    let form = super::behind_parms_by_reference(&mut doc, &entries, b"q 1000 0 0 1000 0 0 cm /ImB Do Q");
    doc.set(ObjRef::new(10, 0), form);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"CONFIDENTIAL_IMAGE_A"), "the removed glyph's image survives through the surviving glyph's form");
    assert!(full_save_holds(&doc, b"PUBLIC_IMAGE_B"), "the image the surviving glyph's form draws was lost");
}

/// The Form a kept glyph draws is walked, for the names its content uses, within the allowance of
/// a page: two forms without resources of their own that it draws, each past half of it, refuse
/// the redaction rather than being decoded in full. Two small ones are the control.
#[test]
fn the_form_a_kept_type3_glyph_draws_is_walked_within_the_allowance_of_a_page() {
    for (size, refused) in [(pdfcraft_cos::MAX_PAGE_DECODED / 2 + 1, true), (1, false)] {
        let packed = pdfcraft_filters::encode_flate(&vec![b' '; size]);
        let blank = || stream("/Type /XObject /Subtype /Form /BBox [0 0 1000 1000] /Filter /FlateDecode", packed.as_slice());
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
            b"<< /Font << /T3 6 0 R >> >>".to_vec(),
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /ImA 9 0 R /Fm 10 0 R >> >> >>".to_vec(),
            stream("", b"/ImA Do"),
            stream("", b"/Fm Do"),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_A"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 1000 1000] /Resources << /XObject << /Sub1 11 0 R /Sub2 12 0 R >> >>", b"/Sub1 Do /Sub2 Do"),
            blank(),
            blank(),
        ]);
        mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
        let out = apply(&mut doc, None);
        if refused {
            assert!(matches!(out, Err(RedactError::Cos(pdfcraft_cos::CosError::Filter(_)))), "forms of {size} bytes each: {out:?}");
        } else {
            assert_eq!(out.unwrap().glyphs, 1, "forms of {size} bytes each");
        }
    }
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

/// Type 3 cleanup decoded a glyph program again for every font listing it: 2 000 fonts sharing
/// one large program read it 2 000 times. It is read once now, and the cleanup still keeps the
/// glyph every font shows.
#[test]
fn a_glyph_program_many_type3_fonts_share_is_read_once() {
    const FONTS: usize = 2_000;
    let names: String = (0..FONTS).map(|i| format!("/T{i} {} 0 R ", 6 + i)).collect();
    let shows: String = (0..FONTS).map(|i| format!("/T{i} 10 Tf (A) Tj ")).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << {names}>> >> >>").into_bytes(),
        stream("", format!("BT 10 100 Td {shows}ET").as_bytes()),
        stream("", [b"SHARED_GLYPH ".as_slice(), &b"0 0 m 1 1 l S ".repeat(50_000)].concat()),
    ];
    objs.extend((0..FONTS).map(|_| {
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [0] /Encoding << /Differences [65 /A] >> /CharProcs << /A 5 0 R >> >>".to_vec()
    }));
    let mut doc = pdf(objs);
    // The glyphs are drawn at x 10 with no width, so a mark away from them leaves them all.
    mark(&mut doc, 0, &[[200.0, 200.0, 250.0, 250.0]], "");
    let started = std::time::Instant::now();
    apply(&mut doc, None).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    let font = doc.get(ObjRef::new(6, 0));
    let procs = doc.resolve(font.as_dict().unwrap().get(b"CharProcs").unwrap());
    assert!(procs.as_dict().unwrap().contains(b"A"), "the glyph every font shows was unlinked");
}

/// A Type 3 font with glyphs A (its program object 7) and B (object 8), at object 6.
const TYPE3: &str = "<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> >>";

/// A form outside the mark shows glyph B of a Type 3 font, its content behind a predictor whose
/// `/DecodeParms` are given by reference: the font's cleanup reads it through them, and keeps B.
#[test]
fn a_type3_glyph_a_form_with_parameters_by_reference_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET /Fm Do"),
        b"<< /Font << /T3 6 0 R >> /XObject << /Fm 9 0 R >> >>".to_vec(),
        TYPE3.as_bytes().to_vec(),
        stream("", b"% SECRET_TYPE3_GLYPH_A"),
        stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50]", b""),
    ]);
    let mut entries = super::form_entries([0, 0, 300, 50]);
    entries.push(("Resources", Object::Ref(ObjRef::new(5, 0))));
    let form = super::behind_parms_by_reference(&mut doc, &entries, b"BT /T3 10 Tf 10 20 Td (B) Tj ET");
    doc.set(ObjRef::new(9, 0), form);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"SECRET_TYPE3_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B"), "the form still shows B");
}

/// A form the page lists but doesn't draw shows glyph B, its content behind a predictor whose
/// `/DecodeParms` are given by reference: the font's cleanup reads every surviving form, this one
/// through them, and keeps B.
#[test]
fn a_type3_glyph_an_undrawn_form_with_parameters_by_reference_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET"),
        b"<< /Font << /T3 6 0 R >> /XObject << /Fm 9 0 R >> >>".to_vec(),
        TYPE3.as_bytes().to_vec(),
        stream("", b"% SECRET_TYPE3_GLYPH_A"),
        stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50]", b""),
    ]);
    let mut entries = super::form_entries([0, 0, 300, 50]);
    entries.push((
        "Resources",
        Object::Dict(Dict::from_iter([(b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"T3".to_vec(), Object::Ref(ObjRef::new(6, 0)))])))])),
    ));
    let form = super::behind_parms_by_reference(&mut doc, &entries, b"BT /T3 10 Tf 10 20 Td (B) Tj ET");
    doc.set(ObjRef::new(9, 0), form);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"SECRET_TYPE3_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B"), "the undrawn form still shows B");
}

/// Glyph B's program draws an image from the font's resources, behind a predictor whose
/// `/DecodeParms` are given by reference: the font's cleanup reads it through them, and keeps the
/// image B still draws while the one only the redacted A drew goes.
#[test]
fn an_image_a_type3_glyph_with_parameters_by_reference_draws_is_kept() {
    let image = "/Type /XObject /Subtype /Image /Width 12 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let font = TYPE3.replace("/CharProcs", "/Resources << /XObject << /ImA 9 0 R /ImB 10 0 R >> >> /CharProcs");
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj 0 100 Td (B) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        font.into_bytes(),
        stream("", b"/ImA Do"),
        stream("", b""),
        stream(image, b"SECRET_IMG_A"),
        stream(image, b"PUBLIC_IMG_B"),
    ]);
    let glyph = super::behind_parms_by_reference(&mut doc, &[], b"/ImB Do");
    doc.set(ObjRef::new(8, 0), glyph);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"SECRET_IMG_A"), "the image only the redacted glyph drew survived");
    assert!(full_save_holds(&doc, b"PUBLIC_IMG_B"), "glyph B still draws its image");
}

/// Whether `out` is the redaction refused with a syntax error whose detail holds `detail`.
fn refused_for(out: &Result<Report, RedactError>, detail: &str) -> bool {
    matches!(out, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail: d, .. })) if d.contains(detail))
}

/// The font's cleanup scans every page and form for the glyphs they show. A string left open in a
/// form nothing else reads (resources of its own, outside the mark) runs to the end of the
/// stream, hiding the glyph the form shows after it, so the redaction is refused: with the check
/// off, that glyph loses its program.
#[test]
fn unreadable_bytes_in_a_form_the_type3_cleanup_scans_refuse_the_redaction() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET /Fm Do"),
        b"<< /Font << /T3 6 0 R >> /XObject << /Fm 9 0 R >> >>".to_vec(),
        TYPE3.as_bytes().to_vec(),
        stream("", b"% SECRET_TYPE3_GLYPH_A"),
        stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /Font << /T3 6 0 R >> >>", b"BT /T3 10 Tf 10 20 Td ( (B) Tj ET"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    let out = apply(&mut doc, None);
    assert!(refused_for(&out, "content stream syntax"), "{out:?}");
}

/// The font's cleanup reads the program of each glyph it keeps, for the resources it uses. A
/// string left open there runs to the end of the program, hiding the image it draws after it, so
/// the redaction is refused: with the check off, the font's resources lose that image.
#[test]
fn unreadable_bytes_in_a_kept_type3_glyph_program_refuse_the_redaction() {
    let font = TYPE3.replace("/CharProcs", "/Resources << /XObject << /ImB 9 0 R >> >> /CharProcs");
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj 0 100 Td (B) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        font.into_bytes(),
        stream("", b"% SECRET_TYPE3_GLYPH_A"),
        stream("", b"500 0 d0 ( /ImB Do"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"o"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    let out = apply(&mut doc, None);
    assert!(refused_for(&out, "glyph stream syntax"), "{out:?}");
}

/// A Form the surviving glyph draws holds `/Resources` as an array that references the removed
/// glyph's image: it can't be pruned, so the redaction refuses rather than keep the image.
#[test]
fn a_type3_glyphs_form_with_non_dictionary_resources_fails_closed() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /ImA 9 0 R /Fm 10 0 R >> >> >>".to_vec(),
        stream("", b"/ImA Do"),
        stream("", b"/Fm Do"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"CONFIDENTIAL_IMAGE_A"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 1000 1000] /Resources [9 0 R]", b""),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert!(apply(&mut doc, None).is_err());
}

/// A Type 3 font with glyphs A, B and C, whose programs are objects `a`, `a + 1` and `a + 2`.
fn type3_abc(a: u32) -> Vec<u8> {
    let (b, c) = (a + 1, a + 2);
    format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 67 /Widths [500 500 500] /Encoding << /Differences [65 /A /B /C] >> /CharProcs << /A {a} 0 R /B {b} 0 R /C {c} 0 R >> >>"
    )
    .into_bytes()
}

/// An annotation's appearance shows glyph B of a Type 3 font the page lists too. The appearance
/// still draws it, so its program is kept; the redacted glyph A goes.
#[test]
fn a_type3_glyph_an_annotation_appearance_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        type3_abc(9),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /DA (/T3 10 Tf 0 g) /AP << /N 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /T3 6 0 R >> >>", b"BT /T3 10 Tf 2 5 Td (B) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
    assert!(full_save_holds(&doc, b"PAGE_GLYPH_C"), "the glyph the page still shows was unlinked");
}

/// The same annotation also has an `/AP` entry no reader draws (`/X`), which shows the redacted
/// glyph A. It keeps nothing: A's program goes, and B, which the normal appearance shows, stays.
#[test]
fn a_type3_glyph_only_an_undrawn_appearance_entry_shows_is_unlinked() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        type3_abc(10),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /DA (/T3 10 Tf 0 g) /AP << /N 8 0 R /X 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /T3 6 0 R >> >>", b"BT /T3 10 Tf 2 5 Td (B) Tj ET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /T3 6 0 R >> >>", b"BT /T3 10 Tf 2 5 Td (A) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "an undrawn appearance entry kept the redacted glyph program");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the normal appearance shows was unlinked");
    assert!(full_save_holds(&doc, b"PAGE_GLYPH_C"), "the glyph the page still shows was unlinked");
}

/// An appearance without /Resources shows glyph B of a second Type 3 font, which the page lists
/// but its content never selects. The appearance draws through the page's resources, so the
/// rewritten page still lists that font and B's program stays.
#[test]
fn a_type3_font_only_an_appearance_without_resources_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [8 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> >>".to_vec(),
        type3_abc(10),
        type3_abc(13),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /DA (/T4 10 Tf 0 g) /AP << /N 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20]", b"BT /T4 10 Tf 2 5 Td (B) Tj ET"),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the appearance selects");
}

/// The same appearance, its content behind a predictor whose `/DecodeParms` are given by
/// reference: the page's cleanup reads it through them, and keeps the font it selects.
#[test]
fn a_type3_font_only_an_appearance_with_parameters_by_reference_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [8 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> >>".to_vec(),
        type3_abc(10),
        type3_abc(13),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /DA (/T4 10 Tf 0 g) /AP << /N 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20]", b""),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% APPEARANCE_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    let appearance = super::behind_parms_by_reference(&mut doc, &super::form_entries([0, 0, 100, 20]), b"BT /T4 10 Tf 2 5 Td (B) Tj ET");
    doc.set(ObjRef::new(9, 0), appearance);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"APPEARANCE_GLYPH_B"), "the glyph the appearance shows was unlinked");
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the appearance selects");
}

/// The operators of a page's appearances count toward one total, as the content's do. Two
/// appearances under the cap alone pass it together, and the redaction is refused rather than
/// judged on part of what draws through the page's fonts.
#[test]
fn a_page_s_appearances_share_one_operator_total() {
    let big = || stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20]", b"n\n".repeat(1_100_000));
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R 8 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        type3_abc(11),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /AP << /N 9 0 R >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 240 250 260] /P 3 0 R /AP << /N 10 0 R >> >>".to_vec(),
        big(),
        big(),
        stream("", b"% GLYPH_A"),
        stream("", b"% GLYPH_B"),
        stream("", b"% GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    let refused = apply(&mut doc, None);
    assert!(refused.is_err(), "two appearances over the operator cap together were judged in part: {refused:?}");
}

/// Their decoded bytes share one page's allowance too. Two compressed appearances, each under it
/// but past it together (and blank, so far from the operator cap), refuse the redaction rather
/// than being decoded and held in full.
#[test]
fn a_page_s_appearances_share_one_decoded_byte_allowance() {
    let packed = pdfcraft_filters::encode_flate(&vec![b' '; pdfcraft_cos::MAX_PAGE_DECODED / 2 + 1]);
    let big = || stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Filter /FlateDecode", packed.as_slice());
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R 8 0 R] >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        type3_abc(11),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /AP << /N 9 0 R >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FreeText /Rect [150 240 250 260] /P 3 0 R /AP << /N 10 0 R >> >>".to_vec(),
        big(),
        big(),
        stream("", b"% GLYPH_A"),
        stream("", b"% GLYPH_B"),
        stream("", b"% GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    let refused = apply(&mut doc, None);
    assert!(refused.is_err(), "two appearances past the page's decoded-byte allowance together were read in full: {refused:?}");
}

/// The appearances are read within what the page's content left of the page's allowance, not
/// within one of their own: 1 MiB of content and an appearance within half a MiB of the allowance
/// together pass it, though the appearance alone fits. A small appearance is the control.
#[test]
fn a_page_s_appearances_spend_what_its_content_left_of_the_allowance() {
    let mut content = b"BT /T3 10 Tf 10 100 Td (AC) Tj ET".to_vec();
    content.resize(1 << 20, b' ');
    let content = pdfcraft_filters::encode_flate(&content);
    for (size, refused) in [(pdfcraft_cos::MAX_PAGE_DECODED - (1 << 19), true), (1, false)] {
        let appearance = pdfcraft_filters::encode_flate(&vec![b' '; size]);
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R /Annots [7 0 R] >>".to_vec(),
            stream("/Filter /FlateDecode", content.as_slice()),
            b"<< /Font << /T3 6 0 R >> >>".to_vec(),
            type3_abc(9),
            b"<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 220] /P 3 0 R /AP << /N 8 0 R >> >>".to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20] /Filter /FlateDecode", appearance.as_slice()),
            stream("", b"% GLYPH_A"),
            stream("", b"% GLYPH_B"),
            stream("", b"% GLYPH_C"),
        ]);
        mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
        let out = apply(&mut doc, None);
        assert_eq!(out.is_err(), refused, "an appearance of {size} bytes: {out:?}");
    }
}

/// Whether the first page of a full save of `doc` lists the font `name` in its resources.
fn saved_first_page_lists_font(doc: &Document, name: &[u8]) -> bool {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let res = saved.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().clone();
    res.get(b"Font").and_then(|f| saved.resolve(f).as_dict().map(|fonts| fonts.contains(name))).unwrap_or(false)
}

/// The group of a soft mask without /Resources shows glyph B of a second Type 3 font, which the
/// page lists but its content never selects. The group draws through the page's resources, so the
/// rewritten page still lists that font and B's program stays.
#[test]
fn a_type3_font_only_a_soft_mask_without_resources_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET /GS1 gs 150 150 100 100 re f"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> /ExtGState << /GS1 << /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 8 0 R >> >> >> >>"
            .to_vec(),
        type3_abc(9),
        type3_abc(12),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >>",
            b"1 g BT /T4 40 Tf 160 180 Td (B) Tj ET",
        ),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% MASK_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"MASK_GLYPH_B"), "the glyph the soft mask shows was unlinked");
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the soft mask selects");
}

/// The same soft mask, its ExtGState selected after a surplus operand (`(x) /GS1 gs`), which the
/// renderer ignores: read from the front, the cleanups never read the group, and dropped the font
/// it selects and its glyph.
#[test]
fn a_type3_font_only_a_soft_mask_selected_after_a_surplus_operand_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET (x) /GS1 gs 150 150 100 100 re f"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> /ExtGState << /GS1 << /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 8 0 R >> >> >> >>"
            .to_vec(),
        type3_abc(9),
        type3_abc(12),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >>",
            b"1 g BT /T4 40 Tf 160 180 Td (B) Tj ET",
        ),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% MASK_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(full_save_holds(&doc, b"MASK_GLYPH_B"), "the glyph the soft mask shows was unlinked");
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the soft mask selects");
}

/// The same group, its content behind a predictor whose `/DecodeParms` are given by reference:
/// the page's cleanup reads it through them, and keeps the font it selects.
#[test]
fn a_type3_font_only_a_soft_mask_with_parameters_by_reference_selects_stays_on_the_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET /GS1 gs 150 150 100 100 re f"),
        b"<< /Font << /T3 6 0 R /T4 7 0 R >> /ExtGState << /GS1 << /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 8 0 R >> >> >> >>"
            .to_vec(),
        type3_abc(9),
        type3_abc(12),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b""),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% UNUSED_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
        stream("", b"% UNUSED_T4_GLYPH_A"),
        stream("", b"% MASK_GLYPH_B"),
        stream("", b"% UNUSED_T4_GLYPH_C"),
    ]);
    let mut entries = super::form_entries([0, 0, 300, 300]);
    entries.push((
        "Group",
        Object::Dict(Dict::from_iter([(b"S".to_vec(), Object::name("Transparency")), (b"CS".to_vec(), Object::name("DeviceGray"))])),
    ));
    let group = super::behind_parms_by_reference(&mut doc, &entries, b"1 g BT /T4 40 Tf 160 180 Td (B) Tj ET");
    doc.set(ObjRef::new(8, 0), group);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"MASK_GLYPH_B"), "the glyph the soft mask shows was unlinked");
    assert!(saved_first_page_lists_font(&doc, b"T4"), "the page no longer lists the font the soft mask selects");
}

/// The cell of a tiling pattern another page fills with shows glyph B: its program is kept. (On
/// the redacted page itself, a pattern that shows text refuses the redaction: `pattern_guard`.)
#[test]
fn a_type3_glyph_a_tiling_pattern_on_another_page_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET"),
        b"<< /Font << /T3 9 0 R >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 7 0 R /Resources 8 0 R >>".to_vec(),
        stream("", b"/Pattern cs /P1 scn 200 200 50 50 re f"),
        b"<< /Pattern << /P1 10 0 R >> >>".to_vec(),
        type3_abc(11),
        stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 /Resources << /Font << /T3 9 0 R >> >>",
            b"BT /T3 10 Tf 2 5 Td (B) Tj ET",
        ),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% PATTERN_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"PATTERN_GLYPH_B"), "the glyph the pattern shows was unlinked");
    assert!(full_save_holds(&doc, b"PAGE_GLYPH_C"), "the glyph the page still shows was unlinked");
}

/// The group of a soft mask the page sets shows glyph B: its program is kept.
#[test]
fn a_type3_glyph_a_soft_mask_shows_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AC) Tj ET /GS1 gs 150 150 100 100 re f"),
        b"<< /Font << /T3 6 0 R >> /ExtGState << /GS1 << /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 7 0 R >> >> >> >>".to_vec(),
        type3_abc(8),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /Font << /T3 6 0 R >> >>",
            b"1 g BT /T3 40 Tf 160 180 Td (B) Tj ET",
        ),
        stream("", b"% REDACTED_GLYPH_A"),
        stream("", b"% MASK_GLYPH_B"),
        stream("", b"% PAGE_GLYPH_C"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"REDACTED_GLYPH_A"), "the redacted glyph program survived");
    assert!(full_save_holds(&doc, b"MASK_GLYPH_B"), "the glyph the soft mask shows was unlinked");
    assert!(full_save_holds(&doc, b"PAGE_GLYPH_C"), "the glyph the page still shows was unlinked");
}

/// Resource cleanup read a form without /Resources of its own again for every name that could
/// still reach the removed image. Here 3 000 names of one redacted image sit in the inherited
/// /Pages resources, and the other page draws a large such form: it is read once, and every name
/// leaves with the image.
#[test]
fn many_names_of_a_redacted_image_are_unlinked_reading_inherited_forms_once() {
    const ALIASES: usize = 3_000;
    let aliases: String = (0..ALIASES).map(|i| format!("/A{i} 6 0 R ")).collect();
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources 5 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 9 0 R >>".to_vec(),
        format!("<< /XObject << /Im 6 0 R /Fm 7 0 R {aliases}>> >>").into_bytes(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"200 200 m 210 210 l S ".repeat(20_000)),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q"),
        stream("", b"/Fm Do"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    let started = std::time::Instant::now();
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    assert!(!full_save_holds(&doc, b"IMAGESECRET"), "an alias kept the removed image reachable");
}

/// A form outside the mark, with resources of its own that give no font, shows glyph B with the
/// page's Type 3 font, through the page's names, as the renderer does: the font's cleanup keeps
/// B's program, also where its own `/T3` is a stream, which is no font. Where the form's own `/T3`
/// is another Type 3 font, the page's B goes and the form's own B stays.
#[test]
fn a_type3_glyph_a_form_shows_through_its_callers_font_is_kept() {
    let run = |form_res: &str| {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET /Fm Do"),
            b"<< /Font << /T3 6 0 R >> /XObject << /Fm 9 0 R >> >>".to_vec(),
            TYPE3.as_bytes().to_vec(),
            stream("", b"% SECRET_TYPE3_GLYPH_A"),
            stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << {form_res} >>"), b"BT /T3 10 Tf 10 20 Td (B) Tj ET"),
            TYPE3.replace("/A 7 0 R /B 8 0 R", "/A 11 0 R /B 12 0 R").into_bytes(),
            stream("", b"% OWN_TYPE3_GLYPH_A"),
            stream("", b"% OWN_TYPE3_GLYPH_B"),
        ]);
        mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
        assert!(!full_save_holds(&doc, b"SECRET_TYPE3_GLYPH_A"), "the redacted glyph program survived");
        (full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B"), full_save_holds(&doc, b"OWN_TYPE3_GLYPH_B"))
    };
    assert!(run("/ProcSet [/PDF]").0, "the form still shows the page's B");
    assert!(run("/Font << /T3 11 0 R >>").0, "a stream is no font: the page's B");
    assert_eq!(run("/Font << /T3 10 0 R >>"), (false, true), "the form's own font shows B");
}

/// Only a form outside the mark, whose own resources give no font, shows text with the page's
/// Type 3 font: the page's font cleanup counts it as drawn through the page's names, and keeps the
/// font and B's program, also where the form's own `/T3` is a stream, which is no font. Where the
/// form's own `/T3` is another Type 3 font, nothing shows the page's, and its programs go.
#[test]
fn a_type3_font_only_a_form_shows_through_its_callers_names_is_kept() {
    let run = |form_res: &str| {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET /Fm Do"),
            b"<< /Font << /T3 6 0 R /F1 13 0 R >> /XObject << /Fm 9 0 R >> >>".to_vec(),
            TYPE3.as_bytes().to_vec(),
            stream("", b"% PAGE_TYPE3_GLYPH_A"),
            stream("", b"% PAGE_TYPE3_GLYPH_B"),
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << {form_res} >>"), b"BT /T3 10 Tf 10 20 Td (B) Tj ET"),
            TYPE3.replace("/A 7 0 R /B 8 0 R", "/A 11 0 R /B 12 0 R").into_bytes(),
            stream("", b"% OWN_TYPE3_GLYPH_A"),
            stream("", b"% OWN_TYPE3_GLYPH_B"),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ]);
        mark(&mut doc, 0, &[[9.0, 95.0, 60.0, 110.0]], "");
        apply(&mut doc, None).unwrap();
        (full_save_holds(&doc, b"PAGE_TYPE3_GLYPH_B"), full_save_holds(&doc, b"OWN_TYPE3_GLYPH_B"))
    };
    assert!(run("/ProcSet [/PDF]").0, "the form still shows the page's B");
    assert!(run("/Font << /T3 11 0 R >>").0, "a stream is no font: the page's B");
    assert_eq!(run("/Font << /T3 10 0 R >>"), (false, true), "the form's own font shows B");
}

/// A form whose own resources give no font shows B with `/T3`, drawn once by the page, where `/T3`
/// is the page's Type 3 font, and once inside a form whose own `/T3` is another: the font's
/// cleanup scans it under each, and keeps both fonts' B.
#[test]
fn a_form_drawn_under_two_forms_names_shows_each_ones_type3_font() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET /Fm Do /G Do"),
        b"<< /Font << /T3 6 0 R /F1 13 0 R >> /XObject << /Fm 9 0 R /G 14 0 R >> >>".to_vec(),
        TYPE3.as_bytes().to_vec(),
        stream("", b"% PAGE_TYPE3_GLYPH_A"),
        stream("", b"% PAGE_TYPE3_GLYPH_B"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /ProcSet [/PDF] >>", b"BT /T3 10 Tf 10 20 Td (B) Tj ET"),
        TYPE3.replace("/A 7 0 R /B 8 0 R", "/A 11 0 R /B 12 0 R").into_bytes(),
        stream("", b"% OWN_TYPE3_GLYPH_A"),
        stream("", b"% OWN_TYPE3_GLYPH_B"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /Font << /T3 10 0 R >> >>", b"/Fm Do"),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 60.0, 110.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(full_save_holds(&doc, b"PAGE_TYPE3_GLYPH_B"), "drawn by the page, it shows the page's B");
    assert!(full_save_holds(&doc, b"OWN_TYPE3_GLYPH_B"), "drawn inside /G, it shows /G's B");
}

/// The kept glyph B draws its font's form `/F`, which draws `/G`, whose own resources give no
/// graphics state or pattern: `/G` sets `/GS0` and paints with `/P0` through `/F`'s names. When
/// glyph A is redacted, the cleanup of `/F`'s resources keeps both.
#[test]
fn a_type3_glyphs_form_keeps_what_a_form_inside_it_uses_through_its_names() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (AB) Tj ET"),
        b"<< /Font << /T3 6 0 R >> >>".to_vec(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 66 /Widths [500 500] /Encoding << /Differences [65 /A /B] >> /CharProcs << /A 7 0 R /B 8 0 R >> /Resources << /XObject << /F 9 0 R >> >> >>".to_vec(),
        stream("", b"% SECRET_GLYPH_A_PAYLOAD"),
        stream("", b"/F Do"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ExtGState << /GS0 10 0 R >> /Pattern << /P0 11 0 R >> /XObject << /G 12 0 R >> >>",
            b"/G Do",
        ),
        b"<< /Type /ExtGState /CA 0.5 /GS0_KEPT_MARKER 1 >>".to_vec(),
        stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", b"% PATTERN_P0_KEPT"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ProcSet [/PDF] >>",
            b"/GS0 gs /Pattern cs /P0 scn 0 0 10 10 re f",
        ),
    ]);
    mark(&mut doc, 0, &[[9.0, 95.0, 14.9, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    assert!(!full_save_holds(&doc, b"SECRET_GLYPH_A_PAYLOAD"));
    assert!(full_save_holds(&doc, b"GS0_KEPT_MARKER"), "/G's graphics state, given by /F, was dropped");
    assert!(full_save_holds(&doc, b"PATTERN_P0_KEPT"), "/G's pattern, given by /F, was dropped");
}

/// Fifty forms with resources of their own draw one form `/F` that shows B with `/T3`. Where `/F`'s
/// own resources give `/T3`, what it shows is the same under each, and it is scanned once; where
/// they give no font, it reads the page's through each, and is scanned under each.
#[test]
fn a_type3_scan_repeats_a_form_only_where_it_reads_its_callers_names() {
    let scans = |f_res: &str| {
        let callers: String = (0..50).map(|i| format!("/G{i} {} 0 R ", 10 + i)).collect();
        let draws: String = (0..50).map(|i| format!("/G{i} Do ")).collect();
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", format!("BT /T3 10 Tf 10 100 Td (A) Tj ET {draws}").as_bytes()),
            format!("<< /Font << /T3 6 0 R >> /XObject << {callers}>> >>").into_bytes(),
            TYPE3.as_bytes().to_vec(),
            stream("", b"% SECRET_TYPE3_GLYPH_A"),
            stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << {f_res} >>"), b"BT /T3 10 Tf 10 20 Td (B) Tj ET"),
        ];
        objs.extend((0..50).map(|_| stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /XObject << /F 9 0 R >> >>", b"/F Do")));
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
        crate::type3::TYPE3_FORM_SCANS.with(|c| c.set(0));
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
        assert!(full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B"));
        crate::type3::TYPE3_FORM_SCANS.with(std::cell::Cell::get)
    };
    // The fifty callers, and `/F` once.
    assert_eq!(scans("/Font << /T3 6 0 R >>"), 51);
    // The fifty callers, and `/F` under each.
    assert_eq!(scans("/ProcSet [/PDF]"), 100);
}

/// Layers of two forms with resources of their own, each drawing both forms of the next, the last
/// showing B with the page's Type 3 font through their names: what each shows depends on the way it
/// is reached. Eight layers are scanned, and B stays; twenty would take about two million scans, and
/// the redaction is refused once 65,536 of those scans are made again.
#[test]
fn a_type3_scan_of_forms_reached_in_too_many_ways_is_refused() {
    let run = |layers: usize| {
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET /A Do /B Do"),
            b"<< /Font << /T3 6 0 R >> /XObject << /A 9 0 R /B 10 0 R >> >>".to_vec(),
            TYPE3.as_bytes().to_vec(),
            stream("", b"% SECRET_TYPE3_GLYPH_A"),
            stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
        ];
        for layer in 0..layers {
            for _ in 0..2 {
                objs.push(if layer + 1 < layers {
                    let next = 9 + 2 * (layer + 1);
                    stream(
                        &format!(
                            "/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /XObject << /A {next} 0 R /B {} 0 R >> >>",
                            next + 1
                        ),
                        b"/A Do /B Do",
                    )
                } else {
                    stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /ProcSet [/PDF] >>", b"BT /T3 10 Tf 10 20 Td (B) Tj ET")
                });
            }
        }
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
        let out = apply(&mut doc, None);
        let kept = out.is_ok() && full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B");
        (out, kept)
    };
    let (out, kept) = run(8);
    assert_eq!((out.map(|r| r.glyphs), kept), (Ok(1), true));
    let started = std::time::Instant::now();
    let (out, _) = run(20);
    assert!(refused_for(&out, "forms are drawn in too many ways"), "{out:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
}

/// `/C` (resources of its own, no Type 3 font) sets a plain font and draws `/F`, which shows B with
/// `/T3` through its callers' names, having resources of its own that give no font, or none. The page draws `/C` twice, with its Type 3 font and with a
/// plain one, then `/G`, whose own `/T3` is another Type 3 font and which draws `/C`. The second
/// draw of `/C` finds `/F` already scanned, yet still depends on the page's names through it, so
/// `/C` is scanned again under `/G`, and `/G`'s font keeps its B.
#[test]
fn a_form_depends_on_its_callers_names_through_a_form_scanned_before() {
    // `/F` with resources of its own that give no font, or without any.
    for f_res in ["/Resources << /ProcSet [/PDF] >>", ""] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
            stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET /C Do BT /F1 10 Tf ET /C Do /G Do"),
            b"<< /Font << /T3 6 0 R /F1 13 0 R >> /XObject << /C 14 0 R /G 15 0 R >> >>".to_vec(),
            TYPE3.as_bytes().to_vec(),
            stream("", b"% SECRET_TYPE3_GLYPH_A"),
            stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] {f_res}"), b"BT /T3 10 Tf 10 20 Td (B) Tj ET"),
            TYPE3.replace("/A 7 0 R /B 8 0 R", "/A 11 0 R /B 12 0 R").into_bytes(),
            stream("", b"% OWN_TYPE3_GLYPH_A"),
            stream("", b"% OWN_TYPE3_GLYPH_B"),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /Font << /F1 13 0 R >> /XObject << /F 9 0 R >> >>",
                b"BT /F1 10 Tf ET /F Do",
            ),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /Font << /T3 10 0 R >> /XObject << /C 14 0 R >> >>", b"/C Do"),
        ]);
        mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
        assert!(full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B"), "drawn by the page, /F shows its B");
        assert!(full_save_holds(&doc, b"OWN_TYPE3_GLYPH_B"), "drawn inside /G, /F shows /G's B ({f_res})");
    }
}

/// Three hundred pages each draw the same 250 forms, whose own resources give no font, showing B
/// with each page's Type 3 font, straight from their content or inside one form `/T` with resources
/// of its own; or each page lists a form of its own it doesn't draw, giving that font and drawing
/// the 250. Each of the 250 is scanned once for each page or form no page draws, 75,000 scans in
/// all, which grow with those, not with the ways through other forms, and the redaction goes on.
#[test]
fn a_type3_scan_of_forms_on_many_pages_is_not_refused() {
    const FORMS: usize = 250;
    const PAGES: usize = 300;
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Shape {
        Drawn,
        InTemplate,
        Undrawn,
    }
    let run = |shape: Shape| {
        let names: String = (0..FORMS).map(|i| format!("/F{i} {} 0 R ", 9 + i)).collect();
        let draws: String = (0..FORMS).map(|i| format!("/F{i} Do ")).collect();
        let first_page = 9 + FORMS;
        let kids: String = (0..PAGES).map(|i| format!("{} 0 R ", first_page + i)).collect();
        let (content, xobjects) = match shape {
            Shape::Drawn => (draws.clone(), names.clone()),
            Shape::InTemplate => ("/T Do".to_string(), "/T 5 0 R ".to_string()),
            Shape::Undrawn => (String::new(), String::new()),
        };
        let mut objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>").into_bytes(),
            stream("", format!("BT /T3 10 Tf 10 100 Td (A) Tj ET {content}").as_bytes()),
            format!("<< /Font << /T3 6 0 R >> /XObject << {xobjects}>> >>").into_bytes(),
            stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /XObject << {names}>> >>"), draws.as_bytes()),
            TYPE3.as_bytes().to_vec(),
            stream("", b"% SECRET_TYPE3_GLYPH_A"),
            stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
        ];
        objs.extend((0..FORMS).map(|_| {
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /ProcSet [/PDF] >>", b"BT /T3 10 Tf 10 20 Td (B) Tj ET")
        }));
        // Each page's resources are its own where it lists a form it doesn't draw.
        let resources = |i| if shape == Shape::Undrawn { first_page + PAGES + i } else { 4 };
        objs.extend((0..PAGES).map(|i| {
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 3 0 R /Resources {} 0 R >>", resources(i)).into_bytes()
        }));
        if shape == Shape::Undrawn {
            objs.extend((0..PAGES).map(|i| format!("<< /Font << /T3 6 0 R >> /XObject << /H {} 0 R >> >>", first_page + 2 * PAGES + i).into_bytes()));
            let holder = format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /Font << /T3 6 0 R >> /XObject << {names}>> >>");
            objs.extend((0..PAGES).map(|_| stream(&holder, draws.as_bytes())));
        }
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
        crate::type3::TYPE3_FORM_SCANS.with(|c| c.set(0));
        let out = apply(&mut doc, None);
        assert!(out.is_ok(), "{shape:?}: {out:?}");
        assert!(full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B"), "{shape:?}");
        crate::type3::TYPE3_FORM_SCANS.with(std::cell::Cell::get)
    };
    assert_eq!(run(Shape::Drawn), FORMS * PAGES);
    // `/T` too, once for each page.
    assert_eq!(run(Shape::InTemplate), (FORMS + 1) * PAGES);
    assert_eq!(run(Shape::Undrawn), FORMS * PAGES);
}

/// A page showing A with its Type 3 font, then drawing the first of `layers` layers of two forms
/// with resources of their own, each drawing both forms of the next; the last draw `last` forms
/// without resources, each showing B with the page's font through their names and holding a comment
/// `pad` bytes long.
fn forms_drawn_in_layers(layers: usize, last: usize, pad: usize) -> Document {
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"BT /T3 10 Tf 10 100 Td (A) Tj ET /A Do /B Do"),
        b"<< /Font << /T3 6 0 R >> /XObject << /A 9 0 R /B 10 0 R >> >>".to_vec(),
        TYPE3.as_bytes().to_vec(),
        stream("", b"% SECRET_TYPE3_GLYPH_A"),
        stream("", b"% PUBLIC_TYPE3_GLYPH_B"),
    ];
    let first_shown = 9 + 2 * layers;
    for layer in 0..layers {
        let (names, draws): (String, String) = if layer + 1 < layers {
            let next = 9 + 2 * (layer + 1);
            (format!("/A {next} 0 R /B {} 0 R ", next + 1), "/A Do /B Do".into())
        } else {
            ((0..last).map(|i| format!("/Z{i} {} 0 R ", first_shown + i)).collect(), (0..last).map(|i| format!("/Z{i} Do ")).collect())
        };
        let form = format!("/Type /XObject /Subtype /Form /BBox [0 0 300 50] /Resources << /XObject << {names}>> >>");
        objs.extend((0..2).map(|_| stream(&form, draws.as_bytes())));
    }
    let shown = format!("BT /T3 10 Tf 10 20 Td (B) Tj ET\n%{}", "x".repeat(pad));
    objs.extend((0..last).map(|_| stream("/Type /XObject /Subtype /Form /BBox [0 0 300 50]", shown.as_bytes())));
    pdf(objs)
}

/// The forms of the last of twelve layers ([`forms_drawn_in_layers`]) each draw sixteen forms
/// without resources, reached in 4,096 ways each. Their scans made again count with the others:
/// neither their 65,520 nor the 8,166 of the forms with resources of their own passes 65,536, but
/// together they do, and the redaction is refused. At eight layers B stays.
#[test]
fn a_type3_scan_counts_forms_without_resources_scanned_again() {
    let run = |layers| {
        let mut doc = forms_drawn_in_layers(layers, 16, 0);
        mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
        let out = apply(&mut doc, None);
        let kept = out.is_ok() && full_save_holds(&doc, b"PUBLIC_TYPE3_GLYPH_B");
        (out, kept)
    };
    let (out, kept) = run(8);
    assert_eq!((out.map(|r| r.glyphs), kept), (Ok(1), true));
    let (out, _) = run(12);
    assert!(refused_for(&out, "forms are drawn in too many ways"), "{out:?}");
}

/// The forms of the last of eight layers ([`forms_drawn_in_layers`]) draw one form without
/// resources, 1.1 MB long, reached in 256 ways. Of the scans made again, 749 would count, far from
/// 65,536, but the 255 of that form would decode about 268 MiB, past an allowance of a page
/// (256 MiB), and the redaction is refused.
#[test]
fn a_type3_scan_charges_what_forms_scanned_again_decode() {
    let mut doc = forms_drawn_in_layers(8, 1, 1_100_000);
    mark(&mut doc, 0, &[[9.0, 95.0, 16.0, 110.0]], "");
    let out = apply(&mut doc, None);
    assert!(refused_for(&out, "forms are drawn in too many ways"), "{out:?}");
}
