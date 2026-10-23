//! Standards ▸ PDF/A: what the document declares, verifying it against PDF/A-2b or 3b, and
//! fixing what can be fixed (Save as PDF/A). Standards ▸ PDF/X: verifying it against PDF/X-1a,
//! PDF/X-3 or PDF/X-4.

use egui::{Align, Layout};
use pdfcraft_engine::pdfa::{Issue, Level};
use pdfcraft_engine::pdfx;

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

/// Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new("PDF/A").font(theme::semibold(18.0)));
    ui.add_space(6.0);
    declared_line(ui, app, t);
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

/// The PDF/X dialog's state: the chosen level and the last result.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfxState {
    pub level: pdfx::Level,
    pub findings: Option<Vec<pdfx::Finding>>,
}

impl Default for PdfxState {
    fn default() -> Self {
        PdfxState { level: pdfx::Level::X4, findings: None }
    }
}

impl PdfCraftApp {
    /// Standards ▸ PDF/X: open the dialog on the level the document declares, else PDF/X-4.
    pub fn open_pdfx(&mut self) {
        let declared = self.active_ids().and_then(|(_, id)| self.session.get(id)).and_then(|d| d.pdfx_declared());
        self.pdfx = PdfxState { level: declared.unwrap_or(pdfx::Level::X4), findings: None };
        self.dialog = Some(crate::Dialog::PdfX);
    }

    /// Verify the active document against the chosen PDF/X level. Verification only: nothing is
    /// changed.
    pub fn pdfx_verify(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let level = self.pdfx.level;
        self.pdfx.findings = self.session.get(id).and_then(|d| d.pdfx_verify(level));
        // Nothing to verify in a document PdfCraft can't read for editing (an empty list would
        // say it conforms): the dialog stays as it was, and the notice says why.
        if self.pdfx.findings.is_none() {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
        }
    }
}

/// Returns `true` to close.
pub(crate) fn pdfx_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new("PDF/X").font(theme::semibold(18.0)));
    ui.add_space(6.0);
    declared_line(ui, app, t);
    ui.add_space(6.0);
    let before = app.pdfx.level;
    ui.horizontal(|ui| {
        ui.label(tl!("Conformance level:"));
        egui::ComboBox::from_id_salt("pdfx-level").selected_text(app.pdfx.level.label()).show_ui(ui, |ui| {
            for l in pdfx::Level::ALL {
                ui.selectable_value(&mut app.pdfx.level, l, l.label());
            }
        });
    });
    if app.pdfx.level != before {
        app.pdfx.findings = None;
    }
    ui.add_space(6.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            ui.set_min_height(120.0);
            match &app.pdfx.findings {
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
    let mut verify = false;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Verify"), true).clicked() {
                verify = true;
            }
            if widgets::pill_button(ui, tl!("Close"), false).clicked() {
                close = true;
            }
        });
    });
    if verify {
        app.pdfx_verify();
    }
    close
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
}
