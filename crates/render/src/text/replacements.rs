//! Replacement text (`/ActualText`) given on structure elements rather than on marked content
//! (ISO 32000-2 §14.9.4). Structured content (the page's own, or a form XObject's with its own
//! `/StructParents`) has a key in the structure tree's parent tree (§14.7.5.4), whose entry lists,
//! by MCID, the element owning each marked-content sequence of that content; that element or an
//! ancestor may carry the text that replaces the sequence.
//!
//! Only well-formed trees are read: a lookup that runs into a cycle or past a bound gives no
//! replacement, so the content keeps its own text. The walk up `/P` ends at the first dictionary
//! without `/S` (the structure tree root), so a malformed element without `/S` hides the
//! replacements of the elements above it.

use std::collections::{HashMap, HashSet};

use hayro::hayro_interpret::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier, String as PdfString};
use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::hayro_syntax::xref::XRef;

/// The replacements over a page's structured content, resolved one parent tree key at a time as
/// the content reaches it: for each MCID under a key, the outermost element over it with
/// `/ActualText` (so its text is given once, however many sequences it spans), and each such
/// element's text.
#[derive(Default)]
pub(super) struct Replacements<'a> {
    /// The document's cross-reference table and structural parent tree; `None` when it has none.
    tree: Option<(&'a XRef, Dict<'a>)>,
    /// The keys looked up so far, and whether each could be resolved within the budgets.
    keys: HashMap<i32, bool>,
    owners: HashMap<(i32, i32), ObjectIdentifier>,
    walk: Walk,
    /// Parent tree entries (nodes, `/Nums` pairs, `/Kids` and MCID items) still to be read.
    work_left: usize,
    /// Set once a budget is spent: no key is resolved after that.
    spent: bool,
}

impl<'a> Replacements<'a> {
    /// The replacements of the content on `page`, none when its document isn't tagged.
    pub(super) fn new(page: &Page<'a>) -> Self {
        let xref = page.xref();
        let tree = xref
            .get::<Dict<'_>>(xref.root_id())
            .and_then(|catalog| catalog.get::<Dict<'_>>(b"StructTreeRoot"))
            .and_then(|root| root.get::<Dict<'_>>(b"ParentTree"));
        Replacements { tree: tree.map(|tree| (xref, tree)), work_left: MAX_WORK, ..Replacements::default() }
    }

    /// The element replacing the sequence `mcid` of the content with parent tree key `key`, and
    /// its text's raw bytes.
    pub(super) fn get(&mut self, key: i32, mcid: i32) -> Option<(ObjectIdentifier, &[u8])> {
        // Once a budget is spent no key is looked up, so none needs recording.
        if !self.spent && !self.keys.contains_key(&key) {
            let resolved = self.resolve(key);
            self.keys.insert(key, resolved);
        }
        let owner = *self.owners.get(&(key, mcid))?;
        Some((owner, self.walk.texts.get(&owner)?.as_slice()))
    }

    /// Looks up `key`'s owners: all of them, or none when a budget runs out on the way.
    fn resolve(&mut self, key: i32) -> bool {
        let Some((xref, tree)) = self.tree.clone() else { return false };
        if self.spent || self.keys.len() >= MAX_KEYS {
            return false;
        }
        let Some(owners) = lookup(xref, tree, key, &mut self.work_left) else {
            // A key the tree lacks, or one past a single lookup's node bound, spends only what
            // was read; other keys may still resolve within the page's budget.
            self.spent |= self.work_left == 0;
            return false;
        };
        let mut found = Vec::new();
        for (mcid, owner) in owners.raw_iter().take(MAX_MCIDS).enumerate() {
            if !charge(&mut self.work_left) {
                self.spent = true;
                return false;
            }
            let (Ok(mcid), MaybeRef::Ref(owner)) = (i32::try_from(mcid), owner) else { continue };
            match self.walk.outermost(xref, owner.into()) {
                Some(Some(element)) => found.push(((key, mcid), element)),
                Some(None) => {}
                None => {
                    self.spent = true;
                    return false;
                }
            }
        }
        if self.owners.len().saturating_add(found.len()) > MAX_MCIDS {
            self.spent = true;
            return false;
        }
        self.owners.extend(found);
        true
    }
}

/// Most parent-tree nodes one lookup visits.
const MAX_TREE_NODES: usize = 4096;
/// Most parent tree entries read for one page, over all its keys; past it, no further key gets
/// replacements. Each key reads its part of the tree, so a page drawing many forms with keys of
/// their own would otherwise read a large tree once per form.
const MAX_WORK: usize = 1 << 20;
/// Most MCIDs read for one page, over all its keys.
const MAX_MCIDS: usize = 1 << 16;
/// Most parent tree keys looked up for one page (its own and its forms').
const MAX_KEYS: usize = 256;
/// Most elements in one chain up `/P`; a deeper chain gives no replacement.
const MAX_ANCESTORS: usize = 256;
/// Most elements resolved for one page; past it, no further key gets replacements.
const MAX_STEPS: usize = 1 << 18;
/// Most replacement-text bytes kept for one page; past it, no further key gets replacements.
const MAX_TEXT: usize = 1 << 24;

/// Takes one entry from `work`, or says there is none left.
fn charge(work: &mut usize) -> bool {
    let Some(left) = work.checked_sub(1) else { return false };
    *work = left;
    true
}

/// The value for `key` in the number tree `tree` (§7.9.7), resolved to an array. Each node,
/// `/Nums` pair and `/Kids` item read takes one entry from `work`; `None` once it is empty.
fn lookup<'a>(xref: &'a XRef, tree: Dict<'a>, key: i32, work: &mut usize) -> Option<Array<'a>> {
    let mut stack = vec![tree];
    let mut seen = HashSet::new();
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > MAX_TREE_NODES || !charge(work) {
            return None;
        }
        if let Some(limits) = node.get::<Array<'_>>(b"Limits") {
            let mut bounds = limits.iter::<f64>();
            if let (Some(low), Some(high)) = (bounds.next(), bounds.next())
                && !(low..=high).contains(&f64::from(key))
            {
                continue;
            }
        }
        if let Some(nums) = node.get::<Array<'_>>(b"Nums") {
            let mut items = nums.raw_iter();
            while let (Some(k), Some(value)) = (items.next(), items.next()) {
                if !charge(work) {
                    return None;
                }
                if !matches!(k, MaybeRef::NotRef(Object::Number(n)) if n.as_f64() == f64::from(key)) {
                    continue;
                }
                return match value {
                    MaybeRef::Ref(r) => xref.get::<Array<'_>>(r.into()),
                    MaybeRef::NotRef(Object::Array(a)) => Some(a),
                    MaybeRef::NotRef(_) => None,
                };
            }
        }
        if let Some(kids) = node.get::<Array<'_>>(b"Kids") {
            for kid in kids.raw_iter() {
                if !charge(work) {
                    return None;
                }
                // Kids are indirect (§7.9.6); one already visited means a cycle.
                if let MaybeRef::Ref(r) = kid
                    && seen.insert(ObjectIdentifier::from(r))
                    && let Some(d) = xref.get::<Dict<'_>>(r.into())
                {
                    stack.push(d);
                }
            }
        }
    }
    None
}

/// The elements resolved for a page, shared by all its keys, with the budgets they spend.
#[derive(Default)]
struct Walk {
    /// Each element resolved so far: the outermost of it and its ancestors with `/ActualText`,
    /// and how many elements its chain up to the root has (itself included).
    memo: HashMap<ObjectIdentifier, (Option<ObjectIdentifier>, usize)>,
    /// The text of each element with `/ActualText` met so far.
    texts: HashMap<ObjectIdentifier, Vec<u8>>,
    steps: usize,
    text_bytes: usize,
}

impl Walk {
    /// The outermost of `element` and its ancestors (up `/P`) that has `/ActualText`: an outer
    /// replacement covers everything inside it, as nested marked content does. `None` once the
    /// page's budget is spent. Shared ancestors are resolved once.
    fn outermost(&mut self, xref: &XRef, element: ObjectIdentifier) -> Option<Option<ObjectIdentifier>> {
        // The chain from `element` up to the first element already resolved (whose answer is
        // `above`) or to the root, with whether each element has a replacement.
        let mut chain: Vec<(ObjectIdentifier, bool)> = Vec::new();
        let mut on_chain = HashSet::new();
        let (mut above, mut above_len) = (None, 0);
        let mut at = Some(element);
        while let Some(id) = at {
            if let Some(&(known, len)) = self.memo.get(&id) {
                (above, above_len) = (known, len);
                break;
            }
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return None;
            }
            if !on_chain.insert(id) || chain.len() >= MAX_ANCESTORS {
                return Some(self.give_up(chain));
            }
            let Some(dict) = xref.get::<Dict<'_>>(id) else { break };
            // The structure tree root has no /S; the walk ends there.
            if dict.get::<Name<'_>>(b"S").is_none() {
                break;
            }
            let text = dict.get::<PdfString<'_>>(b"ActualText");
            if let Some(text) = &text {
                self.text_bytes = self.text_bytes.saturating_add(text.as_bytes().len());
                if self.text_bytes > MAX_TEXT {
                    return None;
                }
                self.texts.insert(id, text.as_bytes().to_vec());
            }
            chain.push((id, text.is_some()));
            at = dict.get_ref(b"P").map(ObjectIdentifier::from);
        }
        // Measured whole, whichever MCID reached the upper part of the chain first.
        if chain.len().saturating_add(above_len) > MAX_ANCESTORS {
            return Some(self.give_up(chain));
        }
        // From the top down, an element takes its parent's answer, or itself when it has a
        // replacement and nothing above it does.
        let mut answer = above;
        let mut len = above_len;
        for (id, has_text) in chain.into_iter().rev() {
            if answer.is_none() && has_text {
                answer = Some(id);
            }
            len += 1;
            self.memo.insert(id, (answer, len));
        }
        Some(answer)
    }

    /// A cycle or an overlong chain: no element on it gets a replacement.
    fn give_up(&mut self, chain: Vec<(ObjectIdentifier, bool)>) -> Option<ObjectIdentifier> {
        for (id, _) in chain {
            self.memo.insert(id, (None, MAX_ANCESTORS));
        }
        None
    }
}
