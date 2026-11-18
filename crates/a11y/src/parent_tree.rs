//! The structural parent tree, the way from a content item back to the structure element that
//! owns it (ISO 32000-2 §14.7.5.4). A page or form XObject with marked content names its entry
//! with /StructParents, whose value lists the element of each marked-content identifier (MCID); an
//! annotation or other object that is a content item in its own right names its entry with
//! /StructParent, whose value is its element.
//!
//! Content whose entry is missing, or whose entry names no element of the structure hierarchy,
//! belongs to no element: it carries a tag in the content stream that no tag in the tree answers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object};

/// Number-tree nodes read at most, kids nested at most this deep, and entries kept at most: a
/// tree past any of them is read up to there.
const MAX_NODES: usize = 1 << 16;
const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 1 << 22;

/// An element's parents are followed at most this many steps up to the structure tree root.
const MAX_ANCESTORS: usize = 256;

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

    /// Whether the number tree was read only in part, so that a missing entry is not known to be
    /// missing.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Whether entry `key` may be in the part of a truncated number tree that was not read.
    pub fn unread(&self, key: i64) -> bool {
        self.truncated && !self.entries.contains_key(&key)
    }

    /// The structure element that entry `key` names for a content item that is an object (an
    /// annotation's /StructParent): a dictionary with a structure type (/S).
    pub fn element(&self, doc: &Document, key: i64) -> Option<Arc<Object>> {
        self.entries.get(&key).map(|v| doc.resolve(v)).filter(|e| self.in_hierarchy(doc, e))
    }

    /// Whether marked content `mcid` of the content stream whose /StructParents is `key` belongs to
    /// a structure element: the entry's array names one at index `mcid`.
    pub fn owns(&self, doc: &Document, key: i64, mcid: i64) -> bool {
        let Some(list) = self.entries.get(&key).map(|v| doc.resolve(v)) else { return false };
        let Ok(at) = usize::try_from(mcid) else { return false };
        list.as_array().and_then(|items| items.get(at)).is_some_and(|e| self.in_hierarchy(doc, &doc.resolve(e)))
    }

    /// Whether `e` is a structure element of the structure hierarchy: a dictionary with a type
    /// (/S) whose parents (/P, required: ISO 32000-2 §14.7.2, Table 355) are structure elements up
    /// to the structure tree root. One without a parent, or whose parents end elsewhere (in a
    /// dictionary that is neither), or go round, is in no tree.
    fn in_hierarchy(&self, doc: &Document, e: &Object) -> bool {
        if !e.as_dict().is_some_and(|d| d.name(b"S").is_some()) {
            return false;
        }
        let mut seen: HashSet<ObjRef> = HashSet::new();
        let mut node: Option<Arc<Object>> = None;
        for _ in 0..MAX_ANCESTORS {
            let current = node.as_deref().unwrap_or(e);
            let Some(parent) = current.as_dict().and_then(|d| d.get(b"P")) else { return false };
            if let Some(r) = parent.as_ref() {
                if Some(r) == self.root {
                    return true;
                }
                if !seen.insert(r) {
                    return false;
                }
            }
            let parent = doc.resolve(parent);
            let Some(d) = parent.as_dict() else { return false };
            if d.name(b"Type") == Some(b"StructTreeRoot") {
                return true;
            }
            // A dictionary without a structure type is no structure element; one whose type is
            // written as a string rather than a name, as some producers write it, still is.
            if !d.contains(b"S") {
                return false;
            }
            node = Some(parent);
        }
        false
    }
}
