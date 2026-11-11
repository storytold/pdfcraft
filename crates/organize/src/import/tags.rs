//! Carrying the sources' structure trees (tags) into a combined document (#7025).
//!
//! Combine copies each source's pages without their structure tree (see `copy`), so the copier
//! strips the keys that point into it. Here, once a source's pages are in, its tree follows them:
//! - each element is rebuilt in the destination with its `/P`, `/K` and `/Pg` pointing at the
//!   copies. Its other entries (`/S`, `/Alt`, `/ActualText`, `/A`, `/C`, `/Lang`, `/ID`…) are
//!   copied as they are, without following a reference to a page or to another part of the tree;
//! - content on a page that was not copied is not kept, nor is an element left with none of its
//!   content (unless the whole source came along, when every element stays);
//! - the parent tree is rebuilt from the copied elements, with new keys. A copied page or form
//!   XObject gets `/StructParents`, a copied annotation or XObject named by an object reference
//!   gets `/StructParent`. Keys a copied form XObject brought along that the new tree doesn't
//!   use are removed, so they can't collide with the new ones;
//! - each source's top-level elements go, in order, under one `Document` element. A source's own
//!   top-level `Document` element becomes a `Part`, since documents don't nest;
//! - the role and class maps are merged. Where two sources map the same name differently, the
//!   first source's entry stands. The ID tree keeps the first element with a given ID likewise;
//! - the document `/Lang` is the first tagged source's (that has one). A later source in another
//!   language puts its own on its top-level elements that don't name one.
//!
//! A source without a structure tree adds untagged pages. The PDF/UA identification is kept only
//! when every source declares the same part, is tagged, and came along whole (no page left out,
//! none of its tree cut off).
//!
//! The walk is bounded: each element is visited once (a cycle ends), nesting deeper than
//! [`MAX_DEPTH`] and marked-content IDs above [`MAX_MCID`] are not carried, and at most
//! [`MAX_NODES`] tree entries are read per source.

use super::{Dict, Document, HashMap, HashSet, ObjRef, Object, OrganizeError};
use pdfcraft_cos::PdfString;

/// How deep an element may sit below the tree root to be carried. Real trees are a few dozen
/// levels at most; the walk recurses, so this also bounds the stack.
const MAX_DEPTH: usize = 256;
/// The largest marked-content ID carried: the parent tree holds an array indexed by it.
const MAX_MCID: i64 = 1 << 20;
/// How many tree entries (elements, content items, attribute objects) one source may bring.
const MAX_NODES: usize = 1 << 22;
/// How deep an element's other entries (attributes and the like) are copied.
const MAX_VALUE_DEPTH: usize = 32;

/// The structure trees of the sources added so far, and what the combined tree needs from them.
#[derive(Default)]
pub(super) struct Tags {
    /// The `Document` element every source's elements go under, once a tagged source is added.
    document: Option<ObjRef>,
    /// The sources' top-level elements, in order.
    kids: Vec<Object>,
    /// The parent tree's entries, by key.
    parents: Vec<(i64, Object)>,
    role_map: Dict,
    class_map: Dict,
    /// The ID tree's entries: the first element with each ID.
    ids: Vec<(Vec<u8>, ObjRef)>,
    seen_ids: HashSet<Vec<u8>>,
    /// The document language: the first tagged source's `/Lang`.
    lang: Option<Vec<u8>>,
    /// How many sources were added.
    sources: usize,
    /// The PDF/UA part every source declares so far (`None` once one doesn't, isn't tagged, or
    /// didn't come along whole).
    ua: Option<String>,
    /// The first source's title, which a PDF/UA claim needs (ISO 14289-1 7.1).
    title: Option<String>,
}

/// One source being carried: its document, what was copied from it, and the new tree's pieces.
struct Walk<'a> {
    src: &'a Document,
    /// Source page → its copy (the first copy, for a page listed twice).
    pages: &'a HashMap<ObjRef, ObjRef>,
    /// Every source object the pages brought along → its copy.
    objects: &'a HashMap<ObjRef, ObjRef>,
    /// Source objects copied here (attribute dictionaries and the like) → their copies.
    values: HashMap<ObjRef, ObjRef>,
    /// Source elements visited, and the copies of those kept.
    seen: HashSet<ObjRef>,
    elements: HashMap<ObjRef, ObjRef>,
    /// Copied elements whose `/Ref` names other elements (PDF 2.0), resolved at the end.
    refs: Vec<(ObjRef, Object)>,
    /// The content each copied page or form XObject holds: its marked-content IDs and their
    /// elements, in the order first seen.
    marked: Vec<(ObjRef, Vec<(i64, ObjRef)>)>,
    marked_at: HashMap<ObjRef, usize>,
    /// Copied objects named by an object reference, and their element (the first one).
    objrs: Vec<(ObjRef, ObjRef)>,
    objr_seen: HashSet<ObjRef>,
    /// Entries read so far, against [`MAX_NODES`].
    nodes: usize,
    /// Whether anything was left out other than content on pages not copied.
    cut: bool,
    /// The source's `/IDTree` entries the copies take.
    ids: Vec<(Vec<u8>, ObjRef)>,
}

impl Tags {
    /// Carry `src`'s structure tree into `out`, after its pages were copied (`pages`: source page
    /// → copy; `objects`: source object → copy). `whole`: every page of `src` was copied.
    pub(super) fn add(
        &mut self,
        out: &mut Document,
        src: &Document,
        pages: &HashMap<ObjRef, ObjRef>,
        objects: &HashMap<ObjRef, ObjRef>,
        whole: bool,
    ) -> Result<(), OrganizeError> {
        let first = self.sources == 0;
        self.sources = self.sources.saturating_add(1);
        let catalog = src.root().and_then(|r| src.get(r).as_dict().cloned()).unwrap_or_default();
        let claimed = crate::pdfx::metadata(src, &catalog).and_then(|x| crate::pdfx::xmp_value(&x, "pdfuaid:part"));
        if first {
            self.ua = claimed.clone();
            self.title = crate::info(src, "Title").filter(|t| !t.trim().is_empty());
        } else if self.ua != claimed {
            self.ua = None;
        }
        let root = catalog.get(b"StructTreeRoot").map(|r| src.resolve(r)).and_then(|r| r.as_dict().cloned());
        let marked = catalog
            .get(b"MarkInfo")
            .map(|m| src.resolve(m))
            .and_then(|m| m.as_dict().and_then(|d| d.get(b"Marked")).map(|v| matches!(&*src.resolve(v), Object::Bool(true))))
            .unwrap_or(false);
        let Some(root) = root else {
            self.ua = None;
            clear_stale_keys(out, objects, &HashSet::new())?;
            return Ok(());
        };
        if !whole || !marked {
            self.ua = None;
        }
        let document = match self.document {
            Some(d) => d,
            None => {
                let d = out.add(Object::Null);
                self.document = Some(d);
                d
            }
        };
        let mut walk = Walk {
            src,
            pages,
            objects,
            values: HashMap::new(),
            seen: HashSet::new(),
            elements: HashMap::new(),
            refs: Vec::new(),
            marked: Vec::new(),
            marked_at: HashMap::new(),
            objr_seen: HashSet::new(),
            objrs: Vec::new(),
            nodes: 0,
            cut: false,
            ids: Vec::new(),
        };
        let lang = catalog.get(b"Lang").and_then(|l| src.resolve(l).as_string().map(|s| s.bytes.clone())).filter(|l| !l.is_empty());
        let own_lang = match (&self.lang, &lang) {
            (None, Some(l)) => {
                self.lang = Some(l.clone());
                None
            }
            (Some(doc), Some(l)) if doc != l => Some(l.clone()),
            _ => None,
        };
        for kid in list(src, root.get(b"K")) {
            if let Some(copy) = walk.element(out, &kid, document, None, 0, whole)? {
                out.update_dict(copy, |d| {
                    if d.name(b"S") == Some(b"Document") {
                        d.set(b"S".to_vec(), Object::name("Part"));
                    }
                    if let Some(l) = &own_lang
                        && !d.contains(b"Lang")
                    {
                        d.set(b"Lang".to_vec(), Object::String(PdfString { bytes: l.clone(), hex: false }));
                    }
                })?;
                self.kids.push(Object::Ref(copy));
            }
        }
        if walk.cut {
            self.ua = None;
        }
        walk.ids_from(root.get(b"IDTree"));
        walk.finish(out, self)?;
        for (key, map) in [(b"RoleMap".as_slice(), &mut self.role_map), (b"ClassMap".as_slice(), &mut self.class_map)] {
            let Some(theirs) = root.get(key).map(|m| src.resolve(m)).and_then(|m| m.as_dict().cloned()) else { continue };
            let role_map = key == b"RoleMap";
            for (name, value) in theirs.iter() {
                // A role maps to a name; anything else there is junk.
                if map.contains(name) || (role_map && !matches!(value, Object::Name(_))) {
                    continue;
                }
                let copied = walk_value(out, src, &mut HashMap::new(), objects, value, 0);
                map.set(name.clone(), copied);
            }
        }
        Ok(())
    }

    /// Write the combined structure tree into `out`, if any source was tagged, and the PDF/UA
    /// identification's part when every source keeps it (see the module notes).
    pub(super) fn finish(self, out: &mut Document) -> Result<Option<String>, OrganizeError> {
        let Some(document) = self.document else { return Ok(None) };
        let catalog = out.root().ok_or(OrganizeError::NoPageTree)?;
        let root = out.add(Object::Null);
        let mut element = Dict::new();
        element.set(b"Type".to_vec(), Object::name("StructElem"));
        element.set(b"S".to_vec(), Object::name("Document"));
        element.set(b"P".to_vec(), Object::Ref(root));
        element.set(b"K".to_vec(), Object::Array(self.kids));
        out.set(document, Object::Dict(element));
        let next_key = self.parents.len();
        let mut nums = Vec::with_capacity(self.parents.len().saturating_mul(2));
        for (key, value) in self.parents {
            nums.push(Object::Int(key));
            nums.push(value);
        }
        let mut parent_tree = Dict::new();
        parent_tree.set(b"Nums".to_vec(), Object::Array(nums));
        let parent_tree = out.add(Object::Dict(parent_tree));
        let mut tree = Dict::new();
        tree.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
        tree.set(b"K".to_vec(), Object::Ref(document));
        tree.set(b"ParentTree".to_vec(), Object::Ref(parent_tree));
        tree.set(b"ParentTreeNextKey".to_vec(), Object::Int(i64::try_from(next_key).unwrap_or(i64::MAX)));
        if !self.role_map.is_empty() {
            tree.set(b"RoleMap".to_vec(), Object::Dict(self.role_map));
        }
        if !self.class_map.is_empty() {
            tree.set(b"ClassMap".to_vec(), Object::Dict(self.class_map));
        }
        if !self.ids.is_empty() {
            let mut ids = self.ids;
            ids.sort_by(|a, b| a.0.cmp(&b.0));
            let names = ids.into_iter().flat_map(|(id, r)| [Object::String(PdfString { bytes: id, hex: false }), Object::Ref(r)]).collect();
            let mut id_tree = Dict::new();
            id_tree.set(b"Names".to_vec(), Object::Array(names));
            tree.set(b"IDTree".to_vec(), Object::Dict(id_tree));
        }
        out.set(root, Object::Dict(tree));
        let ua = self.ua.filter(|_| self.title.is_some());
        let lang = self.lang;
        out.update_dict(catalog, |c| {
            c.set(b"StructTreeRoot".to_vec(), Object::Ref(root));
            let mut mark_info = Dict::new();
            mark_info.set(b"Marked".to_vec(), Object::Bool(true));
            c.set(b"MarkInfo".to_vec(), Object::Dict(mark_info));
            if let Some(l) = lang {
                c.set(b"Lang".to_vec(), Object::String(PdfString { bytes: l, hex: false }));
            }
            if ua.is_some() {
                // PDF/UA asks a viewer to show the title rather than the file name.
                let mut prefs = c.get(b"ViewerPreferences").and_then(Object::as_dict).cloned().unwrap_or_default();
                prefs.set(b"DisplayDocTitle".to_vec(), Object::Bool(true));
                c.set(b"ViewerPreferences".to_vec(), Object::Dict(prefs));
            }
        })?;
        if ua.is_some()
            && let Some(title) = &self.title
            && crate::info(out, "Title").is_none()
        {
            crate::set_info(out, "Title", title)?;
        }
        Ok(ua)
    }
}

impl Walk<'_> {
    /// Count one tree entry against [`MAX_NODES`]; `false` once the budget is spent.
    fn spend(&mut self) -> bool {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > MAX_NODES {
            self.cut = true;
            return false;
        }
        true
    }

    /// The copy of the source page `page`, if it was copied.
    fn page(&self, page: Option<&Object>) -> Option<ObjRef> {
        page.and_then(Object::as_ref).and_then(|p| self.pages.get(&p).copied())
    }

    /// Copy the element `kid` (of the source) under `parent`, with `pg` (a source page) as the page
    /// its content is on unless it names one; `None` when it is not kept.
    fn element(
        &mut self,
        out: &mut Document,
        kid: &Object,
        parent: ObjRef,
        pg: Option<&Object>,
        depth: usize,
        whole: bool,
    ) -> Result<Option<ObjRef>, OrganizeError> {
        if depth >= MAX_DEPTH {
            self.cut = true;
            return Ok(None);
        }
        if !self.spend() {
            return Ok(None);
        }
        if let Some(r) = kid.as_ref()
            && !self.seen.insert(r)
        {
            // Listed twice, or a cycle: an element has one parent.
            self.cut = true;
            return Ok(None);
        }
        let resolved = self.src.resolve(kid);
        let Some(d) = resolved.as_dict().filter(|d| d.contains(b"S")).cloned() else { return Ok(None) };
        let copy = out.add(Object::Null);
        let pg = d.get(b"Pg").cloned().or_else(|| pg.cloned());
        let page = self.page(pg.as_ref());
        let mut kids = Vec::new();
        let listed = list(self.src, d.get(b"K"));
        for k in &listed {
            if !self.spend() {
                break;
            }
            match k {
                Object::Int(mcid) => {
                    if let Some(page) = page
                        && self.content(page, *mcid, copy)
                    {
                        kids.push(Object::Int(*mcid));
                    }
                }
                _ => {
                    let item = self.src.resolve(k);
                    let item = item.as_dict();
                    if let Some(item) = item.filter(|i| i.name(b"Type") == Some(b"MCR") || (i.contains(b"MCID") && !i.contains(b"S"))) {
                        if let Some(c) = self.marked_reference(item, pg.as_ref(), copy) {
                            kids.push(Object::Dict(c));
                        }
                    } else if let Some(item) = item.filter(|i| i.name(b"Type") == Some(b"OBJR") || (i.contains(b"Obj") && !i.contains(b"S"))) {
                        if let Some(c) = self.object_reference(item, pg.as_ref(), copy) {
                            kids.push(Object::Dict(c));
                        }
                    } else if let Some(c) = self.element(out, k, copy, pg.as_ref(), depth.saturating_add(1), whole)? {
                        kids.push(Object::Ref(c));
                    }
                }
            }
        }
        // An element with none of its content left goes; one that never had any (an empty
        // section) stays when its whole source came along.
        if kids.is_empty() && !(whole && listed.is_empty()) {
            out.free(copy);
            return Ok(None);
        }
        let mut e = Dict::new();
        for (k, v) in d.iter() {
            match k.as_slice() {
                b"P" | b"K" | b"Pg" => {}
                b"Ref" => self.refs.push((copy, v.clone())),
                _ => {
                    let v = walk_value(out, self.src, &mut self.values, self.objects, v, 0);
                    e.set(k.clone(), v);
                }
            }
        }
        e.set(b"P".to_vec(), Object::Ref(parent));
        if d.contains(b"Pg")
            && let Some(page) = page
        {
            e.set(b"Pg".to_vec(), Object::Ref(page));
        }
        if !kids.is_empty() {
            e.set(b"K".to_vec(), Object::Array(kids));
        }
        out.set(copy, Object::Dict(e));
        if let Some(r) = kid.as_ref() {
            self.elements.insert(r, copy);
        }
        Ok(Some(copy))
    }

    /// Record marked content `mcid` of the copied page or form XObject `owner` as `element`'s.
    fn content(&mut self, owner: ObjRef, mcid: i64, element: ObjRef) -> bool {
        if !(0..=MAX_MCID).contains(&mcid) {
            self.cut = true;
            return false;
        }
        let at = match self.marked_at.get(&owner) {
            Some(at) => *at,
            None => {
                self.marked.push((owner, Vec::new()));
                self.marked_at.insert(owner, self.marked.len() - 1);
                self.marked.len() - 1
            }
        };
        if let Some((_, list)) = self.marked.get_mut(at) {
            list.push((mcid, element));
        }
        true
    }

    /// A marked-content reference's copy, its content recorded, or `None` when its page or stream
    /// was not copied.
    fn marked_reference(&mut self, item: &Dict, pg: Option<&Object>, element: ObjRef) -> Option<Dict> {
        let mcid = item.get(b"MCID").and_then(|m| self.src.resolve(m).as_int())?;
        let page = self.page(item.get(b"Pg").or(pg))?;
        let mut c = Dict::new();
        c.set(b"Type".to_vec(), Object::name("MCR"));
        if item.contains(b"Pg") {
            c.set(b"Pg".to_vec(), Object::Ref(page));
        }
        let owner = match item.get(b"Stm") {
            Some(stm) => {
                let stm = stm.as_ref().and_then(|s| self.objects.get(&s).copied())?;
                c.set(b"Stm".to_vec(), Object::Ref(stm));
                if let Some(own) = item.get(b"StmOwn").and_then(Object::as_ref).and_then(|o| self.objects.get(&o).copied()) {
                    c.set(b"StmOwn".to_vec(), Object::Ref(own));
                }
                stm
            }
            None => page,
        };
        if !self.content(owner, mcid, element) {
            return None;
        }
        c.set(b"MCID".to_vec(), Object::Int(mcid));
        Some(c)
    }

    /// An object reference's copy, or `None` when its object was not copied.
    fn object_reference(&mut self, item: &Dict, pg: Option<&Object>, element: ObjRef) -> Option<Dict> {
        let obj = item.get(b"Obj").and_then(Object::as_ref)?;
        let copy = self.objects.get(&obj).copied().or_else(|| self.pages.get(&obj).copied())?;
        let mut c = Dict::new();
        c.set(b"Type".to_vec(), Object::name("OBJR"));
        c.set(b"Obj".to_vec(), Object::Ref(copy));
        if let Some(page) = self.page(item.get(b"Pg").or(pg))
            && item.contains(b"Pg")
        {
            c.set(b"Pg".to_vec(), Object::Ref(page));
        }
        if self.objr_seen.insert(copy) {
            self.objrs.push((copy, element));
        }
        Some(c)
    }

    /// The source's ID tree entries whose elements were copied (the IDs as the elements name them
    /// are what matters; the tree is read for the IDs of elements that don't).
    fn ids_from(&mut self, tree: Option<&Object>) {
        let mut entries = Vec::new();
        if let Some(tree) = tree {
            super::name_tree_entries(self.src, tree, 0, &mut HashSet::new(), &mut entries);
        }
        for (id, v) in entries {
            if let Some(copy) = v.as_ref().and_then(|r| self.elements.get(&r).copied()) {
                self.ids.push((id, copy));
            }
        }
    }

    /// Give the copied pages, streams and objects their new parent-tree keys, resolve `/Ref`, and
    /// hand the entries to `tags`.
    fn finish(self, out: &mut Document, tags: &mut Tags) -> Result<(), OrganizeError> {
        let mut keyed = HashSet::new();
        // Keys run 0, 1, 2… across the sources, one per entry.
        let next = |tags: &Tags| i64::try_from(tags.parents.len()).unwrap_or(i64::MAX);
        for (owner, list) in self.marked {
            let key = next(tags);
            let len = list.iter().map(|(m, _)| *m).max().and_then(|m| usize::try_from(m).ok()).map_or(0, |m| m.saturating_add(1));
            let mut parents = vec![Object::Null; len];
            for (mcid, element) in list {
                if let Some(slot) = usize::try_from(mcid).ok().and_then(|m| parents.get_mut(m))
                    && matches!(slot, Object::Null)
                {
                    *slot = Object::Ref(element);
                }
            }
            set_key(out, owner, b"StructParents", key)?;
            keyed.insert(owner);
            tags.parents.push((key, Object::Array(parents)));
        }
        for (owner, element) in self.objrs {
            let key = next(tags);
            set_key(out, owner, b"StructParent", key)?;
            keyed.insert(owner);
            tags.parents.push((key, Object::Ref(element)));
        }
        for (copy, refs) in self.refs {
            let refs: Vec<Object> = list(self.src, Some(&refs))
                .into_iter()
                .filter_map(|r| r.as_ref().and_then(|r| self.elements.get(&r).copied()).map(Object::Ref))
                .collect();
            if !refs.is_empty() {
                out.update_dict(copy, |d| d.set(b"Ref".to_vec(), Object::Array(refs)))?;
            }
        }
        for (id, copy) in self.ids {
            if tags.seen_ids.insert(id.clone()) {
                tags.ids.push((id, copy));
            }
        }
        clear_stale_keys(out, self.objects, &keyed)
    }
}

/// Set `key` (`/StructParents` or `/StructParent`) on the copied page, stream or annotation `r`.
fn set_key(out: &mut Document, r: ObjRef, key: &[u8], value: i64) -> Result<(), OrganizeError> {
    match &*out.get(r) {
        Object::Stream(s) => {
            let mut s = s.clone();
            s.dict.set(key.to_vec(), Object::Int(value));
            out.set(r, Object::Stream(s));
        }
        Object::Dict(_) => out.update_dict(r, |d| d.set(key.to_vec(), Object::Int(value)))?,
        _ => {}
    }
    Ok(())
}

/// Remove the parent-tree keys copied form XObjects brought along from their source, except on
/// `keyed`, the ones the new tree gave keys: left, they would name another source's entries.
fn clear_stale_keys(out: &mut Document, objects: &HashMap<ObjRef, ObjRef>, keyed: &HashSet<ObjRef>) -> Result<(), OrganizeError> {
    for copy in objects.values() {
        if keyed.contains(copy) {
            continue;
        }
        let held = out.get(*copy);
        let Object::Stream(s) = &*held else { continue };
        if !(s.dict.contains(b"StructParents") || s.dict.contains(b"StructParent")) {
            continue;
        }
        let mut s = s.clone();
        s.dict.remove(b"StructParents");
        s.dict.remove(b"StructParent");
        out.set(*copy, Object::Stream(s));
    }
    Ok(())
}

/// The entries of `value` (a kid, or an array of them; resolved), as a list.
fn list(src: &Document, value: Option<&Object>) -> Vec<Object> {
    let Some(value) = value else { return Vec::new() };
    match value {
        Object::Array(a) => a.clone(),
        Object::Ref(_) => match &*src.resolve(value) {
            Object::Array(a) => a.clone(),
            _ => vec![value.clone()],
        },
        Object::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

/// Whether `d` is part of the page tree or the structure tree, which an element's other entries
/// must not drag along.
fn structural(d: &Dict) -> bool {
    matches!(d.name(b"Type"), Some(b"Page" | b"Pages" | b"Catalog" | b"StructTreeRoot" | b"StructElem" | b"MCR" | b"OBJR"))
        || (d.contains(b"S") && d.contains(b"P"))
}

/// Copy an element's entry `v` from `src` into `out`: what the pages brought along already is
/// reused (`objects`), other objects are copied once (`values`). A reference to the page tree or
/// the structure tree, or past [`MAX_VALUE_DEPTH`], becomes null.
fn walk_value(
    out: &mut Document,
    src: &Document,
    values: &mut HashMap<ObjRef, ObjRef>,
    objects: &HashMap<ObjRef, ObjRef>,
    v: &Object,
    depth: usize,
) -> Object {
    if depth > MAX_VALUE_DEPTH {
        return Object::Null;
    }
    match v {
        Object::Ref(r) => {
            if let Some(c) = objects.get(r).or_else(|| values.get(r)) {
                return Object::Ref(*c);
            }
            let held = src.get(*r);
            let skip = match &*held {
                Object::Dict(d) => structural(d),
                Object::Stream(s) => structural(&s.dict),
                _ => false,
            };
            if skip {
                return Object::Null;
            }
            let copy = out.add(Object::Null);
            values.insert(*r, copy);
            let copied = walk_value(out, src, values, objects, &held, depth.saturating_add(1));
            out.set(copy, copied);
            Object::Ref(copy)
        }
        Object::Array(a) => Object::Array(a.iter().map(|x| walk_value(out, src, values, objects, x, depth.saturating_add(1))).collect()),
        Object::Dict(d) => {
            let mut c = Dict::new();
            for (k, x) in d.iter() {
                c.set(k.clone(), walk_value(out, src, values, objects, x, depth.saturating_add(1)));
            }
            Object::Dict(c)
        }
        Object::Stream(s) => {
            let mut s = s.clone();
            let mut c = Dict::new();
            for (k, x) in s.dict.iter() {
                c.set(k.clone(), walk_value(out, src, values, objects, x, depth.saturating_add(1)));
            }
            s.dict = c;
            Object::Stream(s)
        }
        other => other.clone(),
    }
}
