use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, SaveOptions, write_full, write_incremental};
use pdfcraft_testkit::{Pdf, stream_obj as stream};

use super::*;

fn pdf(objs: Vec<Vec<u8>>) -> Document {
    Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap()
}

/// A font where every glyph is 500 units wide (so a 10 pt glyph advances 5 pt).
const FONT: &str = "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 32 /LastChar 126 /Widths 95 0 R /FontDescriptor << /Ascent 800 /Descent -200 >> >>";

fn widths() -> Vec<u8> {
    format!("[{}]", vec!["500"; 95].join(" ")).into_bytes()
}

/// One 300×300 page with `content`, font /F1, and extra resources/objects.
fn one_page(content: &[u8], extra_res: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> {extra_res} >> >>")
            .into_bytes(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ];
    objs.extend(extra);
    pdf(objs)
}

fn mark(doc: &mut Document, page: usize, rects: &[[f64; 4]], overlay: &str) {
    let shape = Shape::Redact { quads: rects.iter().map(|r| rect_quad(*r)).collect(), overlay: overlay.into(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page, shape, style, contents: String::new(), author: "Tester".into() }, &Meta::default()).unwrap();
}

/// The decoded content streams of a page, joined.
fn content(doc: &Document, page: usize) -> String {
    let p = &pdfcraft_model::pages(doc)[page];
    let (_, data) = page_streams(doc, &p.dict, page).unwrap();
    data.iter().map(|d| String::from_utf8_lossy(d).into_owned()).collect::<Vec<_>>().join("\n")
}

/// Glyphs (and inline images) of page `page` under `rects` (the verifier's count).
fn under(doc: &mut Document, page: usize, rects: &[[f64; 4]]) -> usize {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let (_, data) = page_streams(doc, &p.dict, page).unwrap();
    let res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut rep = Report::default();
    let mut scope = Scope::new(rects, Mode::Verify, &mut rep);
    process(doc, &mut scope, &data, &res, pdfcraft_content::Matrix::IDENTITY).residue
}

fn reopen(doc: &Document) -> Document {
    Document::open(Arc::new(write_incremental(doc, &SaveOptions::default()).unwrap())).unwrap()
}

#[test]
fn glyphs_under_a_mark_go_and_the_rest_stays_put() {
    // "AB1234CD" from x = 10 at 10 pt: each glyph 5 pt wide; 1234 spans x 20–40.
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET", "", vec![]);
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2, "C and D before");
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.marks, r.glyphs), (1, 4));
    let c = content(&doc, 0);
    assert!(c.contains("[(AB) -2000 (CD)] TJ"), "{c}");
    assert!(!c.contains("1234"));
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2, "C and D are where they were");
    assert_eq!(under(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]]), 0);
    // The mark became a black box drawn into the page; the annotation is gone.
    assert!(marks(&doc).is_empty());
    assert!(c.contains("0 0 0 rg") && c.contains("20 95 20 15 re f"), "{c}");
    let doc = reopen(&doc);
    assert!(!content(&doc, 0).contains("1234"));
}

#[test]
fn added_text_parameters_do_not_keep_redacted_text() {
    // An Edit ▸ Add content item: its stream dictionary keeps the source text under /PCAdded.
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("/PCMark /Added /PCAdded << /Kind /Text /Text (AB1234CD) >>", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(!content(&doc, 0).contains("1234"));
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes_contain(&bytes, b"1234"), "redacted text left in the saved file");
}

#[test]
fn operators_split_across_content_streams_are_redacted_whole() {
    // One content stream in three pieces, split between tokens: a marked-content dictionary
    // ends in the second piece, and the TJ array that ends it shows its glyphs with the
    // operator in the third.
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R 8 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"/P << /MCID 0"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("", b">> BDC BT /F1 10 Tf 10 100 Td [(AB1234CD)]"),
        stream("", b"TJ ET EMC BT /F1 10 Tf 10 200 Td (KEEP) Tj ET"),
    ]);
    assert_eq!(under(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]]), 4, "the verifier sees the split TJ");
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 4, "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("1234"), "redacted glyphs left in the page: {c}");
    assert!(c.contains("KEEP"), "{c}");
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2, "C and D are where they were");
    // Every operator still has its operands: none was cut off from them.
    for op in pdfcraft_content::parse(c.as_bytes()).ops {
        let want = match op.op.as_slice() {
            b"TJ" | b"Tj" => 1,
            b"BDC" | b"Tf" | b"Td" => 2,
            _ => continue,
        };
        assert_eq!(op.operands.len(), want, "{} lost its operands in {c}", String::from_utf8_lossy(&op.op));
    }
    let doc = reopen(&doc);
    assert!(!content(&doc, 0).contains("1234"));
}

#[test]
fn kerning_spacing_scaling_and_line_operators_are_honoured() {
    // TJ kerning, character and word spacing, 50% horizontal scaling, ' and ".
    let src = b"BT /F1 10 Tf 2 Tc 4 Tw 50 Tz 12 TL 0 200 Td [(AB) -1000 (C D)] TJ (EF) ' 1 0 (GH) \" ET";
    let mut doc = one_page(src, "", vec![]);
    // Line 1 (y 200): A at 0, B at 3.5 (advance (5+2)×0.5), kern +5 → C at 12, space at 15.5,
    // D at 21 (space advance (5+2+4)×0.5 = 5.5). Remove C only.
    mark(&mut doc, 0, &[[12.2, 195.0, 14.8, 210.0]], "");
    // Line 2 (y 188) "EF" and line 3 (y 176) "GH" (Tw 1, Tc 0): remove F and G.
    mark(&mut doc, 0, &[[3.8, 183.0, 6.0, 198.0 - 10.0], [0.2, 171.0, 2.3, 180.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 3, "{r:?}");
    let c = content(&doc, 0);
    assert!(c.contains("[(AB) -1700 ( D)] TJ"), "{c}");
    assert!(c.contains("T*\n[(E) -700] TJ"), "{c}");
    assert!(c.contains("1 Tw\n0 Tc\nT*\n[-500 (H)] TJ"), "{c}");
    // Everything else stays readable where it was.
    assert_eq!(under(&mut doc, 0, &[[20.0, 195.0, 30.0, 210.0]]), 1, "D");
    assert_eq!(under(&mut doc, 0, &[[2.6, 171.0, 6.0, 180.0]]), 1, "H");
}

#[test]
fn rotated_text_and_composite_fonts() {
    // A Type0 Identity-H font (two-byte codes) with /W widths, and a 90° text matrix.
    let t0 = b"<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 /W [1 [500 500] 3 4 250] >>] >>".to_vec();
    let src = b"BT /F2 10 Tf 0 1 -1 0 100 50 Tm <0001000200030004> Tj ET";
    let mut doc = one_page(src, "", vec![]);
    let font_ref = doc.add(Object::Dict(Dict::new()));
    let parsed = {
        let mut lx = pdfcraft_cos::Lexer::new(&t0, 0);
        lx.object().unwrap()
    };
    doc.set(font_ref, parsed);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
        let mut fonts = res.get(b"Font").and_then(Object::as_dict).cloned().unwrap();
        fonts.set(b"F2".to_vec(), Object::Ref(font_ref));
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    // Glyphs run upwards from y 50: CID1 50–55, CID2 55–60, CID3 60–62.5, CID4 62.5–65.
    mark(&mut doc, 0, &[[85.0, 55.5, 105.0, 61.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 2, "CIDs 2 and 3");
    let c = content(&doc, 0);
    assert!(c.contains("[<0001> -750 <0004>] TJ") || c.contains("[(\\000\\001) -750 (\\000\\004)] TJ"), "{c}");
}

#[test]
fn vertical_writing_text_under_a_mark_fails_closed() {
    // A Type0 font laid out vertically (Identity-V / CMap WMode 1) cannot have its glyphs placed
    // under a mark by the horizontal interpreter, so Apply Redactions must fail rather than ship
    // the text under the box. The horizontal control (Identity-H) is redacted normally.
    let make = |encoding: &str| -> Document {
        let t0 = format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /{encoding} /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 /W [1 [500 500] 3 4 250] >>] >>"
        )
        .into_bytes();
        let src = b"BT /F2 10 Tf 0 1 -1 0 100 50 Tm <0001000200030004> Tj ET";
        let mut doc = one_page(src, "", vec![]);
        let font_ref = doc.add(Object::Dict(Dict::new()));
        let parsed = {
            let mut lx = pdfcraft_cos::Lexer::new(&t0, 0);
            lx.object().unwrap()
        };
        doc.set(font_ref, parsed);
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| {
            let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
            let mut fonts = res.get(b"Font").and_then(Object::as_dict).cloned().unwrap();
            fonts.set(b"F2".to_vec(), Object::Ref(font_ref));
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            d.set(b"Resources".to_vec(), Object::Dict(res));
        })
        .unwrap();
        doc
    };

    // Identity-V: apply must refuse rather than report success with the text still in the file.
    let mut vdoc = make("Identity-V");
    mark(&mut vdoc, 0, &[[85.0, 55.5, 105.0, 61.0]], "");
    let err = apply(&mut vdoc, None).unwrap_err();
    assert_eq!(err, RedactError::VerticalText(1), "vertical text with a pending mark must not silently pass");
    let msg = err.to_string();
    assert!(msg.contains("cannot tell whether"), "{msg}");
    assert!(!msg.contains("text under a redaction mark"), "{msg}");

    // Identity-H control: the same glyphs and mark are redacted normally.
    let mut hdoc = make("Identity-H");
    mark(&mut hdoc, 0, &[[85.0, 55.5, 105.0, 61.0]], "");
    let r = apply(&mut hdoc, None).unwrap();
    assert_eq!(r.glyphs, 2, "the horizontal control is redacted: {r:?}");
}

#[test]
fn images_are_removed_or_have_their_pixels_cleared() {
    // Image 1 is fully covered; image 2 (4×1 gray, all white) is half covered.
    let img = stream("/Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", [255, 255, 255, 255]);
    let src = b"q 10 0 0 10 10 10 cm /Im1 Do Q q 40 0 0 10 100 100 cm /Im2 Do Q";
    let mut doc = one_page(src, "/XObject << /Im1 7 0 R /Im2 7 0 R >>", vec![img]);
    mark(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0], [95.0, 95.0, 120.0, 115.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (1, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("/Im1 Do") && !c.contains("/Im2 Do"), "{c}");
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let new = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).map(|(_, v)| v.clone()).expect("a cleared copy");
    let Object::Stream(s) = &*doc.resolve(&new) else { panic!() };
    assert_eq!(s.decoded().unwrap(), [0, 0, 255, 255], "pixels 1–2 (x 100–120) cleared");
    let Object::Stream(orig) = &*doc.get(ObjRef::new(7, 0)) else { panic!() };
    assert_eq!(orig.decoded().unwrap(), [255; 4], "the shared original is untouched");
}

#[test]
fn vectors_are_removed_or_clipped_and_inline_images_go() {
    let src = b"0 g 10 10 20 20 re f 0 0 300 300 re f q 10 0 0 10 50 50 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \x80 EI Q 1 0 0 RG 150 150 m 160 160 l S";
    let mut doc = one_page(src, "", vec![]);
    mark(&mut doc, 0, &[[5.0, 5.0, 35.0, 35.0], [45.0, 45.0, 65.0, 65.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.paths_removed, r.paths_clipped, r.images_removed), (1, 1, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("10 10 20 20 re"), "{c}");
    assert!(c.contains("W*\nn\n0 0 300 300 re\nf\nQ"), "the page-size rect is clipped: {c}");
    assert!(!c.contains("BI"), "{c}");
    assert!(c.contains("150 150 m"), "the line elsewhere stays");
}

#[test]
fn shared_form_xobjects_are_copied_not_changed() {
    let form =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET");
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Fm 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        stream("", b"/Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        form,
    ];
    objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec());
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_rewritten), (6, 1), "{r:?}");
    assert!(content(&doc, 0).contains("/PCRedacted1 Do"));
    assert_eq!(content(&doc, 1), "/Fm Do", "page 2 shares the stream and the form: untouched");
    assert_eq!(under(&mut doc, 1, &[[0.0, 90.0, 300.0, 120.0]]), 6);
    assert_eq!(under(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]]), 0);
}

#[test]
fn redacted_images_and_forms_do_not_survive_a_full_rewrite_save() {
    // Page 1 draws a fully covered image, a partly covered image and a form XObject whose text
    // is under the mark. After Apply Redactions and the forced full rewrite, none of their secret
    // bytes may stay in the saved file, while an unrelated image drawn only on page 2 survives.
    let im1 = b"REDIMGONECOVERED"; // 16 bytes, one gray row
    let impart = b"REDIMGPARTSECRETPUBPARTBOTTOMROW"; // 16-byte secret top row + 16-byte bottom row
    let imkeep = b"PUBIMGKEEPSURVIVE"; // 17 bytes, used only on page 2
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 7 0 R /ImPart 8 0 R /FmSecret 9 0 R >> >> >>".to_vec(),
        stream("", b"q 50 0 0 50 20 230 cm /Im1 Do Q q 50 0 0 100 100 180 cm /ImPart Do Q /FmSecret Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", im1),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", impart),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>",
            b"BT /F1 10 Tf 20 230 Td (REDFORMSECRETTEXT) Tj ET",
        ),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 11 0 R /Resources << /XObject << /ImKeep 12 0 R >> >> >>".to_vec(),
        stream("", b"q 50 0 0 50 20 20 cm /ImKeep Do Q"),
        stream("/Type /XObject /Subtype /Image /Width 17 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", imkeep),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.images_removed >= 1 && r.forms_rewritten >= 1, "{r:?}");

    // The full rewrite that redaction forces must not carry the removed content forward.
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes_contain(&bytes, b"REDIMGONECOVERED"), "covered image samples survived the save");
    assert!(!bytes_contain(&bytes, b"REDIMGPARTSECRET"), "partly covered image's secret row survived the save");
    assert!(!bytes_contain(&bytes, b"REDFORMSECRETTEXT"), "the form's redacted text survived the save");

    // The unrelated image drawn only on page 2 stays referenced and intact.
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let keep = pdfcraft_model::pages(&saved).swap_remove(1);
    let res = saved.resolve(keep.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = saved.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let kref = xo.get(b"ImKeep").and_then(Object::as_ref).expect("ImKeep still referenced");
    let Object::Stream(s) = &*saved.get(kref) else { panic!("ImKeep is a stream") };
    assert_eq!(s.decoded().unwrap(), imkeep, "the unrelated image survives intact");
}

/// Two pages that draw through one resource dictionary: inherited from the /Pages node, or one
/// dictionary object both pages refer to. Page 1 draws a covered image, a partly covered image
/// and a form whose text is under the mark; page 2 draws /ImKeep, and /Im1 too when `again`.
fn two_pages_one_resource_dictionary(inherited: bool, again: bool) -> Document {
    let res = "<< /Font << /F1 5 0 R >> /XObject << /Im1 7 0 R /ImPart 8 0 R /FmSecret 9 0 R /ImKeep 12 0 R >> >>";
    let (pages_res, page_res) = if inherited { (format!("/Resources {res}"), "") } else { (String::new(), "/Resources 13 0 R") };
    let page2: &[u8] =
        if again { b"q 50 0 0 50 20 20 cm /ImKeep Do Q q 50 0 0 50 120 20 cm /Im1 Do Q" } else { b"q 50 0 0 50 20 20 cm /ImKeep Do Q" };
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 2 {pages_res} >>").into_bytes(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R {page_res} >>").into_bytes(),
        stream("", b"q 50 0 0 50 20 230 cm /Im1 Do Q q 50 0 0 100 100 180 cm /ImPart Do Q /FmSecret Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"REDIMGONECOVERED"),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8", b"REDIMGPARTSECRETPUBPARTBOTTOMROW"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>",
            b"BT /F1 10 Tf 20 230 Td (REDFORMSECRETTEXT) Tj ET",
        ),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 11 0 R {page_res} >>").into_bytes(),
        stream("", page2),
        stream("/Type /XObject /Subtype /Image /Width 17 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBIMGKEEPSURVIVE"),
        // 13: the dictionary both pages refer to (unused when the resources are inherited).
        if inherited { b"<< >>".to_vec() } else { res.as_bytes().to_vec() },
    ])
}

/// The samples of the image a page's (possibly inherited) resources name `name`.
fn named_image(doc: &Document, page: usize, name: &[u8]) -> Option<Vec<u8>> {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let res = doc.resolve(p.dict.get(b"Resources")?).as_dict().cloned()?;
    let xo = doc.resolve(res.get(b"XObject")?).as_dict().cloned()?;
    match &*doc.get(xo.get(name)?.as_ref()?) {
        Object::Stream(s) => s.decoded().ok(),
        _ => None,
    }
}

/// Apply Redactions gives the page its own pruned /Resources, but the dictionary it drew through
/// before (inherited from the /Pages node, or shared with another page) still named the removed
/// image and form, so a full-rewrite save kept them.
#[test]
fn redacted_xobjects_do_not_survive_through_inherited_or_shared_resources() {
    for inherited in [true, false] {
        let mut doc = two_pages_one_resource_dictionary(inherited, false);
        mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert!(r.images_removed >= 1 && r.forms_rewritten >= 1, "{r:?}");

        let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
        for secret in [&b"REDIMGONECOVERED"[..], b"REDIMGPARTSECRET", b"REDFORMSECRETTEXT"] {
            assert!(!bytes_contain(&bytes, secret), "{} survived the save (inherited: {inherited})", String::from_utf8_lossy(secret));
        }
        // Page 2 still draws its own image through the same dictionary.
        let saved = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(named_image(&saved, 1, b"ImKeep").as_deref(), Some(&b"PUBIMGKEEPSURVIVE"[..]), "inherited: {inherited}");
    }
}

/// The same image shown on another page is not under a mark there: that page keeps it.
#[test]
fn an_image_another_page_still_draws_through_the_same_resources_is_kept() {
    for inherited in [true, false] {
        let mut doc = two_pages_one_resource_dictionary(inherited, true);
        mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        apply(&mut doc, None).unwrap();
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(named_image(&saved, 1, b"Im1").as_deref(), Some(&b"REDIMGONECOVERED"[..]), "inherited: {inherited}");
        assert_eq!(named_image(&saved, 0, b"Im1"), None, "the redacted page no longer names it (inherited: {inherited})");
        assert!(content(&saved, 1).contains("/Im1 Do"), "page 2 is untouched");
    }
}

/// A form without /Resources draws through its caller's: the image it drew under the mark is
/// named by the page's own dictionary, which no `Do` of the page itself ever used.
#[test]
fn an_image_a_form_drew_through_the_pages_resources_does_not_survive() {
    let mut doc = one_page(
        b"/FmBare Do",
        "/XObject << /FmBare 7 0 R /Im1 8 0 R >>",
        vec![
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 20 230 cm /Im1 Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"REDIMGONECOVERED"),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.images_removed, 1, "{r:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes_contain(&bytes, b"REDIMGONECOVERED"), "the image the form drew survived the save");
}

/// A form without /Resources draws an image both under the mark and outside it, and the page
/// draws the same image under the mark too. The page then no longer draws the image itself and
/// drops it from its own dictionary, so the form's copy must keep resources of its own that
/// still name it: the draw outside the mark stays, with the image's bytes.
#[test]
fn an_image_a_resource_less_form_draws_outside_the_mark_survives() {
    let mut doc = one_page(
        b"q 50 0 0 50 20 230 cm /Im1 Do Q BT /F1 10 Tf 20 210 Td (PAGESECRET) Tj ET /FmBare Do",
        "/XObject << /FmBare 7 0 R /Im1 8 0 R >>",
        vec![
            stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 120 230 cm /Im1 Do Q q 50 0 0 50 20 20 cm /Im1 Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMGKEPTOUTSIDE!!"),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.forms_rewritten), (2, 1), "the page's and the form's draws under the mark go: {r:?}");
    assert!(!saved_contains(&doc, b"PAGESECRET"), "the page's text under the mark survived the save");
    assert!(saved_contains(&doc, b"IMGKEPTOUTSIDE!!"), "the image the form still draws outside the mark left the saved file");

    // The page draws the form's copy, which draws /Im1 once (outside the mark) and finds it
    // through its own /Resources, or through the page's when it has none.
    let saved = reopen(&doc);
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let page_res = saved.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let page_xo = saved.resolve(page_res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let copy = page_xo.iter().find(|(n, _)| n.starts_with(b"PCRedacted")).and_then(|(_, v)| v.as_ref()).expect("the page draws the form's copy");
    let Object::Stream(copy) = &*saved.get(copy) else { panic!("the form's copy is a stream") };
    let drawn = String::from_utf8(copy.decoded().unwrap()).unwrap();
    assert_eq!(drawn.matches("/Im1 Do").count(), 1, "only the draw outside the mark stays: {drawn}");
    let res = match copy.dict.get(b"Resources") {
        Some(r) => saved.resolve(r).as_dict().cloned().unwrap(),
        None => page_res,
    };
    let image = res.get(b"XObject").and_then(|x| saved.resolve(x).as_dict().cloned()).and_then(|xo| xo.get(b"Im1").and_then(Object::as_ref));
    let image = image.map(|r| saved.get(r)).expect("the form's copy can no longer find the image it draws outside the mark");
    let Object::Stream(image) = &*image else { panic!("/Im1 is a stream") };
    assert_eq!(image.decoded().unwrap(), b"IMGKEPTOUTSIDE!!", "the image drawn outside the mark is intact");
}

/// Remove Hidden Information has the same rewrite: an off layer's image named by an inherited
/// /Resources must leave the saved file too.
#[test]
fn a_hidden_layers_image_does_not_survive_through_inherited_resources() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << /OFF [6 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /XObject << /ImOC 5 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream("", b"q 50 0 0 50 20 20 cm /ImOC Do Q"),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OC 6 0 R", b"HIDDENLAYERIMAGE"),
        b"<< /Type /OCG /Name (Secret layer) >>".to_vec(),
    ]);
    let done = remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
    assert!(done.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0), "{done:?}");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes_contain(&bytes, b"HIDDENLAYERIMAGE"), "the off layer's image survived the save");
}

#[test]
fn redacting_an_xfa_field_drops_the_xfa_packet_so_its_value_cannot_survive() {
    // A static AcroForm+XFA form whose field value lives both in the widget /V and in the XFA
    // datasets packet (Designer-style /XFA array). Redacting the widget must delete the field and
    // stop the form being XFA, so neither the saved file nor a later relayout keeps the value.
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [8 0 R] >>".to_vec(),
        stream("", b"BT ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Fields [8 0 R] /XFA 9 0 R /PCXfaLayout << /Pages 1 /Fields 1 >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (ssn) /V (SSN-987-65-4321) /Rect [72 200 272 220] /P 3 0 R >>".to_vec(),
        b"[(datasets) 10 0 R (template) 11 0 R]".to_vec(),
        stream(
            "",
            b"<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><ssn>SSN-987-65-4321</ssn></xfa:data></xfa:datasets>",
        ),
        stream(
            "",
            b"<template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\"><subform name=\"form1\"><field name=\"ssn\"/></subform></template>",
        ),
    ];
    let mut doc = pdf(objs);
    assert_eq!(pdfcraft_forms::fields(&doc).len(), 1, "the XFA-bound field is present before");
    mark(&mut doc, 0, &[[72.0, 200.0, 272.0, 220.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.fields >= 1 && r.xfa_removed, "{r:?}");

    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes_contain(&bytes, b"SSN-987-65-4321"), "the redacted value survived in the datasets packet");

    let saved = Document::open(Arc::new(bytes)).unwrap();
    let cat = saved.get(saved.root().unwrap()).as_dict().cloned().unwrap();
    let acro = saved.resolve(cat.get(b"AcroForm").unwrap()).as_dict().cloned().unwrap();
    assert!(acro.get(b"XFA").is_none(), "/XFA is gone from the saved AcroForm");
    assert!(acro.get(b"PCXfaLayout").is_none(), "the cached layout mark is cleared");
    assert!(cat.get(b"NeedsRendering").is_none(), "/NeedsRendering is cleared");
}

#[test]
fn comments_links_and_fields_under_a_mark_go() {
    let mut doc = one_page(b"", "", vec![]);
    // A comment under the mark, a link elsewhere, and a text field under the mark.
    let square = Shape::Rectangle { rect: [20.0, 20.0, 40.0, 40.0] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&square), shape: square, contents: "x".into(), author: "a".into() },
        &Meta::default(),
    )
    .unwrap();
    let link = Shape::Rectangle { rect: [200.0, 200.0, 220.0, 220.0] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&link), shape: link, contents: "keep".into(), author: "a".into() },
        &Meta::default(),
    )
    .unwrap();
    pdfcraft_forms::add_field(&mut doc, 0, [10.0, 50.0, 100.0, 70.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("ssn")).unwrap();
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "REDACTED");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.annotations, r.fields), (1, 1), "{r:?}");
    assert!(pdfcraft_forms::fields(&doc).is_empty());
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    assert_eq!(annots_of(&doc, &p.dict).len(), 1, "only the far rectangle stays");
    let c = content(&doc, 0);
    assert!(c.contains("(REDACTED) Tj"), "overlay text: {c}");
}

#[test]
fn nothing_to_apply_and_clearing_marks() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB) Tj ET", "", vec![]);
    assert_eq!(apply(&mut doc, None), Err(RedactError::NothingToApply));
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 50.0]], "");
    assert_eq!(marks(&doc).len(), 1);
    assert_eq!(clear_marks(&mut doc, None), Ok(1));
    assert!(marks(&doc).is_empty());
    assert!(content(&doc, 0).contains("(AB) Tj"));
}

#[test]
fn unreadable_content_fails_closed() {
    let mut doc = one_page(b"", "", vec![]);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let bad = doc.add(Object::Stream(Stream::from_raw(
        {
            let mut d = Dict::new();
            d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
            d
        },
        b"garbage".to_vec(),
    )));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(bad))).unwrap();
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 50.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// A document with something in every hidden-information category.
fn hidden_fixture() -> Document {
    let content = b"BT /F1 10 Tf 10 100 Td (Visible) Tj 3 Tr (Hidden) Tj 0 Tr ET BT /F1 10 Tf 500 500 Td (Offpage) Tj ET /OC /L1 BDC BT /F1 10 Tf 10 50 Td (Layer) Tj ET EMC";
    let objs: Vec<Vec<u8>> = vec![
        // 1 catalog
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 8 0 R] >> /JavaScript << /Names [(init) 9 0 R] >> >> /OpenAction 9 0 R /Outlines 10 0 R /PageMode /UseOutlines /PieceInfo << /App << /Private 1 >> >> /OCProperties << /OCGs [13 0 R 14 0 R] /D << /OFF [13 0 R] /Order [13 0 R 14 0 R] >> >> /AcroForm << /Fields [17 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        // 3 page
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /Properties << /L1 13 0 R >> >> /Annots [15 0 R 16 0 R 17 0 R 18 0 R 19 0 R] /PieceInfo << /App << /Private 2 >> >> /AA << /O 9 0 R >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta/>"),
        b"<< /Type /Filespec /F (a.txt) /EF << /F 20 0 R >> >>".to_vec(),
        b"<< /S /JavaScript /JS (app.alert(1)) >>".to_vec(),
        b"<< /Type /Outlines /First 11 0 R /Last 12 0 R /Count 2 >>".to_vec(),
        b"<< /Title (One) /Parent 10 0 R /Next 12 0 R >>".to_vec(),
        b"<< /Title (Two) /Parent 10 0 R /Prev 11 0 R >>".to_vec(),
        b"<< /Type /OCG /Name (Secret layer) >>".to_vec(),
        b"<< /Type /OCG /Name (Shown layer) >>".to_vec(),
        // 15 comment + 16 its pop-up, 17 widget, 18 link, 19 attachment
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /Popup 16 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [60 10 160 60] /Parent 15 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 200 150 220] /A 9 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [10 230 50 240] /A << /S /URI /URI (https://example.org) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FileAttachment /Rect [200 10 210 20] /FS 8 0 R >>".to_vec(),
        stream("", b"attached"),
    ];
    let mut doc = pdf(objs);
    let info = doc.add(Object::Dict({
        let mut d = Dict::new();
        d.set(b"Title".to_vec(), pdfcraft_cos::PdfString::text("Secret plan"));
        d.set(b"Author".to_vec(), pdfcraft_cos::PdfString::text("Ada"));
        d
    }));
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    doc
}

#[test]
fn hidden_information_is_counted_and_removed() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize, scan};
    let mut doc = hidden_fixture();
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::Metadata], 3, "two Info entries and the XMP stream");
    assert_eq!(counts[&Hidden::Attachments], 2, "the embedded file and the attachment annotation");
    assert_eq!(counts[&Hidden::Comments], 1);
    assert_eq!(counts[&Hidden::FormFields], 1);
    assert_eq!(counts[&Hidden::HiddenText], 6 + 7, "Hidden (render mode 3) and Offpage (off the page)");
    assert_eq!(counts[&Hidden::HiddenLayers], 2, "one off layer and its block");
    assert_eq!(counts[&Hidden::Bookmarks], 2);
    assert_eq!(counts[&Hidden::LinksActionsScripts], 5, "link, open action, page actions, widget action, document script");
    assert_eq!(counts[&Hidden::PrivateData], 2);

    // Only hidden text first.
    let done = remove_hidden(&mut doc, &[Hidden::HiddenText]).unwrap();
    assert_eq!(done, [(Hidden::HiddenText, 13)]);
    let c = content(&doc, 0);
    assert!(c.contains("(Visible)") && !c.contains("Hidden") && !c.contains("Offpage") && c.contains("(Layer)"), "{c}");
    assert!(doc.full_save_required());

    // Then everything.
    let done = sanitize(&mut doc).unwrap();
    assert!(done.iter().all(|(h, _)| *h != Hidden::HiddenText));
    let c = content(&doc, 0);
    assert!(!c.contains("Layer") && c.contains("(Visible)"), "{c}");
    let doc = reopen(&doc);
    let after: Vec<(Hidden, usize)> = scan(&doc).into_iter().filter(|(_, n)| *n > 0).collect();
    // A full save writes a fresh /Info with the modification date only.
    assert!(after.iter().all(|(h, _)| *h == Hidden::Metadata), "{after:?}");
    let cat = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    for k in [&b"Outlines"[..], b"OpenAction", b"PieceInfo", b"AcroForm", b"Metadata"] {
        assert!(!cat.contains(k), "{}", String::from_utf8_lossy(k));
    }
    assert!(pdfcraft_forms::fields(&doc).is_empty());
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    assert!(annots_of(&doc, &p.dict).is_empty());
    let shown = doc.object_numbers().into_iter().any(|n| match &*doc.get(ObjRef::new(n, doc.generation(n))) {
        Object::Stream(s) => s.decoded().is_ok_and(|d| bytes_contain(&d, b"(Ada)")),
        _ => false,
    });
    assert!(shown, "the field's value stays visible as page content");
}

/// Issue #1325: removing comments keeps the annotations that are not comments: the multimedia
/// and print-production ones (Watermark, PrinterMark, TrapNet, Screen, Movie, RichMedia, 3D).
#[test]
fn removing_comments_keeps_annotations_that_are_not_comments() {
    use crate::sanitize::{Hidden, remove_hidden, scan};
    let kept = ["Link", "Watermark", "PrinterMark", "TrapNet", "Screen", "Movie", "RichMedia", "3D"];
    let annots: Vec<String> = (0..=kept.len()).map(|i| format!("{} 0 R", i + 4)).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << >> /Annots [{}] >>", annots.join(" ")).into_bytes(),
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] >>".to_vec(),
    ];
    objs.extend(kept.iter().map(|s| format!("<< /Type /Annot /Subtype /{s} /Rect [60 60 90 90] >>").into_bytes()));
    let mut doc = pdf(objs);
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::Comments], 1, "only the Square");
    assert_eq!(remove_hidden(&mut doc, &[Hidden::Comments]).unwrap(), [(Hidden::Comments, 1)]);
    let doc = reopen(&doc);
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let left: Vec<String> = annots_of(&doc, &p.dict)
        .iter()
        .filter_map(|a| doc.resolve(a).as_dict().and_then(|d| d.name(b"Subtype")).map(|s| String::from_utf8_lossy(s).into_owned()))
        .collect();
    assert_eq!(left, kept);
}

#[test]
fn overlay_text_takes_its_font_size_colour_alignment_and_repeats() {
    let mut doc = one_page(b"BT /F1 12 Tf 20 250 Td (Secret salary figures) Tj ET", "", Vec::new());
    let look = pdfcraft_annot::OverlayLook { font: pdfcraft_annot::OverlayFont::Courier, size: 8.0, color: [0.0, 0.0, 1.0], align: 0, repeat: true };
    let shape = Shape::Redact { quads: vec![rect_quad([10.0, 200.0, 290.0, 270.0])], overlay: "REDACTED".into(), look };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "T".into() }, &Meta::default()).unwrap();
    // Written as Acrobat writes it.
    let m = &marks(&doc)[0];
    assert_eq!(m.look, look, "the look round-trips through /DA, /Q and /Repeat");
    apply(&mut doc, None).unwrap();
    let doc = reopen(&doc);
    let c = content(&doc, 0);
    assert!(c.contains("0 0 1 rg /PCCour 8 Tf"), "{c}");
    // 70 pt high at 8 pt × 1.2 leading: several lines, each the word repeated.
    assert!(c.matches(" Tm (REDACTED REDACTED").count() >= 5, "{c}");
    assert!(c.contains("1 0 0 1 11 "), "left aligned at the area's edge: {c}");
    let p = &pdfcraft_model::pages(&doc)[0];
    let fonts = p
        .dict
        .get(b"Resources")
        .and_then(|r| doc.resolve(r).as_dict().cloned())
        .and_then(|r| r.get(b"Font").and_then(|f| doc.resolve(f).as_dict().cloned()))
        .unwrap();
    assert!(fonts.contains(b"PCCour") && !fonts.contains(b"PCTimes"));
}

fn repeated_overlay_document(width: f64, height: f64, repeat: bool) -> Document {
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Annots [5 0 R] >>".to_vec(),
        stream("", b"0 0 20 20 re f"),
        format!("<< /Type /Annot /Subtype /Redact /Rect [0 0 {width} {height}] /IC [0 0 0] /OverlayText (X) /DA (/Helv 2 Tf) /Repeat {repeat} >>")
            .into_bytes(),
    ])
}

#[test]
fn oversized_repeated_overlay_is_refused_before_changing_the_document() {
    // About 11 MiB of generated content before the fix; a bounded reproduction, not an OOM test.
    let mut doc = repeated_overlay_document(16000.0, 1600.0, true);
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    assert_eq!(apply(&mut doc, None), Err(RedactError::OverlayLimit));
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    assert_eq!(marks(&doc).len(), 1);
    let mut single = repeated_overlay_document(16000.0, 1600.0, false);
    apply(&mut single, None).unwrap();
    assert_eq!(content(&reopen(&single), 0).matches("(X) Tj").count(), 1);
}

#[test]
fn overlay_budget_is_shared_by_pages_and_respects_the_page_selection() {
    let mut doc = repeated_overlay_document(8000.0, 800.0, true);
    let page = (*doc.get(ObjRef::new(3, 0))).clone();
    let mut kids = vec![Object::Ref(ObjRef::new(3, 0))];
    for _ in 0..2 {
        let mut page = page.as_dict().unwrap().clone();
        let annotation = doc.add((*doc.get(ObjRef::new(5, 0))).clone());
        page.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annotation)]));
        kids.push(Object::Ref(doc.add(Object::Dict(page))));
    }
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(3));
    })
    .unwrap();
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    // Each appearance fits on its own; their sum exceeds the operation's budget.
    assert_eq!(apply(&mut doc, None), Err(RedactError::OverlayLimit));
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before);
    assert_eq!(marks(&doc).len(), 3);
    apply(&mut doc, Some(&[1])).unwrap();
    assert_eq!(marks(&doc).iter().map(|m| m.page).collect::<Vec<_>>(), [0, 2]);
}

#[test]
fn grouped_marks_keep_overlay_order_and_ignore_duplicate_page_selections() {
    let mut doc = repeated_overlay_document(60.0, 30.0, false);
    let page = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
    let original_mark = (*doc.get(ObjRef::new(5, 0))).clone();
    let mut kids = vec![Object::Ref(ObjRef::new(3, 0))];
    for _ in 0..2 {
        let mut copy = page.clone();
        let mark = doc.add(original_mark.clone());
        copy.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(mark)]));
        kids.push(Object::Ref(doc.add(Object::Dict(copy))));
    }
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(3));
    })
    .unwrap();
    let mut second = original_mark.as_dict().unwrap().clone();
    second.set(b"OverlayText".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"Y".to_vec())));
    let second = doc.add(Object::Dict(second));
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(5, 0)), Object::Ref(second)]));
    })
    .unwrap();
    let untouched = content(&doc, 1);
    let report = apply(&mut doc, Some(&[2, 0, 2])).unwrap();
    assert_eq!((report.pages, report.marks), (2, 3));
    let saved = reopen(&doc);
    assert_eq!(marks(&saved).iter().map(|m| m.page).collect::<Vec<_>>(), [1]);
    assert_eq!(content(&saved, 1), untouched);
    let first = content(&saved, 0);
    assert!(first.find("(X) Tj").unwrap() < first.find("(Y) Tj").unwrap());
    assert_eq!(content(&saved, 2).matches("(X) Tj").count(), 1);
}

#[test]
fn overlay_extreme_counts_and_invalid_geometry_are_refused_without_large_allocations() {
    let doc = repeated_overlay_document(20.0, 10.0, true);
    let original = marks(&doc).remove(0);
    for rect in [[0.0, 0.0, 1e100, 10.0], [0.0, 0.0, 20.0, 1e100], [-f64::MAX, 0.0, f64::MAX, 10.0], [0.0, 0.0, f64::NAN, 10.0]] {
        let mut mark = original.clone();
        mark.rects = vec![rect];
        let mut remaining = overlay::MAX_BYTES;
        assert_eq!(overlay::content(&[&mark], &mut remaining), Err(RedactError::OverlayLimit));
    }
}

#[test]
fn repeated_overlay_keeps_its_exact_appearance_and_counts_escaped_bytes() {
    let doc = repeated_overlay_document(20.0, 10.0, true);
    let mut mark = marks(&doc).remove(0);
    let expected = b"q 0 0 0 rg\n0 0 20 10 re f\nQ\nq 0 0 20 10 re W n BT 1 0 0 rg /PCHelv 2 Tf 1 0 0 1 1.773 7.9 Tm (X X X X X X X X X) Tj 1 0 0 1 1.773 5.5 Tm (X X X X X X X X X) Tj 1 0 0 1 1.773 3.1 Tm (X X X X X X X X X) Tj 1 0 0 1 1.773 0.7 Tm (X X X X X X X X X) Tj ET Q\n";
    let mut remaining = expected.len();
    assert_eq!(overlay::content(&[&mark], &mut remaining).unwrap(), expected);
    assert_eq!(remaining, 0);
    mark.overlay = "é(\\)".into();
    let mut remaining = overlay::MAX_BYTES;
    let encoded = overlay::content(&[&mark], &mut remaining).unwrap();
    let escaped = b"\xe9\\(\\\\\\)";
    assert!(encoded.windows(escaped.len()).any(|w| w == escaped));
    assert_eq!(overlay::MAX_BYTES - remaining, encoded.len());
    let mut too_small = encoded.len() - 1;
    assert_eq!(overlay::content(&[&mark], &mut too_small), Err(RedactError::OverlayLimit));
}

#[test]
#[ignore = "manual overlay timing probe"]
fn repeated_overlay_timing_probe() {
    use std::hash::{Hash, Hasher};
    for (w, h) in [(20.0, 10.0), (2000.0, 200.0), (4000.0, 400.0), (8000.0, 800.0)] {
        let doc = repeated_overlay_document(w, h, true);
        let marks = marks(&doc);
        let refs: Vec<_> = marks.iter().collect();
        let start = std::time::Instant::now();
        let mut bytes = Vec::new();
        for _ in 0..20 {
            let mut remaining = overlay::MAX_BYTES;
            bytes = std::hint::black_box(overlay::content(std::hint::black_box(&refs), &mut remaining).unwrap());
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0 / 20.0;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hash);
        println!("{w}x{h}: bytes={} hash={} ms={elapsed:.3}", bytes.len(), hash.finish());
    }
}

/// Overlay text the standard fonts can't draw: refused when it is marked, and for a mark another
/// program made, refused when it is applied, before anything is redacted, rather than burnt into
/// the page as `?`.
#[test]
fn overlay_text_the_standard_fonts_cannot_draw_is_refused() {
    let mut doc = one_page(b"BT /F1 12 Tf 20 250 Td (Secret salary figures) Tj ET", "", Vec::new());
    let look = pdfcraft_annot::OverlayLook::default();
    let mark = |overlay: &str| Shape::Redact { quads: vec![rect_quad([10.0, 200.0, 290.0, 270.0])], overlay: overlay.into(), look };
    let add = |doc: &mut Document, shape: Shape| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "T".into() }, &Meta::default())
    };
    let err = add(&mut doc, mark("机密")).unwrap_err().to_string();
    assert!(err.contains("\"机\" (U+673A)") && err.contains("overlay text"), "{err}");
    assert!(marks(&doc).is_empty());
    // A mark whose overlay text was written by another program.
    add(&mut doc, mark("REDACTED")).unwrap();
    let obj = marks(&doc)[0].obj;
    doc.update_dict(obj, |d| d.set(b"OverlayText".to_vec(), pdfcraft_cos::PdfString::text("机密"))).unwrap();
    let before = content(&doc, 0);
    let err = apply(&mut doc, None).unwrap_err();
    assert!(
        matches!(&err, RedactError::Undrawable(m) if m.contains("page 1") && m.contains("U+673A") && m.contains("nothing was redacted")),
        "{err:?}"
    );
    assert_eq!(content(&doc, 0), before, "nothing was redacted");
    assert_eq!(marks(&doc).len(), 1, "the mark is still there");
    // Cleared, it applies.
    doc.update_dict(obj, |d| d.set(b"OverlayText".to_vec(), pdfcraft_cos::PdfString::text(""))).unwrap();
    apply(&mut doc, None).unwrap();
    assert!(!content(&reopen(&doc), 0).contains("Secret"));
}

#[test]
fn redaction_codes_join_in_set_order_and_reject_strangers() {
    use crate::codes::{CODE_SETS, CodeSet};
    let foia = CodeSet::from_id("foia").unwrap();
    assert_eq!(foia.overlay(&["(b)(6)", "(b)(1)(A)", "(b)(6)"]), Ok("(b)(1)(A), (b)(6)".into()));
    assert_eq!(foia.overlay(&[]), Ok(String::new()));
    assert_eq!(foia.overlay(&["(b)(6)", "(k)(1)"]), Err("(k)(1)"));
    let privacy = CodeSet::from_id("privacy-act").unwrap();
    assert_eq!(privacy.overlay(&["(k)(7)", "(d)(5)"]), Ok("(d)(5), (k)(7)".into()));
    assert_eq!(CodeSet::from_id("gdpr"), None);
    assert!(CODE_SETS.iter().all(|s| !s.codes.is_empty() && s.codes.iter().all(|c| !c.is_empty())));
}

#[test]
fn tags_lose_what_redaction_removed() {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /K [9 0 R 10 0 R] >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (SECRET) /Alt (the secret) /K 0 >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (Public) /K 1 >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.tags, 1);
    let secret = doc.get(pdfcraft_cos::ObjRef::new(9, 0)).as_dict().cloned().unwrap();
    assert!(secret.get(b"ActualText").is_none() && secret.get(b"Alt").is_none());
    assert!(secret.get(b"K").is_none(), "its marked content is empty now");
    let public = doc.get(pdfcraft_cos::ObjRef::new(10, 0)).as_dict().cloned().unwrap();
    assert_eq!(public.get(b"K").and_then(Object::as_int), Some(1));
    assert!(public.get(b"ActualText").is_some(), "untouched content keeps its tags");
}

/// Fail when `secret` is anywhere in the file a save would write (see [`saved_contains`]).
fn assert_no_secret(doc: &Document, secret: &[u8]) {
    assert!(!saved_contains(doc, secret), "{} remains in the saved file", String::from_utf8_lossy(secret));
}

#[test]
fn named_mcids_and_indirect_content_references_lose_descriptions() {
    let mut doc = one_page(
        b"/P /P0 BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC",
        "/Properties << /P0 7 0 R >>",
        vec![
            b"<< /MCID 0 >>".to_vec(),
            b"<< /Type /StructTreeRoot /K 9 0 R >>".to_vec(),
            b"<< /S /Document /P 8 0 R /K [10 0 R 11 0 R] >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (LEAK_TAG) /K 12 0 R >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (Public) /K 1 >>".to_vec(),
            b"<< /Type /MCR /Pg 3 0 R /MCID 0 >>".to_vec(),
        ],
    );
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"StructTreeRoot".to_vec(), Object::Ref(ObjRef::new(8, 0)))).unwrap();
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().tags, 1);
    assert_no_secret(&doc, b"LEAK_");
    assert!(!doc.get(ObjRef::new(10, 0)).as_dict().unwrap().contains(b"K"));
    assert_eq!(doc.get(ObjRef::new(11, 0)).as_dict().unwrap().get(b"ActualText").unwrap().as_string().unwrap().to_text(), "Public");
}

#[test]
fn form_tags_are_cleaned_and_surviving_references_point_at_the_copy() {
    let mut doc = one_page(
        b"/Fm Do",
        "/XObject << /Fm 7 0 R >>",
        vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /StructParents 0 /Resources << /Font << /F1 5 0 R >> /Properties << /P0 << /MCID 0 >> >> >>",
                b"/P /P0 BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC",
            ),
            b"<< /Type /StructTreeRoot /K 9 0 R /ParentTree 14 0 R >>".to_vec(),
            b"<< /S /Document /P 8 0 R /K [10 0 R 11 0 R] >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (LEAK_FORM_TAG) /K 12 0 R >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (Public) /K 13 0 R >>".to_vec(),
            b"<< /Type /MCR /Pg 3 0 R /Stm 7 0 R /MCID 0 >>".to_vec(),
            b"<< /Type /MCR /Pg 3 0 R /Stm 7 0 R /StmOwn 7 0 R /MCID 1 >>".to_vec(),
            b"<< /Nums [0 [10 0 R 11 0 R]] >>".to_vec(),
        ],
    );
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"StructTreeRoot".to_vec(), Object::Ref(ObjRef::new(8, 0)))).unwrap();
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(&mut doc, None).unwrap();
    assert_no_secret(&doc, b"LEAK_");
    assert_no_secret(&doc, b"SECRET");
    assert!(!doc.get(ObjRef::new(10, 0)).as_dict().unwrap().contains(b"K"));
    let reference = doc.get(ObjRef::new(13, 0)).as_dict().unwrap().get(b"Stm").unwrap().as_ref().unwrap();
    assert_ne!(reference, ObjRef::new(7, 0));
    assert_eq!(doc.get(ObjRef::new(13, 0)).as_dict().unwrap().get(b"StmOwn").unwrap().as_ref(), Some(reference));
    let Object::Stream(s) = &*doc.get(reference) else { panic!() };
    assert!(String::from_utf8(s.decoded().unwrap()).unwrap().contains("Public"));
    assert!(doc.get(ObjRef::new(11, 0)).as_dict().unwrap().contains(b"ActualText"));
}

#[test]
fn inherited_resources_do_not_keep_the_original_form() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT /F1 10 Tf 10 200 Td (LEAK_INHERITED) Tj ET");
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    let resources = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().get(b"Resources").unwrap().clone();
    doc.update_dict(ObjRef::new(2, 0), |d| d.set(b"Resources".to_vec(), resources)).unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.remove(b"Resources");
    })
    .unwrap();
    mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
    apply(&mut doc, None).unwrap();
    assert_no_secret(&doc, b"LEAK_");
    assert!(!doc.get(ObjRef::new(2, 0)).as_dict().unwrap().contains(b"Resources"));
}

#[test]
fn unselected_pages_keep_inherited_forms() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET");
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    let mut page = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
    let resources = page.remove(b"Resources").unwrap();
    let second = doc.add(Object::Dict(page));
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Resources".to_vec(), resources);
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(second)]));
        d.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.remove(b"Resources");
    })
    .unwrap();
    mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
    apply(&mut doc, Some(&[0])).unwrap();
    let mut saved = reopen(&doc);
    assert_eq!(under(&mut saved, 0, &[[0.0, 190.0, 300.0, 220.0]]), 0);
    assert_eq!(under(&mut saved, 1, &[[0.0, 190.0, 300.0, 220.0]]), 6);
}

/// A page tree of `n` pages that all draw form /Fm and inherit a direct `/Resources` from the root.
fn inheriting_pages(n: usize) -> (Document, Vec<ObjRef>) {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET");
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    let mut page = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
    let resources = page.remove(b"Resources").unwrap();
    doc.set(ObjRef::new(3, 0), Object::Dict(page.clone()));
    let mut kids = vec![ObjRef::new(3, 0)];
    kids.extend((1..n).map(|_| doc.add(Object::Dict(page.clone()))));
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Resources".to_vec(), resources);
        d.set(b"Kids".to_vec(), Object::Array(kids.iter().map(|r| Object::Ref(*r)).collect()));
        d.set(b"Count".to_vec(), Object::Int(n as i64));
    })
    .unwrap();
    let saved = reopen(&doc);
    (saved, kids)
}

#[test]
fn redacting_one_page_leaves_the_other_pages_untouched() {
    let (mut doc, kids) = inheriting_pages(50);
    mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
    let before = doc.modified_objects();
    apply(&mut doc, Some(&[0])).unwrap();
    let changed: Vec<u32> = doc.modified_objects().into_iter().filter(|n| !before.contains(n)).collect();
    for kid in &kids[1..] {
        assert!(!changed.contains(&kid.num), "unredacted page {kid:?} was rewritten");
    }
    // The page, its new content and resources, the cleaned form, the parent and one shared
    // resources object: not one object per page.
    assert!(changed.len() < 10, "{changed:?}");
    // The parent keeps the resources the other pages draw with, through a single shared object.
    let parent = doc.get(ObjRef::new(2, 0)).as_dict().unwrap().clone();
    assert!(parent.get(b"Resources").and_then(Object::as_ref).is_some());
    let mut saved = reopen(&doc);
    assert_eq!(under(&mut saved, 0, &[[0.0, 190.0, 300.0, 220.0]]), 0);
    assert_eq!(under(&mut saved, 49, &[[0.0, 190.0, 300.0, 220.0]]), 6);
}

#[test]
fn redacting_every_inheriting_page_retires_the_parent_resources() {
    let (mut doc, kids) = inheriting_pages(20);
    for page in 0..kids.len() {
        mark(&mut doc, page, &[[0.0, 190.0, 300.0, 220.0]], "");
    }
    apply(&mut doc, None).unwrap();
    assert!(!doc.get(ObjRef::new(2, 0)).as_dict().unwrap().contains(b"Resources"));
    assert_no_secret(&doc, b"SECRET");
    let mut saved = reopen(&doc);
    for page in 0..kids.len() {
        assert!(pdfcraft_model::pages(&saved)[page].dict.contains(b"Resources"));
        assert_eq!(under(&mut saved, page, &[[0.0, 190.0, 300.0, 220.0]]), 0);
    }
}

/// Two field dictionaries can share one fully qualified name (merged or repeated forms). The
/// field whose widget is under the mark is the one to delete, by object: deleting the first
/// field of that name leaves the marked value in the saved file (and removes an unmarked field).
#[test]
fn redacting_one_of_two_same_named_fields_deletes_that_field_by_object() {
    let make = || {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [5 0 R 6 0 R] >>".to_vec(),
            stream("", b""),
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (acct) /V (V034-KEEP-CCC) /Rect [50 200 250 220] /P 3 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (acct) /V (V034-GONE-DDD) /Rect [50 100 250 120] /P 3 0 R >>".to_vec(),
        ])
    };
    // Only the second field of the name is marked: it goes, the first keeps its value. Applying
    // forces a full rewrite, so the saved file is that rewrite.
    let mut doc = make();
    assert_eq!(pdfcraft_forms::fields(&doc).len(), 2, "both same-named fields are listed");
    mark(&mut doc, 0, &[[45.0, 95.0, 255.0, 125.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.fields, 1, "{r:?}");
    assert!(!saved_contains(&doc, b"V034-GONE-DDD"), "the marked field's value survived the save");
    assert!(saved_contains(&doc, b"V034-KEEP-CCC"), "the unmarked field was deleted instead");
    let left = pdfcraft_forms::fields(&reopen(&doc));
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].value, ["V034-KEEP-CCC"]);

    // Both marked: both go.
    let mut doc = make();
    mark(&mut doc, 0, &[[45.0, 95.0, 255.0, 125.0], [45.0, 195.0, 255.0, 225.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.fields, 2, "{r:?}");
    assert!(!saved_contains(&doc, b"V034-GONE-DDD") && !saved_contains(&doc, b"V034-KEEP-CCC"), "a marked value survived the save");
    assert!(pdfcraft_forms::fields(&reopen(&doc)).is_empty());
}

/// The samples cleared under a mark must be the ones the page shows there. The renderer resolves
/// an image's colour space by rules of its own (the short /CS key first, an ICC profile's
/// /Alternate when the profile does not load, DeviceGray for anything it cannot resolve), so a
/// layout the redactor cannot be sure of must remove the image whole rather than clear bytes the
/// renderer never draws (here: padding after the real samples).
#[test]
fn an_image_whose_sample_layout_is_uncertain_is_removed_whole() {
    // 8×8 gray samples; rows 4–5 hold the marker; padded to a 3- or 5-component length.
    let samples = |len: usize| {
        let mut d = vec![0x4Du8; 32];
        d.extend_from_slice(b"REDIMGSECRETROW!");
        d.extend(vec![0x4Du8; 16]);
        d.extend(vec![0xFFu8; len - 64]);
        d
    };
    let draw = b"q 80 0 0 80 60 110 cm /Im1 Do Q";
    let variants: [(&str, &str, usize); 6] = [
        ("icc N 3 with a gray alternate and a profile that does not load", "/ColorSpace [/ICCBased 8 0 R]", 192),
        ("bare CalRGB", "/ColorSpace /CalRGB", 192),
        ("short /CS key beside a different /ColorSpace", "/CS /DeviceGray /ColorSpace /DeviceRGB", 192),
        ("icc N 5 without an alternate", "/ColorSpace [/ICCBased 9 0 R]", 320),
        ("CalRGB array without its dictionary", "/ColorSpace [/CalRGB]", 192),
        ("short /F key beside /Filter", "/ColorSpace /DeviceGray /F /ASCIIHexDecode", 64),
    ];
    for (what, cs, len) in variants {
        let image = stream(&format!("/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 {cs}"), samples(len));
        let profiles = vec![stream("/N 3 /Alternate /DeviceGray", b"JUNK"), stream("/N 5", b"JUNK")];
        let mut doc = one_page(draw, "/XObject << /Im1 7 0 R >>", [vec![image], profiles].concat());
        // Rows 4 and 5 (y 130–150 of the 80 pt image at y 110).
        mark(&mut doc, 0, &[[60.0, 130.0, 140.0, 150.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert!(!saved_contains(&doc, b"REDIMGSECRETROW!"), "{what}: the marked samples survived the save");
        assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{what}: {r:?}");
    }
    // The control, a plain DeviceGray image, is cleared in place and keeps its other rows.
    let image = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceGray", samples(64));
    let mut doc = one_page(draw, "/XObject << /Im1 7 0 R >>", vec![image]);
    mark(&mut doc, 0, &[[60.0, 130.0, 140.0, 150.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (0, 1), "control: {r:?}");
    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let cleared = saved.scan_objects().find_map(|(_, o)| match &*o {
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => Some(s.decoded().unwrap()),
        _ => None,
    });
    assert_eq!(cleared.as_deref(), Some(&[vec![0x4Du8; 32], vec![0u8; 16], vec![0x4Du8; 16]].concat()[..]), "control");
}

/// An inline image whose data runs straight into `EI` (no white space) used to swallow the rest
/// of the stream as image data, so the text after it was neither redacted nor verified while the
/// renderer drew it. A filter terminator before `EI` (`>` for ASCIIHex, `~>` for ASCII85) ends the
/// data; otherwise the page is unreadable and apply fails closed.
#[test]
fn content_after_an_inline_image_without_white_space_before_ei_is_redacted_or_refused() {
    let text = b" Q BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
    // ASCIIHex data ending in its `>` terminator, then `EI` at once: the text after it is redacted.
    let mut src = b"q 10 0 0 10 250 20 cm BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 00>EI".to_vec();
    src.extend_from_slice(text);
    let mut doc = one_page(&src, "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 4, "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("1234") && c.contains("BI") && c.contains("(CD)"), "{c}");
    assert!(!content(&reopen(&doc), 0).contains("1234"));

    // Binary data with no terminator before `EI`: the image's end cannot be told, so nothing is
    // changed rather than the following text left under the box.
    let mut src = b"q 10 0 0 10 250 20 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x80EI".to_vec();
    src.extend_from_slice(text);
    let mut doc = one_page(&src, "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
    assert!(content(&doc, 0).contains("(AB1234CD) Tj"), "nothing was changed");
}

/// Twenty 16-byte gray rows, `secret` as the last one, each tagged with a PNG `None` predictor
/// byte when `tagged`; and the page that draws the 16×20 image at [20 50 180 250] with the
/// bottom row (y 50–60) marked.
fn rows_image(tagged: bool, secret: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    for j in 0..20 {
        if tagged {
            data.push(0);
        }
        if j == 19 {
            data.extend_from_slice(secret);
        } else {
            data.extend_from_slice(format!("publicrow{j:02}.....").as_bytes());
        }
    }
    data
}

fn zlib(data: &[u8]) -> Vec<u8> {
    Stream::flate(Dict::new(), data).raw.to_vec()
}

/// `/Filter` and `/DecodeParms` may be indirect references (ISO 32000-2 §7.3.8.2), and a viewer
/// resolves them: the content it shows, and the user marks, is the decoded one. Read as an empty
/// chain, the still-compressed bytes were "redacted" (nothing found), verified clean, and saved;
/// an image's predicted bytes were cleared at the wrong offsets.
#[test]
fn indirect_filter_and_decode_parms_are_resolved_before_redacting() {
    // Content long enough that zlib really compresses it rather than storing it.
    let mut text = b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET ".to_vec();
    text.extend(std::iter::repeat_n(b"BT /F1 10 Tf 10 200 Td (KEEP) Tj ET ", 12).flatten());
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 8 0 R >> >> >>"
            .to_vec(),
        stream("/Filter 7 0 R", zlib(&text)),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"/FlateDecode".to_vec(),
        stream(
            "/Type /XObject /Subtype /Image /Width 16 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms 9 0 R",
            zlib(&rows_image(true, b"COLORSECRETROW19")),
        ),
        b"<< /Predictor 15 /Columns 16 >>".to_vec(),
    ]);
    // The image is drawn by a second content stream so the text stream above stays as written.
    let draw = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 160 0 0 200 20 50 cm /Im1 Do Q".to_vec())));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(4, 0)), Object::Ref(draw)])))
        .unwrap();
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0], [0.0, 50.0, 300.0, 60.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.images_cleared, r.images_removed), (4, 1, 0), "{r:?}");
    assert!(!saved_contains(&doc, b"1234"), "the marked text survived the save");
    assert!(!saved_contains(&doc, b"COLORSECRETROW19"), "the marked image row survived the save");
    let streams = saved_streams(&doc);
    assert!(any_has(&streams, b"(KEEP)") && any_has(&streams, b"publicrow18"), "the rest stays");
}

/// What the user marks is the viewer's decode of a stream. A stream cos cannot decode cleanly
/// and completely (a /Predictor or /Colors out of range, a zlib header with the preset-dictionary
/// bit, an image carrying the inline-image /DP key) used to pass as an empty or differently laid
/// out decode, so apply reported success with the marked content still in the file. It fails
/// closed instead: the page is unreadable, the image is removed whole.
#[test]
fn streams_cos_cannot_decode_cleanly_fail_closed() {
    let content = b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
    let mut tagged = vec![0u8];
    tagged.extend_from_slice(content);
    let mut fdict = vec![0x78, 0xBB, 0, 0, 0, 1, 1, content.len() as u8, 0, !(content.len() as u8), 0xFF];
    fdict.extend_from_slice(content);
    fdict.extend_from_slice(&[0, 0, 0, 0]);
    let cases: [(&str, String, Vec<u8>); 3] = [
        ("predictor 16", format!("/Filter /FlateDecode /DecodeParms << /Predictor 16 /Columns {} >>", content.len()), zlib(&tagged)),
        ("colors 256", format!("/Filter /FlateDecode /DecodeParms << /Predictor 12 /Colors 256 /Columns {} >>", content.len()), zlib(&tagged)),
        ("preset-dictionary zlib header", "/Filter /FlateDecode".into(), fdict),
    ];
    for (what, dict, data) in cases {
        let mut doc = one_page(b"", "", vec![]);
        let bad = doc.add(Object::Stream(Stream::from_raw(
            pdfcraft_cos::Lexer::new(format!("<< {dict} >>").as_bytes(), 0).object().unwrap().as_dict().cloned().unwrap(),
            data,
        )));
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Contents".to_vec(), Object::Ref(bad))).unwrap();
        mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
        assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)), "{what}");
    }
    // The control: a clean PNG-predicted stream decodes and is redacted.
    let mut doc = one_page(b"", "", vec![]);
    let dict = format!("<< /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns {} >> >>", content.len());
    let good = doc.add(Object::Stream(Stream::from_raw(
        pdfcraft_cos::Lexer::new(dict.as_bytes(), 0).object().unwrap().as_dict().cloned().unwrap(),
        zlib(&tagged),
    )));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Contents".to_vec(), Object::Ref(good))).unwrap();
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 4, "control");
    assert!(!saved_contains(&doc, b"1234"));

    // An image carrying /DP: the viewer unpredicts it, cos would not. Removed whole.
    let image = stream(
        "/Type /XObject /Subtype /Image /Width 16 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /DP << /Predictor 15 /Columns 16 >>",
        zlib(&rows_image(true, b"COLORSECRETROW19")),
    );
    let mut doc = one_page(b"q 160 0 0 200 20 50 cm /Im1 Do Q", "/XObject << /Im1 7 0 R >>", vec![image]);
    mark(&mut doc, 0, &[[0.0, 50.0, 300.0, 60.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(!saved_contains(&doc, b"COLORSECRETROW19"), "the marked image row survived the save: {r:?}");
    assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{r:?}");
}

/// The renderer takes a font's writing mode from the parsed CMap: `/WMode 1 def` in an embedded
/// CMap's program (whatever its stream dictionary says) and the predefined CMap named plainly `V`
/// lay the glyphs out vertically. The fail-closed check must see them as the `-V` names and a
/// stream-dictionary `/WMode` are seen, or the marked column is never matched.
#[test]
fn vertical_writing_from_the_cmap_program_or_the_cmap_named_v_fails_closed() {
    let cmap = |wmode: u8| {
        format!(
            "%!PS-Adobe-3.0 Resource-CMap\n/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n/CMapName /TestV def\n/WMode {wmode} def\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n1 begincidrange <0000> <FFFF> 0 endcidrange\nendcmap CMapName currentdict /CMap defineresource pop end end"
        )
    };
    let make = |encoding: &str, extra: Vec<Vec<u8>>| -> Document {
        let t0 = format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding {encoding} /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 /W [1 [500 500] 3 4 250] >>] >>"
        );
        let mut doc = one_page(b"BT /F2 10 Tf 0 1 -1 0 100 50 Tm <0001000200030004> Tj ET", "", extra);
        let font_ref = doc.add(pdfcraft_cos::Lexer::new(t0.as_bytes(), 0).object().unwrap());
        doc.update_dict(ObjRef::new(3, 0), |d| {
            let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
            let mut fonts = res.get(b"Font").and_then(Object::as_dict).cloned().unwrap();
            fonts.set(b"F2".to_vec(), Object::Ref(font_ref));
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            d.set(b"Resources".to_vec(), Object::Dict(res));
        })
        .unwrap();
        mark(&mut doc, 0, &[[85.0, 55.5, 105.0, 61.0]], "");
        doc
    };
    // The embedded CMap's program says vertical; its stream dictionary has no /WMode.
    let mut doc = make(
        "7 0 R",
        vec![stream("/Type /CMap /CMapName /TestV /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>", cmap(1).as_bytes())],
    );
    assert_eq!(apply(&mut doc, None), Err(RedactError::VerticalText(1)), "WMode 1 in the CMap program");
    // The predefined vertical CMap whose name does not end in -V.
    let mut doc = make("/V", vec![]);
    assert_eq!(apply(&mut doc, None), Err(RedactError::VerticalText(1)), "the CMap named V");
    // Control: the same embedded CMap with /WMode 0 is horizontal and redacted normally.
    let mut doc = make(
        "7 0 R",
        vec![stream("/Type /CMap /CMapName /TestV /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>", cmap(0).as_bytes())],
    );
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2, "the horizontal control is redacted");
}

/// The font and size may come from an ExtGState's /Font entry (`gs`) instead of `Tf`, or a form
/// XObject may show text with the font and size its caller selected. The interpreter used to
/// learn them from `Tf` in the current scope only, so such text stayed at size 0 at the line's
/// start, matched no mark further along the line, and survived apply and its verify pass.
#[test]
fn text_set_through_an_extgstate_font_or_inherited_by_a_form_is_redacted() {
    // "AB1234CD" from x = 10 at 10 pt: 1234 spans x 20–40, away from the line's start.
    let mut doc = one_page(b"BT /GS1 gs 10 100 Td (AB1234CD) Tj ET", "/ExtGState << /GS1 << /Type /ExtGState /Font [5 0 R 10] >> >>", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 4, "ExtGState font: {r:?}");
    let c = content(&doc, 0);
    assert!(c.contains("[(AB) -2000 (CD)] TJ") && !c.contains("1234"), "{c}");

    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT 10 100 Td (AB1234CD) Tj ET");
    let mut doc = one_page(b"BT /F1 10 Tf ET /Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_rewritten), (4, 1), "inherited font: {r:?}");
    assert!(!saved_contains(&doc, b"1234"), "the form's text survived the save");
}

/// An annotation held directly in the page's /Annots array (not by reference) is an annotation
/// all the same: under a mark it goes, with its link target or comment text, and a direct pop-up
/// of a removed annotation goes with it.
#[test]
fn annotations_written_directly_in_annots_are_removed_under_a_mark() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [\
          << /Type /Annot /Subtype /Link /Rect [10 195 60 212] /A << /S /URI /URI (http://leak.example/LEAKDIRECTLINK) >> >> \
          << /Type /Annot /Subtype /Text /Rect [10 195 30 212] /Contents (LEAKDIRECTNOTE) /Popup 7 0 R >> \
          << /Type /Annot /Subtype /Popup /Rect [100 100 200 150] /Parent 7 0 R >> \
          7 0 R \
          << /Type /Annot /Subtype /Square /Rect [200 20 240 60] /Contents (KEEPDIRECTFAR) >> ] >>"
            .to_vec(),
        stream("", b"BT /F1 10 Tf 10 200 Td (VISIBLE77) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Annot /Subtype /Square /Rect [10 195 40 212] /Contents (LEAKINDIRECT) >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[5.0, 190.0, 125.0, 215.0]], "");
    let r = apply(&mut doc, None).unwrap();
    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let mut texts = Vec::new();
    for (_, object) in saved.scan_objects() {
        let mut bytes = Vec::new();
        pdfcraft_cos::serialize(&object, &mut bytes);
        texts.push(bytes);
    }
    for secret in [&b"LEAKDIRECTLINK"[..], b"LEAKDIRECTNOTE", b"LEAKINDIRECT", b"VISIBLE77"] {
        assert!(!any_has(&texts, secret), "{} survived the save", String::from_utf8_lossy(secret));
    }
    assert!(any_has(&texts, b"KEEPDIRECTFAR"));
    assert_eq!(r.annotations, 3, "{r:?}");
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let kept: Vec<String> = annots_of(&doc, &p.dict)
        .iter()
        .filter_map(|a| doc.resolve(a).as_dict().and_then(|d| d.name(b"Subtype")).map(|s| String::from_utf8_lossy(s).into_owned()))
        .collect();
    assert_eq!(kept, ["Square"], "only the far square stays");
}

// ── Audit run 4, medium findings (hidden information / sanitize) ─────────────────────────────

/// An off layer stored as a marked-content block (`/OC /L1 BDC … EMC`) that draws an image and a
/// form XObject: the block goes, but nothing unlinked the XObjects it alone drew, so the full
/// rewrite on save wrote their bytes (and the layer's name) into the sanitized file.
fn off_layer_block_fixture() -> Document {
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << /OFF [6 0 R] /Order [6 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /ImOC 7 0 R /ImB 8 0 R /FmB 9 0 R >> /Properties << /L1 6 0 R >> >> >>".to_vec(),
        stream(
            "",
            b"BT /F1 12 Tf 20 20 Td (PUBLIC) Tj ET q 50 0 0 50 20 100 cm /ImOC Do Q /OC /L1 BDC BT /F1 12 Tf 20 60 Td (LAYERBLOCKTEXT) Tj ET q 50 0 0 50 100 100 cm /ImB Do Q /FmB Do EMC",
        ),
        // The font has widths of its own (10), so nothing but the layer's own use reaches the OCG.
        FONT.replace("95 0 R", "10 0 R").into_bytes(),
        b"<< /Type /OCG /Name (SECRETLAYERNAME) >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OC 6 0 R", b"LAYEROCIMAGEBYTE"),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"LAYERBLOCKIMAGEB"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 20 230 Td (LAYERBLOCKFORMTEXT) Tj ET"),
        widths(),
    ])
}

#[test]
fn off_layer_blocks_take_the_images_and_forms_they_draw_with_them() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize};
    for all in [false, true] {
        let mut doc = off_layer_block_fixture();
        let done = if all { sanitize(&mut doc).unwrap() } else { remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap() };
        assert!(done.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0), "{done:?}");
        let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(bytes_contain(&bytes, b"PUBLIC"), "the visible text stays (sanitize: {all})");
        for secret in [&b"LAYERBLOCKTEXT"[..], b"LAYEROCIMAGEBYTE", b"LAYERBLOCKIMAGEB", b"LAYERBLOCKFORMTEXT"] {
            assert!(!bytes_contain(&bytes, secret), "{} survived the save (sanitize: {all})", String::from_utf8_lossy(secret));
        }
        // A full save packs the layer's dictionary into a compressed object stream, where the
        // file bytes do not show it: search the objects themselves.
        assert!(!saved_contains(&doc, b"SECRETLAYERNAME"), "the off layer's name survived the save (sanitize: {all})");
        let saved = Document::open(Arc::new(bytes)).unwrap();
        for name in [&b"ImOC"[..], b"ImB", b"FmB"] {
            assert_eq!(named_image(&saved, 0, name), None, "/{} is still named by the page (sanitize: {all})", String::from_utf8_lossy(name));
        }
    }
}

/// The same block inside a form XObject, drawing an image through the form's own resources.
#[test]
fn off_layer_blocks_inside_a_form_take_their_images_with_them() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /Fm 6 0 R >> >> >>".to_vec(),
        stream("", b"/Fm Do"),
        b"<< /Type /OCG /Name (SECRETLAYERNAME) >>".to_vec(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /ImB 7 0 R /ImKeep 8 0 R >> /Properties << /L1 5 0 R >> >>",
            b"q 50 0 0 50 20 20 cm /ImKeep Do Q /OC /L1 BDC q 50 0 0 50 100 100 cm /ImB Do Q EMC",
        ),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"LAYERBLOCKIMAGEB"),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBIMGKEEPSURVIV"),
    ]);
    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(bytes_contain(&bytes, b"PUBIMGKEEPSURVIV"), "the image drawn outside the block stays");
    assert!(!bytes_contain(&bytes, b"LAYERBLOCKIMAGEB"), "the image the off layer's block drew inside the form survived the save");
    assert!(!bytes_contain(&bytes, b"SECRETLAYERNAME"), "the off layer's name survived the save");
}

/// Whether `needle` occurs in `haystack` (the bytes of a saved file, or one decoded stream).
fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Whether `needle` is anywhere in the file a save would write: in an object's serialized form
/// or in a decoded stream (the save is a full rewrite once anything was removed). A stream that
/// does not decode fails the test, so nothing can hide in one.
fn saved_contains(doc: &Document, needle: &[u8]) -> bool {
    let saved = reopen(doc);
    saved.scan_objects().any(|(r, object)| {
        let mut bytes = Vec::new();
        pdfcraft_cos::serialize(&object, &mut bytes);
        bytes_contain(&bytes, needle)
            || matches!(&*object, Object::Stream(s) if bytes_contain(&s.decoded().unwrap_or_else(|e| panic!("stream {r:?} does not decode: {e}")), needle))
    })
}

/// The decoded streams of a full-rewrite save, for checks that what stays is still drawn (a
/// stream that does not decode fails the test).
fn saved_streams(doc: &Document) -> Vec<Vec<u8>> {
    let saved = Document::open(Arc::new(write_full(doc, &SaveOptions::default()).unwrap())).unwrap();
    saved
        .scan_objects()
        .filter_map(|(r, o)| match &*o {
            Object::Stream(s) => Some(s.decoded().unwrap_or_else(|e| panic!("stream {r:?} does not decode: {e}"))),
            _ => None,
        })
        .collect()
}

/// Whether any of `texts` holds `needle`.
fn any_has(texts: &[Vec<u8>], needle: &[u8]) -> bool {
    texts.iter().any(|t| bytes_contain(t, needle))
}

/// Whether the saved file holds a dictionary `pred` accepts (stream dictionaries included).
fn saved_has_dict(doc: &Document, pred: impl Fn(&Dict) -> bool) -> bool {
    reopen(doc).scan_objects().any(|(_, object)| object.as_dict().is_some_and(&pred))
}

/// A tagged page whose structure tree holds OBJR kids for a text field's widget (7), a FreeText
/// comment (8), a link (9) and a file attachment (10); a reply (11) points at the comment with
/// /IRT and the form's /CO names the field. Taking an annotation out of /Annots (or a field out
/// of the field tree) left it reachable through those referrers, so the full rewrite wrote it.
fn referenced_annotations_fixture() -> Document {
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [7 0 R] /CO [7 0 R] >> /StructTreeRoot 12 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [7 0 R 8 0 R 9 0 R 10 0 R 11 0 R] >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (Public) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (acct) /V (FIELDVALUESECRET) /Rect [10 200 100 220] /P 3 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /FreeText /Rect [10 150 100 170] /Contents (ANNOTBODYSECRET) /DA (/Helv 10 Tf 0 g) /P 3 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [10 120 100 140] /A << /S /URI /URI (http://leak.example/LINKURISECRET) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FileAttachment /Rect [200 10 210 20] /FS << /Type /Filespec /F (a.txt) /EF << /F 17 0 R >> >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [150 150 170 170] /Contents (REPLYKEPT) /IRT 8 0 R >>".to_vec(),
        b"<< /Type /StructTreeRoot /K 13 0 R >>".to_vec(),
        b"<< /S /Document /P 12 0 R /K [14 0 R 15 0 R 16 0 R 18 0 R] >>".to_vec(),
        b"<< /S /Form /P 13 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 7 0 R >> >>".to_vec(),
        b"<< /S /Annot /P 13 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 8 0 R >> >>".to_vec(),
        b"<< /S /Link /P 13 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 9 0 R >> >>".to_vec(),
        stream("", b"ATTACHMENTSECRET"),
        b"<< /S /Annot /P 13 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 10 0 R >> >>".to_vec(),
    ])
}

#[test]
fn redacted_annotations_and_fields_do_not_survive_through_the_structure_tree_irt_or_co() {
    let mut doc = referenced_annotations_fixture();
    // The mark covers the widget, the comment and the link; not the reply or the attachment.
    mark(&mut doc, 0, &[[5.0, 115.0, 105.0, 225.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.annotations, r.fields), (2, 1), "{r:?}");
    for secret in [&b"FIELDVALUESECRET"[..], b"ANNOTBODYSECRET", b"LINKURISECRET"] {
        assert!(!saved_contains(&doc, secret), "{} survived the save", String::from_utf8_lossy(secret));
    }
    assert!(saved_contains(&doc, b"REPLYKEPT") && saved_contains(&doc, b"ATTACHMENTSECRET"), "what was not under the mark stays");
    // The dangling referrers are cleaned, not left pointing at nothing.
    let saved = reopen(&doc);
    let reply = saved.scan_objects().find_map(|(_, o)| o.as_dict().filter(|d| d.name(b"Subtype") == Some(b"Text")).cloned()).unwrap();
    assert!(!reply.contains(b"IRT"), "the reply still points at the redacted comment: {reply:?}");
    let cat = saved.get(saved.root().unwrap()).as_dict().cloned().unwrap();
    let co = saved.resolve(cat.get(b"AcroForm").unwrap()).as_dict().and_then(|a| a.get(b"CO").cloned());
    assert!(co.as_ref().is_none_or(|c| saved.resolve(c).as_array().is_some_and(|a| a.is_empty())), "/CO still names the field: {co:?}");
}

#[test]
fn removed_hidden_annotations_and_fields_do_not_survive_through_the_structure_tree() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize};
    let is_widget = |d: &Dict| d.name(b"Subtype") == Some(b"Widget");
    // Sanitize: every category.
    let mut doc = referenced_annotations_fixture();
    sanitize(&mut doc).unwrap();
    for secret in [&b"ANNOTBODYSECRET"[..], b"LINKURISECRET", b"ATTACHMENTSECRET", b"REPLYKEPT"] {
        assert!(!saved_contains(&doc, secret), "{} survived Sanitize", String::from_utf8_lossy(secret));
    }
    assert!(!saved_has_dict(&doc, is_widget), "the flattened widget dictionary survived Sanitize");
    // Each category on its own.
    for (which, secret) in
        [(Hidden::Comments, &b"ANNOTBODYSECRET"[..]), (Hidden::Attachments, b"ATTACHMENTSECRET"), (Hidden::LinksActionsScripts, b"LINKURISECRET")]
    {
        let mut doc = referenced_annotations_fixture();
        remove_hidden(&mut doc, &[which]).unwrap();
        assert!(!saved_contains(&doc, secret), "{} survived Remove Hidden Information > {which:?}", String::from_utf8_lossy(secret));
    }
    let mut doc = referenced_annotations_fixture();
    remove_hidden(&mut doc, &[Hidden::FormFields]).unwrap();
    assert!(!saved_has_dict(&doc, is_widget), "the flattened widget dictionary survived Remove Hidden Information > Form fields");
}

/// A form with a visible field and two the user cannot see (Hidden and NoView widget flags).
fn hidden_fields_fixture() -> Document {
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << >> /Annots [5 0 R 6 0 R 7 0 R] >>".to_vec(),
        stream("", b""),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (visible) /V (MARK_VISIBLE_VALUE) /Rect [10 250 150 270] /F 4 /P 3 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (ssn) /V (MARK_HIDDEN_VALUE) /Rect [10 200 150 220] /F 6 /P 3 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (memo) /V (MARK_NOVIEW_VALUE) /Rect [10 150 150 170] /F 36 /P 3 0 R >>".to_vec(),
    ])
}

/// Flattening leaves Hidden and NoView annotations alone (neither drawn nor removed), and the
/// Form fields category relied on it to take every widget off the pages: the hidden fields
/// stayed in /Annots with their values, and were live fields again once the file was reopened.
#[test]
fn hidden_and_noview_form_fields_are_removed_with_the_rest() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize, scan};
    for all in [false, true] {
        let mut doc = hidden_fields_fixture();
        assert_eq!(scan(&doc).into_iter().find(|c| c.0 == Hidden::FormFields), Some((Hidden::FormFields, 3)));
        if all {
            sanitize(&mut doc).unwrap();
        } else {
            remove_hidden(&mut doc, &[Hidden::FormFields]).unwrap();
        }
        assert!(saved_contains(&doc, b"MARK_VISIBLE_VALUE"), "the visible field's value stays as page content (sanitize: {all})");
        for secret in [&b"MARK_HIDDEN_VALUE"[..], b"MARK_NOVIEW_VALUE"] {
            assert!(!saved_contains(&doc, secret), "{} survived (sanitize: {all})", String::from_utf8_lossy(secret));
        }
        assert!(!saved_has_dict(&doc, |d| d.name(b"Subtype") == Some(b"Widget")), "a widget survived (sanitize: {all})");
        let saved = reopen(&doc);
        assert!(pdfcraft_forms::fields(&saved).is_empty(), "the saved file still has fields (sanitize: {all})");
        assert_eq!(scan(&saved).into_iter().find(|c| c.0 == Hidden::FormFields), Some((Hidden::FormFields, 0)));
    }
}

/// Hidden information below the page dictionary: XMP on an image and a form XObject, /PieceInfo
/// and an associated file on the form, associated files on the page, a structure element and a
/// kept annotation, with the catalog, page and /Info copies as controls.
fn below_page_fixture() -> Document {
    let xmp = |mark: &str| stream("/Type /Metadata /Subtype /XML", mark.as_bytes());
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 8 0 R /AF [9 0 R] /StructTreeRoot 14 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R /Fm1 6 0 R >> >> /Metadata 10 0 R /PieceInfo << /App << /Private (MARK_PAGE_PIECEINFO) >> >> /AF [11 0 R] /Annots [12 0 R] >>".to_vec(),
        stream("", b"q 50 0 0 50 20 20 cm /Im1 Do Q /Fm1 Do"),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Metadata 7 0 R", [0]),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Metadata 13 0 R /PieceInfo << /App << /Private 16 0 R >> >> /AF [17 0 R]", b""),
        xmp("MARK_IMG_XMP"),
        xmp("MARK_CAT_XMP"),
        b"<< /Type /Filespec /F (cat.txt) /EF << /F 18 0 R >> >>".to_vec(),
        xmp("MARK_PAGE_XMP"),
        b"<< /Type /Filespec /F (page.txt) /EF << /F 19 0 R >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /AF [20 0 R] >>".to_vec(),
        xmp("MARK_FORM_XMP"),
        b"<< /Type /StructTreeRoot /K 15 0 R >>".to_vec(),
        b"<< /S /Document /P 14 0 R /AF [21 0 R] >>".to_vec(),
        stream("", b"MARK_FORM_PIECEINFO"),
        b"<< /Type /Filespec /F (xobj.txt) /EF << /F 22 0 R >> >>".to_vec(),
        stream("", b"MARK_CAT_AF"),
        stream("", b"MARK_PAGE_AF"),
        b"<< /Type /Filespec /F (annot.txt) /EF << /F 23 0 R >> >>".to_vec(),
        b"<< /Type /Filespec /F (struct.txt) /EF << /F 24 0 R >> >>".to_vec(),
        stream("", b"MARK_XOBJ_AF"),
        stream("", b"MARK_ANNOT_AF"),
        stream("", b"MARK_STRUCT_AF"),
    ]);
    let info = doc.add(Object::Dict({
        let mut d = Dict::new();
        d.set(b"Title".to_vec(), pdfcraft_cos::PdfString::text("MARK_INFO_TITLE"));
        d
    }));
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    doc
}

/// Metadata, Attachments and Private data looked at the trailer, the catalog and the page
/// dictionaries only, so XMP on XObjects, /PieceInfo on forms and associated files below the
/// catalog stayed in the saved file, and a document whose only attachments were /AF entries was
/// reported as having none.
#[test]
fn metadata_private_data_and_associated_files_below_the_page_are_removed() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize, scan};
    const XMP: [&[u8]; 5] = [b"MARK_IMG_XMP", b"MARK_FORM_XMP", b"MARK_CAT_XMP", b"MARK_PAGE_XMP", b"MARK_INFO_TITLE"];
    const AF: [&[u8]; 5] = [b"MARK_CAT_AF", b"MARK_PAGE_AF", b"MARK_XOBJ_AF", b"MARK_ANNOT_AF", b"MARK_STRUCT_AF"];
    const PIECE: [&[u8]; 2] = [b"MARK_FORM_PIECEINFO", b"MARK_PAGE_PIECEINFO"];
    let check = |doc: &Document, gone: &[&[u8]], kept: &[&[u8]], what: &str| {
        for m in gone {
            assert!(!saved_contains(doc, m), "{} survived {what}", String::from_utf8_lossy(m));
        }
        for m in kept {
            assert!(saved_contains(doc, m), "{} was removed by {what}", String::from_utf8_lossy(m));
        }
    };
    let mut doc = below_page_fixture();
    remove_hidden(&mut doc, &[Hidden::Metadata]).unwrap();
    check(&doc, &XMP, &[AF[0], PIECE[0]], "Metadata");
    let mut doc = below_page_fixture();
    remove_hidden(&mut doc, &[Hidden::Attachments]).unwrap();
    check(&doc, &AF, &[XMP[0], PIECE[0]], "Attachments");
    let mut doc = below_page_fixture();
    remove_hidden(&mut doc, &[Hidden::PrivateData]).unwrap();
    check(&doc, &PIECE, &[XMP[0], AF[0]], "Private data");
    let mut doc = below_page_fixture();
    sanitize(&mut doc).unwrap();
    check(&doc, &[XMP.as_slice(), &AF, &PIECE].concat(), &[], "Sanitize");
    // The scan counts the same locations, so the dialog offers the category.
    let counts: std::collections::HashMap<Hidden, usize> = scan(&below_page_fixture()).into_iter().collect();
    assert_eq!(counts[&Hidden::Metadata], 1 + 4, "the Info entry and four XMP streams");
    assert_eq!(counts[&Hidden::Attachments], 5, "five associated files");
    assert_eq!(counts[&Hidden::PrivateData], 2, "the page's and the form's");
}

/// Multimedia content: Screen annotations with a Rendition action (carrying a script) and a
/// JavaScript action, a Movie with an external path, a RichMedia with an embedded asset, a 3D
/// annotation whose stream runs a script on instantiation, /Names /Renditions with embedded
/// media, /Names /AlternatePresentations and a page /Thumb. Controls that were already removed:
/// an embedded file, a Sound annotation, a document script and a link with a script.
fn multimedia_fixture() -> Document {
    pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /Renditions << /Names [(r) 13 0 R] >> /AlternatePresentations << /Names [(slides) 14 0 R] >> /EmbeddedFiles << /Names [(a.txt) 15 0 R] >> /JavaScript << /Names [(init) 16 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << >> /Thumb 17 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R] >>".to_vec(),
        stream("", b""),
        b"<< /Type /Annot /Subtype /Screen /Rect [10 10 50 50] /A << /S /Rendition /OP 0 /AN 5 0 R /R 12 0 R /JS (SCRIPTMARKER_RENDITION_ACTION) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Screen /Rect [60 10 100 50] /A << /S /JavaScript /JS (SCRIPTMARKER_SCREEN_JS) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Movie /Rect [110 10 150 50] /Movie << /F (/Volumes/MOVIEPATHMARKER.mov) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /RichMedia /Rect [160 10 200 50] /RichMediaContent << /Assets << /Names [(asset.swf) 18 0 R] >> >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /3D /Rect [210 10 250 50] /3DD 19 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Sound /Rect [10 60 50 100] /Sound 20 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [60 60 100 100] /A << /S /JavaScript /JS (CONTROL_LINK_JS) >> >>".to_vec(),
        b"<< /Type /Rendition /S /MR /C << /Type /MediaClip /S /MCD /D 21 0 R /CT (video/mp4) >> >>".to_vec(),
        b"<< /Type /Rendition /S /MR /C << /Type /MediaClip /S /MCD /D 22 0 R /CT (video/mp4) >> >>".to_vec(),
        b"<< /Type /SlideShow /Subtype /Embedded /Resources << /Names [(index.html) 23 0 R] >> /StartResource (index.html) >>".to_vec(),
        b"<< /Type /Filespec /F (a.txt) /EF << /F 24 0 R >> >>".to_vec(),
        b"<< /S /JavaScript /JS (CONTROL_DOC_JS) >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"THUMBMARKER_PRE_"),
        b"<< /Type /Filespec /F (asset.swf) /EF << /F 25 0 R >> >>".to_vec(),
        stream("/Type /3D /Subtype /U3D /OnInstantiate 26 0 R", b"EMBEDMARKER_3D_STREAM"),
        stream("/Type /Sound /R 8000 /C 1 /B 8", b"CONTROL_SOUND_DATA"),
        b"<< /Type /Filespec /F (screen.mp4) /EF << /F 27 0 R >> >>".to_vec(),
        b"<< /Type /Filespec /F (clip.mp4) /EF << /F 28 0 R >> >>".to_vec(),
        stream("", b"SCRIPTMARKER_ALTPRES_RESOURCE"),
        stream("", b"CONTROL_ATTACHMENT"),
        stream("", b"EMBEDMARKER_RICHMEDIA_ASSET"),
        stream("", b"SCRIPTMARKER_3D_ONINSTANTIATE"),
        stream("", b"EMBEDMARKER_SCREEN_MEDIA"),
        stream("", b"EMBEDMARKER_RENDITION_MEDIA"),
    ])
}

/// Sanitize looked at a fixed list of containers, so scripts, embedded files, a file path and
/// the stored page image held by multimedia content stayed in the saved file while the scan
/// counted none of them and the operation reported success.
#[test]
fn multimedia_scripts_embedded_files_paths_and_thumbnails_are_removed() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize, scan};
    const SCRIPTS: [&[u8]; 4] =
        [b"SCRIPTMARKER_RENDITION_ACTION", b"SCRIPTMARKER_SCREEN_JS", b"SCRIPTMARKER_3D_ONINSTANTIATE", b"SCRIPTMARKER_ALTPRES_RESOURCE"];
    const EMBEDDED: [&[u8]; 5] =
        [b"EMBEDMARKER_RENDITION_MEDIA", b"EMBEDMARKER_SCREEN_MEDIA", b"EMBEDMARKER_RICHMEDIA_ASSET", b"EMBEDMARKER_3D_STREAM", b"MOVIEPATHMARKER"];
    const CONTROLS: [&[u8]; 4] = [b"CONTROL_ATTACHMENT", b"CONTROL_SOUND_DATA", b"CONTROL_DOC_JS", b"CONTROL_LINK_JS"];
    let check = |doc: &Document, gone: &[&[u8]], kept: &[&[u8]], what: &str| {
        for m in gone {
            assert!(!saved_contains(doc, m), "{} survived {what}", String::from_utf8_lossy(m));
        }
        for m in kept {
            assert!(saved_contains(doc, m), "{} was removed by {what}", String::from_utf8_lossy(m));
        }
    };
    let mut doc = multimedia_fixture();
    sanitize(&mut doc).unwrap();
    check(&doc, &[SCRIPTS.as_slice(), &EMBEDDED, &CONTROLS, &[b"THUMBMARKER_PRE_"]].concat(), &[], "Sanitize");
    let mut doc = multimedia_fixture();
    remove_hidden(&mut doc, &[Hidden::LinksActionsScripts]).unwrap();
    check(&doc, &SCRIPTS, &[EMBEDDED[0], EMBEDDED[3], CONTROLS[0], CONTROLS[1]], "Links, actions and scripts");
    let mut doc = multimedia_fixture();
    remove_hidden(&mut doc, &[Hidden::Attachments]).unwrap();
    check(&doc, &EMBEDDED, &[CONTROLS[2], CONTROLS[3], SCRIPTS[3]], "Attachments");
    // A category that changes what a page shows drops its stored thumbnail too.
    let mut doc = multimedia_fixture();
    remove_hidden(&mut doc, &[Hidden::Comments]).unwrap();
    check(&doc, &[b"THUMBMARKER_PRE_", CONTROLS[1]], &[EMBEDDED[3], CONTROLS[0]], "Comments");
    // The scan counts them, so the dialog and doc_hidden_info show them.
    let counts: std::collections::HashMap<Hidden, usize> = scan(&multimedia_fixture()).into_iter().collect();
    assert_eq!(counts[&Hidden::Attachments], 7, "the embedded file, the five multimedia annotations and the rendition");
    assert_eq!(counts[&Hidden::LinksActionsScripts], 6, "the link, two Screen actions, the document script, the slideshow and the 3D script");
}

/// The property list of a `BDC` wrapping redacted glyphs (inline, or a named /Properties entry)
/// kept /ActualText, /Alt and /E, whether every glyph in the span went or only part of it, so
/// the saved content stream and the page's /Properties still spelled the redacted text.
#[test]
fn marked_content_property_lists_lose_the_replacement_text_of_redacted_glyphs() {
    let src = b"/Span <</ActualText (QINLINEACTUAL)>> BDC BT /F1 10 Tf 10 250 Td (GLYPHAAA1) Tj ET EMC \
/Span /PL1 BDC BT /F1 10 Tf 10 235 Td (GLYPHBBB2) Tj ET EMC \
/Span <</Alt (QINLINEALT) /E (QINLINEEXP)>> BDC BT /F1 10 Tf 10 220 Td (GLYPHCCC3) Tj ET EMC \
/Span /PL2 BDC BT /F1 10 Tf 10 205 Td (GLYPHDDD4) Tj ET EMC \
/Span <</ActualText (QPARTIALACTUAL)>> BDC BT /F1 10 Tf 10 150 Td (HALFGONEHALFKEPT) Tj ET EMC \
/Span <</ActualText (QCONTROLKEPT)>> BDC BT /F1 10 Tf 10 50 Td (OUTSIDEMARK) Tj ET EMC \
/Span /PL1 BDC BT /F1 10 Tf 10 30 Td (NAMEDTWICE) Tj ET EMC";
    // PL1 is named by a redacted span and by one outside the marks; PL2 holds an indirect
    // reference, so it cannot be written inline.
    let mut doc = one_page(
        src,
        "/Properties << /PL1 << /ActualText (QNAMEDACTUAL) /Lang (en) >> /PL2 << /ActualText (QNAMEDREFACTUAL) /Lang 7 0 R >> >>",
        vec![b"(en-GB)".to_vec()],
    );
    mark(&mut doc, 0, &[[5.0, 200.0, 100.0, 262.0], [5.0, 145.0, 50.0, 162.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 9 * 4 + 8, "{r:?}");
    for secret in [&b"QINLINEACTUAL"[..], b"QINLINEALT", b"QINLINEEXP", b"QNAMEDREFACTUAL", b"QPARTIALACTUAL", b"HALFGONE"] {
        assert!(!saved_contains(&doc, secret), "{} survived the save", String::from_utf8_lossy(secret));
    }
    let c = content(&doc, 0);
    assert!(c.contains("QNAMEDACTUAL") || saved_contains(&doc, b"QNAMEDACTUAL"), "the list a span outside the marks still names is kept: {c}");
    assert!(!c.contains("/PL1 BDC\nBT\n/F1 10 Tf\n10 235 Td"), "the redacted span no longer names the shared list: {c}");
    for kept in [&b"QCONTROLKEPT"[..], b"OUTSIDEMARK", b"HALFKEPT", b"NAMEDTWICE", b"en-GB"] {
        assert!(saved_contains(&doc, kept), "{} was removed", String::from_utf8_lossy(kept));
    }
    // Every BDC still has its tag and a property list, and the one with the reference is named.
    let saved = reopen(&doc);
    let p = &pdfcraft_model::pages(&saved)[0];
    let props = p
        .dict
        .get(b"Resources")
        .and_then(|r| saved.resolve(r).as_dict().cloned())
        .and_then(|r| r.get(b"Properties").and_then(|x| saved.resolve(x).as_dict().cloned()))
        .unwrap();
    assert!(!props.contains(b"PL2"), "the shared list nothing names any more is gone: {props:?}");
    for op in pdfcraft_content::parse(content(&saved, 0).as_bytes()).ops.iter().filter(|op| op.is("BDC")) {
        assert_eq!(op.operands.len(), 2, "{op:?}");
        if let Some(n) = op.name(1) {
            assert!(props.contains(n), "{} names nothing in {props:?}", String::from_utf8_lossy(n));
        }
    }
}

/// Structure elements above the one that directly owns the redacted marked content describe
/// the same subtree, so their /ActualText, /Alt, /E (and /T) go too; an untouched sibling keeps
/// its own.
#[test]
fn structure_elements_above_the_owner_lose_their_replacement_text() {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (GLYPHDDD4) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /ActualText (QGRANDACTUAL) /K [9 0 R] >>".to_vec(),
        b"<< /S /Sect /P 8 0 R /ActualText (QPARENTACTUAL) /Alt (QPARENTALT) /E (QPARENTEXP) /T (QPARENTTITLE) /K [10 0 R 11 0 R] >>".to_vec(),
        b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (QOWNERACTUAL) /Alt (QOWNERALT) /E (QOWNEREXP) /T (QOWNERTITLE) /K 0 >>".to_vec(),
        b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (QSIBLINGKEPT) /T (QSIBLINGTITLE) /K 1 >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    let r = apply(&mut doc, None).unwrap();
    for secret in [
        &b"GLYPHDDD4"[..],
        b"QOWNERACTUAL",
        b"QOWNERALT",
        b"QOWNEREXP",
        b"QOWNERTITLE",
        b"QPARENTACTUAL",
        b"QPARENTALT",
        b"QPARENTEXP",
        b"QPARENTTITLE",
        b"QGRANDACTUAL",
    ] {
        assert!(!saved_contains(&doc, secret), "{} survived the save", String::from_utf8_lossy(secret));
    }
    assert!(saved_contains(&doc, b"QSIBLINGKEPT") && saved_contains(&doc, b"QSIBLINGTITLE"), "the untouched sibling keeps its text");
    assert_eq!(r.tags, 3, "the owner, its parent and its grandparent changed: {r:?}");
}

/// An applied mark leaves the page with its pop-up. A pop-up left listed would keep the mark,
/// and the note a reviewer wrote on it, reachable through /Parent in the saved file.
#[test]
fn an_applied_marks_popup_goes_with_the_mark() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 200 Td (VISIBLE77) Tj ET", "", vec![]);
    mark(&mut doc, 0, &[[5.0, 190.0, 125.0, 215.0]], "");
    let the_mark = marks(&doc)[0].obj;
    let page = pdfcraft_model::pages(&doc).swap_remove(0).obj;
    let mut popup = Dict::new();
    popup.set(b"Type".to_vec(), Object::name("Annot"));
    popup.set(b"Subtype".to_vec(), Object::name("Popup"));
    popup.set(b"Rect".to_vec(), Object::Array(vec![200.into(), 20.into(), 280.into(), 80.into()]));
    popup.set(b"Parent".to_vec(), Object::Ref(the_mark));
    let popup = doc.add(Object::Dict(popup));
    doc.update_dict(the_mark, |d| {
        d.set(b"Popup".to_vec(), Object::Ref(popup));
        d.set(b"Contents".to_vec(), pdfcraft_cos::PdfString::text("MARKNOTESECRET"));
    })
    .unwrap();
    doc.update_dict(page, |d| {
        let mut annots = d.get(b"Annots").and_then(Object::as_array).cloned().unwrap_or_default();
        annots.push(Object::Ref(popup));
        d.set(b"Annots".to_vec(), Object::Array(annots));
    })
    .unwrap();

    apply(&mut doc, None).unwrap();
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    assert!(annots_of(&doc, &p.dict).is_empty(), "the mark and its pop-up left the page");
    assert!(!saved_contains(&doc, b"MARKNOTESECRET"), "the mark's note survived the save");
    assert!(!saved_contains(&doc, b"VISIBLE77"));
}

#[test]
fn a_hostile_icc_component_count_is_refused_not_multiplied_into_a_crash() {
    // /N comes from the file. 2^60 × 8 bpc × width 2 overflows the row size; a negative /N wrapped
    // to a huge usize. Either way the half-covered image can't be cleared, so it is removed whole.
    for n in ["1152921504606846976", "-1", "2"] {
        let img = stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace [/ICCBased 8 0 R] /BitsPerComponent 8", [255, 255]);
        let icc = stream(&format!("/N {n}"), b"");
        let mut doc = one_page(b"q 20 0 0 10 100 100 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![img, icc]);
        mark(&mut doc, 0, &[[95.0, 95.0, 110.0, 115.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.images_removed, r.images_cleared), (1, 0), "/N {n}: {r:?}");
        assert!(!content(&doc, 0).contains("/Im Do"), "/N {n}");
    }
}

#[test]
fn image_alternatives_and_masks_cannot_keep_redacted_pixels() {
    for extra in ["/Alternates [<< /Image 8 0 R >>]", "/SMask 8 0 R", "/Mask 8 0 R", "/Mask [0 0]"] {
        let mut doc = one_page(
            b"q 0 0 300 300 re W n 8 0 0 10 10 10 cm /Im Do Q",
            "/XObject << /Im 7 0 R >>",
            vec![
                stream(
                    &format!("/Type /XObject /Subtype /Image /Width 8 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 {extra}"),
                    b"LEAK_TOPPUBLIC__",
                ),
                stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"LEAK_ALT"),
            ],
        );
        mark(&mut doc, 0, &[[10.0, 15.0, 18.0, 20.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_no_secret(&doc, b"LEAK_");
        assert_eq!((report.images_cleared, report.images_removed), if extra.starts_with("/Alternates") { (1, 0) } else { (0, 1) });
    }
}

#[test]
fn masks_and_alternates_outside_redaction_stay() {
    let mut doc = one_page(
        b"q 8 0 0 10 10 10 cm /Im Do Q",
        "/XObject << /Im 7 0 R >>",
        vec![
            stream(
                "/Type /XObject /Subtype /Image /Width 8 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 8 0 R /Alternates [<< /Image 8 0 R >>]",
                b"PUBLIC_TOPBOTTOM",
            ),
            stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBLIC__"),
        ],
    );
    mark(&mut doc, 0, &[[100.0, 100.0, 120.0, 120.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!((report.images_removed, report.images_cleared), (0, 0));
    let saved = reopen(&doc);
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let resources = saved.resolve(page.dict.get(b"Resources").unwrap());
    let xobjects = saved.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let image = saved.resolve(xobjects.as_dict().unwrap().get(b"Im").unwrap());
    let Object::Stream(image) = &*image else { panic!() };
    assert!(image.dict.contains(b"SMask") && image.dict.contains(b"Alternates"));
    assert_eq!(image.decoded().unwrap(), b"PUBLIC_TOPBOTTOM");
}

/// An OPI dictionary names the original of a proxy image, and its file specification can embed
/// that original: a cleared copy goes without it, as without `/Alternates`.
#[test]
fn an_opi_original_does_not_keep_redacted_pixels() {
    let mut doc = one_page(
        b"q 0 0 300 300 re W n 8 0 0 10 10 10 cm /Im Do Q",
        "/XObject << /Im 7 0 R >>",
        vec![
            stream(
                "/Type /XObject /Subtype /Image /Width 8 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 \
                 /OPI << /2.0 << /Type /OPI /Version 2.0 /F << /Type /Filespec /F (original.tif) /EF << /F 8 0 R >> >> /Inks /full_color >> >>",
                b"LEAK_TOPPUBLIC__",
            ),
            stream("/Type /EmbeddedFile", b"LEAK_ORIGINAL"),
        ],
    );
    mark(&mut doc, 0, &[[10.0, 15.0, 18.0, 20.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!((report.images_cleared, report.images_removed), (1, 0));
    assert_no_secret(&doc, b"LEAK_");
}
