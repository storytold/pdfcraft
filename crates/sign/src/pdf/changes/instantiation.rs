//! Pages added by instantiating page templates, which DocMDP permissions 2 and 3 allow (ISO
//! 32000-2 §12.8.2.2, Table 257: "filling in forms, instantiating page templates, and signing").
//!
//! A page template is a named page (§12.7.7): one the name dictionary's `/Templates` tree names,
//! kept out of the page tree, or a visible page its `/Pages` tree names (§7.7.4, Table 32). A
//! script adds the named page to the document "as a regular page" (§12.7.7), either as a copy, or
//! by moving a hidden template's own page into the page tree and its name from `/Templates` to
//! `/Pages`. Either way the page shows what the template shows: its content and resources are the
//! template's, as the same objects or as copies equal to them value for value (the page records
//! the template it came from in `/TemplateInstantiated`, Table 31). Its widgets and fields are a
//! form change and are judged as one.
//!
//! [`explained`] lists the objects a later revision changed only to add such pages: the pages, the
//! page-tree nodes and `/Kids` arrays that now list them, the signed pages whose `/Parent` or
//! inherited attributes the tree's rewriting moved without changing what they show, and, for a
//! template shown in place, the name trees it moved between. Anything else about the page tree —
//! a page removed or reordered, a signed page changed, a page that shows something its template
//! does not — leaves the list empty, and every change is judged as before.

use std::cell::Cell;

use super::{Dict, Document, HashMap, HashSet, ObjRef, Object, same_entries, same_object};

/// The page attributes a page inherits from its ancestors (ISO 32000-2 §7.7.3.4).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
/// Ancestors followed for an inherited attribute.
const MAX_INHERITANCE: usize = 64;
/// Names read from one name tree, and its nesting.
const MAX_NAMES: usize = 1 << 16;
const MAX_NAME_DEPTH: u8 = 32;
/// Page-tree nodes and `/Kids` entries read from the new page tree.
const MAX_NODES: usize = 1_000_000;
const MAX_KIDS: usize = 4_000_000;
/// Objects compared, over every page and every template tried, and how deep one comparison goes.
const MAX_COMPARED: usize = 200_000;
const MAX_COMPARE_DEPTH: u8 = 64;

/// The objects `doc` changed or added since the signed revision `old` only to add pages
/// instantiated from the signed revision's page templates; empty when the page tree changed in
/// any other way, or when no page was added. `signed_nodes` and `signed_kids` are the signed page
/// tree's nodes and `/Kids` array objects.
pub(super) fn explained(doc: &Document, old: &Document, signed_nodes: &HashSet<u32>, signed_kids: &HashSet<u32>) -> HashSet<u32> {
    explain(doc, old, signed_nodes, signed_kids).unwrap_or_default()
}

fn explain(doc: &Document, old: &Document, signed_nodes: &HashSet<u32>, signed_kids: &HashSet<u32>) -> Option<HashSet<u32>> {
    let signed_pages = pdfcraft_annot::page_refs(old).ok()?;
    let pages = pdfcraft_annot::page_refs(doc).ok()?;
    let signed: HashSet<u32> = signed_pages.iter().map(|r| r.num).collect();
    let (kept, added): (Vec<ObjRef>, Vec<ObjRef>) = pages.iter().copied().partition(|r| signed.contains(&r.num));
    // Only additions: every signed page is still there, in its order, and none is listed twice.
    if added.is_empty() || kept != signed_pages {
        return None;
    }
    let hidden = name_tree(old, b"Templates")?;
    let named = name_tree(old, b"Pages")?;
    let templates = Templates::of(old, &hidden, &named, &signed);
    // The hidden templates by object, the first entry naming each.
    let mut hidden_at: HashMap<u32, &(Vec<u8>, ObjRef)> = HashMap::new();
    for entry in &hidden.entries {
        hidden_at.entry(entry.1.num).or_insert(entry);
    }
    let budget = Cell::new(MAX_COMPARED);
    let mut explained = HashSet::new();
    let mut shown = Vec::new();
    let mut seen_added = HashSet::new();
    for page in &added {
        if !seen_added.insert(page.num) {
            return None;
        }
        let now = doc.get(*page);
        let now = now.as_dict()?;
        if now.name(b"Type") != Some(b"Page") {
            return None;
        }
        if existed(old, page.num) {
            // A hidden template's own page, moved into the page tree.
            let (key, template) = hidden_at.get(&page.num)?;
            let before = old.get(*template);
            let before = before.as_dict()?;
            // A template with no resources of its own would draw with the page tree's.
            if !shown_in_place(now, before) || (!before.contains(b"Resources") && inherited(doc, *page, b"Resources").is_some()) {
                return None;
            }
            shown.push((key.clone(), *template));
        } else {
            explained.extend(copied(doc, old, *page, now, &templates, &budget)?);
        }
        explained.insert(page.num);
    }
    if !shown.is_empty() {
        explained.extend(moved_names(doc, old, &hidden, &named, &shown)?);
    }
    explained.extend(page_tree(doc, old, &pages, &kept, signed_nodes, signed_kids, &budget)?);
    Some(explained)
}

/// Whether the signed revision holds object `num`.
fn existed(old: &Document, num: u32) -> bool {
    old.try_get(num).ok().is_some_and(|o| !matches!(*o, Object::Null))
}

/// A hidden template's page (`before`, `/Type /Template` with no `/Parent`, §12.7.7) now in the
/// page tree: labelled a page, given a parent, and perhaps the media box, crop box or rotation it
/// had none of (its page tree supplies them); every other entry, content and resources included,
/// as it was.
fn shown_in_place(now: &Dict, before: &Dict) -> bool {
    const GIVEN: [&[u8]; 3] = [b"MediaBox", b"CropBox", b"Rotate"];
    before.name(b"Type") == Some(b"Template")
        && !before.contains(b"Parent")
        && GIVEN.iter().all(|k| !before.contains(k) || same_entry_of(now, before, k))
        && same_entries(now, before, &[b"Type", b"Parent", b"MediaBox", b"CropBox", b"Rotate"])
}

fn same_entry_of(a: &Dict, b: &Dict, key: &[u8]) -> bool {
    super::same_entry(a.get(key), b.get(key))
}

/// The signed revision's page templates a new page can be a copy of (§12.7.7): those its
/// `/Templates` tree names that are hidden templates (`/Type /Template`), and those its `/Pages`
/// tree names that are pages of its page tree.
struct Templates<'a> {
    all: Vec<ObjRef>,
    by_name: HashMap<&'a [u8], Vec<ObjRef>>,
}

impl<'a> Templates<'a> {
    fn of(old: &Document, hidden: &'a NameTree, named: &'a NameTree, signed_pages: &HashSet<u32>) -> Self {
        let is_template = |r: ObjRef| old.get(r).as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Template"));
        let eligible =
            hidden.entries.iter().filter(|(_, r)| is_template(*r)).chain(named.entries.iter().filter(|(_, r)| signed_pages.contains(&r.num)));
        let mut templates = Templates { all: Vec::new(), by_name: HashMap::new() };
        for (name, r) in eligible {
            templates.all.push(*r);
            templates.by_name.entry(name.as_slice()).or_default().push(*r);
        }
        templates
    }
}

/// The new objects of `page`, a new page (`now`), when it is a copy of one of the signed
/// `templates`: the one its `/TemplateInstantiated` names, or, without one, any of them. `None`
/// when it copies none.
fn copied(doc: &Document, old: &Document, page: ObjRef, now: &Dict, templates: &Templates, budget: &Cell<usize>) -> Option<HashSet<u32>> {
    let instantiated = now.get(b"TemplateInstantiated");
    let wanted = match instantiated {
        Some(Object::Name(name)) => Some(name.as_slice()),
        Some(_) => return None,
        None => None,
    };
    let mut fresh = HashSet::new();
    // Its annotations are new ones of its own (copies of the template's), each judged on its own
    // as a form field, signature or comment; never an annotation the signed revision placed.
    match now.get(b"Annots") {
        None => {}
        Some(list) => {
            if let Some(r) = list.as_ref() {
                if existed(old, r.num) {
                    return None;
                }
                fresh.insert(r.num);
            }
            for entry in doc.resolve(list).as_array()? {
                if entry.as_ref().is_none_or(|r| existed(old, r.num)) {
                    return None;
                }
            }
        }
    }
    let candidates = match wanted {
        Some(name) => templates.by_name.get(name).map_or(&[][..], Vec::as_slice),
        None => templates.all.as_slice(),
    };
    for template in candidates {
        // Each template tried spends the budget, even one that differs before a value is
        // compared: pages × templates stays bounded.
        budget.set(budget.get().checked_sub(1)?);
        if let Some(found) = same_page(doc, old, page, now, *template, budget) {
            fresh.extend(found);
            return Some(fresh);
        }
    }
    None
}

/// The new objects the page `now` (object `page`) draws from, when it shows what the signed
/// template page `template` shows: every entry the same value for value, apart from its label,
/// its parent, its annotations and the name of its template; its resources, and any media box,
/// crop box or rotation the template has, the same whether its own or inherited.
fn same_page(doc: &Document, old: &Document, page: ObjRef, now: &Dict, template: ObjRef, budget: &Cell<usize>) -> Option<HashSet<u32>> {
    const OWN: [&[u8]; 5] = [b"Type", b"Parent", b"Annots", b"TemplateInstantiated", b"B"];
    let was = old.get(template);
    let was = was.as_dict()?;
    let mut compare = Compare { doc, old, budget, seen: HashSet::new(), fresh: HashSet::new() };
    let keys: HashSet<&[u8]> = now.iter().chain(was.iter()).map(|(k, _)| k.as_slice()).collect();
    for key in keys {
        if OWN.contains(&key) {
            continue;
        }
        let same = if INHERITABLE.contains(&key) {
            true
        } else {
            match (now.get(key), was.get(key)) {
                (Some(a), Some(b)) => compare.same(a, b, 0),
                _ => false,
            }
        };
        if !same {
            return None;
        }
    }
    for key in INHERITABLE {
        let same = match (inherited(doc, page, key), inherited(old, template, key)) {
            (Some(a), Some(b)) => compare.same(&a, &b, 0),
            // Resources the template lacks would give its content names to draw.
            (Some(_), None) => key != b"Resources",
            (None, None) => true,
            (None, Some(_)) => false,
        };
        if !same {
            return None;
        }
    }
    Some(compare.fresh)
}

/// Value-for-value comparison of a new page's objects (`doc`) with a template's (`old`).
struct Compare<'a> {
    doc: &'a Document,
    old: &'a Document,
    budget: &'a Cell<usize>,
    /// Pairs of objects already being compared: met again, they are taken as the same (any
    /// difference shows elsewhere), so a cycle ends. Only reference against reference: a
    /// reference met against a direct value is compared each time (two different direct values
    /// may meet the same reference), within the depth and the budget.
    seen: HashSet<(ObjRef, ObjRef)>,
    /// The new objects reached on the page's side.
    fresh: HashSet<u32>,
}

impl Compare<'_> {
    fn same(&mut self, a: &Object, b: &Object, depth: u8) -> bool {
        let Some(left) = self.budget.get().checked_sub(1) else { return false };
        self.budget.set(left);
        if depth > MAX_COMPARE_DEPTH {
            return false;
        }
        match (a, b) {
            // The template's own object, shared: whatever happened to it is judged on its own.
            (Object::Ref(x), Object::Ref(y)) if x == y => true,
            (Object::Ref(_), _) | (_, Object::Ref(_)) => {
                if let Some(r) = a.as_ref()
                    && !existed(self.old, r.num)
                {
                    self.fresh.insert(r.num);
                }
                if let (Some(x), Some(y)) = (a.as_ref(), b.as_ref())
                    && !self.seen.insert((x, y))
                {
                    return true;
                }
                let (a, b) = (self.doc.resolve(a), self.old.resolve(b));
                self.same(&a, &b, depth + 1)
            }
            (Object::Dict(x), Object::Dict(y)) => self.same_dict(x, y, depth),
            (Object::Stream(x), Object::Stream(y)) => x.raw == y.raw && self.same_dict(&x.dict, &y.dict, depth),
            (Object::Array(x), Object::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| self.same(v, w, depth + 1)),
            _ => same_object(a, b),
        }
    }

    fn same_dict(&mut self, x: &Dict, y: &Dict, depth: u8) -> bool {
        x.iter().count() == y.iter().count() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| self.same(v, w, depth + 1)))
    }
}

/// Attribute `key` of page `page`: its own, or the nearest ancestor's.
fn inherited(doc: &Document, page: ObjRef, key: &[u8]) -> Option<Object> {
    let mut seen = HashSet::new();
    let mut at = page;
    while seen.insert(at.num) && seen.len() <= MAX_INHERITANCE {
        let node = doc.get(at);
        let node = node.as_dict()?;
        if let Some(value) = node.get(key) {
            return Some(value.clone());
        }
        at = node.reference(b"Parent")?;
    }
    None
}

/// A name tree's entries that name objects, and its nodes that are objects of their own.
#[derive(Default)]
struct NameTree {
    entries: Vec<(Vec<u8>, ObjRef)>,
    nodes: HashSet<u32>,
}

/// The name dictionary's tree `key` (ISO 32000-2 §7.9.6): empty when there is none. `None` past
/// a bound on its names or its nesting, when it can't be known in full.
fn name_tree(doc: &Document, key: &[u8]) -> Option<NameTree> {
    let mut tree = NameTree::default();
    let Some(top) = names(doc).and_then(|n| n.get(key).cloned()) else { return Some(tree) };
    let mut stack = vec![(top, 0u8)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_NAME_DEPTH {
            return None;
        }
        if let Some(r) = node.as_ref()
            && !tree.nodes.insert(r.num)
        {
            continue;
        }
        let node = doc.resolve(&node);
        let Some(node) = node.as_dict() else { continue };
        if let Some(pairs) = node.get(b"Names").map(|n| doc.resolve(n)) {
            for pair in pairs.as_array().into_iter().flat_map(|a| a.as_chunks::<2>().0) {
                let [name, value] = pair;
                let (Some(name), Some(value)) = (doc.resolve(name).as_string().map(|s| s.bytes.clone()), value.as_ref()) else { continue };
                tree.entries.push((name, value));
            }
        }
        if let Some(kids) = node.get(b"Kids").map(|k| doc.resolve(k)) {
            stack.extend(kids.as_array().into_iter().flatten().map(|k| (k.clone(), depth + 1)));
        }
        if tree.entries.len() > MAX_NAMES || stack.len() > MAX_NAMES {
            return None;
        }
    }
    Some(tree)
}

/// The catalog's name dictionary, resolved.
fn names(doc: &Document) -> Option<Dict> {
    let catalog = doc.get(doc.root()?);
    let names = catalog.as_dict()?.get(b"Names")?.clone();
    doc.resolve(&names).as_dict().cloned()
}

/// The objects changed to move the templates `shown` from the signed `/Templates` tree to the
/// `/Pages` tree: the name dictionary (or the catalog holding it), and the trees' nodes. `None`
/// unless the trees now name exactly what they did, those templates moved, and nothing else in
/// the name dictionary changed.
fn moved_names(doc: &Document, old: &Document, hidden: &NameTree, named: &NameTree, shown: &[(Vec<u8>, ObjRef)]) -> Option<HashSet<u32>> {
    let sorted = |mut v: Vec<(Vec<u8>, ObjRef)>| {
        v.sort_by(|a, b| (&a.0, a.1.num, a.1.generation).cmp(&(&b.0, b.1.num, b.1.generation)));
        v
    };
    let (now_hidden, now_named) = (name_tree(doc, b"Templates")?, name_tree(doc, b"Pages")?);
    let expected_hidden = hidden.entries.iter().filter(|e| !shown.contains(e)).cloned().collect();
    let expected_named = named.entries.iter().chain(shown).cloned().collect();
    if sorted(now_hidden.entries.clone()) != sorted(expected_hidden) || sorted(now_named.entries.clone()) != sorted(expected_named) {
        return None;
    }
    let (now_names, was_names) = (names(doc)?, names(old)?);
    if !same_entries(&now_names, &was_names, &[b"Templates", b"Pages"]) {
        return None;
    }
    let mut explained = HashSet::new();
    // Each tree node is one of the signed trees', or new.
    let signed_nodes: HashSet<u32> = hidden.nodes.union(&named.nodes).copied().collect();
    for num in now_hidden.nodes.iter().chain(&now_named.nodes) {
        if existed(old, *num) {
            // A signed node changes only in what holds the names: anything else it holds (a
            // number tree's `/Nums`, were it one too) is no part of the move.
            let (now, was) = (doc.try_get(*num).ok()?, old.try_get(*num).ok()?);
            let kept = match (now.as_dict(), was.as_dict()) {
                (Some(now), Some(was)) => same_entries(now, was, &[b"Names", b"Kids", b"Limits"]),
                _ => same_object(&now, &was),
            };
            if !signed_nodes.contains(num) || !kept {
                return None;
            }
        }
        explained.insert(*num);
    }
    let catalog = |d: &Document| d.root().map(|r| d.get(r)).and_then(|c| c.as_dict().cloned());
    let (now_catalog, was_catalog) = (catalog(doc)?, catalog(old)?);
    match (now_catalog.get(b"Names"), was_catalog.get(b"Names")) {
        // The same dictionary object, rewritten in place.
        (Some(Object::Ref(a)), Some(Object::Ref(b))) if a.num == b.num => {
            explained.insert(a.num);
        }
        (now, _) => {
            // The catalog points at a new dictionary, or holds it: only that entry changed.
            if !same_entries(&now_catalog, &was_catalog, &[b"Names"]) {
                return None;
            }
            if let Some(r) = now.and_then(Object::as_ref) {
                if existed(old, r.num) {
                    return None;
                }
                explained.insert(r.num);
            }
            explained.extend(doc.root().map(|r| r.num));
        }
    }
    Some(explained)
}

/// The page-tree objects changed only to list the added pages: every node of the new tree, its
/// `/Kids` arrays, and the signed pages `kept`. `None` unless the new tree is a tree (each object
/// listed once, each listing its parent), every node counts the pages under it, a node keeps
/// every entry it had but its kids, count, parent and inherited attributes, and every signed page
/// keeps every entry but its parent and inherited attributes, and shows with the same inherited
/// attributes as when signed.
fn page_tree(
    doc: &Document,
    old: &Document,
    pages: &[ObjRef],
    kept: &[ObjRef],
    signed_nodes: &HashSet<u32>,
    signed_kids: &HashSet<u32>,
    budget: &Cell<usize>,
) -> Option<HashSet<u32>> {
    const NODE: [&[u8]; 8] = [b"Type", b"Kids", b"Count", b"Parent", b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
    let catalog = doc.get(doc.root()?);
    let top = catalog.as_dict()?.reference(b"Pages")?;
    let mut explained = HashSet::new();
    let mut listed = HashSet::from([top.num]);
    // Nodes in the order found, each with the pages and nodes it lists.
    let mut found: Vec<(ObjRef, usize, Vec<u32>)> = Vec::new();
    let mut leaves = HashSet::new();
    let mut stack = vec![top];
    let mut kids_read = 0usize;
    while let Some(node) = stack.pop() {
        if found.len() >= MAX_NODES {
            return None;
        }
        let dict = doc.get(node);
        let dict = dict.as_dict()?;
        if dict.name(b"Type") != Some(b"Pages") {
            return None;
        }
        if existed(old, node.num) {
            let before = old.get(node);
            if !signed_nodes.contains(&node.num) || !same_entries(dict, before.as_dict()?, &NODE) {
                return None;
            }
        } else if dict.iter().any(|(k, _)| !NODE.contains(&k.as_slice())) {
            return None;
        }
        let kids = dict.get(b"Kids")?;
        if let Some(r) = kids.as_ref() {
            if existed(old, r.num) && !signed_kids.contains(&r.num) {
                return None;
            }
            explained.insert(r.num);
        }
        let kids = doc.resolve(kids);
        let kids = kids.as_array()?;
        kids_read = kids_read.saturating_add(kids.len());
        if kids_read > MAX_KIDS {
            return None;
        }
        let (mut own_leaves, mut child_nodes) = (0usize, Vec::new());
        // Pushed in reverse, so pages come off the stack in their order.
        let mut below = Vec::new();
        for kid in kids {
            let kid = kid.as_ref()?;
            if !listed.insert(kid.num) {
                return None;
            }
            let kid_dict = doc.get(kid);
            let kid_dict = kid_dict.as_dict()?;
            if kid_dict.reference(b"Parent").map(|p| p.num) != Some(node.num) {
                return None;
            }
            if kid_dict.contains(b"Kids") {
                child_nodes.push(kid.num);
                below.push(kid);
            } else {
                own_leaves = own_leaves.saturating_add(1);
                leaves.insert(kid);
            }
        }
        stack.extend(below.into_iter().rev());
        found.push((node, own_leaves, child_nodes));
        explained.insert(node.num);
    }
    // The pages this walk found are the ones a reader shows (each listed once, so as many).
    if leaves.len() != pages.len() || pages.iter().any(|p| !leaves.contains(p)) {
        return None;
    }
    // Children are found after their parents: counted in reverse, each node's count is ready.
    let mut counts = std::collections::HashMap::new();
    for (node, own, children) in found.iter().rev() {
        let count = children.iter().try_fold(*own, |sum, c| counts.get(c).and_then(|n: &usize| sum.checked_add(*n)))?;
        let written = doc.get(*node).as_dict().and_then(|d| d.get(b"Count").map(|c| doc.resolve(c).as_int()))??;
        if usize::try_from(written).ok()? != count {
            return None;
        }
        counts.insert(node.num, count);
    }
    for page in kept {
        let (now, was) = (doc.get(*page), old.get(*page));
        let (now_dict, was_dict) = (now.as_dict()?, was.as_dict()?);
        if !same_entries(now_dict, was_dict, &[b"Parent", b"Resources", b"MediaBox", b"CropBox", b"Rotate"]) {
            return None;
        }
        // The values it shows, whether held directly or through references on either side (the
        // budget is spent only where the two are written differently).
        let mut compare = Compare { doc, old, budget, seen: HashSet::new(), fresh: HashSet::new() };
        let same_inherited = INHERITABLE.iter().all(|k| match (inherited(doc, *page, k), inherited(old, *page, k)) {
            (Some(a), Some(b)) => same_object(&a, &b) || compare.same(&a, &b, 0),
            (None, None) => true,
            _ => false,
        });
        if !same_inherited {
            return None;
        }
        explained.extend(compare.fresh);
        explained.insert(page.num);
    }
    Some(explained)
}
