//! Linearized saving ("Fast Web View") and telling whether a file is linearized
//! (ISO 32000-2 Annex F).
//!
//! A linearized file puts what the first page needs at the front, so a viewer reading it over a
//! network shows page 1 before the rest has arrived, and its hint tables say where every other
//! page starts. [`write_linearized`] writes the parts of Annex F.3 in order:
//!
//! 1. the header;
//! 2. the linearization parameter dictionary (`/L /H /O /E /N /T`), the first object;
//! 3. the first-page cross-reference table and trailer, whose `/Prev` names the main table;
//! 4. the catalog and the document-level objects needed to open the file (F.3.4);
//! 5. the primary hint stream: the page offset, shared object and outline hint tables (F.4);
//! 6. the first page: its page object, every object it uses, and the outline when the document
//!    opens with bookmarks shown (`/PageMode /UseOutlines`);
//! 7. every other page: its page object followed by the objects only that page uses;
//! 8. the objects more than one of those pages uses;
//! 9. everything else: the page tree, thumbnails, bookmarks, document-level data;
//! 11. the main cross-reference section, whose `startxref` names part 3.
//!
//! Part 10, the overflow hint stream, is never needed: everything is laid out before the hint
//! tables are computed, and hint table offsets leave the hint stream out (F.4), so they never
//! depend on its length.
//!
//! The save is a full rewrite, like [`crate::write_full`]: objects reachable from the trailer,
//! renumbered. Two things change shape without changing meaning, both required for a page to be
//! readable from its own section: page attributes inherited from the page tree (`/Resources`,
//! `/MediaBox`, `/CropBox`, `/Rotate`, Table 31) are copied onto every page that inherits them
//! and removed from the tree, and a page with no `/Type` gets `/Type /Page`. [`Linearized`]
//! counts both. Encrypted documents are linearized with their encryption: the hint stream and
//! the object streams are encrypted like any stream, the parameter dictionary holds only
//! numbers, and cross-reference streams are never encrypted (§7.6.2).
//!
//! Which part an object belongs to follows from who uses it. Every page, every catalog entry and
//! every trailer entry is a *user*; walking from a user marks what it uses, without walking into
//! another page or up a page's `/Parent`. An object used by the first page goes in part 6; one
//! used only by one other page (and nothing else) belongs to that page; one used by several
//! other pages is shared; one used by the catalog entries needed at open goes in part 4;
//! everything else is part 9.
//!
//! With [`SaveOptions::object_streams`] (the default) both cross-reference sections are
//! cross-reference streams, and every object other than a stream, a page, the catalog and the
//! encryption dictionary with what it references is packed into an object stream (PDF 1.5);
//! without it the sections are classic tables and nothing is packed. Hint tables locate objects
//! by file offset, so to them an object stream is one object, used by every user of an object in
//! it, and the objects inside are never named. So each part packs its objects into object
//! streams of its own, placed in that part: the first page's in part 6, each other page's in
//! that page's run, shared ones in part 8; and none of part 9's holds objects of two later
//! pages, which would make it shared. Each section numbers the objects it writes on its own
//! first, in file order, then the objects packed in its object streams, which take its highest
//! numbers (qpdf warns of an uncompressed object numbered after a compressed one).
//!
//! An incremental update appended to a linearized file leaves the parameter dictionary describing
//! a file that no longer exists (`/L` no longer matches the length, and the newest
//! cross-reference section is no longer the first-page one): readers then treat the file as an
//! ordinary one (Annex F.2). [`Document::linearization`] reports that as [`Linearization::Stale`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

use crate::CosError;
use crate::document::{Document, ObjectReader, find};
use crate::object::{Dict, ObjRef, Object, Stream};
use crate::parser::{Lexer, parse_indirect_shared};
use crate::writer::{
    OBJSTM_SIZE, Row, SaveOptions, collect_refs, ensure_id, prepared, renumber, reread, serialize, stamp_mod_date, trailer_key_rewritten, write_dict,
    write_indirect, write_xref_stream_object, write_xref_table,
};

/// Page attributes a page inherits from its ancestors in the page tree (ISO 32000-2 Table 31).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
/// Catalog entries a viewer needs to open the document, which go in part 4 with the catalog
/// (Annex F.3.4). The `/Encrypt` dictionary joins them from the trailer.
const OPEN_KEYS: [&[u8]; 5] = [b"ViewerPreferences", b"PageMode", b"Threads", b"OpenAction", b"AcroForm"];
/// A page tree deeper than this is treated as damaged.
const MAX_TREE_DEPTH: usize = 1024;
/// Objects the grouping walks may visit in all: every page and every document-level entry walks
/// the objects it uses, so a hostile file could make that quadratic. Each visit keeps 4 bytes.
const MAX_VISITS: usize = 20_000_000;
/// The parameter dictionary has to sit in the first 1024 bytes (Annex F.3.2).
const FIRST_KB: usize = 1024;
/// Widest decimal `u64`: placeholders reserve this much so a value never moves what follows it.
const WIDEST: usize = 20;

/// What [`write_linearized`] wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Linearized {
    pub bytes: Vec<u8>,
    /// Pages that received page attributes they used to inherit from the page tree.
    pub pages_given_inherited: usize,
    /// Pages that had no `/Type` and were given `/Type /Page`.
    pub pages_given_type: usize,
}

/// Whether a file is linearized (Annex F), judged from its bytes as opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Linearization {
    /// No linearization parameter dictionary: an ordinary file.
    No,
    /// Linearized, and the parameter dictionary describes the file: Fast Web View.
    Yes,
    /// The file starts with a linearization parameter dictionary that no longer describes it,
    /// so readers treat it as an ordinary file.
    Stale(StaleReason),
}

/// Why a linearization parameter dictionary no longer describes its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleReason {
    /// `/L` is not the file's length: the file was changed (usually by an incremental update)
    /// after it was linearized.
    Updated,
    /// The newest cross-reference section is not the first-page one in front of the first page.
    Xref,
    /// `/N` or `/O` does not match the page tree.
    Pages,
    /// `/H`, `/E` or `/T` does not point where it should, or an entry is missing.
    Parameters,
}

impl Linearization {
    /// Fast Web View: a valid linearized file.
    pub fn is_fast_web_view(self) -> bool {
        self == Linearization::Yes
    }
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StaleReason::Updated => "the file was changed after it was linearized",
            StaleReason::Xref => "the newest cross-reference section is not the first-page one",
            StaleReason::Pages => "the page count or first page does not match the page tree",
            StaleReason::Parameters => "the linearization parameters do not match the file",
        })
    }
}

fn refuse(why: impl Into<String>) -> CosError {
    CosError::Linearize(why.into())
}

fn too_large() -> CosError {
    refuse("the file is too large: hint tables hold 32-bit offsets and lengths")
}

/// Write the document linearized (Fast Web View, Annex F): a full rewrite with the first page's
/// objects first and hint tables locating every page, with object streams and cross-reference
/// streams when [`SaveOptions::object_streams`] is on. See the module documentation.
///
/// Refused ([`CosError::Linearize`]) for a document without pages, a page tree that lists a page
/// or branch twice or holds a page as a direct object, and a file past the 4 GB the hint tables
/// can address.
pub fn write_linearized(doc: &Document, opts: &SaveOptions) -> Result<Linearized, CosError> {
    if doc.stream_limited() {
        return Err(CosError::ReadOnlyLimit);
    }
    let mut reader = doc.object_reader();
    stamp_mod_date(&mut reader.document, opts);
    let trailer_in = reader.document.trailer().clone();
    let root = trailer_in.reference(b"Root").ok_or_else(|| refuse("the document has no catalog"))?;
    let tree = page_tree(&mut reader, root, true)?;
    if tree.pages.is_empty() {
        return Err(refuse("the document has no pages"));
    }
    let graph = Graph::build(&mut reader, &tree.patches, &trailer_in)?;
    let catalog = match &*load(&mut reader, &tree.patches, root) {
        Object::Dict(d) => d.clone(),
        _ => return Err(refuse("the document catalog is not a dictionary")),
    };
    let parts = Parts::group(&graph, &tree, root, &catalog, &trailer_in, opts.object_streams)?;
    let bytes = Layout::write(&mut reader, &graph, &tree, &parts, &trailer_in, opts)?;
    Ok(Linearized { bytes, pages_given_inherited: tree.given_inherited, pages_given_type: tree.given_type })
}

fn load(reader: &mut ObjectReader, patches: &HashMap<ObjRef, Arc<Object>>, r: ObjRef) -> Arc<Object> {
    match patches.get(&r) {
        Some(o) => Arc::clone(o),
        None => reader.get(r),
    }
}

fn is_page(d: &Dict) -> bool {
    d.name(b"Type") == Some(b"Page")
}

/// The page tree in page order, and the edits that make every page self-contained.
struct PageTree {
    pages: Vec<ObjRef>,
    /// Intermediate (`/Pages`) nodes, root first.
    nodes: Vec<ObjRef>,
    /// Pages with inherited attributes copied on (and `/Type` added), nodes with them removed.
    patches: HashMap<ObjRef, Arc<Object>>,
    given_inherited: usize,
    given_type: usize,
}

/// Walk the page tree from the catalog. A node is anything with a `/Kids` array that is not a
/// `/Type /Page`; anything else in `/Kids` is a page. With `patch`, record the edits that copy
/// inherited attributes onto the pages and off the nodes.
fn page_tree(reader: &mut ObjectReader, root: ObjRef, patch: bool) -> Result<PageTree, CosError> {
    let pages_root = match reader.get(root).as_dict().and_then(|c| c.get(b"Pages")) {
        Some(Object::Ref(r)) => *r,
        Some(_) => return Err(refuse("the page tree root is not an indirect object")),
        None => return Err(refuse("the document has no page tree")),
    };
    let mut tree = PageTree { pages: Vec::new(), nodes: Vec::new(), patches: HashMap::new(), given_inherited: 0, given_type: 0 };
    let none: Arc<[Option<Object>; 4]> = Arc::new([None, None, None, None]);
    let mut stack = vec![(pages_root, none, 0usize)];
    let mut seen = HashSet::new();
    while let Some((r, inherited, depth)) = stack.pop() {
        if r == root || !seen.insert(r) {
            return Err(refuse(format!("the page tree lists object {} more than once", r.num)));
        }
        let object = reader.get(r);
        // A dangling kid or one that is not a dictionary is no page, as viewers read it.
        let Object::Dict(d) = &*object else { continue };
        let kind = d.name(b"Type");
        // `/Kids` may be a reference to the array.
        let indirect = match d.get(b"Kids") {
            Some(Object::Ref(k)) => Some(reader.get(*k)),
            _ => None,
        };
        let kids = match indirect.as_deref().or(d.get(b"Kids")) {
            Some(Object::Array(kids)) if kind != Some(b"Page") => Some(kids),
            _ => None,
        };
        let Some(kids) = kids else {
            if patch {
                let mut page = d.clone();
                let mut changed = false;
                if kind != Some(b"Page") {
                    page.set(b"Type".to_vec(), Object::name("Page"));
                    tree.given_type = tree.given_type.saturating_add(1);
                    changed = true;
                }
                let mut given = false;
                for (key, value) in INHERITABLE.iter().zip(inherited.iter()) {
                    if let Some(value) = value
                        && !page.contains(key)
                    {
                        page.set(key.to_vec(), value.clone());
                        given = true;
                    }
                }
                if given {
                    tree.given_inherited = tree.given_inherited.saturating_add(1);
                }
                if changed || given {
                    tree.patches.insert(r, Arc::new(Object::Dict(page)));
                }
            }
            tree.pages.push(r);
            continue;
        };
        tree.nodes.push(r);
        if depth >= MAX_TREE_DEPTH {
            return Err(refuse("the page tree is too deep"));
        }
        let own = INHERITABLE.map(|key| d.get(key).cloned());
        let passed = if own.iter().any(Option::is_some) {
            if patch {
                let mut node = d.clone();
                for key in INHERITABLE {
                    node.remove(key);
                }
                tree.patches.insert(r, Arc::new(Object::Dict(node)));
            }
            let mut merged = (*inherited).clone();
            for (slot, value) in merged.iter_mut().zip(own) {
                if value.is_some() {
                    *slot = value;
                }
            }
            Arc::new(merged)
        } else {
            inherited
        };
        for kid in kids.iter().rev() {
            match kid {
                Object::Ref(k) => stack.push((*k, Arc::clone(&passed), depth + 1)),
                Object::Dict(_) => return Err(refuse("a page is not an indirect object")),
                _ => {}
            }
        }
    }
    Ok(tree)
}

/// One object reachable from the trailer, with the references the grouping walks follow.
struct Node {
    source: ObjRef,
    stream: bool,
    /// A `/Type /Page` dictionary: walks other than its own page's stop here.
    page: bool,
    /// What a walk follows: every reference except a page's `/Parent` and `/Thumb`, a stream's
    /// `/Length` (written direct, so its object is not kept) and references inside a direct page
    /// dictionary.
    refs: Vec<usize>,
    /// A page's `/Thumb` image, walked as that page's thumbnail.
    thumb: Option<usize>,
}

/// Every object a full rewrite keeps, in breadth-first order from the trailer.
struct Graph {
    nodes: Vec<Node>,
    index: HashMap<ObjRef, usize>,
}

#[derive(Default)]
struct Edges {
    refs: Vec<ObjRef>,
    /// References the reachability walk follows but users don't.
    hidden: Vec<ObjRef>,
    thumb: Option<ObjRef>,
}

fn user_refs(o: &Object, e: &mut Edges) {
    match o {
        Object::Ref(r) => e.refs.push(*r),
        Object::Array(a) => a.iter().for_each(|x| user_refs(x, e)),
        Object::Dict(d) if is_page(d) => collect_refs(o, &mut |r| e.hidden.push(r)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| user_refs(v, e)),
        _ => {}
    }
}

fn edges(o: &Object) -> (Edges, bool) {
    let mut e = Edges::default();
    match o {
        Object::Dict(d) if is_page(d) => {
            for (key, value) in d.iter() {
                match (key.as_slice(), value) {
                    (b"Thumb", Object::Ref(r)) => e.thumb = Some(*r),
                    (b"Parent" | b"Thumb", other) => collect_refs(other, &mut |r| e.hidden.push(r)),
                    _ => user_refs(value, &mut e),
                }
            }
            return (e, true);
        }
        Object::Stream(s) => s.dict.iter().filter(|(key, _)| key.as_slice() != b"Length").for_each(|(_, v)| user_refs(v, &mut e)),
        other => user_refs(other, &mut e),
    }
    (e, false)
}

impl Graph {
    fn build(reader: &mut ObjectReader, patches: &HashMap<ObjRef, Arc<Object>>, trailer: &Dict) -> Result<Graph, CosError> {
        let mut queue = std::collections::VecDeque::new();
        for (key, value) in trailer.iter() {
            if matches!(key.as_slice(), b"Root" | b"Info" | b"Encrypt") || !trailer_key_rewritten(key) {
                collect_refs(value, &mut |r| queue.push_back(r));
            }
        }
        let mut index = HashMap::new();
        let mut found: Vec<(ObjRef, bool, bool, Edges)> = Vec::new();
        while let Some(r) = queue.pop_front() {
            if index.contains_key(&r) {
                continue;
            }
            let object = load(reader, patches, r);
            if matches!(*object, Object::Null) {
                continue; // dangling: written as null wherever it is referenced
            }
            index.insert(r, found.len());
            let (e, page) = edges(&object);
            queue.extend(e.refs.iter().chain(&e.hidden).chain(&e.thumb).copied());
            found.push((r, matches!(*object, Object::Stream(_)), page, e));
        }
        let nodes = found
            .into_iter()
            .map(|(source, stream, page, e)| Node {
                source,
                stream,
                page,
                refs: e.refs.iter().filter_map(|r| index.get(r).copied()).collect(),
                thumb: e.thumb.and_then(|r| index.get(&r).copied()),
            })
            .collect();
        Ok(Graph { nodes, index })
    }
}

/// Walks from users, each with its own visited set (an epoch per walk instead of clearing).
struct Walker {
    stamp: Vec<u32>,
    epoch: u32,
    visits: usize,
}

impl Walker {
    fn new(len: usize) -> Self {
        Self { stamp: vec![0; len], epoch: 0, visits: 0 }
    }

    /// Start a new user: nothing is visited yet.
    fn next_user(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.epoch = 1;
        }
    }

    /// Visit what `start` uses (depth-first, in reference order), not entering pages other than
    /// `top`, appending each newly visited node to `out`.
    fn walk(&mut self, nodes: &[Node], start: usize, top: Option<usize>, out: &mut Vec<usize>) -> Result<(), CosError> {
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let Some(node) = nodes.get(i) else { continue };
            if node.page && Some(i) != top {
                continue;
            }
            let Some(stamp) = self.stamp.get_mut(i) else { continue };
            if *stamp == self.epoch {
                continue;
            }
            *stamp = self.epoch;
            self.visits = self.visits.saturating_add(1);
            if self.visits > MAX_VISITS {
                return Err(refuse("the document references its objects from too many places to group them by page"));
            }
            out.push(i);
            stack.extend(node.refs.iter().rev());
        }
        Ok(())
    }
}

/// Who uses an object (Annex F.3: which part it belongs to).
#[derive(Clone, Copy, Default)]
struct Uses {
    root: bool,
    outlines: bool,
    open: bool,
    first_page: bool,
    /// Pages after the first that use it, and the last of them.
    pages: u32,
    page: usize,
    thumbs: u32,
    /// Catalog and trailer entries other than the outline and the ones needed at open.
    others: u32,
    /// Users of any kind.
    users: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Root,
    Outline,
    Open,
    FirstPrivate,
    FirstShared,
    Private(usize),
    Shared,
    Other,
}

impl Uses {
    fn class(&self) -> Class {
        if self.root {
            Class::Root
        } else if self.outlines {
            Class::Outline
        } else if self.open {
            Class::Open
        } else if self.first_page && self.users == 1 {
            Class::FirstPrivate
        } else if self.first_page {
            Class::FirstShared
        } else if self.pages == 1 && self.others == 0 && self.thumbs == 0 {
            Class::Private(self.page)
        } else if self.pages > 1 {
            Class::Shared
        } else {
            Class::Other
        }
    }
}

/// What one place in a part holds, and what the hint tables count: an object written on its own,
/// or an object stream of non-stream objects (graph indices).
enum Unit {
    Object(usize),
    Packed(Vec<usize>),
}

impl Unit {
    /// The objects it holds.
    fn objects(&self) -> &[usize] {
        match self {
            Unit::Object(i) => std::slice::from_ref(i),
            Unit::Packed(members) => members,
        }
    }
}

/// Which object streams an object may join: those of its own part, in part 7 of its own page,
/// and in part 9 of the later page that uses it, if one does: an object stream two later pages
/// use is shared.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pack {
    Open,
    FirstPage,
    Outline,
    Page(usize),
    Shared,
    Other(Option<usize>),
}

/// Builds a part's units in file order. An object that is not packed is a unit of its own; one
/// that is joins the open object stream of its kind, which sits where its first object would
/// have, or starts the next.
#[derive(Default)]
struct Packer {
    units: Vec<Unit>,
    /// The unit of the open object stream of each kind.
    open: HashMap<Pack, usize>,
}

impl Packer {
    fn push(&mut self, i: usize, pack: Option<Pack>) {
        let Some(kind) = pack else {
            self.units.push(Unit::Object(i));
            return;
        };
        if let Some(Unit::Packed(members)) = self.open.get(&kind).and_then(|&at| self.units.get_mut(at))
            && members.len() < OBJSTM_SIZE
        {
            members.push(i);
            return;
        }
        self.open.insert(kind, self.units.len());
        self.units.push(Unit::Packed(vec![i]));
    }
}

/// Pack a part's objects in order: its units, and for each position in `list` the number of
/// units before it (and one more entry for the end), which maps a run of objects to its units.
fn pack(list: &[usize], packing: &dyn Fn(usize) -> Option<Pack>) -> (Vec<Unit>, Vec<usize>) {
    let mut packer = Packer::default();
    let mut before = Vec::with_capacity(list.len().saturating_add(1));
    for &i in list {
        before.push(packer.units.len());
        packer.push(i, packing(i));
    }
    before.push(packer.units.len());
    let mut units = packer.units;
    // An object stream that holds one object is larger than that object on its own.
    for unit in &mut units {
        if let Unit::Packed(members) = unit
            && members.len() == 1
            && let Some(&only) = members.first()
        {
            *unit = Unit::Object(only);
        }
    }
    (units, before)
}

/// The units of the run of objects `start..start + count`, given [`pack`]'s positions.
fn unit_run(before: &[usize], start: usize, count: usize) -> (usize, usize) {
    let from = before.get(start).copied().unwrap_or(0);
    let to = before.get(start.saturating_add(count)).copied().unwrap_or(from);
    (from, to.saturating_sub(from))
}

/// The units of each part in file order, and what the hint tables need.
struct Parts {
    part4: Vec<Unit>,
    part6: Vec<Unit>,
    /// Each later page's run in `part7`: (start, count), the page object first.
    page_runs: Vec<(usize, usize)>,
    part7: Vec<Unit>,
    part8: Vec<Unit>,
    part9: Vec<Unit>,
    /// Shared objects (indices into part 6 followed by part 8) each later page uses.
    page_shared: Vec<Vec<u32>>,
    /// The outline's units as one run: where it starts in its part, how many, in part 6 or 9.
    outline: Option<OutlineRun>,
}

#[derive(Clone, Copy)]
struct OutlineRun {
    in_first_page: bool,
    start: usize,
    count: usize,
}

impl Parts {
    /// Group the objects into parts, and with `object_streams` pack each part's objects that can
    /// be packed into object streams of that part.
    fn group(graph: &Graph, tree: &PageTree, root: ObjRef, catalog: &Dict, trailer: &Dict, object_streams: bool) -> Result<Parts, CosError> {
        let nodes = &graph.nodes;
        let at = |r: &ObjRef| graph.index.get(r).copied();
        let mut uses = vec![Uses::default(); nodes.len()];
        let mut walker = Walker::new(nodes.len());
        let mut visited = Vec::new();
        let root_at = at(&root).ok_or_else(|| refuse("the document catalog is missing"))?;
        if let Some(u) = uses.get_mut(root_at) {
            u.root = true;
            u.users = u.users.saturating_add(1);
        }
        // Catalog and trailer entries: one user each.
        let entries = catalog.iter().map(|(k, v)| (k.as_slice(), v, true)).chain(
            trailer
                .iter()
                .filter(|(k, _)| k.as_slice() != b"Root" && (matches!(k.as_slice(), b"Info" | b"Encrypt") || !trailer_key_rewritten(k)))
                .map(|(k, v)| (k.as_slice(), v, false)),
        );
        for (key, value, in_catalog) in entries {
            walker.next_user();
            visited.clear();
            let mut starts = Edges::default();
            user_refs(value, &mut starts);
            for r in &starts.refs {
                if let Some(i) = at(r) {
                    walker.walk(nodes, i, None, &mut visited)?;
                }
            }
            for &i in &visited {
                let Some(u) = uses.get_mut(i) else { continue };
                if in_catalog && key == b"Outlines" {
                    u.outlines = true;
                } else if (in_catalog && OPEN_KEYS.contains(&key)) || (!in_catalog && key == b"Encrypt") {
                    u.open = true;
                } else {
                    u.others = u.others.saturating_add(1);
                }
                u.users = u.users.saturating_add(1);
            }
        }
        // Pages, and their thumbnails (walked with the page: what the page reaches first is the
        // page's).
        let mut page_lists: Vec<Vec<usize>> = Vec::with_capacity(tree.pages.len());
        let mut thumb_lists: Vec<Vec<usize>> = Vec::with_capacity(tree.pages.len());
        for (n, page) in tree.pages.iter().enumerate() {
            let p = at(page).ok_or_else(|| refuse(format!("page {} is missing", n + 1)))?;
            walker.next_user();
            let mut list = Vec::new();
            walker.walk(nodes, p, Some(p), &mut list)?;
            for &i in &list {
                let Some(u) = uses.get_mut(i) else { continue };
                if n == 0 {
                    u.first_page = true;
                } else {
                    u.pages = u.pages.saturating_add(1);
                    u.page = n;
                }
                u.users = u.users.saturating_add(1);
            }
            let mut thumbs = Vec::new();
            if let Some(t) = nodes.get(p).and_then(|node| node.thumb) {
                walker.walk(nodes, t, None, &mut thumbs)?;
            }
            for &i in &thumbs {
                let Some(u) = uses.get_mut(i) else { continue };
                u.thumbs = u.thumbs.saturating_add(1);
                u.users = u.users.saturating_add(1);
            }
            page_lists.push(list);
            thumb_lists.push(thumbs);
        }
        let class: Vec<Class> = uses.iter().map(Uses::class).collect();
        let class_of = |i: usize| class.get(i).copied().unwrap_or(Class::Other);
        let mut placed = vec![false; nodes.len()];
        let mut take = |i: usize, part: &mut Vec<usize>| {
            if let Some(p) = placed.get_mut(i)
                && !*p
            {
                *p = true;
                part.push(i);
            }
        };

        let mut part4 = Vec::new();
        take(root_at, &mut part4);
        (0..nodes.len()).filter(|&i| class_of(i) == Class::Open).for_each(|i| take(i, &mut part4));

        let first = page_lists.first().map(Vec::as_slice).unwrap_or_default();
        let mut part6 = Vec::new();
        if let Some(&p) = first.first() {
            take(p, &mut part6);
        }
        first.iter().filter(|&&i| class_of(i) == Class::FirstPrivate).for_each(|&i| take(i, &mut part6));
        first.iter().filter(|&&i| class_of(i) == Class::FirstShared).for_each(|&i| take(i, &mut part6));
        // The outline: its root first, then the rest of what only it (or it first) uses.
        let outline_root = catalog.reference(b"Outlines").and_then(|r| at(&r)).filter(|&i| class_of(i) == Class::Outline);
        let use_outlines = catalog.name(b"PageMode") == Some(b"UseOutlines");
        let place_outline = |part: &mut Vec<usize>, take: &mut dyn FnMut(usize, &mut Vec<usize>)| -> Option<OutlineRun> {
            let start = part.len();
            let root = outline_root?;
            take(root, part);
            (0..nodes.len()).filter(|&i| class_of(i) == Class::Outline).for_each(|i| take(i, part));
            Some(OutlineRun { in_first_page: false, start, count: part.len() - start })
        };
        let mut outline = None;
        if use_outlines {
            outline = place_outline(&mut part6, &mut take).map(|o| OutlineRun { in_first_page: true, ..o });
        }

        let mut part7 = Vec::new();
        let mut page_runs = Vec::new();
        for (n, list) in page_lists.iter().enumerate().skip(1) {
            let start = part7.len();
            if let Some(&p) = list.first() {
                take(p, &mut part7);
            }
            list.iter().filter(|&&i| class_of(i) == Class::Private(n)).for_each(|&i| take(i, &mut part7));
            page_runs.push((start, part7.len() - start));
        }

        let mut part8 = Vec::new();
        for list in page_lists.iter().skip(1) {
            list.iter().filter(|&&i| class_of(i) == Class::Shared).for_each(|&i| take(i, &mut part8));
        }

        let mut part9 = Vec::new();
        // The page tree, then thumbnails, then the outline: each only where nothing else claims
        // the object first, so the outline stays one run.
        tree.nodes.iter().filter_map(at).filter(|&i| class_of(i) == Class::Other).for_each(|i| take(i, &mut part9));
        thumb_lists.iter().flatten().filter(|&&i| class_of(i) == Class::Other).for_each(|&i| take(i, &mut part9));
        if !use_outlines {
            outline = place_outline(&mut part9, &mut take);
        }
        (0..nodes.len()).for_each(|i| take(i, &mut part9));

        let placed_count = part4.len() + part6.len() + part7.len() + part8.len() + part9.len();
        if placed_count != nodes.len() {
            return Err(refuse("objects could not be grouped by page"));
        }

        // What stays out of object streams besides streams and pages: the catalog (linearized
        // files keep it out, as qpdf documents and does, and Acrobat refuses an encrypted file
        // whose catalog is packed, #774), and the encryption dictionary with everything it
        // references, which are read before anything can be decrypted (§7.5.7), as `write_full`.
        let mut unpacked = vec![!object_streams; nodes.len()];
        if let Some(catalog) = unpacked.get_mut(root_at) {
            *catalog = true;
        }
        let mut walked = vec![false; nodes.len()];
        let mut pending: Vec<usize> = trailer.reference(b"Encrypt").and_then(|r| at(&r)).into_iter().collect();
        while let Some(i) = pending.pop() {
            if let (Some(was_walked), Some(keep_out)) = (walked.get_mut(i), unpacked.get_mut(i))
                && !*was_walked
            {
                *was_walked = true;
                *keep_out = true;
                pending.extend(nodes.get(i).map(|n| n.refs.as_slice()).unwrap_or_default());
            }
        }
        let packing = |i: usize| -> Option<Pack> {
            let node = nodes.get(i)?;
            let u = uses.get(i)?;
            if node.stream || node.page || unpacked.get(i).copied().unwrap_or(true) {
                return None;
            }
            Some(match u.class() {
                Class::Root => return None,
                Class::Open => Pack::Open,
                Class::Outline => Pack::Outline,
                Class::FirstPrivate | Class::FirstShared => Pack::FirstPage,
                Class::Private(n) => Pack::Page(n),
                Class::Shared => Pack::Shared,
                // At most one later page uses it, or it would be shared.
                Class::Other => Pack::Other((u.pages > 0).then_some(u.page)),
            })
        };
        let (part4, _) = pack(&part4, &packing);
        let (part6, before6) = pack(&part6, &packing);
        let (part7, before7) = pack(&part7, &packing);
        let page_runs = page_runs.iter().map(|&(start, count)| unit_run(&before7, start, count)).collect();
        let (part8, _) = pack(&part8, &packing);
        let (part9, before9) = pack(&part9, &packing);
        let outline = outline.map(|run| {
            let (start, count) = unit_run(if run.in_first_page { &before6 } else { &before9 }, run.start, run.count);
            OutlineRun { start, count, ..run }
        });

        // Shared object identifiers: part 6's units in order, then part 8's (F.4.2). A page that
        // uses an object in an object stream uses the object stream.
        let mut shared_id: HashMap<usize, u32> = HashMap::new();
        for (id, unit) in part6.iter().chain(&part8).enumerate() {
            let id = u32::try_from(id).map_err(|_| too_large())?;
            unit.objects().iter().for_each(|&i| {
                shared_id.insert(i, id);
            });
        }
        let page_shared = page_lists
            .iter()
            .skip(1)
            .map(|list| {
                let mut listed = HashSet::new();
                list.iter()
                    .filter(|&&i| uses.get(i).is_some_and(|u| u.users > 1))
                    .filter_map(|i| shared_id.get(i).copied())
                    .filter(|id| listed.insert(*id))
                    .collect()
            })
            .collect();
        Ok(Parts { part4, part6, page_runs, part7, part8, part9, page_shared, outline })
    }
}

/// Packs hint table items most significant bit first (F.4: items are bit fields, and every
/// table and every run of one item starts at a byte boundary).
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    used: u32,
}

impl Bits {
    /// Write the low `bits` (at most 32) of `value`.
    fn put(&mut self, value: u64, bits: u32) {
        let bits = bits.min(32);
        if bits == 0 {
            return;
        }
        let mask = (1u64 << bits) - 1;
        self.acc = (self.acc << bits) | (value & mask);
        self.used += bits;
        while self.used >= 8 {
            self.used -= 8;
            self.out.push((self.acc >> self.used) as u8);
        }
        self.acc &= (1u64 << self.used) - 1;
    }

    /// Pad to the next byte boundary with zero bits.
    fn align(&mut self) {
        if self.used > 0 {
            self.put(0, 8 - self.used);
        }
    }
}

/// Bits needed for `v` (none for 0).
fn bits_for(v: u64) -> u32 {
    64 - v.leading_zeros()
}

fn u32_of(v: u64) -> Result<u64, CosError> {
    if v > u64::from(u32::MAX) { Err(too_large()) } else { Ok(v) }
}

/// Offset (as if the hint stream were absent, F.4) and length of each object after the hint
/// stream, in file order.
struct Spans {
    offsets: Vec<u64>,
    lengths: Vec<u64>,
}

impl Spans {
    fn run(&self, start: usize, count: usize) -> u64 {
        self.lengths.iter().skip(start).take(count).sum()
    }
}

/// The hint stream's data and the offsets of its tables (`/S`, `/O`).
fn hint_tables(parts: &Parts, spans: &Spans, numbers: &[u32]) -> Result<(Vec<u8>, usize, Option<usize>), CosError> {
    let n6 = parts.part6.len();
    let n7 = parts.part7.len();
    // Page offset hint table (F.4.1, Tables F.3 and F.4).
    let mut pages: Vec<(u64, u64, &[u32])> = vec![(n6 as u64, spans.run(0, n6), &[])];
    for ((start, count), shared) in parts.page_runs.iter().zip(&parts.page_shared) {
        pages.push((*count as u64, spans.run(n6 + start, *count), shared.as_slice()));
    }
    let min_objects = pages.iter().map(|p| p.0).min().unwrap_or(0);
    let max_objects = pages.iter().map(|p| p.0).max().unwrap_or(0);
    let min_length = pages.iter().map(|p| p.1).min().unwrap_or(0);
    let max_length = pages.iter().map(|p| p.1).max().unwrap_or(0);
    let max_shared = pages.iter().map(|p| p.2.len() as u64).max().unwrap_or(0);
    let max_id = pages.iter().flat_map(|p| p.2.iter()).copied().max().unwrap_or(0);
    let object_bits = bits_for(max_objects.saturating_sub(min_objects));
    let length_bits = bits_for(max_length.saturating_sub(min_length));
    let shared_bits = bits_for(max_shared);
    let id_bits = bits_for(u64::from(max_id));
    let mut w = Bits::default();
    w.put(u32_of(min_objects)?, 32); // 1: least number of objects in a page
    w.put(u32_of(spans.offsets.first().copied().unwrap_or(0))?, 32); // 2: first page's page object
    w.put(u64::from(object_bits), 16); // 3
    w.put(u32_of(min_length)?, 32); // 4: least page length
    w.put(u64::from(length_bits), 16); // 5
    // 6–9: content stream offsets and lengths. A page's content streams sit in its own run, so
    // the run is given as the content: offset 0 and the page length (what Acrobat writes, and
    // ignores, per the PDF Reference's implementation notes on Annex F).
    w.put(0, 32); // 6: least content stream offset
    w.put(0, 16); // 7
    w.put(u32_of(min_length)?, 32); // 8: least content stream length
    w.put(u64::from(length_bits), 16); // 9
    w.put(u64::from(shared_bits), 16); // 10: greatest number of shared object references
    w.put(u64::from(id_bits), 16); // 11: greatest shared object identifier
    w.put(0, 16); // 12: fractional position numerators: every shared object counts as needed first
    w.put(1, 16); // 13: their denominator
    // Per-page items, each item for every page before the next item, each run byte-aligned.
    for p in &pages {
        w.put(p.0.saturating_sub(min_objects), object_bits);
    }
    w.align();
    for p in &pages {
        w.put(p.1.saturating_sub(min_length), length_bits);
    }
    w.align();
    for p in &pages {
        w.put(p.2.len() as u64, shared_bits);
    }
    w.align();
    for p in &pages {
        for id in p.2 {
            w.put(u64::from(*id), id_bits);
        }
    }
    w.align();
    // Numerators take 0 bits, content offsets 0 bits: those runs are empty.
    w.align();
    w.align();
    for p in &pages {
        w.put(p.1.saturating_sub(min_length), length_bits);
    }
    w.align();

    // Shared object hint table (F.4.2, Tables F.5 and F.6): the first page's objects, then
    // part 8, one object per group.
    let s = w.out.len();
    let first_shared = n6 + n7;
    let groups: Vec<u64> =
        (0..n6).chain(first_shared..first_shared + parts.part8.len()).map(|i| spans.lengths.get(i).copied().unwrap_or(0)).collect();
    let min_group = groups.iter().copied().min().unwrap_or(0);
    let max_group = groups.iter().copied().max().unwrap_or(0);
    let group_bits = bits_for(max_group.saturating_sub(min_group));
    let (first_obj, first_offset) = match parts.part8.first() {
        Some(_) => (numbers.get(first_shared).copied().unwrap_or(0), spans.offsets.get(first_shared).copied().unwrap_or(0)),
        None => (0, 0),
    };
    w.put(u64::from(first_obj), 32); // 1: first object of part 8
    w.put(u32_of(first_offset)?, 32); // 2: its location
    w.put(u32_of(n6 as u64)?, 32); // 3: entries for the first page
    w.put(u32_of(groups.len() as u64)?, 32); // 4: all entries
    w.put(0, 16); // 5: bits for objects per group minus one (every group is one object)
    w.put(u32_of(min_group)?, 32); // 6: least group length
    w.put(u64::from(group_bits), 16); // 7
    for g in &groups {
        w.put(g.saturating_sub(min_group), group_bits);
    }
    w.align();
    for _ in &groups {
        w.put(0, 1); // no MD5 signature
    }
    w.align();

    // Outline hint table (F.4.4, generic hint table).
    let mut o = None;
    if let Some(run) = parts.outline {
        let start = if run.in_first_page { run.start } else { n6 + n7 + parts.part8.len() + run.start };
        o = Some(w.out.len());
        w.put(u64::from(numbers.get(start).copied().unwrap_or(0)), 32); // 1: first object
        w.put(u32_of(spans.offsets.get(start).copied().unwrap_or(0))?, 32); // 2: its location
        w.put(u32_of(run.count as u64)?, 32); // 3: objects
        w.put(u32_of(spans.run(start, run.count))?, 32); // 4: length
    }
    w.align();
    Ok((w.out, s, o))
}

/// Give `units` the numbers from `next` on, in order: an object its own number, an object stream
/// its number (the objects in it are numbered by [`number_packed`]). Returns the units' numbers.
fn number_units(graph: &Graph, units: &[Unit], next: &mut usize, numbers: &mut HashMap<ObjRef, u32>) -> Result<Vec<u32>, CosError> {
    let mut out = Vec::with_capacity(units.len());
    for unit in units {
        let num = number(*next)?;
        *next = next.saturating_add(1);
        if let Unit::Object(i) = unit
            && let Some(node) = graph.nodes.get(*i)
        {
            numbers.insert(node.source, num);
        }
        out.push(num);
    }
    Ok(out)
}

/// Number the objects packed in `units`' object streams from `next` on.
fn number_packed(graph: &Graph, units: &[Unit], next: &mut usize, numbers: &mut HashMap<ObjRef, u32>) -> Result<(), CosError> {
    for unit in units {
        if let Unit::Packed(members) = unit {
            for node in members.iter().filter_map(|&i| graph.nodes.get(i)) {
                numbers.insert(node.source, number(*next)?);
                *next = next.saturating_add(1);
            }
        }
    }
    Ok(())
}

/// An object as it is saved: with the page tree's edits, or read again.
fn saved_object(reader: &mut ObjectReader, tree: &PageTree, node: &Node) -> Result<Arc<Object>, CosError> {
    match tree.patches.get(&node.source) {
        Some(o) => Ok(Arc::clone(o)),
        None => reread(reader, node.source, node.stream),
    }
}

/// Serialize `units` under their numbers into `out`, returning where each starts, and record in
/// `packed` where each object packed in an object stream sits.
fn write_units(
    reader: &mut ObjectReader,
    graph: &Graph,
    tree: &PageTree,
    units: &[(&Unit, u32)],
    numbers: &HashMap<ObjRef, u32>,
    out: &mut Vec<u8>,
    packed: &mut BTreeMap<u32, Row>,
) -> Result<Vec<u64>, CosError> {
    let missing = || refuse("an object went missing while saving");
    let mut starts = Vec::with_capacity(units.len());
    for &(unit, num) in units {
        starts.push(out.len() as u64);
        match unit {
            Unit::Object(i) => {
                let node = graph.nodes.get(*i).ok_or_else(missing)?;
                let renumbered = renumber(&*saved_object(reader, tree, node)?, numbers);
                write_indirect(num, 0, &prepared(&reader.document, node.source.num, num, 0, &renumbered), out);
            }
            Unit::Packed(members) => {
                // An object stream (§7.5.7): pairs of object number and offset, then the objects.
                // The objects are not encrypted on their own; the stream is (§7.6.2).
                let mut header = Vec::new();
                let mut objects = Vec::new();
                for (k, &i) in members.iter().enumerate() {
                    let node = graph.nodes.get(i).ok_or_else(missing)?;
                    let member = numbers.get(&node.source).copied().ok_or_else(missing)?;
                    let _ = write!(header, "{member} {} ", objects.len());
                    serialize(&renumber(&*saved_object(reader, tree, node)?, numbers), &mut objects);
                    objects.push(b'\n');
                    packed.insert(member, Row::InStream(num, number(k)?));
                }
                let first = header.len();
                header.extend_from_slice(&objects);
                let mut d = Dict::new();
                d.set(b"Type".to_vec(), Object::name("ObjStm"));
                d.set(b"N".to_vec(), Object::Int(members.len() as i64));
                d.set(b"First".to_vec(), Object::Int(first as i64));
                let stream = Object::Stream(Stream::flate(d, &header));
                write_indirect(num, 0, &prepared(&reader.document, u32::MAX, num, 0, &stream), out);
            }
        }
    }
    Ok(starts)
}

fn number(n: usize) -> Result<u32, CosError> {
    u32::try_from(n).map_err(|_| refuse("too many objects to save"))
}

/// Rows of a classic cross-reference table: only objects at an offset (nothing is packed when
/// the sections are tables).
fn table_rows(rows: &BTreeMap<u32, Row>) -> BTreeMap<u32, (u64, u16, bool)> {
    rows.iter().filter_map(|(n, r)| if let Row::InFile(o, g) = *r { Some((*n, (o, g, true))) } else { None }).collect()
}

struct Layout;

impl Layout {
    fn write(
        reader: &mut ObjectReader,
        graph: &Graph,
        tree: &PageTree,
        parts: &Parts,
        trailer_in: &Dict,
        opts: &SaveOptions,
    ) -> Result<Vec<u8>, CosError> {
        let streams = opts.object_streams;
        // Numbers: the main section (parts 7–9) takes 1.., the first-page section the rest. Each
        // numbers what it writes on its own in file order (the first-page section: the parameter
        // dictionary, its cross-reference stream, part 4, the hint stream, part 6), then the
        // objects packed in its object streams: they take a section's highest numbers.
        let mut numbers: HashMap<ObjRef, u32> = HashMap::with_capacity(graph.nodes.len());
        let mut next = 1usize;
        let nums7 = number_units(graph, &parts.part7, &mut next, &mut numbers)?;
        let nums8 = number_units(graph, &parts.part8, &mut next, &mut numbers)?;
        let nums9 = number_units(graph, &parts.part9, &mut next, &mut numbers)?;
        let main_xref_num = number(next)?;
        next = next.saturating_add(usize::from(streams));
        for part in [&parts.part7, &parts.part8, &parts.part9] {
            number_packed(graph, part, &mut next, &mut numbers)?;
        }
        let lin_num = number(next)?;
        next = next.saturating_add(1);
        let first_xref_num = number(next)?;
        next = next.saturating_add(usize::from(streams));
        let nums4 = number_units(graph, &parts.part4, &mut next, &mut numbers)?;
        let hint_num = number(next)?;
        next = next.saturating_add(1);
        let nums6 = number_units(graph, &parts.part6, &mut next, &mut numbers)?;
        for part in [&parts.part4, &parts.part6] {
            number_packed(graph, part, &mut next, &mut numbers)?;
        }
        let size = number(next)?;

        // Serialize everything but the hint stream: part 4, then parts 6–9 (the body).
        let mut packed = BTreeMap::new();
        let part4_units: Vec<(&Unit, u32)> = parts.part4.iter().zip(nums4).collect();
        let mut part4 = Vec::new();
        let part4_starts = write_units(reader, graph, tree, &part4_units, &numbers, &mut part4, &mut packed)?;
        let body_units: Vec<(&Unit, u32)> = (parts.part6.iter().zip(nums6))
            .chain(parts.part7.iter().zip(nums7))
            .chain(parts.part8.iter().zip(nums8))
            .chain(parts.part9.iter().zip(nums9))
            .collect();
        let mut body = Vec::new();
        let body_starts = write_units(reader, graph, tree, &body_units, &numbers, &mut body, &mut packed)?;
        let body_numbers: Vec<u32> = body_units.iter().map(|(_, n)| *n).collect();

        let version = reader.document.version().chars().take(3).collect::<String>();
        // Object streams and cross-reference streams are PDF 1.5.
        let least = if streams { "1.5" } else { "1.2" };
        let version = if version.as_str() < least { least.to_string() } else { version };
        let mut header = Vec::new();
        let _ = writeln!(header, "%PDF-{version}");
        header.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);

        let n6 = parts.part6.len();
        let first_page_num = body_numbers.first().copied().unwrap_or(0);
        let page_count = tree.pages.len();
        let lin_at = header.len() as u64;
        let lin_width = lin_object(lin_num, &LinValues::placeholder(first_page_num, page_count)).len();
        let xref1_at = lin_at + lin_width as u64;

        let mut trailer = Dict::new();
        trailer.set(b"Size".to_vec(), Object::Int(i64::from(size)));
        for key in [&b"Root"[..], b"Info", b"Encrypt"] {
            if let Some(n) = trailer_in.reference(key).and_then(|r| numbers.get(&r)) {
                trailer.set(key.to_vec(), Object::Ref(ObjRef::new(*n, 0)));
            }
        }
        if let Some(Object::Dict(d)) = trailer_in.get(b"Encrypt") {
            trailer.set(b"Encrypt".to_vec(), Object::Dict(d.clone()));
        }
        if let Some(id) = trailer_in.get(b"ID") {
            trailer.set(b"ID".to_vec(), id.clone());
        }
        for (key, value) in trailer_in.iter() {
            if !trailer_key_rewritten(key) {
                trailer.set(key.clone(), renumber(value, &numbers));
            }
        }
        if reader.document.output_security().0.is_none() {
            ensure_id(&mut trailer, opts, reader.document.bytes());
        }

        // The first-page section's rows: the dictionary, part 4, the hint stream, part 6, the
        // objects packed in parts 4 and 6, and its own cross-reference stream.
        let first_rows = |part4_at: u64, hint_at: u64, body_at: u64| -> BTreeMap<u32, Row> {
            let mut rows: BTreeMap<u32, Row> = packed.range(lin_num..).map(|(n, r)| (*n, *r)).collect();
            rows.insert(lin_num, Row::InFile(lin_at, 0));
            for (num, start) in part4_units.iter().map(|(_, n)| *n).zip(&part4_starts) {
                rows.insert(num, Row::InFile(part4_at + start, 0));
            }
            rows.insert(hint_num, Row::InFile(hint_at, 0));
            for (num, start) in body_numbers.iter().zip(&body_starts).take(n6) {
                rows.insert(*num, Row::InFile(body_at + start, 0));
            }
            if streams {
                rows.insert(first_xref_num, Row::InFile(xref1_at, 0));
            }
            rows
        };
        let xref1_count = size.saturating_sub(lin_num) as usize;
        // The first-page section is written before the offsets it holds are known, so its length
        // must not depend on them: a table's entries are 20 bytes each, a stream's rows are
        // unfiltered with a fixed width, the narrowest that holds every offset in them. Its
        // length is measured with every offset and object stream number 0.
        let placeholder: BTreeMap<u32, Row> = first_rows(0, 0, 0)
            .into_iter()
            .map(|(n, r)| match r {
                Row::InStream(_, i) => (n, Row::InStream(0, i)),
                Row::InFile(_, g) => (n, Row::InFile(0, g)),
                free => (n, free),
            })
            .collect();
        let widths: &[usize] = if streams { &[1, 2, 3, 4, 5, 6, 7, 8] } else { &[0] };
        let mut laid_out = None;
        for &w2 in widths {
            let first_len = if streams {
                first_xref_stream(first_xref_num, &trailer, &placeholder, w2, 0)?.len()
            } else {
                format!("xref\n{lin_num} {xref1_count}\n").len() + xref1_count.saturating_mul(20) + first_trailer(&trailer, 0).len()
            };
            let part4_at = xref1_at + first_len as u64;
            let head_len = part4_at + part4.len() as u64;
            // Hint table offsets and lengths leave the hint stream out; all of them fit in 32
            // bits when everything else does.
            u32_of(head_len.saturating_add(body.len() as u64))?;

            // Hint tables, with offsets as if the hint stream were absent (F.4).
            let spans = Spans {
                offsets: body_starts.iter().map(|s| head_len + s).collect(),
                lengths: body_starts
                    .iter()
                    .zip(body_starts.iter().skip(1).chain(std::iter::once(&(body.len() as u64))))
                    .map(|(a, b)| b.saturating_sub(*a))
                    .collect(),
            };
            let (data, s, o) = hint_tables(parts, &spans, &body_numbers)?;
            let mut hint_dict = Dict::new();
            hint_dict.set(b"S".to_vec(), Object::Int(s as i64));
            if let Some(o) = o {
                hint_dict.set(b"O".to_vec(), Object::Int(o as i64));
            }
            let hint = Object::Stream(Stream::flate(hint_dict, &data));
            let mut hint_obj = Vec::new();
            write_indirect(hint_num, 0, &prepared(&reader.document, u32::MAX, hint_num, 0, &hint), &mut hint_obj);
            let body_at = head_len + hint_obj.len() as u64;
            let rows = first_rows(part4_at, head_len, body_at);
            if streams && rows.values().any(|r| field_bytes(row_fields(r).1) > w2) {
                continue;
            }
            laid_out = Some((w2, first_len, part4_at, hint_obj, body_at, rows));
            break;
        }
        let Some((w2, first_len, part4_at, hint_obj, body_at, first_rows)) = laid_out else { return Err(too_large()) };
        let hint_at = part4_at + part4.len() as u64;

        // The main section: object 0, parts 7–9 and the objects packed in them.
        let main_at = body_at + body.len() as u64;
        let mut main_rows: BTreeMap<u32, Row> = packed.range(..lin_num).map(|(n, r)| (*n, *r)).collect();
        for (num, start) in body_numbers.iter().zip(&body_starts).skip(n6) {
            main_rows.insert(*num, Row::InFile(body_at + start, 0));
        }
        let mut main_xref = Vec::new();
        let t = if streams {
            write_xref_stream_object(&mut main_xref, main_at, &mut Dict::new(), main_rows, main_xref_num, i64::from(lin_num));
            // /T: the main cross-reference stream (Table F.1).
            main_at
        } else {
            write_xref_table(&mut main_xref, &table_rows(&main_rows), true);
            let _ = write!(main_xref, "trailer\n<</Size {lin_num}>>\n");
            // /T: the white-space character in front of the entry for object 0.
            main_at + format!("xref\n0 {lin_num}").len() as u64
        };
        let _ = write!(main_xref, "startxref\n{xref1_at}\n%%EOF\n");
        let file_len = main_at + main_xref.len() as u64;
        let first_page_end = body_at + body_starts.get(n6).copied().unwrap_or(body.len() as u64);

        let values = LinValues {
            file_len,
            hint_at,
            hint_len: hint_obj.len() as u64,
            first_page: first_page_num,
            first_page_end,
            pages: page_count,
            main_xref_entry: t,
        };
        let mut lin = lin_object(lin_num, &values);
        let first_section = if streams {
            first_xref_stream(first_xref_num, &trailer, &first_rows, w2, main_at)?
        } else {
            let mut xref1 = Vec::new();
            write_xref_table(&mut xref1, &table_rows(&first_rows), false);
            xref1.extend_from_slice(&first_trailer(&trailer, main_at));
            xref1
        };
        if lin.len() > lin_width || first_section.len() != first_len {
            return Err(refuse("the first-page section did not keep its size"));
        }
        // Padding keeps the dictionary the width that was reserved for it.
        let pad = lin_width - lin.len();
        let close = lin.len().saturating_sub(b"\nendobj\n".len());
        lin.splice(close..close, std::iter::repeat_n(b' ', pad));

        let mut out = Vec::with_capacity(usize::try_from(file_len).unwrap_or(0));
        out.extend_from_slice(&header);
        out.extend_from_slice(&lin);
        out.extend_from_slice(&first_section);
        out.extend_from_slice(&part4);
        out.extend_from_slice(&hint_obj);
        out.extend_from_slice(&body);
        out.extend_from_slice(&main_xref);
        if out.len() as u64 != file_len || lin_at as usize + lin.len() > FIRST_KB {
            return Err(refuse("the layout did not add up"));
        }
        Ok(out)
    }
}

/// A cross-reference stream row's fields (§7.5.8.3, Table 18).
fn row_fields(row: &Row) -> (u8, u64, u64) {
    match *row {
        Row::Free(g) => (0, 0, u64::from(g)),
        Row::InFile(o, g) => (1, o, u64::from(g)),
        Row::InStream(s, i) => (2, u64::from(s), u64::from(i)),
    }
}

/// Bytes a cross-reference stream field needs to hold `v` (at least one).
fn field_bytes(v: u64) -> usize {
    (bits_for(v) as usize).div_ceil(8).max(1)
}

/// The first-page cross-reference stream (§7.5.8) of `rows`, which are numbered consecutively:
/// the trailer's entries, `/Prev` (the main section's offset) padded like [`first_trailer`]'s,
/// and the rows unfiltered with offsets `w2` bytes wide, so that its length depends on how many
/// rows there are and never on the offsets in them. Like the classic first-page trailer, it is
/// followed by a `startxref` of 0.
fn first_xref_stream(num: u32, trailer: &Dict, rows: &BTreeMap<u32, Row>, w2: usize, prev: u64) -> Result<Vec<u8>, CosError> {
    let w2 = w2.clamp(1, 8);
    let first = rows.keys().next().copied().unwrap_or(num);
    if rows.keys().zip(rows.keys().skip(1)).any(|(a, b)| a.checked_add(1) != Some(*b)) {
        return Err(refuse("the first-page section's objects are not numbered consecutively"));
    }
    let w3 = rows.values().map(|r| field_bytes(row_fields(r).2)).max().unwrap_or(1);
    let mut data = Vec::with_capacity(rows.len().saturating_mul(1 + w2 + w3));
    for row in rows.values() {
        let (kind, offset, index) = row_fields(row);
        if field_bytes(offset) > w2 {
            return Err(refuse("an offset does not fit the first-page cross-reference stream"));
        }
        data.push(kind);
        data.extend(offset.to_be_bytes().iter().skip(8 - w2));
        data.extend(index.to_be_bytes().iter().skip(8usize.saturating_sub(w3)));
    }
    let mut d = trailer.clone();
    d.remove(b"Prev");
    d.set(b"Type".to_vec(), Object::name("XRef"));
    d.set(b"Index".to_vec(), Object::Array(vec![Object::Int(i64::from(first)), Object::Int(rows.len() as i64)]));
    d.set(b"W".to_vec(), Object::Array(vec![Object::Int(1), Object::Int(w2 as i64), Object::Int(w3 as i64)]));
    d.set(b"Length".to_vec(), Object::Int(data.len() as i64));
    let mut dict = Vec::new();
    write_dict(&d, &mut dict);
    dict.truncate(dict.len().saturating_sub(2)); // reopen it after the last entry
    let mut out = Vec::with_capacity(dict.len() + data.len() + 96);
    let _ = writeln!(out, "{num} 0 obj");
    out.extend_from_slice(&dict);
    let _ = write!(out, "/Prev {prev:<WIDEST$}>>\nstream\n");
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream\nendobj\nstartxref\n0\n%%EOF\n");
    Ok(out)
}

/// The parameter dictionary's values (Table F.1).
struct LinValues {
    file_len: u64,
    hint_at: u64,
    hint_len: u64,
    first_page: u32,
    first_page_end: u64,
    pages: usize,
    main_xref_entry: u64,
}

impl LinValues {
    /// The widest the offsets could be, to reserve the dictionary's space before they are known.
    fn placeholder(first_page: u32, pages: usize) -> Self {
        let wide = 10u64.pow(WIDEST as u32 - 1);
        LinValues { file_len: wide, hint_at: wide, hint_len: wide, first_page, first_page_end: wide, pages, main_xref_entry: wide }
    }
}

fn lin_object(num: u32, v: &LinValues) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = write!(
        out,
        "{num} 0 obj\n<</Linearized 1/L {}/H [{} {}]/O {}/E {}/N {}/T {}>>\nendobj\n",
        v.file_len, v.hint_at, v.hint_len, v.first_page, v.first_page_end, v.pages, v.main_xref_entry
    );
    out
}

/// The first-page trailer with `/Prev` (the main table's offset) padded to a fixed width, then
/// a `startxref` of 0: readers start from the end of the file, whose `startxref` names the
/// first-page table (Annex F, example in F.3).
fn first_trailer(trailer: &Dict, prev: u64) -> Vec<u8> {
    let mut out = b"trailer\n".to_vec();
    let mut dict = Vec::new();
    write_dict(trailer, &mut dict);
    dict.truncate(dict.len().saturating_sub(2)); // reopen it after the last entry
    out.extend_from_slice(&dict);
    let _ = write!(out, "/Prev {prev:<WIDEST$}>>\nstartxref\n0\n%%EOF\n");
    out
}

impl Document {
    /// Whether the file as opened is linearized (Fast Web View, Annex F): it starts with a
    /// linearization parameter dictionary whose length is the file's, whose newest
    /// cross-reference section is the first-page one, whose page count and first page match the
    /// page tree and whose hint stream is where `/H` says. A dictionary that fails any of these
    /// is [`Linearization::Stale`]: readers treat the file as an ordinary one. Edits made since
    /// opening do not count; saving them incrementally makes a linearized file stale.
    pub fn linearization(&self) -> Linearization {
        let data = self.bytes();
        let Some(header) = find(data, b"%PDF-", 0, data.len().min(FIRST_KB)) else { return Linearization::No };
        let mut lx = Lexer::new(data, header);
        lx.skip_ws(); // the header line and the binary comment are comments
        let at = lx.pos;
        // Only a dictionary that names /Linearized in the first kilobyte is worth parsing: the
        // first object of an ordinary file can be a large stream.
        if at >= FIRST_KB || find(data, b"/Linearized", at, data.len().min(FIRST_KB)).is_none() {
            return Linearization::No;
        }
        let Ok((_, Object::Dict(d))) = parse_indirect_shared(data, at, &|_| None, &Default::default()) else { return Linearization::No };
        if d.get(b"Linearized").and_then(Object::as_f64).is_none() {
            return Linearization::No;
        }
        let len = data.len() as i64;
        if d.int(b"L") != Some(len) {
            return Linearization::Stale(StaleReason::Updated);
        }
        let int = |key: &[u8]| d.int(key).filter(|v| (0..=len).contains(v));
        let hint = d.get(b"H").and_then(Object::as_array).filter(|h| matches!(h.len(), 2 | 4));
        let hint_at = hint.and_then(|h| h.first()).and_then(Object::as_int).filter(|v| (0..len).contains(v));
        let hint_len = hint.and_then(|h| h.get(1)).and_then(Object::as_int).filter(|v| (0..len).contains(v));
        let (Some(first_page), Some(end), Some(pages), Some(main_entry), Some(hint_at), Some(hint_len)) =
            (d.int(b"O"), int(b"E"), d.int(b"N"), int(b"T"), hint_at, hint_len)
        else {
            return Linearization::Stale(StaleReason::Parameters);
        };
        // Readers start from the end: its startxref must lead to the first-page section, which
        // sits between the parameter dictionary and the end of the first page.
        match self.revisions().last() {
            Some(newest) if (at as u64) < newest.xref_offset && (newest.xref_offset as i64) < end => {}
            _ => return Linearization::Stale(StaleReason::Xref),
        }
        // The hint stream object starts exactly at /H[0] and ends (`endobj`, or the indirect
        // /Length object some writers put right after it) /H[1] bytes later.
        let (hint_at, hint_end) = (hint_at as usize, hint_at.saturating_add(hint_len) as usize);
        let starts_there =
            data.get(hint_at).is_some_and(u8::is_ascii_digit) && (hint_at == 0 || data.get(hint_at - 1).is_some_and(|b| crate::is_whitespace(*b)));
        let ends_there = data.get(..hint_end).is_some_and(|d| d.trim_ascii_end().ends_with(b"endobj"));
        let hint_is_stream =
            starts_there && ends_there && matches!(parse_indirect_shared(data, hint_at, &|_| None, &Default::default()), Ok((_, Object::Stream(_))));
        // /T: the white-space in front of the main table's entry for object 0, or the main
        // cross-reference stream (Table F.1; qpdf points at the white-space in front of it).
        let main_section_there = usize::try_from(main_entry).ok().is_some_and(|t| {
            data.get(t).is_some_and(|b| crate::is_whitespace(*b))
                || ((t == 0 || data.get(t - 1).is_some_and(|b| crate::is_whitespace(*b)))
                    && matches!(parse_indirect_shared(data, t, &|_| None, &Default::default()), Ok((_, Object::Stream(s))) if s.dict.name(b"Type") == Some(b"XRef")))
        });
        if !hint_is_stream || !main_section_there {
            return Linearization::Stale(StaleReason::Parameters);
        }
        let Some(root) = self.root() else { return Linearization::Stale(StaleReason::Pages) };
        match page_tree(&mut self.object_reader(), root, false) {
            Ok(tree) if tree.pages.len() as i64 == pages && tree.pages.first().is_some_and(|p| i64::from(p.num) == first_page) => Linearization::Yes,
            _ => Linearization::Stale(StaleReason::Pages),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfString;

    #[test]
    fn bits_pack_most_significant_first_and_align_to_bytes() {
        let mut w = Bits::default();
        w.put(0b101, 3);
        w.put(0b1, 1);
        w.align();
        w.put(0xABCD, 16);
        w.put(0, 0);
        w.put(0b11, 2);
        w.align();
        w.put(u64::from(u32::MAX), 32);
        assert_eq!(w.out, vec![0b1011_0000, 0xAB, 0xCD, 0b1100_0000, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn bit_counts_match_the_values_they_hold() {
        assert_eq!(bits_for(0), 0);
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(2), 2);
        assert_eq!(bits_for(255), 8);
        assert_eq!(bits_for(256), 9);
        assert_eq!(bits_for(u64::from(u32::MAX)), 32);
    }

    #[test]
    fn the_first_page_trailer_keeps_its_width_whatever_prev_is() {
        let mut d = Dict::new();
        d.set(b"Size".to_vec(), Object::Int(9));
        d.set(b"ID".to_vec(), Object::Array(vec![Object::String(PdfString::literal(b"x".to_vec()))]));
        let short = first_trailer(&d, 0);
        let long = first_trailer(&d, u64::MAX);
        assert_eq!(short.len(), long.len());
        let parsed = Lexer::new(&long, b"trailer\n".len()).object().unwrap();
        assert_eq!(
            parsed.as_dict().and_then(|t| t.get(b"Prev")).and_then(|p| match p {
                Object::Int(_) | Object::Real(_) => Some(()),
                _ => None,
            }),
            Some(()),
            "u64::MAX still parses as a number"
        );
        let parsed = Lexer::new(&short, b"trailer\n".len()).object().unwrap();
        assert_eq!(parsed.as_dict().and_then(|t| t.int(b"Prev")), Some(0));
        assert_eq!(parsed.as_dict().and_then(|t| t.int(b"Size")), Some(9));
    }

    #[test]
    fn the_first_page_cross_reference_stream_keeps_its_length_whatever_the_offsets() {
        let mut trailer = Dict::new();
        trailer.set(b"Size".to_vec(), Object::Int(9));
        trailer.set(b"ID".to_vec(), Object::Array(vec![Object::String(PdfString::literal(b"x".to_vec()))]));
        let rows = |o: u64| BTreeMap::from([(5, Row::InFile(o, 0)), (6, Row::InStream(u32::try_from(o).unwrap(), 3)), (7, Row::InFile(o, 0))]);
        let short = first_xref_stream(7, &trailer, &rows(0), 3, 0).unwrap();
        let long = first_xref_stream(7, &trailer, &rows(0xFF_FFFF), 3, u64::MAX).unwrap();
        assert_eq!(short.len(), long.len());
        assert!(long.ends_with(b"endobj\nstartxref\n0\n%%EOF\n"));
        let (r, object) = parse_indirect_shared(&Arc::new(long), 0, &|_| None, &Default::default()).unwrap();
        assert_eq!(r.num, 7);
        let Object::Stream(s) = object else { panic!("a stream") };
        assert_eq!(s.dict.name(b"Type"), Some(b"XRef".as_slice()));
        assert!(s.dict.get(b"Filter").is_none(), "unfiltered");
        let ints = |key: &[u8]| -> Vec<i64> { s.dict.get(key).unwrap().as_array().unwrap().iter().map(|v| v.as_int().unwrap()).collect() };
        assert_eq!(ints(b"Index"), vec![5, 3]);
        assert_eq!(ints(b"W"), vec![1, 3, 1]);
        assert_eq!(s.dict.int(b"Size"), Some(9));
        assert!(s.dict.get(b"ID").is_some() && s.dict.get(b"Prev").is_some());
        assert_eq!(s.decoded().unwrap(), [1, 0xFF, 0xFF, 0xFF, 0, 2, 0xFF, 0xFF, 0xFF, 3, 1, 0xFF, 0xFF, 0xFF, 0]);
        // An offset wider than its field is refused, never cut short; so are gaps in the numbers.
        assert!(first_xref_stream(7, &trailer, &rows(1 << 24), 3, 0).is_err());
        let gap = BTreeMap::from([(5, Row::InFile(1, 0)), (7, Row::InFile(2, 0))]);
        assert!(first_xref_stream(7, &trailer, &gap, 3, 0).is_err());
    }

    #[test]
    fn the_parameter_dictionary_placeholder_is_never_narrower_than_the_real_values() {
        let wide = lin_object(7, &LinValues::placeholder(12, 3)).len();
        let real =
            LinValues { file_len: u64::from(u32::MAX), hint_at: 1, hint_len: 2, first_page: 12, first_page_end: 3, pages: 3, main_xref_entry: 4 };
        assert!(lin_object(7, &real).len() <= wide);
        assert!(wide < FIRST_KB - 32, "the dictionary fits the first kilobyte after the header");
    }
}
