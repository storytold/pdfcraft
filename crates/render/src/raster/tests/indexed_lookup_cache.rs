//! An `/Indexed` colour space named by a resource is built once, and its lookup table read
//! once, however many inline images name it (vendored hayro-interpret patch 60). An earlier
//! version of the patch checked the table again before every inline image, decoding a lookup
//! stream once per image outside the colour-space cache.

use super::image_cache_pdf;

/// A page that draws `count` 2 × 1 inline images in the resource colour space `/P`, an Indexed
/// space over DeviceRGB with `hival` whose lookup is stream object 5 holding `table`.
fn page(count: usize, hival: u8, table: &[u8]) -> Vec<u8> {
    let image = "q 20 0 0 20 0 0 cm BI /W 2 /H 1 /BPC 8 /CS /P /F /AHx ID 0001> EI Q";
    let content = vec![image; count].join("\n");
    let resources = format!("<< /ColorSpace << /P [/Indexed /DeviceRGB {hival} 5 0 R] >> >>");
    let dict = format!("<< /Length {} >>", table.len());
    image_cache_pdf(&content, &resources, &[(&dict, table)])
}

fn lookup_reads(pdf: Vec<u8>) -> [usize; 2] {
    use hayro::hayro_interpret::font::Glyph;
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::{
        BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask,
        TransformExt, interpret_page,
    };
    use kurbo::{Affine, BezPath, Rect};
    use std::sync::Arc;

    /// Draws nothing: only the colour spaces the page reads matter here.
    struct Nothing;
    impl<'a> Device<'a> for Nothing {
        fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
        fn set_blend_mode(&mut self, _: BlendMode) {}
        fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
        fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {}
        fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
        fn pop_clip_path(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }

    let pdf = Pdf::new(Arc::new(pdf)).expect("parses");
    let page = &pdf.pages()[0];
    let cache = InterpreterCache::new();
    [(); 2].map(|()| {
        let initial = page.initial_transform(true).to_kurbo();
        let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, 130.0, 70.0), &cache, page.xref(), InterpreterSettings::default());
        interpret_page(page, &mut ctx, &mut Nothing);
        cache.indexed_lookup_reads()
    })
}

/// Twenty inline images naming one resource read its lookup stream once, in the first render
/// and none in a second render through the same cache: for a full table, and for a short one
/// that is padded (whose images an earlier version of the patch checked, and skipped, one by
/// one).
#[test]
fn inline_images_naming_one_indexed_resource_read_its_lookup_once() {
    let full = page(20, 1, b"\xFF\x00\x00\x00\xFF\x00");
    assert_eq!(lookup_reads(full), [1, 1], "a full table: read once, then cached");
    let short = page(20, 7, b"\xFF\x00\x00\x00\xFF\x00");
    assert_eq!(lookup_reads(short), [1, 1], "a short table: read once, then cached");
}
