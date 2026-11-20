//! The page numbers links name, across page operations.
//!
//! A go-to destination opens a page by its object (`[7 0 R /Fit]`) or by its number in
//! the document (`[1 /Fit]`, ISO 32000-2 §12.3.2.2). The object survives an insert, a
//! move, a split, a delete and a replacement, because those keep the page objects; the
//! number does not: after a move, `1` is another page. PdfCraft itself writes objects,
//! but files from other producers carry numbers, so every operation that reorders the
//! page list has to rewrite them.
//!
//! Only the go-to of a link is rewritten here. A named destination is a name of the
//! document and does not move; a remote go-to's number is a page of *another* file, so
//! page operations of this document must leave it alone; and an imported page (one that
//! was not in the document before the operation) keeps the numbers its own file gave,
//! since they name that file's pages. A number that named a deleted page loses the
//! destination, as a bookmark's does.

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::OrganizeError;

/// What a destination needs after the pages moved.
enum Remap {
    /// The page did not move, or the destination is not a number.
    Keep,
    /// The page it named is gone.
    Drop,
    /// The destination array with its number rewritten.
    Rewritten(Vec<Object>),
}

/// The page objects in document order.
pub(crate) fn page_order(doc: &Document) -> Result<Vec<ObjRef>, OrganizeError> {
    Ok(crate::walk(doc)?.into_iter().map(|(r, _)| r).collect())
}

/// Rewrite the page numbers of links on pages that were in the document before a page
/// operation, so each keeps opening the page it opened. `before` is [`page_order`] taken
/// before the operation. A number that named a deleted page loses the destination; a
/// name, a reference, an action that is not a go-to and a remote go-to are left alone.
pub(crate) fn keep_page_numbers(doc: &mut Document, before: &[ObjRef]) -> Result<(), OrganizeError> {
    let pages = crate::walk(doc)?;
    let current: Vec<ObjRef> = pages.iter().map(|(r, _)| *r).collect();
    if current == before {
        return Ok(());
    }
    for (page, _) in &pages {
        // A page the operation brought in keeps the numbers of its own file: they never
        // named a page of this document, so there is nothing here to rewrite them to.
        if !before.contains(page) {
            continue;
        }
        let annots = doc
            .get(*page)
            .as_dict()
            .and_then(|d| d.get(b"Annots"))
            .map(|annots| doc.resolve(annots).as_array().cloned().unwrap_or_default())
            .unwrap_or_default();
        for annot in annots.iter().filter_map(Object::as_ref) {
            let Some(d) = doc.get(annot).as_dict().cloned() else { continue };
            if d.name(b"Subtype") != Some(b"Link") {
                continue;
            }
            if let Some(dest) = d.get(b"Dest").cloned() {
                match remap(doc, &dest, before, &current)? {
                    Remap::Keep => {}
                    Remap::Drop => doc.update_dict(annot, |d| {
                        d.remove(b"Dest");
                    })?,
                    Remap::Rewritten(array) => doc.update_dict(annot, |d| {
                        d.set(b"Dest".to_vec(), Object::Array(array));
                    })?,
                }
            }
            // A remote go-to (`/S /GoToR`) names a page of another file, and any other
            // action has no page here: only a GoTo's `/D` is this document's numbering.
            let Some(mut action) = d.get(b"A").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()) else { continue };
            if action.name(b"S") != Some(b"GoTo") {
                continue;
            }
            let Some(dest) = action.get(b"D").cloned() else { continue };
            match remap(doc, &dest, before, &current)? {
                Remap::Keep => {}
                Remap::Drop => doc.update_dict(annot, |d| {
                    d.remove(b"A");
                })?,
                Remap::Rewritten(array) => {
                    action.set(b"D".to_vec(), Object::Array(array));
                    doc.update_dict(annot, |d| {
                        d.set(b"A".to_vec(), Object::Dict(action));
                    })?;
                }
            }
        }
    }
    Ok(())
}

/// Rewrite one destination, when it names a page by number.
fn remap(doc: &Document, dest: &Object, before: &[ObjRef], current: &[ObjRef]) -> Result<Remap, OrganizeError> {
    let resolved = doc.resolve(dest);
    let Some(array) = resolved.as_array() else { return Ok(Remap::Keep) };
    let Some(first) = array.first().map(|first| doc.resolve(first)) else { return Ok(Remap::Keep) };
    let Object::Int(number) = &*first else { return Ok(Remap::Keep) };
    let Ok(old) = usize::try_from(*number) else { return Ok(Remap::Keep) };
    // A number the document never had (a broken destination) is not guessed at.
    let Some(target) = before.get(old) else { return Ok(Remap::Keep) };
    let Some(new) = current.iter().position(|page| page == target) else { return Ok(Remap::Drop) };
    if new == old {
        return Ok(Remap::Keep);
    }
    let number = i64::try_from(new).map_err(|_| OrganizeError::Invalid("a page number does not fit the document".into()))?;
    let mut array = array.clone();
    if let Some(slot) = array.first_mut() {
        *slot = Object::Int(number);
    }
    Ok(Remap::Rewritten(array))
}
