//! The tagged structure of deleted pages (ISO 32000-2 14.7). Delete Pages takes their content out
//! of the structure tree: otherwise their elements, alternate and actual text included, stay
//! reachable from the structure tree root, its parent tree and its ID tree, and even a full
//! rewrite keeps them.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::OrganizeError;

/// Nesting followed at most. A deeper tree is refused, so a deletion never leaves tags behind
/// unnoticed; each element and tree node is visited once, so the work is linear in the tree.
const MAX_DEPTH: usize = 256;

/// Drop from the structure tree every marked-content and object reference on the pages `gone`,
/// every element left with nothing, the parent-tree entries of those pages and their annotations,
/// and the ID-tree entries of the dropped elements. An element with content on a kept page stays.
/// The pages are not freed yet: their parent-tree keys are read from them.
pub(crate) fn drop_structure_of(doc: &mut Document, gone: &[ObjRef]) -> Result<(), OrganizeError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let Some(tree_ref) = doc.get(root).as_dict().and_then(|c| c.reference(b"StructTreeRoot")) else { return Ok(()) };
    let Some(tree) = doc.get(tree_ref).as_dict().cloned() else { return Ok(()) };
    let gone: HashSet<ObjRef> = gone.iter().copied().collect();
    let mut keys = HashSet::new();
    for r in &gone {
        let Some(page) = doc.get(*r).as_dict().cloned() else { continue };
        keys.extend(page.get(b"StructParents").and_then(|v| doc.resolve(v).as_int()));
        let annots = page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        keys.extend(
            annots.iter().filter_map(|a| doc.resolve(a).as_dict().and_then(|d| d.get(b"StructParent").and_then(|v| doc.resolve(v).as_int()))),
        );
    }
    let mut p = Pruner { gone: &gone, seen: HashSet::new(), dropped: HashSet::new(), nodes: HashSet::new() };
    if let Some(k) = tree.get(b"K") {
        let kept = p.kids(doc, k, None, 0)?;
        if kept.as_ref() != Some(k) {
            doc.update_dict(tree_ref, |t| match kept {
                Some(k) => t.set(b"K".to_vec(), k),
                None => {
                    t.remove(b"K");
                }
            })?;
        }
    }
    for (name, list) in [(&b"ParentTree"[..], &b"Nums"[..]), (&b"IDTree"[..], &b"Names"[..])] {
        if let Some(node) = tree.get(name) {
            p.tree(doc, node, list, &keys, 0)?;
        }
    }
    // A kept element's references to dropped ones (`/Ref`, PDF 2.0) would keep them too.
    for r in p.seen.difference(&p.dropped) {
        let Some(refs) = doc.get(*r).as_dict().and_then(|e| e.get(b"Ref").and_then(|v| doc.resolve(v).as_array().cloned())) else { continue };
        let left: Vec<Object> = refs.iter().filter(|o| !o.as_ref().is_some_and(|x| p.dropped.contains(&x))).cloned().collect();
        if left.len() != refs.len() {
            doc.update_dict(*r, |e| e.set(b"Ref".to_vec(), Object::Array(left)))?;
        }
    }
    // So would a bookmark's `/SE` (ISO 32000-2 12.3.3) naming a dropped element.
    for item in crate::outline::items(doc) {
        if doc.get(item).as_dict().and_then(|b| b.reference(b"SE")).is_some_and(|e| p.dropped.contains(&e)) {
            doc.update_dict(item, |b| {
                b.remove(b"SE");
            })?;
        }
    }
    Ok(())
}

struct Pruner<'a> {
    gone: &'a HashSet<ObjRef>,
    seen: HashSet<ObjRef>,
    dropped: HashSet<ObjRef>,
    /// Number and name tree nodes visited.
    nodes: HashSet<ObjRef>,
}

fn check_depth(depth: usize) -> Result<(), OrganizeError> {
    if depth > MAX_DEPTH {
        return Err(OrganizeError::Invalid(format!(
            "the structure tree is nested more than {MAX_DEPTH} levels deep, so the pages' tags can't be removed"
        )));
    }
    Ok(())
}

impl Pruner<'_> {
    /// `k` (an element's `/K`: one kid or an array of them) without the kids on deleted pages;
    /// `None` when nothing is left. `page` is the page the kids inherit (`/Pg`).
    fn kids(&mut self, doc: &mut Document, k: &Object, page: Option<ObjRef>, depth: usize) -> Result<Option<Object>, OrganizeError> {
        // An array held in an object of its own is the kids all the same.
        let Some(list) = doc.resolve(k).as_array().cloned() else { return self.kid(doc, k, page, depth) };
        let mut out = Vec::new();
        for kid in &list {
            out.extend(self.kid(doc, kid, page, depth)?);
        }
        if out == list {
            return Ok(Some(k.clone()));
        }
        Ok((!out.is_empty()).then_some(Object::Array(out)))
    }

    /// One kid: a marked-content id, a marked-content or object reference, or an element.
    fn kid(&mut self, doc: &mut Document, kid: &Object, page: Option<ObjRef>, depth: usize) -> Result<Option<Object>, OrganizeError> {
        check_depth(depth)?;
        let on_gone = |p: Option<ObjRef>| p.is_some_and(|p| self.gone.contains(&p));
        if doc.resolve(kid).as_int().is_some() {
            return Ok((!on_gone(page)).then(|| kid.clone()));
        }
        let Some(d) = doc.resolve(kid).as_dict().cloned() else { return Ok(Some(kid.clone())) };
        let own = d.reference(b"Pg").or(page);
        // An element that stays with its `/Pg` gone keeps only kids that name their own page.
        let pg_gone = on_gone(d.reference(b"Pg"));
        if !d.contains(b"S") {
            // A marked-content or object reference.
            return Ok((!on_gone(own)).then(|| kid.clone()));
        }
        // Elements are indirect objects; one reached twice is decided once.
        let Some(r) = kid.as_ref() else { return Ok(Some(kid.clone())) };
        if !self.seen.insert(r) {
            return Ok((!self.dropped.contains(&r)).then(|| kid.clone()));
        }
        let kept = match d.get(b"K") {
            Some(k) => self.kids(doc, k, own, depth + 1)?,
            None if on_gone(own) => None,
            None => return Ok(Some(kid.clone())),
        };
        let Some(kept) = kept else {
            self.dropped.insert(r);
            return Ok(None);
        };
        if d.get(b"K") != Some(&kept) || pg_gone {
            doc.update_dict(r, |e| {
                e.set(b"K".to_vec(), kept);
                if pg_gone {
                    e.remove(b"Pg");
                }
            })?;
        }
        Ok(Some(kid.clone()))
    }

    /// Remove from the number or name tree `node` (`list` is `Nums` or `Names`) the entries keyed
    /// by one of `keys` and those naming only dropped elements; elsewhere a dropped element's array
    /// slot becomes null.
    fn tree(&mut self, doc: &mut Document, node: &Object, list: &[u8], keys: &HashSet<i64>, depth: usize) -> Result<(), OrganizeError> {
        check_depth(depth)?;
        let r = node.as_ref();
        if r.is_some_and(|r| !self.nodes.insert(r)) {
            return Ok(());
        }
        let Some(d) = r.map_or_else(|| node.as_dict().cloned(), |r| doc.get(r).as_dict().cloned()) else { return Ok(()) };
        for kid in d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default() {
            self.tree(doc, &kid, list, keys, depth + 1)?;
        }
        // A root written directly in the structure tree root keeps its own entries; its kids are cleaned.
        let Some(r) = r else { return Ok(()) };
        let Some(pairs) = d.get(list).map(|n| doc.resolve(n)).and_then(|n| n.as_array().cloned()) else { return Ok(()) };
        let dropped = |o: &Object| o.as_ref().is_some_and(|r| self.dropped.contains(&r));
        let mut kept = Vec::with_capacity(pairs.len());
        for pair in pairs.chunks(2) {
            let [k, v] = pair else { continue };
            if doc.resolve(k).as_int().is_some_and(|k| keys.contains(&k)) {
                continue;
            }
            // In an array each slot is an MCID: a dropped element's slot becomes null, as an unused
            // one is, so the later MCIDs keep their place. The entry goes once no slot names an element.
            let v = match doc.resolve(v).as_array() {
                Some(a) if a.iter().any(dropped) => {
                    let left: Vec<Object> = a.iter().map(|o| if dropped(o) { Object::Null } else { o.clone() }).collect();
                    if left.iter().all(|o| matches!(o, Object::Null)) {
                        continue;
                    }
                    Object::Array(left)
                }
                None if dropped(v) => continue,
                _ => v.clone(),
            };
            kept.extend([k.clone(), v]);
        }
        if kept != pairs {
            doc.update_dict(r, |n| n.set(list.to_vec(), Object::Array(kept)))?;
        }
        Ok(())
    }
}
