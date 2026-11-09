//! A Type 0 font the document doesn't embed draws the characters its substitute has no glyph for
//! with the further faces `InterpreterSettings::further_faces` gives (vendored hayro-interpret
//! patch). Contributor-original synthetic fixtures; the further face is hayro's own Symbol
//! stand-in, which has ♠ (U+2660), a character no Latin standard font has.
use hayro::hayro_interpret::font::{FallbackFontQuery, Glyph, StandardFont};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use hayro::{RenderCache, RenderSettings, render};
use kurbo::{Affine, BezPath, Rect};
use std::sync::{Arc, Mutex};

/// Code `<0001>` is ♠, `<0002>` is 1 and `<0003>` is A, by `/ToUnicode`. Both the Latin
/// substitute and Symbol have a glyph for 1, drawn differently; Symbol has none for A.
const SPADE: &str = "0001";
const DIGIT_ONE: &str = "0002";
const LETTER_A: &str = "0003";

/// A page showing the codes `codes` (hex) in a Type 0 font that isn't embedded. Its name matches
/// no standard font and its collection is Adobe-Identity, so the substitute is a Latin standard
/// font chosen from the descriptor's flags.
fn page(codes: &str) -> Vec<u8> {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Test def /CMapType 2 def \
                1 begincodespacerange <0000> <FFFF> endcodespacerange \
                3 beginbfchar <0001> <2660> <0002> <0031> <0003> <0041> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let content = format!("BT /F1 40 Tf 10 20 Td <{codes}> Tj ET");
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 80] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", &content),
        "<< /Type /Font /Subtype /Type0 /BaseFont /CraftSubstitute-W3 /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 8 0 R >>".into(),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /CraftSubstitute-W3 \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 1000 >>"
            .into(),
        "<< /Type /FontDescriptor /FontName /CraftSubstitute-W3 /Flags 32 /FontBBox [0 -200 1000 900] \
         /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 >>"
            .into(),
        stream("", cmap),
    ])
}

/// The same page showing ♠ (code 0xAA) in the standard Symbol font itself.
fn symbol_page() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 80] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream("", "BT /F1 40 Tf 10 20 Td <AA> Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>".into(),
    ])
}

/// Settings whose further faces are hayro's Symbol stand-in; every call is counted in `calls`.
fn with_symbol(calls: Arc<Mutex<Vec<FallbackFontQuery>>>) -> InterpreterSettings {
    InterpreterSettings {
        further_faces: Arc::new(move |query| {
            calls.lock().unwrap().push(query.clone());
            vec![StandardFont::Symbol.get_font_data()]
        }),
        ..InterpreterSettings::default()
    }
}

/// The page's pixels, RGBA.
fn pixels(bytes: Vec<u8>, settings: &InterpreterSettings) -> Vec<u8> {
    let pdf = Pdf::new(bytes).unwrap();
    let pages = pdf.pages();
    let page = pages.first().unwrap();
    let cache = RenderCache::new();
    render(page, &cache, settings, &RenderSettings::default()).data_as_u8_slice().to_vec()
}

/// How many pixels hold ink (more than half opaque on a transparent page).
fn ink(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] >= 128).count()
}

#[test]
fn a_character_the_substitute_lacks_draws_in_a_further_face() {
    let reference = pixels(symbol_page(), &InterpreterSettings::default());
    let without = pixels(page(SPADE), &InterpreterSettings::default());
    let with = pixels(page(SPADE), &with_symbol(Arc::default()));
    assert!(ink(&reference) > 100, "Symbol's own ♠ is drawn");
    // Without further faces the CID is taken as a glyph index of the Latin substitute, which
    // draws an unrelated glyph.
    assert!(without != reference, "the Latin substitute has no ♠");
    assert!(with == reference, "♠ is drawn by the further face, as Symbol draws it ({} inked pixels)", ink(&with));
}

#[test]
fn a_character_the_substitute_has_keeps_the_substitutes_glyph() {
    let without = pixels(page(DIGIT_ONE), &InterpreterSettings::default());
    let with = pixels(page(DIGIT_ONE), &with_symbol(Arc::default()));
    assert!(ink(&without) > 100, "1 is drawn by the substitute");
    assert!(without == with, "the substitute's 1 wins over the further face's");
}

/// In one run, the substitute's glyph and the further face's glyph stay distinct (their ids
/// don't collide in the glyph cache), and a further face's glyph never reports the substitute's
/// bytes as its font data, since its id indexes another face.
#[test]
fn further_face_glyphs_stay_apart_from_the_substitutes() {
    let both = ink(&pixels(page(&format!("{DIGIT_ONE}{SPADE}{DIGIT_ONE}{SPADE}")), &with_symbol(Arc::default())));
    let letter = ink(&pixels(page(DIGIT_ONE), &with_symbol(Arc::default())));
    let spade = ink(&pixels(page(SPADE), &with_symbol(Arc::default())));
    assert_eq!(both, 2 * (letter + spade), "two 1s and two ♠s, each drawn by its own face");

    let glyphs: Vec<(String, bool)> =
        drawn_glyphs(page(&format!("{DIGIT_ONE}{SPADE}")), with_symbol(Arc::default())).into_iter().map(|(text, data, _)| (text, data)).collect();
    assert_eq!(glyphs, vec![("1".to_string(), true), ("♠".to_string(), false)], "(text, has font data) per glyph");
}

/// A callback may rank every face it has, the substitute included: a further face whose data is
/// the substitute's own is skipped (it was tried first), so it isn't parsed a second time. Here
/// the substitute is Symbol and the further faces are Symbol again, then Helvetica: A, which
/// Symbol lacks, is drawn by the first further face left, Helvetica, at position 0.
#[test]
fn the_substitute_itself_is_no_further_face() {
    let settings = InterpreterSettings {
        font_resolver: Arc::new(|_| Some(StandardFont::Symbol.get_font_data())),
        further_faces: Arc::new(|_| vec![StandardFont::Symbol.get_font_data(), StandardFont::Helvetica.get_font_data()]),
        ..InterpreterSettings::default()
    };
    let glyphs = drawn_glyphs(page(LETTER_A), settings);
    let [(text, _, id)] = glyphs.as_slice() else { panic!("one glyph: {glyphs:?}") };
    assert_eq!(text, "A");
    assert_eq!(id >> 16, 1, "drawn by the further face at position 0 (its id carries position + 1): {id:#x}");
}

#[test]
fn further_faces_are_asked_for_once_per_substituted_font() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    pixels(page(&format!("{SPADE}{DIGIT_ONE}{SPADE}")), &with_symbol(calls.clone()));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "one Type 0 font, one request: {calls:?}");
    assert_eq!(calls.first().and_then(|q| q.post_script_name.as_deref()), Some("CraftSubstitute-W3"));
}

/// Every glyph the page draws: its text, whether it reports font data, and its glyph id.
fn drawn_glyphs(bytes: Vec<u8>, settings: InterpreterSettings) -> Vec<(String, bool, u32)> {
    #[derive(Default)]
    struct Glyphs(Vec<(String, bool, u32)>);
    impl<'a> Device<'a> for Glyphs {
        fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
        fn set_blend_mode(&mut self, _: BlendMode) {}
        fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
        fn draw_glyph(&mut self, glyph: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
            if let Glyph::Outline(g) = glyph {
                let text = match g.as_unicode() {
                    Some(BfString::Char(c)) => c.to_string(),
                    Some(BfString::String(s)) => s,
                    None => String::new(),
                };
                self.0.push((text, g.font_data().is_some(), g.glyph_id().to_u32()));
            }
        }
        fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
        fn pop_clip_path(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }
    let pdf = Pdf::new(bytes).unwrap();
    let page = &pdf.pages()[0];
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, 200.0, 80.0), &cache, page.xref(), settings);
    let mut device = Glyphs::default();
    interpret_page(page, &mut ctx, &mut device);
    device.0
}

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn stream(dict: &str, body: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len())
}
