//! Prepare for accessibility ▸ Check for accessibility: the Accessibility Checker Options dialog,
//! the Accessibility Checker panel (results by category, with Fix, Skip rule, Explain, Check
//! again and Show report on each rule) and the Set Alternate Text dialog.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use craft::behavior::{DisplayOrder, IconId, ItemSource, Selection, SelectionChange};
use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::pages::{PageRange, PageScope};
use craft::theme::variant::Tone;
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::{Bind, CollectionEvent, Entry, Outcome as _, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::dialog::{Alert, DialogId, DialogResult, DialogStack, HostedAlert, StackDialog, dialog_id};
use craft::ui::display::{Picture, PictureSize, Shade, Text};
use craft::ui::layout_components::Stack;
use craft::ui::list::{List, ListRow, RowInk, ThumbAspect, ThumbSize};
use craft::ui::menu::Menu;
use craft::ui::notices::{Notice, NoticeAction, NoticeIcon};
use craft::ui::patterns::form_dialogs::{DialogFields, FormDialog, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::primitives::{IconSpec, has_icon};
use craft::ui::sections::Section;
use craft::ui::text::{AltText, AltTextField};
use egui::Ui;
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::DocId;
use pdfcraft_engine::a11y::{Category, Figure, Options, Report, Rule, Status};

use crate::{PdfCraftApp, PropsTab, RightPanel};

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
    /// The prior check of the same document with the same checker options.
    pub previous: Option<(DocId, Report)>,
    pub options: Option<Options>,
    /// Collapsed categories and expanded rules.
    pub collapsed: BTreeSet<Category>,
    pub expanded: BTreeSet<Rule>,
    /// The rule whose explanation is showing.
    pub explain: Option<Rule>,
    /// The rule whose context menu is open.
    pub menu: Option<Rule>,
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
}

/// Rules with an automatic (or guided) fix.
pub(crate) fn fixable(rule: Rule) -> bool {
    matches!(rule, Rule::PrimaryLanguage | Rule::Title | Rule::TabOrder)
}

/// The key of a rule's check box in the options dialog.
fn rule_key(rule: Rule) -> String {
    format!("rule.{rule:?}")
}

/// The id of a category among the dialog's choices.
fn category_id(category: Category) -> String {
    format!("{category:?}")
}

/// The category the options dialog lists rules of.
fn shown_category(fields: &DialogFields) -> Category {
    fields.text("category").and_then(|id| Category::ALL.into_iter().find(|c| category_id(*c) == id)).unwrap_or(Category::Document)
}

/// The keys of the rules the options dialog lists now.
fn shown_rule_keys(fields: &DialogFields) -> Vec<String> {
    let category = shown_category(fields);
    Rule::ALL.into_iter().filter(|r| r.category() == category).map(rule_key).collect()
}

/// The options dialog's values at opening: the saved options, the range held to the document.
fn options_fields(o: &A11yOptions, pages: usize) -> DialogFields {
    let to = o.to.clamp(1, pages);
    let from = o.from.clamp(1, to);
    let range = PageRange { scope: if o.all_pages { PageScope::All } else { PageScope::Range }, from, to: Some(to), ..PageRange::default() };
    let mut fields = DialogFields::new();
    fields.set("create_report", o.create_report);
    fields.set("range", PageRangeOptions::field_value(&range));
    fields.set("category", category_id(o.category));
    for rule in Rule::ALL {
        fields.set(rule_key(rule), o.rules.contains(&rule));
    }
    fields.set("show_dialog", o.show_dialog);
    fields
}

/// The options dialog over its values.
fn options_dialog(fields: &mut DialogFields, pages: usize, current: usize) -> ParamDialog<'_> {
    let category = shown_category(fields);
    let on = Rule::ALL.into_iter().filter(|r| fields.bool(&rule_key(*r)) == Some(true)).count();
    let heading = crate::i18n::fmt(
        tl!("Checking Options ({on} of {all} in all categories)"),
        &[("on", &on.to_string()), ("all", &Rule::ALL.len().to_string())],
    );
    let categories = Category::ALL.into_iter().map(|c| (category_id(c), tl!(c.label()).into())).collect();
    let mut params = vec![
        Param::section(tl!("Report Options")),
        Param::new("create_report", tl!("Create accessibility report"), ParamKind::bool(false)),
        PageRangeOptions::param("range", tl!("Page Range"), pages, current, PageRangeChoices::FromTo),
        // Keyed apart from its changing label, so the section keeps its identity as the count moves.
        Param::new("section:checking", heading, ParamKind::Section),
        Param::new("category", tl!("Category:"), ParamKind::choice(categories, Some(category_id(Category::Document)))),
    ];
    for rule in Rule::ALL {
        params
            .push(Param::new(rule_key(rule), tl!(rule.description()), ParamKind::bool(true)).visible_when("category", category_id(rule.category())));
    }
    params.push(
        Param::custom("rules.all", tl!("Rules"), |ui, fields, _| {
            let keys = shown_rule_keys(fields);
            let all = Button::new(tl!("Select All")).show(ui);
            let none = Button::new(tl!("Clear All")).show(ui);
            if all.response().clicked() {
                for key in &keys {
                    fields.set(key.clone(), true);
                }
            }
            if none.response().clicked() {
                for key in &keys {
                    fields.set(key.clone(), false);
                }
            }
            all.response().union(none.response().clone())
        })
        .companions(shown_rule_keys(fields)),
    );
    params.push(Param::new("show_dialog", tl!("Show this dialog when the Checker starts"), ParamKind::bool(true)));
    ParamDialog::new("a11y.options", tl!("Accessibility Checker Options"), params).values(Bind::bound(fields)).ok_label(tl!("Start Checking").into())
}

/// Opens the Accessibility Checker Options dialog over the saved options.
pub(crate) fn open_a11y_options(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((index, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let pages = doc.info.pages.len().max(1);
    let current = app.views.get(index).map_or(1, |v| v.current() + 1);
    let fields = options_fields(&app.a11y_options, pages);
    app.dialogs.open(HostedParamDialog::new(ctx, "a11y.options", fields, move |fields| options_dialog(fields, pages, current)));
}

/// What the dialogs of this module answered: called once a pass, after the host ran.
pub(crate) fn pass(ctx: &egui::Context, app: &mut PdfCraftApp) {
    if let Some(DialogResult::Ok(result)) = app.dialogs.take_by_id::<HostedParamDialog>(dialog_id("a11y.options")) {
        let Some((_, id)) = app.active_ids() else { return };
        let pages = app.session.get(id).map_or(1, |d| d.info.pages.len().max(1));
        let fields = &result.fields;
        let o = &mut app.a11y_options;
        o.create_report = fields.bool("create_report").unwrap_or(o.create_report);
        o.show_dialog = fields.bool("show_dialog").unwrap_or(o.show_dialog);
        o.category = shown_category(fields);
        o.rules = Rule::ALL.into_iter().filter(|r| fields.bool(&rule_key(*r)).unwrap_or_else(|| o.rules.contains(r))).collect();
        // `FromTo` offers neither Current page nor a subset, so only All and Range arrive.
        if let Some(range) = PageRangeOptions::value(fields, "range") {
            o.all_pages = range.scope == PageScope::All;
            o.from = range.from;
            o.to = range.to.unwrap_or(pages);
        }
        // Starting again forgets the skips made in this document.
        app.a11y_skipped.retain(|(doc, _)| *doc != id);
        app.run_accessibility_check();
    }
    if let Some(DialogResult::Ok(answer)) = app.dialogs.take::<AltTextDialog>() {
        app.save_alt_text(ctx, answer);
    }
}

/// The icon and tone of a rule's status.
fn status_icon(s: Status) -> (&'static str, Tone) {
    match s {
        Status::Passed => ("circle-check", Tone::Good),
        Status::Failed => ("circle-x", Tone::Bad),
        Status::Manual => ("circle-help", Tone::Warn),
        Status::Skipped => ("circle-dashed", Tone::Neutral),
    }
}

/// What a row of the results list stands for.
enum LineKind {
    Category { label: String, issues: usize },
    Rule { rule: Rule, status: Status },
    Finding { rule: Rule, index: usize },
}

/// One row of the results tree, in display order.
struct Line {
    key: u64,
    depth: usize,
    /// `Some(open)` for a row with children.
    group: Option<bool>,
    kind: LineKind,
}

/// The results rows as the list's display order.
struct Lines(Vec<Line>);

/// Rule rows come after the categories' keys; finding rows after every rule's.
const RULE_KEY_BASE: u64 = 100;
const FINDING_KEY_BASE: u64 = 1_000_000;
const FINDING_KEY_STRIDE: u64 = 1_000_000;

impl DisplayOrder<u64> for Lines {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn key(&self, at: usize) -> u64 {
        self.0.get(at).map_or(0, |line| line.key)
    }
    fn depth(&self, at: usize) -> usize {
        self.0.get(at).map_or(0, |line| line.depth)
    }
    fn has_children(&self, at: usize) -> bool {
        self.0.get(at).is_some_and(|line| line.group.is_some())
    }
    fn open(&self, at: usize) -> bool {
        self.0.get(at).is_some_and(|line| line.group == Some(true))
    }
}

impl ItemSource<u64> for Lines {}

fn rule_position(rule: Rule) -> u64 {
    Rule::ALL.iter().position(|r| *r == rule).unwrap_or(0) as u64
}

/// The results tree: categories with their rules, and the findings of the expanded rules.
fn result_lines(report: &Report, collapsed: &BTreeSet<Category>, expanded: &BTreeSet<Rule>) -> Lines {
    let mut lines = Vec::new();
    for (position, c) in Category::ALL.into_iter().enumerate() {
        let rules: Vec<_> = report.in_category(c).collect();
        let issues = rules.iter().filter(|r| r.status == Status::Failed).count();
        let open = !collapsed.contains(&c);
        let label = if issues == 0 {
            tl!(c.label()).to_string()
        } else if issues == 1 {
            crate::i18n::fmt(tl!("{label} (1 issue)"), &[("label", tl!(c.label()))])
        } else {
            crate::i18n::fmt(tl!("{label} ({n} issues)"), &[("label", tl!(c.label())), ("n", &issues.to_string())])
        };
        lines.push(Line { key: position as u64, depth: 0, group: Some(open), kind: LineKind::Category { label, issues } });
        if !open {
            continue;
        }
        for r in rules {
            let at = rule_position(r.rule);
            let shown = !r.findings.is_empty() && expanded.contains(&r.rule);
            lines.push(Line {
                key: RULE_KEY_BASE + at,
                depth: 1,
                group: (!r.findings.is_empty()).then_some(shown),
                kind: LineKind::Rule { rule: r.rule, status: r.status },
            });
            if shown {
                for index in 0..r.findings.len() {
                    lines.push(Line {
                        key: FINDING_KEY_BASE + at * FINDING_KEY_STRIDE + index as u64,
                        depth: 2,
                        group: None,
                        kind: LineKind::Finding { rule: r.rule, index },
                    });
                }
            }
        }
    }
    Lines(lines)
}

/// What opening a finding does: its tag when it has one, else its page.
fn finding_action(report: &Report, rule: Rule, index: usize) -> Option<PanelAction> {
    let f = report.result(rule)?.findings.get(index)?;
    if let Some(object) = f.object { Some(PanelAction::GoToTag(object, f.page, f.bbox)) } else { f.page.map(|p| PanelAction::GoTo(p, f.bbox)) }
}

/// The summary line over the results.
fn summary(report: &Report) -> String {
    let failed = report.count(Status::Failed);
    let manual = report.count(Status::Manual);
    let skipped = report.count(Status::Skipped);
    if failed == 0 && manual == 0 && skipped == 0 {
        tl!("No issues found").to_string()
    } else if failed == 1 {
        tl!("1 issue").to_string()
    } else if failed > 1 {
        crate::i18n::fmt(tl!("{n} issues"), &[("n", &failed.to_string())])
    } else if manual > 0 {
        format!("{}: {manual}", tl!("Needs manual check"))
    } else {
        format!("{}: {skipped}", tl!("Skipped"))
    }
}

/// The Accessibility Checker panel.
pub(crate) fn panel(ui: &mut Ui, state: &mut A11yState, doc: DocId) -> Option<PanelAction> {
    let A11yState { report, previous, collapsed, expanded, explain, menu, .. } = state;
    let mut action = None;
    let Some((_, report)) = report.as_ref().filter(|(d, _)| *d == doc) else {
        let mut notice = Notice::new(Tone::Neutral, tl!("This document hasn't been checked yet."));
        let icon = IconId::new("accessibility");
        if has_icon(&icon) {
            notice = notice.icon(NoticeIcon::Custom(IconSpec::svg(icon)));
        }
        let shown = notice.action(NoticeAction::new(tl!("Check for accessibility"))).show(ui);
        if shown.action == Some(0) {
            action = Some(PanelAction::Options);
        }
        return action;
    };
    let failed = report.count(Status::Failed);
    let manual = report.count(Status::Manual);
    Text::new(summary(report)).shade(Shade::Muted).show(ui);
    if Button::new(tl!("Check again")).show(ui).response().clicked() {
        action = Some(PanelAction::CheckAgain);
    }
    if Button::new(tl!("Report")).show(ui).response().clicked() {
        action = Some(PanelAction::ShowReport);
    }
    if failed > 0 && manual > 0 {
        Text::new(format!("{}: {manual}", tl!("Needs manual check"))).shade(Shade::Muted).show(ui);
    }
    if let Some((_, previous)) = previous.as_ref().filter(|(d, _)| *d == doc) {
        Section::new(tl!("Compare")).show(ui, |ui, _| {
            for status in [Status::Passed, Status::Failed, Status::Manual, Status::Skipped] {
                Text::new(format!("{}: {} → {}", tl!(status.label()), previous.count(status), report.count(status))).show(ui);
            }
            for current in &report.results {
                let Some(prior) = previous.result(current.rule) else { continue };
                if prior.status != current.status || prior.findings.len() != current.findings.len() {
                    Text::new(format!(
                        "{}: {} ({}) → {} ({})",
                        tl!(current.rule.name()),
                        tl!(prior.status.label()),
                        prior.findings.len(),
                        tl!(current.status.label()),
                        current.findings.len()
                    ))
                    .show(ui);
                }
            }
        });
    }

    let lines = result_lines(report, collapsed, expanded);
    let explained = *explain;
    // A click on a finding is reported as a selection of it; the selection is not kept, so the same finding
    // can be opened again.
    let mut picked = Selection::new(Vec::new(), None, None);
    let out = List::<u64>::new(tl!("Accessibility results"))
        .tree(true)
        .natural_height(true)
        .select(SelectMode::Single)
        .selection(Bind::bound(&mut picked))
        .show_items(ui, &lines, &mut |key, _| {
            let Some(line) = lines.0.iter().find(|l| l.key == *key) else { return ListRow::new("") };
            let row = ListRow::new(match &line.kind {
                LineKind::Category { label, .. } => label.clone(),
                LineKind::Rule { rule, status } => {
                    crate::i18n::fmt(tl!("{rule} - {status}"), &[("rule", tl!(rule.name())), ("status", tl!(status.label()))])
                }
                // The checker hands over the sentence and the document's own words apart, so the sentence is said in
                // the panel's language with those words still in it.
                LineKind::Finding { rule, index } => {
                    report.result(*rule).and_then(|r| r.findings.get(*index)).map(|f| f.said(&crate::i18n::words).to_string()).unwrap_or_default()
                }
            })
            .depth(line.depth);
            let row = match line.group {
                Some(open) => row.group(open),
                None => row,
            };
            match &line.kind {
                LineKind::Category { issues, .. } if *issues > 0 => row.icon(IconSpec::svg(IconId::new("circle-x"))).tone(Tone::Bad),
                LineKind::Category { .. } | LineKind::Finding { .. } => row,
                LineKind::Rule { rule, status } => {
                    let (icon, tone) = status_icon(*status);
                    let row = row.icon(IconSpec::svg(IconId::new(icon))).tone(tone);
                    let row = if *status == Status::Skipped { row.ink(RowInk::Muted) } else { row };
                    if explained == Some(*rule) { row.below(Text::new(tl!(rule.explanation())).shade(Shade::Muted), |_| {}) } else { row }
                }
            }
        });
    for event in &out.events {
        match event {
            CollectionEvent::Open(change) => match lines.0.iter().find(|l| l.key == change.key).map(|l| &l.kind) {
                Some(LineKind::Category { .. }) => {
                    if let Some(c) = Category::ALL.into_iter().nth(change.key as usize) {
                        if change.open {
                            collapsed.remove(&c);
                        } else {
                            collapsed.insert(c);
                        }
                    }
                }
                Some(LineKind::Rule { rule, .. }) => {
                    if change.open {
                        expanded.insert(*rule);
                    } else {
                        expanded.remove(rule);
                    }
                }
                _ => {}
            },
            CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                if let Some(LineKind::Finding { rule, index }) = keys.last().and_then(|k| lines.0.iter().find(|l| l.key == *k)).map(|l| &l.kind) {
                    action = finding_action(report, *rule, *index).or(action);
                }
            }
            CollectionEvent::Activate(key) => {
                if let Some(LineKind::Finding { rule, index }) = lines.0.iter().find(|l| l.key == *key).map(|l| &l.kind) {
                    action = finding_action(report, *rule, *index).or(action);
                }
            }
            CollectionEvent::ContextMenu(request) => {
                if let Some(LineKind::Rule { rule, .. }) = lines.0.iter().find(|l| l.key == request.key).map(|l| &l.kind) {
                    *menu = Some(*rule);
                }
            }
            _ => {}
        }
    }
    if let Some(rule) = *menu {
        let status = report.result(rule).map(|r| r.status);
        let entries = [
            Entry::new(String::from("fix"), tl!("Fix")).enabled(fixable(rule) && status == Some(Status::Failed)),
            Entry::new(String::from("skip"), tl!("Skip Rule")).enabled(status != Some(Status::Skipped)),
            Entry::new(String::from("explain"), tl!("Explain")),
            Entry::separator(),
            Entry::new(String::from("again"), tl!("Check Again")),
            Entry::new(String::from("report"), tl!("Show Report")),
            Entry::new(String::from("options"), tl!("Options…")),
        ];
        let shown = Menu::context(tl!("Accessibility results"), &out.response).pick(ui, &entries);
        match shown.picked.as_deref() {
            Some("fix") => action = Some(PanelAction::Fix(rule)),
            Some("skip") => action = Some(PanelAction::Skip(rule)),
            Some("explain") => *explain = if *explain == Some(rule) { None } else { Some(rule) },
            Some("again") => action = Some(PanelAction::CheckAgain),
            Some("report") => action = Some(PanelAction::ShowReport),
            Some("options") => action = Some(PanelAction::Options),
            _ => {}
        }
        if shown.closed.is_some() {
            *menu = None;
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
        self.set_right(Some(RightPanel::Accessibility));
        self.notify(if failed == 0 {
            tl!("No untagged content was found.").to_string()
        } else {
            crate::i18n::fmt(tl!("Found {n} untagged content problems."), &[("n", &failed.to_string())])
        });
    }

    /// Check for accessibility: the options first, unless they are turned off.
    // PLAN: takes the context the options dialog needs; pdf-hub-w3 passes it from `commands.rs`.
    pub(crate) fn start_accessibility_check(&mut self, ctx: &egui::Context) {
        if self.a11y_options.show_dialog {
            open_a11y_options(self, ctx);
        } else {
            self.run_accessibility_check();
        }
    }

    /// Run the full check with the current options and show the results.
    pub fn run_accessibility_check(&mut self) {
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
        self.set_right(Some(RightPanel::Accessibility));
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
            self.run_accessibility_check();
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
            view.session.flash = Some((page, bbox.map(|v| v as f32), 0.0));
        }
    }

    // PLAN: takes `ctx` for the dialogs it opens; pdf-chrome-w13 passes `ui.ctx()` from `panels/right.rs`.
    pub(crate) fn a11y_action(&mut self, ctx: &egui::Context, index: usize, action: PanelAction) {
        match action {
            PanelAction::CheckAgain => self.run_accessibility_check(),
            PanelAction::Options => open_a11y_options(self, ctx),
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
                    self.set_right(Some(RightPanel::Tags));
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
                    Rule::PrimaryLanguage => self.open_properties(ctx, PropsTab::Advanced),
                    _ => {
                        let edit = self.session.get(id).map(|d| d.accessibility_fix(rule, None));
                        match edit {
                            Some(Ok(e)) => {
                                if !self.apply_edit(ctx, e) {
                                    return;
                                }
                                self.run_accessibility_check();
                                // The title can then be edited.
                                if rule == Rule::Title {
                                    self.open_properties(ctx, PropsTab::Description);
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

/// Where a figure sits on its page, in the page's view space, for its preview.
struct FigureShot {
    page: usize,
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
}

fn figure_shot(doc: &pdfcraft_engine::Document, f: &Figure) -> Option<FigureShot> {
    let (page, b) = (f.page?, f.bbox?);
    let info = doc.info.pages.get(page)?;
    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
    let (x0, y0, x1, y1) = (u[0].min(v[0]), u[1].min(v[1]), u[0].max(v[0]), u[1].max(v[1]));
    Some(FigureShot { page, x0, y0, w: (x1 - x0).max(1.0), h: (y1 - y0).max(1.0) })
}

/// Render a figure's area for the dialog, at the preview's device resolution; a blank picture when it can't be drawn.
fn figure_image(bytes: &Arc<Vec<u8>>, password: Option<&str>, shot: Option<&FigureShot>, ppp: f32) -> egui::ColorImage {
    let blank = || egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT);
    let Some(FigureShot { page, x0, y0, w, h }) = shot else { return blank() };
    let ppp = ppp.max(1.0);
    let scale = ((380.0 * ppp) / w).min((220.0 * ppp) / h).clamp(0.05, 32.0);
    let tile = pdfcraft_render::Tile {
        x: (x0 * scale).floor().max(0.0) as u32,
        y: (y0 * scale).floor().max(0.0) as u32,
        w: (w * scale).ceil().max(1.0) as u32,
        h: (h * scale).ceil().max(1.0) as u32,
    };
<<<<<<< HEAD
    // The picture is only shown in the dialog, so it draws like the viewer (installed CJK faces
    // for fonts the PDF doesn't embed); nothing is saved from it (AGENTS.md §1.4).
    let config = pdfcraft_render::RenderConfig { password: password.map(Arc::from), ..Default::default() }.on_screen();
    let mut r = pdfcraft_render::PageRenderer::new(bytes.clone(), config);
=======
    // The picture is only shown in the panel, so it draws like the viewer (installed CJK faces
    // for fonts the PDF doesn't embed); nothing is saved from it (docs/development.md "Fonts and attribution").
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() }.on_screen();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
>>>>>>> upstream/main
    let out = r.render(pdfcraft_render::RenderRequest {
        page: *page,
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: Some(tile),
        scale,
        tag: 0,
        preview: false,
    });
    if out.error.is_some() || out.width == 0 {
        return blank();
    }
    egui::ColorImage::from_rgba_premultiplied([out.width as usize, out.height as usize], &out.rgba)
}

/// The figure the Set Alternate Text dialog is on and what has been typed for each figure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AltTextValue {
    index: usize,
    texts: Vec<String>,
    decorative: Vec<bool>,
}

/// What Save & Close answers: the figures as they were and what was set for them.
pub struct AltTextAnswer {
    figures: Vec<Figure>,
    value: AltTextValue,
}

/// Add alternate text: the figures one by one.
pub struct AltTextDialog {
    id: DialogId,
    figures: Vec<Figure>,
    shots: Vec<Option<FigureShot>>,
    /// The document's bytes and password, which draw the previews.
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    /// The document and its edit generation, which keep a figure's preview apart from another document's.
    doc: DocId,
    generation: u64,
    value: AltTextValue,
    open: bool,
}

impl StackDialog for AltTextDialog {
    type Result = AltTextAnswer;

    fn id(&self) -> DialogId {
        self.id
    }

    fn key(&self) -> Option<&str> {
        Some("a11y.alt_text")
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<AltTextAnswer>> {
        let AltTextDialog { figures, shots, bytes, password, doc, generation, value, open, .. } = self;
        let ppp = ui.ctx().pixels_per_point();
        let n = figures.len();
        let shown = FormDialog::new(tl!("Set Alternate Text"), Bind::bound(&mut *value))
            .opened(Bind::bound(&mut *open))
            .ok_label(tl!("Save & Close"))
            .show(ui, |ui, _, value| {
                let i = value.index.min(n.saturating_sub(1));
                value.index = i;
                let (Some(figure), Some(text), Some(decorative)) = (figures.get(i), value.texts.get_mut(i), value.decorative.get_mut(i)) else {
                    return true;
                };
                Stack::new().show(ui, |ui, _| {
                    let page = figure.page.map(|p| crate::i18n::fmt(tl!(" on page {p}"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
                    Text::new(crate::i18n::fmt(
                        tl!("Figure {i} of {n}{page}"),
                        &[("i", &(i + 1).to_string()), ("n", &n.to_string()), ("page", &page)],
                    ))
                    .shade(Shade::Muted)
                    .show(ui);
                    // The picture renders once per key: the dialog keeps no image of its own.
                    let key = {
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        (*doc, *generation, figure.obj.num).hash(&mut hasher);
                        hasher.finish()
                    };
                    let shot = shots.get(i).and_then(Option::as_ref);
                    let render = || figure_image(bytes, password.as_deref(), shot, ppp);
                    Picture::new(tl!("Figure"), key, &render).aspect(ThumbAspect::new(19, 11)).extent(PictureSize::Sample(ThumbSize::Large)).show(ui);
                    let mut alt = AltText::new(text.clone()).decorative(*decorative);
                    AltTextField::new(tl!("Alternate text"), Bind::bound(&mut alt)).show(ui);
                    if let Some(typed) = alt.text.same() {
                        *text = typed.clone();
                    }
                    if let Some(on) = alt.decorative.same() {
                        *decorative = *on;
                    }
                    let back = IconButton::new(IconSpec::svg(IconId::new("chevron-left"))).label(tl!("Previous figure")).enabled(i > 0).show(ui);
                    let next = IconButton::new(IconSpec::svg(IconId::new("chevron-right"))).label(tl!("Next figure")).enabled(i + 1 < n).show(ui);
                    if back.response().clicked() {
                        value.index = i.saturating_sub(1);
                    }
                    if next.response().clicked() {
                        value.index = (i + 1).min(n.saturating_sub(1));
                    }
                });
                true
            });
        match (shown.accepted, shown.closed) {
            (Some(value), _) => Some(DialogResult::Ok(AltTextAnswer { figures: figures.clone(), value })),
            (None, Some(_)) => Some(DialogResult::Cancel),
            (None, None) => None,
        }
    }
}

/// Opens Set Alternate Text over the figures of the active document, or says it has none.
pub(crate) fn open_alt_text(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((_, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let figures = doc.figures();
    if figures.is_empty() {
        app.dialogs.open(HostedAlert::new(Alert::new(tl!("Set Alternate Text"), tl!("This document has no tagged figures.")).ok_label(tl!("Close"))));
        return;
    }
    let dialog = AltTextDialog {
        id: dialog_id("a11y.alt_text"),
        shots: figures.iter().map(|f| figure_shot(doc, f)).collect(),
        bytes: doc.bytes.clone(),
        password: doc.password.clone(),
        doc: id,
        generation: doc.edit_generation(),
        value: AltTextValue {
            index: 0,
            texts: figures.iter().map(|f| f.alt.clone().unwrap_or_default()).collect(),
            decorative: vec![false; figures.len()],
        },
        figures,
        open: true,
    };
    // A second request while the dialog is open leaves it as it is.
    let _ = app.dialogs.open_unless_open_in(ctx, dialog);
}

impl PdfCraftApp {
    /// Prepare for accessibility ▸ Tag form fields.
    pub(crate) fn autotag_form_fields(&mut self, ctx: &egui::Context) {
        self.apply_edit(ctx, pdfcraft_engine::Edit::AutoTagFormFields);
        self.choose_right_panel(Some(RightPanel::Tags));
    }

    /// Save & Close: one undo step for every change.
    pub(crate) fn save_alt_text(&mut self, ctx: &egui::Context, answer: AltTextAnswer) {
        let AltTextAnswer { figures, value: d } = answer;
        let mut edits = Vec::new();
        for (k, f) in figures.iter().enumerate() {
            let (Some(text), Some(decorative)) = (d.texts.get(k), d.decorative.get(k)) else { continue };
            if *decorative {
                edits.push(pdfcraft_engine::Edit::MarkDecorative { figure: f.obj.num });
            } else if text.trim() != f.alt.as_deref().unwrap_or("").trim() {
                edits.push(pdfcraft_engine::Edit::SetAltText { figure: f.obj.num, alt: Some(text.clone()) });
            }
        }
        if !edits.is_empty() {
            self.apply_edit(ctx, pdfcraft_engine::Edit::Batch { label: "Set alternate text".into(), edits });
        }
    }
}
