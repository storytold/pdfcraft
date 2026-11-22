//! Shared Field Properties for Prepare a form. Each property is opt-in, and appearance
//! changes are merged per field so a mixed selection keeps everything the user did not edit.

use std::borrow::Cow;

use craft::behavior::{Mixed, SelectionChange, Value, keyed};
use craft::theme::Srgba;
use craft::theme::variant::Tone;
use craft::ui::contract::{Bind, CollectionEvent, Entry, Props as _, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, HeldAnswer, HeldOk, StackDialog, dialog_id};
use craft::ui::display::Text;
use craft::ui::layout_components::Stack;
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Label;
use craft::ui::runtime::overlay::CloseReason;
use craft::ui::runtime::requests::ColorRequest;
use craft::ui::sections::GroupBox;
use craft::ui::tabs::{Tab as TabItem, TabKind, TabStrip};
use craft::ui::text::{NumberField, TextField};
use egui::{Context, Ui};
use pdfcraft_engine::{BorderStyle, CheckStyle, DocId, Edit, FieldFont, FieldLookPatch, FieldProps, FormFieldKind, field_flags};
use serde_json::{Value as Json, json};

use crate::comments::colours::{self, RecentColours};
use crate::{PdfCraftApp, prepare};

/// One property shown by the dialog. `apply` is explicit: merely opening the dialog, or
/// visiting a tab, must never turn mixed values into the first field's value.
#[derive(Clone, Debug, PartialEq)]
pub struct Change<T> {
    pub value: T,
    pub mixed: bool,
    pub apply: bool,
}

impl<T: Clone + PartialEq> Change<T> {
    fn from_values(values: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut values = values.into_iter();
        let value = values.next()?;
        let mixed = values.any(|other| other != value);
        Some(Self { value, mixed, apply: false })
    }

    fn changed(&self, current: &T) -> Option<T> {
        (self.apply && current != &self.value).then(|| self.value.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    General,
    Appearance,
    Options,
}

/// A dialog is bound to its source document, not whichever tab happens to be active later.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub doc: DocId,
    pub names: Vec<String>,
    pub tab: Tab,
    pub tooltip: Change<String>,
    pub read_only: Change<bool>,
    pub required: Change<bool>,
    pub locked: Change<bool>,
    pub font_size: Change<f64>,
    pub font: Change<FieldFont>,
    /// The original resource can be a custom font outside the three built-in choices.
    pub font_label: String,
    pub text: Change<[f64; 3]>,
    pub border: Change<Option<[f64; 3]>>,
    pub fill: Change<Option<[f64; 3]>>,
    pub width: Change<f64>,
    pub style: Change<BorderStyle>,
    pub align: Change<i64>,
    pub multiline: Change<bool>,
    /// Zero means no limit, as in the headless form tool.
    pub max_length: Change<usize>,
    pub check_style: Change<CheckStyle>,
    /// A flag, its label, whether the UI wording is inverted, and its proposed value.
    pub flags: Vec<(u32, &'static str, bool, Change<bool>)>,
    text_fields: bool,
    choice_fields: bool,
    check_fields: bool,
    text_appearance: bool,
}

/// A property no field has been compared for yet: off, so it applies nothing.
fn unset<T>(value: T) -> Change<T> {
    Change { value, mixed: false, apply: false }
}

/// The empty draft the form dialog's value type needs; a real one comes from [`Draft::new`].
impl Default for Draft {
    fn default() -> Self {
        Draft {
            doc: DocId(0),
            names: Vec::new(),
            tab: Tab::General,
            tooltip: unset(String::new()),
            read_only: unset(false),
            required: unset(false),
            locked: unset(false),
            font_size: unset(0.0),
            font: unset(FieldFont::Helvetica),
            font_label: String::new(),
            text: unset([0.0; 3]),
            border: unset(None),
            fill: unset(None),
            width: unset(1.0),
            style: unset(BorderStyle::Solid),
            align: unset(0),
            multiline: unset(false),
            max_length: unset(0),
            check_style: unset(CheckStyle::Check),
            flags: Vec::new(),
            text_fields: false,
            choice_fields: false,
            check_fields: false,
            text_appearance: false,
        }
    }
}

impl Draft {
    /// The draft for editing `names` together, or why they can't be (a message for `tl!`).
    pub fn new(doc: &pdfcraft_engine::Document, names: Vec<String>) -> Result<Self, &'static str> {
        const MISSING: &str = "The selected fields could not be found. Select them again.";
        let fields = names.iter().map(|name| doc.form.iter().find(|f| &f.name == name)).collect::<Option<Vec<_>>>().ok_or(MISSING)?;
        let looks = names
            .iter()
            .map(|name| doc.field_look(name))
            .collect::<Option<Vec<_>>>()
            .ok_or("A selected field has no widget on any page, so the fields can't be edited together.")?;
        if fields.len() < 2 {
            return Err(MISSING);
        }
        Self::build(doc, names, &fields, &looks).ok_or(MISSING)
    }

    fn build(
        doc: &pdfcraft_engine::Document,
        names: Vec<String>,
        fields: &[&pdfcraft_engine::FormField],
        looks: &[pdfcraft_engine::FieldLook],
    ) -> Option<Self> {
        let text_fields = fields.iter().all(|f| f.kind == FormFieldKind::Text);
        let choice_fields = fields.iter().all(|f| matches!(f.kind, FormFieldKind::Combo | FormFieldKind::List));
        let check_fields = fields.iter().all(|f| matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio));
        let text_appearance =
            fields.iter().all(|f| matches!(f.kind, FormFieldKind::Text | FormFieldKind::Combo | FormFieldKind::List | FormFieldKind::PushButton));
        let mut flag_defs = Vec::new();
        if text_fields {
            flag_defs.extend([
                (field_flags::DO_NOT_SCROLL, "Scroll long text", true),
                (field_flags::DO_NOT_SPELL_CHECK, "Check spelling", true),
                (field_flags::PASSWORD, "Password", false),
                (field_flags::COMB, "Comb of characters", false),
                (field_flags::FILE_SELECT, "Field is used for file selection", false),
            ]);
        } else if choice_fields {
            flag_defs
                .extend([(field_flags::SORT, "Sort items", false), (field_flags::COMMIT_ON_SEL_CHANGE, "Commit selected value immediately", false)]);
            if fields.iter().all(|f| f.kind == FormFieldKind::Combo) {
                flag_defs.push((field_flags::EDIT, "Allow user to enter custom text", false));
            } else if fields.iter().all(|f| f.kind == FormFieldKind::List) {
                flag_defs.push((field_flags::MULTI_SELECT, "Multiple selection", false));
            }
        }
        let flags = flag_defs
            .into_iter()
            .map(|(bit, label, inverted)| Change::from_values(fields.iter().map(|f| f.has(bit) != inverted)).map(|c| (bit, label, inverted, c)))
            .collect::<Option<Vec<_>>>()?;
        let mut font = Change::from_values(looks.iter().map(|l| l.font))?;
        let resource = prepare::da_font(&fields.first()?.da);
        font.mixed |= fields.iter().any(|f| prepare::da_font(&f.da) != resource);
        let font_label = if font.mixed {
            "Mixed".to_string()
        } else if ["Helv", "Helvetica", "TiRo", "Times-Roman", "Cour", "Courier"].contains(&resource) {
            font.value.label().to_string()
        } else {
            "Custom font".to_string()
        };
        Some(Self {
            doc: doc.id,
            names,
            tab: Tab::General,
            tooltip: Change::from_values(fields.iter().map(|f| f.tooltip.clone().unwrap_or_default()))?,
            read_only: Change::from_values(fields.iter().map(|f| f.read_only()))?,
            required: Change::from_values(fields.iter().map(|f| f.has(field_flags::REQUIRED)))?,
            locked: Change::from_values(fields.iter().map(|f| f.locked()))?,
            font_size: Change::from_values(fields.iter().map(|f| prepare::da_size(&f.da)))?,
            font,
            font_label,
            text: Change::from_values(looks.iter().map(|l| l.text))?,
            border: Change::from_values(looks.iter().map(|l| l.border))?,
            fill: Change::from_values(looks.iter().map(|l| l.fill))?,
            width: Change::from_values(looks.iter().map(|l| l.width))?,
            style: Change::from_values(looks.iter().map(|l| l.style))?,
            align: Change::from_values(fields.iter().map(|f| f.quadding))?,
            multiline: Change::from_values(fields.iter().map(|f| f.has(field_flags::MULTILINE)))?,
            max_length: Change::from_values(fields.iter().map(|f| f.max_len.unwrap_or(0)))?,
            check_style: Change::from_values(fields.iter().map(|f| doc.field_check_style(&f.name).unwrap_or(CheckStyle::Check)))?,
            flags,
            text_fields,
            choice_fields,
            check_fields,
            text_appearance,
        })
    }

    /// Resolve only the chosen changes against the current document. This also preserves
    /// unrelated appearance changes made since the dialog opened (e.g. through UI control).
    pub fn edits(&self, doc: &pdfcraft_engine::Document) -> Result<Vec<Edit>, String> {
        if doc.id != self.doc {
            return Err("The document changed while you were editing field properties.".into());
        }
        let mut edits = Vec::new();
        for name in &self.names {
            let f = doc
                .form
                .iter()
                .find(|f| &f.name == name)
                .ok_or_else(|| crate::i18n::fmt(crate::i18n::t("The field {name} no longer exists."), &[("name", name)]))?;
            let patch = FieldLookPatch {
                border: self.border.apply.then_some(self.border.value),
                fill: self.fill.apply.then_some(self.fill.value),
                width: self.width.apply.then_some(self.width.value),
                style: self.style.apply.then_some(self.style.value),
                text: (self.text_appearance && self.text.apply).then_some(self.text.value),
                font: (self.text_appearance && self.font.apply).then_some(self.font.value),
            };
            let props = FieldProps {
                tooltip: self.tooltip.changed(&f.tooltip.clone().unwrap_or_default()),
                read_only: self.read_only.changed(&f.read_only()),
                required: self.required.changed(&f.has(field_flags::REQUIRED)),
                locked: self.locked.changed(&f.locked()),
                appearance: (patch != FieldLookPatch::default()).then_some(patch),
                font_size: (self.text_appearance && self.font_size.apply).then_some(self.font_size.value),
                multiline: self.text_fields.then(|| self.multiline.changed(&f.has(field_flags::MULTILINE))).flatten(),
                max_len: self.text_fields.then(|| self.max_length.changed(&f.max_len.unwrap_or(0))).flatten().map(|v| (v > 0).then_some(v)),
                quadding: (self.text_fields || self.choice_fields).then(|| self.align.changed(&f.quadding)).flatten(),
                check_style: self.check_fields.then(|| self.check_style.changed(&doc.field_check_style(name).unwrap_or(CheckStyle::Check))).flatten(),
                flags: self
                    .flags
                    .iter()
                    .filter_map(|(bit, _, inverted, c)| c.changed(&(f.has(*bit) != *inverted)).map(|v| (*bit, v != *inverted)))
                    .collect(),
                ..Default::default()
            };
            if props != FieldProps::default() {
                edits.push(Edit::SetFieldProps { name: name.clone(), props: Box::new(props) });
            }
        }
        Ok(edits)
    }
}

/// The shared field properties window, held as an open dialog of `app.dialogs`. OK files the draft with the
/// app, which applies it as one edit and either accepts it (the window closes) or refuses it with a reason shown in
/// the window beside everything typed.
pub struct SharedFieldPropsDialog {
    draft: Draft,
    /// The filed draft keeps beside a refusal the values it refused, so the reason shows only until they change.
    held: HeldAnswer<Draft>,
    recent: RecentColours,
    /// The "Other colour..." request a row made this pass, for [`finish`] to open.
    pending_colour: Option<ColorRequest>,
}

impl SharedFieldPropsDialog {
    /// The dialog for editing `names` together, or why they can't be (a message for `tl!`).
    pub fn new(doc: &pdfcraft_engine::Document, names: Vec<String>) -> Result<Self, &'static str> {
        // PLAN: assumed `RecentColours: Default` (pdf-comments-w10); `finish` replaces it with the app's handle each pass.
        Ok(SharedFieldPropsDialog { draft: Draft::new(doc, names)?, held: HeldAnswer::new(), recent: RecentColours::default(), pending_colour: None })
    }

    /// Gives the row that asked with `key` the colour the picker answered.
    pub(crate) fn set_colour(&mut self, key: &str, colour: [f64; 3]) {
        let d = &mut self.draft;
        match key {
            TEXT_KEY => (d.text.value, d.text.mixed) = (colour, false),
            BORDER_KEY => (d.border.value, d.border.mixed) = (Some(colour), false),
            FILL_KEY => (d.fill.value, d.fill.mixed) = (Some(colour), false),
            _ => {}
        }
    }
}

/// The keys of the colour rows, which the colour pickers answer by.
const TEXT_KEY: &str = "bulk.props.text";
const BORDER_KEY: &str = "bulk.props.border";
const FILL_KEY: &str = "bulk.props.fill";

impl StackDialog for SharedFieldPropsDialog {
    type Result = ();

    fn id(&self) -> DialogId {
        dialog_id("shared-field-props")
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<()>> {
        let measuring = craft::ui::runtime::measure::active(ui.ctx());
        let finish = self.held.is_accepted();
        let SharedFieldPropsDialog { draft, held, recent, pending_colour } = self;
        let refusal = held.refusal().filter(|(_, refused)| **refused == *draft).map(|(reason, _)| reason.clone());
        let waiting = held.is_waiting();
        let shown = FormDialog::new(tl!("Shared Field Properties").to_owned(), Bind::bound(&mut *draft))
            .accept_stays_open(true)
            .collection_body(true)
            .ok_enabled(!waiting)
            .show(ui, |ui, cx, values| {
                // A measuring run closes nothing: the real run of the pass makes the close.
                if finish && !measuring {
                    cx.request_close_as(CloseReason::Activated);
                }
                body(ui, values, recent, pending_colour, refusal.as_ref());
                true
            });
        if held.refusal().is_some_and(|(_, refused)| *refused != *draft) {
            held.clear_refusal();
        }
        // The first footer button is OK; a pass without the press files nothing, so a second pass of a frame files once.
        if shown.picked == Some(0) && !waiting {
            held.file(draft.clone());
        }
        match shown.closed {
            Some(CloseReason::Activated) if finish => Some(held.finish().map_or(DialogResult::Cancel, |_| DialogResult::Ok(()))),
            Some(_) => Some(DialogResult::Cancel),
            None => None,
        }
    }

    fn held_state(&self) -> Option<HeldOk> {
        self.held.state()
    }

    /// Files the draft as OK does and answers none until the app accepted it; then it answers.
    fn confirm(&mut self, _: &Context) -> Option<()> {
        if self.held.is_accepted() {
            return self.held.finish().map(|_| ());
        }
        if !self.held.is_waiting() {
            self.held.file(self.draft.clone());
        }
        None
    }

    /// Each property as `{ value, mixed, apply }`, by the name of its row.
    fn json_fields(&self) -> Option<Json> {
        let d = &self.draft;
        fn field<T>(c: &Change<T>, value: impl FnOnce(&T) -> Json) -> Json {
            json!({ "value": value(&c.value), "mixed": c.mixed, "apply": c.apply })
        }
        let mut map = serde_json::Map::new();
        map.insert("tab".into(), json!(format!("{:?}", d.tab)));
        map.insert("fields".into(), json!(d.names));
        map.insert("tooltip".into(), field(&d.tooltip, |v| json!(v)));
        map.insert("read_only".into(), field(&d.read_only, |v| json!(v)));
        map.insert("required".into(), field(&d.required, |v| json!(v)));
        map.insert("locked".into(), field(&d.locked, |v| json!(v)));
        map.insert("font_size".into(), field(&d.font_size, |v| json!(v)));
        map.insert("font".into(), field(&d.font, |v| json!(v.label())));
        map.insert("text".into(), field(&d.text, |v| json!(v)));
        map.insert("border".into(), field(&d.border, |v| json!(v)));
        map.insert("fill".into(), field(&d.fill, |v| json!(v)));
        map.insert("width".into(), field(&d.width, |v| json!(v)));
        map.insert("style".into(), field(&d.style, |v| json!(v.label())));
        map.insert("align".into(), field(&d.align, |v| json!(v)));
        map.insert("multiline".into(), field(&d.multiline, |v| json!(v)));
        map.insert("max_length".into(), field(&d.max_length, |v| json!(v)));
        map.insert("check_style".into(), field(&d.check_style, |v| json!(v.label())));
        for (_, label, _, change) in &d.flags {
            map.insert((*label).to_owned(), field(change, |v| json!(v)));
        }
        Some(Json::Object(map))
    }
}

/// Applies a filed draft to the document the dialog was opened on, once per pass from the dialogs' results: refreshes the
/// recent colours, opens a colour picker a row asked for, and answers the held OK. A refusal keeps the dialog open with
/// everything typed.
pub(crate) fn finish(ctx: &egui::Context, app: &mut PdfCraftApp) {
    let recent = app.recent_colours.clone();
    let Some(dialog) = app.dialogs.get_mut::<SharedFieldPropsDialog>() else { return };
    dialog.recent = recent;
    let request = dialog.pending_colour.take();
    let filed = dialog.held.take();
    if let Some(request) = request {
        let key = request.key.clone();
        app.open_colour_picker(crate::colour_picker::PickerHost::App, request, move |app: &mut PdfCraftApp, _: &egui::Context, colour: Srgba| {
            if let Some(dialog) = app.dialogs.get_mut::<SharedFieldPropsDialog>() {
                dialog.set_colour(&key, colours::to_rgb(colour));
            }
        });
    }
    let Some(draft) = filed else { return };
    let answer = apply(ctx, app, &draft);
    if let Some(dialog) = app.dialogs.get_mut::<SharedFieldPropsDialog>() {
        dialog.held.answer(answer);
    }
}

/// The draft's chosen changes as one edit; an empty draft changes nothing and is accepted.
fn apply(ctx: &egui::Context, app: &mut PdfCraftApp, draft: &Draft) -> Result<(), Label> {
    if app.active_ids().map(|(_, id)| id) != Some(draft.doc) {
        return Err(tl!("The document changed while you were editing field properties.").to_owned().into());
    }
    let Some(doc) = app.session.get(draft.doc) else { return Err(tl!("The document is no longer open.").to_owned().into()) };
    let edits = draft.edits(doc).map_err(Label::from)?;
    if edits.is_empty() {
        return Ok(());
    }
    // `apply_edit` posts its own reason as a notice; the dialog's refusal is this fixed text.
    app.last_edit_refusal = None;
    if app.apply_edit(ctx, Edit::Batch { label: "Change field properties".into(), edits }) {
        Ok(())
    } else {
        // The edit's own reason (a locked field says to unlock it), as the dialog shows it.
        Err(app.last_edit_refusal.take().map_or_else(|| tl!("The field properties could not be changed.").to_owned().into(), Label::from))
    }
}

/// One property: a group box whose check box is `apply`, enclosing the control, which binds the value as a `Mixed`.
/// An edit makes a mixed value concrete. `unknown` shows the control as mixed while the row is not applied, for a value the
/// control cannot show.
fn row<T: Value>(ui: &mut Ui, label: &str, c: &mut Change<T>, unknown: bool, mut control: impl FnMut(&mut Ui, &mut Mixed<T>)) {
    let Change { value, mixed, apply } = c;
    let hidden = unknown && !*apply;
    GroupBox::apply(tl!(label).to_owned(), Bind::bound(apply)).show(ui, |ui, _| {
        let mut shown = if *mixed || hidden { Mixed::Mixed } else { Mixed::Same(value.clone()) };
        control(ui, &mut shown);
        if let Mixed::Same(edited) = shown {
            *value = edited;
            *mixed = false;
        }
    });
}

/// A Yes/No choice.
fn yes_no(ui: &mut Ui, label: &str, c: &mut Change<bool>) {
    let entries = [Entry::new(true, tl!("Yes").to_owned()), Entry::new(false, tl!("No").to_owned())];
    row(ui, label, c, false, |ui, shown| {
        Select::new(tl!(label).to_owned(), Bind::bound_mixed(shown), &entries).show(ui);
    });
}

/// A choice among `entries`.
fn choose<T: Value>(ui: &mut Ui, label: &str, c: &mut Change<T>, entries: &[Entry<T>]) {
    row(ui, label, c, false, |ui, shown| {
        Select::new(tl!(label).to_owned(), Bind::bound_mixed(shown), entries).show(ui);
    });
}

/// A number within `range`, converted to and from the property's own type.
fn number<T: Value>(
    ui: &mut Ui,
    label: &str,
    c: &mut Change<T>,
    range: std::ops::RangeInclusive<f64>,
    unit: Option<&'static str>,
    to: fn(&T) -> f64,
    from: fn(f64) -> T,
) {
    row(ui, label, c, false, |ui, shown| {
        let mut typed = match shown {
            Mixed::Same(v) => Mixed::Same(Some(to(v))),
            _ => Mixed::Mixed,
        };
        let mut field = NumberField::new(tl!(label).to_owned(), Bind::bound_mixed(&mut typed)).range(range.clone());
        if let Some(unit) = unit {
            field = field.unit(unit);
        }
        field.show(ui);
        if let Mixed::Same(Some(v)) = typed {
            *shown = Mixed::Same(from(v));
        }
    });
}

/// The colour of the text, a palette whose "Other colour..." asks the app's picker.
fn text_colour(ui: &mut Ui, label: &str, c: &mut Change<[f64; 3]>, recent: &RecentColours, request: &mut Option<ColorRequest>) {
    let Change { value, mixed, apply } = c;
    GroupBox::apply(tl!(label).to_owned(), Bind::bound(apply)).show(ui, |ui, _| {
        let current = (!*mixed).then_some(*value);
        let pick = colours::palette(ui, tl!(label), current, TEXT_KEY, recent);
        if let Some(picked) = pick.picked {
            *value = picked;
            *mixed = false;
        }
        if pick.request.is_some() {
            *request = pick.request;
        }
    });
}

/// A colour that may be none: `choice` is the row's border or fill chooser. Its Other… request is parked in `request`.
fn optional_colour(
    ui: &mut Ui,
    label: &str,
    c: &mut Change<Option<[f64; 3]>>,
    request: &mut Option<ColorRequest>,
    mut choice: impl FnMut(&mut Ui, Option<[f64; 3]>, bool) -> colours::ColourChoice,
) {
    let Change { value, mixed, apply } = c;
    GroupBox::apply(tl!(label).to_owned(), Bind::bound(apply)).show(ui, |ui, _| {
        let current = if *mixed { None } else { *value };
        let picked = choice(ui, current, *mixed);
        if let Some(chosen) = picked.choice {
            *value = chosen;
            *mixed = false;
        }
        if picked.request.is_some() {
            *request = picked.request;
        }
    });
}

/// A note under the rows.
fn note(ui: &mut Ui, text: &str) {
    Text::new(tl!(text).to_owned()).show(ui);
}

/// The tabs and the rows of the open tab. Check a property to apply its value to every selected field; no unchecked
/// value is used.
fn body(ui: &mut Ui, d: &mut Draft, recent: &RecentColours, request: &mut Option<ColorRequest>, refusal: Option<&Label>) {
    Stack::new().show(ui, |ui, _| {
        Text::new(crate::i18n::fmt(tl!("{n} fields selected"), &[("n", &d.names.len().to_string())])).show(ui);
        note(ui, "Only checked properties will change. Mixed means the selected fields have different values.");
        tabs(ui, d);
        match d.tab {
            Tab::General => {
                let c = &mut d.tooltip;
                // A mixed tooltip shows the first field's text, editable, and says it is mixed.
                let was_mixed = c.mixed;
                let mut text = c.value.clone();
                row(ui, "Tooltip:", c, false, |ui, _| {
                    ui.horizontal(|ui| {
                        TextField::new(tl!("Tooltip").to_owned(), Bind::bound(&mut text)).show(ui);
                        if was_mixed {
                            Text::new(tl!("Mixed").to_owned()).show(ui);
                        }
                    });
                });
                if text != c.value {
                    c.value = text;
                    c.mixed = false;
                }
                yes_no(ui, "Read Only", &mut d.read_only);
                yes_no(ui, "Required", &mut d.required);
                yes_no(ui, "Locked", &mut d.locked);
                note(ui, "To change locked fields, also set Locked to No.");
            }
            Tab::Appearance => appearance(ui, d, recent, request),
            Tab::Options => options(ui, d),
        }
        if let Some(reason) = refusal {
            Notice::new(Tone::Bad, reason.clone()).layout(NoticeLayout::Inline).show(ui);
        }
    });
}

/// The strip of tabs; Options only when every selected field has some.
fn tabs(ui: &mut Ui, d: &mut Draft) {
    let options = d.text_fields || d.choice_fields || d.check_fields;
    let items: Vec<(Tab, String)> = [(Tab::General, "General"), (Tab::Appearance, "Appearance"), (Tab::Options, "Options")]
        .into_iter()
        .filter(|(tab, _)| *tab != Tab::Options || options)
        .map(|(tab, label)| (tab, tl!(label).to_owned()))
        .collect();
    let source = keyed(&items[..], |item: &(Tab, String)| item.0).with_text(|item| Cow::Borrowed(item.1.as_str()));
    let strip = TabStrip::new(tl!("Shared Field Properties").to_owned()).kind(TabKind::Text).shown(d.tab).natural_height(true);
    let out = strip.show_items(ui, &source, &mut |key, _| {
        TabItem::new(items.iter().find(|item| item.0 == *key).map_or_else(String::new, |item| item.1.clone()))
    });
    for event in out.response.events {
        if let CollectionEvent::Selection(SelectionChange::Set(keys)) = event
            && let Some(chosen) = keys.last()
        {
            d.tab = *chosen;
        }
    }
}

/// The Appearance tab: font and text colour for fields that show text, then the border and the line.
fn appearance(ui: &mut Ui, d: &mut Draft, recent: &RecentColours, request: &mut Option<ColorRequest>) {
    if d.text_appearance {
        number(ui, "Font Size:", &mut d.font_size, 0.0..=100.0, Some("pt"), |v| *v, |v| v);
        let fonts: Vec<Entry<FieldFont>> = FieldFont::ALL.into_iter().map(|font| Entry::new(font, font.label().to_owned())).collect();
        // A custom font, or fonts that differ, show their name as the placeholder until the row is applied.
        let shown_label = d.font.value.label() != d.font_label;
        let (font, placeholder) = (&mut d.font, d.font_label.clone());
        row(ui, "Font:", font, shown_label, |ui, shown| {
            Select::new(tl!("Font:").to_owned(), Bind::bound_mixed(shown), &fonts).placeholder(placeholder.clone()).show(ui);
        });
        text_colour(ui, "Text Color:", &mut d.text, recent, request);
    }
    optional_colour(ui, "Border Color:", &mut d.border, request, |ui, current, mixed| colours::field_choice(ui, current, mixed, BORDER_KEY, recent));
    optional_colour(ui, "Fill Color:", &mut d.fill, request, |ui, current, mixed| colours::field_choice(ui, current, mixed, FILL_KEY, recent));
    number(ui, "Line Thickness:", &mut d.width, 0.0..=12.0, Some("pt"), |v| *v, |v| v);
    let styles: Vec<Entry<BorderStyle>> = BorderStyle::ALL.into_iter().map(|style| Entry::new(style, tl!(style.label()).to_owned())).collect();
    choose(ui, "Line Style:", &mut d.style, &styles);
    if d.text_appearance {
        note(ui, "Font size 0 uses automatic sizing.");
    }
}

/// The Options tab: what applies to the kind of field selected.
fn options(ui: &mut Ui, d: &mut Draft) {
    if d.text_fields || d.choice_fields {
        let aligns: Vec<Entry<i64>> =
            [(0, "Left"), (1, "Center"), (2, "Right")].into_iter().map(|(v, label)| Entry::new(v, tl!(label).to_owned())).collect();
        choose(ui, "Alignment:", &mut d.align, &aligns);
    }
    if d.text_fields {
        yes_no(ui, "Multi-line", &mut d.multiline);
        number(ui, "Character limit:", &mut d.max_length, 0.0..=1_000_000.0, None, |v| *v as f64, |v| v.round().max(0.0) as usize);
    }
    if d.check_fields {
        let styles: Vec<Entry<CheckStyle>> = CheckStyle::ALL.into_iter().map(|style| Entry::new(style, tl!(style.label()).to_owned())).collect();
        choose(ui, "Button Style:", &mut d.check_style, &styles);
    }
    for (_, label, _, change) in &mut d.flags {
        yes_no(ui, label, change);
    }
    if d.text_fields {
        note(ui, "Character limit 0 means no limit. A comb of characters needs a limit.");
    }
}
