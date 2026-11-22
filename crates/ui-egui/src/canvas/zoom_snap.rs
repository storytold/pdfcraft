//! Edit ▸ Take a Snapshot (drag a rectangle to copy that area as an image) and Fit Visible.

use craft::behavior::{CancelReason, Cursor, CursorIcon};
use craft::host::PickRequest;
use craft::pages::Fit;
use craft::pages::ui::{GoTo, PlacedPage};
use craft::platform::clipboard::ClipboardImage;
use craft::print::ui::HostedPrint;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::canvas_controls::TaskBar;
use craft::ui::contract::{Props as _, Show as _};
use craft::ui::primitives::Draw;
use pdfcraft_render::{RenderConfig, RenderRequest, RequestKind, Tile};

use super::page_box_drag::{PageBoxDrag, page_under};
use crate::PdfCraftApp;
use crate::canvas::PageXform;

/// What was chosen in the menu under a copied area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaChoice {
    /// Print only this area (Acrobat's "print selected area").
    Print,
    /// Save the copied picture as a PNG file.
    Save,
}

/// The Snapshot tool, built per pass over the placed pages. A drag reports the page and the
/// rectangle in page view points; a click with no drag, or under 2 points either way, reports nothing.
pub(crate) struct SnapshotTool<'a> {
    placed: Vec<PlacedPage>,
    /// Per placed page: its unrotated view-space size.
    sizes: Vec<(usize, (f32, f32))>,
    rotation: u16,
    /// The session's drag, kept from the press to the release.
    drag: &'a mut PageBoxDrag,
    done: Option<(usize, [f32; 4])>,
}

impl<'a> SnapshotTool<'a> {
    /// A tool over `placed`, in a view turned by `rotation`, with its rectangle in `drag`; `page_size_of` gives a
    /// page's size in view points.
    pub(crate) fn new(drag: &'a mut PageBoxDrag, placed: &[PlacedPage], rotation: u16, page_size_of: impl Fn(usize) -> (f32, f32)) -> Self {
        let sizes = placed.iter().map(|p| (p.page, page_size_of(p.page))).collect();
        Self { placed: placed.to_vec(), sizes, rotation, drag, done: None }
    }

    /// The area the last release made, once.
    pub(crate) fn take_result(&mut self) -> Option<(usize, [f32; 4])> {
        self.done.take()
    }

    /// Whether a drag is running: the kept outline is put away meanwhile.
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_active()
    }
}

impl CanvasTool for SnapshotTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        match page_under(&self.placed, pointer.pos) {
            Some(placed) if self.drag.press(placed, pointer.pos) => ToolAction::Claimed,
            _ => ToolAction::Ignored,
        }
    }

    fn drag(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.drag.drag(pointer.pos);
    }

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.drag.drag(pointer.pos);
        let Some(scale) = self.drag.current().and_then(|(bounds, _)| self.placed.iter().find(|p| p.rect == bounds)).map(|p| p.scale as f32) else {
            self.drag.cancel();
            return;
        };
        let Some((page, rect)) = self.drag.release(2.0 * scale) else { return };
        let Some(placed) = self.placed.iter().find(|p| p.page == page) else { return };
        let Some(size) = self.sizes.iter().find(|(p, _)| *p == page).map(|(_, s)| *s) else { return };
        let xf = PageXform::of(placed, self.rotation, size);
        let (a, b) = (xf.screen_to_view(rect.left_top()), xf.screen_to_view(rect.right_bottom()));
        self.done = Some((page, [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]));
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        self.drag.cancel();
    }

    fn paint(&self, _draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.drag.paint(cx);
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        if cx.pointer.is_some_and(|p| page_under(&self.placed, p.pos).is_some()) {
            Cursor::System(CursorIcon::Crosshair)
        } else {
            Cursor::System(CursorIcon::Default)
        }
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Print area and Save as image commands of the area bar")
    }
}

/// The fixed task bar under the kept area `area_pick` (page, view-space rect) with what can be done
/// with it: the choice made this pass. The outline itself is the page body's (marching ants).
pub(crate) fn area_bar(
    ui: &mut egui::Ui,
    body: &craft::pages::ui::PageBody<'_>,
    placed: &[PlacedPage],
    area_pick: Option<(usize, [f32; 4])>,
    rotation: u16,
    page_size_of: impl Fn(usize) -> (f32, f32),
) -> Option<AreaChoice> {
    let (page, area) = area_pick?;
    let placed = placed.iter().find(|p| p.page == page)?;
    let screen = PageXform::of(placed, rotation, page_size_of(page)).view_rect(area);
    let anchor = [screen.left_top(), screen.right_top(), screen.right_bottom(), screen.left_bottom()].map(|p| body.view.to_doc(p));
    let mut choice = None;
    TaskBar::fixed(tl!("Snapshot area"), &anchor).show(ui, body.rect, |ui, _cx| {
        if craft::ui::buttons::Button::new(tl!("Print area…")).show(ui).clicked() {
            choice = Some(AreaChoice::Print);
        }
        if craft::ui::buttons::Button::new(tl!("Save as image…")).show(ui).clicked() {
            choice = Some(AreaChoice::Save);
        }
    });
    choice
}

impl PdfCraftApp {
    /// Finish a Snapshot drag: copy the area `view_rect` of `page`.
    pub(crate) fn finish_marquee(&mut self, ctx: &egui::Context, index: usize, done: (usize, [f32; 4])) {
        let (page, view_rect) = done;
        // The tool outlives the tab it was chosen on.
        if self.views.get(index).map(|v| v.id).is_some_and(|id| !self.may_copy(id)) {
            return;
        }
        match self.snapshot(ctx, index, page, view_rect) {
            Ok((w, h)) => {
                self.views[index].session.area_pick = Some((page, view_rect));
                self.notify_fmt("The selected area has been copied ({w} × {h} pixels)", &[("w", &w.to_string()), ("h", &h.to_string())])
            }
            Err(e) => self.notify_fmt("Couldn't take the snapshot: {e}", &[("e", &e.to_string())]),
        }
    }

    /// View ▸ Zoom ▸ Fit Visible on the current page: find where the page has ink (a quick
    /// low-resolution render) and ask the page view to bring that to the window's width. A blank
    /// page fits the width.
    pub fn fit_visible(&mut self, index: usize) -> Result<(), String> {
        let view = &self.views[index];
        let page = view.view.current;
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
        let rot = view.view.rotation % 360;
        let (w, h) = doc.info.pages.get(page).map_or((612.0, 792.0), |p| (f64::from(p.width), f64::from(p.height)));
        // The page as the view shows it: turned a quarter swaps its sides.
        let (shown_w, shown_h) = if rot == 90 || rot == 270 { (h, w) } else { (w, h) };
        let view = &mut self.views[index].view;
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
                let (x0, x1) = (f64::from(a.0.min(b.0)) * shown_w, f64::from(a.0.max(b.0)) * shown_w);
                let (y0, y1) = (f64::from(a.1.min(b.1)) * shown_h, f64::from(a.1.max(b.1)) * shown_h);
                view.fit = Fit::None;
                // The page view fits `rect` to its viewport in its own pass; a margin of 0 keeps its side gap.
                view.go_to = Some(GoTo::Rect { page, rect: craft::behavior::kurbo::Rect::new(x0, y0, x1, y1), margin: 0.0 });
            }
            None => view.fit = Fit::Width,
        }
        Ok(())
    }

    /// Print ▸ Selected area: the Print dialog for only `view_rect` (page view points, y down) of
    /// `page`, sized like a page of its own (Fit to paper or Actual size).
    pub(crate) fn open_print_area(&mut self, page: usize, view_rect: [f32; 4]) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(height) = self.session.get(id).and_then(|d| d.info.pages.get(page)).map(|p| f64::from(p.height)) else { return };
        self.open_print();
        if self.dialogs.is_open::<HostedPrint>() {
            let [x0, y0, x1, y1] = view_rect.map(f64::from);
            let d = &mut self.print_draft;
            // The print crate takes the area in display space, y up.
            d.area = Some((page, [x0.min(x1), height - y0.max(y1), x0.max(x1), height - y0.min(y1)]));
            d.which = crate::print_ui::Which::Area;
            d.handling = crate::print_ui::Handling::Size;
            d.sheet = 0;
        }
    }

    /// Save the area `view_rect` of `page` that view `index` copied as a PNG file (asked where).
    /// The last snapshot is used when it was taken of that area; otherwise (another tab
    /// snapshotted since) the area is rendered again.
    pub(crate) fn save_snapshot_image(&mut self, ctx: &egui::Context, index: usize, page: usize, view_rect: [f32; 4]) {
        let this_area = self.views.get(index).is_some_and(|v| self.last_snapshot_of == Some((v.id, page, view_rect)));
        let png = match self.last_snapshot.as_ref() {
            Some((w, h, pixels)) if this_area => pdfcraft_engine::export::encode_png(*w, *h, pixels),
            _ => self.render_area(ctx, index, page, view_rect).and_then(|(w, h, pixels)| pdfcraft_engine::export::encode_png(w, h, &pixels)),
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
        self.pick_first(PickRequest::save("snapshot").file_name(name).filter("PNG", &["png"]), None, move |app, _, path| {
            match craft::atomic::atomic_write(&path, &png) {
                Ok(()) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            }
        });
    }

    /// Render `view_rect` (page view points) of `page` and copy it to the clipboard (and keep it
    /// in `last_snapshot`), at the current zoom or [`SNAPSHOT_MIN_SCALE`] if that is finer.
    pub fn snapshot(&mut self, ctx: &egui::Context, index: usize, page: usize, view_rect: [f32; 4]) -> Result<(u32, u32), String> {
        let (w, h, rgba) = self.render_area(ctx, index, page, view_rect)?;
        self.clipboard.write_image(&ClipboardImage { width: w, height: h, rgba: rgba.to_vec() }).map_err(|e| e.to_string())?;
        self.last_snapshot = Some((w, h, rgba));
        self.last_snapshot_of = Some((self.views[index].id, page, view_rect));
        Ok((w, h))
    }

    /// Render `view_rect` (page view points) of `page` in view `index` at its current zoom:
    /// (width, height, premultiplied RGBA).
    fn render_area(
        &self,
        ctx: &egui::Context,
        index: usize,
        page: usize,
        view_rect: [f32; 4],
    ) -> Result<(u32, u32, pdfcraft_render::Pixels), String> {
        let view = self.views.get(index).ok_or("no document")?;
        let doc = self.session.get(view.id).ok_or("no document")?;
        let scale = snapshot_scale(view.render_scale(ctx.pixels_per_point()), view_rect[2] - view_rect[0], view_rect[3] - view_rect[1]);
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
