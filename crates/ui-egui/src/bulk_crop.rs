//! Bulk Crop dialog (multi-page bulk cropping), M4.4.
//!
//! Provides three preview modes:
//! 1. Blended: Superimposes multiple pages to align crop margins across all text envelopes.
//! 2. Odd & Even: Independent crop margins for odd and even pages (facing pages / book gutters).
//! 3. Page by Page: Individual page margins for granular per-page adjustments.
//!
//! Includes vertical thumbnail strip, content-aware Auto-crop, and interactive on-screen drag handles.

use std::collections::HashSet;

use egui::{Align, Align2, Color32, CornerRadius, CursorIcon, Layout, Pos2, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::{BoxSpec, Edit, PageBox};

use crate::theme::{self, Tokens};
use crate::units::Unit;
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkCropMode {
    Blended,
    OddEven,
    PageByPage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OddEvenTab {
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRangeChoice {
    All,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragHandle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
    Move,
}

const MIN_CROP_PT: f64 = 18.0;
const COLUMN_HEIGHT: f32 = 480.0;

/// Active drag gesture on an on-screen crop box.
#[derive(Clone, Copy, Debug)]
struct ActiveDrag {
    owner_id: &'static str,
    handle: DragHandle,
    start_pointer: Pos2,
    start_margins: [f64; 4],
}

/// State for the Bulk Crop dialog. Margins are kept in PDF points from MediaBox: [left, bottom, right, top].
#[derive(Clone, Debug)]
pub struct BulkCropDraft {
    pub mode: BulkCropMode,
    pub unit: Unit,
    pub range_choice: PageRangeChoice,
    pub range_from: usize,
    pub range_to: usize,
    pub blend_count: usize,
    /// Uniform margins for Blended mode: [left, bottom, right, top] in points from MediaBox.
    pub blended_margins: [f64; 4],
    /// Margins for Odd pages in Odd & Even mode.
    pub odd_margins: [f64; 4],
    /// Margins for Even pages in Odd & Even mode.
    pub even_margins: [f64; 4],
    /// Per-page margins for Page-by-Page mode: (page_index, margins).
    pub page_margins: Vec<[f64; 4]>,
    /// Initial margins when the dialog was opened (used for [ Reset ]).
    pub initial_blended_margins: [f64; 4],
    pub initial_odd_margins: [f64; 4],
    pub initial_even_margins: [f64; 4],
    pub initial_page_margins: Vec<[f64; 4]>,
    /// Original page boxes before temporary uncrop.
    pub original_boxes: Vec<[[f64; 4]; 5]>,
    /// Pages in Page-by-Page mode explicitly edited or reverted by the user.
    pub edited_pages: HashSet<usize>,
    /// Pages marked to revert crop back to full MediaBox.
    pub revert_pages: HashSet<usize>,
    /// Active page being edited / previewed.
    pub current_page: usize,
    /// Initialized flag.
    pub seeded: bool,
    /// Flag indicating if the document was temporarily uncropped for the dialog preview.
    pub was_temporarily_uncropped: bool,
    /// Active tab in Odd & Even mode (Odd or Even).
    pub odd_even_tab: OddEvenTab,
    /// Active interactive handle drag.
    active_drag: Option<ActiveDrag>,
}

impl Default for BulkCropDraft {
    fn default() -> Self {
        Self {
            mode: BulkCropMode::Blended,
            unit: Unit::Inches,
            range_choice: PageRangeChoice::All,
            range_from: 1,
            range_to: 1,
            blend_count: 10,
            blended_margins: [0.0; 4],
            odd_margins: [0.0; 4],
            even_margins: [0.0; 4],
            page_margins: Vec::new(),
            initial_blended_margins: [0.0; 4],
            initial_odd_margins: [0.0; 4],
            initial_even_margins: [0.0; 4],
            initial_page_margins: Vec::new(),
            original_boxes: Vec::new(),
            edited_pages: HashSet::new(),
            revert_pages: HashSet::new(),
            current_page: 0,
            seeded: false,
            was_temporarily_uncropped: false,
            odd_even_tab: OddEvenTab::Odd,
            active_drag: None,
        }
    }
}

impl BulkCropDraft {
    /// Target pages (0-based) for the crop operation.
    pub fn target_pages(&self, total_pages: usize) -> Vec<usize> {
        let total = total_pages.max(1);
        match self.range_choice {
            PageRangeChoice::All => (0..total).collect(),
            PageRangeChoice::Custom => {
                let from = (self.range_from.max(1) - 1).min(total.saturating_sub(1));
                let to = self.range_to.max(self.range_from).min(total);
                (from..to).collect()
            }
        }
    }

    /// Builds the engine Edit to apply when confirmed.
    pub fn build_edit(&self, total_pages: usize) -> Edit {
        let pages = self.target_pages(total_pages);
        match self.mode {
            BulkCropMode::Blended => {
                if self.revert_pages.len() == pages.len() || self.blended_margins.iter().all(|&v| v == 0.0) {
                    Edit::SetPageBox { pages, which: PageBox::Crop, spec: BoxSpec::Remove }
                } else {
                    Edit::SetPageBox { pages, which: PageBox::Crop, spec: BoxSpec::Margins(self.blended_margins) }
                }
            }
            BulkCropMode::OddEven => {
                let odd_p: Vec<usize> = pages.iter().copied().filter(|p| p % 2 == 0).collect();
                let even_p: Vec<usize> = pages.iter().copied().filter(|p| p % 2 != 0).collect();
                let mut edits = Vec::new();
                if !odd_p.is_empty() {
                    let spec = if self.odd_margins.iter().all(|&v| v == 0.0) { BoxSpec::Remove } else { BoxSpec::Margins(self.odd_margins) };
                    edits.push(Edit::SetPageBox { pages: odd_p, which: PageBox::Crop, spec });
                }
                if !even_p.is_empty() {
                    let spec = if self.even_margins.iter().all(|&v| v == 0.0) { BoxSpec::Remove } else { BoxSpec::Margins(self.even_margins) };
                    edits.push(Edit::SetPageBox { pages: even_p, which: PageBox::Crop, spec });
                }
                if edits.len() == 1 { edits.remove(0) } else { Edit::Batch { label: "Bulk crop pages".into(), edits } }
            }
            BulkCropMode::PageByPage => {
                let mut edits = Vec::new();
                for &p in &pages {
                    if self.revert_pages.contains(&p) {
                        edits.push(Edit::SetPageBox { pages: vec![p], which: PageBox::Crop, spec: BoxSpec::Remove });
                    } else if self.edited_pages.contains(&p) {
                        let m = self.page_margins.get(p).copied().unwrap_or([0.0; 4]);
                        let spec = if m.iter().all(|&v| v == 0.0) { BoxSpec::Remove } else { BoxSpec::Margins(m) };
                        edits.push(Edit::SetPageBox { pages: vec![p], which: PageBox::Crop, spec });
                    }
                }
                if edits.len() == 1 { edits.remove(0) } else { Edit::Batch { label: "Bulk crop pages".into(), edits } }
            }
        }
    }
}

/// Renders the Bulk Crop dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((view_idx, doc_id)) = app.active_ids() else { return (false, true) };
    let total_pages = app.session.get(doc_id).map(|d| d.info.pages.len()).unwrap_or(1).max(1);

    if !app.bulk_crop_draft.seeded {
        let unit = app.units;
        let cur = app.views.get(view_idx).map(|v| v.current).unwrap_or(0).min(total_pages.saturating_sub(1));
        let boxes = app.session.get(doc_id).map(|d| d.page_boxes()).unwrap_or_default();

        let has_existing_crop = boxes.iter().any(|b| {
            let (m, c) = (b[0], b[1]);
            (c[0] - m[0]).abs() > 0.5 || (c[1] - m[1]).abs() > 0.5 || (m[2] - c[2]).abs() > 0.5 || (m[3] - c[3]).abs() > 0.5
        });

        let existing_insets: Vec<[f64; 4]> = (0..total_pages)
            .map(|i| {
                if let Some(b) = boxes.get(i) {
                    let (m, c) = (b[0], b[1]);
                    [(c[0] - m[0]).max(0.0), (c[1] - m[1]).max(0.0), (m[2] - c[2]).max(0.0), (m[3] - c[3]).max(0.0)]
                } else {
                    [0.0; 4]
                }
            })
            .collect();

        let (blended_m, odd_m, even_m, page_auto_m) = if !has_existing_crop {
            let target: Vec<usize> = (0..total_pages).collect();
            let bm = app
                .session
                .detect_bulk_auto_crop(doc_id, &target, 18.0)
                .or_else(|| app.session.detect_page_auto_crop(doc_id, cur, 18.0))
                .unwrap_or([0.0; 4]);
            let odd_p: Vec<usize> = target.iter().copied().filter(|p| p % 2 == 0).collect();
            let even_p: Vec<usize> = target.iter().copied().filter(|p| p % 2 != 0).collect();
            let om = (!odd_p.is_empty()).then(|| app.session.detect_bulk_auto_crop(doc_id, &odd_p, 18.0)).flatten().unwrap_or(bm);
            let em = (!even_p.is_empty()).then(|| app.session.detect_bulk_auto_crop(doc_id, &even_p, 18.0)).flatten().unwrap_or(bm);
            let pm = app.session.detect_page_auto_crop(doc_id, cur, 18.0).unwrap_or([0.0; 4]);
            (bm, om, em, pm)
        } else {
            ([0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4])
        };

        if has_existing_crop {
            app.apply_edit(Edit::SetPageBox { pages: (0..total_pages).collect(), which: PageBox::Crop, spec: BoxSpec::Remove });
        }

        let d = &mut app.bulk_crop_draft;
        d.unit = unit;
        d.range_to = total_pages;
        d.revert_pages.clear();
        d.edited_pages.clear();
        d.original_boxes = boxes;
        d.current_page = cur;
        d.odd_even_tab = if cur % 2 == 0 { OddEvenTab::Odd } else { OddEvenTab::Even };

        if has_existing_crop {
            d.was_temporarily_uncropped = true;
            let cur_insets = existing_insets.get(cur).copied().unwrap_or([0.0; 4]);
            d.blended_margins = cur_insets;
            d.odd_margins = cur_insets;
            d.even_margins = cur_insets;
            d.page_margins = existing_insets;
        } else {
            d.was_temporarily_uncropped = false;
            d.blended_margins = blended_m;
            d.odd_margins = odd_m;
            d.even_margins = even_m;
            d.page_margins = vec![[0.0; 4]; total_pages];
            if let Some(slot) = d.page_margins.get_mut(cur) {
                *slot = page_auto_m;
            }
        }

        d.initial_blended_margins = d.blended_margins;
        d.initial_odd_margins = d.odd_margins;
        d.initial_even_margins = d.even_margins;
        d.initial_page_margins = d.page_margins.clone();
        d.seeded = true;
    }

    let d = &mut app.bulk_crop_draft;

    let Some(doc) = app.session.get(doc_id) else { return (false, true) };

    // Title: "Bulk Crop Pages"
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Bulk Crop Pages")).font(theme::semibold(20.0)));
    });
    ui.add_space(12.0);

    let current_page = d.current_page.min(total_pages.saturating_sub(1));
    d.current_page = current_page;
    let target_pages = d.target_pages(total_pages);
    let mut trigger_auto_crop = false;
    let mut trigger_reset = false;
    let mut trigger_clear_crop = false;

    // 3-Column Layout: [Thumbnail Strip (105px)] [Center Canvas Preview (530px)] [Right Controls (280px)]
    // All 3 columns share COLUMN_HEIGHT (480px) so both their tops and bottoms are perfectly aligned!
    ui.horizontal_top(|ui| {
        // --- 1. Left Thumbnail Strip (105px) ---
        let view = &mut app.views[view_idx];
        ui.vertical(|ui| {
            ui.set_width(105.0);
            ui.set_height(COLUMN_HEIGHT);
            render_thumbnail_strip(ui, view, doc, &mut d.current_page, &mut d.odd_even_tab, d.mode, total_pages, COLUMN_HEIGHT, t);
        });

        ui.add_space(12.0);

        // --- 2. Center Preview Canvas Area (530px) ---
        ui.vertical(|ui| {
            ui.set_width(530.0);
            ui.set_height(COLUMN_HEIGHT);

            let active_margins = match d.mode {
                BulkCropMode::Blended => &mut d.blended_margins,
                BulkCropMode::OddEven => {
                    if d.odd_even_tab == OddEvenTab::Odd {
                        &mut d.odd_margins
                    } else {
                        &mut d.even_margins
                    }
                }
                BulkCropMode::PageByPage => {
                    if d.page_margins.len() < total_pages {
                        d.page_margins.resize(total_pages, [0.0; 4]);
                    }
                    &mut d.page_margins[current_page]
                }
            };

            // Blended previews:
            // - Blended mode: blends up to blend_count target pages.
            // - Odd & even mode: blends odd pages when Odd tab is active, even pages when Even tab is active!
            // - Page by page mode: shows the single current page.
            let preview_pages: Vec<usize> = match d.mode {
                BulkCropMode::Blended => target_pages.iter().copied().take(d.blend_count).collect(),
                BulkCropMode::OddEven => {
                    let is_odd = d.odd_even_tab == OddEvenTab::Odd;
                    let sub: Vec<usize> = target_pages.iter().copied().filter(|&p| (p % 2 == 0) == is_odd).take(d.blend_count).collect();
                    if sub.is_empty() { vec![current_page] } else { sub }
                }
                BulkCropMode::PageByPage => vec![current_page],
            };

            // Use displayed page dimensions (MediaBox) so the preview paper reflects the full uncropped sheet
            let base_page = preview_pages.first().copied().unwrap_or(current_page);
            let page_size = if let Some(b) = d.original_boxes.get(base_page) {
                let m = b[0];
                (((m[2] - m[0]) as f32).max(1.0), ((m[3] - m[1]) as f32).max(1.0))
            } else {
                doc.info.pages.get(base_page).map(|p| (p.width, p.height)).unwrap_or((612.0, 792.0))
            };

            let view = &mut app.views[view_idx];
            let mut handle_dragged = false;
            render_interactive_preview(
                ui,
                view,
                &preview_pages,
                page_size,
                active_margins,
                d.mode,
                &mut d.odd_even_tab,
                &mut d.current_page,
                total_pages,
                COLUMN_HEIGHT,
                &mut d.active_drag,
                &mut handle_dragged,
                "main-preview",
                t,
            );
            if handle_dragged && d.mode == BulkCropMode::PageByPage {
                d.edited_pages.insert(current_page);
                d.revert_pages.remove(&current_page);
            }
        });

        ui.add_space(12.0);

        // --- 3. Right Controls Panel (280px) ---
        ui.vertical(|ui| {
            ui.set_width(280.0);
            ui.set_height(COLUMN_HEIGHT);
            render_controls_sidebar(
                ui,
                d,
                total_pages,
                current_page,
                COLUMN_HEIGHT,
                &mut trigger_auto_crop,
                &mut trigger_reset,
                &mut trigger_clear_crop,
                t,
            );
        });
    });

    // Handle Auto-crop trigger:
    // - Blended mode: scans all target pages and fits the safe envelope across all pages.
    // - Odd & Even mode: scans all odd pages for odd margins, and all even pages for even margins.
    // - Page by Page mode: scans the active page.
    if trigger_auto_crop {
        match d.mode {
            BulkCropMode::Blended => {
                let pages = d.target_pages(total_pages);
                if let Some(m) = app.session.detect_bulk_auto_crop(doc_id, &pages, 18.0) {
                    d.blended_margins = m;
                } else if let Some(m) = app.session.detect_page_auto_crop(doc_id, current_page, 18.0) {
                    d.blended_margins = m;
                }
            }
            BulkCropMode::OddEven => {
                let pages = d.target_pages(total_pages);
                let odd_p: Vec<usize> = pages.iter().copied().filter(|p| p % 2 == 0).collect();
                let even_p: Vec<usize> = pages.iter().copied().filter(|p| p % 2 != 0).collect();

                if !odd_p.is_empty() {
                    if let Some(m) = app.session.detect_bulk_auto_crop(doc_id, &odd_p, 18.0) {
                        d.odd_margins = m;
                    } else if let Some(m) = app.session.detect_page_auto_crop(doc_id, odd_p[0], 18.0) {
                        d.odd_margins = m;
                    }
                }
                if !even_p.is_empty() {
                    if let Some(m) = app.session.detect_bulk_auto_crop(doc_id, &even_p, 18.0) {
                        d.even_margins = m;
                    } else if let Some(m) = app.session.detect_page_auto_crop(doc_id, even_p[0], 18.0) {
                        d.even_margins = m;
                    }
                }
            }
            BulkCropMode::PageByPage => {
                if let Some(m) = app.session.detect_page_auto_crop(doc_id, current_page, 18.0) {
                    d.edited_pages.insert(current_page);
                    d.revert_pages.remove(&current_page);
                    if let Some(slot) = d.page_margins.get_mut(current_page) {
                        *slot = m;
                    }
                }
            }
        }
    }

    // Handle Reset trigger: restores crop margins back to initial state upon opening dialog
    if trigger_reset {
        match d.mode {
            BulkCropMode::Blended => {
                d.blended_margins = d.initial_blended_margins;
                d.revert_pages.clear();
                d.edited_pages.clear();
            }
            BulkCropMode::OddEven => {
                d.odd_margins = d.initial_odd_margins;
                d.even_margins = d.initial_even_margins;
                d.revert_pages.clear();
                d.edited_pages.clear();
            }
            BulkCropMode::PageByPage => {
                if let Some(slot) = d.page_margins.get_mut(current_page) {
                    *slot = d.initial_page_margins.get(current_page).copied().unwrap_or([0.0; 4]);
                }
                d.revert_pages.remove(&current_page);
                d.edited_pages.remove(&current_page);
            }
        }
    }

    // Handle Clear crop trigger: removes crop (full sheet / margins = 0.0)
    if trigger_clear_crop {
        match d.mode {
            BulkCropMode::Blended => {
                for &p in &target_pages {
                    d.revert_pages.insert(p);
                    d.edited_pages.insert(p);
                }
                d.blended_margins = [0.0; 4];
            }
            BulkCropMode::OddEven => {
                for &p in &target_pages {
                    d.revert_pages.insert(p);
                    d.edited_pages.insert(p);
                }
                d.odd_margins = [0.0; 4];
                d.even_margins = [0.0; 4];
            }
            BulkCropMode::PageByPage => {
                d.revert_pages.insert(current_page);
                d.edited_pages.insert(current_page);
                if let Some(slot) = d.page_margins.get_mut(current_page) {
                    *slot = [0.0; 4];
                }
            }
        }
    }

    // Bottom Action Bar: Navigation buttons centered under canvas, Cancel and Crop Pages buttons on the right.
    // Both are on the same vertical line, perfectly aligned.
    ui.add_space(14.0);
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);

        // Center pager horizontally under the center canvas:
        // Thumbnail strip is 105px + 12px space + (530px canvas / 2) = 382px center.
        // Pager width is ~170px, so offset is 382 - 85 = 297px from left.
        ui.add_space(297.0);

        let prev_enabled = current_page > 0;
        let prev_btn = egui::Button::new("◀").fill(Color32::WHITE).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(3));
        if ui.add_sized(vec2(28.0, 28.0), prev_btn).clicked() && prev_enabled {
            d.current_page = current_page - 1;
            if d.mode == BulkCropMode::OddEven {
                d.odd_even_tab = if (current_page - 1) % 2 == 0 { OddEvenTab::Odd } else { OddEvenTab::Even };
            }
        }

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(current_page + 1).to_string()), ("n", &total_pages.to_string())]))
                .font(theme::medium(13.0)),
        );
        ui.add_space(4.0);

        let next_enabled = current_page + 1 < total_pages;
        let next_btn = egui::Button::new("▶").fill(Color32::WHITE).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(3));
        if ui.add_sized(vec2(28.0, 28.0), next_btn).clicked() && next_enabled {
            d.current_page = current_page + 1;
            if d.mode == BulkCropMode::OddEven {
                d.odd_even_tab = if (current_page + 1) % 2 == 0 { OddEvenTab::Odd } else { OddEvenTab::Even };
            }
        }

        // Right-aligned Cancel and Crop Pages buttons
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let crop_btn = egui::Button::new(egui::RichText::new(tl!("Crop Pages")).color(Color32::WHITE).font(theme::medium(13.5)))
                .fill(Color32::from_rgb(26, 115, 232))
                .corner_radius(CornerRadius::same(4));

            if ui.add_sized(vec2(105.0, 32.0), crop_btn).clicked() {
                apply = true;
            }

            ui.add_space(10.0);

            let cancel_btn = egui::Button::new(egui::RichText::new(tl!("Cancel")).color(Color32::from_gray(50)).font(theme::medium(13.5)))
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, Color32::from_gray(200)))
                .corner_radius(CornerRadius::same(4));

            if ui.add_sized(vec2(75.0, 32.0), cancel_btn).clicked() {
                cancel = true;
            }
        });
    });

    (apply, cancel)
}

/// Renders the left vertical thumbnail list with border frame.
fn render_thumbnail_strip(
    ui: &mut egui::Ui,
    view: &mut crate::canvas::DocView,
    doc: &pdfcraft_engine::Document,
    current_page: &mut usize,
    odd_even_tab: &mut OddEvenTab,
    mode: BulkCropMode,
    total_pages: usize,
    col_h: f32,
    t: &Tokens,
) {
    let strip_w = 105.0;
    egui::Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(6.0).show(
        ui,
        |ui| {
            ui.set_width(strip_w);
            ui.set_height(col_h - 12.0);
            ui.set_min_height(col_h - 12.0);
            egui::ScrollArea::vertical().id_salt("bulk_crop_thumbs").auto_shrink([false, false]).max_width(strip_w).max_height(col_h - 12.0).show(
                ui,
                |ui| {
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = vec2(0.0, 8.0);
                        for i in 0..total_pages {
                            let (pw, ph) = doc.info.pages.get(i).map(|p| (p.width as f32, p.height as f32)).unwrap_or((612.0, 792.0));
                            let thumb_w = 72.0;
                            let thumb_h = (thumb_w * ph / pw.max(1.0)).clamp(55.0, 100.0);

                            let card_size = vec2(thumb_w + 10.0, thumb_h + 10.0);
                            let (card_rect, resp) = ui.allocate_exact_size(card_size, Sense::click());
                            let is_selected = i == *current_page;

                            if resp.clicked() {
                                *current_page = i;
                                if mode == BulkCropMode::OddEven {
                                    *odd_even_tab = if i % 2 == 0 { OddEvenTab::Odd } else { OddEvenTab::Even };
                                }
                            }

                            let card_fill = if is_selected { Color32::from_rgb(232, 240, 254) } else { Color32::WHITE };
                            let card_stroke = if is_selected {
                                Stroke::new(1.5, Color32::from_rgb(26, 115, 232))
                            } else {
                                Stroke::new(1.0, Color32::from_gray(220))
                            };
                            ui.painter().rect_filled(card_rect, CornerRadius::same(4), card_fill);
                            ui.painter().rect_stroke(card_rect, CornerRadius::same(4), card_stroke, egui::StrokeKind::Inside);

                            let img_rect = Rect::from_center_size(card_rect.center(), vec2(thumb_w, thumb_h));
                            view.need_thumbnail(i, card_rect.intersects(ui.clip_rect()));
                            if let Some(tex) = view.thumb(i) {
                                ui.painter().image(tex.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            } else {
                                ui.painter().rect_filled(img_rect, CornerRadius::ZERO, Color32::WHITE);
                            }
                            ui.painter().rect_stroke(
                                img_rect,
                                CornerRadius::ZERO,
                                Stroke::new(1.0, Color32::from_gray(215)),
                                egui::StrokeKind::Inside,
                            );

                            let label_color = if is_selected { Color32::from_rgb(26, 115, 232) } else { t.text_muted };
                            ui.label(egui::RichText::new((i + 1).to_string()).font(theme::medium(12.0)).color(label_color));
                        }
                    });
                },
            );
        },
    );
}

/// Renders the right controls sidebar card.
fn render_controls_sidebar(
    ui: &mut egui::Ui,
    d: &mut BulkCropDraft,
    total_pages: usize,
    current_page: usize,
    col_h: f32,
    trigger_auto_crop: &mut bool,
    trigger_reset: &mut bool,
    trigger_clear_crop: &mut bool,
    t: &Tokens,
) {
    let panel_w = 280.0;
    egui::Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(14.0).show(
        ui,
        |ui| {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.set_width(panel_w - 28.0);
                ui.set_min_height(col_h - 28.0);

                // --- 1. Preview mode ---
                ui.label(egui::RichText::new(tl!("Preview mode")).font(theme::semibold(13.5)));
                ui.add_space(6.0);
                let prev_mode = d.mode;
                ui.horizontal(|ui| {
                    widgets::radio_value(ui, &mut d.mode, BulkCropMode::Blended, tl!("Blended"));
                    widgets::radio_value(ui, &mut d.mode, BulkCropMode::OddEven, tl!("Odd & even"));
                    widgets::radio_value(ui, &mut d.mode, BulkCropMode::PageByPage, tl!("Page by page"));
                });

                // Smooth mode switching: preserve and propagate configured crop margins
                if d.mode != prev_mode {
                    match d.mode {
                        BulkCropMode::PageByPage => {
                            if prev_mode == BulkCropMode::Blended && d.blended_margins.iter().any(|&v| v > 0.0) {
                                for m in &mut d.page_margins {
                                    *m = d.blended_margins;
                                }
                            } else if prev_mode == BulkCropMode::OddEven {
                                for (i, m) in d.page_margins.iter_mut().enumerate() {
                                    *m = if i % 2 == 0 { d.odd_margins } else { d.even_margins };
                                }
                            }
                        }
                        BulkCropMode::OddEven => {
                            d.odd_even_tab = if current_page % 2 == 0 { OddEvenTab::Odd } else { OddEvenTab::Even };
                            if prev_mode == BulkCropMode::Blended && d.blended_margins.iter().any(|&v| v > 0.0) {
                                d.odd_margins = d.blended_margins;
                                d.even_margins = d.blended_margins;
                            }
                        }
                        BulkCropMode::Blended => {
                            if let Some(&m) = d.page_margins.get(current_page) {
                                if m.iter().any(|&v| v > 0.0) {
                                    d.blended_margins = m;
                                }
                            }
                        }
                    }
                }

                ui.add_space(16.0);

                // --- 2. Pages ---
                ui.label(egui::RichText::new(tl!("Pages")).font(theme::semibold(13.5)));
                ui.add_space(6.0);
                widgets::radio_value(ui, &mut d.range_choice, PageRangeChoice::All, tl!("All"));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let custom = d.range_choice == PageRangeChoice::Custom;
                    if ui.add(widgets::radio(custom, tl!("Range"))).clicked() {
                        d.range_choice = PageRangeChoice::Custom;
                    }
                    ui.add_space(2.0);
                    ui.add_enabled_ui(custom, |ui| {
                        ui.label(egui::RichText::new(tl!("From")).color(t.text_muted));
                        ui.add_sized(vec2(44.0, 24.0), egui::DragValue::new(&mut d.range_from).range(1..=total_pages));
                        ui.label(egui::RichText::new(tl!("to")).color(t.text_muted));
                        ui.add_sized(vec2(44.0, 24.0), egui::DragValue::new(&mut d.range_to).range(d.range_from..=total_pages));
                    });
                });

                ui.add_space(16.0);

                // --- 3. Crop margins ---
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("Crop margins")).font(theme::semibold(13.5)));
                    if d.mode == BulkCropMode::OddEven {
                        let badge = if d.odd_even_tab == OddEvenTab::Odd { tl!("(Odd pages)") } else { tl!("(Even pages)") };
                        ui.label(egui::RichText::new(badge).font(theme::regular(12.0)).color(Color32::from_rgb(26, 115, 232)));
                    }
                });
                ui.add_space(8.0);

                // Segmented Unit Selector [ in | cm | pt ] above margin inputs
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                    let unit_w = (ui.available_width() - 12.0) / 3.0;
                    for (u, label) in [(Unit::Inches, "in"), (Unit::Centimetres, "cm"), (Unit::Points, "pt")] {
                        let is_sel = d.unit == u;
                        let fill = if is_sel { Color32::from_rgb(232, 240, 254) } else { Color32::WHITE };
                        let text_color = if is_sel { Color32::from_rgb(26, 115, 232) } else { Color32::from_gray(100) };
                        let btn = egui::Button::new(egui::RichText::new(label).color(text_color).font(theme::medium(12.0)))
                            .fill(fill)
                            .stroke(if is_sel {
                                Stroke::new(1.0, Color32::from_rgb(26, 115, 232))
                            } else {
                                Stroke::new(1.0, Color32::from_gray(215))
                            })
                            .corner_radius(CornerRadius::same(4));
                        if ui.add_sized(vec2(unit_w, 26.0), btn).clicked() {
                            d.unit = u;
                        }
                    }
                });
                ui.add_space(10.0);

                let k = d.unit.per_point();
                let speed = d.unit.speed();

                // Borrow active margin slice corresponding to the selected mode and Odd/Even tab
                let is_page_by_page = d.mode == BulkCropMode::PageByPage;
                let active_margins = match d.mode {
                    BulkCropMode::Blended => &mut d.blended_margins,
                    BulkCropMode::OddEven => {
                        if d.odd_even_tab == OddEvenTab::Odd {
                            &mut d.odd_margins
                        } else {
                            &mut d.even_margins
                        }
                    }
                    BulkCropMode::PageByPage => {
                        if d.page_margins.len() < total_pages {
                            d.page_margins.resize(total_pages, [0.0; 4]);
                        }
                        &mut d.page_margins[current_page]
                    }
                };

                let col_w = 118.0;
                let edit_h = 28.0;

                // Uniform 2x2 Grid:
                // Row 1: Top & Bottom
                // Row 2: Left & Right
                egui::Grid::new("crop_margins_grid").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| {
                    // Row 1: Top & Bottom
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(tl!("Top")).color(Color32::from_gray(80)));
                        ui.add_space(2.0);
                        let mut top = active_margins[3] * k;
                        if ui
                            .add_sized(vec2(col_w, edit_h), egui::DragValue::new(&mut top).speed(speed).range(0.0..=10_000.0).max_decimals(2))
                            .changed()
                        {
                            active_margins[3] = (top / k).max(0.0);
                            if is_page_by_page {
                                d.edited_pages.insert(current_page);
                                d.revert_pages.remove(&current_page);
                            }
                        }
                    });

                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(tl!("Bottom")).color(Color32::from_gray(80)));
                        ui.add_space(2.0);
                        let mut bottom = active_margins[1] * k;
                        if ui
                            .add_sized(vec2(col_w, edit_h), egui::DragValue::new(&mut bottom).speed(speed).range(0.0..=10_000.0).max_decimals(2))
                            .changed()
                        {
                            active_margins[1] = (bottom / k).max(0.0);
                            if is_page_by_page {
                                d.edited_pages.insert(current_page);
                                d.revert_pages.remove(&current_page);
                            }
                        }
                    });
                    ui.end_row();

                    // Row 2: Left & Right
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(tl!("Left")).color(Color32::from_gray(80)));
                        ui.add_space(2.0);
                        let mut left = active_margins[0] * k;
                        if ui
                            .add_sized(vec2(col_w, edit_h), egui::DragValue::new(&mut left).speed(speed).range(0.0..=10_000.0).max_decimals(2))
                            .changed()
                        {
                            active_margins[0] = (left / k).max(0.0);
                            if is_page_by_page {
                                d.edited_pages.insert(current_page);
                                d.revert_pages.remove(&current_page);
                            }
                        }
                    });

                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(tl!("Right")).color(Color32::from_gray(80)));
                        ui.add_space(2.0);
                        let mut right = active_margins[2] * k;
                        if ui
                            .add_sized(vec2(col_w, edit_h), egui::DragValue::new(&mut right).speed(speed).range(0.0..=10_000.0).max_decimals(2))
                            .changed()
                        {
                            active_margins[2] = (right / k).max(0.0);
                            if is_page_by_page {
                                d.edited_pages.insert(current_page);
                                d.revert_pages.remove(&current_page);
                            }
                        }
                    });
                    ui.end_row();
                });

                ui.add_space(14.0);

                let reset_tooltip = if d.mode == BulkCropMode::PageByPage {
                    tl!("Reset this page to its initial crop")
                } else {
                    tl!("Reset crop margins to initial state")
                };

                // Single Action Row: [ Reset ] [ Clear crop ] [ Auto-crop ]
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                    let btn_w = (ui.available_width() - 12.0) / 3.0;
                    let reset_btn = egui::Button::new(egui::RichText::new(tl!("Reset")).color(Color32::from_gray(60)).font(theme::medium(12.0)))
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, Color32::from_gray(215)))
                        .corner_radius(CornerRadius::same(4));

                    if ui.add_sized(vec2(btn_w, 32.0), reset_btn).on_hover_text(reset_tooltip).clicked() {
                        *trigger_reset = true;
                    }

                    let clear_tooltip = if d.mode == BulkCropMode::PageByPage {
                        tl!("Remove crop from this page (full sheet)")
                    } else {
                        tl!("Remove crop across all target pages (full sheet)")
                    };
                    let clear_btn =
                        egui::Button::new(egui::RichText::new(tl!("Clear crop")).color(Color32::from_rgb(180, 45, 45)).font(theme::medium(12.0)))
                            .fill(Color32::WHITE)
                            .stroke(Stroke::new(1.0, Color32::from_gray(215)))
                            .corner_radius(CornerRadius::same(4));

                    if ui.add_sized(vec2(btn_w, 32.0), clear_btn).on_hover_text(clear_tooltip).clicked() {
                        *trigger_clear_crop = true;
                    }

                    let auto_btn =
                        egui::Button::new(egui::RichText::new(tl!("Auto-crop")).color(Color32::from_rgb(26, 115, 232)).font(theme::medium(12.0)))
                            .fill(Color32::WHITE)
                            .stroke(Stroke::new(1.0, Color32::from_rgb(26, 115, 232)))
                            .corner_radius(CornerRadius::same(4));

                    if ui.add_sized(vec2(btn_w, 32.0), auto_btn).on_hover_text(tl!("Detect margins automatically from content")).clicked() {
                        *trigger_auto_crop = true;
                    }
                });
            });
        },
    );
}

/// Renders the visual page preview with interactive crop handles.
/// In Odd & Even mode, renders the dual tab pill at the top of the canvas and handles Odd vs Even switching.
#[allow(clippy::too_many_arguments)]
fn render_interactive_preview(
    ui: &mut egui::Ui,
    view: &mut crate::canvas::DocView,
    pages: &[usize],
    page_size: (f32, f32),
    margins: &mut [f64; 4],
    mode: BulkCropMode,
    odd_even_tab: &mut OddEvenTab,
    current_page: &mut usize,
    total_pages: usize,
    col_h: f32,
    active_drag: &mut Option<ActiveDrag>,
    handle_dragged: &mut bool,
    id_salt: &'static str,
    t: &Tokens,
) {
    let preview_area_size = vec2(530.0, col_h);
    let (response, painter) = ui.allocate_painter(preview_area_size, Sense::drag());
    let r = response.rect;

    // Canvas background: medium neutral gray as in design mockup
    let canvas_bg = Color32::from_rgb(108, 117, 125);
    painter.rect_filled(r, CornerRadius::same(4), canvas_bg);
    painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);

    let is_odd_even = mode == BulkCropMode::OddEven;

    // Render Dual Tabs for Odd & Even mode at the top of the canvas
    if is_odd_even {
        let pill_w = 320.0;
        let pill_h = 32.0;
        let pill_rect = Rect::from_center_size(pos2(r.center().x, r.top() + 26.0), vec2(pill_w, pill_h));

        // Pill background container
        painter.rect_filled(pill_rect, CornerRadius::same(16), Color32::WHITE);
        painter.rect_stroke(pill_rect, CornerRadius::same(16), Stroke::new(1.0, Color32::from_gray(215)), egui::StrokeKind::Outside);

        let inner_pad = 2.5;
        let tab_w = (pill_w - inner_pad * 2.0) / 2.0;
        let tab_h = pill_h - inner_pad * 2.0;
        let odd_rect = Rect::from_min_size(pos2(pill_rect.left() + inner_pad, pill_rect.top() + inner_pad), vec2(tab_w, tab_h));
        let even_rect = Rect::from_min_size(pos2(pill_rect.left() + inner_pad + tab_w, pill_rect.top() + inner_pad), vec2(tab_w, tab_h));

        // Interactive hover & clicks on Odd / Even tabs
        if let Some(ptr) = ui.input(|i| i.pointer.hover_pos()) {
            if odd_rect.contains(ptr) {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)) {
                    *odd_even_tab = OddEvenTab::Odd;
                    if *current_page % 2 != 0 {
                        *current_page = current_page.saturating_sub(1);
                    }
                }
            } else if even_rect.contains(ptr) {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)) {
                    *odd_even_tab = OddEvenTab::Even;
                    if *current_page % 2 == 0 && *current_page + 1 < total_pages {
                        *current_page += 1;
                    }
                }
            }
        }

        let is_odd_active = *odd_even_tab == OddEvenTab::Odd;

        // Odd pages tab (active = soft blue fill, inactive = transparent)
        let odd_fill = if is_odd_active { Color32::from_rgb(219, 234, 254) } else { Color32::TRANSPARENT };
        let odd_text_color = if is_odd_active { Color32::from_rgb(26, 115, 232) } else { Color32::from_gray(80) };
        painter.rect_filled(odd_rect, CornerRadius::same(14), odd_fill);
        painter.text(odd_rect.center(), Align2::CENTER_CENTER, tl!("Odd pages (1, 3, 5, …)"), theme::medium(12.5), odd_text_color);

        // Even pages tab (active = soft blue fill, inactive = transparent)
        let even_fill = if !is_odd_active { Color32::from_rgb(219, 234, 254) } else { Color32::TRANSPARENT };
        let even_text_color = if !is_odd_active { Color32::from_rgb(26, 115, 232) } else { Color32::from_gray(80) };
        painter.rect_filled(even_rect, CornerRadius::same(14), even_fill);
        painter.text(even_rect.center(), Align2::CENTER_CENTER, tl!("Even pages (2, 4, 6, …)"), theme::medium(12.5), even_text_color);
    }

    // Dynamic aspect ratio based on actual PDF page dimensions
    let (page_w_pt, page_h_pt) = (page_size.0.max(1.0), page_size.1.max(1.0));
    let max_w = page_w_pt as f64;
    let max_h = page_h_pt as f64;

    let top_inset = if is_odd_even { 54.0 } else { 20.0 };
    let bottom_inset = 20.0;
    let avail_w = r.width() - 48.0;
    let avail_h = r.height() - (top_inset + bottom_inset);

    let scale = (avail_w / page_w_pt).min(avail_h / page_h_pt);
    let page_center = pos2(r.center().x, r.top() + top_inset + avail_h / 2.0);
    let page_rect = Rect::from_center_size(page_center, vec2(page_w_pt * scale, page_h_pt * scale));

    // Page drop shadow
    painter.rect_filled(page_rect.translate(vec2(2.0, 3.0)), CornerRadius::ZERO, Color32::from_black_alpha(45));
    // White page sheet
    painter.rect_filled(page_rect, CornerRadius::ZERO, Color32::WHITE);
    painter.rect_stroke(page_rect, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Outside);

    // Render page raster (blended when multiple pages)
    // Use higher dynamic opacity (95..160) so text outlines from all pages stand out with crisp contrast through the paper
    let alpha = if pages.len() <= 1 {
        255
    } else {
        let n = pages.len() as f32;
        ((240.0 / n.sqrt()).clamp(95.0, 160.0)) as u8
    };
    let tint = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);

    for &page in pages {
        view.need_thumbnail(page, true);
        if let Some(tex_id) = view.thumb_id(page) {
            painter.image(tex_id, page_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
    }

    // Clamp margins to ensure crop_rect always stays valid and positive
    margins[0] = margins[0].clamp(0.0, (max_w - MIN_CROP_PT).max(0.0));
    margins[2] = margins[2].clamp(0.0, (max_w - margins[0] - MIN_CROP_PT).max(0.0));
    margins[3] = margins[3].clamp(0.0, (max_h - MIN_CROP_PT).max(0.0));
    margins[1] = margins[1].clamp(0.0, (max_h - margins[3] - MIN_CROP_PT).max(0.0));

    // Map margins to on-screen crop box
    let [l, bo, ri, to] = margins.map(|v| (v as f32) * scale);
    let crop_rect = Rect::from_min_max(pos2(page_rect.left() + l, page_rect.top() + to), pos2(page_rect.right() - ri, page_rect.bottom() - bo));

    // Dim the exterior cut-away area
    for band in [
        Rect::from_min_max(page_rect.min, pos2(page_rect.right(), crop_rect.top())),
        Rect::from_min_max(pos2(page_rect.left(), crop_rect.bottom()), page_rect.max),
        Rect::from_min_max(pos2(page_rect.left(), crop_rect.top()), pos2(crop_rect.left(), crop_rect.bottom())),
        Rect::from_min_max(pos2(crop_rect.right(), crop_rect.top()), pos2(page_rect.right(), crop_rect.bottom())),
    ] {
        if band.is_positive() {
            painter.rect_filled(band, CornerRadius::ZERO, Color32::from_black_alpha(85));
        }
    }

    // Draw crop border in accent blue
    let crop_stroke_color = Color32::from_rgb(26, 115, 232);
    painter.rect_stroke(crop_rect, CornerRadius::ZERO, Stroke::new(1.5, crop_stroke_color), egui::StrokeKind::Middle);

    // Draw 8 interactive handles
    let handle_size = 6.0f32;
    let handle_pts = [
        (DragHandle::Nw, crop_rect.left_top()),
        (DragHandle::N, pos2(crop_rect.center().x, crop_rect.top())),
        (DragHandle::Ne, crop_rect.right_top()),
        (DragHandle::E, pos2(crop_rect.right(), crop_rect.center().y)),
        (DragHandle::Se, crop_rect.right_bottom()),
        (DragHandle::S, pos2(crop_rect.center().x, crop_rect.bottom())),
        (DragHandle::Sw, crop_rect.left_bottom()),
        (DragHandle::W, pos2(crop_rect.left(), crop_rect.center().y)),
    ];

    for &(_, pt) in &handle_pts {
        let h_rect = Rect::from_center_size(pt, vec2(handle_size, handle_size));
        painter.rect_filled(h_rect, CornerRadius::same(1), Color32::WHITE);
        painter.rect_stroke(h_rect, CornerRadius::same(1), Stroke::new(1.0, crop_stroke_color), egui::StrokeKind::Outside);
    }

    // Pointer hover & drag start (scoped to this widget's rect)
    if response.hovered() {
        if let Some(ptr) = ui.input(|i| i.pointer.hover_pos()) {
            let mut hovered_handle = None;
            for &(h, pt) in &handle_pts {
                if pt.distance(ptr) <= handle_size + 4.0 {
                    hovered_handle = Some(h);
                    break;
                }
            }

            let cursor = match hovered_handle {
                Some(DragHandle::Nw | DragHandle::Se) => CursorIcon::ResizeNwSe,
                Some(DragHandle::Ne | DragHandle::Sw) => CursorIcon::ResizeNeSw,
                Some(DragHandle::N | DragHandle::S) => CursorIcon::ResizeVertical,
                Some(DragHandle::E | DragHandle::W) => CursorIcon::ResizeHorizontal,
                None if crop_rect.contains(ptr) => CursorIcon::Move,
                _ => CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(cursor);

            if response.drag_started() {
                if let Some(handle) = hovered_handle.or_else(|| crop_rect.contains(ptr).then_some(DragHandle::Move)) {
                    *active_drag = Some(ActiveDrag { owner_id: id_salt, handle, start_pointer: ptr, start_margins: *margins });
                }
            }
        }
    }

    // Handle active drag (only if owned by this preview instance)
    if let Some(drag) = *active_drag {
        if drag.owner_id == id_salt {
            if ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary)) {
                if let Some(curr_ptr) = ui.input(|i| i.pointer.latest_pos()) {
                    let delta = curr_ptr - drag.start_pointer;
                    let dx = (delta.x / scale) as f64;
                    let dy = (delta.y / scale) as f64;

                    let mut m = drag.start_margins;
                    match drag.handle {
                        DragHandle::Nw => {
                            m[0] = (m[0] + dx).clamp(0.0, (max_w - m[2] - MIN_CROP_PT).max(0.0));
                            m[3] = (m[3] + dy).clamp(0.0, (max_h - m[1] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::N => {
                            m[3] = (m[3] + dy).clamp(0.0, (max_h - m[1] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::Ne => {
                            m[2] = (m[2] - dx).clamp(0.0, (max_w - m[0] - MIN_CROP_PT).max(0.0));
                            m[3] = (m[3] + dy).clamp(0.0, (max_h - m[1] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::E => {
                            m[2] = (m[2] - dx).clamp(0.0, (max_w - m[0] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::Se => {
                            m[2] = (m[2] - dx).clamp(0.0, (max_w - m[0] - MIN_CROP_PT).max(0.0));
                            m[1] = (m[1] - dy).clamp(0.0, (max_h - m[3] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::S => {
                            m[1] = (m[1] - dy).clamp(0.0, (max_h - m[3] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::Sw => {
                            m[0] = (m[0] + dx).clamp(0.0, (max_w - m[2] - MIN_CROP_PT).max(0.0));
                            m[1] = (m[1] - dy).clamp(0.0, (max_h - m[3] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::W => {
                            m[0] = (m[0] + dx).clamp(0.0, (max_w - m[2] - MIN_CROP_PT).max(0.0));
                        }
                        DragHandle::Move => {
                            let clamped_dx = dx.clamp(-m[0], m[2]);
                            let clamped_dy = dy.clamp(-m[3], m[1]);
                            m[0] += clamped_dx;
                            m[2] -= clamped_dx;
                            m[3] += clamped_dy;
                            m[1] -= clamped_dy;
                        }
                    };
                    if *margins != m {
                        *margins = m;
                        *handle_dragged = true;
                    }
                }
            } else {
                *active_drag = None;
            }
        }
    }
}
