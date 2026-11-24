//! Keep a tagged PDF's structure tree in sync when annotations are deleted (ISO 32000-2 §14.7.5).
//!
//! Deleting a link or comment removes it from its page's `/Annots`. In a tagged document, the
//! structure tree may still point at that annotation through an `/OBJR` kid of a structure element
//! and through an entry in `/StructTreeRoot /ParentTree` keyed by the annotation's `/StructParent`.
//! [`untag_removed`] drops those stale references on a best-effort basis without ever failing the
//! deletion.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{annots, page_refs};

/// Maximum number of structure-tree references / walk steps inspected in a single cleanup pass.
const MAX_STRUCT_REFS: usize = 2_000_000;

/// Whether `doc` has a `/StructTreeRoot` in its catalog.
pub(crate) fn has_structure_tree(doc: &Document) -> bool {
    doc.root().and_then(|r| doc.get(r).as_dict().map(|d| d.contains(b"StructTreeRoot"))).unwrap_or(false)
}

/// Read an annotation object's `/StructParent` integer key before it is removed.
pub(crate) fn struct_parent(doc: &Document, annot: ObjRef) -> Option<i64> {
    doc.resolve(&Object::Ref(annot)).as_dict()?.get(b"StructParent").and_then(|o| doc.resolve(o).as_int())
}

/// Best-effort cleanup of structure-tree `/OBJR` kids and `/ParentTree` entries pointing to
/// annotations removed from page `/Annots`.
///
/// `gone` carries each removed annotation's reference and its `/StructParent` read before removal.
/// References still listed in any page's `/Annots` are skipped so shared annotations stay tagged.
pub(crate) fn untag_removed(doc: &mut Document, gone: &[(ObjRef, Option<i64>)]) {
    untag_removed_within(doc, gone, MAX_STRUCT_REFS);
}

pub(crate) fn untag_removed_within(doc: &mut Document, gone: &[(ObjRef, Option<i64>)], limit: usize) {
    if gone.is_empty() || limit == 0 {
        return;
    }
    let Some(catalog_ref) = doc.root() else { return };
    let Some(struct_root_obj) = doc.get(catalog_ref).as_dict().and_then(|c| c.get(b"StructTreeRoot").cloned()) else {
        return;
    };
    let struct_root_ref = struct_root_obj.as_ref();
    let Some(root) = doc.resolve(&struct_root_obj).as_dict().cloned() else { return };

    // Deduplicate removed refs and drop any still listed on some page's `/Annots` (shared
    // annotations stay tagged).
    let mut gone_map: HashMap<ObjRef, Option<i64>> = HashMap::new();
    for &(r, sp) in gone {
        gone_map.entry(r).and_modify(|existing| *existing = existing.or(sp)).or_insert(sp);
    }
    if let Ok(pages) = page_refs(doc) {
        for p in pages {
            for entry in annots(doc, p) {
                if let Some(r) = entry.as_ref() {
                    gone_map.remove(&r);
                }
            }
            if gone_map.is_empty() {
                return;
            }
        }
    }
    if gone_map.is_empty() {
        return;
    }

    // Walk the structure tree from `/K`, `/ParentTree` and `/IDTree`, bounded by `seen` and `limit`.
    let mut todo: Vec<Object> = [&b"K"[..], b"ParentTree", b"IDTree"].iter().filter_map(|key| root.get(key).cloned()).collect();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    if let Some(sr) = struct_root_ref {
        seen.insert(sr);
    }
    let mut steps: usize = 0;
    let mut hits: Vec<ObjRef> = Vec::new();
    let mut parent_tree_targets: HashSet<(i64, ObjRef)> = HashSet::new();

    while let Some(item) = todo.pop() {
        if steps >= limit {
            break;
        }
        steps += 1;
        let remaining = limit.saturating_sub(todo.len());
        match item {
            Object::Array(a) => todo.extend(a.into_iter().take(remaining)),
            // Direct structure element (written in place): leave it unchanged, still walk its kids
            // in case it contains indirect elements.
            Object::Dict(d) if d.contains(b"S") => {
                todo.extend(kids_of(doc, &d).into_iter().take(remaining));
            }
            // Direct number/name tree node written in place.
            Object::Dict(d) => {
                todo.extend(tree_node_entries(&d).into_iter().take(remaining));
            }
            Object::Ref(r) => {
                if !seen.insert(r) {
                    continue;
                }
                if seen.len() > limit {
                    break;
                }
                let object = doc.get(r);
                if let Object::Array(a) = &*object {
                    todo.extend(a.iter().take(remaining).cloned());
                    continue;
                }
                let Some(d) = object.as_dict() else { continue };
                if !d.contains(b"S") {
                    todo.extend(tree_node_entries(d).into_iter().take(remaining));
                    continue;
                }
                let kids = kids_of(doc, d);
                let mut hit = false;
                for kid in &kids {
                    if let Some(annot_ref) = objr_target(doc, kid, &gone_map) {
                        hit = true;
                        if let Some(sp) = gone_map.get(&annot_ref).copied().flatten() {
                            parent_tree_targets.insert((sp, r));
                        }
                    }
                }
                if hit {
                    hits.push(r);
                }
                todo.extend(kids.into_iter().take(remaining));
            }
            _ => {}
        }
    }

    if hits.is_empty() {
        return;
    }

    // Remove matching `/OBJR` kids from each indirect structure element; record any left empty.
    let mut emptied: Vec<ObjRef> = Vec::new();
    for &r in &hits {
        let Some(d) = doc.get(r).as_dict().cloned() else { continue };
        let resolved_k = d.get(b"K").map(|k| doc.resolve(k));
        let was_array = resolved_k.as_ref().is_some_and(|k| matches!(&**k, Object::Array(_)));
        let list: Vec<Object> = match resolved_k.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => d.get(b"K").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        let keep: Vec<Object> = list.into_iter().filter(|x| objr_target(doc, x, &gone_map).is_none()).collect();
        let now_empty = keep.is_empty();
        let _ = doc.update_dict(r, |e| {
            if now_empty {
                e.remove(b"K");
            } else if was_array {
                e.set(b"K".to_vec(), Object::Array(keep));
            } else if let Some(only) = keep.into_iter().next() {
                e.set(b"K".to_vec(), only);
            } else {
                e.remove(b"K");
            }
        });
        if now_empty {
            emptied.push(r);
        }
    }

    // Remove emptied elements from their immediate `/P` parent's `/K` (do not climb further),
    // and drop them from a flat `/IDTree` if present.
    if !emptied.is_empty() {
        detach_from_parents(doc, &emptied, struct_root_ref);
        prune_flat_id_tree(doc, &root, struct_root_ref, &emptied);
    }

    // Remove matching `[k v]` pairs from `/ParentTree` number-tree nodes.
    if !parent_tree_targets.is_empty() {
        prune_parent_tree(doc, &root, struct_root_ref, &parent_tree_targets, limit);
    }
}

/// An element's kids: its `/K` array, or the single kid it holds.
fn kids_of(doc: &Document, d: &Dict) -> Vec<Object> {
    match d.get(b"K").map(|k| doc.resolve(k)) {
        Some(o) => match &*o {
            Object::Array(a) => a.clone(),
            _ => d.get(b"K").cloned().into_iter().collect(),
        },
        None => Vec::new(),
    }
}

/// If `o` is an object reference (`/Type /OBJR`, direct or indirect) to a removed annotation in
/// `gone`, returns that annotation's reference.
fn objr_target(doc: &Document, o: &Object, gone: &HashMap<ObjRef, Option<i64>>) -> Option<ObjRef> {
    let resolved = doc.resolve(o);
    let d = resolved.as_dict()?;
    if d.name(b"Type") != Some(b"OBJR") {
        return None;
    }
    let target = d.get(b"Obj").and_then(Object::as_ref)?;
    gone.contains_key(&target).then_some(target)
}

/// Entries of a number or name tree node to follow: `/Nums`, `/Names`, `/Kids`.
fn tree_node_entries(d: &Dict) -> Vec<Object> {
    [&b"Nums"[..], b"Names", b"Kids"].iter().filter_map(|key| d.get(key).cloned()).collect()
}

/// Remove each element in `emptied` from its immediate `/P` parent's `/K` (never climbing further).
fn detach_from_parents(doc: &mut Document, emptied: &[ObjRef], struct_root_ref: Option<ObjRef>) {
    let mut by_parent: HashMap<ObjRef, HashSet<ObjRef>> = HashMap::new();
    for &elem in emptied {
        let Some(parent_ref) = doc.get(elem).as_dict().and_then(|d| d.get(b"P").and_then(Object::as_ref)) else {
            continue;
        };
        if parent_ref == elem {
            continue;
        }
        let valid_parent = doc.get(parent_ref).as_dict().is_some_and(|pd| pd.contains(b"S") || Some(parent_ref) == struct_root_ref);
        if valid_parent {
            by_parent.entry(parent_ref).or_default().insert(elem);
        }
    }
    for (parent_ref, drop_set) in by_parent {
        let Some(pd) = doc.get(parent_ref).as_dict().cloned() else { continue };
        let Some(k_entry) = pd.get(b"K") else { continue };
        let resolved_k = doc.resolve(k_entry);
        match &*resolved_k {
            Object::Array(items) => {
                let kept: Vec<Object> = items.iter().filter(|x| x.as_ref().is_none_or(|r| !drop_set.contains(&r))).cloned().collect();
                if kept.len() != items.len() {
                    let _ = doc.update_dict(parent_ref, |d| {
                        d.set(b"K".to_vec(), Object::Array(kept));
                    });
                }
            }
            _ => {
                if k_entry.as_ref().is_some_and(|r| drop_set.contains(&r)) {
                    let _ = doc.update_dict(parent_ref, |d| {
                        d.remove(b"K");
                    });
                }
            }
        }
    }
}

/// Drop `emptied` elements from `/IDTree` when it is a single flat `/Names` node (no `/Kids` or
/// `/Limits`); otherwise leave `/IDTree` untouched.
fn prune_flat_id_tree(doc: &mut Document, root: &Dict, struct_root_ref: Option<ObjRef>, emptied: &[ObjRef]) {
    let Some(id_tree) = root.get(b"IDTree") else { return };
    let drop_set: HashSet<ObjRef> = emptied.iter().copied().collect();
    let filter_names = |names: &[Object]| -> Option<Vec<Object>> {
        let mut out = Vec::with_capacity(names.len());
        let mut changed = false;
        let (pairs, remainder) = names.as_chunks::<2>();
        for [k, v] in pairs {
            if v.as_ref().is_some_and(|r| drop_set.contains(&r)) {
                changed = true;
                continue;
            }
            out.push(k.clone());
            out.push(v.clone());
        }
        if !changed {
            return None;
        }
        out.extend_from_slice(remainder);
        Some(out)
    };
    match id_tree {
        Object::Ref(r) => {
            let Some(d) = doc.get(*r).as_dict().cloned() else { return };
            if d.contains(b"Kids") || d.contains(b"Limits") {
                return;
            }
            let Some(Object::Array(names)) = d.get(b"Names") else { return };
            if let Some(new_names) = filter_names(names) {
                let _ = doc.update_dict(*r, |nd| {
                    nd.set(b"Names".to_vec(), Object::Array(new_names));
                });
            }
        }
        Object::Dict(d) => {
            if d.contains(b"Kids") || d.contains(b"Limits") {
                return;
            }
            let Some(sr) = struct_root_ref else { return };
            let Some(Object::Array(names)) = d.get(b"Names") else { return };
            if let Some(new_names) = filter_names(names) {
                let _ = doc.update_dict(sr, |rd| {
                    if let Some(Object::Dict(mut id_dict)) = rd.get(b"IDTree").cloned() {
                        id_dict.set(b"Names".to_vec(), Object::Array(new_names));
                        rd.set(b"IDTree".to_vec(), Object::Dict(id_dict));
                    }
                });
            }
        }
        _ => {}
    }
}

/// Remove `[k v]` pairs from `/ParentTree` number-tree nodes where `(k, v)` is in `targets`.
/// Leaves `/ParentTreeNextKey` alone.
fn prune_parent_tree(doc: &mut Document, root: &Dict, struct_root_ref: Option<ObjRef>, targets: &HashSet<(i64, ObjRef)>, limit: usize) {
    let Some(pt) = root.get(b"ParentTree") else { return };
    let mut todo: Vec<ObjRef> = Vec::new();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    if let Some(sr) = struct_root_ref {
        seen.insert(sr);
    }

    match pt {
        Object::Ref(r) => todo.push(*r),
        Object::Dict(d) => {
            match d.get(b"Nums") {
                Some(Object::Array(nums)) => {
                    if let Some(new_nums) = filter_nums(doc, nums, targets)
                        && let Some(sr) = struct_root_ref
                    {
                        let _ = doc.update_dict(sr, |rd| {
                            if let Some(Object::Dict(mut pt_dict)) = rd.get(b"ParentTree").cloned() {
                                pt_dict.set(b"Nums".to_vec(), Object::Array(new_nums));
                                rd.set(b"ParentTree".to_vec(), Object::Dict(pt_dict));
                            }
                        });
                    }
                }
                Some(Object::Ref(nums_ref)) => {
                    if let Some(nums) = doc.get(*nums_ref).as_array().cloned()
                        && let Some(new_nums) = filter_nums(doc, &nums, targets)
                    {
                        doc.set(*nums_ref, Object::Array(new_nums));
                    }
                }
                _ => {}
            }
            push_kid_refs(doc, d, &mut todo, limit);
        }
        _ => return,
    }

    let mut steps: usize = 0;
    while let Some(node_ref) = todo.pop() {
        if steps >= limit {
            break;
        }
        steps += 1;
        if !seen.insert(node_ref) {
            continue;
        }
        if seen.len() > limit {
            break;
        }
        let Some(d) = doc.get(node_ref).as_dict().cloned() else { continue };
        match d.get(b"Nums") {
            Some(Object::Array(nums)) => {
                if let Some(new_nums) = filter_nums(doc, nums, targets) {
                    let _ = doc.update_dict(node_ref, |nd| {
                        nd.set(b"Nums".to_vec(), Object::Array(new_nums));
                    });
                }
            }
            Some(Object::Ref(nums_ref)) => {
                if let Some(nums) = doc.get(*nums_ref).as_array().cloned()
                    && let Some(new_nums) = filter_nums(doc, &nums, targets)
                {
                    doc.set(*nums_ref, Object::Array(new_nums));
                }
            }
            _ => {}
        }
        push_kid_refs(doc, &d, &mut todo, limit);
    }
}

fn push_kid_refs(doc: &Document, d: &Dict, todo: &mut Vec<ObjRef>, limit: usize) {
    let Some(kids_obj) = d.get(b"Kids") else { return };
    let resolved = doc.resolve(kids_obj);
    let Some(kids) = resolved.as_array() else { return };
    let remaining = limit.saturating_sub(todo.len());
    todo.extend(kids.iter().filter_map(Object::as_ref).take(remaining));
}

/// Filter `[k v]` pairs from a `/Nums` array, returning `Some(new)` only if at least one pair was
/// removed.
fn filter_nums(doc: &Document, nums: &[Object], targets: &HashSet<(i64, ObjRef)>) -> Option<Vec<Object>> {
    let mut out = Vec::with_capacity(nums.len());
    let mut changed = false;
    let (pairs, remainder) = nums.as_chunks::<2>();
    for [k_obj, v_obj] in pairs {
        let key = doc.resolve(k_obj).as_int();
        let val = v_obj.as_ref();
        if let (Some(k), Some(v)) = (key, val)
            && targets.contains(&(k, v))
        {
            changed = true;
            continue;
        }
        out.push(k_obj.clone());
        out.push(v_obj.clone());
    }
    if !changed {
        return None;
    }
    out.extend_from_slice(remainder);
    Some(out)
}
