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
use kurbo::{Affine, BezPath, Rect};
use std::io::Write;
use std::sync::Arc;

/// Counts the paths drawn, and draws each Type 3 glyph through its procedure, as the renderer does.
#[derive(Default)]
struct Paths(usize);

impl<'a> Device<'a> for Paths {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        self.0 += 1;
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

/// A Type 3 font whose one glyph, `!`, is a Flate-compressed procedure filling its 8 × 8 box,
/// shown 200 times on one page and rendered twice through one cache: every copy draws, and the
/// procedure is decoded once, on the first copy.
#[test]
fn a_type3_glyph_shown_again_decodes_its_procedure_once() {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(b"8 0 0 0 8 8 d1 0 0 8 8 re f").unwrap();
    let proc_bytes = z.finish().unwrap();
    let proc_dict = format!("<< /Filter /FlateDecode /Length {} >>", proc_bytes.len());
    let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 8 8] /FontMatrix [0.125 0 0 0.125 0 0] \
                /CharProcs << /dot 5 0 R >> /Encoding << /Type /Encoding /Differences [33 /dot] >> \
                /FirstChar 33 /LastChar 33 /Widths [8] >>";
    let content = format!("BT /F1 0.5 Tf 1 0 0 1 2 30 Tm ({}) Tj ET", "!".repeat(200));
    let bytes = image_cache_pdf(&content, &format!("<< /Font << /F1 {font} >> >>"), &[(&proc_dict, &proc_bytes)]);
    let parsed = Pdf::new(Arc::new(bytes)).expect("parses");
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    for render in ["first", "second"] {
        let initial = page.initial_transform(true).to_kurbo();
        let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = Paths::default();
        interpret_page(page, &mut ctx, &mut device);
        assert_eq!(device.0, 200, "the {render} render draws every copy of the glyph");
        assert_eq!(cache.type3_proc_decodes(), 1, "after the {render} render: the procedure is decoded once");
    }
}
