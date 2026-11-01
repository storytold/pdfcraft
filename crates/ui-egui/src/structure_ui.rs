//! Standards ▸ Validate structure: the document checked against the Arlington PDF Model, and the
//! list of what departs from it. Choosing a finding on a page turns to that page and flashes the
//! annotation it is about; the report stays, so the command shows it again until the document
//! changes.

use std::sync::{Arc, Weak};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::structure::{Finding, MODEL_COMMIT, Options, Report, Severity};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, icons, widgets};

/// Rows the list lays out at once are this tall: the finding, then where it is.
const ROW_HEIGHT: f32 = 38.0;

/// The last check, and which severities the list shows.
#[derive(Clone, Debug)]
pub struct StructureState {
    /// The report, with the document and the working file it was made from.
    pub report: Option<(DocId, Weak<Vec<u8>>, Report)>,
    /// Errors, warnings, information: shown or not. Information (keys the model doesn't
    /// define) starts hidden; there can be many and none is a problem.
    pub show: [bool; 3],
}

impl Default for StructureState {
    fn default() -> Self {
        StructureState { report: None, show: [true, true, false] }
    }
}

fn slot(s: Severity) -> usize {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

fn icon(t: &Tokens, s: Severity) -> (&'static str, egui::Color32) {
    match s {
        Severity::Error => ("circle-x", egui::Color32::from_rgb(0xD7, 0x37, 0x3F)),
        Severity::Warning => ("triangle-alert", egui::Color32::from_rgb(0xD9, 0x8E, 0x04)),
        Severity::Info => ("info", t.text_faint),
    }
}

impl PdfCraftApp {
    /// Check the active document now (the command does when needed, and so does Check again).
    pub fn validate_structure(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        match doc.validate_structure(&Options::default()) {
            Some(report) => self.structure.report = Some((id, Arc::downgrade(&doc.bytes), report)),
            None => {
                self.structure.report = None;
                self.notify_tr("This document can't be checked: it couldn't be read for editing");
            }
        }
    }

    /// The report of the active document as it is now, if there is one.
    pub fn structure_report(&self) -> Option<&Report> {
        let doc = self.active_ids().and_then(|(_, id)| self.session.get(id))?;
        match &self.structure.report {
            Some((id, bytes, report)) if *id == doc.id && Weak::ptr_eq(bytes, &Arc::downgrade(&doc.bytes)) => Some(report),
            _ => None,
        }
    }

    /// Standards ▸ Validate structure: the dialog, checking first unless the last check is of
    /// the document as it is now.
    pub(crate) fn open_structure(&mut self) {
        if self.structure_report().is_none() {
            self.validate_structure();
        }
        self.dialog = Some(Dialog::Structure);
    }

    /// Turn to a finding's page and flash the annotation it is about.
    pub(crate) fn structure_show(&mut self, page: usize, rect: Option<[f64; 4]>) {
        let Some((index, _)) = self.active_ids() else { return };
        let Some(view) = self.views.get_mut(index) else { return };
        view.go_to_page(page);
        if let Some(rect) = rect {
            view.flash = Some((page, rect.map(|v| v as f32), 0.0));
        }
    }
}

/// Where a finding is, as its second line says it.
fn place(f: &Finding) -> String {
    let mut parts = vec![f.path.clone()];
    if let Some(o) = f.object {
        parts.push(o.to_string());
    }
    if let Some(p) = f.page {
        parts.push(crate::i18n::fmt(tl!("Page {label}"), &[("label", &(p + 1).to_string())]));
    }
    parts.join(" · ")
}

/// Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.set_width(640.0);
    ui.label(egui::RichText::new(tl!("Validate structure")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let mut close = false;
    let mut again = false;
    let mut show: Option<(usize, Option<[f64; 4]>)> = None;
    let shown = app.structure.show;
    let mut toggles = shown;
    match app.structure_report() {
        None => {
            ui.label(egui::RichText::new(tl!("This document can't be checked: it couldn't be read for editing")).color(t.text_muted));
        }
        Some(report) => {
            ui.label(crate::i18n::fmt(
                tl!("Checked {objects} objects against the Arlington PDF Model for PDF {version}."),
                &[("objects", &report.objects.to_string()), ("version", &report.version.to_string())],
            ));
            if report.is_valid() {
                // Only a check that evaluated every rule, over the whole document, can say the
                // structure follows the model; the notes below say what wasn't evaluated.
                let complete = report.not_checked.is_empty() && !report.stopped_early;
                let said = if complete {
                    tl!("No errors: the structure follows the Arlington PDF Model.")
                } else {
                    tl!("No errors in the rules that were evaluated.")
                };
                ui.label(egui::RichText::new(said).color(t.text_muted));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (i, (label, count)) in
                    [(tl!("Errors"), report.errors), (tl!("Warnings"), report.warnings), (tl!("Information"), report.infos)].into_iter().enumerate()
                {
                    if let Some(on) = toggles.get_mut(i) {
                        ui.add(widgets::checkbox(on, format!("{label} ({count})")));
                    }
                }
            });
            ui.add_space(4.0);
            let rows: Vec<&Finding> = report.findings.iter().filter(|f| shown.get(slot(f.severity)).copied().unwrap_or(false)).collect();
            egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(160.0);
                egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
                    for f in rows.get(range).unwrap_or_default() {
                        let (name, colour) = icon(t, f.severity);
                        // The validator hands over its sentence and the document's own words apart,
                        // so the sentence reads in this language with the names as they are.
                        let said = f.said(&crate::i18n::words);
                        let row = ui
                            .horizontal(|ui| {
                                ui.set_height(ROW_HEIGHT);
                                ui.add(icons::image(name, 15.0, colour));
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    let line = ui.add(
                                        egui::Label::new(egui::RichText::new(&said).font(theme::regular(12.5)))
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                    );
                                    ui.add(egui::Label::new(egui::RichText::new(place(f)).small().color(t.text_muted)).truncate());
                                    line
                                })
                                .inner
                            })
                            .inner;
                        let row = row.on_hover_text(&said);
                        if let Some(page) = f.page
                            && row.on_hover_text(tl!("Go to the page")).clicked()
                        {
                            show = Some((page, f.rect));
                        }
                    }
                });
            });
            if !report.not_checked.is_empty() {
                let list: Vec<String> = report.not_checked.iter().take(8).map(|(what, n)| format!("{what} ({n})")).collect();
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(tl!("Not evaluated: {list}"), &[("list", &list.join(", "))])).small().color(t.text_muted),
                );
            }
            if report.truncated {
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("Only the first {n} of each kind are listed."),
                        &[("n", &Options::default().max_findings.to_string())],
                    ))
                    .small()
                    .color(t.text_muted),
                );
            }
            if report.stopped_early {
                ui.label(
                    egui::RichText::new(tl!("The document has more objects than one check covers; the rest was not checked."))
                        .small()
                        .color(t.text_muted),
                );
            }
            let commit = MODEL_COMMIT.get(..8).unwrap_or(MODEL_COMMIT);
            ui.label(egui::RichText::new(format!("Arlington PDF Model (PDF Association, Apache-2.0) {commit}")).small().color(t.text_faint));
        }
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
            if widgets::pill_button(ui, tl!("Check again"), false).clicked() {
                again = true;
            }
        });
    });
    app.structure.show = toggles;
    if again {
        app.validate_structure();
    }
    if let Some((page, rect)) = show {
        app.structure_show(page, rect);
        close = true;
    }
    close
}

#[cfg(test)]
mod tests {
    use crate::i18n::{Lang, has, tr};

    /// Spanish says every sentence the validator can, and keeps each of its holes, so the
    /// document's own names still have somewhere to go.
    #[test]
    fn spanish_covers_what_validate_structure_says() {
        let es = Lang::from_code("es").expect("es registered");
        assert!(!pdfcraft_engine::structure::WORDINGS.is_empty());
        for wording in pdfcraft_engine::structure::WORDINGS {
            assert!(has(es, wording), "missing: {wording}");
            let said = tr(es, wording);
            for hole in wording.split('{').skip(1).filter_map(|rest| rest.split_once('}')).map(|(name, _)| name) {
                assert!(said.contains(&format!("{{{hole}}}")), "the Spanish of {wording:?} loses {{{hole}}}: {said:?}");
            }
        }
    }
}
