//! View ▸ Zoom ▸ Loupe tool: a floating, movable window that displays a magnified view
//! of a rectangular aperture on the active document page, with interactive canvas positioning
//! and zoom controls (100%–800%).

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use pdfcraft_engine::DocId;
use pdfcraft_render::{RenderConfig, RenderPool, RenderRequest, RequestKind, Tile};

use crate::canvas::{PT, PageXform};
use crate::theme::Tokens;
use crate::{PdfCraftApp, QuickTool, icons};

/// Cache key for tracking loupe aperture, zoom, and document generation changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoupeCacheKey {
    /// Document identifier.
    pub doc: DocId,
    /// Document edit generation.
    pub generation: u64,
    /// 0-based page index.
    pub page: usize,
    /// Quantized tile bounds: `[x, y, w, h]`.
    pub tile_bounds: [u32; 4],
    /// Quantized scale bits for float comparison.
    pub scale_bits: u32,
}

/// State for the Loupe magnifier tool window and canvas aperture.
pub struct LoupeState {
    /// 0-based page index currently being magnified.
    pub page: usize,
    /// Normalized center on the page `[u, v]` in the range `[0.0..1.0, 0.0..1.0]`.
    pub center: [f32; 2],
    /// Magnification factor (e.g. 1.0 = 100%, 2.0 = 200%, max 8.0 = 800%). Default: 2.0.
    pub zoom: f32,
    /// Whether the floating Loupe Tool window is open.
    pub open: bool,
    /// Viewport size of the loupe image area in screen points.
    pub viewport_size: Vec2,
    /// Single-worker render pool for off-thread tile rendering.
    pub(crate) pool: Option<(DocId, u64, RenderPool)>,
    /// Last requested render tag.
    pub(crate) requested_tag: u64,
    /// Last received render tag.
    pub(crate) received_tag: u64,
    /// Current displayed texture (persists until replacement tile is ready).
    pub(crate) texture: Option<egui::TextureHandle>,
    /// Last requested cache key.
    pub(crate) last_key: Option<LoupeCacheKey>,
}

impl std::fmt::Debug for LoupeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoupeState")
            .field("page", &self.page)
            .field("center", &self.center)
            .field("zoom", &self.zoom)
            .field("open", &self.open)
            .field("viewport_size", &self.viewport_size)
            .field("requested_tag", &self.requested_tag)
            .field("received_tag", &self.received_tag)
            .finish()
    }
}

impl LoupeState {
    /// Creates a new Loupe tool state centered on page `page` with default 200% magnification.
    pub fn new(page: usize) -> Self {
        Self {
            page,
            center: [0.5, 0.5],
            zoom: 2.0,
            open: true,
            viewport_size: vec2(280.0, 180.0),
            pool: None,
            requested_tag: 0,
            received_tag: 0,
            texture: None,
            last_key: None,
        }
    }

    /// Sets the magnification factor clamped to the 100%–800% (1.0–8.0) range.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(1.0, 8.0);
    }

    /// Steps the zoom in or out in discrete increments.
    pub fn zoom_step(&mut self, zoom_in: bool) {
        let step = if self.zoom < 2.0 {
            0.25
        } else if self.zoom < 4.0 {
            0.5
        } else {
            1.0
        };
        if zoom_in {
            self.set_zoom(self.zoom + step);
        } else {
            self.set_zoom(self.zoom - step);
        }
    }

    /// Sets the target aperture center, clamped to `[0.0, 1.0]`.
    pub fn set_center(&mut self, page: usize, center: [f32; 2]) {
        self.page = page;
        self.center = [center[0].clamp(0.0, 1.0), center[1].clamp(0.0, 1.0)];
    }
}

/// Computes the page view points `[x0, y0, x1, y1]` corresponding to the loupe aperture.
pub(crate) fn aperture_view_rect(center: [f32; 2], pw: f32, ph: f32, loupe_zoom: f32, view_zoom: f32, viewport_size: Vec2) -> [f32; 4] {
    let scale = (loupe_zoom * view_zoom * PT).max(0.1);
    let ap_w = (viewport_size.x / scale).clamp(1.0, pw.max(1.0));
    let ap_h = (viewport_size.y / scale).clamp(1.0, ph.max(1.0));
    let nu = ap_w / pw.max(1.0);
    let nv = ap_h / ph.max(1.0);

    let u0 = (center[0] - nu * 0.5).clamp(0.0, (1.0 - nu).max(0.0));
    let v0 = (center[1] - nv * 0.5).clamp(0.0, (1.0 - nv).max(0.0));
    let u1 = (u0 + nu).min(1.0);
    let v1 = (v0 + nv).min(1.0);

    [u0 * pw, v0 * ph, u1 * pw, v1 * ph]
}

/// Paints the loupe aperture frame and focal crosshair on the canvas.
pub(crate) fn paint_canvas_aperture(painter: &egui::Painter, xf: &PageXform, center: [f32; 2], loupe_zoom: f32, view_zoom: f32, viewport_size: Vec2) {
    let vr = aperture_view_rect(center, xf.pw, xf.ph, loupe_zoom, view_zoom, viewport_size);
    let rect = xf.view_rect(vr);

    // Blue frame and translucent tint matching Acrobat's loupe aperture.
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x14, 0x73, 0xE6, 25));
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(2.0, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);

    // Focal crosshair at the aperture center.
    let c = rect.center();
    painter.line_segment([pos2(c.x - 5.0, c.y), pos2(c.x + 5.0, c.y)], Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)));
    painter.line_segment([pos2(c.x, c.y - 5.0), pos2(c.x, c.y + 5.0)], Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)));
}

/// Handles pointer clicks and drags on the canvas when the Loupe tool is active.
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, page: usize) -> Option<(usize, [f32; 2])> {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if (resp.clicked() || resp.dragged())
        && let Some(p) = pointer.filter(|p| xf.rect.contains(*p))
    {
        let (u, v) = xf.screen_to_norm(p);
        Some((page, [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)]))
    } else {
        None
    }
}

/// Displays the floating Loupe Tool window.
pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(active) = app.active else {
        app.loupe = None;
        if app.quick_tool == QuickTool::Loupe {
            app.quick_tool = QuickTool::Select;
        }
        return;
    };
    let Some(loupe) = &mut app.loupe else {
        return;
    };
    if !loupe.open {
        app.loupe = None;
        if app.quick_tool == QuickTool::Loupe {
            app.quick_tool = QuickTool::Select;
        }
        return;
    }

    let Some(view) = app.views.get(active) else {
        app.loupe = None;
        if app.quick_tool == QuickTool::Loupe {
            app.quick_tool = QuickTool::Select;
        }
        return;
    };
    let Some(doc) = app.session.get(view.id) else {
        app.loupe = None;
        if app.quick_tool == QuickTool::Loupe {
            app.quick_tool = QuickTool::Select;
        }
        return;
    };

    let t = Tokens::get(ctx);
    let paper = app
        .session
        .reading_colors(view.id)
        .and_then(|c| c.params)
        .map_or(Color32::WHITE, |p| Color32::from_rgb(p.background[0], p.background[1], p.background[2]));
    let mut is_open = loupe.open;
    let mut drag_delta = None;

    egui::Window::new(tl!("Loupe Tool"))
        .id(egui::Id::new("loupe_tool_window"))
        .open(&mut is_open)
        .default_size(vec2(320.0, 240.0))
        .min_size(vec2(220.0, 160.0))
        .resizable(true)
        .show(ctx, |ui| {
            // Top toolbar: Zoom controls.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Zoom")).color(t.text));
                if icons::button(ui, "minus", 22.0, false, tl!("Zoom out")).clicked() {
                    loupe.zoom_step(false);
                }
                ui.add(egui::Slider::new(&mut loupe.zoom, 1.0..=8.0).show_value(false).step_by(0.25));
                if icons::button(ui, "plus", 22.0, false, tl!("Zoom in")).clicked() {
                    loupe.zoom_step(true);
                }
                ui.label(egui::RichText::new(format!("{:.0}%", loupe.zoom * 100.0)).color(t.text_muted));
            });

            ui.separator();

            // Content area: Viewport showing the magnified page content.
            let avail = ui.available_size();
            loupe.viewport_size = vec2(avail.x.max(80.0), avail.y.max(60.0));

            let page_idx = loupe.page.min(doc.info.pages.len().saturating_sub(1));
            let (pw, ph) = doc.info.pages.get(page_idx).map_or((612.0, 792.0), |p| (p.width, p.height));
            let vr = aperture_view_rect(loupe.center, pw, ph, loupe.zoom, view.zoom, loupe.viewport_size);

            let ppp = ctx.pixels_per_point();
            let effective_scale = (loupe.zoom * view.zoom * PT * ppp).max(0.1);
            let actual_ap_w = (vr[2] - vr[0]).max(1.0);
            let actual_ap_h = (vr[3] - vr[1]).max(1.0);
            let max_scale = (2048.0 / actual_ap_w.max(actual_ap_h)).max(0.1);
            let scale = effective_scale.min(max_scale);

            let tile = Tile {
                x: (vr[0] * scale).floor().max(0.0) as u32,
                y: (vr[1] * scale).floor().max(0.0) as u32,
                w: (actual_ap_w * scale).ceil().clamp(1.0, 2048.0) as u32,
                h: (actual_ap_h * scale).ceil().clamp(1.0, 2048.0) as u32,
            };

            let key = LoupeCacheKey {
                doc: view.id,
                generation: doc.edit_generation(),
                page: page_idx,
                tile_bounds: [tile.x, tile.y, tile.w, tile.h],
                scale_bits: scale.to_bits(),
            };

            // Ensure background render pool matches current document and edit generation.
            let doc_gen = doc.edit_generation();
            if loupe.pool.as_ref().is_none_or(|(id, generation, _)| *id != view.id || *generation != doc_gen) {
                let config = RenderConfig {
                    password: doc.password.as_deref().map(std::sync::Arc::from),
                    reading_colors: app.reading_colors,
                    ..RenderConfig::default()
                };
                let pool = RenderPool::new(doc.bytes.clone(), 1, config).without_replacements();
                loupe.pool = Some((view.id, doc_gen, pool));
                loupe.last_key = None;
            }

            // Receive any completed tile results from the background worker.
            if let Some((_, _, pool)) = &loupe.pool {
                while let Some(out) = pool.try_recv() {
                    if out.request.tag == loupe.requested_tag {
                        loupe.received_tag = out.request.tag;
                        if out.error.is_none() && out.width > 0 && out.height > 0 {
                            let img = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
                            loupe.texture = Some(ctx.load_texture("loupe-magnified", img, egui::TextureOptions::LINEAR));
                        }
                    }
                }
            }

            // Submit a fresh tile request when the aperture, page, zoom, or scale changes.
            if loupe.last_key.as_ref() != Some(&key) {
                loupe.last_key = Some(key);
                loupe.requested_tag = loupe.requested_tag.wrapping_add(1);
                let tag = loupe.requested_tag;
                if let Some((_, _, pool)) = &loupe.pool {
                    pool.set_queue(vec![RenderRequest { page: page_idx, kind: RequestKind::Pixels, tile: Some(tile), scale, tag, preview: false }]);
                }
            }

            // Request next frame update while awaiting background render.
            if loupe.received_tag != loupe.requested_tag {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }

            let (rect, resp) = ui.allocate_exact_size(loupe.viewport_size, egui::Sense::drag());
            ui.painter().rect_filled(rect, CornerRadius::same(4), paper);
            if let Some(tex) = &loupe.texture {
                let shown = vec2(actual_ap_w, actual_ap_h) * (loupe.zoom * view.zoom * PT);
                let img_rect = Rect::from_center_size(rect.center(), shown);
                ui.painter().image(tex.id(), img_rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);

            if resp.dragged() {
                drag_delta = Some(resp.drag_delta());
            }
        });

    if let Some(loupe) = &mut app.loupe {
        loupe.open = is_open;
    }

    if let Some(delta) = drag_delta
        && let Some(view) = app.views.get(active)
        && let Some(doc) = app.session.get(view.id)
        && let Some(loupe) = &mut app.loupe
    {
        let page_idx = loupe.page.min(doc.info.pages.len().saturating_sub(1));
        let (pw, ph) = doc.info.pages.get(page_idx).map_or((612.0, 792.0), |p| (p.width, p.height));
        let delta_u = -delta.x / (pw * loupe.zoom * view.zoom * PT).max(1.0);
        let delta_v = -delta.y / (ph * loupe.zoom * view.zoom * PT).max(1.0);
        loupe.center[0] = (loupe.center[0] + delta_u).clamp(0.0, 1.0);
        loupe.center[1] = (loupe.center[1] + delta_v).clamp(0.0, 1.0);
    }

    if !app.loupe.as_ref().is_some_and(|l| l.open) {
        app.loupe = None;
        if app.quick_tool == QuickTool::Loupe {
            app.quick_tool = QuickTool::Select;
        }
    }
}
