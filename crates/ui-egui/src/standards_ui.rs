//! Standards ▸ PDF/A: what the document declares, verifying it against PDF/A-2b or 3b, and
//! fixing what can be fixed (Save as PDF/A).

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
}

impl Default for PdfaState {
    fn default() -> Self {
        PdfaState { level: Level::A2b, issues: None, fixed: Vec::new() }
    }
}

impl PdfCraftApp {
    pub fn pdfa_verify(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let level = self.pdfa.level;
        self.pdfa.issues = self.session.get(id).map(|d| d.pdfa_verify(level));
        self.pdfa.fixed.clear();
    }

    pub fn pdfa_convert(&mut self) {
        let level = self.pdfa.level;
        if self.apply_edit(pdfcraft_engine::Edit::ConvertPdfA { level }) {
            self.pdfa_verify();
            let left = self.pdfa.issues.as_ref().map_or(0, Vec::len);
            self.notify(if left == 0 {
                crate::i18n::fmt(tl!("The document now conforms to {level}; save it to keep the changes"), &[("level", level.label())])
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
    ui.label(egui::RichText::new("PDF/A").font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let declared = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.standards()).unwrap_or_default();
    let shown = declared.pdfa.as_ref().map_or(tl!("none").to_string(), |(p, c)| format!("PDF/A-{p}{}", c.to_lowercase()));
    ui.label(crate::i18n::fmt(tl!("Declared conformance: {shown}"), &[("shown", &shown)]));
    if !declared.output_intents.is_empty() {
        ui.label(
            egui::RichText::new(crate::i18n::fmt(tl!("Output intent: {list}"), &[("list", &declared.output_intents.join(", "))])).color(t.text_muted),
        );
    }
    ui.add_space(6.0);
    let before = app.pdfa.level;
    ui.horizontal(|ui| {
        ui.label(tl!("Conformance level:"));
        egui::ComboBox::from_id_salt("pdfa-level").selected_text(app.pdfa.level.label()).show_ui(ui, |ui| {
            for l in [Level::A2b, Level::A3b] {
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
                    ui.label(crate::i18n::fmt(tl!("No problems found: the document conforms to {level}."), &[("level", app.pdfa.level.label())]));
                }
                Some(v) => {
                    for i in v {
                        let page = i.page.map(|p| crate::i18n::fmt(tl!(" (page {p})"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
                        let fix = if i.fixable { String::new() } else { tl!("  · not fixable here").to_string() };
                        ui.label(format!("{}{page}", i.message));
                        ui.label(egui::RichText::new(format!("ISO 19005 {}{fix}", i.clause)).small().color(t.text_muted));
                    }
                }
            }
        });
    });
    ui.add_space(10.0);
    let mut close = false;
    let mut action = None;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Save as PDF/A"), true).clicked() {
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
