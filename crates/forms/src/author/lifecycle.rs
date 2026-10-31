use super::look::redraw_field;
use super::tree::{add_to_page, attach, clash, detach, ensure_form, free_name, name_parts, top_names};
use super::{
    Dict, Document, Field, FieldKind, FormError, MIN_FIELD_SIDE, NewField, ObjRef, Object, PdfString, Widget, appearance, fields, flags, invalid,
    page_refs, rgb,
};

fn widget_dict(page: ObjRef, rect: [f64; 4]) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Widget"));
    d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"P".to_vec(), Object::Ref(page));
    d.set(b"F".to_vec(), Object::Int(4));
    let mut mk = Dict::new();
    mk.set(b"BC".to_vec(), rgb([0.55, 0.55, 0.55]));
    mk.set(b"BG".to_vec(), rgb([1.0, 1.0, 1.0]));
    d.set(b"MK".to_vec(), Object::Dict(mk));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Int(1));
    bs.set(b"S".to_vec(), Object::name("S"));
    d.set(b"BS".to_vec(), Object::Dict(bs));
    d
}

/// Add a field on `page` (0-based) in `rect` (user space). Returns its full name.
pub fn add_field(doc: &mut Document, page: usize, rect: [f64; 4], kind: &NewField, name: Option<&str>) -> Result<String, FormError> {
    let pages = page_refs(doc);
    let page_ref = *pages.get(page).ok_or_else(|| FormError::Invalid(format!("page {} does not exist", page + 1)))?;
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    if !rect.iter().all(|v| v.is_finite()) || rect[2] - rect[0] < MIN_FIELD_SIDE || rect[3] - rect[1] < MIN_FIELD_SIDE {
        return invalid("the field is too small");
    }
    let existing = fields(doc);
    // A radio button names its group: a new group takes that name.
    let name = match (name, kind) {
        (None, NewField::Radio { group: Some(g), .. }) => Some(g.as_str()),
        _ => name,
    };
    let name = match name.map(str::trim) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => free_name(&existing, &top_names(doc), kind.base_name()),
    };
    let parts = name_parts(&name)?;
    let radio_group = match kind {
        NewField::Radio { group, .. } => existing.iter().find(|f| f.kind == FieldKind::Radio && Some(&f.name) == group.as_ref()).cloned(),
        _ => None,
    };
    if radio_group.is_none()
        && let Some(why) = existing.iter().find_map(|f| clash(&name, &f.name))
    {
        return invalid(why);
    }
    let af = ensure_form(doc)?;
    let mut w = widget_dict(page_ref, rect);
    let field_name = match kind {
        NewField::Radio { group: Some(g), .. } if radio_group.is_some() => g.clone(),
        _ => name.clone(),
    };
    match kind {
        NewField::Text { multiline } => {
            w.set(b"FT".to_vec(), Object::name("Tx"));
            // Auto size (0): a fixed 12 pt is clipped in a box shorter than that (#1888).
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
            if *multiline {
                w.set(b"Ff".to_vec(), Object::Int(flags::MULTILINE as i64));
            }
        }
        NewField::Date => {
            w.set(b"FT".to_vec(), Object::name("Tx"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
            // Acrobat's format and keystroke actions for mm/dd/yyyy (run by its JavaScript engine).
            let js = |s: &str| {
                let mut a = Dict::new();
                a.set(b"S".to_vec(), Object::name("JavaScript"));
                a.set(b"JS".to_vec(), PdfString::literal(s.as_bytes().to_vec()));
                Object::Dict(a)
            };
            let mut aa = Dict::new();
            aa.set(b"F".to_vec(), js("AFDate_FormatEx(\"mm/dd/yyyy\");"));
            aa.set(b"K".to_vec(), js("AFDate_KeystrokeEx(\"mm/dd/yyyy\");"));
            w.set(b"AA".to_vec(), Object::Dict(aa));
        }
        NewField::CheckBox => {
            w.set(b"FT".to_vec(), Object::name("Btn"));
            w.set(b"V".to_vec(), Object::name("Off"));
            w.set(b"AS".to_vec(), Object::name("Off"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/ZaDb 0 Tf 0 g".to_vec()));
        }
        NewField::Radio { export, .. } => {
            w.set(b"AS".to_vec(), Object::name("Off"));
            if export.trim().is_empty() || export == "Off" {
                return invalid("a radio button needs an export value other than Off");
            }
        }
        NewField::Combo { options, editable } | NewField::List { options, multi: editable } => {
            w.set(b"FT".to_vec(), Object::name("Ch"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
            let combo = matches!(kind, NewField::Combo { .. });
            let mut ff = 0;
            if combo {
                ff |= flags::COMBO;
                if *editable {
                    ff |= flags::EDIT;
                }
            } else if *editable {
                ff |= flags::MULTI_SELECT;
            }
            w.set(b"Ff".to_vec(), Object::Int(ff as i64));
            w.set(b"Opt".to_vec(), Object::Array(options.iter().map(|o| Object::String(PdfString::text(o))).collect()));
        }
        NewField::Button { caption } => {
            w.set(b"FT".to_vec(), Object::name("Btn"));
            w.set(b"Ff".to_vec(), Object::Int(flags::PUSH_BUTTON as i64));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
            let mut mk = w.get(b"MK").and_then(|m| m.as_dict()).cloned().unwrap_or_default();
            mk.set(b"BG".to_vec(), rgb([0.86, 0.86, 0.86]));
            mk.set(b"CA".to_vec(), PdfString::text(caption));
            w.set(b"MK".to_vec(), Object::Dict(mk));
        }
        NewField::Image => {
            w.set(b"FT".to_vec(), Object::name("Btn"));
            w.set(b"Ff".to_vec(), Object::Int(flags::PUSH_BUTTON as i64));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
            let mut mk = w.get(b"MK").and_then(|m| m.as_dict()).cloned().unwrap_or_default();
            // Icon only, no border or fill: the picture is the field.
            mk.remove(b"BC");
            mk.remove(b"BG");
            mk.set(b"TP".to_vec(), Object::Int(1));
            w.set(b"MK".to_vec(), Object::Dict(mk));
            let mut a = Dict::new();
            a.set(b"S".to_vec(), Object::name("JavaScript"));
            a.set(b"JS".to_vec(), PdfString::literal(b"event.target.buttonImportIcon();".to_vec()));
            w.set(b"A".to_vec(), Object::Dict(a));
        }
        NewField::Signature => {
            w.set(b"FT".to_vec(), Object::name("Sig"));
        }
    }
    let widget = doc.add(Object::Dict(w));
    match kind {
        NewField::Radio { export, .. } => {
            let group = match &radio_group {
                Some(g) => g.obj,
                None => {
                    let mut g = Dict::new();
                    g.set(b"FT".to_vec(), Object::name("Btn"));
                    g.set(b"Ff".to_vec(), Object::Int((flags::RADIO | flags::NO_TOGGLE_TO_OFF) as i64));
                    g.set(b"V".to_vec(), Object::name("Off"));
                    g.set(b"DA".to_vec(), PdfString::literal(b"/ZaDb 0 Tf 0 g".to_vec()));
                    g.set(b"Kids".to_vec(), Object::Array(Vec::new()));
                    let g = doc.add(Object::Dict(g));
                    attach(doc, af, g, &parts)?;
                    g
                }
            };
            doc.update_dict(widget, |d| d.set(b"Parent".to_vec(), Object::Ref(group)))?;
            doc.update_dict(group, |d| {
                let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
                kids.push(Object::Ref(widget));
                d.set(b"Kids".to_vec(), Object::Array(kids));
            })?;
            let w = Widget {
                obj: widget,
                page: Some(page),
                rect,
                on_state: Some(export.clone()),
                state: Some("Off".into()),
                tab: usize::MAX,
                locked: false,
                hidden: false,
                read_only: false,
                rotation: 0,
                caption: None,
                list: None,
                round: true,
            };
            let ap = appearance::check_box_states(doc, &w, FieldKind::Radio, export, "/ZaDb 0 Tf 0 g");
            doc.update_dict(widget, |d| d.set(b"AP".to_vec(), Object::Dict(ap)))?;
        }
        _ => attach(doc, af, widget, &parts)?,
    }
    add_to_page(doc, page_ref, widget)?;
    redraw_field(doc, &field_name)?;
    Ok(field_name)
}

/// Delete a field: its widgets leave their pages and the field leaves the form.
pub fn delete_field(doc: &mut Document, name: &str) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    delete_found_field(doc, f)
}

/// [`delete_field`] for the field object `obj`, when separate fields share one name.
pub fn delete_field_object(doc: &mut Document, obj: ObjRef) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.obj == obj).ok_or_else(|| FormError::NoSuchField(format!("{obj:?}")))?;
    delete_found_field(doc, f)
}

/// Delete widget `widget` (an index into the field's widgets) of the field `name`, keeping the
/// field and its other widgets: one radio button of a group, or one copy of a duplicated field,
/// as Acrobat's Prepare Form deletes the selected one (#4609). A field with one widget is deleted
/// whole. A per-widget `/Opt` entry goes with its widget, and a check box or radio group that was
/// on through the deleted widget alone turns off.
pub fn delete_widget(doc: &mut Document, name: &str, widget: usize) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let Some(w) = f.widgets.get(widget).cloned() else {
        return invalid(format!("{name} has no widget {}", widget.saturating_add(1)));
    };
    // A field merged with its widget is that widget's only one.
    if f.widgets.len() < 2 || w.obj == f.obj {
        return delete_found_field(doc, f);
    }
    let per_widget_opt = matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) && f.options.len() == f.widgets.len();
    // Turn the group off when no button left *shows* the deleted one's state: a twin with the same
    // on-state name that is currently off does not keep the choice visible.
    let on_only_here = w.on_state.as_deref().is_some_and(|on| {
        f.value.iter().any(|v| v == on) && !f.widgets.iter().enumerate().any(|(i, o)| i != widget && o.state.as_deref() == Some(on))
    });
    drop_from_pages(doc, &std::iter::once(w.obj).collect())?;
    detach(doc, w.obj)?;
    drop_from_calculation_order(doc, |r| r == w.obj)?;
    let mut d = doc.get(f.obj).as_dict().cloned().unwrap_or_default();
    let mut changed = false;
    // The field's own `/Kids` is what made the widget its: `detach` follows the widget's
    // `/Parent`, which a damaged file can leave out or point elsewhere, so the field would keep
    // a widget no page shows.
    if let Some(kids) = d.get(b"Kids").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned())
        && kids.iter().any(|k| k.as_ref() == Some(w.obj))
    {
        d.set(b"Kids".to_vec(), Object::Array(kids.into_iter().filter(|k| k.as_ref() != Some(w.obj)).collect()));
        changed = true;
    }
    if per_widget_opt
        && let Some(opt) = d.get(b"Opt").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned())
        && opt.len() == f.widgets.len()
    {
        let kept: Vec<Object> = opt.into_iter().enumerate().filter(|(i, _)| *i != widget).map(|(_, o)| o).collect();
        d.set(b"Opt".to_vec(), Object::Array(kept));
        changed = true;
    }
    if on_only_here && d.get(b"V").is_some() {
        d.set(b"V".to_vec(), Object::name("Off"));
        changed = true;
    }
    // The default (`/DV`, what Reset puts back) can't name a state no button has any more.
    let dv_only_here = w.on_state.as_deref().is_some_and(|on| {
        d.get(b"DV").map(|v| doc.resolve(v)).and_then(|v| v.as_name().map(<[u8]>::to_vec)).is_some_and(|dv| dv == on.as_bytes())
            && !f.widgets.iter().enumerate().any(|(i, o)| i != widget && o.on_state.as_deref() == Some(on))
    });
    if dv_only_here {
        d.set(b"DV".to_vec(), Object::name("Off"));
        changed = true;
    }
    if changed {
        doc.set(f.obj, Object::Dict(d));
    }
    Ok(())
}

fn delete_found_field(doc: &mut Document, f: Field) -> Result<(), FormError> {
    // Structure cleanup can fail after the field is already unlinked. Restore on that error.
    let snapshot = doc.clone();
    if let Err(error) = delete_found_field_in(doc, f) {
        *doc = snapshot;
        return Err(error);
    }
    Ok(())
}

/// Take the annotations `widgets` off every page's `/Annots`, held directly or by reference.
fn drop_from_pages(doc: &mut Document, widgets: &std::collections::HashSet<ObjRef>) -> Result<(), FormError> {
    for p in page_refs(doc) {
        let Some(a) = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").cloned()) else { continue };
        let list = doc.resolve(&a).as_array().cloned().unwrap_or_default();
        let kept: Vec<Object> = list.iter().filter(|o| !o.as_ref().is_some_and(|r| widgets.contains(&r))).cloned().collect();
        if kept.len() != list.len() {
            match a.as_ref() {
                Some(r) if doc.get(r).as_array().is_some() => doc.set(r, Object::Array(kept)),
                _ => doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(kept)))?,
            }
        }
    }
    Ok(())
}

/// Remove the `/CO` entries `gone` matches, whether the form and the array are held directly or
/// by reference.
pub(super) fn drop_from_calculation_order(doc: &mut Document, gone: impl Fn(ObjRef) -> bool) -> Result<(), FormError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let Some(af_obj) = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned()) else { return Ok(()) };
    let Some(mut af) = doc.resolve(&af_obj).as_dict().cloned() else { return Ok(()) };
    let Some(co_obj) = af.get(b"CO").cloned() else { return Ok(()) };
    let co = doc.resolve(&co_obj).as_array().cloned().unwrap_or_default();
    let kept: Vec<Object> = co.iter().filter(|o| !o.as_ref().is_some_and(&gone)).cloned().collect();
    if kept.len() == co.len() {
        return Ok(());
    }
    match (co_obj.as_ref(), af_obj.as_ref()) {
        (Some(r), _) if doc.get(r).as_array().is_some() => doc.set(r, Object::Array(kept)),
        (_, Some(af_ref)) => doc.update_dict(af_ref, |d| d.set(b"CO".to_vec(), Object::Array(kept)))?,
        (_, None) => {
            af.set(b"CO".to_vec(), Object::Array(kept));
            doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af)))?;
        }
    }
    Ok(())
}

fn delete_found_field_in(doc: &mut Document, f: Field) -> Result<(), FormError> {
    let mut removed: std::collections::HashSet<ObjRef> = f.widgets.iter().map(|w| w.obj).collect();
    removed.insert(f.obj);

    // Climb the /Parent chain: remove `cur` from its parent's /Kids, and if a non-terminal
    // ancestor container is left with no /Kids, remove that ancestor too.
    let mut cur = f.obj;
    let mut seen_parents = std::collections::HashSet::new();
    while let Some(p) = doc.get(cur).as_dict().and_then(|d| d.reference(b"Parent")) {
        if !seen_parents.insert(p) || seen_parents.len() > 64 {
            break;
        }
        let mut d = doc.get(p).as_dict().cloned().unwrap_or_default();
        let kids_obj = d.get(b"Kids").cloned();
        let list = kids_obj.as_ref().map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        // A malformed /Parent alone does not establish a non-terminal relationship.
        if !list.iter().any(|o| o.as_ref() == Some(cur)) {
            break;
        }
        let kept: Vec<Object> = list.into_iter().filter(|o| !o.as_ref().is_some_and(|r| removed.contains(&r))).collect();
        // A parent of a field is non-terminal even when it declares inheritable /FT.
        if kept.is_empty() {
            d.remove(b"Kids");
            doc.set(p, Object::Dict(d));
            removed.insert(p);
            cur = p;
        } else {
            if let Some(Object::Ref(arr_ref)) = kids_obj
                && doc.get(arr_ref).as_array().is_some()
            {
                doc.set(arr_ref, Object::Array(kept.clone()));
            }
            d.set(b"Kids".to_vec(), Object::Array(kept));
            doc.set(p, Object::Dict(d));
            break;
        }
    }

    let pages = page_refs(doc);
    loop {
        let before = removed.len();
        for r in removed.iter().copied().collect::<Vec<_>>() {
            if let Some(pop) = doc.get(r).as_dict().and_then(|d| d.reference(b"Popup")) {
                removed.insert(pop);
            }
        }
        for &p in &pages {
            let Some(a) = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").cloned()) else { continue };
            for item in doc.resolve(&a).as_array().into_iter().flatten() {
                let Some(r) = item.as_ref() else { continue };
                if removed.contains(&r) {
                    continue;
                }
                // Only a popup is removed through /Parent. A dangling /Parent on anything else is cleared below.
                if let Some(d) = doc.get(r).as_dict()
                    && ((d.name(b"Subtype") == Some(b"Popup") && d.reference(b"Parent").is_some_and(|pr| removed.contains(&pr)))
                        || d.reference(b"IRT").is_some_and(|irt| removed.contains(&irt)))
                {
                    removed.insert(r);
                }
            }
        }
        if removed.len() == before {
            break;
        }
    }

    let mut dangling_parents = Vec::new();
    for &p in &pages {
        let Some(a) = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").cloned()) else { continue };
        for item in doc.resolve(&a).as_array().into_iter().flatten() {
            let Some(r) = item.as_ref() else { continue };
            if removed.contains(&r) {
                continue;
            }
            if doc.get(r).as_dict().and_then(|d| d.reference(b"Parent")).is_some_and(|pr| removed.contains(&pr)) {
                dangling_parents.push(r);
            }
        }
    }
    for r in dangling_parents {
        doc.update_dict(r, |d| {
            d.remove(b"Parent");
        })?;
    }

    drop_from_pages(doc, &removed)?;

    if let Some(root) = doc.root() {
        let clean_af = |doc: &mut Document, mut af: Dict| -> Dict {
            if let Some(fields_obj) = af.get(b"Fields").cloned() {
                let list = doc.resolve(&fields_obj).as_array().cloned().unwrap_or_default();
                let kept: Vec<Object> = list.into_iter().filter(|o| !o.as_ref().is_some_and(|r| removed.contains(&r))).collect();
                if let Some(r) = fields_obj.as_ref()
                    && doc.get(r).as_array().is_some()
                {
                    doc.set(r, Object::Array(kept.clone()));
                }
                af.set(b"Fields".to_vec(), Object::Array(kept));
            }
            if let Some(co_obj) = af.get(b"CO").cloned() {
                let list = doc.resolve(&co_obj).as_array().cloned().unwrap_or_default();
                let kept: Vec<Object> = list.into_iter().filter(|o| !o.as_ref().is_some_and(|r| removed.contains(&r))).collect();
                if let Some(r) = co_obj.as_ref()
                    && doc.get(r).as_array().is_some()
                {
                    doc.set(r, Object::Array(kept.clone()));
                }
                if kept.is_empty() {
                    af.remove(b"CO");
                } else {
                    af.set(b"CO".to_vec(), Object::Array(kept));
                }
            }
            af
        };
        match doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned()) {
            Some(Object::Ref(af_ref)) => {
                let af = doc.get(af_ref).as_dict().cloned().unwrap_or_default();
                let cleaned = clean_af(doc, af);
                doc.set(af_ref, Object::Dict(cleaned));
            }
            Some(Object::Dict(af)) => {
                let cleaned = clean_af(doc, af);
                doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(cleaned)))?;
            }
            _ => {}
        }
    }

    drop_struct_tree_refs(doc, &removed)?;
    Ok(())
}

fn struct_refers_to(doc: &Document, o: &Object, gone: &std::collections::HashSet<ObjRef>) -> bool {
    if o.as_ref().is_some_and(|r| gone.contains(&r)) {
        return true;
    }
    doc.resolve(o).as_dict().is_some_and(|d| {
        (d.name(b"Type") == Some(b"OBJR") && d.get(b"Obj").and_then(Object::as_ref).is_some_and(|r| gone.contains(&r)))
            || (d.name(b"Type") == Some(b"MCR")
                && (d.get(b"Stm").and_then(Object::as_ref).is_some_and(|r| gone.contains(&r))
                    || d.get(b"StmOwn").and_then(Object::as_ref).is_some_and(|r| gone.contains(&r))))
    })
}

fn clean_tree_dict(
    doc: &mut Document,
    d: &mut Dict,
    all_gone: &std::collections::HashSet<ObjRef>,
    struct_parents: &std::collections::HashSet<i64>,
    emptied_elems: &std::collections::HashSet<ObjRef>,
    changed_nodes: &std::collections::HashSet<ObjRef>,
    limits: (usize, &mut usize),
) -> Result<bool, FormError> {
    let (depth, budget) = limits;
    if *budget == 0 {
        return invalid("structure cleanup work limit exceeded");
    }
    *budget -= 1;
    if depth > 128 {
        return invalid("structure name-tree nesting exceeds deletion limit");
    }
    let is_empty_elem = |o: &Object| -> bool { o.as_ref().is_some_and(|r| emptied_elems.contains(&r)) };
    let mut changed = false;
    if let Some(nums_obj) = d.get(b"Nums").cloned()
        && let Some(nums) = doc.resolve(&nums_obj).as_array().cloned()
    {
        let mut kept = Vec::with_capacity(nums.len());
        for pair in nums.chunks(2) {
            let [k, v] = pair else {
                kept.extend_from_slice(pair);
                continue;
            };
            if k.as_int().is_some_and(|key| struct_parents.contains(&key)) || struct_refers_to(doc, v, all_gone) || is_empty_elem(v) {
                changed = true;
                continue;
            }
            if let Some(arr) = doc.resolve(v).as_array().cloned() {
                let mut new_arr = Vec::with_capacity(arr.len());
                let mut arr_changed = false;
                for item in arr {
                    if struct_refers_to(doc, &item, all_gone) || is_empty_elem(&item) {
                        new_arr.push(Object::Null);
                        arr_changed = true;
                    } else {
                        new_arr.push(item);
                    }
                }
                if new_arr.iter().all(|x| matches!(x, Object::Null)) {
                    changed = true;
                    continue;
                }
                if arr_changed {
                    changed = true;
                    if let Some(arr_ref) = v.as_ref()
                        && doc.get(arr_ref).as_array().is_some()
                    {
                        doc.set(arr_ref, Object::Array(new_arr.clone()));
                    }
                    kept.push(k.clone());
                    kept.push(Object::Array(new_arr));
                    continue;
                }
            }
            kept.push(k.clone());
            kept.push(v.clone());
        }
        if changed {
            if let Some(nums_ref) = nums_obj.as_ref()
                && doc.get(nums_ref).as_array().is_some()
            {
                doc.set(nums_ref, Object::Array(kept.clone()));
            }
            d.set(b"Nums".to_vec(), Object::Array(kept));
        }
    }
    if let Some(names_obj) = d.get(b"Names").cloned()
        && let Some(names) = doc.resolve(&names_obj).as_array().cloned()
    {
        let mut kept = Vec::with_capacity(names.len());
        let mut names_changed = false;
        for pair in names.chunks(2) {
            let [k, v] = pair else {
                kept.extend_from_slice(pair);
                continue;
            };
            if struct_refers_to(doc, v, all_gone) {
                names_changed = true;
                continue;
            }
            kept.push(k.clone());
            kept.push(v.clone());
        }
        if names_changed {
            changed = true;
            if let Some(names_ref) = names_obj.as_ref()
                && doc.get(names_ref).as_array().is_some()
            {
                doc.set(names_ref, Object::Array(kept.clone()));
            }
            if d.contains(b"Limits") {
                if let (Some(first), Some(last)) = (kept.first(), kept.get(kept.len().saturating_sub(2))) {
                    d.set(b"Limits".to_vec(), Object::Array(vec![first.clone(), last.clone()]));
                } else {
                    d.remove(b"Limits");
                }
            }
            d.set(b"Names".to_vec(), Object::Array(kept));
        }
    }
    let mut kids_changed = false;
    if let Some(kids_obj) = d.get(b"Kids").cloned()
        && let Some(mut kids) = doc.resolve(&kids_obj).as_array().cloned()
    {
        for kid in &mut kids {
            kids_changed |= kid.as_ref().is_some_and(|r| changed_nodes.contains(&r));
            if let Object::Dict(kd) = kid
                && !kd.contains(b"S")
                && clean_tree_dict(doc, kd, all_gone, struct_parents, emptied_elems, changed_nodes, (depth + 1, budget))?
            {
                kids_changed = true;
            }
        }
        if kids_changed {
            changed = true;
            if let Some(r) = kids_obj.as_ref() {
                doc.set(r, Object::Array(kids.clone()));
            }
            d.set(b"Kids".to_vec(), Object::Array(kids));
        }
    }
    if kids_changed
        && d.get(b"Limits").map(|l| doc.resolve(l)).is_some_and(|l| l.as_array().is_some_and(|a| a.iter().any(|v| matches!(v, Object::String(_)))))
        && !d.contains(b"Names")
        && let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k))
        && let Some(kids) = kids.as_array()
    {
        let bounds: Vec<Object> = kids
            .iter()
            .filter_map(|k| doc.resolve(k).as_dict().and_then(|d| d.get(b"Limits")).map(|l| doc.resolve(l)))
            .filter_map(|l| l.as_array().cloned())
            .flatten()
            .collect();
        let limits = match (bounds.first(), bounds.last()) {
            (Some(first), Some(last)) => Some(Object::Array(vec![first.clone(), last.clone()])),
            _ => None,
        };
        if d.get(b"Limits") != limits.as_ref() {
            changed = true;
            if let Some(limits) = limits {
                d.set(b"Limits".to_vec(), limits);
            } else {
                d.remove(b"Limits");
            }
        }
    }

    Ok(changed)
}

// Prune only elements made empty by this deletion, including direct containers.
// References to live structure elements are handled by the bounded reverse-edge walk.
fn prune_empty_struct_k(
    doc: &Document,
    value: &Object,
    gone: &std::collections::HashSet<ObjRef>,
    depth: usize,
    budget: &mut usize,
) -> Result<Option<Object>, FormError> {
    if *budget == 0 {
        return invalid("structure cleanup work limit exceeded");
    }
    *budget -= 1;
    if depth > 128 {
        return invalid("structure child nesting exceeds deletion limit");
    }
    if struct_refers_to(doc, value, gone) {
        return Ok(None);
    }
    let resolved = doc.resolve(value);
    match &*resolved {
        Object::Array(items) => {
            let mut kept = Vec::with_capacity(items.len());
            for item in items {
                if let Some(item) = prune_empty_struct_k(doc, item, gone, depth + 1, budget)? {
                    kept.push(item);
                }
            }
            if kept.len() == items.len() && kept == *items {
                Ok(Some(value.clone()))
            } else if kept.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Object::Array(kept)))
            }
        }
        Object::Dict(d) if value.as_ref().is_none() && d.contains(b"S") => {
            let Some(k) = d.get(b"K") else { return Ok(Some(value.clone())) };
            let Some(kept) = prune_empty_struct_k(doc, k, gone, depth + 1, budget)? else { return Ok(None) };
            let mut d = d.clone();
            d.set(b"K".to_vec(), kept);
            Ok(Some(Object::Dict(d)))
        }
        _ => Ok(Some(value.clone())),
    }
}

fn prune_hit_elements(
    doc: &mut Document,
    elem_hits: Vec<ObjRef>,
    all_gone: &std::collections::HashSet<ObjRef>,
    mut parents: std::collections::HashMap<ObjRef, Vec<ObjRef>>,
    cleanup_budget: &mut usize,
) -> Result<std::collections::HashSet<ObjRef>, FormError> {
    let mut emptied_elems = std::collections::HashSet::new();
    for r in elem_hits {
        let Some(d) = doc.get(r).as_dict().cloned() else { continue };
        let kids = d.get(b"K").map(|k| doc.resolve(k));
        let array = kids.as_ref().is_some_and(|k| matches!(&**k, Object::Array(_)));
        let list: Vec<Object> = match kids.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => d.get(b"K").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        let mut keep = Vec::new();
        for child in list {
            if let Some(child) = prune_empty_struct_k(doc, &child, all_gone, 0, cleanup_budget)? {
                keep.push(child);
            }
        }
        if keep.is_empty() {
            emptied_elems.insert(r);
        }
        doc.update_dict(r, |e| {
            if array && !keep.is_empty() {
                e.set(b"K".to_vec(), Object::Array(keep.clone()));
            } else if let Some(k) = keep.first().cloned() {
                e.set(b"K".to_vec(), k);
            } else {
                e.remove(b"K");
            }
            if keep.is_empty() {
                for key in [&b"Alt"[..], b"ActualText", b"T", b"E", b"ID", b"A"] {
                    e.remove(key);
                }
            }
        })?;
    }
    let mut pending: Vec<ObjRef> = emptied_elems.iter().copied().collect();
    while let Some(empty) = pending.pop() {
        for parent in parents.remove(&empty).unwrap_or_default() {
            if emptied_elems.contains(&parent) {
                continue;
            }
            let Some(k) = doc.get(parent).as_dict().and_then(|d| d.get(b"K").cloned()) else { continue };
            let kept = prune_empty_struct_k(doc, &k, &emptied_elems, 0, cleanup_budget)?;
            if kept.is_none() && emptied_elems.insert(parent) {
                pending.push(parent);
            }
            doc.update_dict(parent, |d| {
                if let Some(k) = kept.clone() {
                    d.set(b"K".to_vec(), k);
                } else {
                    d.remove(b"K");
                    for key in [&b"Alt"[..], b"ActualText", b"T", b"E", b"ID", b"A"] {
                        d.remove(key);
                    }
                }
            })?;
        }
    }
    Ok(emptied_elems)
}

fn drop_struct_tree_refs(doc: &mut Document, gone: &std::collections::HashSet<ObjRef>) -> Result<(), FormError> {
    if gone.is_empty() {
        return Ok(());
    }
    let Some(cat_ref) = doc.root() else { return Ok(()) };
    let Some(st_obj) = doc.get(cat_ref).as_dict().and_then(|c| c.get(b"StructTreeRoot").cloned()) else {
        return Ok(());
    };
    let st_ref = st_obj.as_ref();
    let Some(root) = doc.resolve(&st_obj).as_dict().cloned() else {
        return Ok(());
    };
    let mut struct_parents = std::collections::HashSet::new();
    for &r in gone {
        if let Some(d) = doc.get(r).as_dict() {
            if let Some(sp) = d.int(b"StructParent") {
                struct_parents.insert(sp);
            }
            if let Some(sp) = d.int(b"StructParents") {
                struct_parents.insert(sp);
            }
        }
    }
    let mut all_gone = gone.clone();
    let mut cleanup_budget = 2_000_000;
    let mut todo: Vec<(Object, Option<ObjRef>)> =
        [&b"K"[..], b"ParentTree", b"IDTree"].iter().filter_map(|k| root.get(k).cloned().map(|o| (o, None))).collect();
    let mut parents: std::collections::HashMap<ObjRef, Vec<ObjRef>> = std::collections::HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut seen_arrays = std::collections::HashSet::new();
    let mut elem_hits: Vec<ObjRef> = Vec::new();
    let mut tree_nodes: Vec<ObjRef> = Vec::new();
    while let Some((k, parent)) = todo.pop() {
        match k {
            Object::Array(a) => todo.extend(a.into_iter().map(|o| (o, parent))),
            Object::Dict(d) if d.contains(b"S") => {
                if let Some(kids) = d.get(b"K") {
                    if let Some(parent) = parent
                        && prune_empty_struct_k(doc, kids, &all_gone, 0, &mut cleanup_budget)?.as_ref() != Some(kids)
                    {
                        elem_hits.push(parent);
                    }
                    todo.push((kids.clone(), parent));
                }
            }
            Object::Dict(d) => {
                todo.extend([&b"Nums"[..], b"Names", b"Kids"].iter().filter_map(|key| d.get(key).cloned().map(|o| (o, None))));
            }
            Object::Ref(r) => {
                if struct_refers_to(doc, &Object::Ref(r), gone) {
                    all_gone.insert(r);
                }
                if let Some(parent) = parent
                    && doc.get(r).as_dict().is_some_and(|d| d.contains(b"S"))
                {
                    parents.entry(r).or_default().push(parent);
                }
                let object = doc.get(r);
                if let Object::Array(items) = &*object {
                    if seen_arrays.insert((r, parent)) {
                        if seen.len() + seen_arrays.len() > 2_000_000 {
                            return invalid("structure tree exceeds deletion limit");
                        }
                        todo.extend(items.iter().cloned().map(|o| (o, parent)));
                    }
                    continue;
                }
                if seen.len() + seen_arrays.len() > 2_000_000 {
                    return invalid("structure tree exceeds deletion limit");
                }
                if !seen.insert(r) {
                    continue;
                }
                let Some(d) = object.as_dict() else { continue };
                if !d.contains(b"S") {
                    if d.contains(b"Nums") || d.contains(b"Names") || d.contains(b"Kids") {
                        tree_nodes.push(r);
                    }
                    todo.extend([&b"Nums"[..], b"Names", b"Kids"].iter().filter_map(|key| d.get(key).cloned().map(|o| (o, None))));
                    continue;
                }
                let kids: Vec<Object> = match d.get(b"K").map(|k| doc.resolve(k)).as_deref() {
                    Some(Object::Array(a)) => a.clone(),
                    Some(_) => d.get(b"K").cloned().into_iter().collect(),
                    None => Vec::new(),
                };
                for x in &kids {
                    if let Some(xr) = x.as_ref()
                        && struct_refers_to(doc, x, gone)
                    {
                        all_gone.insert(xr);
                    }
                }
                if kids.iter().any(|x| struct_refers_to(doc, x, &all_gone)) {
                    elem_hits.push(r);
                }
                todo.extend(kids.into_iter().map(|o| (o, Some(r))));
            }
            _ => {}
        }
    }
    let emptied_elems = prune_hit_elements(doc, elem_hits, &all_gone, parents, &mut cleanup_budget)?;
    all_gone.extend(emptied_elems.iter().copied());
    let mut changed_nodes = std::collections::HashSet::new();
    for r in tree_nodes.into_iter().rev() {
        let Some(mut d) = doc.get(r).as_dict().cloned() else { continue };
        if clean_tree_dict(doc, &mut d, &all_gone, &struct_parents, &emptied_elems, &changed_nodes, (0, &mut cleanup_budget))? {
            doc.set(r, Object::Dict(d));
            changed_nodes.insert(r);
        }
    }
    let mut new_root = root.clone();
    let mut root_changed = false;
    if let Some(k) = new_root.get(b"K").cloned() {
        let kept = prune_empty_struct_k(doc, &k, &all_gone, 0, &mut cleanup_budget)?;
        if kept.as_ref() != Some(&k) {
            root_changed = true;
            if let Some(k) = kept {
                new_root.set(b"K".to_vec(), k);
            } else {
                new_root.remove(b"K");
            }
        }
    }
    for key in [&b"ParentTree"[..], b"IDTree"] {
        if let Some(Object::Dict(mut td)) = new_root.get(key).cloned()
            && clean_tree_dict(doc, &mut td, &all_gone, &struct_parents, &emptied_elems, &changed_nodes, (0, &mut cleanup_budget))?
        {
            new_root.set(key.to_vec(), Object::Dict(td));
            root_changed = true;
        }
    }
    if root_changed {
        if let Some(r) = st_ref {
            doc.set(r, Object::Dict(new_root));
        } else {
            doc.update_dict(cat_ref, |c| c.set(b"StructTreeRoot".to_vec(), Object::Dict(new_root)))?;
        }
    }
    Ok(())
}

/// Widget-only keys: they stay with each widget when a merged field is split.
const WIDGET_KEYS: [&[u8]; 13] = [b"Type", b"Subtype", b"Rect", b"P", b"AP", b"AS", b"MK", b"F", b"BS", b"Border", b"H", b"StructParent", b"A"];

/// The `/AA` triggers of a widget annotation (ISO 32000-2 §12.6.3, Table 197). The rest (`K`,
/// `F`, `V`, `C`, Table 199) belong to the field.
const WIDGET_TRIGGERS: [&[u8]; 10] = [b"E", b"X", b"D", b"U", b"Fo", b"Bl", b"PO", b"PC", b"PV", b"PI"];

/// Split additional-actions dictionary `aa` into the widget's triggers and the field's.
/// An empty half is `None`.
pub(super) fn split_triggers(aa: &Dict) -> (Option<Dict>, Option<Dict>) {
    let (mut widget, mut field) = (Dict::new(), Dict::new());
    for (k, v) in aa.iter() {
        let to = if WIDGET_TRIGGERS.contains(&k.as_slice()) { &mut widget } else { &mut field };
        to.set(k.clone(), v.clone());
    }
    let some = |d: Dict| (!d.is_empty()).then_some(d);
    (some(widget), some(field))
}

/// Duplicate a field onto other pages (Acrobat: right-click a field ▸ Duplicate): each page
/// gets a widget at the same place, belonging to the same field, so they share one value.
/// A field that is its own widget is first split into a field and a widget kid. Returns how
/// many widgets were added (pages that already have one are skipped).
pub fn duplicate_field(doc: &mut Document, name: &str, pages: &[usize]) -> Result<usize, FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let model = f.widgets.first().cloned().ok_or_else(|| FormError::Invalid(format!("{name} has no widget")))?;
    let refs = page_refs(doc);
    for p in pages {
        if *p >= refs.len() {
            return invalid(format!("page {} does not exist", p + 1));
        }
    }
    let have: std::collections::HashSet<usize> = f.widgets.iter().filter_map(|w| w.page).collect();
    let targets: Vec<usize> = pages.iter().copied().filter(|p| !have.contains(p)).collect();
    if targets.is_empty() {
        return Ok(0);
    }
    // A merged field/widget becomes a field with one widget kid.
    let model_ref = if model.obj == f.obj { split_merged(doc, &f, &model)? } else { model.obj };
    let template = doc.get(model_ref).as_dict().cloned().unwrap_or_default();
    let mut added = Vec::new();
    for p in &targets {
        let pr = refs[*p];
        let mut w = template.clone();
        w.set(b"P".to_vec(), Object::Ref(pr));
        w.set(b"Parent".to_vec(), Object::Ref(f.obj));
        w.remove(b"StructParent");
        let wr = doc.add(Object::Dict(w));
        let annots = doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned());
        match annots {
            Some(Object::Ref(ar)) => {
                let mut a = doc.get(ar).as_array().cloned().unwrap_or_default();
                a.push(Object::Ref(wr));
                doc.set(ar, Object::Array(a));
            }
            other => {
                let mut a = other.and_then(|o| o.as_array().cloned()).unwrap_or_default();
                a.push(Object::Ref(wr));
                doc.update_dict(pr, |d| d.set(b"Annots".to_vec(), Object::Array(a)))?;
            }
        }
        added.push(wr);
    }
    doc.update_dict(f.obj, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.extend(added.iter().map(|r| Object::Ref(*r)));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })?;
    redraw_field(doc, name)?;
    Ok(added.len())
}

/// Split field `f`, which is also its only widget `model`, into a field and one widget kid that
/// takes the widget's keys and its place in the page's `/Annots`. Returns the kid.
pub(super) fn split_merged(doc: &mut Document, f: &Field, model: &Widget) -> Result<ObjRef, FormError> {
    let refs = page_refs(doc);
    let d = doc.get(f.obj).as_dict().cloned().unwrap_or_default();
    let mut kid = Dict::new();
    for k in WIDGET_KEYS {
        if let Some(v) = d.get(k) {
            kid.set(k.to_vec(), v.clone());
        }
    }
    // The widget's own triggers go with it; the field's stay.
    let aa = d.get(b"AA").and_then(|a| doc.resolve(a).as_dict().cloned());
    let (widget_aa, field_aa) = aa.as_ref().map(split_triggers).unwrap_or_default();
    if let Some(w) = widget_aa {
        kid.set(b"AA".to_vec(), Object::Dict(w));
    }
    kid.set(b"Parent".to_vec(), Object::Ref(f.obj));
    let kr = doc.add(Object::Dict(kid));
    doc.update_dict(f.obj, |d| {
        for k in WIDGET_KEYS {
            d.remove(k);
        }
        if aa.is_some() {
            match field_aa {
                Some(fa) => d.set(b"AA".to_vec(), Object::Dict(fa)),
                None => {
                    d.remove(b"AA");
                }
            }
        }
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(kr)]));
    })?;
    // The page lists the widget now, not the field.
    if let Some(&pr) = model.page.and_then(|p| refs.get(p)) {
        let annots = doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned());
        let swap = |a: Vec<Object>| -> Vec<Object> { a.into_iter().map(|o| if o.as_ref() == Some(f.obj) { Object::Ref(kr) } else { o }).collect() };
        match annots {
            Some(Object::Ref(ar)) => {
                let a = doc.get(ar).as_array().cloned().unwrap_or_default();
                doc.set(ar, Object::Array(swap(a)));
            }
            Some(Object::Array(a)) => doc.update_dict(pr, |d| d.set(b"Annots".to_vec(), Object::Array(swap(a))))?,
            _ => {}
        }
    }
    Ok(kr)
}

#[cfg(test)]
mod structure_cleanup_limits {
    use super::*;

    #[test]
    fn recursive_cleanup_shares_one_work_budget() {
        let mut doc = Document::new_empty();
        let gone = std::collections::HashSet::new();
        let leaves = Object::Array(vec![Object::Null; 8]);
        let mut budget = 3;
        assert!(prune_empty_struct_k(&doc, &leaves, &gone, 0, &mut budget).is_err());
        assert_eq!(budget, 0);
        let mut node = Dict::new();
        let mut budget = 1;
        assert!(!clean_tree_dict(&mut doc, &mut node, &gone, &std::collections::HashSet::new(), &gone, &gone, (0, &mut budget)).unwrap());
        assert!(clean_tree_dict(&mut doc, &mut node, &gone, &std::collections::HashSet::new(), &gone, &gone, (0, &mut budget)).is_err());
        assert_eq!(budget, 0);
    }
}
