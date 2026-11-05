//! pdfcraft-model — typed views over the PDF object graph (L2). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// Attributes a page inherits from its ancestors (ISO 32000-2 §7.7.3.4).
pub const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// The largest `/UserUnit` honoured, as in Acrobat (a larger value counts as this one).
pub const MAX_USER_UNIT: f64 = 75_000.0;

/// A leaf page and its effective (inherited) attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub obj: ObjRef,
    /// The page dictionary with inherited attributes filled in.
    pub dict: Dict,
}

/// [`Page::user_unit`] of a page dictionary (`/UserUnit` is the page's own entry; it isn't inherited).
pub fn user_unit(doc: &Document, page: &Dict) -> f64 {
    page.get(b"UserUnit").and_then(|o| doc.resolve(o).as_f64()).filter(|u| *u >= 1.0).map_or(1.0, |u| u.min(MAX_USER_UNIT))
}

fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

impl Page {
    /// The visible region (crop box clipped to the media box), in user space.
    pub fn crop(&self, doc: &Document) -> [f64; 4] {
        let media = rect(doc, self.dict.get(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0]);
        match rect(doc, self.dict.get(b"CropBox")) {
            Some(c) => {
                let r = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
                if r[2] > r[0] && r[3] > r[1] { r } else { media }
            }
            None => media,
        }
    }

    /// Clockwise rotation when displayed: 0, 90, 180 or 270. A `/Rotate` that isn't a multiple
    /// of 90 (ISO 32000-2 §7.7.3.3 requires one) counts as 0, as the renderer draws that page.
    pub fn rotation(&self, doc: &Document) -> i64 {
        let r = self.dict.get(b"Rotate").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0).rem_euclid(360);
        if r % 90 == 0 { r } else { 0 }
    }

    /// The size of the page's user-space unit in points (`/UserUnit`; it isn't inherited), read
    /// as Acrobat reads it: a number from 1 up, at most [`MAX_USER_UNIT`]; anything else is 1.
    pub fn user_unit(&self, doc: &Document) -> f64 {
        user_unit(doc, &self.dict)
    }

    /// The displayed size (width, height) in points, after `/UserUnit` and `/Rotate`.
    pub fn display_size(&self, doc: &Document) -> (f64, f64) {
        let c = self.crop(doc);
        let u = self.user_unit(doc);
        let (w, h) = ((c[2] - c[0]) * u, (c[3] - c[1]) * u);
        if self.rotation(doc) % 180 == 0 { (w, h) } else { (h, w) }
    }

    /// The matrix `[a b c d e f]` from display space (points, origin at the bottom-left of the
    /// page as shown, y up, after `/UserUnit` and `/Rotate`) to user space.
    pub fn view_matrix(&self, doc: &Document) -> [f64; 6] {
        let m = view_matrix_for(self.rotation(doc), self.crop(doc));
        let u = self.user_unit(doc);
        if u == 1.0 { m } else { [m[0] / u, m[1] / u, m[2] / u, m[3] / u, m[4], m[5]] }
    }
}

/// [`Page::view_matrix`] for a region `crop` (`[x0, y0, x1, y1]`) shown turned clockwise by
/// `rotation` (0, 90, 180 or 270): the matrix from the turned region's own display space
/// (origin at its bottom-left, y up) to the region's space. For a rectangle in user space it
/// puts an upright picture into that rectangle as the page displays it.
pub fn view_matrix_for(rotation: i64, crop: [f64; 4]) -> [f64; 6] {
    let [x0, y0, x1, y1] = crop;
    match rotation {
        90 => [0.0, 1.0, -1.0, 0.0, x1, y0],
        180 => [-1.0, 0.0, 0.0, -1.0, x1, y1],
        270 => [0.0, -1.0, 1.0, 0.0, x0, y1],
        _ => [1.0, 0.0, 0.0, 1.0, x0, y0],
    }
}

/// Leaf pages in document order, each with its inherited attributes copied in. Lenient: past a
/// [`PageTreeLimit`] the list just ends, so it may be a prefix. Callers that act on every page use
/// [`try_pages`], [`page_refs`] or [`PageLeaves`].
pub fn pages(doc: &Document) -> Vec<Page> {
    copied_pages(doc).map_while(Result::ok).collect()
}

/// [`pages`], or the limit the walk met.
pub fn try_pages(doc: &Document) -> Result<Vec<Page>, PageTreeLimit> {
    copied_pages(doc).collect()
}

/// The leaf page at `index`, with its inherited attributes; `Ok(None)` past the last page. Only
/// this page's inherited values are copied, so a page past the copy budget of a whole [`pages`]
/// walk is found. A limit met before the page is an error, never "no such page".
pub fn page(doc: &Document, index: usize) -> Result<Option<Page>, PageTreeLimit> {
    let mut budget = Limits::current().inherited_copies;
    PageLeaves::new(doc).nth_page(index)?.map(|leaf| leaf.copied(&mut budget)).transpose()
}

/// The leaf page that is object `obj` (told apart by number, as [`Document::get`] does), with its
/// inherited attributes; `Ok(None)` when no page is. As with [`page`], only that page is copied, and
/// a limit exceeded before it is reached is an error, never "not a page".
pub fn page_by_ref(doc: &Document, obj: ObjRef) -> Result<Option<Page>, PageTreeLimit> {
    let mut budget = Limits::current().inherited_copies;
    for leaf in PageLeaves::new(doc) {
        let leaf = leaf?;
        if leaf.obj.num == obj.num {
            return leaf.copied(&mut budget).map(Some);
        }
    }
    Ok(None)
}

/// The page object of each leaf page, in document order, without copying any dictionary.
pub fn page_refs(doc: &Document) -> Result<Vec<ObjRef>, PageTreeLimit> {
    PageLeaves::new(doc).map(|leaf| leaf.map(|leaf| leaf.obj)).collect()
}

/// Approximate bytes of inherited values one walk copies into leaf dictionaries. It charges the
/// inherited copies only: each leaf's own dictionary is cloned uncharged.
pub const MAX_INHERITED_COPIES: usize = 1 << 30;

/// Most page-tree nodes one walk visits.
pub const MAX_PAGE_TREE_NODES: usize = 1_000_000;

/// Most `/Kids` entries one walk reads, of any kind. Each array is read once by number, but an
/// object stream can give many numbers the same bytes, so the node cap alone doesn't bound this.
pub const MAX_KIDS_ENTRIES: usize = 4_000_000;

/// The limits a walk applies: the constants above, unless [`with_limits`] lowered them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub nodes: usize,
    pub kids_entries: usize,
    pub inherited_copies: usize,
}

impl Limits {
    pub const DEFAULT: Self = Self { nodes: MAX_PAGE_TREE_NODES, kids_entries: MAX_KIDS_ENTRIES, inherited_copies: MAX_INHERITED_COPIES };

    /// The limits walks on this thread apply now: [`Limits::DEFAULT`], unless the `test-support`
    /// feature's `with_limits` lowered them.
    pub fn current() -> Self {
        #[cfg(any(test, feature = "test-support"))]
        return LIMITS.with(std::cell::Cell::get);
        #[cfg(not(any(test, feature = "test-support")))]
        Self::DEFAULT
    }

    /// Each limit the smaller of `self`'s and `other`'s.
    #[cfg(any(test, feature = "test-support"))]
    fn min(self, other: Self) -> Self {
        Self {
            nodes: self.nodes.min(other.nodes),
            kids_entries: self.kids_entries.min(other.kids_entries),
            inherited_copies: self.inherited_copies.min(other.inherited_copies),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static LIMITS: std::cell::Cell<Limits> = const { std::cell::Cell::new(Limits::DEFAULT) };
}

/// Run `f` with walks on this thread held to `limits`, never above the limits already in force
/// (so a nested scope can't raise an outer one), and put them back afterwards, on unwind too: the
/// test seam that reaches a limit without building a tree that large. Only with the
/// `test-support` feature, which consumers enable for their tests alone.
#[cfg(any(test, feature = "test-support"))]
pub fn with_limits<T>(limits: Limits, f: impl FnOnce() -> T) -> T {
    struct Restore(Limits);
    impl Drop for Restore {
        fn drop(&mut self) {
            LIMITS.with(|l| l.set(self.0));
        }
    }
    let outer = Limits::current();
    let _restore = Restore(LIMITS.with(|l| l.replace(limits.min(outer))));
    f()
}

/// The limit a page-tree walk met before reading the whole tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageTreeLimit {
    Nodes,
    KidsEntries,
    InheritedCopies,
}

impl std::fmt::Display for PageTreeLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Nodes => "the page tree has too many nodes to read whole",
            Self::KidsEntries => "the page tree has too many /Kids entries to read whole",
            Self::InheritedCopies => "the page tree's inherited values are too large to copy into every page",
        })
    }
}

impl std::error::Error for PageTreeLimit {}

/// Approximate heap bytes of `o`: one [`Object`] per node plus string, name and key bytes.
/// Values nested deeper than the parser allows weigh as too large.
fn weight(o: &Object, depth: usize) -> usize {
    if depth > 256 {
        return usize::MAX;
    }
    let own = std::mem::size_of::<Object>();
    match o {
        Object::String(s) => own.saturating_add(s.bytes.len()),
        Object::Name(n) => own.saturating_add(n.len()),
        Object::Array(a) => a.iter().fold(own, |n, v| n.saturating_add(weight(v, depth + 1))),
        Object::Dict(d) => d.iter().fold(own, |n, (k, v)| n.saturating_add(k.len()).saturating_add(weight(v, depth + 1))),
        Object::Stream(s) => s.dict.iter().fold(own, |n, (k, v)| n.saturating_add(k.len()).saturating_add(weight(v, depth + 1))),
        _ => own,
    }
}

/// The nearest node (the node itself included) that sets one of [`INHERITABLE`], shared rather
/// than copied, and the value's weight.
#[derive(Clone)]
struct Owner {
    node: ObjRef,
    obj: std::sync::Arc<Object>,
    weight: usize,
}

type Inherited = [Option<Owner>; 4];

struct Children {
    /// The node whose direct `/Kids` this is (`direct`), or the indirect array itself.
    owner: std::sync::Arc<Object>,
    direct: bool,
    /// False only for the one-entry list the walk starts from, which isn't a `/Kids` entry.
    counted: bool,
    next: usize,
    inherited: Inherited,
}

impl Children {
    fn kids(&self) -> Option<&Vec<Object>> {
        if self.direct { self.owner.as_dict()?.get(b"Kids")?.as_array() } else { self.owner.as_array() }
    }
}

/// A node of the page tree, before anything is copied.
pub struct PageNode {
    /// At the generation the document holds it at, whatever generation `/Kids` names.
    pub obj: ObjRef,
    node: std::sync::Arc<Object>,
    inherited: Inherited,
    leaf: bool,
}

impl PageNode {
    /// The node's own dictionary.
    pub fn dict(&self) -> Option<&Dict> {
        self.node.as_dict()
    }

    /// A leaf page, rather than a page-tree node.
    pub fn is_leaf(&self) -> bool {
        self.leaf
    }

    /// The node's value for `key` (one of [`INHERITABLE`]): its own, or the nearest ancestor's.
    pub fn inherited(&self, key: &[u8]) -> Option<&Object> {
        self.owner(key)?.obj.as_dict()?.get(key)
    }

    /// The node that supplies [`Self::inherited`]`(key)`.
    pub fn inherited_from(&self, key: &[u8]) -> Option<ObjRef> {
        Some(self.owner(key)?.node)
    }

    fn owner(&self, key: &[u8]) -> Option<&Owner> {
        let i = INHERITABLE.iter().position(|k| *k == key)?;
        self.inherited.get(i)?.as_ref()
    }

    /// Copy into `into` each of [`INHERITABLE`] the node has and `into` lacks, charging a value
    /// from an ancestor its weight against `budget` first.
    pub fn fill_inherited(&self, into: &mut Dict, budget: &mut usize) -> Result<(), PageTreeLimit> {
        for (key, owner) in INHERITABLE.iter().zip(&self.inherited) {
            if into.contains(key) {
                continue;
            }
            let Some(owner) = owner else { continue };
            let Some(value) = owner.obj.as_dict().and_then(|d| d.get(key)) else { continue };
            if owner.node.num != self.obj.num {
                *budget = budget.checked_sub(owner.weight).ok_or(PageTreeLimit::InheritedCopies)?;
            }
            into.set(key.to_vec(), value.clone());
        }
        Ok(())
    }

    fn copied(&self, budget: &mut usize) -> Result<Page, PageTreeLimit> {
        let mut dict = self.dict().cloned().unwrap_or_default();
        self.fill_inherited(&mut dict, budget)?;
        Ok(Page { obj: self.obj, dict })
    }
}

/// Every node of a page tree, depth first in document order, each page-tree node before the nodes
/// under it. The one walk: [`PageLeaves`], [`pages`], [`page`] and [`page_refs`] are built on it.
///
/// - A node is a page-tree node when it is `/Type /Pages`, or has `/Kids` and isn't `/Type /Page`;
///   its `/Kids` lists nothing unless it is an array (direct, or one indirect object). Any other
///   dictionary is a leaf; anything else is skipped.
/// - Nodes are told apart by object number, as [`Document::get`] finds them, so a cycle or a
///   repeated child is visited once, at its first occurrence.
/// - One cursor per open `/Kids` array, and an indirect array shared by several nodes is read
///   under the first only: re-reading it lists nothing new, and made such trees quadratic.
/// - Past [`Limits::nodes`] nodes or [`Limits::kids_entries`] `/Kids` entries the walk yields one
///   `Err`, then nothing; it never ends as if the tree were whole.
pub struct PageNodes<'a> {
    doc: &'a Document,
    seen: std::collections::HashSet<u32>,
    seen_kids: std::collections::HashSet<u32>,
    pending: Vec<Children>,
    examined: usize,
    limits: Limits,
}

impl<'a> PageNodes<'a> {
    /// The nodes of the catalog's `/Pages` tree; none when there is no such tree.
    pub fn new(doc: &'a Document) -> Self {
        Self::from(doc, doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))))
    }

    fn from(doc: &'a Document, top: Option<ObjRef>) -> Self {
        let pending = top
            .into_iter()
            .map(|r| Children {
                owner: std::sync::Arc::new(Object::Array(vec![Object::Ref(r)])),
                direct: false,
                counted: false,
                next: 0,
                inherited: Default::default(),
            })
            .collect();
        Self { doc, seen: Default::default(), seen_kids: Default::default(), pending, examined: 0, limits: Limits::current() }
    }

    /// `/Kids` entries read so far.
    pub fn entries_read(&self) -> usize {
        self.examined
    }

    fn cut_off(&mut self, limit: PageTreeLimit) -> Option<Result<PageNode, PageTreeLimit>> {
        self.pending.clear();
        Some(Err(limit))
    }
}

impl Iterator for PageNodes<'_> {
    type Item = Result<PageNode, PageTreeLimit>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(parent) = self.pending.last_mut() {
            let Some(child) = parent.kids().and_then(|kids| kids.get(parent.next)) else {
                self.pending.pop();
                continue;
            };
            let node = child.as_ref();
            // A successful get means next is strictly below the allocated array length.
            parent.next += 1;
            if parent.counted {
                self.examined = self.examined.saturating_add(1);
            }
            if self.examined > self.limits.kids_entries {
                return self.cut_off(PageTreeLimit::KidsEntries);
            }
            let Some(node) = node else { continue };
            if self.seen.contains(&node.num) {
                continue;
            }
            // Only a node about to be visited counts, so a repeat after the last isn't a cut.
            if self.seen.len() >= self.limits.nodes {
                return self.cut_off(PageTreeLimit::Nodes);
            }
            self.seen.insert(node.num);
            let node = ObjRef { num: node.num, generation: self.doc.generation(node.num) };
            let obj = self.doc.get(node);
            let Some(d) = obj.as_dict() else { continue };
            let mut inherited = parent.inherited.clone();
            for (key, owner) in INHERITABLE.iter().zip(&mut inherited) {
                if let Some(value) = d.get(key) {
                    *owner = Some(Owner { node, obj: obj.clone(), weight: weight(value, 0) });
                }
            }
            let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
            if !is_pages {
                return Some(Ok(PageNode { obj: node, node: obj, inherited, leaf: true }));
            }
            let kids = match d.get(b"Kids") {
                Some(Object::Ref(r)) if self.seen_kids.insert(r.num) => Some((self.doc.get(*r), false)),
                Some(Object::Array(_)) => Some((obj.clone(), true)),
                _ => None,
            };
            if let Some((owner, direct)) = kids {
                self.pending.push(Children { owner, direct, counted: true, next: 0, inherited: inherited.clone() });
            }
            return Some(Ok(PageNode { obj: node, node: obj, inherited, leaf: false }));
        }
        None
    }
}

/// The leaves of [`PageNodes`].
pub struct PageLeaves<'a>(PageNodes<'a>);

impl<'a> PageLeaves<'a> {
    /// The leaves of the catalog's `/Pages` tree; none when there is no such tree.
    pub fn new(doc: &'a Document) -> Self {
        Self(PageNodes::new(doc))
    }

    /// The leaves of the tree under the node `top`.
    pub fn under(doc: &'a Document, top: ObjRef) -> Self {
        Self(PageNodes::from(doc, Some(top)))
    }

    /// The leaf at `index`; `Ok(None)` past the last. (`Iterator::nth` would count an `Err` as a
    /// leaf and then report the page missing.)
    pub fn nth_page(&mut self, index: usize) -> Result<Option<PageNode>, PageTreeLimit> {
        for (i, leaf) in self.enumerate() {
            let leaf = leaf?;
            if i == index {
                return Ok(Some(leaf));
            }
        }
        Ok(None)
    }

    /// How many leaves there are. (`Iterator::count` would count an `Err` as a leaf.)
    pub fn page_count(self) -> Result<usize, PageTreeLimit> {
        let mut n = 0usize;
        for leaf in self {
            leaf?;
            n = n.saturating_add(1);
        }
        Ok(n)
    }
}

impl Iterator for PageLeaves<'_> {
    type Item = Result<PageNode, PageTreeLimit>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.0.next()? {
                Ok(node) if !node.leaf => continue,
                item => return Some(item),
            }
        }
    }
}

/// Each leaf copied ([`pages`], [`try_pages`]), the inherited copies of the whole walk within
/// [`Limits::inherited_copies`]. Its consumers stop at the first `Err`.
fn copied_pages(doc: &Document) -> impl Iterator<Item = Result<Page, PageTreeLimit>> + '_ {
    let mut budget = Limits::current().inherited_copies;
    PageLeaves::new(doc).map(move |leaf| leaf.and_then(|leaf| leaf.copied(&mut budget)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn doc(rotate: i64) -> Document {
        doc_with(rotate, "", "")
    }

    /// `pages` and `page` add entries to the page tree node and to the page.
    fn doc_with(rotate: i64, pages: &str, page: &str) -> Document {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] /Rotate {rotate} /Resources << /Font << >> >> {pages} >>"),
            format!("<< /Type /Page /Parent 2 0 R /CropBox [10 20 210 320] {page} >>"),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    #[test]
    fn inherited_copies_stop_at_a_budget() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = d.get(ObjRef { num: 3, generation: 0 }).as_dict().unwrap().clone();
        let kids: Vec<_> = (0..200).map(|_| Object::Ref(d.add(leaf.clone()))).collect();
        let mut resources = Dict::new();
        resources.set(b"Payload".to_vec(), Object::Array(vec![Object::Int(42); 8192]));
        let resources = Object::Dict(resources);
        let each = weight(&resources, 0);
        d.update_dict(root, |dict| {
            dict.set(b"Kids".to_vec(), Object::Array(kids));
            dict.set(b"Resources".to_vec(), resources);
        })
        .unwrap();
        let budget = 1 << 20;
        let pages: Vec<Page> = with_limits(Limits { inherited_copies: budget, ..Limits::DEFAULT }, || super::pages(&d));
        let copied: usize = pages.iter().filter_map(|p| p.dict.get(b"Resources")).map(|r| weight(r, 0)).sum();
        assert!(copied <= budget, "{copied} bytes of inherited values copied into {} pages", pages.len());
        assert_eq!(pages.len(), budget / each);
        assert_eq!(super::pages(&d).len(), 200);
    }

    #[test]
    fn repeated_children_do_not_queue_resource_copies() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        d.update_dict(root, |dict| {
            // Modest fixture: the assertion measures retained work, not an OOM or timing.
            dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(leaf); 128]));
            let mut resources = Dict::new();
            resources.set(b"Payload".to_vec(), Object::Array(vec![Object::Int(42); 1024]));
            dict.set(b"Resources".to_vec(), Object::Dict(resources));
        })
        .unwrap();
        let all = pages(&d);
        assert_eq!(all.iter().map(|p| p.obj).collect::<Vec<_>>(), [leaf]);
        let p = &all[0];
        assert_eq!(p.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"Payload").unwrap().as_array().unwrap().len(), 1024);
        let mut walk = PageLeaves::new(&d);
        assert_eq!(walk.next().map(|l| l.map(|l| l.obj)), Some(Ok(leaf)));
        assert!(walk.0.pending.len() <= 2, "duplicate edges retained {} pending entries", walk.0.pending.len());
        assert!(walk.next().is_none());
    }

    #[test]
    fn shared_children_keep_depth_first_order_and_nearest_attributes() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        let mut second = d.get(leaf).as_dict().unwrap().clone();
        second.set(b"Rotate".to_vec(), Object::Int(180));
        let second = d.add(second);
        let mut branch = Dict::new();
        branch.set(b"Type".to_vec(), Object::name("Pages"));
        branch.set(b"Rotate".to_vec(), Object::Int(90));
        // A cycle, a shared child, an invalid entry, and an indirect Kids array.
        let kids = d.add(Object::Array(vec![Object::Ref(root), Object::Null, Object::Ref(leaf), Object::Ref(second)]));
        branch.set(b"Kids".to_vec(), Object::Ref(kids));
        let branch = d.add(branch);
        d.update_dict(root, |dict| {
            dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(branch), Object::Ref(leaf), Object::Ref(second)]));
        })
        .unwrap();
        let pages = pages(&d);
        assert_eq!(pages.iter().map(|p| p.obj).collect::<Vec<_>>(), vec![leaf, second]);
        assert_eq!(pages[0].rotation(&d), 90, "first depth-first occurrence supplies inheritance");
        assert_eq!(pages[1].rotation(&d), 180, "a leaf's own attribute wins");
        assert!(pages.iter().all(|p| p.dict.contains(b"Resources")));
    }

    #[test]
    fn wide_page_tree_retains_only_ancestor_cursors() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = d.get(ObjRef { num: 3, generation: 0 }).as_dict().unwrap().clone();
        let kids: Vec<Object> = (0..64).map(|_| Object::Ref(d.add(leaf.clone()))).collect();
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(kids.clone()))).unwrap();
        let mut walk = PageLeaves::new(&d);
        for kid in kids {
            assert_eq!(walk.next().unwrap().unwrap().obj, kid.as_ref().unwrap());
            assert!(walk.0.pending.len() <= 2);
        }
        assert!(walk.next().is_none());
    }

    fn capped<T>(nodes: usize, kids_entries: usize, f: impl FnOnce() -> T) -> T {
        with_limits(Limits { nodes, kids_entries, ..Limits::DEFAULT }, f)
    }

    fn leaf_refs(walk: PageLeaves<'_>) -> Vec<ObjRef> {
        walk.map(|leaf| leaf.unwrap().obj).collect()
    }

    /// A tree whose root lists `n` copies of the page at object 3.
    fn wide(n: usize) -> (Document, ObjRef) {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = d.get(ObjRef { num: 3, generation: 0 }).as_dict().unwrap().clone();
        let kids: Vec<Object> = (0..n).map(|_| Object::Ref(d.add(leaf.clone()))).collect();
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(kids))).unwrap();
        (d, root)
    }

    /// Many page-tree nodes sharing one indirect `/Kids` array read it once, not once per node
    /// (quadratic work on a hostile file, commit 05d09de1 in annot).
    #[test]
    fn a_kids_array_shared_by_many_nodes_is_read_once() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        let shared = d.add(Object::Array(vec![Object::Ref(leaf); 1_000]));
        let nodes: Vec<Object> = (0..1_000)
            .map(|_| {
                let mut node = Dict::new();
                node.set(b"Type".to_vec(), Object::name("Pages"));
                node.set(b"Kids".to_vec(), Object::Ref(shared));
                Object::Ref(d.add(node))
            })
            .collect();
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(nodes))).unwrap();
        let mut walk = PageLeaves::new(&d);
        assert_eq!(walk.by_ref().map(|l| l.unwrap().obj).collect::<Vec<_>>(), [leaf]);
        assert!(walk.0.entries_read() <= 3_000, "{} kids entries read", walk.0.entries_read());
        assert_eq!(page_refs(&d), Ok(vec![leaf]));
        assert_eq!(pages(&d).iter().map(|p| p.obj).collect::<Vec<_>>(), [leaf]);
    }

    /// Every `/Kids` entry counts toward the cap, references or not, and meeting it is an error,
    /// not a shorter list.
    #[test]
    fn entries_that_are_not_references_count_toward_the_cap() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        let mut kids = vec![Object::Null; 100];
        kids.push(Object::Ref(leaf));
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(kids))).unwrap();
        let mut walk = capped(10, 50, || PageLeaves::under(&d, root));
        assert_eq!(walk.next().map(|l| l.err()), Some(Some(PageTreeLimit::KidsEntries)), "the walk ends at the cap, before the page");
        assert!(walk.next().is_none(), "and then ends");
        assert!(walk.0.entries_read() <= 51, "{} entries read", walk.0.entries_read());
        assert_eq!(leaf_refs(PageLeaves::under(&d, root)), [leaf]);
    }

    /// The cap counts the tree's own `/Kids` entries, not the start: a tree with exactly that many
    /// lists all its pages.
    #[test]
    fn the_entry_cap_counts_only_the_tree_s_kids_entries() {
        let (d, root) = wide(3);
        assert_eq!(capped(100, 3, || PageLeaves::under(&d, root)).page_count(), Ok(3), "exactly at the cap");
        assert_eq!(capped(100, 2, || PageLeaves::under(&d, root)).page_count(), Err(PageTreeLimit::KidsEntries), "one past the cap");
    }

    /// A walk that meets either cap ends with that limit as an error, after the leaves before it;
    /// a repeated entry or a null after the last node is not a node left unvisited.
    #[test]
    fn a_walk_cut_off_at_a_cap_says_so() {
        let (mut d, root) = wide(3);
        let whole: Vec<_> = capped(100, 3, || PageLeaves::under(&d, root)).collect();
        assert_eq!(whole.len(), 3);
        assert!(whole.iter().all(Result::is_ok), "exactly at the entry cap");
        let entries: Vec<_> = capped(100, 2, || PageLeaves::under(&d, root)).map(|l| l.map(|l| l.obj.num)).collect();
        assert_eq!(entries, [Ok(4), Ok(5), Err(PageTreeLimit::KidsEntries)]);
        let nodes: Vec<_> = capped(2, 100, || PageLeaves::under(&d, root)).map(|l| l.map(|l| l.obj.num)).collect();
        assert_eq!(nodes, [Ok(4), Err(PageTreeLimit::Nodes)]);
        let kids = d.get(root).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().clone();
        let mut repeated = kids.clone();
        repeated.extend([kids[0].clone(), Object::Null]);
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(repeated))).unwrap();
        assert_eq!(capped(4, 100, || PageLeaves::under(&d, root)).page_count(), Ok(3), "the root and its three pages are every node");
    }

    /// `nth_page` and `page` report a limit met before the page, never "no such page".
    #[test]
    fn a_page_behind_a_cap_is_an_error_not_missing() {
        let (d, root) = wide(3);
        assert_eq!(capped(100, 2, || PageLeaves::under(&d, root)).nth_page(1).map(|l| l.map(|l| l.obj.num)), Ok(Some(5)));
        assert_eq!(capped(100, 2, || PageLeaves::under(&d, root)).nth_page(2).map(|l| l.is_some()), Err(PageTreeLimit::KidsEntries));
        assert_eq!(capped(100, 100, || PageLeaves::under(&d, root)).nth_page(3).map(|l| l.is_some()), Ok(false));
        assert_eq!(page(&d, 3), Ok(None));
    }

    /// `3 1 R` names object 3, as `3 0 R` does ([`Document::get`] goes by number): one page, once,
    /// at the generation the document holds it at, whichever alias comes first.
    #[test]
    fn references_differing_only_in_generation_are_one_page() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let aliases: Vec<Object> = (0..5).rev().map(|generation| Object::Ref(ObjRef { num: 3, generation })).collect();
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(aliases))).unwrap();
        assert_eq!(page_refs(&d), Ok(vec![ObjRef { num: 3, generation: 0 }]));
        assert_eq!(pages(&d).iter().map(|p| p.obj).collect::<Vec<_>>(), [ObjRef { num: 3, generation: 0 }]);
    }

    /// `/Kids` that refers to a dictionary lists no pages, whatever that dictionary holds.
    #[test]
    fn kids_referring_to_a_dictionary_list_nothing() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        let mut hidden = Dict::new();
        hidden.set(b"Type".to_vec(), Object::name("Page"));
        let hidden = d.add(hidden);
        let mut decoy = Dict::new();
        decoy.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(hidden)]));
        let decoy = d.add(decoy);
        let mut branch = Dict::new();
        branch.set(b"Type".to_vec(), Object::name("Pages"));
        branch.set(b"Kids".to_vec(), Object::Ref(decoy));
        let branch = d.add(branch);
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(branch), Object::Ref(leaf)]))).unwrap();
        assert_eq!(page_refs(&d), Ok(vec![leaf]));
    }

    /// A `/Type /Pages` node without `/Kids` lists no pages; it is not a page itself.
    #[test]
    fn a_pages_node_without_kids_is_not_a_page() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        let mut empty = Dict::new();
        empty.set(b"Type".to_vec(), Object::name("Pages"));
        empty.set(b"Count".to_vec(), Object::Int(0));
        let empty = d.add(empty);
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(empty), Object::Ref(leaf)]))).unwrap();
        assert_eq!(page_refs(&d), Ok(vec![leaf]));
        assert_eq!(pages(&d).iter().map(|p| p.obj).collect::<Vec<_>>(), [leaf]);
        // The node walk still sees it, as a page-tree node.
        let nodes: Vec<(u32, bool)> = PageNodes::new(&d).map(|n| n.map(|n| (n.obj.num, n.is_leaf()))).collect::<Result<_, _>>().unwrap();
        assert_eq!(nodes, [(2, false), (empty.num, false), (3, true)]);
    }

    /// A leaf carries the nodes that set its inherited attributes, not copies of them.
    #[test]
    fn a_leaf_reads_inherited_values_without_copying_them() {
        let d = doc_with(90, "", "/MediaBox [0 0 100 100]");
        let leaf = PageLeaves::new(&d).next().unwrap().unwrap();
        assert_eq!(leaf.obj, ObjRef { num: 3, generation: 0 });
        assert_eq!(leaf.inherited(b"Rotate").and_then(Object::as_int), Some(90), "from the tree node");
        assert_eq!(leaf.inherited_from(b"Rotate"), Some(ObjRef { num: 2, generation: 0 }));
        let media = leaf.inherited(b"MediaBox").and_then(Object::as_array).map(|m| m.iter().filter_map(Object::as_int).collect::<Vec<_>>());
        assert_eq!(media, Some(vec![0, 0, 100, 100]), "the page's own value wins");
        assert_eq!(leaf.inherited_from(b"MediaBox"), Some(leaf.obj));
        assert!(leaf.inherited(b"Resources").is_some_and(|r| r.as_dict().is_some()));
        assert_eq!(leaf.inherited(b"Contents"), None, "not inheritable");
        assert!(leaf.dict().is_some_and(|dict| !dict.contains(b"Rotate")), "the page's own dictionary is untouched");
    }

    /// The seam only lowers the limits, and puts them back afterwards.
    /// The seam only lowers the limits, never below an outer scope's, and puts them back on return
    /// and on unwind; another thread keeps the defaults.
    #[test]
    fn with_limits_only_lowers_them_and_puts_them_back() {
        let lowered = with_limits(Limits { nodes: usize::MAX, kids_entries: 1, inherited_copies: usize::MAX }, Limits::current);
        assert_eq!(lowered, Limits { kids_entries: 1, ..Limits::DEFAULT });
        let outer = Limits { nodes: 1, ..Limits::DEFAULT };
        let nested = with_limits(outer, || with_limits(Limits { kids_entries: 2, ..Limits::DEFAULT }, Limits::current));
        assert_eq!(nested, Limits { nodes: 1, kids_entries: 2, ..Limits::DEFAULT }, "a nested scope can't raise the outer node limit");
        let other_thread = with_limits(outer, || std::thread::spawn(Limits::current).join().unwrap());
        assert_eq!(other_thread, Limits::DEFAULT);
        let unwound = std::panic::catch_unwind(|| with_limits(outer, || panic!("inside the scope")));
        assert!(unwound.is_err());
        assert_eq!(Limits::current(), Limits::DEFAULT, "restored on unwind");
    }

    /// `page` copies one page's inherited values only, so it finds a page past the copy budget of
    /// a whole walk, with its inherited rotation; `try_pages` refuses the walk the budget cuts.
    #[test]
    fn one_page_is_found_past_the_budget_of_a_whole_walk() {
        let mut d = doc(90);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = d.get(ObjRef { num: 3, generation: 0 }).as_dict().unwrap().clone();
        let kids: Vec<Object> = (0..3).map(|_| Object::Ref(d.add(leaf.clone()))).collect();
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(kids))).unwrap();
        let tree = d.get(root);
        let tree = tree.as_dict().unwrap();
        let one_page: usize = INHERITABLE.iter().filter_map(|k| tree.get(k).map(|v| weight(v, 0))).sum();
        let copies = Limits { inherited_copies: one_page, ..Limits::DEFAULT };
        assert_eq!(with_limits(copies, || pages(&d).len()), 1);
        assert_eq!(with_limits(copies, || try_pages(&d).map(|p| p.len())), Err(PageTreeLimit::InheritedCopies));
        assert_eq!(with_limits(copies, || page(&d, 2).map(|p| p.map(|p| p.rotation(&d)))), Ok(Some(90)), "the same budget, one page");
        assert_eq!(try_pages(&d).map(|p| p.len()), Ok(3));
    }

    fn apply(m: [f64; 6], x: f64, y: f64) -> [f64; 2] {
        [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
    }

    #[test]
    fn inherited_attributes_and_display_geometry() {
        for rotate in [0, 90, 180, 270, 450] {
            let d = doc(rotate);
            let p = &pages(&d)[0];
            assert!(p.dict.contains(b"Resources") && p.dict.contains(b"MediaBox"));
            assert_eq!(p.crop(&d), [10.0, 20.0, 210.0, 320.0]);
            let (w, h) = p.display_size(&d);
            let m = p.view_matrix(&d);
            // The displayed page's corners map onto the crop box's corners.
            let corners: Vec<[f64; 2]> = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| apply(m, *x, *y)).collect();
            for c in &corners {
                assert!([10.0, 210.0].contains(&c[0]) && [20.0, 320.0].contains(&c[1]), "{rotate}: {c:?}");
            }
            // The displayed top-left corner is where a reader sees it.
            let tl = apply(m, 0.0, h);
            let expect = match rotate % 360 {
                0 => [10.0, 320.0],
                90 => [10.0, 20.0],
                180 => [210.0, 20.0],
                _ => [210.0, 320.0],
            };
            assert_eq!(tl, expect, "rotation {rotate}");
        }
    }

    /// A `/Rotate` that isn't a quarter turn counts as 0, as the renderer draws the page
    /// upright, so display space matches what's on screen.
    #[test]
    fn rotate_off_a_quarter_turn_counts_as_upright() {
        for rotate in [135, 30, -45, 359] {
            let d = doc(rotate);
            let p = &pages(&d)[0];
            assert_eq!(p.rotation(&d), 0, "{rotate}");
            assert_eq!(p.display_size(&d), (200.0, 300.0), "{rotate}");
            assert_eq!(p.view_matrix(&d), [1.0, 0.0, 0.0, 1.0, 10.0, 20.0], "{rotate}");
        }
        let d = doc(-90);
        assert_eq!(pages(&d)[0].rotation(&d), 270);
    }

    /// Display space is in points: `/UserUnit 2` doubles it. As in Acrobat, the value isn't
    /// inherited, values below 1 or not numbers count as 1, and values over 75,000 as 75,000.
    #[test]
    fn user_unit_scales_display_space() {
        for rotate in [0, 90, 180, 270] {
            let d = doc_with(rotate, "", "/UserUnit 2");
            let p = &pages(&d)[0];
            let (w, h) = p.display_size(&d);
            assert_eq!((w, h), if rotate % 180 == 0 { (400.0, 600.0) } else { (600.0, 400.0) }, "{rotate}");
            let plain = doc(rotate);
            let unscaled = pages(&plain)[0].view_matrix(&plain);
            for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (37.0, 81.0)] {
                assert_eq!(apply(p.view_matrix(&d), x, y), apply(unscaled, x / 2.0, y / 2.0), "{rotate}: ({x}, {y})");
            }
        }
        for (pages_attrs, page_attrs, unit) in [
            ("/UserUnit 2", "", 1.0),
            ("", "/UserUnit 1.5", 1.5),
            ("", "/UserUnit 0.5", 1.0),
            ("", "/UserUnit 0", 1.0),
            ("", "/UserUnit -2", 1.0),
            ("", "/UserUnit /Two", 1.0),
            ("", "/UserUnit 100000", MAX_USER_UNIT),
        ] {
            let d = doc_with(0, pages_attrs, page_attrs);
            assert_eq!(pages(&d)[0].user_unit(&d), unit, "{pages_attrs} {page_attrs}");
        }
    }
}
