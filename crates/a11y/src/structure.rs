//! The structure tree (tags) as the checker needs it, and the rules that read it: alternate
//! text, tables, lists and headings (ISO 32000-2 §14.7–14.8).
//!
//! The tree is never built as a whole. [`walk`] reads it depth first with an explicit stack and
//! tells a [`Visitor`] about each element as it opens and as it closes. What the rules keep is
//! bounded by the elements still open, plus, for a table, the row spans that reach into later
//! rows. Findings are kept in document order, up to the report's limit.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

use crate::{Finding, MAX_FINDINGS, Rule};

/// Standard structure types (PDF 1.7 and 2.0); anything else goes through the role map.
const STANDARD: &[&[u8]] = &[
    b"Document",
    b"DocumentFragment",
    b"Part",
    b"Art",
    b"Sect",
    b"Div",
    b"BlockQuote",
    b"Caption",
    b"TOC",
    b"TOCI",
    b"Index",
    b"NonStruct",
    b"Private",
    b"Aside",
    b"Title",
    b"FENote",
    b"P",
    b"H",
    b"H1",
    b"H2",
    b"H3",
    b"H4",
    b"H5",
    b"H6",
    b"L",
    b"LI",
    b"Lbl",
    b"LBody",
    b"Table",
    b"TR",
    b"TH",
    b"TD",
    b"THead",
    b"TBody",
    b"TFoot",
    b"Span",
    b"Quote",
    b"Note",
    b"Reference",
    b"BibEntry",
    b"Code",
    b"Link",
    b"Annot",
    b"Ruby",
    b"RB",
    b"RT",
    b"RP",
    b"Warichu",
    b"WT",
    b"WP",
    b"Figure",
    b"Formula",
    b"Form",
    b"Sub",
    b"Em",
    b"Strong",
    b"Artifact",
];

pub(crate) fn is_standard(t: &[u8]) -> bool {
    STANDARD.contains(&t)
}

/// Structure nested deeper than this is not read (a guard against absurd trees).
const MAX_DEPTH: usize = 256;

/// A span (ColSpan or RowSpan) counts as at most this many columns or rows.
const MAX_SPAN: usize = 10_000;

/// The rules this module evaluates.
const RULES: [Rule; 13] = [
    Rule::FiguresAltText,
    Rule::NestedAltText,
    Rule::AltTextAssociated,
    Rule::AltTextHidesAnnotation,
    Rule::OtherElementsAltText,
    Rule::TableRows,
    Rule::TableCells,
    Rule::TableHeaders,
    Rule::TableRegularity,
    Rule::TableSummary,
    Rule::ListItems,
    Rule::LblLBody,
    Rule::HeadingNesting,
];

/// The standard types the rules tell apart. Any other type is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Figure,
    Formula,
    Table,
    /// THead, TBody or TFoot.
    Section,
    Tr,
    Th,
    Td,
    L,
    Li,
    Lbl,
    LBody,
    /// TOCI, BibEntry, Note or FENote: places where a label may sit outside a list item.
    NoteOrToc,
    /// H1 to H6.
    Heading(u8),
    Other,
}

fn kind_of(ty: &[u8]) -> Kind {
    match ty {
        b"Figure" => Kind::Figure,
        b"Formula" => Kind::Formula,
        b"Table" => Kind::Table,
        b"THead" | b"TBody" | b"TFoot" => Kind::Section,
        b"TR" => Kind::Tr,
        b"TH" => Kind::Th,
        b"TD" => Kind::Td,
        b"L" => Kind::L,
        b"LI" => Kind::Li,
        b"Lbl" => Kind::Lbl,
        b"LBody" => Kind::LBody,
        b"TOCI" | b"BibEntry" | b"Note" | b"FENote" => Kind::NoteOrToc,
        [b'H', d @ b'1'..=b'6'] => Kind::Heading(d - b'0'),
        _ => Kind::Other,
    }
}

/// An element while it is open. What only becomes known later (its content, what lies below it)
/// is filled in as the walk goes on.
#[derive(Debug)]
pub(crate) struct Frame {
    /// The standard type after the role map, or the type as written when it maps to nothing.
    ty: Vec<u8>,
    kind: Kind,
    /// Its position in document order: the number it has in the tree read in full.
    index: usize,
    /// Its page: its own /Pg, else its parent's.
    page: Option<usize>,
    /// Has /Alt or /ActualText.
    alt: bool,
    /// An element above it has alternate text.
    alt_above: bool,
    /// Has a /Summary attribute.
    summary: bool,
    /// ColSpan and RowSpan, each read as 1 to `MAX_SPAN`.
    col_span: usize,
    row_span: usize,
    /// Marked content, or an object reference, directly under it.
    content: bool,
    /// An annotation (OBJR) directly under it.
    annot: bool,
    /// Content, or an annotation, under it: set as its children close.
    sub_content: bool,
    sub_annot: bool,
}

impl Frame {
    /// Content in this element or anywhere below it. Complete once the element has closed.
    fn subtree_content(&self) -> bool {
        self.content || self.sub_content
    }

    /// An annotation in this element or anywhere below it. Complete once the element has closed.
    fn subtree_annot(&self) -> bool {
        self.annot || self.sub_annot
    }
}

/// Told about each element as it opens and as it closes.
pub(crate) trait Visitor {
    /// `e` opens. `ancestors` are the elements around it, outermost first.
    fn enter(&mut self, ancestors: &[Frame], e: &Frame);

    /// `e` closes, and everything below it has closed already. `ancestors` are the elements
    /// still open.
    fn exit(&mut self, ancestors: &[Frame], e: &Frame);
}

/// The dictionary (or stream) that `/StructTreeRoot` names in the catalog, when there is one.
fn struct_root(doc: &Document) -> Option<Arc<Object>> {
    let catalog = doc.get(doc.root()?);
    let root = doc.resolve(catalog.as_dict()?.get(b"StructTreeRoot")?);
    root.as_dict().is_some().then_some(root)
}

/// Whether the document has a structure tree.
pub(crate) fn has_struct_tree(doc: &Document) -> bool {
    struct_root(doc).is_some()
}

/// Walks the structure tree in document order, telling `v` about each element.
fn walk<V: Visitor>(doc: &Document, pages: &[Page], v: &mut V) {
    let Some(root) = struct_root(doc) else { return };
    let role_map = root.as_dict().and_then(|d| d.get(b"RoleMap")).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let page_index: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let mut w = Walker { doc, role_map, page_index, seen: HashSet::new(), frames: Vec::new(), tasks: Vec::new(), opened: 0 };
    if root.as_dict().is_some_and(|d| d.contains(b"K")) {
        w.tasks.push(Task::Node { loc: Loc { owner: root, path: vec![Step::K] }, depth: 0, page: None });
    }
    while let Some(task) = w.tasks.pop() {
        w.step(task, v);
    }
}

/// Where a value sits: the object `owner`, reached through `path`. A location holds its owner
/// (rather than a borrow), so the walk can keep positions on its stack without copying the tree.
#[derive(Clone)]
struct Loc {
    owner: Arc<Object>,
    path: Vec<Step>,
}

#[derive(Clone, Copy)]
enum Step {
    /// Item `n` of an array.
    Index(usize),
    /// The /K entry of a dictionary.
    K,
}

impl Loc {
    fn value(&self) -> Option<&Object> {
        self.path.iter().try_fold(&*self.owner, |cur, step| match step {
            Step::Index(n) => match cur {
                Object::Array(items) => items.get(*n),
                _ => None,
            },
            Step::K => cur.as_dict()?.get(b"K"),
        })
    }

    fn with(&self, step: Step) -> Loc {
        let mut path = self.path.clone();
        path.push(step);
        Loc { owner: self.owner.clone(), path }
    }
}

/// Work still to do. The walk is depth first: the last task pushed runs first.
enum Task {
    /// Read the value at `loc`, a kid at nesting `depth`, on page `page` if known.
    Node { loc: Loc, depth: usize, page: Option<usize> },
    /// Items `next..len` of the array at `loc`, each at nesting `depth`.
    Items { loc: Loc, next: usize, len: usize, depth: usize, page: Option<usize> },
    /// The innermost open element has no more kids.
    Close,
}

/// The state of a walk: what is open, what is still to read and what has been read.
struct Walker<'a> {
    doc: &'a Document,
    role_map: Dict,
    page_index: HashMap<ObjRef, usize>,
    /// Indirect kids already read. A kid reached again (a shared kid, or a cycle) is skipped, so a
    /// kid counts once, where it first appears.
    seen: HashSet<ObjRef>,
    /// The open elements, outermost first.
    frames: Vec<Frame>,
    tasks: Vec<Task>,
    /// Elements opened so far: the next element's position in document order.
    opened: usize,
}

impl Walker<'_> {
    fn step<V: Visitor>(&mut self, task: Task, v: &mut V) {
        match task {
            Task::Node { loc, depth, page } => self.node(&loc, depth, page, v),
            Task::Items { loc, next, len, depth, page } => {
                // The rest of the array waits below the item that is read now.
                if next + 1 < len {
                    self.tasks.push(Task::Items { loc: loc.clone(), next: next + 1, len, depth, page });
                }
                self.tasks.push(Task::Node { loc: loc.with(Step::Index(next)), depth, page });
            }
            Task::Close => self.close(v),
        }
    }

    /// A kid: arrays hand their items on one level deeper, a reference is read once, and only a
    /// dictionary can open an element.
    fn node<V: Visitor>(&mut self, loc: &Loc, depth: usize, page: Option<usize>, v: &mut V) {
        if depth > MAX_DEPTH {
            return;
        }
        let Some(value) = loc.value() else { return };
        match value {
            Object::Array(items) if !items.is_empty() => {
                self.tasks.push(Task::Items { loc: loc.clone(), next: 0, len: items.len(), depth: depth + 1, page });
            }
            Object::Int(_) => self.mark_content(),
            Object::Ref(r) => self.reference(*r, depth, page, v),
            Object::Dict(_) => self.dict(loc, depth, page, v),
            _ => {}
        }
    }

    /// An indirect kid, read unless it has been read before.
    fn reference<V: Visitor>(&mut self, r: ObjRef, depth: usize, page: Option<usize>, v: &mut V) {
        if !self.seen.insert(r) {
            return;
        }
        let target = self.doc.get(r);
        if target.as_dict().is_some() {
            self.dict(&Loc { owner: target, path: Vec::new() }, depth, page, v);
        }
    }

    /// A dictionary. Marked content and object references only mark the open element; a
    /// dictionary with /S is an element, and its /K kids are read inside it.
    fn dict<V: Visitor>(&mut self, loc: &Loc, depth: usize, page: Option<usize>, v: &mut V) {
        let Some(d) = loc.value().and_then(|o| o.as_dict()) else { return };
        match d.name(b"Type") {
            Some(b"MCR") => return self.mark_content(),
            Some(b"OBJR") => {
                self.mark_content();
                let annotation = d
                    .get(b"Obj")
                    .and_then(Object::as_ref)
                    .is_some_and(|o| self.doc.get(o).as_dict().is_some_and(|a| a.contains(b"Subtype") && a.contains(b"Rect")));
                if annotation && let Some(parent) = self.frames.last_mut() {
                    parent.annot = true;
                }
                return;
            }
            _ => {}
        }
        let Some(s) = d.name(b"S") else { return };
        let page = self.page_of(d).or(page);
        let alt = self.has_text(d, b"Alt") || self.has_text(d, b"ActualText");
        let ty = self.standard(s);
        let kind = kind_of(&ty);
        let attrs = if matches!(kind, Kind::Table | Kind::Td | Kind::Th) { self.attr_dicts(d) } else { Vec::new() };
        let frame = Frame {
            ty,
            kind,
            index: self.opened,
            page,
            alt,
            alt_above: self.frames.last().is_some_and(|p| p.alt || p.alt_above),
            summary: attr_int(&attrs, b"Summary").is_some(),
            col_span: span(&attrs, b"ColSpan"),
            row_span: span(&attrs, b"RowSpan"),
            content: false,
            annot: false,
            sub_content: false,
            sub_annot: false,
        };
        self.opened = self.opened.saturating_add(1);
        self.frames.push(frame);
        if let Some((e, ancestors)) = self.frames.split_last() {
            v.enter(ancestors, e);
        }
        self.tasks.push(Task::Close);
        if d.contains(b"K") {
            self.tasks.push(Task::Node { loc: loc.with(Step::K), depth: depth + 1, page });
        }
    }

    /// The innermost element has no more kids: report it, and pass its content up to its parent.
    fn close<V: Visitor>(&mut self, v: &mut V) {
        let Some(frame) = self.frames.pop() else { return };
        if let Some(parent) = self.frames.last_mut() {
            parent.sub_content |= frame.subtree_content();
            parent.sub_annot |= frame.subtree_annot();
        }
        v.exit(&self.frames, &frame);
    }

    /// Marked content, or an object reference, directly under the open element.
    fn mark_content(&mut self) {
        if let Some(parent) = self.frames.last_mut() {
            parent.content = true;
        }
    }

    fn page_of(&self, d: &Dict) -> Option<usize> {
        d.get(b"Pg").and_then(Object::as_ref).and_then(|r| self.page_index.get(&r).copied())
    }

    /// Whether `key` holds a string with some text in it.
    fn has_text(&self, d: &Dict, key: &[u8]) -> bool {
        d.get(key).map(|o| self.doc.resolve(o)).and_then(|o| o.as_string().map(|s| !s.to_text().trim().is_empty())).unwrap_or(false)
    }

    /// The standard type of `ty`, following the role map (a bounded number of steps).
    fn standard(&self, ty: &[u8]) -> Vec<u8> {
        let mut t = ty.to_vec();
        for _ in 0..12 {
            if STANDARD.contains(&t.as_slice()) {
                return t;
            }
            match self.role_map.get(&t).map(|o| self.doc.resolve(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec)) {
                Some(n) if n != t => t = n,
                _ => break,
            }
        }
        t
    }

    /// The attribute dictionaries of an element: the dictionaries its /A entry holds, one or an array.
    fn attr_dicts(&self, d: &Dict) -> Vec<Arc<Object>> {
        let Some(a) = d.get(b"A").map(|o| self.doc.resolve(o)) else { return Vec::new() };
        let held: Vec<Arc<Object>> = match &*a {
            Object::Array(items) => items.iter().map(|x| self.doc.resolve(x)).collect(),
            other => vec![self.doc.resolve(other)],
        };
        held.into_iter().filter(|x| x.as_dict().is_some()).collect()
    }
}

/// The integer value of `key` in the first attribute dictionary that has the key. `None` when no
/// attribute dictionary has it; `Some(None)` when its value is not an integer.
fn attr_int(attrs: &[Arc<Object>], key: &[u8]) -> Option<Option<i64>> {
    attrs.iter().find_map(|a| a.as_dict()?.get(key)).map(Object::as_int)
}

/// A span attribute: at least 1, at most `MAX_SPAN`; 1 when absent or not an integer.
fn span(attrs: &[Arc<Object>], key: &[u8]) -> usize {
    attr_int(attrs, key).flatten().filter(|n| *n >= 1).map_or(1, |n| n.min(MAX_SPAN as i64) as usize)
}

/// The depth of the table a row belongs to, given the row's ancestors: the table itself, or the
/// table around its THead, TBody or TFoot. `None` for any other row.
fn row_owner(path: &[Frame]) -> Option<usize> {
    let (parent, rest) = path.split_last()?;
    match parent.kind {
        Kind::Table => Some(rest.len()),
        Kind::Section => rest.last().filter(|grand| grand.kind == Kind::Table).map(|_| rest.len() - 1),
        _ => None,
    }
}

fn finding(e: &Frame, what: &str) -> Finding {
    let ty = String::from_utf8_lossy(&e.ty);
    let at = e.page.map(|p| format!(" on page {}", p + 1)).unwrap_or_default();
    Finding { page: e.page, message: format!("{ty} element{at}: {what}") }
}

/// Columns `start..end` that a row span covers, with the rows still to come after this one.
#[derive(Clone, Copy, Debug)]
struct Run {
    start: usize,
    end: usize,
    rows: usize,
}

/// The end of the run that covers `col`, if one does. `cursor` is the first run that can still
/// cover a column from `col` on, so callers must ask for columns in non-decreasing order.
fn covered_end(carry: &[Run], cursor: &mut usize, col: usize) -> Option<usize> {
    while carry.get(*cursor).is_some_and(|r| r.end <= col) {
        *cursor += 1;
    }
    carry.get(*cursor).filter(|r| r.start <= col).map(|r| r.end)
}

/// The runs that reach into the next row: `carry` one row shorter, merged with this row's `row`
/// runs. A column takes the longer of the two. Both inputs are sorted and disjoint.
fn carry_into_next_row(carry: &[Run], row: &[Run]) -> Vec<Run> {
    let mut points: Vec<usize> = carry.iter().chain(row).flat_map(|r| [r.start, r.end]).collect();
    points.sort_unstable();
    points.dedup();
    let mut out: Vec<Run> = Vec::new();
    let (mut a, mut b) = (0, 0);
    for pair in points.windows(2) {
        let &[from, to] = pair else { continue };
        while carry.get(a).is_some_and(|r| r.end <= from) {
            a += 1;
        }
        while row.get(b).is_some_and(|r| r.end <= from) {
            b += 1;
        }
        let before = carry.get(a).filter(|r| r.start <= from).map_or(0, |r| r.rows.saturating_sub(1));
        let here = row.get(b).filter(|r| r.start <= from).map_or(0, |r| r.rows);
        let rows = before.max(here);
        if rows == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == from && last.rows == rows => last.end = to,
            _ => out.push(Run { start: from, end: to, rows }),
        }
    }
    out
}

/// A table that is open: the widths of its rows, and the spans that reach into later rows.
struct Table {
    /// Its position among the open elements: the number of elements around it.
    depth: usize,
    /// A header cell (TH) is in one of its rows.
    has_th: bool,
    /// The narrowest and the widest row so far, in columns.
    range: Option<(usize, usize)>,
    /// Columns that spans from earlier rows still cover. Sorted and disjoint.
    carry: Vec<Run>,
    /// The current row: the column its next cell may start in, the cursor into `carry`, and the
    /// spans its cells start (these carry into the next row).
    col: usize,
    cursor: usize,
    starts: Vec<Run>,
}

impl Table {
    fn new(depth: usize) -> Table {
        Table { depth, has_th: false, range: None, carry: Vec::new(), col: 0, cursor: 0, starts: Vec::new() }
    }

    fn start_row(&mut self) {
        self.col = 0;
        self.cursor = 0;
        self.starts.clear();
    }

    /// Places a cell in the current row, at the first column that no earlier row covers.
    fn place(&mut self, col_span: usize, row_span: usize) {
        let mut start = self.col;
        while let Some(end) = covered_end(&self.carry, &mut self.cursor, start) {
            start = end;
        }
        let end = start.saturating_add(col_span);
        if row_span > 1 {
            self.starts.push(Run { start, end, rows: row_span.saturating_sub(1) });
        }
        self.col = end;
    }

    /// Ends the current row. Its width is where its cells end, plus any span from earlier rows
    /// that runs on past them.
    fn end_row(&mut self) {
        let mut width = self.col;
        while let Some(end) = covered_end(&self.carry, &mut self.cursor, width) {
            width = end;
        }
        self.range = Some(match self.range {
            None => (width, width),
            Some((narrow, wide)) => (narrow.min(width), wide.max(width)),
        });
        self.carry = carry_into_next_row(&self.carry, &self.starts);
        self.starts.clear();
    }
}

/// One rule's findings during the walk: how many there are, and the first `MAX_FINDINGS` of them
/// in document order. The text of the others is never built.
#[derive(Default)]
struct Found {
    total: usize,
    kept: BinaryHeap<Kept>,
}

/// A finding that is kept. The heap orders them by document position, so its top is the last one
/// to keep.
struct Kept {
    index: usize,
    finding: Finding,
}

impl PartialEq for Kept {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl Eq for Kept {}

impl PartialOrd for Kept {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Kept {
    fn cmp(&self, other: &Self) -> Ordering {
        self.index.cmp(&other.index)
    }
}

impl Found {
    /// Counts a finding at document position `index`. Its text is built only if it is kept.
    fn add(&mut self, index: usize, make: impl FnOnce() -> Finding) {
        self.total = self.total.saturating_add(1);
        let keep = self.kept.len() < MAX_FINDINGS || self.kept.peek().is_some_and(|last| index < last.index);
        if keep {
            if self.kept.len() >= MAX_FINDINGS {
                self.kept.pop();
            }
            self.kept.push(Kept { index, finding: make() });
        }
    }

    fn into_result(self) -> (Vec<Finding>, usize) {
        let findings = self.kept.into_sorted_vec().into_iter().map(|k| k.finding).collect();
        (findings, self.total)
    }
}

/// The structure rules during a walk.
struct Checks {
    found: BTreeMap<Rule, Found>,
    /// The level of the last heading read (0 before the first).
    heading: u8,
    /// The open tables, outermost first.
    tables: Vec<Table>,
}

impl Checks {
    fn add(&mut self, rule: Rule, index: usize, make: impl FnOnce() -> Finding) {
        if let Some(found) = self.found.get_mut(&rule) {
            found.add(index, make);
        }
    }

    /// The open table at `depth`, if it is the innermost open table.
    fn table_at(&mut self, depth: usize) -> Option<&mut Table> {
        self.tables.last_mut().filter(|t| t.depth == depth)
    }
}

impl Visitor for Checks {
    fn enter(&mut self, ancestors: &[Frame], e: &Frame) {
        let parent = ancestors.last().map(|p| p.kind);
        if e.alt && e.alt_above {
            self.add(Rule::NestedAltText, e.index, || finding(e, "its alternate text is inside another element's and will never be read"));
        }
        match e.kind {
            Kind::Figure if !e.alt && !e.alt_above => {
                self.add(Rule::FiguresAltText, e.index, || finding(e, "no alternate text"));
            }
            Kind::Formula if !e.alt && !e.alt_above => {
                self.add(Rule::OtherElementsAltText, e.index, || finding(e, "no alternate text"));
            }
            Kind::Table => {
                self.tables.push(Table::new(ancestors.len()));
                if !e.summary {
                    self.add(Rule::TableSummary, e.index, || finding(e, "no summary"));
                }
            }
            Kind::Tr => {
                if !matches!(parent, Some(Kind::Table | Kind::Section)) {
                    self.add(Rule::TableRows, e.index, || finding(e, "not in a Table, THead, TBody or TFoot"));
                }
                if let Some(table) = row_owner(ancestors).and_then(|depth| self.table_at(depth)) {
                    table.start_row();
                }
            }
            Kind::Th | Kind::Td => {
                let in_row = parent == Some(Kind::Tr);
                if !in_row {
                    self.add(Rule::TableCells, e.index, || finding(e, "not in a TR"));
                }
                if in_row
                    && let Some((_, row_path)) = ancestors.split_last()
                    && let Some(table) = row_owner(row_path).and_then(|depth| self.table_at(depth))
                {
                    table.place(e.col_span, e.row_span);
                    table.has_th |= e.kind == Kind::Th;
                }
            }
            Kind::Li if parent != Some(Kind::L) => {
                self.add(Rule::ListItems, e.index, || finding(e, "not in an L (list)"));
            }
            Kind::Lbl | Kind::LBody if !matches!(parent, Some(Kind::Li | Kind::NoteOrToc)) => {
                self.add(Rule::LblLBody, e.index, || finding(e, "not in an LI (list item)"));
            }
            Kind::Heading(level) => {
                let prev = self.heading;
                if level > prev + 1 {
                    self.add(Rule::HeadingNesting, e.index, || {
                        let what = if prev == 0 { format!("the first heading is an H{level}") } else { format!("H{level} follows an H{prev}") };
                        finding(e, &format!("{what}; headings should not skip levels"))
                    });
                }
                self.heading = level;
            }
            _ => {}
        }
    }

    fn exit(&mut self, ancestors: &[Frame], e: &Frame) {
        match e.kind {
            Kind::Table => {
                let is_ours = self.tables.last().is_some_and(|t| t.depth == ancestors.len());
                let table = if is_ours { self.tables.pop() } else { None };
                if let Some(table) = table {
                    if !table.has_th {
                        self.add(Rule::TableHeaders, e.index, || finding(e, "no header cells (TH)"));
                    }
                    if let Some((narrow, wide)) = table.range.filter(|(lo, hi)| lo != hi) {
                        self.add(Rule::TableRegularity, e.index, || finding(e, &format!("rows have between {narrow} and {wide} columns")));
                    }
                }
            }
            Kind::Tr => {
                if let Some(table) = row_owner(ancestors).and_then(|depth| self.table_at(depth)) {
                    table.end_row();
                }
            }
            _ => {}
        }
        if e.alt {
            if !e.subtree_content() {
                self.add(Rule::AltTextAssociated, e.index, || finding(e, "alternate text on an element with no page content"));
            }
            if e.subtree_annot() {
                self.add(Rule::AltTextHidesAnnotation, e.index, || finding(e, "alternate text hides an annotation inside it"));
            }
        }
    }
}

/// Evaluates the structure rules in `selected` in one walk of the tree. Each rule's result is its
/// first `MAX_FINDINGS` findings in document order, and the total number of findings.
pub(crate) fn run(doc: &Document, pages: &[Page], selected: &BTreeSet<Rule>) -> BTreeMap<Rule, (Vec<Finding>, usize)> {
    let found: BTreeMap<Rule, Found> = RULES.into_iter().filter(|r| selected.contains(r)).map(|r| (r, Found::default())).collect();
    if found.is_empty() {
        return BTreeMap::new();
    }
    let mut checks = Checks { found, heading: 0, tables: Vec::new() };
    walk(doc, pages, &mut checks);
    checks.found.into_iter().map(|(rule, found)| (rule, found.into_result())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The column-by-column computation that the check used before it streamed: each row's width,
    /// as the (narrowest, widest) range over the rows. Rows are lists of (ColSpan, RowSpan).
    fn dense_range(rows: &[Vec<(usize, usize)>]) -> Option<(usize, usize)> {
        let mut carry: Vec<usize> = Vec::new();
        let mut widths = Vec::new();
        for row in rows {
            let mut col = 0;
            let mut next: Vec<usize> = carry.iter().map(|n| n.saturating_sub(1)).collect();
            for &(cs, rs) in row {
                while col < carry.len() && carry[col] > 0 {
                    col += 1;
                }
                if next.len() < col + cs {
                    next.resize(col + cs, 0);
                }
                for n in next.iter_mut().skip(col).take(cs) {
                    *n = (*n).max(rs - 1);
                }
                col += cs;
            }
            while col < carry.len() && carry[col] > 0 {
                col += 1;
            }
            widths.push(col);
            carry = next;
        }
        Some((*widths.iter().min()?, *widths.iter().max()?))
    }

    /// A small deterministic generator, so the random tables are the same on every run.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as usize) % n
        }
    }

    #[test]
    fn spans_carry_into_later_rows_like_the_dense_computation() {
        let mut rng = Lcg(7);
        for _ in 0..4_000 {
            let mut rows = Vec::new();
            for _ in 0..1 + rng.below(6) {
                let mut row = Vec::new();
                for _ in 0..rng.below(6) {
                    row.push((1 + rng.below(12), 1 + rng.below(5)));
                }
                rows.push(row);
            }
            let mut table = Table::new(0);
            for row in &rows {
                table.start_row();
                for &(cs, rs) in row {
                    table.place(cs, rs);
                }
                table.end_row();
            }
            assert_eq!(table.range, dense_range(&rows), "{rows:?}");
        }
    }
}
