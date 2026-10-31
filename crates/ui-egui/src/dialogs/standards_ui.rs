//! Standards ▸ PDF/A and Standards ▸ PDF/X: what the document declares, verifying it against a
//! level, and Save as PDF/A or PDF/X: converting it, then choosing where to save it and verifying
//! the file written. A conversion that can't reach the level changes nothing and lists what
//! stands in the way.

use egui::{Align, Layout};
use pdfcraft_engine::pdfa::conform::Refusal;
use pdfcraft_engine::pdfa::output::{CHARACTERIZATIONS, IccProfile, OutputCondition};
use pdfcraft_engine::pdfa::{Issue, Level};
use pdfcraft_engine::{DocId, Edit, pdfx};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The file the last Save as wrote, and how many problems verifying it found.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedCheck {
    pub name: String,
    pub problems: usize,
}

/// The PDF/A dialog's state: the chosen level and profile, and the last result.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfaState {
    pub level: Level,
    pub issues: Option<Vec<Issue>>,
    /// What the last conversion changed (English, as the engine reports it).
    pub fixed: Vec<String>,
    /// The output intent's ICC profile, when one is chosen: a document with CMYK device colour
    /// needs one. `None`: the document's own intent, or sRGB.
    pub condition: Option<OutputCondition>,
    /// Why the last Save as PDF/A was refused.
    pub refusal: Option<Refusal>,
    /// The file the last Save as PDF/A wrote, verified.
    pub saved: Option<SavedCheck>,
}

impl Default for PdfaState {
    fn default() -> Self {
        PdfaState { level: Level::A2b, issues: None, fixed: Vec::new(), condition: None, refusal: None, saved: None }
    }
}

impl PdfCraftApp {
    pub fn pdfa_verify(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let level = self.pdfa.level;
        self.pdfa.issues = self.session.get(id).map(|d| d.pdfa_verify(level));
        self.pdfa.refusal = None;
    }

    /// Save as PDF/A: convert the active document, then ask where to save it and verify what was
    /// written. A refusal changes nothing and is listed in the dialog.
    pub fn pdfa_convert(&mut self) {
        let Some((index, id)) = self.active_ids() else { return };
        let level = self.pdfa.level;
        self.pdfa.refusal = None;
        self.pdfa.saved = None;
        self.pdfa.fixed.clear();
        if !self.apply_edit(Edit::ConvertPdfA { level, condition: self.pdfa.condition.clone() }) {
            return;
        }
        self.pdfa.fixed = self.session.get(id).map(|d| d.converted.clone()).unwrap_or_default();
        self.pdfa_verify();
        self.save_then(index, crate::SaveTarget::As, move |app: &mut PdfCraftApp| {
            let Some(doc) = app.session.get(id) else { return };
            let issues = doc.pdfa_verify(level);
            app.pdfa.saved = Some(SavedCheck { name: doc.name.clone(), problems: issues.len() });
            app.pdfa.issues = Some(issues);
        });
    }

    /// A conversion the engine refused: the dialog it came from lists why.
    pub(crate) fn conversion_refused(&mut self, refusal: Refusal) {
        if refusal.level.starts_with("PDF/X") {
            self.pdfx.refusal = Some(refusal);
        } else {
            self.pdfa.refusal = Some(refusal);
        }
    }

    /// Ask for an ICC profile file for the PDF/A dialog (`pdfx`: false) or the PDF/X one.
    fn choose_profile(&mut self, pdfx: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Choose ICC profile…")).add_filter("ICC", &["icc", "icm"]);
            self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let read = read_profile(&path).and_then(IccProfile::read);
                match read {
                    Ok(profile) => app.set_profile(pdfx, profile),
                    Err(e) => app.notify_fmt("Couldn't read the ICC profile {name}: {e}", &[("name", &name), ("e", &e)]),
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = pdfx;
    }

    /// Use `profile` as the chosen ICC profile of the PDF/A dialog (`pdfx`: false) or the PDF/X
    /// one.
    pub fn set_profile(&mut self, pdfx: bool, profile: IccProfile) {
        let condition = Some(OutputCondition::profile(profile, None));
        if pdfx {
            self.pdfx.condition = condition;
            self.pdfx.refusal = None;
        } else {
            self.pdfa.condition = condition;
            self.pdfa.refusal = None;
        }
    }
}

/// What the document declares: its standards and its output intents.
fn declared_line(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let declared = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.standards()).unwrap_or_default();
    let mut standards: Vec<String> = declared.pdfa.iter().map(|(p, c)| format!("PDF/A-{p}{}", c.to_lowercase())).collect();
    standards.extend(declared.pdfua.map(|p| format!("PDF/UA-{p}")));
    standards.extend(declared.pdfx);
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
}

/// How a chosen profile is named in the dialogs.
fn profile_name(condition: &OutputCondition) -> String {
    crate::i18n::fmt(tl!("ICC profile: {name}"), &[("name", &condition.label())])
}

/// What the last conversion and save came to: why it was refused, or the file written and its
/// verification, and what the conversion changed.
fn outcome(ui: &mut egui::Ui, refusal: Option<&Refusal>, saved: Option<&SavedCheck>, fixed: &[String], level: &str, t: &Tokens) {
    if let Some(r) = refusal {
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Can't save as {level}; nothing was changed:"), &[("level", r.level)])).strong());
        for b in &r.blockers {
            let page = b.page.map(|p| crate::i18n::fmt(tl!(" (page {p})"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
            ui.label(format!("• {}{page}", b.said(&crate::i18n::words)));
            if let Some(rule) = &b.rule {
                ui.label(egui::RichText::new(rule).small().color(t.text_muted));
            }
        }
        ui.add_space(6.0);
        return;
    }
    if let Some(s) = saved {
        let line = if s.problems == 0 {
            crate::i18n::fmt(tl!("Saved {name} and verified it: it conforms to {level}."), &[("name", &s.name), ("level", level)])
        } else {
            crate::i18n::fmt(tl!("Saved {name}, but verifying it found {n} problems."), &[("name", &s.name), ("n", &s.problems.to_string())])
        };
        ui.label(egui::RichText::new(line).strong());
    }
    if !fixed.is_empty() {
        ui.label(crate::i18n::fmt(tl!("Converted to {level} ({n} changes)"), &[("level", level), ("n", &fixed.len().to_string())]));
        egui::CollapsingHeader::new(tl!("Details")).id_salt("conversion-details").show(ui, |ui| {
            for f in fixed {
                ui.label(egui::RichText::new(f).small().color(t.text_muted));
            }
        });
        ui.add_space(6.0);
    }
}

/// Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new("PDF/A").font(theme::semibold(18.0)));
    ui.add_space(6.0);
    declared_line(ui, app, t);
    ui.add_space(6.0);
    let before = app.pdfa.level;
    ui.horizontal(|ui| {
        ui.label(tl!("Conformance level:"));
        craft_ui::ComboBox::from_id_salt("pdfa-level").selected_text(app.pdfa.level.label()).show_ui(ui, |ui| {
            for l in Level::ALL {
                ui.selectable_value(&mut app.pdfa.level, l, l.label());
            }
        });
    });
    if app.pdfa.level != before {
        app.pdfa.issues = None;
        app.pdfa.refusal = None;
        // What was converted and saved was for the old level.
        app.pdfa.saved = None;
        app.pdfa.fixed.clear();
    }
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut, reason = "the button that sets it is on the desktop only"))]
    let mut choose = false;
    ui.horizontal(|ui| {
        ui.label(tl!("Output intent profile:"));
        let shown = app.pdfa.condition.as_ref().map(profile_name).unwrap_or_else(|| tl!("None: sRGB for RGB and gray documents").to_string());
        ui.label(egui::RichText::new(shown).color(t.text_muted));
        #[cfg(not(target_arch = "wasm32"))]
        if ui.button(tl!("Choose ICC profile…")).clicked() {
            choose = true;
        }
        if app.pdfa.condition.is_some() && ui.button(tl!("Clear")).clicked() {
            app.pdfa.condition = None;
            app.pdfa.refusal = None;
        }
    });
    if choose {
        app.choose_profile(false);
    }
    ui.add_space(6.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            ui.set_min_height(120.0);
            outcome(ui, app.pdfa.refusal.as_ref(), app.pdfa.saved.as_ref(), &app.pdfa.fixed, app.pdfa.level.label(), t);
            match &app.pdfa.issues {
                // A refusal already says what the document needs.
                None if app.pdfa.refusal.is_some() => {}
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

/// The PDF/X dialog's state: the chosen level, printing condition and flattening, and the last
/// result.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfxState {
    pub level: pdfx::Level,
    pub findings: Option<Vec<pdfx::Finding>>,
    /// The printing condition: an ICC registry characterization or a chosen ICC profile. `None`:
    /// FOGRA39 for PDF/X-1a and -3; PDF/X-4 needs a profile.
    pub condition: Option<OutputCondition>,
    /// Flatten comments and form fields first (PDF/X allows no annotations in the bleed).
    pub flatten: bool,
    pub fixed: Vec<String>,
    pub refusal: Option<Refusal>,
    pub saved: Option<SavedCheck>,
}

impl Default for PdfxState {
    fn default() -> Self {
        PdfxState { level: pdfx::Level::X4, findings: None, condition: None, flatten: false, fixed: Vec::new(), refusal: None, saved: None }
    }
}

impl PdfCraftApp {
    /// Standards ▸ PDF/X: open the dialog on the level and printing condition the document
    /// declares, else PDF/X-4 and the profile chosen last.
    pub fn open_pdfx(&mut self) {
        let doc = self.active_ids().and_then(|(_, id)| self.session.get(id));
        let declared = doc.and_then(|d| d.pdfx_declared());
        let condition = doc.and_then(|d| d.pdfx_condition()).or_else(|| self.pdfx.condition.take());
        self.pdfx = PdfxState { level: declared.unwrap_or(pdfx::Level::X4), condition, ..PdfxState::default() };
        self.dialog = Some(crate::Dialog::PdfX);
    }

    /// Verify the active document against the chosen PDF/X level. Verification only: nothing is
    /// changed.
    pub fn pdfx_verify(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let level = self.pdfx.level;
        self.pdfx.findings = self.session.get(id).and_then(|d| d.pdfx_verify(level));
        self.pdfx.refusal = None;
        // Nothing to verify in a document PdfCraft can't read for editing (an empty list would
        // say it conforms): the dialog stays as it was, and the notice says why.
        if self.pdfx.findings.is_none() {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
        }
    }

    /// Save as PDF/X: convert the active document, then ask where to save it and verify what was
    /// written. A refusal changes nothing and is listed in the dialog.
    pub fn pdfx_save_as(&mut self) {
        let Some((index, id)) = self.active_ids() else { return };
        let level = self.pdfx.level;
        self.pdfx.refusal = None;
        self.pdfx.saved = None;
        self.pdfx.fixed.clear();
        let edit = Edit::ConvertPdfX { level, condition: self.pdfx.condition.clone(), flatten: self.pdfx.flatten };
        if !self.apply_edit(edit) {
            return;
        }
        self.pdfx.fixed = self.session.get(id).map(|d| d.converted.clone()).unwrap_or_default();
        self.pdfx_verify();
        self.save_then(index, crate::SaveTarget::As, move |app: &mut PdfCraftApp| app.pdfx_check_saved(id, level));
    }

    /// Verify the file Save as PDF/X wrote: after a save the document is the file as written.
    fn pdfx_check_saved(&mut self, id: DocId, level: pdfx::Level) {
        let Some(doc) = self.session.get(id) else { return };
        let findings = doc.pdfx_verify(level);
        // No object graph to verify is no verification: never "no problems".
        let problems = findings.as_ref().map_or(1, Vec::len);
        self.pdfx.saved = Some(SavedCheck { name: doc.name.clone(), problems });
        self.pdfx.findings = findings;
    }
}

/// Returns `true` to close.
pub(crate) fn pdfx_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new("PDF/X").font(theme::semibold(18.0)));
    ui.add_space(6.0);
    declared_line(ui, app, t);
    ui.add_space(6.0);
    let before = (app.pdfx.level, app.pdfx.condition.clone());
    ui.horizontal(|ui| {
        ui.label(tl!("Conformance level:"));
        craft_ui::ComboBox::from_id_salt("pdfx-level").selected_text(app.pdfx.level.label()).show_ui(ui, |ui| {
            for l in pdfx::Level::ALL {
                ui.selectable_value(&mut app.pdfx.level, l, l.label());
            }
        });
    });
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut, reason = "the button that sets it is on the desktop only"))]
    let mut choose = false;
    ui.horizontal(|ui| {
        ui.label(tl!("Printing condition:"));
        let shown = match &app.pdfx.condition {
            Some(c @ OutputCondition::Profile { .. }) => profile_name(c),
            Some(c) => c.identifier(),
            // PDF/X-4 has no default: it embeds a profile.
            None if app.pdfx.level == pdfx::Level::X4 => tl!("Choose ICC profile…").to_string(),
            None => CHARACTERIZATIONS.first().map(|c| c.id.to_owned()).unwrap_or_default(),
        };
        craft_ui::ComboBox::from_id_salt("pdfx-condition").selected_text(shown).show_ui(ui, |ui| {
            if let Some(c @ OutputCondition::Profile { .. }) = app.pdfx.condition.clone() {
                ui.selectable_value(&mut app.pdfx.condition, Some(c.clone()), profile_name(&c));
            }
            // Without a choice, the default (the first) is the one in use.
            let current = match &app.pdfx.condition {
                Some(OutputCondition::Registered(c)) => Some(c.id),
                None if app.pdfx.level == pdfx::Level::X4 => None,
                None => CHARACTERIZATIONS.first().map(|c| c.id),
                Some(OutputCondition::Profile { .. }) => None,
            };
            for c in CHARACTERIZATIONS {
                if ui.selectable_label(current == Some(c.id), c.id).on_hover_text(c.condition).clicked() {
                    app.pdfx.condition = Some(OutputCondition::Registered(*c));
                }
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        if ui.button(tl!("Choose ICC profile…")).clicked() {
            choose = true;
        }
    });
    if choose {
        app.choose_profile(true);
    }
    ui.add(widgets::checkbox(&mut app.pdfx.flatten, tl!("Flatten comments and form fields")));
    if (app.pdfx.level, app.pdfx.condition.clone()) != before {
        app.pdfx.findings = None;
        app.pdfx.refusal = None;
        // What was converted and saved was for the old level or condition.
        app.pdfx.saved = None;
        app.pdfx.fixed.clear();
    }
    ui.add_space(6.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            ui.set_min_height(120.0);
            outcome(ui, app.pdfx.refusal.as_ref(), app.pdfx.saved.as_ref(), &app.pdfx.fixed, app.pdfx.level.label(), t);
            match &app.pdfx.findings {
                // A refusal already says what the document needs.
                None if app.pdfx.refusal.is_some() => {}
                None => {
                    ui.label(egui::RichText::new(tl!("Verify to see what the document needs.")).color(t.text_muted));
                }
                Some(v) if v.is_empty() => {
                    ui.label(crate::i18n::fmt(tl!("No problems found: the document conforms to {level}."), &[("level", app.pdfx.level.label())]));
                }
                Some(v) => {
                    for f in v {
                        let page = f.page.map(|p| crate::i18n::fmt(tl!(" (page {p})"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
                        ui.label(format!("{}{page}", f.said(&crate::i18n::words)));
                        // Where the rule comes from and the object it was found in, as PDF names
                        // them: the same in every language.
                        let object = f.object.map(|n| format!(" · {n} 0 obj")).unwrap_or_default();
                        ui.label(
                            egui::RichText::new(format!("{} · {} · {}{object}", app.pdfx.level.standard(), f.clause, f.rule))
                                .small()
                                .color(t.text_muted),
                        );
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
            if widgets::pill_button(ui, tl!("Save as PDF/X"), true).clicked() {
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
        Some(true) => app.pdfx_save_as(),
        Some(false) => app.pdfx_verify(),
        None => {}
    }
    close
}

/// An ICC profile file's bytes: refused when the path isn't a regular file, or when it holds more
/// than the largest profile PdfCraft takes, even if it grew after it was measured.
#[cfg(not(target_arch = "wasm32"))]
fn read_profile(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let max = pdfcraft_engine::pdfa::output::MAX_PROFILE;
    let too_large = || format!("larger than {} MB", max / (1024 * 1024));
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > max as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(max as u64 + 1).read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err(too_large());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// A page that conforms to PDF/X-1a:2003, its output intent a characterization in the ICC
    /// registry.
    fn pdfx1a() -> Vec<u8> {
        let content = "0 0 0 1 k 20 20 100 100 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (CGATS TR 001) \
             /RegistryName (http://www.color.org) >>] >>"
                .to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /TrimBox [10 10 190 290] /Contents 4 0 R /Resources << >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size 5 /Root 1 0 R /Info << /GTS_PDFXVersion (PDF/X-1a:2003) /Trapped /False >> /ID [<0A0B> <0A0B>] >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        out
    }

    fn harness(language: &'static str, doc: Vec<u8>) -> Harness<'static, PdfCraftApp> {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", language).unwrap();
            app.open_bytes("press.pdf", None, doc.clone()).unwrap();
            app
        });
        h.run_steps(4);
        h
    }

    #[test]
    fn pdfx_dialog_verifies_the_level_the_document_declares() {
        let mut h = harness("en", pdfx1a());
        assert!(h.state_mut().execute("standards.pdfx"));
        h.run_steps(2);
        assert_eq!(h.state().dialog, Some(crate::Dialog::PdfX));
        assert_eq!(h.state().pdfx.level, pdfx::Level::X1a2003);
        h.get_by_label("Declared conformance: PDF/X-1a:2003");
        h.get_by_label("Output intent: CGATS TR 001");
        h.get_by_label("Verify to see what the document needs.");
        h.get_by_label("Verify").click();
        h.run_steps(3);
        h.get_by_label("No problems found: the document conforms to PDF/X-1a:2003.");
        // Verifying changes nothing.
        let id = h.state().active_ids().unwrap().1;
        assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
        assert!(!h.state().session.get(id).unwrap().dirty);
    }

    #[test]
    fn pdfx_dialog_lists_what_breaks_the_level_in_the_interface_language() {
        let plain = pdfcraft_engine::Session::new().create_from_text("t", "Proof").unwrap().to_vec();
        let mut h = harness("es", plain);
        assert!(h.state_mut().execute("standards.pdfx"));
        h.run_steps(2);
        assert_eq!(h.state().pdfx.level, pdfx::Level::X4, "a document that declares nothing is checked as PDF/X-4");
        h.state_mut().pdfx_verify();
        h.run_steps(3);
        let findings = h.state().pdfx.findings.clone().unwrap();
        assert!(findings.iter().any(|f| f.rule == "pdfx.output-intent"), "{findings:#?}");
        h.get_by_label("No hay propósito de salida PDF/X (GTS_PDFX)");
        h.get_by_label("La página no tiene ni TrimBox ni ArtBox (página 1)");
    }

    /// One page of RGB colour.
    fn rgb_page() -> Vec<u8> {
        pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
            .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R /Resources << >> >>")
            .stream("", b"1 0 0 rg 20 20 100 100 re f")
            .build()
    }

    #[test]
    fn save_as_pdfx_converts_saves_and_verifies_the_file_written() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-save-pdfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("press.pdf");
        let mut h = harness("en", rgb_page());
        h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
        assert!(h.state_mut().execute("standards.pdfx"));
        h.run_steps(2);
        h.state_mut().pdfx.level = pdfx::Level::X1a2003;
        let press = IccProfile::read(pdfcraft_testkit::icc::cmyk_printer_profile("Test press (synthetic)")).unwrap();
        h.state_mut().set_profile(true, press);
        h.run_steps(2);
        assert_eq!(h.state().pdfx.condition.as_ref().map(OutputCondition::identifier).as_deref(), Some("Test press (synthetic)"));
        h.get_by_label("Save as PDF/X").click();
        h.run_steps(4);
        // Converted, written where Save As was pointed, and verified as written.
        let saved = h.state().pdfx.saved.clone().unwrap_or_else(|| panic!("{:#?}", h.state().pdfx.refusal));
        assert_eq!(saved, SavedCheck { name: "press.pdf".into(), problems: 0 });
        h.get_by_label("Saved press.pdf and verified it: it conforms to PDF/X-1a:2003.");
        // The file itself, read afresh: PDF 1.4, and conforming.
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
        assert_eq!(pdfx::verify(&doc, pdfx::Level::X1a2003), Vec::new());
        // Another level: the file was saved and verified as PDF/X-1a, so the dialog stops saying so.
        h.get_by_value("PDF/X-1a:2003").click();
        h.run_steps(2);
        h.get_by_label("PDF/X-4").click();
        h.run_steps(2);
        assert_eq!(h.state().pdfx.level, pdfx::Level::X4);
        assert_eq!(h.state().pdfx.saved, None);
        assert!(h.state().pdfx.fixed.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_save_as_lists_why_in_the_interface_language_and_changes_nothing() {
        let mut h = harness("es", rgb_page());
        assert!(h.state_mut().execute("standards.pdfx"));
        h.run_steps(2);
        h.get_by_label("Guardar como PDF/X").click();
        h.run_steps(3);
        let refusal = h.state().pdfx.refusal.clone().expect("refused: PDF/X-4 needs a profile");
        assert_eq!(refusal.level, "PDF/X-4");
        h.get_by_label("No se puede guardar como PDF/X-4; no se ha cambiado nada:");
        h.get_by_label("• PDF/X-4 incrusta el perfil ICC de la condición de impresión: elige un archivo de perfil");
        let id = h.state().active_ids().unwrap().1;
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.can_undo(), None);
        assert!(!doc.dirty);
        // The PDF/A dialog offers the a and u levels.
        assert!(h.state_mut().execute("standards.pdfa"));
        h.run_steps(2);
        h.state_mut().pdfa.level = Level::A3a;
        h.state_mut().pdfa_convert();
        h.run_steps(2);
        h.get_by_label("No se puede guardar como PDF/A-3a; no se ha cambiado nada:");
        h.get_by_label("• El documento no tiene árbol de estructura: no está etiquetado");
    }

    /// Screenshots of Save as PDF/X when PDFCRAFT_SHOT_DIR names a folder (for review): refused,
    /// in Spanish, and saved and verified, in English.
    #[test]
    fn save_as_pdfx_screenshots() {
        let Ok(dir) = std::env::var("PDFCRAFT_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = harness("es", rgb_page());
        assert!(h.state_mut().execute("standards.pdfx"));
        h.run_steps(2);
        h.state_mut().pdfx_save_as();
        h.run_steps(4);
        h.render().unwrap().save(dir.join("save-as-pdfx-refused-es.png")).unwrap();
        let mut h = harness("en", rgb_page());
        h.state_mut().save_override = Some(dir.join("press.pdf").to_string_lossy().into_owned());
        assert!(h.state_mut().execute("standards.pdfx"));
        h.run_steps(2);
        h.state_mut().pdfx.level = pdfx::Level::X1a2003;
        h.state_mut().set_profile(true, IccProfile::read(pdfcraft_testkit::icc::cmyk_printer_profile("Test press (synthetic)")).unwrap());
        h.state_mut().pdfx_save_as();
        h.run_steps(4);
        h.render().unwrap().save(dir.join("save-as-pdfx-saved-en.png")).unwrap();
    }
}
