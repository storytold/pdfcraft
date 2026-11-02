//! Scan & OCR ▸ Enhance ▸ Camera image: four corner handles on a page, dragged onto the edges of
//! a photographed document; Enhance page straightens what they enclose
//! (`Session::enhance_camera_page`, one undoable step).

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Shape, Stroke, vec2};

use crate::canvas::{DocView, PageXform, ViewAction};
use crate::theme::Tokens;
pub use pdfcraft_engine::camera_clean::Look;

/// How far in from the page's edges the corners start, as a share of its size.
const INSET: f32 = 0.05;
/// How close (screen points) the pointer must be to pick up a corner.
const GRAB: f32 = 12.0;
/// The slider's top.
pub const MAX_LEVEL: u8 = pdfcraft_engine::camera_clean::MAX_LEVEL;
const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// The quadrilateral being placed: its page, its corners in view space (points, y down) and the
/// corner being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraQuad {
    pub page: usize,
    pub corners: [[f32; 2]; 4],
    pub grab: Option<usize>,
}

impl CameraQuad {
    /// The page `page` (view size `w` × `h` points) inset by [`INSET`] on every side.
    pub fn inset(page: usize, w: f32, h: f32) -> Self {
        let (dx, dy) = (w * INSET, h * INSET);
        Self { page, corners: [[dx, dy], [w - dx, dy], [w - dx, h - dy], [dx, h - dy]], grab: None }
    }
}

/// The clean-up level a page is enhanced at before the slider is touched.
pub const DEFAULT_LEVEL: u8 = 50;

/// A page just enhanced: what the bar's slider needs to enhance it again at another level
/// (from the straightened photo, keeping the turns made since).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraDone {
    pub page: usize,
    /// The corners it was straightened from, in PDF user space.
    pub corners: [[f64; 2]; 4],
    /// The look and slider level chosen, and the ones the page shows.
    pub look: Look,
    pub level: u8,
    pub applied: (Look, u8),
    /// Quarter turns made since, in degrees (clockwise positive).
    pub turns: i64,
    /// The undo steps the enhancement and the turns took.
    pub steps: usize,
}

fn screen(xf: &PageXform, c: [f32; 2]) -> Pos2 {
    xf.norm_to_screen(c[0] / xf.pw.max(1e-3), c[1] / xf.ph.max(1e-3))
}

/// Corner dragging on one page. A press on a page without the quadrilateral moves it there.
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, page: usize, view: &mut DocView, allowed: bool) {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let origin = ui.input(|i| i.pointer.press_origin());
    if !allowed {
        if pointer.is_some_and(|p| xf.rect.contains(p)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        }
        return;
    }
    // Once the page is enhanced, the bar's buttons take over.
    if view.camera_done.is_some() {
        return;
    }
    // The tool starts with the corners on the current page.
    if view.camera.is_none() && page == view.current {
        view.camera = Some(CameraQuad::inset(page, xf.pw, xf.ph));
    }
    if resp.drag_started() || resp.clicked() {
        let pressed = origin.filter(|o| xf.rect.contains(*o));
        if pressed.is_some() && view.camera.is_none_or(|q| q.page != page) {
            view.camera = Some(CameraQuad::inset(page, xf.pw, xf.ph));
        }
        if resp.drag_started()
            && let (Some(o), Some(q)) = (pressed, view.camera.as_mut())
        {
            q.grab = (0..4)
                .map(|i| (i, screen(xf, q.corners[i]).distance(o)))
                .filter(|(_, d)| *d <= GRAB)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
        }
    }
    let Some(q) = view.camera.as_mut().filter(|q| q.page == page) else { return };
    let near = pointer.is_some_and(|p| q.corners.iter().any(|&c| screen(xf, c).distance(p) <= GRAB));
    if let Some(i) = q.grab {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(p) = pointer {
            let (x, y) = xf.screen_to_view(p);
            if let Some(c) = q.corners.get_mut(i) {
                *c = [x.clamp(0.0, xf.pw), y.clamp(0.0, xf.ph)];
            }
        }
        if resp.drag_stopped() {
            q.grab = None;
        }
    } else if near {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

/// The quadrilateral and its handles, with what will be cut away dimmed.
pub(crate) fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, view: &DocView) {
    let Some(q) = view.camera.filter(|q| q.page == page) else { return };
    let pts: Vec<Pos2> = q.corners.iter().map(|&c| screen(xf, c)).collect();
    // Dim the page outside the quadrilateral: a band from each edge out to the page's border.
    let r = xf.rect;
    let mut mesh = egui::Mesh::default();
    let dim = Color32::from_black_alpha(70);
    let outer = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    // Pair each quad corner with the page corner nearest to it, in order around the page.
    let start = (0..4).min_by(|&a, &b| pts[0].distance_sq(outer[a]).total_cmp(&pts[0].distance_sq(outer[b]))).unwrap_or(0);
    for i in 0..4 {
        let (a, b) = (pts[i], pts[(i + 1) % 4]);
        let (oa, ob) = (outer[(start + i) % 4], outer[(start + i + 1) % 4]);
        let base = mesh.vertices.len() as u32;
        for p in [a, b, ob, oa] {
            mesh.colored_vertex(p, dim);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::closed_line(pts.clone(), Stroke::new(1.5, BLUE)));
    for p in pts {
        painter.circle(p, 6.0, Color32::WHITE, Stroke::new(2.0, BLUE));
    }
}

/// The floating bar at the top right of the document area (where the find bar goes, clear of a
/// centred page's corners): Enhance page and Cancel; once the page is enhanced, rotate it either
/// way, choose its look (original, colour, greyscale, black and white) and clean-up level,
/// Crop pages, Organize pages, or Done. The instructions are a notice (`ocr.enhance_camera`).
pub(crate) fn bar(ctx: &egui::Context, view: &mut DocView, area: Rect, t: &Tokens, allowed: bool) {
    egui::Area::new(egui::Id::new("camera-bar"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(area.right_top() + vec2(-18.0, 12.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }) })
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // After Enhance page: put the page the right way up, trim it, or move on.
                        if let Some(done) = view.camera_done.as_mut() {
                            let mut again = false;
                            for (look, label) in [
                                (Look::Original, tl!("Original")),
                                (Look::Colour, tl!("Colour")),
                                (Look::Greyscale, tl!("Greyscale")),
                                (Look::BlackWhite, tl!("Black and white")),
                            ] {
                                if ui.add_enabled(allowed, egui::Button::selectable(done.look == look, label)).clicked() && done.look != look {
                                    done.look = look;
                                    again = true;
                                }
                            }
                            ui.separator();
                            ui.label(tl!("Clean-up"));
                            let slider = ui.add_enabled(
                                allowed && done.look != Look::Original,
                                egui::Slider::new(&mut done.level, 0..=crate::camera_ui::MAX_LEVEL).show_value(false),
                            );
                            // Enhance again once the slider is let go (or moved with the keys).
                            again |= slider.drag_stopped() || (slider.changed() && !slider.dragged());
                            if again && (done.look, done.level) != done.applied {
                                view.pending_action = Some(ViewAction::CameraLevel);
                            }
                            ui.separator();
                            let icon = |ui: &mut egui::Ui, name: &str, tip: &str| {
                                let r = ui.add_enabled(allowed, egui::Button::image(crate::icons::image(name, 18.0, t.text)).frame(false));
                                // Named for screen readers (and tests), not only on hover.
                                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, allowed, tip));
                                r.on_hover_text(tip).clicked()
                            };
                            if icon(ui, "rotate-ccw", tl!("Rotate counterclockwise")) {
                                view.pending_action = Some(ViewAction::CameraRotate { clockwise: false });
                            }
                            if icon(ui, "rotate-cw", tl!("Rotate clockwise")) {
                                view.pending_action = Some(ViewAction::CameraRotate { clockwise: true });
                            }
                            if crate::widgets::pill_button(ui, tl!("Crop pages"), false).clicked() {
                                view.pending_action = Some(ViewAction::CameraNext("page.crop"));
                            }
                            if crate::widgets::pill_button(ui, tl!("Organize pages"), false).clicked() {
                                view.pending_action = Some(ViewAction::CameraNext("page.organize"));
                            }
                            if crate::widgets::pill_button(ui, tl!("Done"), true).clicked() {
                                view.pending_action = Some(ViewAction::CancelCamera);
                            }
                            return;
                        }
                        let ready = allowed && view.camera.is_some();
                        if ui.add_enabled_ui(ready, |ui| crate::widgets::pill_button(ui, tl!("Enhance page"), true)).inner.clicked() {
                            view.pending_action = Some(ViewAction::EnhanceCamera);
                        }
                        if crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                            view.pending_action = Some(ViewAction::CancelCamera);
                        }
                    });
                });
        });
}
