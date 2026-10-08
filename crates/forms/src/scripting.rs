//! Field scripts beyond the AF calls: the hook through which the engine runs JavaScript.
//!
//! `forms` does not depend on a JavaScript engine (architecture §3: `forms → js` is not an
//! allowed edge). It calls a [`Scripts`] implementation at the points Acrobat runs field
//! events — Keystroke (on commit) and Validate before a value is accepted, Calculate in the
//! calculation order, Format when the appearance is drawn — and applies what the script asks
//! for: the event's value, `rc`, and changes to other fields.

use pdfcraft_cos::{Document, Object, PdfString};

use crate::{Field, FieldKind, FieldValue, FormError, fields, flags};

/// The field events forms run scripts for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    Keystroke,
    Validate,
    Calculate,
    Format,
}

impl FieldEvent {
    /// `event.name`.
    pub fn name(self) -> &'static str {
        match self {
            FieldEvent::Keystroke => "Keystroke",
            FieldEvent::Validate => "Validate",
            FieldEvent::Calculate => "Calculate",
            FieldEvent::Format => "Format",
        }
    }
}

/// A change a script made to a field (`None`: unchanged).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldChange {
    pub name: String,
    pub value: Option<Vec<String>>,
    pub read_only: Option<bool>,
    pub required: Option<bool>,
    /// `display.visible` 0, `hidden` 1, `noPrint` 2, `noView` 3.
    pub display: Option<i32>,
}

/// What a script did.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptResult {
    /// `event.rc`.
    pub rc: bool,
    /// `event.value` afterwards.
    pub value: String,
    pub changes: Vec<FieldChange>,
    /// Why it rejected the value (its alert), for the error shown.
    pub message: Option<String>,
}

/// Runs field scripts (JavaScript).
pub trait Scripts {
    /// Run `script` for `event` on `target`, whose value is `value`; `fields` is the whole form.
    fn run(&mut self, event: FieldEvent, script: &str, target: &Field, value: &str, fields: &[Field]) -> ScriptResult;
}

/// No script engine (or JavaScript turned off): scripts change nothing.
pub struct NoScripts;

impl Scripts for NoScripts {
    fn run(&mut self, _: FieldEvent, _: &str, _: &Field, value: &str, _: &[Field]) -> ScriptResult {
        ScriptResult { rc: true, value: value.to_string(), changes: Vec::new(), message: None }
    }
}

/// Run the Keystroke (commit) and Validate scripts for a new text value. Returns the value to
/// store, or why it is rejected.
pub(crate) fn accept(doc: &mut Document, f: &Field, value: &str, scripts: &mut dyn Scripts) -> Result<String, FormError> {
    let mut v = value.to_string();
    for (event, script, default) in [
        (FieldEvent::Keystroke, &f.actions.scripts.keystroke, "The value entered does not match the format of the field"),
        (FieldEvent::Validate, &f.actions.scripts.validate, "The value entered is not valid for the field"),
    ] {
        let Some(js) = script else { continue };
        let all = fields(doc);
        let r = scripts.run(event, js, f, &v, &all);
        apply_changes(doc, &r.changes, &f.name)?;
        if !r.rc {
            return Err(FormError::Invalid(r.message.unwrap_or_else(|| format!("{default} [ {} ]", f.name))));
        }
        v = r.value;
    }
    Ok(v)
}

/// The text a custom Format script shows for `value`, if the field has one.
pub(crate) fn formatted(doc: &Document, f: &Field, value: &str, scripts: &mut dyn Scripts) -> Option<String> {
    let js = f.actions.scripts.format.as_ref()?;
    let r = scripts.run(FieldEvent::Format, js, f, value, &fields(doc));
    Some(r.value)
}

/// Apply script changes to other fields (values written without re-running their scripts;
/// read-only, required and visibility as flags). `except` is the event's own field, whose value
/// the event sets.
pub(crate) fn apply_changes(doc: &mut Document, changes: &[FieldChange], except: &str) -> Result<(), FormError> {
    for c in changes {
        let all = fields(doc);
        let Some(f) = all.iter().find(|f| f.name == c.name) else { continue };
        if let Some(v) = &c.value
            && c.name != except
            && *v != f.value
        {
            let value = match f.kind {
                FieldKind::Text => FieldValue::Text(v.first().cloned().unwrap_or_default()),
                FieldKind::CheckBox => FieldValue::Check(!v.is_empty()),
                FieldKind::Radio => FieldValue::Radio(v.first().cloned()),
                FieldKind::Combo | FieldKind::List => FieldValue::Choice(v.clone()),
                _ => continue,
            };
            let mut tmp = f.clone();
            tmp.flags &= !flags::NO_TOGGLE_TO_OFF;
            tmp.actions = Default::default();
            crate::write_value(doc, &tmp, &value, &mut NoScripts)?;
        }
        let mut ff = f.flags;
        for (on, bit) in [(c.read_only, flags::READ_ONLY), (c.required, flags::REQUIRED)] {
            match on {
                Some(true) => ff |= bit,
                Some(false) => ff &= !bit,
                None => {}
            }
        }
        if ff != f.flags {
            doc.update_dict(f.obj, |d| d.set(b"Ff".to_vec(), Object::Int(ff as i64)))?;
        }
        if let Some(display) = c.display {
            for w in &f.widgets {
                let old = doc.get(w.obj).as_dict().and_then(|d| d.int(b"F")).unwrap_or(0);
                // Hidden 2, Print 4, NoView 32 (Table 167).
                let base = old & !(2 | 4 | 32);
                let new = match display {
                    1 => base | 2 | 4,
                    2 => base,
                    3 => base | 4 | 32,
                    _ => base | 4,
                };
                if new != old {
                    doc.update_dict(w.obj, |d| d.set(b"F".to_vec(), Object::Int(new)))?;
                }
            }
        }
    }
    Ok(())
}

/// Apply what a button or console script changed (values, read-only, required, visibility),
/// then recalculate.
pub fn apply_script_changes(doc: &mut Document, changes: &[FieldChange], scripts: &mut dyn Scripts) -> Result<(), FormError> {
    apply_changes(doc, changes, "")?;
    crate::recalculate_with(doc, scripts)?;
    Ok(())
}

/// Document-level JavaScripts (the catalog's `/Names /JavaScript` tree), in name order: they
/// define the functions field scripts call.
pub fn document_scripts(doc: &Document) -> Vec<String> {
    document_scripts_named(doc).into_iter().map(|(_, js)| js).collect()
}

/// [`document_scripts`] with their names.
pub fn document_scripts_named(doc: &Document) -> Vec<(String, String)> {
    let Some(root) = doc.root() else { return Vec::new() };
    let cat = doc.get(root);
    let Some(names) = cat.as_dict().and_then(|c| c.get(b"Names")).map(|n| doc.resolve(n)) else { return Vec::new() };
    let Some(tree) = names.as_dict().and_then(|n| n.get(b"JavaScript")).map(|t| doc.resolve(t)) else { return Vec::new() };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut stack = vec![(*tree).clone()];
    let mut seen = 0;
    while let Some(node) = stack.pop() {
        seen += 1;
        if seen > 10_000 {
            break;
        }
        let Some(d) = node.as_dict() else { continue };
        if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
            stack.extend(kids.iter().map(|k| (*doc.resolve(k)).clone()));
        }
        if let Some(pairs) = d.get(b"Names").map(|n| doc.resolve(n)).and_then(|n| n.as_array().cloned()) {
            for pair in pairs.chunks(2) {
                let [k, v] = pair else { continue };
                let key = match &*doc.resolve(k) {
                    Object::String(s) => s.to_text(),
                    _ => continue,
                };
                if let Some(js) = crate::script(doc, v) {
                    out.push((key, js));
                }
            }
        }
    }
    out.sort();
    out
}

/// Add (or replace) a document-level JavaScript named `name`.
pub fn set_document_script(doc: &mut Document, name: &str, js: Option<&str>) -> Result<(), FormError> {
    let root = doc.root().ok_or(FormError::NoForm)?;
    let cat = doc.get(root).as_dict().cloned().unwrap_or_default();
    let mut names = cat.get(b"Names").map(|n| doc.resolve(n)).and_then(|n| n.as_dict().cloned()).unwrap_or_default();
    let tree = names.get(b"JavaScript").map(|t| doc.resolve(t)).and_then(|t| t.as_dict().cloned()).unwrap_or_default();
    if tree.contains(b"Kids") {
        return Err(FormError::Invalid("the document's JavaScript name tree has kids; edit it with a full editor".into()));
    }
    let mut pairs: Vec<Object> = tree.get(b"Names").map(|n| doc.resolve(n)).and_then(|n| n.as_array().cloned()).unwrap_or_default();
    let mut kept = Vec::new();
    for pair in pairs.chunks(2) {
        if let [k, v] = pair
            && !matches!(&*doc.resolve(k), Object::String(s) if s.to_text() == name)
        {
            kept.push((k.clone(), v.clone()));
        }
    }
    if let Some(js) = js {
        let mut action = pdfcraft_cos::Dict::new();
        action.set(b"S".to_vec(), Object::name("JavaScript"));
        action.set(b"JS".to_vec(), PdfString::text(js));
        let a = doc.add(Object::Dict(action));
        kept.push((Object::String(PdfString::text(name)), Object::Ref(a)));
    }
    kept.sort_by_key(|(k, _)| match k {
        Object::String(s) => s.to_text(),
        _ => String::new(),
    });
    pairs = kept.into_iter().flat_map(|(k, v)| [k, v]).collect();
    let mut t = pdfcraft_cos::Dict::new();
    t.set(b"Names".to_vec(), Object::Array(pairs));
    names.set(b"JavaScript".to_vec(), Object::Dict(t));
    doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(names)))?;
    Ok(())
}

/// Field Properties ▸ Format / Validate / Calculate ▸ custom script, or Actions ▸ Mouse Up ▸
/// Run a JavaScript: set (or remove, with `None`) field `name`'s script for `event` — one of
/// `keystroke`, `format`, `validate`, `calculate` (the field's `/AA` K, F, V, C) or `mouse_up`
/// (its widgets' `/A`). Calculated fields join the calculation order (`/CO`).
pub fn set_field_script(doc: &mut Document, name: &str, event: &str, js: Option<&str>) -> Result<(), FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?.clone();
    let action = |doc: &mut Document, js: &str| {
        let mut a = pdfcraft_cos::Dict::new();
        a.set(b"S".to_vec(), Object::name("JavaScript"));
        a.set(b"JS".to_vec(), PdfString::text(js));
        Object::Ref(doc.add(Object::Dict(a)))
    };
    if event == "mouse_up" {
        for w in &f.widgets {
            let a = js.map(|j| action(doc, j));
            doc.update_dict(w.obj, |d| match a {
                Some(a) => d.set(b"A".to_vec(), a),
                None => {
                    d.remove(b"A");
                }
            })?;
        }
        return Ok(());
    }
    let key: &[u8] = match event {
        "keystroke" => b"K",
        "format" => b"F",
        "validate" => b"V",
        "calculate" => b"C",
        other => return Err(FormError::Invalid(format!("unknown field event {other:?} (keystroke, format, validate, calculate, mouse_up)"))),
    };
    let mut aa = doc.get(f.obj).as_dict().and_then(|d| d.get(b"AA").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
    match js {
        Some(j) => {
            let a = action(doc, j);
            aa.set(key.to_vec(), a);
        }
        None => {
            aa.remove(key);
        }
    }
    doc.update_dict(f.obj, |d| {
        if aa.is_empty() {
            d.remove(b"AA");
        } else {
            d.set(b"AA".to_vec(), Object::Dict(aa));
        }
    })?;
    if key == b"C" {
        let af_ref = crate::author::ensure_form(doc)?;
        let mut co: Vec<Object> = doc
            .get(af_ref)
            .as_dict()
            .and_then(|d| d.get(b"CO").cloned())
            .map(|c| doc.resolve(&c))
            .and_then(|c| c.as_array().cloned())
            .unwrap_or_default();
        co.retain(|o| o.as_ref() != Some(f.obj));
        if js.is_some() {
            co.push(Object::Ref(f.obj));
        }
        doc.update_dict(af_ref, |d| {
            if co.is_empty() {
                d.remove(b"CO");
            } else {
                d.set(b"CO".to_vec(), Object::Array(co));
            }
        })?;
    }
    Ok(())
}
