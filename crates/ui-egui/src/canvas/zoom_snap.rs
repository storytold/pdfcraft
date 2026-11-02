//! View ▸ Zoom ▸ Marquee Zoom (drag a rectangle to fill the window with it; click to zoom in)
//! and Edit ▸ Take a Snapshot (drag a rectangle to copy that area as an image).

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_render::{RenderConfig, RenderRequest, RequestKind, Tile};

use crate::canvas::{DocView, PageXform};
use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;
use crate::{PdfCraftApp, QuickTool};

/// A finished gesture: (page, the rectangle on screen, the same in page view points
/// [x0, y0, x1, y1]); a plain click has an empty rectangle.
pub type Marquee = (usize, Rect, [f32; 4]);

/// Drag a rectangle on page `page` (Marquee Zoom, Snapshot), with a press nobody holds (`free`).
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, ptr: &PagePointer, page: usize, free: bool, view: &mut DocView) {
    let (pointer, origin) = (ptr.pos, ptr.origin);
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
    }
    // The tool's own press, of the primary button: one nobody holds.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && free
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.marquee = Some((page, o));
    }
    if let Some((p, start)) = view.marquee
        && p == page
    {
        // Where the pointer is, or where it went down when it is off the window.
        let end = pointer.unwrap_or(start);
        let end = Pos2::new(end.x.clamp(xf.rect.left(), xf.rect.right()), end.y.clamp(xf.rect.top(), xf.rect.bottom()));
        let r = Rect::from_two_pos(start, end);
        ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
        if resp.drag_stopped() || !ptr.primary_down {
            view.marquee = None;
            let (a, b) = (xf.screen_to_view(r.left_top()), xf.screen_to_view(r.right_bottom()));
            view.marquee_done = Some((page, r, [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]));
        }
    }
    if free
        && resp.clicked()
        && let Some(p) = pointer.filter(|p| xf.rect.contains(*p))
    {
        let v = xf.screen_to_view(p);
        view.marquee_done = Some((page, Rect::from_min_size(p, egui::Vec2::ZERO), [v.0, v.1, v.0, v.1]));
    }
}

impl PdfCraftApp {
    /// Finish a marquee gesture for the current tool.
    pub(crate) fn finish_marquee(&mut self, index: usize, done: Marquee) {
        let (page, rect, view_rect) = done;
        match self.quick_tool {
            QuickTool::MarqueeZoom => {
                let view = &mut self.views[index];
                if rect.width() < 6.0 || rect.height() < 6.0 {
                    // A click zooms in one step around the point.
                    let z = (view.zoom * 1.5).min(64.0);
                    view.zoom_at(z, rect.center());
                } else {
                    view.zoom_to_rect(rect);
                }
            }
            QuickTool::Snapshot => {
                if view_rect[2] - view_rect[0] < 2.0 || view_rect[3] - view_rect[1] < 2.0 {
                    return;
                }
                // The tool outlives the tab it was chosen on.
                if self.views.get(index).map(|v| v.id).is_some_and(|id| !self.may_copy(id)) {
                    return;
                }
                match self.snapshot(index, page, view_rect) {
                    Ok((w, h)) => {
                        self.notify_fmt("The selected area has been copied ({w} × {h} pixels)", &[("w", &w.to_string()), ("h", &h.to_string())])
                    }
                    Err(e) => self.notify_fmt("Couldn't take the snapshot: {e}", &[("e", &e.to_string())]),
                }
            }
            _ => {}
        }
    }

    /// View ▸ Zoom ▸ Fit Visible on the current page: find where the page has ink (a quick
    /// low-resolution render) and zoom so that spans the window's width. A blank page fits the
    /// width.
    pub fn fit_visible(&mut self, index: usize) -> Result<(), String> {
        let view = &self.views[index];
        let page = view.current;
        let doc = self.session.get(view.id).ok_or("no document")?;
        let config = RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..RenderConfig::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        // About 800 px along the longer side is plenty to find the margins.
        let size = doc.info.pages.get(page).map_or(792.0, |p| p.width.max(p.height));
        let out =
            r.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: (800.0 / size).clamp(0.1, 4.0), tag: 0, preview: false });
        if let Some(e) = out.error {
            return Err(e);
        }
        let rot = view.rotation;
        let view = &mut self.views[index];
        match ink_bounds(out.width as usize, out.height as usize, &out.rgba) {
            Some([u0, v0, u1, v1]) => {
                // View space → the rotated view on screen.
                let rotate = |u: f32, v: f32| match rot {
                    90 => (1.0 - v, u),
                    180 => (1.0 - u, 1.0 - v),
                    270 => (v, 1.0 - u),
                    _ => (u, v),
                };
                let (a, b) = (rotate(u0, v0), rotate(u1, v1));
                if !view.fit_content(page, [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]) {
                    view.fit = crate::canvas::Fit::Width;
                }
            }
            None => view.fit = crate::canvas::Fit::Width,
        }
        Ok(())
    }

    /// Render `view_rect` (page view points) of `page` and copy it to the clipboard (and keep it
    /// in `last_snapshot`), at the current zoom or [`SNAPSHOT_MIN_SCALE`] if that is finer.
    pub fn snapshot(&mut self, index: usize, page: usize, view_rect: [f32; 4]) -> Result<(u32, u32), String> {
        let view = &self.views[index];
        let doc = self.session.get(view.id).ok_or("no document")?;
        let ppp = self.ctx.as_ref().map_or(2.0, |c| c.pixels_per_point());
        let scale = snapshot_scale(view.render_scale(ppp), view_rect[2] - view_rect[0], view_rect[3] - view_rect[1]);
        let tile = Tile {
            x: (view_rect[0] * scale).floor().max(0.0) as u32,
            y: (view_rect[1] * scale).floor().max(0.0) as u32,
            w: ((view_rect[2] - view_rect[0]) * scale).ceil().max(1.0) as u32,
            h: ((view_rect[3] - view_rect[1]) * scale).ceil().max(1.0) as u32,
        };
        if (tile.w as u64) * (tile.h as u64) > 64_000_000 {
            return Err("the area is too large at this zoom".into());
        }
        let config = RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..RenderConfig::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let out = r.render(RenderRequest { page, kind: RequestKind::Pixels, tile: Some(tile), scale, tag: 0, preview: false });
        if let Some(e) = out.error {
            return Err(e);
        }
        let (w, h) = (out.width, out.height);
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_clipboard {
            let img = arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(&out.rgba) };
            crate::os_clipboard::with(|c| c.set_image(img)).map_err(|e| e.to_string())?;
        }
        self.last_snapshot = Some((w, h, out.rgba));
        Ok((w, h))
    }
}

/// The fewest pixels per point a snapshot is drawn at: 300 ppi, so what it copies stays readable
/// however far the page is zoomed out (#5716).
const SNAPSHOT_MIN_SCALE: f32 = 300.0 / 72.0;

/// The longest side a snapshot has, in pixels: the renderer draws at most 4096 a side in one tile
/// and cuts a larger area short, and this leaves room for rounding up.
const SNAPSHOT_MAX_SIDE: f32 = 4095.0;

/// The pixels per point to draw a `width` × `height` point area at, given the `on_screen` scale:
/// the finer of that and the minimum, or as fine as the renderer can draw the whole area.
fn snapshot_scale(on_screen: f32, width: f32, height: f32) -> f32 {
    on_screen.max(SNAPSHOT_MIN_SCALE).min(SNAPSHOT_MAX_SIDE / width.max(height).max(1.0)).max(0.01)
}

/// The normalised bounds [u0, v0, u1, v1] of the pixels that aren't (near) white, if any.
pub(crate) fn ink_bounds(w: usize, h: usize, rgba: &[u8]) -> Option<[f32; 4]> {
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return None;
    }
    let ink = |p: &[u8]| p[3] > 16 && (p[0] < 240 || p[1] < 240 || p[2] < 240);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (y, row) in rgba.chunks_exact(w * 4).take(h).enumerate() {
        for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
            if ink(p) {
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0 && y1 > y0).then(|| [x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32])
}

#[cfg(test)]
mod tests {
    use super::{SNAPSHOT_MAX_SIDE, SNAPSHOT_MIN_SCALE, snapshot_scale};

    #[test]
    fn a_snapshot_is_drawn_at_the_finer_of_the_zoom_and_the_minimum() {
        assert_eq!(snapshot_scale(0.5, 100.0, 100.0), SNAPSHOT_MIN_SCALE, "zoomed out");
        assert_eq!(snapshot_scale(SNAPSHOT_MIN_SCALE, 100.0, 100.0), SNAPSHOT_MIN_SCALE);
        assert_eq!(snapshot_scale(8.0, 100.0, 100.0), 8.0, "zoomed in further, it keeps the detail on screen");
    }

    #[test]
    fn a_snapshot_stays_within_what_the_renderer_draws_at_once() {
        // Past 983 pt a side, 300 ppi is more than one tile holds.
        for (on_screen, w, h) in [(1.0, 2384.0, 3370.0), (1.0, 3370.0, 100.0), (8.0, 1000.0, 1000.0), (8.0, 14_400.0, 14_400.0)] {
            let scale = snapshot_scale(on_screen, w, h);
            let side = (w * scale).ceil().max((h * scale).ceil());
            assert!(side <= SNAPSHOT_MAX_SIDE + 1.0, "{on_screen} on {w} × {h}: {side} pixels");
        }
    }

    #[test]
    fn a_snapshot_of_no_area_is_drawn_at_the_minimum() {
        assert_eq!(snapshot_scale(1.0, 0.0, 0.0), SNAPSHOT_MIN_SCALE);
    }

    #[test]
    fn a_snapshot_scale_survives_numbers_that_are_not() {
        for scale in [snapshot_scale(f32::NAN, 5000.0, 5000.0), snapshot_scale(1.0, f32::NAN, 100.0), snapshot_scale(1.0, f32::INFINITY, 100.0)] {
            assert!(scale.is_finite() && scale > 0.0, "{scale}");
        }
    }

    #[test]
    fn ink_bounds_finds_the_marked_area() {
        let (w, h) = (10, 20);
        let mut px = vec![255u8; w * h * 4];
        for (x, y) in [(2, 5), (7, 15)] {
            px[(y * w + x) * 4] = 0;
        }
        assert_eq!(super::ink_bounds(w, h, &px), Some([0.2, 0.25, 0.8, 0.8]));
        assert_eq!(super::ink_bounds(w, h, &vec![255u8; w * h * 4]), None);
    }
}
