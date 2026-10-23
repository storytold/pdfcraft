//! Remove White Margins (Acrobat: Set Page Boxes ▸ Margin Controls, "Crops the page to the artwork
//! boundary"): the bounds of what a page shows that is not white.
//!
//! The page is rendered and its non-white pixels are measured, so everything that decides what is
//! visible counts as the renderer decides it: clipping, white fills (which stay margin), images,
//! and comments (a crop must not hide one). Layers are measured in the document's own default
//! state, not the view's: the crop is saved into the file, and others open it with those defaults.

use std::sync::Arc;

use pdfcraft_render::{PageInfo, PageRenderer, ReadingColors, RenderConfig, RenderRequest};

use crate::{DocId, Document, Edit, EditError, PageBox, Session, check_permission, guard};

/// Device pixels per point the page is measured at: half a point is finer than anyone sets a margin.
const SCALE: f32 = 2.0;
/// The most pixels measured for one page; a larger page is measured at a lower resolution.
const MAX_PIXELS: f32 = 4_000_000.0;
/// A pixel this light in every channel is margin (anti-aliasing and near-white paper noise).
const WHITE: u8 = 250;

/// The user-space bounds of what `page` shows that is not white, inside its crop box. `None`
/// when the page is blank or can't be rendered.
fn content_bounds_with(r: &mut PageRenderer, info: &PageInfo, page: usize) -> Option<[f64; 4]> {
    let (w, h) = (info.width, info.height);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let pixels = w * h * SCALE * SCALE;
    let scale = if pixels > MAX_PIXELS { SCALE * (MAX_PIXELS / pixels).sqrt() } else { SCALE };
    let shot = guard(|| r.render(RenderRequest { page, scale, ..Default::default() })).ok()?;
    if shot.error.is_some() || shot.width == 0 || shot.height == 0 {
        return None;
    }
    let width = shot.width as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, px) in shot.rgba.as_chunks::<4>().0.iter().enumerate() {
        if px[..3].iter().all(|c| *c >= WHITE) {
            continue;
        }
        let (col, row) = (i % width, i / width);
        x0 = x0.min(col);
        y0 = y0.min(row);
        x1 = x1.max(col + 1);
        y1 = y1.max(row + 1);
    }
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    // One pixel more on every side, so a partly covered edge pixel is never cut.
    let (sx, sy) = (w / shot.width as f32, h / shot.height as f32);
    let view = [
        (x0.saturating_sub(1) as f32 * sx).max(0.0),
        (y0.saturating_sub(1) as f32 * sy).max(0.0),
        ((x1 + 1) as f32 * sx).min(w),
        ((y1 + 1) as f32 * sy).min(h),
    ];
    let (a, b) = (info.view_to_user(view[0], view[1]), info.view_to_user(view[2], view[3]));
    let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
    // Set Page Boxes refuses a box under a point: such a page keeps its box.
    (rect.iter().all(|v| v.is_finite()) && rect[2] - rect[0] >= 1.0 && rect[3] - rect[1] >= 1.0).then_some(rect)
}

impl Document {
    /// What the pages are measured on: what is displayed (comments without an appearance get the
    /// one drawn for display), in the document's true colours on white paper, with every comment
    /// whether or not View ▸ Hide all comments is on, and the layers as the file sets them.
    fn measuring_config(&self) -> RenderConfig {
        RenderConfig {
            layers: Arc::new(Vec::new()),
            reading_colors: ReadingColors::default(),
            transparent: false,
            hide_comments: false,
            ..self.config.clone()
        }
    }

    fn measuring_renderer(&self) -> PageRenderer {
        PageRenderer::new(self.display.clone(), self.measuring_config())
    }

    /// The user-space bounds of what `page` (0-based) shows that is not white, inside its crop
    /// box. `None` when the page is blank or can't be rendered.
    pub fn content_bounds(&self, page: usize) -> Option<[f64; 4]> {
        let info = self.info.pages.get(page)?;
        content_bounds_with(&mut self.measuring_renderer(), info, page)
    }

    /// [`Self::content_bounds`] of each of `pages` that shows something, for
    /// [`crate::Edit::RemoveWhiteMargins`]. Blank pages are left out, so they keep their boxes.
    pub fn white_margin_crops(&self, pages: &[usize]) -> Vec<(usize, [f64; 4])> {
        let mut r = self.measuring_renderer();
        pages.iter().filter_map(|&p| content_bounds_with(&mut r, self.info.pages.get(p)?, p).map(|b| (p, b))).collect()
    }
}

/// Remove White Margins over a page range, detached from the session so a long range can be
/// measured on a worker thread.
pub struct WhiteMarginsJob {
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    infos: Vec<PageInfo>,
    pub pages: Vec<usize>,
    pub which: PageBox,
}

impl WhiteMarginsJob {
    /// Measure the pages. `progress(done, total)` is called before each page; returning `false`
    /// stops, and then there is no edit to apply.
    pub fn run(self, mut progress: impl FnMut(usize, usize) -> bool) -> Option<Edit> {
        let mut r = PageRenderer::new(self.bytes, self.config);
        let total = self.pages.len();
        let mut rects = Vec::new();
        for (done, &page) in self.pages.iter().enumerate() {
            if !progress(done, total) {
                return None;
            }
            if let Some(info) = self.infos.get(page)
                && let Some(b) = content_bounds_with(&mut r, info, page)
            {
                rects.push((page, b));
            }
        }
        Some(Edit::RemoveWhiteMargins { which: self.which, rects })
    }
}

impl Session {
    /// A job measuring `pages` (0-based) of document `id` for Remove White Margins. Refused up
    /// front, before any page is measured, when the document can't be edited or forbids page changes.
    pub fn white_margins_job(&self, id: DocId, pages: &[usize], which: PageBox) -> Result<WhiteMarginsJob, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        if let Some(p) = editor.cos.permissions() {
            check_permission(&Edit::RemoveWhiteMargins { which, rects: Vec::new() }, &p)?;
        }
        let all = doc.info.pages.len();
        Ok(WhiteMarginsJob {
            bytes: doc.display.clone(),
            config: doc.measuring_config(),
            infos: doc.info.pages.clone(),
            pages: pages.iter().copied().filter(|p| *p < all).collect(),
            which,
        })
    }
}
