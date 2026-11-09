use super::appearances::{declares_pdfa, need_appearances};
use super::read::{acroform, field_flags, fields, listed_indices, listing_fields_named, page_refs, selected_options};
use super::{
    Dict, Document, Field, FieldEvent, FieldKind, FieldValue, FormError, NoScripts, ObjRef, Object, PdfString, Scripts, Widget, af, appearance,
    author, calc_order, flags, invalid, name_obj, scripting,
};

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
    let named: Vec<usize> = all.iter().enumerate().filter(|(_, f)| f.name == name).map(|(i, _)| i).collect();
    fill(doc, &all, &named, name, value, scripts)?;
    recalculate_with(doc, scripts)?;
    Ok(())
}

/// Fill several fields, as [`set_value`] fills each in turn, but reading the form once and
/// recalculating once, after the last value. Filling fields one by one reads the whole form twice
/// per field, which takes minutes on a form with thousands of fields.
///
/// Returns each value's result, in order. The outer `Err` is the recalculation failing, after
/// which, as with [`set_value`], no value counts as filled (though each was written).
pub fn set_values(doc: &mut Document, values: &[(String, FieldValue)]) -> Result<Vec<Result<(), FormError>>, FormError> {
    let mut scripts = NoScripts;
    let mut form = NamedFields::read(doc);
    // The field objects this batch wrote since the form was last read.
    let mut written = std::collections::HashSet::new();
    let mut results = Vec::with_capacity(values.len());
    let mut filled = false;
    for (name, value) in values {
        if form.all.is_empty() {
            results.push(Err(FormError::NoForm));
            continue;
        }
        // A field written earlier in the batch (named twice, or a twin's name) is read again,
        // so this write starts from what the earlier one left, as it would one by one.
        if form.objs(name).any(|o| written.contains(&o)) {
            form = NamedFields::read(doc);
            written.clear();
        }
        let named = form.named(name);
        let result = fill(doc, &form.all, named, name, value, &mut scripts);
        written.extend(form.objs(name));
        filled |= result.is_ok();
        results.push(result);
    }
    if filled {
        recalculate_with(doc, &mut scripts)?;
    }
    Ok(results)
}

/// The form's fields, with the positions of each fully qualified name in them (in field order).
struct NamedFields {
    all: Vec<Field>,
    by_name: std::collections::HashMap<String, Vec<usize>>,
}

impl NamedFields {
    fn read(doc: &Document) -> Self {
        let all = fields(doc);
        let mut by_name: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
        for (i, f) in all.iter().enumerate() {
            by_name.entry(f.name.clone()).or_default().push(i);
        }
        NamedFields { all, by_name }
    }

    fn named(&self, name: &str) -> &[usize] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    /// The field objects named `name`.
    fn objs<'a>(&'a self, name: &str) -> impl Iterator<Item = ObjRef> + 'a {
        self.named(name).iter().filter_map(|&i| self.all.get(i)).map(|f| f.obj)
    }
}

/// Write `value` into the field `name`, without recalculating: `named` are the positions in
/// `all` (the form as read before this write) of the fields with that name, in field order.
fn fill(doc: &mut Document, all: &[Field], named: &[usize], name: &str, value: &FieldValue, scripts: &mut dyn Scripts) -> Result<(), FormError> {
    let mut same = named.iter().filter_map(|&i| all.get(i));
    let first = same.clone().next().ok_or_else(|| FormError::NoSuchField(name.into()))?;
    // Fields with the same fully qualified name are one field (ISO 32000-2 §12.7.4.2), so a radio
    // option any of them draws is an option of the name: it is written through the first one
    // that draws it, and the others turn off below.
    let choice = match value {
        FieldValue::Radio(Some(c)) | FieldValue::Text(c) => Some(c.as_str()),
        _ => None,
    };
    let f = match choice {
        Some(c) if first.kind == FieldKind::Radio => same.find(|t| t.kind == FieldKind::Radio && t.state_for(c).is_some()).unwrap_or(first),
        _ => first,
    };
    // The name is read-only when the first field of it is, whichever field draws the choice; and
    // a radio choice reaches every radio field of the name, so none of them may be read-only.
    let radio_locked = f.kind == FieldKind::Radio && named.iter().filter_map(|&i| all.get(i)).any(|t| t.kind == FieldKind::Radio && t.read_only());
    if f.read_only() || first.read_only() || radio_locked {
        return Err(FormError::ReadOnly(name.into()));
    }
    write_value(doc, f, value, scripts)?;
    author::form_fonts_as_objects(doc, all)?;
    // Fields with the same fully qualified name are one field (§12.7.3.1). Writers that repeat a
    // name as separate field dictionaries, instead of widgets under one parent, get the value
    // the first one took after its scripts, without running them again.
    let twins: Vec<&Field> = named.iter().filter_map(|&i| all.get(i)).filter(|t| t.obj != f.obj && t.kind == f.kind).collect();
    if !twins.is_empty() {
        let taken = fields(doc).into_iter().find(|x| x.obj == f.obj).map(|x| x.value).unwrap_or_default();
        for t in twins {
            share_value(doc, (f.obj, &f.options), t, &taken, scripts)?;
        }
    }
    // A field whose `/Kids` lists radio fields that don't name it as their `/Parent` has their
    // name too, so it takes the same value (§12.7.4.2) rather than keep a stale choice.
    if f.kind == FieldKind::Radio {
        let objs: Vec<ObjRef> = named.iter().filter_map(|&i| all.get(i)).map(|t| t.obj).collect();
        if let Some(v) = doc.get(f.obj).as_dict().and_then(|d| d.get(b"V").cloned()) {
            for p in listing_fields_named(doc, name, &objs) {
                doc.update_dict(p, |d| d.set(b"V".to_vec(), v.clone()))?;
            }
        }
    }
    Ok(())
}

/// Give `twin` the value `source` (a field and its options) holds: its `/V`, and a choice's `/I`
/// mapped to the twin's own options, drawn in the twin's own widgets.
fn share_value(
    doc: &mut Document,
    (source, source_options): (ObjRef, &[(String, String)]),
    twin: &Field,
    value: &[String],
    scripts: &mut dyn Scripts,
) -> Result<(), FormError> {
    match twin.kind {
        FieldKind::CheckBox => {
            let state = twin.widgets.iter().find_map(|w| w.on_state.clone()).unwrap_or_else(|| "Yes".into());
            set_states(doc, twin, (!value.is_empty()).then_some(state.as_str()), None)
        }
        FieldKind::Radio => {
            let choice = value.first().filter(|c| twin.widgets.iter().any(|w| w.on_state.as_deref() == Some(c.as_str())));
            set_states(doc, twin, choice.map(String::as_str), None)?;
            // The same value (§12.7.4.2), even when none of the twin's buttons draws it: they all
            // stay off, and only the field that draws the choice shows it.
            match value.first() {
                Some(v) if choice.is_none() => Ok(doc.update_dict(twin.obj, |d| d.set(b"V".to_vec(), name_obj(v)))?),
                _ => Ok(()),
            }
        }
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            let src = doc.get(source).as_dict().cloned().unwrap_or_default();
            // `/I` holds positions in the source's options: each goes to the twin's first unused option
            // with the same export value and text, its options perhaps in another order. One with no
            // match leaves the twin no `/I` (its `/V` picks), never part of one.
            let positions = match twin.kind {
                FieldKind::Combo | FieldKind::List => {
                    listed_indices(doc, &src, source_options.len()).and_then(|picked| twin_positions(&picked, source_options, &twin.options))
                }
                _ => None,
            };
            doc.update_dict(twin.obj, |d| {
                match src.get(b"V") {
                    Some(v) => d.set(b"V".to_vec(), v.clone()),
                    None => {
                        d.remove(b"V");
                    }
                }
                match &positions {
                    Some(p) => d.set(b"I".to_vec(), Object::Array(p.iter().map(|&i| Object::Int(i64::try_from(i).unwrap_or(i64::MAX))).collect())),
                    None => {
                        d.remove(b"I");
                    }
                }
                d.remove(b"RV");
            })?;
            // Drawn with the options that `/I` selects, not only the export values.
            let mut shown = twin.clone();
            shown.value = value.to_vec();
            if matches!(twin.kind, FieldKind::Combo | FieldKind::List) {
                shown.selected = selected_options(&twin.options, value, positions.as_deref());
            }
            redraw(doc, &shown, value, scripts)
        }
        FieldKind::PushButton | FieldKind::Signature => Ok(()),
    }
}

/// Run every field's Calculate script in the form's calculation order (`/CO`, then the other
/// calculated fields), as Acrobat does after any value changes. Returns how many changed.
pub fn recalculate(doc: &mut Document) -> Result<usize, FormError> {
    recalculate_with(doc, &mut NoScripts)
}

/// [`recalculate`], running custom Calculate (and Format) scripts through `scripts`.
pub fn recalculate_with(doc: &mut Document, scripts: &mut dyn Scripts) -> Result<usize, FormError> {
    let mut all = fields(doc);
    if !all.iter().any(calc_order::is_calculated) {
        return Ok(0);
    }
    // By object: two fields can share a name in a malformed form, and each calculates.
    let order: Vec<ObjRef> = calc_order::run_order(doc, &all).0.into_iter().filter_map(|(i, _)| all.get(i)).map(|f| f.obj).collect();
    let mut changed = 0;
    for obj in order {
        let Some(f) = all.iter().find(|f| f.obj == obj).cloned() else { continue };
        let lookup = |n: &str| -> Vec<String> {
            let prefix = format!("{n}.");
            all.iter().filter(|x| x.name == n || x.name.starts_with(&prefix)).flat_map(|x| x.value.first().cloned()).collect()
        };
        let current = f.value.first().cloned().unwrap_or_default();
        let v = if let Some(js) = &f.actions.scripts.calculate {
            let r = scripts.run(FieldEvent::Calculate, js, &f, &current, &all);
            // A script may set its own field through the field object (`getField("Total").value
            // = …`) rather than through `event.value`. `apply_changes` leaves the event's own
            // field to the event, so that value is the result unless the script also assigned
            // `event.value`, even the value it already had.
            let own = r.changes.iter().find(|c| c.name == f.name).and_then(|c| c.value.as_ref()).map(|v| v.first().cloned().unwrap_or_default());
            scripting::apply_changes(doc, &r.changes, &f.name)?;
            if !r.changes.is_empty() {
                all = fields(doc);
            }
            if !r.rc {
                continue;
            }
            match own {
                Some(v) if !r.value_set => v,
                _ => r.value,
            }
        } else {
            let Some(v) = af::calculate(&f.actions.calculate, &lookup) else { continue };
            v
        };
        // A combo box result naming an option, by export value or display text, is stored as that
        // option's export value (the first element of an /Opt pair, ISO 32000-2 Table 234), as
        // `write_value` stores a choice, so /V, /I and exported form data agree.
        let combo = f.kind == FieldKind::Combo;
        let matched = if combo { option_named(&f, &v) } else { None };
        let v = match matched.and_then(|i| f.options.get(i)) {
            Some((export, _)) => export.clone(),
            None => v,
        };
        if current != v && matches!(f.kind, FieldKind::Text | FieldKind::Combo) {
            // A combo box's /I (its option indices, ISO 32000-2 Table 234) follows the value, as
            // `write_value` keeps it, so the selection kept never disagrees with the value.
            let picked = matched.and_then(|i| i64::try_from(i).ok());
            doc.update_dict(f.obj, |d| {
                d.set(b"V".to_vec(), PdfString::text(&v));
                match picked {
                    Some(i) if combo => d.set(b"I".to_vec(), Object::Array(vec![Object::Int(i)])),
                    _ if combo => {
                        d.remove(b"I");
                    }
                    _ => {}
                }
            })?;
            redraw(doc, &f, std::slice::from_ref(&v), scripts)?;
            if let Some(target) = all.iter_mut().find(|x| x.obj == obj) {
                target.value = vec![v];
            }
            changed += 1;
        }
    }
    Ok(changed)
}

/// The index of the choice option `v` names: the option whose export value it is, else the first
/// whose display text it is, so a display text never shadows a later option's export value.
fn option_named(f: &Field, v: &str) -> Option<usize> {
    f.options.iter().position(|(e, _)| e == v).or_else(|| f.options.iter().position(|(_, d)| d == v))
}

/// The fields whose calculation differs between two readings of a form, matched by field object
/// and named as in `after`: the formulas an edit rewrote, such as a rename's.
pub fn calculations_changed(before: &[Field], after: &[Field]) -> Vec<String> {
    let before: std::collections::HashMap<ObjRef, &af::Actions> = before.iter().map(|f| (f.obj, &f.actions)).collect();
    after
        .iter()
        .filter(|f| before.get(&f.obj).is_some_and(|b| b.calculate != f.actions.calculate || b.scripts.calculate != f.actions.scripts.calculate))
        .map(|f| f.name.clone())
        .collect()
}

pub(crate) fn write_value(doc: &mut Document, f: &Field, value: &FieldValue, scripts: &mut dyn Scripts) -> Result<(), FormError> {
    match (f.kind, value) {
        (FieldKind::Text, FieldValue::Text(t)) => {
            if let Some(max) = f.max_len
                && t.chars().count() > max
            {
                return invalid(format!("{:?} takes at most {max} characters", f.name));
            }
            // Thai this build can't draw is refused (before any script runs), rather than stored
            // and drawn as `?`.
            if let Some(why) = appearance::thai_refusal(doc, f, t) {
                return invalid(format!("{:?} can't take this text: {why}", f.name));
            }
            // Keystroke (on commit) and Validate, as Acrobat runs them before accepting a value.
            let t = &f.actions.keystroke(&f.name, t).map_err(FormError::Format)?;
            af::validate(&f.actions.validate, t).map_err(FormError::Invalid)?;
            let t = &scripting::accept(doc, f, t, scripts)?;
            let shadow = t.is_empty() && crate::author::ancestor_sets(doc, f.obj, b"V");
            doc.update_dict(f.obj, |d| {
                if t.is_empty() && !shadow {
                    d.remove(b"V");
                } else {
                    d.set(b"V".to_vec(), PdfString::text(t));
                }
                d.remove(b"RV");
            })?;
            redraw(doc, f, std::slice::from_ref(t), scripts)
        }
        (FieldKind::CheckBox | FieldKind::Radio, FieldValue::Widget(i)) => {
            // A button can be read-only on its own (its own `/Ff`), though the field isn't.
            if f.widgets.get(*i).is_some_and(|w| w.read_only) {
                return Err(FormError::ReadOnly(f.name.clone()));
            }
            let Some(state) = f.widgets.get(*i).and_then(|w| w.on_state.clone()) else {
                return invalid(format!("{:?} has no button {} with an on state", f.name, i.saturating_add(1)));
            };
            set_states(doc, f, Some(&state), Some(*i))
        }
        (FieldKind::CheckBox, v) => {
            // The kids of one check box field can have different on states, and the field's
            // value is the on state of the kid that is on (ISO 32000-2 §12.7.5.2.3, Table 230).
            // "Checked" keeps the state that is on, else takes the first kid's.
            let first = f.widgets.iter().find_map(|w| w.on_state.clone()).unwrap_or_else(|| "Yes".into());
            let checked = || (0..f.widgets.len()).find(|&i| f.widget_on(i)).and_then(|i| f.widgets.get(i)?.on_state.clone()).unwrap_or(first.clone());
            let state = match v {
                FieldValue::Check(on) => on.then(checked),
                // An on-state name (or `/Opt` export value) turns on the kids that have it; a
                // yes/no word also works (agents, FDF-style data).
                FieldValue::Text(t) | FieldValue::Radio(Some(t)) => {
                    let t = t.trim();
                    if let Some(s) = f.state_for(t) {
                        Some(s.to_owned())
                    } else if t == first || ["yes", "true", "on", "1", "x", "checked"].contains(&t.to_lowercase().as_str()) {
                        Some(checked())
                    } else if t.is_empty() || ["no", "false", "off", "0", "unchecked"].contains(&t.to_lowercase().as_str()) {
                        None
                    } else {
                        return invalid(format!("{:?} is a check box: use true or false", f.name));
                    }
                }
                FieldValue::Radio(None) => None,
                FieldValue::Choice(_) | FieldValue::ChoiceIndices(_) | FieldValue::Widget(_) => {
                    return invalid(format!("{:?} is a check box: use true or false", f.name));
                }
            };
            set_states(doc, f, state.as_deref(), None)
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
            set_states(doc, f, choice.as_deref(), None)
        }
        (FieldKind::Combo | FieldKind::List, FieldValue::Choice(_) | FieldValue::Text(_) | FieldValue::ChoiceIndices(_)) => {
            let count = match value {
                FieldValue::Choice(v) => v.len(),
                FieldValue::ChoiceIndices(v) => v.len(),
                _ => 1,
            };
            if count > 1 && !(f.kind == FieldKind::List && f.has(flags::MULTI_SELECT)) {
                return invalid(format!("{:?} takes a single value", f.name));
            }
            // Each value as its export value and, when it is one of the options, that option's
            // position: two options can share an export value, and the position (`/I`) says
            // which one is picked (ISO 32000-2 Table 234).
            let mut exports = Vec::new();
            let mut picked: Vec<usize> = Vec::new();
            // Each option once in both `/V` and `/I`: a reader that finds them differ (a repeat in one)
            // goes by the export values alone and may pick another option sharing one.
            let mut seen = vec![false; f.options.len()];
            match value {
                FieldValue::ChoiceIndices(positions) => {
                    for &i in positions {
                        let Some((e, _)) = f.options.get(i) else {
                            return invalid(format!("{:?} has no option {} (it has {})", f.name, i.saturating_add(1), f.options.len()));
                        };
                        if first_pick(&mut seen, i) {
                            exports.push(e.clone());
                            picked.push(i);
                        }
                    }
                }
                _ => {
                    let vals: Vec<String> = match value {
                        FieldValue::Choice(v) => v.clone(),
                        FieldValue::Text(t) if t.is_empty() => Vec::new(),
                        FieldValue::Text(t) => vec![t.clone()],
                        other => return invalid(format!("{:?} can't take {other:?}", f.name)),
                    };
                    // Accept export values or display texts; free text only in editable combo boxes.
                    for v in &vals {
                        match option_named(f, v) {
                            Some(i) => {
                                if first_pick(&mut seen, i) {
                                    exports.push(f.options.get(i).map(|(e, _)| e.clone()).unwrap_or_else(|| v.clone()));
                                    picked.push(i);
                                }
                            }
                            None if f.kind == FieldKind::Combo && f.has(flags::EDIT) => exports.push(v.clone()),
                            None if f.options.is_empty() => exports.push(v.clone()),
                            None => {
                                let opts: Vec<&str> = f.options.iter().map(|(_, d)| d.as_str()).collect();
                                return invalid(format!("{:?} has no option {v:?} (options: {})", f.name, opts.join(", ")));
                            }
                        }
                    }
                }
            }
            // /I lists option indices in ascending order, each once; /V keeps the caller's order.
            picked.sort_unstable();
            picked.dedup();
            let indices: Vec<Object> = picked.iter().map(|&i| Object::Int(i64::try_from(i).unwrap_or(i64::MAX))).collect();
            // A list box scrolls (`/TI`) so the picked options are among the rows it draws, as
            // in other viewers: a pick below the last visible row would otherwise not show (#1524).
            let top = list_scroll(f, &picked, f.options.len());
            // An empty value of its own keeps a parent field's /V from showing through.
            let shadow = exports.is_empty() && crate::author::ancestor_sets(doc, f.obj, b"V");
            doc.update_dict(f.obj, |d| {
                match exports.as_slice() {
                    [] if shadow => d.set(b"V".to_vec(), PdfString::text("")),
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
                if let Some((old, new)) = top
                    && old != new
                {
                    d.set(b"TI".to_vec(), Object::Int(i64::try_from(new).unwrap_or(0)));
                }
            })?;
            scroll_widgets(doc, f, &picked, f.options.len())?;
            let mut shown = f.clone();
            shown.value = exports.clone();
            shown.selected = picked;
            redraw(doc, &shown, &exports, scripts)
        }
        (FieldKind::PushButton, _) => invalid(format!("{:?} is a button; it has no value", f.name)),
        (FieldKind::Signature, _) => invalid(format!("{:?} is a signature field; sign it with Fill & Sign or a digital ID", f.name)),
        (kind, v) => invalid(format!("{:?} is a {kind:?} field and can't take {v:?}", f.name)),
    }
}

/// A list box's first visible row (`/TI`) before and after scrolling so the `picked` positions of
/// its `count` options are among the rows it draws, as in other viewers: a pick below the last
/// visible row would otherwise not show (#1524). `None` for any other field.
pub(crate) fn list_scroll(f: &Field, picked: &[usize], count: usize) -> Option<(usize, usize)> {
    f.widgets.first().filter(|_| f.kind == FieldKind::List).and_then(|w| widget_scroll(w, picked, count))
}

/// A list box widget's first row before and after scrolling so `picked` shows.
fn widget_scroll(w: &Widget, picked: &[usize], count: usize) -> Option<(usize, usize)> {
    w.list.map(|rows| {
        let height = if matches!(w.rotation, 90 | 270) { w.rect[2] - w.rect[0] } else { w.rect[3] - w.rect[1] };
        (rows.top, appearance::ListRows::top_showing(rows.top, picked, rows.visible(height), count))
    })
}

/// Scroll every widget of `f` with its own `/TI` (read before the field's) by its own rows and
/// size, so `picked` shows in each.
pub(crate) fn scroll_widgets(doc: &mut Document, f: &Field, picked: &[usize], count: usize) -> Result<(), FormError> {
    for w in f.widgets.iter().filter(|w| w.obj != f.obj) {
        if doc.get(w.obj).as_dict().is_some_and(|d| d.contains(b"TI"))
            && let Some((old, new)) = widget_scroll(w, picked, count)
            && old != new
        {
            doc.update_dict(w.obj, |d| d.set(b"TI".to_vec(), Object::Int(i64::try_from(new).unwrap_or(0))))?;
        }
    }
    Ok(())
}

/// Check boxes and radio buttons: `/V` on the field, `/AS` on each widget. The widgets whose on
/// state is `on` turn on together in a check box (ISO 32000-2 §12.7.5.2.3, EXAMPLE 2) and in a
/// radio group with RadiosInUnison. Without it, at most one radio button is on (Table 229,
/// §12.7.5.2.3): widget `pick` when it has that state, else the one already on, else the first.
fn set_states(doc: &mut Document, f: &Field, on: Option<&str>, pick: Option<usize>) -> Result<(), FormError> {
    let exclusive = f.kind == FieldKind::Radio && !f.has(flags::RADIOS_IN_UNISON);
    let chosen = on.filter(|_| exclusive).and_then(|c| {
        let has = |i: &usize| f.widgets.get(*i).is_some_and(|w| w.on_state.as_deref() == Some(c));
        let n = f.widgets.len();
        pick.filter(has).or_else(|| (0..n).filter(has).find(|&i| f.widget_on(i))).or_else(|| (0..n).find(has))
    });
    // `/V` names the on state, the same for every widget that has it: which of them is on is
    // their `/AS` alone (§12.7.5.2.4).
    let v = on.unwrap_or("Off");
    doc.update_dict(f.obj, |d| d.set(b"V".to_vec(), name_obj(v)))?;
    for (i, w) in f.widgets.iter().enumerate() {
        let state = match (on, w.on_state.as_deref()) {
            (Some(c), Some(s)) if c == s && chosen.is_none_or(|k| k == i) => s,
            _ => "Off",
        };
        // A widget without appearances for its states gets PdfCraft's own.
        let has_ap = doc.get(w.obj).as_dict().and_then(|d| d.get(b"AP").cloned()).is_some();
        if !has_ap {
            let on_name = w.on_state.clone().unwrap_or_else(|| on.unwrap_or("Yes").to_string());
            let ap = appearance::check_box_states(doc, w, f.kind, &on_name, &f.da);
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
    let in_place = f.kind == FieldKind::Text && f.has(flags::PASSWORD);
    let mut undrawn = false;
    // The Format script again with English month and weekday names, once, for widgets whose
    // fonts can't draw the date language's (#5303).
    let mut english: Option<Option<String>> = None;
    for w in &f.widgets {
        let (mut stream, missing) = match &shown {
            Some(s) => {
                let drawn = appearance::field_appearance_drawn(doc, f, w, std::slice::from_ref(s), false);
                // Taken when the English run happened and draws whole, even where it is the
                // stored value ("18-Oct-26" printed again from itself).
                let again = match (&drawn.1, values) {
                    (Some(_), [one]) => english.get_or_insert_with(|| scripting::formatted_in_english(doc, f, one, scripts)).as_ref(),
                    _ => None,
                };
                match again.map(|e| appearance::field_appearance_drawn(doc, f, w, std::slice::from_ref(e), false)) {
                    Some(english) if english.1.is_none() => english,
                    _ => drawn,
                }
            }
            None => appearance::field_appearance_drawn(doc, f, w, values, true),
        };
        undrawn |= missing.is_some();
        // Thai glyphs are streams of their own.
        pdfcraft_fonts::shaped_text::store_streams(doc, &mut stream.dict);
        let existing_ap = in_place
            .then(|| {
                doc.get(w.obj)
                    .as_dict()
                    .and_then(|d| d.get(b"AP").map(|a| doc.resolve(a)))
                    .and_then(|a| a.as_dict().and_then(|a| a.get(b"N").cloned()))
                    .and_then(|n| n.as_ref())
                    .filter(|r| doc.is_edited(r.num) && matches!(&*doc.get(*r), Object::Stream(_)))
            })
            .flatten();
        let ap = match existing_ap {
            Some(r) => {
                doc.set(r, Object::Stream(stream));
                r
            }
            None => doc.add(Object::Stream(stream)),
        };
        let mut fresh = Dict::new();
        fresh.set(b"N".to_vec(), Object::Ref(ap));
        let apd = merged_appearance(doc, w.obj, &fresh);
        doc.update_dict(w.obj, |d| {
            d.set(b"AP".to_vec(), Object::Dict(apd));
            d.remove(b"AS");
            pdfcraft_fonts::set_undrawn_mark(d, missing);
        })?;
    }
    if undrawn {
        ask_viewers_to_redraw(doc)?;
    }
    Ok(())
}

/// A value its widget's font can't show is stored as typed and drawn with `?` (as Acrobat keeps
/// such a value): `/NeedAppearances` then asks other viewers to draw the fields from their values
/// with fonts that have those characters (the interactive form dictionary, §12.7.3). Not in a
/// document that declares PDF/A, which forbids it: there the widget's mark
/// (`pdfcraft_fonts::UNDRAWN_MARK`) is all, and the edit's report says the `?` shows everywhere.
pub(crate) fn ask_viewers_to_redraw(doc: &mut Document) -> Result<(), FormError> {
    // The form's own flag first: once it is set, the metadata needn't be decoded again for each
    // field drawn after.
    if need_appearances(doc) || declares_pdfa(doc) {
        return Ok(());
    }
    let Some(root) = doc.root() else { return Ok(()) };
    let entry = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").cloned());
    match entry {
        Some(Object::Ref(r)) => doc.update_dict(r, |d| d.set(b"NeedAppearances".to_vec(), Object::Bool(true)))?,
        Some(Object::Dict(mut af)) => {
            af.set(b"NeedAppearances".to_vec(), Object::Bool(true));
            doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(af)))?;
        }
        _ => {}
    }
    Ok(())
}

/// Text fields with the Password flag (inherited or their own) whose value (`/V`, inherited or
/// their own) is not empty and not their default value (`/DV`, which the form's author wrote and
/// a save restores), or one of whose separate widgets carries a `/V`.
fn password_fields(doc: &Document) -> Vec<Field> {
    if !has_password_flag(doc) {
        return Vec::new();
    }
    let on_widget = |f: &Field| f.widgets.iter().any(|w| w.obj != f.obj && doc.get(w.obj).as_dict().is_some_and(|d| d.get(b"V").is_some()));
    let typed = |f: &Field| f.value.iter().any(|v| !v.is_empty()) && f.value.first() != f.default.first();
    fields(doc).into_iter().filter(|f| f.kind == FieldKind::Text && f.has(flags::PASSWORD) && (typed(f) || on_widget(f))).collect()
}

/// Whether any field dictionary sets the Password flag (#3943 review). A field that inherits the
/// flag has an ancestor that sets it, so that ancestor is found instead; a kid clearing the flag
/// again only makes the answer cautious. It reads only `/Ff`, `/Kids` and `/Parent`, so a save
/// or recovery snapshot of a form without such a field doesn't build every [`Field`]. Like
/// [`fields`], it looks at the form's field tree and at the widgets on the pages, which may be
/// missing from `/Fields`. Each dictionary is visited once and at most 64 levels deep.
pub(crate) fn has_password_flag(doc: &Document) -> bool {
    let sets_it = |r: ObjRef| doc.get(r).as_dict().and_then(|d| field_flags(doc, d)).is_some_and(|ff| ff & flags::PASSWORD != 0);
    let top = acroform(doc).and_then(|af| af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned())).unwrap_or_default();
    let mut stack: Vec<(Object, usize)> = top.into_iter().map(|o| (o, 0)).collect();
    let mut seen = std::collections::HashSet::new();
    while let Some((o, depth)) = stack.pop() {
        let Object::Ref(r) = o else { continue };
        if depth > 64 || !seen.insert(r) {
            continue;
        }
        if sets_it(r) {
            return true;
        }
        let kids = doc.get(r).as_dict().and_then(|d| d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned())).unwrap_or_default();
        stack.extend(kids.into_iter().map(|k| (k, depth + 1)));
    }
    // Widgets on the pages, and the fields above them.
    page_refs(doc).into_iter().any(|p| {
        let annots =
            doc.get(p).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned())).unwrap_or_default();
        annots.iter().filter_map(Object::as_ref).filter(|r| !seen.contains(r)).any(|r| field_chain(doc, r).into_iter().any(sets_it))
    })
}

/// Whether any text field with the Password flag holds a value in `doc`.
pub fn has_password_values(doc: &Document) -> bool {
    !password_fields(doc).is_empty()
}

/// The typed values of the text fields with the Password flag, as (name, text) pairs, for
/// [`restore_password_values`] after the document was rebased on a file written without them.
pub fn password_values(doc: &Document) -> Vec<(String, String)> {
    password_fields(doc).into_iter().filter_map(|f| f.value.first().map(|v| (f.name.clone(), v.clone()))).collect()
}

/// Put values from [`password_values`] back into the open document, in memory only. A field
/// that is gone or read-only keeps what it has.
pub fn restore_password_values(doc: &mut Document, values: &[(String, String)]) -> Result<(), FormError> {
    for (name, v) in values {
        match set_value(doc, name, &FieldValue::Text(v.clone())) {
            Ok(()) | Err(FormError::NoSuchField(_) | FormError::ReadOnly(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `r` and its field ancestors (`/Parent`), nearest first, at most 64 deep and without repeats.
fn field_chain(doc: &Document, r: ObjRef) -> Vec<ObjRef> {
    let mut chain = vec![r];
    let mut cur = r;
    while chain.len() < 64 {
        let Some(Object::Ref(parent)) = doc.get(cur).as_dict().and_then(|d| d.get(b"Parent").cloned()) else { break };
        if chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        cur = parent;
    }
    chain
}

/// Remove the values of the text fields with the Password flag, for a copy of the document about
/// to be written to a file (#3926). ISO 32000-2 §12.7.5.3, Table 231, Password flag, note: "it is
/// imperative that PDF processors never store the value of the text field in the PDF file if
/// this flag is set". As Acrobat saves such a field (#3943 review, black-box test): a field with
/// a default value (`/DV`, which the form's author wrote) gets it back as its value, one without
/// is left empty, and its appearance is drawn again from that value, so the masking characters
/// no longer show how long the typed password was. `/DV` and the field's scripts stay.
///
/// A value can sit on the field, on one of its widgets, or on an ancestor it inherits from
/// (`/V` is inheritable). Each is removed; an ancestor's only when every field inheriting from it
/// is such a password field. Where an ancestor's value is shared with other fields and stays, the
/// password field gets an empty value of its own so it no longer inherits it. Returns how many
/// fields were cleared.
pub fn clear_password_values(doc: &mut Document) -> Result<usize, FormError> {
    let targets = password_fields(doc);
    if targets.is_empty() {
        return Ok(0);
    }
    let is_password = |f: &Field| f.kind == FieldKind::Text && f.has(flags::PASSWORD);
    // Each field's ancestors, found once: only values change below, never `/Parent` (#3943
    // review: walking them again for every ancestor of every target was quadratic).
    let chains: Vec<(bool, Vec<ObjRef>)> = fields(doc).iter().map(|g| (is_password(g), field_chain(doc, g.obj))).collect();
    for f in &targets {
        let chain = field_chain(doc, f.obj);
        for w in &f.widgets {
            if w.obj != f.obj {
                doc.update_dict(w.obj, |d| {
                    d.remove(b"V");
                })?;
            }
        }
        doc.update_dict(f.obj, |d| {
            d.remove(b"V");
        })?;
        let mut shared_value_kept = false;
        for a in chain.iter().skip(1) {
            if doc.get(*a).as_dict().is_none_or(|d| d.get(b"V").is_none()) {
                continue;
            }
            let exclusive = chains.iter().filter(|(_, chain)| chain.contains(a)).all(|(password, _)| *password);
            if exclusive {
                doc.update_dict(*a, |d| {
                    d.remove(b"V");
                })?;
            } else {
                shared_value_kept = true;
            }
        }
        let restored: Vec<String> = f.default.first().cloned().into_iter().collect();
        match restored.first() {
            Some(dv) => doc.update_dict(f.obj, |d| d.set(b"V".to_vec(), PdfString::text(dv)))?,
            None if shared_value_kept => doc.update_dict(f.obj, |d| d.set(b"V".to_vec(), PdfString::text("")))?,
            None => {}
        }
        redraw(doc, f, &restored, &mut NoScripts)?;
    }
    Ok(targets.len())
}

/// Whether `field`'s `key` (`/V` or `/DV`, its own or inherited) is a stream with data, which
/// [`fields`] reads as empty once its value budget is spent.
fn unread_stream(doc: &Document, field: ObjRef, key: &[u8]) -> bool {
    let mut node = Some(field);
    for _ in 0..64 {
        let Some(r) = node else { break };
        let obj = doc.get(r);
        let Some(d) = obj.as_dict() else { break };
        if let Some(value) = d.get(key) {
            return matches!(&*doc.resolve(value), Object::Stream(s) if !s.raw.is_empty());
        }
        node = d.reference(b"Parent");
    }
    false
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
        // A default the reading didn't decode (its budget spent) is unknown, not empty: clearing
        // the field to nothing would lose the value it should go back to. A value it didn't decode
        // isn't known to be the default, so it is reset.
        let unread = |key: &[u8], read: &[String]| read.is_empty() && unread_stream(doc, f.obj, key);
        if unread(b"DV", &f.default) || (f.value == f.default && !unread(b"V", &f.value)) {
            continue;
        }
        let v = match f.kind {
            FieldKind::Text => FieldValue::Text(f.default.first().cloned().unwrap_or_default()),
            FieldKind::CheckBox => f.check_box_value(f.default.first().map(String::as_str)),
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

/// Marks option `i` picked; false when it already was, or there is no such option.
fn first_pick(seen: &mut [bool], i: usize) -> bool {
    match seen.get_mut(i) {
        Some(taken) if !*taken => {
            *taken = true;
            true
        }
        _ => false,
    }
}

/// The twin's positions for the source's picked positions `picked`, as
/// [`crate::option_positions_in`] maps them, in ascending order, as `/I` lists them.
fn twin_positions(picked: &[usize], source: &[(String, String)], twin: &[(String, String)]) -> Option<Vec<usize>> {
    let mut out = crate::option_positions_in(picked, source, twin)?;
    out.sort_unstable();
    Some(out)
}
