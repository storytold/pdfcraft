//! Preseparated pages (ISO 32000-1 §14.11.4): each plate of a page is a page object of its own,
//! and its `/SeparationInfo /Pages` lists every plate of that page, the same list on each. Page
//! operations keep that list to pages of the same document that list it back: a deleted plate
//! leaves the others' lists, and a copied plate lists only the copies made with it. Membership is
//! decided by page identity; colorant names are never compared.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::OrganizeError;

/// After the `gone` pages leave `doc`, the separation lists of the pages that stay drop them.
/// A list that named none of them is left as it is, and so is one that is not an array.
pub(crate) fn drop_deleted_plates(doc: &mut Document, kept: &[ObjRef], gone: &[ObjRef]) -> Result<(), OrganizeError> {
    let gone: HashSet<ObjRef> = gone.iter().copied().collect();
    for page in kept {
        let Some(stored) = doc.get(*page).as_dict().and_then(|d| d.get(b"SeparationInfo").cloned()) else { continue };
        let Some(mut info) = doc.dict(&stored) else { continue };
        let Some(list) = info.get(b"Pages").and_then(|p| doc.resolve(p).as_array().cloned()) else { continue };
        if !list.iter().any(|e| e.as_ref().is_some_and(|r| gone.contains(&r))) {
            continue;
        }
        let list: Vec<Object> = list.into_iter().filter(|e| e.as_ref().is_none_or(|r| !gone.contains(&r))).collect();
        info.set(b"Pages".to_vec(), Object::Array(list));
        // Plates sharing one dictionary are all fixed by this one write; the next finds nothing gone.
        match stored.as_ref() {
            Some(shared) => doc.set(shared, Object::Dict(info)),
            None => {
                doc.update_dict(*page, |d| d.set(b"SeparationInfo".to_vec(), Object::Dict(info)))?;
            }
        }
    }
    Ok(())
}

/// The copy of a page's separation dictionary `info` (read from `src`) for its copy `this`: its
/// list names the copies of the plates copied with it (`pages`, source page to copy), in the
/// list's order, and no page left behind. A `repeat` (a second copy of a page) is a set of its
/// own, since the other copies don't list it. Every other entry goes through `copy`. `None` when
/// `info` is not a dictionary.
pub(crate) fn copy_info(
    src: &Document,
    info: &Object,
    pages: &HashMap<ObjRef, ObjRef>,
    this: ObjRef,
    repeat: bool,
    copy: &mut dyn FnMut(&Object) -> Result<Object, OrganizeError>,
) -> Result<Option<Object>, OrganizeError> {
    let Some(info) = src.dict(info) else { return Ok(None) };
    let mut out = Dict::new();
    for (k, v) in info.iter() {
        if k.as_slice() != b"Pages" {
            out.set(k.clone(), copy(v)?);
            continue;
        }
        let copies: Vec<ObjRef> = if repeat {
            vec![this]
        } else {
            let mut seen = HashSet::new();
            let list = src.resolve(v).as_array().cloned().unwrap_or_default();
            list.iter().filter_map(|e| e.as_ref()).filter_map(|r| pages.get(&r).copied()).filter(|c| seen.insert(*c)).collect()
        };
        out.set(k.clone(), Object::Array(copies.into_iter().map(Object::Ref).collect()));
    }
    Ok(Some(Object::Dict(out)))
}
