//! Named destinations (ISO 32000-2 §12.3.2.3): the catalog's `/Names /Dests`
//! name tree and the older `/Dests` dictionary, edited as one list.
//!
//! A name maps to a destination — in this application a page and the point on
//! it, the same shape a bookmark's `/XYZ` destination has. Reading follows the
//! specification's lookup order: the name tree first, then the `/Dests`
//! dictionary. Writing puts the entry in the name tree and removes the name
//! from the dictionary, so a name resolves to exactly one destination whichever
//! way a reader looks it up.
//!
//! The tree is rewritten as one leaf, with its keys in ascending byte order as
//! §7.9.6 requires. Entries whose key is not valid UTF-8 are kept: they travel
//! in the rewritten leaf untouched, and they are not listed, because the room
//! addresses a name as text.
//!
//! Nothing else in `/Names` is touched — JavaScript, embedded files and the
//! other trees keep their objects — and a name's entry is only ever replaced by
//! an edit that names it.

use std::collections::BTreeMap;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::OrganizeError;
use crate::outline::{BookmarkDest, Targets, explicit_dest, name_tree, page_of_explicit, view_of_explicit};

/// Most named destinations a document may hold. The room enforces the same
/// number on the wire, so a document that already has more can still be opened
/// and read; it just cannot gain one.
pub const MAX_NAMED_DESTINATIONS: usize = 10_000;

type Result<T> = std::result::Result<T, OrganizeError>;

/// A named destination as a listing shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedDestination {
    /// The name, as text. Only names that are valid UTF-8 are listed.
    pub name: String,
    /// The 0-based page it opens, when that page is in this document.
    pub page: Option<usize>,
    /// Where on the page: the destination's type and the numbers after it, in
    /// PDF user space (`None` for `null` or anything that is not a number).
    pub view: Option<(Vec<u8>, Vec<Option<f64>>)>,
}

/// Every named destination, sorted by name (the name tree's own order). A name
/// held by both the tree and the dictionary is listed once, from the tree —
/// the entry a reader resolves.
pub fn named_destinations(doc: &Document) -> Vec<NamedDestination> {
    let mut entries: BTreeMap<Vec<u8>, Object> = name_tree(doc).into_iter().collect();
    if let Some(legacy) = legacy_dests(doc) {
        for (key, value) in legacy {
            entries.entry(key).or_insert(value);
        }
    }
    let mut targets = Targets::new(doc);
    entries
        .into_iter()
        .filter_map(|(key, value)| {
            let name = String::from_utf8(key).ok()?;
            let dest = explicit_dest(doc, &mut targets, &value, 0);
            Some(describe(doc, &targets, name, dest.as_deref()))
        })
        .collect()
}

/// The named destination `name`, if this document defines one. The name is
/// matched as its UTF-8 bytes, which is how [`set_named_destination`] writes it.
pub fn named_destination(doc: &Document, name: &str) -> Option<NamedDestination> {
    let mut targets = Targets::new(doc);
    let value = targets.named(doc, name.as_bytes())?;
    let dest = explicit_dest(doc, &mut targets, &value, 0);
    Some(describe(doc, &targets, name.to_owned(), dest.as_deref()))
}

fn describe(doc: &Document, targets: &Targets, name: String, dest: Option<&[Object]>) -> NamedDestination {
    let page = dest.and_then(|dest| page_of_explicit(targets, dest));
    let view = dest.and_then(|dest| view_of_explicit(doc, dest));
    NamedDestination { name, page, view }
}

/// Point the named destination `name` at `dest`: a page and a point
/// ([`BookmarkDest::Page`], as `/XYZ` with the zoom kept) or a page with the view
/// type the file already had ([`BookmarkDest::View`], which an undo puts back as
/// it was). A name that is blank, a destination that does not name a page, a page
/// this document lacks, a coordinate that is not finite and a document already at
/// [`MAX_NAMED_DESTINATIONS`] are all refused before anything is written.
pub fn set_named_destination(doc: &mut Document, name: &str, dest: &BookmarkDest) -> Result<()> {
    let name = check_name(name)?;
    let dest = crate::outline::dest_object(doc, dest)?.ok_or_else(|| OrganizeError::Invalid("a named destination must name a page".into()))?;
    let mut entries = entries(doc);
    if entries.len() >= MAX_NAMED_DESTINATIONS && !entries.contains_key(name) {
        return Err(OrganizeError::Invalid(format!("a document may hold at most {MAX_NAMED_DESTINATIONS} named destinations")));
    }
    entries.insert(name.to_vec(), dest);
    // The tree shadows the dictionary; dropping the dictionary's entry of the
    // same name keeps one destination per name whichever way it is looked up.
    remove_legacy(doc, name)?;
    write_entries(doc, &entries)
}

/// Remove the named destination `name`. A name this document does not define is
/// refused rather than silently ignored, so an undo cannot claim to have
/// removed something that was not there.
pub fn delete_named_destination(doc: &mut Document, name: &str) -> Result<()> {
    let name = check_name(name)?;
    let mut entries = entries(doc);
    let from_tree = entries.remove(name).is_some();
    let from_legacy = remove_legacy(doc, name)?;
    if !from_tree && !from_legacy {
        return Err(OrganizeError::Invalid(format!("there is no named destination {:?}", String::from_utf8_lossy(name))));
    }
    write_entries(doc, &entries)
}

/// A name must be text that a reader can address: not blank, and not longer
/// than the wire allows (the room checks the same bound).
fn check_name(name: &str) -> Result<&[u8]> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(OrganizeError::Invalid("a named destination needs a name".into()));
    }
    Ok(trimmed.as_bytes())
}

/// The entries to write back: the tree's, then the legacy dictionary's that the
/// tree does not already shadow.
fn entries(doc: &Document) -> BTreeMap<Vec<u8>, Object> {
    let mut entries: BTreeMap<Vec<u8>, Object> = name_tree(doc).into_iter().collect();
    if let Some(legacy) = legacy_dests(doc) {
        for (key, value) in legacy {
            entries.entry(key).or_insert(value);
        }
    }
    entries
}

/// The catalog's `/Dests` dictionary, when it is one. Entries of the name tree
/// take precedence over these.
fn legacy_dests(doc: &Document) -> Option<Vec<(Vec<u8>, Object)>> {
    let root = doc.root()?;
    let dests = doc.get(root).as_dict().and_then(|c| c.get(b"Dests").cloned())?;
    let dests = doc.resolve(&dests);
    let dict = dests.as_dict()?;
    Some(dict.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
}

/// Drop `name` from the catalog's `/Dests` dictionary (an inline one, or a
/// reference to one). Returns whether it was there.
fn remove_legacy(doc: &mut Document, name: &[u8]) -> Result<bool> {
    let Some(root) = doc.root() else { return Ok(false) };
    let Some(slot) = doc.get(root).as_dict().and_then(|c| c.get(b"Dests").cloned()) else { return Ok(false) };
    match slot {
        Object::Ref(r) => {
            let Some(dict) = doc.get(r).as_dict().cloned() else { return Ok(false) };
            if !dict.get(name).is_some() {
                return Ok(false);
            }
            doc.update_dict(r, |d| {
                d.remove(name);
            })?;
            Ok(true)
        }
        Object::Dict(mut dict) => {
            if !dict.get(name).is_some() {
                return Ok(false);
            }
            dict.remove(name);
            doc.update_dict(root, |c| c.set(b"Dests".to_vec(), Object::Dict(dict)))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Write `entries` as the catalog's `/Names /Dests` name tree, one leaf with
/// its keys in ascending byte order (§7.9.6). An empty list removes the slot
/// rather than leaving an empty tree behind; every other tree in `/Names` stays.
fn write_entries(doc: &mut Document, entries: &BTreeMap<Vec<u8>, Object>) -> Result<()> {
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    if entries.is_empty() {
        return remove_slot(doc, root);
    }
    let mut pairs = Vec::with_capacity(entries.len().saturating_mul(2));
    for (key, value) in entries {
        pairs.push(Object::String(PdfString::literal(key.clone())));
        pairs.push(value.clone());
    }
    let mut leaf = Dict::new();
    leaf.set(b"Names".to_vec(), Object::Array(pairs));
    let tree = doc.add(Object::Dict(leaf));
    let names = doc.get(root).as_dict().and_then(|c| c.get(b"Names").cloned());
    match names {
        Some(Object::Ref(n)) if doc.get(n).as_dict().is_some() => {
            doc.update_dict(n, |d| d.set(b"Dests".to_vec(), Object::Ref(tree)))?;
        }
        Some(Object::Dict(mut dict)) => {
            dict.set(b"Dests".to_vec(), Object::Ref(tree));
            doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(dict)))?;
        }
        _ => {
            let mut dict = Dict::new();
            dict.set(b"Dests".to_vec(), Object::Ref(tree));
            doc.update_dict(root, |c| c.set(b"Names".to_vec(), Object::Dict(dict)))?;
        }
    }
    Ok(())
}

/// Remove `/Names /Dests` (and `/Names` itself when nothing else is left in it).
fn remove_slot(doc: &mut Document, root: ObjRef) -> Result<()> {
    match doc.get(root).as_dict().and_then(|c| c.get(b"Names").cloned()) {
        Some(Object::Ref(n)) if doc.get(n).as_dict().is_some() => {
            let empty = doc.get(n).as_dict().is_some_and(|d| d.iter().all(|(key, _)| key.as_slice() == b"Dests") && d.get(b"Dests").is_some());
            if empty {
                doc.update_dict(root, |c| {
                    c.remove(b"Names");
                })?;
            } else {
                doc.update_dict(n, |d| {
                    d.remove(b"Dests");
                })?;
            }
            Ok(())
        }
        Some(Object::Dict(mut dict)) => {
            dict.remove(b"Dests");
            doc.update_dict(root, |c| {
                if dict.is_empty() {
                    c.remove(b"Names");
                } else {
                    c.set(b"Names".to_vec(), Object::Dict(dict));
                }
            })?;
            Ok(())
        }
        _ => Ok(()),
    }
}
