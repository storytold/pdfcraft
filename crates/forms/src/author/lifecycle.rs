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
    let widgets: std::collections::HashSet<ObjRef> = f.widgets.iter().map(|w| w.obj).collect();
    drop_from_pages(doc, &widgets)?;
    detach(doc, f.obj)?;
    // Out of the calculation order too: a `/CO` entry would keep the field, its value and its
    // widgets reachable for a full save.
    drop_from_calculation_order(doc, |r| r == f.obj || widgets.contains(&r))
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
