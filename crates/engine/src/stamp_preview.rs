//! A stamp while it is moved or resized: the page without it, and the stamp alone. Any other
//! comment while it is moved: the comment alone.

use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderedPage};

use crate::{Document, guard};

/// Read-only layers for interactive movement and resizing: the page without the stamp, and the
/// stamp alone on a transparent page. Moving or resizing a stamp only changes its `/Rect`, which
/// its appearance is stretched to fit, so these two show the result before it is applied.
/// Preparing them never changes the working bytes, dirty flag or undo history.
pub struct StampPreview {
    page: usize,
    background: Arc<Vec<u8>>,
    stamp: Arc<Vec<u8>>,
    config: RenderConfig,
}

impl StampPreview {
    /// One worker for the page beneath the stamp.
    pub fn background_renderer(&self) -> RenderPool {
        RenderPool::new(self.background.clone(), 1, self.config.clone())
    }

    /// One worker for the stamp alone: its page has no content, no other annotations and no
    /// background, so only the stamp's own pixels are drawn.
    pub fn stamp_renderer(&self) -> RenderPool {
        RenderPool::new(self.stamp.clone(), 1, self.stamp_config())
    }

    /// The page beneath the stamp, rendered synchronously with the renderer's normal pixel limits.
    pub fn render_background(&self, scale: f32) -> Result<RenderedPage, String> {
        self.render(&self.background, self.config.clone(), scale)
    }

    /// The stamp alone on a transparent page, rendered like [`Self::render_background`].
    pub fn render_stamp(&self, scale: f32) -> Result<RenderedPage, String> {
        self.render(&self.stamp, self.stamp_config(), scale)
    }

    fn stamp_config(&self) -> RenderConfig {
        RenderConfig { transparent: true, ..self.config.clone() }
    }

    fn render(&self, bytes: &Arc<Vec<u8>>, config: RenderConfig, scale: f32) -> Result<RenderedPage, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("preview scale must be finite and positive".into());
        }
        guard(|| {
            let out = PageRenderer::new(bytes.clone(), config).render(RenderRequest { page: self.page, scale, ..Default::default() });
            if let Some(e) = &out.error {
                return Err(e.clone());
            }
            Ok(out)
        })?
    }
}

impl Document {
    /// The layers that show the stamp at `(page, index)` moved or resized as it is dragged.
    /// `None` for an annotation that isn't a stamp, or one without an appearance to draw.
    pub fn stamp_preview(&self, page: usize, index: usize) -> Result<Option<StampPreview>, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let is_stamp = self.info.annotations.iter().any(|a| a.page == page && a.index == index && a.subtype == "Stamp");
            if !is_stamp {
                return Ok(None);
            }
            let mut stamp = editor.cos.clone();
            if !pdfcraft_annot::isolate_annotation(&mut stamp, page, index).map_err(|e| e.to_string())? {
                return Ok(None);
            }
            let mut background = editor.cos.clone();
            pdfcraft_annot::lift_annotation(&mut background, page, index).map_err(|e| e.to_string())?;
            // The page beneath as it is displayed: comments without an appearance get theirs.
            let background = pdfcraft_annot::with_missing_appearances(&background).unwrap_or(background);
            let write = |cos: &pdfcraft_cos::Document| write_incremental(cos, &SaveOptions::default()).map(Arc::new).map_err(|e| e.to_string());
            Ok(Some(StampPreview { page, background: write(&background)?, stamp: write(&stamp)?, config: self.config.clone() }))
        })?
    }
}

/// Comments alone on a transparent page, for drawing them where they go while they are moved.
/// The page beneath keeps showing them where they were until the move is applied: rendering the page
/// without it would cost a whole page's render at every drag on a heavy plan. Read-only, as
/// [`StampPreview`] is.
pub struct CommentLayer {
    page: usize,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
}

impl CommentLayer {
    /// One worker for the comments alone: their page has no content and no other annotations.
    pub fn renderer(&self) -> RenderPool {
        RenderPool::new(self.bytes.clone(), 1, self.transparent())
    }

    /// The comments alone, rendered synchronously with the renderer's normal pixel limits.
    pub fn render(&self, scale: f32) -> Result<RenderedPage, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("preview scale must be finite and positive".into());
        }
        guard(|| {
            let out =
                PageRenderer::new(self.bytes.clone(), self.transparent()).render(RenderRequest { page: self.page, scale, ..Default::default() });
            if let Some(e) = &out.error {
                return Err(e.clone());
            }
            Ok(out)
        })?
    }

    fn transparent(&self) -> RenderConfig {
        RenderConfig { transparent: true, ..self.config.clone() }
    }
}

impl Document {
    /// The comments at `indices` on `page` alone (the ones moved together), drawn as on screen:
    /// one without an appearance of its own gets the one the page view draws. `None` when one
    /// of them doesn't exist or none has anything to draw.
    pub fn comment_layer(&self, page: usize, indices: &[usize]) -> Result<Option<CommentLayer>, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            if indices.is_empty() || !indices.iter().all(|&i| self.info.annotations.iter().any(|a| a.page == page && a.index == i)) {
                return Ok(None);
            }
            let mut alone = pdfcraft_annot::with_missing_appearances(&editor.cos).unwrap_or_else(|| editor.cos.clone());
            if !pdfcraft_annot::isolate_annotations(&mut alone, page, indices).map_err(|e| e.to_string())? {
                return Ok(None);
            }
            let bytes = write_incremental(&alone, &SaveOptions::default()).map(Arc::new).map_err(|e| e.to_string())?;
            Ok(Some(CommentLayer { page, bytes, config: self.config.clone() }))
        })?
    }
}
