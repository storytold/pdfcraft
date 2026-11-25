//! Where `/Artifact BMC … EMC` wraps go on one page (ISO 32000-1 §14.8.2.2.2). A wrap must nest
//! properly with the page's marked content and with `q`/`Q`, `BT`/`ET` and `BX`/`EX` (§14.6), so
//! the content is first read into a tree of those pairs, and each wrap opens and closes inside
//! one pair and inside one content stream of the page.
//!
//! Adapted from pdfcer (MIT, same author) at commit debd0d63, `preflight/pdfua/artifact.rs`.

use std::collections::HashMap;
use std::ops::Range;

use pdfcraft_content::{Op, Pieces};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

use crate::untagged::Claims;

const OPEN: &[u8] = b"/Artifact BMC\n";
const CLOSE: &[u8] = b"\nEMC\n";
/// Pairs open at once. The wrap planner recurses once per level.
const MAX_NESTING: usize = 256;
/// Forms drawn inside forms, as deep as the checker reads them.
const MAX_FORM_DEPTH: usize = 12;

/// The wraps for one page, and what was left.
#[derive(Debug, Default)]
pub(crate) struct Wraps {
    /// Insertions into the joined content, in ascending order.
    pub edits: Vec<(Range<usize>, Vec<u8>)>,
    pub sequences: usize,
    /// Content items the wraps cover.
    pub items: usize,
    /// Sequences with an `/MCID` the structure tree doesn't claim, left as they are.
    pub orphans: usize,
    /// Forms holding tagged content beside untagged content.
    pub mixed_forms: Vec<ObjRef>,
    /// Forms that could not be read in full, left as they are.
    pub unread_forms: Vec<ObjRef>,
    /// Untagged items whose operands and operator are in different content streams.
    pub split_items: usize,
    /// Paths begun in one content stream and painted or ended in a later one.
    pub split_paths: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Root,
    Save,
    Text,
    Compat,
    Mark(Mark),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    /// Claimed by the structure tree, or an artifact already.
    Covered,
    /// An `/MCID` nothing claims.
    Orphan,
    /// Any other sequence (`/OC`, a `/Span` without an MCID, …).
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leaf {
    /// Untagged painted content.
    Item,
    /// Draws nothing.
    Neutral,
    /// Must not be enclosed: tagged content, or a form not read in full.
    Barrier,
}

#[derive(Clone, Copy, Debug)]
enum Unit {
    Leaf { start: usize, end: usize, class: Leaf },
    Frame(usize),
}

#[derive(Debug)]
struct Frame {
    kind: Kind,
    start: usize,
    end: usize,
    units: Vec<Unit>,
    items: usize,
    barrier: bool,
}

impl Frame {
    fn new(kind: Kind, start: usize) -> Self {
        let barrier = matches!(kind, Kind::Mark(Mark::Covered | Mark::Orphan));
        Frame { kind, start, end: start, units: Vec::new(), items: 0, barrier }
    }

    fn descends(&self) -> bool {
        !matches!(self.kind, Kind::Mark(Mark::Covered | Mark::Orphan))
    }
}

/// What a form draws, read as the checker reads it.
#[derive(Debug, Default)]
struct Census {
    /// Marked content with an `/MCID`, or an image or form with `/StructParent` (§14.7.4.3).
    tagged: bool,
    /// Items neither in marked content with an `/MCID` nor in an artifact.
    untagged: usize,
    /// Part of it could not be read: its depth, its data or the page's allowance.
    unread: bool,
    /// The forms being read, outermost first, so a form that draws itself is read once.
    active: Vec<ObjRef>,
}

const ITEMS: &[&[u8]] = &[b"Tj", b"TJ", b"'", b"\"", b"f", b"F", b"f*", b"B", b"B*", b"b", b"b*", b"S", b"s", b"sh", b"BI"];
/// Operators that build a path or set it as the clip (ISO 32000-1 §8.5.2, §8.5.4).
const PATH: &[&[u8]] = &[b"m", b"l", b"c", b"v", b"y", b"h", b"re", b"W", b"W*"];
/// Operators that end a path (§8.5.3).
const PATH_ENDS: &[&[u8]] = &[b"S", b"s", b"f", b"F", b"f*", b"B", b"B*", b"b", b"b*", b"n"];

fn sub(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

fn closes(op: &[u8], kind: Kind) -> Option<bool> {
    match op {
        b"Q" => Some(kind == Kind::Save),
        b"ET" => Some(kind == Kind::Text),
        b"EX" => Some(kind == Kind::Compat),
        b"EMC" => Some(matches!(kind, Kind::Mark(_))),
        _ => None,
    }
}

/// A `BDC`'s property list, inline or named in `props`.
fn properties(doc: &Document, props: &Dict, op: &Op) -> Option<Dict> {
    match op.operands.get(1)? {
        Object::Dict(d) => Some(d.clone()),
        Object::Name(n) => props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()),
        _ => None,
    }
}

pub(crate) struct Planner<'a> {
    doc: &'a Document,
    page: ObjRef,
    claims: &'a Claims,
    props: Dict,
    xobjects: Dict,
    resources: Dict,
    budget: DecodeBudget,
    forms: HashMap<ObjRef, Leaf>,
    /// The content stream the path being built began in.
    path: Option<usize>,
    wraps: Wraps,
}

impl<'a> Planner<'a> {
    pub(crate) fn new(doc: &'a Document, page: ObjRef, page_dict: &Dict, claims: &'a Claims) -> Self {
        let resources = page_dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        Planner {
            doc,
            page,
            claims,
            props: sub(doc, &resources, b"Properties"),
            xobjects: sub(doc, &resources, b"XObject"),
            resources,
            budget: DecodeBudget::page(),
            forms: HashMap::new(),
            path: None,
            wraps: Wraps::default(),
        }
    }

    /// The wraps that make every untagged item of `ops` (parsed from `pieces`) an artifact.
    ///
    /// # Errors
    ///
    /// Why nothing can be planned: the pairs don't match, or nest deeper than this reads.
    pub(crate) fn plan(mut self, pieces: &Pieces, ops: &[Op]) -> Result<Wraps, String> {
        let frames = self.read(pieces, ops)?;
        let root = frames.len().checked_sub(1).ok_or("its content has no root")?;
        let mut marks: Vec<(usize, u8, &'static [u8])> = Vec::new();
        wrap_frame(&frames, root, pieces, &mut marks, &mut self.wraps);
        marks.sort_by_key(|&(at, order, _)| (at, order));
        self.wraps.edits = marks.into_iter().map(|(at, _, bytes)| (at..at, bytes.to_vec())).collect();
        Ok(self.wraps)
    }

    /// The pairs of the content, children before parents; the root is last.
    fn read(&mut self, pieces: &Pieces, ops: &[Op]) -> Result<Vec<Frame>, String> {
        let mut done: Vec<Frame> = Vec::new();
        let mut open: Vec<Frame> = vec![Frame::new(Kind::Root, 0)];
        for op in ops {
            let (start, end) = (op.span.start, op.span.end);
            let opens = match op.op.as_slice() {
                b"q" => Some(Kind::Save),
                b"BT" => Some(Kind::Text),
                b"BX" => Some(Kind::Compat),
                b"BMC" | b"BDC" => Some(Kind::Mark(self.mark(op))),
                _ => None,
            };
            if let Some(kind) = opens {
                if open.len() >= MAX_NESTING {
                    return Err(format!("its content nests more than {MAX_NESTING} levels deep"));
                }
                open.push(Frame::new(kind, start));
                continue;
            }
            if let Some(kind) = open.last().map(|f| f.kind) {
                match closes(&op.op, kind) {
                    Some(true) => {
                        close(&mut open, &mut done, end, &mut self.wraps)?;
                        continue;
                    }
                    Some(false) => return Err(format!("its content operators don't pair up (an unmatched {})", String::from_utf8_lossy(&op.op))),
                    None => {}
                }
            }
            let leaf = self.leaf(op);
            let class = self.path(op, pieces, leaf);
            let top = open.last_mut().ok_or("its content has no root")?;
            top.items = top.items.saturating_add(usize::from(class == Leaf::Item));
            top.barrier |= class == Leaf::Barrier;
            top.units.push(Unit::Leaf { start, end, class });
        }
        if open.len() != 1 {
            return Err("its content leaves a q, BT, BX or marked-content sequence open".into());
        }
        done.extend(open.pop());
        Ok(done)
    }

    /// `class` for `op`, or a barrier when `op` ends a path begun in an earlier content stream:
    /// no marked-content operator may come inside a path object (§8.2, Figure 9).
    fn path(&mut self, op: &Op, pieces: &Pieces, class: Leaf) -> Leaf {
        let name = op.op.as_slice();
        if PATH.contains(&name) {
            self.path.get_or_insert(pieces.piece_of(op.span.start));
            return class;
        }
        if !PATH_ENDS.contains(&name) {
            return class;
        }
        match self.path.take() {
            Some(first) if first != pieces.piece_of(op.span.end.saturating_sub(1)) => {
                self.wraps.split_paths = self.wraps.split_paths.saturating_add(1);
                Leaf::Barrier
            }
            _ => class,
        }
    }

    fn mark(&self, op: &Op) -> Mark {
        if op.name(0) == Some(b"Artifact") {
            return Mark::Covered;
        }
        if op.op != b"BDC" {
            return Mark::Other;
        }
        let Some(props) = properties(self.doc, &self.props, op) else { return Mark::Other };
        match props.get(b"MCID").map(|m| self.doc.resolve(m).as_int()) {
            None => Mark::Other,
            Some(Some(mcid)) if self.claims.claims(self.page, mcid) => Mark::Covered,
            Some(_) => Mark::Orphan,
        }
    }

    fn leaf(&mut self, op: &Op) -> Leaf {
        if ITEMS.contains(&op.op.as_slice()) {
            return Leaf::Item;
        }
        if op.op != b"Do" {
            return Leaf::Neutral;
        }
        let Some(r) = op.name(0).and_then(|n| self.xobjects.get(n)).and_then(Object::as_ref) else { return Leaf::Neutral };
        if let Some(class) = self.forms.get(&r) {
            return *class;
        }
        let class = self.xobject(r);
        self.forms.insert(r, class);
        class
    }

    fn xobject(&mut self, r: ObjRef) -> Leaf {
        let obj = self.doc.get(r);
        let Object::Stream(x) = &*obj else { return Leaf::Neutral };
        match x.dict.name(b"Subtype") {
            Some(b"Image" | b"Form") if x.dict.contains(b"StructParent") => Leaf::Barrier,
            Some(b"Image") => Leaf::Item,
            Some(b"Form") => {
                let mut census = Census::default();
                let resources = self.resources.clone();
                self.census(r, &resources, false, 0, &mut census);
                if census.unread {
                    self.wraps.unread_forms.push(r);
                    Leaf::Barrier
                } else if census.tagged {
                    if census.untagged > 0 {
                        self.wraps.mixed_forms.push(r);
                    }
                    Leaf::Barrier
                } else if census.untagged > 0 {
                    Leaf::Item
                } else {
                    Leaf::Neutral
                }
            }
            _ => Leaf::Neutral,
        }
    }

    /// Read form `r`, drawn with `outer` resources and inside covered content when `covered`.
    fn census(&mut self, r: ObjRef, outer: &Dict, covered: bool, depth: usize, out: &mut Census) {
        if out.active.contains(&r) {
            return;
        }
        let obj = self.doc.get(r);
        let Object::Stream(x) = &*obj else { return };
        let data = if depth >= MAX_FORM_DEPTH || !self.budget.enter_form() { None } else { self.budget.decode(x).ok() };
        let Some(data) = data else {
            out.unread = true;
            return;
        };
        let res = x.dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_else(|| outer.clone());
        let (props, xobjects) = (sub(self.doc, &res, b"Properties"), sub(self.doc, &res, b"XObject"));
        out.active.push(r);
        let mut stack: Vec<bool> = Vec::new();
        for op in pdfcraft_content::ops(&data) {
            let inside = covered || stack.last().copied().unwrap_or(false);
            match op.op.as_slice() {
                b"BMC" => stack.push(inside || op.name(0) == Some(b"Artifact")),
                b"BDC" => {
                    let mcid = properties(self.doc, &props, &op).is_some_and(|d| d.contains(b"MCID"));
                    out.tagged |= mcid;
                    stack.push(inside || mcid || op.name(0) == Some(b"Artifact"));
                }
                b"EMC" => {
                    stack.pop();
                }
                b"Do" => self.nested(&op, &xobjects, &res, inside, depth, out),
                name if ITEMS.contains(&name) => out.untagged = out.untagged.saturating_add(usize::from(!inside)),
                _ => {}
            }
            if out.unread {
                break;
            }
        }
        out.active.pop();
    }

    fn nested(&mut self, op: &Op, xobjects: &Dict, res: &Dict, inside: bool, depth: usize, out: &mut Census) {
        let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { return };
        let obj = self.doc.get(r);
        let Object::Stream(x) = &*obj else { return };
        match x.dict.name(b"Subtype") {
            Some(b"Image" | b"Form") if x.dict.contains(b"StructParent") => out.tagged = true,
            Some(b"Image") => out.untagged = out.untagged.saturating_add(usize::from(!inside)),
            Some(b"Form") => self.census(r, res, inside, depth.saturating_add(1), out),
            _ => {}
        }
    }
}

/// Close the innermost pair at `end` and add it to its parent.
fn close(open: &mut Vec<Frame>, done: &mut Vec<Frame>, end: usize, wraps: &mut Wraps) -> Result<(), String> {
    let mut frame = open.pop().ok_or("its content has no root")?;
    frame.end = end;
    if frame.kind == Kind::Mark(Mark::Orphan) {
        wraps.orphans = wraps.orphans.saturating_add(1);
    }
    let parent = open.last_mut().ok_or("its content operators don't pair up")?;
    parent.items = parent.items.saturating_add(frame.items);
    parent.barrier |= frame.barrier;
    parent.units.push(Unit::Frame(done.len()));
    done.push(frame);
    Ok(())
}

/// A unit's span, how a wrap treats it, and the items it holds.
fn status(frames: &[Frame], unit: Unit) -> (usize, usize, Leaf, usize) {
    match unit {
        Unit::Leaf { start, end, class } => (start, end, class, usize::from(class == Leaf::Item)),
        Unit::Frame(i) => match frames.get(i) {
            Some(f) if f.barrier => (f.start, f.end, Leaf::Barrier, 0),
            Some(f) if f.items > 0 => (f.start, f.end, Leaf::Item, f.items),
            Some(f) => (f.start, f.end, Leaf::Neutral, 0),
            None => (0, 0, Leaf::Barrier, 0),
        },
    }
}

/// An open segment of a frame's body: where its wrap opens, the piece it is in, the end of its
/// last item and the items it covers.
#[derive(Default)]
struct Segment {
    open: Option<(usize, usize)>,
    last: Option<usize>,
    items: usize,
}

impl Segment {
    fn flush(&mut self, marks: &mut Vec<(usize, u8, &'static [u8])>, wraps: &mut Wraps) {
        if let (Some((start, _)), Some(end)) = (self.open, self.last) {
            marks.push((start, 1, OPEN));
            marks.push((end, 0, CLOSE));
            wraps.sequences = wraps.sequences.saturating_add(1);
            wraps.items = wraps.items.saturating_add(self.items);
        }
        *self = Segment::default();
    }
}

/// Wrap the untagged runs of `frames[index]`'s body, and descend into each child that holds
/// tagged content or crosses from one content stream into the next. A run starts at the first
/// unit after a barrier, so no path object is split, and ends after its last item.
fn wrap_frame(frames: &[Frame], index: usize, pieces: &Pieces, marks: &mut Vec<(usize, u8, &'static [u8])>, wraps: &mut Wraps) {
    let Some(frame) = frames.get(index) else { return };
    let mut segment = Segment::default();
    for unit in &frame.units {
        let (start, end, class, count) = status(frames, *unit);
        let piece = pieces.piece_of(start);
        if class == Leaf::Barrier || piece != pieces.piece_of(end.saturating_sub(1)) {
            segment.flush(marks, wraps);
            match unit {
                Unit::Frame(child) if frames.get(*child).is_some_and(Frame::descends) => wrap_frame(frames, *child, pieces, marks, wraps),
                Unit::Leaf { class: Leaf::Item, .. } => wraps.split_items = wraps.split_items.saturating_add(count),
                _ => {}
            }
            continue;
        }
        if segment.open.is_some_and(|(_, p)| p != piece) {
            segment.flush(marks, wraps);
        }
        segment.open.get_or_insert((start, piece));
        if class == Leaf::Item {
            segment.last = Some(end);
            segment.items = segment.items.saturating_add(count);
        }
    }
    segment.flush(marks, wraps);
}
