//! The structural parent tree, the way from a content item back to the structure element that
//! owns it (ISO 32000-2 §14.7.5.4). A page or form XObject with marked content names its entry
//! with /StructParents, whose value lists the element of each marked-content identifier (MCID); an
//! annotation or other object that is a content item in its own right names its entry with
//! /StructParent, whose value is its element.
//!
//! Content whose entry is missing, or whose entry names no element of the structure hierarchy,
//! or names one that doesn't hold it among its kids (/K), belongs to no element: it carries a tag
//! in the content stream that no tag in the tree answers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// Number-tree nodes read at most, kids nested at most this deep, and entries kept at most: a
/// tree past any of them is read up to there.
const MAX_NODES: usize = 1 << 16;
const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 1 << 22;

/// An element's parents are followed at most this many steps up to the structure tree root.
const MAX_ANCESTORS: usize = 256;

/// An element's kids (/K) are read at most this many: an item not among them may be in the part
/// not read.
const MAX_KIDS: usize = 1 << 16;

/// Kids kept at most across all the elements [`Holdings`] remembers: past it, what another element
/// holds is not read and its items are left undecided.
const MAX_KEPT: usize = 1 << 22;

/// Where a marked-content sequence is: the page it is drawn on, and the form XObject whose
/// content stream holds it when that is not the page's own (ISO 32000-2 Table 357).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Place {
    pub page: ObjRef,
    pub stm: Option<ObjRef>,
}

/// The content items an element holds directly among its kids (/K, ISO 32000-2 Table 355): marked
/// content by MCID and where it is, and objects by reference (an /OBJR's /Obj, Table 358).
#[derive(Debug, Default)]
struct Held {
    /// (MCID, page, stream): the page for a sequence in a page's own content stream, the stream
    /// (and no page) for one in another.
    mcids: HashSet<(i64, Option<ObjRef>, Option<ObjRef>)>,
    objs: HashSet<ObjRef>,
    /// Whether the kids were read only up to [`MAX_KIDS`].
    truncated: bool,
}

impl Held {
    /// What element `e` holds. A kid that is a structure element holds content of its own, not
    /// `e`'s: the parent tree names the element a content item is a kid of (§14.7.5.4), so only
    /// `e`'s own kids count and none is descended into.
    fn read(doc: &Document, e: &Dict) -> Held {
        let mut held = Held::default();
        // An integer kid is on the element's page; a marked-content reference may name its own.
        let pg = e.get(b"Pg").and_then(Object::as_ref);
        let Some(k) = e.get(b"K") else { return held };
        let k = doc.resolve(k);
        let kids: &[Object] = match k.as_array() {
            Some(kids) => kids,
            None => std::slice::from_ref(&*k),
        };
        if kids.len() > MAX_KIDS {
            held.truncated = true;
        }
        for kid in kids.iter().take(MAX_KIDS) {
            match &*doc.resolve(kid) {
                Object::Int(mcid) => {
                    held.mcids.insert((*mcid, pg, None));
                }
                Object::Dict(d) => {
                    if let Some(mcid) = d.get(b"MCID").and_then(|m| doc.resolve(m).as_int()) {
                        // A sequence in another content stream is found by that stream (/Stm);
                        // one in a page's own by the page: the reference's /Pg, or the element's.
                        let at = match d.get(b"Stm").and_then(Object::as_ref) {
                            Some(stm) => (mcid, None, Some(stm)),
                            None => (mcid, d.get(b"Pg").and_then(Object::as_ref).or(pg), None),
                        };
                        held.mcids.insert(at);
                    } else if let Some(obj) = d.get(b"Obj").and_then(Object::as_ref) {
                        held.objs.insert(obj);
                    }
                }
                _ => {}
            }
        }
        held
    }

    fn len(&self) -> usize {
        self.mcids.len().saturating_add(self.objs.len())
    }

    /// Yes when the item looked for was `found`; otherwise no, or not known when the kids were
    /// read only in part.
    fn answer(&self, found: bool) -> Option<bool> {
        if found {
            Some(true)
        } else if self.truncated {
            None
        } else {
            Some(false)
        }
    }
}

/// What each element holds, read once per element: the items of a page, or a document's
/// annotations, often share one element with many kids.
#[derive(Debug, Default)]
pub(crate) struct Holdings {
    by_element: HashMap<ObjRef, Held>,
    kept: usize,
}

impl Holdings {
    /// Whether element `e` (the object `at` refers to, when given by reference) holds what `has`
    /// looks for; `None` when that is not known (kids read only in part, or past [`MAX_KEPT`]).
    fn holds(&mut self, doc: &Document, at: Option<ObjRef>, e: &Object, has: impl Fn(&Held) -> bool) -> Option<bool> {
        let Some(d) = e.as_dict() else { return Some(false) };
        let Some(r) = at else {
            let held = Held::read(doc, d);
            return held.answer(has(&held));
        };
        if let Some(held) = self.by_element.get(&r) {
            return held.answer(has(held));
        }
        if self.kept >= MAX_KEPT {
            return None;
        }
        let held = Held::read(doc, d);
        let answer = held.answer(has(&held));
        self.kept = self.kept.saturating_add(held.len());
        self.by_element.insert(r, held);
        answer
    }

    /// Whether element `e` (the object `at` refers to, when given by reference) holds object `obj`
    /// among its kids through an object reference (/OBJR, ISO 32000-2 §14.7.5.3, Table 358).
    /// `None` when that is not known.
    pub fn holds_object(&mut self, doc: &Document, at: Option<ObjRef>, e: &Object, obj: ObjRef) -> Option<bool> {
        self.holds(doc, at, e, |h| h.objs.contains(&obj))
    }
}

/// Both answers together: no when either is no, yes when both are yes, else not known. `b` is
/// asked only when `a` is not no.
pub(crate) fn both(a: Option<bool>, b: impl FnOnce() -> Option<bool>) -> Option<bool> {
    if a == Some(false) {
        return Some(false);
    }
    match (a, b()) {
        (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

/// The parent tree's entries by key, as read from the number tree: the first value given for a
/// key is the one kept.
#[derive(Debug, Default)]
pub(crate) struct ParentTree {
    entries: HashMap<i64, Object>,
    /// The structure tree root, when the catalog names it by reference.
    root: Option<ObjRef>,
    /// Whether the number tree was read only up to one of the limits: an entry not found may then
    /// be in the part not read.
    truncated: bool,
}

impl ParentTree {
    /// The parent tree of `doc`'s structure tree; empty when it has none.
    pub fn read(doc: &Document) -> ParentTree {
        let root_ref = doc.root().and_then(|c| doc.get(c).as_dict().and_then(|c| c.get(b"StructTreeRoot")).and_then(Object::as_ref));
        let mut tree = ParentTree { root: root_ref, ..ParentTree::default() };
        let Some(root) = crate::alt::tree_root(doc) else { return tree };
        let Some(top) = root.get(b"ParentTree") else { return tree };
        let mut seen: HashSet<ObjRef> = HashSet::new();
        let mut nodes = 0usize;
        let mut stack: Vec<(Object, usize)> = vec![(top.clone(), 0)];
        while let Some((node, depth)) = stack.pop() {
            if let Object::Ref(r) = &node
                && !seen.insert(*r)
            {
                continue;
            }
            nodes += 1;
            if nodes > MAX_NODES {
                tree.truncated = true;
                break;
            }
            let node = doc.resolve(&node);
            let Some(d) = node.as_dict() else { continue };
            if let Some(nums) = d.get(b"Nums").map(|n| doc.resolve(n)) {
                for [key, value] in nums.as_array().map(Vec::as_slice).unwrap_or_default().as_chunks::<2>().0 {
                    let Some(key) = doc.resolve(key).as_int() else { continue };
                    if tree.entries.len() >= MAX_ENTRIES && !tree.entries.contains_key(&key) {
                        tree.truncated = true;
                        break;
                    }
                    tree.entries.entry(key).or_insert_with(|| value.clone());
                }
            }
            if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k))
                && let Some(kids) = kids.as_array()
                && !kids.is_empty()
            {
                if depth < MAX_DEPTH {
                    // Pushed in reverse, so the kids are read in order and an earlier key wins.
                    stack.extend(kids.iter().rev().map(|k| (k.clone(), depth + 1)));
                } else {
                    tree.truncated = true;
                }
            }
        }
        tree
    }

    /// Whether entry `key` may be in the part of a truncated number tree that was not read.
    fn unread(&self, key: i64) -> bool {
        self.truncated && !self.entries.contains_key(&key)
    }

    /// What entry `key` names for a content item that is an object (an annotation's
    /// /StructParent): the reference, when the entry is one, and the value resolved, before any
    /// check that it is a structure element. `Some(None)` for no entry, and `None` when the entry
    /// may be in the part of a truncated tree not read.
    pub fn entry(&self, doc: &Document, key: i64) -> Option<Option<(Option<ObjRef>, Arc<Object>)>> {
        if self.unread(key) {
            return None;
        }
        Some(self.entries.get(&key).map(|v| (v.as_ref(), doc.resolve(v))))
    }

    /// Whether marked content `mcid` of the content stream whose /StructParents is `key`, at
    /// `place`, belongs to a structure element: the entry's array names one at index `mcid`, and
    /// that element holds the sequence among its kids (ISO 32000-2 §14.7.5.2, §14.7.5.4): as an
    /// integer, for one in the content stream of the element's page (/Pg), or as a marked-content
    /// reference (Table 357) naming that page, or the form XObject's stream (/Stm) for one in a
    /// form's own. `None` when that is not known: the entry may be in the part of a truncated tree
    /// not read, the element's kids were read only in part, or its parents go on past the
    /// ancestor limit. An MCID no array has a slot for (a negative one) is owned by nothing either
    /// way.
    pub fn owns(&self, doc: &Document, holdings: &mut Holdings, key: i64, mcid: i64, place: Place) -> Option<bool> {
        let Ok(at) = usize::try_from(mcid) else { return Some(false) };
        if self.unread(key) {
            return None;
        }
        let Some(list) = self.entries.get(&key).map(|v| doc.resolve(v)) else { return Some(false) };
        let Some(slot) = list.as_array().and_then(|items| items.get(at)) else { return Some(false) };
        let e = doc.resolve(slot);
        let item = match place.stm {
            Some(stm) => (mcid, None, Some(stm)),
            None => (mcid, Some(place.page), None),
        };
        both(holdings.holds(doc, slot.as_ref(), &e, |h| h.mcids.contains(&item)), || self.in_hierarchy(doc, &e))
    }

    /// Whether `e` is a structure element of the structure hierarchy: a dictionary with a type
    /// (/S) whose parents (/P, required: ISO 32000-2 §14.7.2, Table 355) are structure elements up
    /// to the structure tree root. One without a parent, or whose parents end elsewhere (in a
    /// dictionary that is neither), or go round, is in no tree. `None` when the parents go on past
    /// [`MAX_ANCESTORS`] steps: the walk stopped before it could tell.
    pub fn in_hierarchy(&self, doc: &Document, e: &Object) -> Option<bool> {
        // The type may be given by reference, as any value may (ISO 32000-2 §7.3.10).
        if !e.as_dict().is_some_and(|d| d.get(b"S").is_some_and(|s| matches!(*doc.resolve(s), Object::Name(_)))) {
            return Some(false);
        }
        let mut seen: HashSet<ObjRef> = HashSet::new();
        let mut node: Option<Arc<Object>> = None;
        for _ in 0..MAX_ANCESTORS {
            let current = node.as_deref().unwrap_or(e);
            let Some(parent) = current.as_dict().and_then(|d| d.get(b"P")) else { return Some(false) };
            if let Some(r) = parent.as_ref() {
                if Some(r) == self.root {
                    return Some(true);
                }
                if !seen.insert(r) {
                    return Some(false);
                }
            }
            let parent = doc.resolve(parent);
            let Some(d) = parent.as_dict() else { return Some(false) };
            // The root is the one the catalog names (ISO 32000-2 §14.7.2): another, as a merged
            // file may keep from a source document, roots no tree of this one. Only when the
            // catalog gives its root directly, with no reference to compare, does the type tell.
            if self.root.is_none() && d.name(b"Type") == Some(b"StructTreeRoot") {
                return Some(true);
            }
            // A dictionary without a structure type is no structure element; one whose type is
            // written as a string rather than a name, as some producers write it, still is. A
            // null or any other value is no type.
            if !d.get(b"S").is_some_and(|s| matches!(*doc.resolve(s), Object::Name(_) | Object::String(_))) {
                return Some(false);
            }
            node = Some(parent);
        }
        None
    }
}
