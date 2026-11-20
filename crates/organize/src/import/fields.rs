use super::copy::WIDGET_KEYS;
use super::{Copier, Dict, Document, HashMap, HashSet, ObjRef, Object, OrganizeError, walk};

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

/// PdfCraft patch (#7279): bring the fonts the imported fields' default appearances (`/DA`) name
/// from the source form's `/DR` into the destination's, so editing a value redraws it in the
/// field's own font rather than in whatever the destination calls by that name (or Helvetica).
/// A name the destination already gives to another font is renamed to the first free `Name_1`,
/// `Name_2`… and the `/DA` follows. A top-level field that took its `/DA` from the source form
/// gets it written on itself, since the destination form's may differ.
///
/// Every font is copied before anything is written, and the call comes before
/// [`register_fields`]: a source font graph too deep to copy then refuses the import while the
/// destination form is still as it was, so [`import_pages_with`](super::import_pages_with) has
/// only the objects the import added to free.
///
/// Returns the objects that already existed and now hold a reference to a copied font (the form,
/// its `/DR`, its `/Font` map): deduplication must rewrite references in these too, or a copy
/// merged into an equal font already there would leave them dangling.
pub(super) fn carry_form_fonts(dst: &mut Document, copier: &mut Copier<'_>, roots: &[ObjRef]) -> Result<Vec<ObjRef>, OrganizeError> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let src = copier.src;
    let src_form = form_of(src);
    let src_fonts = src_form.as_ref().and_then(|f| fonts_of(src, f)).unwrap_or_default();
    let src_da = src_form.as_ref().and_then(|f| da_of(src, f));
    let dst_da = form_of(dst).and_then(|f| da_of(dst, &f));
    let mut fonts = form_of(dst).and_then(|f| fonts_of(dst, &f)).unwrap_or_default();
    let mut fonts_changed = false;
    // Source font name → its name in the destination (`None`: the source form has no such font).
    let mut names: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    let is_root: HashSet<ObjRef> = roots.iter().copied().collect();
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    for &root in roots {
        field_nodes(dst, root, 0, &mut seen, &mut nodes);
    }
    // Read and copy first, write second: the loop below only adds objects, so a refusal while
    // copying leaves the destination's form untouched.
    let mut planned = Vec::new();
    for node in nodes {
        #[cfg(test)]
        crate::work(1);
        let Some(d) = dst.get(node).as_dict().cloned() else { continue };
        let (da, inherited) = match da_of(dst, &d) {
            Some(da) => (da, false),
            None if is_root.contains(&node) => match &src_da {
                Some(da) => (da.clone(), true),
                None => continue,
            },
            None => continue,
        };
        let mut new_da = da.clone();
        if let Some((at, name)) = da_font(&da) {
            let target = match names.get(&name) {
                Some(t) => t.clone(),
                None => {
                    let t = place_font(dst, copier, &src_fonts, &mut fonts, &name, &mut fonts_changed)?;
                    names.insert(name.clone(), t.clone());
                    t
                }
            };
            if let Some(t) = target.filter(|t| *t != name) {
                // The `/DA` is a string: the new name needs the escapes a name in a dictionary
                // would have been parsed with.
                new_da.splice(at, encode_name(&t));
            }
        }
        if new_da != da || (inherited && dst_da.as_ref() != Some(&new_da)) {
            planned.push((node, new_da));
        }
    }
    for (node, new_da) in planned {
        dst.update_dict(node, |f| f.set(b"DA".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(new_da))))?;
    }
    if fonts_changed { set_form_fonts(dst, fonts) } else { Ok(Vec::new()) }
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

/// The destination name for the source form's font `name`, copying the font into `fonts` when
/// the destination lacks it: `name` itself when free or already the same font, else the first
/// free `name_N`. `None` when the source form has no font of that name (nothing to bring).
fn place_font(
    dst: &mut Document,
    copier: &mut Copier<'_>,
    src_fonts: &Dict,
    fonts: &mut Dict,
    name: &[u8],
    changed: &mut bool,
) -> Result<Option<Vec<u8>>, OrganizeError> {
    let Some(entry) = src_fonts.get(name).cloned() else { return Ok(None) };
    let mut budget = SAME_FONT_BUDGET;
    if let Some(theirs) = fonts.get(name)
        && same_object(dst, theirs, copier.src, &entry, 0, &mut budget)
    {
        return Ok(Some(name.to_vec()));
    }
    let mut target = name.to_vec();
    let mut n = 0u32;
    while fonts.contains(&target) {
        n = n.saturating_add(1);
        target = [name, b"_", n.to_string().as_bytes()].concat();
    }
    let copied = copier.copy_value(dst, &entry, None)?;
    fonts.set(target.clone(), copied);
    *changed = true;
    Ok(Some(target))
}

/// How many objects [`same_object`] compares before calling two fonts different.
const SAME_FONT_BUDGET: usize = 4096;

/// Whether `a` (of `a_doc`) and `b` (of `b_doc`) are the same value, following references on
/// both sides. Bounded in depth and in work; past either, they count as different.
fn same_object(a_doc: &Document, a: &Object, b_doc: &Document, b: &Object, depth: u8, budget: &mut usize) -> bool {
    if depth > 16 || *budget == 0 {
        return false;
    }
    *budget -= 1;
    let (a, b) = (a_doc.resolve(a), b_doc.resolve(b));
    let same_dict = |x: &Dict, y: &Dict, budget: &mut usize| {
        x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_object(a_doc, v, b_doc, w, depth + 1, budget)))
    };
    match (a.as_ref(), b.as_ref()) {
        (Object::Dict(x), Object::Dict(y)) => same_dict(x, y, budget),
        (Object::Stream(x), Object::Stream(y)) => x.raw == y.raw && same_dict(&x.dict, &y.dict, budget),
        (Object::Array(x), Object::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(v, w)| same_object(a_doc, v, b_doc, w, depth + 1, budget))
        }
        (Object::Ref(_), _) | (_, Object::Ref(_)) => false,
        (x, y) => x == y,
    }
}

/// Every node of the field tree under `node` (kids before their own kids), each once, to the same
/// depth the form reader reads fields (`pdfcraft_forms` walks them 64 levels down): a field it
/// reads must have its `/DA` font carried, however deep it sits.
fn field_nodes(doc: &Document, node: ObjRef, depth: u8, seen: &mut HashSet<ObjRef>, out: &mut Vec<ObjRef>) {
    if depth > 64 || !seen.insert(node) {
        return;
    }
    out.push(node);
    let Some(d) = doc.get(node).as_dict().cloned() else { return };
    let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for kid in kids.iter().filter_map(|k| k.as_ref()) {
        field_nodes(doc, kid, depth + 1, seen, out);
    }
}

/// The document's `/AcroForm` dictionary.
fn form_of(doc: &Document) -> Option<Dict> {
    let catalog = doc.get(doc.root()?).as_dict().cloned()?;
    doc.resolve(catalog.get(b"AcroForm")?).as_dict().cloned()
}

/// A form's `/DR /Font` dictionary.
fn fonts_of(doc: &Document, form: &Dict) -> Option<Dict> {
    let dr = doc.resolve(form.get(b"DR")?).as_dict().cloned()?;
    doc.resolve(dr.get(b"Font")?).as_dict().cloned()
}

/// A dictionary's `/DA` string, as stored.
fn da_of(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    d.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.bytes.clone()))
}

/// The font a `/DA` sets, as the appearance reads it (the name two tokens before the last
/// `Tf`): where its name starts in `da` (after the `/`) and its name. A `/DA` is a string, so a
/// name written in it keeps the `#xx` escapes a name in a dictionary loses: they are decoded
/// here, to look the font up, while the range still names the bytes written in `da`.
fn da_font(da: &[u8]) -> Option<(std::ops::Range<usize>, Vec<u8>)> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, b) in da.iter().enumerate().chain(std::iter::once((da.len(), &b' '))) {
        match (b.is_ascii_whitespace(), start) {
            (true, Some(s)) => {
                tokens.push(s..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    let tf = tokens.iter().rposition(|t| da.get(t.clone()) == Some(b"Tf".as_slice()))?;
    let name = tokens.get(tf.checked_sub(2)?)?.clone();
    if da.get(name.start) != Some(&b'/') {
        return None;
    }
    let name = name.start + 1..name.end;
    let bytes = da.get(name.clone())?.to_vec();
    Some((name, decode_name(&bytes)))
}

/// `name` as a PDF name means it (ISO 32000-2 §7.3.5): every `#xx` of two hex digits is the byte
/// it escapes. A `#` that does not start an escape is kept as written, as readers keep a
/// malformed name.
fn decode_name(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len());
    let mut i = 0;
    while let Some(&b) = name.get(i) {
        if b == b'#'
            && let (Some(hi), Some(lo)) = (name.get(i + 1).and_then(hex_digit), name.get(i + 2).and_then(hex_digit))
        {
            out.push(hi << 4 | lo);
            i = i.saturating_add(3);
            continue;
        }
        out.push(b);
        i = i.saturating_add(1);
    }
    out
}

/// `name` as it is written after a `/` in a string: every byte that is not a regular character
/// escaped as `#xx`, which is what `serialize` does for a name.
fn encode_name(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    pdfcraft_cos::serialize(&Object::Name(name.to_vec()), &mut out);
    out.get(1..).unwrap_or_default().to_vec()
}

/// The value of one hex digit.
fn hex_digit(b: &u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Replace the destination form's `/DR /Font` with `fonts`, wherever the form keeps it
/// (`/DR` and `/Font` may each be direct or indirect). Returns the objects that were already
/// there and now hold a reference to a copied font, for deduplication to rewrite.
fn set_form_fonts(dst: &mut Document, fonts: Dict) -> Result<Vec<ObjRef>, OrganizeError> {
    let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
    let catalog = dst.get(root).as_dict().cloned().ok_or(OrganizeError::NoPageTree)?;
    let (form_ref, mut form) = match catalog.get(b"AcroForm") {
        Some(Object::Ref(r)) => (Some(*r), dst.get(*r).as_dict().cloned().unwrap_or_default()),
        Some(Object::Dict(d)) => (None, d.clone()),
        _ => (None, Dict::new()),
    };
    let mut owners = Vec::new();
    let put = |dst: &mut Document, owners: &mut Vec<ObjRef>, dr: &mut Dict, fonts: Dict| match dr.get(b"Font") {
        Some(Object::Ref(r)) => {
            dst.set(*r, Object::Dict(fonts));
            owners.push(*r);
        }
        _ => dr.set(b"Font".to_vec(), Object::Dict(fonts)),
    };
    if let Some(Object::Ref(dr_ref)) = form.get(b"DR").cloned() {
        let mut dr = dst.get(dr_ref).as_dict().cloned().unwrap_or_default();
        put(dst, &mut owners, &mut dr, fonts);
        dst.set(dr_ref, Object::Dict(dr));
        owners.push(dr_ref);
        return Ok(owners);
    }
    let mut dr = form.get(b"DR").and_then(|d| d.as_dict().cloned()).unwrap_or_default();
    put(dst, &mut owners, &mut dr, fonts);
    form.set(b"DR".to_vec(), Object::Dict(dr));
    match form_ref {
        Some(r) => {
            dst.set(r, Object::Dict(form));
            owners.push(r);
        }
        None => {
            // A form that was not there is a new object, and so is the reference the catalog gains
            // to it: deduplication rewrites both already, so neither is an owner.
            let r = dst.add(form);
            dst.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(r)))?;
        }
    }
    Ok(owners)
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

    /// A one-page form with a text field `field` in the `face` font named `alias` in its `/DR`;
    /// the field's own `/DA` is `field_da` (`""`: it takes the form's).
    fn font_form(face: &str, field: &str, alias: &str, field_da: &str) -> Document {
        build(&[
            &format!(
                "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DR << /Font << /{alias} 4 0 R >> >> /DA (/{alias} 12 Tf 0 g) >> >>"
            ),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
            &format!("<< /Type /Font /Subtype /Type1 /BaseFont /{face} >>"),
            &format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({field}) {field_da} /Rect [0 0 10 10] /P 3 0 R >>"),
        ])
    }

    /// A one-page form whose `/AcroForm` is an object of its own, as a real file's usually is:
    /// the import did not create it, so it is not one of the objects the import may rewrite.
    fn indirect_font_form(face: &str, field: &str, alias: &str) -> Document {
        build(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
            &format!("<< /Type /Font /Subtype /Type1 /BaseFont /{face} >>"),
            &format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({field}) /DA (/{alias} 12 Tf 0 g) /Rect [0 0 10 10] /P 3 0 R >>"),
            &format!("<< /Fields [5 0 R] /DR << /Font << /{alias} 4 0 R >> >> /DA (/{alias} 12 Tf 0 g) >>"),
        ])
    }

    /// The top-level fields the form lists.
    fn form_fields(doc: &Document) -> Vec<Object> {
        form_of(doc).and_then(|f| f.get(b"Fields").map(|l| doc.resolve(l))).and_then(|l| l.as_array().cloned()).unwrap_or_default()
    }

    /// The `/DA` of field `name` (its own, or else its parent's) and the base font its font name
    /// has in the form's `/DR`, plus how many fonts the `/DR` lists.
    fn field_font(doc: &Document, name: &str) -> (String, Option<String>, usize) {
        let form = form_of(doc).expect("form");
        let fonts = fonts_of(doc, &form).expect("fonts");
        for n in doc.object_numbers() {
            let Some(d) = doc.get(ObjRef::new(n, doc.generation(n))).as_dict().cloned() else { continue };
            if d.get(b"T").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).as_deref() != Some(name) {
                continue;
            }
            let da = da_of(doc, &d).expect("the field has a /DA");
            let (_, font) = da_font(&da).expect("the /DA sets a font");
            let base = fonts
                .get(&font)
                .and_then(|f| doc.resolve(f).as_dict().and_then(|f| f.name(b"BaseFont")).map(|b| String::from_utf8_lossy(b).into_owned()));
            return (String::from_utf8_lossy(&da).into_owned(), base, fonts.len());
        }
        panic!("no field {name}");
    }

    #[test]
    fn an_imported_field_brings_its_form_font() {
        let src = font_form("Courier", "imported", "F1", "/DA (/F1 12 Tf 0 g)");
        // The name is taken by another font: the imported one is renamed, and its /DA follows.
        let mut dst = font_form("Helvetica", "destination", "F1", "/DA (/F1 12 Tf 0 g)");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert_eq!(field_font(&dst, "imported"), ("/F1_1 12 Tf 0 g".into(), Some("Courier".into()), 2));
        assert_eq!(field_font(&dst, "destination"), ("/F1 12 Tf 0 g".into(), Some("Helvetica".into()), 2));
        // The same font under the same name is neither copied again nor renamed (Duplicate).
        let mut dst = font_form("Courier", "destination", "F1", "/DA (/F1 12 Tf 0 g)");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert_eq!(field_font(&dst, "imported"), ("/F1 12 Tf 0 g".into(), Some("Courier".into()), 1));
    }

    #[test]
    fn an_imported_field_keeps_the_default_appearance_of_its_form() {
        // The field has no /DA of its own: it took its source form's, which the copy now carries.
        let src = font_form("Courier", "imported", "F2", "");
        let mut dst = font_form("Helvetica", "destination", "F1", "/DA (/F1 12 Tf 0 g)");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert_eq!(field_font(&dst, "imported"), ("/F2 12 Tf 0 g".into(), Some("Courier".into()), 2));
    }

    #[test]
    fn an_imported_field_finds_its_font_through_an_escaped_name() {
        // The source names its font `/F#31` in both the `/DR` and the `/DA`: that is the name
        // `F1`, which the destination already gives to Helvetica, so the Courier comes in as
        // `F1_1` and the `/DA` follows.
        let src = font_form("Courier", "imported", "F#31", "/DA (/F#31 12 Tf 0 g)");
        let mut dst = font_form("Helvetica", "destination", "F1", "/DA (/F1 12 Tf 0 g)");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert_eq!(field_font(&dst, "imported"), ("/F1_1 12 Tf 0 g".into(), Some("Courier".into()), 2));
    }

    #[test]
    fn a_renamed_font_is_escaped_back_into_the_da() {
        // The source names its font `/F#20x`, which is the name `F x`. The destination gives
        // `F x` to Helvetica, so the Courier comes in as `F x_1`, and the `/DA` must write that
        // name escaped, or its space would end the name and break the `/DA`.
        let src = font_form("Courier", "imported", "F#20x", "/DA (/F#20x 12 Tf 0 g)");
        let mut dst = font_form("Helvetica", "destination", "F#20x", "/DA (/F#20x 12 Tf 0 g)");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert_eq!(field_font(&dst, "imported"), ("/F#20x_1 12 Tf 0 g".into(), Some("Courier".into()), 2));
        assert_eq!(field_font(&dst, "destination"), ("/F#20x 12 Tf 0 g".into(), Some("Helvetica".into()), 2));
    }

    #[test]
    fn an_imported_font_survives_deduplication_in_an_existing_form() {
        // The destination has the same Courier under another name and its form is an object of
        // its own. The import copies the Courier, deduplication merges the copy into the font
        // already there, and the form must end up pointing at the font that is still there.
        let src = font_form("Courier", "imported", "F2", "/DA (/F2 12 Tf 0 g)");
        let mut dst = indirect_font_form("Courier", "destination", "F1");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        let fonts = fonts_of(&dst, &form_of(&dst).expect("form")).expect("fonts");
        assert_eq!(fonts.len(), 2);
        for (name, font) in fonts.iter() {
            let base = dst.resolve(font).as_dict().and_then(|f| f.name(b"BaseFont")).map(|b| b.to_vec());
            assert_eq!(base.as_deref(), Some(b"Courier".as_slice()), "the font /{} is gone", String::from_utf8_lossy(name));
        }
        assert_eq!(field_font(&dst, "imported").1.as_deref(), Some("Courier"));
    }

    #[test]
    fn an_imported_field_brings_its_font_from_thirty_three_levels_down() {
        // The form reader reads fields 64 levels down, and the copy follows the whole tree: a
        // field that deep must have its font carried too, or editing it falls back to Helvetica.
        const DEEP: usize = 33;
        // The field tree starts at object 7, one node per level: the leaf is the last one.
        let (first, leaf) = (7, 7 + DEEP);
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /Annots [{leaf} 0 R] >>"),
            // The form's own `/DA` is the destination's Helvetica, which is already there: only
            // the deep field's `/F2` needs carrying, so nothing else brings it in.
            format!("<< /Fields [{first} 0 R] /DR << /Font << /F1 6 0 R /F2 5 0 R >> >> /DA (/F1 12 Tf 0 g) >>"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for d in 0..DEEP {
            let kids = format!("/Kids [{} 0 R]", first + d + 1);
            bodies.push(match d {
                0 => format!("<< /T (f0) {kids} >>"),
                _ => format!("<< /T (f{d}) /Parent {} 0 R {kids} >>", first + d - 1),
            });
        }
        bodies.push(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (leaf) /Parent {} 0 R /DA (/F2 12 Tf 0 g) /Rect [0 0 10 10] /P 3 0 R >>",
            leaf - 1
        ));
        let src = build(&bodies.iter().map(String::as_str).collect::<Vec<_>>());
        let mut dst = font_form("Helvetica", "destination", "F1", "/DA (/F1 12 Tf 0 g)");
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert_eq!(field_font(&dst, "leaf").1.as_deref(), Some("Courier"), "the deep field's font came along");
    }

    #[test]
    fn a_carried_font_is_never_merged_away_with_the_form_it_was_carried_into() {
        // The destination's `/DR` is a page's `/Resources` too, and another page's resources hold
        // the same fonts the import brings in. Once the font is carried in, the `/DR` equals that
        // resources dictionary: deduplication may rewrite it, but it must not merge it away — the
        // catalog's `/AcroForm /DR` and the page's `/Resources` still point at it.
        let src = font_form("Courier", "imported", "F2", "/DA (/F2 12 Tf 0 g)");
        let mut dst = build(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 9 0 R] /Count 2 /MediaBox [0 0 100 100] >>",
            "<< /Type /Page /Parent 2 0 R /Resources 7 0 R >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
            "<< /Fields [] /DR 7 0 R /DA (/F1 12 Tf 0 g) >>",
            "<< /Font << /F1 4 0 R >> >>",
            "<< /Font << /F1 4 0 R /F2 5 0 R >> >>",
            "<< /Type /Page /Parent 2 0 R /Resources 8 0 R >>",
        ]);
        let dr = ObjRef::new(7, 0);
        crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).expect("imports");
        assert!(dst.object_numbers().contains(&dr.num), "the form's /DR is still there");
        let catalog = dst.get(dst.root().expect("root")).as_dict().cloned().expect("catalog");
        let form = dst.resolve(catalog.get(b"AcroForm").expect("AcroForm")).as_dict().cloned().expect("form");
        assert_eq!(form.get(b"DR"), Some(&Object::Ref(dr)), "the catalog still names the /DR that is there");
        let page = dst.get(ObjRef::new(3, 0)).as_dict().cloned().expect("page");
        assert_eq!(page.get(b"Resources"), Some(&Object::Ref(dr)), "the page still shares it");
        // The copied Courier merged into the one already there, and the /DR followed it.
        let fonts = fonts_of(&dst, &form).expect("fonts");
        let base = |name: &[u8]| fonts.get(name).and_then(|f| dst.resolve(f).as_dict().and_then(|f| f.name(b"BaseFont")).map(|b| b.to_vec()));
        assert_eq!(base(b"F2").as_deref(), Some(b"Courier".as_slice()));
        assert_eq!(base(b"F1").as_deref(), Some(b"Helvetica".as_slice()));
    }

    #[test]
    fn a_font_the_source_nests_too_deep_leaves_the_destination_form_as_it_was() {
        // The page copies, then the source form's font turns out to be a chain deeper than a copy
        // follows. The import is refused, and the destination must be as it was: its form must
        // not list the fields the rollback frees.
        const DEEP: usize = 300;
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DR << /Font << /F1 4 0 R >> >> /DA (/F1 12 Tf 0 g) >> >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Deep 6 0 R >>".to_string(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (imported) /DA (/F1 12 Tf 0 g) /Rect [0 0 10 10] /P 3 0 R >>".to_string(),
        ];
        for i in 0..DEEP {
            bodies.push(if i + 1 < DEEP { format!("<< /Deep {} 0 R >>", 7 + i) } else { "<< >>".to_string() });
        }
        let src = build(&bodies.iter().map(String::as_str).collect::<Vec<_>>());
        let mut dst = font_form("Helvetica", "destination", "F1", "/DA (/F1 12 Tf 0 g)");
        let before = form_fields(&dst);
        assert!(crate::import::import_pages_with(&mut dst, &src, &[0], 1, crate::import::FieldConflicts::Rename).is_err());
        assert_eq!(form_fields(&dst), before);
    }

    #[test]
    fn da_font_finds_the_name_the_last_tf_sets() {
        assert_eq!(da_font(b"/F1 12 Tf 0 g"), Some((1..3, b"F1".to_vec())));
        assert_eq!(da_font(b"0 g /A 9 Tf\n/Cour 0 Tf"), Some((13..17, b"Cour".to_vec())));
        // The `/DA` is a string, so a name in it keeps the escapes a name in a dictionary loses.
        assert_eq!(da_font(b"/F#31 12 Tf 0 g"), Some((1..5, b"F1".to_vec())));
        assert_eq!(da_font(b"/F#2 12 Tf 0 g"), Some((1..4, b"F#2".to_vec())), "a malformed escape is kept");
        assert_eq!(da_font(b"12 Tf 0 g"), None);
        assert_eq!(da_font(b"F1 12 Tf"), None);
        assert_eq!(da_font(b""), None);
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
