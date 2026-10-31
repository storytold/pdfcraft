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
    let mut budget = pdfcraft_cos::DecodeBudget::page();
    let (_, data) = page_streams_within(doc, &p.dict, page, &mut budget).unwrap();
    let res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut rep = Report::default();
    let mut scope = Scope::new(rects, Mode::Verify, &mut rep, budget);
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

/// #2130: a font with no /Widths is drawn with its embedded program's advances. Measured with a
/// guess from its name instead (Courier's 600 units for "Mono"), the end of the line seemed to
/// lie past a mark that covers all of it on screen, and was kept.
#[test]
fn a_whole_line_under_a_mark_goes_even_without_widths() {
    let text = b"illicit little lilies";
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /HouseMono /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", [b"BT /F1 10 Tf 10 100 Td (".as_slice(), text, b") Tj ET"].concat()),
        font.as_bytes().to_vec(),
        b"<< /Type /FontDescriptor /FontName /HouseMono /Flags 32 /Ascent 900 /Descent -200 /FontFile2 7 0 R >>".to_vec(),
        stream(&format!("/Length1 {}", pdfcraft_fonts::INTER_REGULAR.len()), pdfcraft_fonts::INTER_REGULAR),
    ]);
    // Inter draws these 21 narrow glyphs in about 60 pt; Courier's guess would need 126.
    let line = [5.0, 95.0, 90.0, 112.0];
    mark(&mut doc, 0, &[line], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, text.len(), "every glyph of the line goes");
    assert_eq!(under(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]]), 0, "{}", content(&doc, 0));
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
    assert_eq!((r.paths_removed, r.paths_clipped, r.images_removed), (2, 0, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("10 10 20 20 re"), "{c}");
    assert!(!c.contains("300 300 re"), "the intersecting rect is removed: {c}");
    assert!(!c.contains("BI"), "{c}");
    assert!(c.contains("150 150 m"), "the line elsewhere stays");
}

#[test]
fn partially_covered_vector_paths_do_not_leak_drawing_commands() {
    // A vector path with detailed drawing commands (e.g. outlined text or confidential geometry)
    // that is only partially covered by a redaction mark must be completely removed, not retained
    // behind a clipping path (F-008).
    let src = b"0 g 100 100 m 150 150 l 200 100 l h f 10 10 m 20 20 l W b 300 300 m 350 350 l S";
    let mut doc = one_page(src, "", vec![]);
    // Redaction mark covers only part of the triangle (100..200, 100..150):
    // mark is [120, 120, 160, 160], partially intersecting the triangle.
    // Also partially intersect the clip-and-stroke path [10..20, 10..20] with [15, 15, 25, 25].
    mark(&mut doc, 0, &[[120.0, 120.0, 160.0, 160.0], [15.0, 15.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.paths_removed, 2, "both partially intersected paths removed");
    assert_eq!(r.paths_clipped, 0, "no paths retained via clipping");

    let c = content(&doc, 0);
    // Neither the drawing operators nor the coordinates should leak in the content stream.
    assert!(!c.contains("100 100 m"), "underlying geometry must not leak: {c}");
    assert!(!c.contains("150 150 l"), "underlying geometry must not leak: {c}");
    assert!(!c.contains("200 100 l"), "underlying geometry must not leak: {c}");
    assert!(!c.contains("10 10 m"), "partial clip geometry must not leak: {c}");
    assert!(!c.contains("20 20 l"), "partial clip geometry must not leak: {c}");
    // Unrelated non-intersecting path remains untouched.
    assert!(c.contains("300 300 m\n350 350 l\nS"), "unaffected vector path must be kept: {c}");
}

/// A chain of nested forms deeper than the pass can walk was removed from the page, taking
/// whatever else it drew with it even when nothing in it was marked. The limit now reaches as
/// deep as the renderer and Edit text read (50), so anything a reader can see can be walked;
/// past that nothing is drawn either, so removing it costs nothing visible (#3645).
#[test]
fn a_deeply_nested_form_keeps_the_text_nobody_marked() {
    /// A page that draws `HEAD` at the top, then a chain of `depth` forms whose innermost draws
    /// `DEEP`, then `TAIL`. Only `HEAD` is marked.
    fn page_with_chain(depth: usize) -> Document {
        let first = 7;
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R                  /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 {first} 0 R >> >> >>"
            )
            .into_bytes(),
            stream("", b"BT /F1 10 Tf 10 280 Td (HEAD) Tj ET /Fm1 Do BT /F1 10 Tf 10 10 Td (TAIL) Tj ET"),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
        ];
        for k in 1..=depth {
            let res = if k < depth {
                format!("/Resources << /Font << /F1 5 0 R >> /XObject << /Fm{} {} 0 R >> >>", k + 1, first + k)
            } else {
                "/Resources << /Font << /F1 5 0 R >> >>".to_string()
            };
            let body: Vec<u8> = if k < depth { format!("/Fm{} Do", k + 1).into_bytes() } else { b"BT /F1 10 Tf 10 150 Td (DEEP) Tj ET".to_vec() };
            objs.push(stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {res}"), &body));
        }
        pdf(objs)
    }

    // Deeper than the old limit of 12 and within the new one: the innermost text survives.
    for depth in [1, 12, 13, 20, 50] {
        let mut doc = page_with_chain(depth);
        mark(&mut doc, 0, &[[5.0, 275.0, 60.0, 295.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!(report.forms_removed, 0, "depth {depth}: a form was removed");
        let c = content(&doc, 0);
        assert!(c.contains("(TAIL)"), "depth {depth}: {c}");
        // The innermost form's text is still in the saved file.
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(saved.windows(6).any(|w| w == b"(DEEP)"), "depth {depth}: the innermost form's text was deleted");
    }

    // Past what any reader follows, the form is still removed: the fail-safe, and nothing the
    // renderer draws is lost because it stops at 50 too.
    let mut doc = page_with_chain(crate::interp::MAX_DEPTH + 1);
    mark(&mut doc, 0, &[[5.0, 275.0, 60.0, 295.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().forms_removed, 1, "a chain past the limit is removed");
}

/// Depth alone does not bound the walk: a form that draws the next level twice, or a cycle of two
/// forms, costs 2^depth interpretations, which at a depth limit of 50 is a hang on a file anyone
/// can send. The pass also counts the forms it enters, and a page past that budget is treated as
/// one it cannot read.
#[test]
fn a_form_that_fans_out_or_loops_is_bounded_by_the_page_budget() {
    /// A page drawing `Fm1`, where each of `n` forms draws the next **twice** and the last draws
    /// text: 2^n paths if nothing stops the walk. With `looping`, the last form draws the first
    /// one again instead, so the chain has no end at all.
    fn fan_out(n: usize, looping: bool) -> Document {
        let first = 7;
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R                  /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 {first} 0 R >> >> >>"
            )
            .into_bytes(),
            stream("", b"BT /F1 10 Tf 10 280 Td (HEAD) Tj ET /Fm1 Do"),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
        ];
        for k in 1..=n {
            let (next, body) = if k < n {
                (first + k, format!("/Fm{0} Do /Fm{0} Do", k + 1))
            } else if looping {
                (first, "/Fm1 Do /Fm1 Do".to_string())
            } else {
                (first, "BT /F1 10 Tf 10 150 Td (DEEP) Tj ET".to_string())
            };
            let name = if k < n { format!("/Fm{}", k + 1) } else { "/Fm1".to_string() };
            let res = format!("/Resources << /Font << /F1 5 0 R >> /XObject << {name} {next} 0 R >> >>");
            objs.push(stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {res}"), body.as_bytes()));
        }
        pdf(objs)
    }

    for looping in [false, true] {
        let start = std::time::Instant::now();
        let mut doc = fan_out(crate::interp::MAX_DEPTH, looping);
        mark(&mut doc, 0, &[[5.0, 275.0, 60.0, 295.0]], "");
        let result = apply(&mut doc, None);
        let took = start.elapsed();
        // An unbounded walk would never end; the limit leaves room for a loaded test machine.
        assert!(took < std::time::Duration::from_secs(120), "looping {looping}: the walk took {took:?}");
        // The page's form budget stops the walk, and Apply refuses the page rather than
        // removing forms it could not read.
        assert!(result.is_err(), "looping {looping}: the page past its form budget is refused");
    }
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

#[test]
fn redaction_clears_pixels_intersected_without_covering_their_centres() {
    for (content, rect) in
        [(&b"q 200 0 0 100 0 0 cm /Im Do Q"[..], [10.0, 10.0, 20.0, 90.0]), (&b"q 0 100 -200 0 200 0 cm /Im Do Q"[..], [10.0, 5.0, 190.0, 10.0])]
    {
        let mut doc = one_page(
            content,
            "/XObject << /Im 7 0 R >>",
            vec![stream("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", [123, 234])],
        );
        mark(&mut doc, 0, &[rect], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!(report.images_cleared, 1);
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(named_image(&saved, 0, b"Im"), None);
        assert_eq!(named_image(&saved, 0, b"PCRedacted1"), Some(vec![0, 234]));
    }
}

/// An image whose PNG predictor is given by reference has its covered pixel cleared, and only
/// that one, whichever part is the reference: the dictionary, an array's entry, or the number
/// itself. `Stream::decoded` reads only a direct `/DecodeParms` with direct numbers, so the row
/// tags stayed in the data, the cleared byte drifted off the covered pixel and the covered sample
/// survived.
#[test]
fn an_image_whose_predictor_is_given_by_reference_has_the_covered_pixel_cleared() {
    // Two rows of four grey samples, each behind a PNG "None" row tag.
    let predicted = pdfcraft_filters::encode_flate(&[0, 10, 20, 30, 40, 0, 50, 60, 70, 80]);
    let parms = "/Predictor 15 /Colors 1 /BitsPerComponent 8 /Columns 4";
    for (decode_parms, object8) in [
        ("8 0 R".to_owned(), format!("<< {parms} >>")),
        ("[8 0 R]".to_owned(), format!("<< {parms} >>")),
        ("<< /Predictor 8 0 R /Colors 1 /BitsPerComponent 8 /Columns 4 >>".to_owned(), "15".to_owned()),
    ] {
        let image = format!(
            "/Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms {decode_parms}"
        );
        let mut doc = one_page(b"q 40 0 0 20 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![stream(&image, &predicted), object8.into_bytes()]);
        // The top row's second sample.
        mark(&mut doc, 0, &[[12.0, 12.0, 18.0, 18.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!(report.images_cleared, 1, "{decode_parms}");
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(named_image(&saved, 0, b"PCRedacted1"), Some(vec![10, 0, 30, 40, 50, 60, 70, 80]), "{decode_parms}");
    }
}

/// A CCITT fax image (a scan the optimizer or Create PDF stored as Group 4) has its covered
/// pixels cleared in a copy, like any image PdfCraft can decode, instead of being removed whole:
/// whether it sits behind another filter, with its parameters given by reference, or is a mask,
/// its decode array inverting it given directly, by reference or with its first number so.
#[test]
fn a_fax_image_has_its_covered_pixels_cleared_instead_of_being_removed() {
    // 16 × 2, all 1 bits (white), or all 0 bits (a mask painting everywhere); the left half of
    // the top row is covered.
    let g4 = pdfcraft_filters::encode_g4(&[0xFF; 4], 16, 2).unwrap();
    let painted = pdfcraft_filters::encode_g4(&[0x00; 4], 16, 2).unwrap();
    let hex: Vec<u8> = g4.iter().map(|b| format!("{b:02x}")).collect::<String>().bytes().chain(*b">").collect();
    let image = "/Type /XObject /Subtype /Image /Width 16 /Height 2 /BitsPerComponent 1";
    let parms = "<< /K -1 /Columns 16 /Rows 2 >>";
    for (dict, data, extra, cleared) in [
        (format!("{image} /ColorSpace /DeviceGray /Filter /CCITTFaxDecode /DecodeParms {parms}"), g4.clone(), vec![], [0x00, 0xFF, 0xFF, 0xFF]),
        (
            format!("{image} /ColorSpace /DeviceGray /Filter [/ASCIIHexDecode /CCITTFaxDecode] /DecodeParms [null 8 0 R]"),
            hex,
            vec![parms.as_bytes().to_vec()],
            [0x00, 0xFF, 0xFF, 0xFF],
        ),
        // A mask's 0 bits paint: cleared bits are set to 1, which paints nothing.
        (format!("{image} /ImageMask true /Filter /CCITTFaxDecode /DecodeParms {parms}"), painted, vec![], [0xFF, 0x00, 0x00, 0x00]),
        // With /Decode [1 0] a mask's 1 bits paint, and cleared bits are 0.
        (format!("{image} /ImageMask true /Decode [1 0] /Filter /CCITTFaxDecode /DecodeParms {parms}"), g4.clone(), vec![], [0x00, 0xFF, 0xFF, 0xFF]),
        (
            format!("{image} /ImageMask true /Decode 8 0 R /Filter /CCITTFaxDecode /DecodeParms {parms}"),
            g4.clone(),
            vec![b"[1 0]".to_vec()],
            [0x00, 0xFF, 0xFF, 0xFF],
        ),
        (
            format!("{image} /ImageMask true /Decode [8 0 R 0] /Filter /CCITTFaxDecode /DecodeParms {parms}"),
            g4.clone(),
            vec![b"1".to_vec()],
            [0x00, 0xFF, 0xFF, 0xFF],
        ),
    ] {
        let mut objs = vec![stream(&dict, data)];
        objs.extend(extra);
        let mut doc = one_page(b"q 160 0 0 20 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 12.0, 75.0, 18.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!((report.images_cleared, report.images_removed), (1, 0), "{dict}");
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert_eq!(named_image(&saved, 0, b"PCRedacted1").as_deref(), Some(&cleared[..]), "{dict}");
    }
}

/// Images whose data is more than twice their raster before its last filter are cleared, not
/// taken for decompression bombs: grey stored as ASCIIHex text broken into lines under Flate (two
/// digits a byte and a line break every 64), and RGB behind an array of filters, which a fax
/// image's bound (a byte a pixel) would refuse.
#[test]
fn images_behind_ascii_hex_in_lines_or_a_filter_array_are_cleared() {
    let (w, h) = (1000usize, 200usize);
    let grey: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
    let digits: Vec<u8> = grey.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    let mut lines: Vec<u8> = digits.chunks(64).flat_map(|line| line.iter().copied().chain(*b"\n")).collect();
    lines.push(b'>');
    let rgb: Vec<u8> = (0..100 * 100 * 3).map(|i| (i % 253) as u8).collect();
    let rgb_hex: Vec<u8> = pdfcraft_filters::encode_flate(&rgb).iter().flat_map(|b| format!("{b:02x}").into_bytes()).chain(*b">").collect();
    for (dict, data) in [
        (
            format!(
                "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter [/FlateDecode /ASCIIHexDecode]"
            ),
            pdfcraft_filters::encode_flate(&lines),
        ),
        (
            "/Type /XObject /Subtype /Image /Width 100 /Height 100 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter [/ASCIIHexDecode /FlateDecode]"
                .to_string(),
            rgb_hex,
        ),
    ] {
        let mut doc = one_page(b"q 100 0 0 20 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![stream(&dict, data)]);
        mark(&mut doc, 0, &[[0.0, 0.0, 10.0, 10.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!((report.images_cleared, report.images_removed), (1, 0), "{dict}");
    }
}

/// A fax image that can't be decoded in full is removed whole, as any other image is: one whose
/// code is cut short, and one behind a filter that inflates past what a code for its pixels can
/// take (a decompression bomb, which `decoded_image` alone would undo up to a gigabyte).
#[test]
fn a_fax_image_that_cannot_be_decoded_in_full_is_removed_whole() {
    let g4 = pdfcraft_filters::encode_g4(&[0xFF; 4], 16, 2).unwrap();
    // A busy picture's code, cut off in its first row.
    let busy = pdfcraft_filters::encode_g4(&[0xAA, 0xAA, 0x55, 0x55], 16, 2).unwrap();
    let short = busy.get(..1).unwrap().to_vec();
    // The real code, then four megabytes of padding the fax decoder would never read.
    let mut padded = g4.clone();
    padded.resize(4 << 20, 0);
    let bomb = pdfcraft_filters::encode_flate(&padded);
    let image = "/Type /XObject /Subtype /Image /Width 16 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1";
    let parms = "<< /K -1 /Columns 16 /Rows 2 >>";
    for (filters, data) in [("/CCITTFaxDecode /DecodeParms", short), ("[/FlateDecode /CCITTFaxDecode] /DecodeParms [null", bomb)] {
        let close = if filters.ends_with("[null") { "]" } else { "" };
        let dict = format!("{image} /Filter {filters} {parms}{close}");
        let mut doc = one_page(b"q 160 0 0 20 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![stream(&dict, data)]);
        mark(&mut doc, 0, &[[0.0, 12.0, 75.0, 18.0]], "");
        let report = apply(&mut doc, None).unwrap();
        assert_eq!((report.images_cleared, report.images_removed), (0, 1), "{dict}");
    }
}

/// Group 3 on bytes is removed whole: the renderer draws some of its layouts as other rows than
/// the decoder reads, so the pixels a mark covers on screen needn't be the ones cleared. Group 3
/// off bytes and Group 4 on bytes are cleared. Each picture is 16 × 2 white, each row a white run
/// of 16 (`101010`): off bytes; on bytes with no end-of-line codes; on bytes after codes whose fill
/// ends each on a byte, at `/K 0` and at `/K 1` (a 1 after each code), `/EncodedByteAlign` given by
/// reference there; and Group 4 rows on bytes, each the one code 1 (vertical, no change).
#[test]
fn group_3_on_bytes_is_removed_whole_and_other_fax_images_cleared() {
    let image = "/Type /XObject /Subtype /Image /Width 16 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode";
    for (k, on_bytes, parms, data, extra, removed) in [
        (0, false, "<< /K 0 /Columns 16 >>", vec![0xAA, 0xA0], vec![], false),
        (0, true, "<< /K 0 /Columns 16 /EncodedByteAlign true >>", vec![0xA8, 0xA8], vec![], true),
        (0, true, "<< /K 0 /Columns 16 /EncodedByteAlign true >>", vec![0x00, 0x01, 0xA8, 0x00, 0x01, 0xA8], vec![], true),
        (1, true, "<< /K 1 /Columns 16 /EncodedByteAlign 8 0 R >>", vec![0x00, 0x01, 0xD4, 0x00, 0x01, 0xD4], vec![b"true".to_vec()], true),
        (-1, true, "<< /K -1 /Columns 16 /EncodedByteAlign true >>", vec![0x80, 0x80], vec![], false),
    ] {
        let p = pdfcraft_filters::CcittParams { k, columns: 16, encoded_byte_align: on_bytes, ..Default::default() };
        assert_eq!(pdfcraft_filters::decode_ccitt(&data, &p, 2).unwrap(), [0xFF; 4], "{parms}: the picture is white");
        let dict = format!("{image} /DecodeParms {parms}");
        let mut objs = vec![stream(&dict, data)];
        objs.extend(extra);
        let mut doc = one_page(b"q 160 0 0 20 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", objs);
        mark(&mut doc, 0, &[[0.0, 12.0, 75.0, 18.0]], "");
        let report = apply(&mut doc, None).unwrap();
        if removed {
            assert_eq!((report.images_cleared, report.images_removed), (0, 1), "{parms}");
        } else {
            assert_eq!((report.images_cleared, report.images_removed), (1, 0), "{parms}");
            let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
            assert_eq!(named_image(&saved, 0, b"PCRedacted1").as_deref(), Some(&[0x00, 0xFF, 0xFF, 0xFF][..]), "{parms}");
        }
    }
}

/// A busy fax image is cleared, though its code is bigger than three times its raster (the bound
/// for other images): a code takes up to about a byte per pixel, and the fax image gets that much.
#[test]
fn a_busy_fax_image_behind_another_filter_is_cleared() {
    // 256 × 256, rows of a one-pixel checkerboard between white ones: each checkered row is coded
    // afresh against the white one above it, about four bits a pixel.
    let rows: Vec<u8> = (0..256).flat_map(|y| [if y % 2 == 0 { 0xAA } else { 0xFF }; 32]).collect();
    let g4 = pdfcraft_filters::encode_g4(&rows, 256, 256).unwrap();
    let raster = 32 * 256;
    assert!(g4.len() > (raster + 256) * 3 + 4096, "{} bytes: within the bound for other images", g4.len());
    let dict = "/Type /XObject /Subtype /Image /Width 256 /Height 256 /ColorSpace /DeviceGray /BitsPerComponent 1 \
                /Filter [/FlateDecode /CCITTFaxDecode] /DecodeParms [null << /K -1 /Columns 256 /Rows 256 >>]";
    let mut doc = one_page(b"q 256 0 0 256 0 0 cm /Im Do Q", "/XObject << /Im 7 0 R >>", vec![stream(dict, pdfcraft_filters::encode_flate(&g4))]);
    mark(&mut doc, 0, &[[0.0, 0.0, 10.0, 10.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!((report.images_cleared, report.images_removed), (1, 0));
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
fn hidden_ocmd_all_on_content_is_removed_before_its_off_group_is_unlinked() {
    use crate::sanitize::{Hidden, remove_hidden};

    let mut doc = one_page(
        b"BT /F1 10 Tf 10 200 Td (PUBLIC) Tj ET /OC /M BDC BT /F1 10 Tf 10 100 Td (SECRET) Tj ET EMC",
        "/Properties << /M 9 0 R >>",
        vec![
            b"<< /Type /OCG /Name (On) >>".to_vec(),
            b"<< /Type /OCG /Name (Off) >>".to_vec(),
            b"<< /Type /OCMD /OCGs [7 0 R 8 0 R] /P /AllOn >>".to_vec(),
        ],
    );
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(
            b"OCProperties".to_vec(),
            Object::Dict(pdfcraft_cos::Dict::from_iter([
                (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                (
                    b"D".to_vec(),
                    Object::Dict(pdfcraft_cos::Dict::from_iter([
                        (b"Order".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                        (b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))])),
                    ])),
                ),
            ])),
        );
    })
    .unwrap();

    let before = pdfcraft_edit::text_lines(&doc, 0).unwrap();
    assert!(before.iter().any(|line| line.text.contains("PUBLIC")));
    assert!(before.iter().any(|line| line.text.contains("SECRET")), "extractor sees source text before sanitization");
    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();

    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let lines = pdfcraft_edit::text_lines(&saved, 0).unwrap();
    assert!(lines.iter().any(|line| line.text.contains("PUBLIC")), "visible content remains: {lines:?}");
    assert!(!lines.iter().any(|line| line.text.contains("SECRET")), "hidden text is extractable: {lines:?}");
    assert!(!write_full(&saved, &SaveOptions::default()).unwrap().windows(6).any(|w| w == b"SECRET"));
}

#[test]
fn hidden_ocmd_content_in_a_form_and_nested_marked_blocks_is_removed() {
    use crate::sanitize::{Hidden, remove_hidden};

    let mut doc = one_page(
        b"/Fm Do",
        "/Properties << /M 9 0 R >> /XObject << /Fm 10 0 R >>",
        vec![
            b"<< /Type /OCG /Name (On) >>".to_vec(),
            b"<< /Type /OCG /Name (Off) >>".to_vec(),
            b"<< /Type /OCMD /OCGs [7 0 R 8 0 R] /P /AllOn >>".to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /Properties << /M 9 0 R >> >>",
                b"/OC /M BDC /OC /M BDC BT /F1 10 Tf 10 100 Td (SECRET) Tj ET EMC EMC",
            ),
        ],
    );
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(
            b"OCProperties".to_vec(),
            Object::Dict(Dict::from_iter([
                (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                (b"D".to_vec(), Object::Dict(Dict::from_iter([(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))]))]))),
            ])),
        );
    })
    .unwrap();

    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert!(!pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("SECRET")));
}

#[test]
fn hidden_ocmd_alloff_is_sanitized_when_every_group_is_on() {
    use crate::sanitize::{Hidden, remove_hidden, scan};

    let mut doc = one_page(
        b"/OC /M BDC BT /F1 10 Tf 10 100 Td (SECRET) Tj ET EMC",
        "/Properties << /M 9 0 R >>",
        vec![
            b"<< /Type /OCG /Name (On) >>".to_vec(),
            b"<< /Type /OCG /Name (Unused) >>".to_vec(),
            b"<< /Type /OCMD /OCGs [7 0 R] /P /AllOff >>".to_vec(),
        ],
    );
    let counts = scan(&doc);
    assert!(counts.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0), "{counts:?}");
    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();

    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert!(!pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("SECRET")));
}

#[test]
fn hidden_ve_not_is_sanitized_when_its_group_is_on() {
    use crate::sanitize::{Hidden, remove_hidden, scan};

    let mut doc = one_page(
        b"/OC /M BDC BT /F1 10 Tf 10 100 Td (SECRET) Tj ET EMC",
        "/Properties << /M 9 0 R >>",
        vec![b"<< /Type /OCG /Name (On) >>".to_vec(), b"<< /Type /OCG /Name (Unused) >>".to_vec(), b"<< /Type /OCMD /VE [/Not 7 0 R] >>".to_vec()],
    );
    let counts = scan(&doc);
    assert!(counts.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0), "{counts:?}");
    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();

    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert!(!pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("SECRET")));
}

fn all_on_ocmd_xobject(subtype: &str, policy: &str) -> Document {
    let membership = match policy {
        "AllOff" => "<< /Type /OCMD /OCGs [7 0 R] /P /AllOff >>",
        "Not" => "<< /Type /OCMD /VE [/Not 7 0 R] >>",
        _ => "<< /Type /OCMD /OCGs [7 0 R] /P /AllOff >>",
    };
    let (object_number, oc) = if policy == "DirectAllOff" { (8, membership) } else { (9, "8 0 R") };
    let xobject = match subtype {
        "Image" => stream(
            &format!("/Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OC {oc}"),
            b"PRIVATE_HIDDEN_XOBJECT_IMAGE",
        ),
        _ => stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /OC {oc}"),
            b"BT /F1 10 Tf 10 100 Td (PRIVATE_HIDDEN_XOBJECT_FORM) Tj ET",
        ),
    };
    let mut extras = vec![b"<< /Type /OCG /Name (On) >>".to_vec()];
    if object_number == 9 {
        extras.push(membership.as_bytes().to_vec());
    }
    extras.push(xobject);
    let mut doc = one_page(b"/X Do", &format!("/XObject << /X {object_number} 0 R >>"), extras);
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(
            b"OCProperties".to_vec(),
            Object::Dict(Dict::from_iter([
                (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0))])),
                (b"D".to_vec(), Object::Dict(Dict::from_iter([(b"BaseState".to_vec(), Object::name("ON"))]))),
            ])),
        );
    })
    .unwrap();
    doc
}

#[test]
fn hidden_all_on_ocmd_image_and_form_xobjects_are_counted_and_removed() {
    use crate::sanitize::{Hidden, remove_hidden, scan};

    let mut failures = Vec::new();
    for subtype in ["Image", "Form"] {
        for policy in ["AllOff", "Not"] {
            let mut doc = all_on_ocmd_xobject(subtype, policy);
            let before = scan(&doc);
            let hidden_count = before.iter().find(|(h, _)| *h == Hidden::HiddenLayers).map_or(0, |(_, n)| *n);
            if hidden_count == 0 {
                failures.push(format!("{subtype} /{policy} was not counted: {before:?}"));
            }

            match remove_hidden(&mut doc, &[Hidden::HiddenLayers]) {
                Ok(removed) if removed.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0) => {}
                Ok(removed) => failures.push(format!("{subtype} /{policy} remove report is empty: {removed:?}")),
                Err(error) => failures.push(format!("{subtype} /{policy} remove failed: {error}")),
            }
            let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
            if bytes.windows(b"PRIVATE_HIDDEN_XOBJECT_IMAGE".len()).any(|w| w == b"PRIVATE_HIDDEN_XOBJECT_IMAGE") {
                failures.push(format!("{subtype} /{policy} image payload remains in full save"));
            }
            if bytes.windows(b"PRIVATE_HIDDEN_XOBJECT_FORM".len()).any(|w| w == b"PRIVATE_HIDDEN_XOBJECT_FORM") {
                failures.push(format!("{subtype} /{policy} Form payload remains in full save"));
            }
            let saved = Document::open(Arc::new(bytes)).unwrap();
            if pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("PRIVATE_HIDDEN_XOBJECT_FORM")) {
                failures.push(format!("{subtype} /{policy} Form text remains extractable"));
            }
            if !pdfcraft_edit::page_images(&saved, 0).unwrap().is_empty() {
                failures.push(format!("{subtype} /{policy} image remains on the page"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn repeated_image_with_direct_alloff_membership_is_counted_per_draw() {
    use crate::sanitize::{Hidden, scan};

    let mut doc = all_on_ocmd_xobject("Image", "DirectAllOff");
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let contents = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), b"/X Do /X Do /X Do")));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(contents))).unwrap();

    let original = write_full(&doc, &SaveOptions::default()).unwrap();
    let counts = scan(&doc);
    assert_eq!(counts.iter().find(|(h, _)| *h == Hidden::HiddenLayers).map(|(_, n)| *n), Some(3), "{counts:?}");
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), original, "Verify scan must not mutate the document");
}

#[test]
fn undecodable_form_is_counted_and_removed_by_hidden_layer_sanitization() {
    use crate::sanitize::{Hidden, remove_hidden, scan};

    let mut doc = one_page(
        b"/Bad Do",
        "/XObject << /Bad 7 0 R >>",
        vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Filter /UnknownFilter", b"PRIVATE_UNDECODABLE_FORM")],
    );
    let counts = scan(&doc);
    assert!(counts.iter().any(|(h, n)| *h == Hidden::HiddenLayers && *n > 0), "{counts:?}");
    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();

    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(b"PRIVATE_UNDECODABLE_FORM".len()).any(|w| w == b"PRIVATE_UNDECODABLE_FORM"));
}

#[test]
fn form_chain_at_max_depth_is_counted_and_removed_by_hidden_layer_sanitization() {
    use crate::sanitize::{Hidden, remove_hidden, scan};

    const PRIVATE: &[u8] = b"PRIVATE_FORM_BEYOND_MAX_DEPTH";
    // The final Form is encountered at MAX_DEPTH and is removed conservatively without being
    // entered. Verify must count that removal or sanitize can skip the HiddenLayers pass entirely.
    let max = crate::interp::MAX_DEPTH;
    let mut forms = Vec::new();
    for depth in 0..=max {
        let content = if depth == max {
            b"BT /F1 10 Tf 10 100 Td (PRIVATE_FORM_BEYOND_MAX_DEPTH) Tj ET".to_vec()
        } else {
            format!("/F{} Do", depth + 1).into_bytes()
        };
        forms.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", &content));
    }
    let xobjects = (0..=max).map(|depth| format!("/F{depth} {} 0 R", 7 + depth)).collect::<Vec<_>>().join(" ");
    let mut doc = one_page(b"/F0 Do", &format!("/XObject << {xobjects} >>"), forms);
    assert!(
        write_full(&doc, &SaveOptions::default()).unwrap().windows(PRIVATE.len()).any(|w| w == PRIVATE),
        "fixture lacked nested payload before sanitization"
    );

    let counts = scan(&doc);
    assert!(counts.iter().any(|(hidden, count)| *hidden == Hidden::HiddenLayers && *count > 0), "depth-limited Form was not counted: {counts:?}");
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
    assert!(
        removed.iter().any(|(hidden, count)| *hidden == Hidden::HiddenLayers && *count > 0),
        "depth-limited Form was not reported removed: {removed:?}"
    );
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(PRIVATE.len()).any(|w| w == PRIVATE), "depth-limited Form payload remains in full save");
}

#[test]
fn hidden_ocmd_resolves_an_indirect_policy_name() {
    use crate::sanitize::{Hidden, remove_hidden};

    let mut doc = one_page(
        b"/OC /M BDC BT /F1 10 Tf 10 100 Td (VISIBLE) Tj ET EMC",
        "/Properties << /M 9 0 R >>",
        vec![
            b"<< /Type /OCG /Name (On) >>".to_vec(),
            b"<< /Type /OCG /Name (Off) >>".to_vec(),
            b"<< /Type /OCMD /OCGs [7 0 R 8 0 R] /P 12 0 R >>".to_vec(),
        ],
    );
    doc.set(ObjRef::new(12, 0), Object::name("AnyOn"));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(
            b"OCProperties".to_vec(),
            Object::Dict(Dict::from_iter([
                (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                (b"D".to_vec(), Object::Dict(Dict::from_iter([(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))]))]))),
            ])),
        );
    })
    .unwrap();

    remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
    let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert!(pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("VISIBLE")));
}

#[test]
fn surviving_anyoff_and_not_content_keeps_its_off_group_state() {
    use crate::sanitize::{Hidden, remove_hidden};

    for use_not in [false, true] {
        let mut doc = one_page(
            b"/OC /M BDC BT /F1 10 Tf 10 100 Td (VISIBLE) Tj ET EMC",
            "/Properties << /M 9 0 R >>",
            vec![
                b"<< /Type /OCG /Name (On) >>".to_vec(),
                b"<< /Type /OCG /Name (Off) >>".to_vec(),
                if use_not { b"<< /Type /OCMD /VE [/Not 8 0 R] >>".to_vec() } else { b"<< /Type /OCMD /OCGs [8 0 R] /P /AnyOff >>".to_vec() },
            ],
        );
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| {
            d.set(
                b"OCProperties".to_vec(),
                Object::Dict(Dict::from_iter([
                    (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                    (b"D".to_vec(), Object::Dict(Dict::from_iter([(b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))]))]))),
                ])),
            );
        })
        .unwrap();

        remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
        let root = doc.root().unwrap();
        let catalog = doc.get(root);
        let oc = catalog.as_dict().and_then(|d| d.get(b"OCProperties")).map(|o| doc.resolve(o));
        let default = oc.as_deref().and_then(|o| o.as_dict()).and_then(|d| d.get(b"D")).map(|o| doc.resolve(o));
        let off = default.as_deref().and_then(|o| o.as_dict()).and_then(|d| d.get(b"OFF")).map(|o| doc.resolve(o));
        assert!(
            off.as_deref().and_then(Object::as_array).is_some_and(|a| a.iter().any(|o| o.as_ref() == Some(ObjRef::new(8, 0)))),
            "off group lost for /VE Not={use_not}"
        );
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        assert!(pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("VISIBLE")));
    }
}

#[test]
fn hidden_ocmd_policy_matches_the_default_configuration() {
    use crate::sanitize::{Hidden, remove_hidden};

    let cases = [
        (None, false, None, false),
        (Some("AnyOn"), false, None, false),
        (Some("AllOn"), false, None, true),
        (Some("AnyOff"), false, None, false),
        (Some("AllOff"), false, None, true),
        (Some("UnknownPolicy"), false, None, true),
        (None, true, None, true),
        (None, false, Some("and"), true),
        (None, false, Some("and_single_not"), true),
        (None, false, Some("and_single_off"), true),
        (None, false, Some("or_single_on"), false),
        (None, false, Some("or_single_off"), true),
        (Some("AllOn"), false, Some("not_off"), false),
        (None, false, Some("malformed_not"), true),
        (None, false, Some("malformed_without_ocgs"), true),
    ];
    for (policy, malformed_policy, visibility, should_remove) in cases {
        let mut doc = one_page(
            b"/OC /M BDC BT /F1 10 Tf 10 100 Td (SECRET) Tj ET EMC",
            "/Properties << /M 9 0 R >>",
            vec![
                b"<< /Type /OCG /Name (On) >>".to_vec(),
                b"<< /Type /OCG /Name (Off) >>".to_vec(),
                b"<< /Type /OCMD /OCGs [7 0 R 8 0 R] /P /AllOn >>".to_vec(),
            ],
        );
        let mut membership = Dict::new();
        membership.set(b"Type".to_vec(), Object::name("OCMD"));
        if visibility != Some("malformed_without_ocgs") {
            membership.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))]));
        }
        if let Some(policy) = policy {
            membership.set(b"P".to_vec(), Object::name(policy));
        } else if malformed_policy {
            membership.set(b"P".to_vec(), Object::Int(17));
        }
        match visibility {
            Some("and") => membership
                .set(b"VE".to_vec(), Object::Array(vec![Object::name("And"), Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
            Some("and_single_not") => membership.set(
                b"VE".to_vec(),
                Object::Array(vec![Object::name("Not"), Object::Array(vec![Object::name("And"), Object::Ref(ObjRef::new(7, 0))])]),
            ),
            Some("and_single_off") => membership.set(b"VE".to_vec(), Object::Array(vec![Object::name("And"), Object::Ref(ObjRef::new(8, 0))])),
            Some("or_single_on") => membership.set(b"VE".to_vec(), Object::Array(vec![Object::name("Or"), Object::Ref(ObjRef::new(7, 0))])),
            Some("or_single_off") => membership.set(b"VE".to_vec(), Object::Array(vec![Object::name("Or"), Object::Ref(ObjRef::new(8, 0))])),
            Some("not_off") => membership.set(b"VE".to_vec(), Object::Array(vec![Object::name("Not"), Object::Ref(ObjRef::new(8, 0))])),
            Some("malformed_not") => membership
                .set(b"VE".to_vec(), Object::Array(vec![Object::name("Not"), Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
            Some("malformed_without_ocgs") => {
                membership.set(b"VE".to_vec(), Object::Array(vec![Object::name("UnknownOperator"), Object::Ref(ObjRef::new(8, 0))]))
            }
            _ => {}
        }
        doc.set(ObjRef::new(9, 0), Object::Dict(membership));
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| {
            d.set(
                b"OCProperties".to_vec(),
                Object::Dict(Dict::from_iter([
                    (b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                    (
                        b"D".to_vec(),
                        Object::Dict(Dict::from_iter([
                            (b"Order".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(8, 0))])),
                            (b"OFF".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))])),
                        ])),
                    ),
                ])),
            );
        })
        .unwrap();

        remove_hidden(&mut doc, &[Hidden::HiddenLayers]).unwrap();
        let saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
        let has_secret = pdfcraft_edit::text_lines(&saved, 0).unwrap().iter().any(|line| line.text.contains("SECRET"));
        assert_eq!(has_secret, !should_remove, "policy {policy:?}, malformed={malformed_policy}, VE={visibility:?}");
    }
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
fn replies_to_a_redacted_comment_go_with_it() {
    // #2659: a highlight under the mark has a reply placed elsewhere, and that reply a reply of
    // its own. The replies' /IRT would keep the highlight (its comment and QuadPoints) in the file.
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>".to_vec(),
        stream("", b"BT ET"),
        b"<< /Type /Annot /Subtype /Highlight /Rect [20 20 60 30] /QuadPoints [20 30 60 30 20 20 60 20] /Contents (HIGHLIGHT-NOTE) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 250 270 270] /IRT 5 0 R /Contents (FIRST-REPLY) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 200 270 220] /IRT 6 0 R /Contents (SECOND-REPLY) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 100 270 120] /Contents (UNRELATED) >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 40.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.annotations, 3, "the highlight and both replies: {r:?}");
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    assert_eq!(annots_of(&doc, &p.dict).len(), 1, "only the unrelated note stays");
    // A classic table, so the objects' strings are plain in the file.
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    for gone in [&b"HIGHLIGHT-NOTE"[..], b"FIRST-REPLY", b"SECOND-REPLY"] {
        assert!(!bytes.windows(gone.len()).any(|w| w == gone), "{} survived the save", String::from_utf8_lossy(gone));
    }
    assert!(bytes.windows(9).any(|w| w == b"UNRELATED"));
}

/// The replies listed before the annotations they answer, deepest first, and a pop-up on a
/// reply: one pass in `/Annots` order would keep them, the repeated passes don't (review of
/// #2697).
#[test]
fn replies_listed_before_their_parent_and_their_pop_ups_go_too() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [7 0 R 9 0 R 6 0 R 5 0 R 8 0 R] >>".to_vec(),
        stream("", b"BT ET"),
        b"<< /Type /Annot /Subtype /Highlight /Rect [20 20 60 30] /QuadPoints [20 30 60 30 20 20 60 20] /Contents (HIGHLIGHT-NOTE) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 250 270 270] /IRT 5 0 R /Contents (FIRST-REPLY) /Popup 9 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 200 270 220] /IRT 6 0 R /Contents (SECOND-REPLY) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 100 270 120] /Contents (UNRELATED) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [200 150 290 190] /Parent 6 0 R >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 40.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.annotations, 3, "the highlight and both replies: {r:?}");
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let left = annots_of(&doc, &p.dict);
    assert_eq!(left, vec![Object::Ref(ObjRef::new(8, 0))], "only the unrelated note stays, the reply's pop-up goes too");
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

/// Clearing marks puts a page's annotation list back as it was: a page that had none has no
/// `/Annots` again, replies to the marks included, and one listed through an indirect array keeps
/// pointing at that array.
#[test]
fn clearing_marks_leaves_the_annotation_list_as_it_was() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB) Tj ET", "", vec![]);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let before = doc.get(page);
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 50.0]], "");
    pdfcraft_annot::add_reply(&mut doc, 0, 0, "a reply", "Tester", &Meta::default()).unwrap();
    // And a reply written inline, as a file can.
    let mut inline = Dict::new();
    inline.set(b"Subtype".to_vec(), Object::name("Text"));
    inline.set(b"IRT".to_vec(), Object::Ref(marks(&doc)[0].obj));
    let mut listed = annots_of(&doc, doc.get(page).as_dict().unwrap());
    listed.push(Object::Dict(inline));
    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(listed))).unwrap();
    assert_eq!(clear_marks(&mut doc, None), Ok(1), "one mark");
    assert_eq!(doc.get(page), before, "the page has no /Annots again: both replies went with their mark");

    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots 4 0 R >>".to_vec(),
        b"[5 0 R]".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".to_vec(),
    ]);
    let (page, list) = (ObjRef::new(3, 0), ObjRef::new(4, 0));
    let before = (doc.get(page), doc.get(list));
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 50.0]], "");
    assert_eq!(doc.get(list).as_array().map(|a| a.len()), Some(2), "the mark is listed in the shared array");
    clear_marks(&mut doc, None).unwrap();
    assert_eq!((doc.get(page), doc.get(list)), before, "the page points at its array, which lists the link alone");
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

/// Leaves a mark with nothing readable: /QuadPoints becomes a name and /Rect is removed.
fn break_area(doc: &mut Document, obj: ObjRef) {
    doc.update_dict(obj, |d| {
        d.set(b"QuadPoints".to_vec(), Object::name("oops"));
        d.remove(b"Rect");
    })
    .unwrap();
}

/// A Redact annotation with no readable area (neither usable QuadPoints nor a usable Rect) used to
/// vanish: `marks` skipped it, so applying the readable mark next to it reported success and left
/// the text under the broken mark in place. Now the whole operation is refused before anything
/// changes, and the count is of the unreadable marks.
#[test]
fn a_mark_with_no_readable_area_is_refused_instead_of_vanishing() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET", "", vec![]);
    mark(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]], "");
    mark(&mut doc, 0, &[[0.0, 95.0, 10.0, 110.0]], "");
    for m in marks(&doc) {
        break_area(&mut doc, m.obj);
    }
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    assert_eq!(marks(&doc).len(), 1, "only the readable mark is listed");
    assert_eq!(apply(&mut doc, None), Err(RedactError::UnreadableMark(2)));
    // Nothing changed: the readable mark is still pending and its text is still drawn.
    assert_eq!(marks(&doc).len(), 1);
    assert!(content(&doc, 0).contains("(AB1234CD) Tj"), "{}", content(&doc, 0));
}

/// A mark whose QuadPoints can't be used still redacts through its Rect, as it always did.
#[test]
fn a_mark_with_only_a_usable_rect_still_redacts() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET", "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let m = marks(&doc).swap_remove(0);
    doc.update_dict(m.obj, |d| {
        d.set(b"QuadPoints".to_vec(), Object::name("oops"));
        d.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(20), Object::Int(95), Object::Int(40), Object::Int(110)]));
    })
    .unwrap();
    assert_eq!(marks(&doc).len(), 1, "the Rect keeps the mark readable");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.marks, r.glyphs), (1, 4));
    assert!(!content(&doc, 0).contains("1234"));
}

/// Only the pages being applied decide the refusal: an unreadable mark on another page does not
/// stop the marks of the selected page from being applied.
#[test]
fn an_unreadable_mark_on_another_page_does_not_stop_the_selected_page() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 8 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (XY) Tj ET"),
    ]);
    mark(&mut doc, 1, &[[10.0, 95.0, 30.0, 110.0]], "");
    let broken = marks(&doc).swap_remove(0);
    break_area(&mut doc, broken.obj);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::UnreadableMark(1)));
    assert_eq!(apply(&mut doc, Some(&[0])).map(|r| r.glyphs), Ok(4));
    assert!(!content(&doc, 0).contains("1234"));
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

/// A one-page document whose catalog names an outline root (object 5). `items` are the outline
/// items, objects 6 onward.
fn outlined_document(items: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream("", b"BT ET"),
        b"<< /Type /Outlines /First 6 0 R >>".to_vec(),
    ];
    objs.extend(items);
    pdf(objs)
}

/// An outline of `levels` levels with two items each. Both items of a level name the first item
/// of the next level as their /First, so each of those items has two parents, and a walk that
/// follows every path takes 2^levels steps. Items a and b of level k are objects 6 + 2k and 7 + 2k.
fn hostile_outline(levels: usize) -> Document {
    let mut items = Vec::new();
    for k in 0..levels {
        let (a, b) = (6 + 2 * k, 7 + 2 * k);
        let child = if k + 1 < levels { format!(" /First {} 0 R", a + 2) } else { String::new() };
        items.push(format!("<< /Title (a) /Next {b} 0 R{child} >>").into_bytes());
        items.push(format!("<< /Title (b){child} /Parent {a} 0 R >>").into_bytes());
    }
    outlined_document(items)
}

/// Runs `work` on another thread and returns its result. A walk that spins fails the test after
/// `bound` instead of hanging the whole run; the thread stays behind and ends with the process.
fn finishes_within<T: Send + 'static>(bound: std::time::Duration, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    match rx.recv_timeout(bound) {
        Ok(out) => out,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("the walk did not finish within {bound:?}: it is spinning on shared outline items")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the walk panicked"),
    }
}

/// Generous for a slow machine: the bounded walk takes a few milliseconds.
const WALK_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

/// An outline whose items name shared children is counted item by item, each item once, and the
/// walk ends. Counting along every path instead takes 2^60 steps.
#[test]
fn an_outline_whose_items_share_children_is_counted_without_spinning() {
    use crate::sanitize::{Hidden, scan};
    let doc = hostile_outline(60);
    let counts = finishes_within(WALK_BOUND, move || scan(&doc));
    assert_eq!(counts.iter().find(|c| c.0 == Hidden::Bookmarks), Some(&(Hidden::Bookmarks, 120)), "60 levels of two distinct items");
}

/// Removing the bookmarks counts them through the same walk first, so it must finish too, and
/// it must take the outline out of the catalog.
#[test]
fn removing_bookmarks_from_an_outline_whose_items_share_children_finishes() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = hostile_outline(60);
    let (done, outlines_left) = finishes_within(WALK_BOUND, move || {
        let done = remove_hidden(&mut doc, &[Hidden::Bookmarks]);
        let root = doc.root().unwrap();
        let left = doc.get(root).as_dict().is_some_and(|c| c.contains(b"Outlines"));
        (done, left)
    });
    assert_eq!(done.unwrap(), [(Hidden::Bookmarks, 120)]);
    assert!(!outlines_left, "the outline is removed from the catalog");
}

/// Nesting and siblings: every item of an ordinary outline is counted once, as before.
#[test]
fn a_nested_outline_is_counted_item_by_item() {
    use crate::sanitize::{Hidden, scan};
    let doc = outlined_document(vec![
        b"<< /Title (Chapter 1) /Parent 5 0 R /First 7 0 R /Last 8 0 R /Next 9 0 R >>".to_vec(),
        b"<< /Title (1.1) /Parent 6 0 R /Next 8 0 R >>".to_vec(),
        b"<< /Title (1.2) /Parent 6 0 R /Prev 7 0 R >>".to_vec(),
        b"<< /Title (Chapter 2) /Parent 5 0 R /Prev 6 0 R /First 10 0 R >>".to_vec(),
        b"<< /Title (2.1) /Parent 9 0 R >>".to_vec(),
    ]);
    assert_eq!(scan(&doc).iter().find(|c| c.0 == Hidden::Bookmarks), Some(&(Hidden::Bookmarks, 5)));
}

/// Two levels of 60,000 distinct items: each level is under the per-level cap, so only the total
/// cap stops the count at 100,000 (without it the count would be 120,000).
#[test]
fn an_outline_counts_no_more_items_than_its_total_budget() {
    use crate::sanitize::{Hidden, scan};
    let per_level = 60_000;
    let mut items = Vec::with_capacity(2 * per_level);
    // Item j is object 6 + j. Level one is items 0..per_level; its first item leads to level two.
    for i in 0..per_level {
        let next = if i + 1 < per_level { format!(" /Next {} 0 R", 7 + i) } else { String::new() };
        let first = if i == 0 { format!(" /First {} 0 R", 6 + per_level) } else { String::new() };
        items.push(format!("<< /Title (a){next}{first} >>").into_bytes());
    }
    for i in 0..per_level {
        let next = if i + 1 < per_level { format!(" /Next {} 0 R", 7 + per_level + i) } else { String::new() };
        items.push(format!("<< /Title (b){next} >>").into_bytes());
    }
    let doc = outlined_document(items);
    assert_eq!(scan(&doc).iter().find(|c| c.0 == Hidden::Bookmarks), Some(&(Hidden::Bookmarks, 100_000)));
}

/// Items written inline (not as indirect objects) count towards the total budget too. Each of
/// 60,000 indirect siblings has one inline child, so 120,000 items are reached, and the total must
/// still stop at 100,000.
#[test]
fn inline_outline_children_count_towards_the_total_budget() {
    use crate::sanitize::{Hidden, scan};
    let siblings = 60_000;
    let mut items = Vec::with_capacity(siblings);
    for i in 0..siblings {
        let next = if i + 1 < siblings { format!(" /Next {} 0 R", 7 + i) } else { String::new() };
        items.push(format!("<< /Title (a){next} /First << /Title (c) >> >>").into_bytes());
    }
    let doc = outlined_document(items);
    assert_eq!(scan(&doc).iter().find(|c| c.0 == Hidden::Bookmarks), Some(&(Hidden::Bookmarks, 100_000)));
}

/// Issue #2447: in a tagged PDF the structure tree points at the link and the comment it tags
/// (`/OBJR`). Removing them must drop those references too, or the full rewrite keeps the
/// annotations, the URL and the comment text included, while the page no longer lists them.
#[test]
fn removed_links_and_comments_leave_the_structure_tree_and_the_saved_file() {
    use crate::sanitize::{Hidden, remove_hidden};
    let objs: Vec<Vec<u8>> = vec![
        // 1 catalog, with a structure tree
        b"<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 8 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        // 3 page: a tagged paragraph (MCID 0), a link and a comment, both tagged
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R] /StructParents 0 >>".to_vec(),
        stream("", b"/P << /MCID 0 >> BDC BT /F1 10 Tf 10 230 Td (See the site) Tj ET EMC"),
        FONT.replace("95 0 R", "13 0 R").into_bytes(),
        // 6 link, 7 comment
        b"<< /Type /Annot /Subtype /Link /Rect [10 228 80 240] /StructParent 1 /A << /S /URI /URI (https://secret.example/token=ZX81) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /StructParent 2 /Contents (COMMENTSECRETNOTE) >>".to_vec(),
        // 8 struct tree root, 9 document element, 10 paragraph, 11 link element (direct OBJR),
        // 12 annotation element (OBJR by reference, as K's only kid)
        b"<< /Type /StructTreeRoot /K 9 0 R /ParentTree << /Nums [0 [10 0 R] 1 11 0 R 2 12 0 R] >> /ParentTreeNextKey 3 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 8 0 R /K [10 0 R 11 0 R 12 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 9 0 R /Pg 3 0 R /K 0 >>".to_vec(),
        // the link element's alternate text repeats the URL
        b"<< /Type /StructElem /S /Link /P 9 0 R /Pg 3 0 R /Alt (secret.example link) /K [<< /Type /OBJR /Obj 6 0 R /Pg 3 0 R >>] >>".to_vec(),
        b"<< /Type /StructElem /S /Annot /P 9 0 R /Pg 3 0 R /K 14 0 R >>".to_vec(),
        widths(),
        b"<< /Type /OBJR /Obj 7 0 R /Pg 3 0 R >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    let done = remove_hidden(&mut doc, &[Hidden::LinksActionsScripts, Hidden::Comments]).unwrap();
    assert!(done.contains(&(Hidden::LinksActionsScripts, 1)) && done.contains(&(Hidden::Comments, 1)), "{done:?}");
    // Without object streams, whose compression would hide the strings from the search.
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"(See the site)"), "the search sees the saved objects");
    assert!(!has(b"secret.example"), "the removed link's URL survived the save");
    assert!(!has(b"COMMENTSECRETNOTE"), "the removed comment's text survived the save");
    // The tagged paragraph and its content stay; the elements lose only the references.
    let saved = Document::open(Arc::new(bytes)).unwrap();
    assert!(content(&saved, 0).contains("(See the site)"));
    let objrs = saved.object_numbers().into_iter().filter(|&n| {
        let o = saved.get(ObjRef::new(n, saved.generation(n)));
        let direct = o.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OBJR"));
        let nested = o.as_dict().and_then(|d| d.get(b"K")).is_some_and(|k| match &*saved.resolve(k) {
            Object::Array(a) => a.iter().any(|x| saved.resolve(x).as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OBJR"))),
            _ => false,
        });
        direct || nested
    });
    assert_eq!(objrs.count(), 0, "no object reference to a removed annotation is left");
    let root = saved.get(saved.root().unwrap()).as_dict().cloned().unwrap();
    assert!(root.contains(b"StructTreeRoot"), "the structure tree itself stays");
}

/// #2532 review: a tagged form's widgets, flattened when form fields are removed, leave the
/// structure tree too (their field value with them), and a pop-up listed on another page than
/// its removed comment goes with it.
#[test]
fn flattened_widgets_and_far_pop_ups_leave_the_structure_tree() {
    use crate::sanitize::{Hidden, remove_hidden};
    let objs: Vec<Vec<u8>> = vec![
        // 1 catalog with a form and a structure tree
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> /StructTreeRoot 8 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 13 0 R] /Count 2 >>".to_vec(),
        // 3 page 1: a text field's widget and a comment
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R] /StructParents 0 >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 230 Td (Name:) Tj ET"),
        FONT.replace("95 0 R", "12 0 R").into_bytes(),
        // 6 the field and its widget, with an appearance
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (FIELDSECRETVALUE) /Rect [50 225 200 240] /StructParent 1 /AP << /N 11 0 R >> >>".to_vec(),
        // 7 comment, its pop-up (15) listed on page 2
        b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents (note) /Popup 15 0 R >>".to_vec(),
        // 8 struct tree root, 9 document, 10 form element tagging the widget
        b"<< /Type /StructTreeRoot /K 9 0 R /ParentTree << /Nums [1 10 0 R] >> /ParentTreeNextKey 2 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 8 0 R /K [10 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /Form /P 9 0 R /Pg 3 0 R /K [<< /Type /OBJR /Obj 6 0 R /Pg 3 0 R >>] >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 150 15] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 2 3 Td (FIELDSECRETVALUE) Tj ET"),
        widths(),
        // 13 page 2, listing the pop-up of page 1's comment
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [15 0 R 16 0 R] >>".to_vec(),
        b"<< >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [0 0 100 50] /Parent 7 0 R >>".to_vec(),
        // 16 an unrelated link on page 2 stays
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    remove_hidden(&mut doc, &[Hidden::FormFields, Hidden::Comments]).unwrap();
    let page2 = pdfcraft_model::pages(&doc)[1].dict.clone();
    let left: Vec<Object> = page2.get(b"Annots").map(|a| doc.resolve(a).as_array().map(|a| a.to_vec()).unwrap_or_default()).unwrap_or_default();
    assert_eq!(left, [Object::Ref(ObjRef::new(16, 0))], "the far pop-up went with its comment");
    let form = doc.get(ObjRef::new(10, 0)).as_dict().cloned().unwrap();
    let kids = form.get(b"K").map(|k| doc.resolve(k).as_array().map(Vec::len).unwrap_or(1));
    assert!(kids.unwrap_or(0) == 0, "the form element no longer references the flattened widget");
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let field_left = saved.object_numbers().into_iter().any(|n| {
        saved.get(ObjRef::new(n, saved.generation(n))).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget") || d.contains(b"FT"))
    });
    assert!(!field_left, "the flattened field didn't survive the save through the structure tree");
}

/// Issue #2874: applying redactions removes the comment and the form field under a mark, and
/// with them the structure tree's object references (`/OBJR`) to them, so a full rewrite of a
/// tagged PDF can't keep the comment's text or the field's value. The field's widget on another
/// page goes with it; a comment outside the mark keeps its reference.
#[test]
fn applied_redactions_drop_structure_tree_references_to_what_they_remove() {
    let objs: Vec<Vec<u8>> = vec![
        // 1 catalog with a form and a structure tree
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> /MarkInfo << /Marked true >> /StructTreeRoot 9 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 16 0 R] /Count 2 >>".to_vec(),
        // 3 page 1: a label, a text field's widget and a comment under the mark, a comment outside
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [7 0 R 8 0 R 15 0 R] /StructParents 0 >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 230 Td (Name:) Tj ET"),
        FONT.replace("95 0 R", "14 0 R").into_bytes(),
        // 6 the field, 7 its widget on page 1 (17 is its widget on page 2), 8 the comment
        b"<< /FT /Tx /T (name) /V (FIELDSECRETVALUE) /Kids [7 0 R 17 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R /P 3 0 R /Rect [50 225 200 240] /StructParent 1 /AP << /N 13 0 R >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /P 3 0 R /Rect [200 200 220 220] /StructParent 2 /Contents (COMMENTSECRETNOTE) >>".to_vec(),
        // 9 struct tree root, 10 document, 11 form element (direct OBJRs to both widgets),
        // 12 annotation element (OBJR to the comment), 18 annotation element for the kept comment
        b"<< /Type /StructTreeRoot /K 10 0 R /ParentTree << /Nums [1 11 0 R 2 12 0 R 3 18 0 R 4 11 0 R] >> /ParentTreeNextKey 5 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 9 0 R /K [11 0 R 12 0 R 18 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /Form /P 10 0 R /Pg 3 0 R /Alt (FIELDSECRETVALUE) /K [<< /Type /OBJR /Obj 7 0 R /Pg 3 0 R >> << /Type /OBJR /Obj 17 0 R /Pg 16 0 R >>] >>".to_vec(),
        b"<< /Type /StructElem /S /Annot /P 10 0 R /Pg 3 0 R /K [<< /Type /OBJR /Obj 8 0 R /Pg 3 0 R >>] >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 150 15] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 2 3 Td (FIELDSECRETVALUE) Tj ET"),
        widths(),
        // 15 the comment outside the mark
        b"<< /Type /Annot /Subtype /Text /P 3 0 R /Rect [250 10 270 30] /StructParent 3 /Contents (Kept note) >>".to_vec(),
        // 16 page 2, with the field's second widget
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [17 0 R] /StructParents 4 >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R /P 16 0 R /Rect [50 50 200 65] /StructParent 4 /AP << /N 13 0 R >> >>".to_vec(),
        b"<< /Type /StructElem /S /Annot /P 10 0 R /Pg 3 0 R /K [<< /Type /OBJR /Obj 15 0 R /Pg 3 0 R >>] >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[40.0, 190.0, 230.0, 245.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!((report.annotations, report.fields), (1, 1), "{report:?}");
    assert!(report.tags >= 3, "the three references to the comment and both widgets are counted: {report:?}");
    // Without object streams, whose compression would hide the strings from the search.
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"(Name:)") && has(b"(Kept note)"), "the search sees the saved objects");
    assert!(!has(b"COMMENTSECRETNOTE"), "the redacted comment's text survived the save through the structure tree");
    assert!(!has(b"FIELDSECRETVALUE"), "the redacted field's value survived the save through the structure tree");
    let saved = Document::open(Arc::new(bytes)).unwrap();
    let widget_left = saved
        .object_numbers()
        .into_iter()
        .any(|n| saved.get(ObjRef::new(n, saved.generation(n))).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")));
    assert!(!widget_left, "neither of the field's widgets survived the save");
    // The kept comment is still tagged.
    let kept = saved.object_numbers().into_iter().any(|n| {
        saved.get(ObjRef::new(n, saved.generation(n))).as_dict().and_then(|d| d.get(b"K")).is_some_and(|k| match &*saved.resolve(k) {
            Object::Array(a) => a.iter().any(|x| {
                saved.resolve(x).as_dict().and_then(|d| d.get(b"Obj")).and_then(Object::as_ref).is_some_and(|r| {
                    saved.get(r).as_dict().and_then(|d| d.get(b"Contents")).and_then(Object::as_string).is_some_and(|s| s.to_text() == "Kept note")
                })
            }),
            _ => false,
        })
    });
    assert!(kept, "the comment outside the mark keeps its structure reference");
}

/// #2874: a structure element written in place that tags a comment under a mark can't be edited
/// without rewriting what holds it, so applying is refused rather than leaving the comment
/// reachable (fail-closed, as in Sanitize).
#[test]
fn applying_redactions_refuses_an_in_place_element_tagging_a_removed_comment() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /StructParent 0 /Contents (COMMENTSECRETNOTE) >>".to_vec(),
        b"<< >>".to_vec(),
        // 6 the root's only kid, an annotation element written in place
        b"<< /Type /StructTreeRoot /K [<< /Type /StructElem /S /Annot /K [<< /Type /OBJR /Obj 4 0 R >>] >>] >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[190.0, 190.0, 230.0, 230.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::Tags)));
}

/// #2532 review: a structure tree larger than the walk's limit can't be searched in full, so a
/// reference the walk never reached could keep a removed annotation. Then it is an error, and
/// nothing is changed, rather than success with references left behind (removing only the tree
/// would leave an outline's `/SE` reaching them, review on #3285); under the limit only the
/// reference goes.
#[test]
fn a_structure_tree_too_large_to_walk_is_refused() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>".to_vec(),
        // 4: the removed link
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI (https://secret.example) >> >>".to_vec(),
        // 5 root, 6 document, 7 section, 8 link element with the OBJR
        b"<< /Type /StructTreeRoot /K 6 0 R >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R /K [7 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /Sect /P 6 0 R /K [8 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /Link /P 7 0 R /K [<< /Type /OBJR /Obj 4 0 R >>] >>".to_vec(),
    ];
    let gone: std::collections::HashSet<ObjRef> = [ObjRef::new(4, 0)].into();
    let catalog = |doc: &Document| doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();

    let mut doc = pdf(objs.clone());
    assert_eq!(crate::tags::drop_object_refs_within(&mut doc, &gone, 100).unwrap(), 1);
    assert!(catalog(&doc).contains(b"StructTreeRoot") && catalog(&doc).contains(b"MarkInfo"), "under the limit the tree stays");

    let mut doc = pdf(objs);
    assert!(matches!(crate::tags::drop_object_refs_within(&mut doc, &gone, 2), Err(RedactError::Tags)), "a tree the walk couldn't finish is refused");
    assert!(catalog(&doc).contains(b"StructTreeRoot"), "nothing changed");
}

/// #3302: an element listed only in the parent tree (not under the root's /K) still owns the
/// marked content redaction changed, so its /ActualText and /Alt, which repeat the redacted
/// words, go as they do for an element under /K.
#[test]
fn an_element_listed_only_in_the_parent_tree_loses_its_redacted_text() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 9 0 R >> >> >>".to_vec(),
        stream("", b"/P <</MCID 0>> BDC BT /F1 14 Tf 20 150 Td (Codename Bluefin) Tj ET EMC"),
        // The root's /K reaches only the empty document element 6; the paragraph 7 is in the
        // parent tree only.
        b"<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 [7 0 R]] >> /ParentTreeNextKey 1 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K 0 /ActualText (Codename Bluefin) /Alt (Codename Bluefin) >>".to_vec(),
        b"<< /Type /StructElem /S /Note /P 6 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 140.0, 290.0, 170.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.glyphs > 0 && r.tags >= 1, "{r:?}");
    let element = doc.get(ObjRef::new(7, 0)).as_dict().cloned().unwrap();
    assert!(!element.contains(b"ActualText") && !element.contains(b"Alt"), "{element:?}");
}

/// #3302: a structure tree too large to walk in full fails closed: an error, so the redaction
/// isn't made, instead of the walk stopping and reporting success with elements never checked.
#[test]
fn a_tag_walk_cut_short_is_refused() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /StructParents 0 >>".to_vec(),
        b"<< >>".to_vec(),
        b"<< /Type /StructTreeRoot /K 6 0 R >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R /K [7 0 R 8 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K 0 /ActualText (Codename Bluefin) >>".to_vec(),
        b"<< /Type /StructElem /S /Figure /P 6 0 R /Alt (A chart) /E (chart) >>".to_vec(),
    ];
    let page = ObjRef::new(3, 0);
    let changed: std::collections::HashSet<i64> = [0].into();
    let mut doc = pdf(objs.clone());
    assert_eq!(crate::tags::clean_within(&mut doc, page, None, &changed, &Default::default(), 100).unwrap(), 1);
    let figure = doc.get(ObjRef::new(8, 0)).as_dict().cloned().unwrap();
    assert!(figure.contains(b"Alt"), "a walk that finishes keeps an unrelated element's text");

    let mut doc = pdf(objs);
    assert!(matches!(crate::tags::clean_within(&mut doc, page, None, &changed, &Default::default(), 1), Err(RedactError::Tags)));
}

/// #2532 follow-up: an element reached only through the parent tree or the ID tree, not from
/// the root's /K, still points at a removed annotation, and the root keeps it reachable in a full
/// rewrite. Both trees are walked too, written in place or as their own objects.
#[test]
fn elements_listed_only_in_the_parent_tree_or_the_id_tree_lose_their_references() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>".to_vec(),
        // 4: the removed link; 8: the removed comment
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI (https://secret.example) >> >>".to_vec(),
        // 5: the root; its /K reaches only the empty document element 6. The parent tree is
        // written in place, the ID tree's kid (9) as its own object.
        b"<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 7 0 R] >> /IDTree << /Kids [9 0 R] >> >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R >>".to_vec(),
        // 7: a link element only the parent tree lists
        b"<< /Type /StructElem /S /Link /P 6 0 R /K << /Type /OBJR /Obj 4 0 R >> /Alt (secret.example link) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (hushhush note) >>".to_vec(),
        b"<< /Names [(note1) 10 0 R] >>".to_vec(),
        // 10: an annotation element only the ID tree lists
        b"<< /Type /StructElem /S /Annot /P 6 0 R /ID (note1) /K [<< /Type /OBJR /Obj 8 0 R >>] >>".to_vec(),
    ];
    let gone: std::collections::HashSet<ObjRef> = [ObjRef::new(4, 0), ObjRef::new(8, 0)].into();
    let mut doc = pdf(objs);
    assert_eq!(crate::tags::drop_object_refs(&mut doc, &gone).unwrap(), 2, "both references go");
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    for secret in [&b"secret.example"[..], b"hushhush"] {
        assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{} survived the rewrite", String::from_utf8_lossy(secret));
    }
}

/// #3285 review: an element without its own /Pg takes its ancestor's page, also when the parent
/// tree lists it. One under /K must not be reached first through the parent tree without that
/// page, and one only the parent tree lists finds it through /P.
#[test]
fn an_element_inheriting_its_page_loses_its_redacted_text_when_the_parent_tree_lists_it() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 9 0 R >> >> >>".to_vec(),
        stream("", b"/P <</MCID 0>> BDC BT /F1 14 Tf 20 150 Td (Codename Bluefin) Tj ET EMC /P <</MCID 1>> BDC BT /F1 14 Tf 20 100 Td (Codename Bluefin) Tj ET EMC"),
        // The document element 6 carries the page; 7 sits under it, 8 only points at it.
        b"<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 [7 0 R 8 0 R]] >> /ParentTreeNextKey 1 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R /Pg 3 0 R /K [7 0 R] >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 6 0 R /K 0 /ActualText (Codename Bluefin) >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 6 0 R /K 1 /Alt (Codename Bluefin) >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 90.0, 290.0, 170.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.glyphs > 0, "{r:?}");
    for n in [7, 8] {
        let element = doc.get(ObjRef::new(n, 0)).as_dict().cloned().unwrap();
        assert!(!element.contains(b"ActualText") && !element.contains(b"Alt"), "{n}: {element:?}");
    }
}

/// #3285 review: a structure element written in place, here as an ID tree value, can't be
/// edited without rewriting what holds it; when it tags a removed annotation, removing hidden
/// information is refused instead of keeping the annotation reachable in a full rewrite.
/// Direct callers discard the failed working copy to preserve the original document.
#[test]
fn an_element_written_in_place_that_tags_a_removed_annotation_is_refused() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (hushhush note) >>".to_vec(),
        b"<< /Type /StructTreeRoot /K 6 0 R /IDTree << /Names [(note1) << /Type /StructElem /S /Annot /P 6 0 R /K [<< /Type /OBJR /Obj 4 0 R >>] >>] >> >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R >>".to_vec(),
    ];
    let original = pdf(objs);
    let before = write_full(&original, &SaveOptions::default()).unwrap();
    let mut working = original.clone();
    assert!(matches!(sanitize::remove_hidden(&mut working, &[sanitize::Hidden::Comments]), Err(RedactError::Tags)));
    assert!(!working.get(ObjRef::new(3, 0)).as_dict().unwrap().contains(b"Annots"), "the failed working copy was partially changed");
    drop(working);
    assert_eq!(write_full(&original, &SaveOptions::default()).unwrap(), before, "discarding the working copy preserves the original");
}

/// Whether a full rewrite of `doc` still holds the redacted word.
fn bluefin_survives(doc: &Document) -> bool {
    let bytes = write_full(doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    bytes.windows(7).any(|w| w == b"Bluefin")
}

/// #3285 review (the maintainer's probes): an element written in place in the parent tree that
/// owns the redacted text can't be changed, so applying is refused; the same element as an object
/// of its own is cleaned, and nothing of the word is left in a full rewrite. A refusal leaves the
/// document as it was.
#[test]
fn an_element_written_in_place_owning_redacted_text_is_refused() {
    let objs = |element: &str| -> Vec<Vec<u8>> {
        vec![
            b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 9 0 R >> >> >>"
                .to_vec(),
            stream("", b"/P <</MCID 0>> BDC BT /F1 14 Tf 20 150 Td (Codename Bluefin) Tj ET EMC"),
            format!("<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 [{element}]] >> /ParentTreeNextKey 1 >>").into_bytes(),
            b"<< /Type /StructElem /S /Document /P 5 0 R >>".to_vec(),
            b"<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K 0 /ActualText (Codename Bluefin) >>".to_vec(),
            b"<< >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
        ]
    };
    let mut doc = pdf(objs("<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K 0 /ActualText (Codename Bluefin) >>"));
    mark(&mut doc, 0, &[[10.0, 140.0, 290.0, 170.0]], "");
    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let error = apply(&mut doc, None).unwrap_err();
    assert!(matches!(error, RedactError::Tags));
    assert_eq!(
        error.to_string(),
        "the document's tags can't be cleaned completely (an element written in place, or a structure tree too large or too deep to follow); the operation was refused"
    );
    assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "the refused document is unchanged");
    let mut doc = pdf(objs("7 0 R"));
    mark(&mut doc, 0, &[[10.0, 140.0, 290.0, 170.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.glyphs > 0 && !bluefin_survives(&doc), "{r:?}");
}

/// #3285 review: an element whose inherited page can't be found (more ancestors than the lookup
/// follows) isn't taken for one with no page: applying is refused. Within the limit the page is
/// found and the text goes.
#[test]
fn an_element_whose_inherited_page_is_out_of_reach_is_refused() {
    let objs = |depth: u32| -> Vec<Vec<u8>> {
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 9 0 R >> >> >>"
                .to_vec(),
            stream("", b"/P <</MCID 0>> BDC BT /F1 14 Tf 20 150 Td (Codename Bluefin) Tj ET EMC"),
            b"<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 [7 0 R]] >> /ParentTreeNextKey 1 >>".to_vec(),
            b"<< /Type /StructElem /S /Document /P 5 0 R /Pg 3 0 R >>".to_vec(),
            format!("<< /Type /StructElem /S /P /P {} 0 R /K 0 /ActualText (Codename Bluefin) >>", 10 + depth - 1).into_bytes(),
            b"<< >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
        ];
        for i in 0..depth {
            let parent = if i == 0 { 6 } else { 10 + i - 1 };
            objs.push(format!("<< /Type /StructElem /S /Div /P {parent} 0 R >>").into_bytes());
        }
        objs
    };
    let mut doc = pdf(objs(300));
    mark(&mut doc, 0, &[[10.0, 140.0, 290.0, 170.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::Tags)));
    let mut doc = pdf(objs(100));
    mark(&mut doc, 0, &[[10.0, 140.0, 290.0, 170.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(!bluefin_survives(&doc));
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

#[test]
fn nested_resource_less_forms_keep_live_resources_after_redacting_another_page() {
    for own_resources in [false, true] {
        for still_drawn in [false, true] {
            let original = b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET";
            let mut extra = vec![
                stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", original),
                stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", if still_drawn { b"/Fm Do".as_slice() } else { b"q Q".as_slice() }),
            ];
            if own_resources {
                extra.push(stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm 7 0 R /Relay 8 0 R >> >>",
                    b"/Relay Do",
                ));
            }
            let resources =
                if own_resources { "/XObject << /Fm 7 0 R /Relay 8 0 R /Wrapper 9 0 R >>" } else { "/XObject << /Fm 7 0 R /Relay 8 0 R >>" };
            let mut doc = one_page(b"/Fm Do", resources, extra);
            let mut page = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
            let inherited = page.remove(b"Resources").unwrap();
            doc.set(ObjRef::new(3, 0), Object::Dict(page.clone()));
            let second_content =
                doc.add(Object::Stream(Stream::from_raw(Dict::new(), if own_resources { b"/Wrapper Do".to_vec() } else { b"/Relay Do".to_vec() })));
            page.set(b"Contents".to_vec(), Object::Ref(second_content));
            let second = doc.add(Object::Dict(page));
            doc.update_dict(ObjRef::new(2, 0), |d| {
                d.set(b"Resources".to_vec(), inherited);
                d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(second)]));
                d.set(b"Count".to_vec(), Object::Int(2));
            })
            .unwrap();

            let region = [[0.0, 190.0, 300.0, 220.0]];
            let surviving = if still_drawn { 6 } else { 0 };
            assert_eq!(under(&mut doc, 0, &region), 6, "the marked occurrence is real text");
            assert_eq!(under(&mut doc, 1, &region), surviving, "the relay's original nested content is visible");
            mark(&mut doc, 0, &region, "");
            let report = apply(&mut doc, Some(&[0])).unwrap();
            assert_eq!((report.marks, report.glyphs), (1, 6));
            assert_eq!(under(&mut doc, 0, &region), 0);
            assert_eq!(under(&mut doc, 1, &region), surviving, "redacting another page must preserve live nested content");

            // A full saved document must retain both the reachable resource and its content,
            // without relying on an old incremental revision's otherwise-unreachable objects.
            let mut saved = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
            assert_eq!(under(&mut saved, 0, &region), 0);
            assert_eq!(under(&mut saved, 1, &region), surviving);
            let page = pdfcraft_model::pages(&saved).swap_remove(1);
            let res = saved.resolve(page.dict.get(b"Resources").unwrap());
            let xo = saved.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
            let mut holder = xo.as_dict().unwrap().clone();
            if own_resources {
                let wrapper = saved.resolve(holder.get(b"Wrapper").unwrap());
                let Object::Stream(wrapper) = &*wrapper else { panic!("a real wrapper form") };
                let res = saved.resolve(wrapper.dict.get(b"Resources").unwrap());
                let xo = saved.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
                holder = xo.as_dict().unwrap().clone();
            }
            assert_eq!(holder.contains(b"Fm"), still_drawn, "keep only resources a surviving nested form actually uses");
            if still_drawn {
                let form = saved.resolve(holder.get(b"Fm").unwrap());
                let Object::Stream(form) = &*form else { panic!("the preserved original form") };
                assert_eq!(form.decoded().unwrap(), original);
            }
        }
    }
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
fn replacement_text_and_property_resources_do_not_survive_redaction() {
    let mut doc = one_page(
        b"/Span /P0 BDC /Span <</ActualText (LEAK_INLINE)>> BDC BT /F1 10 Tf 10 200 Td (SECRET)",
        "/Properties << /P0 7 0 R >>",
        vec![
            b"<< /MCID 0 /ActualText (LEAK_PROPERTY) /Alt (LEAK_ALT) /E (LEAK_EXPANSION) >>".to_vec(),
            stream("", b"Tj ET EMC EMC /Span <</ActualText (Public point)>> DP BT /F1 10 Tf 10 100 Td (Public) Tj ET"),
        ],
    );
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(4, 0)), Object::Ref(ObjRef::new(8, 0))]));
    })
    .unwrap();
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(&mut doc, None).unwrap();
    assert_no_secret(&doc, b"LEAK_");
    let saved = reopen(&doc);
    assert!(content(&saved, 0).contains("Public"));
    let p = pdfcraft_model::pages(&saved).swap_remove(0);
    assert!(!saved.resolve(p.dict.get(b"Resources").unwrap()).as_dict().unwrap().contains(b"Properties"));
}

#[test]
fn inherited_form_properties_lose_replacement_text() {
    let mut doc = one_page(
        b"/Fm Do",
        "/XObject << /Fm 7 0 R >> /Properties << /P0 8 0 R >>",
        vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300]",
                b"/Span /P0 BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC BT /F1 10 Tf 10 100 Td (Public) Tj ET",
            ),
            b"<< /ActualText (LEAK_FORM_PROPERTY) >>".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(&mut doc, None).unwrap();
    assert_no_secret(&doc, b"LEAK_");
    assert_no_secret(&doc, b"SECRET");
    let mut saved = reopen(&doc);
    assert_eq!(under(&mut saved, 0, &[[0.0, 90.0, 100.0, 110.0]]), 6);
}

#[test]
fn properties_used_by_unmodified_inheriting_forms_stay() {
    let mut doc = one_page(
        b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET /Fm Do",
        "/XObject << /Fm 7 0 R >> /Properties << /P0 8 0 R >>",
        vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"/Span /P0 BDC 0 0 10 10 re f EMC"), b"<< /ActualText (Public) >>".to_vec()],
    );
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    apply(&mut doc, None).unwrap();
    let saved = reopen(&doc);
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    let resources = saved.resolve(page.dict.get(b"Resources").unwrap());
    let properties = saved.resolve(resources.as_dict().unwrap().get(b"Properties").unwrap());
    let property = saved.resolve(properties.as_dict().unwrap().get(b"P0").unwrap());
    assert_eq!(property.as_dict().unwrap().get(b"ActualText").unwrap().as_string().unwrap().to_text(), "Public");
}

#[test]
fn untouched_replacement_descriptions_survive_redaction_and_hidden_text_cleanup() {
    for hidden in [false, true] {
        let mode = if hidden { 3 } else { 0 };
        let data = format!(
            "/Span <</ActualText (LEAK_REMOVED)>> BDC BT /F1 10 Tf {mode} Tr 10 200 Td (SECRET) Tj ET EMC \
             /Span /Public BDC BT /F1 10 Tf 0 Tr 10 100 Td (Public) Tj ET EMC"
        );
        let mut doc = one_page(
            data.as_bytes(),
            "/Properties << /Public 7 0 R >>",
            vec![b"<< /ActualText (PUBLIC_ACTUAL) /Alt (PUBLIC_ALT) /E (PUBLIC_EXPANSION) /T (PUBLIC_TITLE) >>".to_vec()],
        );
        if hidden {
            sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenText]).unwrap();
        } else {
            mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
            apply(&mut doc, None).unwrap();
        }
        assert_no_secret(&doc, b"LEAK_");
        assert_no_secret(&doc, b"SECRET");
        let saved = reopen(&doc);
        let page = pdfcraft_model::pages(&saved).swap_remove(0);
        let resources = saved.resolve(page.dict.get(b"Resources").unwrap());
        let properties = saved.resolve(resources.as_dict().unwrap().get(b"Properties").unwrap());
        let property = saved.resolve(properties.as_dict().unwrap().get(b"Public").unwrap());
        let dict = property.as_dict().unwrap();
        for key in [&b"ActualText"[..], b"Alt", b"E", b"T"] {
            assert!(dict.contains(key), "surviving description removed: {key:?}, hidden={hidden}");
        }
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

#[test]
fn named_hidden_text_descriptions_are_removed_from_saved_bytes() {
    let mut doc = one_page(
        b"/Span /P0 BDC BT /F1 10 Tf 3 Tr 10 200 Td (SECRET) Tj ET EMC",
        "/Properties << /P0 7 0 R >>",
        vec![b"<< /ActualText (LEAK_ACTUAL) /Alt (LEAK_ALT) /E (LEAK_EXPANSION) /T (LEAK_TITLE) >>".to_vec()],
    );
    sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenText]).unwrap();
    assert_no_secret(&doc, b"SECRET");
    assert_no_secret(&doc, b"LEAK_");
    let saved = reopen(&doc);
    let page = pdfcraft_model::pages(&saved).swap_remove(0);
    assert!(!saved.resolve(page.dict.get(b"Resources").unwrap()).as_dict().unwrap().contains(b"Properties"));
}

#[test]
fn unused_descriptions_in_shared_or_inherited_resources_are_retired() {
    for hidden in [false, true] {
        for inherited in [false, true] {
            let mode = if hidden { 3 } else { 0 };
            let data = format!("/Span /P0 BDC BT /F1 10 Tf {mode} Tr 10 200 Td (SECRET) Tj ET EMC /Fm Do");
            let mut doc = one_page(
                data.as_bytes(),
                "/Properties << /P0 7 0 R /Public 8 0 R >> /XObject << /Fm 9 0 R >>",
                vec![
                    b"<< /ActualText (LEAK_ACTUAL) /Alt (LEAK_ALT) /E (LEAK_EXPANSION) /T (LEAK_TITLE) >>".to_vec(),
                    b"<< /ActualText (PUBLIC_ACTUAL) /Alt (PUBLIC_ALT) /E (PUBLIC_EXPANSION) /T (PUBLIC_TITLE) >>".to_vec(),
                    stream("/Type /XObject /Subtype /Form /BBox [0 0 100 120]", b"/Span /Public BDC BT /F1 10 Tf 0 Tr 10 100 Td (Public) Tj ET EMC"),
                ],
            );
            let first = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
            let resources = doc.add(first.get(b"Resources").unwrap().clone());
            let second_content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec())));
            let mut second = first;
            second.set(b"Contents".to_vec(), Object::Ref(second_content));
            if inherited {
                second.remove(b"Resources");
            } else {
                second.set(b"Resources".to_vec(), Object::Ref(resources));
            }
            let second = doc.add(Object::Dict(second));
            doc.update_dict(ObjRef::new(3, 0), |d| {
                if inherited {
                    d.remove(b"Resources");
                } else {
                    d.set(b"Resources".to_vec(), Object::Ref(resources));
                }
            })
            .unwrap();
            doc.update_dict(ObjRef::new(2, 0), |d| {
                d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(second)]));
                d.set(b"Count".to_vec(), Object::Int(2));
                if inherited {
                    d.set(b"Resources".to_vec(), Object::Ref(resources));
                }
            })
            .unwrap();
            if hidden {
                sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenText]).unwrap();
            } else {
                mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
                apply(&mut doc, Some(&[0])).unwrap();
            }
            assert_no_secret(&doc, b"LEAK_");
            assert_no_secret(&doc, b"SECRET");
            let mut saved = reopen(&doc);
            for page_index in 0..2 {
                assert_eq!(under(&mut saved, page_index, &[[0.0, 90.0, 100.0, 110.0]]), 6);
                let page = pdfcraft_model::pages(&saved).swap_remove(page_index);
                let resources = saved.resolve(page.dict.get(b"Resources").unwrap());
                let properties = saved.resolve(resources.as_dict().unwrap().get(b"Properties").unwrap());
                assert!(!properties.as_dict().unwrap().contains(b"P0"));
                let property = saved.resolve(properties.as_dict().unwrap().get(b"Public").unwrap());
                for key in [&b"ActualText"[..], b"Alt", b"E", b"T"] {
                    assert!(property.as_dict().unwrap().contains(key), "shared form lost {key:?}: hidden={hidden}, inherited={inherited}");
                }
            }
        }
    }
}

mod descriptions;
mod inline_images;
mod limits;
mod property_retirement;
mod surviving_references;
mod text_state;

/// `[(FIRSTARR)] [(LASTARR)] TJ` is malformed (a surplus array operand), but the operator still
/// draws only the one immediately before it (LASTARR, 7 glyphs, 35 pt wide from x = 10) -- not
/// the first (FIRSTARR, 8 glyphs), which is never drawn at all, as the renderer agrees (#3569).
#[test]
fn a_tj_surplus_arrays_undrawn_glyphs_are_not_seen_under_a_mark() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td [(FIRSTARR)] [(LASTARR)] TJ ET", "", vec![]);
    assert_eq!(under(&mut doc, 0, &[[45.0, 95.0, 50.0, 110.0]]), 0, "nothing is drawn past LASTARR's 35pt width");
    assert_eq!(under(&mut doc, 0, &[[10.0, 95.0, 45.0, 110.0]]), 7, "LASTARR's 7 glyphs, the one the page draws");
}

#[test]
fn form_entries_share_one_budget_per_page() {
    // 20 draws of a form that draws the next 20 times, four levels down: 160,000 glyphs from a
    // few hundred bytes.
    let draws = "/X Do ".repeat(20);
    let mut objs = vec![stream("/Type /XObject /Subtype /Form /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 10 100 Td (A) Tj ET")];
    for inner in 7..10 {
        objs.push(stream(&format!("/Type /XObject /Subtype /Form /Resources << /XObject << /X {inner} 0 R >> >>"), draws.as_bytes()));
    }
    let mut doc = one_page(draws.as_bytes(), "/XObject << /X 10 0 R >>", objs);
    let residue = under(&mut doc, 0, &[[0.0, 90.0, 50.0, 120.0]]);
    assert!(residue > 0 && residue <= pdfcraft_cos::MAX_PAGE_FORMS, "{residue} glyphs counted");
    // Applying refuses the page rather than leave forms past the budget unread.
    mark(&mut doc, 0, &[[0.0, 90.0, 50.0, 120.0]], "");
    assert!(apply(&mut doc, None).is_err());
}

/// A name tree whose levels each list the next level twice: a walk that follows every path
/// visits 2^levels nodes.
#[test]
fn a_name_tree_whose_nodes_share_kids_counts_each_entry_once() {
    use crate::sanitize::{Hidden, scan};
    let levels = 12;
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 5 0 R >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream("", b"BT ET"),
    ];
    for k in 0..levels {
        let next = 6 + k;
        objs.push(format!("<< /Kids [{next} 0 R {next} 0 R] >>").into_bytes());
    }
    objs.push(b"<< /Names [(a.txt) << /Type /Filespec /F (a.txt) >>] >>".to_vec());
    let doc = pdf(objs);
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::Attachments], 1, "one entry, reached through 2^{levels} paths");
}

/// Page content that stops decoding at its start (`z` inside an ASCII85 group) while the rest of
/// the stream draws hidden text. A tolerant decode reads nothing, counts nothing and keeps the
/// stream; sanitizing must refuse instead.
#[test]
fn hidden_text_in_content_that_stops_decoding_is_refused() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize};
    let encoded: &[u8] = br"!!z6<#'\7PQ#?0Ha>,+>Y->EZd(j+>PW)+B2ko-rWQ%;FOP6+B3(u78s~>";
    let fresh = || {
        let mut doc = one_page(b"", "", vec![stream("/Filter /ASCII85Decode", encoded)]);
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Contents".to_vec(), Object::Ref(ObjRef::new(7, 0)))).unwrap();
        doc
    };
    let mut doc = fresh();
    let out = remove_hidden(&mut doc, &[Hidden::HiddenText]);
    assert!(matches!(out, Err(RedactError::Unreadable(1))), "{out:?}");
    let mut doc = fresh();
    let out = sanitize(&mut doc);
    assert!(matches!(out, Err(RedactError::Unreadable(1))), "{out:?}");
}

/// An array of references to objects `nums` (generation 0).
fn ref_array(nums: &[u32]) -> Object {
    Object::Array(nums.iter().map(|&n| Object::Ref(ObjRef::new(n, 0))).collect())
}

/// An interactive form dictionary listing fields `nums`.
fn fields_dict(nums: &[u32]) -> Object {
    let mut d = Dict::new();
    d.set(b"Fields".to_vec(), ref_array(nums));
    Object::Dict(d)
}

/// Flattening keeps hidden widgets (they draw nothing); removing form fields must still take them
/// and their values, and their parent field's value, out of the file.
#[test]
fn removing_form_fields_takes_hidden_widgets_and_their_values_out_of_the_file() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = one_page(
        b"BT /F1 10 Tf 10 100 Td (Visible) Tj ET",
        "",
        vec![
            // 7 a hidden terminal field merged with its widget, 8 a parent field, 9 its hidden widget
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (own) /V (LEAK_WIDGET_VALUE) /F 2 /Rect [10 200 150 220] /AP << /N 10 0 R >> >>".to_vec(),
            b"<< /FT /Tx /T (parent) /V (LEAK_PARENT_VALUE) /Kids [9 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Parent 8 0 R /F 32 /Rect [10 230 150 250] /AP << /N 10 0 R >> >>".to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 140 20]", b"0 g"),
        ],
    );
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"AcroForm".to_vec(), fields_dict(&[7, 8]))).unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), ref_array(&[7, 9]))).unwrap();
    remove_hidden(&mut doc, &[Hidden::FormFields]).unwrap();
    assert_no_secret(&doc, b"LEAK_");
    assert!(annots_of(&doc, &pdfcraft_model::pages(&doc).swap_remove(0).dict).is_empty());
}

/// F-021: removing form fields on a document with an AcroForm dictionary (even one with no
/// enumerable terminal fields) removes the AcroForm dictionary, processes widgets, and requires full save.
#[test]
fn removing_form_fields_removes_acroform_even_without_terminal_fields() {
    use crate::sanitize::{Hidden, remove_hidden, scan};
    let mut doc =
        one_page(b"BT /F1 10 Tf 10 100 Td (Visible) Tj ET", "", vec![b"<< /Type /Annot /Subtype /Widget /Rect [10 200 150 220] >>".to_vec()]);
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"AcroForm".to_vec(), fields_dict(&[]))).unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), ref_array(&[7]))).unwrap();
    let counts = scan(&doc);
    assert_eq!(counts.iter().find(|c| c.0 == Hidden::FormFields).map(|c| c.1), Some(1));
    assert!(doc.get(doc.root().unwrap()).as_dict().is_some_and(|d| d.contains(b"AcroForm")));
    assert!(!doc.full_save_required());

    let done = remove_hidden(&mut doc, &[Hidden::FormFields]).unwrap();
    assert_eq!(done, [(Hidden::FormFields, 1)]);
    assert!(!doc.get(doc.root().unwrap()).as_dict().is_some_and(|d| d.contains(b"AcroForm")));
    assert!(annots_of(&doc, &pdfcraft_model::pages(&doc).swap_remove(0).dict).is_empty());
    assert!(doc.full_save_required());
}

/// Removing hidden text cleans the structure tree as redaction does: the elements describing the
/// removed text lose their descriptions, and a reference into a rewritten Form points at the copy.
#[test]
fn removing_hidden_text_cleans_the_structure_tree() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = one_page(
        b"/P <</MCID 0>> BDC BT /F1 10 Tf 3 Tr 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 0 Tr 10 100 Td (Public) Tj ET EMC /Fm Do",
        "/XObject << /Fm 7 0 R >>",
        vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>",
                b"/P <</MCID 0>> BDC BT /F1 10 Tf 3 Tr 10 150 Td (FORMSECRET) Tj ET EMC",
            ),
            b"<< /Type /StructTreeRoot /K 9 0 R >>".to_vec(),
            b"<< /S /Document /P 8 0 R /K [10 0 R 11 0 R 12 0 R] >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (LEAK_PAGE_TAG) /K 0 >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /ActualText (Public) /K 1 >>".to_vec(),
            b"<< /S /P /P 9 0 R /Pg 3 0 R /Alt (LEAK_FORM_TAG) /K << /Type /MCR /Pg 3 0 R /Stm 7 0 R /MCID 0 >> >>".to_vec(),
        ],
    );
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"StructTreeRoot".to_vec(), Object::Ref(ObjRef::new(8, 0)))).unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"StructParents".to_vec(), Object::Int(0))).unwrap();
    remove_hidden(&mut doc, &[Hidden::HiddenText]).unwrap();
    assert_no_secret(&doc, b"LEAK_");
    assert_no_secret(&doc, b"SECRET");
    assert!(doc.get(ObjRef::new(11, 0)).as_dict().unwrap().contains(b"ActualText"), "untouched content keeps its tags");
}

/// One page drawing image /Im (object 7) and form /Fm (object 8), with `extra` objects from 9 on.
fn owners_fixture(image: &str, form: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs = vec![
        stream(&format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 {image}"), b"\x80"),
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 10 10] {form}"), b"0 0 5 5 re f"),
    ];
    objs.extend(extra);
    one_page(b"BT /F1 10 Tf 10 100 Td (Visible) Tj ET q 10 0 0 10 0 0 cm /Im Do Q /Fm Do", "/XObject << /Im 7 0 R /Fm 8 0 R >>", objs)
}

/// Removing one category and saving leaves none of `secret` in the file, and the scan counted it.
fn removes_from_every_owner(mut doc: Document, h: crate::sanitize::Hidden, secret: &[u8]) {
    use crate::sanitize::{remove_hidden, scan};
    let counts: std::collections::HashMap<_, usize> = scan(&doc).into_iter().collect();
    assert!(counts[&h] > 0, "{h:?} is counted");
    remove_hidden(&mut doc, &[h]).unwrap();
    assert_no_secret(&doc, secret);
    assert!(scan(&reopen(&doc)).into_iter().all(|(k, n)| k != h || n == 0 || h == crate::sanitize::Hidden::Metadata));
}

#[test]
fn private_data_of_a_form_xobject_is_removed() {
    let doc = owners_fixture("", "/PieceInfo << /App << /Private (LEAK_PRIVATE) >> >>", vec![]);
    removes_from_every_owner(doc, crate::sanitize::Hidden::PrivateData, b"LEAK_");
}

#[test]
fn metadata_of_an_image_is_removed() {
    let doc = owners_fixture("/Metadata 9 0 R", "", vec![stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta>LEAK_XMP</x:xmpmeta>")]);
    removes_from_every_owner(doc, crate::sanitize::Hidden::Metadata, b"LEAK_");
}

#[test]
fn a_pages_associated_files_are_removed_with_attachments() {
    let mut doc = owners_fixture(
        "",
        "",
        vec![b"<< /Type /Filespec /F (a.txt) /AFRelationship /Data /EF << /F 10 0 R >> >>".to_vec(), stream("/Type /EmbeddedFile", b"LEAK_ATTACHED")],
    );
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"AF".to_vec(), ref_array(&[9]))).unwrap();
    removes_from_every_owner(doc, crate::sanitize::Hidden::Attachments, b"LEAK_");
}

#[test]
fn a_fields_own_actions_are_removed_with_links_actions_and_scripts() {
    let mut doc = owners_fixture(
        "",
        "",
        vec![
            b"<< /FT /Tx /T (total) /AA << /C << /S /JavaScript /JS (LEAK_SCRIPT) >> >> /Kids [10 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Parent 9 0 R /Rect [10 200 150 220] >>".to_vec(),
        ],
    );
    doc.update_dict(doc.root().unwrap(), |d| d.set(b"AcroForm".to_vec(), fields_dict(&[9]))).unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Annots".to_vec(), ref_array(&[10]))).unwrap();
    removes_from_every_owner(doc, crate::sanitize::Hidden::LinksActionsScripts, b"LEAK_");
}

/// Resource names are chosen by the producer: a font named like an owned entry stays.
#[test]
fn a_resource_named_like_an_owned_entry_is_kept() {
    let mut doc = one_page(
        b"BT /AF 10 Tf 10 100 Td (Visible) Tj ET",
        "",
        vec![b"<< /Type /Filespec /F (a.txt) /EF << /F 8 0 R >> >>".to_vec(), stream("/Type /EmbeddedFile", b"LEAK_ATTACHED")],
    );
    doc.update_dict(ObjRef::new(3, 0), |d| {
        let mut fonts = Dict::new();
        fonts.set(b"AF".to_vec(), Object::Ref(ObjRef::new(5, 0)));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        d.set(b"Resources".to_vec(), Object::Dict(res));
        d.set(b"AF".to_vec(), ref_array(&[7]));
    })
    .unwrap();
    removes_from_every_owner(doc.clone(), crate::sanitize::Hidden::Attachments, b"LEAK_");
    crate::sanitize::remove_hidden(&mut doc, &[crate::sanitize::Hidden::Attachments]).unwrap();
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    assert!(res.get(b"Font").and_then(|f| f.as_dict()).is_some_and(|f| f.contains(b"AF")));
    assert!(!page.dict.contains(b"AF"));
}

/// Owners written in place deeper than the sweep follows are refused, not skipped.
#[test]
fn owners_nested_too_deeply_are_refused() {
    use crate::sanitize::{Hidden, remove_hidden};
    let nested = format!("{}<< /AA << /O << /S /JavaScript /JS (LEAK_DEEP) >> >> >>{}", "[".repeat(100), "]".repeat(100));
    let mut doc = one_page(b"BT ET", "", vec![format!("<< /Deep {nested} >>").into_bytes()]);
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"PieceInfo".to_vec(), Object::Ref(ObjRef::new(7, 0)))).unwrap();
    let out = remove_hidden(&mut doc, &[Hidden::LinksActionsScripts]);
    assert!(matches!(out, Err(RedactError::TooDeep)), "{out:?}");
}

/// A name tree node that lists the same kid more times than the node budget: each repeat costs a
/// step, so the walk stops at the budget instead of scanning every repeat, and the distinct kid
/// listed after them is never reached.
#[test]
fn repeated_name_tree_kids_count_towards_the_node_budget() {
    use crate::sanitize::{Hidden, scan};
    let repeats = "6 0 R ".repeat(100_010);
    let doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles 5 0 R >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>".to_vec(),
        stream("", b"BT ET"),
        format!("<< /Kids [{repeats}7 0 R] >>").into_bytes(),
        b"<< /Names [(a.txt) << /Type /Filespec /F (a.txt) >>] >>".to_vec(),
        b"<< /Names [(b.txt) << /Type /Filespec /F (b.txt) >>] >>".to_vec(),
    ]);
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::Attachments], 1, "the walk went past its node budget to the last kid");
}

/// Content streams and the forms they draw spend one page allowance: 130 MiB of content and a
/// 130 MiB form are each under it, together past it.
#[test]
fn content_and_forms_share_one_budget_per_page() {
    use pdfcraft_cos::{Dict, Stream};
    let mut doc = one_page(b"", "", vec![]);
    let mut content = b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET /X Do\n%".to_vec();
    content.resize(130 << 20, b'x');
    let content = doc.add(Object::Stream(Stream::flate(Dict::new(), &content)));
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    let mut body = b"%".to_vec();
    body.resize(130 << 20, b'x');
    let form = doc.add(Object::Stream(Stream::flate(form, &body)));
    doc.update_dict(ObjRef::new(3, 0), |d| {
        let mut xobjects = Dict::new();
        xobjects.set(b"X".to_vec(), Object::Ref(form));
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Ref(ObjRef::new(5, 0)));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        d.set(b"Resources".to_vec(), Object::Dict(res));
        d.set(b"Contents".to_vec(), Object::Ref(content));
    })
    .unwrap();
    mark(&mut doc, 0, &[[0.0, 90.0, 50.0, 120.0]], "");
    assert!(apply(&mut doc, None).is_err(), "the form past the page's allowance was read");
}

/// A page that fails to read after another page was already redacted leaves the whole document as
/// it was, the shared resources the first page's rewrite localised included.
#[test]
fn a_failure_on_a_later_page_leaves_the_document_unchanged() {
    let mut doc = two_pages_one_resource_dictionary(true, false);
    let all = pdfcraft_model::pages(&doc);
    let (first, second, parent) = (all[0].obj, all[1].obj, doc.get(all[0].obj).as_dict().unwrap().reference(b"Parent").unwrap());
    let mut dct = Dict::new();
    dct.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    let bad = doc.add(Object::Stream(Stream::from_raw(dct, b"garbage".to_vec())));
    doc.update_dict(second, |d| d.set(b"Contents".to_vec(), Object::Ref(bad))).unwrap();
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
    mark(&mut doc, 1, &[[0.0, 0.0, 50.0, 50.0]], "");
    let before: Vec<Object> = [first, second, parent].iter().map(|r| (*doc.get(*r)).clone()).collect();
    let text = content(&doc, 0);
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(2)));
    let after: Vec<Object> = [first, second, parent].iter().map(|r| (*doc.get(*r)).clone()).collect();
    assert_eq!(after, before);
    assert_eq!(content(&doc, 0), text);
}

#[test]
fn a_reply_and_pop_up_on_another_page_go_with_the_removed_comment() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Annots [6 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Annots [7 0 R 8 0 R] >>".to_vec(),
        stream("", b"BT ET"),
        b"<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (PARENT-NOTE) /Popup 8 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 250 270 270] /IRT 6 0 R /Contents (OTHER-PAGE-REPLY) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [200 150 290 190] /Parent 6 0 R >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 50.0]], "");
    apply(&mut doc, None).unwrap();
    let second = pdfcraft_model::pages(&doc).swap_remove(1);
    assert!(annots_of(&doc, &second.dict).is_empty(), "the reply and the pop-up leave page 2");
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(!bytes.windows(10).any(|w| w == b"PARENT-NOT"), "the removed comment survived the save");
}

#[test]
fn a_redact_mark_written_in_place_is_refused() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Annots [<< /Type /Annot /Subtype /Redact /Rect [10 10 70 40] >>] >>"
            .to_vec(),
        stream("", b"BT ET"),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[100.0, 100.0, 120.0, 120.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::UnreadableMark(1))));
}

#[test]
fn do_acts_on_the_operand_before_the_operator_like_the_renderer() {
    // `/Missing /Im Do`: the renderer draws /Im, so redaction must find the image there.
    let img = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"\x00");
    let mut doc = one_page(b"q 100 0 0 100 10 10 cm /Missing /Im Do Q", "/XObject << /Im 7 0 R >>", vec![img]);
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.images_removed, 1, "{r:?}");
}

#[test]
fn hidden_text_removal_drops_the_source_text_of_added_content() {
    let mut doc = one_page(b"BT /F1 12 Tf 3 Tr 20 20 Td (SECRET) Tj ET", "", vec![]);
    let content_ref = pdfcraft_model::pages(&doc)[0].dict.get(b"Contents").and_then(Object::as_ref).unwrap();
    doc.update_dict(content_ref, |d| {
        d.set(b"PCAdded".to_vec(), Object::Dict(Dict::new()));
    })
    .unwrap();
    sanitize::remove_hidden(&mut doc, &[sanitize::Hidden::HiddenText]).unwrap();
    let p = &pdfcraft_model::pages(&doc)[0];
    let (list, _) = page_streams(&doc, &p.dict, 0).unwrap();
    assert!(list.iter().all(|o| doc.resolve(o).as_dict().is_some_and(|d| !d.contains(b"PCAdded"))));
}

#[test]
fn a_reply_on_another_page_to_a_removed_popup_goes_too() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Annots [6 0 R 8 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Annots [7 0 R] >>".to_vec(),
        stream("", b"BT ET"),
        b"<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (PARENT-NOTE) /Popup 8 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Rect [250 250 270 270] /IRT 8 0 R /Contents (POPUP-REPLY) >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [200 150 290 190] /Parent 6 0 R /Contents (POPUP-TEXT) >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[10.0, 10.0, 70.0, 50.0]], "");
    apply(&mut doc, None).unwrap();
    let second = pdfcraft_model::pages(&doc).swap_remove(1);
    assert!(annots_of(&doc, &second.dict).is_empty());
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    for gone in [&b"POPUP-REPLY"[..], b"POPUP-TEXT", b"PARENT-NOTE"] {
        assert!(!bytes.windows(gone.len()).any(|w| w == gone), "{} survived", String::from_utf8_lossy(gone));
    }
}
