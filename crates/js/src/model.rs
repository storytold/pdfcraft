/// The kinds of field, as `field.type` names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    Text,
    CheckBox,
    RadioButton,
    ComboBox,
    ListBox,
    Button,
    Signature,
}

impl FieldType {
    pub fn name(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::CheckBox => "checkbox",
            FieldType::RadioButton => "radiobutton",
            FieldType::ComboBox => "combobox",
            FieldType::ListBox => "listbox",
            FieldType::Button => "button",
            FieldType::Signature => "signature",
        }
    }
}

/// `display.*`: visible, hidden, noPrint, noView.
pub const DISPLAY_VISIBLE: i32 = 0;

pub const DISPLAY_HIDDEN: i32 = 1;

pub const DISPLAY_NO_PRINT: i32 = 2;

pub const DISPLAY_NO_VIEW: i32 = 3;

/// A field as scripts see it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldState {
    pub name: String,
    pub kind: FieldType,
    /// Text: the text. Check box / radio: the selected export value (empty when off). Choice:
    /// the selected export values.
    pub value: Vec<String>,
    pub default: Vec<String>,
    pub readonly: bool,
    pub required: bool,
    pub display: i32,
    /// Choice options (export value, display text); check boxes and radios: their export
    /// values, one per widget.
    pub options: Vec<(String, String)>,
    pub char_limit: Option<usize>,
    /// `textColor` / `fillColor` as Acrobat colour arrays (`["RGB", r, g, b]` etc.), when set.
    pub text_color: Option<Vec<String>>,
    pub fill_color: Option<Vec<String>>,
}

impl FieldState {
    pub fn new(name: impl Into<String>, kind: FieldType, value: Vec<String>) -> FieldState {
        FieldState {
            name: name.into(),
            kind,
            value,
            default: Vec::new(),
            readonly: false,
            required: false,
            display: DISPLAY_VISIBLE,
            options: Vec::new(),
            char_limit: None,
            text_color: None,
            fill_color: None,
        }
    }

    pub(crate) fn single(&self) -> String {
        self.value.first().cloned().unwrap_or_default()
    }
}

/// The event a script runs for.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// `Keystroke`, `Validate`, `Calculate`, `Format`, `Mouse Up`, `Open`, …
    pub name: String,
    /// `Field`, `Doc`, `Page`, …
    pub kind: String,
    /// The field the event is for.
    pub target: Option<String>,
    pub value: String,
    pub change: String,
    pub will_commit: bool,
}

impl Event {
    /// A field event (`event.type` = "Field").
    pub fn field(name: &str, target: &str, value: &str) -> Event {
        Event { name: name.into(), kind: "Field".into(), target: Some(target.into()), value: value.into(), change: String::new(), will_commit: true }
    }

    /// A document event (`event.type` = "Doc"), e.g. Open or WillSave.
    pub fn doc(name: &str) -> Event {
        Event { name: name.into(), kind: "Doc".into(), target: None, value: String::new(), change: String::new(), will_commit: false }
    }
}

/// About the document, for `this.numPages`, `this.info` and the like.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocInfo {
    pub file_name: String,
    pub num_pages: usize,
    /// The current page, 0-based.
    pub page: usize,
    /// Document information (`this.info.title` etc.): lower-case key, value.
    pub info: Vec<(String, String)>,
    /// The document has an XFA form, so scripts see `xfa_installed`.
    pub xfa: bool,
}

/// The icon a script's message shows: `app.alert`'s `nIcon` and `xfa.host.messageBox`'s
/// icon, numbered 0 to 3 in the JavaScript for Acrobat API Reference and the XFA
/// specification alike.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageIcon {
    /// 0, the default.
    #[default]
    Error,
    /// 1.
    Warning,
    /// 2.
    Question,
    /// 3.
    Status,
}

impl MessageIcon {
    /// The icon numbered `code`; a missing, fractional or out-of-range number is the default.
    pub fn from_code(code: Option<f64>) -> MessageIcon {
        match whole_code(code) {
            Some(1) => MessageIcon::Warning,
            Some(2) => MessageIcon::Question,
            Some(3) => MessageIcon::Status,
            _ => MessageIcon::Error,
        }
    }

    /// A stable name for tools and logs: `error`, `warning`, `question` or `status`.
    pub fn name(self) -> &'static str {
        match self {
            MessageIcon::Error => "error",
            MessageIcon::Warning => "warning",
            MessageIcon::Question => "question",
            MessageIcon::Status => "status",
        }
    }
}

/// The buttons a script's message offers: `app.alert`'s `nType` and `xfa.host.messageBox`'s
/// buttons, numbered 0 to 3 in both references.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageButtons {
    /// 0, the default.
    #[default]
    Ok,
    /// 1.
    OkCancel,
    /// 2.
    YesNo,
    /// 3.
    YesNoCancel,
}

impl MessageButtons {
    /// The buttons numbered `code`; a missing, fractional or out-of-range number is the default.
    pub fn from_code(code: Option<f64>) -> MessageButtons {
        match whole_code(code) {
            Some(1) => MessageButtons::OkCancel,
            Some(2) => MessageButtons::YesNo,
            Some(3) => MessageButtons::YesNoCancel,
            _ => MessageButtons::Ok,
        }
    }

    /// A stable name for tools and logs: `ok`, `ok_cancel`, `yes_no` or `yes_no_cancel`.
    pub fn name(self) -> &'static str {
        match self {
            MessageButtons::Ok => "ok",
            MessageButtons::OkCancel => "ok_cancel",
            MessageButtons::YesNo => "yes_no",
            MessageButtons::YesNoCancel => "yes_no_cancel",
        }
    }
}

/// `code` as one of the numbers 0 to 3 the references define, when it is exactly one.
fn whole_code(code: Option<f64>) -> Option<u8> {
    let c = code.filter(|c| c.is_finite() && c.fract() == 0.0 && (0.0..=3.0).contains(c))?;
    // In 0..=3 and whole, so the cast is exact.
    Some(c as u8)
}

/// A message a script showed (`app.alert`, `xfa.host.messageBox`), with the title, icon and
/// buttons it asked for. Its text and title are cut to a message's length.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    /// The dialog's title, when the script gave one.
    pub title: Option<String>,
    pub icon: MessageIcon,
    pub buttons: MessageButtons,
}

impl Message {
    /// A message with only text: no title, the default icon and an OK button.
    pub fn plain(text: impl Into<String>) -> Message {
        Message { text: text.into(), ..Default::default() }
    }

    /// Bytes of text the message holds, charged to a run's budget.
    pub(crate) fn bytes(&self) -> usize {
        self.text.len().saturating_add(self.title.as_ref().map_or(0, String::len))
    }
}

/// Something a script asked the viewer to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// `this.resetForm([names])` (empty: every field).
    Reset(Vec<String>),
    Print,
    /// `this.pageNum = n` (0-based).
    GoToPage(usize),
    LaunchUrl(String),
    /// `this.submitForm(url)`: never sent on PdfCraft's own.
    Submit(String),
    /// `field.setFocus()`.
    Focus(String),
    Beep,
    /// `app.execMenuItem("SaveAs")` (XFA forms' Save buttons).
    SaveAs,
    /// `app.execMenuItem("ShowHide…")`: show the side panel, or hide it when it is showing.
    ShowPanel(Panel),
}

/// A side panel a script can show or hide (`app.execMenuItem("ShowHideFileAttachment")`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    /// The document's bookmarks (outline).
    Bookmarks,
    /// Page thumbnails.
    Pages,
    /// The files attached to the document.
    Attachments,
    /// The document's signatures and their validity.
    Signatures,
    /// The document's layers (optional content).
    Layers,
}

impl Panel {
    /// The panel's name in automation output, fixed whatever the variant is called.
    pub fn id(self) -> &'static str {
        match self {
            Panel::Bookmarks => "bookmarks",
            Panel::Pages => "pages",
            Panel::Attachments => "attachments",
            Panel::Signatures => "signatures",
            Panel::Layers => "layers",
        }
    }
}

/// What running a script did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// `event.rc`: `false` rejects the keystroke or value.
    pub rc: bool,
    /// `event.value` after the script.
    pub value: String,
    pub change: String,
    /// Fields whose value or properties the script changed, as they are now.
    pub changed: Vec<FieldState>,
    /// The messages the script showed, in order.
    pub alerts: Vec<Message>,
    pub console: Vec<String>,
    pub requests: Vec<Request>,
    /// The script's error (syntax error, exception, or a limit reached), if it failed.
    pub error: Option<String>,
    /// Errors in the document-level scripts that ran first. Each is a script of its own: one
    /// that throws stops where it failed, and the others and the script itself still run (#3940).
    pub doc_errors: Vec<String>,
    /// The script's completion value (what the console prints), unless undefined.
    pub result: Option<String>,
}

/// Bounds that keep a script from hanging the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub loop_iterations: u64,
    pub recursion: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { loop_iterations: 1_000_000, recursion: 256 }
    }
}
