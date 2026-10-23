//! Prepare for accessibility ▸ Check for accessibility: the Accessibility Checker Options dialog
//! and the Accessibility Checker panel (results by category, with Fix, Skip rule, Explain,
//! Check again and Show report on each rule). The same dialog and panel verify PDF/UA: the
//! conditions of the Matterhorn Protocol, failed ones first, then those a person must judge.

use std::collections::BTreeSet;

use egui::{Align, Layout};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::DocId;
use pdfcraft_engine::a11y::pdfua::{Outcome, Part, UaReport};
use pdfcraft_engine::a11y::{Category, Finding, Options, Report, Rule, Status};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, PropsTab, RightPanel, icons, widgets};

/// What a check checks against: the Accessibility Checker's rules, or a part of PDF/UA.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Checker,
    PdfUa(Part),
}

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Checker, Profile::PdfUa(Part::Ua1), Profile::PdfUa(Part::Ua2)];

    fn label(self) -> String {
        match self {
            Profile::Checker => tl!("Accessibility Checker").to_string(),
            // A standard's name reads the same in every language.
            Profile::PdfUa(part) => part.label().to_owned(),
        }
    }
}

/// The options dialog's settings (kept for the session, like Acrobat's).
#[derive(Clone, Debug, PartialEq)]
pub struct A11yOptions {
    /// The checker's rules, or PDF/UA.
    pub profile: Profile,
    pub rules: BTreeSet<Rule>,
    pub all_pages: bool,
    /// 1-based page range when not all pages.
    pub from: usize,
    pub to: usize,
    /// Save the accessibility report when checking finishes.
    pub create_report: bool,
    /// Show the options when the checker starts.
    pub show_dialog: bool,
    /// The category whose rules the dialog lists.
    pub category: Category,
}

impl Default for A11yOptions {
    fn default() -> Self {
        Self {
            profile: Profile::Checker,
            rules: Options::default().rules,
            all_pages: true,
            from: 1,
            to: 1,
            create_report: false,
            show_dialog: true,
            category: Category::Document,
        }
    }
}

/// The last check of a document, as the panel shows it.
#[derive(Clone, Debug, Default)]
pub struct A11yState {
    pub report: Option<(DocId, Report)>,
    /// The prior check of the same document with the same checker options.
    pub previous: Option<(DocId, Report)>,
    pub options: Option<Options>,
    /// Collapsed categories and expanded rules.
    pub collapsed: BTreeSet<Category>,
    pub expanded: BTreeSet<Rule>,
    /// The rule whose explanation is showing.
    pub explain: Option<Rule>,
    /// The last PDF/UA verification; the panel shows it instead of `report` while it is there.
    pub ua: Option<(DocId, UaReport)>,
    /// PDF/UA conditions whose findings are showing.
    pub ua_expanded: BTreeSet<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PanelAction {
    Fix(Rule),
    Skip(Rule),
    CheckAgain,
    ShowReport,
    Options,
    GoTo(usize, Option<[f64; 4]>),
    GoToTag(ObjRef, Option<usize>, Option<[f64; 4]>),
    VerifyPdfUa(Part),
}

/// Rules with an automatic (or guided) fix.
pub(crate) fn fixable(rule: Rule) -> bool {
    matches!(rule, Rule::PrimaryLanguage | Rule::Title | Rule::TabOrder)
}

pub(crate) fn options_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
    let o = &mut app.a11y_options;
    o.to = o.to.clamp(1, pages);
    o.from = o.from.clamp(1, o.to);
    // What to check against sits on the title's row, so the dialog is no taller for it.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Accessibility Checker Options")).font(theme::semibold(18.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            egui::ComboBox::from_id_salt("a11y-profile").selected_text(o.profile.label()).width(160.0).show_ui(ui, |ui| {
                for p in Profile::ALL {
                    ui.selectable_value(&mut o.profile, p, p.label());
                }
            });
            ui.label(tl!("Standards"));
        });
    });
    ui.add_space(8.0);
    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
        egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
        ui.add_space(8.0);
    };
    // PDF/UA is verified on every page against its own conditions; the rest is the checker's.
    if o.profile == Profile::Checker {
        checker_options(ui, o, pages, &group);
    }
    ui.add(widgets::checkbox(&mut o.show_dialog, tl!("Show this dialog when the Checker starts")));
    ui.add_space(10.0);
    let (mut start, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Start Checking"), true).clicked() {
                start = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (start, cancel)
}

/// Draws a titled group of options: the title, then what `body` puts in it.
type Group<'a> = dyn Fn(&mut egui::Ui, &str, &mut dyn FnMut(&mut egui::Ui)) + 'a;

/// The checker's own options: the report, the page range and the rules.
fn checker_options(ui: &mut egui::Ui, o: &mut A11yOptions, pages: usize, group: &Group<'_>) {
    group(ui, "Report Options", &mut |ui| {
        ui.add(widgets::checkbox(&mut o.create_report, tl!("Create accessibility report")));
    });
    group(ui, "Page Range", &mut |ui| {
        ui.horizontal(|ui| {
            widgets::radio_value(ui, &mut o.all_pages, true, tl!("All pages in document"));
            widgets::radio_value(ui, &mut o.all_pages, false, tl!("Pages from"));
            ui.add_enabled(!o.all_pages, egui::DragValue::new(&mut o.from).range(1..=pages));
            ui.label(tl!("to"));
            ui.add_enabled(!o.all_pages, egui::DragValue::new(&mut o.to).range(1..=pages));
        });
    });
    let on = o.rules.len();
    group(ui, &crate::i18n::fmt(tl!("Checking Options ({on} of 32 in all categories)"), &[("on", &on.to_string())]), &mut |ui| {
        ui.horizontal(|ui| {
            ui.label(tl!("Category:"));
            egui::ComboBox::from_id_salt("a11y-category").selected_text(tl!(o.category.label())).width(260.0).show_ui(ui, |ui| {
                for c in Category::ALL {
                    ui.selectable_value(&mut o.category, c, tl!(c.label()));
                }
            });
        });
        ui.add_space(4.0);
        for r in Rule::ALL.into_iter().filter(|r| r.category() == o.category) {
            let mut checked = o.rules.contains(&r);
            if ui.add(widgets::checkbox(&mut checked, tl!(r.description()))).changed() {
                if checked {
                    o.rules.insert(r);
                } else {
                    o.rules.remove(&r);
                }
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tl!("Select All")).clicked() {
                o.rules.extend(Rule::ALL.into_iter().filter(|r| r.category() == o.category));
            }
            if ui.button(tl!("Clear All")).clicked() {
                let c = o.category;
                o.rules.retain(|r| r.category() != c);
            }
        });
    });
}

/// The red of failed checks (as for invalid signatures).
const FAILED: egui::Color32 = egui::Color32::from_rgb(0xD7, 0x37, 0x3F);

fn status_icon(t: &Tokens, s: Status) -> (&'static str, egui::Color32) {
    match s {
        Status::Passed => ("circle-check", egui::Color32::from_rgb(0x2D, 0x9D, 0x4F)),
        Status::Failed => ("circle-x", FAILED),
        Status::Manual => ("circle-help", egui::Color32::from_rgb(0xD9, 0x8E, 0x04)),
        Status::Skipped => ("circle-dashed", t.text_faint),
    }
}

/// The findings under a rule or a condition: each goes to its tag, or else to its page.
fn findings_list(ui: &mut egui::Ui, t: &Tokens, findings: &[Finding], action: &mut Option<PanelAction>) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 46, right: 4, top: 0, bottom: 6 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for f in findings {
            // The checker hands over the sentence and the document's own words apart, so the
            // sentence is said in the panel's language with those words still in it.
            let said = f.said(&crate::i18n::words);
            let l = ui.add(egui::Label::new(egui::RichText::new(said).small().color(t.text_muted)).sense(egui::Sense::click()));
            if let Some(object) = f.object {
                if l.on_hover_text(tl!("Tags")).clicked() {
                    *action = Some(PanelAction::GoToTag(object, f.page, f.bbox));
                }
            } else if let Some(p) = f.page
                && l.on_hover_text(tl!("Go to the page")).clicked()
            {
                *action = Some(PanelAction::GoTo(p, f.bbox));
            }
        }
    });
}

fn outcome_icon(t: &Tokens, o: Outcome) -> (&'static str, egui::Color32) {
    match o {
        Outcome::Passed => status_icon(t, Status::Passed),
        Outcome::Failed => status_icon(t, Status::Failed),
        Outcome::Manual => status_icon(t, Status::Manual),
        Outcome::NotChecked => status_icon(t, Status::Skipped),
    }
}

/// The panel for a PDF/UA verification: the failed conditions with what fails and where, then
/// those a person must judge, those PdfCraft doesn't check, and those that passed.
fn pdfua_panel(ui: &mut egui::Ui, t: &Tokens, report: &UaReport, expanded: &mut BTreeSet<&'static str>) -> Option<PanelAction> {
    let mut action = None;
    let failed = report.count(Outcome::Failed);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(report.part.label()).font(theme::semibold(13.0)));
        ui.label(
            egui::RichText::new(if failed == 0 {
                tl!("No issues found").to_string()
            } else if failed == 1 {
                tl!("1 issue").to_string()
            } else {
                crate::i18n::fmt(tl!("{n} issues"), &[("n", &failed.to_string())])
            })
            .color(t.text_muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Check again"), false).clicked() {
                action = Some(PanelAction::VerifyPdfUa(report.part));
            }
        });
    });
    let declared = report.declared.map(|p| format!("PDF/UA-{p}")).unwrap_or_else(|| tl!("none").to_string());
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Declared conformance: {shown}"), &[("shown", &declared)])).small().color(t.text_muted));
    // What the verification leaves to a person, or to another validator, is never a pass.
    ui.label(
        egui::RichText::new(format!(
            "{}: {} · {}: {}",
            tl!("Needs manual check"),
            report.count(Outcome::Manual),
            tl!(Outcome::NotChecked.label()),
            report.count(Outcome::NotChecked)
        ))
        .small()
        .color(t.text_muted),
    );
    ui.add_space(6.0);
    for outcome in Outcome::ALL {
        let results: Vec<_> = report.with(outcome).collect();
        if results.is_empty() {
            continue;
        }
        let (icon, color) = outcome_icon(t, outcome);
        let head = egui::RichText::new(format!("{} ({})", tl!(outcome.label()), results.len())).font(theme::semibold(13.0));
        let head = if outcome == Outcome::Failed { head.color(FAILED) } else { head };
        egui::CollapsingHeader::new(head).id_salt(("pdfua", outcome.id())).default_open(outcome == Outcome::Failed).show(ui, |ui| {
            for r in results {
                let c = r.condition;
                // A Matterhorn condition goes by its number; one of ISO 14289's own by its clause.
                let id = if c.matterhorn { c.id.to_owned() } else { c.clause() };
                let text = format!("{id} {}: {}", tl!(c.checkpoint), tl!(c.text));
                let row = ui
                    .horizontal(|ui| {
                        ui.add(icons::image(icon, 15.0, color));
                        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(12.5)).color(t.text)).sense(egui::Sense::click()))
                    })
                    .inner
                    .on_hover_text(c.clause());
                if row.clicked() && !r.findings.is_empty() && !expanded.remove(c.id) {
                    expanded.insert(c.id);
                }
                if expanded.contains(c.id) {
                    findings_list(ui, t, &r.findings, &mut action);
                }
            }
        });
    }
    action
}

/// The Accessibility Checker panel.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, state: &mut A11yState, doc: DocId) -> Option<PanelAction> {
    if let Some((_, ua)) = state.ua.as_ref().filter(|(d, _)| *d == doc) {
        return pdfua_panel(ui, t, ua, &mut state.ua_expanded);
    }
    let mut action = None;
    let Some((_, report)) = state.report.as_ref().filter(|(d, _)| *d == doc) else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("accessibility", 32.0, t.text_faint));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("This document hasn't been checked yet.")).color(t.text_muted));
            ui.add_space(6.0);
            if widgets::pill_button(ui, tl!("Check for accessibility"), true).clicked() {
                action = Some(PanelAction::Options);
            }
        });
        return action;
    };
    let failed = report.count(Status::Failed);
    let manual = report.count(Status::Manual);
    let skipped = report.count(Status::Skipped);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(if failed == 0 && manual == 0 && skipped == 0 {
                tl!("No issues found").to_string()
            } else if failed == 1 {
                tl!("1 issue").to_string()
            } else if failed > 1 {
                crate::i18n::fmt(tl!("{n} issues"), &[("n", &failed.to_string())])
            } else if manual > 0 {
                format!("{}: {manual}", tl!("Needs manual check"))
            } else {
                format!("{}: {skipped}", tl!("Skipped"))
            })
            .color(t.text_muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Check again"), false).clicked() {
                action = Some(PanelAction::CheckAgain);
            }
            if widgets::pill_button(ui, tl!("Report"), false).on_hover_text(tl!("Show the accessibility report")).clicked() {
                action = Some(PanelAction::ShowReport);
            }
        });
    });
    if failed > 0 && manual > 0 {
        ui.label(egui::RichText::new(format!("{}: {manual}", tl!("Needs manual check"))).small().color(t.text_muted));
    }
    if let Some((_, previous)) = state.previous.as_ref().filter(|(d, _)| *d == doc) {
        egui::CollapsingHeader::new(tl!("Compare")).default_open(true).show(ui, |ui| {
            for status in [Status::Passed, Status::Failed, Status::Manual, Status::Skipped] {
                ui.label(format!("{}: {} → {}", tl!(status.label()), previous.count(status), report.count(status)));
            }
            for current in &report.results {
                let Some(prior) = previous.result(current.rule) else { continue };
                if prior.status != current.status || prior.findings.len() != current.findings.len() {
                    ui.label(format!(
                        "{}: {} ({}) → {} ({})",
                        tl!(current.rule.name()),
                        tl!(prior.status.label()),
                        prior.findings.len(),
                        tl!(current.status.label()),
                        current.findings.len()
                    ));
                }
            }
        });
    }
    ui.add_space(6.0);
    for c in Category::ALL {
        let rules: Vec<_> = report.in_category(c).collect();
        let issues = rules.iter().filter(|r| r.status == Status::Failed).count();
        let open = !state.collapsed.contains(&c);
        let head = if issues == 0 {
            tl!(c.label()).to_string()
        } else if issues == 1 {
            crate::i18n::fmt(tl!("{label} (1 issue)"), &[("label", tl!(c.label()))])
        } else {
            crate::i18n::fmt(tl!("{label} ({n} issues)"), &[("label", tl!(c.label())), ("n", &issues.to_string())])
        };
        let clicked = ui
            .horizontal(|ui| {
                let toggle = icons::button(
                    ui,
                    if open { "chevron-down" } else { "chevron-right" },
                    20.0,
                    false,
                    if open { tl!("Collapse") } else { tl!("Expand") },
                )
                .clicked();
                let color = if issues > 0 { FAILED } else { t.text };
                let l = ui.add(egui::Label::new(egui::RichText::new(&head).font(theme::semibold(13.0)).color(color)).sense(egui::Sense::click()));
                toggle || l.clicked()
            })
            .inner;
        if clicked {
            if open {
                state.collapsed.insert(c);
            } else {
                state.collapsed.remove(&c);
            }
        }
        if !open {
            continue;
        }
        for r in rules {
            let (icon, color) = status_icon(t, r.status);
            let row = ui
                .horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.add(icons::image(icon, 15.0, color));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::i18n::fmt(
                                tl!("{rule} - {status}"),
                                &[("rule", tl!(r.rule.name())), ("status", tl!(r.status.label()))],
                            ))
                            .font(theme::regular(12.5))
                            .color(t.text),
                        )
                        .sense(egui::Sense::click()),
                    )
                })
                .inner;
            if row.clicked() && !r.findings.is_empty() && !state.expanded.remove(&r.rule) {
                state.expanded.insert(r.rule);
            }
            row.context_menu(|ui| {
                ui.set_min_width(170.0);
                if ui.add_enabled(fixable(r.rule) && r.status == Status::Failed, egui::Button::new(tl!("Fix"))).clicked() {
                    action = Some(PanelAction::Fix(r.rule));
                    ui.close();
                }
                if ui.add_enabled(r.status != Status::Skipped, egui::Button::new(tl!("Skip Rule"))).clicked() {
                    action = Some(PanelAction::Skip(r.rule));
                    ui.close();
                }
                if ui.button(tl!("Explain")).clicked() {
                    state.explain = if state.explain == Some(r.rule) { None } else { Some(r.rule) };
                    ui.close();
                }
                ui.separator();
                if ui.button(tl!("Check Again")).clicked() {
                    action = Some(PanelAction::CheckAgain);
                    ui.close();
                }
                if ui.button(tl!("Show Report")).clicked() {
                    action = Some(PanelAction::ShowReport);
                    ui.close();
                }
                if ui.button(tl!("Options…")).clicked() {
                    action = Some(PanelAction::Options);
                    ui.close();
                }
            });
            if state.explain == Some(r.rule) {
                egui::Frame::NONE.inner_margin(egui::Margin { left: 46, right: 4, top: 0, bottom: 6 }).show(ui, |ui| {
                    ui.label(egui::RichText::new(tl!(r.rule.explanation())).small().color(t.text_muted));
                });
            }
            if state.expanded.contains(&r.rule) {
                findings_list(ui, t, &r.findings, &mut action);
            }
        }
    }
    action
}

impl PdfCraftApp {
    /// Prepare for accessibility ▸ Find untagged content.
    pub(crate) fn find_untagged_content(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let options = Options { rules: [Rule::TaggedContent, Rule::TaggedAnnotations, Rule::TaggedFormFields].into_iter().collect(), pages: None };
        let Some(report) = doc.accessibility_check(&options) else {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        let failed = report.count(Status::Failed);
        self.a11y.previous = None;
        self.a11y.options = Some(options);
        self.a11y.report = Some((id, report));
        self.a11y.ua = None;
        self.right = Some(RightPanel::Accessibility);
        self.notify(if failed == 0 {
            tl!("No untagged content was found.").to_string()
        } else {
            crate::i18n::fmt(tl!("Found {n} untagged content problems."), &[("n", &failed.to_string())])
        });
    }

    /// Check for accessibility: the options first, unless they are turned off.
    pub(crate) fn start_accessibility_check(&mut self) {
        if self.a11y_options.show_dialog {
            self.dialog = Some(Dialog::AccessibilityOptions);
        } else {
            self.run_accessibility_check();
        }
    }

    /// Prepare for accessibility ▸ Verify PDF/UA: against the part the document declares (PDF/UA-1
    /// unless it declares PDF/UA-2), which the options dialog then keeps.
    pub(crate) fn check_pdfua(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let part = Part::for_declared(doc.standards().pdfua);
        self.a11y_options.profile = Profile::PdfUa(part);
        self.verify_pdfua(part);
    }

    /// Verify the document against `part` of PDF/UA and show the conditions in the panel.
    pub fn verify_pdfua(&mut self, part: Part) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(report) = doc.pdfua_verify(part) else {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        let failed = report.count(Outcome::Failed);
        let manual = report.count(Outcome::Manual);
        self.a11y.ua = Some((id, report));
        self.a11y.ua_expanded.clear();
        self.right = Some(RightPanel::Accessibility);
        let found = if failed == 0 {
            tl!("No issues found").to_string()
        } else if failed == 1 {
            tl!("1 issue").to_string()
        } else {
            crate::i18n::fmt(tl!("{n} issues"), &[("n", &failed.to_string())])
        };
        self.notify(format!("{}: {found} · {}: {manual}", part.label(), tl!("Needs manual check")));
    }

    /// Run the full check with the current options and show the results: PDF/UA verification when
    /// the options' profile is PDF/UA, else the Accessibility Checker.
    pub fn run_accessibility_check(&mut self) {
        if let Profile::PdfUa(part) = self.a11y_options.profile {
            self.verify_pdfua(part);
            return;
        }
        self.run_checker();
    }

    /// The Accessibility Checker's rules, whatever the options' profile: what the Checker panel's
    /// Check Again, Fix and Report act on, after a PDF/UA verification too.
    fn run_checker(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let o = &self.a11y_options;
        // The range was picked on whatever document had the options dialog last; with the dialog
        // turned off it reaches here unchecked, and a page past the end would select nothing.
        let to = o.to.clamp(1, doc.info.pages.len().max(1));
        let from = o.from.clamp(1, to);
        let options = Options { rules: o.rules.clone(), pages: (!o.all_pages).then(|| (from - 1..to).collect()) };
        let Some(report) = doc.accessibility_check(&options) else {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        // Rules skipped by hand stay skipped when checking again.
        let mut report = report;
        for r in report.results.iter_mut().filter(|r| self.a11y_skipped.contains(&(id, r.rule))) {
            r.status = Status::Skipped;
            r.findings.clear();
        }
        let failed = report.count(Status::Failed);
        let manual = report.count(Status::Manual);
        let skipped = report.count(Status::Skipped);
        self.a11y.previous =
            if self.a11y.options.as_ref() == Some(&options) { self.a11y.report.take().filter(|(prior_id, _)| *prior_id == id) } else { None };
        self.a11y.options = Some(options);
        self.a11y.report = Some((id, report));
        self.a11y.ua = None;
        self.right = Some(RightPanel::Accessibility);
        self.notify(if failed == 0 && manual == 0 && skipped == 0 {
            tl!("The checker found no problems in this document.").to_string()
        } else if failed == 1 {
            tl!("The checker found 1 problem which may prevent the document from being fully accessible.").to_string()
        } else if failed > 1 {
            crate::i18n::fmt(
                tl!("The checker found {n} problems which may prevent the document from being fully accessible."),
                &[("n", &failed.to_string())],
            )
        } else if manual > 0 {
            format!("{}: {manual}", tl!("Needs manual check"))
        } else {
            format!("{}: {skipped}", tl!("Skipped"))
        });
        if self.a11y_options.create_report {
            self.show_accessibility_report();
        }
    }

    /// Save the accessibility report (HTML) for the last check, checking first if needed.
    pub fn show_accessibility_report(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        if !self.a11y.report.as_ref().is_some_and(|(d, _)| *d == id) {
            let create = std::mem::replace(&mut self.a11y_options.create_report, false);
            // The report is the Checker's, even when the last check was a PDF/UA verification.
            self.run_checker();
            self.a11y_options.create_report = create;
        }
        let Some((_, report)) = self.a11y.report.as_ref().filter(|(d, _)| *d == id) else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (y, m, d) = self.session.today();
        // The saved report reads the way the interface reads, down to its `lang` attribute.
        let lang = crate::i18n::current();
        let html = pdfcraft_engine::a11y::report_html(report, &doc.name, &format!("{y}-{m:02}-{d:02}"), lang.code(), &crate::i18n::words);
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let title = tl!("Save the accessibility report").to_string();
        self.write_files(&[(format!("{stem} Accessibility Report.html"), std::sync::Arc::new(html.into_bytes()))], &title);
    }

    /// Turn to a finding's page and, when the checker located it, flash its box there.
    fn a11y_show_on_page(&mut self, index: usize, page: usize, bbox: Option<[f64; 4]>) {
        let Some(view) = self.views.get_mut(index) else { return };
        view.go_to_page(page);
        if let Some(bbox) = bbox {
            view.flash = Some((page, bbox.map(|v| v as f32), 0.0));
        }
    }

    pub(crate) fn a11y_action(&mut self, index: usize, action: PanelAction) {
        match action {
            // The Checker panel's own actions run the Checker; the PDF/UA panel sends VerifyPdfUa.
            PanelAction::CheckAgain => self.run_checker(),
            PanelAction::VerifyPdfUa(part) => self.verify_pdfua(part),
            PanelAction::Options => self.dialog = Some(Dialog::AccessibilityOptions),
            PanelAction::ShowReport => self.show_accessibility_report(),
            PanelAction::GoTo(p, bbox) => self.a11y_show_on_page(index, p, bbox),
            PanelAction::GoToTag(object, page, bbox) => {
                let Some((_, id)) = self.active_ids() else { return };
                let Some(tags) = self.session.get(id).map(|doc| self.tags_state.tags(doc)) else { return };
                if !tags.iter().any(|tag| tag.object == object) {
                    if let Some(page) = page {
                        self.a11y_show_on_page(index, page, bbox);
                    }
                    return;
                }
                if self.tags_state.select(id, &tags, object) {
                    self.right = Some(RightPanel::Tags);
                    if let Some(page) = page {
                        self.a11y_show_on_page(index, page, bbox);
                    }
                } else {
                    self.notify_tr("Save or revert changes before selecting another tag.");
                }
            }
            PanelAction::Skip(rule) => {
                if let Some((id, r)) = self.a11y.report.as_mut() {
                    self.a11y_skipped.insert((*id, rule));
                    if let Some(x) = r.results.iter_mut().find(|x| x.rule == rule) {
                        x.status = Status::Skipped;
                        x.findings.clear();
                    }
                }
            }
            PanelAction::Fix(rule) => {
                let Some((_, id)) = self.active_ids() else { return };
                match rule {
                    // The language is chosen in Document Properties ▸ Advanced.
                    Rule::PrimaryLanguage => self.dialog = Some(Dialog::Properties(PropsTab::Advanced)),
                    _ => {
                        let edit = self.session.get(id).map(|d| d.accessibility_fix(rule, None));
                        match edit {
                            Some(Ok(e)) => {
                                if !self.apply_edit(e) {
                                    return;
                                }
                                self.run_checker();
                                // The title can then be edited.
                                if rule == Rule::Title {
                                    self.dialog = Some(Dialog::Properties(PropsTab::Description));
                                }
                            }
                            Some(Err(e)) => self.notify_error(e),
                            None => {}
                        }
                    }
                }
            }
        }
    }
}

/// Add alternate text: the figures one by one.
#[derive(Clone, Default)]
pub struct AltDraft {
    pub doc: Option<DocId>,
    pub figures: Vec<pdfcraft_engine::a11y::Figure>,
    pub index: usize,
    pub texts: Vec<String>,
    pub decorative: Vec<bool>,
    /// The figure shown (index, picture).
    preview: Option<(usize, egui::TextureHandle)>,
}

impl std::fmt::Debug for AltDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AltDraft").field("figures", &self.figures.len()).field("index", &self.index).finish()
    }
}

/// Render a figure's area for the dialog, at the preview pane's device resolution.
fn figure_picture(ctx: &egui::Context, doc: &pdfcraft_engine::Document, f: &pdfcraft_engine::a11y::Figure) -> Option<egui::TextureHandle> {
    let (page, b) = (f.page?, f.bbox?);
    let info = doc.info.pages.get(page)?;
    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
    let (x0, y0, x1, y1) = (u[0].min(v[0]), u[1].min(v[1]), u[0].max(v[0]), u[1].max(v[1]));
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let ppp = ctx.pixels_per_point().max(1.0);
    let scale = ((380.0 * ppp) / w).min((220.0 * ppp) / h).clamp(0.05, 32.0);
    let tile = pdfcraft_render::Tile {
        x: (x0 * scale).floor().max(0.0) as u32,
        y: (y0 * scale).floor().max(0.0) as u32,
        w: (w * scale).ceil().max(1.0) as u32,
        h: (h * scale).ceil().max(1.0) as u32,
    };
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
    let out = r.render(pdfcraft_render::RenderRequest {
        page,
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: Some(tile),
        scale,
        tag: 0,
        preview: false,
    });
    if out.error.is_some() || out.width == 0 {
        return None;
    }
    let img = egui::ColorImage::from_rgba_premultiplied([out.width as usize, out.height as usize], &out.rgba);
    Some(ctx.load_texture("alt-figure", img, egui::TextureOptions::LINEAR))
}

pub(crate) fn alt_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let ctx = ui.ctx().clone();
    let doc = app.alt_draft.doc.and_then(|id| app.session.get(id));
    let d = &mut app.alt_draft;
    ui.label(egui::RichText::new(tl!("Set Alternate Text")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let n = d.figures.len();
    if n == 0 {
        ui.label(egui::RichText::new(tl!("This document has no tagged figures.")).color(t.text_muted));
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| cancel = widgets::pill_button(ui, tl!("Close"), true).clicked())
        });
        return (false, cancel);
    }
    d.index = d.index.min(n - 1);
    let i = d.index;
    if d.preview.as_ref().is_none_or(|(k, _)| *k != i)
        && let Some(doc) = doc
    {
        d.preview = figure_picture(&ctx, doc, &d.figures[i]).map(|tex| (i, tex));
    }
    let page = d.figures[i].page.map(|p| crate::i18n::fmt(tl!(" on page {p}"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
    ui.label(
        egui::RichText::new(crate::i18n::fmt(tl!("Figure {i} of {n}{page}"), &[("i", &(i + 1).to_string()), ("n", &n.to_string()), ("page", &page)]))
            .color(t.text_muted),
    );
    ui.add_space(6.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        let (area, _) = ui.allocate_exact_size(egui::vec2(380.0, 220.0), egui::Sense::hover());
        match &d.preview {
            Some((_, tex)) => {
                let s = tex.size_vec2();
                let k = (area.width() / s.x).min(area.height() / s.y).min(2.0);
                let fit = egui::Rect::from_center_size(area.center(), s * k);
                ui.painter().image(tex.id(), fit, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None => {
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, tl!("No preview"), theme::regular(13.0), t.text_faint);
            }
        }
    });
    ui.add_space(8.0);
    ui.add_enabled_ui(!d.decorative[i], |ui| {
        let l = ui.label(tl!("Alternate text"));
        ui.add(crate::widgets::block(&mut d.texts[i]).desired_rows(3).desired_width(380.0).hint_text(tl!("Describe the figure"))).labelled_by(l.id);
    });
    ui.add(widgets::checkbox(&mut d.decorative[i], tl!("Decorative figure")));
    ui.add_space(10.0);
    let (mut save, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if ui.add_enabled_ui(i > 0, |ui| icons::button(ui, "chevron-left", 28.0, false, tl!("Previous figure"))).inner.clicked() {
            d.index -= 1;
        }
        if ui.add_enabled_ui(i + 1 < n, |ui| icons::button(ui, "chevron-right", 28.0, false, tl!("Next figure"))).inner.clicked() {
            d.index += 1;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Save & Close"), true).clicked() {
                save = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (save, cancel)
}

impl PdfCraftApp {
    /// Prepare for accessibility ▸ Tag form fields.
    pub(crate) fn autotag_form_fields(&mut self) {
        self.apply_edit(pdfcraft_engine::Edit::AutoTagFormFields);
        self.choose_right_panel(Some(RightPanel::Tags));
    }

    /// Prepare for accessibility ▸ Add alternate text.
    pub(crate) fn start_alt_text(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let figures = doc.figures();
        self.alt_draft = AltDraft {
            doc: Some(id),
            texts: figures.iter().map(|f| f.alt.clone().unwrap_or_default()).collect(),
            decorative: vec![false; figures.len()],
            figures,
            index: 0,
            preview: None,
        };
        self.dialog = Some(Dialog::AltText);
    }

    /// Save & Close: one undo step for every change.
    pub(crate) fn save_alt_text(&mut self) {
        let d = std::mem::take(&mut self.alt_draft);
        let mut edits = Vec::new();
        for (k, f) in d.figures.iter().enumerate() {
            if d.decorative[k] {
                edits.push(pdfcraft_engine::Edit::MarkDecorative { figure: f.obj.num });
            } else if d.texts[k].trim() != f.alt.as_deref().unwrap_or("").trim() {
                edits.push(pdfcraft_engine::Edit::SetAltText { figure: f.obj.num, alt: Some(d.texts[k].clone()) });
            }
        }
        if !edits.is_empty() {
            self.apply_edit(pdfcraft_engine::Edit::Batch { label: "Set alternate text".into(), edits });
        }
    }
}
