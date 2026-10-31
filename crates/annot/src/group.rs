//! Annotation groups (ISO 32000-2 §12.5.6.2): a primary and the subordinates whose `/IRT` leads
//! to it with `/RT /Group`. "Operations that manipulate any annotation in a group, such as
//! movement, cut, and copy, shall be treated by interactive PDF processors as acting on the entire
//! group", so moving, restyling or deleting a member goes through [`group_members`].
//!
//! A group is one page's, as Acrobat reads it: a subordinate whose primary is on another page is
//! a comment of its own. The standard doesn't limit a group to a page, but Acrobat (desktop and
//! iPad) and PDF-XChange all delete a primary without its subordinates on other pages, and
//! Acrobat desktop lists such a subordinate on its own. Keep in step with
//! `pdfcraft_render::link_replies` and `pdfcraft_annot::summaries`.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::AnnotError;
use crate::info::{MAX_GROUP_DEPTH, final_ref, is_group_subordinate};
use crate::lookup::{annots, page_ref};

/// Every member of the group the annotation at `(page, index)` belongs to, as `(page, index)`
/// positions on that page: the primary first, then its subordinates (and theirs) in `/Annots`
/// order. An annotation in no group is its own group of one. The primary is found up the `/IRT`
/// chain while it leads to another annotation on the same page; a loop, or an `/IRT` to an
/// annotation on another page or to nothing listed, ends it there. A chain still on a
/// subordinate after [`MAX_GROUP_DEPTH`] annotations is refused: acting on part of a group would
/// split it.
pub fn group_members(doc: &Document, page: usize, index: usize) -> Result<Vec<(usize, usize)>, AnnotError> {
    let list: Vec<Object> = annots(doc, page_ref(doc, page)?);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    // An inline annotation has no reference for an `/IRT` to name, and none of its own to follow
    // from unless it is a subordinate; an inline subordinate is found from its primary below.
    let start = match entry.as_ref() {
        Some(r) => final_ref(doc, r),
        None if entry.as_dict().is_some_and(is_group_subordinate) => match entry.as_dict().and_then(|d| d.reference(b"IRT")) {
            Some(irt) => final_ref(doc, irt),
            None => return Ok(vec![(page, index)]),
        },
        None => return Ok(vec![(page, index)]),
    };
    // Where each referenced annotation is first listed on the page.
    let mut slots: HashMap<ObjRef, usize> = HashMap::new();
    // The subordinates on the page hanging off each annotation by `/IRT`, in `/Annots` order.
    let mut under: HashMap<ObjRef, Vec<(usize, Option<ObjRef>)>> = HashMap::new();
    for (i, e) in list.iter().enumerate() {
        let own = e.as_ref().map(|r| final_ref(doc, r));
        if let Some(r) = own {
            slots.entry(r).or_insert(i);
        }
        let obj = doc.resolve(e);
        if let Some(irt) = obj.as_dict().filter(|d| is_group_subordinate(d)).and_then(|d| d.reference(b"IRT")) {
            under.entry(final_ref(doc, irt)).or_default().push((i, own));
        }
    }
    if !slots.contains_key(&start) {
        return Ok(vec![(page, index)]);
    }
    let mut primary = start;
    let mut seen = HashSet::from([start]);
    let mut found = false;
    for _ in 0..MAX_GROUP_DEPTH {
        let obj = doc.get(primary);
        let next = obj.as_dict().filter(|d| is_group_subordinate(d)).and_then(|d| d.reference(b"IRT")).map(|r| final_ref(doc, r));
        match next {
            Some(n) if slots.contains_key(&n) && seen.insert(n) => primary = n,
            _ => {
                found = true;
                break;
            }
        }
    }
    if !found {
        return Err(AnnotError::Invalid(format!(
            "this comment's group can't be found: its primary is more than {MAX_GROUP_DEPTH} comments up its chain"
        )));
    }
    let Some(first) = slots.get(&primary).copied() else { return Ok(vec![(page, index)]) };
    let mut members = vec![first];
    let mut placed = HashSet::from([first]);
    let mut visited = HashSet::from([primary]);
    let mut todo = vec![primary];
    while let Some(r) = todo.pop() {
        for &(i, own) in under.get(&r).into_iter().flatten() {
            match own {
                Some(o) if !visited.insert(o) => continue,
                Some(o) => todo.push(o),
                None => {}
            }
            if placed.insert(i) {
                members.push(i);
            }
        }
    }
    if let [_, rest @ ..] = members.as_mut_slice() {
        rest.sort_unstable();
    }
    Ok(members.into_iter().map(|i| (page, i)).collect())
}
