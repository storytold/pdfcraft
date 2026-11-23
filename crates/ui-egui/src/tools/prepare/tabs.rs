use craft::behavior::{Value, keyed};
use craft::theme::variant::Tone;
use craft::ui::buttons::Button;
use craft::ui::choice::{Radio, RadioGroup};
use craft::ui::contract::{
    Bind, CollectionEvent, Entry, FieldLayout, Props as _, Scroll, SelectMode, Show as _, ShowCollection as _, ShowContainer as _, SideLabel, Sizing,
    clicked,
};
use craft::ui::display::Text;
use craft::ui::form::{Field, Help};
use craft::ui::layout_components::Stack;
use craft::ui::list::{List, ListRow, RowEvent, TrailingFace, TrailingPart};
use craft::ui::runtime::env::Label;
use craft::ui::text::TextField;

use super::draft::{ACTION_KINDS, ActionDraft, FieldDraft};
use super::properties::{check, code_row, count_row, note, select_row, text_row};
use super::{CalcOp, Calculate, DATE_PRESETS, Format, TIME_PRESETS, Validate, format_value};

/// The categories of the Format tab.
#[derive(Clone, Copy, PartialEq)]
enum Category {
    None,
    Number,
    Percent,
    Date,
    Time,
    Special,
}

fn category(f: &Format) -> Category {
    match f {
        Format::None => Category::None,
        Format::Number { .. } => Category::Number,
        Format::Percent { .. } => Category::Percent,
        Format::Date(_) => Category::Date,
        Format::Time(_) => Category::Time,
        Format::Special(_) | Format::Mask(_) => Category::Special,
    }
}

const SEPARATORS: [&str; 5] = ["1,234.56", "1234.56", "1.234,56", "1234,56", "1'234.56"];

const NEGATIVES: [&str; 4] = ["-1,234.01", "1,234.01 (red)", "(1,234.01)", "(1,234.01) (red)"];

const SPECIALS: [&str; 5] = ["Zip Code", "Zip Code + 4", "Phone Number", "Social Security Number", "Arbitrary Mask"];

/// The Delete part of an action's row.
const DELETE: TrailingPart = TrailingPart::new(0);
/// The Edit part of a script action's row.
const EDIT: TrailingPart = TrailingPart::new(1);

/// A group of radio buttons over `options`, with the rows of `after(ui, value)` shown under the button of the
/// value they belong to; the group's value is written back when another is chosen.
fn radios<T: Value + Copy>(ui: &mut egui::Ui, name: &str, value: &mut T, options: &[(T, &str)], mut after: impl FnMut(&mut egui::Ui, T)) {
    let shown = *value;
    RadioGroup::new(name.to_owned(), Bind::bound(value)).show(ui, |ui, _| {
        Stack::new().show(ui, |ui, cx| {
            for (at, (option, label)) in options.iter().enumerate() {
                let side = SideLabel::new().layout(FieldLayout::After);
                cx.labelled(ui, at, Label::from((*label).to_owned()), side, Radio::new(*option));
                if *option == shown {
                    after(ui, *option);
                }
            }
        });
    });
}

/// A choice among the `names`, numbered from 0, over `value`.
fn index_row<T: Value + Copy + TryFrom<usize> + Into<usize>>(ui: &mut egui::Ui, salt: &'static str, label: &str, value: &mut T, names: &[&str]) {
    let entries: Vec<Entry<usize>> = names.iter().enumerate().map(|(i, name)| Entry::new(i, (*name).to_owned())).collect();
    let mut shown: usize = (*value).into();
    select_row(ui, salt, label, &mut shown, &entries);
    if let Ok(v) = T::try_from(shown) {
        *value = v;
    }
}

/// A whole number of `decimals` places.
fn decimals_row(ui: &mut egui::Ui, salt: &'static str, decimals: &mut u8) {
    let mut shown = usize::from(*decimals);
    count_row(ui, salt, tl!("Decimal places:"), &mut shown, 0..=10, true);
    *decimals = u8::try_from(shown).unwrap_or(10);
}

/// Draw the field-format editor for the selected category and its options.
pub(super) fn format_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    let cat = category(&d.format);
    let mut picked = cat;
    let categories = [
        (Category::None, tl!("None")),
        (Category::Number, tl!("Number")),
        (Category::Percent, tl!("Percentage")),
        (Category::Date, tl!("Date")),
        (Category::Time, tl!("Time")),
        (Category::Special, tl!("Special")),
    ]
    .map(|(c, name)| Entry::new(c, name));
    select_row(ui, "format-cat", tl!("Select format category:"), &mut picked, &categories);
    if picked != cat {
        d.format = match picked {
            Category::None => Format::None,
            Category::Number => Format::Number { decimals: 2, sep: 0, neg: 0, currency: String::new(), prepend: true },
            Category::Percent => Format::Percent { decimals: 2, sep: 0 },
            Category::Date => Format::Date("mm/dd/yyyy".into()),
            Category::Time => Format::Time("HH:MM".into()),
            Category::Special => Format::Special(0),
        };
    }
    let sample = match &d.format {
        Format::Number { .. } | Format::Percent { .. } => "-1234.5",
        Format::Date(_) => "10/1/2026",
        Format::Time(_) => "14:05",
        Format::Special(2) => "5551234567",
        Format::Special(3) => "123456789",
        Format::Special(_) => "123456789",
        _ => "",
    };
    let mut format = d.format.clone();
    match &mut format {
        Format::None => note(ui, tl!("The value is shown as typed.")),
        Format::Number { decimals, sep, neg, currency, prepend } => {
            decimals_row(ui, "fmt-decimals", decimals);
            index_row(ui, "fmt-sep", tl!("Separator style:"), sep, &SEPARATORS);
            let symbols = ["", "$", "€", "£", "¥", "CHF "].map(|c| Entry::new(c.to_string(), if c.is_empty() { tl!("None") } else { c }));
            select_row(ui, "fmt-cur", tl!("Currency symbol:"), currency, &symbols);
            check(ui, tl!("Before the number"), prepend, true);
            index_row(ui, "fmt-neg", tl!("Negative number style:"), neg, &NEGATIVES);
        }
        Format::Percent { decimals, sep } => {
            decimals_row(ui, "fmt-pdecimals", decimals);
            index_row(ui, "fmt-psep", tl!("Separator style:"), sep, &SEPARATORS);
        }
        Format::Date(p) | Format::Time(p) => {
            let presets: Vec<&str> = if picked == Category::Time {
                TIME_PRESETS.to_vec()
            } else {
                DATE_PRESETS.iter().copied().chain(["m/d/yyyy", "mm/dd/yyyy", "yyyy-mm-dd", "dd/mm/yyyy"]).collect()
            };
            let entries: Vec<Entry<String>> = presets.into_iter().map(|pr| Entry::new(pr.to_string(), pr)).collect();
            select_row(ui, "fmt-date", tl!("Format:"), p, &entries);
            text_row(ui, "fmt-custom", tl!("Custom:"), p);
        }
        Format::Special(n) => {
            let mut idx = usize::from(*n);
            special_row(ui, &mut idx);
            if idx == 4 {
                format = Format::Mask("999-999".into());
            } else {
                *n = u8::try_from(idx).unwrap_or(0);
            }
        }
        Format::Mask(m) => {
            let mut idx = 4usize;
            special_row(ui, &mut idx);
            text_row(ui, "fmt-mask", tl!("Mask:"), m);
            note(ui, tl!("9 digit, A letter, O letter or digit, X any character"));
            if idx < 4 {
                format = Format::Special(u8::try_from(idx).unwrap_or(0));
            }
        }
    }
    d.format = format;
    if !sample.is_empty() && d.format != Format::None {
        note(ui, crate::i18n::fmt(tl!("Example: {sample}"), &[("sample", &format_value(&d.format, sample))]));
    }
}

/// The Special format's choice among the zip, phone, social security and mask formats.
fn special_row(ui: &mut egui::Ui, idx: &mut usize) {
    let entries: Vec<Entry<usize>> = SPECIALS.iter().enumerate().map(|(i, s)| Entry::new(i, tl!(s))).collect();
    select_row(ui, "fmt-special", tl!("Special:"), idx, &entries);
}

pub(super) fn validate_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    let mut ranged = matches!(d.validate, Validate::Range { .. });
    let options = [(false, tl!("Field value is not validated")), (true, tl!("Field value is in range:"))];
    radios(ui, tl!("Validation"), &mut ranged, &options, |_, _| {});
    if !ranged {
        d.validate = Validate::None;
        return;
    }
    if d.validate == Validate::None {
        d.validate = Validate::Range { min: Some(0.0), max: Some(100.0) };
    }
    if let Validate::Range { min, max } = &mut d.validate {
        for (salt, label, bound, fallback) in [("min", tl!("From:"), min, 0.0), ("max", tl!("To:"), max, 100.0)] {
            let mut has = bound.is_some();
            check(ui, label, &mut has, true);
            let mut v = bound.unwrap_or(fallback);
            super::properties::number_row(ui, salt, label, &mut v, f64::MIN..=f64::MAX, None, has);
            *bound = has.then_some(v);
        }
    }
}

pub(super) fn actions_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    use pdfcraft_engine::FieldTrigger as T;
    let editing = d.editing.is_some();
    Text::new(if editing { tl!("Edit Action") } else { tl!("Add an Action") }).show(ui);
    // Editing changes the script of one trigger's action: its trigger and kind stay.
    Stack::new().enabled(!editing).show(ui, |ui, _| {
        let triggers: Vec<Entry<T>> = T::ALL.iter().map(|tr| Entry::new(*tr, tl!(tr.label()))).collect();
        select_row(ui, "action-trigger", tl!("Select Trigger:"), &mut d.new_action.trigger, &triggers);
        let kinds: Vec<Entry<usize>> = ACTION_KINDS.iter().enumerate().map(|(i, k)| Entry::new(i, tl!(k))).collect();
        select_row(ui, "action-kind", tl!("Select Action:"), &mut d.new_action.kind, &kinds);
    });
    let a = &mut d.new_action;
    if a.kind == 0 {
        code_row(ui, "action-text", tl!("Script:"), &mut a.text, 3);
    } else {
        let hint = match a.kind {
            1 => "https://…",
            2 => tl!("Fields to reset (comma-separated; empty: all)"),
            3 => tl!("Print, NextPage, PrevPage, FirstPage or LastPage"),
            4 => tl!("Page number"),
            5 | 6 => tl!("Field names (comma-separated)"),
            _ => tl!("URL to submit to"),
        };
        Field::new(tl!("Value:")).id_salt("action-text").show(ui, |ui, _| {
            TextField::new(tl!("Value:"), Bind::bound(&mut a.text)).placeholder(hint).show(ui);
        });
    }
    let commit = clicked(&Button::new(if editing { tl!("Update") } else { tl!("Add") }).show(ui));
    let discard = editing && clicked(&Button::new(tl!("Discard")).show(ui));
    if commit {
        match d.new_action.action() {
            Ok(act) => {
                let tr = d.new_action.trigger;
                d.actions.retain(|(x, _)| *x != tr);
                d.actions.push((tr, act));
                d.actions.sort_by_key(|(x, _)| T::ALL.iter().position(|y| y == x));
                d.new_action.text.clear();
                d.editing = None;
            }
            Err(e) => note(ui, tl!(&e).to_owned()),
        }
    }
    if discard {
        d.new_action = ActionDraft::default();
        d.editing = None;
    }
    Text::new(tl!("Actions")).show(ui);
    if d.actions.is_empty() {
        note(ui, tl!("No actions"));
        return;
    }
    let rows: Vec<usize> = (0..d.actions.len()).collect();
    let source = keyed(&rows, |row: &usize| *row);
    let (actions, chained) = (&d.actions, &d.chained);
    let list = List::<usize>::new(tl!("Actions")).select(SelectMode::None).sizing(Sizing::Content).natural_height(true).show_items(
        ui,
        &source,
        &mut |row, _| {
            let Some((tr, act)) = actions.get(*row) else { return ListRow::new("") };
            let more = chained.iter().find(|(x, _)| x == tr).map_or(0, |(_, n)| *n);
            let summary = if more == 0 {
                act.describe()
            } else {
                let then = crate::i18n::trn(crate::i18n::current(), more as u64, "then {n} more action", "then {n} more actions");
                format!("{}, {then}", act.describe())
            };
            let delete = Label::from(crate::i18n::fmt(tl!("Delete {o}"), &[("o", tl!(tr.label()))]));
            let mut item =
                ListRow::new(format!("{}: {summary}", tl!(tr.label()))).trailing_part(DELETE, TrailingFace::Strong(tl!("Delete").into()), delete);
            if !editing && matches!(act, pdfcraft_engine::FieldAction::JavaScript(_)) {
                item = item.trailing_part(EDIT, TrailingFace::Strong(tl!("Edit").into()), tl!("Edit"));
            }
            item
        },
    );
    let (mut remove, mut edit) = (None, None);
    for event in list.events {
        if let CollectionEvent::Item(row, RowEvent::Trailing { part, .. }) = event {
            match part {
                DELETE => remove = Some(row),
                EDIT => edit = Some(row),
                _ => {}
            }
        }
    }
    if let Some((trigger, text)) = edit.and_then(|i| match d.actions.get(i) {
        Some((tr, pdfcraft_engine::FieldAction::JavaScript(js))) if d.editing.is_none() => Some((*tr, js.clone())),
        _ => None,
    }) {
        d.new_action = ActionDraft { trigger, kind: 0, text };
        d.editing = Some(trigger);
    }
    if let Some(i) = remove.filter(|i| *i < d.actions.len()) {
        if d.actions.get(i).map(|(x, _)| Some(*x)) == Some(d.editing) {
            d.new_action = ActionDraft::default();
            d.editing = None;
        }
        d.actions.remove(i);
    }
}

/// The longest custom calculation script the Calculate tab offers to edit. A file can carry far
/// more (forms read up to 64 MiB of script), and laying that out in a text box every frame would
/// hang the dialog; of a longer one the start is shown, read-only, and the script is kept as it is
/// unless another calculation is chosen. The JavaScript engine runs no script past 256 KiB either.
const MAX_EDITED_SCRIPT: usize = 256 * 1024;

pub(super) fn calculate_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    #[derive(PartialEq, Clone, Copy)]
    enum Mode {
        None,
        Simple,
        Notation,
        Custom,
    }
    // Choosing Custom again brings back the script the field had, not an empty box.
    let custom = |d: &FieldDraft| match d.original.as_ref().as_ref().map(|o| &o.calculate) {
        Some(Calculate::Custom(js)) => Calculate::Custom(js.clone()),
        _ => Calculate::Custom(String::new()),
    };
    let mode = match d.calculate {
        Calculate::None => Mode::None,
        Calculate::Simple { .. } => Mode::Simple,
        Calculate::Notation(_) => Mode::Notation,
        Calculate::Custom(_) => Mode::Custom,
    };
    let mut m = mode;
    let options = [
        (Mode::None, tl!("Value is not calculated")),
        (Mode::Simple, tl!("Value is the")),
        (Mode::Notation, tl!("Simplified field notation:")),
        (Mode::Custom, tl!("Custom calculation script:")),
    ];
    let calculate = &mut d.calculate;
    let (others, refused) = (&d.others, &mut d.script_refused);
    radios(ui, tl!("Calculation"), &mut m, &options, |ui, shown| match (shown, &mut *calculate) {
        (Mode::Simple, Calculate::Simple { op, fields }) => {
            let ops: Vec<Entry<CalcOp>> = CalcOp::ALL.iter().map(|o| Entry::new(*o, tl!(o.label()))).collect();
            select_row(ui, "calc-op", tl!("Operation:"), op, &ops);
            Text::new(tl!("of the following fields:")).show(ui);
            Stack::new().scroll(Scroll::vertical()).show(ui, |ui, _| {
                for name in others {
                    let mut on = fields.contains(name);
                    if check(ui, name, &mut on, true) {
                        if on {
                            fields.push(name.clone());
                        } else {
                            fields.retain(|f| f != name);
                        }
                    }
                }
            });
        }
        (Mode::Notation, Calculate::Notation(expr)) => {
            let label = tl!("Simplified field notation:");
            Field::new(label).id_salt("calc-notation").show(ui, |ui, _| {
                TextField::new(label, Bind::bound(&mut *expr)).lines(2).placeholder("Price * Quantity").show(ui);
            });
            note(ui, tl!("Field names with + - * / and parentheses; put \\ before spaces in names."));
        }
        (Mode::Custom, Calculate::Custom(js)) => custom_script(ui, js, refused),
        _ => {}
    });
    if m != mode {
        d.calculate = match m {
            Mode::None => Calculate::None,
            Mode::Simple => Calculate::Simple { op: CalcOp::Sum, fields: Vec::new() },
            Mode::Notation => Calculate::Notation(String::new()),
            Mode::Custom => custom(d),
        };
    }
    if !matches!(d.calculate, Calculate::Custom(_)) {
        d.script_refused = false;
    }
}

/// The most lines the script box grows to before its text scrolls inside it: about the 180 points production's box
/// stopped at. Without it a long script would grow the box past what any layout can show.
const SCRIPT_LINES: usize = 10;

/// The custom calculation script's box, with the 256 KiB rule on what it edits.
fn custom_script(ui: &mut egui::Ui, js: &mut String, refused: &mut bool) {
    let label = tl!("Custom calculation script:");
    if js.len() > MAX_EDITED_SCRIPT {
        // Too long to edit: its start is shown, read-only (selectable, so it can be copied).
        let kib = (js.len() / 1024).to_string();
        let note_text =
            crate::i18n::fmt(tl!("This script is too long to edit here ({kib} KiB); its start is shown. It is kept as it is."), &[("kib", &kib)]);
        let start = js.get(..js.floor_char_boundary(MAX_EDITED_SCRIPT)).unwrap_or_default().to_string();
        Field::new(label).id_salt("calc-script").help(Help::Text(note_text.into())).show(ui, |ui, _| {
            TextField::new(label, Bind::given(start.clone())).lines(6).max_lines(SCRIPT_LINES).mono().read_only(true).show(ui);
        });
        return;
    }
    // The box edits a copy: an edit that would take the script past the limit (a large paste) is refused and
    // said so, rather than hiding the box the user is typing in. The limit is on bytes (`len`), as the JavaScript
    // engine's is, so `TextField::limit`, which counts characters, is not used.
    // The refusal is the box's help line, under it and in its description, until an edit is taken.
    let mut draft = js.clone();
    let mut field = Field::new(label).id_salt("calc-script");
    if *refused {
        field = field.help(Help::Status(Tone::Bad, tl!("A script edited here can be at most 256 KiB long; that change wasn't made.").into()));
    }
    field.show(ui, |ui, _| {
        TextField::new(label, Bind::bound(&mut draft))
            .lines(6)
            .max_lines(SCRIPT_LINES)
            .mono()
            .tab_inserts(true)
            .placeholder("event.value = …;")
            .show(ui);
    });
    if draft != *js {
        *refused = draft.len() > MAX_EDITED_SCRIPT;
        if !*refused {
            *js = draft;
        }
    }
}
