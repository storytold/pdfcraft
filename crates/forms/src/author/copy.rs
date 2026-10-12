//! Copy and paste a field (#1571): a new, independently named field from some or all of a
//! field's widgets.

use super::lifecycle::WIDGET_KEYS;
use super::tree::{add_to_fields, add_to_page, ensure_form, free_name, top_names};
use super::{Dict, Document, FieldKind, FormError, Object, PdfString, Widget, fields, invalid, page_refs};


/// Field entries a kid inherits from its ancestors (ISO 32000-2 §12.7.4.1, Tables 226 and 228).
const INHERITED_KEYS: [&[u8]; 6] = [b"FT", b"Ff", b"V", b"DV", b"DA", b"Q"];

/// "Check Box1" → "Check Box", "address.city" → "city": what a copy's name counts up from.
fn copy_base(name: &str) -> &str {
    let leaf = name.rsplit('.').next().unwrap_or(name);
    let base = leaf.trim_end_matches(|c: char| c.is_ascii_digit());
    if base.trim().is_empty() { "Field" } else { base }
}

/// Copy and paste a field (#1571): a new field of its own, with its own name and value, made of
/// copies of widgets `widgets` (indices in [`Field::widgets`]; every widget when empty), placed
/// on `page` (0-based) and moved by `offset` in user space. Type, flags, options, value,
/// appearance and actions come along; a signature's value doesn't. The name is the first free
/// one after the original's ("Check Box1" → "Check Box2"). Returns it.
pub fn copy_field(doc: &mut Document, name: &str, widgets: &[usize], page: usize, offset: [f64; 2]) -> Result<String, FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let page_ref = *page_refs(doc).get(page).ok_or_else(|| FormError::Invalid(format!("page {} does not exist", page + 1)))?;
    if !offset.iter().all(|v| v.is_finite()) {
        return invalid("the offset must be a finite number");
    }
    let mut picked: Vec<&Widget> = Vec::new();
    for i in widgets {
        let w = f.widgets.get(*i).ok_or_else(|| FormError::Invalid(format!("{name} has no widget {i}")))?;
        if !picked.iter().any(|p| p.obj == w.obj) {
            picked.push(w);
        }
    }
    if widgets.is_empty() {
        picked = f.widgets.iter().collect();
    }
    if picked.is_empty() {
        return invalid(format!("{name} has no widget"));
    }
    let own = doc.get(f.obj).as_dict().cloned().unwrap_or_default();
    let mut field = Dict::new();
    for (k, v) in own.iter() {
        if !matches!(k.as_slice(), b"Parent" | b"Kids" | b"T") && !WIDGET_KEYS.contains(&k.as_slice()) {
            field.set(k.clone(), v.clone());
        }
    }
    // The copy is a top-level field: it takes along what the original inherits.
    let mut seen = std::collections::HashSet::from([f.obj]);
    let mut up = own.reference(b"Parent");
    while let Some(p) = up.filter(|p| seen.len() < 64 && seen.insert(*p)) {
        let Some(d) = doc.get(p).as_dict().cloned() else { break };
        for k in INHERITED_KEYS {
            if let (false, Some(v)) = (field.contains(k), d.get(k)) {
                field.set(k.to_vec(), v.clone());
            }
        }
        up = d.reference(b"Parent");
    }
    // A signature's value is someone's signature: the copy is a blank signature field.
    if field.name(b"FT") == Some(&b"Sig"[..]) {
        field.remove(b"V");
        field.remove(b"Lock");
    }
    // Radio buttons copied without the selected one: none of the copies is selected.
    if f.kind == FieldKind::Radio && !f.value.iter().any(|v| picked.iter().any(|w| w.on_state.as_ref() == Some(v))) {
        field.set(b"V".to_vec(), Object::name("Off"));
    }
    let new_name = free_name(&all, &top_names(doc), copy_base(&f.name));
    field.set(b"T".to_vec(), PdfString::text(&new_name));
    let mut kids = Vec::new();
    for w in &picked {
        let mut d = if w.obj == f.obj {
            let mut d = Dict::new();
            for k in WIDGET_KEYS {
                if let Some(v) = own.get(k) {
                    d.set(k.to_vec(), v.clone());
                }
            }
            d
        } else {
            doc.get(w.obj).as_dict().cloned().unwrap_or_default()
        };
        d.remove(b"Parent");
        d.remove(b"StructParent");
        d.set(b"P".to_vec(), Object::Ref(page_ref));
        let r = w.rect;
        let rect = [r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]];
        if !rect.iter().all(|v| v.is_finite()) {
            return invalid("the offset moves the field out of range");
        }
        d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
        kids.push(d);
    }
    let af = ensure_form(doc)?;
    let top = match kids.pop() {
        // One widget: field and widget in one dictionary, as added fields are.
        Some(only) if kids.is_empty() => {
            for (k, v) in only.iter() {
                field.set(k.clone(), v.clone());
            }
            let r = doc.add(Object::Dict(field));
            add_to_page(doc, page_ref, r)?;
            r
        }
        last => {
            kids.extend(last);
            let parent = doc.add(Object::Dict(field));
            let mut refs = Vec::new();
            for mut d in kids {
                d.set(b"Parent".to_vec(), Object::Ref(parent));
                let r = doc.add(Object::Dict(d));
                add_to_page(doc, page_ref, r)?;
                refs.push(Object::Ref(r));
            }
            doc.update_dict(parent, |d| d.set(b"Kids".to_vec(), Object::Array(refs)))?;
            parent
        }
    };
    add_to_fields(doc, af, top)?;
    // A calculated field's copy calculates too.
    let co = doc.get(af).as_dict().and_then(|d| d.get(b"CO").cloned()).map(|c| doc.resolve(&c)).and_then(|c| c.as_array().cloned());
    if let Some(mut co) = co.filter(|co| co.iter().any(|o| o.as_ref() == Some(f.obj))) {
        co.push(Object::Ref(top));
        doc.update_dict(af, |d| d.set(b"CO".to_vec(), Object::Array(co)))?;
    }
    Ok(new_name)
}
