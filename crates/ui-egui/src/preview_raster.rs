//! A retained preview renderer with independently invalidated display colours.

use egui::{TextureHandle, TextureOptions};
use pdfcraft_render::{PRINT_TAG, RenderPool, RenderRequest, RenderedPage};

use crate::page_colors::PageColors;

pub(crate) struct PreviewRaster {
    renderer: RenderPool,
    texture: Option<TextureHandle>,
    wanted: Option<RenderRequest>,
    received: Option<u64>,
    next_tag: u64,
    colors: Option<PageColors>,
    failed: bool,
}

impl PreviewRaster {
    pub fn new(renderer: RenderPool) -> Self {
        Self { renderer, texture: None, wanted: None, received: None, next_tag: 0, colors: None, failed: false }
    }

    pub fn pending(&self) -> bool {
        !self.failed && self.wanted.is_some_and(|request| self.received != Some(request.tag))
    }

    /// The preview can't be drawn: its render failed, or its scale or tag can't be used.
    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    pub fn prepare(&mut self, ctx: &egui::Context, page: usize, scale: f32, colors: PageColors, preview: bool) {
        self.request(page, scale, colors, preview);
        if let Some(result) = self.renderer.try_recv() {
            self.receive(ctx, result);
        }
        if self.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> pdfcraft_render::RenderStats {
        self.renderer.stats()
    }

    fn request(&mut self, page: usize, scale: f32, colors: PageColors, preview: bool) {
        if !scale.is_finite() || scale <= 0.0 {
            self.failed = true;
            self.texture = None;
            self.wanted = None;
            self.renderer.set_queue(Vec::new());
            return;
        }
        let recolor = self.colors != Some(colors);
        let resized = self.wanted.is_none_or(|request| request.page != page || request.scale != scale || request.preview != preview);
        if recolor || resized {
            // Preview tags are local sequence numbers, never print tags. If exhausted,
            // leave the ordinary page usable instead of reusing a potentially stale tag.
            let Some(tag) = self.next_tag.checked_add(1).filter(|tag| *tag < PRINT_TAG) else {
                self.failed = true;
                self.texture = None;
                self.wanted = None;
                self.renderer.set_queue(Vec::new());
                return;
            };
            self.next_tag = tag;
            self.colors = Some(colors);
            self.failed = false;
            if recolor {
                self.texture = None;
            }
            let request = RenderRequest { page, scale, tag, preview, ..Default::default() };
            self.wanted = Some(request);
            self.renderer.set_queue(vec![request]);
        }
    }

    fn receive(&mut self, ctx: &egui::Context, result: RenderedPage) {
        // Even an obsolete failure must not disable a newer request.
        if self.wanted != Some(result.request) {
            return;
        }
        let count = (result.width as usize).checked_mul(result.height as usize);
        if result.error.is_some() || result.width == 0 || result.height == 0 || count.and_then(|n| n.checked_mul(4)) != Some(result.rgba.len()) {
            self.failed = true;
            self.texture = None;
            return;
        }
        let Some(colors) = self.colors else { return };
        self.texture = Some(ctx.load_texture(
            "page-preview-background",
            crate::canvas::texture_image([result.width as usize, result.height as usize], result.rgba, colors),
            TextureOptions::LINEAR,
        ));
        self.received = Some(result.request.tag);
        self.failed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;
    use std::sync::Arc;

    fn raster() -> PreviewRaster {
        let bytes = Arc::new(b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 2 2] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF".to_vec());
        PreviewRaster::new(RenderPool::new_inline(bytes, Default::default()))
    }

    fn result(request: RenderRequest, error: Option<String>) -> RenderedPage {
        RenderedPage {
            request,
            width: 2,
            height: 2,
            rgba: pdfcraft_render::Pixels::from(vec![u32::from_ne_bytes([255; 4]); 4]),
            error,
            text: None,
            warnings: Vec::new(),
            millis: 0,
        }
    }

    #[test]
    fn preview_rejects_late_pixels_and_errors_after_pending_mode_changes() {
        let ctx = egui::Context::default();
        let mut raster = raster();
        raster.prepare(&ctx, 0, 1.0, PageColors::Original, false);
        crate::page_colors::uploaded_pixels(&ctx, raster.texture().unwrap());
        let old = raster.wanted.unwrap();
        for colors in
            [PageColors::Dark(pdfcraft_render::DarkPalette::Soft), PageColors::Original, PageColors::Dark(pdfcraft_render::DarkPalette::Soft)]
        {
            raster.request(0, 1.0, colors, false);
            assert!(raster.pending());
            assert!(raster.texture().is_none(), "obsolete display textures are not painted");
            raster.receive(&ctx, result(old, None));
            raster.receive(&ctx, result(old, Some("obsolete failure".into())));
            assert!(raster.pending() && !raster.failed);
        }
        raster.receive(&ctx, result(raster.wanted.unwrap(), None));
        let texture = raster.texture().unwrap();
        assert_eq!(crate::page_colors::uploaded_pixels(&ctx, texture), vec![Color32::from_gray(25); 4]);
        let id = texture.id();
        let request = raster.wanted;
        raster.request(0, 1.0, PageColors::Dark(pdfcraft_render::DarkPalette::Soft), false);
        assert_eq!(raster.wanted, request, "unchanged mode requests no work");
        assert_eq!(raster.texture().unwrap().id(), id);
        assert!(!raster.pending());
    }

    #[test]
    fn preview_invalid_pixels_and_exhausted_tags_leave_a_usable_fallback() {
        let ctx = egui::Context::default();
        let mut raster = raster();
        raster.request(0, 1.0, PageColors::Dark(pdfcraft_render::DarkPalette::Soft), false);
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let old = raster.wanted.unwrap();
            raster.request(0, scale, PageColors::Dark(pdfcraft_render::DarkPalette::Soft), false);
            raster.receive(&ctx, result(old, None));
            assert!(raster.failed && !raster.pending() && raster.texture().is_none());
            raster.request(0, 1.0, PageColors::Dark(pdfcraft_render::DarkPalette::Soft), false);
            assert!(raster.pending(), "a valid request recovers after an invalid scale");
        }
        let mut bad = result(raster.wanted.unwrap(), None);
        bad.rgba = pdfcraft_render::Pixels::from(vec![0u32]);
        raster.receive(&ctx, bad);
        assert!(raster.failed && !raster.pending() && raster.texture().is_none());
        raster.next_tag = PRINT_TAG - 1;
        raster.request(0, 1.0, PageColors::Original, false);
        assert!(raster.failed && !raster.pending() && raster.texture().is_none());
    }
}
