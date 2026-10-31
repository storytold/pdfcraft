//! The structure parent tree (ISO 32000-2 §14.7.5.4): a number tree from the `/StructParents`
//! key of a page (to an array indexed by MCID) and the `/StructParent` key of an annotation or
//! XObject (to one element), back to the structure elements that own the content.
//!
//! Any number-tree layout is read: a flat `/Nums` array, intermediate `/Kids` nodes with
//! `/Limits`, `/Nums` arrays and page arrays stored as indirect objects. Edits change only the
//! entries they are about and keep every other key, value and node as it was.

use std::collections::{BTreeMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How many nodes and how deep the tree is read.
const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 32;
/// The largest MCID a page array is grown to hold.
pub(crate) const MAX_MCID: i64 = 1_000_000;

/// Where one key's value is stored.
#[derive(Clone, Debug)]
struct Slot {
    /// The node whose `/Nums` holds the pair.
    node: ObjRef,
    /// Index of the value in that `/Nums`.
    at: usize,
    value: Object,
}

/// What a key maps to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    /// A page's (or form's) array of elements indexed by MCID.
    Array(Vec<Object>),
    /// One element (an annotation's or XObject's key).
    Single(Object),
}

#[derive(Clone, Debug)]
pub(crate) struct ParentTree {
    root: ObjRef,
    slots: BTreeMap<i64, Slot>,
    duplicates: HashSet<i64>,
    /// Why keys can't be added (a layout the writer doesn't extend).
    fixed: Option<String>,
    /// The next unused key (`/ParentTreeNextKey`, or one past the greatest key).
    pub next_key: i64,
}

fn nums_of(doc: &Document, node: &Dict) -> Option<(Vec<Object>, Option<ObjRef>)> {
    match node.get(b"Nums")? {
        Object::Array(a) => Some((a.clone(), None)),
        Object::Ref(r) => doc.get(*r).as_array().map(|a| (a.clone(), Some(*r))),
        _ => None,
    }
}

impl ParentTree {
    /// Read the tree whose root node is `root`. `next_key` is the structure tree root's
    /// `/ParentTreeNextKey`.
    pub fn read(doc: &Document, root: ObjRef, next_key: Option<i64>) -> ParentTree {
        let mut tree = ParentTree { root, slots: BTreeMap::new(), duplicates: HashSet::new(), fixed: None, next_key: 0 };
        let mut seen = HashSet::new();
        let mut stack = vec![(root, 0usize)];
        while let Some((node, depth)) = stack.pop() {
            if depth > MAX_DEPTH || seen.len() >= MAX_NODES {
                tree.fixed = Some("it is too large or too deep".into());
                continue;
            }
            if !seen.insert(node) {
                tree.fixed = Some("a node is listed twice".into());
                continue;
            }
            let Some(dict) = doc.get(node).as_dict().cloned() else { continue };
            if let Some((nums, _)) = nums_of(doc, &dict) {
                for ([key, value], at) in nums.as_chunks::<2>().0.iter().zip((1usize..).step_by(2)) {
                    let Some(key) = key.as_int() else { continue };
                    if tree.slots.insert(key, Slot { node, at, value: value.clone() }).is_some() {
                        tree.duplicates.insert(key);
                    }
                }
            }
            if let Some(kids) = dict.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
                for kid in kids.iter().rev() {
                    match kid.as_ref() {
                        Some(r) => stack.push((r, depth.saturating_add(1))),
                        None => tree.fixed = Some("a node lists a kid that is not an indirect reference".into()),
                    }
                }
            }
        }
        let past_greatest = tree.slots.keys().next_back().map_or(0, |k| k.saturating_add(1));
        tree.next_key = next_key.filter(|k| *k >= 0).unwrap_or(0).max(past_greatest).max(0);
        tree
    }

    pub fn keys(&self) -> impl Iterator<Item = i64> + '_ {
        self.slots.keys().copied()
    }

    pub fn is_duplicate(&self, key: i64) -> bool {
        self.duplicates.contains(&key)
    }

    /// What `key` maps to.
    pub fn get(&self, doc: &Document, key: i64) -> Option<Value> {
        let slot = self.slots.get(&key)?;
        Some(match &slot.value {
            Object::Array(a) => Value::Array(a.clone()),
            Object::Ref(r) => match &*doc.get(*r) {
                Object::Array(a) => Value::Array(a.clone()),
                _ => Value::Single(slot.value.clone()),
            },
            other => Value::Single(other.clone()),
        })
    }

    /// The element at `mcid` of `key`'s array.
    pub fn element_at(&self, doc: &Document, key: i64, mcid: i64) -> Option<ObjRef> {
        match self.get(doc, key)? {
            Value::Array(a) => usize::try_from(mcid).ok().and_then(|i| a.get(i)).and_then(Object::as_ref),
            Value::Single(_) => None,
        }
    }

    /// Hand out keys past `held` only: a key a page or an annotation holds, which the tree may
    /// not list.
    pub fn reserve(&mut self, held: i64) {
        self.next_key = self.next_key.max(held.saturating_add(1));
    }

    /// Hand out a fresh key.
    pub fn allocate(&mut self) -> Result<i64, String> {
        let key = self.next_key;
        self.next_key = key.checked_add(1).ok_or("the parent tree has no free key left")?;
        Ok(key)
    }

    /// Set entries of `key`'s array (`(mcid, element or null)`), creating the key when it is new.
    pub fn set_array(&mut self, doc: &mut Document, key: i64, changes: &[(i64, Object)]) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        if self.is_duplicate(key) {
            return Err(format!("key {key} is listed twice"));
        }
        let place = |array: &mut Vec<Object>| -> Result<(), String> {
            for (mcid, value) in changes {
                if !(0..=MAX_MCID).contains(mcid) {
                    return Err(format!("marked-content id {mcid} is out of range"));
                }
                let i = usize::try_from(*mcid).map_err(|_| format!("marked-content id {mcid} is out of range"))?;
                if array.len() <= i {
                    if matches!(value, Object::Null) {
                        continue;
                    }
                    array.resize(i.saturating_add(1), Object::Null);
                }
                if let Some(slot) = array.get_mut(i) {
                    *slot = value.clone();
                }
            }
            Ok(())
        };
        match self.slots.get(&key).cloned() {
            // Clearing entries of a key that maps nothing changes nothing.
            None if changes.iter().all(|(_, v)| matches!(v, Object::Null)) => Ok(()),
            None => {
                let mut array = Vec::new();
                place(&mut array)?;
                self.insert(doc, key, Object::Array(array))
            }
            Some(slot) => match &slot.value {
                Object::Array(a) => {
                    let mut array = a.clone();
                    place(&mut array)?;
                    self.store(doc, &slot, Object::Array(array), key)
                }
                Object::Ref(r) => {
                    let Some(a) = doc.get(*r).as_array().cloned() else { return Err(format!("key {key} maps one element, not a page's content")) };
                    let mut array = a;
                    place(&mut array)?;
                    doc.set(*r, Object::Array(array));
                    Ok(())
                }
                Object::Null => {
                    let mut array = Vec::new();
                    place(&mut array)?;
                    self.store(doc, &slot, Object::Array(array), key)
                }
                _ => Err(format!("key {key} maps something other than an array of elements")),
            },
        }
    }

    /// Point `key` at one element (or null), creating the key when it is new.
    pub fn set_single(&mut self, doc: &mut Document, key: i64, value: Object) -> Result<(), String> {
        if self.is_duplicate(key) {
            return Err(format!("key {key} is listed twice"));
        }
        match self.slots.get(&key).cloned() {
            None if matches!(value, Object::Null) => Ok(()),
            None => self.insert(doc, key, value),
            Some(slot) => {
                if let Object::Ref(r) = &slot.value
                    && doc.get(*r).as_array().is_some()
                {
                    return Err(format!("key {key} maps a page's content, not one element"));
                }
                if matches!(slot.value, Object::Array(_)) {
                    return Err(format!("key {key} maps a page's content, not one element"));
                }
                self.store(doc, &slot, value, key)
            }
        }
    }

    /// Replace the value of an existing slot.
    fn store(&mut self, doc: &mut Document, slot: &Slot, value: Object, key: i64) -> Result<(), String> {
        let Some(dict) = doc.get(slot.node).as_dict().cloned() else { return Err("a parent tree node is missing".into()) };
        let Some((mut nums, holder)) = nums_of(doc, &dict) else { return Err("a parent tree node lost its Nums array".into()) };
        let Some(entry) = nums.get_mut(slot.at) else { return Err("a parent tree node changed while it was edited".into()) };
        *entry = value.clone();
        match holder {
            Some(array) => doc.set(array, Object::Array(nums)),
            None => doc.update_dict(slot.node, |d| d.set(b"Nums".to_vec(), Object::Array(nums))).map_err(|e| e.to_string())?,
        }
        self.slots.insert(key, Slot { node: slot.node, at: slot.at, value });
        Ok(())
    }

    /// Add a new key: into the leaf its value belongs to, widening `/Limits` on the way down.
    fn insert(&mut self, doc: &mut Document, key: i64, value: Object) -> Result<(), String> {
        if let Some(why) = &self.fixed {
            return Err(format!("the parent tree can't be extended: {why}"));
        }
        let mut node = self.root;
        let mut path: Vec<ObjRef> = Vec::new();
        for _ in 0..=MAX_DEPTH {
            let Some(dict) = doc.get(node).as_dict().cloned() else { return Err("a parent tree node is missing".into()) };
            let kids = dict.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).filter(|k| !k.is_empty());
            let Some(kids) = kids.filter(|_| !dict.contains(b"Nums")) else {
                let (mut nums, holder) = nums_of(doc, &dict).unwrap_or_default();
                // Whole pairs only; the new pair goes before the first greater key.
                let mut at = nums.len() - nums.len() % 2;
                for (i, [k, _]) in nums.as_chunks::<2>().0.iter().enumerate() {
                    if k.as_int().is_some_and(|k| k > key) {
                        at = i.saturating_mul(2);
                        break;
                    }
                }
                nums.insert(at.min(nums.len()), value);
                nums.insert(at.min(nums.len()), Object::Int(key));
                match holder {
                    Some(array) => doc.set(array, Object::Array(nums)),
                    None => doc.update_dict(node, |d| d.set(b"Nums".to_vec(), Object::Array(nums))).map_err(|e| e.to_string())?,
                }
                for n in path.iter().chain(std::iter::once(&node)) {
                    widen_limits(doc, *n, key, *n == self.root)?;
                }
                *self = ParentTree::read(doc, self.root, Some(self.next_key));
                return Ok(());
            };
            let mut choice = None;
            for kid in &kids {
                let Some(r) = kid.as_ref() else { return Err("a node lists a kid that is not an indirect reference".into()) };
                let high = doc.get(r).as_dict().and_then(|d| limits(doc, d)).map(|(_, high)| high);
                choice = Some(r);
                if high.is_some_and(|high| key <= high) {
                    break;
                }
            }
            let Some(next) = choice else { return Err("a parent tree node has no kids".into()) };
            path.push(node);
            node = next;
        }
        Err("the parent tree is too deep".into())
    }
}

fn limits(doc: &Document, node: &Dict) -> Option<(i64, i64)> {
    let l = node.get(b"Limits").map(|l| doc.resolve(l)).and_then(|l| l.as_array().cloned())?;
    Some((l.first()?.as_int()?, l.get(1)?.as_int()?))
}

/// Include `key` in a node's `/Limits` (the root has none, and one without them gets none).
fn widen_limits(doc: &mut Document, node: ObjRef, key: i64, root: bool) -> Result<(), String> {
    if root {
        return Ok(());
    }
    let Some(dict) = doc.get(node).as_dict().cloned() else { return Ok(()) };
    let Some((low, high)) = limits(doc, &dict) else { return Ok(()) };
    if (low..=high).contains(&key) {
        return Ok(());
    }
    let limits = Object::Array(vec![Object::Int(low.min(key)), Object::Int(high.max(key))]);
    doc.update_dict(node, |d| d.set(b"Limits".to_vec(), limits)).map_err(|e| e.to_string())
}
