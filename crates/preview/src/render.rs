//! First-page rendering for the preview pane (same bounds and white-paper compositing as thumbnails).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// The largest width or height of a preview-pane bitmap.
pub const MAX_PREVIEW_SIDE: u32 = 4096;

/// A rendered page: top-down BGRA pixels (Windows 32-bit DIB), opaque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewBitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Renders the first page fitted into a rectangle of at most `max_width` × `max_height` pixels.
/// Never panics.
pub fn render_page_fit(bytes: impl Into<Arc<Vec<u8>>>, max_width: u32, max_height: u32) -> Result<PreviewBitmap, String> {
    let max_width = max_width.clamp(1, MAX_PREVIEW_SIDE);
    let max_height = max_height.clamp(1, MAX_PREVIEW_SIDE);
    let bytes = bytes.into();
    catch_unwind(AssertUnwindSafe(|| render(bytes, max_width, max_height))).unwrap_or_else(|_| Err("the renderer crashed".into()))
}

fn render(bytes: Arc<Vec<u8>>, max_width: u32, max_height: u32) -> Result<PreviewBitmap, String> {
    let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
    if renderer.page_count() == 0 {
        return Err("the document could not be opened or has no pages".into());
    }
    let (w, h) = renderer.page_size(0).ok_or("the first page's size could not be read")?;
    if !w.is_finite() || !h.is_finite() || w < 0.5 || h < 0.5 {
        return Err(format!("the first page has an invalid size ({w}×{h} pt)"));
    }
    let scale = (max_width as f32 / w).min(max_height as f32 / h);
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview: true });
    if let Some(e) = page.error {
        return Err(e);
    }
    let pixels =
        usize::try_from(page.width).ok().and_then(|w| w.checked_mul(usize::try_from(page.height).ok()?)).ok_or("the preview is too large")?;
    let rgba: &[u8] = &page.rgba;
    if pixels == 0 || rgba.len() != pixels.saturating_mul(4) {
        return Err("the renderer returned no pixels".into());
    }
    let mut bgra = Vec::with_capacity(rgba.len());
    for p in rgba.as_chunks::<4>().0 {
        let paper = 255 - p[3];
        bgra.extend_from_slice(&[p[2].saturating_add(paper), p[1].saturating_add(paper), p[0].saturating_add(paper), 255]);
    }
    Ok(PreviewBitmap { width: page.width, height: page.height, bgra })
}
