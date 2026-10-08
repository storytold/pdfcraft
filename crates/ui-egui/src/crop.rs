//! The Crop tool (Acrobat: Edit ▸ Crop pages): drag a rectangle on a page to crop it to that
//! rectangle; double-click a page to open Set Page Boxes for it.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{BoxSpec, Edit, PageBox};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};

/// A crop rectangle being dragged: (page, start on screen).
pub type CropDrag = Option<(usize, Pos2)>;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Crop gestures on one page. Returns `Some(true)` to open Set Page Boxes for this page.
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    view: &mut DocView,
    allowed: bool,
) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let origin = ui.input(|i| i.pointer.press_origin());
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        ui.ctx().set_cursor_icon(if allowed { egui::CursorIcon::Crosshair } else { egui::CursorIcon::NotAllowed });
    }
    if !allowed {
        return false;
    }
    if resp.drag_started()
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.crop_drag = Some((page, o));
    }
    if let Some((p, start)) = view.crop_drag
        && p == page
        && let Some(end) = pointer
    {
        let end = Pos2::new(end.x.clamp(xf.rect.left(), xf.rect.right()), end.y.clamp(xf.rect.top(), xf.rect.bottom()));
        let r = Rect::from_two_pos(start, end);
        // Dim what will be cut away.
        let painter = ui.painter();
        for band in [
            Rect::from_min_max(xf.rect.min, Pos2::new(xf.rect.right(), r.top())),
            Rect::from_min_max(Pos2::new(xf.rect.left(), r.bottom()), xf.rect.max),
            Rect::from_min_max(Pos2::new(xf.rect.left(), r.top()), Pos2::new(r.left(), r.bottom())),
            Rect::from_min_max(Pos2::new(r.right(), r.top()), Pos2::new(xf.rect.right(), r.bottom())),
        ] {
            painter.rect_filled(band, CornerRadius::ZERO, Color32::from_black_alpha(70));
        }
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
        if resp.drag_stopped() {
            view.crop_drag = None;
            if r.width() >= 4.0 && r.height() >= 4.0 {
                let (a, b) = (to_user(xf, info, page, r.left_top()), to_user(xf, info, page, r.right_bottom()));
                let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
                view.pending_edit = Some(Edit::SetPageBox { pages: vec![page], which: PageBox::Crop, spec: BoxSpec::Rect(rect) });
            }
        }
    }
    resp.double_clicked() && pointer.is_some_and(|p| xf.rect.contains(p))
}
