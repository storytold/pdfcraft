//! Tag form fields: a `/Form` tag with an object reference for every page widget no tag
//! references yet, in any tag-tree layout the other edits read.
//!
//! The tags go where the Tags panel's Tag annotation puts one: after the last tag that owns
//! content on the widget's page, else next to the structure of the pages around it. A widget a
//! tag already references but whose `/StructParent` doesn't lead back to that tag gets the
//! missing parent-tree entry instead of a second tag.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object};

use super::parent_tree::Value;
use super::tree::Owner;
use super::{
    Destination, TaggingError, after_page_content, annotation_tag, annotations, auto_destination, container_ref, ensure_structure, kids_raw,
    parent_tree, pt_error, read_tree, resolve_destination, save_next_key, set_kids,
};

/// Tag every form field widget on the pages that no tag references, and link every widget a
/// tag references whose `/StructParent` doesn't lead back to it; returns how many widgets were
/// tagged or linked.
///
/// An untagged form gets a tag tree (a `/Document` tag) first. Any existing layout is extended
/// in place: a root stored in the catalog, a `/K` that is one entry, an array or an indirect
/// array, several top tags, and a parent tree flat or with `/Kids` and `/Limits`, with direct or
/// indirect arrays. Keys come after `/ParentTreeNextKey` and skip every key a page or
/// annotation already uses. A widget listed on several pages is tagged once, on the first. A
/// stale `/StructParent` (one no tag answers to) is replaced. Text, figures, links and other
/// content remain untagged.
pub fn tag_form_fields(doc: &mut Document) -> Result<usize, TaggingError> {
    let catalog = doc.root().ok_or(TaggingError::NoCatalog)?;
    if !doc.get(catalog).as_dict().is_some_and(|c| c.contains(b"AcroForm")) {
        return Err(TaggingError::NoForm);
    }
    let mut next = doc.clone();
    let root = ensure_structure(&mut next, true)?;
    let mut tree = read_tree(&next)?;
    let pages = pdfcraft_model::pages(&next);
    let mut pt = parent_tree(&next, root)?;
    // Untagged widgets by page, widgets to link to the tag that references them, and widgets
    // whose tags can't be linked.
    let mut untagged: Vec<(usize, Vec<ObjRef>)> = Vec::new();
    let mut unlinked: Vec<(ObjRef, ObjRef)> = Vec::new();
    let mut left = 0usize;
    let mut seen = HashSet::new();
    for (i, page) in pages.iter().enumerate() {
        let mut here = Vec::new();
        for item in annotations(&next, &tree, page) {
            if item.subtype != "Widget" || !seen.insert(item.object) {
                continue;
            }
            match tree.object_owners.get(&item.object).map(Vec::as_slice) {
                None | Some([]) => here.push(item.object),
                Some([Owner::Element(tag)]) => {
                    let key = next.get(item.object).as_dict().and_then(|d| d.get(b"StructParent").and_then(Object::as_int));
                    if key.and_then(|k| pt.get(&next, k)) != Some(Value::Single(Object::Ref(*tag))) {
                        unlinked.push((item.object, *tag));
                    }
                }
                Some(_) => left = left.saturating_add(1),
            }
        }
        if !here.is_empty() {
            untagged.push((i, here));
        }
    }
    let count = untagged.iter().map(|(_, w)| w.len()).sum::<usize>().saturating_add(unlinked.len());
    if count == 0 {
        return Err(if left > 0 { TaggingError::FieldsNotLinkable(left) } else { TaggingError::NoUntaggedFields });
    }
    let mut entries: Vec<(i64, Object)> = Vec::with_capacity(count);
    for (widget, tag) in unlinked {
        let key = pt.allocate().map_err(pt_error)?;
        next.update_dict(widget, |d| d.set(b"StructParent".to_vec(), Object::Int(key)))?;
        entries.push((key, Object::Ref(tag)));
    }
    // New tags never move existing ones, so `tree` as read before them still places tags next
    // to a page's own content. Placing them by page order reads it again, to see the earlier
    // pages' new tags.
    let mut added = false;
    for (page, widgets) in &untagged {
        let Some(p) = pages.get(*page) else { continue };
        let dest = match after_page_content(&next, &tree, p) {
            Some(d) => d,
            None => {
                if added {
                    tree = read_tree(&next)?;
                }
                auto_destination(&next, &tree, &pages, *page, &[], &[])
            }
        };
        // A destination that can't be used (its parent lists it twice) leaves the root.
        let (container, index) = resolve_destination(&tree, &next, root, dest, None)
            .or_else(|_| resolve_destination(&tree, &next, root, Destination::Into(None), None))?;
        let parent = container_ref(root, container).ok_or(TaggingError::InlineParent(root))?;
        let mut tags = Vec::with_capacity(widgets.len());
        for widget in widgets {
            let key = pt.allocate().map_err(pt_error)?;
            let tag = annotation_tag(&mut next, "Form", parent, p.obj, *widget, key)?;
            entries.push((key, Object::Ref(tag)));
            tags.push(Object::Ref(tag));
        }
        // The page's tags in /Annots order, side by side.
        let mut kids = kids_raw(&next, root, container)?;
        if index > kids.len() {
            return Err(TaggingError::BadIndex(index));
        }
        kids.splice(index..index, tags);
        set_kids(&mut next, root, container, kids)?;
        added = true;
    }
    pt.add_singles(&mut next, entries).map_err(pt_error)?;
    save_next_key(&mut next, root, &pt)?;
    mark_tagged(&mut next, catalog)?;
    *doc = next;
    Ok(count)
}

/// `/MarkInfo /Marked true`, in the catalog or in the dictionary it refers to.
fn mark_tagged(doc: &mut Document, catalog: ObjRef) -> Result<(), TaggingError> {
    let entry = doc.get(catalog).as_dict().and_then(|c| c.get(b"MarkInfo").cloned());
    let info = entry.as_ref().and_then(|m| doc.resolve(m).as_dict().cloned());
    if info.as_ref().and_then(|d| d.get(b"Marked")) == Some(&Object::Bool(true)) {
        return Ok(());
    }
    match (entry, info) {
        (Some(Object::Ref(r)), Some(_)) => doc.update_dict(r, |d| d.set(b"Marked".to_vec(), Object::Bool(true)))?,
        (_, info) => {
            let mut info = info.unwrap_or_default();
            info.set(b"Marked".to_vec(), Object::Bool(true));
            doc.update_dict(catalog, |c| c.set(b"MarkInfo".to_vec(), Object::Dict(info)))?;
        }
    }
    Ok(())
}
