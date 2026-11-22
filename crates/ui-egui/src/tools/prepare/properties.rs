//! The Field Properties dialog's body: the tab strip, and the General, Appearance, Position and
//! Options tabs. The Format, Validate, Calculate and Actions tabs are in `tabs`.

use std::ops::RangeInclusive;

use craft::behavior::{SelectionChange, Value, keyed};
use craft::theme::variant::Tone;
use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, CollectionEvent, Entry, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _, Sizing};
use craft::ui::display::{Shade, Text};
use craft::ui::form::{Field, FieldGroup};
use craft::ui::layout_components::Stack;
use craft::ui::list::{List, ListRow, RowEvent, TrailingFace, TrailingPart};
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Label;
use craft::ui::runtime::requests::ColorRequest;
use craft::ui::tabs::{Tab, TabKind, TabStrip};
use craft::ui::text::{NumberField, TextField};

use super::draft::{FieldDraft, FieldTab};
use super::tabs::{actions_tab, calculate_tab, format_tab, validate_tab};
use super::{BorderStyle, FieldFont, FormFieldKind};
use crate::comments::colours::{RecentColours, field_choice, palette};

/// The Delete part of an item row of a choice field.
const DELETE: TrailingPart = TrailingPart::new(0);
/// The Up part of an item row of a choice field.
const UP: TrailingPart = TrailingPart::new(1);

/// What the body shares with the dialog that owns it.
pub(super) struct Shared<'a> {
    /// The custom colours picked this session, for the colour rows.
    pub(super) recent: &'a RecentColours,
    /// Where a colour row parks the "Other colour…" request: the dialog has no app to open a picker with.
    pub(super) pending_colour: &'a mut Option<ColorRequest>,
    /// Why the app refused the last OK, shown until the values change.
    pub(super) refusal: Option<&'a Label>,
    /// The OK is filed and waits for the app's answer: nothing more can be accepted.
    pub(super) waiting: bool,
}

/// A `/Ff` bit as a check box reads it: `inverted` is checked while the bit is clear.
#[derive(Clone, Copy)]
struct FlagBit {
    bit: u32,
    inverted: bool,
}

impl FlagBit {
    fn get(self, flags: u32) -> bool {
        (flags & self.bit != 0) != self.inverted
    }

    fn set(self, flags: u32, on: bool) -> u32 {
        if on != self.inverted { flags | self.bit } else { flags & !self.bit }
    }
}

/// A check box; whether the user changed it.
pub(super) fn check(ui: &mut egui::Ui, label: &str, value: &mut bool, enabled: bool) -> bool {
    Checkbox::new(Bind::bound(&mut *value)).label(label.to_owned()).enabled(enabled).show(ui).events.committed().is_some()
}

/// A check box for a `/Ff` bit.
fn flag_box(ui: &mut egui::Ui, d: &mut FieldDraft, bit: u32, inverted: bool, label: &str) {
    let flag = FlagBit { bit, inverted };
    let mut on = flag.get(d.flags);
    if check(ui, label, &mut on, true) {
        d.flags = flag.set(d.flags, on);
    }
}

/// A labelled one-line text field.
pub(super) fn text_row(ui: &mut egui::Ui, salt: &'static str, label: &str, value: &mut String) {
    Field::new(label.to_owned()).id_salt(salt).show(ui, |ui, _| {
        TextField::new(label.to_owned(), Bind::bound(&mut *value)).show(ui);
    });
}

/// A labelled multi-line field for script text: monospaced, and Tab types a tab.
pub(super) fn code_row(ui: &mut egui::Ui, salt: &'static str, label: &str, value: &mut String, lines: usize) {
    Field::new(label.to_owned()).id_salt(salt).show(ui, |ui, _| {
        TextField::new(label.to_owned(), Bind::bound(&mut *value)).lines(lines).mono().tab_inserts(true).show(ui);
    });
}

/// A labelled choice among `entries`.
pub(super) fn select_row<T: Value>(ui: &mut egui::Ui, salt: &'static str, label: &str, value: &mut T, entries: &[Entry<T>]) {
    Field::new(label.to_owned()).id_salt(salt).show(ui, |ui, _| {
        Select::new(label.to_owned(), Bind::bound(&mut *value), entries).show(ui);
    });
}

/// A labelled number in `range`; an emptied field leaves the value as it was.
pub(super) fn number_row(
    ui: &mut egui::Ui,
    salt: &'static str,
    label: &str,
    value: &mut f64,
    range: RangeInclusive<f64>,
    unit: Option<&str>,
    enabled: bool,
) {
    let mut shown = Some(*value);
    Field::new(label.to_owned()).id_salt(salt).show(ui, |ui, _| {
        let mut field = NumberField::new(label.to_owned(), Bind::bound(&mut shown)).range(range.clone());
        if let Some(unit) = unit {
            field = field.unit(unit.to_owned());
        }
        field.enabled(enabled).show(ui);
    });
    if let Some(v) = shown.filter(|v| v.is_finite()) {
        *value = v;
    }
}

/// A labelled whole number in `range`.
pub(super) fn count_row(ui: &mut egui::Ui, salt: &'static str, label: &str, value: &mut usize, range: RangeInclusive<usize>, enabled: bool) {
    let mut shown = *value as f64;
    number_row(ui, salt, label, &mut shown, (*range.start() as f64)..=(*range.end() as f64), None, enabled);
    *value = shown.max(0.0) as usize;
}

/// Text of the muted shade.
pub(super) fn note(ui: &mut egui::Ui, text: impl Into<Label>) {
    Text::new(text).shade(Shade::Muted).show(ui);
}

/// The entries of a choice over `values`, each named by `name`.
fn entries<T: Value + Copy>(values: &[T], name: impl Fn(T) -> String) -> Vec<Entry<T>> {
    values.iter().map(|v| Entry::new(*v, name(*v))).collect()
}

/// Draws Field Properties; returns whether OK can be accepted (it cannot while a filed OK waits for the app).
pub(super) fn body(ui: &mut egui::Ui, d: &mut FieldDraft, shared: Shared<'_>) -> bool {
    let Shared { recent, pending_colour, refusal, waiting } = shared;
    let tabs = d.tabs();
    let keys: Vec<FieldTab> = tabs.iter().map(|(tab, _)| *tab).collect();
    let source = keyed(&keys, |key: &FieldTab| *key);
    let strip = TabStrip::new(tl!("Field properties"))
        .kind(TabKind::Panel)
        .shown(d.tab)
        .show_items(ui, &source, &mut |key, _| Tab::new(tabs.iter().find(|(tab, _)| tab == key).map_or("", |(_, label)| tl!(label))));
    for event in strip.response.events {
        if let CollectionEvent::Selection(SelectionChange::Set(picked)) = event
            && let Some(tab) = picked.last()
        {
            d.tab = *tab;
        }
    }
    if let Some(tab) = strip.picked {
        d.tab = tab;
    }
    // A locked field keeps its properties until it is unlocked (Acrobat greys them out).
    let locked = d.is_locked();
    if locked {
        Notice::new(Tone::Info, tl!("This field is locked. Unlock it to change its properties.")).layout(NoticeLayout::Inline).show(ui);
    }
    Stack::new().enabled(!locked).show(ui, |ui, _| match d.tab {
        FieldTab::General => general_tab(ui, d),
        FieldTab::Appearance => appearance_tab(ui, d, recent, pending_colour),
        FieldTab::Position => position_tab(ui, d),
        FieldTab::Format => format_tab(ui, d),
        FieldTab::Validate => validate_tab(ui, d),
        FieldTab::Calculate => calculate_tab(ui, d),
        FieldTab::Actions => actions_tab(ui, d),
        FieldTab::Options => options_tab(ui, d),
    });
    // Comb holds whichever tab set it.
    d.settle_comb();

    if let Some(reason) = refusal {
        Notice::new(Tone::Bad, reason.clone()).layout(NoticeLayout::Inline).show(ui);
    }
    !waiting
}

fn general_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    FieldGroup::new(tl!("General")).show(ui, |ui, _| {
        text_row(ui, "name", tl!("Name:"), &mut d.name);
        text_row(ui, "tooltip", tl!("Tooltip:"), &mut d.tooltip);
    });
    Text::new(tl!("Common Properties")).show(ui);
    check(ui, tl!("Read Only"), &mut d.read_only, true);
    check(ui, tl!("Required"), &mut d.required, true);
}

/// A colour row's request for the colour picker is kept for the dialog's owner to open.
fn park(pending: &mut Option<ColorRequest>, request: Option<ColorRequest>) {
    if request.is_some() {
        *pending = request;
    }
}

fn appearance_tab(ui: &mut egui::Ui, d: &mut FieldDraft, recent: &RecentColours, pending: &mut Option<ColorRequest>) {
    let mut auto = d.font_size == 0.0;
    if check(ui, tl!("Auto"), &mut auto, true) {
        d.font_size = if auto { 0.0 } else { 12.0 };
    }
    number_row(ui, "font-size", tl!("Font Size:"), &mut d.font_size, 2.0..=100.0, Some(tl!("pt")), !auto);
    let Some(l) = d.look.as_mut() else { return };
    let fonts = entries(&FieldFont::ALL, |f| tl!(f.label()).to_string());
    select_row(ui, "font", tl!("Font:"), &mut l.font, &fonts);
    FieldGroup::new(tl!("Text Color:")).id_salt("text-colour").show(ui, |ui, _| {
        let pick = palette(ui, tl!("Text Color:"), Some(l.text), "field.props.text", recent);
        if let Some(c) = pick.picked {
            l.text = c;
        }
        park(pending, pick.request);
    });
    Text::new(tl!("Borders and Colors")).show(ui);
    FieldGroup::new(tl!("Border Color:")).id_salt("border-colour").show(ui, |ui, _| {
        let pick = field_choice(ui, l.border, false, "field.props.border", recent);
        if let Some(c) = pick.choice {
            l.border = c;
        }
        park(pending, pick.request);
    });
    FieldGroup::new(tl!("Fill Color:")).id_salt("fill-colour").show(ui, |ui, _| {
        let pick = field_choice(ui, l.fill, false, "field.props.fill", recent);
        if let Some(c) = pick.choice {
            l.fill = c;
        }
        park(pending, pick.request);
    });
    let widths = [(1.0, tl!("Thin")), (2.0, tl!("Medium")), (3.0, tl!("Thick"))].map(|(w, name)| Entry::new(w, name));
    select_row(ui, "width", tl!("Line Thickness:"), &mut l.width, &widths);
    let styles = entries(&BorderStyle::ALL, |s| tl!(s.label()).to_string());
    select_row(ui, "style", tl!("Line Style:"), &mut l.style, &styles);
}

fn position_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    for (i, (salt, label)) in
        [("left", tl!("Left:")), ("bottom", tl!("Bottom:")), ("width", tl!("Width:")), ("height", tl!("Height:"))].into_iter().enumerate()
    {
        let range = if i < 2 { -14_400.0..=14_400.0 } else { 4.0..=14_400.0 };
        number_row(ui, salt, label, &mut d.position[i], range, Some(tl!("pt")), true);
    }
    note(ui, tl!("Points from the page's bottom-left corner."));
    // The field keeps its place and size: the rotation turns its content (#4296).
    let turns = entries(&[0_i64, 90, 180, 270], |r| format!("{r}°"));
    select_row(ui, "rotation", tl!("Rotation:"), &mut d.rotation, &turns);
}

/// The Alignment choice over `/Q`.
fn alignment(ui: &mut egui::Ui, quadding: &mut i64) {
    let sides = [(0_i64, tl!("Left")), (1, tl!("Center")), (2, tl!("Right"))].map(|(q, name)| Entry::new(q, name));
    let mut shown = (*quadding).clamp(0, 2);
    select_row(ui, "quadding", tl!("Alignment:"), &mut shown, &sides);
    *quadding = shown;
}

fn options_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    use pdfcraft_engine::field_flags as ff;
    match d.kind {
        FormFieldKind::PushButton => text_row(ui, "caption", tl!("Label"), &mut d.caption),
        FormFieldKind::Text => {
            alignment(ui, &mut d.quadding);
            text_row(ui, "default", tl!("Default Value:"), &mut d.default);
            check(ui, tl!("Multi-line"), &mut d.multiline, true);
            flag_box(ui, d, ff::DO_NOT_SCROLL, true, tl!("Scroll long text"));
            flag_box(ui, d, ff::RICH_TEXT, false, tl!("Allow Rich Text Formatting"));
            check(ui, tl!("Limit of"), &mut d.limit, true);
            count_row(ui, "limit", tl!("Limit characters:"), &mut d.max_len, 0..=10_000, d.limit);
            flag_box(ui, d, ff::PASSWORD, false, tl!("Password"));
            flag_box(ui, d, ff::FILE_SELECT, false, tl!("Field is used for file selection"));
            flag_box(ui, d, ff::DO_NOT_SPELL_CHECK, true, tl!("Check spelling"));
            flag_box(ui, d, ff::COMB, false, tl!("Comb of"));
            count_row(ui, "comb", tl!("Comb characters:"), &mut d.max_len, 1..=500, d.flags & ff::COMB != 0);
        }
        FormFieldKind::CheckBox | FormFieldKind::Radio => {
            let on = d.on_state.clone();
            text_row(ui, "export", tl!("Export value"), &mut d.export_value);
            let mut checked = !d.default.is_empty() && d.default == on;
            let label = if d.kind == FormFieldKind::CheckBox { tl!("Check box is checked by default") } else { tl!("Button is checked by default") };
            if check(ui, label, &mut checked, true) {
                d.default = if checked { on } else { String::new() };
            }
            if let Some(style) = d.check_style.as_mut() {
                let styles = entries(&pdfcraft_engine::CheckStyle::ALL, |s| tl!(s.label()).to_string());
                let label = if d.kind == FormFieldKind::CheckBox { tl!("Check Box Style:") } else { tl!("Button Style:") };
                select_row(ui, "check-style", label, style, &styles);
            }
            if d.kind == FormFieldKind::Radio {
                flag_box(ui, d, ff::RADIOS_IN_UNISON, false, tl!("Buttons with the same name and value are selected in unison"));
            }
        }
        _ => choice_options(ui, d),
    }
}

/// The Options tab of a combo or list box: alignment, the items and what is chosen from them.
fn choice_options(ui: &mut egui::Ui, d: &mut FieldDraft) {
    use pdfcraft_engine::field_flags as ff;
    alignment(ui, &mut d.quadding);
    // PLAN: Enter in the Item field is the dialog's OK (`FormDialog` takes Enter), so an item is added by the Add button only.
    text_row(ui, "item", tl!("Item:"), &mut d.new_option);
    let typed = d.new_option.trim().to_string();
    let add = craft::ui::contract::clicked(&craft::ui::buttons::Button::new(tl!("Add")).enabled(!typed.is_empty()).show(ui));
    if add && !typed.is_empty() {
        d.options.push(typed);
        d.new_option.clear();
    }
    Text::new(tl!("Item List")).show(ui);
    let rows: Vec<usize> = (0..d.options.len()).collect();
    let source = keyed(&rows, |row: &usize| *row);
    let options = &d.options;
    let list = List::<usize>::new(tl!("Item List")).select(SelectMode::None).sizing(Sizing::Content).natural_height(true).show_items(
        ui,
        &source,
        &mut |row, _| {
            let name = options.get(*row).cloned().unwrap_or_default();
            let delete = Label::from(crate::i18n::fmt(tl!("Delete {o}"), &[("o", name.as_str())]));
            let mut item = ListRow::new(name).trailing_part(DELETE, TrailingFace::Strong(tl!("Delete").into()), delete);
            if *row > 0 {
                item = item.trailing_part(UP, TrailingFace::Strong(tl!("Up").into()), tl!("Up"));
            }
            item
        },
    );
    let (mut remove, mut up) = (None, None);
    for event in list.events {
        if let CollectionEvent::Item(row, RowEvent::Trailing { part, .. }) = event {
            match part {
                DELETE => remove = Some(row),
                UP => up = Some(row),
                _ => {}
            }
        }
    }
    if let Some(i) = remove.filter(|i| *i < d.options.len()) {
        d.options.remove(i);
    } else if let Some(i) = up.filter(|i| *i > 0 && *i < d.options.len()) {
        d.options.swap(i - 1, i);
    }
    if d.options.is_empty() {
        note(ui, tl!("Add the choices people pick from."));
    }
    let mut chosen = d.options.iter().map(|o| Entry::new(o.clone(), o.clone())).collect::<Vec<_>>();
    chosen.insert(0, Entry::new(String::new(), tl!("(none)")));
    select_row(ui, "default", tl!("Default:"), &mut d.default, &chosen);
    flag_box(ui, d, ff::SORT, false, tl!("Sort items"));
    if d.kind == FormFieldKind::Combo {
        flag_box(ui, d, ff::EDIT, false, tl!("Allow user to enter custom text"));
        flag_box(ui, d, ff::DO_NOT_SPELL_CHECK, true, tl!("Check spelling"));
    } else {
        flag_box(ui, d, ff::MULTI_SELECT, false, tl!("Multiple selection"));
    }
    flag_box(ui, d, ff::COMMIT_ON_SEL_CHANGE, false, tl!("Commit selected value immediately"));
}
