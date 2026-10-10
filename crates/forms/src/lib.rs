//! pdfcraft-forms — interactive forms (AcroForm, ISO 32000-2 §12.7), execution plan M6.1–M6.2.
//!
//! - [`fields`]: the field tree flattened to terminal fields, each with its widgets (page,
//!   rectangle, on-state), inherited attributes (`/FT`, `/Ff`, `/V`, `/DV`, `/DA`, `/Q`,
//!   `/MaxLen`) and choice options.
//! - [`set_value`]: fill a field. Text and choice fields get new appearance streams
//!   ([`appearance`]); check boxes and radio buttons switch `/V` and each widget's `/AS` between
//!   the states their appearances already define.
//! - [`reset`]: Acrobat's Clear form (back to `/DV`).
//!
//! Not yet: JavaScript actions (format, keystroke, validate, calculate — M6.4/M6.5) and rich text
//! values (`/RV`, which is removed when a value is set so it can't contradict `/V`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

mod actions;
pub mod af;
pub mod appearance;
mod author;
pub mod detect;
mod scripting;
pub use actions::{FieldAction, Trigger, field_actions, set_field_actions};
pub use author::{
    BorderStyle, CheckStyle, FieldFont, FieldProps, Look, NewField, add_field, check_style, delete_field, duplicate_field, look, redraw_field,
    set_button_icon, set_props,
};
pub use scripting::{
    FieldChange, FieldEvent, NoScripts, ScriptResult, Scripts, apply_script_changes, document_scripts, document_scripts_named, set_document_script,
    set_field_script,
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
    /// `/MK /R` as 0, 90, 180 or 270 degrees counterclockwise. Anything else is stored as 0.
    pub rotation: i64,
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

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// A push button's mouse-up action (`/A`, or `/AA /U`, on the field or its widget).
fn button_action(doc: &Document, d: &Dict, widgets: &[ObjRef]) -> Option<af::ButtonAction> {
    let pick = |dict: &Dict| -> Option<Object> {
        dict.get(b"A").cloned().or_else(|| dict.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|aa| aa.get(b"U").cloned())))
    };
    let action = pick(d).or_else(|| widgets.iter().find_map(|w| doc.get(*w).as_dict().and_then(pick)))?;
    let a = doc.resolve(&action);
    let a = a.as_dict()?;
    let s = |k: &[u8]| a.get(k).and_then(|v| text_of(&doc.resolve(v)));
    Some(match a.name(b"S")? {
        b"ResetForm" => {
            let fields = a
                .get(b"Fields")
                .map(|f| doc.resolve(f))
                .and_then(|f| f.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|o| match o {
                    Object::Ref(r) => doc.get(*r).as_dict().and_then(|fd| fd.get(b"T").and_then(|t| text_of(&doc.resolve(t)))),
                    other => text_of(&doc.resolve(other)),
                })
                .collect();
            let exclude = a.get(b"Flags").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0) & 1 != 0;
            af::ButtonAction::Reset { fields, exclude }
        }
        b"Named" => af::ButtonAction::Named(String::from_utf8_lossy(a.name(b"N")?).into_owned()),
        b"URI" => af::ButtonAction::Uri(s(b"URI")?),
        b"SubmitForm" => af::ButtonAction::Submit(
            a.get(b"F")
                .map(|f| doc.resolve(f))
                .and_then(|f| f.as_dict().and_then(|fd| fd.get(b"F").and_then(|x| text_of(&doc.resolve(x)))).or_else(|| text_of(&f)))
                .unwrap_or_default(),
        ),
        b"GoTo" => {
            let dest = doc.resolve(a.get(b"D")?);
            let target = dest.as_array()?.first()?.as_ref()?;
            af::ButtonAction::GoTo(page_refs(doc).iter().position(|p| *p == target)?)
        }
        b"Hide" => {
            // /T: a field name, an annotation (or field) reference, or an array of them.
            let target = |o: &Object| match o {
                Object::Ref(r) => qualified_name(doc, *r).or_else(|| text_of(&doc.resolve(o))),
                other => text_of(other),
            };
            let fields = match a.get(b"T") {
                Some(t) => match &*doc.resolve(t) {
                    Object::Array(items) => items.iter().filter_map(target).collect(),
                    _ => target(t).into_iter().collect(),
                },
                None => Vec::new(),
            };
            let hide = !a.get(b"H").is_some_and(|h| matches!(&*doc.resolve(h), Object::Bool(false)));
            af::ButtonAction::ShowHide { fields, hide }
        }
        b"SetOCGState" => {
            // /State: ON, OFF or Toggle, each followed by the groups it applies to.
            let mut changes = Vec::new();
            if let Some(state) = a.get(b"State") {
                let mut op = None;
                for item in doc.resolve(state).as_array().into_iter().flatten().take(MAX_LAYER_STATE) {
                    match item {
                        Object::Name(n) => {
                            op = match n.as_slice() {
                                b"ON" => Some(af::LayerOp::On),
                                b"OFF" => Some(af::LayerOp::Off),
                                b"Toggle" => Some(af::LayerOp::Toggle),
                                _ => None,
                            }
                        }
                        Object::Ref(r) => changes.extend(op.map(|op| (op, (r.num, r.generation)))),
                        _ => {}
                    }
                }
            }
            let preserve_rb = !a.get(b"PreserveRB").is_some_and(|p| matches!(&*doc.resolve(p), Object::Bool(false)));
            af::ButtonAction::SetLayers { changes, preserve_rb }
        }
        b"JavaScript" => af::button_script(&script(doc, &action)?),
        _ => return None,
    })
}

/// The fully qualified name of the field that `r` (a field or one of its widgets) belongs to,
/// from the partial names (`/T`) up its `/Parent` chain.
fn qualified_name(doc: &Document, r: ObjRef) -> Option<String> {
    let mut parts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = Some(r);
    while let Some(c) = cur {
        if seen.len() > 64 || !seen.insert(c) {
            break;
        }
        let obj = doc.get(c);
        let d = obj.as_dict()?;
        if let Some(t) = d.get(b"T").and_then(|o| text_of(&doc.resolve(o))) {
            parts.push(t);
        }
        cur = d.get(b"Parent").and_then(|p| p.as_ref());
    }
    parts.reverse();
    (!parts.is_empty()).then(|| parts.join("."))
}

/// The JavaScript of an action dictionary (`/JS` string or stream).
fn script(doc: &Document, action: &Object) -> Option<String> {
    let a = doc.resolve(action);
    let js = a.as_dict()?.get(b"JS")?.clone();
    match &*doc.resolve(&js) {
        Object::String(s) => Some(s.to_text()),
        Object::Stream(s) => s.decoded().ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// A field's format/keystroke, validate and calculate scripts (field dictionary, or its only
/// widget's when the field has none).
fn actions_of(doc: &Document, d: &Dict, widgets: &[ObjRef]) -> af::Actions {
    let aa = d.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).or_else(|| {
        (widgets.len() == 1)
            .then(|| doc.get(widgets[0]))
            .and_then(|w| w.as_dict().and_then(|wd| wd.get(b"AA").map(|a| doc.resolve(a))))
            .and_then(|a| a.as_dict().cloned())
    });
    let mut out = af::Actions::default();
    let Some(aa) = aa else { return out };
    let js = |k: &[u8]| aa.get(k).and_then(|a| script(doc, a));
    if let Some(f) = js(b"F") {
        match af::parse_format(&f) {
            Some(fm) => out.format = fm,
            None if !f.trim().is_empty() => {
                out.unsupported.push("format");
                out.scripts.format = Some(f);
            }
            None => {}
        }
    }
    if let Some(k) = js(b"K")
        && out.format == af::Format::None
    {
        // A mask has only a keystroke script.
        match af::parse_format(&k) {
            Some(fm @ af::Format::Mask(_)) => out.format = fm,
            Some(_) => {}
            None if !k.trim().is_empty() => {
                out.unsupported.push("keystroke");
                out.scripts.keystroke = Some(k);
            }
            None => {}
        }
    }
    if let Some(v) = js(b"V") {
        match af::parse_validate(&v) {
            Some(vv) => out.validate = vv,
            None if !v.trim().is_empty() => {
                out.unsupported.push("validate");
                out.scripts.validate = Some(v);
            }
            None => {}
        }
    }
    if let Some(c) = js(b"C") {
        match af::parse_calculate(&c) {
            Some(cc) => out.calculate = cc,
            None if !c.trim().is_empty() => {
                out.unsupported.push("calculate");
                out.scripts.calculate = Some(c);
            }
            None => {}
        }
    }
    out
}

fn values_of(doc: &Document, o: Option<&Object>) -> Vec<String> {
    let Some(o) = o else { return Vec::new() };
    match &*doc.resolve(o) {
        Object::Array(a) => a.iter().filter_map(|x| text_of(&doc.resolve(x))).collect(),
        Object::Stream(s) => s.decoded().ok().map(|b| PdfString::literal(b).to_text()).into_iter().collect(),
        other => text_of(other).filter(|s| s != "Off" && !s.is_empty()).into_iter().collect(),
    }
}

fn nums(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = doc.resolve(o?);
    o.as_array()?.iter().map(|x| doc.resolve(x).as_f64()).collect()
}

/// Leaf page objects in order (a small walker; `organize` and `annot` have their own).
pub(crate) fn page_refs(doc: &Document) -> Vec<ObjRef> {
    let Some(root) = doc.root() else { return Vec::new() };
    let Some(pages) = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")) else { return Vec::new() };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![pages];
    while let Some(node) = stack.pop() {
        if !seen.insert(node) || seen.len() > 1_000_000 {
            continue;
        }
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        match d.get(b"Kids").map(|k| doc.resolve(k)) {
            Some(kids) if d.name(b"Type") != Some(b"Page") => {
                if let Some(a) = kids.as_array() {
                    stack.extend(a.iter().rev().filter_map(|k| k.as_ref()));
                }
            }
            _ => out.push(node),
        }
    }
    out
}

/// The AcroForm dictionary.
pub fn acroform(doc: &Document) -> Option<Dict> {
    let root = doc.root()?;
    let cat = doc.get(root);
    let af = cat.as_dict()?.get(b"AcroForm")?.clone();
    doc.resolve(&af).as_dict().cloned()
}

#[derive(Clone, Default)]
struct Inherited {
    name: String,
    ft: Option<Vec<u8>>,
    ff: Option<u32>,
    v: Option<Object>,
    dv: Option<Object>,
    da: Option<String>,
    q: Option<i64>,
    max_len: Option<usize>,
}

/// Every terminal field: the `/Fields` tree in order, then the fields reachable only through the
/// page annotations (see [`adopt_page_fields`]).
pub fn fields(doc: &Document) -> Vec<Field> {
    enumerate(doc).0
}

/// How many of [`fields`] were reachable only through the page annotations: the form's `/Fields`
/// list names none or only some of them, so the leniency that adopts them is worth recording.
pub fn adopted_page_fields(doc: &Document) -> usize {
    enumerate(doc).1
}

fn enumerate(doc: &Document) -> (Vec<Field>, usize) {
    let af = acroform(doc);
    let mut page_of = std::collections::HashMap::new();
    let mut annot_index = std::collections::HashMap::new();
    let pages = page_refs(doc);
    for (i, p) in pages.iter().enumerate() {
        if let Some(a) = doc.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()) {
            for (k, e) in doc.resolve(&a).as_array().into_iter().flatten().enumerate() {
                if let Some(r) = e.as_ref() {
                    page_of.entry(r).or_insert(i);
                    annot_index.entry(r).or_insert(k);
                }
            }
        }
    }
    let base = match &af {
        Some(af) => Inherited {
            da: af.get(b"DA").and_then(|o| text_of(&doc.resolve(o))),
            q: af.get(b"Q").and_then(|o| doc.resolve(o).as_int()),
            ..Default::default()
        },
        None => Inherited::default(),
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(af) = &af {
        for f in af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default() {
            if let Some(r) = f.as_ref() {
                walk(doc, r, &base, &page_of, &mut seen, &mut out, 0);
            }
        }
    }
    let listed = out.len();
    adopt_page_fields(doc, &pages, &base, &page_of, &mut seen, &mut out);
    let adopted = out.len() - listed;
    rank_tabs(doc, &pages, &annot_index, &mut out);
    (out, adopted)
}

/// Widget annotations no `/Fields` entry reaches are adopted as fields, as Acrobat and the
/// browsers do: some writers list only some of the fields, or none at all. Each widget is adopted
/// through its topmost unlisted `/Parent`, so a field split across several widgets stays one
/// field. Tree-listed fields keep their order first; the rest follow in page-annotation order.
fn adopt_page_fields(
    doc: &Document,
    pages: &[ObjRef],
    base: &Inherited,
    page_of: &std::collections::HashMap<ObjRef, usize>,
    seen: &mut std::collections::HashSet<ObjRef>,
    out: &mut Vec<Field>,
) {
    for p in pages {
        let Some(a) = doc.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()) else { continue };
        let annots = doc.resolve(&a);
        let Some(annots) = annots.as_array() else { continue };
        for e in annots {
            let Some(r) = e.as_ref() else { continue };
            if seen.contains(&r) {
                continue;
            }
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if d.name(b"Subtype") != Some(b"Widget") {
                continue;
            }
            if !d.contains(b"FT") && !d.contains(b"T") && !d.contains(b"Parent") {
                continue;
            }
            if let Some(root) = unlisted_root(doc, r, seen) {
                walk(doc, root, base, page_of, seen, out, 0);
            }
        }
    }
}

/// The topmost field above `r` (itself without a `/Parent`), or `None` when `r` or an ancestor is
/// already reachable from the `/Fields` tree, or a `/Parent` loop hides the top.
fn unlisted_root(doc: &Document, r: ObjRef, seen: &std::collections::HashSet<ObjRef>) -> Option<ObjRef> {
    let mut top = r;
    let mut visited = std::collections::HashSet::from([r]);
    loop {
        let parent = doc.get(top).as_dict().and_then(|d| d.reference(b"Parent"));
        let Some(p) = parent else {
            return if seen.contains(&top) { None } else { Some(top) };
        };
        if !visited.insert(p) || visited.len() > 64 {
            return None;
        }
        if seen.contains(&p) {
            return None;
        }
        top = p;
    }
}

/// A page's tab order (`/Tabs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabOrder {
    /// Rows, top to bottom, left to right within a row (`/R`).
    Row,
    /// Columns, left to right, top to bottom within a column (`/C`).
    Column,
    /// The document structure (`/S`; annotation order when the file has none).
    Structure,
    /// Unspecified: annotation order.
    Annotations,
}

impl TabOrder {
    fn of(name: Option<&[u8]>) -> TabOrder {
        match name {
            Some(b"R") => TabOrder::Row,
            Some(b"C") => TabOrder::Column,
            Some(b"S") => TabOrder::Structure,
            _ => TabOrder::Annotations,
        }
    }
}

/// A widget on a page for tab ordering: (page, field, widget, rect, annotation index).
type TabItem = (usize, usize, usize, [f64; 4], usize);

/// Every widget's place in the tab order.
fn rank_tabs(doc: &Document, pages: &[ObjRef], annot_index: &std::collections::HashMap<ObjRef, usize>, out: &mut [Field]) {
    let mut items: Vec<TabItem> = Vec::new();
    for (fi, f) in out.iter().enumerate() {
        for (wi, w) in f.widgets.iter().enumerate() {
            if let Some(p) = w.page {
                items.push((p, fi, wi, w.rect, annot_index.get(&w.obj).copied().unwrap_or(usize::MAX)));
            }
        }
    }
    let mut ordered: Vec<(usize, usize)> = Vec::with_capacity(items.len());
    for (pi, p) in pages.iter().enumerate() {
        let mut on: Vec<_> = items.iter().filter(|i| i.0 == pi).collect();
        if on.is_empty() {
            continue;
        }
        let tabs = TabOrder::of(doc.get(*p).as_dict().and_then(|d| d.name(b"Tabs")));
        match tabs {
            TabOrder::Row | TabOrder::Column => {
                let row = tabs == TabOrder::Row;
                // Primary axis: rows by top edge (down the page), columns by left edge.
                on.sort_by(|a, b| if row { b.3[3].total_cmp(&a.3[3]) } else { a.3[0].total_cmp(&b.3[0]) });
                let mut groups: Vec<Vec<&TabItem>> = Vec::new();
                for it in on {
                    let fits = groups.last().is_some_and(|g| {
                        let first = g[0];
                        if row {
                            (first.3[3] - it.3[3]).abs() < ((first.3[3] - first.3[1]) / 2.0).max(2.0)
                        } else {
                            (it.3[0] - first.3[0]).abs() < ((first.3[2] - first.3[0]) / 2.0).max(2.0)
                        }
                    });
                    if fits && let Some(g) = groups.last_mut() {
                        g.push(it);
                    } else {
                        groups.push(vec![it]);
                    }
                }
                for mut g in groups {
                    g.sort_by(|a, b| if row { a.3[0].total_cmp(&b.3[0]) } else { b.3[3].total_cmp(&a.3[3]) });
                    ordered.extend(g.iter().map(|i| (i.1, i.2)));
                }
            }
            TabOrder::Structure | TabOrder::Annotations => {
                on.sort_by_key(|i| i.4);
                ordered.extend(on.iter().map(|i| (i.1, i.2)));
            }
        }
    }
    for f in out.iter_mut() {
        for w in &mut f.widgets {
            w.tab = usize::MAX;
        }
    }
    for (rank, (fi, wi)) in ordered.into_iter().enumerate() {
        out[fi].widgets[wi].tab = rank;
    }
}

/// Set the tab order of pages (0-based).
pub fn set_tab_order(doc: &mut Document, pages: &[usize], order: TabOrder) -> Result<(), FormError> {
    let refs = page_refs(doc);
    for &p in pages {
        let r = *refs.get(p).ok_or_else(|| FormError::Invalid(format!("page {} does not exist", p + 1)))?;
        doc.update_dict(r, |d| match order {
            TabOrder::Row => d.set(b"Tabs".to_vec(), Object::name("R")),
            TabOrder::Column => d.set(b"Tabs".to_vec(), Object::name("C")),
            TabOrder::Structure => d.set(b"Tabs".to_vec(), Object::name("S")),
            TabOrder::Annotations => {
                d.remove(b"Tabs");
            }
        })?;
    }
    Ok(())
}

fn walk(
    doc: &Document,
    r: ObjRef,
    parent: &Inherited,
    page_of: &std::collections::HashMap<ObjRef, usize>,
    seen: &mut std::collections::HashSet<ObjRef>,
    out: &mut Vec<Field>,
    depth: usize,
) {
    if depth > 64 || !seen.insert(r) {
        return;
    }
    let obj = doc.get(r);
    let Some(d) = obj.as_dict() else { return };
    let mut inh = parent.clone();
    if let Some(t) = d.get(b"T").and_then(|o| text_of(&doc.resolve(o))) {
        inh.name = if inh.name.is_empty() { t } else { format!("{}.{t}", inh.name) };
    }
    if let Some(ft) = d.name(b"FT") {
        inh.ft = Some(ft.to_vec());
    }
    if let Some(ff) = d.get(b"Ff").and_then(|o| doc.resolve(o).as_int()) {
        inh.ff = Some(ff as u32);
    }
    for (k, slot) in [(&b"V"[..], &mut inh.v), (b"DV", &mut inh.dv)] {
        if let Some(v) = d.get(k) {
            *slot = Some(v.clone());
        }
    }
    if let Some(da) = d.get(b"DA").and_then(|o| text_of(&doc.resolve(o))) {
        inh.da = Some(da);
    }
    if let Some(q) = d.get(b"Q").and_then(|o| doc.resolve(o).as_int()) {
        inh.q = Some(q);
    }
    if let Some(m) = d.get(b"MaxLen").and_then(|o| doc.resolve(o).as_int()) {
        inh.max_len = usize::try_from(m).ok();
    }
    let kids: Vec<ObjRef> =
        d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default().iter().filter_map(|k| k.as_ref()).collect();
    // Kids with /T are fields; kids without are this field's widgets.
    let field_kids: Vec<ObjRef> = kids.iter().copied().filter(|k| doc.get(*k).as_dict().is_some_and(|kd| kd.contains(b"T"))).collect();
    if !field_kids.is_empty() {
        for k in field_kids {
            walk(doc, k, &inh, page_of, seen, out, depth + 1);
        }
        return;
    }
    let widget_refs: Vec<ObjRef> = if d.contains(b"Rect") { vec![r] } else { kids };
    let actions = actions_of(doc, d, &widget_refs);
    let button =
        (inh.ft.as_deref() == Some(b"Btn") && inh.ff.unwrap_or(0) & flags::PUSH_BUTTON != 0).then(|| button_action(doc, d, &widget_refs)).flatten();
    let ff = inh.ff.unwrap_or(0);
    let kind = match inh.ft.as_deref() {
        Some(b"Tx") => FieldKind::Text,
        Some(b"Btn") if ff & flags::PUSH_BUTTON != 0 => FieldKind::PushButton,
        Some(b"Btn") if ff & flags::RADIO != 0 => FieldKind::Radio,
        Some(b"Btn") => FieldKind::CheckBox,
        Some(b"Ch") if ff & flags::COMBO != 0 => FieldKind::Combo,
        Some(b"Ch") => FieldKind::List,
        Some(b"Sig") => FieldKind::Signature,
        _ => return,
    };
    let widgets = widget_refs
        .into_iter()
        .filter_map(|w| {
            let wo = doc.get(w);
            let wd = wo.as_dict()?;
            let rect = nums(doc, wd.get(b"Rect")).filter(|r| r.len() == 4).unwrap_or_else(|| vec![0.0; 4]);
            let annot_flags = wd.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
            let on_state = wd
                .get(b"AP")
                .map(|ap| doc.resolve(ap))
                .and_then(|ap| ap.as_dict().and_then(|a| a.get(b"N").cloned()))
                .and_then(|n| doc.resolve(&n).as_dict().cloned())
                .and_then(|n| n.iter().map(|(k, _)| name_text(k)).find(|k| k != "Off"));
            Some(Widget {
                obj: w,
                page: page_of.get(&w).copied(),
                rect: [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])],
                on_state: on_state.filter(|_| matches!(kind, FieldKind::CheckBox | FieldKind::Radio)),
                state: wd.name(b"AS").map(name_text),
                tab: usize::MAX,
                locked: annot_flags & 128 != 0,
                hidden: annot_flags & (2 | 32) != 0,
                rotation: appearance::mk_rotation(doc, wd),
            })
        })
        .collect();
    let options = d
        .get(b"Opt")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|o| match &*doc.resolve(o) {
            Object::Array(pair) if pair.len() >= 2 => Some((text_of(&doc.resolve(&pair[0]))?, text_of(&doc.resolve(&pair[1]))?)),
            other => text_of(other).map(|t| (t.clone(), t)),
        })
        .collect();
    out.push(Field {
        name: inh.name.clone(),
        obj: r,
        kind,
        value: values_of(doc, inh.v.as_ref()),
        default: values_of(doc, inh.dv.as_ref()),
        flags: ff,
        max_len: inh.max_len,
        options,
        da: inh.da.clone().unwrap_or_else(|| "/Helv 0 Tf 0 g".into()),
        quadding: inh.q.unwrap_or(0),
        tooltip: d.get(b"TU").and_then(|o| text_of(&doc.resolve(o))),
        widgets,
        actions,
        button,
    });
}

// ── writing ─────────────────────────────────────────────────────────────────────────────────

/// Fill the field `name`.
pub fn set_value(doc: &mut Document, name: &str, value: &FieldValue) -> Result<(), FormError> {
    set_value_with(doc, name, value, &mut NoScripts)
}

/// [`set_value`], running the fields' JavaScript through `scripts`.
pub fn set_value_with(doc: &mut Document, name: &str, value: &FieldValue, scripts: &mut dyn Scripts) -> Result<(), FormError> {
    let all = fields(doc);
    if all.is_empty() {
        return Err(FormError::NoForm);
    }
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    if f.read_only() {
        return Err(FormError::ReadOnly(name.into()));
    }
    write_value(doc, f, value, scripts)?;
    recalculate_with(doc, scripts)?;
    Ok(())
}

/// Run every field's Calculate script in the form's calculation order (`/CO`, then the other
/// calculated fields), as Acrobat does after any value changes. Returns how many changed.
pub fn recalculate(doc: &mut Document) -> Result<usize, FormError> {
    recalculate_with(doc, &mut NoScripts)
}

/// [`recalculate`], running custom Calculate (and Format) scripts through `scripts`.
pub fn recalculate_with(doc: &mut Document, scripts: &mut dyn Scripts) -> Result<usize, FormError> {
    let all = fields(doc);
    let calculated = |f: &Field| f.actions.calculate != af::Calculate::None || f.actions.scripts.calculate.is_some();
    if !all.iter().any(calculated) {
        return Ok(0);
    }
    let co: Vec<ObjRef> = acroform(doc)
        .and_then(|af| af.get(b"CO").map(|c| doc.resolve(c)).and_then(|c| c.as_array().cloned()))
        .unwrap_or_default()
        .iter()
        .filter_map(Object::as_ref)
        .collect();
    let mut order: Vec<String> = co.iter().filter_map(|r| all.iter().find(|f| f.obj == *r)).map(|f| f.name.clone()).collect();
    for f in &all {
        if calculated(f) && !order.contains(&f.name) {
            order.push(f.name.clone());
        }
    }
    let mut changed = 0;
    for name in order {
        let now = fields(doc);
        let Some(f) = now.iter().find(|f| f.name == name) else { continue };
        let lookup = |n: &str| -> Vec<String> {
            let prefix = format!("{n}.");
            now.iter().filter(|x| x.name == n || x.name.starts_with(&prefix)).flat_map(|x| x.value.first().cloned()).collect()
        };
        let current = f.value.first().cloned().unwrap_or_default();
        let v = if let Some(js) = &f.actions.scripts.calculate {
            let f = f.clone();
            let r = scripts.run(FieldEvent::Calculate, js, &f, &current, &now);
            scripting::apply_changes(doc, &r.changes, &f.name)?;
            if !r.rc {
                continue;
            }
            r.value
        } else {
            let Some(v) = af::calculate(&f.actions.calculate, &lookup) else { continue };
            v
        };
        let now = fields(doc);
        let Some(f) = now.iter().find(|f| f.name == name) else { continue };
        if current != v && matches!(f.kind, FieldKind::Text | FieldKind::Combo) {
            let f = f.clone();
            doc.update_dict(f.obj, |d| d.set(b"V".to_vec(), PdfString::text(&v)))?;
            redraw(doc, &f, std::slice::from_ref(&v), scripts)?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn invalid<T>(m: impl Into<String>) -> Result<T, FormError> {
    Err(FormError::Invalid(m.into()))
}

fn write_value(doc: &mut Document, f: &Field, value: &FieldValue, scripts: &mut dyn Scripts) -> Result<(), FormError> {
    match (f.kind, value) {
        (FieldKind::Text, FieldValue::Text(t)) => {
            if let Some(max) = f.max_len
                && t.chars().count() > max
            {
                return invalid(format!("{:?} takes at most {max} characters", f.name));
            }
            // Keystroke (on commit) and Validate, as Acrobat runs them before accepting a value.
            let t = &af::keystroke(&f.actions.format, &f.name, t).map_err(FormError::Invalid)?;
            af::validate(&f.actions.validate, t).map_err(FormError::Invalid)?;
            let t = &scripting::accept(doc, f, t, scripts)?;
            doc.update_dict(f.obj, |d| {
                if t.is_empty() {
                    d.remove(b"V");
                } else {
                    d.set(b"V".to_vec(), PdfString::text(t));
                }
                d.remove(b"RV");
            })?;
            redraw(doc, f, std::slice::from_ref(t), scripts)
        }
        (FieldKind::CheckBox, v) => {
            let state = f.widgets.iter().find_map(|w| w.on_state.clone()).unwrap_or_else(|| "Yes".into());
            let on = match v {
                FieldValue::Check(on) => *on,
                // The on-state name or a yes/no word also work (agents, FDF-style data).
                FieldValue::Text(t) | FieldValue::Radio(Some(t)) => {
                    let t = t.trim();
                    if t == state || f.state_for(t).is_some() || ["yes", "true", "on", "1", "x", "checked"].contains(&t.to_lowercase().as_str()) {
                        true
                    } else if t.is_empty() || ["no", "false", "off", "0", "unchecked"].contains(&t.to_lowercase().as_str()) {
                        false
                    } else {
                        return invalid(format!("{:?} is a check box: use true or false", f.name));
                    }
                }
                FieldValue::Radio(None) => false,
                FieldValue::Choice(_) => return invalid(format!("{:?} is a check box: use true or false", f.name)),
            };
            set_states(doc, f, on.then_some(state.as_str()))
        }
        (FieldKind::Radio, v) => {
            let choice = match v {
                FieldValue::Radio(c) => c.clone(),
                FieldValue::Text(t) if t.is_empty() => None,
                FieldValue::Text(t) => Some(t.clone()),
                FieldValue::Check(false) => None,
                _ => return invalid(format!("{:?} is a radio group: choose one of its options", f.name)),
            };
            // An export value from /Opt selects its widget's on state.
            let choice = choice.map(|c| f.state_for(&c).map(str::to_owned).unwrap_or(c));
            if let Some(c) = &choice
                && !f.widgets.iter().any(|w| w.on_state.as_deref() == Some(c.as_str()))
            {
                let opts: Vec<&str> = (0..f.widgets.len()).filter_map(|i| f.export_of(i)).collect();
                return invalid(format!("{:?} has no option {c:?} (options: {})", f.name, opts.join(", ")));
            }
            if choice.is_none() && f.has(flags::NO_TOGGLE_TO_OFF) && !f.value.is_empty() {
                return invalid(format!("{:?} must keep one option selected", f.name));
            }
            set_states(doc, f, choice.as_deref())
        }
        (FieldKind::Combo | FieldKind::List, FieldValue::Choice(_) | FieldValue::Text(_)) => {
            let vals: Vec<String> = match value {
                FieldValue::Choice(v) => v.clone(),
                FieldValue::Text(t) if t.is_empty() => Vec::new(),
                FieldValue::Text(t) => vec![t.clone()],
                other => return invalid(format!("{:?} can't take {other:?}", f.name)),
            };
            if vals.len() > 1 && !(f.kind == FieldKind::List && f.has(flags::MULTI_SELECT)) {
                return invalid(format!("{:?} takes a single value", f.name));
            }
            // Accept export values or display texts; free text only in editable combo boxes.
            let mut exports = Vec::new();
            for v in &vals {
                match f.options.iter().find(|(e, d)| e == v || d == v) {
                    Some((e, _)) => exports.push(e.clone()),
                    None if f.kind == FieldKind::Combo && f.has(flags::EDIT) => exports.push(v.clone()),
                    None if f.options.is_empty() => exports.push(v.clone()),
                    None => {
                        let opts: Vec<&str> = f.options.iter().map(|(_, d)| d.as_str()).collect();
                        return invalid(format!("{:?} has no option {v:?} (options: {})", f.name, opts.join(", ")));
                    }
                }
            }
            let indices: Vec<Object> =
                exports.iter().filter_map(|e| f.options.iter().position(|(x, _)| x == e)).map(|i| Object::Int(i as i64)).collect();
            doc.update_dict(f.obj, |d| {
                match exports.as_slice() {
                    [] => {
                        d.remove(b"V");
                    }
                    [one] => d.set(b"V".to_vec(), PdfString::text(one)),
                    many => d.set(b"V".to_vec(), Object::Array(many.iter().map(|e| Object::String(PdfString::text(e))).collect())),
                }
                if indices.is_empty() {
                    d.remove(b"I");
                } else {
                    d.set(b"I".to_vec(), Object::Array(indices));
                }
            })?;
            redraw(doc, f, &exports, scripts)
        }
        (FieldKind::PushButton, _) => invalid(format!("{:?} is a button; it has no value", f.name)),
        (FieldKind::Signature, _) => invalid(format!("{:?} is a signature field; sign it with Fill & Sign or a digital ID", f.name)),
        (kind, v) => invalid(format!("{:?} is a {kind:?} field and can't take {v:?}", f.name)),
    }
}

/// Check boxes and radio buttons: `/V` on the field, `/AS` on each widget.
fn set_states(doc: &mut Document, f: &Field, on: Option<&str>) -> Result<(), FormError> {
    let v = on.unwrap_or("Off");
    doc.update_dict(f.obj, |d| d.set(b"V".to_vec(), name_obj(v)))?;
    for w in &f.widgets {
        let state = match (on, w.on_state.as_deref()) {
            (Some(c), Some(s)) if c == s => s,
            _ => "Off",
        };
        // A widget without appearances for its states gets PdfCraft's own.
        let has_ap = doc.get(w.obj).as_dict().and_then(|d| d.get(b"AP").cloned()).is_some();
        if !has_ap {
            let on_name = w.on_state.clone().unwrap_or_else(|| on.unwrap_or("Yes").to_string());
            let ap = appearance::check_box_states(doc, w, f.kind, &on_name);
            doc.update_dict(w.obj, |d| d.set(b"AP".to_vec(), Object::Dict(ap)))?;
        }
        doc.update_dict(w.obj, |d| d.set(b"AS".to_vec(), name_obj(state)))?;
    }
    Ok(())
}

/// Copy an existing appearance dictionary before replacing its normal appearance.
fn appearance_dict(doc: &Document, widget: ObjRef) -> Dict {
    doc.get(widget).as_dict().and_then(|d| d.get(b"AP").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned()).unwrap_or_default()
}

/// `widget`'s appearance dictionary (a copy, so a shared one is left alone) with the freshly
/// drawn entries of `fresh` (its `/N`). Other entries are kept, but the old down and rollover
/// appearances (`/D`, `/R`) would show the old value, caption or colour on press or hover, so
/// they go — except, for check boxes and radio buttons (whose `/N` is a dictionary of states),
/// the states the new `/N` still draws under the same names.
pub(crate) fn merged_appearance(doc: &Document, widget: ObjRef, fresh: &Dict) -> Dict {
    let mut apd = appearance_dict(doc, widget);
    // Only a dictionary of states counts (`as_dict` would also see a stream's own dictionary).
    let states_of = |o: &Object| match &*doc.resolve(o) {
        Object::Dict(d) => Some(d.clone()),
        _ => None,
    };
    let states: Option<Vec<Vec<u8>>> = fresh.get(b"N").and_then(states_of).map(|d| d.iter().map(|(k, _)| k.clone()).collect());
    for key in [&b"D"[..], &b"R"[..]] {
        if fresh.contains(key) {
            continue;
        }
        let kept = states.as_ref().and_then(|names| {
            let old = apd.get(key).and_then(states_of)?;
            let mut d = Dict::new();
            for (k, v) in old.iter().filter(|(k, _)| names.contains(k)) {
                d.set(k.clone(), v.clone());
            }
            (!d.is_empty()).then_some(d)
        });
        match kept {
            Some(d) => apd.set(key.to_vec(), Object::Dict(d)),
            None => {
                apd.remove(key);
            }
        }
    }
    for (k, v) in fresh.iter() {
        apd.set(k.clone(), v.clone());
    }
    apd
}

/// Regenerate the normal appearance of every widget of a text or choice field.
fn redraw(doc: &mut Document, f: &Field, values: &[String], scripts: &mut dyn Scripts) -> Result<(), FormError> {
    let shown = match values {
        [one] if matches!(f.kind, FieldKind::Text | FieldKind::Combo) => scripting::formatted(doc, f, one, scripts),
        _ => None,
    };
    for w in &f.widgets {
        let stream = match &shown {
            Some(s) => appearance::field_appearance_as(doc, f, w, std::slice::from_ref(s), false),
            None => appearance::field_appearance(doc, f, w, values),
        };
        let ap = doc.add(Object::Stream(stream));
        let mut fresh = Dict::new();
        fresh.set(b"N".to_vec(), Object::Ref(ap));
        let apd = merged_appearance(doc, w.obj, &fresh);
        doc.update_dict(w.obj, |d| {
            d.set(b"AP".to_vec(), Object::Dict(apd));
            d.remove(b"AS");
        })?;
    }
    Ok(())
}

/// Acrobat's Clear form: every field (or the named ones) back to its default value.
pub fn reset(doc: &mut Document, names: Option<&[String]>) -> Result<usize, FormError> {
    let all = fields(doc);
    if all.is_empty() {
        return Err(FormError::NoForm);
    }
    if let Some(n) = names
        && let Some(missing) = n.iter().find(|n| !all.iter().any(|f| &f.name == *n))
    {
        return Err(FormError::NoSuchField(missing.clone()));
    }
    let mut changed = 0;
    for f in all.iter().filter(|f| names.is_none_or(|n| n.contains(&f.name))) {
        if f.value == f.default {
            continue;
        }
        let v = match f.kind {
            FieldKind::Text => FieldValue::Text(f.default.first().cloned().unwrap_or_default()),
            FieldKind::CheckBox => FieldValue::Check(!f.default.is_empty()),
            FieldKind::Radio => FieldValue::Radio(f.default.first().cloned()),
            FieldKind::Combo | FieldKind::List => FieldValue::Choice(f.default.clone()),
            FieldKind::PushButton | FieldKind::Signature => continue,
        };
        // A reset is not blocked by NoToggleToOff: go through the writer directly.
        let mut tmp = f.clone();
        tmp.flags &= !flags::NO_TOGGLE_TO_OFF;
        write_value(doc, &tmp, &v, &mut NoScripts)?;
        changed += 1;
    }
    recalculate(doc)?;
    Ok(changed)
}

/// Order tabs manually (Acrobat: Fields ▸ Tab Order ▸ Order Tabs Manually, then drag a field):
/// move field `name` one place earlier or later in its page's tab order. The page switches to
/// annotation order (`/Tabs` removed) and its widgets are reordered in `/Annots`; other
/// annotations keep their places.
pub fn move_in_tab_order(doc: &mut Document, name: &str, earlier: bool) -> Result<(), FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let page = f.widgets.iter().filter_map(|w| w.page).min().ok_or_else(|| FormError::Invalid(format!("{name} is not on a page")))?;
    // The page's fields in their current tab order (a field counts once, at its first widget).
    let mut order: Vec<(usize, &str)> =
        all.iter().filter_map(|g| g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.tab).min().map(|t| (t, g.name.as_str()))).collect();
    order.sort();
    let names: Vec<&str> = order.iter().map(|(_, n)| *n).collect();
    let Some(i) = names.iter().position(|n| *n == name) else { return invalid(format!("{name} is not on a page")) };
    let j = if earlier { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < names.len()) };
    let Some(j) = j else { return Ok(()) };
    let mut names = names.into_iter().map(str::to_string).collect::<Vec<_>>();
    names.swap(i, j);
    // The widgets of this page in the new order.
    let mut widgets: Vec<ObjRef> = Vec::new();
    for n in &names {
        if let Some(g) = all.iter().find(|g| &g.name == n) {
            widgets.extend(g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.obj));
        }
    }
    let pr = *page_refs(doc).get(page).ok_or_else(|| FormError::Invalid("no such page".into()))?;
    let annots_obj =
        doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned()).ok_or_else(|| FormError::Invalid("the page has no annotations".into()))?;
    let list = doc.resolve(&annots_obj).as_array().cloned().unwrap_or_default();
    let mut next = widgets.into_iter();
    let set: std::collections::HashSet<ObjRef> = all.iter().flat_map(|g| g.widgets.iter().filter(|w| w.page == Some(page)).map(|w| w.obj)).collect();
    let new_list: Vec<Object> = list
        .iter()
        .map(|o| match o.as_ref() {
            Some(r) if set.contains(&r) => next.next().map(Object::Ref).unwrap_or_else(|| o.clone()),
            _ => o.clone(),
        })
        .collect();
    match annots_obj {
        Object::Ref(ar) => doc.set(ar, Object::Array(new_list)),
        _ => doc.update_dict(pr, |d| d.set(b"Annots".to_vec(), Object::Array(new_list)))?,
    }
    doc.update_dict(pr, |d| {
        d.remove(b"Tabs");
    })?;
    Ok(())
}
