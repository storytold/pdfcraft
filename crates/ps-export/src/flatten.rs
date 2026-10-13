//! Transparency flattening for the PostScript exporter (Wave 2).
//!
//! PDF transparency — blend modes, soft masks, constant alpha — has no vector PostScript
//! equivalent, so regions that use it are rasterized: the page's draws are recorded, the
//! transparent region is composited offscreen through hayro's own `Device` implementation
//! ([`hayro::OffscreenRenderer`], added by this patch series) and emitted as a PostScript
//! image. Task 7 ships the vendor patch this module will drive; the flattening scene
//! itself lands with Task 8.

#[cfg(test)]
mod tests {
    use hayro::OffscreenRenderer;
    use hayro::hayro_interpret::color::{AlphaColor, Color};
    use hayro::hayro_interpret::font::Glyph;
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::{
        BlendMode, ClipPath, Context, Device, FillRule, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask,
        interpret_page,
    };
    use kurbo::{Affine, BezPath, Rect};

    /// A `<< … >> stream … endstream` object body, in the hand-written style of
    /// `crates/ps-export/src/lib.rs`'s tests (length computed, not counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// Assemble a PDF from object bodies (numbered from 1), computing the xref like
    /// `crates/ps-export/src/lib.rs`'s tests.
    fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A one-page PDF drawing two image XObjects: a 2×2 `/DeviceRGB` raster and a 2×2
    /// `/ImageMask` stencil (the replay log must capture both `Image` variants by value).
    fn image_pdf() -> Vec<u8> {
        // Two rows of one byte each: a 2-wide row uses the top 2 bits. Samples 0,0,1,1:
        // under `/Decode [0 1]` the zeros paint, so the left column of each row is ink.
        let mask: [u8; 2] = [0x30, 0x30];
        let raster: [u8; 12] = [
            255, 0, 0, 255, 0, 0, //
            0, 255, 0, 0, 255, 0,
        ];
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 50] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R /Msk0 6 0 R >> >> >>".to_vec(),
            stream_obj("", b"q 10 0 0 10 10 10 cm /Im0 Do Q\nq 10 0 0 10 50 10 cm /Msk0 Do Q"),
            stream_obj("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8", &raster),
            stream_obj("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ImageMask true /BitsPerComponent 1 /Decode [0 1]", &mask),
        ])
    }

    /// A `DummyDevice` that proves every `draw_image` argument can be captured by value:
    /// each received `Image` is cloned in place (the compile-time assertion this test
    /// exists for) and the clone's dimensions are compared with the original's.
    #[derive(Default)]
    struct CloningDevice {
        /// `(variant, original width, cloned width)` per received image.
        images: Vec<(&'static str, u32, u32)>,
    }

    impl Device<'_> for CloningDevice {
        fn set_soft_mask(&mut self, _: Option<SoftMask<'_>>) {}
        fn set_blend_mode(&mut self, _: BlendMode) {}
        fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'_>, _: &PathDrawMode) {}
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}
        fn draw_glyph(&mut self, _: &Glyph<'_>, _: Affine, _: Affine, _: &Paint<'_>, _: &GlyphDrawMode) {}
        fn draw_image(&mut self, image: Image<'_, '_>, _: Affine) {
            // The replay log captures images by value: `Image` must be `Clone`.
            let cloned = image.clone();
            let variant = match &image {
                Image::Stencil(_) => "stencil",
                Image::Raster(_) => "raster",
            };
            self.images.push((variant, image.width(), cloned.width()));
        }
        fn pop_clip_path(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }

    /// The offscreen renderer is the same `Device` hayro's `page_render` drives, so paint
    /// must flow through it: a white rect over the transparent 4×4 context leaves an
    /// opaque white corner pixel (RGBA 255,255,255,255) in the finished pixmap.
    #[test]
    fn offscreen_renderer_paints_a_rect() {
        let mut renderer = OffscreenRenderer::new(4, 4);
        let white = Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 1.0, 1.0, 1.0])));
        renderer.draw_rect(&Rect::new(0.0, 0.0, 4.0, 4.0), Affine::IDENTITY, &white, &PathDrawMode::Fill(FillRule::NonZero));
        let pixmap = renderer.finish();
        let corner = pixmap.sample(0, 0);
        assert_eq!(
            [corner.r, corner.g, corner.b, corner.a],
            [255, 255, 255, 255],
            "the white fill must reach the pixmap (the context starts transparent)"
        );
    }

    /// Both `Image` variants reach `draw_image` and are cloneable by value, and each
    /// clone reports the same dimensions as its original — the vendor `Clone` patch the
    /// flattener's replay log depends on.
    #[test]
    fn replay_images_are_cloneable() {
        let pdf = Pdf::new(image_pdf()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 100.0, 50.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = CloningDevice::default();
        interpret_page(page, &mut state, &mut device);

        assert_eq!(device.images.len(), 2, "both XObjects must reach draw_image: {:?}", device.images);
        assert_eq!(device.images[0], ("raster", 2, 2), "the raster image clones with its dimensions");
        assert_eq!(device.images[1], ("stencil", 2, 2), "the stencil image clones with its dimensions");
    }
}
