//! Interactive Barcode & QR Code Scanner HUD (L7 UI).
//!
//! Provides automated page detection, unified compact HUD (data of one code at a time),
//! multi-code navigation (◀ / ▶ and arrow keys), 0.5px blue highlight box, clipboard copy,
//! and pixel-perfect 100% scale re-embedding.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use egui::{Color32, CornerRadius, Margin, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::barcode_scan::ScannedCode;
use pdfcraft_engine::{Document, Edit};
use pdfcraft_render::DocInfo;
use std::sync::Arc;

use crate::canvas::DocView;
use crate::theme;

/// Persistent HUD state for barcode / QR detection and interactive editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarcodeHudState {
    /// Cached detected codes for the active page.
    pub scanned_codes: Vec<ScannedCode>,
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
    let new_image_rect = [min_x, min_y, max_x, max_y];

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

impl BarcodeScannerHud {
    /// Reactive Auto-Page Tracking hook:
    ///
    /// Scans current page on page change so interactive canvas clicks work immediately.
    /// `active` is the HUD being open or the barcode tool being picked: the scan runs only then,
    /// but it never opens the HUD by itself. The page itself is scanned on a worker thread; its
    /// result is shown only if the document and page still match when it arrives.
    pub fn auto_scan_on_page_change(view: &mut DocView, doc: &Document, now: f64, active: bool, ctx: Option<&egui::Context>) {
        // A finished scan belongs to the document and page it was asked for: the user may have
        // turned the page or switched tabs while it ran.
        if let Some((doc_id, page, generation, rx)) = &view.barcode_scan {
            match rx.try_recv() {
                Ok(codes) => {
                    let (doc_id, page, generation) = (*doc_id, *page, *generation);
                    view.barcode_scan = None;
                    if view.id.0 == doc_id && view.current == page && doc.edit_generation() == generation {
                        Self::apply_scan(view, codes, now);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => view.barcode_scan = None,
            }
        }
        if !active {
            return;
        }
        let current_page = view.current;
        if view.barcode_scan.is_some() || view.barcode_hud.last_scanned_page == Some(current_page) {
            return;
        }
        view.barcode_hud.last_scanned_page = Some(current_page);
        if cfg!(target_arch = "wasm32") {
            // No threads on the web: scan here rather than never.
            Self::apply_scan(view, doc.scan_page_codes(current_page), now);
            return;
        }
        let (bytes, password, name) = (doc.bytes.clone(), doc.password.clone(), doc.name.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.cloned();
        let spawned = std::thread::Builder::new().name("barcode-scan".into()).spawn(move || {
            // A scan-only session: the interface's own session cannot be moved to a worker.
            let mut session = pdfcraft_engine::Session::new();
            let codes = match session.open(name, None, bytes, password.as_deref()) {
                Ok(id) => session.get(id).map(|d| d.scan_page_codes(current_page)).unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            let _ = tx.send(codes);
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
        let generation = doc.edit_generation();
        match spawned {
            Ok(_) => view.barcode_scan = Some((view.id.0, current_page, generation, rx)),
            // No thread could be started: fall back to scanning this frame.
            Err(_) => Self::apply_scan(view, doc.scan_page_codes(current_page), now),
        }
    }

    /// Show a finished page scan, keeping whether the HUD and its panel were open.
    fn apply_scan(view: &mut DocView, codes: Vec<ScannedCode>, now: f64) {
        let page = view.current;
        let count = codes.len();
        let open = view.barcode_hud.open;
        let panel_open = view.barcode_hud.panel_open;
        view.barcode_hud.update_codes_conditional(codes, open);
        view.barcode_hud.panel_open = panel_open;
        if open {
            if count > 0 {
                view.barcode_hud.show_toast(format!("Detected {count} code(s) on page {}", page + 1), now);
            } else {
                view.barcode_hud.show_toast(format!("No codes on page {}", page + 1), now);
            }
        }
    }

    /// Render the unified interactive HUD and canvas highlight overlays.
    ///
    /// Returns a pending replacement `Edit` if the user modified and applied a code.
    pub fn show(ui: &mut egui::Ui, view: &mut DocView, _doc: &Document, info: &DocInfo, can_modify: bool) -> Option<Edit> {
        if !view.barcode_hud.open || !view.barcode_hud.panel_open {
            return None;
        }

        let current_page = view.current;
        let total_pages = info.pages.len();
        let viewport = view.viewport_rect();
        let view_id = view.id.0;
        let now = ui.input(|i| i.time);
        let page_xf = view.page_xform(current_page);

        let state = &mut view.barcode_hud;
        let total_codes = state.scanned_codes.len();

        // 2. Keyboard arrow navigation when viewing (not editing text)
        let mut step_prev = false;
        let mut step_next = false;
        if state.edit_buffer.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                step_prev = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                step_next = true;
            }
        }

        let mut pending_edit = None;
        let mut close_hud = false;
        let mut trigger_rescan = false;
        let mut copy_text = None;
        let mut start_edit = false;
        let mut apply_code = None;
        let mut cancel_edit = false;
        let mut delete_code = None;
        let mut download_code = None;
        let mut trigger_generate = false;

        // Floating context menu near focused barcode with 🗑 Delete and ⬇ Download
        if let Some(active) = state.active_code()
            && active.page == current_page
            && let Some(xf) = page_xf
        {
            let code_rect = xf.user_rect(info, current_page, active.rect.map(|v| v as f32));
            let menu_pos = pos2(
                (code_rect.center().x - 70.0).clamp(viewport.left() + 10.0, viewport.right() - 150.0),
                if code_rect.bottom() + 38.0 <= viewport.bottom() {
                    code_rect.bottom() + 4.0
                } else {
                    (code_rect.top() - 34.0).max(viewport.top() + 10.0)
                },
            );
            egui::Area::new(egui::Id::new(("barcode-context-menu", view_id, state.active_index)))
                .order(egui::Order::Foreground)
                .fixed_pos(menu_pos)
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(Color32::from_rgb(0x1B, 0x1D, 0x26))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(0x3B, 0x82, 0xF6)))
                        .corner_radius(CornerRadius::same(6))
                        .shadow(egui::Shadow { offset: [0, 4], blur: 10, spread: 0, color: Color32::from_black_alpha(160) })
                        .inner_margin(Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if can_modify
                                    && ui
                                        .add(egui::Button::new(
                                            egui::RichText::new("🗑 Delete")
                                                .font(theme::medium(11.0))
                                                .color(Color32::from_rgb(0xEF, 0x44, 0x44)),
                                        ))
                                        .on_hover_text("Delete barcode and whiteout region")
                                        .clicked()
                                {
                                    delete_code = Some(active.clone());
                                }
                                if ui
                                    .add(egui::Button::new(
                                        egui::RichText::new("⬇ Download")
                                            .font(theme::medium(11.0))
                                            .color(Color32::from_rgb(0x60, 0xA5, 0xFA)),
                                    ))
                                    .on_hover_text("Export barcode PNG to Downloads folder")
                                    .clicked()
                                {
                                    download_code = Some(active.clone());
                                }
                            });
                        });
                });
        }

        // Modern Dark Aesthetics
        let card_bg = Color32::from_rgb(0x13, 0x14, 0x1A);
        let border_stroke = Stroke::new(1.0, Color32::from_rgb(0x28, 0x2C, 0x3B));
        let item_card_bg = Color32::from_rgb(0x1B, 0x1D, 0x26);
        let item_card_stroke = Stroke::new(1.0, Color32::from_rgb(0x28, 0x2C, 0x3B));
        let text_primary = Color32::from_rgb(0xF8, 0xFA, 0xFC);
        let text_muted = Color32::from_rgb(0x94, 0xA3, 0xB8);
        let accent_blue = Color32::from_rgb(0x3B, 0x82, 0xF6);
        let emerald_green = Color32::from_rgb(0x10, 0xB9, 0x81);

        // Fixed compact panel at top-right of canvas viewport
        let hud_w = 330.0;
        let hud_pos = Pos2::new((viewport.right() - hud_w - 18.0).max(viewport.left() + 18.0), viewport.top() + 18.0);

        if state.panel_open {
            egui::Window::new("QR & Barcodes")
                .id(egui::Id::new(("barcode-scanner-hud-window", view_id)))
                .order(egui::Order::Foreground)
                .default_pos(hud_pos)
                .default_width(hud_w)
                .resizable(true)
                .collapsible(false)
                .title_bar(false)
                .frame(egui::Frame::NONE)
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(card_bg)
                        .stroke(border_stroke)
                        .corner_radius(CornerRadius::same(12))
                        .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(170) })
                        .inner_margin(Margin::symmetric(14, 12))
                        .show(ui, |ui| {
                            ui.set_width((hud_w - 28.0).max(ui.available_width()));

                            // Header: Icon + Title + Close button
                            ui.horizontal(|ui| {
                                crate::icons::paint(ui, Rect::from_min_size(ui.cursor().min, vec2(16.0, 16.0)), "scan", 16.0, accent_blue);
                                ui.add_space(22.0);
                                ui.label(egui::RichText::new("QR & Barcodes").font(theme::semibold(13.5)).color(text_primary));

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("✕").on_hover_text("Close HUD").clicked() {
                                        close_hud = true;
                                    }
                                });
                            });

                            ui.add_space(8.0);

                            // Section 1: Page Tracker & Multi-Code Navigation Bar
                            egui::Frame::NONE
                                .fill(item_card_bg)
                                .stroke(item_card_stroke)
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!("Page {} of {}", current_page + 1, total_pages))
                                                .font(theme::medium(11.5))
                                                .color(text_muted),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("⟳").on_hover_text("Rescan current page").clicked() {
                                                trigger_rescan = true;
                                            }
                                            ui.label(egui::RichText::new("● AUTO").font(theme::semibold(9.5)).color(emerald_green));
                                        });
                                    });

                                    ui.add_space(6.0);

                                    if total_codes > 0 {
                                        ui.horizontal(|ui| {
                                            let prev_btn = ui
                                                .add_sized([26.0, 22.0], egui::Button::new("◀"))
                                                .on_hover_text("Previous barcode/QR code (Left Arrow)");
                                            if prev_btn.clicked() {
                                                step_prev = true;
                                            }

                                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                                ui.painter().rect_filled(
                                                    Rect::from_min_size(ui.cursor().min, vec2(136.0, 22.0)),
                                                    CornerRadius::same(5),
                                                    Color32::from_rgb(0x13, 0x14, 0x1A),
                                                );
                                                ui.add_space(8.0);
                                                ui.label(
                                                    egui::RichText::new(format!("Code {} of {}", state.active_index + 1, total_codes))
                                                        .font(theme::semibold(12.0))
                                                        .color(accent_blue),
                                                );
                                            });

                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                let next_btn = ui
                                                    .add_sized([26.0, 22.0], egui::Button::new("▶"))
                                                    .on_hover_text("Next barcode/QR code (Right Arrow)");
                                                if next_btn.clicked() {
                                                    step_next = true;
                                                }
                                            });
                                        });
                                    } else {
                                        ui.label(
                                            egui::RichText::new("No barcodes or QR codes on this page").font(theme::regular(11.0)).color(text_muted),
                                        );
                                    }
                                });

                            // Section 2: Active Code Details Card (Data of ONE code at a time)
                            if let Some(active) = state.active_code().cloned() {
                                ui.add_space(8.0);
                                egui::Frame::NONE
                                    .fill(item_card_bg)
                                    .stroke(item_card_stroke)
                                    .corner_radius(CornerRadius::same(8))
                                    .inner_margin(Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        // Symbology + Dimensions
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new("●").color(accent_blue).font(theme::semibold(11.0)));
                                            ui.label(egui::RichText::new(active.kind.label()).font(theme::semibold(12.0)).color(text_primary));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("{}×{}px • 100%", active.pixel_size[0], active.pixel_size[1]))
                                                        .font(theme::semibold(10.0))
                                                        .color(Color32::from_rgb(0x38, 0xBD, 0xF8)),
                                                );
                                            });
                                        });

                                        ui.add_space(6.0);

                                        // Payload View or Edit Mode
                                        if let Some(buf) = &mut state.edit_buffer {
                                            ui.label(
                                                egui::RichText::new("EDIT PAYLOAD & SYMBOLOGY").font(theme::semibold(9.5)).color(text_muted),
                                            );
                                            ui.add_space(3.0);
                                            ui.add(egui::TextEdit::multiline(buf).desired_width(ui.available_width()).desired_rows(3));
                                            ui.add_space(4.0);

                                            // Symbology Dropdown
                                            let mut selected_kind = state.reembed_kind.unwrap_or(active.kind);
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new("Format:").font(theme::medium(11.0)).color(text_muted));
                                                egui::ComboBox::from_id_salt("reembed-symbology-dropdown")
                                                    .selected_text(selected_kind.label())
                                                    .width(180.0)
                                                    .show_ui(ui, |ui| {
                                                        ui.selectable_value(&mut selected_kind, pdfcraft_engine::barcode_scan::CodeKind::QrCode, "QR Code");
                                                        ui.selectable_value(&mut selected_kind, pdfcraft_engine::barcode_scan::CodeKind::Code39, "Code 39 Barcode (139)");
                                                        ui.selectable_value(&mut selected_kind, pdfcraft_engine::barcode_scan::CodeKind::Code128, "Code 128 Barcode");
                                                        ui.selectable_value(&mut selected_kind, pdfcraft_engine::barcode_scan::CodeKind::Ean13, "EAN-13 Barcode");
                                                    });
                                            });
                                            state.reembed_kind = Some(selected_kind);

                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                let can_apply = can_modify && !buf.trim().is_empty();
                                                if ui.add_enabled(can_apply, egui::Button::new("⚡ Re-embed")).clicked() {
                                                    apply_code = Some((active.clone(), buf.clone(), selected_kind));
                                                }
                                                if ui.button("Cancel").clicked() {
                                                    cancel_edit = true;
                                                }
                                            });
                                        } else {
                                            // Monospace text container
                                            egui::Frame::NONE
                                                .fill(Color32::from_rgb(0x0C, 0x0D, 0x12))
                                                .stroke(Stroke::new(1.0, Color32::from_rgb(0x22, 0x25, 0x30)))
                                                .corner_radius(CornerRadius::same(6))
                                                .inner_margin(Margin::symmetric(8, 6))
                                                .show(ui, |ui| {
                                                    ui.horizontal_wrapped(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(&active.text)
                                                                .font(theme::medium(12.0))
                                                                .monospace()
                                                                .color(text_primary),
                                                        );
                                                    });
                                                });

                                            ui.add_space(6.0);
                                            ui.horizontal(|ui| {
                                                if ui.button("📋 Copy").on_hover_text("Copy payload to clipboard").clicked() {
                                                    copy_text = Some(active.text.clone());
                                                }
                                                if can_modify && ui.button("✏ Edit Payload").on_hover_text("Edit and re-embed code").clicked() {
                                                    start_edit = true;
                                                }
                                                if can_modify
                                                    && ui.button("🗑 Delete Code").on_hover_text("Whiteout and remove code from page").clicked()
                                                {
                                                    delete_code = Some(active.clone());
                                                }
                                            });
                                        }
                                    });
                            }

                            // Section 3: Collapsible "Generate New QR / Barcode" Section
                            ui.add_space(8.0);
                            ui.collapsing("Generate New QR / Barcode", |ui| {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Type:").font(theme::medium(11.0)).color(text_muted));
                                    egui::ComboBox::from_id_salt("generate-code-type-dropdown")
                                        .selected_text(state.generate_kind.label())
                                        .width(180.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut state.generate_kind, pdfcraft_engine::barcode_scan::CodeKind::QrCode, "QR Code");
                                            ui.selectable_value(&mut state.generate_kind, pdfcraft_engine::barcode_scan::CodeKind::Code39, "Code 39 Barcode (139)");
                                            ui.selectable_value(&mut state.generate_kind, pdfcraft_engine::barcode_scan::CodeKind::Code128, "Code 128 Barcode");
                                            ui.selectable_value(&mut state.generate_kind, pdfcraft_engine::barcode_scan::CodeKind::Ean13, "EAN-13 Barcode");
                                        });
                                });
                                ui.add_space(4.0);
                                let hint = match state.generate_kind {
                                    pdfcraft_engine::barcode_scan::CodeKind::QrCode => "https://example.com or text...",
                                    pdfcraft_engine::barcode_scan::CodeKind::Code39 => "ALPHANUMERIC (e.g. CODE39)",
                                    pdfcraft_engine::barcode_scan::CodeKind::Code128 => "Any ASCII text...",
                                    pdfcraft_engine::barcode_scan::CodeKind::Ean13 => "12 or 13 digits (e.g. 590123412345)",
                                    _ => "Enter payload...",
                                };
                                ui.add(
                                    egui::TextEdit::multiline(&mut state.qr_input)
                                        .desired_width(ui.available_width())
                                        .desired_rows(2)
                                        .hint_text(hint),
                                );
                                ui.add_space(6.0);
                                let can_gen = can_modify && !state.qr_input.trim().is_empty();
                                let btn_label = format!("⚡ Embed {} on Page", state.generate_kind.label());
                                if ui.add_enabled(can_gen, egui::Button::new(btn_label)).clicked() {
                                    trigger_generate = true;
                                }
                            });

                            // Feedback Toast
                            if let Some((msg, expiry)) = &state.toast
                                && *expiry > now
                            {
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new(msg).font(theme::medium(11.0)).color(emerald_green));
                            }
                        });
                },
            );
        }

        if trigger_rescan {
            state.target_rect = state.active_code().map(|c| c.rect);
            // The background scan picks this up next frame (see auto_scan_on_page_change).
            state.last_scanned_page = None;
            state.show_toast("Rescanning…".into(), now);
        }
        if cancel_edit {
            state.edit_buffer = None;
            state.reembed_kind = None;
        }
        if start_edit && let Some((text, kind)) = state.active_code().map(|c| (c.text.clone(), c.kind)) {
            state.edit_buffer = Some(text);
            state.reembed_kind = Some(kind);
        }
        if let Some((active, buf, kind)) = apply_code {
            let (whiteout_rect, new_image_rect, pw, ph) =
                safe_rect_and_pixels(active.rect, active.pixel_size, kind, Some(&active.text), Some(&buf));
            let (edit_whiteout, edit_image) =
                (display_rect(whiteout_rect, Some(info), current_page), display_rect(new_image_rect, Some(info), current_page));

            match Document::generate_replacement_code(kind, &buf, pw, ph) {
                Ok(png_bytes) => {
                    if let Some(image_idx) = active.image_index {
                        // Replace the embedded raster image directly so no old payload remains underneath.
                        pending_edit = Some(Edit::EditPageImage {
                            page: current_page,
                            index: image_idx,
                            change: pdfcraft_engine::ImageEdit::Replace {
                                name: "UpdatedCode.png".into(),
                                bytes: Arc::new(png_bytes),
                            },
                        });
                    } else {
                        // Two-step overlay: first whiteout the old code, then place new code over active.rect
                        let whiteout_bytes = Document::generate_whiteout(pw, ph).unwrap_or_default();
                        let edits = if !whiteout_bytes.is_empty() {
                            vec![
                                Edit::AddImage {
                                    page: current_page,
                                    rect: Some(edit_whiteout),
                                    name: "Whiteout.png".into(),
                                    bytes: Arc::new(whiteout_bytes),
                                },
                                Edit::AddImage {
                                    page: current_page,
                                    rect: Some(edit_image),
                                    name: "UpdatedCode.png".into(),
                                    bytes: Arc::new(png_bytes),
                                },
                            ]
                        } else {
                            vec![Edit::AddImage {
                                page: current_page,
                                rect: Some(edit_image),
                                name: "UpdatedCode.png".into(),
                                bytes: Arc::new(png_bytes),
                            }]
                        };

                        pending_edit = Some(Edit::Batch { label: "Replace code".into(), edits });
                    }
                    state.target_rect = Some(new_image_rect);
                    state.last_scanned_page = None;
                    state.edit_buffer = None;
                    state.reembed_kind = None;
                    state.show_toast(format!("Replaced at 100% dimensions ({}×{}px)!", pw, ph), now);
                }
                Err(e) => {
                    state.show_toast(format!("Replacement failed: {e}"), now);
                }
            }
        }
        if let Some(active) = delete_code {
            if let Some(image_idx) = active.image_index {
                // Perform actual removal of the embedded image object so no data remains.
                pending_edit = Some(Edit::EditPageImage {
                    page: current_page,
                    index: image_idx,
                    change: pdfcraft_engine::ImageEdit::Delete,
                });
                state.target_rect = None;
                state.last_scanned_page = None;
                state.edit_buffer = None;
                state.show_toast("Image code deleted from page!".into(), now);
            } else {
                let (whiteout_rect, _, pw, ph) = safe_rect_and_pixels(active.rect, active.pixel_size, active.kind, Some(&active.text), None);
                let edit_whiteout = display_rect(whiteout_rect, Some(info), current_page);
                match Document::generate_whiteout(pw, ph) {
                    Ok(whiteout_png) => {
                        pending_edit = Some(Edit::AddImage {
                            page: current_page,
                            rect: Some(edit_whiteout),
                            name: "Whiteout.png".into(),
                            bytes: Arc::new(whiteout_png),
                        });
                        state.target_rect = None;
                        state.last_scanned_page = None;
                        state.edit_buffer = None;
                        state.show_toast("Vector code covered with whiteout (original vector data preserved)".into(), now);
                    }
                    Err(e) => {
                        state.show_toast(format!("Delete failed: {e}"), now);
                    }
                }
            }
        }
        if let Some(active) = download_code {
            let (_, _, pw, ph) = safe_rect_and_pixels(active.rect, active.pixel_size, active.kind, Some(&active.text), None);
            let pw = pw.max(128);
            let ph = if active.kind == pdfcraft_engine::barcode_scan::CodeKind::QrCode { pw } else { ph.max(48) };
            match Document::generate_replacement_code(active.kind, &active.text, pw, ph) {
                Ok(png_bytes) => {
                    let kind_name = format!("{:?}", active.kind).to_ascii_lowercase();
                    #[cfg(target_arch = "wasm32")]
                    {
                        let filename = format!("barcode_{}_{}.png", kind_name, now as u64);
                        let _ = crate::editing::download(&filename, &png_bytes);
                        state.show_toast(format!("Downloaded {filename}!"), now);
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let filename = {
                            let mut n = 0u64;
                            loop {
                                let name = format!("barcode_{}_{}_{}.png", kind_name, (now as u64) % 10000, n);
                                if !get_downloads_dir().join(&name).exists() {
                                    break name;
                                }
                                n = n.saturating_add(1);
                                if n > 1000 {
                                    break format!("barcode_{}_{}.png", kind_name, now as u64);
                                }
                            }
                        };
                        let dest = get_downloads_dir().join(&filename);
                        match std::fs::write(&dest, &png_bytes) {
                            Ok(()) => state.show_toast(format!("Saved to Downloads: {filename}"), now),
                            Err(e) => state.show_toast(format!("Download failed: {e}"), now),
                        }
                    }
                }
                Err(e) => {
                    state.show_toast(format!("Download failed: {e}"), now);
                }
            }
        }
        if let Some(text) = copy_text {
            ui.ctx().copy_text(text);
            state.show_toast("Copied payload to clipboard!".into(), now);
        }
        if step_prev {
            state.prev_code();
        }
        if step_next {
            state.next_code();
        }
        if close_hud {
            state.panel_open = false;
        }

        if trigger_generate {
            let text = state.qr_input.trim();
            let kind = state.generate_kind;
            let (pw, ph, rect_w, rect_h) = match kind {
                pdfcraft_engine::barcode_scan::CodeKind::QrCode => (240, 240, 144.0, 144.0),
                _ => (360, 120, 216.0, 72.0),
            };
            match Document::generate_replacement_code(kind, text, pw, ph) {
                Ok(png_bytes) => {
                    let (page_w, page_h) = info.pages.get(current_page).map(|p| (p.width as f64, p.height as f64)).unwrap_or((612.0, 792.0));
                    let x = (page_w - rect_w) * 0.5;
                    let y = (page_h - rect_h) * 0.5;
                    let rect = [x, y, x + rect_w, y + rect_h];
                    let filename = match kind {
                        pdfcraft_engine::barcode_scan::CodeKind::QrCode => "QRCode.png",
                        pdfcraft_engine::barcode_scan::CodeKind::Code39 => "Code39.png",
                        pdfcraft_engine::barcode_scan::CodeKind::Code128 => "Code128.png",
                        pdfcraft_engine::barcode_scan::CodeKind::Ean13 => "EAN13.png",
                        _ => "Barcode.png",
                    };

                    pending_edit =
                        Some(Edit::AddImage { page: current_page, rect: Some(rect), name: filename.into(), bytes: Arc::new(png_bytes) });
                    state.show_toast(format!("{} placed on page!", kind.label()), now);
                }
                Err(e) => {
                    state.show_toast(format!("Generation error: {e}"), now);
                }
            }
        }

        pending_edit
    }

    /// Legacy compatibility stub for `show_panel` (functionality unified into `show`).
    pub fn show_panel(_ui: &mut egui::Ui, _view: &mut DocView, _doc: &Document, _info: &DocInfo, _can_modify: bool) -> Option<Edit> {
        None
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
        let (w_rect, img_rect, pw, _ph) =
            safe_rect_and_pixels(rect, pixel_size, CodeKind::Code128, Some("1234"), Some("12345"));
        assert_eq!(img_rect[0], 10.0, "left must stay anchored");
        assert!(img_rect[2] > 110.0, "right must expand to accommodate 5th digit");
        assert!(pw > 100, "pixel width must scale to keep module thickness");
        assert!(w_rect[2] >= img_rect[2], "whiteout must cover expanded right edge");
    }
}
