//! The paragraphs of a tagged PDF's logical structure (ISO 32000-2 §14.7), as seen from a page's
//! marked content: Word tags each paragraph as a `P` (or `H1`…) element and marks each run on the
//! page as a `/Span` with an `/MCID`, so the content stream alone does not say where one paragraph
//! ends. The structure does, and Edit text uses it to keep Word's paragraphs apart.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// Structure types that hold a paragraph of their own (§14.8.4).
const BLOCKS: &[&[u8]] =
    &[b"P", b"H", b"H1", b"H2", b"H3", b"H4", b"H5", b"H6", b"LI", b"LBody", b"Lbl", b"TD", b"TH", b"Caption", b"Title", b"BlockQuote", b"Note"];
/// How far up from a marked-content item, and down a number tree, the structure is followed.
const MAX_DEPTH: usize = 32;
/// The most marked-content ids read for one page.
const MAX_IDS: usize = 65_536;
/// The most number-tree nodes visited for one page: a tree whose nodes list the same kid over and
/// over must not be walked along every path.
const MAX_NODES: usize = 4_096;

/// For each marked-content id on the page `page` (object `page_ref`, through its `/StructParents`
/// entry in the parent tree), the object number of the paragraph-level element that holds it.
/// Empty for an untagged page, or one whose structure can't be read. An element that doesn't say
/// it holds the id on this page (see [`holds`]) is left out.
pub(super) fn paragraph_of_mcids(doc: &Document, page: &Dict, page_ref: ObjRef) -> HashMap<i64, u32> {
    let mut out = HashMap::new();
    let Some(key) = page.get(b"StructParents").and_then(Object::as_int) else { return out };
    let Some(catalog) = doc.trailer().get(b"Root").and_then(|r| doc.dict(r)) else { return out };
    let Some(root) = catalog.get(b"StructTreeRoot").and_then(|r| doc.dict(r)) else { return out };
    let Some(tree) = root.get(b"ParentTree").and_then(|r| doc.dict(r)) else { return out };
    let role_map = root.get(b"RoleMap").and_then(|r| doc.dict(r)).unwrap_or_default();
    let mut walk = Walk { seen: HashSet::new(), nodes: 0 };
    let Some(items) = number_tree_get(doc, &tree, key, 0, &mut walk) else { return out };
    let items = doc.resolve(&items);
    let Some(items) = items.as_array() else { return out };
    for (mcid, item) in items.iter().enumerate().take(MAX_IDS) {
        let Ok(mcid) = i64::try_from(mcid) else { continue };
        if !item.as_ref().and_then(|r| doc.dict(&Object::Ref(r))).is_some_and(|element| holds(doc, &element, mcid, page_ref)) {
            continue;
        }
        if let Some(block) = block_of(doc, item, &role_map) {
            out.insert(mcid, block);
        }
    }
    out
}

/// Whether structure element `element` says it holds marked-content id `mcid` of page `page`: its
/// `/K` lists the id, or the page it names (`/Pg`, its own or the nearest parent's) is this one.
/// Two merged pages can share a `/StructParents` key, and their ids then point at the other
/// page's elements.
fn holds(doc: &Document, element: &Dict, mcid: i64, page: ObjRef) -> bool {
    let lists = |k: &Object| match k {
        Object::Int(n) => *n == mcid,
        other => doc.dict(other).is_some_and(|mcr| mcr.get(b"MCID").and_then(Object::as_int) == Some(mcid)),
    };
    let kids = element.get(b"K").map(|k| doc.resolve(k));
    let listed = match kids.as_deref() {
        Some(Object::Array(kids)) => kids.iter().take(MAX_IDS).any(lists),
        Some(k) => lists(k),
        None => false,
    };
    if listed {
        return true;
    }
    let mut at = element.clone();
    for _ in 0..MAX_DEPTH {
        if let Some(pg) = at.get(b"Pg").and_then(Object::as_ref) {
            return pg == page;
        }
        let Some(parent) = at.get(b"P").and_then(|p| doc.dict(p)) else { return false };
        at = parent;
    }
    false
}

/// The number-tree nodes visited so far for one lookup, and how many: see [`MAX_NODES`].
struct Walk {
    seen: HashSet<u32>,
    nodes: usize,
}

/// The value for `key` in a number tree (§7.9.7), following at most [`MAX_DEPTH`] levels of kids,
/// each node once, and at most [`MAX_NODES`] nodes.
fn number_tree_get(doc: &Document, node: &Dict, key: i64, depth: usize, walk: &mut Walk) -> Option<Object> {
    walk.nodes = walk.nodes.saturating_add(1);
    if depth > MAX_DEPTH || walk.nodes > MAX_NODES {
        return None;
    }
    if let Some(nums) = node.get(b"Nums").map(|n| doc.resolve(n)) {
        let nums = nums.as_array()?;
        return nums.as_chunks::<2>().0.iter().find(|[k, _]| k.as_int() == Some(key)).map(|[_, value]| value.clone());
    }
    let kids = node.get(b"Kids").map(|k| doc.resolve(k))?;
    for kid in kids.as_array()? {
        if let Some(r) = kid.as_ref()
            && !walk.seen.insert(r.num)
        {
            continue;
        }
        let Some(kid) = doc.dict(kid) else { continue };
        let limits = kid.get(b"Limits").map(|l| doc.resolve(l));
        let within = limits.as_ref().and_then(|l| l.as_array()).is_none_or(|l| {
            let bound = |i: usize| l.get(i).and_then(Object::as_int);
            bound(0).is_none_or(|lo| key >= lo) && bound(1).is_none_or(|hi| key <= hi)
        });
        if within && let Some(found) = number_tree_get(doc, &kid, key, depth + 1, walk) {
            return Some(found);
        }
    }
    None
}

/// The object number of the paragraph-level element at or above structure element `item`.
fn block_of(doc: &Document, item: &Object, role_map: &Dict) -> Option<u32> {
    let mut at = item.as_ref()?;
    for _ in 0..MAX_DEPTH {
        let element = doc.dict(&Object::Ref(at))?;
        let kind = element.get(b"S").and_then(Object::as_name)?;
        // A custom type maps to a standard one through the role map (one step is enough for
        // the producers that use it).
        let standard = role_map.get(kind).and_then(Object::as_name).unwrap_or(kind);
        if BLOCKS.contains(&standard) || BLOCKS.contains(&kind) {
            return Some(at.num);
        }
        at = element.get(b"P").and_then(Object::as_ref)?;
    }
    None
}
