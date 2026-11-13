//! Field scripts beyond the AF calls: the hook through which the engine runs JavaScript.
//!
//! `forms` does not depend on a JavaScript engine (architecture §3: `forms → js` is not an
//! allowed edge). It calls a [`Scripts`] implementation at the points Acrobat runs field
//! events — Keystroke (on commit) and Validate before a value is accepted, Calculate in the
//! calculation order, Format when the appearance is drawn — and applies what the script asks
//! for: the event's value, `rc`, and changes to other fields.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object, PdfString};

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
    /// Optional exact object target for malformed forms with duplicate field names.
    pub object: Option<ObjRef>,
    pub value: Option<Vec<String>>,
    /// This value came from a Calculate script; redraw it through the field's Format script.
    pub formatted: bool,
    pub read_only: Option<bool>,
    pub required: Option<bool>,
    /// `display.visible` 0, `hidden` 1, `noPrint` 2, `noView` 3.
    pub display: Option<i32>,
    /// Normal push-button caption, applied to every widget of the field.
    pub caption: Option<String>,
    pub text_color: Option<Vec<String>>,
    pub fill_color: Option<Vec<String>>,
    pub rect: Option<[f64; 4]>,
    pub stroke_color: Option<Vec<String>>,
    pub text_size: Option<f64>,
    pub line_width: Option<f64>,
    pub border_style: Option<String>,
    pub user_name: Option<String>,
    pub icon: Option<Option<u64>>,
    /// A combo box's or list box's new items (export value, text), from `setItems`.
    pub options: Option<Vec<(String, String)>>,
    /// JavaScript actions the script installed with `setAction`, by trigger name (`MouseUp`,
    /// `OnFocus`, `Keystroke`, …), each replacing what the field ran for that trigger.
    pub actions: Vec<(String, String)>,
}

/// What a script did.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptResult {
    /// `event.rc`.
    pub rc: bool,
    /// `event.value` afterwards.
    pub value: String,
    /// Whether the script assigned `event.value`, even the value it already had.
    pub value_set: bool,
    pub changes: Vec<FieldChange>,
    /// Why it rejected the value (its alert), for the error shown.
    pub message: Option<String>,
}

/// Runs field scripts (JavaScript).
pub trait Scripts {
    /// Whether the host stopped this edit's callbacks.
    fn stopped(&self) -> bool {
        false
    }

    /// Run `script` for `event` on `target`, whose value is `value`; `fields` is the whole form.
    fn run(&mut self, event: FieldEvent, script: &str, target: &Field, value: &str, fields: &[Field], doc: &Document) -> ScriptResult;

    /// [`Self::run`] again for an appearance, with dates written in English names (`util.printd`)
    /// because no font of the field can draw the date language's (#5303); what it logs was
    /// already logged by the first run. `None` when it can't: no English dates here, or the
    /// script failed. What it returns is what the script made, even when that is the value.
    fn rerun_with_english_dates(
        &mut self,
        _event: FieldEvent,
        _script: &str,
        _target: &Field,
        _value: &str,
        _fields: &[Field],
        _doc: &Document,
    ) -> Option<ScriptResult> {
        None
    }
}

/// No script engine (or JavaScript turned off): scripts change nothing.
pub struct NoScripts;

impl Scripts for NoScripts {
    /// Provide the no-JavaScript fallback: accept the current field value without additional script mutations.
    fn run(&mut self, _: FieldEvent, _: &str, _: &Field, value: &str, _: &[Field], _: &Document) -> ScriptResult {
        ScriptResult { rc: true, value: value.to_string(), value_set: false, changes: Vec::new(), message: None }
    }
}

/// Run the Keystroke (commit) and Validate scripts for a new text value. Returns the value to
/// store, or why it is rejected.
pub(crate) fn accept(doc: &mut Document, f: &Field, value: &str, scripts: &mut dyn Scripts) -> Result<(String, bool), FormError> {
    let mut v = value.to_string();
    let mut refresh_field = false;
    for (event, script, default) in [
        (FieldEvent::Keystroke, &f.actions.scripts.keystroke, "The value entered does not match the format of the field"),
        (FieldEvent::Validate, &f.actions.scripts.validate, "The value entered is not valid for the field"),
    ] {
        let Some(js) = script else { continue };
        let all = fields(doc);
        let r = scripts.run(event, js, f, &v, &all, doc);
        refresh_field |= r.changes.iter().any(|change| change.name == f.name && change.object.is_none_or(|id| id == f.obj));
        apply_changes(doc, &r.changes, &f.name, Some(f.obj), scripts)?;
        if !r.rc {
            return Err(FormError::Invalid(r.message.unwrap_or_else(|| format!("{default} [ {} ]", f.name))));
        }
        v = r.value;
    }
    Ok((v, refresh_field))
}

/// The text a custom Format script shows for `value`, if the field has one.
pub(crate) fn formatted(doc: &Document, f: &Field, value: &str, scripts: &mut dyn Scripts) -> Option<String> {
    let js = f.actions.scripts.format.as_ref()?;
    let r = scripts.run(FieldEvent::Format, js, f, value, &fields(doc), doc);
    Some(r.value)
}

/// Immediate and aliased MK references of one widget; cycles and depth are bounded.
fn widget_mk_refs(doc: &Document, widget: ObjRef) -> Vec<ObjRef> {
    let mut value = doc.get(widget).as_dict().and_then(|d| d.get(b"MK")).cloned();
    let mut seen = Vec::new();
    for _ in 0..64 {
        let Some(Object::Ref(reference)) = value else { break };
        if seen.contains(&reference) {
            break;
        }
        seen.push(reference);
        value = Some(doc.get(reference).as_ref().clone());
    }
    seen
}

/// Keep target widgets' shared indirect MK in place, while outside widgets retain old colors.
fn protect_other_mk_owners(doc: &mut Document, field: &Field, owners: &std::collections::HashMap<ObjRef, Vec<ObjRef>>) -> Result<(), FormError> {
    let own: HashSet<_> = field.widgets.iter().map(|w| w.obj).collect();
    let refs: HashSet<_> = field.widgets.iter().filter_map(|w| doc.get(w.obj).as_dict().and_then(|d| d.reference(b"MK"))).collect();
    for reference in refs {
        for widget in owners.get(&reference).into_iter().flatten().filter(|w| !own.contains(w)) {
            if !widget_mk_refs(doc, *widget).contains(&reference) {
                continue;
            }
            let mk =
                doc.get(*widget).as_dict().and_then(|d| d.get(b"MK")).map(|m| doc.resolve(m)).and_then(|v| v.as_dict().cloned()).unwrap_or_default();
            doc.update_dict(*widget, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
        }
    }
    Ok(())
}

/// Most items a script may give a choice field ([`FieldChange::options`]).
const MAX_SCRIPT_OPTIONS: usize = 10_000;

/// Write a combo box's or list box's items (export value, text) as its `/Opt`: a plain text
/// where the two are the same, a pair where they differ (ISO 32000-2 Table 231). The selected
/// indices (`/I`) no longer apply and go; the value is written after, and the field redrawn.
fn write_options(doc: &mut Document, f: &Field, options: &[(String, String)]) -> Result<(), FormError> {
    if !matches!(f.kind, FieldKind::Combo | FieldKind::List) {
        return Err(FormError::Invalid(format!("{} is not a combo box or list box: it has no items to set", f.name)));
    }
    if options.len() > MAX_SCRIPT_OPTIONS {
        return Err(FormError::Invalid(format!("a script may give a field at most {MAX_SCRIPT_OPTIONS} items")));
    }
    let opt = options
        .iter()
        .map(|(export, text)| {
            if export == text {
                Object::String(PdfString::text(text))
            } else {
                Object::Array(vec![Object::String(PdfString::text(export)), Object::String(PdfString::text(text))])
            }
        })
        .collect();
    doc.update_dict(f.obj, |d| {
        d.set(b"Opt".to_vec(), Object::Array(opt));
        d.remove(b"I");
    })?;
    crate::author::redraw_field_object(doc, &f.name, f.obj)
}

/// Install `script` as field `f`'s JavaScript for `trigger` (a `setAction` trigger name),
/// replacing the action it had there: the field's `/AA` for Keystroke, Validate, Calculate and
/// Format, each widget's `/A` (Mouse Up) or `/AA` for the rest.
fn set_action(doc: &mut Document, f: &Field, trigger: &str, script: &str) -> Result<(), FormError> {
    let event = match trigger {
        "Keystroke" => Some("keystroke"),
        "Validate" => Some("validate"),
        "Calculate" => Some("calculate"),
        "Format" => Some("format"),
        "MouseUp" => Some("mouse_up"),
        _ => None,
    };
    if let Some(event) = event {
        // The field the script's object stands for, even when another shares its name.
        return set_script_of(doc, f, event, Some(script));
    }
    let key: &[u8] = match trigger {
        "MouseDown" => b"D",
        "MouseEnter" => b"E",
        "MouseExit" => b"X",
        "OnFocus" => b"Fo",
        "OnBlur" => b"Bl",
        other => return Err(FormError::Invalid(format!("unknown action trigger {other:?}"))),
    };
    let mut action = pdfcraft_cos::Dict::new();
    action.set(b"S".to_vec(), Object::name("JavaScript"));
    action.set(b"JS".to_vec(), PdfString::text(script));
    let action = doc.add(Object::Dict(action));
    for w in &f.widgets {
        let mut aa =
            doc.get(w.obj).as_dict().and_then(|d| d.get(b"AA").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
        aa.set(key.to_vec(), Object::Ref(action));
        doc.update_dict(w.obj, |d| d.set(b"AA".to_vec(), Object::Dict(aa)))?;
    }
    Ok(())
}

/// [`formatted`] with dates in English names, for widgets whose fonts can't draw the date
/// language's; `None` when the field has no Format script or the English run didn't happen.
pub(crate) fn formatted_in_english(doc: &Document, f: &Field, value: &str, scripts: &mut dyn Scripts) -> Option<String> {
    let js = f.actions.scripts.format.as_ref()?;
    scripts.rerun_with_english_dates(FieldEvent::Format, js, f, value, &fields(doc), doc).map(|r| r.value)
}

/// Apply script changes to other fields (values written without re-running their scripts;
/// read-only, required and visibility as flags; button captions). `except` is the event's own
/// field, whose value the event sets.
pub(crate) fn apply_changes(
    doc: &mut Document,
    changes: &[FieldChange],
    except: &str,
    except_object: Option<ObjRef>,
    scripts: &mut dyn Scripts,
) -> Result<(), FormError> {
    if changes.is_empty() {
        return Ok(());
    }
    // Validate every colour before applying any part of the script's changes.
    for c in changes {
        for color in [&c.text_color, &c.fill_color, &c.stroke_color].into_iter().flatten() {
            js_color(color)?;
        }
    }
    let all = fields(doc);
    let mut mk_owners: std::collections::HashMap<ObjRef, Vec<ObjRef>> = std::collections::HashMap::new();
    if changes.iter().any(|c| c.fill_color.is_some() || c.stroke_color.is_some()) {
        for field in &all {
            for widget in &field.widgets {
                for reference in widget_mk_refs(doc, widget.obj) {
                    mk_owners.entry(reference).or_default().push(widget.obj);
                }
            }
        }
    }

    for c in changes {
        let Some(f) = all.iter().find(|f| f.name == c.name && c.object.is_none_or(|obj| f.obj == obj)) else { continue };
        // New items go first, so a value among them is written against them.
        let reread;
        let f = match &c.options {
            Some(options) => {
                write_options(doc, f, options)?;
                reread = fields(doc).into_iter().find(|x| x.obj == f.obj).unwrap_or_else(|| f.clone());
                &reread
            }
            None => f,
        };
        for (trigger, script) in &c.actions {
            set_action(doc, f, trigger, script)?;
        }
        if let Some(v) = &c.value
            && !except_object.map_or(c.name == except, |obj| f.obj == obj)
            && *v != f.value
        {
            let value = match f.kind {
                FieldKind::Text => Some(FieldValue::Text(v.first().cloned().unwrap_or_default())),
                FieldKind::CheckBox => Some(f.check_box_value(v.first().map(String::as_str))),
                FieldKind::Radio => Some(FieldValue::Radio(v.first().cloned())),
                FieldKind::Combo | FieldKind::List => Some(FieldValue::Choice(v.clone())),
                _ => None,
            };
            if let Some(value) = value {
                let mut tmp = f.clone();
                tmp.flags &= !flags::NO_TOGGLE_TO_OFF;
                tmp.actions = Default::default();
                crate::write_value(doc, &tmp, &value, &mut NoScripts)?;
                if c.formatted {
                    crate::redraw_calculated_value(doc, f, v, scripts)?;
                }
            }
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
        if c.fill_color.is_some() || c.stroke_color.is_some() {
            protect_other_mk_owners(doc, f, &mk_owners)?;
        }
        let appearance = crate::LookPatch {
            text: c.text_color.as_deref().map(js_color).transpose()?.flatten(),
            fill: c.fill_color.as_deref().map(js_color).transpose()?,
            border: c.stroke_color.as_deref().map(js_color).transpose()?,
            width: c.line_width,
            style: c
                .border_style
                .as_deref()
                .map(|style| match style {
                    "solid" => Ok(crate::BorderStyle::Solid),
                    "dashed" => Ok(crate::BorderStyle::Dashed),
                    "beveled" => Ok(crate::BorderStyle::Beveled),
                    "inset" => Ok(crate::BorderStyle::Inset),
                    "underline" => Ok(crate::BorderStyle::Underline),
                    _ => Err(FormError::Invalid("unsupported borderStyle".into())),
                })
                .transpose()?,
            ..Default::default()
        };
        if appearance != crate::LookPatch::default() || c.text_size.is_some() || c.user_name.is_some() {
            crate::author::set_props_object(
                doc,
                &c.name,
                f.obj,
                &crate::FieldProps {
                    appearance: (appearance != crate::LookPatch::default()).then_some(appearance),
                    font_size: c.text_size,
                    tooltip: c.user_name.clone(),
                    ..Default::default()
                },
            )?;
        }
        if let Some(rect) = c.rect {
            if !rect.iter().all(|v| v.is_finite() && v.abs() <= 1e7) || rect[2] < rect[0] || rect[3] < rect[1] {
                return Err(FormError::Invalid("invalid script rectangle".into()));
            }
            let widget = f.widgets.first().ok_or_else(|| FormError::Invalid("field has no widget rectangle".into()))?;
            doc.update_dict(widget.obj, |d| d.set(b"Rect".to_vec(), Object::Array(rect.into_iter().map(Object::Real).collect())))?;
            crate::author::redraw_field_object(doc, &c.name, f.obj)?;
        }
        if let Some(icon) = c.icon {
            if f.kind != FieldKind::PushButton {
                return Err(FormError::Invalid("icons require a push button".into()));
            }
            if icon.is_some_and(|id| id >> 48 != 0) {
                return Err(FormError::Invalid("invalid button icon handle".into()));
            }
            let reference = icon.map(|id| pdfcraft_cos::ObjRef::new((id >> 16) as u32, id as u16));
            if let Some(reference) = reference {
                let obj = doc.get(reference);
                let valid = match &*obj {
                    Object::Stream(stream) => stream.dict.name(b"Subtype") == Some(b"Form".as_slice()),
                    _ => false,
                };
                if !valid {
                    return Err(FormError::Invalid("button icon is not a form in this document".into()));
                }
            }
            for widget in &f.widgets {
                let mut mk = doc
                    .get(widget.obj)
                    .as_dict()
                    .and_then(|d| d.get(b"MK").map(|m| doc.resolve(m)))
                    .and_then(|m| m.as_dict().cloned())
                    .unwrap_or_default();
                match reference {
                    Some(r) => mk.set(b"I".to_vec(), Object::Ref(r)),
                    None => {
                        mk.remove(b"I");
                    }
                }
                doc.update_dict(widget.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
            }
            crate::author::redraw_field_object(doc, &c.name, f.obj)?;
        }
        if let Some(caption) = &c.caption {
            if f.kind != FieldKind::PushButton {
                return Err(FormError::Invalid(format!("{} is not a push button", f.name)));
            }
            crate::author::set_props_object(doc, &f.name, f.obj, &crate::FieldProps { caption_all: Some(caption.clone()), ..Default::default() })?;
        }
    }
    Ok(())
}

/// An Acrobat colour array as the RGB the look model stores, `None` for transparent (`["T"]`).
/// Gray and CMYK convert as Acrobat's `color.convert` does. Anything else (an unknown colour
/// space, the wrong number of components, or a component that is not a finite number between
/// 0 and 1) is refused, and with it the script's changes.
fn js_color(values: &[String]) -> Result<Option<[f64; 3]>, FormError> {
    let invalid = || FormError::Invalid("JavaScript field colours must be T, G, RGB or CMYK arrays with finite components between 0 and 1".into());
    let (space, parts) = values.split_first().ok_or_else(invalid)?;
    let mut numbers = [0.0; 4];
    if parts.len() > numbers.len() {
        return Err(invalid());
    }
    for (slot, part) in numbers.iter_mut().zip(parts) {
        *slot = part.trim().parse::<f64>().ok().filter(|v| v.is_finite() && (0.0..=1.0).contains(v)).ok_or_else(invalid)?;
    }
    match (space.as_str(), parts.len()) {
        ("T", 0) => Ok(None),
        ("G", 1) => Ok(Some([numbers[0]; 3])),
        ("RGB", 3) => Ok(Some([numbers[0], numbers[1], numbers[2]])),
        ("CMYK", 4) => {
            let [c, m, y, k] = numbers;
            Ok(Some([1.0 - (c + k).min(1.0), 1.0 - (m + k).min(1.0), 1.0 - (y + k).min(1.0)]))
        }
        _ => Err(invalid()),
    }
}

/// Apply what a button or console script changed (values, read-only, required, visibility),
/// then recalculate.
pub fn apply_script_changes(doc: &mut Document, changes: &[FieldChange], scripts: &mut dyn Scripts) -> Result<(), FormError> {
    apply_changes(doc, changes, "", None, scripts)?;
    // Acrobat's automatic calculation is driven by value changes. A keypad caption,
    // position or visibility update must not execute every calculated field again.
    if changes.iter().any(|change| change.value.is_some()) {
        crate::recalculate_with(doc, scripts)?;
    }
    Ok(())
}

/// Document-level JavaScripts (the catalog's `/Names /JavaScript` tree), in name order: they
/// define the functions field scripts call.
pub fn document_scripts(doc: &Document) -> Vec<String> {
    document_scripts_named(doc).into_iter().map(|(_, js)| js).collect()
}

/// Most `/Kids` entries the document JavaScript name tree walk reads in all.
const MAX_QUEUED_KIDS: usize = 1 << 20;

/// [`document_scripts`] with their names.
pub fn document_scripts_named(doc: &Document) -> Vec<(String, String)> {
    let Some(root) = doc.root() else { return Vec::new() };
    let cat = doc.get(root);
    let Some(names) = cat.as_dict().and_then(|c| c.get(b"Names")).map(|n| doc.resolve(n)) else { return Vec::new() };
    let Some(tree) = names.as_dict().and_then(|n| n.get(b"JavaScript")).cloned() else { return Vec::new() };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut scripts = crate::script_budget();
    // Kids are indirect references (ISO 32000-2 §7.9.6), queued unresolved, each the first time
    // it is listed: a node listed many times, or among its own kids, must not multiply the work,
    // and the stack holds only references (a direct object there would be copied whole, once
    // for every node listing it). Only the root may be direct. Nodes are told apart by object
    // number, as `Document::get` finds them, so `5 0 R` and `5 1 R` are one node.
    let mut seen: HashSet<u32> = tree.as_ref().map(|r| r.num).into_iter().collect();
    // The `/Names` arrays read, by object number: nodes sharing one would otherwise each list
    // all its scripts again.
    let mut names_read: HashSet<u32> = HashSet::new();
    let mut stack = vec![tree];
    let mut visited = 0;
    // Every `/Kids` entry read, over the whole walk. Nodes sharing one long `/Kids` array would
    // otherwise read all of it once per node.
    let mut queued = 0usize;
    while let Some(raw) = stack.pop() {
        visited += 1;
        if visited > 10_000 {
            break;
        }
        let node = doc.resolve(&raw);
        let Some(d) = node.as_dict() else { continue };
        if let Some(kids) = d.get(b"Kids") {
            in_array(doc, kids, |kids| {
                // Past the limit nothing more is read, but the nodes already queued still are.
                // Pushed in reverse, so the kids are read in the order listed.
                let room = MAX_QUEUED_KIDS.saturating_sub(queued);
                queued = queued.saturating_add(kids.len().min(room));
                let new: Vec<ObjRef> = kids.iter().take(room).filter_map(Object::as_ref).filter(|r| seen.insert(r.num)).collect();
                stack.extend(new.into_iter().rev().map(Object::Ref));
            });
        }
        if let Some(pairs) = d.get(b"Names")
            && pairs.as_ref().is_none_or(|r| names_read.insert(r.num))
        {
            in_array(doc, pairs, |pairs| {
                for pair in pairs.chunks(2) {
                    let [k, v] = pair else { continue };
                    let key = match &*doc.resolve(k) {
                        Object::String(s) => s.to_text(),
                        _ => continue,
                    };
                    if let Some(js) = crate::script(doc, v, &mut scripts) {
                        out.push((key, js));
                    }
                }
            });
        }
    }
    out.sort();
    out
}

/// A document-level action trigger whose JavaScript the viewer runs (ISO 32000-2 §12.6.3,
/// Table 199): the catalog's `/OpenAction`, or its `/AA` entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentTrigger {
    /// The catalog `/OpenAction`, once the document has opened.
    Open,
    /// `/AA /WS`, before the document is saved.
    WillSave,
    /// `/AA /WP`, before the document is printed.
    WillPrint,
}

/// Most actions read for one document trigger, along its `/Next` chain. A chain can be cyclic
/// or arbitrarily long; the rest are ignored.
const MAX_CHAINED_ACTIONS: usize = 256;

/// The JavaScript of the catalog actions for `trigger`, in the order a viewer runs them: the
/// action, then its `/Next` actions depth first (§12.6.2). Actions of other types (a `/GoTo`
/// open action, say) are skipped; their `/Next` chains are still followed. Bounded: each object
/// is read once and at most [`MAX_CHAINED_ACTIONS`] actions are visited.
pub fn document_action_scripts(doc: &Document, trigger: DocumentTrigger) -> Vec<String> {
    let Some(root) = doc.root() else { return Vec::new() };
    let cat = doc.get(root);
    let Some(cat) = cat.as_dict() else { return Vec::new() };
    let first = match trigger {
        DocumentTrigger::Open => cat.get(b"OpenAction").cloned(),
        DocumentTrigger::WillSave | DocumentTrigger::WillPrint => {
            let key: &[u8] = if trigger == DocumentTrigger::WillSave { b"WS" } else { b"WP" };
            cat.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|d| d.get(key)).cloned())
        }
    };
    chained_scripts(doc, first)
}

/// The JavaScript of action `first` and of the actions chained after it, in the order they run:
/// the action, then its `/Next` actions depth first (ISO 32000-2 §12.6.2). Actions of other
/// types are skipped; their `/Next` chains are still followed. Bounded: each object is read
/// once and at most [`MAX_CHAINED_ACTIONS`] actions are visited.
pub(crate) fn chained_scripts(doc: &Document, first: Option<Object>) -> Vec<String> {
    let mut budget = crate::script_budget();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut pending: Vec<Object> = first.into_iter().collect();
    let mut visited = 0usize;
    let mut out = Vec::new();
    while let Some(raw) = pending.pop() {
        visited += 1;
        if visited > MAX_CHAINED_ACTIONS {
            break;
        }
        if let Some(r) = raw.as_ref()
            && !seen.insert(r.num)
        {
            continue;
        }
        let action = doc.resolve(&raw);
        if let Some(list) = action.as_array() {
            // Pushed in reverse so they run in the order listed.
            pending.extend(list.iter().take(MAX_CHAINED_ACTIONS).rev().cloned());
            continue;
        }
        let Some(d) = action.as_dict() else { continue };
        if d.name(b"S") == Some(b"JavaScript".as_slice())
            && let Some(js) = crate::script(doc, &raw, &mut budget)
        {
            out.push(js);
        }
        if let Some(next) = d.get(b"Next") {
            pending.push(next.clone());
        }
    }
    out
}

/// Call `f` on the array `o` is, or refers to, read in place: resolving a direct array would
/// copy it.
fn in_array(doc: &Document, o: &Object, f: impl FnOnce(&[Object])) {
    match o {
        Object::Ref(r) => {
            if let Some(a) = doc.get(*r).as_array() {
                f(a);
            }
        }
        Object::Array(a) => f(a),
        _ => {}
    }
}

/// When a page's actions run: as it becomes the page being read, or stops being it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEvent {
    Open,
    Close,
}

/// A script a page runs as it opens or closes, with the field whose widget carries it (`None`:
/// the page's own action, or an annotation that isn't a field's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageScript {
    pub field: Option<String>,
    pub script: String,
}

/// Most annotations of one page whose actions are read.
const MAX_PAGE_ANNOTATIONS: usize = 10_000;

/// The JavaScript page `page` (0-based) runs for `event`, in the order it runs: the page's own
/// action (`/AA /O` or `/C`), then each annotation's, in the page's `/Annots` order (`/AA /PO`
/// then `/PV` as the page opens, `/PC` then `/PI` as it closes). Other kinds of action, and
/// actions chained with `/Next`, are left out.
pub fn page_scripts(doc: &Document, page: usize, event: PageEvent) -> Vec<PageScript> {
    let Some(page_ref) = crate::page_refs(doc).get(page).copied() else { return Vec::new() };
    let page_obj = doc.get(page_ref);
    let Some(pd) = page_obj.as_dict() else { return Vec::new() };
    let (own, keys): (&[u8], [&[u8]; 2]) = match event {
        PageEvent::Open => (b"O", [b"PO", b"PV"]),
        PageEvent::Close => (b"C", [b"PC", b"PI"]),
    };
    let additional = |d: &pdfcraft_cos::Dict| d.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned());
    let mut budget = crate::script_budget();
    let mut out = Vec::new();
    if let Some(action) = additional(pd).and_then(|aa| aa.get(own).cloned())
        && let Some(script) = javascript(doc, &action, &mut budget)
    {
        out.push(PageScript { field: None, script });
    }
    let annots = pd.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    for item in annots.iter().take(MAX_PAGE_ANNOTATIONS) {
        let annot = doc.resolve(item);
        let Some(ad) = annot.as_dict() else { continue };
        let Some(aa) = additional(ad) else { continue };
        let field = match item {
            Object::Ref(r) if ad.name(b"Subtype") == Some(b"Widget".as_slice()) => crate::qualified_name(doc, *r),
            _ => None,
        };
        for k in keys {
            if let Some(action) = aa.get(k)
                && let Some(script) = javascript(doc, action, &mut budget)
            {
                out.push(PageScript { field: field.clone(), script });
            }
        }
    }
    out
}

/// The script of a JavaScript action (`/S /JavaScript`); `None` for any other action.
fn javascript(doc: &Document, action: &Object, budget: &mut pdfcraft_cos::DecodeBudget) -> Option<String> {
    let a = doc.resolve(action);
    if a.as_dict()?.name(b"S")? != b"JavaScript" {
        return None;
    }
    crate::script(doc, action, budget)
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
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    set_script_of(doc, f, event, js)
}

/// [`set_field_script`] for the field `f` itself, which may share its name with another.
fn set_script_of(doc: &mut Document, f: &Field, event: &str, js: Option<&str>) -> Result<(), FormError> {
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
        crate::calc_order::keep_listed(doc, f.obj, js.is_some())?;
    }
    Ok(())
}
