//! View ▸ Zoom ▸ Pan & Zoom: a floating, movable overview window that displays
//! a miniature of the active document page with a red viewport frame representing
//! the visible area of the canvas.
//!
//! Dragging the red viewport frame or clicking anywhere on the miniature overview
//! pans the main document in real time. Dragging the resize handle or using the zoom
//! slider adjusts the magnification level of the main document canvas.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use pdfcraft_engine::DocId;

use crate::theme::Tokens;
use crate::{PdfCraftApp, icons};

/// Tracks the active pointer drag interaction within the Pan & Zoom overview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanZoomDrag {
    /// No pointer drag is currently in progress.
    None,
    /// Dragging the red navigation viewport frame to pan the document view.
    Box,
    /// Dragging the bottom-right corner handle to resize the viewport and adjust zoom.
    Handle,
}

/// State for the floating Pan & Zoom overview window.
#[derive(Clone, Debug)]
pub struct PanZoomState {
    /// 0-based page index currently displayed in the overview.
    pub page: usize,
    /// Whether the floating Pan & Zoom overview window is open.
    pub open: bool,
    /// Viewport size of the thumbnail overview area in screen points.
    pub viewport_size: Vec2,
    /// Current pointer drag interaction mode.
    pub drag_mode: PanZoomDrag,
    /// Active document ID for which the drag state is valid.
    pub active_doc: Option<DocId>,
}

impl PanZoomState {
    /// Creates a new Pan & Zoom window state for page `page`.
    pub fn new(page: usize) -> Self {
        Self { page, open: true, viewport_size: vec2(240.0, 240.0), drag_mode: PanZoomDrag::None, active_doc: None }
    }
}

/// Action to be applied to the active document view after the UI pass.
#[derive(Clone, Debug, PartialEq)]
enum PanZoomAction {
    /// Center the view on normalized coordinates `[u, v]` on `page`.
    CenterFraction(usize, f32, f32),
    /// Step zoom in (`true`) or out (`false`).
    ZoomStep(bool),
    /// Set explicit zoom level.
    SetZoom(f32),
    /// Navigate to a specific page index.
    GoToPage(usize),
}

/// Converts normalized displayed-page coordinates `(u, v)` to thumbnail-oriented coordinates.
fn display_to_thumbnail(rotation: u16, u: f32, v: f32) -> (f32, f32) {
    match rotation % 360 {
        90 => (v, 1.0 - u),
        180 => (1.0 - u, 1.0 - v),
        270 => (1.0 - v, u),
        _ => (u, v),
    }
}

/// Converts normalized thumbnail-oriented coordinates `(u, v)` to displayed-page coordinates.
fn thumbnail_to_display(rotation: u16, u: f32, v: f32) -> (f32, f32) {
    match rotation % 360 {
        90 => (1.0 - v, u),
        180 => (1.0 - u, 1.0 - v),
        270 => (v, 1.0 - u),
        _ => (u, v),
    }
}

/// Converts a normalized displayed-page rectangle `[u0, v0, u1, v1]` to a thumbnail-oriented rectangle.
fn display_rect_to_thumbnail(rotation: u16, rect: [f32; 4]) -> [f32; 4] {
    let corners = [
        display_to_thumbnail(rotation, rect[0], rect[1]),
        display_to_thumbnail(rotation, rect[2], rect[1]),
        display_to_thumbnail(rotation, rect[0], rect[3]),
        display_to_thumbnail(rotation, rect[2], rect[3]),
    ];
    let u0 = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let v0 = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let u1 = corners.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let v1 = corners.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    [u0, v0, u1, v1]
}

/// Computes the normalized page viewport bounds `[u0, v0, u1, v1]` currently visible on screen.
pub fn page_viewport_fraction(view: &crate::DocView, page_idx: usize, pw: f32, ph: f32) -> [f32; 4] {
    if let Some(r) = view.page_screen_rect(page_idx) {
        let vp = view.viewport_rect();
        if vp.is_positive() && r.is_positive() {
            let vis = r.intersect(vp);
            if vis.is_positive() {
                let u0 = ((vis.min.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                let v0 = ((vis.min.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                let u1 = ((vis.max.x - r.min.x) / r.width()).clamp(0.0, 1.0);
                let v1 = ((vis.max.y - r.min.y) / r.height()).clamp(0.0, 1.0);
                return [u0, v0, u1, v1];
            }
        }
    }
    // Fallback if the page is not on screen or viewport is uninitialized:
    let vp = view.viewport_rect();
    let vw = if vp.is_positive() { vp.width() } else { 800.0 };
    let vh = if vp.is_positive() { vp.height() } else { 600.0 };
    let pt = crate::canvas::PT;
    let page_w = (pw * pt * view.zoom).max(1.0);
    let page_h = (ph * pt * view.zoom).max(1.0);
    let u_span = (vw / page_w).clamp(0.05, 1.0);
    let v_span = (vh / page_h).clamp(0.05, 1.0);
    let u0 = (0.5 - u_span * 0.5).clamp(0.0, 1.0);
    let v0 = (0.5 - v_span * 0.5).clamp(0.0, 1.0);
    let u1 = (u0 + u_span).min(1.0);
    let v1 = (v0 + v_span).min(1.0);
    [u0, v0, u1, v1]
}

/// Renders the floating Pan & Zoom overview window if currently active.
pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(active) = app.active else {
        app.pan_zoom = None;
        return;
    };
    let Some(pan_zoom) = &mut app.pan_zoom else {
        return;
    };
    if !pan_zoom.open {
        app.pan_zoom = None;
        return;
    };

    let Some(view) = app.views.get(active) else {
        app.pan_zoom = None;
        return;
    };
    let Some(doc) = app.session.get(view.id) else {
        app.pan_zoom = None;
        return;
    };
    if doc.info.pages.is_empty() {
        return;
    }

    let t = Tokens::get(ctx);
    let total_pages = doc.info.pages.len();
    let document_changed = pan_zoom.active_doc != Some(view.id);
    pan_zoom.active_doc = Some(view.id);
    pan_zoom.page = view.current.min(total_pages.saturating_sub(1));
    let page_idx = pan_zoom.page;
    let (pw, ph) = doc.info.pages.get(page_idx).map_or((612.0, 792.0), |p| (p.width, p.height));

    let mut is_open = pan_zoom.open;
    let mut action: Option<PanZoomAction> = None;
    let mut next_drag_mode = if document_changed { PanZoomDrag::None } else { pan_zoom.drag_mode };

    egui::Window::new(tl!("Pan & Zoom"))
        .id(egui::Id::new("pan_zoom_overview_window"))
        .open(&mut is_open)
        .collapsible(false)
        .resizable(true)
        .default_pos(pos2(120.0, 140.0))
        .default_size(vec2(280.0, 340.0))
        .min_size(vec2(220.0, 240.0))
        .show(ctx, |ui| {
            // Page navigation row when document has multiple pages.
            if total_pages > 1 {
                ui.horizontal(|ui| {
                    if icons::button(ui, "chevron-left", 22.0, false, tl!("Previous page")).clicked() && page_idx > 0 {
                        action = Some(PanZoomAction::GoToPage(page_idx - 1));
                    }
                    let label = crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(page_idx + 1).to_string()), ("n", &total_pages.to_string())]);
                    ui.label(egui::RichText::new(label).small().color(t.text_muted));
                    if icons::button(ui, "chevron-right", 22.0, false, tl!("Next page")).clicked() && page_idx + 1 < total_pages {
                        action = Some(PanZoomAction::GoToPage(page_idx + 1));
                    }
                });
                ui.add_space(2.0);
            }

            // Thumbnail overview area.
            let avail = ui.available_size_before_wrap();
            let preview_box = vec2(avail.x, (avail.y - 44.0).max(100.0));
            let scale = (preview_box.x / pw).min(preview_box.y / ph).clamp(0.01, 10.0);
            let thumb_size = vec2((pw * scale).max(10.0), (ph * scale).max(10.0));

            let (thumb_rect, resp) = ui.allocate_exact_size(thumb_size, Sense::click_and_drag());

            // Paint paper background and cached thumbnail.
            ui.painter().rect_filled(thumb_rect, CornerRadius::same(2), Color32::WHITE);
            if let Some(tex) = view.thumb(page_idx) {
                ui.painter().image(tex.id(), thumb_rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(thumb_rect, CornerRadius::same(2), Stroke::new(1.0, t.border), egui::StrokeKind::Outside);

            // Compute current viewport navigation box bounds on the thumbnail.
            let displayed = page_viewport_fraction(view, page_idx, pw, ph);
            let [u0, v0, u1, v1] = display_rect_to_thumbnail(view.rotation, displayed);
            let nav_min = thumb_rect.min + vec2(u0 * thumb_rect.width(), v0 * thumb_rect.height());
            let nav_max = thumb_rect.min + vec2(u1 * thumb_rect.width(), v1 * thumb_rect.height());
            let nav_box = Rect::from_min_max(nav_min, nav_max);

            // Paint red navigation viewport box.
            let red = Color32::from_rgb(225, 29, 72);
            let red_wash = Color32::from_rgba_unmultiplied(225, 29, 72, 35);
            ui.painter().rect_filled(nav_box, CornerRadius::ZERO, red_wash);
            ui.painter().rect_stroke(nav_box, CornerRadius::ZERO, Stroke::new(1.5, red), egui::StrokeKind::Inside);

            // Corner resize handle at bottom-right of the viewport box.
            let handle_rect = Rect::from_center_size(nav_box.max, vec2(8.0, 8.0));
            ui.painter().rect_filled(handle_rect, CornerRadius::same(1), red);

            // Pointer cursor styling and interaction handling.
            let pointer_pos = resp.interact_pointer_pos();
            if let Some(pos) = pointer_pos {
                if handle_rect.expand(3.0).contains(pos) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                } else if nav_box.contains(pos) {
                    ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
                } else {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            }

            // Drag start: determine whether the handle, the viewport box, or outside was grabbed.
            if resp.drag_started()
                && let Some(pos) = pointer_pos
            {
                if handle_rect.expand(3.0).contains(pos) {
                    next_drag_mode = PanZoomDrag::Handle;
                } else if nav_box.contains(pos) {
                    next_drag_mode = PanZoomDrag::Box;
                } else {
                    next_drag_mode = PanZoomDrag::None;
                }
            }

            // Drag update.
            if resp.dragged() {
                match next_drag_mode {
                    PanZoomDrag::Box => {
                        let delta = resp.drag_delta();
                        let du = delta.x / thumb_rect.width().max(1.0);
                        let dv = delta.y / thumb_rect.height().max(1.0);
                        let cu = ((u0 + u1) * 0.5 + du).clamp(0.0, 1.0);
                        let cv = ((v0 + v1) * 0.5 + dv).clamp(0.0, 1.0);
                        let (cu, cv) = thumbnail_to_display(view.rotation, cu, cv);
                        action = Some(PanZoomAction::CenterFraction(page_idx, cu, cv));
                    }
                    PanZoomDrag::Handle => {
                        let delta = resp.drag_delta();
                        let cur_diag = (nav_box.width().powi(2) + nav_box.height().powi(2)).sqrt().max(1.0);
                        let min_w = (nav_box.width() * 0.5).clamp(0.5, 4.0);
                        let min_h = (nav_box.height() * 0.5).clamp(0.5, 4.0);
                        let new_w = (nav_box.width() + delta.x).max(min_w);
                        let new_h = (nav_box.height() + delta.y).max(min_h);
                        let new_diag = (new_w.powi(2) + new_h.powi(2)).sqrt().max(1.0);
                        let scale = cur_diag / new_diag;
                        let new_zoom = (view.zoom * scale).clamp(0.08, 64.0);
                        action = Some(PanZoomAction::SetZoom(new_zoom));
                    }
                    PanZoomDrag::None => {}
                }
            }

            // Click outside nav box: jump center to clicked position.
            if resp.clicked()
                && let Some(pos) = pointer_pos
                && !handle_rect.expand(3.0).contains(pos)
            {
                let u = ((pos.x - thumb_rect.min.x) / thumb_rect.width().max(1.0)).clamp(0.0, 1.0);
                let v = ((pos.y - thumb_rect.min.y) / thumb_rect.height().max(1.0)).clamp(0.0, 1.0);
                let (u, v) = thumbnail_to_display(view.rotation, u, v);
                action = Some(PanZoomAction::CenterFraction(page_idx, u, v));
            }

            if resp.drag_stopped() {
                next_drag_mode = PanZoomDrag::None;
            }

            // Bottom toolbar: Zoom controls.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if icons::button(ui, "zoom-out", 24.0, false, tl!("Zoom out")).clicked() {
                    action = Some(PanZoomAction::ZoomStep(false));
                }
                let mut pct = (view.zoom * 100.0).round();
                let drag_val = ui.add(egui::DragValue::new(&mut pct).speed(1.0).range(8.0..=6400.0).suffix("%"));
                if drag_val.changed() {
                    action = Some(PanZoomAction::SetZoom(pct / 100.0));
                }
                if icons::button(ui, "zoom-in", 24.0, false, tl!("Zoom in")).clicked() {
                    action = Some(PanZoomAction::ZoomStep(true));
                }
            });
        });

    // Update pan zoom state with window open and drag status.
    if let Some(pz) = &mut app.pan_zoom {
        pz.open = is_open;
        pz.drag_mode = next_drag_mode;
    }

    // Apply any interaction action to the active document view.
    if let Some(act) = action
        && let Some(v) = app.views.get_mut(active)
    {
        match act {
            PanZoomAction::CenterFraction(p, u, v_coord) => {
                v.center_on_fraction(p, u, v_coord);
            }
            PanZoomAction::ZoomStep(up) => {
                v.zoom_step(up);
            }
            PanZoomAction::SetZoom(zoom) => {
                v.set_zoom(zoom);
            }
            PanZoomAction::GoToPage(p) => {
                v.go_to_page(p);
            }
        }
    }

    // Keep page thumbnail demanded for rendering.
    if let Some(v) = app.views.get_mut(active) {
        v.need_thumbnail(page_idx, true);
    }

    if !app.pan_zoom.as_ref().is_some_and(|pz| pz.open) {
        app.pan_zoom = None;
    }
}
