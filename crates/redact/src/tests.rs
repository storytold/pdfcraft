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
    assert!(!bytes.windows(4).any(|w| w == b"1234"), "redacted text left in the saved file");
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
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!has(b"REDIMGONECOVERED"), "covered image samples survived the save");
    assert!(!has(b"REDIMGPARTSECRET"), "partly covered image's secret row survived the save");
    assert!(!has(b"REDFORMSECRETTEXT"), "the form's redacted text survived the save");

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
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        for secret in [&b"REDIMGONECOVERED"[..], b"REDIMGPARTSECRET", b"REDFORMSECRETTEXT"] {
            assert!(!has(secret), "{} survived the save (inherited: {inherited})", String::from_utf8_lossy(secret));
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
    assert!(!bytes.windows(16).any(|w| w == b"REDIMGONECOVERED"), "the image the form drew survived the save");
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
    assert!(!bytes.windows(16).any(|w| w == b"HIDDENLAYERIMAGE"), "the off layer's image survived the save");
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
    assert!(!bytes.windows(15).any(|w| w == b"SSN-987-65-4321"), "the redacted value survived in the datasets packet");

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
        Object::Stream(s) => s.decoded().is_ok_and(|d| d.windows(5).any(|w| w == b"(Ada)")),
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

fn assert_no_secret(doc: &Document, secret: &[u8]) {
    let saved = reopen(doc);
    for (r, object) in saved.scan_objects() {
        let mut bytes = Vec::new();
        pdfcraft_cos::serialize(&object, &mut bytes);
        assert!(!bytes.windows(secret.len()).any(|w| w == secret), "secret remains in object {r:?}");
        if let Object::Stream(s) = &*object {
            let data = s.decoded().unwrap();
            assert!(!data.windows(secret.len()).any(|w| w == secret), "secret remains in stream {r:?}");
        }
    }
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

#[test]
fn stream_boundary_comment_stays_inert_after_unrelated_redaction() {
    for tail in [b"BT /F1 10 Tf 10 200 Td (HIDDEN) Tj ET".as_slice(), b"BT /F1 10 Tf 10 200 Td (HIDDEN) Tj ET\nBT /F1 10 Tf 10 250 Td (LIVE) Tj ET"] {
        let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (REMOVE) Tj ET % note", "", vec![]);
        let hidden = doc.add(Object::Stream(Stream::from_raw(Dict::new(), tail.to_vec())));
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(4, 0)), Object::Ref(hidden)]))).unwrap();
        mark(&mut doc, 0, &[[9.0, 95.0, 50.0, 110.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
        let reopened = reopen(&doc);
        let page = &pdfcraft_model::pages(&reopened)[0];
        let (_, data) = page_streams(&reopened, &page.dict, 0).unwrap();
        // Independent of Pieces::join: the renderer concatenates streams with spaces.
        let ops = pdfcraft_content::parse(&data.join(&b' ')).ops;
        assert!(!ops.iter().any(|o| o.operands.iter().any(|v| v.as_string().is_some_and(|s| s.bytes == b"HIDDEN"))), "hidden text became executable");
        let live = ops.iter().any(|o| o.operands.iter().any(|v| v.as_string().is_some_and(|s| s.bytes == b"LIVE")));
        assert_eq!(live, tail.contains(&b'\n'));
    }
}

#[test]
fn rewriting_a_piece_keeps_its_leading_comment_terminator() {
    for eol in [b"\n".as_slice(), b"\r", b"\r\n"] {
        let mut doc = one_page(b"q Q % note", "", vec![]);
        let mut tail = eol.to_vec();
        tail.extend(b"BT /F1 10 Tf 10 100 Td (REMOVE) Tj ET BT /F1 10 Tf 10 200 Td (KEEP) Tj ET");
        let second = doc.add(Object::Stream(Stream::from_raw(Dict::new(), tail)));
        let page = pdfcraft_model::pages(&doc)[0].obj;
        doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(4, 0)), Object::Ref(second)]))).unwrap();
        mark(&mut doc, 0, &[[9.0, 95.0, 50.0, 110.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
        let p = &pdfcraft_model::pages(&doc)[0];
        let (_, data) = page_streams(&doc, &p.dict, 0).unwrap();
        let ops = pdfcraft_content::parse(&data.join(&b' ')).ops;
        assert_eq!(ops.iter().filter(|o| o.is("BT")).count(), 2, "leading EOL lost; first live operator swallowed");
        assert!(ops.iter().any(|o| o.operands.iter().any(|v| v.as_string().is_some_and(|s| s.bytes == b"KEEP"))));
    }
}
