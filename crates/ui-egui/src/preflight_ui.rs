//! Print production ▸ Preflight: the Preflight panel. Pick a profile or a single check and run it;
//! the results read by severity and then by check, each with its count; select a hit to turn to its
//! page, flash its box there and read its details; apply the fixups a check offers (one undoable
//! step), or every fixup the results offer; save the report as text, JSON or PDF.

use std::collections::BTreeSet;

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::preflight::{self, Fixup, Profile, Results, Severity, Status};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, RightPanel, icons, widgets};

/// The panel's state: the chosen profile, the last results and what is open in the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct PreflightState {
    /// The chosen profile's id.
    pub profile: String,
    /// The last run, of which document.
    pub results: Option<(DocId, Results)>,
    /// The document's edit generation when it ran: results of a document that changed since are
    /// made again.
    pub generation: u64,
    /// Severities folded shut.
    pub collapsed: BTreeSet<Severity>,
    /// Checks (by index in the results) opened to show their hits.
    pub expanded: BTreeSet<usize>,
    /// The selected hit: check index and hit index.
    pub selected: Option<(usize, usize)>,
}

impl Default for PreflightState {
    fn default() -> Self {
        PreflightState {
            profile: "commercial-print".into(),
            results: None,
            generation: 0,
            collapsed: BTreeSet::new(),
            expanded: BTreeSet::new(),
            selected: None,
        }
    }
}

/// A report's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportFormat {
    Text,
    Json,
    Pdf,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PanelAction {
    Run,
    Fix(Vec<Fixup>),
    Report(ReportFormat),
    Select(usize, usize),
}

/// Severity colours: the red of failed checks, an amber for warnings, the accent for information.
fn severity_look(t: &Tokens, s: Severity) -> (&'static str, egui::Color32) {
    match s {
        Severity::Error => ("circle-x", egui::Color32::from_rgb(0xD7, 0x37, 0x3F)),
        Severity::Warning => ("triangle-alert", egui::Color32::from_rgb(0xD9, 0x8E, 0x04)),
        Severity::Info => ("info", t.accent),
    }
}

/// How the engine's wordings read here.
fn say(wording: &str) -> String {
    crate::i18n::words(wording)
}

/// The fixups' names, one per line, for a Fix button's hover text.
fn fixup_names(fixups: &[Fixup]) -> String {
    fixups.iter().map(|f| say(f.label())).collect::<Vec<_>>().join("\n")
}

/// Every profile the picker offers: the built-in profiles, then the single checks.
pub(crate) fn profiles() -> (Vec<Profile>, Vec<Profile>) {
    (preflight::builtin(), preflight::single_checks())
}

/// The Preflight panel.
pub(crate) fn panel(
    ui: &mut egui::Ui,
    t: &Tokens,
    state: &mut PreflightState,
    doc: &pdfcraft_engine::Document,
    editable: bool,
) -> Option<PanelAction> {
    let mut action = None;
    let (builtin, single) = profiles();
    let chosen = builtin.iter().chain(&single).find(|p| p.id == state.profile).cloned().or_else(|| builtin.first().cloned());
    let shown = chosen.as_ref().map(|p| say(&p.name)).unwrap_or_default();
    let before = state.profile.clone();
    craft_ui::ComboBox::from_id_salt("preflight-profile").selected_text(shown).width((ui.available_width() - 8.0).max(80.0)).show_ui(ui, |ui| {
        for p in &builtin {
            ui.selectable_value(&mut state.profile, p.id.clone(), say(&p.name));
        }
        ui.separator();
        ui.label(egui::RichText::new(say("Single checks")).small().color(t.text_muted));
        for p in &single {
            ui.selectable_value(&mut state.profile, p.id.clone(), say(&p.name));
        }
    });
    if state.profile != before {
        state.selected = None;
        state.expanded.clear();
    }
    if let Some(p) = chosen.as_ref().filter(|p| !p.description.is_empty()) {
        ui.label(egui::RichText::new(say(&p.description)).small().color(t.text_muted));
    }
    ui.add_space(4.0);
    let current = state.results.as_ref().filter(|(d, r)| *d == doc.id && r.profile.id == state.profile).map(|(_, r)| r);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Run"), true).clicked() {
            action = Some(PanelAction::Run);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let report = ui.add_enabled(current.is_some(), egui::Button::new(tl!("Report")));
            egui::Popup::menu(&report).show(|ui| {
                for (label, format) in [(tl!("Text"), ReportFormat::Text), ("JSON", ReportFormat::Json), ("PDF", ReportFormat::Pdf)] {
                    if ui.button(label).clicked() {
                        action = Some(PanelAction::Report(format));
                    }
                }
            });
            if let Some(r) = current {
                let all = r.fixups();
                let fix = ui.add_enabled(editable && !all.is_empty(), egui::Button::new(tl!("Fix")));
                if !all.is_empty() {
                    fix.clone().on_hover_text(fixup_names(&all));
                }
                if fix.clicked() {
                    action = Some(PanelAction::Fix(all));
                }
            }
        });
    });
    let Some(results) = current else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("file-check", 32.0, t.text_faint));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("This document hasn't been checked yet.")).color(t.text_muted));
        });
        return action;
    };
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for s in Severity::ALL {
            let (icon, colour) = severity_look(t, s);
            ui.add(icons::image(icon, 14.0, colour));
            ui.label(egui::RichText::new(format!("{} {}", say(s.label()), results.count(s))).small());
            ui.add_space(6.0);
        }
    });
    if Severity::ALL.into_iter().all(|s| results.count(s) == 0) {
        ui.label(egui::RichText::new(tl!("No issues found")).color(t.text_muted));
    }
    if !results.unread_pages.is_empty() {
        let pages: Vec<String> = results.unread_pages.iter().map(|p| (p + 1).to_string()).collect();
        let note = crate::i18n::fmt(&say("Content not read to the end on pages: {pages}"), &[("pages", &pages.join(", "))]);
        ui.label(egui::RichText::new(note).small().color(t.text_muted));
    }
    ui.add_space(6.0);
    for s in Severity::ALL {
        let found: Vec<usize> =
            (0..results.checks.len()).filter(|&i| results.checks.get(i).is_some_and(|c| c.severity == s && c.status == Status::Found)).collect();
        if found.is_empty() {
            continue;
        }
        let (icon, colour) = severity_look(t, s);
        let open = !state.collapsed.contains(&s);
        let toggled = ui
            .horizontal(|ui| {
                let toggle = icons::button(
                    ui,
                    if open { "chevron-down" } else { "chevron-right" },
                    20.0,
                    false,
                    if open { tl!("Collapse") } else { tl!("Expand") },
                )
                .clicked();
                ui.add(icons::image(icon, 15.0, colour));
                let head = format!("{} ({})", say(s.label()), results.count(s));
                let l = ui.add(egui::Label::new(egui::RichText::new(head).font(theme::semibold(13.0)).color(colour)).sense(egui::Sense::click()));
                toggle || l.clicked()
            })
            .inner;
        if toggled && !state.collapsed.remove(&s) {
            state.collapsed.insert(s);
        }
        if !open {
            continue;
        }
        for i in found {
            let Some(c) = results.checks.get(i) else { continue };
            let expanded = state.expanded.contains(&i);
            let fixups = c.fixups();
            let row = ui
                .horizontal(|ui| {
                    ui.add_space(20.0);
                    let toggle = icons::button(
                        ui,
                        if expanded { "chevron-down" } else { "chevron-right" },
                        18.0,
                        false,
                        if expanded { tl!("Collapse") } else { tl!("Expand") },
                    )
                    .clicked();
                    let title = format!("{} ({})", c.check.said(&crate::i18n::words), c.hits.len());
                    let l = ui.add(
                        egui::Label::new(egui::RichText::new(title).font(theme::regular(12.5)).color(t.text)).truncate().sense(egui::Sense::click()),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !fixups.is_empty() {
                            let fix = ui
                                .add_enabled(editable, egui::Button::new(egui::RichText::new(tl!("Fix")).small()))
                                .on_hover_text(fixup_names(&fixups));
                            if fix.clicked() {
                                action = Some(PanelAction::Fix(fixups.clone()));
                            }
                        }
                    });
                    toggle || l.clicked()
                })
                .inner;
            if row && !state.expanded.remove(&i) {
                state.expanded.insert(i);
            }
            if !state.expanded.contains(&i) {
                continue;
            }
            egui::Frame::NONE.inner_margin(egui::Margin { left: 46, right: 4, top: 0, bottom: 6 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (h, hit) in c.hits.iter().enumerate() {
                    let page = hit.page.map(|p| crate::i18n::fmt(tl!(" (page {p})"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
                    let selected = state.selected == Some((i, h));
                    let text = egui::RichText::new(format!("{}{page}", hit.said(&crate::i18n::words))).small();
                    let text = if selected { text.color(t.accent_text).strong() } else { text.color(t.text_muted) };
                    let l = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                    let l = if hit.page.is_some() { l.on_hover_text(tl!("Go to the page")) } else { l };
                    if l.clicked() {
                        action = Some(PanelAction::Select(i, h));
                    }
                    // The selected hit's details, right under it.
                    if selected {
                        details(ui, t, doc, hit);
                    }
                }
            });
        }
    }
    // What passed and what couldn't run, folded away.
    let passed: Vec<String> = results.checks.iter().filter(|c| c.status == Status::Passed).map(|c| c.check.said(&crate::i18n::words)).collect();
    if !passed.is_empty() {
        egui::CollapsingHeader::new(format!("{} ({})", tl!("Passed"), passed.len())).id_salt("preflight-passed").default_open(false).show(ui, |ui| {
            for p in passed {
                ui.horizontal(|ui| {
                    ui.add(icons::image("circle-check", 13.0, egui::Color32::from_rgb(0x2D, 0x9D, 0x4F)));
                    ui.label(egui::RichText::new(p).small());
                });
            }
        });
    }
    let skipped: Vec<_> = results.checks.iter().filter(|c| c.status == Status::Skipped).collect();
    if !skipped.is_empty() {
        egui::CollapsingHeader::new(format!("{} ({})", tl!("Skipped"), skipped.len())).id_salt("preflight-skipped").default_open(false).show(
            ui,
            |ui| {
                for c in skipped {
                    ui.horizontal(|ui| {
                        ui.add(icons::image("circle-dashed", 13.0, t.text_faint));
                        ui.label(egui::RichText::new(c.check.said(&crate::i18n::words)).small());
                    });
                    if let Some(note) = c.note {
                        ui.label(egui::RichText::new(say(note)).small().color(t.text_muted));
                    }
                }
            },
        );
    }
    action
}

/// A hit's details: its page, its object and what the check measured.
fn details(ui: &mut egui::Ui, t: &Tokens, doc: &pdfcraft_engine::Document, hit: &preflight::Hit) {
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(tl!("Details")).small().strong());
        let row = |ui: &mut egui::Ui, name: &str, value: &str| {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{name}:")).small().color(t.text_muted));
                ui.label(egui::RichText::new(value).small());
            });
        };
        if let Some(p) = hit.page {
            let label = doc.info.pages.get(p).map(|pi| pi.label.clone()).unwrap_or_else(|| (p + 1).to_string());
            row(ui, tl!("Page"), &label);
        }
        // The object as PDF names it, and its type: the same in every language.
        if let Some(n) = hit.object {
            row(ui, tl!("Type"), &object_type(doc, n));
        }
        for (name, value) in &hit.details {
            row(ui, &say(name), value);
        }
    });
}

/// An object's number and its `/Type` and `/Subtype`, as PDF writes them (`7 0 obj /Font /Type1`).
fn object_type(doc: &pdfcraft_engine::Document, n: u32) -> String {
    match doc.object_names(n) {
        Some(names) if !names.is_empty() => format!("{n} 0 obj {names}"),
        _ => format!("{n} 0 obj"),
    }
}

impl PdfCraftApp {
    /// Print production ▸ Preflight: show the panel, and run the chosen profile when this document
    /// has no results yet.
    pub fn open_preflight(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        self.right = Some(RightPanel::Preflight);
        if !self.preflight.results.as_ref().is_some_and(|(d, _)| *d == id) {
            self.run_preflight();
        }
    }

    /// Run the chosen profile over the active document. Nothing is changed.
    pub fn run_preflight(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(profile) = preflight::find(&self.preflight.profile) else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(results) = doc.preflight(&profile) else {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        self.preflight.selected = None;
        self.preflight.generation = doc.edit_generation();
        // A check that found something opens; the rest stay folded.
        self.preflight.expanded = results.checks.iter().enumerate().filter(|(_, c)| c.status == Status::Found).map(|(i, _)| i).collect();
        self.preflight.results = Some((id, results));
        self.right = Some(RightPanel::Preflight);
    }

    /// Run again when the document has changed since the results were made (an edit, an undo, a
    /// fixup), so the panel never shows what the document no longer says.
    pub(crate) fn refresh_preflight(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let ran = self.preflight.results.as_ref().is_some_and(|(d, _)| *d == id);
        if ran && self.session.get(id).is_some_and(|d| d.edit_generation() != self.preflight.generation) {
            self.run_preflight();
        }
    }

    /// Save the report of the last run, said in the interface's language (JSON stays English).
    pub fn save_preflight_report(&mut self, format: ReportFormat) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some((_, results)) = self.preflight.results.as_ref().filter(|(d, _)| *d == id) else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (y, m, d) = self.session.today();
        let date = format!("{y}-{m:02}-{d:02}");
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let words = crate::i18n::words;
        let (name, bytes) = match format {
            ReportFormat::Text => (format!("{stem} Preflight Report.txt"), preflight::text(results, &doc.name, &date, &words).into_bytes()),
            ReportFormat::Json => {
                let json = serde_json::to_vec_pretty(&preflight::json(results, &doc.name)).unwrap_or_default();
                (format!("{stem} Preflight Report.json"), json)
            }
            ReportFormat::Pdf => match self.session.preflight_report_pdf(results, &doc.name, &date, &words) {
                Ok(pdf) => (format!("{stem} Preflight Report.pdf"), pdf.to_vec()),
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            },
        };
        let title = tl!("Report").to_string();
        self.write_files(&[(name, std::sync::Arc::new(bytes))], &title);
    }

    /// Turn to a hit's page and, when it has a box, flash it there.
    fn preflight_show(&mut self, index: usize, check: usize, hit: usize) {
        self.preflight.selected = Some((check, hit));
        let Some((page, bbox)) =
            self.preflight.results.as_ref().and_then(|(_, r)| r.checks.get(check)).and_then(|c| c.hits.get(hit)).map(|h| (h.page, h.bbox))
        else {
            return;
        };
        let Some(page) = page else { return };
        let Some(view) = self.views.get_mut(index) else { return };
        view.go_to_page(page);
        if let Some(b) = bbox {
            view.flash = Some((page, b.map(|v| v as f32), 0.0));
        }
    }

    pub(crate) fn preflight_action(&mut self, index: usize, action: PanelAction) {
        match action {
            PanelAction::Run => self.run_preflight(),
            PanelAction::Select(check, hit) => self.preflight_show(index, check, hit),
            PanelAction::Report(format) => self.save_preflight_report(format),
            // The panel makes its results again once the document has changed (`refresh_preflight`).
            PanelAction::Fix(fixups) => {
                self.apply_edit(pdfcraft_engine::Edit::PreflightFix { fixups });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::preflight::Fixup;

    use super::*;

    /// A print job: RGB content, a font that isn't embedded, a 100 ppi image, a JavaScript open
    /// action, no TrimBox and no Trapped key.
    fn print_job() -> Vec<u8> {
        let content = "1 0 0 rg 0 0 10 10 re f BT /F1 12 Tf (a) Tj ET q 216 0 0 216 10 10 cm /Im Do Q";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /OpenAction 5 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [10 10 590 790] /Contents 4 0 R \
             /Resources << /Font << /F1 6 0 R >> /XObject << /Im 7 0 R >> >> >>"
                .to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Action /S /JavaScript /JS (print()) >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Type /XObject /Subtype /Image /Width 300 /Height 300 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 0 >>\nstream\n\nendstream"
                .to_string(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn harness(language: &'static str) -> Harness<'static, PdfCraftApp> {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", language).unwrap();
            app.open_bytes("job.pdf", None, print_job()).unwrap();
            app
        });
        h.run_steps(4);
        h
    }

    fn results(h: &Harness<'static, PdfCraftApp>) -> Results {
        h.state().preflight.results.clone().map(|(_, r)| r).expect("preflight results")
    }

    #[test]
    fn the_panel_runs_the_profile_and_lists_results_by_severity() {
        let mut h = harness("en");
        assert!(h.state_mut().execute("preflight.run"));
        h.run_steps(3);
        assert_eq!(h.state().right, Some(RightPanel::Preflight));
        let r = results(&h);
        assert_eq!((r.profile.id.as_str(), r.count(Severity::Error)), ("commercial-print", 3));
        h.get_by_label("Errors (3)");
        h.get_by_label("Fonts not embedded (1)");
        h.get_by_label("Pages without a TrimBox (1)");
        // Select a hit: its page comes up, its box flashes, and its details show.
        h.get_by_label("A colour image is drawn at 100 ppi (page 1)").click();
        h.run_steps(3);
        let image = r.checks.iter().position(|c| c.check.id() == "image-resolution").unwrap();
        assert_eq!(h.state().preflight.selected, Some((image, 0)));
        assert_eq!(h.state().views[0].flash.map(|(p, b, _)| (p, b)), Some((0, [10.0, 10.0, 226.0, 226.0])));
        h.get_by_label("Details");
        h.get_by_label("7 0 obj /XObject /Image");
        h.get_by_label("300 × 300");
        // Analyzing changes nothing.
        let id = h.state().active_ids().unwrap().1;
        assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
    }

    #[test]
    fn fixups_are_one_undoable_step_and_the_profile_runs_again() {
        let mut h = harness("en");
        h.state_mut().open_preflight();
        h.run_steps(2);
        // The fixup a check offers.
        h.state_mut().preflight_action(0, PanelAction::Fix(vec![Fixup::SetTrimBox]));
        h.run_steps(2);
        let id = h.state().active_ids().unwrap().1;
        assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Set missing TrimBoxes from the CropBox"));
        let r = results(&h);
        assert_eq!(r.checks.iter().find(|c| c.check.id() == "trim-box").map(|c| c.status), Some(Status::Passed));
        // Every fixup the results offer, from the button at the top.
        h.get_all_by_label("Fix").next().unwrap().click();
        h.run_steps(3);
        assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Apply preflight fixups"));
        assert!(results(&h).fixups().is_empty());
        h.state_mut().undo();
        h.run_steps(2);
        assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Set missing TrimBoxes from the CropBox"));
        // The panel runs again for what the document says now.
        let r = results(&h);
        assert_eq!(r.checks.iter().find(|c| c.check.id() == "scripts").map(|c| c.status), Some(Status::Found));
        assert_eq!(r.checks.iter().find(|c| c.check.id() == "trim-box").map(|c| c.status), Some(Status::Passed));
    }

    #[test]
    fn the_control_channel_runs_a_profile_and_selects_a_hit() {
        let mut h = harness("en");
        h.state_mut().set_option("preflight", "check:image-resolution").unwrap();
        h.state_mut().set_option("preflight-hit", "0:0").unwrap();
        h.run_steps(3);
        assert_eq!(h.state().right, Some(RightPanel::Preflight));
        assert_eq!(h.state().preflight.selected, Some((0, 0)));
        assert!(h.state().views[0].flash.is_some());
        assert!(h.state_mut().set_option("preflight", "no-such-profile").is_err());
        assert!(h.state_mut().set_option("preflight-hit", "0:9").is_err());
        assert!(h.state_mut().set_option("preflight-hit", "zero").is_err());
        // Another tab is active: the results are the first document's, not this one's.
        let other = h.state().session.get(h.state().views[0].id).unwrap().bytes.to_vec();
        h.state_mut().open_bytes("other.pdf", None, other).unwrap();
        h.run_steps(2);
        assert!(h.state_mut().set_option("preflight-hit", "0:0").is_err());
        // With no document at all, nothing runs.
        let mut empty = PdfCraftApp::new();
        assert!(empty.set_option("preflight", "check:image-resolution").is_err());
    }

    #[test]
    fn single_checks_and_reports_in_the_interface_language() {
        let mut h = harness("es");
        h.state_mut().preflight.profile = "check:fonts-embedded".into();
        h.state_mut().run_preflight();
        h.run_steps(3);
        let r = results(&h);
        assert_eq!(r.checks.len(), 1);
        h.get_by_label("Errores (1)");
        h.get_by_label("Fuentes no incrustadas (1)");
        h.get_by_label("La fuente Helvetica no está incrustada (página 1)");
        let dir = std::env::temp_dir().join(format!("pdfcraft-preflight-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
        for format in [ReportFormat::Text, ReportFormat::Json, ReportFormat::Pdf] {
            h.state_mut().preflight_action(0, PanelAction::Report(format));
        }
        let text = std::fs::read_to_string(dir.join("job Preflight Report.txt")).unwrap();
        assert!(text.starts_with("Informe de comprobaciones preliminares\nDocumento: job.pdf\nPerfil: Fuentes no incrustadas\n"), "{text}");
        assert!(text.contains("- La fuente Helvetica no está incrustada"), "{text}");
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("job Preflight Report.json")).unwrap()).unwrap();
        assert_eq!(json["checks"][0]["hits"][0]["message"], "The font Helvetica isn't embedded", "JSON stays in English");
        assert!(std::fs::read(dir.join("job Preflight Report.pdf")).unwrap().starts_with(b"%PDF-"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
