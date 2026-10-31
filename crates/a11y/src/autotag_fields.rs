//! Conservative tagging for AcroForm widgets.
//!
//! It creates `/Form` structure elements and `/OBJR` children so assistive technology can find
//! widgets. An existing direct `/Document` root and direct `/ParentTree /Nums` are extended;
//! unusual tree layouts are refused before any edit is made.

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

fn reference(object: Option<&Object>, what: &str) -> Result<ObjRef, String> {
    object.and_then(Object::as_ref).ok_or_else(|| format!("the {what} must be an indirect reference"))
}

fn root_document_ref(doc: &Document, k: Option<&Object>) -> Result<ObjRef, String> {
    let Some(k) = k else {
        return Err("the structure tree root K must be an indirect reference".into());
    };
    if let Some(r) = k.as_ref()
        && doc.get(r).as_dict().is_some()
    {
        return Ok(r);
    }
    if let Some([item]) = doc.resolve(k).as_array().map(Vec::as_slice)
        && let Some(r) = item.as_ref()
    {
        return Ok(r);
    }
    Err("the structure tree root K must be an indirect reference".into())
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
    if !catalog.get(b"AcroForm").is_some_and(|v| !matches!(*doc.resolve(v), Object::Null)) {
        return Err("the PDF has no AcroForm fields".into());
    }
    let existing_root = catalog.get(b"StructTreeRoot").filter(|v| !matches!(*doc.resolve(v), Object::Null)).cloned();
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
            let is_widget = dict.get(b"Subtype").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) == Some(b"Widget");
            let has_parent = dict.get(b"StructParent").map(|v| doc.resolve(v)).and_then(|v| v.as_int()).is_some_and(|k| k >= 0);
            if is_widget && !has_parent && seen.insert(reference) {
                widgets.push((page.obj, reference));
            }
        }
    }
    if widgets.is_empty() {
        return Err("the AcroForm has no untagged page widgets".into());
    }
    let count_delta = i64::try_from(widgets.len().saturating_sub(1)).map_err(|_| "too many form widgets")?;

    let (struct_root, document_ref, mut existing_kids, parent_tree, mut parent_nums, first_key, has_next_key, new_tree) =
        if let Some(existing_root) = existing_root {
            let struct_root = reference(Some(&existing_root), "structure tree root")?;
            let structure_object = doc.get(struct_root);
            let structure = structure_object.as_dict().ok_or("the structure tree root is invalid")?;
            let document_ref = root_document_ref(doc, structure.get(b"K"))?;
            let document_obj = doc.get(document_ref);
            let Some(document_dict) = document_obj.as_dict() else {
                return Err("the structure tree root must point directly to a Document element".into());
            };
            if document_dict.get(b"S").map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name) != Some(b"Document") {
                return Err("the structure tree root must point directly to a Document element".into());
            }
            let existing_kids = match document_dict.get(b"K") {
                None => Vec::new(),
                Some(k) => match &*doc.resolve(k) {
                    Object::Null => Vec::new(),
                    Object::Array(items) => items.clone(),
                    _ => vec![k.clone()],
                },
            };
            let parent_tree = reference(structure.get(b"ParentTree"), "structure parent tree")?;
            let parent_nums = nums(doc, parent_tree)?;
            if parent_nums.len() % 2 != 0 {
                return Err("the structure parent tree has an odd number of entries".into());
            }
            let mut greatest = -1_i64;
            for pair in parent_nums.as_chunks::<2>().0 {
                let Some(key) = doc.resolve(&pair[0]).as_int() else {
                    return Err("the structure parent tree has a non-integer key".into());
                };
                if key < 0 {
                    return Err("the structure parent tree has a negative key".into());
                }
                greatest = greatest.max(key);
            }
            let mut first_key = greatest.checked_add(1).ok_or("the structure parent tree has no available key")?;
            let mut has_next_key = false;
            if let Some(next_obj) = structure.get(b"ParentTreeNextKey") {
                let resolved_next = doc.resolve(next_obj);
                if !matches!(*resolved_next, Object::Null) {
                    let next_key = resolved_next.as_int().ok_or("the structure parent tree next key is invalid")?;
                    if next_key < 0 {
                        return Err("the structure parent tree next key is negative".into());
                    }
                    first_key = first_key.max(next_key);
                    has_next_key = true;
                }
            }
            first_key.checked_add(count_delta).ok_or("too many form widgets")?;
            (struct_root, document_ref, existing_kids, parent_tree, parent_nums, first_key, has_next_key, false)
        } else {
            let mut root_dict = Dict::new();
            root_dict.set(b"Type".to_vec(), Object::Name(b"StructTreeRoot".to_vec()));
            let struct_root = doc.add(Object::Dict(root_dict));
            let mut document = Dict::new();
            document.set(b"S".to_vec(), Object::Name(b"Document".to_vec()));
            document.set(b"P".to_vec(), Object::Ref(struct_root));
            let document_ref = doc.add(Object::Dict(document));
            let parent_tree = doc.add(Object::Dict(Dict::new()));
            (struct_root, document_ref, Vec::new(), parent_tree, Vec::new(), 0, false, true)
        };
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
        existing_kids.push(Object::Ref(form_ref));
    }
    doc.update_dict(document_ref, |d| {
        d.set(b"K".to_vec(), Object::Array(existing_kids));
    })
    .map_err(|e| e.to_string())?;
    doc.update_dict(parent_tree, |d| d.set(b"Nums".to_vec(), Object::Array(parent_nums))).map_err(|e| e.to_string())?;
    let next_key = first_key.checked_add(count_delta).and_then(|last| last.checked_add(1));
    if new_tree {
        doc.update_dict(struct_root, |d| {
            d.set(b"K".to_vec(), Object::Ref(document_ref));
            d.set(b"ParentTree".to_vec(), Object::Ref(parent_tree));
            if let Some(next_key) = next_key {
                d.set(b"ParentTreeNextKey".to_vec(), Object::Int(next_key));
            }
        })
        .map_err(|e| e.to_string())?;
    } else if has_next_key && let Some(next_key) = next_key {
        doc.update_dict(struct_root, |d| {
            d.set(b"ParentTreeNextKey".to_vec(), Object::Int(next_key));
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tags;

    fn pdf(bodies: &[String]) -> Document {
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
        out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    #[test]
    fn extends_existing_tree_with_array_root_k_indirect_document_kids_and_indirect_nums_keys() {
        let mut doc = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] /StructParents 0 >>".into(),
            "<< /Length 0 >>\nstream\n\nendstream".into(),
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] /StructParent null >>".into(),
            "<< /Type /StructTreeRoot /K [7 0 R] /ParentTree 8 0 R /ParentTreeNextKey 5 >>".into(),
            "<< /S /Document /P 6 0 R /K 10 0 R >>".into(),
            "<< /Nums [11 0 R [9 0 R]] >>".into(),
            "<< /S /P /P 7 0 R /Pg 3 0 R /K 0 >>".into(),
            "[9 0 R]".into(),
            "0".into(),
        ]);
        assert_eq!(tag_form_fields(&mut doc).unwrap(), 1);
        // Existing paragraph (9 0 R) and new Form tag remain flat siblings under /Document (7 0 R).
        let doc_k = doc.get(ObjRef::new(7, 0)).as_dict().unwrap().get(b"K").unwrap().as_array().unwrap().clone();
        assert_eq!(doc_k.len(), 2);
        assert_eq!(doc_k[0].as_ref(), Some(ObjRef::new(9, 0)));
        let listed: Vec<_> = tags(&doc).into_iter().map(|t| t.kind).collect();
        assert_eq!(listed, ["Document", "P", "Form"]);
        // Widget got key 5 from /ParentTreeNextKey, and /ParentTreeNextKey advanced to 6.
        assert_eq!(doc.get(ObjRef::new(5, 0)).as_dict().unwrap().int(b"StructParent"), Some(5));
        assert_eq!(doc.get(ObjRef::new(6, 0)).as_dict().unwrap().int(b"ParentTreeNextKey"), Some(6));
    }
}
