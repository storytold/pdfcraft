//! Prepare a form: create, change and delete fields (execution plan M6.6).
//!
//! New fields follow Acrobat's conventions: names "Text1", "Check Box1", "Group1" (radio groups),
//! "Dropdown1", "List Box1", "Button1", "Signature1", "Date1"; Helvetica at Acrobat's Auto size
//! for text and date fields (12 pt in a box that has room, smaller in a short one), 12 pt in
//! choice fields, a thin grey border and a white background, and appearance streams generated
//! immediately so every viewer shows the empty field.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::{Field, FieldKind, FormError, Widget, appearance, fields, flags, page_refs};

mod edit;
mod lifecycle;
mod look;
mod tree;

pub use edit::set_props;
pub use lifecycle::{add_field, delete_field, delete_field_object, duplicate_field};
pub(crate) use look::button_appearance;
pub use look::{BorderStyle, CheckStyle, FieldFont, Look, LookPatch, check_style, look, redraw_field, set_button_icon};
pub(crate) use tree::{ensure_form, form_fonts_as_objects};

/// The smallest width and height of a field's widget, in points: a smaller one is refused.
pub const MIN_FIELD_SIDE: f64 = 4.0;

/// The kind of field to add.
#[derive(Clone, Debug, PartialEq)]
pub enum NewField {
    Text {
        multiline: bool,
    },
    /// A date field: a text field with Acrobat's date format action (`AFDate_FormatEx`).
    Date,
    CheckBox,
    /// One radio button in `group` (a new group is created when none of that name exists),
    /// selected by `export`.
    Radio {
        group: Option<String>,
        export: String,
    },
    Combo {
        options: Vec<String>,
        editable: bool,
    },
    List {
        options: Vec<String>,
        multi: bool,
    },
    Button {
        caption: String,
    },
    /// An image field: an icon-only push button that asks for a picture when clicked.
    Image,
    Signature,
}

impl NewField {
    fn base_name(&self) -> &'static str {
        match self {
            NewField::Text { .. } => "Text",
            NewField::Date => "Date",
            NewField::CheckBox => "Check Box",
            NewField::Radio { .. } => "Group",
            NewField::Combo { .. } => "Dropdown",
            NewField::List { .. } => "List Box",
            NewField::Button { .. } => "Button",
            NewField::Image => "Image",
            NewField::Signature => "Signature",
        }
    }
}

/// Field properties the Properties dialog edits (`None` leaves a value unchanged).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldProps {
    pub name: Option<String>,
    pub tooltip: Option<String>,
    /// Normal push-button label (`/MK /CA`) of one widget (0-based index).
    /// An empty string clears the label; other widgets and icon layouts are preserved.
    pub caption: Option<(usize, String)>,
    pub read_only: Option<bool>,
    pub required: Option<bool>,
    pub multiline: Option<bool>,
    pub max_len: Option<Option<usize>>,
    /// Choice options (display texts; export values equal them).
    pub options: Option<Vec<String>>,
    /// Font size (0 = auto).
    pub font_size: Option<f64>,
    /// Position: move or resize one widget (its index in [`Field::widgets`]) to a new rect in
    /// user space.
    pub rect: Option<(usize, [f64; 4])>,
    /// Appearance tab (border, fill, line, text colour, font).
    pub look: Option<Look>,
    /// Change only these appearance properties. Unlike `look`, this preserves each widget's
    /// other colours, border settings and default appearance, including custom font resources.
    pub appearance: Option<LookPatch>,
    /// Format, Validate and Calculate tabs (written as Acrobat's AF scripts).
    pub format: Option<crate::af::Format>,
    pub validate: Option<crate::af::Validate>,
    pub calculate: Option<crate::af::Calculate>,
    /// Options-tab flags (`/Ff` bits from [`crate::flags`]) to set or clear.
    pub flags: Vec<(u32, bool)>,
    /// Alignment (`/Q`): 0 left, 1 centre, 2 right.
    pub quadding: Option<i64>,
    /// Default value (`/DV`): what Reset form restores.
    pub default_value: Option<Option<String>>,
    /// Lock or unlock the field (its widgets' Locked flag).
    pub locked: Option<bool>,
    /// Actions tab: every trigger's action (replacing the field's).
    pub actions: Option<Vec<(crate::Trigger, crate::FieldAction)>>,
    /// Options tab: the mark of a check box or radio button (every widget).
    pub check_style: Option<CheckStyle>,
    /// Position tab: rotate one widget (`/MK /R`) to 0, 90, 180 or 270 degrees counterclockwise.
    /// Changing between horizontal and vertical swaps that widget's width and height around its
    /// center, unless [`Self::rect`] is also set.
    pub rotation: Option<(usize, i64)>,
}

fn invalid<T>(m: impl Into<String>) -> Result<T, FormError> {
    Err(FormError::Invalid(m.into()))
}

fn rgb(c: [f64; 3]) -> Object {
    Object::Array(c.iter().map(|v| Object::Real(*v)).collect())
}
