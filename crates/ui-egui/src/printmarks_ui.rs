//! Print production ▸ Add printer marks: which marks, their line weight and the pages.

use egui::{Align, Layout};
use pdfcraft_engine::{Edit, PrinterMarks};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The dialog's choices (kept between openings).
#[derive(Clone, Debug, PartialEq)]
pub struct MarksDraft {
    pub marks: PrinterMarks,
    /// Every page (`true`) or the current one.
    pub all_pages: bool,
}

impl Default for MarksDraft {
    fn default() -> Self {
        MarksDraft { marks: PrinterMarks::default(), all_pages: true }
    }
}

impl MarksDraft {
    /// The edit for a document of `count` pages showing page `current` (0-based), named `title`.
    pub fn edit(&self, current: usize, count: usize, title: &str) -> Edit {
        let pages = if self.all_pages { (0..count).collect() } else { vec![current.min(count.saturating_sub(1))] };
        Edit::AddPrinterMarks { pages, marks: self.marks, title: title.to_string() }
    }
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Add printer marks")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let d = &mut app.printer_marks_draft;
    ui.add(widgets::checkbox(&mut d.marks.crop, tl!("Crop marks")));
    ui.add(widgets::checkbox(&mut d.marks.bleed, tl!("Bleed marks")));
    ui.add(widgets::checkbox(&mut d.marks.registration, tl!("Registration marks")));
    ui.add(widgets::checkbox(&mut d.marks.color_bars, tl!("Color bars")));
    ui.add(widgets::checkbox(&mut d.marks.page_info, tl!("Page information")));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(tl!("Line weight"));
        craft_ui::ComboBox::from_id_salt("printer-marks-weight").selected_text(format!("{} pt", d.marks.line_width)).show_ui(ui, |ui| {
            for w in [0.125, 0.25, 0.5] {
                ui.selectable_value(&mut d.marks.line_width, w, format!("{w} pt"));
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add(widgets::radio(d.all_pages, tl!("All pages"))).clicked() {
            d.all_pages = true;
        }
        if ui.add(widgets::radio(!d.all_pages, tl!("Current page"))).clicked() {
            d.all_pages = false;
        }
    });
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    let any = d.marks.any();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(any, |ui| widgets::pill_button(ui, tl!("OK"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

impl PdfCraftApp {
    /// Apply the Add printer marks dialog to the active document.
    pub(crate) fn apply_printer_marks(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let edit = self.printer_marks_draft.edit(self.views[i].current, doc.info.pages.len(), &doc.name);
        self.apply_edit(edit);
    }
}

/// Print production ▸ Fix hairlines: the dialog's choices (points).
#[derive(Clone, Debug, PartialEq)]
pub struct HairlinesDraft {
    pub threshold: f64,
    pub replace: f64,
    pub all_pages: bool,
}

impl Default for HairlinesDraft {
    fn default() -> Self {
        HairlinesDraft { threshold: 0.25, replace: 0.25, all_pages: true }
    }
}

impl HairlinesDraft {
    pub fn edit(&self, current: usize, count: usize) -> Edit {
        let pages = if self.all_pages { (0..count).collect() } else { vec![current.min(count.saturating_sub(1))] };
        Edit::FixHairlines { pages, threshold: self.threshold, replace: self.replace.max(self.threshold) }
    }
}

/// The Fix hairlines dialog body; returns (apply, cancel).
pub(crate) fn hairlines_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Fix hairlines")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let d = &mut app.hairlines_draft;
    egui::Grid::new("hairlines-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tl!("Hairline width threshold"));
        ui.add(egui::DragValue::new(&mut d.threshold).range(0.01..=72.0).speed(0.01).suffix(" pt"));
        ui.end_row();
        ui.label(tl!("Replace with"));
        ui.add(egui::DragValue::new(&mut d.replace).range(0.01..=72.0).speed(0.01).suffix(" pt"));
        ui.end_row();
    });
    if d.replace < d.threshold {
        d.replace = d.threshold;
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add(widgets::radio(d.all_pages, tl!("All pages"))).clicked() {
            d.all_pages = true;
        }
        if ui.add(widgets::radio(!d.all_pages, tl!("Current page"))).clicked() {
            d.all_pages = false;
        }
    });
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

impl PdfCraftApp {
    /// Apply the Fix hairlines dialog to the active document.
    pub(crate) fn apply_fix_hairlines(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let edit = self.hairlines_draft.edit(self.views[i].current, doc.info.pages.len());
        self.apply_edit(edit);
    }
}

/// Print production ▸ Convert colors: the dialog's choices.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorsDraft {
    pub target: pdfcraft_engine::ColorTarget,
    pub all_pages: bool,
}

impl Default for ColorsDraft {
    fn default() -> Self {
        ColorsDraft { target: pdfcraft_engine::ColorTarget::Gray, all_pages: true }
    }
}

impl ColorsDraft {
    pub fn edit(&self, current: usize, count: usize) -> Edit {
        let pages = if self.all_pages { (0..count).collect() } else { vec![current.min(count.saturating_sub(1))] };
        Edit::ConvertColors { pages, target: self.target }
    }
}

/// The Convert colors dialog body; returns (apply, cancel).
pub(crate) fn colors_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens) -> (bool, bool) {
    use pdfcraft_engine::ColorTarget;
    ui.label(egui::RichText::new(tl!("Convert colors")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let d = &mut app.colors_draft;
    ui.label(tl!("Convert to"));
    ui.horizontal(|ui| {
        if ui.add(widgets::radio(d.target == ColorTarget::Gray, tl!("Grayscale"))).clicked() {
            d.target = ColorTarget::Gray;
        }
        if ui.add(widgets::radio(d.target == ColorTarget::Cmyk, "CMYK")).clicked() {
            d.target = ColorTarget::Cmyk;
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add(widgets::radio(d.all_pages, tl!("All pages"))).clicked() {
            d.all_pages = true;
        }
        if ui.add(widgets::radio(!d.all_pages, tl!("Current page"))).clicked() {
            d.all_pages = false;
        }
    });
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

impl PdfCraftApp {
    /// Apply the Convert colors dialog to the active document.
    pub(crate) fn apply_convert_colors(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let edit = self.colors_draft.edit(self.views[i].current, doc.info.pages.len());
        self.apply_edit(edit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draft_marks_every_page_or_the_current_one() {
        let mut d = MarksDraft::default();
        assert!(matches!(d.edit(2, 4, "a.pdf"), Edit::AddPrinterMarks { ref pages, .. } if pages == &[0, 1, 2, 3]));
        d.all_pages = false;
        assert!(matches!(d.edit(2, 4, "a.pdf"), Edit::AddPrinterMarks { ref pages, ref title, .. } if pages == &[2] && title == "a.pdf"));
    }
}
