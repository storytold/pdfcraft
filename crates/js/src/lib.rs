//! pdfcraft-js — Acrobat JavaScript for forms (L3).
//!
//! Field scripts (format, keystroke, validate, calculate), button actions and document-level
//! scripts run in [boa](https://boajs.dev), a JavaScript engine written in Rust, with the subset
//! of Acrobat's object model that forms use, as the public *JavaScript for Acrobat API Reference*
//! describes it:
//!
//! - `event`: `value`, `change`, `willCommit`, `rc`, `name`, `type`, `target`, `targetName`.
//! - the document (`this`, also the global object): `getField`, `numFields`,
//!   `getNthFieldName`, `numPages`, `pageNum`, `documentFileName`, `info`, `resetForm`,
//!   `calculateNow`, `print`, `submitForm`, `mailDoc`.
//! - Field objects: `name`, `type`, `value`, `valueAsString`, `defaultValue`, `readonly`,
//!   `required`, `display`, `hidden`, `numItems`, `getItemAt`, `currentValueIndices`,
//!   `isBoxChecked`, `checkThisBox`, `exportValues`, `charLimit`, `textColor`, `fillColor`,
//!   `setFocus`.
//! - `app` (`alert`, `beep`, `response`, `launchURL`, `execMenuItem`, `viewerType`, `viewerVersion`,
//!   `platform`, `setTimeOut`), `util` (`printf`, `printd`, `printx`, `scand`), `console` (`println`,
//!   `show`, `clear`), `display` and `color`.
//!
//! The engine is a sandbox: no file, network or timer access exists, and loops and recursion
//! are bounded. Side effects (alerts, field changes, resets, printing, navigation) are returned
//! in an [`Outcome`] for the caller to apply.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

use boa_engine::context::HostHooks;
use boa_engine::object::builtins::{JsArray, JsDate, JsFunction};
use boa_engine::object::{FunctionObjectBuilder, ObjectInitializer};
use boa_engine::property::Attribute;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, Source, js_string};

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

    fn single(&self) -> String {
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
    /// Request only: the host must obtain consent before accessing a file.
    OpenDoc(String),
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
    pub alerts: Vec<String>,
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

struct Host {
    fields: Arc<Vec<FieldState>>,
    changed: BTreeSet<usize>,
    alerts: Vec<String>,
    console: Vec<String>,
    requests: Vec<Request>,
    doc: DocInfo,
    /// Bytes of text kept in alerts, console lines and requests, against [`xfa::MAX_PAYLOAD`].
    kept: usize,
    /// Alerts, console lines and requests the limits left out.
    dropped: usize,
}

impl Host {
    /// Room for `bytes` more bytes of kept text: charged when there is, a drop counted when not.
    fn room(&mut self, bytes: usize) -> bool {
        match self.kept.checked_add(bytes).filter(|k| *k <= xfa::MAX_PAYLOAD) {
            Some(k) => {
                self.kept = k;
                true
            }
            None => {
                self.dropped = self.dropped.saturating_add(1);
                false
            }
        }
    }

    fn alert(&mut self, m: String) {
        if self.alerts.len() >= xfa::MAX_ALERTS {
            self.dropped = self.dropped.saturating_add(1);
        } else if self.room(m.len()) {
            self.alerts.push(m);
        }
    }

    fn print(&mut self, m: String) {
        if self.console.len() >= xfa::MAX_CONSOLE {
            self.dropped = self.dropped.saturating_add(1);
        } else if self.room(m.len()) {
            self.console.push(m);
        }
    }

    /// Record `r`, its names and URLs cut to a message's length, up to [`xfa::MAX_EFFECTS`]
    /// requests and the run's byte budget.
    fn request(&mut self, r: Request) {
        if self.requests.len() >= xfa::MAX_EFFECTS {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let r = match r {
            Request::Reset(names) => Request::Reset(names.into_iter().map(xfa::clip).collect()),
            Request::LaunchUrl(u) => Request::LaunchUrl(xfa::clip(u)),
            Request::Submit(u) => Request::Submit(xfa::clip(u)),
            Request::Focus(n) => Request::Focus(xfa::clip(n)),
            other => other,
        };
        let bytes = match &r {
            Request::Reset(names) => names.iter().fold(0usize, |n, s| n.saturating_add(s.len()).saturating_add(std::mem::size_of::<String>())),
            Request::LaunchUrl(u) | Request::Submit(u) | Request::Focus(u) | Request::OpenDoc(u) => u.len(),
            _ => 0,
        };
        if self.room(bytes) {
            self.requests.push(r);
        }
    }
}

type Shared = RefCell<Host>;

/// The host state, installed before scripts run.
fn host(ctx: &Context) -> JsResult<&Shared> {
    ctx.get_data::<Shared>().ok_or_else(|| error("the script host is not set up"))
}

fn s(v: &str) -> JsValue {
    JsValue::from(JsString::from(v))
}

fn text(v: &JsValue, ctx: &mut Context) -> JsResult<String> {
    if v.is_undefined() || v.is_null() {
        return Ok(String::new());
    }
    Ok(v.to_string(ctx)?.to_std_string_escaped())
}

/// [`text`] cut to a message's length ([`xfa::clip`]), without first copying all of a longer
/// string.
fn text_line(v: &JsValue, ctx: &mut Context) -> JsResult<String> {
    if v.is_undefined() || v.is_null() {
        return Ok(String::new());
    }
    let js = v.to_string(ctx)?;
    // Two UTF-16 units per character at most: the clip still sees more than a line's worth.
    let keep = xfa::MAX_LINE.saturating_add(1).saturating_mul(2);
    let js = if js.len() > keep { js.get(..keep).unwrap_or_default() } else { js };
    Ok(xfa::clip(js.to_std_string_escaped()))
}

fn arg(args: &[JsValue], i: usize) -> JsValue {
    args.get(i).cloned().unwrap_or_default()
}

fn error(msg: impl Into<String>) -> JsError {
    JsNativeError::typ().with_message(msg.into()).into()
}

/// A JavaScript number for a text value that reads as one, as Acrobat's `field.value` gives.
fn as_number(t: &str) -> Option<f64> {
    let t = t.trim();
    if t.is_empty() || t.starts_with('+') || t.len() > 1 && t.starts_with('0') && !t.starts_with("0.") {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn function(ctx: &mut Context, f: NativeFunction) -> JsFunction {
    FunctionObjectBuilder::new(ctx.realm(), f).build()
}

fn strings(v: &JsValue, ctx: &mut Context) -> JsResult<Vec<String>> {
    if let Some(o) = v.as_object()
        && o.is_array()
    {
        let a = JsArray::from_object(o.clone())?;
        let n = a.length(ctx)?;
        if n > MAX_ARRAY_LENGTH {
            return Err(error(format!("array length exceeds {MAX_ARRAY_LENGTH}")));
        }
        let mut out = Vec::new();
        for i in 0..n {
            let x = a.get(i, ctx)?;
            out.push(text(&x, ctx)?);
        }
        return Ok(out);
    }
    Ok(if v.is_undefined() || v.is_null() { Vec::new() } else { vec![text(v, ctx)?] })
}

fn colour(c: &Option<Vec<String>>, ctx: &mut Context) -> JsValue {
    match c {
        None => JsArray::from_iter([s("T")], ctx).into(),
        Some(parts) => {
            let vals: Vec<JsValue> =
                parts.iter().enumerate().map(|(i, p)| if i == 0 { s(p) } else { JsValue::from(p.parse::<f64>().unwrap_or(0.0)) }).collect();
            JsArray::from_iter(vals, ctx).into()
        }
    }
}

// ── field objects ───────────────────────────────────────────────────────────────────────────

fn field_index(ctx: &Context, name: &JsString) -> JsResult<usize> {
    let n = name.to_std_string_escaped();
    host(ctx)?.borrow().fields.iter().position(|f| f.name == n).ok_or_else(|| error(format!("no field named {n}")))
}

fn read(ctx: &Context, name: &JsString) -> JsResult<FieldState> {
    let i = field_index(ctx, name)?;
    Ok(host(ctx)?.borrow().fields[i].clone())
}

fn write(ctx: &Context, name: &JsString, change: impl FnOnce(&mut FieldState)) -> JsResult<()> {
    let i = field_index(ctx, name)?;
    let mut h = host(ctx)?.borrow_mut();
    let field = Arc::make_mut(&mut h.fields).get_mut(i).ok_or_else(|| error("the field is no longer available"))?;
    change(field);
    h.changed.insert(i);
    Ok(())
}

fn value_of(f: &FieldState, ctx: &mut Context) -> JsValue {
    match f.kind {
        FieldType::CheckBox | FieldType::RadioButton => s(f.value.first().map_or("Off", String::as_str)),
        FieldType::ListBox if f.value.len() > 1 => {
            let vals: Vec<JsValue> = f.value.iter().map(|v| s(v)).collect();
            JsArray::from_iter(vals, ctx).into()
        }
        _ => {
            let v = f.single();
            as_number(&v).map_or_else(|| s(&v), JsValue::from)
        }
    }
}

fn set_value(f: &mut FieldState, vals: Vec<String>) {
    f.value = match f.kind {
        FieldType::CheckBox | FieldType::RadioButton => vals.into_iter().filter(|v| v != "Off" && !v.is_empty()).take(1).collect(),
        FieldType::ListBox => vals,
        _ => vals.into_iter().take(1).collect(),
    };
}

macro_rules! getter {
    ($ctx:expr, $name:expr, |$f:ident, $c:ident| $body:expr) => {{
        fn get(_: &JsValue, _: &[JsValue], name: &JsString, $c: &mut Context) -> JsResult<JsValue> {
            let $f = read($c, name)?;
            Ok($body)
        }
        function($ctx, NativeFunction::from_copy_closure_with_captures(get, $name.clone()))
    }};
}

fn field_object(ctx: &mut Context, name: &str) -> JsObject {
    let n = JsString::from(name);
    let value_get = getter!(ctx, n, |f, c| value_of(&f, c));
    let value_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let vals = strings(&arg(args, 0), c)?;
            write(c, name, |f| set_value(f, vals))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let as_string = getter!(ctx, n, |f, _c| s(&match f.kind {
        FieldType::CheckBox | FieldType::RadioButton => f.value.first().cloned().unwrap_or_else(|| "Off".into()),
        _ => f.value.join(","),
    }));
    let default = getter!(ctx, n, |f, _c| s(&f.default.first().cloned().unwrap_or_default()));
    let kind = getter!(ctx, n, |f, _c| s(f.kind.name()));
    let fname = getter!(ctx, n, |f, _c| s(&f.name));
    let readonly_get = getter!(ctx, n, |f, _c| JsValue::from(f.readonly));
    let readonly_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let on = arg(args, 0).to_boolean();
            write(c, name, |f| f.readonly = on)?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let required_get = getter!(ctx, n, |f, _c| JsValue::from(f.required));
    let required_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let on = arg(args, 0).to_boolean();
            write(c, name, |f| f.required = on)?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let display_get = getter!(ctx, n, |f, _c| JsValue::from(f.display));
    let display_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let d = arg(args, 0).to_number(c)? as i32;
            write(c, name, |f| f.display = d.clamp(0, 3))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let hidden_get = getter!(ctx, n, |f, _c| JsValue::from(f.display == DISPLAY_HIDDEN));
    let hidden_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let on = arg(args, 0).to_boolean();
            write(c, name, |f| f.display = if on { DISPLAY_HIDDEN } else { DISPLAY_VISIBLE })?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let num_items = getter!(ctx, n, |f, _c| JsValue::from(f.options.len() as f64));
    let char_limit = getter!(ctx, n, |f, _c| JsValue::from(f.char_limit.unwrap_or(0) as f64));
    let export_values = getter!(ctx, n, |f, c| {
        let vals: Vec<JsValue> = f.options.iter().map(|o| s(&o.0)).collect();
        JsArray::from_iter(vals, c).into()
    });
    let indices = getter!(ctx, n, |f, c| {
        let idx: Vec<JsValue> = f.options.iter().enumerate().filter(|(_, o)| f.value.contains(&o.0)).map(|(i, _)| JsValue::from(i as f64)).collect();
        if idx.len() == 1 { idx[0].clone() } else { JsArray::from_iter(idx, c).into() }
    });
    let text_color_get = getter!(ctx, n, |f, c| colour(&f.text_color, c));
    let text_color_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let v = strings(&arg(args, 0), c)?;
            write(c, name, |f| f.text_color = Some(v))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };
    let fill_color_get = getter!(ctx, n, |f, c| colour(&f.fill_color, c));
    let fill_color_set = {
        fn set(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
            let v = strings(&arg(args, 0), c)?;
            write(c, name, |f| f.fill_color = Some(v))?;
            Ok(JsValue::undefined())
        }
        function(ctx, NativeFunction::from_copy_closure_with_captures(set, n.clone()))
    };

    fn get_item_at(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        let f = read(c, name)?;
        let i = arg(args, 0).to_number(c)?;
        // bExportValue defaults to true.
        let export = args.get(1).is_none_or(JsValue::to_boolean);
        let i = if i < 0.0 { f.options.len().saturating_sub(1) } else { i as usize };
        let o = f.options.get(i).ok_or_else(|| error(format!("{} has no item {i}", f.name)))?;
        Ok(s(if export { &o.0 } else { &o.1 }))
    }
    fn is_box_checked(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        let f = read(c, name)?;
        let i = arg(args, 0).to_number(c)? as usize;
        let on = f.options.get(i).is_some_and(|o| f.value.first() == Some(&o.0));
        Ok(JsValue::from(on))
    }
    fn check_this_box(_: &JsValue, args: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        let f = read(c, name)?;
        let i = arg(args, 0).to_number(c)? as usize;
        let on = args.get(1).is_none_or(JsValue::to_boolean);
        let export = f.options.get(i).map(|o| o.0.clone()).ok_or_else(|| error(format!("{} has no widget {i}", f.name)))?;
        write(c, name, |f| {
            if on {
                f.value = vec![export];
            } else if f.value.first() == Some(&export) {
                f.value.clear();
            }
        })?;
        Ok(JsValue::undefined())
    }
    fn set_focus(_: &JsValue, _: &[JsValue], name: &JsString, c: &mut Context) -> JsResult<JsValue> {
        host(c)?.borrow_mut().request(Request::Focus(name.to_std_string_escaped()));
        Ok(JsValue::undefined())
    }

    let rw = Attribute::CONFIGURABLE | Attribute::ENUMERABLE;
    let get_item_at = NativeFunction::from_copy_closure_with_captures(get_item_at, n.clone());
    let is_box_checked = NativeFunction::from_copy_closure_with_captures(is_box_checked, n.clone());
    let check_this_box = NativeFunction::from_copy_closure_with_captures(check_this_box, n.clone());
    let set_focus = NativeFunction::from_copy_closure_with_captures(set_focus, n.clone());
    ObjectInitializer::new(ctx)
        .accessor(js_string!("value"), Some(value_get), Some(value_set), rw)
        .accessor(js_string!("valueAsString"), Some(as_string), None, rw)
        .accessor(js_string!("defaultValue"), Some(default), None, rw)
        .accessor(js_string!("type"), Some(kind), None, rw)
        .accessor(js_string!("name"), Some(fname), None, rw)
        .accessor(js_string!("readonly"), Some(readonly_get), Some(readonly_set), rw)
        .accessor(js_string!("required"), Some(required_get), Some(required_set), rw)
        .accessor(js_string!("display"), Some(display_get), Some(display_set), rw)
        .accessor(js_string!("hidden"), Some(hidden_get), Some(hidden_set), rw)
        .accessor(js_string!("numItems"), Some(num_items), None, rw)
        .accessor(js_string!("charLimit"), Some(char_limit), None, rw)
        .accessor(js_string!("exportValues"), Some(export_values), None, rw)
        .accessor(js_string!("currentValueIndices"), Some(indices), None, rw)
        .accessor(js_string!("textColor"), Some(text_color_get), Some(text_color_set), rw)
        .accessor(js_string!("fillColor"), Some(fill_color_get), Some(fill_color_set), rw)
        .function(get_item_at, js_string!("getItemAt"), 2)
        .function(is_box_checked, js_string!("isBoxChecked"), 1)
        .function(check_this_box, js_string!("checkThisBox"), 2)
        .function(set_focus, js_string!("setFocus"), 0)
        .build()
}

// ── the document (global object) ────────────────────────────────────────────────────────────

fn get_field(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name = text(&arg(args, 0), ctx)?;
    let exists = host(ctx)?.borrow().fields.iter().any(|f| f.name == name);
    if exists {
        return Ok(field_object(ctx, &name).into());
    }
    // A parent name (e.g. "total" for "total.0"): its first kid stands in, as for groups.
    let prefix = format!("{name}.");
    let kid = host(ctx)?.borrow().fields.iter().find(|f| f.name.starts_with(&prefix)).map(|f| f.name.clone());
    Ok(kid.map_or(JsValue::null(), |k| field_object(ctx, &k).into()))
}

fn get_nth_field_name(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let i = arg(args, 0).to_number(ctx)?;
    let name = host(ctx)?.borrow().fields.get(i.max(0.0) as usize).map(|f| f.name.clone());
    Ok(name.map_or(JsValue::null(), |n| s(&n)))
}

fn reset_form(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let names = strings(&arg(args, 0), ctx)?;
    host(ctx)?.borrow_mut().request(Request::Reset(names));
    Ok(JsValue::undefined())
}

fn print(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().request(Request::Print);
    Ok(JsValue::undefined())
}

fn submit_form(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let a = arg(args, 0);
    let url = match a.as_object() {
        Some(o) if !o.is_array() => {
            let u = o.get(js_string!("cURL"), ctx)?;
            text(&u, ctx)?
        }
        _ => text(&a, ctx)?,
    };
    host(ctx)?.borrow_mut().request(Request::Submit(url));
    Ok(JsValue::undefined())
}

fn noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn num_fields(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().fields.len() as f64))
}

fn page_num_get(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(host(ctx)?.borrow().doc.page as f64))
}

fn page_num_set(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let n = arg(args, 0).to_number(ctx)?;
    let mut h = host(ctx)?.borrow_mut();
    let last = h.doc.num_pages.saturating_sub(1);
    let p = (n.max(0.0) as usize).min(last);
    h.doc.page = p;
    h.request(Request::GoToPage(p));
    Ok(JsValue::undefined())
}

// ── app, util, console ──────────────────────────────────────────────────────────────────────

/// A message argument: a string, or an object with `cMsg`, cut to a message's length.
fn message(v: &JsValue, key: &str, ctx: &mut Context) -> JsResult<String> {
    match v.as_object() {
        Some(o) if !o.is_array() && !o.is_callable() => {
            let m = o.get(JsString::from(key), ctx)?;
            text_line(&m, ctx)
        }
        _ => text_line(v, ctx),
    }
}

/// `app.alert(cMsg)`: records the message, given as a string or as an object with `cMsg`, and
/// returns 1 (the OK button).
fn alert(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = message(&arg(args, 0), "cMsg", ctx)?;
    host(ctx)?.borrow_mut().alert(m);
    // The OK button.
    Ok(JsValue::from(1))
}

/// `app.beep()`: queues a request to beep.
fn beep(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().request(Request::Beep);
    Ok(JsValue::undefined())
}

/// `app.launchURL(cURL)`: queues a request to open the URL, given as a string or as an object
/// with `cURL`.
fn launch_url(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let u = message(&arg(args, 0), "cURL", ctx)?;
    host(ctx)?.borrow_mut().request(Request::LaunchUrl(u));
    Ok(JsValue::undefined())
}

/// What `app.execMenuItem(name)` asks for, for the menu items forms use: the side panels, Print
/// and Save As. Other items do nothing.
pub(crate) fn menu_item(name: &str) -> Option<Request> {
    Some(match name {
        "ShowHideFileAttachment" => Request::ShowPanel(Panel::Attachments),
        "ShowHideBookmarks" => Request::ShowPanel(Panel::Bookmarks),
        "ShowHideThumbnails" => Request::ShowPanel(Panel::Pages),
        "ShowHideSignatures" => Request::ShowPanel(Panel::Signatures),
        "ShowHideOptCont" => Request::ShowPanel(Panel::Layers),
        "Print" => Request::Print,
        // Save opens Save As too, as it always has for XFA forms' buttons: a click on a form
        // never overwrites the file unasked.
        "SaveAs" | "Save" => Request::SaveAs,
        _ => return None,
    })
}

/// `app.execMenuItem(name)`: queues the request [`menu_item`] gives for the name (Save opens
/// Save As too); an unknown name does nothing.
fn exec_menu_item(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let item = text(&arg(args, 0), ctx)?;
    if let Some(r) = menu_item(&item) {
        host(ctx)?.borrow_mut().request(r);
    }
    Ok(JsValue::undefined())
}

/// `app.response(…)`: always `null`, the answer for a canceled dialog, since no one is there to
/// type one.
fn response(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    // No one to answer: as if the dialog was cancelled.
    Ok(JsValue::null())
}

fn println(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let m = text_line(&arg(args, 0), ctx)?;
    host(ctx)?.borrow_mut().print(m);
    Ok(JsValue::undefined())
}

fn console_clear(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    host(ctx)?.borrow_mut().console.clear();
    Ok(JsValue::undefined())
}

fn timer(_: &JsValue, _: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    // Timers never fire in the sandbox; scripts get an object they can pass to clear*.
    Ok(ObjectInitializer::new(ctx).build().into())
}

/// `util.printf(cFormat, …)`: `%[,n][+ 0#][width][.prec](d|f|s|x)`, where `,0` groups
/// thousands with commas (the default), `,1` doesn't, `,2` uses `.` and `,`, `,3` uses `.`
/// without grouping.
fn printf(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let fmt = text(&arg(args, 0), ctx)?;
    let mut out = String::new();
    let check_output = |len: usize| -> JsResult<()> {
        if len > MAX_PRINTF_OUTPUT_BYTES {
            return Err(error(format!("util.printf output exceeds {MAX_PRINTF_OUTPUT_BYTES} bytes")));
        }
        Ok(())
    };
    let mut next = 1;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            check_output(out.len().saturating_add(c.len_utf8()))?;
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            check_output(out.len().saturating_add(1))?;
            out.push('%');
            continue;
        }
        let mut sep = 1u8;
        if chars.peek() == Some(&',') {
            chars.next();
            sep = chars.next().and_then(|d| d.to_digit(10)).unwrap_or(0) as u8;
        }
        let (mut plus, mut space, mut zero) = (false, false, false);
        while let Some(&f) = chars.peek() {
            match f {
                '+' => plus = true,
                ' ' => space = true,
                '0' => zero = true,
                '#' => {}
                _ => break,
            }
            chars.next();
        }
        let mut width = 0usize;
        while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
            width = width.saturating_mul(10).saturating_add(d as usize);
            chars.next();
        }
        let mut prec = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p = 0usize;
            while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                p = p.saturating_mul(10).saturating_add(d as usize);
                chars.next();
            }
            prec = Some(p);
        }
        if width > MAX_PRINTF_WIDTH_PRECISION || prec.is_some_and(|p| p > MAX_PRINTF_WIDTH_PRECISION) {
            return Err(error(format!("util.printf width or precision exceeds {MAX_PRINTF_WIDTH_PRECISION}")));
        }
        let conv = chars.next().unwrap_or('s');
        let v = arg(args, next);
        next += 1;
        let body = match conv {
            'd' | 'f' => {
                let n = v.to_number(ctx)?;
                let p = if conv == 'd' { 0 } else { prec.unwrap_or(6) };
                let n = if conv == 'd' { n.trunc() } else { n };
                let mut t = group(&format!("{:.*}", p, n.abs()), sep);
                if n < 0.0 {
                    t.insert(0, '-');
                } else if plus {
                    t.insert(0, '+');
                } else if space {
                    t.insert(0, ' ');
                }
                t
            }
            'x' => format!("{:X}", v.to_number(ctx)? as i64),
            _ => {
                let t = text(&v, ctx)?;
                match prec {
                    Some(p) => t.chars().take(p).collect(),
                    None => t,
                }
            }
        };
        let pad = width.saturating_sub(body.chars().count());
        check_output(out.len().saturating_add(pad).saturating_add(body.len()))?;
        if zero && matches!(conv, 'd' | 'f') {
            let (sign, digits) = if body.starts_with(['-', '+', ' ']) { body.split_at(1) } else { ("", body.as_str()) };
            out.push_str(sign);
            out.push_str(&"0".repeat(pad));
            out.push_str(digits);
        } else {
            out.push_str(&" ".repeat(pad));
            out.push_str(&body);
        }
    }
    Ok(s(&out))
}

/// Group the integer part of a plain decimal number per `util.printf`'s `,n` flag.
fn group(t: &str, sep: u8) -> String {
    let (int, frac) = t.split_once('.').map_or((t, None), |(a, b)| (a, Some(b)));
    let (thousands, point) = match sep {
        0 => (Some(','), '.'),
        2 => (Some('.'), ','),
        3 => (None, ','),
        _ => (None, '.'),
    };
    let mut g = String::new();
    for (i, ch) in int.chars().enumerate() {
        if let Some(th) = thousands
            && i > 0
            && (int.len() - i).is_multiple_of(3)
        {
            g.push(th);
        }
        g.push(ch);
    }
    if let Some(f) = frac {
        g.push(point);
        g.push_str(f);
    }
    g
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// `util.printd(cFormat, oDate)`: yyyy yy mmmm mmm mm m dddd ddd dd d HH H hh h MM M ss s tt,
/// and the numeric formats 0 (D:yyyymmddHHMMss), 1 (yyyy.mm.dd HH:MM:ss) and 2 (m/d/yy h:MM:ss tt).
fn printd(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let f = arg(args, 0);
    let fmt = if f.is_number() {
        match f.to_number(ctx)? as i32 {
            0 => "D:yyyymmddHHMMss".to_string(),
            1 => "yyyy.mm.dd HH:MM:ss".to_string(),
            _ => "m/d/yy h:MM:ss tt".to_string(),
        }
    } else {
        text(&f, ctx)?
    };
    let d = arg(args, 1);
    let Some(date) = d.as_object() else { return Err(error("util.printd needs a Date")) };
    let mut part = |m: &str| -> JsResult<i64> {
        let func = date.get(JsString::from(m), ctx)?;
        let func = func.as_callable().ok_or_else(|| error("util.printd needs a Date"))?;
        let v = func.call(&d, &[], ctx)?.to_number(ctx)?;
        if v.is_nan() {
            return Err(error("invalid date"));
        }
        Ok(v as i64)
    };
    let (y, mo, day, wd, h, mi, sec) =
        (part("getFullYear")?, part("getMonth")?, part("getDate")?, part("getDay")?, part("getHours")?, part("getMinutes")?, part("getSeconds")?);
    let mut out = String::new();
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // Only date letters read as runs; a literal is copied one character at a time, so
        // measuring its run would rescan a long literal stretch at every position.
        let run = if b"ymdHhMst".contains(&c) { b[i..].iter().take_while(|x| **x == c).count() } else { 1 };
        #[cfg(test)]
        PRINTD_SCANNED.fetch_add(run as u64, std::sync::atomic::Ordering::Relaxed);
        let push_num = |out: &mut String, v: i64, n: usize| {
            if n >= 2 {
                out.push_str(&format!("{v:02}"));
            } else {
                out.push_str(&v.to_string());
            }
        };
        match c {
            b'y' => {
                if run >= 4 {
                    out.push_str(&format!("{y:04}"));
                } else {
                    out.push_str(&format!("{:02}", y % 100));
                }
            }
            b'm' => match run {
                r if r >= 4 => out.push_str(MONTHS[mo as usize % 12]),
                3 => out.push_str(&MONTHS[mo as usize % 12][..3]),
                n => push_num(&mut out, mo + 1, n),
            },
            b'd' => match run {
                r if r >= 4 => out.push_str(DAYS[wd as usize % 7]),
                3 => out.push_str(&DAYS[wd as usize % 7][..3]),
                n => push_num(&mut out, day, n),
            },
            b'H' => push_num(&mut out, h, run),
            b'h' => push_num(&mut out, if h % 12 == 0 { 12 } else { h % 12 }, run),
            b'M' => push_num(&mut out, mi, run),
            b's' => push_num(&mut out, sec, run),
            b't' => {
                let ampm = if h < 12 { "am" } else { "pm" };
                out.push_str(if run >= 2 { ampm } else { &ampm[..1] });
            }
            b'\\' if i + 1 < b.len() => {
                // The escaped character may be multi-byte; copy it whole.
                let ch = fmt[i + 1..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += 1 + ch.len_utf8();
                continue;
            }
            _ => {
                // Copy the rest of a multi-byte character too.
                let ch = fmt[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
        }
        i += run;
    }
    Ok(s(&out))
}

/// A field of a `util.scand` format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateUnit {
    /// `yyyy`: four digits.
    Year,
    /// `yy`: two digits, read as 1950–2049.
    ShortYear,
    Month,
    /// `mmm`, `mmmm`: a month's name or its three-letter abbreviation.
    MonthName,
    Day,
    /// `ddd`, `dddd`: a weekday's name, read and not checked.
    Weekday,
    Hour,
    /// `h`, `hh`: 0–12, with `tt` for the afternoon.
    Hour12,
    Minute,
    Second,
    /// `t`, `tt`: am or pm.
    AmPm,
}

impl DateUnit {
    /// A year, month or day: a format needs one of these for `util.scand` to read anything.
    fn is_date(self) -> bool {
        matches!(self, Self::Year | Self::ShortYear | Self::Month | Self::MonthName | Self::Day)
    }

    /// Read from a run of digits, not from letters.
    fn is_numeric(self) -> bool {
        !matches!(self, Self::MonthName | Self::Weekday | Self::AmPm)
    }
}

/// A piece of a `util.scand` format.
#[derive(Debug, PartialEq, Eq)]
enum ScanPart {
    /// A field, the digits it takes when typed together with the next one (`width`), and whether
    /// the next one follows it with nothing between them in the format (`joined`).
    Field { unit: DateUnit, width: usize, joined: bool },
    /// Letters the format writes as they are, which the text must have there.
    Word(String),
}

/// The fields and literal words of a `util.scand` format, in order. A numeric field is marked
/// `joined` when another numeric field follows it directly; anything but letters and digits
/// outside a field is left out.
fn scan_parts(fmt: &str) -> Vec<ScanPart> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut parts: Vec<ScanPart> = Vec::new();
    // The last part is a field written right before this point, or letters still being collected.
    let (mut after_field, mut in_word) = (false, false);
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        // Counted for field letters only, so a long run of anything else is read once.
        let run = if "ymdHhMst".contains(c) { chars.get(i..).unwrap_or_default().iter().take_while(|x| **x == c).count() } else { 1 };
        let field = match (c, run) {
            ('y', 4..) => Some((DateUnit::Year, 4)),
            ('y', _) => Some((DateUnit::ShortYear, 2)),
            ('m', 3..) => Some((DateUnit::MonthName, 0)),
            ('m', n) => Some((DateUnit::Month, n)),
            ('d', 3..) => Some((DateUnit::Weekday, 0)),
            ('d', n) => Some((DateUnit::Day, n)),
            ('H', n) => Some((DateUnit::Hour, n.min(2))),
            ('h', n) => Some((DateUnit::Hour12, n.min(2))),
            ('M', n) => Some((DateUnit::Minute, n.min(2))),
            ('s', n) => Some((DateUnit::Second, n.min(2))),
            ('t', _) => Some((DateUnit::AmPm, 0)),
            _ => None,
        };
        if let Some((unit, width)) = field {
            if after_field
                && unit.is_numeric()
                && let Some(ScanPart::Field { unit: last, joined, .. }) = parts.last_mut()
                && last.is_numeric()
            {
                *joined = true;
            }
            parts.push(ScanPart::Field { unit, width, joined: false });
            (after_field, in_word) = (true, false);
            i += run;
            continue;
        }
        // `\` writes the next character as it is.
        let (literal, step) = if c == '\\' { (chars.get(i + 1).copied(), 2) } else { (Some(c), 1) };
        i += step;
        after_field = false;
        match literal {
            Some(l) if l.is_alphanumeric() => match parts.last_mut() {
                Some(ScanPart::Word(w)) if in_word => w.push(l),
                _ => {
                    parts.push(ScanPart::Word(l.to_string()));
                    in_word = true;
                }
            },
            _ => in_word = false,
        }
    }
    parts
}

/// A run of digits or of letters in the text `util.scand` reads.
enum ScanToken<'a> {
    Digits(&'a str),
    Letters(&'a str),
}

/// The text's runs of digits and of letters, in order; anything else only separates them, and
/// may not begin or end the text. `None` when it does.
fn scan_tokens(text: &str) -> Option<Vec<ScanToken<'_>>> {
    // Some(true) for a digit, Some(false) for a letter, None for a separator.
    let kind = |c: char| (c.is_ascii_digit() || c.is_alphabetic()).then_some(c.is_ascii_digit());
    kind(text.chars().next()?)?;
    kind(text.chars().next_back()?)?;
    let mut out = Vec::new();
    let mut start: Option<(usize, bool)> = None;
    for (at, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        let k = kind(c);
        if let Some((from, digits)) = start
            && k != Some(digits)
        {
            let piece = text.get(from..at)?;
            out.push(if digits { ScanToken::Digits(piece) } else { ScanToken::Letters(piece) });
            start = None;
        }
        if let Some(digits) = k
            && start.is_none()
        {
            start = Some((at, digits));
        }
    }
    Some(out)
}

/// What `util.scand` reads.
#[derive(Debug, PartialEq, Eq)]
enum Scanned {
    /// The text is empty: today, at noon.
    Today,
    /// A date and time: year, month (1–12), day, hours, minutes, seconds.
    At(i32, u32, u32, u32, u32, u32),
}

/// The fields `util.scand` has read so far.
#[derive(Default)]
struct ScanValues {
    y: Option<i32>,
    mo: Option<u32>,
    d: Option<u32>,
    h: Option<u32>,
    mi: Option<u32>,
    sec: Option<u32>,
    pm: Option<bool>,
    /// The hour was read from `h` or `hh`.
    twelve: bool,
}

impl ScanValues {
    /// Records `v` as the value of `unit`: a `yy` year as 1950–2049, and an `h` or `hh` hour
    /// marked as 12-hour. Month names, weekdays and am/pm are not read here.
    fn set(&mut self, unit: DateUnit, v: u32) {
        match unit {
            DateUnit::Year => self.y = i32::try_from(v).ok(),
            DateUnit::ShortYear => self.y = i32::try_from(v).ok().map(|v| if v < 50 { 2000 + v } else { 1900 + v }),
            DateUnit::Month => self.mo = Some(v),
            DateUnit::Day => self.d = Some(v),
            DateUnit::Hour => self.h = Some(v),
            DateUnit::Hour12 => {
                self.h = Some(v);
                self.twelve = true;
            }
            DateUnit::Minute => self.mi = Some(v),
            DateUnit::Second => self.sec = Some(v),
            DateUnit::MonthName | DateUnit::Weekday | DateUnit::AmPm => {}
        }
    }
}

/// Reads `text` by a `util.scand` format, as another viewer was observed to (black-box):
///
/// - Fields may be separated by anything but digits and letters, whether or not the format
///   separates them, but nothing may come before the first or after the last.
/// - Digits typed together fill the format's fields written together with exactly as many
///   digits as each has letters (`yyyymd`: 4, 1 and 1). A run of digits for one field alone may
///   have one or two, except a four-digit year.
/// - `yy` is 1950–2049; month names may be written in full or as three letters, in any case.
/// - Impossible dates are refused, but a month or day of 0 is read as 1.
/// - A format with no year, month or day reads nothing; an empty text is today, at noon.
///
/// Not observed: a year left out is `this_year`, and a month or day left out is 1.
fn scan_date(fmt: &str, text: &str, this_year: i32) -> Option<Scanned> {
    let parts = scan_parts(fmt);
    if !parts.iter().any(|p| matches!(p, ScanPart::Field { unit, .. } if unit.is_date())) {
        return None;
    }
    if text.is_empty() {
        return Some(Scanned::Today);
    }
    let tokens = scan_tokens(text)?;
    let mut v = ScanValues::default();
    let (mut p, mut t) = (0, 0);
    while let Some(part) = parts.get(p) {
        let token = tokens.get(t)?;
        t += 1;
        match (part, token) {
            (ScanPart::Word(w), ScanToken::Letters(l) | ScanToken::Digits(l)) => {
                if w.to_lowercase() != l.to_lowercase() {
                    return None;
                }
                p += 1;
            }
            (ScanPart::Field { unit: DateUnit::MonthName, .. }, ScanToken::Letters(l)) => {
                v.mo = Some(named(&MONTHS, l)?.saturating_add(1));
                p += 1;
            }
            (ScanPart::Field { unit: DateUnit::Weekday, .. }, ScanToken::Letters(l)) => {
                named(&DAYS, l)?;
                p += 1;
            }
            (ScanPart::Field { unit: DateUnit::AmPm, .. }, ScanToken::Letters(l)) => {
                v.pm = Some(match l.to_lowercase().as_str() {
                    "am" | "a" => false,
                    "pm" | "p" => true,
                    _ => return None,
                });
                p += 1;
            }
            (ScanPart::Field { unit, width, joined }, ScanToken::Digits(run)) if unit.is_numeric() => {
                // The fields written together from here, no more than the run's digits can fill,
                // and the fewest of them the run fills.
                let mut group = vec![(*unit, *width)];
                let (mut more, mut widths) = (*joined, *width);
                while more
                    && widths < run.len()
                    && let Some(ScanPart::Field { unit, width, joined }) = parts.get(p + group.len())
                {
                    group.push((*unit, *width));
                    widths = widths.saturating_add(*width);
                    more = *joined;
                }
                let mut sum = 0;
                let fills = group.iter().position(|(_, w)| {
                    sum += w;
                    sum == run.len()
                });
                if let Some(last) = fills {
                    let mut at = 0;
                    for (unit, w) in group.iter().take(last + 1) {
                        v.set(*unit, run.get(at..at + w)?.parse().ok()?);
                        at += w;
                    }
                    p += last + 1;
                } else if *unit != DateUnit::Year && run.len() <= 2 {
                    v.set(*unit, run.parse().ok()?);
                    p += 1;
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if t != tokens.len() {
        return None;
    }
    let y = v.y.unwrap_or(this_year);
    let mo = v.mo.unwrap_or(1).max(1);
    let d = v.d.unwrap_or(1).max(1);
    if mo > 12 || i64::from(d) > i64::from(formcalc::days_in(i64::from(y), mo)) {
        return None;
    }
    let mut h = v.h.unwrap_or(0);
    if v.twelve && h > 12 {
        return None;
    }
    match v.pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    let (mi, sec) = (v.mi.unwrap_or(0), v.sec.unwrap_or(0));
    (h <= 23 && mi <= 59 && sec <= 59).then_some(Scanned::At(y, mo, d, h, mi, sec))
}

/// The index of the name `word` is, in full or as its first three letters, in any case.
fn named(names: &[&str], word: &str) -> Option<u32> {
    let word = word.to_lowercase();
    let i = names.iter().position(|n| {
        let n = n.to_lowercase();
        word == n || n.get(..3) == Some(word.as_str())
    })?;
    u32::try_from(i).ok()
}

/// `util.scand(cFormat, cDate)`: the date `cDate` gives by a `util.printd` format, as a local
/// `Date`, or `null` when it doesn't read as one (see [`scan_date`]). Only text is read.
fn scand(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(typed) = arg(args, 1).as_string().map(|t| t.to_std_string_escaped()) else {
        return Ok(JsValue::null());
    };
    let fmt = text(&arg(args, 0), ctx)?;
    let date = JsDate::new(ctx);
    let this_year = date.get_full_year(ctx)?.to_number(ctx)?;
    let this_year = if this_year.is_finite() { this_year as i32 } else { 2000 };
    match scan_date(&fmt, &typed, this_year) {
        None => return Ok(JsValue::null()),
        Some(Scanned::Today) => {
            date.set_hours(&[12.into(), 0.into(), 0.into(), 0.into()], ctx)?;
        }
        Some(Scanned::At(y, mo, d, h, mi, sec)) => {
            // setFullYear, not the constructor, so years 0–99 stay as they are.
            date.set_full_year(&[y.into(), (mo.saturating_sub(1)).into(), d.into()], ctx)?;
            date.set_hours(&[h.into(), mi.into(), sec.into(), 0.into()], ctx)?;
        }
    }
    Ok(date.into())
}

/// `util.printx(cFormat, cSource)`: `?` any character, `X` letter or digit, `A` letter, `9`
/// digit, `*` the rest, `\` escape, `>` / `<` / `=` upper / lower / as is.
fn printx(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let fmt: Vec<char> = text(&arg(args, 0), ctx)?.chars().collect();
    let src: Vec<char> = text(&arg(args, 1), ctx)?.chars().collect();
    let (mut out, mut j, mut case) = (String::new(), 0, 0);
    let mut i = 0;
    let push = |out: &mut String, c: char, case: i32| match case {
        1 => out.extend(c.to_uppercase()),
        -1 => out.extend(c.to_lowercase()),
        _ => out.push(c),
    };
    while i < fmt.len() {
        let f = fmt[i];
        i += 1;
        let take = |j: &mut usize, ok: &dyn Fn(char) -> bool| -> Option<char> {
            while *j < src.len() {
                let c = src[*j];
                *j += 1;
                if ok(c) {
                    return Some(c);
                }
            }
            None
        };
        match f {
            '?' => {
                if let Some(c) = take(&mut j, &|_| true) {
                    push(&mut out, c, case)
                }
            }
            'X' => {
                if let Some(c) = take(&mut j, &|c| c.is_alphanumeric()) {
                    push(&mut out, c, case)
                }
            }
            'A' => {
                if let Some(c) = take(&mut j, &|c| c.is_alphabetic()) {
                    push(&mut out, c, case)
                }
            }
            '9' => {
                if let Some(c) = take(&mut j, &|c| c.is_ascii_digit()) {
                    push(&mut out, c, case)
                }
            }
            '*' => {
                for c in src[j.min(src.len())..].iter().copied() {
                    push(&mut out, c, case);
                }
                j = src.len();
            }
            '\\' if i < fmt.len() => {
                out.push(fmt[i]);
                i += 1;
            }
            '>' => case = 1,
            '<' => case = -1,
            '=' => case = 0,
            c => out.push(c),
        }
    }
    Ok(s(&out))
}

/// The `color` object: each named color (`transparent`, `black`, `red`, `cyan`, `gray`, …) as
/// an array of its color space (`T`, `G`, `RGB` or `CMYK`) followed by its components.
fn color_object(ctx: &mut Context) -> JsObject {
    let mut entries: Vec<(&str, Vec<JsValue>)> = vec![
        ("transparent", vec![s("T")]),
        ("black", vec![s("G"), 0.into()]),
        ("white", vec![s("G"), 1.into()]),
        ("red", vec![s("RGB"), 1.into(), 0.into(), 0.into()]),
        ("green", vec![s("RGB"), 0.into(), 1.into(), 0.into()]),
        ("blue", vec![s("RGB"), 0.into(), 0.into(), 1.into()]),
        ("cyan", vec![s("CMYK"), 1.into(), 0.into(), 0.into(), 0.into()]),
        ("magenta", vec![s("CMYK"), 0.into(), 1.into(), 0.into(), 0.into()]),
        ("yellow", vec![s("CMYK"), 0.into(), 0.into(), 1.into(), 0.into()]),
        ("dkGray", vec![s("G"), JsValue::from(0.25)]),
        ("gray", vec![s("G"), JsValue::from(0.5)]),
        ("ltGray", vec![s("G"), JsValue::from(0.75)]),
    ];
    let arrays: Vec<(&str, JsValue)> = entries.drain(..).map(|(k, v)| (k, JsArray::from_iter(v, ctx).into())).collect();
    let mut o = ObjectInitializer::new(ctx);
    for (k, v) in arrays {
        o.property(JsString::from(k), v, Attribute::all());
    }
    o.build()
}

/// The XFA version scripts see as `xfa_version`.
const XFA_VERSION: f64 = 3.5;

/// Defines the form object model's globals: the document's functions (`getField`, `resetForm`,
/// `print`, `submitForm` and others) and properties (`numFields`, `pageNum`, `numPages`,
/// `documentFileName`, `info`, the XFA version), and the `app`, `util`, `console`, `display` and
/// `color` objects.
fn install(ctx: &mut Context) -> JsResult<()> {
    let globals: [(&str, usize, NativeFunction); 9] = [
        ("getField", 1, NativeFunction::from_fn_ptr(get_field)),
        ("getNthFieldName", 1, NativeFunction::from_fn_ptr(get_nth_field_name)),
        ("resetForm", 1, NativeFunction::from_fn_ptr(reset_form)),
        ("print", 1, NativeFunction::from_fn_ptr(print)),
        ("submitForm", 1, NativeFunction::from_fn_ptr(submit_form)),
        ("mailDoc", 1, NativeFunction::from_fn_ptr(noop)),
        ("calculateNow", 0, NativeFunction::from_fn_ptr(noop)),
        ("syncAnnotScan", 0, NativeFunction::from_fn_ptr(noop)),
        ("dirty", 0, NativeFunction::from_fn_ptr(noop)),
    ];
    for (name, len, f) in globals {
        ctx.register_global_callable(JsString::from(name), len, f)?;
    }
    let global = ctx.global_object();
    let rw = Attribute::CONFIGURABLE;
    let nf = function(ctx, NativeFunction::from_fn_ptr(num_fields));
    let pg = function(ctx, NativeFunction::from_fn_ptr(page_num_get));
    let ps = function(ctx, NativeFunction::from_fn_ptr(page_num_set));
    let (file, pages, info, xfa) = {
        let h = host(ctx)?.borrow();
        (h.doc.file_name.clone(), h.doc.num_pages, h.doc.info.clone(), h.doc.xfa)
    };
    let mut io = ObjectInitializer::new(ctx);
    for (k, v) in &info {
        io.property(JsString::from(k.as_str()), s(v), Attribute::all());
    }
    let info = io.build();
    use boa_engine::property::PropertyDescriptor;
    global.define_property_or_throw(js_string!("numFields"), PropertyDescriptor::builder().get(nf).configurable(true), ctx)?;
    global.define_property_or_throw(js_string!("pageNum"), PropertyDescriptor::builder().get(pg).set(ps).configurable(true), ctx)?;
    ctx.register_global_property(js_string!("numPages"), JsValue::from(pages as f64), rw)?;
    ctx.register_global_property(js_string!("documentFileName"), s(&file), rw)?;
    ctx.register_global_property(js_string!("info"), info, rw)?;
    // XFA forms check both before they run: every document sees the XFA version, and only an
    // XFA form sees `xfa_installed`.
    ctx.register_global_property(js_string!("xfa_version"), JsValue::from(XFA_VERSION), rw)?;
    if xfa {
        ctx.register_global_property(js_string!("xfa_installed"), JsValue::from(true), rw)?;
    }

    let app = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(alert), js_string!("alert"), 1)
        .function(NativeFunction::from_fn_ptr(beep), js_string!("beep"), 0)
        .function(NativeFunction::from_fn_ptr(response), js_string!("response"), 1)
        .function(NativeFunction::from_fn_ptr(launch_url), js_string!("launchURL"), 2)
        .function(NativeFunction::from_fn_ptr(popup_menu), js_string!("popUpMenuEx"), 0)
        .function(NativeFunction::from_fn_ptr(open_doc), js_string!("openDoc"), 1)
        .function(NativeFunction::from_fn_ptr(exec_menu_item), js_string!("execMenuItem"), 1)
        .function(NativeFunction::from_fn_ptr(timer), js_string!("setTimeOut"), 2)
        .function(NativeFunction::from_fn_ptr(timer), js_string!("setInterval"), 2)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clearTimeOut"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("clearInterval"), 1)
        .property(js_string!("viewerType"), s("Exchange-Pro"), Attribute::READONLY)
        .property(js_string!("viewerVariation"), s("Full"), Attribute::READONLY)
        .property(js_string!("viewerVersion"), JsValue::from(24.0), Attribute::READONLY)
        .property(js_string!("formsVersion"), JsValue::from(24.0), Attribute::READONLY)
        .property(js_string!("platform"), s(platform()), Attribute::READONLY)
        .property(js_string!("language"), s("ENU"), Attribute::READONLY)
        .build();
    ctx.register_global_property(js_string!("app"), app, rw)?;
    let util = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(printf), js_string!("printf"), 1)
        .function(NativeFunction::from_fn_ptr(printd), js_string!("printd"), 2)
        .function(NativeFunction::from_fn_ptr(printx), js_string!("printx"), 2)
        .function(NativeFunction::from_fn_ptr(scand), js_string!("scand"), 2)
        .build();
    ctx.register_global_property(js_string!("util"), util, rw)?;
    let console = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(println), js_string!("println"), 1)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("show"), 0)
        .function(NativeFunction::from_fn_ptr(noop), js_string!("hide"), 0)
        .function(NativeFunction::from_fn_ptr(console_clear), js_string!("clear"), 0)
        .build();
    ctx.register_global_property(js_string!("console"), console, rw)?;
    let display = ObjectInitializer::new(ctx)
        .property(js_string!("visible"), DISPLAY_VISIBLE, Attribute::READONLY)
        .property(js_string!("hidden"), DISPLAY_HIDDEN, Attribute::READONLY)
        .property(js_string!("noPrint"), DISPLAY_NO_PRINT, Attribute::READONLY)
        .property(js_string!("noView"), DISPLAY_NO_VIEW, Attribute::READONLY)
        .build();
    ctx.register_global_property(js_string!("display"), display, rw)?;
    let color = color_object(ctx);
    ctx.register_global_property(js_string!("color"), color, rw)?;
    Ok(())
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "MAC"
    } else if cfg!(target_os = "windows") {
        "WIN"
    } else {
        "UNIX"
    }
}

/// Build a fresh event object: a named field takes precedence as `target`; without one,
/// `document_target` selects the document/global object, otherwise target is null.
/// `targetName` is empty without a field. `run_here` enables document targets for Link
/// and its synthetic Doc/Open initialization; it also owns the global event binding's
/// non-enumerability, which is separate from this object's property attributes.
fn event_object(ctx: &mut Context, e: &Event, document_target: bool) -> JsObject {
    let target: JsValue = match &e.target {
        Some(t) => field_object(ctx, t).into(),
        None if document_target => ctx.global_object().into(),
        None => JsValue::null(),
    };
    ObjectInitializer::new(ctx)
        .property(js_string!("name"), s(&e.name), Attribute::all())
        .property(js_string!("type"), s(&e.kind), Attribute::all())
        .property(js_string!("value"), s(&e.value), Attribute::all())
        .property(js_string!("change"), s(&e.change), Attribute::all())
        .property(js_string!("changeEx"), s(&e.change), Attribute::all())
        .property(js_string!("willCommit"), e.will_commit, Attribute::all())
        .property(js_string!("rc"), true, Attribute::all())
        .property(js_string!("target"), target, Attribute::all())
        .property(js_string!("targetName"), s(e.target.as_deref().unwrap_or("")), Attribute::all())
        .property(js_string!("selStart"), 0, Attribute::all())
        .property(js_string!("selEnd"), 0, Attribute::all())
        .property(js_string!("commitKey"), 0, Attribute::all())
        .property(js_string!("modifier"), false, Attribute::all())
        .property(js_string!("shift"), false, Attribute::all())
        .build()
}

// ── stack safety ────────────────────────────────────────────────────────────────────────────
//
// boa's parser and compiler recurse once per nesting level and have no limit of their own: about
// a hundred nested parentheses overflow a 2 MiB stack, which aborts the whole app. Scripts come
// from documents, so `run` refuses nesting no real form needs and runs the engine on a thread
// with a large stack (address space, committed only as it is used). Together these bound the
// stack a script can use: brackets and prefix operators by the limits below, and every other
// construct (a few KiB per level, several bytes of source each) by the length limit.

/// Bytes `util.printd` has scanned for runs of a format character, for tests that bound its work.
#[cfg(test)]
pub(crate) static PRINTD_SCANNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Longest script, in bytes.
pub(crate) const MAX_SCRIPT_BYTES: usize = 256 * 1024;
/// Largest width or precision accepted by `util.printf`.
const MAX_PRINTF_WIDTH_PRECISION: usize = 4096;
/// Longest `util.printf` output, in bytes.
const MAX_PRINTF_OUTPUT_BYTES: usize = 1 << 20;
/// Longest array read item by item (`resetForm`, a field's `value` or colours). A sparse array's
/// length costs the script nothing but the host one string per item; real lists are tiny.
const MAX_ARRAY_LENGTH: u64 = 1 << 20;
/// Deepest nesting of `(`, `[` and `{`.
const MAX_BRACKET_DEPTH: usize = 64;
/// Longest run of prefix operators (`!`, `~`, `+`, `-`).
const MAX_PREFIX_RUN: usize = 64;
/// Most `?` (conditional expressions nest without brackets).
const MAX_CONDITIONALS: usize = 4096;
/// The script thread's stack.
#[cfg(not(target_arch = "wasm32"))]
const SCRIPT_STACK: usize = 1 << 30;

/// Why `source` is refused, if it is: too long, or nested deeper than any form needs. Strings
/// and comments are skipped; brackets in regular expressions and templates count, which only
/// errs on the safe side.
fn refuse(source: &str) -> Option<String> {
    if source.len() > MAX_SCRIPT_BYTES {
        return Some(format!("the script is too long ({} KiB; the limit is {} KiB)", source.len() / 1024, MAX_SCRIPT_BYTES / 1024));
    }
    let too_deep = || Some("the script is nested too deeply to run safely".to_string());
    let b = source.as_bytes();
    let (mut depth, mut prefix, mut conditionals) = (0usize, 0usize, 0usize);
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        i += 1;
        match c {
            b'\'' | b'"' => {
                // To the closing quote (or the end of the line, where an unclosed string ends).
                while let Some(&d) = b.get(i) {
                    i += 1;
                    if d == b'\\' {
                        i += 1;
                    } else if d == c || d == b'\n' {
                        break;
                    }
                }
                prefix = 0;
            }
            b'/' if b.get(i) == Some(&b'/') => {
                while b.get(i).is_some_and(|d| *d != b'\n') {
                    i += 1;
                }
            }
            b'/' if b.get(i) == Some(&b'*') => {
                i += 1;
                while i < b.len() && !b[i..].starts_with(b"*/") {
                    i += 1;
                }
                i += 2;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > MAX_BRACKET_DEPTH {
                    return too_deep();
                }
                prefix = 0;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                prefix = 0;
            }
            b'!' | b'~' | b'+' | b'-' => {
                prefix += 1;
                if prefix > MAX_PREFIX_RUN {
                    return too_deep();
                }
            }
            b'?' => {
                conditionals += 1;
                if conditionals > MAX_CONDITIONALS {
                    return too_deep();
                }
                prefix = 0;
            }
            c if c.is_ascii_whitespace() => {}
            _ => prefix = 0,
        }
    }
    None
}

/// Run `script` for `event`, after the document-level scripts (`doc_scripts`, which define the
/// functions field scripts call), with the form's `fields`.
pub fn run(script: &str, event: &Event, doc: &DocInfo, fields: &[FieldState], doc_scripts: &[String], limits: Limits) -> Outcome {
    run_shared(script, event, doc, Arc::new(fields.to_vec()), doc_scripts, limits)
}

/// Like [`run`], sharing an immutable initial field snapshot. Scripts still get a fresh
/// context; the first field write copies the snapshot, so neither the caller nor another
/// script can observe uncommitted writes. Read-only calculations avoid copying the whole form.
pub fn run_shared(script: &str, event: &Event, doc: &DocInfo, fields: Arc<Vec<FieldState>>, doc_scripts: &[String], limits: Limits) -> Outcome {
    let failed = |why: String| Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error: Some(why), ..Default::default() };
    if let Some(why) = initialization_limit(doc_scripts).or_else(|| refuse(script)) {
        return failed(why);
    }
    // Link initialization is fail-closed; ordinary form scripts retain upstream's
    // skip-and-report behavior below, within the shared aggregate initialization limit.
    if event.kind == "Link"
        && let Some(why) = doc_scripts.iter().find_map(|d| refuse(d))
    {
        return failed(why);
    }
    // A document script too long or too deeply nested to run safely is skipped and reported,
    // as one that throws is; the others and the script still run (#3940).
    let mut skipped = Vec::new();
    let kept: Vec<(usize, &str)> = doc_scripts
        .iter()
        .enumerate()
        .filter_map(|(i, d)| match refuse(d) {
            Some(why) => {
                skipped.push((i, why));
                None
            }
            None => Some((i, d.as_str())),
        })
        .collect();
    let (kept, skipped) = (&kept, &skipped);
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::scope(|scope| {
            let spawned = std::thread::Builder::new()
                .name("pdfcraft-js".into())
                .stack_size(SCRIPT_STACK)
                .spawn_scoped(scope, || run_here(script, event, doc, fields, kept, skipped, limits));
            match spawned {
                Ok(thread) => thread.join().unwrap_or_else(|_| failed("the script stopped with an internal error".into())),
                Err(e) => failed(format!("the script engine could not start: {e}")),
            }
        })
    }
    #[cfg(target_arch = "wasm32")]
    run_here(script, event, doc, fields, kept, skipped, limits)
}

fn initialization_limit(scripts: &[String]) -> Option<String> {
    if scripts.len() > 1024 || scripts.iter().try_fold(0usize, |total, s| total.checked_add(s.len())).is_none_or(|total| total > 1 << 20) {
        Some("document JavaScript initialization exceeds its limit".into())
    } else {
        None
    }
}

/// Host hooks that refuse to compile code from strings (`eval`, `Function` and its generator
/// and async kin). Such code is built at run time, so it never passed [`refuse`], and boa's
/// recursive parser would overflow the stack on a deeply nested one.
struct NoCodeFromStrings;

impl HostHooks for NoCodeFromStrings {
    fn ensure_can_compile_strings(&self, _: Realm, _: &[JsString], _: &JsString, _: bool, _: &mut Context) -> JsResult<()> {
        Err(JsNativeError::typ().with_message("scripts can't compile code from strings (eval, Function)").into())
    }
}

/// Host hooks that hold code compiled from strings to the checks a script passes ([`refuse`]),
/// for runners that start the script itself through `eval`.
pub(crate) struct CheckedCodeFromStrings;

impl HostHooks for CheckedCodeFromStrings {
    fn ensure_can_compile_strings(&self, _: Realm, parameters: &[JsString], body: &JsString, _: bool, _: &mut Context) -> JsResult<()> {
        let units = parameters.iter().chain(std::iter::once(body)).fold(0usize, |n, s| n.saturating_add(s.len()));
        if units > MAX_SCRIPT_BYTES {
            return Err(JsNativeError::range().with_message("the code is too long to compile").into());
        }
        // As one text: brackets left open in one parameter carry into the next.
        let mut source = parameters.iter().map(JsString::to_std_string_lossy).collect::<Vec<_>>().join(",");
        source.push('\n');
        source.push_str(&body.to_std_string_lossy());
        match refuse(&source) {
            Some(why) => Err(JsNativeError::range().with_message(why).into()),
            None => Ok(()),
        }
    }
}

/// How [`Outcome::doc_errors`] names a failing document script: by its 1-based place.
fn doc_error(index: usize, error: &dyn std::fmt::Display) -> String {
    format!("document script {}: {error}", index.saturating_add(1))
}

/// Evaluate in a fresh Boa context with string-code generation disabled and COW fields.
/// Link actions initialize document scripts under a synthetic Doc/Open event, then restore
/// the real event before the action; other callers initialize under their actual event.
/// A Link initialization error stops evaluation. Other hosts record document-script errors
/// and continue with later initializers and the action, preserving upstream form behavior.
/// Collected field changes, diagnostics and requests can still accompany an error; this
/// function only returns an Outcome, leaving eligibility and effect application to the caller.
/// Callers enforce the aggregate initialization limit and preflight refused Link initializers.
/// `doc_scripts` keeps original indices; `skipped` records ordinary initializers refused
/// before evaluation. Limits are cooperative, not hard wall-time or heap quotas.
fn run_here(
    script: &str,
    event: &Event,
    doc: &DocInfo,
    fields: Arc<Vec<FieldState>>,
    doc_scripts: &[(usize, &str)],
    skipped: &[(usize, String)],
    limits: Limits,
) -> Outcome {
    let mut ctx = match Context::builder().host_hooks(std::rc::Rc::new(NoCodeFromStrings)).build() {
        Ok(ctx) => ctx,
        Err(e) => {
            let error = Some(format!("the script engine could not start: {e}"));
            return Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), error, ..Default::default() };
        }
    };
    ctx.runtime_limits_mut().set_loop_iteration_limit(limits.loop_iterations);
    ctx.runtime_limits_mut().set_recursion_limit(limits.recursion);
    ctx.insert_data(RefCell::new(Host {
        fields,
        changed: BTreeSet::new(),
        alerts: Vec::new(),
        console: Vec::new(),
        requests: Vec::new(),
        doc: doc.clone(),
        kept: 0,
        dropped: 0,
    }));
    let mut out = Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), ..Default::default() };
    let mut doc_errors: Vec<(usize, String)> = skipped.iter().map(|(i, why)| (*i, doc_error(*i, why))).collect();
    let result = (|| -> JsResult<()> {
        install(&mut ctx)?;
        // Link actions have their own document initialization contract. Existing field,
        // button and console callers initialize under their actual event, not a synthetic Open.
        let link = event.kind == "Link";
        let ev = event_object(&mut ctx, event, link);
        // The Link target is the document/global object. Keeping its event binding
        // non-enumerable avoids a global -> event -> target cycle during serialization.
        let event_attributes = if link { Attribute::WRITABLE | Attribute::CONFIGURABLE } else { Attribute::all() };
        ctx.register_global_property(js_string!("event"), ev.clone(), event_attributes)?;
        if link {
            let open_event = event_object(&mut ctx, &Event::doc("Open"), true);
            ctx.register_global_property(js_string!("event"), open_event, event_attributes)?;
        }
        // Unrequested field initialization is tolerant upstream (#3940). A user-activated
        // Link retains its all-or-error initialization contract before its own action runs.
        for &(i, d) in doc_scripts {
            if link {
                ctx.eval(Source::from_bytes(d))?;
            } else if let Err(e) = ctx.eval(Source::from_bytes(d)) {
                doc_errors.push((i, doc_error(i, &e)));
            }
        }
        ctx.register_global_property(js_string!("event"), ev.clone(), event_attributes)?;
        let r = ctx.eval(Source::from_bytes(script))?;
        if !r.is_undefined() {
            out.result = Some(text(&r, &mut ctx)?);
        }
        let rc = ev.get(js_string!("rc"), &mut ctx)?;
        out.rc = rc.to_boolean();
        let v = ev.get(js_string!("value"), &mut ctx)?;
        out.value = text(&v, &mut ctx)?;
        let c = ev.get(js_string!("change"), &mut ctx)?;
        out.change = text(&c, &mut ctx)?;
        Ok(())
    })();
    if let Err(e) = result {
        out.error = Some(e.to_string());
    }
    doc_errors.sort_by_key(|(i, _)| *i);
    out.doc_errors = doc_errors.into_iter().map(|(_, e)| e).collect();
    if let Some(h) = ctx.remove_data::<Shared>() {
        let h = h.into_inner();
        out.changed = h.changed.iter().map(|i| h.fields[*i].clone()).collect();
        out.alerts = h.alerts;
        out.console = h.console;
        out.requests = h.requests;
        if h.dropped > 0 {
            out.console.push(format!("the script's messages, console lines and requests passed their limits; {} more were left out", h.dropped));
        }
    }
    out
}

pub mod formcalc;
pub mod xfa;
pub mod xfa_session;

#[cfg(test)]
mod tests;

/// A menu tree; the UI never evaluates script-provided text.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub name: String,
    pub value: String,
    pub enabled: bool,
    pub marked: bool,
    pub children: Vec<MenuItem>,
}

/// UI rendezvous. Only the script worker waits for the reply.
pub enum InteractiveEvent {
    /// Suspend the worker until a validated leaf value, cancellation or reply timeout.
    Menu(Vec<MenuItem>, std::sync::mpsc::Sender<Option<String>>),
    /// Final outcome; the host must revalidate its document before applying any effects.
    Complete(Outcome),
}
thread_local! {
    static MENU_HOST: RefCell<Option<(std::sync::mpsc::Sender<InteractiveEvent>, usize)>> = const { RefCell::new(None) };
}

/// Read one menu tree, sharing `remaining` across all arguments and descendants.
/// `popup_menu` supplies 1024 entries; roots have depth zero and depths above 16 fail.
/// Names/return values are limited to 4096 UTF-8 bytes each; absent cReturn uses cName.
/// Malformed items, exceeded limits and Boa property/conversion errors are returned.
/// Reading properties may run script code; these bounds are not a hard time/heap quota.
fn read_menu(value: &JsValue, ctx: &mut Context, depth: usize, remaining: &mut usize) -> JsResult<MenuItem> {
    if depth > 16 || *remaining == 0 {
        return Err(error("popup menu exceeds its size or depth limit"));
    }
    *remaining -= 1;
    let obj = value.as_object().ok_or_else(|| error("popup menu item must be an object"))?;
    let name = text(&obj.get(js_string!("cName"), ctx)?, ctx)?;
    let ret = obj.get(js_string!("cReturn"), ctx)?;
    let value = if ret.is_undefined() { name.clone() } else { text(&ret, ctx)? };
    if name.len() > 4096 || value.len() > 4096 {
        return Err(error("popup menu text exceeds its limit"));
    }
    let enabled = obj.get(js_string!("bEnabled"), ctx)?;
    let marked = obj.get(js_string!("bMarked"), ctx)?.to_boolean();
    let sub = obj.get(js_string!("oSubMenu"), ctx)?;
    let mut children = Vec::new();
    if let Some(o) = sub.as_object() {
        if o.is_array() {
            let array = JsArray::from_object(o.clone())?;
            let len = array.length(ctx)?;
            if len > *remaining as u64 {
                return Err(error("popup menu has too many entries"));
            }
            for i in 0..len {
                children.push(read_menu(&array.get(i, ctx)?, ctx, depth + 1, remaining)?);
            }
        } else {
            children.push(read_menu(&sub, ctx, depth + 1, remaining)?);
        }
    } else if !sub.is_undefined() && !sub.is_null() {
        return Err(error("invalid popup submenu"));
    }
    Ok(MenuItem { name, value, enabled: enabled.is_undefined() || enabled.to_boolean(), marked, children })
}

/// Parse a bounded menu and rendezvous with the interactive host, up to 32 times per action.
/// Without a host, a successfully parsed menu returns null. Only the script worker waits
/// for a reply: up to 300 seconds, with timeout, cancellation or reply disconnection
/// yielding null. Sending to a closed host or receiving a forged selection is an error.
/// A selection must match an enabled, non-separator leaf through enabled ancestors.
/// Reply validation does not authorize document effects; the caller revalidates completion.
fn popup_menu(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let mut remaining = 1024;
    let mut items = Vec::new();
    for arg in args {
        items.push(read_menu(arg, ctx, 0, &mut remaining)?);
    }
    let sender = MENU_HOST.with(|h| {
        let mut host = h.borrow_mut();
        let Some((sender, count)) = host.as_mut() else { return Ok(None) };
        if *count >= 32 {
            return Err(error("too many popup menus in one action"));
        }
        *count += 1;
        Ok(Some(sender.clone()))
    })?;
    let Some(sender) = sender else { return Ok(JsValue::null()) };
    let (tx, rx) = std::sync::mpsc::channel();
    sender.send(InteractiveEvent::Menu(items.clone(), tx)).map_err(|_| error("popup host closed"))?;
    let selected = rx.recv_timeout(std::time::Duration::from_secs(300)).ok().flatten();
    fn allowed(items: &[MenuItem], value: &str) -> bool {
        items.iter().any(|i| i.enabled && i.name != "-" && (if i.children.is_empty() { i.value == value } else { allowed(&i.children, value) }))
    }
    match selected {
        Some(value) if allowed(&items, &value) => Ok(s(&value)),
        Some(_) => Err(error("invalid popup menu selection")),
        None => Ok(JsValue::null()),
    }
}

/// Queue OpenDoc as a viewer request and return null, never a live document or file contents.
/// Reject empty/oversized paths, nonempty cFS, non-null oDoc and unsupported positional
/// options. This host performs no file I/O; request retention follows the host's limits.
/// Local-path resolution and consent belong to the frontend, while headless tools return
/// the request as data. A request alone is not permission to open a file.
fn open_doc(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let arg = arg(args, 0);
    let path = if let Some(o) = arg.as_object() {
        let fs = o.get(js_string!("cFS"), ctx)?;
        let base = o.get(js_string!("oDoc"), ctx)?;
        if (!fs.is_undefined() && !text(&fs, ctx)?.is_empty()) || (!base.is_undefined() && !base.is_null()) {
            return Err(error("openDoc supports only the current document's local file system"));
        }
        text(&o.get(js_string!("cPath"), ctx)?, ctx)?
    } else {
        if !superfluous_open_arguments(args) {
            return Err(error("unsupported openDoc options"));
        }
        text(&arg, ctx)?
    };
    if path.is_empty() || path.len() > 4096 {
        return Err(error("invalid openDoc path"));
    }
    host(ctx)?.borrow_mut().request(Request::OpenDoc(path));
    // A document object cannot be returned before host consent. Fail closed when used as one.
    Ok(JsValue::null())
}

/// Start a fresh script worker with the normal cooperative Boa and host limits.
/// There is no hard wall-time/heap bound or admission limit for callers of this API.
/// Dropping the receiver discards queued menus; an already received menu must be
/// cancelled through its reply sender. Abandoning results does not terminate the VM.
#[cfg(not(target_arch = "wasm32"))]
pub fn interactive(
    script: String,
    event: Event,
    doc: DocInfo,
    fields: Vec<FieldState>,
    doc_scripts: Vec<String>,
) -> Result<std::sync::mpsc::Receiver<InteractiveEvent>, String> {
    if let Some(why) =
        initialization_limit(&doc_scripts).or_else(|| std::iter::once(script.as_str()).chain(doc_scripts.iter().map(String::as_str)).find_map(refuse))
    {
        return Err(why);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("pdfcraft-js-interactive".into())
        .stack_size(SCRIPT_STACK)
        .spawn(move || {
            MENU_HOST.with(|h| *h.borrow_mut() = Some((tx.clone(), 0)));
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let kept: Vec<_> = doc_scripts.iter().enumerate().map(|(i, d)| (i, d.as_str())).collect();
                run_here(&script, &event, &doc, Arc::new(fields), &kept, &[], Limits::default())
            }))
            .unwrap_or_else(|_| Outcome { error: Some("script worker failed".into()), ..Default::default() });
            MENU_HOST.with(|h| *h.borrow_mut() = None);
            let _ = tx.send(InteractiveEvent::Complete(out));
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}

fn superfluous_open_arguments(args: &[JsValue]) -> bool {
    args.iter().skip(1).all(|v| v.is_undefined() || v.is_null())
}
