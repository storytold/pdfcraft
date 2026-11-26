use super::copy::{Copier, WIDGET_KEYS};
use super::{Dict, Document, HashMap, HashSet, ObjRef, Object, OrganizeError, walk};
use pdfcraft_cos::PdfString;

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
    let mut reserved = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for &root in roots {
        collect_names(doc, root, "", &mut seen, 0, &mut reserved);
    }
    let mut planned = Vec::new();
    let mut walk = RenameWalk { reserved, ..RenameWalk::default() };
    for &root in roots {
        plan_renames_under(doc, root, "", taken, &mut walk, 0, &mut planned);
    }
    planned
}

/// What one [`plan_renames`] call remembers: the nodes it visited, the incoming tree's own
/// names (so an earlier rename does not steal a later sibling's name), and for each colliding
/// name the suffix to try next.
#[derive(Default)]
struct RenameWalk {
    seen: HashSet<ObjRef>,
    reserved: HashSet<String>,
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
            let mut candidate;
            while {
                #[cfg(test)]
                crate::work(1);
                variant = format!("{name}-{n}");
                candidate = qualified(prefix, &variant);
                taken.contains(&candidate) || walk.reserved.contains(&candidate)
            } {
                n = n.saturating_add(1);
            }
            walk.next.insert(fqn.clone(), n.saturating_add(1));
            planned.push((node, variant.clone(), fqn, candidate.clone()));
            taken.insert(candidate);
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
            if w.get(b"Subtype").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) != Some(b"Widget") {
                continue;
            }
            // Walk up to the field the widget belongs to, however deep it sits; a widget that
            // is its own field is its own root.
            let Some(mut root) = a.as_ref() else { continue };
            let mut up = 0;
            let mut visited = HashSet::from([root]);
            while let Some(parent) = doc.get(root).as_dict().and_then(|p| p.reference(b"Parent")) {
                if up >= 32 || !visited.insert(parent) || doc.get(parent).as_dict().is_none() {
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

/// Copy the source AcroForm's `/DR /Font` entries that the imported fields' `/DA` strings name
/// into the destination's `/DR`, renaming on a collision, and rewrite the copied `/DA` strings to
/// match, so a value-only edit of an imported field keeps the font its source form gave it.
/// Without this, an imported field whose `/DA` names a font the destination form does not have
/// (or has under a different face) regenerates its appearance in the fallback Helvetica.
pub(super) fn import_dr_fonts(dst: &mut Document, src: &Document, copier: &mut Copier, fields: &[ObjRef]) -> Result<(), OrganizeError> {
    // A field without its own `/DA` would otherwise fall back to the destination AcroForm's
    // default (which selects another face): give it the source form's default first.
    materialize_default_da(dst, src, fields)?;
    let wanted = da_fonts(dst, fields);
    if wanted.is_empty() {
        return Ok(());
    }
    let Some(src_fonts) = dr_fonts(src) else { return Ok(()) };
    let (font_ref, mut fonts) = match dr_font_entry(dst) {
        Some(Object::Ref(r)) => (Some(r), dst.get(r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    // Every name the `/DA`s select is reserved up front, so a replacement handed to one alias
    // can never collide with another alias's kept name, whichever field comes first.
    let mut taken: Vec<Vec<u8>> = fonts.iter().map(|(k, _)| k.clone()).collect();
    taken.extend(wanted.iter().map(|n| n.as_bytes().to_vec()));
    // Names existing destination fields resolve their edits through even though `/DR` lacks
    // them: taking such a name for an incoming font would silently change those fields' face.
    let effective = effective_field_aliases(dst);
    // They are reserved for replacement allocation too: a fresh `name-N` variant must not land
    // on an alias an existing field falls back to.
    taken.extend(effective.iter().cloned());
    let mut map: HashMap<String, String> = HashMap::new();
    let mut pending: Vec<(String, Object)> = Vec::new();
    for name in wanted {
        let Some(entry) = src_fonts.get(name.as_bytes()) else { continue };
        let font_obj = src.resolve(entry).clone();
        let Some(src_font) = font_obj.as_dict() else { continue };
        let src_face = face_key(src_font);
        match fonts.get(name.as_bytes()).map(|o| dst.resolve(o).as_dict().map(face_key)) {
            // The destination already keeps that name for the same face: reuse it.
            Some(Some(face)) if face == src_face => {
                map.insert(name.clone(), name.clone());
            }
            // The name is taken by a different face: hand the imported font a fresh one.
            Some(_) => {
                let new = fresh_alias(&name, &taken);
                taken.push(new.clone().into_bytes());
                map.insert(name.clone(), new.clone());
                pending.push((new, entry.clone()));
            }
            // The name is free in the `/DR`, but an existing destination field falls back to it
            // (its effective `/DA` selects it, or the reader's built-in `/Helv` applies): give
            // the imported font a fresh one too, or that field's next edit would draw Courier.
            None if effective.contains(name.as_bytes()) => {
                let new = fresh_alias(&name, &taken);
                taken.push(new.clone().into_bytes());
                map.insert(name.clone(), new.clone());
                pending.push((new, entry.clone()));
            }
            // The name is free in the destination (and reserved above, so no replacement can
            // steal it): keep it and copy the font under it.
            None => {
                map.insert(name.clone(), name.clone());
                pending.push((name.clone(), entry.clone()));
            }
        }
    }
    if map.is_empty() {
        return Ok(());
    }
    for (name, entry) in pending {
        let copied = copier.copy_value(dst, &entry, None)?;
        fonts.set(name.as_bytes().to_vec(), copied);
    }
    set_dr_fonts(dst, font_ref, fonts)?;
    rewrite_da(dst, fields, &map)
}

/// A document's AcroForm dictionary, resolved and cloned.
fn acroform_dict(doc: &Document) -> Option<Dict> {
    let catalog = doc.get(doc.root()?).as_dict().cloned()?;
    let form_ref = catalog.get(b"AcroForm")?;
    doc.resolve(form_ref).as_dict().cloned()
}

/// A document's AcroForm-level default appearance string, resolved and cloned.
fn acroform_da(doc: &Document) -> Option<PdfString> {
    acroform_dict(doc)?.get(b"DA").map(|d| doc.resolve(d)).and_then(|d| d.as_string().cloned())
}

/// The first free `name-N` variant in `taken`.
fn fresh_alias(name: &str, taken: &[Vec<u8>]) -> String {
    let mut n = 1;
    loop {
        let candidate = format!("{name}-{n}");
        if !taken.iter().any(|t| t.as_slice() == candidate.as_bytes()) {
            return candidate;
        }
        n += 1;
    }
}

/// The aliases existing destination fields resolve an edit through even though `/DR` may lack
/// them: the name each field's effective `/DA` selects (its own, an inherited one, or the
/// AcroForm default), or the reader's built-in `/Helv` when nothing else applies. A name taken
/// for an incoming font under one of these would silently change those fields' face.
fn effective_field_aliases(dst: &Document) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    let Some(form) = acroform_dict(dst) else { return out };
    let form_da = form.get(b"DA").map(|d| dst.resolve(d)).and_then(|d| d.as_string().cloned());
    let roots = form.get(b"Fields").map(|f| dst.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    let mut stack: Vec<(ObjRef, Option<PdfString>)> = roots.iter().filter_map(Object::as_ref).map(|r| (r, None)).collect();
    let mut seen = HashSet::new();
    while let Some((r, inherited)) = stack.pop() {
        if !seen.insert(r) {
            continue;
        }
        let Some(d) = dst.get(r).as_dict().cloned() else { continue };
        let da = d.get(b"DA").map(|o| dst.resolve(o)).and_then(|o| o.as_string().cloned()).or(inherited);
        match &da {
            Some(da) => {
                for name in da_font_names(da.to_text().as_bytes()) {
                    out.insert(name.into_bytes());
                }
            }
            None => {
                if let Some(form_da) = &form_da {
                    for name in da_font_names(form_da.to_text().as_bytes()) {
                        out.insert(name.into_bytes());
                    }
                } else {
                    out.insert(b"Helv".to_vec());
                }
            }
        }
        if let Some(kids) = d.get(b"Kids").map(|k| dst.resolve(k)).and_then(|k| k.as_array().cloned()) {
            let kids: Vec<ObjRef> = kids.iter().filter_map(Object::as_ref).collect();
            stack.extend(kids.into_iter().map(|k| (k, da.clone())));
        }
    }
    out
}

/// A document's AcroForm `/DR /Font` dictionary, resolved and cloned.
fn dr_fonts(doc: &Document) -> Option<Dict> {
    let catalog = doc.get(doc.root()?).as_dict().cloned()?;
    let form_ref = catalog.get(b"AcroForm")?;
    let form = doc.resolve(form_ref).as_dict()?.clone();
    let dr_ref = form.get(b"DR")?;
    let dr = doc.resolve(dr_ref).as_dict()?.clone();
    doc.resolve(dr.get(b"Font")?).as_dict().cloned()
}

/// Give every imported field root that lacks its own `/DA` the source AcroForm's default
/// appearance: readers fall back to the *destination* AcroForm's default, which selects another
/// face. The destination's own document-wide default is left alone.
fn materialize_default_da(dst: &mut Document, src: &Document, fields: &[ObjRef]) -> Result<(), OrganizeError> {
    let Some(da) = acroform_da(src) else { return Ok(()) };
    for f in fields {
        if dst.get(*f).as_dict().is_some_and(|d| !d.contains(b"DA")) {
            dst.update_dict(*f, |d| d.set(b"DA".to_vec(), Object::String(da.clone())))?;
        }
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
mod template_tests {
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

/// The `/DR /Font` object of the destination, in whatever form it is stored.
fn dr_font_entry(dst: &Document) -> Option<Object> {
    let catalog = dst.get(dst.root()?).as_dict().cloned()?;
    let form_ref = catalog.get(b"AcroForm")?;
    let form = dst.resolve(form_ref).as_dict()?.clone();
    let dr_ref = form.get(b"DR")?;
    let dr = dst.resolve(dr_ref).as_dict()?.clone();
    dr.get(b"Font").cloned()
}

/// Write the destination AcroForm's `/DR /Font` back as `fonts`, keeping dictionaries indirect
/// where they already were (mirroring [`register_fields`]).
fn set_dr_fonts(dst: &mut Document, font_ref: Option<ObjRef>, fonts: Dict) -> Result<(), OrganizeError> {
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let mut catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (form_ref, mut form) = match catalog.get(b"AcroForm") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let (dr_ref, mut dr) = match form.get(b"DR") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let font = match font_ref {
        Some(r) => {
            dst.set(r, Object::Dict(fonts));
            Object::Ref(r)
        }
        None => Object::Dict(fonts),
    };
    dr.set(b"Font".to_vec(), font);
    let dr = match dr_ref {
        Some(r) => {
            dst.set(r, Object::Dict(dr));
            Object::Ref(r)
        }
        None => Object::Dict(dr),
    };
    form.set(b"DR".to_vec(), dr);
    match form_ref {
        Some(r) => dst.set(r, Object::Dict(form)),
        None => catalog.set(b"AcroForm".to_vec(), Object::Dict(form)),
    }
    if form_ref.is_none() {
        dst.set(root, Object::Dict(catalog));
    }
    Ok(())
}

/// The face a font dictionary paints with: enough to tell Helvetica from Courier (the case at
/// hand) though not two embedded subsets of one face.
fn face_key(d: &Dict) -> Vec<Vec<u8>> {
    [&b"Subtype"[..], &b"BaseFont"[..], &b"Encoding"[..]].iter().map(|k| d.name(k).unwrap_or_default().to_vec()).collect()
}

/// The font names every `/DA` of the copied field trees selects, in order, deduplicated.
/// Indirect `/DA` strings are resolved.
fn da_fonts(dst: &Document, fields: &[ObjRef]) -> Vec<String> {
    let mut dicts = Vec::new();
    let mut seen = HashSet::new();
    collect_field_dicts(dst, fields, 0, &mut seen, &mut dicts);
    let mut out: Vec<String> = Vec::new();
    for (_, d) in dicts {
        let da = d.get(b"DA").map(|o| dst.resolve(o).as_ref().clone());
        if let Some(Object::String(s)) = da {
            for name in da_font_names(&s.bytes) {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out
}

/// Every field/widget dictionary of the trees rooted at `fields`, cycle- and depth-safe.
fn collect_field_dicts(dst: &Document, fields: &[ObjRef], depth: u8, seen: &mut HashSet<ObjRef>, out: &mut Vec<(ObjRef, Dict)>) {
    // The field reader walks trees to depth 64 (forms::read); match it so every copied field is
    // scanned. Cycles are cut by the seen set.
    if depth > 64 {
        return;
    }
    for r in fields {
        if !seen.insert(*r) {
            continue;
        }
        let Some(d) = dst.get(*r).as_dict().cloned() else { continue };
        let kids = d.get(b"Kids").map(|k| dst.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
        out.push((*r, d));
        let kids: Vec<ObjRef> = kids.iter().filter_map(Object::as_ref).collect();
        collect_field_dicts(dst, &kids, depth + 1, seen, out);
    }
}

/// The font names a `/DA` string selects (`/F1 18 Tf`, `/F1 Tf`), in order. PDF `#xx` escapes in
/// the names are decoded, matching the resource keys the parser stores.
fn da_font_names(bytes: &[u8]) -> Vec<String> {
    let tokens = da_tokens(bytes);
    let mut out = Vec::new();
    for i in 0..tokens.len() {
        if bytes[tokens[i].0] == b'/' && selects_font(&tokens[i + 1..], bytes) {
            let name = decode_pdf_name(&bytes[tokens[i].0 + 1..tokens[i].1]);
            out.push(String::from_utf8_lossy(&name).into_owned());
        }
    }
    out
}

/// Whether the tokens after a `/Name` end in a `Tf` operator (a number may sit between).
fn selects_font(rest: &[(usize, usize)], bytes: &[u8]) -> bool {
    match rest.first() {
        Some(t) if &bytes[t.0..t.1] == b"Tf" => true,
        Some(t) if parses_number(&bytes[t.0..t.1]) => rest.get(1).is_some_and(|t| &bytes[t.0..t.1] == b"Tf"),
        _ => false,
    }
}

fn parses_number(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).ok().and_then(|s| s.parse::<f64>().ok()).is_some()
}

/// Whitespace-delimited token spans of a `/DA` string.
fn da_tokens(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, b) in bytes.iter().chain(std::iter::once(&b' ')).enumerate() {
        if b.is_ascii_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else {
            start.get_or_insert(i);
        }
    }
    out
}

/// Rewrite the copied `/DA` strings so every selected font name is the destination one.
/// Indirect `/DA` strings are resolved; a rewritten one is written back as a direct string on
/// the field rather than onto the shared object, so other fields referencing it keep theirs.
fn rewrite_da(dst: &mut Document, fields: &[ObjRef], map: &HashMap<String, String>) -> Result<(), OrganizeError> {
    let mut dicts = Vec::new();
    let mut seen = HashSet::new();
    collect_field_dicts(dst, fields, 0, &mut seen, &mut dicts);
    for (r, d) in dicts {
        let da = d.get(b"DA").map(|o| dst.resolve(o).as_ref().clone());
        let Some(Object::String(s)) = da else { continue };
        let Some(new) = rewrite_da_bytes(&s.bytes, map) else { continue };
        dst.update_dict(r, |f| f.set(b"DA".to_vec(), Object::String(PdfString { bytes: new, hex: s.hex })))?;
    }
    Ok(())
}

/// The `/DA` bytes with every colliding font name replaced, or `None` when nothing changes.
/// Selected names are matched after decoding PDF `#xx` escapes, and a name is rewritten whenever
/// its written form differs from the canonical one — an escaped alias is normalized even when it
/// maps to itself, because the appearance parser does not decode escapes. Replacements run back
/// to front so earlier spans stay valid.
fn rewrite_da_bytes(bytes: &[u8], map: &HashMap<String, String>) -> Option<Vec<u8>> {
    let tokens = da_tokens(bytes);
    let mut edits = Vec::new();
    for i in 0..tokens.len() {
        if bytes[tokens[i].0] == b'/' && selects_font(&tokens[i + 1..], bytes) {
            let raw = &bytes[tokens[i].0 + 1..tokens[i].1];
            let name = String::from_utf8_lossy(&decode_pdf_name(raw)).into_owned();
            if let Some(new) = map.get(&name) {
                let encoded = encode_pdf_name(new);
                if encoded != raw {
                    edits.push((tokens[i].0 + 1, tokens[i].1, encoded));
                }
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = bytes.to_vec();
    for (start, end, name) in edits.into_iter().rev() {
        out.splice(start..end, name);
    }
    Some(out)
}

/// A PDF name's bytes with its `#xx` escape sequences decoded; malformed escapes pass through.
fn decode_pdf_name(bytes: &[u8]) -> Vec<u8> {
    if !bytes.contains(&b'#') {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi << 4 | lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A name's bytes with every character a PDF name cannot carry written as a `#xx` escape.
fn encode_pdf_name(name: &str) -> Vec<u8> {
    const DELIMITERS: &[u8] = b"()<>[]{}/%";
    let mut out = Vec::with_capacity(name.len());
    for &b in name.as_bytes() {
        if b.is_ascii_whitespace() || b == b'#' || DELIMITERS.contains(&b) || !b.is_ascii() {
            out.extend(format!("#{b:02X}").into_bytes());
        } else {
            out.push(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn build(bodies: &[&str]) -> Document {
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

    #[test]
    fn renaming_colliding_field_does_not_steal_later_sibling_name() {
        let dst = build(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (item) /Rect [0 0 10 10] >>",
        ]);
        let src = build(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (item) /Rect [0 0 10 10] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (item-1) /Rect [0 20 10 30] >>",
        ]);
        // Only "item" collides with dst; "item-1" in src is already unique and must not be stolen.
        assert_eq!(field_name_conflicts(&dst, &src), [("item".to_string(), "item-2".to_string())]);
    }

    #[test]
    fn field_roots_keeps_widget_with_dangling_parent_or_indirect_subtype() {
        let dst = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype 5 0 R /FT /Tx /T (orphan) /Parent 99 0 R /Rect [0 0 10 10] >>",
            "/Widget",
        ]);
        let src = build(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (orphan) /Rect [0 0 10 10] >>",
        ]);
        assert_eq!(field_name_conflicts(&dst, &src), [("orphan".to_string(), "orphan-1".to_string())]);
    }
}
