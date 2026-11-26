//! Interactive Barcode & QR Code Scanner HUD (L7 UI).
//!
//! Provides automated page detection, unified compact HUD (data of one code at a time),
//! multi-code navigation (◀ / ▶ and arrow keys), 0.5px blue highlight box, clipboard copy,
//! and pixel-perfect 100% scale re-embedding.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use craft::behavior::IconId;
use craft::theme::TypeRole;
use craft::theme::variant::Variant;
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, clicked};
use craft::ui::display::{Shade, Text};
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Env;
use egui::{Color32, CornerRadius, Margin, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::barcode_scan::{CodeKind, ScannedCode};
use pdfcraft_engine::{Document, Edit};
use pdfcraft_render::DocInfo;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::canvas::DocView;

struct HudPalette {
    card_bg: Color32,
    item_bg: Color32,
    code_bg: Color32,
    border: Color32,
    accent: Color32,
    good: Color32,
    warn: Color32,
    shadow: Color32,
}

impl HudPalette {
    fn from_ui(ui: &egui::Ui) -> Self {
        let to_c32 = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        let colors = &Env::of(ui).tokens.color;
        let card_bg = to_c32(colors.surface.raised.0);
        let item_bg = to_c32(colors.surface.canvas.0);
        let dark = card_bg.r() < 128;
        Self {
            card_bg,
            item_bg,
            code_bg: if dark { card_bg } else { item_bg },
            border: to_c32(colors.line.border.0),
            accent: to_c32(colors.accent.solid.0),
            good: to_c32(colors.status.good.solid.0),
            warn: to_c32(colors.status.warn.solid.0),
            shadow: Color32::from_black_alpha(if dark { 140 } else { 36 }),
        }
    }
}

/// Persistent HUD state for barcode / QR detection and interactive editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarcodeHudState {
    /// Cached detected codes for the active page.
    pub scanned_codes: Vec<ScannedCode>,
    /// Cached detected codes per 0-based page index so clicks work on any visible page.
    pub page_codes: BTreeMap<usize, Vec<ScannedCode>>,
    /// Edit generation that populated `page_codes`.
    pub cache_generation: Option<u64>,
    /// Index in `scanned_codes` currently focused by the HUD.
    pub active_index: usize,
    /// Whether HUD is currently open and visible.
    pub open: bool,
    /// Compatibility alias for `open`.
    pub panel_open: bool,
    /// In-progress text string when editing a code.
    pub edit_buffer: Option<String>,
    /// Selected symbology when re-embedding / replacing a code.
    pub reembed_kind: Option<pdfcraft_engine::barcode_scan::CodeKind>,
    /// Selected symbology when generating a new code.
    pub generate_kind: pdfcraft_engine::barcode_scan::CodeKind,
    /// Feedback banner timestamp: `(message, expiry_time)`.
    pub toast: Option<(String, f64)>,
    /// Text or URL payload for generating a new QR code.
    pub qr_input: String,
    /// The page that was last scanned for codes.
    pub last_scanned_page: Option<usize>,
    /// Target rectangle coordinates to track and refocus after rescanning.
    pub target_rect: Option<[f64; 4]>,
    /// Queued PNG download `(filename, bytes)` for the web shell to hand to the browser host.
    pub pending_download: Option<(String, Arc<Vec<u8>>)>,
}

impl BarcodeHudState {
    /// Find the index of a code whose bounding rectangle matches `target`.
    pub fn find_code_matching_rect(&self, target: [f64; 4]) -> Option<usize> {
        self.scanned_codes
            .iter()
            .position(|c| {
                (c.rect[0] - target[0]).abs() < 5.0
                    && (c.rect[1] - target[1]).abs() < 5.0
                    && (c.rect[2] - target[2]).abs() < 5.0
                    && (c.rect[3] - target[3]).abs() < 5.0
            })
            .or_else(|| {
                let target_cx = (target[0] + target[2]) * 0.5;
                let target_cy = (target[1] + target[3]) * 0.5;
                self.scanned_codes
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        let acx = (a.rect[0] + a.rect[2]) * 0.5;
                        let acy = (a.rect[1] + a.rect[3]) * 0.5;
                        let bcx = (b.rect[0] + b.rect[2]) * 0.5;
                        let bcy = (b.rect[1] + b.rect[3]) * 0.5;
                        let dist_a = (acx - target_cx).hypot(acy - target_cy);
                        let dist_b = (bcx - target_cx).hypot(bcy - target_cy);
                        dist_a.partial_cmp(&dist_b).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(idx, _)| idx)
            })
    }

    /// Populate or update scanned codes for a page with explicit open control.
    pub fn update_codes_conditional(&mut self, codes: Vec<ScannedCode>, keep_open: bool) {
        let prev_code_identity = self.active_code().map(|c| (c.page, c.id, c.text.clone(), c.rect));
        let target_rect = self.target_rect.take().or_else(|| self.active_code().map(|c| c.rect));
        self.scanned_codes = codes;
        if let Some(target) = target_rect {
            if let Some(pos) = self.find_code_matching_rect(target) {
                self.active_index = pos;
            } else if self.active_index >= self.scanned_codes.len() {
                self.active_index = 0;
            }
        } else if self.active_index >= self.scanned_codes.len() {
            self.active_index = 0;
        }
        let new_code_identity = self.active_code().map(|c| (c.page, c.id, c.text.clone(), c.rect));
        if prev_code_identity != new_code_identity {
            self.edit_buffer = None;
            if let Some(active) = self.active_code() {
                self.reembed_kind = Some(active.kind);
            } else {
                self.reembed_kind = None;
            }
        }
        self.open = keep_open;
        self.panel_open = keep_open;
    }

    /// Populate or update scanned codes for a page.
    pub fn update_codes(&mut self, codes: Vec<ScannedCode>) {
        let has_codes = !codes.is_empty();
        self.update_codes_conditional(codes, has_codes);
    }

    /// Step to previous code on the page.
    pub fn prev_code(&mut self) {
        if !self.scanned_codes.is_empty() {
            if self.active_index == 0 {
                self.active_index = self.scanned_codes.len().saturating_sub(1);
            } else {
                self.active_index = self.active_index.saturating_sub(1);
            }
            self.edit_buffer = None;
        }
    }

    /// Step to next code on the page.
    pub fn next_code(&mut self) {
        if !self.scanned_codes.is_empty() {
            self.active_index = (self.active_index.saturating_add(1)) % self.scanned_codes.len();
            self.edit_buffer = None;
        }
    }

    /// Current active code.
    pub fn active_code(&self) -> Option<&ScannedCode> {
        self.scanned_codes.get(self.active_index)
    }

    /// Cached detected codes for `page`, if scanned at the current edit generation.
    pub fn codes_for_page(&self, page: usize) -> Option<&[ScannedCode]> {
        self.page_codes.get(&page).map(Vec::as_slice)
    }

    /// Focus the HUD on `code_index` within `page`'s cached scanned codes and open the panel.
    pub fn focus_page_code(&mut self, page: usize, code_index: usize) {
        if let Some(codes) = self.page_codes.get(&page).cloned() {
            self.scanned_codes = codes;
            self.active_index = code_index.min(self.scanned_codes.len().saturating_sub(1));
            self.last_scanned_page = Some(page);
            self.edit_buffer = None;
            self.reembed_kind = self.active_code().map(|c| c.kind);
            self.open = true;
            self.panel_open = true;
        }
    }

    /// Show feedback toast in HUD.
    pub fn show_toast(&mut self, msg: String, now: f64) {
        self.toast = Some((msg, now + 3.0));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn get_downloads_dir() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            let p = std::path::PathBuf::from(profile).join("Downloads");
            if p.exists() {
                return p;
            }
        }
    }
    #[cfg(unix)]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = std::path::PathBuf::from(home).join("Downloads");
            if p.exists() {
                return p;
            }
        }
    }
    std::env::temp_dir()
}

fn safe_rect_and_pixels(
    rect: [f64; 4],
    pixel_size: [u32; 2],
    kind: pdfcraft_engine::barcode_scan::CodeKind,
    old_text: Option<&str>,
    new_text: Option<&str>,
) -> ([f64; 4], [f64; 4], u32, u32) {
    let min_x = if rect[0].is_finite() && rect[2].is_finite() { rect[0].min(rect[2]) } else { 0.0 };
    let mut max_x = if rect[0].is_finite() && rect[2].is_finite() { rect[0].max(rect[2]) } else { 100.0 };
    let min_y = if rect[1].is_finite() && rect[3].is_finite() { rect[1].min(rect[3]) } else { 0.0 };
    let max_y = if rect[1].is_finite() && rect[3].is_finite() { rect[1].max(rect[3]) } else { 50.0 };

    let mut orig_w = (max_x - min_x).max(15.0);
    let orig_h = (max_y - min_y).max(15.0);

    let rect_w = orig_w.round().max(16.0) as u32;
    let rect_h = orig_h.round().max(16.0) as u32;
    let mut pw = if pixel_size[0] > 0 { pixel_size[0].max(16) } else { rect_w };
    let ph = if pixel_size[1] > 0 { pixel_size[1].max(16) } else { rect_h };

    // When payload increases on 1D barcodes (e.g. from 4 to 5 digits), keeping module width requires
    // expanding to the right: anchor min_x and grow max_x and pixel width pw proportionally.
    if kind != pdfcraft_engine::barcode_scan::CodeKind::QrCode
        && let (Some(old_t), Some(new_t)) = (old_text, new_text)
    {
        let old_len = old_t.chars().count();
        let new_len = new_t.chars().count();
        if new_len > old_len && old_len > 0 {
            let scale_x = (new_len as f64) / (old_len as f64);
            orig_w = (orig_w * scale_x).max(orig_w);
            max_x = min_x + orig_w;
            pw = (pw as f64 * scale_x).round().max(pw as f64) as u32;
        }
    }

    // Exact normalized safe rect without inflation for re-embedding new code
    // Center a square inside original rect for QR code kind, preserving rectangular footprint for 1D barcodes.
    let new_image_rect = if kind == pdfcraft_engine::barcode_scan::CodeKind::QrCode {
        let w = (max_x - min_x).abs();
        let h = (max_y - min_y).abs();
        let side = w.min(h);
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        [cx - side / 2.0, cy - side / 2.0, cx + side / 2.0, cy + side / 2.0]
    } else {
        [min_x, min_y, max_x, max_y]
    };

    // Inflate whiteout_rect by 3 PDF points on all sides to completely cover anti-aliased edges
    let min_x = (min_x - 3.0).max(0.0);
    let max_x = max_x + 3.0;
    let min_y = (min_y - 3.0).max(0.0);
    let max_y = max_y + 3.0;
    let whiteout_rect = [min_x, min_y, max_x, max_y];

    let (pw, ph) = if kind == pdfcraft_engine::barcode_scan::CodeKind::QrCode {
        let side = pw.max(ph).max(64);
        (side, side)
    } else {
        let h = if ph >= pw { (pw / 3).max(24) } else { ph.max(24) };
        (pw.max(64), h)
    };
    (whiteout_rect, new_image_rect, pw, ph)
}

/// A scanned user-space rectangle in the display page's space, which `Edit::AddImage` uses.
/// The CropBox origin and `/Rotate` move the display page away from user space.
fn display_rect(rect: [f64; 4], page: Option<&DocInfo>, page_index: usize) -> [f64; 4] {
    let Some(page) = page.and_then(|info| info.pages.get(page_index)) else { return rect };
    let corners = [[rect[0], rect[1]], [rect[2], rect[1]], [rect[2], rect[3]], [rect[0], rect[3]]].map(|[x, y]| {
        let [vx, vy] = page.user_to_view(x as f32, y as f32);
        [vx as f64, page.height as f64 - vy as f64]
    });
    [
        corners.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
        corners.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
        corners.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max),
        corners.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// Unified Floating HUD widget.
pub struct BarcodeScannerHud;

struct FloatingMenuAnchor<'a> {
    current_page: usize,
    page_xf: Option<&'a crate::canvas::PageXform>,
    viewport: Rect,
    view_id: u64,
}

fn render_floating_menu(
    ui: &mut egui::Ui,
    state: &BarcodeHudState,
    anchor: FloatingMenuAnchor<'_>,
    info: &DocInfo,
    can_modify: bool,
) -> (Option<ScannedCode>, Option<ScannedCode>) {
    let mut delete_code = None;
    let mut download_code = None;

    let FloatingMenuAnchor { current_page, page_xf, viewport, view_id } = anchor;

    if let Some(active) = state.active_code()
        && active.page == current_page
        && let Some(xf) = page_xf
    {
        let code_rect = xf.user_rect(info, current_page, active.rect.map(|v| v as f32));
        let min_x = viewport.left() + 10.0;
        let max_x = (viewport.right() - 150.0).max(min_x);
        let menu_pos = pos2(
            (code_rect.center().x - 70.0).clamp(min_x, max_x),
            if code_rect.bottom() + 38.0 <= viewport.bottom() {
                code_rect.bottom() + 4.0
            } else {
                (code_rect.top() - 34.0).max(viewport.top() + 10.0)
            },
        );
        let palette = HudPalette::from_ui(ui);
        egui::Area::new(egui::Id::unique(("barcode-context-menu", view_id, state.active_index)))
            .order(egui::Order::Foreground)
            .fixed_pos(menu_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(palette.card_bg)
                    .stroke(Stroke::new(1.0, palette.accent))
                    .corner_radius(CornerRadius::same(6))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 10, spread: 0, color: palette.shadow })
                    .inner_margin(Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if can_modify
                                && clicked(
                                    &Button::new(tl!("Delete"))
                                        .icon(IconId::new("trash-2"))
                                        .variant(Variant::GHOST)
                                        .tooltip(tl!("Delete barcode and whiteout region"))
                                        .show(ui),
                                )
                            {
                                delete_code = Some(active.clone());
                            }
                            if clicked(
                                &Button::new(tl!("Download"))
                                    .icon(IconId::new("file-down"))
                                    .variant(Variant::GHOST)
                                    .tooltip(tl!("Export barcode PNG to Downloads folder"))
                                    .show(ui),
                            ) {
                                download_code = Some(active.clone());
                            }
                        });
                    });
            });
    }

    (delete_code, download_code)
}

impl BarcodeScannerHud {
    /// Paint highlight boxes over detected barcodes/QR codes on `page`.
    pub fn paint_page_highlights(ui: &egui::Ui, clip: Rect, xf: &crate::canvas::PageXform, info: &DocInfo, state: &BarcodeHudState, page: usize) {
        let codes = state.codes_for_page(page).unwrap_or(state.scanned_codes.as_slice());
        let active_key = state.active_code().map(|c| (c.page, c.id, c.rect));
        let painter = ui.painter().with_clip_rect(clip);
        let accent = {
            let c = Env::of(ui).tokens.color.accent.solid.0;
            Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
        };
        for code in codes.iter().filter(|c| c.page == page) {
            let rect = xf.user_rect(info, page, code.rect.map(|v| v as f32));
            if !rect.is_positive() {
                continue;
            }
            let is_active = active_key == Some((code.page, code.id, code.rect));
            let stroke = if is_active { Stroke::new(1.5, accent) } else { Stroke::new(1.0, accent.gamma_multiply(0.55)) };
            painter.rect_stroke(rect, CornerRadius::same(2), stroke, egui::StrokeKind::Outside);
            if is_active {
                painter.rect_filled(rect, CornerRadius::same(2), accent.gamma_multiply(0.08));
            }
        }
    }

    /// Reactive Auto-Page Tracking hook:
    ///
    /// Scans the current page and any other visible pages so interactive canvas clicks work
    /// immediately on every visible page. `active` is the HUD being open or the barcode tool
    /// being picked: the scan runs only then, and never opens the HUD by itself.
    pub fn auto_scan_on_page_change(
        view: &mut DocView,
        doc: &Document,
        now: f64,
        active: bool,
        ctx: Option<&egui::Context>,
        visible_pages: &[usize],
    ) {
        let generation = doc.edit_generation();
        if view.session.barcode_hud.cache_generation != Some(generation) {
            view.session.barcode_hud.cache_generation = Some(generation);
            view.session.barcode_hud.page_codes.clear();
            view.session.barcode_hud.last_scanned_page = None;
            view.session.barcode_scan = None;
        }

        // A finished scan belongs to the document and generation it was asked for: the user may
        // have switched tabs or edited the document while it ran.
        if let Some((doc_id, scanned_page, scan_gen, rx)) = &view.session.barcode_scan {
            match rx.try_recv() {
                Ok(codes) => {
                    let (doc_id, scanned_page, scan_gen) = (*doc_id, *scanned_page, *scan_gen);
                    view.session.barcode_scan = None;
                    if view.id.0 == doc_id && doc.edit_generation() == scan_gen {
                        view.session.barcode_hud.page_codes.insert(scanned_page, codes.clone());
                        if view.view.current == scanned_page {
                            Self::apply_scan(view, scanned_page, codes, now);
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => view.session.barcode_scan = None,
            }
        }
        if !active {
            return;
        }
        if view.session.barcode_scan.is_some() {
            return;
        }
        let current_page = view.view.current;
        let next_page = if view.session.barcode_hud.last_scanned_page != Some(current_page) {
            view.session.barcode_hud.last_scanned_page = Some(current_page);
            if let Some(cached) = view.session.barcode_hud.page_codes.get(&current_page).cloned() {
                Self::apply_scan(view, current_page, cached, now);
                visible_pages.iter().copied().find(|p| !view.session.barcode_hud.page_codes.contains_key(p))
            } else {
                Some(current_page)
            }
        } else {
            visible_pages.iter().copied().find(|p| !view.session.barcode_hud.page_codes.contains_key(p))
        };
        let Some(target_page) = next_page else {
            return;
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
            // No threads on the web: scan here rather than never.
            let codes = doc.scan_page_codes(target_page);
            view.session.barcode_hud.page_codes.insert(target_page, codes.clone());
            if target_page == current_page {
                Self::apply_scan(view, target_page, codes, now);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (bytes, password, name) = (doc.bytes.clone(), doc.password.clone(), doc.name.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            let repaint = ctx.cloned();
            let spawned = std::thread::Builder::new().name("barcode-scan".into()).spawn(move || {
                // A scan-only session: the interface's own session cannot be moved to a worker.
                let mut session = pdfcraft_engine::Session::new();
                let codes = match session.open(name, None, bytes, password.as_deref()) {
                    Ok(id) => session.get(id).map(|d| d.scan_page_codes(target_page)).unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let _ = tx.send(codes);
                if let Some(ctx) = repaint {
                    ctx.request_repaint();
                }
            });
            match spawned {
                Ok(_) => view.session.barcode_scan = Some((view.id.0, target_page, generation, rx)),
                // No thread could be started: fall back to scanning this frame.
                Err(_) => {
                    let codes = doc.scan_page_codes(target_page);
                    view.session.barcode_hud.page_codes.insert(target_page, codes.clone());
                    if target_page == current_page {
                        Self::apply_scan(view, target_page, codes, now);
                    }
                }
            }
        }
    }

    /// Show a finished page scan, keeping whether the HUD and its panel were open.
    fn apply_scan(view: &mut DocView, page: usize, codes: Vec<ScannedCode>, now: f64) {
        let count = codes.len();
        let open = view.session.barcode_hud.open;
        let panel_open = view.session.barcode_hud.panel_open;
        view.session.barcode_hud.update_codes_conditional(codes, open);
        view.session.barcode_hud.panel_open = panel_open;
        if open {
            let msg = if count > 0 {
                let template =
                    crate::i18n::trn(crate::i18n::current(), count as u64, "Detected {n} code on page {page}", "Detected {n} codes on page {page}");
                crate::i18n::fmt(&template, &[("page", &(page + 1).to_string())])
            } else {
                crate::i18n::fmt(tl!("No codes on page {page}"), &[("page", &(page + 1).to_string())])
            };
            view.session.barcode_hud.show_toast(msg, now);
        }
    }

    /// Render the unified interactive HUD and canvas highlight overlays.
    ///
    /// Returns a pending replacement `Edit` if the user modified and applied a code.
    pub fn show(ui: &mut egui::Ui, view: &mut DocView, doc: &Document, info: &DocInfo, can_modify: bool) -> Option<Edit> {
        if !view.session.barcode_hud.open || !view.session.barcode_hud.panel_open {
            return None;
        }

        let current_page = view.session.barcode_hud.active_code().map_or(view.view.current, |c| c.page);
        let view_page = view.view.current;
        let total_pages = info.pages.len();
        let viewport = view.viewport_rect();
        let view_id = view.id.0;
        let now = ui.input(|i| i.time);
        let page_xf = view.page_xform(current_page);

        let state = &mut view.session.barcode_hud;
        let mut actions = HudActions::default();

        if state.edit_buffer.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            actions.step_prev = ui.input(|i| i.key_pressed(egui::Key::ArrowLeft));
            actions.step_next = ui.input(|i| i.key_pressed(egui::Key::ArrowRight));
        }

        let anchor = FloatingMenuAnchor { current_page, page_xf: page_xf.as_ref(), viewport, view_id };
        let (menu_delete, download_code) = render_floating_menu(ui, state, anchor, info, can_modify);
        if menu_delete.is_some() {
            actions.delete_code = menu_delete;
        }

        if state.panel_open {
            let palette = HudPalette::from_ui(ui);
            let render_cx = HudRenderCx { view_id, viewport, current_page, total_pages, can_modify, palette: &palette, now };
            render_hud_window(ui, state, &render_cx, &mut actions);
        }

        if actions.trigger_rescan {
            state.target_rect = state.active_code().map(|c| c.rect);
            state.page_codes.clear();
            // The background scan picks this up next frame (see auto_scan_on_page_change).
            state.last_scanned_page = None;
            state.show_toast(tl!("Rescanning…").to_string(), now);
        }
        if actions.cancel_edit {
            state.edit_buffer = None;
            state.reembed_kind = None;
        }
        if actions.start_edit
            && let Some((text, kind)) = state.active_code().map(|c| (c.text.clone(), c.kind))
        {
            state.edit_buffer = Some(text);
            state.reembed_kind = Some(kind);
        }

        let mut pending_edit = actions.apply_code.and_then(|(active, buf, kind)| apply_code_replacement(state, doc, info, active, buf, kind, now));
        if let Some(active) = actions.delete_code {
            let target_page = active.page;
            if let Some(edit) = handle_barcode_delete(state, active, target_page, doc, info, now) {
                pending_edit = Some(edit);
            }
        }
        if let Some(active) = download_code {
            handle_barcode_download(state, &active, now);
        }
        if let Some(text) = actions.copy_text {
            ui.ctx().copy_text(text);
            state.show_toast(tl!("Copied payload to clipboard!").to_string(), now);
        }
        if actions.step_prev {
            state.prev_code();
        }
        if actions.step_next {
            state.next_code();
        }
        if actions.close_hud {
            state.panel_open = false;
        }
        if actions.trigger_generate
            && let Some(edit) = generate_new_code(state, info, view_page, now)
        {
            pending_edit = Some(edit);
        }

        pending_edit
    }

    /// Legacy compatibility stub for `show_panel` (functionality unified into `show`).
    pub fn show_panel(_ui: &mut egui::Ui, _view: &mut DocView, _doc: &Document, _info: &DocInfo, _can_modify: bool) -> Option<Edit> {
        None
    }
}

#[derive(Default)]
struct HudActions {
    close_hud: bool,
    trigger_rescan: bool,
    step_prev: bool,
    step_next: bool,
    copy_text: Option<String>,
    start_edit: bool,
    apply_code: Option<(ScannedCode, String, CodeKind)>,
    cancel_edit: bool,
    delete_code: Option<ScannedCode>,
    trigger_generate: bool,
}

struct HudRenderCx<'a> {
    view_id: u64,
    viewport: Rect,
    current_page: usize,
    total_pages: usize,
    can_modify: bool,
    palette: &'a HudPalette,
    now: f64,
}

fn render_hud_window(ui: &mut egui::Ui, state: &mut BarcodeHudState, cx: &HudRenderCx<'_>, actions: &mut HudActions) {
    let hud_w = 330.0;
    let hud_pos = Pos2::new((cx.viewport.right() - hud_w - 18.0).max(cx.viewport.left() + 18.0), cx.viewport.top() + 18.0);
    egui::Window::new(tl!("QR & Barcodes"))
        .id(egui::Id::unique(("barcode-scanner-hud-window", cx.view_id)))
        .order(egui::Order::Foreground)
        .default_pos(hud_pos)
        .default_width(hud_w)
        .resizable(true)
        .collapsible(false)
        .title_bar(false)
        .frame(egui::Frame::NONE)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(cx.palette.card_bg)
                .stroke(Stroke::new(1.0, cx.palette.border))
                .corner_radius(CornerRadius::same(12))
                .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: cx.palette.shadow })
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_width((hud_w - 28.0).max(ui.available_width()));
                    render_hud_header(ui, cx.palette, actions);
                    ui.add_space(8.0);
                    render_page_tracker(ui, state, cx.current_page, cx.total_pages, cx.palette, actions);
                    if let Some(active) = state.active_code().cloned() {
                        ui.add_space(8.0);
                        render_active_code_card(ui, state, &active, cx.can_modify, cx.palette, actions);
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    render_generate_section(ui, state, cx.can_modify, cx.palette, cx.now, actions);
                });
        });
}

fn render_hud_header(ui: &mut egui::Ui, palette: &HudPalette, actions: &mut HudActions) {
    ui.horizontal(|ui| {
        let (icon_r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
        ui.painter().rect_stroke(icon_r.shrink(1.0), CornerRadius::same(3), Stroke::new(1.5, palette.accent), egui::StrokeKind::Inside);
        ui.painter().rect_filled(icon_r.shrink(5.0), CornerRadius::same(1), palette.accent);
        Text::new(tl!("QR & Barcodes")).role(TypeRole::Heading).show(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if IconButton::new(IconId::new("close")).label(tl!("Close QR & Barcodes")).show(ui).clicked() {
                actions.close_hud = true;
            }
        });
    });
}

fn render_page_tracker(
    ui: &mut egui::Ui,
    state: &BarcodeHudState,
    current_page: usize,
    total_pages: usize,
    palette: &HudPalette,
    actions: &mut HudActions,
) {
    let total_codes = state.scanned_codes.len();
    egui::Frame::NONE
        .fill(palette.item_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let hud_page = state.active_code().map_or(current_page, |c| c.page);
                let page_label = crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(hud_page + 1).to_string()), ("n", &total_pages.to_string())]);
                Text::new(page_label).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if IconButton::new(IconId::new("file-scan")).label(tl!("Rescan visible pages")).show(ui).clicked() {
                        actions.trigger_rescan = true;
                    }
                    ui.colored_label(palette.good, egui::RichText::new(tl!("Auto")).small());
                });
            });
            ui.add_space(6.0);
            if total_codes > 0 {
                ui.horizontal(|ui| {
                    if IconButton::new(IconId::new("chevron-left")).label(tl!("Previous barcode or QR code")).show(ui).clicked() {
                        actions.step_prev = true;
                    }
                    let count_label =
                        crate::i18n::fmt(tl!("Code {i} of {n}"), &[("i", &(state.active_index + 1).to_string()), ("n", &total_codes.to_string())]);
                    ui.add_space(6.0);
                    ui.colored_label(palette.accent, count_label);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if IconButton::new(IconId::new("chevron-right")).label(tl!("Next barcode or QR code")).show(ui).clicked() {
                            actions.step_next = true;
                        }
                    });
                });
            } else {
                Text::new(tl!("No barcodes or QR codes on this page")).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
            }
        });
}

fn render_active_code_card(
    ui: &mut egui::Ui,
    state: &mut BarcodeHudState,
    active: &ScannedCode,
    can_modify: bool,
    palette: &HudPalette,
    actions: &mut HudActions,
) {
    egui::Frame::NONE
        .fill(palette.item_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot_r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(dot_r.center(), 3.0, palette.accent);
                Text::new(tl!(active.kind.label())).role(TypeRole::Label).show(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let dims = crate::i18n::fmt(
                        tl!("{w}×{h}px • 100%"),
                        &[("w", &active.pixel_size[0].to_string()), ("h", &active.pixel_size[1].to_string())],
                    );
                    Text::new(dims).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
                });
            });
            ui.add_space(6.0);
            if let Some(buf) = &mut state.edit_buffer {
                Text::new(tl!("Edit payload and symbology")).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
                ui.add_space(3.0);
                ui.add(egui::TextEdit::multiline(buf).desired_width(ui.available_width()).desired_rows(3));
                ui.add_space(4.0);
                let mut selected_kind = state.reembed_kind.unwrap_or(active.kind);
                let kind_entries = [
                    Entry::new(CodeKind::QrCode, tl!("QR Code")),
                    Entry::new(CodeKind::Code39, tl!("Code 39 Barcode")),
                    Entry::new(CodeKind::Code128, tl!("Code 128 Barcode")),
                    Entry::new(CodeKind::Ean13, tl!("EAN-13 Barcode")),
                ];
                Select::new(tl!("Format"), Bind::bound(&mut selected_kind), &kind_entries).show(ui);
                state.reembed_kind = Some(selected_kind);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_apply = can_modify && !buf.trim().is_empty();
                    if ui.add_enabled_ui(can_apply, |ui| clicked(&Button::new(tl!("Re-embed")).variant(Variant::PRIMARY).show(ui))).inner {
                        actions.apply_code = Some((active.clone(), buf.clone(), selected_kind));
                    }
                    if clicked(&Button::new(tl!("Cancel")).variant(Variant::GHOST).show(ui)) {
                        actions.cancel_edit = true;
                    }
                });
            } else {
                egui::Frame::NONE
                    .fill(palette.code_bg)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(&active.text).monospace());
                        });
                    });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if clicked(
                        &Button::new(tl!("Copy"))
                            .icon(IconId::new("copy"))
                            .variant(Variant::SECONDARY)
                            .tooltip(tl!("Copy payload to clipboard"))
                            .show(ui),
                    ) {
                        actions.copy_text = Some(active.text.clone());
                    }
                    if can_modify
                        && clicked(
                            &Button::new(tl!("Edit Payload"))
                                .icon(IconId::new("file-pen-line"))
                                .variant(Variant::SECONDARY)
                                .tooltip(tl!("Edit and re-embed code"))
                                .show(ui),
                        )
                    {
                        actions.start_edit = true;
                    }
                    let del_tip = if active.image_index.is_none() {
                        tl!("Cover code with whiteout overlay (underlying vector graphics remain in document)")
                    } else {
                        tl!("Whiteout and remove code from page")
                    };
                    if can_modify
                        && clicked(
                            &Button::new(tl!("Delete Code")).icon(IconId::new("trash-2")).variant(Variant::SECONDARY).tooltip(del_tip).show(ui),
                        )
                    {
                        actions.delete_code = Some(active.clone());
                    }
                });
                if active.image_index.is_none() {
                    ui.add_space(2.0);
                    ui.colored_label(palette.warn, tl!("Vector code: changes cover original graphics with an overlay."));
                }
            }
        });
}

fn render_generate_section(
    ui: &mut egui::Ui,
    state: &mut BarcodeHudState,
    can_modify: bool,
    palette: &HudPalette,
    now: f64,
    actions: &mut HudActions,
) {
    Text::new(tl!("Generate & embed code on page")).role(TypeRole::Label).show(ui);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut state.generate_kind, CodeKind::QrCode, tl!("QR Code"));
        ui.selectable_value(&mut state.generate_kind, CodeKind::Code128, tl!("Code 128 Barcode"));
        ui.selectable_value(&mut state.generate_kind, CodeKind::Code39, tl!("Code 39"));
        ui.selectable_value(&mut state.generate_kind, CodeKind::Ean13, tl!("EAN-13"));
    });
    ui.add_space(4.0);
    let hint = match state.generate_kind {
        CodeKind::QrCode => tl!("QR payload (URL, text)…"),
        CodeKind::Code39 => tl!("Code 39 payload (e.g. CODE39)…"),
        CodeKind::Code128 => tl!("Code 128 barcode payload (e.g. 12345678)…"),
        CodeKind::Ean13 => tl!("12 or 13 digits (e.g. 590123412345)…"),
        _ => tl!("Enter code payload…"),
    };
    ui.add(egui::TextEdit::multiline(&mut state.qr_input).desired_width(ui.available_width()).desired_rows(2).hint_text(hint));
    ui.add_space(6.0);
    let can_gen = can_modify && !state.qr_input.trim().is_empty();
    let btn_label = match state.generate_kind {
        CodeKind::QrCode => tl!("Embed QR code on page").to_string(),
        _ => crate::i18n::fmt(tl!("Embed {kind} on page"), &[("kind", tl!(state.generate_kind.label()))]),
    };
    if ui.add_enabled_ui(can_gen, |ui| clicked(&Button::new(btn_label).variant(Variant::PRIMARY).show(ui))).inner {
        actions.trigger_generate = true;
    }
    if let Some((msg, expiry)) = &state.toast
        && *expiry > now
    {
        ui.add_space(6.0);
        ui.colored_label(palette.good, msg);
    }
}

fn apply_code_replacement(
    state: &mut BarcodeHudState,
    doc: &Document,
    info: &DocInfo,
    active: ScannedCode,
    buf: String,
    kind: CodeKind,
    now: f64,
) -> Option<Edit> {
    let target_page = active.page;
    let (whiteout_rect, new_image_rect, pw, ph) = safe_rect_and_pixels(active.rect, active.pixel_size, kind, Some(&active.text), Some(&buf));
    let (edit_whiteout, edit_image) = (display_rect(whiteout_rect, Some(info), target_page), display_rect(new_image_rect, Some(info), target_page));

    match Document::generate_replacement_code(kind, &buf, pw, ph) {
        Ok(png_bytes) => {
            let whole_image_idx =
                active.image_index.filter(|i| doc.page_images(target_page).get(*i).is_some_and(|im| code_is_whole_image(&active, im.rect)));
            let pending = if let Some(image_idx) = whole_image_idx {
                // Replace the embedded raster image directly so no old payload remains underneath.
                Edit::EditPageImage {
                    page: target_page,
                    index: image_idx,
                    change: pdfcraft_engine::ImageEdit::Replace { name: "UpdatedCode.png".into(), bytes: Arc::new(png_bytes) },
                }
            } else {
                let whiteout_bytes = Document::generate_whiteout(pw, ph).unwrap_or_default();
                let edits = if !whiteout_bytes.is_empty() {
                    vec![
                        Edit::AddImage { page: target_page, rect: Some(edit_whiteout), name: "Whiteout.png".into(), bytes: Arc::new(whiteout_bytes) },
                        Edit::AddImage { page: target_page, rect: Some(edit_image), name: "UpdatedCode.png".into(), bytes: Arc::new(png_bytes) },
                    ]
                } else {
                    vec![Edit::AddImage { page: target_page, rect: Some(edit_image), name: "UpdatedCode.png".into(), bytes: Arc::new(png_bytes) }]
                };
                Edit::Batch { label: "Replace code".into(), edits }
            };
            state.target_rect = Some(new_image_rect);
            state.page_codes.remove(&target_page);
            state.last_scanned_page = None;
            state.edit_buffer = None;
            state.reembed_kind = None;
            let msg = crate::i18n::fmt(tl!("Replaced at 100% dimensions ({w}×{h}px)!"), &[("w", &pw.to_string()), ("h", &ph.to_string())]);
            state.show_toast(msg, now);
            Some(pending)
        }
        Err(e) => {
            state.show_toast(crate::i18n::fmt(tl!("Replacement failed: {error}"), &[("error", &e.to_string())]), now);
            None
        }
    }
}

fn generate_new_code(state: &mut BarcodeHudState, info: &DocInfo, target_page: usize, now: f64) -> Option<Edit> {
    let text = state.qr_input.trim();
    let kind = state.generate_kind;
    let (pw, ph, rect_w, rect_h) = match kind {
        CodeKind::QrCode => (240, 240, 144.0, 144.0),
        _ => (360, 120, 216.0, 72.0),
    };
    match Document::generate_replacement_code(kind, text, pw, ph) {
        Ok(png_bytes) => {
            let (page_w, page_h) = info.pages.get(target_page).map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
            let x = (page_w - rect_w) * 0.5;
            let y = (page_h - rect_h) * 0.5;
            let rect = [x, y, x + rect_w, y + rect_h];
            let filename = match kind {
                CodeKind::QrCode => "QRCode.png",
                CodeKind::Code39 => "Code39.png",
                CodeKind::Code128 => "Code128.png",
                CodeKind::Ean13 => "EAN13.png",
                _ => "Barcode.png",
            };
            state.page_codes.remove(&target_page);
            state.last_scanned_page = None;
            state.show_toast(crate::i18n::fmt(tl!("{kind} placed on page!"), &[("kind", tl!(kind.label()))]), now);
            Some(Edit::AddImage { page: target_page, rect: Some(rect), name: filename.into(), bytes: Arc::new(png_bytes) })
        }
        Err(e) => {
            state.show_toast(crate::i18n::fmt(tl!("Generation error: {error}"), &[("error", &e.to_string())]), now);
            None
        }
    }
}

fn code_is_whole_image(code: &ScannedCode, img_rect: [f64; 4]) -> bool {
    let area = |r: [f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).abs();
    area(img_rect) > 0.0 && area(code.rect) / area(img_rect) > 0.9
}

fn handle_barcode_delete(
    state: &mut BarcodeHudState,
    active: ScannedCode,
    target_page: usize,
    doc: &Document,
    info: &DocInfo,
    now: f64,
) -> Option<Edit> {
    state.target_rect = None;
    state.page_codes.remove(&target_page);
    state.last_scanned_page = None;
    state.edit_buffer = None;

    let whole_image_idx = active.image_index.filter(|i| doc.page_images(target_page).get(*i).is_some_and(|im| code_is_whole_image(&active, im.rect)));

    if let Some(image_idx) = whole_image_idx {
        state.show_toast(tl!("Image code deleted from page!").to_string(), now);
        Some(Edit::EditPageImage { page: target_page, index: image_idx, change: pdfcraft_engine::ImageEdit::Delete })
    } else {
        let (whiteout_rect, _, pw, ph) = safe_rect_and_pixels(active.rect, active.pixel_size, active.kind, Some(&active.text), None);
        let edit_whiteout = display_rect(whiteout_rect, Some(info), target_page);
        match Document::generate_whiteout(pw, ph) {
            Ok(whiteout_png) => {
                state.show_toast(tl!("Vector code covered with whiteout (original vector data preserved)").to_string(), now);
                Some(Edit::AddImage { page: target_page, rect: Some(edit_whiteout), name: "Whiteout.png".into(), bytes: Arc::new(whiteout_png) })
            }
            Err(e) => {
                state.show_toast(crate::i18n::fmt(tl!("Delete failed: {error}"), &[("error", &e.to_string())]), now);
                None
            }
        }
    }
}

fn handle_barcode_download(state: &mut BarcodeHudState, active: &ScannedCode, now: f64) {
    let (_, _, pw, ph) = safe_rect_and_pixels(active.rect, active.pixel_size, active.kind, Some(&active.text), None);
    let pw = pw.max(128);
    let ph = if active.kind == CodeKind::QrCode { pw } else { ph.max(48) };
    match Document::generate_replacement_code(active.kind, &active.text, pw, ph) {
        Ok(png_bytes) => {
            let kind_name = format!("{:?}", active.kind).to_ascii_lowercase();
            #[cfg(target_arch = "wasm32")]
            {
                let filename = format!("barcode_{}_{}.png", kind_name, now as u64);
                state.pending_download = Some((filename, Arc::new(png_bytes)));
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                use std::io::Write;
                let dir = get_downloads_dir();
                let mut n = 0u64;
                let mut saved = false;
                while n < 1000 {
                    let filename = format!("barcode_{}_{}_{}.png", kind_name, (now as u64) % 10000, n);
                    let dest = dir.join(&filename);
                    match std::fs::OpenOptions::new().write(true).create_new(true).open(&dest) {
                        Ok(mut file) => {
                            if let Err(e) = file.write_all(&png_bytes) {
                                state.show_toast(crate::i18n::fmt(tl!("Download failed: {error}"), &[("error", &e.to_string())]), now);
                            } else {
                                state.show_toast(crate::i18n::fmt(tl!("Saved to Downloads: {filename}"), &[("filename", &filename)]), now);
                            }
                            saved = true;
                            break;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            n = n.saturating_add(1);
                        }
                        Err(e) => {
                            state.show_toast(crate::i18n::fmt(tl!("Download failed: {error}"), &[("error", &e.to_string())]), now);
                            saved = true;
                            break;
                        }
                    }
                }
                if !saved {
                    state.show_toast(tl!("Download failed: could not create unique file").to_string(), now);
                }
            }
        }
        Err(e) => {
            state.show_toast(crate::i18n::fmt(tl!("Download failed: {error}"), &[("error", &e.to_string())]), now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::barcode_scan::CodeKind;

    #[test]
    fn test_barcode_hud_state_navigation() {
        let mut state = BarcodeHudState::default();
        let code1 = ScannedCode {
            id: 0,
            page: 0,
            kind: CodeKind::QrCode,
            text: "https://example.com/1".into(),
            rect: [10.0, 10.0, 100.0, 100.0],
            pixel_size: [100, 100],
            image_index: Some(0),
        };
        let code2 = ScannedCode {
            id: 1,
            page: 0,
            kind: CodeKind::Code128,
            text: "TRACK123456".into(),
            rect: [10.0, 120.0, 200.0, 180.0],
            pixel_size: [200, 60],
            image_index: Some(1),
        };

        state.update_codes(vec![code1.clone(), code2.clone()]);
        assert_eq!(state.scanned_codes.len(), 2);
        assert!(state.open);
        assert_eq!(state.active_index, 0);
        assert_eq!(state.active_code().map(|c| c.text.as_str()), Some("https://example.com/1"));

        // Step next
        state.next_code();
        assert_eq!(state.active_index, 1);
        assert_eq!(state.active_code().map(|c| c.text.as_str()), Some("TRACK123456"));

        // Step next wraps to 0
        state.next_code();
        assert_eq!(state.active_index, 0);

        // Step prev wraps to 1
        state.prev_code();
        assert_eq!(state.active_index, 1);
    }

    #[test]
    fn test_toast_display() {
        let mut state = BarcodeHudState::default();
        state.show_toast("Code detected".into(), 10.0);
        assert!(state.toast.is_some());
        let (msg, expiry) = state.toast.unwrap();
        assert_eq!(msg, "Code detected");
        assert_eq!(expiry, 13.0);
    }

    #[test]
    fn test_barcode_hud_ordering_shift_fix() {
        let mut state = BarcodeHudState::default();
        let code1 = ScannedCode {
            id: 0,
            page: 0,
            kind: CodeKind::QrCode,
            text: "Code 1".into(),
            rect: [10.0, 10.0, 100.0, 100.0],
            pixel_size: [100, 100],
            image_index: Some(0),
        };
        let code2 = ScannedCode {
            id: 1,
            page: 0,
            kind: CodeKind::Code128,
            text: "Code 2".into(),
            rect: [10.0, 120.0, 200.0, 180.0],
            pixel_size: [200, 60],
            image_index: Some(1),
        };

        state.update_codes(vec![code1.clone(), code2.clone()]);
        state.next_code();
        assert_eq!(state.active_index, 1);
        assert_eq!(state.active_code().map(|c| c.text.as_str()), Some("Code 2"));

        // Simulate replacement of Code 2: new scan reorders updated Code 2 to index 0 ("Code 1")
        let updated_code2 = ScannedCode { text: "Code 2 Updated".into(), ..code2.clone() };
        state.target_rect = Some(code2.rect);
        state.update_codes(vec![updated_code2.clone(), code1.clone()]);

        // active_index tracks target_rect and points to index 0 where Code 2 is now
        assert_eq!(state.active_index, 0);
        assert_eq!(state.active_code().map(|c| c.text.as_str()), Some("Code 2 Updated"));
    }

    #[test]
    fn test_safe_rect_and_pixels_payload_expansion() {
        let rect = [10.0, 20.0, 110.0, 50.0];
        let pixel_size = [100, 30];
        let (w_rect, img_rect, pw, _ph) = safe_rect_and_pixels(rect, pixel_size, CodeKind::Code128, Some("1234"), Some("12345"));
        assert_eq!(img_rect[0], 10.0, "left must stay anchored");
        assert!(img_rect[2] > 110.0, "right must expand to accommodate 5th digit");
        assert!(pw > 100, "pixel width must scale to keep module thickness");
        assert!(w_rect[2] >= img_rect[2], "whiteout must cover expanded right edge");
    }

    #[test]
    fn generate_new_code_targets_requested_view_page() {
        let mut state = BarcodeHudState { qr_input: "https://example.com".into(), generate_kind: CodeKind::QrCode, ..Default::default() };
        state.page_codes.insert(1, Vec::new());
        let info = DocInfo::default();
        let edit = generate_new_code(&mut state, &info, 1, 5.0).expect("QR edit");
        match edit {
            Edit::AddImage { page, .. } => assert_eq!(page, 1),
            other => panic!("unexpected edit: {other:?}"),
        }
        assert!(!state.page_codes.contains_key(&1));
    }
}
