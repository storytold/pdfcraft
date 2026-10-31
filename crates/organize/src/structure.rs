//! Carrying the logical structure (tags, ISO 32000-2 §14.7) of the files Combine Files copies, so
//! combining tagged files gives a tagged file.
//!
//! The pages are copied first (`import`); then each tagged source's structure tree is copied for
//! those pages, and the result gets one structure tree root holding every source's top-level
//! elements in order:
//! - elements keep their attributes, classes, alternate and actual text, language and IDs; their
//!   `/P`, `/Pg`, marked-content references (`MCR`, with `/Stm`) and object references (`OBJR`:
//!   annotations, XObjects) point at the copies. When only some of a source's pages are combined,
//!   the elements with no content on them are left out;
//! - each source's parent tree entries move to keys of their own (`/StructParents` of pages and
//!   form XObjects, `/StructParent` of annotations and XObjects are renumbered to match);
//! - role maps and class maps are merged; a name two sources map differently is renamed in the
//!   later one, and so is an element ID another source already uses (with the table `/Headers`
//!   that name it);
//! - the catalog gets `/MarkInfo << /Marked true >>` when every source is tagged, and the first
//!   source's `/Lang`. A source in another language has its language set on its top-level elements.
//!
//! A source whose structure can't be read within the limits below is copied untagged, as before.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::OrganizeError;
use crate::import::Copier;

/// How many structure elements one source may have, and how deep they may nest.
const MAX_ELEMENTS: usize = 1_000_000;
const MAX_DEPTH: usize = 256;
/// How many parent tree entries one source may have, and how deep its number tree may nest.
const MAX_ENTRIES: usize = 4_000_000;
const MAX_TREE_DEPTH: usize = 32;

/// One structure element of a source, as read.
struct Element {
    /// The element object (`None` for a direct dictionary).
    obj: Option<ObjRef>,
    dict: Dict,
    /// Its parent element (index), `None` at the top level.
    parent: Option<usize>,
    /// The page its marked content is on unless it says otherwise (its `/Pg`, or its parent's).
    page: Option<ObjRef>,
    /// Its children written as direct dictionaries (indices, in order): they have no reference to
    /// find them by.
    direct: Vec<usize>,
}

/// The structure trees of the sources added so far, ready to become the result's.
#[derive(Default)]
pub(crate) struct Merged {
    /// The result's structure tree root, reserved when the first tagged source is added.
    root: Option<ObjRef>,
    /// The top-level elements, in order.
    kids: Vec<Object>,
    /// Parent tree entries, by key.
    nums: Vec<(i64, Object)>,
    next_key: i64,
    role_map: Dict,
    class_map: Dict,
    /// Element IDs and their elements.
    ids: Vec<(Vec<u8>, ObjRef)>,
    taken_ids: HashSet<Vec<u8>>,
    namespaces: Vec<Object>,
    sources: usize,
    tagged: usize,
    suspects: bool,
    /// The first source's language: the result's.
    lang: Option<Object>,
}

impl Merged {
    /// Add the structure of `src`, whose pages were copied as `pages` (source page → copy) along
    /// with the objects in `objects` (source → copy).
    pub(crate) fn add(&mut self, dst: &mut Document, src: &Document, pages: &HashMap<ObjRef, ObjRef>, objects: &HashMap<ObjRef, ObjRef>) {
        self.sources += 1;
        let catalog = src.root().and_then(|r| src.get(r).as_dict().cloned()).unwrap_or_default();
        let lang = catalog.get(b"Lang").map(|l| src.resolve(l).as_ref().clone()).filter(|l| l.as_string().is_some());
        if self.sources == 1 {
            self.lang = lang.clone();
        }
        let Some(tree_root) = catalog.get(b"StructTreeRoot").map(|r| src.resolve(r)).and_then(|r| r.as_dict().cloned()) else { return };
        let Some(elements) = read_elements(src, &tree_root) else { return };
        let Some(parent_tree) = read_number_tree(src, tree_root.get(b"ParentTree")) else { return };
        // Map attributes can contain indirect chains unrelated to pages or elements. Bound
        // those graphs before handing them to the recursive import copier.
        if [b"RoleMap".as_slice(), b"ClassMap".as_slice()].iter().any(|key| tree_root.get(key).is_some_and(|map| !within_copy_limits(src, map))) {
            return;
        }
        let mark_info = catalog.get(b"MarkInfo").map(|m| src.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
        if matches!(mark_info.get(b"Marked"), Some(Object::Bool(true))) {
            self.tagged += 1;
        }
        self.suspects |= matches!(mark_info.get(b"Suspects"), Some(Object::Bool(true)));
        let root = *self.root.get_or_insert_with(|| dst.add(Object::Null));

        // Which elements come along: every one when the whole source was copied, else those with
        // content on a copied page (and so their ancestors).
        let whole = crate::page_count(src).is_ok_and(|n| n == pages.len());
        let keep = if whole { vec![true; elements.len()] } else { with_copied_content(src, &elements, pages, objects) };

        // References out of the elements must not drag the source's pages or tree along: every
        // page, element and the tree root maps to its copy, or to a null object when left out.
        let null = dst.add(Object::Null);
        let mut map = objects.clone();
        for (page, _) in crate::walk(src).unwrap_or_default() {
            map.insert(page, pages.get(&page).copied().unwrap_or(null));
        }
        if let Some(r) = catalog.reference(b"StructTreeRoot") {
            map.insert(r, root);
        }
        let mut new: Vec<Option<ObjRef>> = Vec::with_capacity(elements.len());
        for (e, kept) in elements.iter().zip(&keep) {
            let copy = kept.then(|| dst.add(Object::Null));
            if let Some(obj) = e.obj {
                map.insert(obj, copy.unwrap_or(null));
            }
            new.push(copy);
        }
        let element_of: HashMap<ObjRef, ObjRef> = elements.iter().zip(&new).filter_map(|(e, n)| Some((e.obj?, (*n)?))).collect();
        let index_of: HashMap<ObjRef, usize> = elements.iter().enumerate().filter_map(|(i, e)| Some((e.obj?, i))).collect();

        let mut copier = Copier::seeded(src, map, pages.clone());
        let roles = merge_names(&mut self.role_map, src, tree_root.get(b"RoleMap"), &mut copier, dst);
        let classes = merge_names(&mut self.class_map, src, tree_root.get(b"ClassMap"), &mut copier, dst);
        if let Some(ns) = tree_root.get(b"Namespaces").map(|n| src.resolve(n)).and_then(|n| n.as_array().cloned()) {
            self.namespaces.extend(ns.iter().map(|n| copier.copy_val(dst, n, None)));
        }

        // IDs another source already uses are renamed (and the /Headers that name them).
        let mut renamed_ids: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
        for (e, n) in elements.iter().zip(&new) {
            let (Some(id), Some(n)) = (e.dict.get(b"ID").map(|i| src.resolve(i)).and_then(|i| i.as_string().map(|s| s.bytes.clone())), n) else {
                continue;
            };
            let unique = if self.taken_ids.contains(&id) { fresh(&id, &self.taken_ids) } else { id.clone() };
            if unique != id {
                renamed_ids.insert(id, unique.clone());
            }
            self.taken_ids.insert(unique.clone());
            self.ids.push((unique, *n));
        }

        let source_lang = lang.filter(|l| Some(l) != self.lang.as_ref());
        for (i, e) in elements.iter().enumerate() {
            let Some(Some(this)) = new.get(i) else { continue };
            let parent = e.parent.and_then(|p| new.get(p).copied().flatten()).unwrap_or(root);
            let mut d = Dict::new();
            for (k, v) in e.dict.iter() {
                let value = match k.as_slice() {
                    b"P" => Some(Object::Ref(parent)),
                    b"Pg" => v.as_ref().and_then(|p| pages.get(&p)).map(|p| Object::Ref(*p)),
                    b"K" => {
                        let direct: Vec<Option<ObjRef>> = e.direct.iter().map(|d| new.get(*d).copied().flatten()).collect();
                        // An element is this one's child only where it was read as one: a kid
                        // listed twice, or a cycle, is not written again.
                        let child = |r: ObjRef| {
                            index_of
                                .get(&r)
                                .map(|&j| elements.get(j).and_then(|c| c.parent).filter(|p| *p == i).and_then(|_| new.get(j).copied().flatten()))
                        };
                        kids(src, &mut copier, dst, v, e.page, pages, objects, &child, &direct)
                    }
                    b"S" => Some(rename(v, &roles).unwrap_or_else(|| copier.copy_val(dst, v, None))),
                    b"C" => Some(match v {
                        Object::Array(a) => {
                            Object::Array(a.iter().map(|c| rename(c, &classes).unwrap_or_else(|| copier.copy_val(dst, c, None))).collect())
                        }
                        other => rename(other, &classes).unwrap_or_else(|| copier.copy_val(dst, other, None)),
                    }),
                    b"ID" => Some(match src.resolve(v).as_string() {
                        Some(s) => {
                            Object::String(pdfcraft_cos::PdfString::literal(renamed_ids.get(&s.bytes).cloned().unwrap_or_else(|| s.bytes.clone())))
                        }
                        None => copier.copy_val(dst, v, None),
                    }),
                    b"Ref" => Some(Object::Array(
                        src.resolve(v)
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|r| r.as_ref().and_then(|r| element_of.get(&r)))
                            .map(|r| Object::Ref(*r))
                            .collect(),
                    )),
                    b"A" => Some(with_headers_renamed(&copier.copy_val(dst, v, None), dst, &renamed_ids)),
                    _ => Some(copier.copy_val(dst, v, None)),
                };
                if let Some(value) = value {
                    d.set(k.clone(), value);
                }
            }
            if e.parent.is_none() {
                if let Some(l) = &source_lang
                    && !d.contains(b"Lang")
                {
                    d.set(b"Lang".to_vec(), l.clone());
                }
                self.kids.push(Object::Ref(*this));
            }
            dst.set(*this, Object::Dict(d));
        }

        // Parent tree entries, under keys of this source's own, and the owners renumbered to match.
        let offset = self.next_key;
        let mut next = offset;
        let mut entry = |key: i64, value: Object, nums: &mut Vec<(i64, Object)>| -> Option<i64> {
            let k = key.checked_add(offset).filter(|k| *k >= 0)?;
            next = next.max(k.saturating_add(1));
            nums.push((k, value));
            Some(k)
        };
        let array_of = |value: &Object| -> Object {
            Object::Array(
                src.resolve(value)
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| r.as_ref().and_then(|r| element_of.get(&r)).map_or(Object::Null, |n| Object::Ref(*n)))
                    .collect(),
            )
        };
        for (page, copy) in pages {
            let key = src.get(*page).as_dict().and_then(|d| d.int(b"StructParents"));
            let value = key.and_then(|k| parent_tree.get(&k));
            let new_key = match (key, value) {
                (Some(k), Some(v)) => entry(k, array_of(v), &mut self.nums),
                _ => None,
            };
            let _ = dst.update_dict(*copy, |d| match new_key {
                Some(k) => d.set(b"StructParents".to_vec(), Object::Int(k)),
                None => {
                    d.remove(b"StructParents");
                }
            });
        }
        let mut owners: Vec<(&ObjRef, &ObjRef)> = objects.iter().collect();
        owners.sort();
        for (obj, copy) in owners {
            let source = src.get(*obj);
            let Some(sd) = (match &*source {
                Object::Dict(d) => Some(d),
                Object::Stream(s) => Some(&s.dict),
                _ => None,
            }) else {
                continue;
            };
            // A form XObject's marked content (/StructParents) or an annotation or XObject
            // that is one element's content (/StructParent).
            let many = sd.int(b"StructParents").map(|k| (&b"StructParents"[..], k, parent_tree.get(&k).map(array_of)));
            let one = sd.int(b"StructParent").map(|k| {
                let value = parent_tree.get(&k).and_then(|v| v.as_ref()).and_then(|r| element_of.get(&r)).map(|n| Object::Ref(*n));
                (&b"StructParent"[..], k, value)
            });
            for (key_name, k, value) in [many, one].into_iter().flatten() {
                let new_key = value.and_then(|v| entry(k, v, &mut self.nums));
                let mut copied = (*dst.get(*copy)).clone();
                let d = match &mut copied {
                    Object::Dict(d) => d,
                    Object::Stream(s) => &mut s.dict,
                    _ => continue,
                };
                match new_key {
                    Some(k) => d.set(key_name.to_vec(), Object::Int(k)),
                    None => {
                        d.remove(key_name);
                    }
                }
                dst.set(*copy, copied);
            }
        }
        self.next_key = next;
    }

    /// Write the result's language (the first source's), and its structure tree root and
    /// `/MarkInfo` when a source was tagged.
    pub(crate) fn finish(mut self, dst: &mut Document) -> Result<(), OrganizeError> {
        let catalog = dst.root().ok_or(OrganizeError::NoPageTree)?;
        if let Some(l) = self.lang.take() {
            dst.update_dict(catalog, |c| c.set(b"Lang".to_vec(), l))?;
        }
        let Some(root) = self.root else { return Ok(()) };
        // Sorted, and one entry per key even when a malformed source gave two owners one key.
        self.nums.sort_by_key(|(k, _)| *k);
        self.nums.dedup_by_key(|(k, _)| *k);
        let nums: Vec<Object> = self.nums.into_iter().flat_map(|(k, v)| [Object::Int(k), v]).collect();
        let mut parent_tree = Dict::new();
        parent_tree.set(b"Nums".to_vec(), Object::Array(nums));
        let parent_tree = dst.add(Object::Dict(parent_tree));
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
        d.set(b"K".to_vec(), Object::Array(self.kids));
        d.set(b"ParentTree".to_vec(), Object::Ref(parent_tree));
        d.set(b"ParentTreeNextKey".to_vec(), Object::Int(self.next_key));
        if !self.role_map.is_empty() {
            d.set(b"RoleMap".to_vec(), Object::Dict(self.role_map));
        }
        if !self.class_map.is_empty() {
            d.set(b"ClassMap".to_vec(), Object::Dict(self.class_map));
        }
        if !self.ids.is_empty() {
            self.ids.sort_by(|a, b| a.0.cmp(&b.0));
            let names: Vec<Object> =
                self.ids.into_iter().flat_map(|(id, r)| [Object::String(pdfcraft_cos::PdfString::literal(id)), Object::Ref(r)]).collect();
            let mut tree = Dict::new();
            tree.set(b"Names".to_vec(), Object::Array(names));
            let tree = dst.add(Object::Dict(tree));
            d.set(b"IDTree".to_vec(), Object::Ref(tree));
        }
        if !self.namespaces.is_empty() {
            d.set(b"Namespaces".to_vec(), Object::Array(self.namespaces));
        }
        dst.set(root, Object::Dict(d));
        let marked = self.tagged == self.sources;
        dst.update_dict(catalog, |c| {
            c.set(b"StructTreeRoot".to_vec(), Object::Ref(root));
            if marked {
                let mut m = Dict::new();
                m.set(b"Marked".to_vec(), Object::Bool(true));
                if self.suspects {
                    m.set(b"Suspects".to_vec(), Object::Bool(true));
                }
                c.set(b"MarkInfo".to_vec(), Object::Dict(m));
            }
        })?;
        Ok(())
    }
}

/// The elements of a structure tree, parents before their children. `None` past the limits.
fn read_elements(src: &Document, tree_root: &Dict) -> Option<Vec<Element>> {
    let mut out: Vec<Element> = Vec::new();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    // (kid entry, parent index, inherited page, depth), in reverse so that order is kept.
    let mut stack: Vec<(Object, Option<usize>, Option<ObjRef>, usize)> = Vec::new();
    let top: Vec<Object> = match tree_root.get(b"K") {
        Some(k) => match &*src.resolve(k) {
            Object::Array(a) => a.clone(),
            _ => vec![k.clone()],
        },
        None => Vec::new(),
    };
    stack.extend(top.into_iter().rev().map(|k| (k, None, None, 0)));
    while let Some((k, parent, page, depth)) = stack.pop() {
        if depth > MAX_DEPTH || out.len() >= MAX_ELEMENTS {
            return None;
        }
        let obj = k.as_ref();
        if obj.is_some_and(|r| !seen.insert(r)) {
            continue;
        }
        let Some(dict) = src.resolve(&k).as_dict().cloned() else { continue };
        // Marked-content and object references are an element's content, not elements.
        if !dict.contains(b"S") || matches!(dict.name(b"Type"), Some(b"MCR" | b"OBJR")) {
            continue;
        }
        let page = dict.reference(b"Pg").or(page);
        let index = out.len();
        let kids: Vec<Object> = match dict.get(b"K") {
            Some(k) => match &*src.resolve(k) {
                Object::Array(a) => a.clone(),
                _ => vec![k.clone()],
            },
            None => Vec::new(),
        };
        if obj.is_none()
            && let Some(p) = parent.and_then(|p| out.get_mut(p))
        {
            p.direct.push(index);
        }
        out.push(Element { obj, dict, parent, page, direct: Vec::new() });
        stack.extend(kids.into_iter().rev().map(|k| (k, Some(index), page, depth + 1)));
    }
    Some(out)
}

/// Which elements have content on a copied page (an MCID or `MCR` on one, or an `OBJR` to a copied
/// object), directly or through their children.
fn with_copied_content(src: &Document, elements: &[Element], pages: &HashMap<ObjRef, ObjRef>, objects: &HashMap<ObjRef, ObjRef>) -> Vec<bool> {
    let mut keep: Vec<bool> = elements
        .iter()
        .map(|e| {
            let kids: Vec<Object> = match e.dict.get(b"K").map(|k| src.resolve(k)).as_deref() {
                Some(Object::Array(a)) => a.clone(),
                Some(other) => vec![other.clone()],
                None => Vec::new(),
            };
            kids.iter().any(|k| match &*src.resolve(k) {
                Object::Int(_) => e.page.is_some_and(|p| pages.contains_key(&p)),
                Object::Dict(d) if d.name(b"Type") == Some(b"MCR") => d.reference(b"Pg").or(e.page).is_some_and(|p| pages.contains_key(&p)),
                Object::Dict(d) if d.name(b"Type") == Some(b"OBJR") => d.reference(b"Obj").is_some_and(|o| objects.contains_key(&o)),
                _ => false,
            })
        })
        .collect();
    // Children come after their parents: walking backwards marks every ancestor of what is kept.
    for i in (0..elements.len()).rev() {
        if keep.get(i).copied().unwrap_or(false)
            && let Some(p) = elements.get(i).and_then(|e| e.parent)
            && let Some(k) = keep.get_mut(p)
        {
            *k = true;
        }
    }
    keep
}

/// An element's kids in the result: MCIDs on copied pages, marked-content and object references
/// pointing at the copies, and the child elements that come along (`child` gives a referenced
/// element's copy, `direct` the copies of the children written in place, in order). `None` when
/// nothing is left.
#[allow(clippy::too_many_arguments)]
fn kids(
    src: &Document,
    copier: &mut Copier<'_>,
    dst: &mut Document,
    k: &Object,
    page: Option<ObjRef>,
    pages: &HashMap<ObjRef, ObjRef>,
    objects: &HashMap<ObjRef, ObjRef>,
    child: &dyn Fn(ObjRef) -> Option<Option<ObjRef>>,
    direct: &[Option<ObjRef>],
) -> Option<Object> {
    let list: Vec<Object> = match &*src.resolve(k) {
        Object::Array(a) => a.clone(),
        _ => vec![k.clone()],
    };
    let on_copied = |p: Option<ObjRef>| p.is_some_and(|p| pages.contains_key(&p));
    let mut direct = direct.iter();
    let mut written = HashSet::new();
    let mut out = Vec::new();
    for kid in &list {
        // A child element given by reference: its copy, if it came along.
        if let Some(r) = kid.as_ref()
            && let Some(copy) = child(r)
        {
            if let Some(copy) = copy.filter(|c| written.insert(*c)) {
                out.push(Object::Ref(copy));
            }
            continue;
        }
        match &*src.resolve(kid) {
            Object::Int(mcid) if on_copied(page) => out.push(Object::Int(*mcid)),
            Object::Dict(d) if d.name(b"Type") == Some(b"MCR") => {
                let pg = d.reference(b"Pg").or(page);
                if !on_copied(pg) {
                    continue;
                }
                let mut m = Dict::new();
                let mut stream_kept = true;
                for (key, v) in d.iter() {
                    match key.as_slice() {
                        b"Pg" => {
                            if let Some(p) = pg.and_then(|p| pages.get(&p)) {
                                m.set(key.clone(), Object::Ref(*p));
                            }
                        }
                        b"Stm" => match v.as_ref().and_then(|s| objects.get(&s)) {
                            Some(s) => m.set(key.clone(), Object::Ref(*s)),
                            None => stream_kept = false,
                        },
                        _ => m.set(key.clone(), copier.copy_val(dst, v, None)),
                    }
                }
                if stream_kept {
                    out.push(Object::Dict(m));
                }
            }
            // A child element written in place, copied (or left out) in order.
            Object::Dict(d) if kid.as_ref().is_none() && d.contains(b"S") && !matches!(d.name(b"Type"), Some(b"MCR" | b"OBJR")) => {
                if let Some(Some(copy)) = direct.next() {
                    out.push(Object::Ref(*copy));
                }
            }
            Object::Dict(d) if d.name(b"Type") == Some(b"OBJR") => {
                let Some(obj) = d.reference(b"Obj").and_then(|o| objects.get(&o)) else { continue };
                let mut o = Dict::new();
                o.set(b"Type".to_vec(), Object::name("OBJR"));
                o.set(b"Obj".to_vec(), Object::Ref(*obj));
                if let Some(p) = d.reference(b"Pg").or(page).and_then(|p| pages.get(&p)) {
                    o.set(b"Pg".to_vec(), Object::Ref(*p));
                }
                out.push(Object::Dict(o));
            }
            _ => {}
        }
    }
    match out.len() {
        0 => None,
        1 => out.pop(),
        _ => Some(Object::Array(out)),
    }
}

/// A number tree (§7.9.7) as key → value. `None` past the limits.
fn read_number_tree(src: &Document, node: Option<&Object>) -> Option<HashMap<i64, Object>> {
    let mut out = HashMap::new();
    let Some(node) = node else { return Some(out) };
    let mut seen = HashSet::new();
    let mut stack = vec![(node.clone(), 0usize)];
    while let Some((n, depth)) = stack.pop() {
        if depth > MAX_TREE_DEPTH || out.len() > MAX_ENTRIES {
            return None;
        }
        if n.as_ref().is_some_and(|r| !seen.insert(r)) {
            continue;
        }
        let Some(d) = src.resolve(&n).as_dict().cloned() else { continue };
        if let Some(nums) = d.get(b"Nums").map(|x| src.resolve(x)).and_then(|x| x.as_array().cloned()) {
            for pair in nums.chunks(2) {
                if let [Object::Int(k), v] = pair {
                    out.insert(*k, v.clone());
                }
            }
        }
        if let Some(kids) = d.get(b"Kids").map(|x| src.resolve(x)).and_then(|x| x.as_array().cloned()) {
            stack.extend(kids.into_iter().map(|k| (k, depth + 1)));
        }
    }
    Some(out)
}

/// Check each resolved map value, the roots the copier actually copies. Each value gets a
/// fresh seen-set: the copier's seeded/global map can only shorten these conservative paths.
/// Traverse children in copier order, and bound the total work across all values.
fn within_copy_limits(src: &Document, value: &Object) -> bool {
    let Some(map) = src.resolve(value).as_dict().cloned() else { return true };
    if map.len() > MAX_ELEMENTS {
        return false;
    }
    let mut count = 0usize;
    for (_, value) in map.iter() {
        let mut pending = vec![(src.resolve(value).as_ref().clone(), 0usize)];
        let mut seen = HashSet::new();
        while let Some((value, depth)) = pending.pop() {
            count = count.saturating_add(1);
            if depth > MAX_DEPTH || count > MAX_ELEMENTS {
                return false;
            }
            match value {
                Object::Ref(r) => {
                    if seen.insert(r) {
                        pending.push((src.get(r).as_ref().clone(), depth + 1));
                    }
                }
                Object::Array(a) => {
                    if a.len().saturating_add(pending.len()) > MAX_ELEMENTS {
                        return false;
                    }
                    pending.extend(a.into_iter().rev().map(|v| (v, depth + 1)));
                }
                Object::Dict(d) => {
                    if d.len().saturating_add(pending.len()) > MAX_ELEMENTS {
                        return false;
                    }
                    pending.extend(d.iter().map(|(_, v)| (v.clone(), depth + 1)).collect::<Vec<_>>().into_iter().rev());
                }
                Object::Stream(s) => {
                    if s.dict.len().saturating_add(pending.len()) > MAX_ELEMENTS {
                        return false;
                    }
                    pending.extend(s.dict.iter().map(|(_, v)| (v.clone(), depth + 1)).collect::<Vec<_>>().into_iter().rev());
                }
                _ => {}
            }
            if pending.len() > MAX_ELEMENTS {
                return false;
            }
        }
    }
    true
}

/// Add a source's role map or class map entries to `merged`. Returns the source's names that
/// had to be renamed because an earlier source maps them to something else.
fn merge_names(merged: &mut Dict, src: &Document, map: Option<&Object>, copier: &mut Copier<'_>, dst: &mut Document) -> HashMap<Vec<u8>, Vec<u8>> {
    let mut renamed = HashMap::new();
    let Some(map) = map.map(|m| src.resolve(m)).and_then(|m| m.as_dict().cloned()) else { return renamed };
    // Resolve each entry, then copy its nested references into the result. Source object
    // numbers are not meaningful in the result (including references inside arrays).
    let entries: Vec<(Vec<u8>, Object)> = map.iter().map(|(name, value)| (name.clone(), copier.copy_val(dst, &src.resolve(value), None))).collect();
    // A role map value can name another of the source's types (mappings are transitive), so it
    // follows that type's new name; that can make one more entry differ from an earlier
    // source's: repeat until no new name collides.
    loop {
        let mut more = false;
        for (name, value) in &entries {
            if renamed.contains_key(name) {
                continue;
            }
            let value = rename(value, &renamed).unwrap_or_else(|| value.clone());
            if merged.get(name).is_some_and(|existing| *existing != value) {
                let mut n = 2usize;
                let new = loop {
                    let candidate = [name.as_slice(), format!("_{n}").as_bytes()].concat();
                    // Not a name already in the result, given to another entry, or used by this source.
                    let taken = merged.contains(&candidate) || renamed.values().chain(entries.iter().map(|(n, _)| n)).any(|t| *t == candidate);
                    if !taken {
                        break candidate;
                    }
                    n = n.saturating_add(1);
                };
                renamed.insert(name.clone(), new);
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    for (name, value) in entries {
        let value = rename(&value, &renamed).unwrap_or(value);
        let key = renamed.get(&name).cloned().unwrap_or(name);
        if !merged.contains(&key) {
            merged.set(key, value);
        }
    }
    renamed
}

/// A structure type or class name, renamed when `renamed` says so; `None` when `v` is not a name.
fn rename(v: &Object, renamed: &HashMap<Vec<u8>, Vec<u8>>) -> Option<Object> {
    let name = v.as_name()?;
    Some(Object::Name(renamed.get(name).cloned().unwrap_or_else(|| name.to_vec())))
}

/// An ID not in `taken`, made from `id`.
fn fresh(id: &[u8], taken: &HashSet<Vec<u8>>) -> Vec<u8> {
    let mut n = 2usize;
    loop {
        let candidate = [id, format!("-{n}").as_bytes()].concat();
        if !taken.contains(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

/// Attributes (`/A`: one attribute object or an array of them and revision numbers) with the IDs
/// their table `/Headers` name renamed as `renamed` says.
fn with_headers_renamed(a: &Object, dst: &mut Document, renamed: &HashMap<Vec<u8>, Vec<u8>>) -> Object {
    if renamed.is_empty() {
        return a.clone();
    }
    let fix = |d: &mut Dict| {
        if let Some(Object::Array(h)) = d.get(b"Headers") {
            let h: Vec<Object> = h
                .iter()
                .map(|x| match x.as_string().and_then(|s| renamed.get(&s.bytes)) {
                    Some(new) => Object::String(pdfcraft_cos::PdfString::literal(new.clone())),
                    None => x.clone(),
                })
                .collect();
            d.set(b"Headers".to_vec(), Object::Array(h));
        }
    };
    let fix_one = |o: &Object, dst: &mut Document| -> Object {
        match o {
            Object::Dict(d) => {
                let mut d = d.clone();
                fix(&mut d);
                Object::Dict(d)
            }
            // Attribute objects are often indirect: the copy is this source's own, so fix it there.
            Object::Ref(r) => {
                if let Some(mut d) = dst.get(*r).as_dict().cloned() {
                    fix(&mut d);
                    dst.set(*r, Object::Dict(d));
                }
                o.clone()
            }
            other => other.clone(),
        }
    };
    match a {
        Object::Array(items) => Object::Array(items.iter().map(|o| fix_one(o, dst)).collect()),
        other => fix_one(other, dst),
    }
}
