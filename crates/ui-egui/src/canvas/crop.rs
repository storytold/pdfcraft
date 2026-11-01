//! The Crop tool (Acrobat: Edit ▸ Crop pages): drag a rectangle on a page to crop it to that
//! rectangle; double-click a page to open Set Page Boxes for it.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{BoxSpec, Edit, PageBox};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;

/// A crop rectangle being dragged: (page, start on screen).
pub type CropDrag = Option<(usize, Pos2)>;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Crop gestures on one page. Returns `Some(true)` to open Set Page Boxes for this page.
pub(crate) struct CropPageInput<'a> {
    pub(crate) resp: &'a egui::Response,
    pub(crate) xf: &'a PageXform,
    pub(crate) ptr: &'a PagePointer,
    pub(crate) page: usize,
    pub(crate) info: &'a DocInfo,
    pub(crate) view: &'a mut DocView,
    pub(crate) allowed: bool,
}

pub(crate) fn page_input(ui: &egui::Ui, input: CropPageInput<'_>) -> bool {
    let CropPageInput { resp, xf, ptr, page, info, view, allowed } = input;
    let (pointer, origin) = (ptr.pos, ptr.origin);
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        view.cursor.claim(Layer::ToolArea, if allowed { egui::CursorIcon::Crosshair } else { egui::CursorIcon::NotAllowed });
    }
    if !allowed {
        return false;
    }
    // The tool's own press, of the primary button: nothing drawn over the page here holds it.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && view.router.owner(page).is_none()
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.crop_drag = Some((page, o));
    }
    if let Some((p, start)) = view.crop_drag
        && p == page
    {
        // Where the pointer is, or where it went down when it is off the window.
        let end = pointer.unwrap_or(start);
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
        if resp.drag_stopped() || !ptr.primary_down {
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
