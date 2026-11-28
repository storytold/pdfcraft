//! Copy and paste a field (#1571): a new, independently named field from some or all of a
//! field's widgets.

use super::lifecycle::WIDGET_KEYS;
use super::look::redraw_field;
use super::tree::{add_to_fields, add_to_page, ensure_form, free_name, inherited, top_names};
use super::{Dict, Document, FieldKind, FormError, Object, PdfString, Widget, fields, invalid, page_refs};

/// Keys a copy never takes from its source: what ties a field into the tree, names it, or binds
/// it to a structure element or an XFA template node.
pub(super) const COPY_DROPS: [&[u8]; 8] = [b"Kids", b"Parent", b"T", b"NM", b"StructParent", b"PCSom", b"PCXfaNode", b"PCItems"];

/// Give widget `w` a look of its own: every stream its `/AP` reaches (`/N`, `/D`, `/R`, directly
/// or per state) is added again as a new object, and an indirect `/MK` or `/BS` becomes direct,
/// so redrawing or restyling the copy can't change the source. Resources the streams use stay
/// shared.
pub(super) fn own_look(doc: &mut Document, w: &mut Dict) {
    for key in [&b"MK"[..], b"BS"] {
        if let Some(value) = w.get(key).filter(|v| Object::as_ref(v).is_some())
            && let Some(dict) = doc.resolve(value).as_dict().cloned()
        {
            w.set(key.to_vec(), Object::Dict(dict));
        }
    }
    let Some(ap) = w.get(b"AP").and_then(|o| doc.resolve(o).as_dict().cloned()) else { return };
    let mut fresh = Dict::new();
    for (key, look) in ap.iter() {
        let owned = match &*doc.resolve(look) {
            Object::Stream(_) => own_stream(doc, look),
            Object::Dict(states) => {
                let mut mine = Dict::new();
                for (state, stream) in states.iter() {
                    let value = own_stream(doc, stream);
                    mine.set(state.clone(), value);
                }
                Object::Dict(mine)
            }
            _ => look.clone(),
        };
        fresh.set(key.clone(), owned);
    }
    w.set(b"AP".to_vec(), Object::Dict(fresh));
}

/// A new object holding the stream `look` refers to; anything else as it is.
fn own_stream(doc: &mut Document, look: &Object) -> Object {
    match Object::as_ref(look) {
        Some(r) if matches!(&*doc.get(r), Object::Stream(_)) => {
            let stream = (*doc.get(r)).clone();
            Object::Ref(doc.add(stream))
        }
        _ => look.clone(),
    }
}

/// `name` as a position in a button field's `/Opt` (`/0`, `/1` ...), as ISO 32000-2 lets the
/// on states of buttons with `/Opt` be named.
fn opt_position(name: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(name).ok()?;
    s.parse().ok().filter(|i: &usize| i.to_string() == s)
}

/// `name` renamed after the new position `moved` gives its old one, when it is a position.
fn renumbered(name: &[u8], moved: &std::collections::HashMap<usize, usize>) -> Option<Vec<u8>> {
    opt_position(name).and_then(|i| moved.get(&i)).map(|j| j.to_string().into_bytes())
}

/// Rename a copied widget's on states that are `/Opt` positions (in `/AP /N`, `/AP /D` and
/// `/AS`) after the positions its field's copied `/Opt` gives them. Other names stay.
fn renumber_states(doc: &Document, w: &mut Dict, moved: &std::collections::HashMap<usize, usize>) {
    if let Some(mut ap) = w.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()) {
        let mut changed = false;
        for k in [&b"N"[..], b"D"] {
            let Some(states) = ap.get(k).map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()) else { continue };
            if states.iter().any(|(n, _)| renumbered(n, moved).is_some()) {
                let mut out = Dict::new();
                for (n, v) in states.iter() {
                    out.set(renumbered(n, moved).unwrap_or_else(|| n.clone()), v.clone());
                }
                ap.set(k.to_vec(), Object::Dict(out));
                changed = true;
            }
        }
        if changed {
            w.set(b"AP".to_vec(), Object::Dict(ap));
        }
    }
    if let Some(s) = w.name(b"AS").and_then(|s| renumbered(s, moved)) {
        w.set(b"AS".to_vec(), Object::Name(s));
    }
}

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
        if !COPY_DROPS.contains(&k.as_slice()) && !WIDGET_KEYS.contains(&k.as_slice()) {
            field.set(k.clone(), v.clone());
        }
    }
    // The copy is a top-level field: it takes along what the original inherits.
    for (k, v) in inherited(doc, f.obj).iter() {
        field.set(k.clone(), v.clone());
    }
    // A signature's value is someone's signature: the copy is a blank signature field.
    let signature = field.name(b"FT") == Some(&b"Sig"[..]);
    if signature {
        field.remove(b"V");
        field.remove(b"Lock");
    }
    // A check box or radio group's /Opt holds one export value per widget: the copied ones'.
    // When its on states are named after those positions (/0, /1 ...), the copies' are renamed
    // after their new ones, and a value naming a button left behind is off.
    let mut moved = std::collections::HashMap::new();
    if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) {
        let opt = field.get(b"Opt").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned());
        if let Some(opt) = opt.filter(|o| o.len() == f.widgets.len()) {
            let at: Vec<usize> = picked.iter().filter_map(|p| f.widgets.iter().position(|w| w.obj == p.obj)).collect();
            field.set(b"Opt".to_vec(), Object::Array(at.iter().filter_map(|i| opt.get(*i).cloned()).collect()));
            if f.widgets.iter().enumerate().all(|(i, w)| w.on_state.as_deref().and_then(|s| opt_position(s.as_bytes())) == Some(i)) {
                moved = at.iter().enumerate().map(|(new, old)| (*old, new)).collect();
                for k in [&b"V"[..], b"DV"] {
                    let v = field.get(k).map(|v| doc.resolve(v)).and_then(|v| v.as_name().map(<[u8]>::to_vec));
                    if let Some(v) = v.filter(|v| opt_position(v).is_some()) {
                        field.set(k.to_vec(), Object::Name(renumbered(&v, &moved).unwrap_or_else(|| b"Off".to_vec())));
                    }
                }
            }
        }
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
        for k in COPY_DROPS {
            d.remove(k);
        }
        // The signer's visible signature isn't the copy's: redrawn blank below.
        if signature {
            d.remove(b"AP");
            d.remove(b"AS");
        }
        renumber_states(doc, &mut d, &moved);
        own_look(doc, &mut d);
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
    if signature {
        redraw_field(doc, &new_name)?;
    }
    // A calculated field's copy calculates too.
    let co = doc.get(af).as_dict().and_then(|d| d.get(b"CO").cloned()).map(|c| doc.resolve(&c)).and_then(|c| c.as_array().cloned());
    if let Some(mut co) = co.filter(|co| co.iter().any(|o| o.as_ref() == Some(f.obj))) {
        co.push(Object::Ref(top));
        doc.update_dict(af, |d| d.set(b"CO".to_vec(), Object::Array(co)))?;
    }
    Ok(new_name)
}
