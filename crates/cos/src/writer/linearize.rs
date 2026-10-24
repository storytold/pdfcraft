//! Linearized files: Fast Web View (ISO 32000-2 Annex F).
//!
//! The first page and the objects it needs come first, after a dictionary saying the file is
//! linearized and a hint stream locating every later page, so a reader fetching the file over
//! HTTP can show page 1 before the rest arrives. File order (Annex F.3):
//!
//! 1. the header
//! 2. the linearization parameter dictionary
//! 3. the first-page cross-reference section and the trailer
//! 4. the catalog and the document-level objects
//! 5. the primary hint stream
//! 6. the first page and every object it reaches
//! 7. each later page, with the objects only it reaches
//! 8. the objects later pages share
//! 9. everything else
//! 11. the main cross-reference section, for parts 7–9, which take the low object numbers
//!
//! What is document-level and what a page's section holds follow what `qpdf
//! --check-linearization` expects, observed as a black box: the catalog's `/OpenAction`,
//! `/ViewerPreferences`, `/Threads` and `/AcroForm` with everything they reach are
//! document-level; a page's section stops at other pages, the page tree, those document-level
//! objects and thumbnails, and goes through the catalog; open outlines take only what no page
//! reaches.
//!
//! Written with classic cross-reference tables and no object streams, so a linearized file is
//! larger than a compressed full save. Hint tables hold 32-bit offsets: larger files are refused.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Write;

use super::{
    SaveOptions, collect_refs, prepared, renumber, reread, rewritten_trailer, stamp_mod_date, trailer_roots, write_dict, write_indirect,
    write_xref_table,
};
use crate::CosError;
use crate::document::{Document, ObjectReader};
use crate::object::{Dict, ObjRef, Object, Stream};

/// Page tree nodes visited before giving up, as the model's page tree walk.
const MAX_TREE_NODES: usize = 1_000_000;
/// References followed while sorting objects into pages. Every page walks the objects it
/// reaches, so a document needing more is refused rather than taking unbounded time.
const MAX_WORK: usize = 50_000_000;
/// Page attributes a page can inherit (§7.7.3.4), which a linearized page states itself.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
/// Catalog entries whose whole reach is document-level (part 4).
const DOCUMENT_LEVEL: [&[u8]; 4] = [b"OpenAction", b"ViewerPreferences", b"Threads", b"AcroForm"];
/// Characters a linearization dictionary value or the first trailer's `/Prev` is padded to: any
/// `u32`. They are only known once everything after them is laid out.
const WIDTH: usize = 10;

fn refuse(detail: impl std::fmt::Display) -> CosError {
    CosError::Syntax { offset: 0, detail: format!("can't linearize: {detail}") }
}

/// A position, length or count as a hint table or the linearization dictionary holds it.
fn small(v: u64) -> Result<u32, CosError> {
    u32::try_from(v).map_err(|_| refuse("the file would be 4 GB or larger"))
}

/// Whether `bytes` is a linearized file: its first object is a linearization dictionary, within
/// the first 1,024 bytes, whose `/L` is the file's length. An update appended later leaves the
/// dictionary behind but changes the length, and the file is then no longer linearized.
pub fn is_linearized(bytes: &[u8]) -> bool {
    let head = bytes.get(..bytes.len().min(1024)).unwrap_or(bytes);
    let Ok((_, Object::Dict(d))) = crate::parser::parse_indirect(head, 0, &|_| None) else { return false };
    d.contains(b"Linearized") && d.int(b"L").and_then(|l| usize::try_from(l).ok()) == Some(bytes.len())
}

/// Where an object goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// Not reached from any page: part 9.
    Rest,
    /// Part 4.
    Document,
    /// Part 6.
    First,
    /// Part 7, in this page's section: no other page reaches it.
    Page(usize),
    /// Part 8: later pages share it and the first page doesn't reach it.
    Shared,
}

/// The page objects in order, and every intermediate node.
struct Tree {
    pages: Vec<ObjRef>,
    nodes: Vec<ObjRef>,
}

/// The inheritable attributes a node passes down to its kids.
type Inherited = Vec<(&'static [u8], Object)>;

/// Whether a dictionary states a value: null, or a reference to nothing, counts as absent
/// (§7.3.9–7.3.10), so a page with a dangling `/Resources` still inherits its node's.
fn stated(reader: &mut ObjectReader, v: Option<&Object>) -> bool {
    match v {
        None | Some(Object::Null) => false,
        Some(Object::Ref(r)) => !matches!(*reader.get(*r), Object::Null),
        Some(_) => true,
    }
}

/// Walk the page tree, never following `/Parent`, and move the inheritable attributes from the
/// nodes onto each page. A value a node passes down, other than a number or a reference, becomes
/// one object its pages share rather than a copy on each.
fn page_tree(reader: &mut ObjectReader) -> Result<Tree, CosError> {
    let catalog = reader.document.root().map(|r| reader.get(r));
    let top =
        catalog.as_deref().and_then(Object::as_dict).and_then(|d| d.reference(b"Pages")).ok_or_else(|| refuse("the document has no page tree"))?;
    let mut tree = Tree { pages: Vec::new(), nodes: Vec::new() };
    // The catalog can't be a page or a node either: it has its own place in the file.
    let mut seen: HashSet<u32> = reader.document.root().map(|r| r.num).into_iter().collect();
    let mut stack: Vec<(ObjRef, Inherited)> = vec![(top, Vec::new())];
    while let Some((r, mut inherited)) = stack.pop() {
        // A page listed twice would need two places in the file.
        if !seen.insert(r.num) {
            return Err(refuse(format!("object {} appears twice in the page tree", r.num)));
        }
        if seen.len() > MAX_TREE_NODES {
            return Err(refuse("the page tree is too large"));
        }
        let o = reader.get(r);
        let Some(d) = o.as_dict() else { continue }; // a broken kid, which readers skip too
        let node = d.name(b"Type") == Some(&b"Pages"[..]) || (d.contains(b"Kids") && d.name(b"Type") != Some(&b"Page"[..]));
        if !node {
            let mut missing = Vec::new();
            for (k, v) in inherited {
                if !stated(reader, d.get(k)) {
                    missing.push((k, v));
                }
            }
            if !missing.is_empty() {
                reader.document.update_dict(r, |page| missing.into_iter().for_each(|(k, v)| page.set(k.to_vec(), v)))?;
            }
            tree.pages.push(r);
            continue;
        }
        tree.nodes.push(r);
        let mut moved = false;
        for key in INHERITED {
            moved |= d.contains(key);
            let Some(v) = d.get(key).filter(|v| stated(reader, Some(v))) else { continue };
            let v = match v {
                Object::Bool(_) | Object::Int(_) | Object::Real(_) | Object::Ref(_) => v.clone(),
                // Every kid on the stack carries what it inherits: never a copy of a large value.
                other => {
                    // Adding scans every object number once a file has used them all up.
                    if reader.document.next_num() == u32::MAX {
                        return Err(refuse("the document has no object numbers left"));
                    }
                    Object::Ref(reader.document.add(other.clone()))
                }
            };
            inherited.retain(|(k, _)| *k != key);
            inherited.push((key, v));
        }
        // The pages state them now; qpdf's check rejects a node that still does.
        if moved {
            reader.document.update_dict(r, |n| INHERITED.iter().for_each(|k| drop(n.remove(k))))?;
        }
        let kids = match d.get(b"Kids") {
            Some(Object::Ref(k)) => reader.get(*k).as_array().cloned().unwrap_or_default(),
            Some(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        stack.extend(kids.iter().rev().filter_map(Object::as_ref).map(|k| (k, inherited.clone())));
    }
    if tree.pages.is_empty() {
        return Err(refuse("the document has no pages"));
    }
    Ok(tree)
}

/// Every object a full rewrite writes, breadth-first from the trailer, with the objects each
/// one references. Objects are keyed by number: `5 1 R` and `5 0 R` name the same object.
struct Graph {
    order: Vec<(ObjRef, bool)>,
    index: HashMap<u32, usize>,
    edges: Vec<Vec<usize>>,
    /// Every reference that resolved, so each is renumbered to its object.
    refs: HashSet<ObjRef>,
}

impl Graph {
    fn walk(reader: &mut ObjectReader, trailer: &Dict) -> Self {
        let mut order = Vec::new();
        let mut index = HashMap::new();
        let mut targets: Vec<Vec<u32>> = Vec::new();
        let mut refs = HashSet::new();
        let mut queue = VecDeque::new();
        trailer_roots(trailer, &mut |r| queue.push_back(r));
        while let Some(r) = queue.pop_front() {
            if index.contains_key(&r.num) {
                refs.insert(r);
                continue;
            }
            let o = reader.get(r);
            if matches!(*o, Object::Null) {
                continue; // dangling: written as null
            }
            refs.insert(r);
            index.insert(r.num, order.len());
            order.push((r, matches!(*o, Object::Stream(_))));
            let mut out = Vec::new();
            collect_refs(&o, &mut |x| {
                queue.push_back(x);
                out.push(x.num);
            });
            targets.push(out);
        }
        let edges = targets.iter().map(|t| t.iter().filter_map(|n| index.get(n).copied()).collect()).collect();
        Self { order, index, edges, refs }
    }

    fn at(&self, r: ObjRef) -> Option<usize> {
        self.index.get(&r.num).copied()
    }

    /// Breadth-first from `start` through the objects `enter` accepts, each once, `start` first.
    /// Every reference followed counts against `work`.
    fn reach(&self, start: usize, mark: &mut [u32], stamp: u32, work: &mut usize, enter: impl Fn(usize) -> bool) -> Result<Vec<usize>, CosError> {
        let mut visited = Vec::new();
        let mut queue = VecDeque::from([start]);
        if let Some(m) = mark.get_mut(start) {
            *m = stamp;
        }
        while let Some(i) = queue.pop_front() {
            visited.push(i);
            for &t in self.edges.get(i).into_iter().flatten() {
                *work += 1;
                if *work > MAX_WORK {
                    return Err(refuse("the pages share too many objects to sort out"));
                }
                let Some(m) = mark.get_mut(t) else { continue };
                if *m != stamp && enter(t) {
                    *m = stamp;
                    queue.push_back(t);
                }
            }
        }
        Ok(visited)
    }
}

/// Which part each object goes in, in file order within each part.
struct Plan {
    /// Part 4, the catalog first.
    document: Vec<usize>,
    /// Part 6, the first page's page object first.
    first: Vec<usize>,
    /// Part 7: each later page's section, its page object first.
    pages: Vec<Vec<usize>>,
    /// Part 8.
    shared: Vec<usize>,
    /// Part 9, the outlines first.
    rest: Vec<usize>,
    /// For each later page, the part 6 and part 8 objects it reaches.
    uses: Vec<Vec<usize>>,
    /// The outline tree, together at the end of part 6 or the start of part 9, for the outline
    /// hint table.
    outlines: Vec<usize>,
}

impl Plan {
    fn new(reader: &mut ObjectReader, graph: &Graph, tree: &Tree, trailer: &Dict) -> Result<Self, CosError> {
        let n = graph.order.len();
        let missing = |r: ObjRef| refuse(format!("object {} is missing", r.num));
        let pages: Vec<usize> = tree.pages.iter().map(|r| graph.at(*r).ok_or_else(|| missing(*r))).collect::<Result<_, _>>()?;
        let root = trailer.reference(b"Root").ok_or_else(|| refuse("the document has no catalog"))?;
        let catalog = graph.at(root).ok_or_else(|| missing(root))?;
        let mut part = vec![Part::Rest; n];
        // Other pages, the page tree and thumbnails end a page's section. (A thumbnail another
        // page also draws stays out of that page's too: qpdf places such an image by rules of
        // its own, which neither choice matches.)
        let mut blocked = vec![false; n];
        for (p, &i) in pages.iter().enumerate() {
            blocked[i] = true;
            part[i] = if p == 0 { Part::First } else { Part::Page(p) };
            if let Some(t) = reader.get(tree.pages[p]).as_dict().and_then(|d| d.reference(b"Thumb")).and_then(|t| graph.at(t)) {
                blocked[t] = true;
            }
        }
        for i in tree.nodes.iter().filter_map(|r| graph.at(*r)) {
            blocked[i] = true;
        }
        let mut mark = vec![0u32; n];
        let mut stamp = 0u32;
        let mut work = 0usize;

        // Part 4: the catalog, the encryption dictionary with everything it references, and the
        // reach of the document-level catalog entries.
        part[catalog] = Part::Document;
        let mut document = vec![catalog];
        let catalog_dict = reader.get(root).as_dict().cloned().unwrap_or_default();
        let mut starts: Vec<(usize, bool)> = trailer.reference(b"Encrypt").and_then(|r| graph.at(r)).map(|e| (e, true)).into_iter().collect();
        for key in DOCUMENT_LEVEL {
            if let Some(v) = catalog_dict.get(key) {
                collect_refs(v, &mut |r| starts.extend(graph.at(r).map(|i| (i, false))));
            }
        }
        for (start, encryption) in starts {
            if part[start] != Part::Rest || (!encryption && blocked[start]) {
                continue;
            }
            stamp += 1;
            let reached = graph.reach(start, &mut mark, stamp, &mut work, |t| part[t] == Part::Rest && (encryption || !blocked[t]))?;
            for i in reached {
                part[i] = Part::Document;
                document.push(i);
            }
        }

        // Part 6: the first page and its reach. A page's reach goes through the catalog, which
        // stays document-level, but not past the other document-level objects.
        let enter = |part: &[Part], t: usize| !blocked[t] && (t == catalog || part[t] != Part::Document);
        stamp += 1;
        let first = graph.reach(pages[0], &mut mark, stamp, &mut work, |t| enter(&part, t))?;
        let mut first: Vec<usize> = first.into_iter().filter(|&i| i == pages[0] || part[i] == Part::Rest).collect();
        for &i in &first {
            part[i] = Part::First;
        }

        // Parts 7 and 8: what only one later page reaches is that page's; what several reach,
        // shared.
        let mut own: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
        let mut shared = Vec::new();
        for (p, &page) in pages.iter().enumerate().skip(1) {
            stamp += 1;
            for i in graph.reach(page, &mut mark, stamp, &mut work, |t| enter(&part, t))? {
                match part[i] {
                    Part::Rest => {
                        part[i] = Part::Page(p);
                        own[p].push(i);
                    }
                    Part::Page(q) if q != p => {
                        part[i] = Part::Shared;
                        shared.push(i);
                    }
                    _ => {}
                }
            }
        }

        // Outlines shown on opening end part 6: what no page reaches, which qpdf gives the pages.
        let outline_root = catalog_dict.reference(b"Outlines").and_then(|r| graph.at(r)).filter(|&o| !blocked[o]);
        let page_mode = match catalog_dict.get(b"PageMode") {
            Some(Object::Ref(r)) => reader.get(*r).as_name().map(<[u8]>::to_vec),
            v => v.and_then(Object::as_name).map(<[u8]>::to_vec),
        };
        let mut outlines = Vec::new();
        if page_mode.as_deref() == Some(&b"UseOutlines"[..])
            && let Some(root) = outline_root.filter(|&o| part[o] == Part::Rest)
        {
            stamp += 1;
            outlines = graph.reach(root, &mut mark, stamp, &mut work, |t| !blocked[t] && part[t] == Part::Rest)?;
            for &i in &outlines {
                part[i] = Part::First;
            }
            first.extend(&outlines);
        }

        // Each later page's uses of part 6 and part 8, transitively.
        let mut uses = vec![Vec::new(); pages.len()];
        for (p, &page) in pages.iter().enumerate().skip(1) {
            stamp += 1;
            let reached = graph.reach(page, &mut mark, stamp, &mut work, |t| enter(&part, t))?;
            uses[p] = reached.into_iter().filter(|&i| matches!(part[i], Part::First | Part::Shared)).collect();
        }
        let pages: Vec<Vec<usize>> = pages
            .iter()
            .enumerate()
            .skip(1)
            .map(|(p, &page)| std::iter::once(page).chain(own[p].iter().copied().filter(|&i| part[i] == Part::Page(p))).collect())
            .collect();
        // Outlines not shown on opening start part 9, in one piece.
        let mut rest = Vec::new();
        if outlines.is_empty()
            && let Some(root) = outline_root.filter(|&o| part[o] == Part::Rest)
        {
            stamp += 1;
            outlines = graph.reach(root, &mut mark, stamp, &mut work, |t| !blocked[t] && part[t] == Part::Rest)?;
            rest.clone_from(&outlines);
        }
        let mut placed = vec![false; n];
        rest.iter().for_each(|&i| placed[i] = true);
        rest.extend((0..n).filter(|&i| part[i] == Part::Rest && !placed[i]));
        Ok(Self { document, first, pages, shared, rest, uses, outlines })
    }
}

/// A big-endian bit stream (Annex F.4): each table, and each array of one item for every page
/// or group, starts on a byte boundary.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    current: u8,
    used: u32,
}

impl Bits {
    /// The low `width` bits of `value`, most significant first.
    fn put(&mut self, value: u32, width: u32) {
        for bit in (0..width.min(32)).rev() {
            self.current = (self.current << 1) | ((value >> bit) & 1) as u8;
            self.used += 1;
            if self.used == 8 {
                self.bytes.push(self.current);
                self.current = 0;
                self.used = 0;
            }
        }
    }

    /// Pad with zero bits to the next byte boundary.
    fn align(&mut self) {
        if self.used > 0 {
            self.bytes.push(self.current << (8 - self.used));
            self.current = 0;
            self.used = 0;
        }
    }
}

/// Bits needed to store `v`: zero for zero.
fn bits(v: u32) -> u32 {
    32 - v.leading_zeros()
}

/// The least of `values` and the bits for the largest difference from it.
fn spread(values: &[u32]) -> (u32, u32) {
    let least = values.iter().copied().min().unwrap_or(0);
    let most = values.iter().copied().max().unwrap_or(0);
    (least, bits(most - least))
}

/// What the hint tables describe. Positions are as if the hint stream were not in the file
/// (Annex F.4): the layout before it is placed.
struct Hints<'a> {
    /// Objects and length of each page's section, the first page's being part 6.
    objects: Vec<u32>,
    lengths: Vec<u32>,
    /// Shared object identifiers each page uses (none for the first page).
    uses: Vec<Vec<u32>>,
    first_page_at: u32,
    /// Length of each shared object table entry: part 6 objects, then part 8 objects.
    groups: &'a [u32],
    first_page_groups: u32,
    /// Number and position of the first part 8 object.
    shared_at: Option<(u32, u32)>,
    /// The outline tree's first object number and position, object count and length.
    outlines: Option<[u32; 4]>,
}

impl Hints<'_> {
    /// The hint stream's data, `/S` (the shared object table's position in it) and `/O` (the
    /// outline table's).
    fn encode(&self) -> Result<(Vec<u8>, usize, Option<usize>), CosError> {
        let mut b = Bits::default();
        // Page offset hint table (Table F.3): content stream fields are written as qpdf and
        // Acrobat write them (offset 0, length the page's), which Acrobat ignores on reading.
        let (least_objects, objects_bits) = spread(&self.objects);
        let (least_length, length_bits) = spread(&self.lengths);
        let counts: Vec<u32> = self.uses.iter().map(|u| small(u.len() as u64)).collect::<Result<_, _>>()?;
        let count_bits = bits(counts.iter().copied().max().unwrap_or(0));
        let groups = small(self.groups.len() as u64)?;
        let id_bits = bits(groups.saturating_sub(1));
        for (value, width) in [
            (least_objects, 32),
            (self.first_page_at, 32),
            (objects_bits, 16),
            (least_length, 32),
            (length_bits, 16),
            (0, 32),
            (0, 16),
            (least_length, 32),
            (length_bits, 16),
            (count_bits, 16),
            (id_bits, 16),
            (0, 16),
            (4, 16),
        ] {
            b.put(value, width);
        }
        let objects: Vec<u32> = self.objects.iter().map(|n| n - least_objects).collect();
        let lengths: Vec<u32> = self.lengths.iter().map(|n| n - least_length).collect();
        for (values, width) in [(&objects, objects_bits), (&lengths, length_bits), (&counts, count_bits)] {
            for &v in values {
                b.put(v, width);
            }
            b.align();
        }
        for id in self.uses.iter().flatten() {
            b.put(*id, id_bits);
        }
        b.align();
        // Numerators and content stream offsets take no bits; content lengths are page lengths.
        for &v in &lengths {
            b.put(v, length_bits);
        }
        b.align();
        let shared_table = b.bytes.len();
        // Shared object hint table (Table F.5): one object per group.
        let (least_group, group_bits) = spread(self.groups);
        let (shared_num, shared_at) = self.shared_at.unwrap_or((0, 0));
        for (value, width) in
            [(shared_num, 32), (shared_at, 32), (self.first_page_groups, 32), (groups, 32), (0, 16), (least_group, 32), (group_bits, 16)]
        {
            b.put(value, width);
        }
        for &g in self.groups {
            b.put(g - least_group, group_bits);
        }
        b.align();
        // No MD5 signatures.
        for _ in self.groups {
            b.put(0, 1);
        }
        b.align();
        // Generic hint table for the outlines (Table F.9), which qpdf requires when there are any.
        let outline_table = self.outlines.map(|table| {
            let at = b.bytes.len();
            table.iter().for_each(|&v| b.put(v, 32));
            at
        });
        Ok((b.bytes, shared_table, outline_table))
    }
}

/// The linearization parameter dictionary, its values padded so its length doesn't depend on
/// them: [`/L`, `/H` offset, `/H` length, `/O`, `/E`, `/N`, `/T`].
fn parameters(num: u32, v: [u32; 7]) -> Vec<u8> {
    let [l, h, hl, o, e, n, t] = v;
    let mut out = Vec::new();
    let _ = writeln!(
        out,
        "{num} 0 obj\n<</Linearized 1/L {l:<WIDTH$}/H [ {h:<WIDTH$} {hl:<WIDTH$} ]/O {o:<WIDTH$}/E {e:<WIDTH$}/N {n:<WIDTH$}/T {t:<WIDTH$}>>\nendobj"
    );
    out
}

/// The first-page cross-reference section (one subsection, no free entry) and the trailer,
/// whose `/Prev` names the main section.
fn first_section(first: u32, offsets: &[u64], trailer: &Dict, prev: u64) -> Vec<u8> {
    let rows: BTreeMap<u32, (u64, u16, bool)> = (first..).zip(offsets).map(|(n, o)| (n, (*o, 0, true))).collect();
    let mut out = Vec::new();
    write_xref_table(&mut out, &rows, false);
    out.extend_from_slice(b"trailer\n");
    write_dict(trailer, &mut out);
    out.truncate(out.len().saturating_sub(2)); // `>>`
    let _ = writeln!(out, "/Prev {prev:<WIDTH$}>>");
    out
}

/// A linearized full rewrite (see the module documentation).
pub(super) fn write(doc: &Document, opts: &SaveOptions) -> Result<Vec<u8>, CosError> {
    if doc.stream_limited() {
        return Err(CosError::ReadOnlyLimit);
    }
    let mut reader = doc.object_reader();
    stamp_mod_date(&mut reader.document, opts);
    let tree = page_tree(&mut reader)?;
    let trailer_in = reader.document.trailer().clone();
    let graph = Graph::walk(&mut reader, &trailer_in);
    let plan = Plan::new(&mut reader, &graph, &tree, &trailer_in)?;

    // Numbers: parts 7–9 from 1 in file order; then the linearization dictionary, part 4, the
    // hint stream and part 6 (Annex F.3).
    let too_many = || refuse("too many objects");
    let count = |n: usize| u32::try_from(n).map_err(|_| too_many());
    let main: Vec<usize> = plan.pages.iter().flatten().chain(&plan.shared).chain(&plan.rest).copied().collect();
    let mut number = vec![0u32; graph.order.len()];
    for (n, &i) in (1u32..).zip(&main) {
        number[i] = n;
    }
    let lin = count(main.len())?.checked_add(1).ok_or_else(too_many)?;
    let mut next = lin.checked_add(1).ok_or_else(too_many)?;
    for &i in &plan.document {
        number[i] = next;
        next = next.checked_add(1).ok_or_else(too_many)?;
    }
    let hint = next;
    next = next.checked_add(1).ok_or_else(too_many)?;
    for &i in &plan.first {
        number[i] = next;
        next = next.checked_add(1).ok_or_else(too_many)?;
    }
    let size = next;
    let map: HashMap<ObjRef, u32> = graph.refs.iter().filter_map(|r| graph.at(*r).map(|i| (*r, number[i]))).collect();

    // Every object written once, encrypted with its new number.
    let mut body = |i: usize| -> Result<Vec<u8>, CosError> {
        let (r, is_stream) = graph.order[i];
        let o = renumber(&*reread(&mut reader, r, is_stream)?, &map);
        let mut out = Vec::new();
        write_indirect(number[i], 0, &prepared(&reader.document, r.num, number[i], 0, &o), &mut out);
        Ok(out)
    };
    let document: Vec<Vec<u8>> = plan.document.iter().map(|&i| body(i)).collect::<Result<_, _>>()?;
    let first: Vec<Vec<u8>> = plan.first.iter().map(|&i| body(i)).collect::<Result<_, _>>()?;
    let rest: Vec<Vec<u8>> = main.iter().map(|&i| body(i)).collect::<Result<_, _>>()?;

    let mut trailer = rewritten_trailer(&reader.document, &trailer_in, &map, opts);
    trailer.set(b"Size".to_vec(), Object::Int(i64::from(size)));
    let version = reader.document.version().chars().take(3).collect::<String>();
    let mut header = format!("%PDF-{version}\n").into_bytes();
    header.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);

    // Layout without the hint stream: these are the positions hint tables hold.
    let len = |b: &[u8]| b.len() as u64;
    let lin_len = len(&parameters(lin, [u32::MAX; 7]));
    let first_count = 1 + plan.document.len() + 1 + plan.first.len();
    let section_at = len(&header) + lin_len;
    let section_len = len(&first_section(lin, &vec![0; first_count], &trailer, 0));
    let mut at = section_at + section_len;
    let mut document_at = Vec::with_capacity(document.len());
    for b in &document {
        document_at.push(at);
        at += len(b);
    }
    let hint_at = at;
    let mut first_at = Vec::with_capacity(first.len());
    for b in &first {
        first_at.push(at);
        at += len(b);
    }
    let first_end = at;
    let mut rest_at = Vec::with_capacity(rest.len());
    for b in &rest {
        rest_at.push(at);
        at += len(b);
    }
    let main_at = at;

    // Hint tables.
    let position: HashMap<usize, usize> = main.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let mut objects = vec![count(plan.first.len())?];
    let mut lengths = vec![small(first_end - hint_at)?];
    for section in &plan.pages {
        objects.push(count(section.len())?);
        let length = section.iter().filter_map(|i| position.get(i)).map(|&k| len(&rest[k])).sum();
        lengths.push(small(length)?);
    }
    let mut groups: Vec<u32> = first.iter().map(|b| small(len(b))).collect::<Result<_, _>>()?;
    let mut identifier = HashMap::new();
    for (g, &i) in plan.first.iter().enumerate() {
        identifier.insert(i, count(g)?);
    }
    for (g, &i) in plan.shared.iter().enumerate() {
        identifier.insert(i, count(plan.first.len() + g)?);
        let k = position.get(&i).copied().ok_or_else(too_many)?;
        groups.push(small(len(&rest[k]))?);
    }
    let mut uses = vec![Vec::new()];
    for used in plan.uses.iter().skip(1) {
        // In object number order, as qpdf lists them.
        let mut ids: Vec<(u32, u32)> = used.iter().filter_map(|i| identifier.get(i).map(|id| (number[*i], *id))).collect();
        ids.sort_unstable();
        uses.push(ids.into_iter().map(|(_, id)| id).collect());
    }
    let shared_at = match plan.shared.first() {
        Some(&i) => {
            let k = position.get(&i).copied().ok_or_else(too_many)?;
            Some((number[i], small(rest_at[k])?))
        }
        None => None,
    };
    // Where each part 6 and main-section object is, and how long, for the outline group.
    let placed: HashMap<usize, (u64, u64)> = plan
        .first
        .iter()
        .zip(first_at.iter().zip(&first))
        .chain(main.iter().zip(rest_at.iter().zip(&rest)))
        .map(|(&i, (&at, b))| (i, (at, len(b))))
        .collect();
    let outlines = match plan.outlines.first().and_then(|i| placed.get(i).map(|p| (number[*i], p.0))) {
        Some((num, at)) => {
            let length = plan.outlines.iter().filter_map(|i| placed.get(i)).map(|p| p.1).sum();
            Some([num, small(at)?, count(plan.outlines.len())?, small(length)?])
        }
        None => None,
    };
    let hints = Hints {
        objects,
        lengths,
        uses,
        first_page_at: small(hint_at)?,
        groups: &groups,
        first_page_groups: count(plan.first.len())?,
        shared_at,
        outlines,
    };
    let (data, shared_table, outline_table) = hints.encode()?;
    let mut dict = Dict::new();
    dict.set(b"S".to_vec(), Object::Int(shared_table as i64));
    if let Some(at) = outline_table {
        dict.set(b"O".to_vec(), Object::Int(at as i64));
    }
    let stream = Object::Stream(Stream::flate(dict, &data));
    let mut hint_body = Vec::new();
    write_indirect(hint, 0, &prepared(&reader.document, u32::MAX, hint, 0, &stream), &mut hint_body);

    // Real positions: everything after the hint stream moves by its length.
    let shift = len(&hint_body);
    let main_xref = main_at + shift;
    let main_header = format!("0 {lin}");
    let total = main_xref + len(b"xref\n") + len(main_header.as_bytes()) + 1 + 20 * u64::from(lin) + len(b"trailer\n");
    let mut main_trailer = Dict::new();
    main_trailer.set(b"Size".to_vec(), Object::Int(i64::from(lin)));
    if let Some(id) = trailer.get(b"ID") {
        main_trailer.set(b"ID".to_vec(), id.clone());
    }
    let mut tail = Vec::new();
    write_dict(&main_trailer, &mut tail);
    let _ = write!(tail, "\nstartxref\n{section_at}\n%%EOF\n");
    let total = total + len(&tail);
    let first_page = plan.first.first().map(|&i| number[i]).ok_or_else(|| refuse("the first page is missing"))?;
    let values = [
        small(total)?,
        small(hint_at)?,
        small(shift)?,
        first_page,
        small(first_end + shift)?,
        count(tree.pages.len())?,
        small(main_xref + len(b"xref\n") + len(main_header.as_bytes()))?,
    ];

    let mut out = Vec::with_capacity(usize::try_from(total).map_err(|_| refuse("the file would be too large"))?);
    out.extend_from_slice(&header);
    out.extend_from_slice(&parameters(lin, values));
    let mut offsets = vec![len(&header)];
    offsets.extend(&document_at);
    offsets.push(hint_at);
    offsets.extend(first_at.iter().map(|o| o + shift));
    out.extend_from_slice(&first_section(lin, &offsets, &trailer, main_xref));
    document.iter().for_each(|b| out.extend_from_slice(b));
    out.extend_from_slice(&hint_body);
    first.iter().chain(&rest).for_each(|b| out.extend_from_slice(b));
    let rows: BTreeMap<u32, (u64, u16, bool)> = (1u32..).zip(&rest_at).map(|(n, o)| (n, (o + shift, 0, true))).collect();
    write_xref_table(&mut out, &rows, true);
    out.extend_from_slice(b"trailer\n");
    out.extend_from_slice(&tail);
    if len(&out) != total {
        return Err(refuse("the layout came out a different length than planned; nothing was written"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_testkit::Pdf;

    use super::*;
    use crate::writer::write_full;

    fn opts() -> SaveOptions {
        SaveOptions { linearize: true, ..SaveOptions::default() }
    }

    /// Three pages: the first with its own font and a link to page 3; the other two sharing a
    /// font (with a descriptor and font file) and an image; the media box inherited.
    fn fixture() -> Document {
        let bytes = Pdf::new()
            .version("1.6")
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [14 0 R] >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources 13 0 R >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Resources 13 0 R >>")
            .stream("", "BT /F1 12 Tf 10 10 Td (one) Tj ET")
            .stream("", "BT /F2 12 Tf 10 10 Td (two) Tj ET /Im Do")
            .stream("", "BT /F2 12 Tf 10 10 Td (three) Tj ET")
            .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
            .obj("<< /Type /Font /Subtype /TrueType /BaseFont /Foo /FontDescriptor 11 0 R >>")
            .obj("<< /Type /FontDescriptor /FontName /Foo /FontFile2 12 0 R >>")
            .stream("", "font")
            .obj("<< /Font << /F2 10 0 R >> /XObject << /Im 15 0 R >> >>")
            .obj("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [5 0 R /Fit] >>")
            .stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "x")
            .build();
        Document::open(Arc::new(bytes)).unwrap()
    }

    fn parameters_of(bytes: &[u8]) -> Dict {
        let (_, o) = crate::parser::parse_indirect(bytes, 0, &|_| None).unwrap();
        o.as_dict().unwrap().clone()
    }

    #[test]
    fn bits_pack_most_significant_first_and_pad_each_array() {
        let mut b = Bits::default();
        b.put(0b1, 1);
        b.put(0b01, 2);
        b.put(0, 0);
        b.align();
        b.put(0xABCD, 16);
        b.put(0b1_0000_0001, 9);
        b.align();
        b.put(u32::MAX, 32);
        b.align();
        b.align();
        assert_eq!(b.bytes, [0b1010_0000, 0xAB, 0xCD, 0b1000_0000, 0b1000_0000, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!([bits(0), bits(1), bits(2), bits(255), bits(256), bits(u32::MAX)], [0, 1, 2, 8, 9, 32]);
    }

    #[test]
    fn the_layout_matches_its_parameters() {
        let doc = fixture();
        let out = write_full(&doc, &opts()).unwrap();
        assert!(is_linearized(&out));
        assert!(!is_linearized(&write_full(&doc, &SaveOptions::default()).unwrap()));
        assert_eq!(out, write_full(&doc, &opts()).unwrap(), "deterministic");
        let p = parameters_of(&out);
        assert_eq!(p.int(b"L"), Some(out.len() as i64));
        assert_eq!(p.int(b"N"), Some(3));
        // /T is the newline before the main section's first entry; /E is where page 2 starts,
        // and page 2's page object is object 1.
        let t = p.int(b"T").unwrap() as usize;
        assert_eq!(&out[t..t + 20], b"\n0000000000 65535 f ");
        let e = p.int(b"E").unwrap() as usize;
        assert!(out[e..].starts_with(b"1 0 obj"), "{}", String::from_utf8_lossy(&out[e..e + 20]));
        let o = p.int(b"O").unwrap();
        let h = p.get(b"H").unwrap().as_array().unwrap();
        let (h_at, h_len) = (h[0].as_int().unwrap() as usize, h[1].as_int().unwrap() as usize);
        assert!(out[h_at + h_len..].starts_with(format!("{o} 0 obj").as_bytes()), "the first page follows the hint stream");
        // The first-page section is one subsection, right after the dictionary and before the
        // first page; the main one comes last.
        let text = String::from_utf8_lossy(&out);
        let first = text.find("xref\n").unwrap();
        assert!(first < h_at && text.rfind("xref\n").unwrap() > e);
        let header = text[first + 5..].lines().next().unwrap();
        let entries: usize = header.split(' ').nth(1).unwrap().parse().unwrap();
        assert!(text[first + 5 + header.len() + 1 + 20 * entries..].starts_with("trailer\n"), "one subsection: {header}");

        let back = Document::open(Arc::new(out.clone())).unwrap();
        assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
        assert_eq!(back.revisions().len(), 2);
        assert_eq!(back.revision_ends().len(), 1);
        let size = back.trailer().int(b"Size").unwrap();
        assert_eq!(size as usize, back.object_numbers().into_iter().max().unwrap() as usize + 1);
        // Every page states its media box (Annex F: nothing inherited).
        for n in back.object_numbers() {
            let o = back.get(ObjRef::new(n, 0));
            if o.as_dict().and_then(|d| d.name(b"Type")) == Some(&b"Page"[..]) {
                assert!(o.as_dict().unwrap().contains(b"MediaBox"));
            }
        }
    }

    #[test]
    fn page_trees_it_cannot_lay_out_are_refused() {
        let no_pages = Document::new_empty();
        assert!(write_full(&no_pages, &opts()).is_err());
        let looped = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
            .obj("<< /Type /Pages /Kids [2 0 R] /Count 1 >>")
            .build();
        assert!(write_full(&Document::open(Arc::new(looped)).unwrap(), &opts()).is_err());
        let twice = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>")
            .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>")
            .build();
        let err = write_full(&Document::open(Arc::new(twice)).unwrap(), &opts()).unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");
        // The catalog as its own page tree, or as a kid, would be written twice under one number.
        for kids in ["/Pages 1 0 R", "/Pages 2 0 R"] {
            let catalog = Pdf::new()
                .obj(format!("<< /Type /Catalog {kids} >>"))
                .obj("<< /Type /Pages /Kids [3 0 R 1 0 R] /Count 2 >>")
                .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>")
                .build();
            let err = write_full(&Document::open(Arc::new(catalog)).unwrap(), &opts()).unwrap_err();
            assert!(err.to_string().contains("appears twice"), "{kids}: {err}");
        }
        // With every object number used, each new shared object would scan them all.
        let exhausted = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 10 10] >>")
            .obj("<< /Type /Page /Parent 2 0 R >>")
            .build();
        let exhausted = String::from_utf8(exhausted).unwrap().replace("/Size 4 ", "/Size 4294967295 ");
        let err = write_full(&Document::open(Arc::new(exhausted.into_bytes())).unwrap(), &opts()).unwrap_err();
        assert!(err.to_string().contains("no object numbers left"), "{err}");
    }

    /// What a node passes down reaches its pages: once, however large, and in place of a value
    /// the page states but that isn't there (a reference to nothing).
    #[test]
    fn inherited_values_reach_the_pages_once() {
        let rotate = "x".repeat(10_000);
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj(format!("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 10 10] /Rotate ({rotate}) /Resources << /Font << /F1 6 0 R >> >> >>"))
            .obj("<< /Type /Page /Parent 2 0 R /Resources 99 0 R >>")
            .obj("<< /Type /Page /Parent 2 0 R >>")
            .obj("<< /Type /Page /Parent 2 0 R >>")
            .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
            .build();
        let out = write_full(&Document::open(Arc::new(bytes)).unwrap(), &opts()).unwrap();
        let back = Document::open(Arc::new(out.clone())).unwrap();
        for n in back.object_numbers() {
            let page = back.get(ObjRef::new(n, 0));
            let Some(d) = page.as_dict().filter(|d| d.name(b"Type") == Some(&b"Page"[..])) else { continue };
            let resources = back.resolve(d.get(b"Resources").unwrap());
            assert!(resources.as_dict().is_some_and(|r| r.contains(b"Font")), "page {n}: {resources:?}");
        }
        assert_eq!(out.windows(rotate.len()).filter(|w| *w == rotate.as_bytes()).count(), 1, "one shared copy");
    }
}
