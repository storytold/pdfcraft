//! After Split Pages (#517): a destination that shows a point of a split page (a bookmark, a
//! link, a named destination, the document's opening view) goes to the part holding that point,
//! not always to the first part (CodeRabbit on #5587).

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{OrganizeError, outline, walk};

/// The parts each split page became: every part's page and crop box, in reading order. The first
/// part is the page itself.
pub(crate) type Parts = HashMap<ObjRef, Vec<(ObjRef, [f64; 4])>>;

/// How many references and `/D` entries a destination is followed through.
const MAX_DEST_DEPTH: u8 = 4;

/// Retarget every destination in `doc` that shows a point of a split page: the `/Dest` and GoTo
/// `/A` of every annotation on every page and of every bookmark, the named destinations (the
/// `/Names /Dests` tree and the legacy `/Dests` dictionary) and the catalog's `/OpenAction`.
/// A destination that shows a whole page (`/Fit`, `/FitB`), or a point outside every part, stays
/// on the first part.
pub(crate) fn retarget(doc: &mut Document, parts: &Parts) -> Result<(), OrganizeError> {
    if parts.is_empty() {
        return Ok(());
    }
    let mut holders = outline::items(doc);
    for (page, _) in walk(doc)? {
        let annots = doc.get(page).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| doc.resolve(&a));
        holders.extend(annots.as_deref().and_then(Object::as_array).into_iter().flatten().filter_map(Object::as_ref));
    }
    let mut seen = HashSet::new();
    for h in holders {
        if !seen.insert(h) {
            continue;
        }
        let Some(mut d) = doc.get(h).as_dict().cloned() else { continue };
        let mut changed = false;
        if let Some(dest) = d.get(b"Dest").cloned()
            && let Some(new) = retarget_dest(doc, parts, &dest, 0)?
        {
            d.set(b"Dest".to_vec(), new);
            changed = true;
        }
        if let Some(action) = d.get(b"A").cloned()
            && let Some(new) = retarget_goto(doc, parts, &action)?
        {
            d.set(b"A".to_vec(), new);
            changed = true;
        }
        if changed {
            doc.update_dict(h, |x| *x = d)?;
        }
    }
    let Some(root) = doc.root() else { return Ok(()) };
    let Some(catalog) = doc.get(root).as_dict().cloned() else { return Ok(()) };
    if let Some(open) = catalog.get(b"OpenAction") {
        let new = match &*doc.resolve(open) {
            Object::Dict(_) => retarget_goto(doc, parts, open)?,
            _ => retarget_dest(doc, parts, open, 0)?,
        };
        if let Some(new) = new {
            doc.update_dict(root, |c| c.set(b"OpenAction".to_vec(), new))?;
        }
    }
    match catalog.get(b"Dests") {
        Some(Object::Ref(r)) => {
            if let Some(mut d) = doc.get(*r).as_dict().cloned()
                && retarget_legacy(doc, parts, &mut d)?
            {
                doc.update_dict(*r, |x| *x = d)?;
            }
        }
        Some(Object::Dict(d)) => {
            let mut d = d.clone();
            if retarget_legacy(doc, parts, &mut d)? {
                doc.update_dict(root, |c| c.set(b"Dests".to_vec(), Object::Dict(d)))?;
            }
        }
        _ => {}
    }
    match catalog.get(b"Names") {
        Some(Object::Ref(n)) => {
            let tree = doc.get(*n).as_dict().and_then(|d| d.get(b"Dests").cloned());
            if let Some(new) = tree.map(|t| retarget_tree(doc, parts, &t)).transpose()?.flatten() {
                doc.update_dict(*n, |d| d.set(b"Dests".to_vec(), Object::Dict(new)))?;
            }
        }
        Some(Object::Dict(names)) => {
            if let Some(new) = names.get(b"Dests").map(|t| retarget_tree(doc, parts, t)).transpose()?.flatten() {
                let mut names = names.clone();
                names.set(b"Dests".to_vec(), Object::Dict(new));
                doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(names)))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A GoTo action (or a reference to one) retargeted; other actions name no page of this document.
fn retarget_goto(doc: &mut Document, parts: &Parts, action: &Object) -> Result<Option<Object>, OrganizeError> {
    if doc.resolve(action).as_dict().is_some_and(|a| a.name(b"S") == Some(b"GoTo")) { retarget_dest(doc, parts, action, 0) } else { Ok(None) }
}

/// `o` retargeted: a destination array, a dictionary whose `/D` is one (a named destination's
/// value, or a GoTo action), or a reference to either. A reference is rewritten where it points
/// and yields `None`, as does a destination that stays.
fn retarget_dest(doc: &mut Document, parts: &Parts, o: &Object, depth: u8) -> Result<Option<Object>, OrganizeError> {
    if depth > MAX_DEST_DEPTH {
        return Ok(None);
    }
    match o {
        Object::Array(a) => Ok(retarget_array(doc, parts, a).map(Object::Array)),
        Object::Dict(d) => {
            let Some(inner) = d.get(b"D") else { return Ok(None) };
            Ok(retarget_dest(doc, parts, inner, depth + 1)?.map(|new| {
                let mut d = d.clone();
                d.set(b"D".to_vec(), new);
                Object::Dict(d)
            }))
        }
        Object::Ref(r) => {
            let target = doc.get(*r);
            if let Some(new) = retarget_dest(doc, parts, &target, depth + 1)? {
                doc.set(*r, new);
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// A destination array whose page was split, pointed at the part holding the point it shows
/// (ISO 32000-2 §12.3.2.2): `/XYZ` left and top, `/FitH` and `/FitBH` top, `/FitV` and `/FitBV`
/// left, the centre of a `/FitR` rectangle. `None` when it stays as it is.
fn retarget_array(doc: &Document, parts: &Parts, a: &[Object]) -> Option<Vec<Object>> {
    let page = a.first()?.as_ref()?;
    let cells = parts.get(&page)?;
    let num = |i: usize| a.get(i).map(|o| doc.resolve(o)).and_then(|o| o.as_f64()).filter(|v| v.is_finite());
    let (x, y) = match doc.resolve(a.get(1)?).as_name()? {
        b"XYZ" => (num(2), num(3)),
        b"FitH" | b"FitBH" => (None, num(2)),
        b"FitV" | b"FitBV" => (num(2), None),
        b"FitR" => match (num(2), num(3), num(4), num(5)) {
            (Some(l), Some(b), Some(r), Some(t)) => (Some(l / 2.0 + r / 2.0), Some(b / 2.0 + t / 2.0)),
            _ => (None, None),
        },
        _ => (None, None),
    };
    if x.is_none() && y.is_none() {
        return None;
    }
    // An unset coordinate keeps the viewer's, so it matches every part.
    let within = |v: Option<f64>, lo: f64, hi: f64| v.is_none_or(|v| lo <= v && v <= hi);
    let (part, _) = cells.iter().find(|(_, c)| within(x, c[0], c[2]) && within(y, c[1], c[3]))?;
    if *part == page {
        return None;
    }
    let mut out = a.to_vec();
    *out.first_mut()? = Object::Ref(*part);
    Some(out)
}

/// Retarget the values of a legacy `/Dests` dictionary. True when one changed in place.
fn retarget_legacy(doc: &mut Document, parts: &Parts, dests: &mut Dict) -> Result<bool, OrganizeError> {
    let mut changed = false;
    let entries: Vec<(Vec<u8>, Object)> = dests.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (k, v) in entries {
        if let Some(new) = retarget_dest(doc, parts, &v, 0)? {
            dests.set(k, new);
            changed = true;
        }
    }
    Ok(changed)
}

/// Retarget the values of a name tree held in `slot`. A tree held by reference is rewritten in
/// place and yields `None`; an inline root that changed is handed back. The walk is iterative and
/// visits each indirect node once, so a cyclic or shared tree ends.
fn retarget_tree(doc: &mut Document, parts: &Parts, slot: &Object) -> Result<Option<Dict>, OrganizeError> {
    let mut stack = Vec::new();
    let inline = match slot {
        Object::Ref(r) => {
            stack.push(*r);
            None
        }
        Object::Dict(d) => {
            let mut node = d.clone();
            retarget_node(doc, parts, &mut node, &mut stack)?.then_some(node)
        }
        _ => None,
    };
    let mut seen = HashSet::new();
    while let Some(r) = stack.pop() {
        if !seen.insert(r) {
            continue;
        }
        let Some(mut node) = doc.get(r).as_dict().cloned() else { continue };
        if retarget_node(doc, parts, &mut node, &mut stack)? {
            doc.update_dict(r, |d| *d = node)?;
        }
    }
    Ok(inline)
}

/// Retarget the values of one name tree node's `/Names` pairs and queue its `/Kids`. True when
/// `node` itself changed.
fn retarget_node(doc: &mut Document, parts: &Parts, node: &mut Dict, stack: &mut Vec<ObjRef>) -> Result<bool, OrganizeError> {
    if let Some(kids) = node.get(b"Kids").map(|k| doc.resolve(k)) {
        stack.extend(kids.as_array().into_iter().flatten().filter_map(Object::as_ref));
    }
    let Some(slot) = node.get(b"Names").cloned() else { return Ok(false) };
    let Some(mut pairs) = doc.resolve(&slot).as_array().cloned() else { return Ok(false) };
    let mut changed = false;
    for value in pairs.iter_mut().skip(1).step_by(2) {
        if let Some(new) = retarget_dest(doc, parts, value, 0)? {
            *value = new;
            changed = true;
        }
    }
    if !changed {
        return Ok(false);
    }
    match slot {
        Object::Ref(r) => {
            doc.set(r, Object::Array(pairs));
            Ok(false)
        }
        _ => {
            node.set(b"Names".to_vec(), Object::Array(pairs));
            Ok(true)
        }
    }
}
