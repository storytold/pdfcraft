//! AcroForm inspection, native filling, and signature field interaction (§12.7).

use crate::document::FfiRect;
use pdfcraft_engine::FormFieldKind;

/// Kind of an interactive AcroForm field (§12.7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiFieldKind {
    Text,
    CheckBox,
    Radio,
    PushButton,
    Choice,
    List,
    Signature,
}

impl From<FormFieldKind> for FfiFieldKind {
    fn from(k: FormFieldKind) -> Self {
        match k {
            FormFieldKind::Text => FfiFieldKind::Text,
            FormFieldKind::CheckBox => FfiFieldKind::CheckBox,
            FormFieldKind::Radio => FfiFieldKind::Radio,
            FormFieldKind::PushButton => FfiFieldKind::PushButton,
            FormFieldKind::Combo => FfiFieldKind::Choice,
            FormFieldKind::List => FfiFieldKind::List,
            FormFieldKind::Signature => FfiFieldKind::Signature,
        }
    }
}

/// An option in a choice (combo box or list) field.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiChoiceOption {
    /// Export value stored in the PDF document.
    pub export_value: String,
    /// Display text shown in the user interface.
    pub display_text: String,
}

/// A representation of a form field widget on a page for native input overlay placement and data binding.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiFormField {
    /// Fully qualified field name (`"parent.child"`).
    pub name: String,
    /// 0-based page index where this field widget is rendered (if placed on a page).
    pub page: Option<u32>,
    /// Index of this widget within the field's widget list.
    pub widget_index: u32,
    /// Field type.
    pub kind: FfiFieldKind,
    /// Current field value(s) (text contents, checked state name, or selected choice export value).
    pub value: Vec<String>,
    /// Default field value(s) (/DV).
    pub default_value: Vec<String>,
    /// Options for choice or list fields.
    pub options: Vec<FfiChoiceOption>,
    /// Bounding rectangle in view coordinates (points, y down).
    pub rect: FfiRect,
    /// Whether the field is read-only or locked against input.
    pub read_only: bool,
    /// Whether the field is required.
    pub required: bool,
    /// Whether a text field allows multiple lines.
    pub multiline: bool,
    /// Whether a text field is a password field.
    pub password: bool,
    /// Maximum character length if constrained by /MaxLen.
    pub max_len: Option<u32>,
    /// Tooltip / alternate descriptive text (/TU).
    pub tooltip: Option<String>,
    /// The name of the "on" state for check boxes and radio buttons.
    pub on_state: Option<String>,
}
