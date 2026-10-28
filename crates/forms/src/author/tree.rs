use super::{Dict, Document, Field, FormError, ObjRef, Object, PdfString, invalid};

/// The AcroForm dictionary's reference, creating the form (with `/Helv` in `/DR`) if needed.
pub(crate) fn ensure_form(doc: &mut Document) -> Result<ObjRef, FormError> {
    let root = doc.root().ok_or(FormError::NoForm)?;
    let existing = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned());
    let r = match existing {
        Some(Object::Ref(r)) if doc.get(r).as_dict().is_some() => r,
        other => {
            let d = other.and_then(|o| o.as_dict().cloned()).unwrap_or_default();
            let r = doc.add(Object::Dict(d));
            doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(r)))?;
            r
        }
    };
    let mut af = doc.get(r).as_dict().cloned().unwrap_or_default();
    if !af.contains(b"Fields") {
        af.set(b"Fields".to_vec(), Object::Array(Vec::new()));
    }
    if !af.contains(b"DA") {
        af.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
    }
    let (dr, mut fonts) = form_fonts(doc, &af);
    let mut added = inline_fonts_as_objects(doc, &mut fonts);
    // Acrobat's resource names for the standard fonts its Appearance tab offers.
    for (name, base) in [("Helv", "Helvetica"), ("TiRo", "Times-Roman"), ("Cour", "Courier")] {
        if fonts.contains(name.as_bytes()) {
            continue;
        }
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        let r = doc.add(Object::Dict(f));
        fonts.set(name.as_bytes().to_vec(), Object::Ref(r));
        added = true;
    }
    if added {
        store_form_fonts(doc, &mut af, dr, fonts);
    }
    doc.set(r, Object::Dict(af));
    Ok(r)
}

/// The form dictionary's `/DR` and its `/Font` dictionary, resolved (empty where missing).
fn form_fonts(doc: &Document, af: &Dict) -> (Dict, Dict) {
    let dr = af.get(b"DR").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let fonts = dr.get(b"Font").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    (dr, fonts)
}

/// Each /DR font its own object: Acrobat, redrawing a text field after it is typed in, shows
/// nothing when the font is a dictionary written inline in /DR (#3441). Fonts written inline
/// (by an earlier save, or another writer) become objects saying the same; a font that is an
/// object already, or an entry that isn't a dictionary, is left alone. Whether any changed.
fn inline_fonts_as_objects(doc: &mut Document, fonts: &mut Dict) -> bool {
    let inline: Vec<(Vec<u8>, Dict)> = fonts.iter().filter_map(|(k, v)| Some((k.clone(), v.as_dict()?.clone()))).collect();
    let changed = !inline.is_empty();
    for (name, font) in inline {
        let r = doc.add(Object::Dict(font));
        fonts.set(name, Object::Ref(r));
    }
    changed
}

/// Write `fonts` back as the `/DR /Font` of the form dictionary `af`. An indirect `/DR` or
/// `/Font` stays the object it is.
fn store_form_fonts(doc: &mut Document, af: &mut Dict, mut dr: Dict, fonts: Dict) {
    match dr.get(b"Font").and_then(Object::as_ref) {
        Some(fr) => doc.set(fr, Object::Dict(fonts)),
        None => dr.set(b"Font".to_vec(), Object::Dict(fonts)),
    }
    match af.get(b"DR").and_then(Object::as_ref) {
        Some(dr_ref) => doc.set(dr_ref, Object::Dict(dr)),
        None => af.set(b"DR".to_vec(), Object::Dict(dr)),
    }
}

/// Filling a field: the form's inline `/DR` fonts become objects (see
/// [`inline_fonts_as_objects`]), so a form an earlier version of PdfCraft made works in Acrobat
/// once it has been filled here too (#3441). Only the font storage changes; nothing is added.
/// Not in a form a signature covers: there the file stays as the signer saw it, apart from the
/// value itself.
pub(crate) fn form_fonts_as_objects(doc: &mut Document, all: &[crate::Field]) -> Result<(), FormError> {
    let signed = all.iter().any(|f| f.kind == crate::FieldKind::Signature && holds_value(doc, f.obj));
    if signed {
        return Ok(());
    }
    let Some(root) = doc.root() else { return Ok(()) };
    let Some(entry) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").cloned()) else { return Ok(()) };
    match entry {
        Object::Ref(r) => {
            let Some(mut af) = doc.get(r).as_dict().cloned() else { return Ok(()) };
            let (dr, mut fonts) = form_fonts(doc, &af);
            if inline_fonts_as_objects(doc, &mut fonts) {
                store_form_fonts(doc, &mut af, dr, fonts);
                doc.set(r, Object::Dict(af));
            }
        }
        // A form dictionary written directly in the catalog (the catalog's /AcroForm need not be
        // an indirect reference) is written back there.
        Object::Dict(mut af) => {
            let (dr, mut fonts) = form_fonts(doc, &af);
            if inline_fonts_as_objects(doc, &mut fonts) {
                store_form_fonts(doc, &mut af, dr, fonts);
                doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(af)))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether the field `r`, or the nearest ancestor that has one, holds a `/V` (the entry is
/// inheritable, ISO 32000-2 §12.7.4). The `/Parent` chain is followed at most 64 deep, as deep
/// as the field walk goes, so a cyclic chain ends.
fn holds_value(doc: &Document, r: ObjRef) -> bool {
    let mut at = r;
    for _ in 0..=64 {
        let obj = doc.get(at);
        let Some(d) = obj.as_dict() else { return false };
        if d.contains(b"V") {
            return true;
        }
        match d.reference(b"Parent") {
            Some(p) => at = p,
            None => return false,
        }
    }
    false
}

pub(super) fn add_to_page(doc: &mut Document, page: ObjRef, widget: ObjRef) -> Result<(), FormError> {
    let existing = doc.get(page).as_dict().and_then(|d| d.get(b"Annots").cloned());
    match existing.as_ref().and_then(|o| o.as_ref()).filter(|r| doc.get(*r).as_array().is_some()) {
        Some(r) => {
            let mut a = doc.get(r).as_array().cloned().unwrap_or_default();
            a.push(Object::Ref(widget));
            doc.set(r, Object::Array(a));
        }
        None => {
            let mut a = existing.and_then(|o| o.as_array().cloned()).unwrap_or_default();
            a.push(Object::Ref(widget));
            doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(a)))?;
        }
    }
    Ok(())
}

fn add_to_fields(doc: &mut Document, af: ObjRef, field: ObjRef) -> Result<(), FormError> {
    let mut d = doc.get(af).as_dict().cloned().unwrap_or_default();
    let mut list = d.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    list.push(Object::Ref(field));
    d.set(b"Fields".to_vec(), Object::Array(list));
    doc.set(af, Object::Dict(d));
    Ok(())
}

/// The most parts a dotted field name may have ("a.b.c" has three).
const MAX_NAME_PARTS: usize = 32;

/// The field attributes a field inherits from its parents (ISO 32000-2 §12.7.4.1).
const INHERITED_KEYS: [&[u8]; 7] = [b"FT", b"Ff", b"V", b"DV", b"DA", b"Q", b"MaxLen"];

/// The parts of a full field name: `a.b` is the field `b` under the parent field `a`, since a
/// field's own (partial) name can't contain a period (ISO 32000-2 §12.7.4.2) (#2696).
pub(super) fn name_parts(name: &str) -> Result<Vec<&str>, FormError> {
    let parts: Vec<&str> = name.split('.').take(MAX_NAME_PARTS.saturating_add(1)).collect();
    if parts.len() > MAX_NAME_PARTS {
        return invalid(format!("field names can have at most {MAX_NAME_PARTS} parts"));
    }
    if parts.iter().any(|p| p.trim().is_empty()) {
        return invalid("field names can't be empty, or have an empty part between periods");
    }
    Ok(parts)
}

/// Why the full name `name` can't be used beside the field named `other`, if it can't: the
/// same name, or one inside the other (a field that has a value can't also hold fields).
pub(super) fn clash(name: &str, other: &str) -> Option<String> {
    if name == other {
        return Some(format!("a field named {name:?} already exists"));
    }
    let inside = |a: &str, b: &str| a.strip_prefix(b).is_some_and(|rest| rest.starts_with('.'));
    (inside(name, other) || inside(other, name)).then(|| format!("{name:?} can't be used beside the field {other:?}"))
}

/// The array `key` of `holder` (`/Fields` of the form, `/Kids` of a field).
fn list_of(doc: &Document, holder: ObjRef, key: &[u8]) -> Vec<Object> {
    let d = doc.get(holder).as_dict().cloned().unwrap_or_default();
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default()
}

/// Add `field` to `parent`'s `/Kids`, or to the form's `/Fields` at the top.
fn link(doc: &mut Document, af: ObjRef, parent: Option<ObjRef>, field: ObjRef) -> Result<(), FormError> {
    let Some(p) = parent else { return add_to_fields(doc, af, field) };
    let mut kids = list_of(doc, p, b"Kids");
    kids.push(Object::Ref(field));
    doc.update_dict(p, |d| d.set(b"Kids".to_vec(), Object::Array(kids)))?;
    Ok(())
}

/// The node named `name` (its own `/T`) directly under `parent`, or at the top of the form.
pub(super) fn child_named(doc: &Document, af: ObjRef, parent: Option<ObjRef>, name: &str) -> Option<ObjRef> {
    let list = match parent {
        Some(p) => list_of(doc, p, b"Kids"),
        None => list_of(doc, af, b"Fields"),
    };
    list.iter()
        .filter_map(|o| o.as_ref())
        .find(|r| doc.get(*r).as_dict().and_then(|d| d.get(b"T").cloned()).and_then(|t| crate::text_of(&doc.resolve(&t))).as_deref() == Some(name))
}

/// The values a parent field passes to the fields under it that can't be turned off by giving
/// the field a neutral one of its own, as `/Ff` and `/Q` can.
const PASSED_VALUES: [&[u8]; 3] = [b"V", b"DV", b"MaxLen"];

/// Whether `field` may go in the field tree under the full name `parts`, changing nothing. The
/// field list holds only terminal fields, so a parent left empty by a move isn't in it: a node
/// of the same name there is found here.
/// `inherited` is what the field will hold as its own before it moves (see [`inherited`]).
pub(super) fn check_place(doc: &Document, af: ObjRef, field: ObjRef, parts: &[&str], inherited: &Dict) -> Result<(), FormError> {
    let Some((own, parents)) = parts.split_last() else { return invalid("field names can't be empty") };
    let mut owned = doc.get(field).as_dict().cloned().unwrap_or_default();
    for (k, v) in inherited.iter() {
        owned.set(k.to_vec(), v.clone());
    }
    let mut at: Option<ObjRef> = None;
    let mut whole_path = true;
    for part in parents {
        let Some(r) = child_named(doc, af, at, part) else {
            whole_path = false;
            break;
        };
        // A value, reset value or length limit held by a parent would pass to this field.
        let held = doc.get(r).as_dict().is_some_and(|d| PASSED_VALUES.iter().any(|k| d.contains(k) && !owned.contains(k)));
        if held {
            return invalid(format!("{part:?} holds a value for the fields under it, so {:?} can't be put there", parts.join(".")));
        }
        at = Some(r);
    }
    // A field being moved is still in the tree here, but under another full name (a move to its
    // own name changes nothing and doesn't get here), so a node found is another.
    if whole_path && child_named(doc, af, at, own).is_some() {
        return invalid(format!("a field named {:?} already exists", parts.join(".")));
    }
    Ok(())
}

/// Put `field` in the field tree under the full name `parts`: its own `/T` is the last part,
/// and the parts before it name parent fields, reused where the tree has them and made where
/// not (Acrobat makes `a.b` a field `b` under a field `a`).
pub(super) fn attach(doc: &mut Document, af: ObjRef, field: ObjRef, parts: &[&str]) -> Result<(), FormError> {
    let Some((own, parents)) = parts.split_last() else { return invalid("field names can't be empty") };
    check_place(doc, af, field, parts, &Dict::new())?;
    let mut parent: Option<ObjRef> = None;
    let mut inherits = false;
    for part in parents {
        let next = match child_named(doc, af, parent, part) {
            Some(r) => {
                inherits |= doc.get(r).as_dict().is_some_and(|d| d.contains(b"Ff") || d.contains(b"Q"));
                r
            }
            None => {
                let mut d = Dict::new();
                d.set(b"T".to_vec(), PdfString::text(part));
                d.set(b"Kids".to_vec(), Object::Array(Vec::new()));
                if let Some(p) = parent {
                    d.set(b"Parent".to_vec(), Object::Ref(p));
                }
                let r = doc.add(Object::Dict(d));
                link(doc, af, parent, r)?;
                r
            }
        };
        parent = Some(next);
    }
    doc.update_dict(field, |d| {
        d.set(b"T".to_vec(), PdfString::text(own));
        match parent {
            Some(p) => d.set(b"Parent".to_vec(), Object::Ref(p)),
            None => {
                d.remove(b"Parent");
            }
        }
        // An existing parent's flags and alignment would otherwise pass to this field.
        if inherits {
            for k in [&b"Ff"[..], b"Q"] {
                if !d.contains(k) {
                    d.set(k.to_vec(), Object::Int(0));
                }
            }
        }
    })?;
    link(doc, af, parent, field)
}

/// What `field` inherits from its parents and doesn't hold itself, changing nothing.
pub(super) fn inherited(doc: &Document, field: ObjRef) -> Dict {
    let own = doc.get(field).as_dict().cloned().unwrap_or_default();
    let mut add = Dict::new();
    let mut seen = std::collections::HashSet::new();
    let mut parent = own.reference(b"Parent");
    while let Some(p) = parent {
        if seen.len() > 64 || !seen.insert(p) {
            break;
        }
        let Some(d) = doc.get(p).as_dict().cloned() else { break };
        for k in INHERITED_KEYS {
            if let Some(v) = d.get(k).filter(|_| !own.contains(k) && !add.contains(k)) {
                add.set(k.to_vec(), v.clone());
            }
        }
        parent = d.reference(b"Parent");
    }
    add
}

/// Copy onto `field` what it inherits (`add`, from [`inherited`]), so it keeps it when it moves.
pub(super) fn own_inherited(doc: &mut Document, field: ObjRef, add: &Dict) -> Result<(), FormError> {
    if add.iter().next().is_some() {
        doc.update_dict(field, |d| {
            for (k, v) in add.iter() {
                d.set(k.to_vec(), v.clone());
            }
        })?;
    }
    Ok(())
}

/// Take `field` out of the field tree: from its parent's `/Kids`, or from `/Fields`.
pub(super) fn detach(doc: &mut Document, field: ObjRef) -> Result<(), FormError> {
    let parent = doc.get(field).as_dict().and_then(|d| d.reference(b"Parent"));
    let remove_from = |doc: &mut Document, holder: ObjRef, key: &[u8]| -> Result<(), FormError> {
        let mut d = doc.get(holder).as_dict().cloned().unwrap_or_default();
        let list = d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        let kept: Vec<Object> = list.into_iter().filter(|o| o.as_ref() != Some(field)).collect();
        d.set(key.to_vec(), Object::Array(kept));
        doc.set(holder, Object::Dict(d));
        Ok(())
    };
    match parent {
        Some(p) => remove_from(doc, p, b"Kids")?,
        None => {
            let root = doc.root().ok_or(FormError::NoForm)?;
            match doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned()) {
                Some(Object::Ref(af)) => remove_from(doc, af, b"Fields")?,
                // A form dictionary held directly in the catalog.
                Some(Object::Dict(mut af)) => {
                    let list = af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
                    let kept: Vec<Object> = list.into_iter().filter(|o| o.as_ref() != Some(field)).collect();
                    af.set(b"Fields".to_vec(), Object::Array(kept));
                    doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af)))?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// The first free "<base>N" name: not a field's, and not held by a node at the top of the field
/// tree (`held`), such as a parent field left empty by a move, which isn't in `existing`.
pub(super) fn free_name(existing: &[Field], held: &[String], base: &str) -> String {
    (1..=u64::MAX)
        .map(|n| format!("{base}{n}"))
        .find(|c| !held.contains(c) && !existing.iter().any(|f| f.name == *c || f.name.starts_with(&format!("{c}."))))
        .unwrap_or_else(|| base.to_string())
}

/// The own names (`/T`) of the nodes in the form's `/Fields`, changing nothing.
pub(super) fn top_names(doc: &Document) -> Vec<String> {
    let Some(root) = doc.root() else { return Vec::new() };
    let Some(af) = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").map(|o| doc.resolve(o))).and_then(|o| o.as_dict().cloned()) else {
        return Vec::new();
    };
    let list = af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    list.iter().filter_map(|o| doc.resolve(o).as_dict().and_then(|d| d.get(b"T").cloned())).filter_map(|t| crate::text_of(&doc.resolve(&t))).collect()
}
