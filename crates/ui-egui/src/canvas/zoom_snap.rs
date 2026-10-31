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

/// What was chosen in the menu under a copied area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaChoice {
    /// Print only this area (Acrobat's "print selected area").
    Print,
    /// Save the copied picture as a PNG file.
    Save,
}

/// Drag a rectangle on page `page` (Marquee Zoom, Snapshot).
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, ptr: &PagePointer, page: usize, view: &mut DocView) {
    let (pointer, origin) = (ptr.pos, ptr.origin);
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
    }
    // The tool's own press, of the primary button: nothing drawn over the page here holds it.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && view.router.owner(page).is_none()
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.marquee = Some((page, o));
        view.area_pick = None;
    }
    // The area just copied stays outlined, with what can be done with it underneath.
    if let Some((p, area)) = view.area_pick
        && p == page
        && view.marquee.is_none()
    {
        let r = xf.view_rect(area);
        ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
        egui::Area::new(egui::Id::unique("snapshot-area-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(r.left_bottom() + egui::vec2(0.0, 6.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Print area…")).clicked() {
                            view.area_choice = Some(AreaChoice::Print);
                        }
                        if ui.button(tl!("Save as image…")).clicked() {
                            view.area_choice = Some(AreaChoice::Save);
                        }
                    });
                });
            });
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
    if resp.clicked()
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
                        self.views[index].area_pick = Some((page, view_rect));
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

    /// Print ▸ Selected area: the Print dialog for only `view_rect` (page view points, y down) of
    /// `page`, sized like a page of its own (Fit to paper or Actual size).
    pub(crate) fn open_print_area(&mut self, page: usize, view_rect: [f32; 4]) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(height) = self.session.get(id).and_then(|d| d.info.pages.get(page)).map(|p| f64::from(p.height)) else { return };
        self.open_print();
        if self.dialog == Some(crate::Dialog::Print) {
            let [x0, y0, x1, y1] = view_rect.map(f64::from);
            let d = &mut self.print_draft;
            // The print crate takes the area in display space, y up.
            d.area = Some((page, [x0.min(x1), height - y0.max(y1), x0.max(x1), height - y0.min(y1)]));
            d.which = crate::print_ui::Which::Area;
            d.handling = crate::print_ui::Handling::Size;
            d.sheet = 0;
        }
    }

    /// Save the area `view_rect` of `page` that view `index` copied as a PNG file (asked where;
    /// `save_override` in tests). The last snapshot is used when it was taken of that area;
    /// otherwise (another tab snapshotted since) the area is rendered again.
    pub(crate) fn save_snapshot_image(&mut self, index: usize, page: usize, view_rect: [f32; 4]) {
        let this_area = self.views.get(index).is_some_and(|v| self.last_snapshot_of == Some((v.id, page, view_rect)));
        let png = match self.last_snapshot.as_ref() {
            Some((w, h, pixels)) if this_area => pdfcraft_engine::export::encode_png(*w, *h, pixels),
            _ => self.render_area(index, page, view_rect).and_then(|(w, h, pixels)| pdfcraft_engine::export::encode_png(w, h, &pixels)),
        };
        let png = match png {
            Ok(png) => png,
            Err(e) => {
                self.notify_fmt("Couldn't save the image: {e}", &[("e", &e)]);
                return;
            }
        };
        let stem = self
            .active_ids()
            .and_then(|(_, id)| self.session.get(id))
            .map_or_else(String::new, |d| d.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string());
        let name = format!("{stem} area.png");
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), &png) {
                Ok(()) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            };
            match self.save_override.clone() {
                Some(p) => write(self, p.into()),
                None => {
                    let dialog =
                        rfd::AsyncFileDialog::new().set_title(tl!("Save as image").to_string()).add_filter("PNG", &["png"]).set_file_name(name);
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Err(e) = crate::editing::download(&name, &png) {
            self.notify_error(e);
        }
    }

    /// Render `view_rect` (page view points) of `page` at the current zoom and copy it to the
    /// clipboard (and keep it in `last_snapshot`).
    pub fn snapshot(&mut self, index: usize, page: usize, view_rect: [f32; 4]) -> Result<(u32, u32), String> {
        let (w, h, rgba) = self.render_area(index, page, view_rect)?;
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_clipboard {
            let img = arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(&rgba) };
            arboard::Clipboard::new().and_then(|mut c| c.set_image(img)).map_err(|e| e.to_string())?;
        }
        self.last_snapshot = Some((w, h, rgba));
        self.last_snapshot_of = Some((self.views[index].id, page, view_rect));
        Ok((w, h))
    }

    /// Render `view_rect` (page view points) of `page` in view `index` at its current zoom:
    /// (width, height, premultiplied RGBA).
    fn render_area(&self, index: usize, page: usize, view_rect: [f32; 4]) -> Result<(u32, u32, pdfcraft_render::Pixels), String> {
        let view = self.views.get(index).ok_or("no document")?;
        let doc = self.session.get(view.id).ok_or("no document")?;
        let ppp = self.ctx.as_ref().map_or(2.0, |c| c.pixels_per_point());
        let scale = (view.zoom * ppp).clamp(0.5, 8.0);
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
        Ok((out.width, out.height, out.rgba))
    }
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
