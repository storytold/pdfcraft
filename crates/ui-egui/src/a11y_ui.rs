//! Prepare for accessibility ▸ Check for accessibility: the Accessibility Checker Options dialog
//! and the Accessibility Checker panel (results by category, with Fix, Skip rule, Explain,
//! Check again and Show report on each rule).

use std::collections::BTreeSet;

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::a11y::{Category, Options, Report, Rule, Status};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, PropsTab, RightPanel, icons, widgets};

/// The options dialog's settings (kept for the session, like Acrobat's).
#[derive(Clone, Debug, PartialEq)]
pub struct A11yOptions {
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
    /// Collapsed categories and expanded rules.
    pub collapsed: BTreeSet<Category>,
    pub expanded: BTreeSet<Rule>,
    /// The rule whose explanation is showing.
    pub explain: Option<Rule>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PanelAction {
    Fix(Rule),
    Skip(Rule),
    CheckAgain,
    ShowReport,
    Options,
    GoTo(usize),
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
    ui.label(egui::RichText::new(tl!("Accessibility Checker Options")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
        egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
        ui.add_space(8.0);
    };
    group(ui, "Report Options", &mut |ui| {
        ui.checkbox(&mut o.create_report, tl!("Create accessibility report"));
    });
    group(ui, "Page Range", &mut |ui| {
        ui.horizontal(|ui| {
            ui.radio_value(&mut o.all_pages, true, tl!("All pages in document"));
            ui.radio_value(&mut o.all_pages, false, tl!("Pages from"));
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
            if ui.checkbox(&mut checked, tl!(r.description())).changed() {
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
    ui.checkbox(&mut o.show_dialog, tl!("Show this dialog when the Checker starts"));
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

/// The Accessibility Checker panel.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, state: &mut A11yState, doc: DocId) -> Option<PanelAction> {
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
    ui.horizontal(|ui| {
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
                action = Some(PanelAction::CheckAgain);
            }
            if widgets::pill_button(ui, tl!("Report"), false).on_hover_text(tl!("Show the accessibility report")).clicked() {
                action = Some(PanelAction::ShowReport);
            }
        });
    });
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
                egui::Frame::NONE.inner_margin(egui::Margin { left: 46, right: 4, top: 0, bottom: 6 }).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for f in &r.findings {
                        let l = ui.add(egui::Label::new(egui::RichText::new(&f.message).small().color(t.text_muted)).sense(egui::Sense::click()));
                        if let Some(p) = f.page
                            && l.on_hover_text(tl!("Go to the page")).clicked()
                        {
                            action = Some(PanelAction::GoTo(p));
                        }
                    }
                });
            }
        }
    }
    action
}

impl PdfCraftApp {
    /// Check for accessibility: the options first, unless they are turned off.
    pub(crate) fn start_accessibility_check(&mut self) {
        if self.a11y_options.show_dialog {
            self.dialog = Some(Dialog::AccessibilityOptions);
        } else {
            self.run_accessibility_check();
        }
    }

    /// Run the full check with the current options and show the results.
    pub fn run_accessibility_check(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let o = &self.a11y_options;
        let options = Options { rules: o.rules.clone(), pages: (!o.all_pages).then(|| (o.from.max(1) - 1..o.to.max(o.from)).collect()) };
        let Some(report) = doc.accessibility_check(&options) else {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        // Rules skipped by hand stay skipped when checking again.
        let mut report = report;
        for r in report.results.iter_mut().filter(|r| self.a11y_skipped.contains(&r.rule)) {
            r.status = Status::Skipped;
            r.findings.clear();
        }
        let failed = report.count(Status::Failed);
        self.a11y.report = Some((id, report));
        self.right = Some(RightPanel::Accessibility);
        self.notify(if failed == 0 {
            tl!("The checker found no problems in this document.").to_string()
        } else if failed == 1 {
            tl!("The checker found 1 problem which may prevent the document from being fully accessible.").to_string()
        } else {
            crate::i18n::fmt(
                tl!("The checker found {n} problems which may prevent the document from being fully accessible."),
                &[("n", &failed.to_string())],
            )
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
            self.run_accessibility_check();
            self.a11y_options.create_report = create;
        }
        let Some((_, report)) = self.a11y.report.as_ref().filter(|(d, _)| *d == id) else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (y, m, d) = self.session.today();
        let html = pdfcraft_engine::a11y::report_html(report, &doc.name, &format!("{y}-{m:02}-{d:02}"));
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let title = tl!("Save the accessibility report").to_string();
        self.write_files(&[(format!("{stem} Accessibility Report.html"), std::sync::Arc::new(html.into_bytes()))], &title);
    }

    pub(crate) fn a11y_action(&mut self, index: usize, action: PanelAction) {
        match action {
            PanelAction::CheckAgain => self.run_accessibility_check(),
            PanelAction::Options => self.dialog = Some(Dialog::AccessibilityOptions),
            PanelAction::ShowReport => self.show_accessibility_report(),
            PanelAction::GoTo(p) => self.views[index].go_to_page(p),
            PanelAction::Skip(rule) => {
                self.a11y_skipped.insert(rule);
                if let Some((_, r)) = self.a11y.report.as_mut()
                    && let Some(x) = r.results.iter_mut().find(|x| x.rule == rule)
                {
                    x.status = Status::Skipped;
                    x.findings.clear();
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
                                if self.apply_edit(e) {
                                    self.run_accessibility_check();
                                    // The title can then be edited.
                                    if rule == Rule::Title {
                                        self.dialog = Some(Dialog::Properties(PropsTab::Description));
                                    }
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

/// Render a figure's area for the dialog (at most 360 × 220 px).
fn figure_picture(ctx: &egui::Context, doc: &pdfcraft_engine::Document, f: &pdfcraft_engine::a11y::Figure) -> Option<egui::TextureHandle> {
    let (page, b) = (f.page?, f.bbox?);
    let info = doc.info.pages.get(page)?;
    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
    let (x0, y0, x1, y1) = (u[0].min(v[0]), u[1].min(v[1]), u[0].max(v[0]), u[1].max(v[1]));
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let scale = (360.0 / w).min(220.0 / h).clamp(0.05, 8.0);
    let tile = pdfcraft_render::Tile {
        x: (x0 * scale).floor().max(0.0) as u32,
        y: (y0 * scale).floor().max(0.0) as u32,
        w: (w * scale).ceil().max(1.0) as u32,
        h: (h * scale).ceil().max(1.0) as u32,
    };
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
    let out = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Pixels, tile: Some(tile), scale, tag: 0 });
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
        ui.add(egui::TextEdit::multiline(&mut d.texts[i]).desired_rows(3).desired_width(380.0).hint_text(tl!("Describe the figure")))
            .labelled_by(l.id);
    });
    ui.checkbox(&mut d.decorative[i], tl!("Decorative figure"));
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
