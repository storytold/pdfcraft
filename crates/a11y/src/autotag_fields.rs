//! Conservative tagging for AcroForm widgets.
//!
//! It creates `/Form` structure elements and `/OBJR` children so assistive technology can find
//! widgets. An existing direct `/Document` root and direct `/ParentTree /Nums` are extended;
//! unusual tree layouts are refused before any edit is made.

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

fn reference(object: Option<&Object>, what: &str) -> Result<ObjRef, String> {
    object.and_then(Object::as_ref).ok_or_else(|| format!("the {what} must be an indirect reference"))
}

fn nums(doc: &Document, parent_tree: ObjRef) -> Result<Vec<Object>, String> {
    let tree = doc.get(parent_tree);
    let tree = tree.as_dict().ok_or("the structure parent tree is invalid")?;
    tree.get(b"Nums")
        .map(|value| doc.resolve(value))
        .as_deref()
        .and_then(Object::as_array)
        .cloned()
        .ok_or_else(|| "the structure parent tree must use a direct Nums array".to_string())
}

/// Add `/Form` tags for every page widget that does not already have a structure parent.
///
/// A new structure tree is written for an untagged form. A conventional existing tree is
/// extended by adding the form tags to its direct `/Document` element and fresh entries to its
/// direct `/ParentTree /Nums` array. Text, figures, links, and non-widget annotations remain
/// outside this operation and need separate review.
pub fn tag_form_fields(doc: &mut Document) -> Result<usize, String> {
    let root = doc.root().ok_or("the PDF has no catalog")?;
    let catalog_object = doc.get(root);
    let catalog = catalog_object.as_dict().ok_or("the PDF catalog is invalid")?;
    if !catalog.contains(b"AcroForm") {
        return Err("the PDF has no AcroForm fields".into());
    }
    let existing_root = catalog.get(b"StructTreeRoot").cloned();
    let mark_info = catalog.get(b"MarkInfo").map(|v| doc.resolve(v)).as_deref().and_then(Object::as_dict).cloned().unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    let mut widgets = Vec::new();
    // A widget listed in more than one page's /Annots is tagged once (one key, one /Form).
    let mut seen = std::collections::HashSet::new();
    for page in &pages {
        let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a)).as_deref().and_then(Object::as_array).cloned().unwrap_or_default();
        for annot in annots {
            let Some(reference) = annot.as_ref() else { continue };
            let resolved = doc.get(reference);
            let Some(dict) = resolved.as_dict() else { continue };
            if dict.name(b"Subtype") == Some(b"Widget") && !dict.contains(b"StructParent") && seen.insert(reference) {
                widgets.push((page.obj, reference));
            }
        }
    }
    if widgets.is_empty() {
        return Err("the AcroForm has no untagged page widgets".into());
    }

    let (struct_root, document_ref, parent_tree, mut parent_nums, first_key, new_tree) = if let Some(existing_root) = existing_root {
        let struct_root = reference(Some(&existing_root), "structure tree root")?;
        let structure_object = doc.get(struct_root);
        let structure = structure_object.as_dict().ok_or("the structure tree root is invalid")?;
        let document_ref = reference(structure.get(b"K"), "structure tree root K")?;
        let document = doc.get(document_ref);
        if document.as_dict().is_none_or(|d| d.name(b"S") != Some(b"Document")) {
            return Err("the structure tree root must point directly to a Document element".into());
        }
        let parent_tree = reference(structure.get(b"ParentTree"), "structure parent tree")?;
        let parent_nums = nums(doc, parent_tree)?;
        if parent_nums.len() % 2 != 0 {
            return Err("the structure parent tree has an odd number of entries".into());
        }
        let mut greatest = -1_i64;
        for pair in parent_nums.as_chunks::<2>().0 {
            let Some(key) = pair[0].as_int() else { return Err("the structure parent tree has a non-integer key".into()) };
            if key < 0 {
                return Err("the structure parent tree has a negative key".into());
            }
            greatest = greatest.max(key);
        }
        let first_key = greatest.checked_add(1).ok_or("the structure parent tree has no available key")?;
        (struct_root, document_ref, parent_tree, parent_nums, first_key, false)
    } else {
        let mut root_dict = Dict::new();
        root_dict.set(b"Type".to_vec(), Object::Name(b"StructTreeRoot".to_vec()));
        let struct_root = doc.add(Object::Dict(root_dict));
        let mut document = Dict::new();
        document.set(b"S".to_vec(), Object::Name(b"Document".to_vec()));
        document.set(b"P".to_vec(), Object::Ref(struct_root));
        let document_ref = doc.add(Object::Dict(document));
        let parent_tree = doc.add(Object::Dict(Dict::new()));
        (struct_root, document_ref, parent_tree, Vec::new(), 0, true)
    };
    let mut kids = Vec::with_capacity(widgets.len());
    for (offset, (page, widget)) in widgets.iter().copied().enumerate() {
        let key = first_key.checked_add(i64::try_from(offset).map_err(|_| "too many form widgets")?).ok_or("too many form widgets")?;
        let mut objr = Dict::new();
        objr.set(b"Type".to_vec(), Object::Name(b"OBJR".to_vec()));
        objr.set(b"Obj".to_vec(), Object::Ref(widget));
        objr.set(b"Pg".to_vec(), Object::Ref(page));
        let mut form = Dict::new();
        form.set(b"S".to_vec(), Object::Name(b"Form".to_vec()));
        form.set(b"P".to_vec(), Object::Ref(document_ref));
        form.set(b"Pg".to_vec(), Object::Ref(page));
        form.set(b"K".to_vec(), Object::Dict(objr));
        let form_ref = doc.add(Object::Dict(form));
        doc.update_dict(widget, |d| d.set(b"StructParent".to_vec(), Object::Int(key))).map_err(|e| e.to_string())?;
        parent_nums.extend([Object::Int(key), Object::Ref(form_ref)]);
        kids.push(Object::Ref(form_ref));
    }
    doc.update_dict(document_ref, |d| {
        let old = d.remove(b"K").unwrap_or(Object::Array(Vec::new()));
        let mut all = match old {
            Object::Array(items) => items,
            item => vec![item],
        };
        all.extend(kids);
        d.set(b"K".to_vec(), Object::Array(all));
    })
    .map_err(|e| e.to_string())?;
    doc.update_dict(parent_tree, |d| d.set(b"Nums".to_vec(), Object::Array(parent_nums))).map_err(|e| e.to_string())?;
    if new_tree {
        doc.update_dict(struct_root, |d| {
            d.set(b"K".to_vec(), Object::Ref(document_ref));
            d.set(b"ParentTree".to_vec(), Object::Ref(parent_tree));
        })
        .map_err(|e| e.to_string())?;
    }
    let mut mark_info = mark_info;
    mark_info.set(b"Marked".to_vec(), Object::Bool(true));
    doc.update_dict(root, |d| {
        d.set(b"MarkInfo".to_vec(), Object::Dict(mark_info));
        if new_tree {
            d.set(b"StructTreeRoot".to_vec(), Object::Ref(struct_root));
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(widgets.len())
}
