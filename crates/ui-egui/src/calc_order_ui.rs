//! Prepare a form ▸ Calculation order, a page of the left panel, and what it shows on the page.
//!
//! The panel: a card that says whether the order is right and what Fix order does, a card for
//! each loop, and the calculated fields in the order they calculate, each with its formula, the
//! value it shows and what is wrong with its place. A field moves by dragging, with the arrows of
//! the selected field (Acrobat's Set Field Calculation Order has Up and Down), or with Alt+↑/↓.
//! Every change is one undoable edit.
//!
//! The page: each calculated field's step in the order, and for the field in focus (under the
//! pointer in the list or on the page, else the selected one) arrows from the fields it reads and
//! to the fields that read it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2, vec2};
use pdfcraft_engine::form_scripts::{CalcOp, Calculate, format_value};
use pdfcraft_engine::{CalculatedField, CalculationIssue, CalculationOrder, Edit, FormField, FormFieldKind};
use pdfcraft_render::DocInfo;

use crate::canvas::PageXform;
use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, icons, widgets};

/// Links and steps on the page, which is paper white in either theme.
const LINK_OK: Color32 = Color32::from_rgb(0x1B, 0x63, 0xE0);
const LINK_LATE: Color32 = Color32::from_rgb(0xA8, 0x64, 0x00);
const LINK_LOOP: Color32 = Color32::from_rgb(0xC2, 0x26, 0x2E);
const STEP: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x42);
const ROW: f32 = 46.0;
/// Links kept at most: a formula reading a group reads every field under it.
const MAX_LINKS: usize = 20_000;
/// Names a sentence lists before it stops with "…".
const MAX_NAMES: usize = 6;
/// Loop cards shown at most.
const MAX_LOOPS: usize = 5;
/// Fields too early that the card names one by one.
const MAX_LATE: usize = 3;

/// How a field's place in the order, or a link between two fields, stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Ok,
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
    /// The field whose row is under the pointer, this frame.
    hover_row: Option<String>,
    /// The field under the pointer on the page, last frame.
    hover_page: Option<String>,
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
    tone: Tone,
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

    /// The field under the pointer on the page, for the next frame.
    pub(crate) fn set_hover_page(&mut self, name: Option<String>) {
        self.hover_page = name;
    }

    /// The field whose links show: under the pointer in the list or on the page, else selected.
    fn focus(&self, selected: Option<&str>) -> Option<String> {
        self.hover_row.clone().or_else(|| self.hover_page.clone()).or_else(|| selected.map(str::to_owned))
    }
}

fn tone(issues: &[CalculationIssue]) -> Tone {
    if issues.iter().any(|i| matches!(i, CalculationIssue::Cycle { .. })) {
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

/// Each calculated field's value, formula and standing, in the order's order.
fn rows(calculated: &[CalculatedField], form: &[FormField]) -> Vec<Row> {
    let mut by_name: HashMap<&str, &FormField> = HashMap::with_capacity(form.len());
    for f in form {
        by_name.entry(f.name.as_str()).or_insert(f);
    }
    calculated
        .iter()
        .map(|c| {
            let field = by_name.get(c.name.as_str());
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
                out.push(Link { from: from.to_owned(), to: c.name.clone(), tone });
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
    for (k, f) in calculated.iter().enumerate().filter(|(_, f)| tone(&f.issues) == Tone::Loop) {
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
fn trace(links: &[Link], focus: &str) -> String {
    let mut reads: Vec<&str> = Vec::new();
    let mut read_by: Vec<&str> = Vec::new();
    for l in links {
        if l.to == focus && !reads.contains(&l.from.as_str()) {
            reads.push(&l.from);
        }
        if l.from == focus && !read_by.contains(&l.to.as_str()) {
            read_by.push(&l.to);
        }
    }
    let reads = if reads.is_empty() {
        crate::i18n::fmt(tl!("{field} reads no other field"), &[("field", focus)])
    } else {
        crate::i18n::fmt(tl!("{field} reads {fields}"), &[("field", focus), ("fields", &names(&reads))])
    };
    let read_by = if read_by.is_empty() {
        tl!("No field reads it").to_owned()
    } else {
        let template = crate::i18n::trn(crate::i18n::current(), read_by.len() as u64, "{fields} reads it", "{fields} read it");
        crate::i18n::fmt(&template, &[("fields", &names(&read_by))])
    };
    format!("{reads} · {read_by}")
}

/// The colours of the cards and the steps in the list, for the theme.
struct Palette {
    warn: (Color32, Color32, Color32, Color32),
    good: (Color32, Color32, Color32),
    looped: (Color32, Color32, Color32, Color32),
    /// A step's fill and number, and the caption colour, by tone.
    step_ok: (Color32, Color32),
    step_late: (Color32, Color32),
    step_loop: (Color32, Color32),
}

impl Palette {
    /// Fill, border, title and text of each card; light and dark pass 4.5:1 for their text.
    fn new(dark: bool) -> Self {
        let c = Color32::from_rgb;
        if dark {
            Palette {
                warn: (c(0x3A, 0x2E, 0x17), c(0x6B, 0x53, 0x21), c(0xF2, 0xC2, 0x6B), c(0xE6, 0xD7, 0xB8)),
                good: (c(0x1D, 0x33, 0x26), c(0x2F, 0x5C, 0x40), c(0x8F, 0xD6, 0xA8)),
                looped: (c(0x3D, 0x20, 0x22), c(0x6E, 0x32, 0x36), c(0xF1, 0x9A, 0x9E), c(0xE9, 0xC9, 0xCB)),
                step_ok: (c(0x3A, 0x3A, 0x40), c(0xD4, 0xD4, 0xDA)),
                step_late: (c(0x4A, 0x3A, 0x1A), c(0xF2, 0xC2, 0x6B)),
                step_loop: (c(0x4A, 0x22, 0x24), c(0xF1, 0x9A, 0x9E)),
            }
        } else {
            Palette {
                warn: (c(0xFF, 0xF6, 0xE5), c(0xF2, 0xD9, 0xA6), c(0x8A, 0x51, 0x00), c(0x4A, 0x3A, 0x1E)),
                good: (c(0xEE, 0xF8, 0xF1), c(0xBF, 0xE3, 0xCC), c(0x1E, 0x6B, 0x3C)),
                looped: (c(0xFD, 0xF0, 0xF0), c(0xF1, 0xC4, 0xC6), c(0xA8, 0x1E, 0x25), c(0x5A, 0x2A, 0x2D)),
                step_ok: (c(0xED, 0xED, 0xF0), c(0x3A, 0x3A, 0x42)),
                step_late: (c(0xFF, 0xF0, 0xD6), c(0x8A, 0x51, 0x00)),
                step_loop: (c(0xFD, 0xE8, 0xE8), c(0xC2, 0x26, 0x2E)),
            }
        }
    }

    fn step(&self, tone: Tone) -> (Color32, Color32) {
        match tone {
            Tone::Ok => self.step_ok,
            Tone::Late => self.step_late,
            Tone::Loop => self.step_loop,
        }
    }
}

/// A field being dragged to a new place: its place now.
#[derive(Clone, Copy)]
struct Dragged(usize);

/// What the page asks for, done once it is drawn.
enum Act {
    Order(Vec<String>),
    Undo,
    EditFormula(String),
    Select(String),
}

/// A card: a tinted, outlined box as wide as the panel.
fn card<R>(ui: &mut egui::Ui, fill: Color32, edge: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui)
        })
        .inner
}

/// A card's first line: its icon and title.
fn card_title(ui: &mut egui::Ui, icon: &str, title: &str, colour: Color32) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 17.0), Sense::hover());
        icons::paint(ui, r, icon, 15.0, colour);
        ui.add(egui::Label::new(egui::RichText::new(title).font(theme::semibold(13.0)).color(colour)).wrap());
    });
}

fn card_text(ui: &mut egui::Ui, text: &str, colour: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(12.0)).color(colour)).wrap());
}

/// A word that acts: "Undo", "Edit total's formula".
fn text_button(ui: &mut egui::Ui, text: &str, t: &Tokens) -> bool {
    let resp =
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::semibold(12.5)).color(t.accent_text)).selectable(false).sense(Sense::click()));
    theme::hand(resp).clicked()
}

/// The cards above the list: what is wrong and Fix order, or that the order is right; and loops.
fn cards(ui: &mut egui::Ui, t: &Tokens, pal: &Palette, order: &CalculationOrder, editable: bool, undo: bool) -> Option<Act> {
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
    if order.fixable() {
        let (fill, edge, title, text) = pal.warn;
        card(ui, fill, edge, |ui| {
            let heading = if late.is_empty() {
                tl!("The order saved in the PDF needs tidying").to_owned()
            } else {
                crate::i18n::trn(lang, late.len() as u64, "{n} field shows an old value", "{n} fields show old values")
            };
            card_title(ui, "triangle-alert", &heading, title);
            for (field, later) in late.iter().take(MAX_LATE) {
                let template = crate::i18n::trn(
                    lang,
                    later.len() as u64,
                    "{field} calculates before {fields}, which it reads.",
                    "{field} calculates before {fields}, which it reads.",
                );
                card_text(ui, &crate::i18n::fmt(&template, &[("field", field), ("fields", &names(later))]), text);
            }
            if late.len() > MAX_LATE {
                let more = (late.len() - MAX_LATE) as u64;
                card_text(ui, &crate::i18n::trn(lang, more, "{n} more field calculates too early.", "{n} more fields calculate too early."), text);
            }
            if !unlisted.is_empty() {
                let template = crate::i18n::trn(
                    lang,
                    unlisted.len() as u64,
                    "{fields} is missing from the order saved in the PDF, so other PDF viewers may not calculate it.",
                    "{fields} are missing from the order saved in the PDF, so other PDF viewers may not calculate them.",
                );
                card_text(ui, &crate::i18n::fmt(&template, &[("fields", &names(&unlisted))]), text);
            }
            if !order.extra.is_empty() {
                let note = crate::i18n::trn(
                    lang,
                    order.extra.len() as u64,
                    "The calculation order has {n} entry that calculates nothing. Fix order removes it.",
                    "The calculation order has {n} entries that calculate nothing. Fix order removes them.",
                );
                card_text(ui, &note, text);
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let fix = ui.add_enabled_ui(editable, |ui| widgets::pill_button(ui, tl!("Fix order"), true)).inner;
                if fix.on_hover_text(tl!("Move only what must move, so every field calculates after the fields it reads.")).clicked() {
                    act = Some(Act::Order(order.fixed()));
                }
                ui.add(egui::Label::new(egui::RichText::new(tl!("Moves only what must move")).font(theme::regular(11.5)).color(t.text_muted)).wrap());
            });
        });
    } else {
        let (fill, edge, colour) = pal.good;
        card(ui, fill, edge, |ui| {
            let said = if loops.is_empty() {
                tl!("Every field calculates after the fields it reads.")
            } else {
                tl!("Every field outside a loop calculates after the fields it reads.")
            };
            card_title(ui, "check", said, colour);
            if undo && text_button(ui, tl!("Undo"), t) {
                act = Some(Act::Undo);
            }
        });
    }
    for members in loops.iter().take(MAX_LOOPS) {
        let (fill, edge, title, text) = pal.looped;
        ui.add_space(6.0);
        card(ui, fill, edge, |ui| {
            let heading = match members.as_slice() {
                [one] => crate::i18n::fmt(tl!("{field} reads its own value"), &[("field", *one)]),
                _ => crate::i18n::fmt(tl!("{fields} read each other"), &[("fields", &names(members))]),
            };
            card_title(ui, "rotate-cw", &heading, title);
            card_text(ui, tl!("No order can fix a loop. Change one of the formulas."), text);
            if editable
                && let Some(first) = members.first()
                && text_button(ui, &crate::i18n::fmt(tl!("Edit {field}'s formula"), &[("field", first)]), t)
            {
                act = Some(Act::EditFormula((*first).to_owned()));
            }
        });
    }
    act
}

/// Why a row is marked, for its tooltip.
fn explained(issues: &[CalculationIssue]) -> String {
    issues
        .iter()
        .map(|i| match i {
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
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The Calculation order page; Back returns to Prepare a form.
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (back, close) = crate::panels::panel_header(ui, t, "Calculation order", Some(("chevron-left", tl!("Back to Prepare a form"))));
    if back {
        app.left = LeftPanel::Tool("form");
    }
    if close {
        app.left_open = false;
    }
    let Some((index, id)) = app.active_ids() else {
        ui.label(egui::RichText::new(tl!("Open a document first")).color(t.text_muted));
        return;
    };
    let Some(doc) = app.session.get(id) else { return };
    let Some(view) = app.views.get_mut(index) else { return };
    let listed = view.calc_order.listed(doc);
    let order = &listed.order;
    if order.fields.is_empty() && order.extra.is_empty() {
        ui.label(
            egui::RichText::new(tl!(
                "No field in this form calculates its value yet. Give a field a calculation on the Calculate tab of its properties, and it shows here."
            ))
            .color(t.text_muted),
        );
        return;
    }
    let editable = doc.allows_modification();
    let pal = Palette::new(ui.visuals().dark_mode);
    let mut act = cards(ui, t, &pal, order, editable, doc.can_undo() == Some("Set calculation order"));

    widgets::section_title(ui, tl!("Order · top calculates first"));
    let mut links_on = !view.calc_order.hide_links;

    let names: Vec<&str> = order.fields.iter().map(|f| f.name.as_str()).collect();
    let moved = |from: usize, to: usize| -> Option<Act> {
        let mut next: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        (from != to && from < next.len() && to < next.len()).then(|| {
            let name = next.remove(from);
            next.insert(to, name);
            Act::Order(next)
        })
    };
    let selected = view.prepare.selected.as_ref().map(|(name, _)| name.clone());
    let place = selected.as_deref().and_then(|s| names.iter().position(|n| *n == s));
    // Alt+↑/↓ move the selected field, as in Combine files.
    let ctx = ui.ctx().clone();
    if editable
        && let Some(k) = place
        && !ctx.text_edit_focused()
        && !egui::Popup::is_any_open(&ctx)
        && ctx.memory(|m| m.top_modal_layer().is_none())
    {
        let (up, down) =
            ctx.input_mut(|i| (i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp), i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown)));
        if up && let Some(a) = k.checked_sub(1).and_then(|to| moved(k, to)) {
            act = Some(a);
        } else if down && let Some(a) = moved(k, k + 1) {
            act = Some(a);
        }
    }

    let focus_page = view.calc_order.hover_page.clone();
    // A field selected since the last frame, whose row the list shows.
    let reveal = (selected != view.calc_order.scrolled_to).then(|| selected.clone()).flatten();
    view.calc_order.scrolled_to = selected.clone();
    let mut hover_row = None;
    let count = names.len();
    egui::ScrollArea::vertical().id_salt(("calc-order", id)).auto_shrink([false, false]).show(ui, |ui| {
        for (k, (f, row)) in order.fields.iter().zip(&listed.rows).enumerate() {
            let sense = if editable { Sense::click_and_drag() } else { Sense::click() };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), sense);
            let is_selected = selected.as_deref() == Some(f.name.as_str());
            resp.widget_info(|| {
                let label = if row.value.is_empty() { format!("{}. {}", k + 1, f.name) } else { format!("{}. {}: {}", k + 1, f.name, row.value) };
                egui::WidgetInfo::selected(egui::Role::Button, true, is_selected, label)
            });
            if reveal.as_deref() == Some(f.name.as_str()) {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            let pointed = ui.rect_contains_pointer(rect);
            if pointed {
                hover_row = Some(f.name.clone());
            }
            let visible = ui.is_rect_visible(rect);
            // The arrows' column, kept for every row so nothing moves when they show.
            let arrows_x = rect.right() - 26.0;
            if visible {
                let press = theme::Press::track(ui, &resp);
                if !is_selected && focus_page.as_deref() == Some(f.name.as_str()) {
                    ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
                }
                press.wash(ui, rect, 8, is_selected);
                let body = rect.translate(press.offset());
                let (fill, number) = pal.step(row.tone);
                let step = pos2(body.left() + 19.0, body.center().y);
                ui.painter().circle_filled(step, 11.0, fill);
                ui.painter().text(step, Align2::CENTER_CENTER, (k + 1).to_string(), theme::semibold(11.0), number);
                let top = body.top() + 15.0;
                let bottom = body.top() + 31.0;
                let text_x = body.left() + 38.0;
                let value_end = arrows_x - 4.0;
                let caption = match row.tone {
                    Tone::Loop => Some((tl!("Loop"), pal.step_loop.1)),
                    Tone::Late => Some((tl!("Too early"), pal.step_late.1)),
                    Tone::Ok if !f.listed => Some((tl!("Not in order"), pal.step_late.1)),
                    Tone::Ok => None,
                };
                let value_font = theme::medium(13.0);
                let value = widgets::fit_line(&row.value, (value_end - text_x) * 0.4, |s| widgets::text_width(ui, s, &value_font));
                let value_w = widgets::text_width(ui, &value, &value_font);
                let value_colour = if row.tone == Tone::Late { pal.step_late.1 } else { t.text };
                ui.painter().text(pos2(value_end, top), Align2::RIGHT_CENTER, value, value_font, value_colour);
                let caption_w = caption.map_or(0.0, |(c, colour)| {
                    let font = theme::semibold(10.5);
                    let w = widgets::text_width(ui, c, &font);
                    ui.painter().text(pos2(value_end, bottom), Align2::RIGHT_CENTER, c, font, colour);
                    w
                });
                let room = (value_end - text_x - value_w.max(caption_w) - 10.0).max(0.0);
                let name_font = theme::medium(13.0);
                let tag = row.script.then(|| (tl!("Script"), widgets::text_width(ui, tl!("Script"), &theme::semibold(9.5)) + 8.0));
                let name_room = (room - tag.map_or(0.0, |(_, w)| w + 6.0)).max(0.0);
                widgets::paint_left(ui, pos2(text_x, top), &f.name, name_font.clone(), t.text, name_room);
                if let Some((label, w)) = tag {
                    let x = text_x + widgets::text_width(ui, &f.name, &name_font).min(name_room) + 6.0;
                    let chip = Rect::from_min_size(pos2(x, top - 7.5), vec2(w, 15.0));
                    ui.painter().rect_stroke(chip, CornerRadius::same(4), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                    ui.painter().text(chip.center(), Align2::CENTER_CENTER, label, theme::semibold(9.5), t.text_muted);
                }
                widgets::paint_left(ui, pos2(text_x, bottom), &row.formula, egui::FontId::monospace(11.0), t.text_muted, room);
            }
            // The selected field's arrows, or the one under the pointer.
            if editable && (is_selected || pointed) {
                let up = Rect::from_min_size(pos2(arrows_x, rect.top() + 3.0), vec2(24.0, 19.0));
                let down = Rect::from_min_size(pos2(arrows_x, rect.bottom() - 22.0), vec2(24.0, 19.0));
                for (r, icon, earlier, tip) in
                    [(up, "chevron-up", true, tl!("Earlier in calculation order")), (down, "chevron-down", false, tl!("Later in calculation order"))]
                {
                    let to = if earlier { k.checked_sub(1) } else { Some(k + 1).filter(|&to| to < count) };
                    let b =
                        ui.interact(r, ui.scope_id().with(("calc-order", k, earlier)), if to.is_some() { Sense::click() } else { Sense::hover() });
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, to.is_some(), format!("{tip}: {}", f.name)));
                    if visible {
                        let bpress = theme::Press::track(ui, &b);
                        bpress.wash(ui, r, 4, false);
                        icons::paint(ui, r.shrink(2.5).translate(bpress.offset()), icon, 14.0, if to.is_some() { t.icon } else { t.text_faint });
                    }
                    if theme::hand(b).on_hover_text(tip).clicked()
                        && let Some(a) = to.and_then(|to| moved(k, to))
                    {
                        act = Some(a);
                    }
                }
            }
            if editable {
                resp.dnd_set_drag_payload(Dragged(k));
                if resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                // A field dragged over this one goes where the line is drawn.
                if let Some(from) = resp.dnd_hover_payload::<Dragged>().map(|d| d.0)
                    && from != k
                {
                    let y = if from > k { rect.top() } else { rect.bottom() };
                    ui.painter().hline(rect.x_range(), y, Stroke::new(2.0, t.accent));
                }
                if let Some(from) = resp.dnd_release_payload::<Dragged>().map(|d| d.0)
                    && let Some(a) = moved(from, k)
                {
                    act = Some(a);
                }
            }
            let resp = theme::hand(resp);
            // The whole formula, then what is wrong; written only while the pointer is on it.
            let resp = if resp.hovered() {
                let why = [row.formula.clone(), explained(&f.issues)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
                if why.is_empty() { resp } else { resp.on_hover_text(why) }
            } else {
                resp
            };
            if resp.clicked() {
                act = Some(Act::Select(f.name.clone()));
            }
        }
        ui.add_space(8.0);
        ui.add(widgets::checkbox(&mut links_on, egui::RichText::new(tl!("Links on page")).font(theme::regular(12.0)).color(t.text_muted)))
            .on_hover_text(tl!("Show on the page which fields each field reads"));
        if editable {
            ui.add_space(4.0);
            let (up, down) = if cfg!(target_os = "macos") { ("⌥↑", "⌥↓") } else { ("Alt+↑", "Alt+↓") };
            let hint = crate::i18n::fmt(tl!("Drag a field to move it, or select it and press {up} or {down}."), &[("up", up), ("down", down)]);
            ui.add(egui::Label::new(egui::RichText::new(hint).font(theme::regular(11.5)).color(t.text_faint)).wrap());
        }
    });
    view.calc_order.hover_row = hover_row;
    view.calc_order.hide_links = !links_on;
    match act {
        Some(Act::Order(names)) => {
            app.apply_edit(Edit::SetCalculationOrder { names });
        }
        Some(Act::Undo) => {
            app.execute("edit.undo");
        }
        Some(Act::EditFormula(name)) => {
            if let Some(view) = app.views.get_mut(index) {
                view.prepare.select((name.clone(), 0), false);
            }
            app.open_field_props(&name, 0);
            if let Some(d) = app.field_props.as_mut() {
                d.tab = crate::prepare::FieldTab::Calculate;
            }
        }
        Some(Act::Select(name)) => {
            // Where its first widget is, to show it there.
            let shown = app
                .session
                .get(id)
                .and_then(|doc| doc.form.iter().find(|g| g.name == name).and_then(|g| g.widgets.first()).and_then(|w| Some((w.page?, w.rect))));
            if let Some(view) = app.views.get_mut(index) {
                view.prepare.select((name, 0), false);
                if let Some((page, rect)) = shown {
                    view.go_to_page(page);
                    view.flash = Some((page, rect.map(|v| v as f32), 0.0));
                }
            }
        }
        None => {}
    }
}

/// The calculation order on page `page`: the links of the field in focus, with the fields it has
/// nothing to do with faded, and the steps of the focus and its linked fields; with nothing in
/// focus, the steps of the fields whose place is wrong. A form with many calculations stays
/// readable either way. Returns the field under the pages' `pointer`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_page(
    painter: &egui::Painter,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    form: &[FormField],
    view: &CalcOrderView,
    generation: u64,
    selected: Option<&str>,
    pointer: Option<Pos2>,
) -> Option<String> {
    let listed = view.current(generation)?;
    // Each field's first widget on this page.
    let mut on_page: Vec<(&str, Rect)> = Vec::new();
    for f in form {
        if let Some(w) = f.widgets.iter().find(|w| w.page == Some(page) && !w.hidden) {
            let r = w.rect;
            on_page.push((&f.name, xf.user_rect(info, page, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32])));
        }
    }
    if on_page.is_empty() {
        return None;
    }
    let hovered = pointer.and_then(|p| on_page.iter().rev().find(|(_, r)| r.contains(p))).map(|(n, _)| (*n).to_owned());
    let at: HashMap<&str, Rect> = on_page.iter().copied().collect();
    let zoom = xf.zoom().clamp(0.5, 3.0);
    let focus = if view.hide_links { None } else { view.focus(selected) };
    // The focus's links, and the fields at either end of them (with the focus itself).
    let shown: Vec<&Link> = focus.as_deref().map(|f| listed.links.iter().filter(|l| l.from == f || l.to == f).collect()).unwrap_or_default();
    let related: Option<HashSet<&str>> = focus.as_deref().map(|f| shown.iter().flat_map(|l| [l.from.as_str(), l.to.as_str()]).chain([f]).collect());
    if let (Some(focus), Some(related)) = (focus.as_deref(), &related) {
        for (name, r) in &on_page {
            if !related.contains(name) {
                painter.rect_filled(r.expand(1.0), CornerRadius::ZERO, Color32::from_white_alpha(150));
            } else {
                let width = if *name == focus { 2.0 } else { 1.0 };
                painter.rect_stroke(r.expand(1.0), CornerRadius::same(2), Stroke::new(width, LINK_OK), egui::StrokeKind::Outside);
            }
        }
        for l in &shown {
            if let (Some(a), Some(b)) = (at.get(l.from.as_str()), at.get(l.to.as_str())) {
                arrow(painter, *a, *b, l.tone, zoom);
            }
        }
    }
    for (k, (f, row)) in listed.order.fields.iter().zip(&listed.rows).enumerate() {
        let Some(r) = at.get(f.name.as_str()) else { continue };
        let shown = match &related {
            Some(related) => related.contains(f.name.as_str()),
            None => row.tone != Tone::Ok || !f.listed,
        };
        if !shown {
            continue;
        }
        let fill = match row.tone {
            Tone::Ok => STEP,
            Tone::Late => LINK_LATE,
            Tone::Loop => LINK_LOOP,
        };
        // No bigger than the field, so a small one keeps its contents in view.
        let radius = (8.0 * zoom).clamp(6.5, 10.0).min((r.height() * 0.6).max(5.0));
        let c = r.left_top();
        painter.circle(c, radius, fill, Stroke::new(1.5, Color32::WHITE));
        painter.text(c, Align2::CENTER_CENTER, (k + 1).to_string(), theme::semibold(radius * 1.1), Color32::WHITE);
    }
    hovered
}

/// Where a ray from `r`'s centre in direction `dir` (a unit vector) leaves `r`.
fn edge(r: Rect, dir: Vec2) -> Pos2 {
    let half = r.size() / 2.0;
    let along = |h: f32, d: f32| if d.abs() > 1e-6 { h / d.abs() } else { f32::INFINITY };
    r.center() + dir * along(half.x, dir.x).min(along(half.y, dir.y))
}

/// An arrow from field `a` to field `b`: dashed when the value arrives too late, red in a loop.
/// Fields one above the other link out to the right, and fields side by side over the top, clear
/// of the fields between them (a grid's neighbours would otherwise get arrows too short to see).
/// Other fields link between their nearest edges, bent a little to the arrow's left so the two
/// arrows of a loop don't overlap. A field that reads itself gets a loop off its right edge.
fn arrow(painter: &egui::Painter, a: Rect, b: Rect, tone: Tone, zoom: f32) {
    let colour = match tone {
        Tone::Ok => LINK_OK,
        Tone::Late => LINK_LATE,
        Tone::Loop => LINK_LOOP,
    };
    let stroke = Stroke::new(1.75, colour);
    let head = 9.0;
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
        let (start, end) = (edge(a, dir) + dir * 2.0, edge(b, -dir) - dir * 2.0);
        let bend = vec2(dir.y, -dir.x) * ((end - start).length() * 0.18).min(60.0);
        (start, start + (end - start) / 3.0 + bend, start + (end - start) * (2.0 / 3.0) + bend, end)
    };
    // The line stops where the head starts, which points along the curve's last stretch.
    let back = (end - c2).normalized();
    let back = if back.is_finite() && back != Vec2::ZERO { back } else { vec2(-1.0, 0.0) };
    let base = end - back * head;
    let curve = egui::epaint::CubicBezierShape::from_points_stroke([start, c1, c2, base], false, Color32::TRANSPARENT, stroke);
    let points: Vec<Pos2> = curve.flatten(Some(0.5));
    if tone == Tone::Late {
        painter.extend(Shape::dashed_line(&points, stroke, 5.0, 4.0));
    } else {
        painter.add(Shape::line(points, stroke));
    }
    let side = vec2(-back.y, back.x) * 4.5;
    painter.add(Shape::convex_polygon(vec![end, base + side, base - side], colour, Stroke::NONE));
}

/// The focused field's links in words, in a pill at the top of the page view.
pub(crate) fn paint_trace(ui: &egui::Ui, painter: &egui::Painter, viewport: Rect, view: &CalcOrderView, generation: u64, selected: Option<&str>) {
    if view.hide_links {
        return;
    }
    let (Some(listed), Some(focus)) = (view.current(generation), view.focus(selected)) else { return };
    let t = Tokens::get(ui.ctx());
    let text = trace(&listed.links, &focus);
    let font = theme::regular(12.0);
    // Clear of the floating tool strip at the view's left.
    let room = (viewport.width() - 220.0).max(0.0);
    let shown = widgets::fit_line(&text, room, |s| widgets::text_width(ui, s, &font));
    if shown.is_empty() {
        return;
    }
    let w = widgets::text_width(ui, &shown, &font) + 46.0;
    let pill = Rect::from_center_size(pos2(viewport.center().x, viewport.top() + 26.0), vec2(w, 30.0));
    painter.rect(pill, CornerRadius::same(15), t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui, Rect::from_center_size(pill.left_center() + vec2(18.0, 0.0), vec2(14.0, 14.0)), "move-right", 13.0, LINK_OK);
    painter.text(pill.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, shown, font, t.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, reads: &[&str], issues: Vec<CalculationIssue>) -> CalculatedField {
        CalculatedField { name: name.into(), reads: reads.iter().map(|r| (*r).to_owned()).collect(), listed: true, issues }
    }

    #[test]
    fn a_name_list_reads_as_a_sentence_and_stops_after_six() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        assert_eq!(names(&[]), "");
        assert_eq!(names(&["a"]), "a");
        assert_eq!(names(&["a", "b"]), "a and b");
        assert_eq!(names(&["a", "b", "c"]), "a, b and c");
        assert_eq!(names(&["1", "2", "3", "4", "5", "6", "7"]), "1, 2, 3, 4, 5, 6, …");
    }

    fn link(from: &str, to: &str, tone: Tone) -> Link {
        Link { from: from.into(), to: to.into(), tone }
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

    #[test]
    fn links_stop_at_the_cap() {
        let names: Vec<String> = (0..MAX_LINKS + 10).map(|k| format!("g.{k}")).collect();
        let calculated = [field("sum", &["g"], vec![])];
        assert_eq!(links(&calculated, names.iter().map(String::as_str).collect()).len(), MAX_LINKS);
    }

    #[test]
    fn the_summary_says_what_a_field_reads_and_what_reads_it() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let links = [link("sub", "total", Tone::Late), link("tax", "total", Tone::Ok), link("qty", "sub", Tone::Ok), link("sub", "tax", Tone::Ok)];
        assert_eq!(trace(&links, "total"), "total reads sub and tax · No field reads it");
        assert_eq!(trace(&links, "sub"), "sub reads qty · total and tax read it");
        assert_eq!(trace(&links, "tax"), "tax reads sub · total reads it");
        assert_eq!(trace(&links, "qty"), "qty reads no other field · sub reads it");
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
}
