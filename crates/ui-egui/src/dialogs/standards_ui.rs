//! Standards ▸ PDF/A: what the document declares, verifying it against PDF/A-2b, 3b, 4, 4e or 4f,
//! and fixing what can be fixed for PDF/A-2b and 3b (Save as PDF/A).

use egui::{Align, Layout};
use pdfcraft_engine::pdfa::{Issue, Level};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The dialog's state: the chosen level and the last result.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfaState {
    pub level: Level,
    pub issues: Option<Vec<Issue>>,
    pub fixed: Vec<String>,
    /// The document `level` was chosen for ([`PdfCraftApp::pdfa_choose_level`]).
    pub chosen_for: Option<pdfcraft_engine::DocId>,
}

impl Default for PdfaState {
    fn default() -> Self {
        PdfaState { level: Level::A2b, issues: None, fixed: Vec::new(), chosen_for: None }
    }
}

/// The levels the dialog verifies against. Save as PDF/A makes only the PDF/A-2b and 3b ones.
const LEVELS: [Level; 5] = [Level::A2b, Level::A3b, Level::A4, Level::A4e, Level::A4f];

/// Whether Save as PDF/A makes files of `level`: PDF/A-2b and 3b, not the PDF/A-4 levels.
fn convertible(level: Level) -> bool {
    level.part() != 4
}

impl PdfCraftApp {
    /// Chooses the level for the active document once, when the dialog opens or another document
    /// becomes active: the level the document declares when the dialog verifies it, PDF/A-2b
    /// otherwise. A level picked by hand stays while the same document is active.
    pub fn pdfa_choose_level(&mut self) {
        let id = self.active_ids().map(|(_, id)| id);
        if self.pdfa.chosen_for == id {
            return;
        }
        let declared = id.and_then(|id| self.session.get(id)).and_then(|d| d.standards().pdfa);
        let level = declared.and_then(|(part, conformance)| Level::from_declared(part, &conformance)).filter(|l| LEVELS.contains(l));
        self.pdfa = PdfaState { level: level.unwrap_or(PdfaState::default().level), chosen_for: id, ..PdfaState::default() };
    }

    pub fn pdfa_verify(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let level = self.pdfa.level;
        self.pdfa.issues = self.session.get(id).map(|d| d.pdfa_verify(level));
        self.pdfa.fixed.clear();
    }

    pub fn pdfa_convert(&mut self) {
        let level = self.pdfa.level;
        if !convertible(level) {
            return;
        }
        if self.apply_edit(pdfcraft_engine::Edit::ConvertPdfA { level }) {
            self.pdfa_verify();
            let left = self.pdfa.issues.as_ref().map_or(0, Vec::len);
            self.notify(if left == 0 {
                crate::i18n::fmt(
                    tl!("PdfCraft's checks now find no problems for {level}. This is not a validator result. Save to keep the changes."),
                    &[("level", level.label())],
                )
            } else if left == 1 {
                crate::i18n::fmt(tl!("Fixed what could be fixed; 1 problem remains"), &[])
            } else {
                crate::i18n::fmt(tl!("Fixed what could be fixed; {n} problems remain"), &[("n", &left.to_string())])
            });
        }
    }
}

/// Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    app.pdfa_choose_level();
    ui.label(egui::RichText::new("PDF/A").font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let declared = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.standards()).unwrap_or_default();
    let mut standards: Vec<String> = declared.pdfa.iter().map(|(p, c)| format!("PDF/A-{p}{}", c.to_lowercase())).collect();
    standards.extend(declared.pdfua.map(|p| format!("PDF/UA-{p}")));
    let shown = if standards.is_empty() { tl!("none").to_string() } else { standards.join(", ") };
    let removal = pdfcraft_engine::Edit::RemoveStandardId { standard: pdfcraft_engine::Standard::PdfA };
    let mut remove = false;
    ui.horizontal(|ui| {
        ui.label(crate::i18n::fmt(tl!("Declared conformance: {shown}"), &[("shown", &shown)]));
        // Worded as Edit ▸ Undo will name it.
        remove = declared.pdfa.is_some() && widgets::pill_button(ui, &crate::i18n::edit_label(&removal.label()), false).clicked();
    });
    if remove && app.apply_edit(removal) {
        app.pdfa.issues = None;
    }
    if !declared.output_intents.is_empty() {
        ui.label(
            egui::RichText::new(crate::i18n::fmt(tl!("Output intent: {list}"), &[("list", &declared.output_intents.join(", "))])).color(t.text_muted),
        );
    }
    ui.add_space(6.0);
    let before = app.pdfa.level;
    ui.horizontal(|ui| {
        ui.label(tl!("Conformance level:"));
        craft_ui::ComboBox::from_id_salt("pdfa-level").selected_text(app.pdfa.level.label()).show_ui(ui, |ui| {
            for l in LEVELS {
                ui.selectable_value(&mut app.pdfa.level, l, l.label());
            }
        });
    });
    if app.pdfa.level != before {
        app.pdfa.issues = None;
    }
    ui.add_space(6.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            ui.set_min_height(120.0);
            match &app.pdfa.issues {
                None => {
                    ui.label(egui::RichText::new(tl!("Verify to see what the document needs.")).color(t.text_muted));
                }
                Some(v) if v.is_empty() => {
                    ui.label(crate::i18n::fmt(
                        tl!("PdfCraft's checks found no problems for {level}. This is not a validator result."),
                        &[("level", app.pdfa.level.label())],
                    ));
                }
                Some(v) => {
                    for i in v {
                        let page = i.page.map(|p| crate::i18n::fmt(tl!(" (page {p})"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
                        let fix = if i.fixable { String::new() } else { tl!("  · not fixable here").to_string() };
                        // The verifier hands the wording over whole, with the document's own name
                        // apart from it, so the problem reads in this language and the name as it is.
                        ui.label(format!("{}{page}", i.said(&crate::i18n::words)));
                        ui.label(egui::RichText::new(format!("ISO 19005 {}{fix}", i.clause)).small().color(t.text_muted));
                    }
                }
            }
        });
    });
    ui.add_space(10.0);
    let mut close = false;
    let mut action = None;
    let can_convert = convertible(app.pdfa.level);
    if !can_convert {
        ui.label(
            egui::RichText::new(tl!("PDF/A-4 levels are verified only: Save as PDF/A makes PDF/A-2b and 3b files.")).small().color(t.text_muted),
        );
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(can_convert, |ui| widgets::pill_button(ui, tl!("Save as PDF/A"), true)).inner.clicked() {
                action = Some(true);
            }
            if widgets::pill_button(ui, tl!("Verify"), false).clicked() {
                action = Some(false);
            }
            if widgets::pill_button(ui, tl!("Close"), false).clicked() {
                close = true;
            }
        });
    });
    match action {
        Some(true) => app.pdfa_convert(),
        Some(false) => app.pdfa_verify(),
        None => {}
    }
    close
}

#[cfg(test)]
mod wording_tests {
    /// The Standards panel reports what PdfCraft's own checks found; only a run of an external
    /// validator can state the stronger thing, and none is recorded here.
    #[test]
    fn no_standards_panel_wording_claims_conformance() {
        let source = include_str!("standards_ui.rs");
        // Built in pieces so that this file does not match itself.
        for phrase in [concat!("conform", "s to"), concat!("now ", "conforms")] {
            assert!(!source.contains(phrase), "a standards message makes a claim only a validator run can make: {phrase}");
        }
    }
}
