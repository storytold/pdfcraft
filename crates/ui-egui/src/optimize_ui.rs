//! Optimize PDF ▸ Advanced optimization (Acrobat's PDF Optimizer): Images, Discard Objects,
//! Discard User Data and Clean Up panels. The result is saved as a copy, like Reduce File Size.

use egui::{Align, Layout};
use pdfcraft_engine::Hidden;
use pdfcraft_engine::optimize::{Compression, ImageSettings, QUALITIES, Settings};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizeTab {
    Images,
    DiscardObjects,
    DiscardUserData,
    CleanUp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptimizeDraft {
    pub tab: OptimizeTab,
    pub settings: Settings,
    /// Remove Hidden Information categories to discard.
    pub discard: Vec<Hidden>,
    /// "Audit space usage…" was pressed.
    pub audit: bool,
}

impl Default for OptimizeDraft {
    fn default() -> Self {
        Self { tab: OptimizeTab::Images, settings: Settings::default(), discard: Vec::new(), audit: false }
    }
}

fn image_row(ui: &mut egui::Ui, id: &str, title: &str, s: &mut ImageSettings) {
    ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.downsample, tl!("Bicubic downsampling to"));
        ui.add_enabled(s.downsample, egui::DragValue::new(&mut s.target_ppi).range(9.0..=2400.0).suffix(" ppi"));
        ui.label(tl!("for images above"));
        ui.add_enabled(s.downsample, egui::DragValue::new(&mut s.above_ppi).range(9.0..=2400.0).suffix(" ppi"));
    });
    s.above_ppi = s.above_ppi.max(s.target_ppi);
    ui.horizontal(|ui| {
        ui.label(tl!("Compression"));
        let label = match s.compression {
            Compression::Jpeg(_) => "JPEG",
            Compression::Flate => tl!("ZIP"),
            Compression::Retain => tl!("Retain existing"),
        };
        egui::ComboBox::from_id_salt((id, "compression")).selected_text(label).show_ui(ui, |ui| {
            let q = match s.compression {
                Compression::Jpeg(q) => q,
                _ => 60,
            };
            ui.selectable_value(&mut s.compression, Compression::Jpeg(q), "JPEG");
            ui.selectable_value(&mut s.compression, Compression::Flate, tl!("ZIP"));
            ui.selectable_value(&mut s.compression, Compression::Retain, tl!("Retain existing"));
        });
        if let Compression::Jpeg(q) = &mut s.compression {
            ui.label(tl!("Quality"));
            let name = QUALITIES.iter().min_by_key(|(_, v)| (*v as i32 - *q as i32).abs()).map_or(tl!("Medium"), |(n, _)| tl!(n));
            egui::ComboBox::from_id_salt((id, "quality")).selected_text(name).show_ui(ui, |ui| {
                for (n, v) in QUALITIES {
                    ui.selectable_value(q, v, tl!(n));
                }
            });
        }
    });
    ui.add_space(8.0);
}

fn discard_box(ui: &mut egui::Ui, list: &mut Vec<Hidden>, h: Hidden, label: &str) {
    let mut on = list.contains(&h);
    if ui.checkbox(&mut on, tl!(label)).changed() {
        if on {
            list.push(h);
        } else {
            list.retain(|x| *x != h);
        }
    }
}

/// Draw the dialog; returns (ok, cancel).
pub(crate) fn body(ui: &mut egui::Ui, d: &mut OptimizeDraft, t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("PDF Optimizer")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (tab, label) in [
            (OptimizeTab::Images, tl!("Images")),
            (OptimizeTab::DiscardObjects, tl!("Discard Objects")),
            (OptimizeTab::DiscardUserData, tl!("Discard User Data")),
            (OptimizeTab::CleanUp, tl!("Clean Up")),
        ] {
            if widgets::mode_tab(ui, label, d.tab == tab).clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    ui.add_space(6.0);
    let s = &mut d.settings;
    match d.tab {
        OptimizeTab::Images => {
            image_row(ui, "color", "Color Images", &mut s.color);
            image_row(ui, "gray", "Grayscale Images", &mut s.gray);
            ui.label(
                egui::RichText::new(tl!("Each image is measured where pages draw it; an image is replaced only if the result is smaller."))
                    .small()
                    .color(t.text_muted),
            );
        }
        OptimizeTab::DiscardObjects => {
            discard_box(ui, &mut d.discard, Hidden::LinksActionsScripts, "Discard all links, actions and JavaScript");
            ui.checkbox(&mut s.discard_alternate_images, tl!("Discard alternate images"));
            ui.checkbox(&mut s.discard_thumbnails, tl!("Discard embedded page thumbnails"));
            ui.checkbox(&mut s.discard_tags, tl!("Discard document tags"));
            ui.checkbox(&mut s.discard_print_settings, tl!("Discard embedded print settings"));
            discard_box(ui, &mut d.discard, Hidden::Bookmarks, "Discard bookmarks");
            discard_box(ui, &mut d.discard, Hidden::FormFields, "Flatten form fields");
            discard_box(ui, &mut d.discard, Hidden::HiddenLayers, "Discard hidden layer content");
        }
        OptimizeTab::DiscardUserData => {
            discard_box(ui, &mut d.discard, Hidden::Comments, "Discard all comments, forms and multimedia");
            discard_box(ui, &mut d.discard, Hidden::Metadata, "Discard document information and metadata");
            discard_box(ui, &mut d.discard, Hidden::Attachments, "Discard all object data (file attachments)");
            discard_box(ui, &mut d.discard, Hidden::PrivateData, "Discard private data of other applications");
            discard_box(ui, &mut d.discard, Hidden::HiddenText, "Discard hidden text");
        }
        OptimizeTab::CleanUp => {
            ui.checkbox(&mut s.flate_unencoded, tl!("Use Flate to encode streams that are not encoded"));
            ui.checkbox(&mut s.remove_invalid_links, tl!("Remove invalid links and bookmarks"));
            ui.checkbox(&mut s.remove_unreferenced_dests, tl!("Remove unreferenced named destinations"));
            ui.add_enabled(false, egui::Checkbox::new(&mut true, tl!("Compress document structure (object streams)")));
            ui.add_enabled(false, egui::Checkbox::new(&mut true, tl!("Remove unused objects and merge identical ones")));
        }
    }
    ui.add_space(12.0);
    let (mut ok, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Audit space usage…"), false).clicked() {
            d.audit = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                ok = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (ok, cancel)
}

/// Audit Space Usage: bytes and share of the file per kind of content.
pub(crate) fn audit_body(ui: &mut egui::Ui, rows: &[pdfcraft_engine::optimize::SpaceUse], t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Space Audit")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("space-audit").num_columns(3).striped(true).spacing([24.0, 4.0]).show(ui, |ui| {
        for h in [tl!("Description"), tl!("Bytes"), tl!("Percentage")] {
            ui.label(egui::RichText::new(h).color(t.text_muted));
        }
        ui.end_row();
        let total: u64 = rows.iter().map(|r| r.bytes).sum();
        for r in rows.iter().filter(|r| r.bytes > 0) {
            // "Patterns" also names the redaction search patterns ("模式"); the audit's PDF
            // graphics objects need their own key.
            let category = if r.category == pdfcraft_engine::optimize::SpaceCategory::Patterns {
                tl!("Patterns (graphics objects)").to_string()
            } else {
                tl!(r.category.label()).to_string()
            };
            ui.label(category);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(r.bytes.to_string()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(format!("{:.2}%", r.percent)));
            ui.end_row();
        }
        ui.label(egui::RichText::new(tl!("Total")).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(egui::RichText::new(total.to_string()).strong()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(egui::RichText::new("100.00%").strong()));
        ui.end_row();
    });
    ui.add_space(12.0);
    let mut ok = false;
    ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| ok = widgets::pill_button(ui, tl!("OK"), true).clicked()));
    ok
}

impl PdfCraftApp {
    /// Optimize PDF with the dialog's choices and save the copy.
    pub fn optimize_with_draft(&mut self) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let d = self.optimize_draft.clone();
        let result = self.session.optimized_bytes(id, &d.settings, &d.discard).map(|(b, r)| {
            let o = &r.optimize;
            let mut parts = Vec::new();
            if o.images_resampled + o.images_recompressed > 0 {
                parts.push(format!(
                    "{} image{} optimized",
                    o.images_resampled + o.images_recompressed,
                    if o.images_resampled + o.images_recompressed == 1 { "" } else { "s" }
                ));
            }
            let discarded: usize = r.discarded.iter().map(|(_, n)| n).sum();
            if discarded > 0 {
                parts.push(format!("{discarded} item{} discarded", if discarded == 1 { "" } else { "s" }));
            }
            (b, if parts.is_empty() { String::new() } else { format!("; {}", parts.join(", ")) })
        });
        self.save_optimized(id, "optimized", result);
    }
}
