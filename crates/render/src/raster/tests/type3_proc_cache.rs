//! A Type 3 glyph shown many times decodes its glyph procedure once (vendored hayro-interpret
//! patch 87). hayro inflated the procedure again for every copy of the glyph, so a font used as
//! a fill pattern, one glyph drawn hundreds of thousands of times, took seconds a page.

use super::image_cache_pdf;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use kurbo::{Affine, BezPath, Rect, Shape};
use std::io::Write;
use std::sync::Arc;

/// Records the width in device pixels of each path drawn, and draws each Type 3 glyph through its
/// procedure, as the renderer does.
#[derive(Default)]
struct Paths(Vec<f64>);

impl<'a> Device<'a> for Paths {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, path: &BezPath, transform: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        self.0.push((transform * path.clone()).bounding_box().width());
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, _: &GlyphDrawMode) {
        if let Glyph::Type3(glyph) = glyph {
            glyph.interpret(self, transform, glyph_transform, paint);
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

fn flate(data: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(data).unwrap();
    z.finish().unwrap()
}

/// A Type 3 font on an 8 × 8 grid whose glyphs, named `names`, take the codes from 33 on and
/// have their procedures in the objects from `first_object` on.
fn font(names: &[String], first_object: usize) -> String {
    let procs: String = names.iter().enumerate().map(|(i, n)| format!("/{n} {} 0 R ", first_object + i)).collect();
    let names_list: String = names.iter().map(|n| format!("/{n} ")).collect();
    format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 8 8] /FontMatrix [0.125 0 0 0.125 0 0] /CharProcs << {procs}>> \
         /Encoding << /Type /Encoding /Differences [33 {names_list}] >> /FirstChar 33 /LastChar {} /Widths [{}] >>",
        32 + names.len(),
        "8 ".repeat(names.len())
    )
}

/// Renders the page of `bytes` through a new cache; the widths of the paths drawn, and how many
/// glyph procedures were decoded.
fn render(bytes: Vec<u8>) -> (Vec<f64>, usize) {
    let parsed = Pdf::new(Arc::new(bytes)).expect("parses");
    let page = &parsed.pages()[0];
    let initial = page.initial_transform(true).to_kurbo();
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), InterpreterSettings::default());
    let mut device = Paths::default();
    interpret_page(page, &mut ctx, &mut device);
    (device.0, cache.type3_proc_decodes())
}

/// A Type 3 font whose one glyph, `!`, is a Flate-compressed procedure filling its 8 × 8 box,
/// shown 200 times on one page and rendered twice through one cache: every copy draws, and the
/// procedure is decoded once, on the first copy.
#[test]
fn a_type3_glyph_shown_again_decodes_its_procedure_once() {
    let proc_bytes = flate(b"8 0 0 0 8 8 d1 0 0 8 8 re f");
    let proc_dict = format!("<< /Filter /FlateDecode /Length {} >>", proc_bytes.len());
    let content = format!("BT /F1 0.5 Tf 1 0 0 1 2 30 Tm ({}) Tj ET", "!".repeat(200));
    let resources = format!("<< /Font << /F1 {} >> >>", font(&["dot".into()], 5));
    let parsed = Pdf::new(Arc::new(image_cache_pdf(&content, &resources, &[(&proc_dict, &proc_bytes)]))).expect("parses");
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    for render in ["first", "second"] {
        let initial = page.initial_transform(true).to_kurbo();
        let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = Paths::default();
        interpret_page(page, &mut ctx, &mut device);
        assert_eq!(device.0.len(), 200, "the {render} render draws every copy of the glyph");
        assert_eq!(cache.type3_proc_decodes(), 1, "after the {render} render: the procedure is decoded once");
    }
}

/// A procedure that decodes to more than 64 KiB isn't kept: shown three times, it is decoded three
/// times, and draws each time.
#[test]
fn a_procedure_over_64_kib_is_decoded_for_each_copy() {
    let mut program = b"8 0 0 0 8 8 d1 0 0 8 8 re f".to_vec();
    program.resize((64 << 10) + 1, b' ');
    let proc_bytes = flate(&program);
    let proc_dict = format!("<< /Filter /FlateDecode /Length {} >>", proc_bytes.len());
    let resources = format!("<< /Font << /F1 {} >> >>", font(&["big".into()], 5));
    let bytes = image_cache_pdf("BT /F1 0.5 Tf 1 0 0 1 2 30 Tm (!!!) Tj ET", &resources, &[(&proc_dict, &proc_bytes)]);
    let (widths, decodes) = render(bytes);
    assert_eq!(widths.len(), 3);
    assert_eq!(decodes, 3, "a procedure over the size kept is decoded for each copy");
}

/// The fonts of one cache keep at most 32 MiB of procedures together: 600 glyphs of 60,000 bytes
/// each, over three fonts (36 MB), each shown twice, all draw; the procedures kept are decoded
/// once and the rest twice, so there are more decodes than glyphs and fewer than copies.
#[test]
fn the_procedures_one_cache_keeps_stop_at_32_mib() {
    let mut program = b"8 0 0 0 8 8 d1 0 0 8 8 re f".to_vec();
    program.resize(60_000, b' ');
    let proc_bytes = flate(&program);
    let proc_dict = format!("<< /Filter /FlateDecode /Length {} >>", proc_bytes.len());
    let (fonts, per_font) = (3, 200);
    let mut resources = String::from("<< /Font << ");
    let mut content = String::new();
    for f in 0..fonts {
        let names: Vec<String> = (0..per_font).map(|g| format!("g{f}x{g}")).collect();
        resources.push_str(&format!("/F{f} {} ", font(&names, 5 + f * per_font)));
        let codes: String = (0..per_font).map(|g| format!("{:02x}", 33 + g)).collect();
        content.push_str(&format!("BT /F{f} 0.05 Tf 1 0 0 1 2 {} Tm <{codes}{codes}> Tj ET ", 10 + 20 * f));
    }
    resources.push_str(">> >>");
    let objects: Vec<(&str, &[u8])> = (0..fonts * per_font).map(|_| (proc_dict.as_str(), proc_bytes.as_slice())).collect();
    let bytes = image_cache_pdf(&content, &resources, &objects);
    let (widths, decodes) = render(bytes);
    assert_eq!(widths.len(), 2 * fonts * per_font, "every copy draws");
    assert!(decodes > fonts * per_font && decodes < 2 * fonts * per_font, "{decodes} decodes: some procedures kept, not all");
}

/// Two fonts each with a glyph named `dot`, one filling 8 × 8 and the other 4 × 4, draw their own
/// procedures: what one font keeps isn't looked up by another with the same glyph name.
#[test]
fn fonts_with_the_same_glyph_name_keep_their_own_procedures() {
    let (big, small) = (b"8 0 0 0 8 8 d1 0 0 8 8 re f".to_vec(), b"8 0 0 0 8 8 d1 0 0 4 4 re f".to_vec());
    let dict = |data: &[u8]| format!("<< /Length {} >>", data.len());
    let (big_dict, small_dict) = (dict(&big), dict(&small));
    let resources = format!("<< /Font << /A {} /B {} >> >>", font(&["dot".into()], 5), font(&["dot".into()], 6));
    let bytes = image_cache_pdf(
        "BT /A 8 Tf 1 0 0 1 2 30 Tm (!!) Tj /B 8 Tf (!!) Tj ET",
        &resources,
        &[(big_dict.as_str(), big.as_slice()), (small_dict.as_str(), small.as_slice())],
    );
    let (widths, _) = render(bytes);
    assert_eq!(widths.len(), 4);
    let (a, b) = (widths[0], widths[3]);
    assert!((a - 2.0 * b).abs() < 0.01, "font A's glyph is twice as wide as font B's: {widths:?}");
    assert_eq!((widths[1], widths[2]), (a, b), "each font draws its own glyph again: {widths:?}");
}
