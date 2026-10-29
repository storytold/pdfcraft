//! pdfcraft-forms — interactive forms (AcroForm, ISO 32000-2 §12.7), execution plan M6.1–M6.2.
//!
//! - [`fields`]: the field tree flattened to terminal fields, each with its widgets (page,
//!   rectangle, on-state), inherited attributes (`/FT`, `/Ff`, `/V`, `/DV`, `/DA`, `/Q`,
//!   `/MaxLen`) and choice options.
//! - [`set_value`]: fill a field. Text and choice fields get new appearance streams
//!   ([`appearance`]); check boxes and radio buttons switch `/V` and each widget's `/AS` between
//!   the states their appearances already define.
//! - [`reset`]: Acrobat's Clear form (back to `/DV`).
//! - [`table`]: a table of text fields, laid out and named before any of it is added.
//!
//! Not yet: JavaScript actions (format, keystroke, validate, calculate — M6.4/M6.5) and rich text
//! values (`/RV`, which is removed when a value is set so it can't contradict `/V`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, PdfString};

mod actions;
pub mod af;
pub mod appearance;
mod author;
mod calc_order;
pub mod detect;
mod scripting;
pub mod table;
pub use actions::{FieldAction, Trigger, field_action_chains, field_actions, set_field_actions};
pub use author::{
    BorderStyle, CheckStyle, FieldFont, FieldProps, Look, LookPatch, MIN_FIELD_SIDE, NewField, add_field, check_style, delete_field,
    delete_field_object, delete_widget, duplicate_field, look, redraw_field, set_button_icon, set_props,
};
pub use calc_order::{CalculatedField, CalculationIssue, CalculationOrder, calculation_order, set_calculation_order};
pub use scripting::{
    FieldChange, FieldEvent, NoScripts, ScriptResult, Scripts, apply_script_changes, document_scripts, document_scripts_named, set_document_script,
    set_field_script,
};

mod appearances;
mod read;
mod tab_order;
mod values;

pub use appearances::{FieldAppearanceCache, declares_pdfa, need_appearances, undrawn_fields, with_field_appearances, with_field_appearances_cached};
pub(crate) use read::page_refs;
pub use read::{Fields, acroform, adopted_page_fields, fields, fields_with_adopted};
#[cfg(test)]
use read::{MAX_SCRIPT_BYTES, PageWalk, enumerate, form_reads};
use read::{script, script_budget};
pub use tab_order::{TabOrder, move_in_tab_order, set_tab_order, tab_orders};
#[cfg(test)]
use values::has_password_flag;
use values::write_value;
pub(crate) use values::{ask_viewers_to_redraw, merged_appearance};
pub use values::{
    calculations_changed, clear_password_values, has_password_values, password_values, recalculate, recalculate_with, reset, restore_password_values,
    set_value, set_value_with, set_values,
};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FormError {
    #[error("the document has no form fields")]
    NoForm,
    #[error("there is no field named {0:?}")]
    NoSuchField(String),
    #[error("{0:?} is read-only")]
    ReadOnly(String),
    #[error("{0}")]
    Invalid(String),
    /// The value doesn't fit the field's format (its keystroke check refused it), which is
    /// checked before any of the field's scripts run.
    #[error("{0}")]
    Format(String),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    CheckBox,
    Radio,
    PushButton,
    Combo,
    List,
    Signature,
}

/// Field flags (§12.7.4, Tables 226, 228, 229, 231).
pub mod flags {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSH_BUTTON: u32 = 1 << 16;
    pub const COMBO: u32 = 1 << 17;
    pub const EDIT: u32 = 1 << 18;
    pub const SORT: u32 = 1 << 19;
    pub const FILE_SELECT: u32 = 1 << 20;
    pub const MULTI_SELECT: u32 = 1 << 21;
    pub const DO_NOT_SPELL_CHECK: u32 = 1 << 22;
    pub const DO_NOT_SCROLL: u32 = 1 << 23;
    pub const COMB: u32 = 1 << 24;
    /// Buttons: radios in unison. Text fields use the same bit for rich text.
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
    pub const RICH_TEXT: u32 = 1 << 25;
    pub const COMMIT_ON_SEL_CHANGE: u32 = 1 << 26;
}

/// One widget (the field's appearance on a page).
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub obj: ObjRef,
    /// 0-based page index, when the widget is on a page.
    pub page: Option<usize>,
    /// In user space, normalized.
    pub rect: [f64; 4],
    /// Check boxes and radio buttons: the name of the "on" appearance state.
    pub on_state: Option<String>,
    /// The current appearance state (`/AS`).
    pub state: Option<String>,
    /// Position in the document's tab order (pages in order; within a page by its `/Tabs`:
    /// rows, columns, or annotation/structure order). `usize::MAX` when not on a page.
    pub tab: usize,
    /// The widget's Locked flag (`/F` bit 8): its properties can't be changed.
    pub locked: bool,
    /// The widget's Hidden or NoView flag (`/F` bit 2 or 6): it isn't shown and takes no input.
    pub hidden: bool,
    /// Read-only: the field is, or the widget's own `/Ff` says so (a radio button of a group can
    /// be locked on its own, as an XFA exclusion group's layout writes it).
    pub read_only: bool,
    /// `/MK /R` as 0, 90, 180 or 270 degrees counterclockwise. Anything else is stored as 0.
    pub rotation: i64,
    /// Push buttons: the normal label (`/MK /CA`), separate from the field's name.
    pub caption: Option<String>,
    /// List boxes: where the options are drawn, so a click can pick the one under the pointer.
    pub list: Option<appearance::ListRows>,
    /// Drawn round: a radio button, or a check box laid out for a round XFA check button
    /// (`/PCXfaRound true`).
    pub round: bool,
}

/// A terminal form field.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Fully qualified name (`parent.child`).
    pub name: String,
    pub obj: ObjRef,
    pub kind: FieldKind,
    /// Text: the text. Check box / radio: the selected state (none when off). Choice: the
    /// selected export values.
    pub value: Vec<String>,
    pub default: Vec<String>,
    pub flags: u32,
    pub max_len: Option<usize>,
    /// Choice options: (export value, display text).
    pub options: Vec<(String, String)>,
    /// Default appearance string (`/DA`, inherited from the form if absent).
    pub da: String,
    /// Quadding: 0 left, 1 centred, 2 right.
    pub quadding: i64,
    pub tooltip: Option<String>,
    pub widgets: Vec<Widget>,
    /// Format, validate and calculate scripts (Acrobat's AF functions), see [`af`].
    pub actions: af::Actions,
    /// Push buttons: what a click does.
    pub button: Option<af::ButtonAction>,
}

impl Field {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// A comb field: the Comb flag set with a `/MaxLen`, neither multiline nor password (ISO
    /// 32000-2 Table 228). Each cell holds one character.
    pub fn is_comb(&self) -> bool {
        self.has(flags::COMB) && !self.has(flags::MULTILINE) && !self.has(flags::PASSWORD) && self.max_len.is_some_and(|m| m > 0)
    }

    pub fn read_only(&self) -> bool {
        self.has(flags::READ_ONLY)
    }

    /// Locked (Field Properties ▸ General ▸ Locked): its properties can't be changed.
    pub fn locked(&self) -> bool {
        self.widgets.iter().any(|w| w.locked)
    }

    /// Check boxes and radio groups with `/Opt` (PDF 1.4): one export value per widget, in widget
    /// order. Their on states are then often just positions (`/0`, `/1` …), as pdf-lib writes them.
    fn button_exports(&self) -> Option<&[(String, String)]> {
        (matches!(self.kind, FieldKind::CheckBox | FieldKind::Radio) && !self.options.is_empty() && self.options.len() == self.widgets.len())
            .then_some(self.options.as_slice())
    }

    /// The export value of widget `i` of a check box or radio group: its `/Opt` entry when the
    /// field has one per widget, otherwise the widget's on-state name.
    pub fn export_of(&self, i: usize) -> Option<&str> {
        let on = self.widgets.get(i)?.on_state.as_deref()?;
        Some(self.button_exports().and_then(|o| o.get(i)).map_or(on, |(e, _)| e.as_str()))
    }

    /// The on-state name that selects `choice` in a check box or radio group, where `choice` is an
    /// on-state name or an export value from `/Opt`.
    pub fn state_for(&self, choice: &str) -> Option<&str> {
        self.widgets.iter().filter_map(|w| w.on_state.as_deref()).find(|s| *s == choice).or_else(|| {
            let i = (0..self.widgets.len()).find(|&i| self.export_of(i) == Some(choice))?;
            self.widgets.get(i)?.on_state.as_deref()
        })
    }

    /// The export value for an on-state name such as the field's value; the name itself when the
    /// field has no `/Opt`.
    pub fn export_for_state<'a>(&'a self, state: &'a str) -> &'a str {
        self.widgets.iter().position(|w| w.on_state.as_deref() == Some(state)).and_then(|i| self.export_of(i)).unwrap_or(state)
    }

    /// The value as one string: text, the state name, or the selected display texts.
    pub fn display_value(&self) -> String {
        match self.kind {
            FieldKind::Combo | FieldKind::List => self
                .value
                .iter()
                .map(|v| self.options.iter().find(|(e, _)| e == v).map_or(v.clone(), |(_, d)| d.clone()))
                .collect::<Vec<_>>()
                .join(", "),
            _ => self.value.join(", "),
        }
    }
}

/// A value to put in a field.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Text(String),
    /// Check box: checked or not.
    Check(bool),
    /// Radio group: the on-state to select (`None`: none selected).
    Radio(Option<String>),
    /// Combo box or list box: export values (several only for multi-select lists).
    Choice(Vec<String>),
}

// ── reading ─────────────────────────────────────────────────────────────────────────────────

fn text_of(o: &Object) -> Option<String> {
    match o {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(name_text(n)),
        _ => None,
    }
}

/// A name object's bytes as text that [`name_bytes`] turns back into the same bytes. UTF-8 names
/// read as themselves; other bytes (Shift-JIS check box states such as 「はい」 in Japanese
/// forms) and `#` are written `#XX`, as in PDF name syntax.
pub fn name_text(bytes: &[u8]) -> String {
    let escape = |b: u8| format!("#{b:02X}");
    match std::str::from_utf8(bytes) {
        Ok(s) => s.replace('#', "#23"),
        Err(_) => bytes.iter().map(|&b| if (0x21..=0x7e).contains(&b) && b != b'#' { char::from(b).to_string() } else { escape(b) }).collect(),
    }
}

/// The bytes of the name written as `text` by [`name_text`] (`#XX` is the byte XX).
pub fn name_bytes(text: &str) -> Vec<u8> {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| b.get(j).and_then(|h| char::from(*h).to_digit(16));
        match (c, hex(i + 1), hex(i + 2)) {
            // Both digits are below 16, so the byte fits.
            (b'#', Some(hi), Some(lo)) => {
                out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'#'));
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// A name object from text written by [`name_text`].
fn name_obj(text: &str) -> Object {
    Object::Name(name_bytes(text))
}

fn invalid<T>(m: impl Into<String>) -> Result<T, FormError> {
    Err(FormError::Invalid(m.into()))
}
