use super::join::{check_join, join_radio_group};
use super::look::{BorderStyle, da_string, patch_da, patch_widget_look, redraw_widgets};
use super::tree::{ancestor_sets, attach, check_place, child_named, clash, detach, ensure_form, inherited, name_parts, own_inherited};
use super::{Dict, Document, Field, FieldKind, FieldProps, FormError, MIN_FIELD_SIDE, ObjRef, Object, PdfString, fields, flags, invalid};

/// Swap width and height, keeping the center. `None` when that is not a usable widget.
fn swapped_around_center(r: [f64; 4]) -> Option<[f64; 4]> {
    if !r.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (x0, y0, x1, y1) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (nw, nh) = (y1 - y0, x1 - x0);
    let out = [cx - nw / 2.0, cy - nh / 2.0, cx + nw / 2.0, cy + nh / 2.0];
    (out.iter().all(|v| v.is_finite()) && out[2] - out[0] >= MIN_FIELD_SIDE && out[3] - out[1] >= MIN_FIELD_SIDE).then_some(out)
}

/// Re-point a choice field's `/I`, positions in `/Opt`, at the new list `exports` after `/Opt` was
/// replaced (ISO 32000-2 §12.7.5.4): the positions of the selected `values` still offered. `kept`
/// are the old `/I` positions carried into the new list, which tell apart options sharing an
/// export value; a selected value none of them covers takes its first position, by `set_value`'s
/// rule. Returns them.
fn repoint_selection(d: &mut Dict, values: &[String], exports: &[String], kept: &[usize]) -> Vec<usize> {
    let mut indices: Vec<usize> = kept.iter().copied().filter(|&i| exports.get(i).is_some_and(|e| values.contains(e))).collect();
    for v in values {
        if !indices.iter().any(|&i| exports.get(i) == Some(v)) {
            indices.extend(exports.iter().position(|e| e == v));
        }
    }
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() {
        d.remove(b"I");
    } else {
        d.set(b"I".to_vec(), Object::Array(indices.iter().map(|&i| Object::Int(i64::try_from(i).unwrap_or(i64::MAX))).collect()));
    }
    indices
}

/// The positions in `/Opt` that a choice field's `/I` holds, those below `count`.
fn selected_indices(doc: &Document, field: ObjRef, count: usize) -> Vec<usize> {
    let Some(i) = doc.get(field).as_dict().and_then(|d| d.get(b"I").cloned()) else {
        return Vec::new();
    };
    let positions = doc.resolve(&i);
    positions.as_array().map(|a| a.iter().filter_map(|o| usize::try_from(o.as_int()?).ok()).filter(|&n| n < count).collect()).unwrap_or_default()
}

/// The items (export value, display text) "Sort items" orders: the new `props.options`, or else
/// the field's own, whose selection, `/I` included, moves with them.
fn items_to_sort(doc: &Document, f: &Field, props: &FieldProps) -> (Vec<(String, String)>, Vec<usize>) {
    match &props.options {
        Some(o) => (o.iter().map(|x| (x.clone(), x.clone())).collect(), Vec::new()),
        None => (f.options.clone(), selected_indices(doc, f.obj, f.options.len())),
    }
}

/// Sort a choice field's items (export value, display text) by display text, ignoring case, and
/// write them as `/Opt`; export values stay with their items. Returns the export values in their
/// new order and the `old` positions carried through the sort.
fn write_sorted_opt(d: &mut Dict, items: &[(String, String)], old: &[usize]) -> (Vec<String>, Vec<usize>) {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| items.get(i).map(|(_, display)| display.to_lowercase()));
    let sorted: Vec<&(String, String)> = order.iter().filter_map(|&i| items.get(i)).collect();
    let opt = sorted
        .iter()
        .map(|(e, disp)| {
            if e == disp {
                Object::String(PdfString::text(disp))
            } else {
                Object::Array(vec![Object::String(PdfString::text(e)), Object::String(PdfString::text(disp))])
            }
        })
        .collect();
    d.set(b"Opt".to_vec(), Object::Array(opt));
    let carried = old.iter().filter_map(|o| order.iter().position(|i| i == o)).collect();
    (sorted.into_iter().map(|(e, _)| e.clone()).collect(), carried)
}

/// Build /Opt before modifying the document, preserving appearance states and selection.
fn button_export_options(f: &Field, index: usize, value: &str) -> Result<Vec<Object>, FormError> {
    if !matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) {
        return invalid("only check boxes and radio buttons have an export value");
    }
    if value.is_empty() || value == "Off" || value.len() > 4096 {
        return invalid("export value must be non-empty, at most 4096 bytes, and not Off");
    }
    let widget = f.widgets.get(index).ok_or_else(|| FormError::Invalid("button widget index is out of range".into()))?;
    let state = widget.on_state.as_deref().ok_or_else(|| FormError::Invalid("button has no on appearance state".into()))?;
    if !f.options.is_empty() && f.options.len() != f.widgets.len() {
        return invalid("button export options do not match its widgets");
    }
    let previous = f.export_of(index);
    let mut out = Vec::with_capacity(f.widgets.len());
    for (i, w) in f.widgets.iter().enumerate() {
        let export = f.export_of(i).ok_or_else(|| FormError::Invalid("button has no on appearance state".into()))?;
        let same_state = w.on_state.as_deref() == Some(state);
        if !same_state && (export == value || w.on_state.as_deref() == Some(value)) {
            return invalid("export value conflicts with another button choice");
        }
        if same_state && Some(export) != previous {
            return invalid("widgets with the same appearance state have conflicting export values");
        }
        out.push(Object::String(PdfString::text(if same_state { value } else { export })));
    }
    Ok(out)
}

/// Change a field's properties (General and Options tabs) and redraw it.
pub fn set_props(doc: &mut Document, name: &str, props: &FieldProps) -> Result<String, FormError> {
    if props.font_size.is_some_and(|size| !size.is_finite() || size < 0.0) {
        return invalid("font size must be a finite, non-negative number");
    }
    if let Some(patch) = &props.appearance {
        if props.look.is_some() {
            return invalid("give an appearance patch or a complete look, not both");
        }
        patch.validate()?;
    }
    // Refused here, before anything is written, rather than stored as a different formula.
    if let Some(crate::af::Calculate::Notation(n)) = &props.calculate
        && !crate::af::notation_storable(n)
    {
        return invalid(crate::af::NOTATION_NOT_STORABLE);
    }
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?.clone();
    let export_options = props.export_value.as_ref().map(|(index, value)| button_export_options(&f, *index, value)).transpose()?;
    if props.export_value.is_some() && props.options.is_some() {
        return invalid("give export_value or options, not both");
    }
    // Refuse a wrong type/index before any other requested property is written.
    if let Some((wi, _)) = &props.caption {
        if f.kind != FieldKind::PushButton {
            return invalid("only push buttons have a caption");
        }
        if f.widgets.get(*wi).is_none() {
            return invalid(format!("{name} has no widget {}", wi.saturating_add(1)));
        }
    }
    // Validated before anything is written, so a refused rotation leaves the document unchanged.
    // `Some(Some(rect))`: the widget's axis changes and it gets the swapped rectangle.
    let mut rotated_rect: Option<Option<[f64; 4]>> = None;
    if let Some((wi, rot)) = props.rotation {
        if !matches!(rot, 0 | 90 | 180 | 270) {
            return invalid("rotation must be 0, 90, 180 or 270 degrees");
        }
        let Some(w) = f.widgets.get(wi) else {
            return invalid(format!("{name} has no widget {}", wi.saturating_add(1)));
        };
        let axis_change = (w.rotation % 180 == 0) != (rot % 180 == 0);
        rotated_rect = Some(None);
        if axis_change && props.rect.is_none() {
            let Some(rect) = swapped_around_center(w.rect) else { return invalid("the field is too small") };
            rotated_rect = Some(Some(rect));
        }
    }
    // A locked field only takes unlocking (Acrobat greys its properties out).
    if f.locked() && props.locked != Some(false) && *props != (FieldProps { locked: props.locked, ..FieldProps::default() }) {
        return invalid(format!("{name} is locked: unlock it first"));
    }
    if let Some(options) = export_options.as_ref()
        && *props == (FieldProps { export_value: props.export_value.clone(), ..FieldProps::default() })
    {
        doc.update_dict(f.obj, |d| d.set(b"Opt".to_vec(), Object::Array(options.clone())))?;
        return Ok(name.to_string());
    }
    if let Some(lock) = props.locked {
        for w in &f.widgets {
            let flags = doc.get(w.obj).as_dict().and_then(|d| d.get(b"F").and_then(|v| doc.resolve(v).as_int())).unwrap_or(0);
            let v = if lock { flags | 128 } else { flags & !128 };
            doc.update_dict(w.obj, |d| d.set(b"F".to_vec(), Object::Int(v)))?;
        }
        if *props == (FieldProps { locked: props.locked, ..FieldProps::default() }) {
            return Ok(name.to_string());
        }
    }
    if let Some(acts) = &props.actions {
        crate::set_field_actions(doc, name, acts)?;
    }
    // A radio button renamed to another radio group's name joins that group (#4949). The other
    // properties are written first, to this field, and its buttons then move.
    let (new_name, join) = rename_field(doc, &all, &f, name, props)?;
    // The new default appearance when the size or the look changes.
    let size = props.font_size.unwrap_or_else(|| crate::appearance::parse_da(&f.da).size);
    let new_da = match &props.look {
        Some(l) => da_string(l.font, size, l.text),
        None => patch_da(&f.da, props.font_size, &props.appearance.unwrap_or_default()),
    };
    if props.look.is_some() || props.appearance.is_some_and(|p| p.font.is_some()) {
        ensure_form(doc)?;
    }
    let mut ff = f.flags;
    let mut set_flag = |flag: u32, on: Option<bool>| {
        if let Some(on) = on {
            if on {
                ff |= flag;
            } else {
                ff &= !flag;
            }
        }
    };
    set_flag(flags::READ_ONLY, props.read_only);
    set_flag(flags::REQUIRED, props.required);
    if f.kind == FieldKind::Text {
        set_flag(flags::MULTILINE, props.multiline);
    }
    for (flag, on) in &props.flags {
        set_flag(*flag, Some(*on));
    }
    // A comb spreads a fixed number of characters across the field: it needs a limit and
    // excludes multi-line, password, file selection and scrolling (ISO 32000-2 Table 229).
    if f.kind == FieldKind::Text && ff & flags::COMB != 0 {
        let limit = match props.max_len {
            Some(m) => m,
            None => f.max_len,
        };
        if limit.is_none_or(|m| m == 0) {
            return invalid("a comb of characters needs a character limit");
        }
        ff &= !(flags::MULTILINE | flags::PASSWORD | flags::FILE_SELECT);
    }
    if let Some(q) = props.quadding
        && !(0..=2).contains(&q)
    {
        return invalid("alignment is 0 (left), 1 (centre) or 2 (right)");
    }
    if props.options.as_ref().is_some_and(Vec::is_empty) && matches!(f.kind, FieldKind::Combo | FieldKind::List) {
        return invalid("a list needs at least one option");
    }
    if props.check_style.is_some() && !matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) {
        return invalid("only check boxes and radio buttons have a check style");
    }
    let mut list_picked = None;
    let (sort_items, old_selection) = items_to_sort(doc, &f, props);
    let parent_default = ancestor_sets(doc, f.obj, b"DV");
    doc.update_dict(f.obj, |d| {
        d.set(b"Ff".to_vec(), Object::Int(ff as i64));
        if let Some(t) = &props.tooltip {
            if t.is_empty() {
                d.remove(b"TU");
            } else {
                d.set(b"TU".to_vec(), PdfString::text(t));
            }
        }
        if let Some(m) = props.max_len {
            match m {
                Some(m) => d.set(b"MaxLen".to_vec(), Object::Int(m as i64)),
                None => {
                    d.remove(b"MaxLen");
                }
            }
        }
        // The export values in `/Opt` order once it is rewritten, for re-pointing `/I`.
        let mut new_exports: Option<Vec<String>> = None;
        let mut kept = Vec::new();
        if let Some(options) = &export_options {
            d.set(b"Opt".to_vec(), Object::Array(options.clone()));
        }
        if let Some(opts) = &props.options {
            d.set(b"Opt".to_vec(), Object::Array(opts.iter().map(|o| Object::String(PdfString::text(o))).collect()));
            new_exports = Some(opts.clone());
        }
        // "Sort items": the list itself is kept in order (export values stay with their items).
        if ff & flags::SORT != 0 && matches!(f.kind, FieldKind::Combo | FieldKind::List) {
            let (exports, carried) = write_sorted_opt(d, &sort_items, &old_selection);
            new_exports = Some(exports);
            kept = carried;
        }
        if let Some(exports) = &new_exports
            && matches!(f.kind, FieldKind::Combo | FieldKind::List)
        {
            list_picked = Some((repoint_selection(d, &f.value, exports, &kept), exports.len()));
        }
        if let Some(q) = props.quadding {
            d.set(b"Q".to_vec(), Object::Int(q));
        }
        match &props.default_value {
            // Buttons name their default state; text and choices hold a string.
            Some(Some(v)) if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) => d.set(b"DV".to_vec(), Object::Name(crate::name_bytes(v))),
            Some(Some(v)) => d.set(b"DV".to_vec(), PdfString::text(v)),
            // `/DV` is inheritable: with an ancestor's in place, removing the field's own would
            // bring that one back, so the field keeps an empty default of its own instead.
            Some(None) if parent_default && matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) => {
                d.set(b"DV".to_vec(), Object::Name(b"Off".to_vec()))
            }
            Some(None) if parent_default => d.set(b"DV".to_vec(), PdfString::text("")),
            Some(None) => {
                d.remove(b"DV");
            }
            None => {}
        }
        if props.font_size.is_some() || props.look.is_some() || props.appearance.is_some_and(|p| p.font.is_some() || p.text.is_some()) {
            d.set(b"DA".to_vec(), PdfString::literal(new_da.clone().into_bytes()));
        }
    })?;
    if props.appearance.is_some() || (props.font_size.is_some() && props.look.is_none()) {
        let patch = &props.appearance.unwrap_or_default();
        for w in &f.widgets {
            patch_widget_look(doc, w, patch)?;
            // A widget's own /DA overrides its field's /DA. Patch that override separately
            // rather than replacing its font or colour with the first widget's defaults.
            if w.obj != f.obj && (props.font_size.is_some() || patch.font.is_some() || patch.text.is_some()) {
                let da = doc
                    .get(w.obj)
                    .as_dict()
                    .and_then(|d| d.get(b"DA").cloned())
                    .map(|v| doc.resolve(&v))
                    .and_then(|v| v.as_string().map(|s| s.to_text()));
                if let Some(da) = da {
                    let da = patch_da(&da, props.font_size, patch);
                    doc.update_dict(w.obj, |d| d.set(b"DA".to_vec(), PdfString::literal(da.into_bytes())))?;
                }
            }
        }
    }
    apply_widget_style_props(doc, &f, name, props)?;
    if props.format.is_some() || props.validate.is_some() || props.calculate.is_some() {
        if !matches!(f.kind, FieldKind::Text | FieldKind::Combo) {
            return invalid("only text fields and drop-down lists have format, validate and calculate scripts");
        }
        let mut aa = doc
            .get(f.obj)
            .as_dict()
            .and_then(|d| d.get(b"AA").cloned())
            .map(|a| doc.resolve(&a))
            .and_then(|a| a.as_dict().cloned())
            .unwrap_or_default();
        let action = |js: &str| -> Object {
            let mut a = Dict::new();
            a.set(b"S".to_vec(), Object::name("JavaScript"));
            a.set(b"JS".to_vec(), PdfString::literal(js.as_bytes().to_vec()));
            Object::Dict(a)
        };
        if let Some(fm) = &props.format {
            aa.remove(b"F");
            aa.remove(b"K");
            if let Some((fjs, kjs)) = crate::af::format_js(fm) {
                if !fjs.is_empty() {
                    aa.set(b"F".to_vec(), action(&fjs));
                }
                aa.set(b"K".to_vec(), action(&kjs));
            }
        }
        if let Some(v) = &props.validate {
            aa.remove(b"V");
            if let Some(js) = crate::af::validate_js(v) {
                aa.set(b"V".to_vec(), action(&js));
            }
        }
        if let Some(c) = &props.calculate {
            aa.remove(b"C");
            if let Some(js) = crate::af::calculate_js(c) {
                aa.set(b"C".to_vec(), action(&js));
            }
            crate::calc_order::keep_listed(doc, f.obj, *c != crate::af::Calculate::None)?;
        }
        doc.update_dict(f.obj, |d| {
            if aa.is_empty() {
                d.remove(b"AA");
            } else {
                d.set(b"AA".to_vec(), Object::Dict(aa));
            }
        })?;
    }
    let only = apply_late_widget_props(doc, &f, props, &new_da, rotated_rect)?;
    scroll_list(doc, f.obj, list_picked)?;
    redraw_or_join(doc, &f, name, &new_name, join.as_ref(), only)?;
    if props.calculate.is_some() {
        crate::recalculate(doc)?;
    }
    Ok(new_name)
}

/// Apply a new name from `props`: rename the field, move it under the parents a full name
/// names, or, for a radio button named after another radio group, pick that group to join.
/// Returns the field's new full name and the group it joins.
fn rename_field(doc: &mut Document, all: &[Field], f: &Field, name: &str, props: &FieldProps) -> Result<(String, Option<Field>), FormError> {
    let mut new_name = name.to_string();
    let mut join = None;
    if let Some(n) = props.name.as_deref().map(str::trim).filter(|n| *n != f.name) {
        // A full name under the same parents only renames the field's own part (the editor
        // shows nested fields by their full name): it isn't moved, so it keeps its place and
        // goes on inheriting from its parents.
        let n = match (n.rsplit_once('.'), f.name.rsplit_once('.')) {
            (Some((parents, own)), Some((current, _))) if parents == current => own,
            _ => n,
        };
        let full = match f.name.rsplit_once('.') {
            Some((parents, _)) if !n.contains('.') => format!("{parents}.{n}"),
            _ => n.to_string(),
        };
        let group = all.iter().find(|x| x.obj != f.obj && x.kind == FieldKind::Radio && x.name == full);
        if let Some(group) = group.filter(|_| f.kind == FieldKind::Radio) {
            check_join(doc, f, group)?;
            join = Some(group.clone());
            new_name = full;
        } else if n.contains('.') {
            // A full name: the field moves under the parent fields it names (#2696).
            let parts = name_parts(n)?;
            if let Some(why) = all.iter().filter(|x| x.obj != f.obj).find_map(|x| clash(n, &x.name)) {
                return invalid(why);
            }
            let af = ensure_form(doc)?;
            // Checked before anything changes, so a refused move leaves the field as it was.
            let add = inherited(doc, f.obj);
            check_place(doc, af, f.obj, &parts, &add)?;
            // From here a failure (a write that `check_place` couldn't foresee) would leave the
            // field half moved in `doc`. The engine runs every edit on a copy of the document and
            // keeps it only when the edit succeeds (`Session::apply`), so the open document never
            // sees that.
            own_inherited(doc, f.obj, &add)?;
            detach(doc, f.obj)?;
            attach(doc, af, f.obj, &parts)?;
            new_name = n.to_string();
        } else {
            if n.is_empty() {
                return invalid("field names can't be empty");
            }
            // Rename the terminal part (the field's own /T). The parents' part stays, so the name
            // that must be free is the full one: `MASTER.a` renamed to `b` takes `MASTER.b` (ISO
            // 32000-2 §12.7.4.2). The field itself is no clash when it keeps its own name.
            let prefix = f.name.rsplit_once('.').map(|(p, _)| format!("{p}.")).unwrap_or_default();
            new_name = format!("{prefix}{n}");
            if let Some(why) = all.iter().filter(|x| x.obj != f.obj).find_map(|x| clash(&new_name, &x.name)) {
                return invalid(why);
            }
            // A node of that name beside it that isn't a terminal field, such as a parent
            // field left empty by a move.
            let af = ensure_form(doc)?;
            let parent = doc.get(f.obj).as_dict().and_then(|d| d.reference(b"Parent"));
            if child_named(doc, af, parent, n).is_some_and(|r| r != f.obj) {
                return invalid(format!("a field named {new_name:?} already exists"));
            }
            doc.update_dict(f.obj, |d| d.set(b"T".to_vec(), PdfString::text(n)))?;
        }
        // Calculations read the field by its new name, unless another field keeps the old one.
        if all.iter().filter(|x| x.name == f.name).count() == 1 {
            rename_in_calculations(doc, all, &f.name, &new_name)?;
        }
    }
    Ok((new_name, join))
}

fn apply_widget_style_props(doc: &mut Document, f: &Field, name: &str, props: &FieldProps) -> Result<(), FormError> {
    if let Some(l) = &props.look {
        let arr = |c: [f64; 3]| Object::Array(c.iter().map(|v| Object::Real(v.clamp(0.0, 1.0))).collect());
        for w in &f.widgets {
            // Resolved first: an indirect /MK keeps its mark, rotation and other entries.
            let mut mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m).as_dict().cloned())).flatten().unwrap_or_default();
            doc.update_dict(w.obj, |d| {
                match l.border {
                    Some(c) => mk.set(b"BC".to_vec(), arr(c)),
                    None => {
                        mk.remove(b"BC");
                    }
                }
                match l.fill {
                    Some(c) => mk.set(b"BG".to_vec(), arr(c)),
                    None => {
                        mk.remove(b"BG");
                    }
                }
                d.set(b"MK".to_vec(), Object::Dict(mk));
                let mut bs = Dict::new();
                bs.set(b"W".to_vec(), Object::Real(l.width.clamp(0.0, 12.0)));
                bs.set(b"S".to_vec(), Object::name(l.style.code()));
                if l.style == BorderStyle::Dashed {
                    bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(3)]));
                }
                d.set(b"BS".to_vec(), Object::Dict(bs));
            })?;
        }
    }
    if let Some(style) = props.check_style {
        for w in &f.widgets {
            // Resolved first: an indirect /MK keeps its colours.
            let mut mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m).as_dict().cloned())).flatten().unwrap_or_default();
            mk.set(b"CA".to_vec(), PdfString::text(style.code()));
            doc.update_dict(w.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
        }
    }
    if let Some((wi, r)) = props.rect {
        let w = f.widgets.get(wi).ok_or_else(|| FormError::Invalid(format!("{name} has no widget {}", wi + 1)))?;
        let r = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
        if !r.iter().all(|v| v.is_finite()) || r[2] - r[0] < MIN_FIELD_SIDE || r[3] - r[1] < MIN_FIELD_SIDE {
            return invalid("the field is too small");
        }
        doc.update_dict(w.obj, |d| d.set(b"Rect".to_vec(), Object::Array(r.iter().map(|v| Object::Real(*v)).collect())))?;
    }
    Ok(())
}

/// Redraw the edited widgets. A radio button joining another group (`join`) is redrawn under its
/// old name `name` first; then its buttons move, and only they are redrawn as `new_name`, so the
/// group's own buttons keep their appearances.
fn redraw_or_join(doc: &mut Document, f: &Field, name: &str, new_name: &str, join: Option<&Field>, only: Option<usize>) -> Result<(), FormError> {
    if let Some(group) = join {
        redraw_widgets(doc, name, only)?;
        join_radio_group(doc, f, group)?;
        let moved: Vec<usize> = fields(doc)
            .into_iter()
            .find(|g| g.name == new_name)
            .map(|g| g.widgets.iter().enumerate().filter(|(_, w)| f.widgets.iter().any(|m| m.obj == w.obj)).map(|(i, _)| i).collect())
            .unwrap_or_default();
        for index in moved {
            redraw_widgets(doc, new_name, Some(index))?;
        }
        Ok(())
    } else {
        redraw_widgets(doc, new_name, only)
    }
}

/// A list box scrolls, as for a new value, so its first row is still an option and the
/// selection is in view. Its rows are measured with the edited font size and rectangle.
fn scroll_list(doc: &mut Document, field: ObjRef, list_picked: Option<(Vec<usize>, usize)>) -> Result<(), FormError> {
    if let Some((picked, count)) = list_picked
        && let Some(g) = fields(doc).into_iter().find(|g| g.obj == field)
    {
        if let Some((old, new)) = crate::values::list_scroll(&g, &picked, count)
            && old != new
        {
            doc.update_dict(field, |d| d.set(b"TI".to_vec(), Object::Int(i64::try_from(new).unwrap_or(0))))?;
        }
        crate::values::scroll_widgets(doc, &g, &picked, count)?;
    }
    Ok(())
}

fn apply_late_widget_props(
    doc: &mut Document,
    f: &Field,
    props: &FieldProps,
    new_da: &str,
    rotated_rect: Option<Option<[f64; 4]>>,
) -> Result<Option<usize>, FormError> {
    // A complete look replaces widget /DA overrides. Partial changes were patched above
    // against each override, including size-only changes that keep its font and colour.
    if props.look.is_some() {
        for w in &f.widgets {
            if w.obj != f.obj {
                doc.update_dict(w.obj, |d| {
                    if d.contains(b"DA") {
                        d.set(b"DA".to_vec(), PdfString::literal(new_da.to_owned().into_bytes()));
                    }
                })?;
            }
        }
    }
    if let (Some((wi, rot)), Some(swapped)) = (props.rotation, rotated_rect)
        && let Some(w) = f.widgets.get(wi)
    {
        if let Some(rect) = swapped {
            doc.update_dict(w.obj, |d| d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect())))?;
        }
        let mut mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m).as_dict().cloned())).flatten().unwrap_or_default();
        if rot == 0 {
            mk.remove(b"R");
        } else {
            mk.set(b"R".to_vec(), Object::Int(rot));
        }
        doc.update_dict(w.obj, |d| {
            if mk.is_empty() {
                d.remove(b"MK");
            } else {
                d.set(b"MK".to_vec(), Object::Dict(mk));
            }
        })?;
    }
    if let Some((wi, caption)) = &props.caption
        && let Some(w) = f.widgets.get(*wi)
    {
        // Copy an indirect/shared MK before patching it; colours, icons, rotation and
        // unknown entries belong to this widget, and other owners keep their old label.
        let mut mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m).as_dict().cloned())).flatten().unwrap_or_default();
        mk.set(b"CA".to_vec(), PdfString::text(caption));
        doc.update_dict(w.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
    }
    // A label-only edit must not replace another widget's custom appearances.
    let only = props.caption.as_ref().filter(|_| *props == (FieldProps { caption: props.caption.clone(), ..Default::default() })).map(|(wi, _)| *wi);
    Ok(only)
}

/// Name field `old` `new` in each field's Calculate script that reads it
/// ([`crate::af::rename_in_calculation`]).
fn rename_in_calculations(doc: &mut Document, all: &[Field], old: &str, new: &str) -> Result<(), FormError> {
    let aa_of =
        |doc: &Document, r: ObjRef| doc.get(r).as_dict().and_then(|d| d.get(b"AA").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned());
    for f in all {
        // The script is the field's, or its only widget's when the field has none, as
        // `actions_of` reads it.
        let only_widget = || match f.widgets.as_slice() {
            [w] => aa_of(doc, w.obj).map(|aa| (w.obj, aa)),
            _ => None,
        };
        let Some((owner, mut aa)) = aa_of(doc, f.obj).map(|aa| (f.obj, aa)).or_else(only_widget) else { continue };
        let Some(mut action) = aa.get(b"C").and_then(|a| doc.resolve(a).as_dict().cloned()) else { continue };
        let js = match action.get(b"JS").map(|js| doc.resolve(js)).as_deref() {
            Some(Object::String(s)) => s.to_text(),
            // A stream that isn't UTF-8 reads with replacement characters, which must not be
            // written back in place of what it holds.
            Some(Object::Stream(s)) => match s.decoded().ok().and_then(|b| String::from_utf8(b).ok()) {
                Some(js) => js,
                None => continue,
            },
            _ => continue,
        };
        let Some(js) = crate::af::rename_in_calculation(&js, old, new) else { continue };
        action.set(b"JS".to_vec(), PdfString::text(&js));
        aa.set(b"C".to_vec(), Object::Dict(action));
        doc.update_dict(owner, |d| d.set(b"AA".to_vec(), Object::Dict(aa)))?;
    }
    Ok(())
}
