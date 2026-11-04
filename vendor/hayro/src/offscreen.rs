use crate::renderer::Renderer;
use crate::RenderCache;
use hayro_interpret::font::Glyph;
use hayro_interpret::{BlendMode, ClipPath, Device, GlyphDrawMode, Image, Paint, PathDrawMode, SoftMask};
use kurbo::{Affine, BezPath, Rect};
use vello_cpu::{Level, Pixmap, RenderMode, RenderSettings};

/// PdfCraft patch (ps-export): an offscreen `Renderer` sized by its caller (the
/// PostScript exporter's transparency flattener, which renders the flattened
/// region at region dimensions) driven through the `Device` trait it already
/// implements, so flattened regions composite exactly as `page_render` composites
/// them (same `Device` implementation, same tiling/shading/image/mask logic).
///
/// The device draws in pixel space, like the device `render_into` constructs: any
/// y-flip or page matrix is the caller's (the flattener's) job, baked into the
/// transforms it passes to the `Device` methods.
pub struct OffscreenRenderer {
    inner: Renderer,
}

impl OffscreenRenderer {
    /// Offscreen context of the caller's dimensions, `RenderCache::new()`, no colour
    /// replacement, not a preview, vello settings matching `render_into` (Level::new,
    /// 0 threads, OptimizeSpeed).
    pub fn new(width: u16, height: u16) -> Self {
        let cache = RenderCache::new();
        Self {
            inner: Renderer::new(
                width,
                height,
                RenderSettings {
                    level: Level::new(),
                    num_threads: 0,
                    render_mode: RenderMode::OptimizeSpeed,
                },
                &cache,
                None,
                false,
            ),
        }
    }

    /// Flush and render to a fresh pixmap (premultiplied RGBA8). Layers/clips must
    /// be balanced — the caller replays a balanced prefix and pops the remainder
    /// before calling this, or vello's rasterizer asserts on the open layers.
    ///
    /// The renderer stays usable afterwards: further draws accumulate on top and a
    /// later `finish` rasterizes them too (vello's `render_to_pixmap` does not
    /// consume the context).
    pub fn finish(&mut self) -> Pixmap {
        let width = self.inner.ctx.width();
        let height = self.inner.ctx.height();
        self.inner.ctx.flush();
        let mut pixmap = Pixmap::new(width, height);
        self.inner
            .ctx
            .render_to_pixmap(&mut vello_cpu::Resources::default(), &mut pixmap);
        pixmap
    }
}

/// Every method delegates to the inner [`Renderer`] unchanged, so a flattened region
/// composites exactly as `page_render` composites it. The named `'a` mirrors the
/// `Renderer`'s own impl: `Glyph` is invariant over its lifetime, so the glyph and its
/// paint must share one.
impl<'a> Device<'a> for OffscreenRenderer {
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'_>>) {
        self.inner.set_soft_mask(mask);
    }

    fn set_blend_mode(&mut self, blend_mode: BlendMode) {
        self.inner.set_blend_mode(blend_mode);
    }

    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        self.inner.draw_path(path, transform, paint, draw_mode);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        self.inner.push_clip_path(clip_path);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'_>>,
        blend_mode: BlendMode,
    ) {
        self.inner
            .push_transparency_group(opacity, mask, blend_mode);
    }

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        draw_mode: &GlyphDrawMode,
    ) {
        self.inner
            .draw_glyph(glyph, transform, glyph_transform, paint, draw_mode);
    }

    fn draw_image(&mut self, image: Image<'_, '_>, transform: Affine) {
        self.inner.draw_image(image, transform);
    }

    fn pop_clip_path(&mut self) {
        self.inner.pop_clip_path();
    }

    fn pop_transparency_group(&mut self) {
        self.inner.pop_transparency_group();
    }

    fn draw_rect(
        &mut self,
        rect: &Rect,
        transform: Affine,
        paint: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        self.inner.draw_rect(rect, transform, paint, draw_mode);
    }

    fn begin_marked_content(&mut self, tag: &[u8], mcid: Option<i32>) {
        self.inner.begin_marked_content(tag, mcid);
    }

    fn actual_text(&mut self, text: &[u8]) {
        self.inner.actual_text(text);
    }

    fn end_marked_content(&mut self) {
        self.inner.end_marked_content();
    }
}
