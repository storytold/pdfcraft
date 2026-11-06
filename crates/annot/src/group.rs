//! Annotation groups (ISO 32000-2 §12.5.6.2): a primary and the subordinates whose `/IRT` leads
//! to it with `/RT /Group`. "Operations that manipulate any annotation in a group, such as
//! movement, cut, and copy, shall be treated by interactive PDF processors as acting on the entire
//! group". PdfCraft follows Acrobat desktop instead, where the two differ: selecting any member
//! selects the group, and colour, opacity, width and fill restyle the whole group through
//! [`group_members`]; moving, resizing and deleting act only on the member asked for, the primary
//! included (`move_annotation`, `set_rect`, `set_points` and `delete_annotation` in `modify.rs`).
//! Replace Text's strikeout and caret are deleted together.
//!
//! A group is one page's, as Acrobat reads it: a subordinate whose primary is on another page is
//! a comment of its own. The standard doesn't limit a group to a page, but Acrobat (desktop and
//! iPad) and PDF-XChange all delete a primary without its subordinates on other pages, and
//! Acrobat desktop lists such a subordinate on its own. Keep in step with
//! `pdfcraft_render::link_replies`, `pdfcraft_render::group_primaries` and
//! `pdfcraft_annot::summaries`: a restyle reaches exactly the members the comment list shows.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::AnnotError;
use crate::info::{MAX_GROUP_DEPTH, final_ref, is_comment_subtype, is_group_subordinate};
use crate::lookup::{annots, page_ref};

/// Whether `d` is listed as a comment (or a reply, or a group member), as the comment list reads
/// it: links, form widgets, pop-ups and other non-markup annotations are not, so an `/IRT` to one
/// of them leads nowhere.
fn listed(d: &Dict) -> bool {
    d.name(b"Subtype").is_some_and(|s| is_comment_subtype(&String::from_utf8_lossy(s)))
}

/// Every member of the group the annotation at `(page, index)` belongs to, as `(page, index)`
/// positions on that page: the primary first, then its subordinates (and theirs) in `/Annots`
/// order. An annotation in no group is its own group of one.
///
/// The group is the one the comment list shows (`pdfcraft_render::link_replies` and
/// `group_primaries`), so a restyle never reaches an annotation listed on its own:
/// - The `/IRT` chain is followed up through the comments on the page, whatever their `/RT`. An
///   `/IRT` to the annotation itself, to one on another page, to a link, widget or pop-up, or to
///   nothing listed ends it there: that annotation has no primary above it.
/// - The primary is the first annotation up the chain that is not a subordinate (`/RT /Group`
///   with somewhere to lead).
/// - A chain that loops, or runs past [`MAX_GROUP_DEPTH`] links, leaves the annotation a comment
///   of its own, as the list shows it; so does a primary more than `MAX_GROUP_DEPTH - 1` links up.
/// - Subordinates are collected down from the primary to the same depths, each visited once.
pub fn group_members(doc: &Document, page: usize, index: usize) -> Result<Vec<(usize, usize)>, AnnotError> {
    let list: Vec<Object> = annots(doc, page_ref(doc, page)?);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let alone = vec![(page, index)];
    // Where each referenced comment is first listed on the page, where its own `/IRT` leads, and
    // whether it is a subordinate; and the subordinates hanging off each comment, in `/Annots`
    // order. An inline entry has no reference to be found by, but it can hang off one.
    let mut slots: HashMap<ObjRef, usize> = HashMap::new();
    let mut up: HashMap<ObjRef, (ObjRef, bool)> = HashMap::new();
    let mut under: HashMap<ObjRef, Vec<(usize, Option<ObjRef>)>> = HashMap::new();
    for (i, e) in list.iter().enumerate() {
        let obj = doc.resolve(e);
        let Some(d) = obj.as_dict().filter(|d| listed(d)) else { continue };
        let own = e.as_ref().map(|r| final_ref(doc, r));
        // An `/IRT` to itself is no link (`link_replies` drops it).
        let irt = d.reference(b"IRT").map(|r| final_ref(doc, r)).filter(|t| Some(*t) != own);
        if let Some(r) = own {
            slots.entry(r).or_insert(i);
            if let Some(t) = irt {
                up.entry(r).or_insert((t, is_group_subordinate(d)));
            }
        }
        if let Some(t) = irt.filter(|_| is_group_subordinate(d)) {
            under.entry(t).or_default().push((i, own));
        }
    }
    let obj = doc.resolve(entry);
    let Some(d) = obj.as_dict().filter(|d| listed(d)) else { return Ok(alone) };
    let own = entry.as_ref().map(|r| final_ref(doc, r));
    // The chain up from the entry: each link and whether the annotation it leaves is a
    // subordinate. It settles when it reaches an annotation with no link.
    let mut links: Vec<(ObjRef, bool)> = Vec::new();
    let mut seen: HashSet<ObjRef> = own.into_iter().collect();
    let mut next = d.reference(b"IRT").map(|r| final_ref(doc, r)).filter(|t| Some(*t) != own).map(|t| (t, is_group_subordinate(d)));
    while let Some((n, grouped)) = next.filter(|(n, _)| slots.contains_key(n)) {
        if !seen.insert(n) || links.len() >= MAX_GROUP_DEPTH {
            // A loop, or a chain past the depth limit: the list leaves it a comment of its own.
            return Ok(alone);
        }
        links.push((n, grouped));
        next = up.get(&n).copied();
    }
    // The primary: the first annotation up the chain that is not a subordinate leading on.
    let hops = links.iter().take_while(|(_, grouped)| *grouped).count();
    if hops >= MAX_GROUP_DEPTH {
        return Ok(alone);
    }
    let primary = match hops.checked_sub(1).and_then(|k| links.get(k)) {
        Some(&(p, _)) => p,
        // The entry is the primary. An inline one can't be named by an `/IRT`, so it has no
        // subordinates.
        None => match own {
            Some(r) => r,
            None => return Ok(alone),
        },
    };
    let Some(&first) = slots.get(&primary) else { return Ok(alone) };
    // How far below the primary a subordinate is still listed in its group: within the depth the
    // list follows to a primary, and within the links the primary's own chain leaves.
    let primary_links = links.len().saturating_sub(hops);
    let deepest = (MAX_GROUP_DEPTH - 1).min(MAX_GROUP_DEPTH.saturating_sub(primary_links));
    let mut members = vec![first];
    let mut placed = HashSet::from([first]);
    let mut visited = HashSet::from([primary]);
    let mut todo = vec![(primary, 0usize)];
    while let Some((r, depth)) = todo.pop() {
        if depth >= deepest {
            continue;
        }
        for &(i, own) in under.get(&r).into_iter().flatten() {
            match own {
                Some(o) if !visited.insert(o) => continue,
                Some(o) => todo.push((o, depth + 1)),
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
