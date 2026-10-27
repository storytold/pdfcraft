use super::*;

/// How far below its baseline an underline is drawn, in the text's size: an underline is the
/// level line pushed just before its span, this far below it (the PDF writer tells it apart from
/// a rule by this).
pub(crate) const UNDERLINE_DROP: f64 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Fill {
        rect: Rect,
        color: [f64; 3],
    },
    Line {
        from: (f64, f64),
        to: (f64, f64),
        width: f64,
        color: [f64; 3],
        dashed: bool,
    },
    /// A box with rounded (or inverted) corners: its fill, and its edge drawn inside it.
    Rounded(Box<Rounded>),
    Text(Span),
    Image {
        rect: Rect,
        data: Arc<Vec<u8>>,
        content_type: String,
    },
    Widget(Box<Widget>),
}

/// A box with rounded corners ([`Item::Rounded`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Rounded {
    pub rect: Rect,
    /// Top-left, top-right, bottom-right and bottom-left radii, each at most half the box's
    /// shorter side.
    pub radii: [f64; 4],
    /// Corners whose curve goes into the box rather than around it, in the same order.
    pub inverted: [bool; 4],
    pub fill: Option<[f64; 3]>,
    /// The edge: its width, colour and whether it is dashed.
    pub edge: Option<(f64, [f64; 3], bool)>,
    /// A page break cut the box here: that side has no edge, and its corners are square.
    pub open_top: bool,
    pub open_bottom: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetKind {
    Text,
    /// A text field whose characters are shown as asterisks.
    Password,
    /// A drop-down (`list_box` false) or list box: `options` are (saved value, shown text).
    Choice {
        options: Vec<(String, String)>,
        list_box: bool,
        multi: bool,
        editable: bool,
    },
    /// A signature field, to be signed in the app.
    Signature,
    /// An image field: a push button that picks a picture (the picture itself is page content).
    Image,
    /// A date field with its Acrobat format pattern (`yyyy-mm-dd`).
    Date(String),
    CheckBox {
        on: String,
        round: bool,
    },
    Radio {
        group: String,
        on: String,
        round: bool,
    },
    Button {
        caption: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderShape {
    None,
    Full,
    Underline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WidgetBorder {
    pub color: Option<[f64; 3]>,
    pub fill: Option<[f64; 3]>,
    pub shape: BorderShape,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Reset,
    Print,
    SaveAs,
    Url(String),
    /// Any other click script: run by the XFA scripting engine.
    Script(String),
}

/// An interactive field to create as an AcroForm widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub rect: Rect,
    pub kind: WidgetKind,
    /// The PDF field name (unique in the form, no periods).
    pub name: String,
    /// The template's scripting object model path, for data round trips.
    pub som: String,
    pub tooltip: Option<String>,
    pub face: Face,
    pub h_align: HAlign,
    pub multiline: bool,
    pub max_chars: Option<usize>,
    pub read_only: bool,
    pub border: WidgetBorder,
    /// The field's value: saved (bound) values for choice lists, newline-separated when several.
    pub value: Option<String>,
    /// The template's default value (what a reset restores); data values are not defaults.
    pub default: Option<String>,
    pub action: Option<Action>,
    /// Check boxes: the on and off values of the data; radio buttons: the on value.
    pub items: Vec<String>,
    /// A text field that doesn't scroll: it takes no more text than it shows (`DoNotScroll`).
    pub no_scroll: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Form {
    pub pages: Vec<Page>,
    pub fields: usize,
    pub warnings: Vec<String>,
}
