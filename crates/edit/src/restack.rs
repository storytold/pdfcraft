//! Restack a page object: bring it to the front, raise or lower it one level, or send it to the
//! back.
//!
//! A PDF paints its content in stream order, so an object's level is where its own drawing
//! operators sit. Restacking takes the operators of one object — a paragraph, an image or Form,
//! an inline vector graphic, or an added item — out of their place and draws them again at the
//! new level:
//!
//! * a paragraph, image or graphic painted inside a `q … Q` scope (or the `BT … ET` text object
//!   of a paragraph) that holds nothing else moves as a whole, so its clip box and nested state
//!   travel with it;
//! * content painted in a scope shared with other objects moves its own operators, wrapped in
//!   `q`, the paint state it inherited and a `cm` that restores its page-space transform at the
//!   new level, then `Q`, so nothing else on the page changes;
//! * an added item is one content stream with its own `/PCAdded` parameters, so its stream entry
//!   moves whole (with `q cm … Q` around it) and stays editable.
//!
//! The page's other content is copied byte for byte, in the same order: only the moved drawing
//! changes place. Every edit is refused as a whole when the object shares its drawing with
//! content that would have to move with it, or when the level can't be expressed (for example
//! text whose `BT … ET` holds another paragraph).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use pdfcraft_content::{Matrix, Op, Pieces, num, serialize_ops};
use pdfcraft_cos::{Dict, Document, Object, Stream};

use crate::EditError;
use crate::objects::{ObjectKind, ObjectTarget};
use crate::text::{content_streams, selectable_block_ops, selectable_blocks};

/// The most operators a page may have for restacking.
const MAX_OPERATIONS: usize = 100_000;
/// The most scopes one page may nest.
const MAX_SCOPES: usize = 4_096;
/// The most operator ranges one object may move as.
const MAX_RANGES: usize = 1_000;
const MAX_COORDINATE: f64 = 1e12;

/// Where an object goes in the page's painting order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restack {
    /// Above every other object on the page.
    Front,
    /// One level up, above the object painted just after it.
    Forward,
    /// One level down, below the object painted just before it.
    Backward,
    /// Below every other object on the page (but above a background or watermark already there).
    Back,
}

/// What one object's level consists of.
#[derive(Clone, Debug)]
enum Unit {
    /// Balanced operator ranges in the page's streams joined, each one a whole `q … Q` scope,
    /// `BT … ET` text object or self-contained paint.
    Ops(Vec<Range<usize>>),
    /// One `/Contents` entry drawn by its own stream (an added item).
    Entry(usize),
    /// The object's drawing is the whole page: there is no other level to move it to.
    Only,
}

#[derive(Clone, Debug)]
struct Level {
    target: ObjectTarget,
    /// The first and last operator (end exclusive) the level occupies.
    first: usize,
    end: usize,
    unit: Unit,
}

/// Whether `op` puts paint on the page, as opposed to setting state or building a path.
fn is_paint(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"Do" | b"sh" | b"BI" | b"Tj" | b"TJ" | b"'" | b"\"")
}

/// A balanced enclosing scope: a `q … Q` or a `BT … ET` text object.
#[derive(Clone, Debug)]
struct Scope {
    start: usize,
    end: usize,
    parent: Option<usize>,
    closed: bool,
}

/// The scopes of a page, and the innermost one holding each operator.
struct Scopes {
    list: Vec<Scope>,
    of: Vec<Option<usize>>,
}

impl Scopes {
    fn new(ops: &[Op]) -> Result<Self, EditError> {
        let mut list: Vec<Scope> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut of = vec![None; ops.len()];
        for (i, op) in ops.iter().enumerate() {
            if op.is("q") || op.is("BT") {
                if list.len() >= MAX_SCOPES {
                    return Err(EditError::Invalid("the page content is nested too deeply to restack an object".into()));
                }
                let parent = stack.last().copied();
                let index = list.len();
                list.push(Scope { start: i, end: ops.len(), parent, closed: false });
                stack.push(index);
                of[i] = Some(index);
            } else if op.is("Q") || op.is("ET") {
                let open = stack.pop();
                if let Some(index) = open
                    && let Some(scope) = list.get_mut(index)
                {
                    scope.end = i + 1;
                    scope.closed = true;
                }
                of[i] = open;
            } else {
                of[i] = stack.last().copied();
            }
        }
        if list.len() > MAX_SCOPES {
            return Err(EditError::Invalid("the page content is nested too deeply to restack an object".into()));
        }
        Ok(Self { list, of })
    }

    /// The innermost scope holding all of `min..=max`.
    fn common(&self, min: usize, max: usize) -> Option<usize> {
        let mut s = self.of.get(min).copied().flatten();
        while let Some(i) = s {
            let scope = &self.list[i];
            if scope.start <= min && scope.end > max {
                return Some(i);
            }
            s = scope.parent;
        }
        None
    }

    /// The nearest enclosing `BT … ET` scope of operator `i`.
    fn text_scope(&self, ops: &[Op], i: usize) -> Option<usize> {
        let mut s = self.of.get(i).copied().flatten();
        while let Some(id) = s {
            if ops.get(self.list[id].start).is_some_and(|op| op.is("BT")) {
                return Some(id);
            }
            s = self.list[id].parent;
        }
        None
    }
}

/// The transform in effect before operator `at`, from the stream's `q`, `Q` and `cm`.
fn ctm_at(ops: &[Op], at: usize) -> Option<Matrix> {
    let mut stack = Vec::new();
    let mut matrix = Matrix::IDENTITY;
    for op in ops.iter().take(at) {
        match op.op.as_slice() {
            b"q" => stack.push(matrix),
            b"Q" => matrix = stack.pop().unwrap_or(matrix),
            b"cm" => {
                let m = op.nums::<6>()?;
                if !m.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) {
                    return None;
                }
                matrix = Matrix(m).then(&matrix);
            }
            _ => {}
        }
    }
    Some(matrix)
}

fn identity(m: Matrix) -> bool {
    let i = Matrix::IDENTITY.0;
    m.0.iter().zip(i).all(|(a, b)| (a - b).abs() < 1e-9)
}

/// The paint state an object inherits where it is drawn, restated before it at its new level so
/// moving it can't change its colour, line, text state or transparency.
#[derive(Clone, Default)]
struct PaintState {
    fill: Vec<Op>,
    stroke: Vec<Op>,
    line: BTreeMap<&'static str, Op>,
    text: BTreeMap<&'static str, Op>,
    gs: Vec<Op>,
}

impl PaintState {
    fn at(ops: &[Op], at: usize) -> Self {
        let mut stack: Vec<Self> = Vec::new();
        let mut state = Self::default();
        for op in &ops[..at.min(ops.len())] {
            let keyword = match op.op.as_slice() {
                b"q" => {
                    stack.push(state.clone());
                    continue;
                }
                b"Q" => {
                    state = stack.pop().unwrap_or_default();
                    continue;
                }
                k => k,
            };
            if matches!(keyword, b"g" | b"rg" | b"k" | b"cs") {
                state.fill = vec![op.clone()];
            } else if matches!(keyword, b"sc" | b"scn") {
                if !state.fill.last().is_some_and(|o| o.is("cs")) {
                    state.fill.clear();
                }
                state.fill.push(op.clone());
            } else if matches!(keyword, b"G" | b"RG" | b"K" | b"CS") {
                state.stroke = vec![op.clone()];
            } else if matches!(keyword, b"SC" | b"SCN") {
                if !state.stroke.last().is_some_and(|o| o.is("CS")) {
                    state.stroke.clear();
                }
                state.stroke.push(op.clone());
            } else if let Some(name) = line_name(keyword) {
                state.line.insert(name, op.clone());
            } else if let Some(name) = text_name(keyword) {
                state.text.insert(name, op.clone());
            } else if keyword == b"gs" {
                state.gs.push(op.clone());
            }
        }
        state
    }

    /// The operators that restate it, defaults included so nothing leaks in from the level it
    /// moves to.
    fn ops(&self) -> Vec<Op> {
        let mut ops = self.gs.clone();
        if self.fill.is_empty() {
            ops.push(Op::new("g", vec![Object::Int(0)]));
        } else {
            ops.extend(self.fill.clone());
        }
        if self.stroke.is_empty() {
            ops.push(Op::new("G", vec![Object::Int(0)]));
        } else {
            ops.extend(self.stroke.clone());
        }
        for (name, default) in [
            ("w", Op::new("w", vec![Object::Int(1)])),
            ("J", Op::new("J", vec![Object::Int(0)])),
            ("j", Op::new("j", vec![Object::Int(0)])),
            ("M", Op::new("M", vec![Object::Int(10)])),
            ("d", Op::new("d", vec![Object::Array(Vec::new()), Object::Int(0)])),
            ("ri", Op::new("ri", vec![Object::name("RelativeColorimetric")])),
        ] {
            ops.push(self.line.get(name).cloned().unwrap_or(default));
        }
        for (name, default) in [
            ("Tc", Op::new("Tc", vec![Object::Int(0)])),
            ("Tw", Op::new("Tw", vec![Object::Int(0)])),
            ("Tz", Op::new("Tz", vec![Object::Int(100)])),
            ("TL", Op::new("TL", vec![Object::Int(0)])),
            ("Ts", Op::new("Ts", vec![Object::Int(0)])),
            ("Tr", Op::new("Tr", vec![Object::Int(0)])),
        ] {
            ops.push(self.text.get(name).cloned().unwrap_or(default));
        }
        if let Some(font) = self.text.get("Tf") {
            ops.push(font.clone());
        }
        ops
    }
}

fn line_name(keyword: &[u8]) -> Option<&'static str> {
    match keyword {
        b"w" => Some("w"),
        b"J" => Some("J"),
        b"j" => Some("j"),
        b"M" => Some("M"),
        b"d" => Some("d"),
        b"ri" => Some("ri"),
        _ => None,
    }
}

fn text_name(keyword: &[u8]) -> Option<&'static str> {
    match keyword {
        b"Tf" => Some("Tf"),
        b"Tc" => Some("Tc"),
        b"Tw" => Some("Tw"),
        b"Tz" => Some("Tz"),
        b"TL" => Some("TL"),
        b"Ts" => Some("Ts"),
        b"Tr" => Some("Tr"),
        _ => None,
    }
}

/// A page's content, ready to be rebuilt around a moved level.
struct Sheet {
    page: pdfcraft_model::Page,
    streams: Vec<(Object, Vec<u8>)>,
    pieces: Pieces,
    ops: Vec<Op>,
    entries: Vec<Object>,
    /// Per entry: the content stream must stay whole (`/PCMark`, `/PCAdded`).
    tagged: Vec<bool>,
    /// Per entry: a plain content stream that may be split and rewritten.
    base: Vec<bool>,
    /// Byte offset of each entry in the joined data.
    starts: Vec<usize>,
    /// The entry each operator comes from.
    op_entry: Vec<usize>,
    /// The first operator of each entry, and `ops.len()` past the last.
    entry_ops: Vec<usize>,
    scopes: Scopes,
}

impl Sheet {
    fn open(doc: &Document, index: usize) -> Result<Self, EditError> {
        let page = pdfcraft_model::pages(doc).into_iter().nth(index).ok_or(EditError::NoSuchPage(index))?;
        let streams = content_streams(doc, &page.dict)?;
        if streams.len() > MAX_OPERATIONS {
            return Err(EditError::Invalid("this page has too many content streams to restack an object".into()));
        }
        let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
        let ops = pieces.parse().ops;
        if ops.len() > MAX_OPERATIONS {
            return Err(EditError::Invalid("this page has too many operators to restack an object".into()));
        }
        let entries: Vec<Object> = match page.dict.get(b"Contents") {
            None => Vec::new(),
            Some(c) => match &*doc.resolve(c) {
                Object::Array(a) => a.clone(),
                _ => vec![c.clone()],
            },
        };
        if entries.len() != streams.len() {
            return Err(EditError::Invalid("the page's content streams could not be read".into()));
        }
        let mut starts = Vec::with_capacity(streams.len());
        let mut at = 0usize;
        for (_, bytes) in &streams {
            starts.push(at);
            at = at.saturating_add(bytes.len()).saturating_add(1);
        }
        let mut tagged = Vec::with_capacity(entries.len());
        let mut base = Vec::with_capacity(entries.len());
        for entry in &entries {
            let whole = crate::place::stream_tag(doc, entry).is_some() || doc.resolve(entry).as_dict().is_some_and(|d| d.get(b"PCAdded").is_some());
            tagged.push(whole);
            base.push(!whole);
        }
        let mut op_entry = Vec::with_capacity(ops.len());
        for op in &ops {
            op_entry.push(pieces.pieces_of(op).0.min(entries.len().saturating_sub(1)));
        }
        let mut entry_ops = vec![ops.len(); entries.len() + 1];
        for (i, e) in op_entry.iter().enumerate() {
            if entry_ops[*e] == ops.len() {
                entry_ops[*e] = i;
            }
        }
        for e in (0..entries.len()).rev() {
            if entry_ops[e] == ops.len() {
                entry_ops[e] = entry_ops[e + 1];
            }
        }
        let scopes = Scopes::new(&ops)?;
        Ok(Self { page, streams, pieces, ops, entries, tagged, base, starts, op_entry, entry_ops, scopes })
    }

    fn data(&self) -> &[u8] {
        self.pieces.data()
    }

    fn entry_range(&self, e: usize) -> Range<usize> {
        let start = self.starts.get(e).copied().unwrap_or(0);
        start..start.saturating_add(self.streams.get(e).map_or(0, |(_, b)| b.len()))
    }

    /// Every scope active at the position before operator `pos`, outermost first.
    fn active(&self, pos: usize) -> Vec<usize> {
        let mut list: Vec<usize> = self
            .scopes
            .list
            .iter()
            .enumerate()
            .filter(|(_, s)| s.start < pos && (pos < s.end || (pos == s.end && !s.closed)))
            .map(|(i, _)| i)
            .collect();
        list.sort_by_key(|i| self.scopes.list[*i].start);
        list
    }

    /// Whether scope `id` clips its content before `pos`.
    fn clipped(&self, id: usize, pos: usize) -> bool {
        let scope = &self.scopes.list[id];
        (scope.start..pos.min(scope.end))
            .any(|i| self.ops.get(i).is_some_and(|op| op.is("W") || op.is("W*")) && self.scopes.of.get(i) == Some(&Some(id)))
    }

    /// Whether the only paint inside `scope` is `owned`.
    fn clean(&self, scope: usize, owned: &BTreeSet<usize>) -> bool {
        let s = &self.scopes.list[scope];
        (s.start..s.end.min(self.ops.len())).all(|i| !self.ops.get(i).is_some_and(is_paint) || owned.contains(&i))
    }

    fn only_paint(&self, owned: &BTreeSet<usize>) -> bool {
        self.ops.iter().enumerate().all(|(i, op)| !is_paint(op) || owned.contains(&i))
    }

    /// The page-space transform a unit must see at the insertion point `pos`, chosen so that no
    /// clip the unit did not inherit where it was drawn applies to it.
    fn place_after(&self, mut pos: usize, keep: &BTreeSet<usize>) -> Result<usize, EditError> {
        for _ in 0..=MAX_SCOPES {
            let culprit = self.active(pos).into_iter().find(|id| !keep.contains(id) && self.clipped(*id, pos));
            let Some(id) = culprit else { return Ok(pos) };
            let end = self.scopes.list[id].end;
            if end <= pos {
                return Err(EditError::Invalid("the page's content is nested too deeply to restack an object".into()));
            }
            pos = end;
        }
        Err(EditError::Invalid("the page's content is nested too deeply to restack an object".into()))
    }

    /// [`Sheet::place_after`], for an object placed before the content at `pos`.
    fn place_before(&self, mut pos: usize, keep: &BTreeSet<usize>) -> Result<usize, EditError> {
        for _ in 0..=MAX_SCOPES {
            let culprit = self.active(pos).into_iter().find(|id| !keep.contains(id) && self.clipped(*id, pos));
            let Some(id) = culprit else { return Ok(pos) };
            let start = self.scopes.list[id].start;
            if start >= pos {
                return Err(EditError::Invalid("the page's content is nested too deeply to restack an object".into()));
            }
            pos = start;
        }
        Err(EditError::Invalid("the page's content is nested too deeply to restack an object".into()))
    }

    /// The clean scope holding every operator in `owned`, `Only` when that is the whole page,
    /// or `None` when other content shares the narrowest scope.
    fn isolate(&self, owned: &BTreeSet<usize>) -> Option<Result<Unit, ()>> {
        let min = *owned.iter().next()?;
        let max = *owned.iter().next_back()?;
        let Some(lca) = self.scopes.common(min, max) else {
            return Some(if self.only_paint(owned) { Ok(Unit::Only) } else { Err(()) });
        };
        if !self.clean(lca, owned) {
            return Some(Err(()));
        }
        let mut best = lca;
        let mut next = self.scopes.list[lca].parent;
        while let Some(parent) = next {
            if self.clean(parent, owned) {
                best = parent;
                next = self.scopes.list[parent].parent;
            } else {
                break;
            }
        }
        let scope = &self.scopes.list[best];
        Some(Ok(Unit::Ops(std::iter::once(scope.start..scope.end).collect())))
    }

    /// The operator bounds of `unit` in the page's content. A unit that takes a whole scope (its
    /// `BT … ET`, or a `q … Q` around artwork) starts and ends with that scope, not with its
    /// paint operators: inserting between the paint operators would land inside the object.
    fn bounds(&self, unit: &Unit) -> Option<(usize, usize)> {
        match unit {
            Unit::Entry(entry) => Some((*self.entry_ops.get(*entry)?, self.entry_ops.get(entry + 1).copied().unwrap_or(self.ops.len()))),
            Unit::Ops(ranges) => Some((ranges.first()?.start, ranges.last()?.end)),
            Unit::Only => None,
        }
    }

    fn levels(&self, doc: &Document, index: usize) -> Result<Vec<Level>, EditError> {
        let added: Vec<_> = crate::list_added(doc).into_iter().filter(|a| a.page == index).collect();
        let excluded: BTreeSet<_> = added.iter().map(|a| a.obj).collect();
        let mut levels = Vec::new();
        for (i, a) in added.into_iter().enumerate() {
            let entry = self
                .entries
                .iter()
                .position(|e| e.as_ref() == Some(a.obj))
                .ok_or_else(|| EditError::Invalid("an added item is not one of the page's content streams".into()))?;
            let first = self.entry_ops[entry];
            let end = self.entry_ops.get(entry + 1).copied().unwrap_or(self.ops.len());
            levels.push(Level { target: ObjectTarget { kind: ObjectKind::Added, index: i }, first, end, unit: Unit::Entry(entry) });
        }
        let blocks = selectable_blocks(doc, index, &excluded)?;
        let block_ops = selectable_block_ops(doc, index, &excluded)?;
        if blocks.len() != block_ops.len() {
            return Err(EditError::Invalid("the page's paragraphs changed while reading them".into()));
        }
        for (i, _) in blocks.iter().enumerate() {
            let owned: BTreeSet<usize> = block_ops.get(i).into_iter().flatten().copied().collect();
            let (first, end, unit) = match self.isolate(&owned) {
                Some(Ok(unit)) => {
                    let (first, end) = self
                        .bounds(&unit)
                        .unwrap_or_else(|| (owned.iter().next().copied().unwrap_or(0), owned.iter().next_back().map_or(0, |m| m + 1)));
                    (first, end, unit)
                }
                Some(Err(())) => {
                    let ranges = self.text_ranges(&owned, block_ops.get(i).map(Vec::as_slice).unwrap_or(&[]))?;
                    (ranges.first().map_or(0, |r| r.start), ranges.last().map_or(0, |r| r.end), Unit::Ops(ranges))
                }
                None => {
                    return Err(EditError::Invalid("the paragraph has no text to restack".into()));
                }
            };
            levels.push(Level { target: ObjectTarget { kind: ObjectKind::Text, index: i }, first, end, unit });
        }
        for (i, image) in crate::images::selectable_images(doc, index, &excluded)?.into_iter().enumerate() {
            let op = image.1.paint_op();
            let owned = BTreeSet::from([op]);
            let (unit, first, end) = match self.isolate(&owned) {
                Some(Ok(unit)) => {
                    let (first, end) = self.bounds(&unit).unwrap_or((op, op.saturating_add(1)));
                    (unit, first, end)
                }
                Some(Err(())) | None => (Unit::Ops(std::iter::once(op..op.saturating_add(1)).collect()), op, op.saturating_add(1)),
            };
            levels.push(Level { target: ObjectTarget { kind: ObjectKind::Image, index: i }, first, end, unit });
        }
        for (i, graphic) in crate::graphics::page_graphics(doc, index)?.into_iter().enumerate() {
            let owned: BTreeSet<usize> = graphic.run_ranges().flat_map(|(a, b)| a..b).collect();
            let (unit, first, end) = match self.isolate(&owned) {
                Some(Ok(unit)) => {
                    let (first, end) = self
                        .bounds(&unit)
                        .unwrap_or_else(|| (owned.iter().next().copied().unwrap_or(0), owned.iter().next_back().map_or(0, |m| m + 1)));
                    (unit, first, end)
                }
                Some(Err(())) | None => {
                    let ranges: Vec<_> = graphic.run_ranges().map(|(a, b)| a..b).collect();
                    if ranges.len() > MAX_RANGES {
                        return Err(EditError::Invalid("a graphic has too many separate parts to restack".into()));
                    }
                    let first = ranges.first().map_or(0, |r| r.start);
                    let end = ranges.last().map_or(0, |r| r.end);
                    (Unit::Ops(ranges), first, end)
                }
            };
            levels.push(Level { target: ObjectTarget { kind: ObjectKind::Graphic, index: i }, first, end, unit });
        }
        levels.sort_by_key(|l| (l.first, l.end));
        Ok(levels)
    }

    /// The `BT … ET` scopes of a paragraph whose wider scope holds other content, each one
    /// holding only this paragraph's text.
    fn text_ranges(&self, owned: &BTreeSet<usize>, show_ops: &[usize]) -> Result<Vec<Range<usize>>, EditError> {
        let mut scopes = BTreeSet::new();
        for op in show_ops {
            let scope = self
                .scopes
                .text_scope(&self.ops, *op)
                .ok_or_else(|| EditError::Invalid("this paragraph's text has no text object to move".into()))?;
            scopes.insert(scope);
        }
        if scopes.is_empty() {
            return Err(EditError::Invalid("the paragraph has no text to restack".into()));
        }
        let mut ranges = Vec::with_capacity(scopes.len());
        for id in scopes {
            if !self.clean(id, owned) {
                return Err(EditError::Invalid("this paragraph shares its text object with other text; it can't be moved above or below it".into()));
            }
            let scope = &self.scopes.list[id];
            ranges.push(scope.start..scope.end);
        }
        Ok(ranges)
    }
}

/// A place in the rebuilt content.
#[derive(Clone, Debug)]
enum Place {
    /// Between whole `/Contents` entries, before entry `e`.
    Before(usize),
    /// At a byte offset in the page's joined content.
    Offset(usize),
}

#[derive(Clone, Debug)]
enum Part {
    Bytes(Vec<u8>),
    Entry(usize),
}

impl Sheet {
    /// The operator ranges to remove, as joined byte ranges (merged, in order).
    fn removals(&self, units: &[&Unit]) -> Result<Vec<Range<usize>>, EditError> {
        let mut bytes = Vec::new();
        for unit in units {
            let Unit::Ops(ranges) = unit else { continue };
            for range in ranges {
                let first = self.ops.get(range.start).ok_or_else(|| EditError::Invalid("the object's drawing is missing".into()))?;
                let last = self.ops.get(range.end.saturating_sub(1)).ok_or_else(|| EditError::Invalid("the object's drawing is missing".into()))?;
                if range.end <= range.start || first.span.start >= last.span.end {
                    return Err(EditError::Invalid("the object's drawing is missing".into()));
                }
                bytes.push(first.span.start..last.span.end);
            }
        }
        bytes.sort_by_key(|r| (r.start, r.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(bytes.len());
        for range in bytes {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        let data_len = self.data().len();
        for range in &merged {
            if range.start >= range.end || range.end > data_len {
                return Err(EditError::Invalid("the object's drawing is missing".into()));
            }
            let overlaps_tagged = (0..self.entries.len())
                .filter(|e| self.tagged[*e])
                .any(|e| range.start < self.entry_range(e).end && self.entry_range(e).start < range.end);
            if overlaps_tagged {
                return Err(EditError::Invalid("the object's drawing shares a content stream with content that can't move with it".into()));
            }
        }
        Ok(merged)
    }

    /// One moved range, drawn at `insert` (the transform at its new level).
    fn wrap(&self, range: &Range<usize>, insert: Matrix) -> Result<Vec<u8>, EditError> {
        let original =
            ctm_at(&self.ops, range.start).ok_or_else(|| EditError::Invalid("the page's content has a transform that can't be read".into()))?;
        let state = PaintState::at(&self.ops, range.start).ops();
        let data = self.data();
        let start = self.ops.get(range.start).map_or(0, |op| op.span.start);
        let end = self.ops.get(range.end.saturating_sub(1)).map_or(0, |op| op.span.end);
        if start >= end || end > data.len() {
            return Err(EditError::Invalid("the object's drawing is missing".into()));
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"q\n");
        bytes.extend_from_slice(&serialize_ops(&state));
        let compensation =
            original.then(&insert.invert().ok_or_else(|| EditError::Invalid("the page's content has a transform that can't be restacked".into()))?);
        if !identity(compensation) {
            bytes.extend_from_slice(&serialize_ops(&[Op::new("cm", compensation.0.map(num).to_vec())]));
        }
        bytes.extend_from_slice(&data[start..end]);
        bytes.extend_from_slice(b"\nQ\n");
        Ok(bytes)
    }

    /// The parts that draw the moved objects at their new level, in their original order.
    fn moved_parts(&self, levels: &[&Level], insert: Matrix) -> Result<Vec<Part>, EditError> {
        let mut parts = Vec::new();
        for level in levels {
            parts.extend(self.moved_parts_of(level, insert)?);
        }
        Ok(parts)
    }

    /// The parts that draw one moved object at its new level.
    fn moved_parts_of(&self, level: &Level, insert: Matrix) -> Result<Vec<Part>, EditError> {
        match &level.unit {
            Unit::Entry(e) => {
                let first = self.entry_ops[*e];
                let state = PaintState::at(&self.ops, first).ops();
                let original =
                    ctm_at(&self.ops, first).ok_or_else(|| EditError::Invalid("the page's content has a transform that can't be read".into()))?;
                let compensation = original
                    .then(&insert.invert().ok_or_else(|| EditError::Invalid("the page's content has a transform that can't be restacked".into()))?);
                let mut open = Vec::new();
                open.extend_from_slice(b"q\n");
                open.extend_from_slice(&serialize_ops(&state));
                if !identity(compensation) {
                    open.extend_from_slice(&serialize_ops(&[Op::new("cm", compensation.0.map(num).to_vec())]));
                }
                Ok(vec![Part::Bytes(open), Part::Entry(*e), Part::Bytes(b"\nQ\n".to_vec())])
            }
            Unit::Ops(ranges) => {
                let mut bytes = Vec::new();
                for range in ranges {
                    bytes.extend_from_slice(&self.wrap(range, insert)?);
                }
                Ok(vec![Part::Bytes(bytes)])
            }
            Unit::Only => Ok(Vec::new()),
        }
    }

    /// Rebuild the page with `levels` moved to `place`, removing them from where they were.
    fn rebuild(&self, doc: &mut Document, levels: &[&Level], place: Place, insert: Matrix) -> Result<(), EditError> {
        let units: Vec<&Unit> = levels.iter().map(|l| &l.unit).collect();
        let removals = self.removals(&units)?;
        let removed_entries: BTreeSet<usize> = levels
            .iter()
            .filter_map(|level| match level.unit {
                Unit::Entry(e) => Some(e),
                _ => None,
            })
            .collect();
        let mut pending = Some((place, self.moved_parts(levels, insert)?));
        let mut out: Vec<Part> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let flush = |buf: &mut Vec<u8>, out: &mut Vec<Part>| {
            if !buf.is_empty() {
                out.push(Part::Bytes(std::mem::take(buf)));
            }
        };
        let data = self.data();

        for e in 0..self.entries.len() {
            if removed_entries.contains(&e) {
                continue;
            }
            if let Some((Place::Before(at), parts)) = &pending
                && *at == e
            {
                flush(&mut buf, &mut out);
                out.extend(parts.iter().cloned());
                pending = None;
            }
            if !self.base[e] {
                flush(&mut buf, &mut out);
                out.push(Part::Entry(e));
                continue;
            }
            let entry = self.entry_range(e);
            let mut segments = vec![entry.clone()];
            for removal in &removals {
                let mut next = Vec::with_capacity(segments.len());
                for seg in segments {
                    if removal.end <= seg.start || removal.start >= seg.end {
                        next.push(seg);
                        continue;
                    }
                    if seg.start < removal.start {
                        next.push(seg.start..removal.start);
                    }
                    if removal.end < seg.end {
                        next.push(removal.end..seg.end);
                    }
                }
                segments = next;
            }
            for seg in segments {
                match &pending {
                    Some((Place::Offset(at), parts)) if *at >= seg.start && *at <= seg.end => {
                        let at = *at;
                        if seg.start < at {
                            buf.extend_from_slice(&data[seg.start..at]);
                        }
                        flush(&mut buf, &mut out);
                        out.extend(parts.iter().cloned());
                        pending = None;
                        if at < seg.end {
                            buf.extend_from_slice(&data[at..seg.end]);
                        }
                    }
                    _ => buf.extend_from_slice(&data[seg.start..seg.end]),
                }
            }
        }
        // An insertion after all content.
        if let Some((_, parts)) = pending.take() {
            flush(&mut buf, &mut out);
            out.extend(parts);
        }
        flush(&mut buf, &mut out);

        let mut contents: Vec<Object> = Vec::with_capacity(out.len());
        for part in out {
            match part {
                Part::Bytes(bytes) if !bytes.is_empty() => {
                    contents.push(Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &bytes)))));
                }
                Part::Bytes(_) => {}
                Part::Entry(e) => {
                    let entry = self
                        .entries
                        .get(e)
                        .cloned()
                        .ok_or_else(|| EditError::Invalid("the page's content streams changed while restacking".into()))?;
                    contents.push(entry);
                }
            }
        }
        let contents = match <[Object; 1]>::try_from(contents) {
            Ok([one]) => one,
            Err(many) => Object::Array(many),
        };
        doc.update_dict(self.page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
        Ok(())
    }
}

/// Restack objects on `page` (0-based) as one undoable rewrite. The group keeps its own paint
/// order and moves above or below the object it is next to, or to the page's front or back.
pub fn restack_objects(doc: &mut Document, page: usize, targets: &[ObjectTarget], direction: Restack) -> Result<(), EditError> {
    let sheet = Sheet::open(doc, page)?;
    let levels = sheet.levels(doc, page)?;
    let mut indexes = Vec::with_capacity(targets.len());
    for target in targets {
        let index = levels
            .iter()
            .position(|l| l.target == *target)
            .ok_or_else(|| EditError::Invalid("a selected object no longer exists; select the object again".into()))?;
        if !indexes.contains(&index) {
            indexes.push(index);
        }
    }
    if indexes.is_empty() {
        return Err(EditError::Invalid("no object is selected".into()));
    }
    indexes.sort_unstable();
    let selected: Vec<&Level> = indexes.iter().filter_map(|i| levels.get(*i)).collect();
    if selected.iter().any(|level| matches!(level.unit, Unit::Only)) {
        return Ok(());
    }
    let (first, last) = (indexes[0], indexes[indexes.len() - 1]);
    let nothing_to_do = match direction {
        Restack::Front => last + 1 >= levels.len(),
        Restack::Back => first == 0,
        Restack::Forward => last + 1 >= levels.len(),
        Restack::Backward => first == 0,
    };
    if nothing_to_do {
        return Ok(());
    }
    let mut keep = BTreeSet::new();
    for level in &selected {
        keep.extend(sheet.active(level.first));
    }
    let (pos, insert_op) = match direction {
        Restack::Front => (sheet.ops.len(), sheet.ops.len()),
        Restack::Back => {
            // Above backgrounds and watermarks already at the start, below everything else.
            let behind = [crate::marks::MarkKind::Background.tag(), crate::marks::MarkKind::Watermark.tag()];
            let after = sheet.entries.iter().take_while(|e| crate::place::stream_tag(doc, e).is_some_and(|t| behind.contains(&t.as_str()))).count();
            let op = sheet.entry_ops.get(after).copied().unwrap_or(sheet.ops.len());
            let op = sheet.place_before(op, &keep)?;
            (op, op)
        }
        Restack::Forward => {
            let anchor = levels.get(last + 1).ok_or_else(|| EditError::Invalid("there is no other object to move above".into()))?;
            let op = sheet.place_after(anchor.end, &keep)?;
            (op, op)
        }
        Restack::Backward => {
            let anchor = levels.get(first.saturating_sub(1)).ok_or_else(|| EditError::Invalid("there is no other object to move below".into()))?;
            let op = sheet.place_before(anchor.first, &keep)?;
            (op, op)
        }
    };
    let place = if pos >= sheet.ops.len() {
        Place::Offset(sheet.data().len())
    } else {
        let entry = sheet.op_entry[pos];
        if sheet.base[entry] { Place::Offset(sheet.ops[pos].span.start) } else { Place::Before(entry) }
    };
    let insert = ctm_at(&sheet.ops, insert_op).ok_or_else(|| EditError::Invalid("the page's content has a transform that can't be read".into()))?;
    let mut work = doc.clone();
    sheet.rebuild(&mut work, &selected, place, insert)?;
    *doc = work;
    Ok(())
}

/// Restack one object on `page` (0-based) as one undoable rewrite.
pub fn restack_object(doc: &mut Document, page: usize, target: ObjectTarget, direction: Restack) -> Result<(), EditError> {
    restack_objects(doc, page, &[target], direction)
}
