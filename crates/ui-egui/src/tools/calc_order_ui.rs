//! Prepare a form ▸ Calculation order, a page of the left panel, and what it shows on the page.
//!
//! The panel: a notice that says whether the order is right and what Fix order does, a notice for
//! each loop, a notice for the formulas that read fields the form doesn't have, and the calculated
//! fields in the order they calculate, each with its formula, the value it shows and what is wrong
//! with it. A field moves by dragging, with the arrows of the selected field (Acrobat's Set Field
//! Calculation Order has Up and Down), or with Alt+↑/↓. Every change is one undoable edit.
//!
//! The page: each calculated field's step in the order, and for the field in focus (under the
//! pointer in the list or on the page, else the selected one) arrows from the fields it reads and
//! to the fields that read it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use craft::behavior::item_source::keyed;
use craft::behavior::kurbo::{CubicBez, ParamCurve as _, Point};
use craft::behavior::selection::Selection;
use craft::behavior::{EditEvent, IconId, Input, Metrics, ReorderKeys, SelectionChange};
use craft::theme::variant::Tone as Status;
use craft::ui::buttons::IconButton;
use craft::ui::choice::Checkbox;
use craft::ui::contract::{
    Access, Bind, CollectionEvent, Common, Component, Cx, MeasureCx, Outcome as _, PaintUi, Props as _, ReorderRule, Role, SelectMode, Show as _,
    ShowCollection as _, ShowContainer as _,
};
use craft::ui::display::{Badge, Shade, Text};
use craft::ui::layout_components::Stack;
use craft::ui::list::{List, ListRow, RowEvent, TrailingFace, TrailingPart};
use craft::ui::notices::{Notice, NoticeAction};
use craft::ui::primitives::{HAlign, IconSpec, PaintShape, StrokeWidth, TextRun};
use craft::ui::style::{ComponentStyle, StyleColor, StyleCx};
use egui::{CornerRadius, Pos2, Rect, Response, Vec2, pos2, vec2};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::form_scripts::{CalcOp, Calculate, format_value};
use pdfcraft_engine::{CalculatedField, CalculationIssue, CalculationOrder, Edit, FormField, FormFieldKind};
use pdfcraft_render::DocInfo;

use crate::canvas::PageXform;
use crate::{LeftPanel, PdfCraftApp};

/// The command the panel's Back button runs: it returns to Prepare a form.
const BACK: &str = "form.calculation_order.back";
/// Links kept at most: a formula reading a group reads every field under it.
const MAX_LINKS: usize = 20_000;
/// Names a sentence lists before it stops with "…".
const MAX_NAMES: usize = 6;
/// Loop notices shown at most.
const MAX_LOOPS: usize = 5;
/// Fields a notice names one by one: too early, or reading fields the form doesn't have.
const MAX_LISTED: usize = 3;
/// The arrows of a row, in its trailing parts.
const UP: TrailingPart = TrailingPart::new(0);
const DOWN: TrailingPart = TrailingPart::new(1);

/// How a field's formula and place in the order, or a link between two fields, stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Ok,
    /// Its formula reads a field the form doesn't have.
    Missing,
    /// It calculates before a field it reads, so it uses that field's old value.
    Late,
    /// Its formula reads its own result through other formulas.
    Loop,
}

/// A view's calculation order, read again only after an edit (filling a field in is one), and
/// what the pointer is over.
#[derive(Clone, Debug, Default)]
pub struct CalcOrderView {
    cache: Option<(u64, Arc<Listed>)>,
    /// The field whose row is under the pointer, this pass. The panel is shown before the page, so
    /// the page's overlay reads it in the same pass.
    hover_row: Option<Focus>,
    /// Links on page is off.
    hide_links: bool,
    /// The selected field the list last scrolled to, so a new selection (on the page too) brings
    /// its row into view once.
    scrolled_to: Option<String>,
}

/// The calculation order with what the panel and the page show of it.
#[derive(Debug)]
struct Listed {
    order: CalculationOrder,
    /// One per field of the order, in its order.
    rows: Vec<Row>,
    /// From each field a formula reads to the calculated field that reads it.
    links: Vec<Link>,
}

#[derive(Debug)]
struct Row {
    /// The value as the field shows it (its format applied), on one line.
    value: String,
    /// The formula on one line: the Calculate tab's, or a script's first line.
    formula: String,
    script: bool,
    tone: Tone,
}

#[derive(Clone, Debug, PartialEq)]
struct Link {
    from: String,
    to: String,
    /// The calculated field `to` names, whose formula reads `from`.
    reader: ObjRef,
    tone: Tone,
}

/// The field in focus: its name, and its object when the focus is one field (its row, or its
/// widget on the page). The Prepare selection has only a name, so it stands for every field of
/// that name: two fields can share a name in a malformed form.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Focus {
    name: String,
    obj: Option<ObjRef>,
}

impl Focus {
    fn named(name: &str) -> Self {
        Self { name: name.to_owned(), obj: None }
    }

    /// A calculated field's row.
    fn row(f: &CalculatedField) -> Self {
        Self { name: f.name.clone(), obj: Some(f.obj) }
    }

    /// A form field's widget on the page, as one field.
    pub(crate) fn of_field(f: &FormField) -> Self {
        Self { name: f.name.clone(), obj: Some(f.obj) }
    }

    /// Whether field `name`, object `obj`, is in focus.
    fn is(&self, name: &str, obj: ObjRef) -> bool {
        self.name == name && self.obj.is_none_or(|o| o == obj)
    }

    /// `l` brings a value into the focus: its formula reads `l.from`.
    fn reads(&self, l: &Link) -> bool {
        self.is(&l.to, l.reader)
    }

    /// `l` takes the focus's value to a formula. A formula reads by name, so it reads every field
    /// of the focus's name alike.
    fn read_by(&self, l: &Link) -> bool {
        l.from == self.name
    }
}

impl CalcOrderView {
    fn listed(&mut self, doc: &pdfcraft_engine::Document) -> Arc<Listed> {
        let generation = doc.edit_generation();
        if let Some(listed) = self.current(generation) {
            return listed;
        }
        let order = doc.calculation_order();
        let rows = rows(&order.fields, &doc.form);
        let names: Vec<&str> = doc.form.iter().map(|f| f.name.as_str()).collect();
        let links = links(&order.fields, names);
        let listed = Arc::new(Listed { order, rows, links });
        self.cache = Some((generation, listed.clone()));
        listed
    }

    /// The order read at `generation`, if it was.
    fn current(&self, generation: u64) -> Option<Arc<Listed>> {
        self.cache.as_ref().filter(|(g, _)| *g == generation).map(|(_, listed)| listed.clone())
    }

    /// The field whose links show: under the pointer in the list or on the page (`page`), else selected.
    fn focus(&self, page: Option<&Focus>, selected: Option<&str>) -> Option<Focus> {
        self.hover_row.clone().or_else(|| page.cloned()).or_else(|| selected.map(Focus::named))
    }
}

fn tone(issues: &[CalculationIssue]) -> Tone {
    if issues.iter().any(|i| matches!(i, CalculationIssue::MissingField { .. })) {
        Tone::Missing
    } else if issues.iter().any(|i| matches!(i, CalculationIssue::Cycle { .. })) {
        Tone::Loop
    } else if issues.iter().any(|i| matches!(i, CalculationIssue::RunsTooEarly { .. })) {
        Tone::Late
    } else {
        Tone::Ok
    }
}

/// `text` on one line, cut at 200 characters.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

/// Each calculated field's value, formula and standing, in the order's order. Fields are found by
/// their object, not their name: two calculated fields can share a name in a malformed form.
fn rows(calculated: &[CalculatedField], form: &[FormField]) -> Vec<Row> {
    let by_obj: HashMap<_, &FormField> = form.iter().map(|f| (f.obj, f)).collect();
    calculated
        .iter()
        .map(|c| {
            let field = by_obj.get(&c.obj);
            let value = match field {
                Some(f) if f.kind == FormFieldKind::Text => f.value.first().map(|v| format_value(&f.actions.format, v)).unwrap_or_default(),
                Some(f) => f.display_value(),
                None => String::new(),
            };
            let (formula, script) = match field.map(|f| (&f.actions.calculate, f.actions.scripts.calculate.as_deref())) {
                Some((Calculate::Simple { op, fields }, _)) => {
                    let op = match op {
                        CalcOp::Sum => "sum",
                        CalcOp::Product => "product",
                        CalcOp::Average => "average",
                        CalcOp::Minimum => "min",
                        CalcOp::Maximum => "max",
                    };
                    (format!("= {op}({})", fields.join(", ")), false)
                }
                Some((Calculate::Notation(n), _)) => (format!("= {}", one_line(n)), false),
                Some((Calculate::None, Some(js))) => (js.lines().map(str::trim).find(|l| !l.is_empty()).map(one_line).unwrap_or_default(), true),
                _ => (String::new(), false),
            };
            Row { value: one_line(&value), formula, script, tone: tone(&c.issues) }
        })
        .collect()
}

/// The links between the form's fields (`names`): a formula's read name finds the field of that
/// name and the fields under it, as the calculation looks values up. At most [`MAX_LINKS`].
fn links(calculated: &[CalculatedField], mut names: Vec<&str>) -> Vec<Link> {
    names.sort_unstable();
    names.dedup();
    let mut out = Vec::new();
    for c in calculated {
        let mut seen = HashSet::new();
        for read in &c.reads {
            let prefix = format!("{read}.");
            let exact = names.partition_point(|n| *n < read.as_str());
            let under = names.partition_point(|n| *n < prefix.as_str());
            let same = names.get(exact..).unwrap_or_default().iter().take_while(|n| **n == read.as_str());
            let below = names.get(under..).unwrap_or_default().iter().take_while(|n| n.starts_with(&prefix));
            for &from in same.chain(below) {
                if out.len() >= MAX_LINKS {
                    return out;
                }
                if !seen.insert(from) {
                    continue;
                }
                let tone = if c.issues.iter().any(|i| matches!(i, CalculationIssue::Cycle { fields } if fields.iter().any(|x| x == from))) {
                    Tone::Loop
                } else if c.issues.iter().any(|i| matches!(i, CalculationIssue::RunsTooEarly { field } if field == from)) {
                    Tone::Late
                } else {
                    Tone::Ok
                };
                out.push(Link { from: from.to_owned(), to: c.name.clone(), reader: c.obj, tone });
            }
        }
    }
    out
}

/// The loops: the fields that read each other, each loop in the order's order.
fn loops(calculated: &[CalculatedField]) -> Vec<Vec<&str>> {
    let index: HashMap<&str, usize> = calculated.iter().enumerate().map(|(k, f)| (f.name.as_str(), k)).collect();
    // Each field points at an earlier field of its loop, and a loop's first field at itself.
    let mut first: Vec<usize> = (0..calculated.len()).collect();
    let root = |first: &[usize], mut k: usize| {
        // Every step goes to an earlier field, so there are fewer steps than fields.
        for _ in 0..first.len() {
            match first.get(k) {
                Some(&p) if p != k => k = p,
                _ => break,
            }
        }
        k
    };
    for (k, f) in calculated.iter().enumerate() {
        for issue in &f.issues {
            if let CalculationIssue::Cycle { fields } = issue {
                for j in fields.iter().filter_map(|x| index.get(x.as_str()).copied()) {
                    let (a, b) = (root(&first, k), root(&first, j));
                    if let Some(p) = first.get_mut(a.max(b)) {
                        *p = a.min(b);
                    }
                }
            }
        }
    }
    let mut groups: Vec<(usize, Vec<&str>)> = Vec::new();
    for (k, f) in calculated.iter().enumerate().filter(|(_, f)| f.issues.iter().any(|i| matches!(i, CalculationIssue::Cycle { .. }))) {
        let r = root(&first, k);
        match groups.iter_mut().find(|(g, _)| *g == r) {
            Some((_, names)) => names.push(&f.name),
            None => groups.push((r, vec![&f.name])),
        }
    }
    groups.into_iter().map(|(_, names)| names).collect()
}

/// Names in a sentence: "a, b and c", cut with "…" after [`MAX_NAMES`].
fn names(list: &[&str]) -> String {
    match list {
        [] => String::new(),
        [one] => (*one).to_owned(),
        _ if list.len() > MAX_NAMES => format!("{}, …", list.iter().take(MAX_NAMES).copied().collect::<Vec<_>>().join(", ")),
        [rest @ .., last] => crate::i18n::fmt(tl!("{names} and {last}"), &[("names", &rest.join(", ")), ("last", *last)]),
    }
}

/// What the focused field reads and what reads it: "total reads sub and tax · No field reads it".
/// If the focus calculates (its fields in the order's `calculated`), the fields it reads that the
/// form doesn't have, and names its script builds as it runs, are said too.
fn trace(links: &[Link], focus: &Focus, calculated: &[CalculatedField]) -> String {
    let lang = crate::i18n::current();
    let mut reads: Vec<&str> = Vec::new();
    let mut read_by: Vec<&str> = Vec::new();
    for l in links {
        if focus.reads(l) && !reads.contains(&l.from.as_str()) {
            reads.push(&l.from);
        }
        if focus.read_by(l) && !read_by.contains(&l.to.as_str()) {
            read_by.push(&l.to);
        }
    }
    let (mut missing, mut said): (Vec<&str>, HashSet<&str>) = (Vec::new(), HashSet::new());
    let mut built = false;
    for issue in calculated.iter().filter(|c| focus.is(&c.name, c.obj)).flat_map(|c| &c.issues) {
        match issue {
            CalculationIssue::MissingField { field } if said.insert(field) => missing.push(field),
            CalculationIssue::BuiltNames => built = true,
            _ => {}
        }
    }
    reads.extend(&missing);
    let focus = focus.name.as_str();
    let mut parts = vec![if !reads.is_empty() {
        crate::i18n::fmt(tl!("{field} reads {fields}"), &[("field", focus), ("fields", &names(&reads))])
    } else if built {
        crate::i18n::fmt(tl!("{field} reads fields named as it runs"), &[("field", focus)])
    } else {
        crate::i18n::fmt(tl!("{field} reads no other field"), &[("field", focus)])
    }];
    if !missing.is_empty() {
        let template = crate::i18n::trn(lang, missing.len() as u64, "{fields} is missing", "{fields} are missing");
        parts.push(crate::i18n::fmt(&template, &[("fields", &names(&missing))]));
    }
    if built && !reads.is_empty() {
        parts.push(tl!("Some names are built as it runs").to_owned());
    }
    parts.push(if read_by.is_empty() {
        tl!("No field reads it").to_owned()
    } else {
        let template = crate::i18n::trn(lang, read_by.len() as u64, "{fields} reads it", "{fields} read it");
        crate::i18n::fmt(&template, &[("fields", &names(&read_by))])
    });
    parts.join(" · ")
}

/// What the page asks for, done once it is drawn.
enum Act {
    Order(Vec<String>),
    Undo,
    EditFormula(String),
    Select(String),
}

/// A notice of `status` that says `heading` and `lines`, with one action per entry of `actions`. Returns
/// the act of the action taken.
fn notice(ui: &mut egui::Ui, status: Status, heading: String, lines: Vec<String>, actions: Vec<(String, Act)>) -> Option<Act> {
    let mut n = Notice::new(status, heading).lines(lines);
    for (label, _) in &actions {
        n = n.action(NoticeAction::new(label.clone()));
    }
    let taken = n.show(ui).action?;
    actions.into_iter().nth(taken).map(|(_, act)| act)
}

/// The notices above the list: what is wrong and Fix order, or that the order is right; and loops.
fn cards(ui: &mut egui::Ui, order: &CalculationOrder, editable: bool, undo: bool) -> Option<Act> {
    let lang = crate::i18n::current();
    let mut act = None;
    // Each field that calculates too early, with the fields it reads that calculate later.
    let late: Vec<(&str, Vec<&str>)> = order
        .fields
        .iter()
        .map(|f| {
            let later = f.issues.iter().filter_map(|i| if let CalculationIssue::RunsTooEarly { field } = i { Some(field.as_str()) } else { None });
            (f.name.as_str(), later.collect::<Vec<_>>())
        })
        .filter(|(_, later)| !later.is_empty())
        .collect();
    let unlisted: Vec<&str> = order.fields.iter().filter(|f| !f.listed).map(|f| f.name.as_str()).collect();
    let loops = loops(&order.fields);
    // Each formula that reads fields the form doesn't have, with those fields.
    let missing: Vec<(&str, Vec<&str>)> = order
        .fields
        .iter()
        .map(|f| {
            let gone = f.issues.iter().filter_map(|i| if let CalculationIssue::MissingField { field } = i { Some(field.as_str()) } else { None });
            (f.name.as_str(), gone.collect::<Vec<_>>())
        })
        .filter(|(_, gone)| !gone.is_empty())
        .collect();
    let undo_action = || (tl!("Undo").to_owned(), Act::Undo);
    if !missing.is_empty() {
        let heading = crate::i18n::trn(
            lang,
            missing.len() as u64,
            "{n} formula reads a field the form doesn't have",
            "{n} formulas read fields the form doesn't have",
        );
        let mut lines = Vec::new();
        for (field, gone) in missing.iter().take(MAX_LISTED) {
            let template = crate::i18n::trn(
                lang,
                gone.len() as u64,
                "{field} reads {fields}, which the form doesn't have.",
                "{field} reads {fields}, which the form doesn't have.",
            );
            lines.push(crate::i18n::fmt(&template, &[("field", field), ("fields", &names(gone))]));
        }
        if missing.len() > MAX_LISTED {
            let more = (missing.len() - MAX_LISTED) as u64;
            lines.push(crate::i18n::trn(lang, more, "{n} more formula reads a missing field.", "{n} more formulas read missing fields."));
        }
        let mut actions = Vec::new();
        if editable && let Some((first, _)) = missing.first() {
            actions.push((crate::i18n::fmt(tl!("Edit {field}'s formula"), &[("field", first)]), Act::EditFormula((*first).to_owned())));
        }
        // The all-clear, which carries Undo after a reorder, stays hidden while a formula is
        // broken, so Undo is offered here.
        if undo {
            actions.push(undo_action());
        }
        act = notice(ui, Status::Bad, heading, lines, actions).or(act);
    }
    if order.fixable() {
        let heading = if late.is_empty() {
            tl!("The order saved in the PDF needs tidying").to_owned()
        } else {
            crate::i18n::trn(lang, late.len() as u64, "{n} field shows an old value", "{n} fields show old values")
        };
        let mut lines = Vec::new();
        for (field, later) in late.iter().take(MAX_LISTED) {
            let template = crate::i18n::trn(
                lang,
                later.len() as u64,
                "{field} calculates before {fields}, which it reads.",
                "{field} calculates before {fields}, which it reads.",
            );
            lines.push(crate::i18n::fmt(&template, &[("field", field), ("fields", &names(later))]));
        }
        if late.len() > MAX_LISTED {
            let more = (late.len() - MAX_LISTED) as u64;
            lines.push(crate::i18n::trn(lang, more, "{n} more field calculates too early.", "{n} more fields calculate too early."));
        }
        if !unlisted.is_empty() {
            let template = crate::i18n::trn(
                lang,
                unlisted.len() as u64,
                "{fields} is missing from the order saved in the PDF, so other PDF viewers may not calculate it.",
                "{fields} are missing from the order saved in the PDF, so other PDF viewers may not calculate them.",
            );
            lines.push(crate::i18n::fmt(&template, &[("fields", &names(&unlisted))]));
        }
        if !order.extra.is_empty() {
            lines.push(crate::i18n::trn(
                lang,
                order.extra.len() as u64,
                "The calculation order has {n} entry that calculates nothing. Fix order removes it.",
                "The calculation order has {n} entries that calculate nothing. Fix order removes them.",
            ));
        }
        lines.push(tl!("Move only what must move, so every field calculates after the fields it reads.").to_owned());
        let actions = if editable { vec![(tl!("Fix order").to_owned(), Act::Order(order.fixed()))] } else { Vec::new() };
        act = notice(ui, Status::Warn, heading, lines, actions).or(act);
    } else if missing.is_empty() {
        let said = if loops.is_empty() {
            tl!("Every field calculates after the fields it reads.")
        } else {
            tl!("Every field outside a loop calculates after the fields it reads.")
        };
        let actions = if undo { vec![undo_action()] } else { Vec::new() };
        act = notice(ui, Status::Good, said.to_owned(), Vec::new(), actions).or(act);
    }
    for members in loops.iter().take(MAX_LOOPS) {
        let heading = match members.as_slice() {
            [one] => crate::i18n::fmt(tl!("{field} reads its own value"), &[("field", *one)]),
            _ => crate::i18n::fmt(tl!("{fields} read each other"), &[("fields", &names(members))]),
        };
        let mut actions = Vec::new();
        if editable && let Some(first) = members.first() {
            actions.push((crate::i18n::fmt(tl!("Edit {field}'s formula"), &[("field", first)]), Act::EditFormula((*first).to_owned())));
        }
        let lines = vec![tl!("No order can fix a loop. Change one of the formulas.").to_owned()];
        act = notice(ui, Status::Bad, heading, lines, actions).or(act);
    }
    let built: Vec<&str> = order.fields.iter().filter(|f| f.issues.contains(&CalculationIssue::BuiltNames)).map(|f| f.name.as_str()).collect();
    if !built.is_empty() {
        let template = crate::i18n::trn(
            lang,
            built.len() as u64,
            "{fields} builds some field names as it runs, so those reads aren't checked.",
            "{fields} build some field names as they run, so those reads aren't checked.",
        );
        act = notice(ui, Status::Neutral, crate::i18n::fmt(&template, &[("fields", &names(&built))]), Vec::new(), Vec::new()).or(act);
    }
    act
}

/// Why a row is marked, for its tooltip.
fn explained(issues: &[CalculationIssue]) -> String {
    issues
        .iter()
        .map(|i| match i {
            CalculationIssue::MissingField { field } => {
                crate::i18n::fmt(tl!("Reads {name}, which the form doesn't have, so it gets no value there."), &[("name", field)])
            }
            CalculationIssue::RunsTooEarly { field } => {
                crate::i18n::fmt(tl!("Reads {name}, which calculates later, so it shows an old value."), &[("name", field)])
            }
            CalculationIssue::Cycle { fields } => crate::i18n::fmt(
                tl!("Reads {names}, which read it back. No order can fix a loop: change one of the formulas."),
                &[("names", &fields.join(", "))],
            ),
            CalculationIssue::NotInOrder => {
                tl!("Missing from the calculation order saved in the PDF: other PDF viewers may not calculate it.").to_owned()
            }
            CalculationIssue::BuiltNames => {
                tl!("Its script also reads fields whose names it builds as it runs, so those reads can't be checked.").to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The order after the rows `keys` (in their order) move to position `to` of what remains once
/// they are taken out: the move a list's `ReorderMove` describes, whether a drag, Alt+↑/↓ or an arrow.
fn reordered(order: &[ObjRef], keys: &[ObjRef], to: usize) -> Vec<ObjRef> {
    let mut rest: Vec<ObjRef> = order.iter().copied().filter(|o| !keys.contains(o)).collect();
    let at = to.min(rest.len());
    rest.splice(at..at, keys.iter().copied());
    rest
}

/// The row of one calculated field: its step, name and value, then its formula and what is wrong.
fn order_row<'a>(k: usize, count: usize, f: &CalculatedField, row: &Row, editable: bool) -> ListRow<'a> {
    let label = if row.value.is_empty() { f.name.clone() } else { format!("{}: {}", f.name, row.value) };
    let caption = match row.tone {
        Tone::Missing => Some((tl!("Missing field"), Status::Bad)),
        Tone::Loop => Some((tl!("Loop"), Status::Bad)),
        Tone::Late => Some((tl!("Too early"), Status::Warn)),
        Tone::Ok if !f.listed => Some((tl!("Not in order"), Status::Warn)),
        Tone::Ok if f.issues.contains(&CalculationIssue::BuiltNames) => Some((tl!("Unchecked reads"), Status::Neutral)),
        Tone::Ok => None,
    };
    let why = [row.formula.clone(), explained(&f.issues)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
    let mut r = ListRow::new(label).lead_text((k + 1).to_string(), count.to_string());
    if !why.is_empty() {
        r = r.tooltip(why);
    }
    if !row.formula.is_empty() {
        r = r.below(Text::new(row.formula.clone()).shade(Shade::Muted), |_| {});
    }
    if let Some((text, status)) = caption {
        r = r.below(Text::new(text).tone(status), |_| {});
    }
    // Rows without a script keep the badge's room, so the arrows stay in line.
    r = if row.script { r.trailing_field(Badge::new(tl!("Script")), |_| {}) } else { r.trailing_space(Badge::new(tl!("Script"))) };
    if editable {
        // Each arrow names the field it moves.
        let named = |tip: &str| format!("{}: {}", tl!(tip), f.name);
        if k > 0 {
            r = r.trailing_part_revealed(UP, TrailingFace::Icon(IconSpec::svg(IconId::new("chevron-up"))), named("Earlier in calculation order"));
        }
        if k + 1 < count {
            r = r.trailing_part_revealed(DOWN, TrailingFace::Icon(IconSpec::svg(IconId::new("chevron-down"))), named("Later in calculation order"));
        }
    }
    r
}

/// The panel's title row: Back to Prepare a form. Returns the command to run when it was pressed.
pub(crate) fn header(ui: &mut egui::Ui) -> Option<&'static str> {
    // A command-bound button fires its command itself; the return also lets the caller run it.
    let button = IconButton::command(IconSpec::svg(IconId::new("chevron-left")), BACK).show(ui);
    button.response().clicked().then_some(BACK)
}

/// The Calculation order page's body: the notices, the order and the links switch.
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some((index, id)) = app.active_ids() else {
        Text::new(tl!("Open a document first")).shade(Shade::Muted).show(ui);
        return;
    };
    let Some(doc) = app.session.get(id) else { return };
    let Some(view) = app.views.get_mut(index) else { return };
    let listed = view.session.calc_order.listed(doc);
    let order = &listed.order;
    if order.fields.is_empty() && order.extra.is_empty() {
        Text::new(tl!(
            "No field in this form calculates its value yet. Give a field a calculation on the Calculate tab of its properties, and it shows here."
        ))
        .shade(Shade::Muted)
        .show(ui);
        return;
    }
    let editable = doc.allows_modification();
    let undo = doc.can_undo() == Some("Set calculation order");
    let selected = view.session.prepare.selected.as_ref().map(|(name, _)| name.clone());
    let selected_objs: Vec<ObjRef> =
        selected.as_deref().map(|s| order.fields.iter().filter(|f| f.name == s).map(|f| f.obj).collect()).unwrap_or_default();
    let lead = selected_objs.first().copied();
    let mut selection = Selection::new(selected_objs, lead, lead);
    // A field selected since the last pass, whose row the list shows once.
    let reveal = lead.filter(|_| selected != view.session.calc_order.scrolled_to);
    view.session.calc_order.scrolled_to = selected.clone();
    let mut links_on = !view.session.calc_order.hide_links;
    let objs: Vec<ObjRef> = order.fields.iter().map(|f| f.obj).collect();
    let count = objs.len();
    let mut hovered = None;
    let mut act = None;

    Stack::new().show(ui, |ui, _| {
        act = cards(ui, order, editable, undo);
        Text::new(tl!("Order · top calculates first")).shade(Shade::Muted).show(ui);
        let source = keyed(&order.fields, |c| c.obj);
        let mut list = List::new(tl!("Calculation order")).select(SelectMode::Single).selection(Bind::bound(&mut selection));
        if editable {
            list = list.reorder(ReorderRule::new().keys(ReorderKeys::AltArrows));
        }
        if let Some(key) = reveal {
            list = list.reveal(key);
        }
        let shown = list.show_items(ui, &source, &mut |_, icx| match (order.fields.get(icx.at), listed.rows.get(icx.at)) {
            (Some(f), Some(row)) => order_row(icx.at, count, f, row, editable),
            _ => ListRow::new(String::new()),
        });
        hovered = shown.hovered;
        for event in shown.events {
            let moved = match event {
                CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                    let name = keys.last().and_then(|k| order.fields.iter().find(|f| f.obj == *k)).map(|f| f.name.clone());
                    if let Some(name) = name {
                        act = Some(Act::Select(name));
                    }
                    None
                }

                CollectionEvent::Reorder(EditEvent::Set { after, .. } | EditEvent::Commit { after, .. }) => Some((after.keys, after.to)),
                CollectionEvent::Item(key, RowEvent::Trailing { part, .. }) => {
                    let at = objs.iter().position(|o| *o == key);
                    match at {
                        Some(k) if part == UP => k.checked_sub(1).map(|to| (vec![key], to)),
                        Some(k) if part == DOWN => Some((vec![key], k + 1)),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((keys, to)) = moved {
                let next = reordered(&objs, &keys, to);
                let names = next.iter().filter_map(|o| order.fields.iter().find(|f| f.obj == *o)).map(|f| f.name.clone()).collect();
                act = Some(Act::Order(names));
            }
        }
        Text::new(String::new()).show(ui);
        Checkbox::new(Bind::bound(&mut links_on)).label(tl!("Links on page")).tooltip(tl!("Show on the page which fields each field reads")).show(ui);
        if editable {
            let (up, down) = if cfg!(target_os = "macos") { ("⌥↑", "⌥↓") } else { ("Alt+↑", "Alt+↓") };
            let hint = crate::i18n::fmt(tl!("Drag a field to move it, or select it and press {up} or {down}."), &[("up", up), ("down", down)]);
            Text::new(hint).shade(Shade::Faint).show(ui);
        }
    });
    view.session.calc_order.hover_row = hovered.and_then(|key| order.fields.iter().find(|f| f.obj == key)).map(Focus::row);
    view.session.calc_order.hide_links = !links_on;
    match act {
        Some(Act::Order(names)) => {
            app.apply_edit(ui.ctx(), Edit::SetCalculationOrder { names });
        }
        Some(Act::Undo) => {
            app.execute(ui.ctx(), "edit.undo");
        }
        Some(Act::EditFormula(name)) => {
            if let Some(view) = app.views.get_mut(index) {
                view.session.prepare.select((name.clone(), 0), false);
            }
            crate::prepare::dialog::open(app, &name, 0, crate::prepare::FieldTab::Calculate);
        }
        Some(Act::Select(name)) => {
            // Where its first widget is, to show it there.
            let shown = app
                .session
                .get(id)
                .and_then(|doc| doc.form.iter().find(|g| g.name == name).and_then(|g| g.widgets.first()).and_then(|w| Some((w.page?, w.rect))));
            if let Some(view) = app.views.get_mut(index) {
                view.session.prepare.select((name, 0), false);
                if let Some((page, rect)) = shown {
                    view.go_to_page(page);
                    view.session.flash = Some((page, rect.map(|v| v as f32), 0.0));
                }
            }
        }
        None => {}
    }
    let _ = LeftPanel::CalculationOrder;
}

/// The focus's links, and whether a field is at either end of one or is the focus itself: the
/// fields it reads go by name, as a read name stands for every field of that name, and the
/// formulas that read it by object.
fn focus_links<'a>(links: &'a [Link], focus: &'a Focus) -> (Vec<&'a Link>, impl Fn(&str, ObjRef) -> bool + 'a) {
    let shown: Vec<&Link> = links.iter().filter(|l| focus.reads(l) || focus.read_by(l)).collect();
    let names: HashSet<&str> = shown.iter().filter(|l| focus.reads(l)).map(|l| l.from.as_str()).collect();
    let readers: HashSet<ObjRef> = shown.iter().filter(|l| focus.read_by(l)).map(|l| l.reader).collect();
    (shown, move |name: &str, obj: ObjRef| focus.is(name, obj) || names.contains(name) || readers.contains(&obj))
}

/// Whether a field, by name and object, is at an end of the focus's links or is the focus.
type Related<'a> = &'a dyn Fn(&str, ObjRef) -> bool;

/// The steps the page shows: each one's place in the order (from 0), the widget it goes on (its
/// own field's, so fields that share a name each get theirs) and its tone. With a focus, the
/// steps of the fields `related` to it; without one, those whose place is wrong.
fn steps(listed: &Listed, by_obj: &HashMap<ObjRef, Rect>, related: Option<Related<'_>>) -> Vec<(usize, Rect, Tone)> {
    listed
        .order
        .fields
        .iter()
        .zip(&listed.rows)
        .enumerate()
        .filter(|(_, (f, row))| match related {
            Some(related) => related(&f.name, f.obj),
            None => row.tone != Tone::Ok || !f.listed,
        })
        .filter_map(|(k, (f, row))| Some((k, *by_obj.get(&f.obj)?, row.tone)))
        .collect()
}

/// Where a ray from `r`'s centre in direction `dir` (a unit vector) leaves `r`.
fn edge(r: Rect, dir: Vec2) -> Pos2 {
    let half = r.size() / 2.0;
    let along = |h: f32, d: f32| if d.abs() > 1e-6 { h / d.abs() } else { f32::INFINITY };
    r.center() + dir * along(half.x, dir.x).min(along(half.y, dir.y))
}

/// The route of an arrow from field `a` to field `b`: the line's points, which stop where the head
/// starts, and the head's three corners. Fields one above the other link out to the right, and
/// fields side by side over the top, clear of the fields between them (a grid's neighbours would
/// otherwise get arrows too short to see). Other fields link between their nearest edges, bent a
/// little to the arrow's left so the two arrows of a loop don't overlap. A field that reads itself
/// gets a loop off its right edge.
fn route(a: Rect, b: Rect, zoom: f32, style: &CalcOrderOverlayStyle) -> (Vec<Pos2>, Vec<Pos2>) {
    let (head, margin, bend_cap) = (style.head, style.margin, style.bend_cap);
    let gap = b.center() - a.center();
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
    let (start, c1, c2, end) = if gap.length() < 1.0 {
        let bow = 24.0 * zoom;
        let (start, end) = (pos2(a.right(), a.center().y - 4.0), pos2(a.right(), a.center().y + 4.0));
        (start, start + vec2(bow, -bow * 0.6), end + vec2(bow, bow * 0.6), end)
    } else if overlap(a.left(), a.right(), b.left(), b.right()) > 0.0 {
        let (start, end) = (pos2(a.right() + 2.0, a.center().y - 3.0), pos2(b.right() + 2.0, b.center().y + 3.0));
        let bow = (18.0 * zoom + (end.y - start.y).abs() * 0.3).min(140.0);
        (start, start + vec2(bow, 0.0), end + vec2(bow, 0.0), end)
    } else if overlap(a.top(), a.bottom(), b.top(), b.bottom()) > 0.0 {
        let (start, end) = (pos2(a.center().x - 3.0, a.top() - 2.0), pos2(b.center().x + 3.0, b.top() - 2.0));
        let bow = (14.0 * zoom + (end.x - start.x).abs() * 0.25).min(100.0);
        (start, start - vec2(0.0, bow), end - vec2(0.0, bow), end)
    } else {
        let dir = gap / gap.length();
        let (start, end) = (edge(a, dir) + dir * margin, edge(b, -dir) - dir * margin);
        let bend = vec2(dir.y, -dir.x) * ((end - start).length() * 0.18).min(bend_cap);
        (start, start + (end - start) / 3.0 + bend, start + (end - start) * (2.0 / 3.0) + bend, end)
    };
    // The line stops where the head starts, which points along the curve's last stretch.
    let back = (end - c2).normalized();
    let back = if back.is_finite() && back != Vec2::ZERO { back } else { vec2(-1.0, 0.0) };
    let base = end - back * head;
    let point = |p: Pos2| Point::new(f64::from(p.x), f64::from(p.y));
    let curve = CubicBez::new(point(start), point(c1), point(c2), point(base));
    // About one segment per four points of chord, so a long arrow stays smooth and a short one cheap.
    let steps = (start.distance(base) / 4.0).ceil().clamp(8.0, 64.0) as usize;
    let line = (0..=steps).map(|k| curve.eval(k as f64 / steps as f64)).map(|p| pos2(p.x as f32, p.y as f32)).collect();
    let side = vec2(-back.y, back.x) * (head / 2.0);
    (line, vec![end, base + side, base - side])
}

/// The colours of the page overlay.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct CalcOrderOverlayStyle {
    #[style(metrics)]
    metrics: Metrics,
    /// The length of an arrowhead; its width is the same.
    head: f32,
    /// The room between an arrow's end and the field it points at.
    margin: f32,
    /// The most an arrow between two far fields bends.
    bend_cap: f32,
    /// A link that arrives in time, and the outline of a related field.
    ok: StyleColor,
    /// A link that arrives too late.
    late: StyleColor,
    /// A link in a loop, or a formula reading a field the form doesn't have.
    bad: StyleColor,
    /// A step of a field in its place.
    step: StyleColor,
    /// The number and rim of a step.
    number: StyleColor,
    /// The cover over the fields the focus has nothing to do with.
    scrim: StyleColor,
    /// The trace pill's fill.
    pill: StyleColor,
    /// The trace pill's text.
    text: StyleColor,
}

impl CalcOrderOverlayStyle {
    fn of(&self, tone: Tone) -> StyleColor {
        match tone {
            Tone::Ok => self.step,
            Tone::Late => self.late,
            Tone::Loop | Tone::Missing => self.bad,
        }
    }
}

/// The calculation order over the pages: the links of the field in focus, with the fields it has
/// nothing to do with covered, and the steps of the focus and its linked fields; with nothing in
/// focus, the steps of the fields whose place is wrong. A form with many calculations stays
/// readable either way. Above the pages' top edge a pill says in words what the focus reads and
/// what reads it. It is display only: the hovered field of a pass comes from the page's parts.
#[derive(Component, craft::ui::contract::Props)]
#[component(canvas_control)]
pub struct CalcOrderOverlay<'a> {
    #[common]
    common: Common,
    view: &'a CalcOrderView,
    form: &'a [FormField],
    info: &'a DocInfo,
    /// The pages on screen this pass.
    #[prop]
    placed: &'a [craft::pages::ui::PlacedPage],
    /// The view's rotation.
    #[prop]
    rotation: u16,
    /// The document's edit generation the order is read at.
    #[prop]
    generation: u64,
    /// The Prepare selection.
    #[prop(into)]
    selected: Option<&'a str>,
    /// The field under the pointer on the page this pass.
    #[prop(into)]
    hovered: Option<&'a Focus>,
    /// The page view's rect, which the pill sits at the top of.
    #[prop]
    viewport: Rect,
}

impl<'a> CalcOrderOverlay<'a> {
    /// The overlay of `view`'s order on `info`'s pages, with `form`'s fields. The pages, rotation,
    /// generation, selection, hover and viewport are props.
    pub fn new(view: &'a CalcOrderView, form: &'a [FormField], info: &'a DocInfo) -> Self {
        CalcOrderOverlay {
            common: Common::new(),
            view,
            form,
            info,
            placed: &[],
            rotation: 0,
            generation: 0,
            selected: None,
            hovered: None,
            viewport: Rect::NOTHING,
        }
        .label(tl!("Calculation order links"))
    }
}

impl Component for CalcOrderOverlay<'_> {
    const ROLE: Role = Role::GRAPHICS_OBJECT;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    type Style = CalcOrderOverlayStyle;
    type Output = Response;

    fn style(cx: &StyleCx<'_>, _: CalcOrderOverlayStyleKey) -> CalcOrderOverlayStyle {
        let c = &cx.tokens.color;
        CalcOrderOverlayStyle {
            metrics: Metrics::new(),
            head: cx.tokens.size.icon.sm,
            margin: cx.tokens.space.gap.tight,
            bend_cap: cx.tokens.size.control.md * 2.0,
            ok: cx.tone(Status::Good),
            late: cx.tone(Status::Warn),
            bad: cx.tone(Status::Bad),
            step: cx.color(c.text.muted),
            number: cx.color(c.surface.raised),
            scrim: cx.color(c.surface.scrim),
            pill: cx.color(c.surface.raised),
            text: cx.color(c.text.normal),
        }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let Some(listed) = self.view.current(self.generation) else { return };
        let style = &cx.style;
        let draw = ui.draw();
        let focus = if self.view.hide_links { None } else { self.view.focus(self.hovered, self.selected) };
        let focused = focus.as_ref().map(|f| focus_links(&listed.links, f));
        for p in self.placed {
            let Some(page) = self.info.pages.get(p.page) else { continue };
            let xf = PageXform { rect: p.rect, rot: self.rotation, pw: page.width, ph: page.height };
            // Each field's first widget on this page.
            let mut on_page: Vec<(&str, ObjRef, Rect)> = Vec::new();
            for f in self.form {
                if let Some(w) = f.widgets.iter().find(|w| w.page == Some(p.page) && !w.hidden) {
                    let r = w.rect;
                    on_page.push((&f.name, f.obj, xf.user_rect(self.info, p.page, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32])));
                }
            }
            if on_page.is_empty() {
                continue;
            }
            // Arrows start at a read name's field (one of them, if fields share it) and end at the formula's.
            let by_name: HashMap<&str, Rect> = on_page.iter().map(|(name, _, r)| (*name, *r)).collect();
            let by_obj: HashMap<ObjRef, Rect> = on_page.iter().map(|(_, obj, r)| (*obj, *r)).collect();
            let zoom = xf.zoom().clamp(0.5, 3.0);
            if let (Some(focus), Some((shown, related))) = (&focus, &focused) {
                for (name, obj, r) in &on_page {
                    let rect = r.expand(1.0);
                    if !related(name, *obj) {
                        draw.fill(&PaintShape::Rect(rect, CornerRadius::ZERO), style.scrim);
                    } else {
                        let width = if focus.is(name, *obj) { StrokeWidth::Medium } else { StrokeWidth::Thin };
                        draw.stroke(&PaintShape::Rect(rect, CornerRadius::same(2)), width, style.ok);
                    }
                }
                for l in shown {
                    if let (Some(a), Some(b)) = (by_name.get(l.from.as_str()), by_obj.get(&l.reader)) {
                        let colour = match l.tone {
                            Tone::Ok => style.ok,
                            Tone::Late => style.late,
                            Tone::Loop | Tone::Missing => style.bad,
                        };
                        let (line, head) = route(*a, *b, zoom, style);
                        if l.tone == Tone::Late {
                            draw.dashed(&line, draw.dashes(), StrokeWidth::Medium, colour);
                        } else {
                            draw.stroke(&PaintShape::Path(line), StrokeWidth::Medium, colour);
                        }
                        draw.fill(&PaintShape::Polygon(head), colour);
                    }
                }
            }
            let related = focused.as_ref().map(|(_, related)| related as Related<'_>);
            for (k, r, tone) in steps(&listed, &by_obj, related) {
                // No bigger than the field, so a small one keeps its contents in view.
                let radius = (8.0 * zoom).clamp(6.5, 10.0).min((r.height() * 0.6).max(5.0));
                let c = r.left_top();
                draw.fill(&PaintShape::Circle(c, radius), style.of(tone));
                draw.stroke(&PaintShape::Circle(c, radius), StrokeWidth::Medium, style.number);
                let face = Rect::from_center_size(c, Vec2::splat(radius * 2.0));
                draw.text(face, &TextRun::new((k + 1).to_string()).color(style.number).align(HAlign::Center));
            }
        }
        if let Some(focus) = &focus {
            let text = trace(&listed.links, focus, &listed.order.fields);
            // Clear of the floating tool strip at the view's left.
            let width = (self.viewport.width() - 220.0).max(0.0);
            if width > 0.0 {
                let pill = Rect::from_center_size(pos2(self.viewport.center().x, self.viewport.top() + 26.0), vec2(width, 30.0));
                draw.fill(&PaintShape::Rect(pill, CornerRadius::same(15)), style.pill);
                draw.stroke(&PaintShape::Rect(pill, CornerRadius::same(15)), StrokeWidth::Thin, style.step);
                let icon = Rect::from_center_size(pill.left_center() + vec2(18.0, 0.0), vec2(14.0, 14.0));
                draw.icon(icon, &IconSpec::svg(IconId::new("move-right")).color(style.ok));
                let room = Rect::from_min_max(pill.left_center() - vec2(-32.0, 15.0), pill.right_center() + vec2(-14.0, 15.0));
                draw.text(room, &TextRun::new(text).color(style.text).elide());
            }
        }
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, reads: &[&str], issues: Vec<CalculationIssue>) -> CalculatedField {
        let obj = ObjRef { num: 0, generation: 0 };
        CalculatedField { name: name.into(), obj, reads: reads.iter().map(|r| (*r).to_owned()).collect(), listed: true, issues }
    }

    fn link(from: &str, to: &str, tone: Tone) -> Link {
        Link { from: from.into(), to: to.into(), reader: ObjRef { num: 0, generation: 0 }, tone }
    }

    #[test]
    fn a_read_name_links_the_field_of_that_name_and_the_fields_under_it() {
        let calculated = [field("total", &["row", "row.1", "tax"], vec![])];
        let got = links(&calculated, vec!["rows", "row.2", "total", "row", "row.1", "tax"]);
        let from: Vec<&str> = got.iter().map(|l| l.from.as_str()).collect();
        // `rows` isn't under `row`, and `row.1` named twice links once.
        assert_eq!(from, ["row", "row.1", "row.2", "tax"]);
        assert!(got.iter().all(|l| l.to == "total" && l.tone == Tone::Ok));
    }

    #[test]
    fn a_link_is_late_or_in_a_loop_as_its_reader_s_issues_say() {
        let calculated = [
            field("total", &["sub", "shipping"], vec![CalculationIssue::RunsTooEarly { field: "sub".into() }]),
            field("sub", &["qty"], vec![]),
            field("x", &["y"], vec![CalculationIssue::Cycle { fields: vec!["y".into()] }]),
            field("y", &["x"], vec![CalculationIssue::Cycle { fields: vec!["x".into()] }]),
        ];
        let got = links(&calculated, vec!["qty", "shipping", "sub", "total", "x", "y"]);
        assert_eq!(
            got,
            [
                link("sub", "total", Tone::Late),
                link("shipping", "total", Tone::Ok),
                link("qty", "sub", Tone::Ok),
                link("y", "x", Tone::Loop),
                link("x", "y", Tone::Loop),
            ]
        );
    }

    /// The names a focus reads stand before the joiner and the names that read it after it.
    #[test]
    fn the_summary_says_what_a_field_reads_and_what_reads_it() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let links = [link("sub", "total", Tone::Late), link("tax", "total", Tone::Ok), link("qty", "sub", Tone::Ok), link("sub", "tax", Tone::Ok)];
        let sides = |name: &str| {
            let text = trace(&links, &Focus::named(name), &[]);
            let (reads, read_by) = text.split_once('·').map(|(a, b)| (a.to_owned(), b.to_owned())).expect("two sides");
            (reads, read_by)
        };
        let (reads, read_by) = sides("total");
        assert!(reads.contains("sub") && reads.contains("tax") && !read_by.contains("sub") && !read_by.contains("tax"));
        let (reads, read_by) = sides("sub");
        assert!(reads.contains("qty") && !reads.contains("total"));
        assert!(read_by.contains("total") && read_by.contains("tax"));
        let (reads, read_by) = sides("qty");
        assert!(!reads.contains("sub") && read_by.contains("sub"));
    }

    /// A field the form doesn't have is read too, and said to be missing; a script's names built
    /// as it runs change what is said.
    #[test]
    fn the_summary_says_what_is_missing_and_what_is_built() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let links = [link("qty", "sub", Tone::Ok), link("sub", "total", Tone::Ok)];
        let sub = field("sub", &["qty", "discount"], vec![CalculationIssue::MissingField { field: "discount".into() }]);
        assert!(trace(&links, &Focus::named("sub"), &[sub]).contains("discount"));
        let plain = field("rows", &[], vec![]);
        let built = field("rows", &[], vec![CalculationIssue::BuiltNames]);
        assert_ne!(trace(&links, &Focus::named("rows"), &[plain]), trace(&links, &Focus::named("rows"), &[built]));
    }

    /// Two calculated fields named `total`, as a malformed form can have, each reading a field the
    /// form doesn't have, and `grand` reading `total`.
    fn two_totals() -> ([CalculatedField; 3], Vec<Link>) {
        let total = |num, reads: &[&str], gone: &str| CalculatedField {
            obj: ObjRef { num, generation: 0 },
            ..field("total", reads, vec![CalculationIssue::MissingField { field: gone.into() }])
        };
        let grand = CalculatedField { obj: ObjRef { num: 3, generation: 0 }, ..field("grand", &["total"], vec![]) };
        let calculated = [total(1, &["qty", "lost"], "lost"), total(2, &["tax", "gone"], "gone"), grand];
        let links = links(&calculated, vec!["qty", "tax", "total", "total", "grand"]);
        (calculated, links)
    }

    /// Hovering a row of a field that shares its name says that field's reads and missing fields,
    /// not the first one's of that name. The selection is only a name, so it says both.
    #[test]
    fn a_row_sharing_its_name_is_traced_by_its_field() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let (calculated, links) = two_totals();
        let mut view = CalcOrderView::default();
        let mut said = Vec::new();
        for f in &calculated {
            view.hover_row = Some(Focus::row(f));
            let focus = view.focus(None, Some("total")).expect("a hovered row is the focus");
            assert_eq!(focus, Focus::row(f));
            said.push(trace(&links, &focus, &calculated));
        }
        assert!(said[0].contains("lost") && !said[0].contains("gone"));
        assert!(said[1].contains("gone") && !said[1].contains("lost"));
        view.hover_row = None;
        let selected = view.focus(None, Some("total")).expect("the selection is the focus");
        assert_eq!(selected, Focus::named("total"));
        let both = trace(&links, &selected, &calculated);
        assert!(both.contains("lost") && both.contains("gone"));
    }

    /// On the page too, a field that shares its name is outlined, and its arrows drawn, as one
    /// field: the fields it reads by name, the formulas that read it by object.
    #[test]
    fn a_field_sharing_its_name_is_related_on_the_page_by_its_object() {
        let (calculated, links) = two_totals();
        let [first, second, grand] = calculated.each_ref().map(|f| f.obj);
        let elsewhere = ObjRef { num: 9, generation: 0 };
        let focus = Focus::row(&calculated[0]);
        let (shown, related) = focus_links(&links, &focus);
        assert!(shown.iter().all(|l| l.reader == first || l.to == "grand"), "{shown:?}");
        assert!(related("total", first) && !related("total", second), "only the field in focus");
        assert!(related("qty", elsewhere) && !related("tax", elsewhere), "only the fields it reads");
        assert!(related("grand", grand), "the formula that reads it");
        let selected = Focus::named("total");
        let (_, related) = focus_links(&links, &selected);
        assert!(related("total", first) && related("total", second) && related("tax", elsewhere), "a selected name is every field of it");
    }

    /// Each step goes on its own field's widget: two fields named `total` get one each, and a
    /// focus on one shows its step and its reader's, not the other's.
    #[test]
    fn a_step_goes_on_its_own_field_when_names_are_shared() {
        let (calculated, links) = two_totals();
        let [first, second, grand] = calculated.each_ref().map(|f| f.obj);
        let at = |x: f32| Rect::from_min_size(pos2(x, 0.0), vec2(10.0, 10.0));
        let by_obj = HashMap::from([(first, at(0.0)), (second, at(100.0)), (grand, at(200.0))]);
        let rows = rows(&calculated, &[]);
        let mut order = CalculationOrder::default();
        order.fields = calculated.to_vec();
        let listed = Listed { order, rows, links };
        // Unfocused, the two broken formulas show their steps.
        assert_eq!(steps(&listed, &by_obj, None), [(0, at(0.0), Tone::Missing), (1, at(100.0), Tone::Missing)]);
        let focus = Focus::row(&calculated[0]);
        let (_, related) = focus_links(&listed.links, &focus);
        assert_eq!(steps(&listed, &by_obj, Some(&related)), [(0, at(0.0), Tone::Missing), (2, at(200.0), Tone::Ok)]);
    }

    /// A missing field outranks a loop on the row, but the field is still in its loop.
    #[test]
    fn a_missing_field_marks_the_row_and_keeps_its_loop() {
        let gone = CalculationIssue::MissingField { field: "gone".into() };
        assert_eq!(tone(&[gone.clone(), CalculationIssue::Cycle { fields: vec!["b".into()] }]), Tone::Missing);
        assert_eq!(tone(&[CalculationIssue::BuiltNames]), Tone::Ok, "built names are a note, not a fault");
        let calculated = [
            field("a", &["b", "gone"], vec![gone, CalculationIssue::Cycle { fields: vec!["b".into()] }]),
            field("b", &["a"], vec![CalculationIssue::Cycle { fields: vec!["a".into()] }]),
        ];
        assert_eq!(loops(&calculated), vec![vec!["a", "b"]]);
    }

    #[test]
    fn loops_group_the_fields_that_read_each_other() {
        let calculated = [
            field("a", &["b"], vec![CalculationIssue::Cycle { fields: vec!["b".into()] }]),
            field("x", &["x"], vec![CalculationIssue::Cycle { fields: vec!["x".into()] }]),
            field("ok", &[], vec![]),
            field("c", &["a"], vec![CalculationIssue::Cycle { fields: vec!["a".into()] }]),
            field("b", &["c"], vec![CalculationIssue::Cycle { fields: vec!["c".into()] }]),
        ];
        assert_eq!(loops(&calculated), vec![vec!["a", "c", "b"], vec!["x"]]);
    }

    /// A move's `to` counts the order that remains once the moved rows are out, so Alt+↓ on a row
    /// at 1 is `to` 2, and a group keeps its order.
    #[test]
    fn a_move_lands_at_its_place_among_the_rows_that_stay() {
        let o = |num| ObjRef { num, generation: 0 };
        let order = [o(1), o(2), o(3), o(4), o(5)];
        assert_eq!(reordered(&order, &[o(2)], 3), [o(1), o(3), o(4), o(2), o(5)]);
        assert_eq!(reordered(&order, &[o(2), o(4)], 0), [o(2), o(4), o(1), o(3), o(5)]);
        assert_eq!(reordered(&order, &[o(1)], 99), [o(2), o(3), o(4), o(5), o(1)]);
    }
}
