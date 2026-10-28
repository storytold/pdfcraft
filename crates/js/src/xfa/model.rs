use super::HashSet;

/// What kind of template object a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XfaKind {
    /// `xfa.form` itself.
    Form,
    Subform,
    Field,
    ExclGroup,
    Draw,
    Area,
}

impl XfaKind {
    pub(crate) fn class_name(self) -> &'static str {
        match self {
            XfaKind::Form => "form",
            XfaKind::Subform => "subform",
            XfaKind::Field => "field",
            XfaKind::ExclGroup => "exclGroup",
            XfaKind::Draw => "draw",
            XfaKind::Area => "area",
        }
    }
}

/// One object of the form, as the caller snapshots it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaNode {
    pub name: String,
    /// The SOM expression naming this instance (`form[0].head[0].familyName[0]`).
    pub som: String,
    pub kind: Option<XfaKind>,
    /// Fields: the raw value (ISO dates, on values for buttons); empty when null.
    pub value: String,
    /// Numeric fields give numbers to scripts.
    pub numeric: bool,
    /// `visible`, `invisible`, `hidden` or `inactive`.
    pub presence: String,
    /// `open`, `readOnly`, `protected` or `nonInteractive`.
    pub access: String,
    /// Subforms: may have several instances (`occur max` other than 1).
    pub repeatable: bool,
    pub occur_min: usize,
    pub occur_max: Option<usize>,
    /// This instance's index among its same-named siblings.
    pub index: usize,
    pub children: Vec<XfaNode>,
    /// Fields: the value shown in the field's display picture, what `formattedValue` reads;
    /// `None` when the picture doesn't change it (the raw value is read then).
    pub formatted: Option<String>,
}

/// The event a script runs for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaEvent {
    /// `click`, `calculate`, `validate`, `initialize`, `change`, `exit`, …
    pub activity: String,
    /// The SOM of `this`.
    pub target: String,
    pub new_text: String,
    pub prev_text: String,
}

/// About the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaDoc {
    pub file_name: String,
    /// The current page, 0-based.
    pub page: usize,
    pub page_count: usize,
}

/// Something a script changed or asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum XfaEffect {
    SetValue {
        som: String,
        value: String,
    },
    SetPresence {
        som: String,
        presence: String,
    },
    SetAccess {
        som: String,
        access: String,
    },
    /// A new instance of a repeating subform, named by its own SOM (`…row[3]`).
    AddInstance {
        som: String,
    },
    /// The instance at this SOM is removed.
    RemoveInstance {
        som: String,
    },
    MessageBox(String),
    /// `xfa.host.resetData()`: the named fields, or every field when empty.
    ResetData(Vec<String>),
    /// `app.execMenuItem("ShowHide…")`: show or hide a side panel.
    ShowPanel(crate::Panel),
    Print,
    SaveAs,
    LaunchUrl(String),
    SetFocus(String),
    Beep,
    /// `xfa.form.recalculate(true)`.
    Recalculate,
    /// `xfa.form.remerge()`, `xfa.layout.relayout()`.
    Relayout,
}

/// What running a script did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaOutcome {
    pub effects: Vec<XfaEffect>,
    pub console: Vec<String>,
    /// The script's error, if it failed.
    pub error: Option<String>,
    /// The script's completion value (a calculate script's result; a validate script's verdict).
    pub result: Option<String>,
    /// The completion value as a boolean, when it was one.
    pub result_bool: Option<bool>,
    /// Limits the script ran into (too many effects, message boxes or console lines): what
    /// was left out, for the caller to report.
    pub notes: Vec<String>,
    /// The script ran past its time limit and was left running on its own thread (the engine
    /// can't interrupt it): the caller should run no more of this form's scripts.
    pub abandoned: bool,
}

/// Most nodes a snapshot may hold.
pub(super) const MAX_NODES: usize = 200_000;

/// Most instances a script may add in one run.
pub(crate) const MAX_ADDED: usize = 1_000;

/// Deepest tree followed.
pub(super) const MAX_DEPTH: usize = 64;

/// Most effects one run may produce (after values set again on the same object are merged).
pub const MAX_EFFECTS: usize = 10_000;

/// Most message boxes one run may show.
pub const MAX_ALERTS: usize = 100;

/// Most console lines one run may print.
pub const MAX_CONSOLE: usize = 1_000;

/// Longest message or console line kept, in characters.
pub(crate) const MAX_LINE: usize = 4_096;

/// Most bytes of text one run may hand back or write: messages, console lines, URLs, names
/// and the values it sets. Past it, more are left out and noted.
pub(crate) const MAX_PAYLOAD: usize = 32 << 20;

/// Longest value a script may put in a field (characters); longer ones are cut and noted.
pub(super) const MAX_VALUE: usize = 65_536;

/// `m` cut to [`MAX_LINE`] characters.
pub(crate) fn clip(m: String) -> String {
    match m.char_indices().nth(MAX_LINE) {
        Some((at, _)) => format!("{}…", m.get(..at).unwrap_or_default()),
        None => m,
    }
}

/// `v` cut to [`MAX_VALUE`] characters, and whether it was cut.
pub(crate) fn clip_value(v: String) -> (String, bool) {
    match v.char_indices().nth(MAX_VALUE) {
        Some((at, _)) => (v.get(..at).unwrap_or_default().to_string(), true),
        None => (v, false),
    }
}

/// The field names of a `resetData` list (comma-separated), each cut to [`MAX_LINE`] and
/// named once, at most as many as a snapshot holds nodes.
pub(crate) fn reset_names(list: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if out.len() >= MAX_NODES {
            break;
        }
        let name = clip(name.to_string());
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

/// The (property, SOM) an effect setting a property of an object is merged by, see
/// [`XHost::set_at`].
pub(super) fn effect_key(e: &XfaEffect) -> Option<(u8, String)> {
    match e {
        XfaEffect::SetValue { som, .. } => Some((0, som.clone())),
        XfaEffect::SetPresence { som, .. } => Some((1, som.clone())),
        XfaEffect::SetAccess { som, .. } => Some((2, som.clone())),
        _ => None,
    }
}

/// Bytes of text `e` holds.
pub(super) fn payload(e: &XfaEffect) -> usize {
    match e {
        XfaEffect::SetValue { som, value: s } | XfaEffect::SetPresence { som, presence: s } | XfaEffect::SetAccess { som, access: s } => {
            som.len().saturating_add(s.len())
        }
        XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } => som.len(),
        XfaEffect::MessageBox(s) | XfaEffect::LaunchUrl(s) | XfaEffect::SetFocus(s) => s.len(),
        XfaEffect::ResetData(names) => names.iter().fold(0usize, |n, s| n.saturating_add(s.len()).saturating_add(std::mem::size_of::<String>())),
        XfaEffect::Print | XfaEffect::SaveAs | XfaEffect::Beep | XfaEffect::Recalculate | XfaEffect::Relayout | XfaEffect::ShowPanel(_) => 0,
    }
}

/// A script's completion value (what a calculate script puts in its field) cut to
/// [`MAX_VALUE`] characters, as values a script sets are, with a note when it was.
pub(crate) fn clip_result(out: &mut XfaOutcome) {
    if let Some(r) = out.result.take() {
        let (r, cut) = clip_value(r);
        if cut {
            out.notes.push(format!("the script's result was longer than {MAX_VALUE} characters; it was cut"));
        }
        out.result = Some(r);
    }
}
