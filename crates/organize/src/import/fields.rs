use super::copy::WIDGET_KEYS;
use super::{Dict, Document, HashMap, HashSet, ObjRef, Object, OrganizeError, walk};

/// Rename the top-level fields about to be registered in `dst` whose fully-qualified names
/// (`parent.T.child`) are already taken, so each imported field stays its own field instead of
/// blurring into the destination's field of the same name (fields of one fully-qualified name
/// are one field). A colliding node's own `/T` becomes the first free `Name-1`, `Name-2`…
/// variant, so everything below it keeps its place in the tree. Returns what was renamed
/// (old fully-qualified name → new one), in order.
pub(super) fn resolve_field_conflicts(dst: &mut Document, fields: &[ObjRef]) -> Result<Vec<(String, String)>, OrganizeError> {
    let mut taken = taken_field_names(dst)?;
    let planned = plan_renames(dst, fields, &mut taken);
    let mut renamed = Vec::new();
    for (node, new_t, old, new) in planned {
        dst.update_dict(node, |d| d.set(b"T".to_vec(), Object::String(pdfcraft_cos::PdfString::text(&new_t))))?;
        renamed.push((old, new));
    }
    Ok(renamed)
}

/// What importing `src` into `dst` would rename, computed without changing either document:
/// each fully-qualified name of `src`'s fields that `dst` already uses (or that an earlier
/// field of `src` itself uses) is renamed to the first free variant, exactly as the import
/// would. Unreadable structure previews as nothing: the import itself will say why it failed.
pub fn field_name_conflicts(dst: &Document, src: &Document) -> Vec<(String, String)> {
    let Ok(mut taken) = taken_field_names(dst) else { return Vec::new() };
    let Ok(roots) = field_roots(src) else { return Vec::new() };
    plan_renames(src, &roots, &mut taken).into_iter().map(|(_, _, old, new)| (old, new)).collect()
}

/// What combining `sources`, in order, would rename in each one: the whole list is simulated
/// with the same first-free-variant rule the combine uses, so a later file's preview sees the
/// earlier files' renames. Unreadable structure previews as nothing (the combine itself will
/// say why it fails); non-colliding names still count as taken for the files after them.
pub fn combine_field_conflicts(sources: &[&Document]) -> Vec<Vec<(String, String)>> {
    let mut taken = std::collections::HashSet::new();
    sources
        .iter()
        .map(|src| match field_roots(src) {
            Ok(roots) => plan_renames(src, &roots, &mut taken).into_iter().map(|(_, _, old, new)| (old, new)).collect(),
            Err(_) => Vec::new(),
        })
        .collect()
}

/// Walk field trees `roots` in `doc` and plan the renames that would keep every name separate
/// from `taken`: each node's fully-qualified name joins `taken` (so a later field of the same
/// import collides with an earlier one), and a node already taken is renamed to the first free
/// variant of its own `/T`. Names are compared and renamed as decoded text, not as stored
/// bytes, so one name written in two encodings (PDFDocEncoding, UTF-16BE) is one name.
/// Read-only: returns (node, new `/T`, old name, new name). Cycle- and depth-safe.
fn plan_renames(doc: &Document, roots: &[ObjRef], taken: &mut std::collections::HashSet<String>) -> Vec<(ObjRef, String, String, String)> {
    let mut planned = Vec::new();
    let mut walk = RenameWalk::default();
    for &root in roots {
        plan_renames_under(doc, root, "", taken, &mut walk, 0, &mut planned);
    }
    planned
}

/// What one [`plan_renames`] call remembers: the nodes it visited, and for each colliding name
/// the suffix to try next.
#[derive(Default)]
struct RenameWalk {
    seen: HashSet<ObjRef>,
    next: HashMap<String, u32>,
}

/// One node of [`plan_renames`].
fn plan_renames_under(
    doc: &Document,
    node: ObjRef,
    prefix: &str,
    taken: &mut std::collections::HashSet<String>,
    walk: &mut RenameWalk,
    depth: u8,
    planned: &mut Vec<(ObjRef, String, String, String)>,
) {
    if depth > 32 || !walk.seen.insert(node) {
        return;
    }
    let Some(d) = doc.get(node).as_dict().cloned() else { return };
    let mut name = d.get(b"T").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
    if !name.is_empty() {
        let fqn = qualified(prefix, &name);
        if taken.contains(&fqn) {
            // `taken` only grows, so every suffix below the last one handed out for this name is
            // still taken: resume there instead of at 1.
            let mut n = walk.next.get(&fqn).copied().unwrap_or(1);
            let mut variant;
            while {
                #[cfg(test)]
                crate::work(1);
                variant = format!("{name}-{n}");
                taken.contains(&qualified(prefix, &variant))
            } {
                n = n.saturating_add(1);
            }
            walk.next.insert(fqn.clone(), n.saturating_add(1));
            let new_fqn = qualified(prefix, &variant);
            planned.push((node, variant.clone(), fqn, new_fqn.clone()));
            taken.insert(new_fqn);
            name = variant;
        } else {
            taken.insert(fqn);
        }
    }
    let prefix = qualified_optional(prefix, &name);
    let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for kid in kids.iter().filter_map(|k| k.as_ref()) {
        plan_renames_under(doc, kid, &prefix, taken, walk, depth + 1, planned);
    }
}

/// The field names in use in `dst`: every name in its form, and every name hanging only off a
/// page's widgets — a field the form never lists still owns its name.
fn taken_field_names(dst: &Document) -> Result<std::collections::HashSet<String>, OrganizeError> {
    let mut taken = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for root in field_roots(dst)? {
        collect_names(dst, root, "", &mut seen, 0, &mut taken);
    }
    Ok(taken)
}

/// A document's field trees, wherever they are rooted: the `/AcroForm /Fields` entries, and
/// the fields behind widgets on pages (climbed to their root, for forms that never registered
/// them).
fn field_roots(doc: &Document) -> Result<Vec<ObjRef>, OrganizeError> {
    let mut roots: Vec<ObjRef> = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"AcroForm").map(|f| doc.resolve(f)))
        .and_then(|f| f.as_dict().cloned())
        .and_then(|f| f.get(b"Fields").map(|l| doc.resolve(l)))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| f.as_ref())
        .collect();
    for (page, _) in walk(doc)? {
        let Some(d) = doc.get(page).as_dict().cloned() else { continue };
        let annots = d.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in annots.iter() {
            let Some(w) = doc.resolve(a).as_dict().cloned() else { continue };
            if w.name(b"Subtype") != Some(b"Widget") {
                continue;
            }
            // Walk up to the field the widget belongs to, however deep it sits; a widget that
            // is its own field is its own root.
            let Some(mut root) = a.as_ref() else { continue };
            let mut up = 0;
            while let Some(parent) = doc.get(root).as_dict().and_then(|p| p.reference(b"Parent")) {
                if up >= 32 || root == parent {
                    break;
                }
                root = parent;
                up += 1;
            }
            roots.push(root);
        }
    }
    Ok(roots)
}

/// Record the fully-qualified names of one field tree. Cycle- and depth-safe.
fn collect_names(
    dst: &Document,
    node: ObjRef,
    prefix: &str,
    seen: &mut std::collections::HashSet<ObjRef>,
    depth: u8,
    out: &mut std::collections::HashSet<String>,
) {
    if depth > 32 || !seen.insert(node) {
        return;
    }
    let Some(d) = dst.get(node).as_dict().cloned() else { return };
    let name = d.get(b"T").and_then(|t| dst.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
    let prefix = qualified_optional(prefix, &name);
    if !name.is_empty() {
        out.insert(prefix.clone());
    }
    let kids = d.get(b"Kids").map(|k| dst.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for kid in kids.iter().filter_map(|k| k.as_ref()) {
        collect_names(dst, kid, &prefix, seen, depth + 1, out);
    }
}

/// `prefix.name`, or `name` at the top level.
fn qualified(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") }
}

/// The prefix a node's children hang under: its own name, when it has one.
fn qualified_optional(prefix: &str, name: &str) -> String {
    if name.is_empty() { prefix.to_string() } else { qualified(prefix, name) }
}

/// Add `kid` to `field`'s `/Kids`, in the array object itself when `/Kids` is one, so the field
/// keeps its other widgets.
pub(super) fn add_kid(dst: &mut Document, field: ObjRef, kid: ObjRef) -> Result<(), OrganizeError> {
    match dst.get(field).as_dict().and_then(|f| f.get(b"Kids").cloned()) {
        Some(Object::Ref(list)) => {
            let mut kids = dst.get(list).as_array().cloned().unwrap_or_default();
            kids.push(Object::Ref(kid));
            dst.set(list, Object::Array(kids));
        }
        _ => dst.update_dict(field, |f| {
            let mut kids = f.get(b"Kids").and_then(|k| k.as_array()).cloned().unwrap_or_default();
            kids.push(Object::Ref(kid));
            f.set(b"Kids".to_vec(), Object::Array(kids));
        })?,
    }
    Ok(())
}

/// Take `kid` out of field `field`'s `/Kids` (direct or held in an array object of its own).
fn remove_kid(dst: &mut Document, field: ObjRef, kid: ObjRef) -> Result<(), OrganizeError> {
    match dst.get(field).as_dict().and_then(|f| f.get(b"Kids").cloned()) {
        // Only an array is rewritten: `/Kids` pointing at anything else (a font, in a hostile
        // file) is left as it is rather than overwritten.
        Some(Object::Ref(list)) => {
            if let Some(mut kids) = dst.get(list).as_array().cloned() {
                kids.retain(|k| k.as_ref() != Some(kid));
                dst.set(list, Object::Array(kids));
            }
        }
        Some(Object::Array(mut kids)) => {
            kids.retain(|k| k.as_ref() != Some(kid));
            dst.update_dict(field, |f| f.set(b"Kids".to_vec(), Object::Array(kids)))?;
        }
        _ => {}
    }
    Ok(())
}

/// PdfCraft patch (#1810): split a widget that doubles as its own field into a parent field
/// holding the field keys (name, value, flags…) and a widget child holding the annotation keys
/// (rectangle, appearance, page). The parent takes the widget's place among the top-level fields;
/// later copies of the page add widget children of it, so every page has its own widget and the
/// field stays one.
pub(super) fn split_own_field(dst: &mut Document, fields: &mut [ObjRef], widget: ObjRef) {
    let Some(d) = dst.get(widget).as_dict().cloned() else { return };
    let (mut field, mut child) = field_and_widget(dst, &d);
    let parent = dst.add(Object::Null);
    field.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(widget)]));
    child.set(b"Parent".to_vec(), Object::Ref(parent));
    dst.set(parent, Object::Dict(field));
    dst.set(widget, Object::Dict(child));
    if let Some(entry) = fields.iter_mut().find(|f| **f == widget) {
        *entry = parent;
    }
}

/// A combined field/widget has two additional-action namespaces (ISO 32000-1 tables 194/196).
/// Partition them as well as the ordinary entries, including when copying a later widget.
pub(super) fn field_and_widget(doc: &Document, d: &Dict) -> (Dict, Dict) {
    let (mut field, mut child) = (Dict::new(), Dict::new());
    for (k, v) in d.iter() {
        if k.as_slice() == b"AA"
            && let Some(actions) = doc.resolve(v).as_dict().cloned()
        {
            let (mut field_actions, mut widget_actions) = (Dict::new(), Dict::new());
            for (trigger, action) in actions.iter() {
                if [b"E".as_slice(), b"X", b"D", b"U", b"Fo", b"Bl", b"PO", b"PC", b"PV", b"PI"].contains(&trigger.as_slice()) {
                    widget_actions.set(trigger.clone(), action.clone());
                } else {
                    field_actions.set(trigger.clone(), action.clone());
                }
            }
            if field_actions.iter().next().is_some() {
                field.set(k.clone(), Object::Dict(field_actions));
            }
            if widget_actions.iter().next().is_some() {
                child.set(k.clone(), Object::Dict(widget_actions));
            }
        } else if WIDGET_KEYS.contains(&k.as_slice()) {
            child.set(k.clone(), v.clone());
        } else {
            field.set(k.clone(), v.clone());
        }
    }
    (field, child)
}

/// Add top-level fields to the destination's `/AcroForm /Fields` (creating the form if needed).
pub(super) fn register_fields(dst: &mut Document, fields: &[ObjRef]) -> Result<(), OrganizeError> {
    if fields.is_empty() {
        return Ok(());
    }
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (form_ref, mut form) = match catalog.get(b"AcroForm") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let mut list = form.get(b"Fields").map(|f| dst.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    let mut listed: HashSet<ObjRef> = list.iter().filter_map(Object::as_ref).collect();
    for f in fields {
        #[cfg(test)]
        crate::work(1);
        if listed.insert(*f) {
            list.push(Object::Ref(*f));
        }
    }
    form.set(b"Fields".to_vec(), Object::Array(list));
    match form_ref {
        Some(r) => dst.set(r, Object::Dict(form)),
        None => {
            let r = dst.add(form);
            dst.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(r)))?;
        }
    }
    Ok(())
}

/// At most this many field nodes are read when matching copied widgets to their originals.
const MAX_FIELD_NODES: usize = 100_000;

/// Make the widgets on `page`, copied from a page of the same document, more widgets of the
/// fields they were copied from, so that each field stays one field, with one value shown on
/// every page (#1429: a page template spawned without renaming). A copied field is matched to
/// the field of the same fully-qualified name under the top-level fields the form listed before
/// the copy (`before`); the copies of the matched fields leave the form.
pub(crate) fn share_fields_with_originals(dst: &mut Document, page: ObjRef, before: &HashSet<ObjRef>) -> Result<(), OrganizeError> {
    let catalog = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let form_ref = dst.get(catalog).as_dict().and_then(|c| c.reference(b"AcroForm"));
    let form = dst.get(catalog).as_dict().and_then(|c| c.get(b"AcroForm").cloned()).map(|f| dst.resolve(&f).as_ref().clone());
    let mut list: Vec<Object> = form
        .as_ref()
        .and_then(Object::as_dict)
        .and_then(|f| f.get(b"Fields"))
        .map(|l| dst.resolve(l).as_ref().clone())
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default();
    // The fields that were there before the copy, by fully-qualified name.
    let mut originals: HashMap<String, ObjRef> = HashMap::new();
    let mut seen = HashSet::new();
    let mut pending: Vec<(ObjRef, String, u8)> =
        list.iter().filter_map(Object::as_ref).filter(|r| before.contains(r)).map(|r| (r, String::new(), 0)).collect();
    while let Some((node, prefix, depth)) = pending.pop() {
        if depth > 32 || seen.len() >= MAX_FIELD_NODES || !seen.insert(node) {
            continue;
        }
        let Some(d) = dst.get(node).as_dict().cloned() else { continue };
        let name = match d.get(b"T").and_then(|t| dst.resolve(t).as_string().map(|s| s.to_text())) {
            Some(t) => qualified_optional(&prefix, &t),
            None => prefix.clone(),
        };
        if d.contains(b"T") {
            originals.entry(name.clone()).or_insert(node);
        }
        for kid in d.get(b"Kids").map(|k| dst.resolve(k).as_ref().clone()).and_then(|k| k.as_array().cloned()).unwrap_or_default() {
            if let Some(kid) = kid.as_ref() {
                pending.push((kid, name.clone(), depth.saturating_add(1)));
            }
        }
    }
    let annots = dst.get(page).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| dst.resolve(&a).as_ref().clone());
    let widgets: Vec<ObjRef> = annots
        .as_ref()
        .and_then(Object::as_array)
        .into_iter()
        .flatten()
        .filter_map(Object::as_ref)
        .filter(|w| dst.get(*w).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")))
        .collect();
    let (mut shared, mut kept): (HashSet<ObjRef>, HashSet<ObjRef>) = (HashSet::new(), HashSet::new());
    for widget in widgets {
        let Some(d) = dst.get(widget).as_dict().cloned() else { continue };
        // The copied field holding the widget: the widget itself when it is its own field.
        let field = if d.contains(b"T") { Some(widget) } else { d.reference(b"Parent") };
        let Some((name, top)) = field.and_then(|f| name_and_top(dst, f)) else { continue };
        let Some(&original) = originals.get(&name) else {
            kept.insert(top);
            continue;
        };
        // An original that is its own widget becomes a field with that widget as its child.
        let original = if dst.get(original).as_dict().is_some_and(|o| o.name(b"Subtype") == Some(b"Widget")) {
            let mut slot = [original];
            split_own_field(dst, &mut slot, original);
            let [parent] = slot;
            replace_kid(dst, &mut list, original, parent)?;
            originals.insert(name, parent);
            parent
        } else {
            original
        };
        // The copied field it leaves no longer lists it, so no widget has two parents.
        if let Some(former) = d.reference(b"Parent") {
            remove_kid(dst, former, widget)?;
        }
        let (_, mut child) = if d.contains(b"T") { field_and_widget(dst, &d) } else { (Dict::new(), d) };
        child.set(b"Parent".to_vec(), Object::Ref(original));
        dst.set(widget, Object::Dict(child));
        add_kid(dst, original, widget)?;
        shared.insert(top);
    }
    list.retain(|f| f.as_ref().is_none_or(|r| !shared.contains(&r) || kept.contains(&r)));
    match (form_ref, form) {
        (Some(r), _) => dst.update_dict(r, |f| f.set(b"Fields".to_vec(), Object::Array(list)))?,
        (None, Some(_)) => dst.update_dict(catalog, |c| {
            if let Some(Object::Dict(f)) = c.get(b"AcroForm").cloned() {
                let mut f = f;
                f.set(b"Fields".to_vec(), Object::Array(list));
                c.set(b"AcroForm".to_vec(), Object::Dict(f));
            }
        })?,
        (None, None) => {}
    }
    Ok(())
}

/// The fully-qualified name of field `node` and its top-level field, climbing `/Parent` at most
/// 32 levels and never through a node twice.
fn name_and_top(doc: &Document, node: ObjRef) -> Option<(String, ObjRef)> {
    let mut parts = Vec::new();
    let mut seen = HashSet::new();
    let mut at = node;
    for _ in 0..32 {
        if !seen.insert(at) {
            return None;
        }
        let d = doc.get(at).as_dict()?.clone();
        if let Some(t) = d.get(b"T").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())) {
            parts.push(t);
        }
        match d.reference(b"Parent") {
            Some(parent) => at = parent,
            None => {
                parts.reverse();
                return Some((parts.join("."), at));
            }
        }
    }
    None
}

/// Put `new` where `old` was: among its parent field's `/Kids`, or among the form's top-level
/// `fields` when it has no parent.
fn replace_kid(dst: &mut Document, fields: &mut [Object], old: ObjRef, new: ObjRef) -> Result<(), OrganizeError> {
    let swap = |kids: &mut Vec<Object>| {
        for k in kids.iter_mut() {
            if k.as_ref() == Some(old) {
                *k = Object::Ref(new);
            }
        }
    };
    match dst.get(new).as_dict().and_then(|d| d.reference(b"Parent")) {
        Some(parent) => match dst.get(parent).as_dict().and_then(|p| p.get(b"Kids").cloned()) {
            Some(Object::Ref(list)) => {
                let mut kids = dst.get(list).as_array().cloned().unwrap_or_default();
                swap(&mut kids);
                dst.set(list, Object::Array(kids));
            }
            _ => dst.update_dict(parent, |p| {
                let mut kids = p.get(b"Kids").and_then(|k| k.as_array()).cloned().unwrap_or_default();
                swap(&mut kids);
                p.set(b"Kids".to_vec(), Object::Array(kids));
            })?,
        },
        None => {
            for f in fields.iter_mut() {
                if f.as_ref() == Some(old) {
                    *f = Object::Ref(new);
                }
            }
        }
    }
    Ok(())
}

/// The top-level fields `doc`'s form lists now, before an import adds its own.
pub(crate) fn listed_fields(doc: &Document) -> HashSet<ObjRef> {
    doc.root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"AcroForm").map(|f| doc.resolve(f).as_ref().clone()))
        .and_then(|f| f.as_dict().and_then(|f| f.get(b"Fields")).map(|l| doc.resolve(l).as_ref().clone()))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Object::as_ref)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn doc(bodies: &[&str]) -> Document {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, b) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        Document::open(Arc::new(out)).expect("opens")
    }

    /// `/Kids` held in an array object of its own loses the kid; `/Kids` pointing at something
    /// that isn't an array (here a font) is never overwritten.
    #[test]
    fn removing_a_kid_rewrites_only_an_array() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [] /Count 0 >>",
            "<< /T (G) /Kids 4 0 R >>",
            "[6 0 R 7 0 R]",
            "<< /T (H) /Kids 8 0 R >>",
            "<< /T (a) /Parent 3 0 R >>",
            "<< /T (b) /Parent 3 0 R >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ]);
        remove_kid(&mut d, ObjRef::new(3, 0), ObjRef::new(6, 0)).unwrap();
        assert_eq!(d.get(ObjRef::new(4, 0)).as_array().cloned(), Some(vec![Object::Ref(ObjRef::new(7, 0))]));
        remove_kid(&mut d, ObjRef::new(5, 0), ObjRef::new(6, 0)).unwrap();
        assert_eq!(d.get(ObjRef::new(8, 0)).as_dict().and_then(|f| f.name(b"Type")), Some(&b"Font"[..]), "the font is untouched");
    }
}
