//! Edit a PDF ▸ arrange page objects: bring a vector path, an image or grouped artwork (a Form)
//! to the front or forward, or send it backward or to the back. Only objects it overlaps count.
//! Its operators move byte for byte to just after a later object (or before an earlier one),
//! wrapped in `q … Q` with the graphics state they were painted in. A `q … Q` or marked-content
//! sequence holding nothing else (its private clip, its `/MCID`) moves with it. Only the streams
//! that change become new objects, and the page must list the same objects in the same places
//! afterwards.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use pdfcraft_content::{Matrix, Op, Pieces, overlaps, serialize_ops};
use pdfcraft_cos::{Dict, Document, Object};

use crate::objects::checked_streams;
use crate::{EditError, EditableObject, ObjectKind, ObjectTarget, PageImage, PagePath, editable_objects, page_images, page_paths, text_lines};

/// Where [`arrange_object`] moves an object in the page's painting order, among the objects it
/// overlaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZOrder {
    /// Above the last later-painted object it overlaps.
    Front,
    /// Above the next later-painted object it overlaps.
    Forward,
    /// Below the previous earlier-painted object it overlaps.
    Backward,
    /// Below the first earlier-painted object it overlaps.
    Back,
}

// The nesting families: `q … Q`, marked content (`BMC`/`BDC … EMC`) and text objects (`BT … ET`).
const Q: usize = 0;
const MC: usize = 1;
const BT: usize = 2;
/// Line style, rendering intent, flatness and ExtGState settings replayed for an object.
const MAX_SETTINGS: usize = 256;
/// The initial values (ISO 32000-2 §8.4.1) of what an object didn't set but its destination did.
const DEFAULTS: [(&[u8], &[u8]); 7] =
    [(b"w", b"1 w"), (b"J", b"0 J"), (b"j", b"0 j"), (b"M", b"10 M"), (b"d", b"[] 0 d"), (b"ri", b"/RelativeColorimetric ri"), (b"i", b"1 i")];
const IRREGULAR: &str = "the page content around this object is too irregular to rearrange it";
const OWN_LAYER: &str = "items added in PdfCraft and page marks keep their own layer";

fn invalid(message: &str) -> EditError {
    EditError::Invalid(message.into())
}

fn is_paint(op: &[u8]) -> bool {
    matches!(op, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"sh" | b"BI" | b"Do" | b"Tj" | b"TJ" | b"'" | b"\"")
}

fn is_show(op: &[u8]) -> bool {
    matches!(op, b"Tj" | b"TJ" | b"'" | b"\"")
}

/// Operators that set graphics state for whatever comes after them.
fn is_state(op: &[u8]) -> bool {
    matches!(
        op,
        b"cm"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"W"
            | b"W*"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            | b"Tf"
            | b"Tr"
            | b"Ts"
    )
}

/// The nesting family an operator opens (`true`) or closes.
fn family(op: &[u8]) -> Option<(usize, bool)> {
    match op {
        b"q" => Some((Q, true)),
        b"Q" => Some((Q, false)),
        b"BMC" | b"BDC" => Some((MC, true)),
        b"EMC" => Some((MC, false)),
        b"BT" => Some((BT, true)),
        b"ET" => Some((BT, false)),
        _ => None,
    }
}

/// How the page's operators nest. Gap `g` is just before operator `g` (gap `n` is the end).
struct Nesting {
    /// Each opener's matching closer.
    close: Vec<Option<usize>>,
    /// Per family, at each gap: the innermost open opener, and how many are open.
    inner: [Vec<Option<usize>>; 3],
    depth: [Vec<usize>; 3],
    /// How many painting operators come before each gap.
    paint: Vec<usize>,
    /// The text rendering mode each operator runs with.
    tr: Vec<i64>,
}

impl Nesting {
    fn new(ops: &[Op]) -> Self {
        let mut nest = Nesting {
            close: vec![None; ops.len()],
            inner: Default::default(),
            depth: Default::default(),
            paint: Vec::with_capacity(ops.len().saturating_add(1)),
            tr: Vec::with_capacity(ops.len()),
        };
        let mut stacks: [Vec<usize>; 3] = Default::default();
        let (mut painted, mut tr, mut saved) = (0_usize, 0_i64, Vec::new());
        for (i, op) in ops.iter().enumerate() {
            nest.record(&stacks, painted);
            nest.tr.push(tr);
            if is_paint(&op.op) {
                painted = painted.saturating_add(1);
            }
            // A closer without an opener is ignored, as readers do.
            if let Some((f, opens)) = family(&op.op)
                && let Some(stack) = stacks.get_mut(f)
            {
                if opens {
                    stack.push(i);
                } else if let Some(o) = stack.pop()
                    && let Some(close) = nest.close.get_mut(o)
                {
                    *close = Some(i);
                }
            }
            match op.op.as_slice() {
                b"q" => saved.push(tr),
                b"Q" => tr = saved.pop().unwrap_or(tr),
                b"Tr" => tr = op.num(0).map_or(tr, |mode| mode as i64),
                _ => {}
            }
        }
        nest.record(&stacks, painted);
        nest
    }

    fn record(&mut self, stacks: &[Vec<usize>; 3], painted: usize) {
        for ((inner, depth), stack) in self.inner.iter_mut().zip(self.depth.iter_mut()).zip(stacks) {
            inner.push(stack.last().copied());
            depth.push(stack.len());
        }
        self.paint.push(painted);
    }

    fn inner(&self, family: usize, gap: usize) -> Option<usize> {
        self.inner.get(family).and_then(|v| v.get(gap)).copied().flatten()
    }

    fn depth(&self, family: usize, gap: usize) -> usize {
        self.depth.get(family).and_then(|v| v.get(gap)).copied().unwrap_or(0)
    }

    fn close(&self, opener: usize) -> Option<usize> {
        self.close.get(opener).copied().flatten()
    }

    /// How many painting operators `s..=e` holds.
    fn paints(&self, s: usize, e: usize) -> usize {
        let before = |gap: usize| self.paint.get(gap).copied().unwrap_or(0);
        before(e.saturating_add(1)).saturating_sub(before(s))
    }

    /// Grow an object's own operators `(s, e)` out one enclosing `q … Q` or marked-content
    /// sequence at a time, while it paints nothing else, nests cleanly in all three families
    /// and, for marked content, sets no state for what follows it (a `q`-less `/Artifact BMC
    /// 1 0 0 rg … EMC` leaves its colour to later content).
    fn grow(&self, ops: &[Op], (mut s, mut e): (usize, usize)) -> (usize, usize) {
        let paints = self.paints(s, e);
        loop {
            let marked = self.inner(MC, s);
            let Some(o) = self.inner(Q, s).max(marked) else { break };
            let Some(c) = self.close(o).filter(|&c| c > e) else { break };
            let end = c.saturating_add(1);
            // Only the newly covered operators need checking: the range inside already passed.
            let fresh = || (o..s).chain(e.saturating_add(1)..end);
            let balanced = [Q, MC, BT].into_iter().all(|f| {
                let depth = self.depth(f, o);
                self.depth(f, end) == depth && fresh().all(|j| self.depth(f, j.saturating_add(1)) >= depth)
            });
            let leaks = marked == Some(o) && fresh().any(|j| ops.get(j).is_some_and(|op| is_state(&op.op)) && self.depth(Q, j) == self.depth(Q, o));
            if self.paints(o, c) != paints || !balanced || leaks {
                break;
            }
            (s, e) = (o, c);
        }
        (s, e)
    }

    /// The gaps an object at `(s, e)` can go to from `x` (just past its destination), innermost
    /// first: out of every marked-content sequence and text object it isn't in now, then each
    /// out of one more `q … Q` it isn't in.
    fn gaps(&self, x: usize, forward: bool, (s, e): (usize, usize)) -> Result<Vec<usize>, EditError> {
        let extra = |o: usize| if forward { o > e } else { self.close(o).is_some_and(|c| c < s) };
        let outermost = |x: usize, f: usize| {
            let (mut found, mut open) = (None, self.inner(f, x));
            while let Some(o) = open.filter(|&o| extra(o)) {
                found = Some(o);
                open = self.inner(f, o);
            }
            found
        };
        let leave = |o: usize| {
            if forward { self.close(o).map(|c| c.saturating_add(1)).ok_or_else(|| invalid(IRREGULAR)) } else { Ok(o) }
        };
        // Each step moves strictly outwards, so these loops end.
        let snap = |mut x: usize| -> Result<usize, EditError> {
            while let Some(o) = [outermost(x, MC), outermost(x, BT)].into_iter().flatten().min() {
                x = leave(o)?;
            }
            Ok(x)
        };
        let mut x = snap(x)?;
        let mut out = vec![x];
        while let Some(o) = self.inner(Q, x).filter(|&o| extra(o)) {
            // A `q` never closed lasts to the end, so there is no getting out of it.
            let Ok(next) = leave(o) else { break };
            x = snap(next)?;
            out.push(x);
        }
        Ok(out)
    }
}

/// What an object's painting depends on at a gap, as the operators that set it.
#[derive(Clone, Default, PartialEq)]
struct St {
    ctm: Matrix,
    /// `w J j M d ri i gs` in order, each with the CTM it ran under: replayed in order, they
    /// rebuild the line style, intent, flatness and ExtGState settings (a map would lose the
    /// difference between `/GS gs 2 w` and `2 w /GS gs`).
    list: Vec<(usize, Matrix)>,
    fill: Vec<usize>,
    stroke: Vec<usize>,
    /// The newest clipping operator in effect.
    clip: Option<usize>,
}

/// The state at each of the gaps `at`, from one walk over the page.
fn states(ops: &[Op], tr: &[i64], at: &BTreeSet<usize>) -> Result<BTreeMap<usize, St>, EditError> {
    let is = |j: usize, name: &[u8]| ops.get(j).is_some_and(|op| op.op == name);
    let mut st = St::default();
    let mut stack: Vec<St> = Vec::new();
    let (mut clipping, mut glyph_clip) = (false, false);
    let mut out = BTreeMap::new();
    for g in 0..=at.last().copied().unwrap_or(0) {
        if at.contains(&g) {
            out.insert(g, st.clone());
        }
        let Some(op) = ops.get(g) else { break };
        let name = op.op.as_slice();
        match name {
            b"q" => stack.push(st.clone()),
            b"Q" => {
                if let Some(saved) = stack.pop() {
                    st = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    st.ctm = Matrix(m).then(&st.ctm);
                }
            }
            b"w" | b"J" | b"j" | b"M" | b"d" | b"ri" | b"i" => {
                st.list.retain(|&(j, _)| !is(j, name));
                st.list.push((g, st.ctm));
            }
            // The same name is the same dictionary, setting the same keys.
            b"gs" => {
                let same_name = |j: usize| ops.get(j).is_some_and(|o| o.is("gs") && o.name(0) == op.name(0));
                st.list.retain(|&(j, _)| !same_name(j));
                st.list.push((g, st.ctm));
                if st.list.len() > MAX_SETTINGS {
                    return Err(invalid("this page has too many graphics-state settings to rearrange an object"));
                }
            }
            b"g" | b"rg" | b"k" | b"cs" => st.fill = vec![g],
            // Keep what chose the colour space: `g`, `rg` and `k` choose one too.
            b"sc" | b"scn" => {
                st.fill.retain(|&j| ops.get(j).is_some_and(|o| matches!(o.op.as_slice(), b"g" | b"rg" | b"k" | b"cs")));
                st.fill.push(g);
            }
            b"G" | b"RG" | b"K" | b"CS" => st.stroke = vec![g],
            b"SC" | b"SCN" => {
                st.stroke.retain(|&j| ops.get(j).is_some_and(|o| matches!(o.op.as_slice(), b"G" | b"RG" | b"K" | b"CS")));
                st.stroke.push(g);
            }
            b"W" | b"W*" => clipping = true,
            b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if std::mem::take(&mut clipping) {
                    st.clip = Some(g);
                }
            }
            b"BT" => glyph_clip = false,
            b"Tj" | b"TJ" | b"'" | b"\"" => glyph_clip |= tr.get(g).is_some_and(|mode| (4..=7).contains(mode)),
            // Glyphs shown in a clipping mode clip at the end of the text object (§9.3.6).
            b"ET" if glyph_clip => {
                glyph_clip = false;
                st.clip = Some(g);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// An object another can go above or below: its painting operator (a text line has one per
/// text-showing operator), its visible box and its own operators.
struct Unit {
    key: usize,
    rect: [f64; 4],
    core: Option<(usize, usize)>,
}

/// Where each image's `Do` is among the page's operators parsed joined.
fn image_keys(images: &[PageImage], ops: &[Op]) -> Vec<Option<usize>> {
    images.iter().map(|image| image.drawn_at(ops)).collect()
}

/// An object's own operators: a path's construction operators and its painting one, or a `Do`.
/// None of them sets state for what follows.
fn check_core(ops: &[Op], (s, e): (usize, usize), kind: ObjectKind) -> Result<(), EditError> {
    let core = ops.get(s..=e).unwrap_or_default();
    if core.iter().any(|op| op.is("W") || op.is("W*")) {
        return Err(invalid("this shape also clips other content, so it cannot be arranged"));
    }
    let neutral = match core.split_last() {
        Some((last, [])) if kind == ObjectKind::Image => last.is("Do"),
        Some((last, body)) if kind == ObjectKind::Path => {
            matches!(last.op.as_slice(), b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*")
                && body.iter().all(|op| matches!(op.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re"))
        }
        _ => false,
    };
    if neutral { Ok(()) } else { Err(invalid(IRREGULAR)) }
}

/// Whether a `BDC` operator's properties (inline, or named in the page's `/Properties`) give a
/// marked-content identifier, which the structure tree refers to, or text standing in for the
/// content (`/ActualText`, `/Alt`, `/E`), which a copy would repeat: it can't be split.
fn has_mcid(doc: &Document, properties: &Dict, op: &Op) -> bool {
    let Some(props) = op.operands.get(1) else { return false };
    let props = match props.as_name() {
        Some(name) => properties.get(name).map(|p| doc.resolve(p)),
        None => Some(doc.resolve(props)),
    };
    props.is_some_and(|p| p.as_dict().is_some_and(|d| [&b"MCID"[..], b"ActualText", b"Alt", b"E"].iter().any(|key| d.contains(key))))
}

/// The `cm` that turns CTM `from` into `to` (`M × from = to`), in full precision.
fn cm(to: Matrix, from: Matrix) -> Result<Vec<u8>, EditError> {
    let m = to.then(&from.invert().ok_or_else(|| invalid(IRREGULAR))?);
    if !m.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
        return Err(invalid(IRREGULAR));
    }
    Ok(serialize_ops(&[Op::new("cm", m.0.map(Object::Real).to_vec())]))
}

/// What goes in at the destination: the marked-content sequences the object leaves opened again
/// (`reopened`), then its operators (`owned`) in `q … Q` with the state it was painted in (`a`)
/// rebuilt from the state there (`x`). Defaults come first, so the object's own `gs` still wins,
/// and each `gs` runs under its own CTM (a soft mask is in its coordinates) before the last `cm`.
fn payload(ops: &[Op], data: &[u8], reopened: &[usize], a: &St, x: &St, owned: &[u8]) -> Result<Vec<u8>, EditError> {
    let bytes = |i: usize| ops.get(i).and_then(|op| data.get(op.span.clone())).unwrap_or_default();
    let line = |out: &mut Vec<u8>, b: &[u8]| {
        out.extend_from_slice(b);
        out.push(b'\n');
    };
    let mut out = b"\n".to_vec();
    for &o in reopened {
        line(&mut out, bytes(o));
    }
    out.extend_from_slice(b"q\n");
    let sets = |st: &St, kind: &[u8]| st.list.iter().any(|&(j, _)| ops.get(j).is_some_and(|op| op.op == kind));
    for (kind, default) in DEFAULTS {
        if sets(x, kind) && !sets(a, kind) {
            line(&mut out, default);
        }
    }
    for (mine, theirs, black, gray) in [(&a.fill, &x.fill, b"0 g", b"/DeviceGray cs"), (&a.stroke, &x.stroke, b"0 G", b"/DeviceGray CS")] {
        if mine == theirs {
            continue;
        }
        if mine.is_empty() {
            line(&mut out, black);
        }
        // A colour set by `sc` alone is in the initial space, which the destination may have left.
        if mine.first().and_then(|&j| ops.get(j)).is_some_and(|op| matches!(op.op.as_slice(), b"sc" | b"scn" | b"SC" | b"SCN")) {
            line(&mut out, gray);
        }
        for &j in mine {
            line(&mut out, bytes(j));
        }
    }
    let mut current = x.ctm;
    if a.list != x.list {
        for &(j, m) in &a.list {
            if ops.get(j).is_some_and(|op| op.is("gs")) && m != current {
                out.extend(cm(m, current)?);
                current = m;
            }
            line(&mut out, bytes(j));
        }
    }
    if a.ctm != current {
        out.extend(cm(a.ctm, current)?);
    }
    out.extend_from_slice(owned);
    out.extend_from_slice(b"\nQ\n");
    for _ in reopened {
        out.extend_from_slice(b"EMC\n");
    }
    Ok(out)
}

fn near(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-6 * x.abs().max(1.0))
}

type Inventory = Vec<(&'static str, [f64; 4])>;

/// What a page lists: `target`'s own entries in listing order, and everything else in an order
/// that doesn't depend on painting order. Only the target's places can change by rounding, so
/// sorting can't pair it with a neighbour that shares a coordinate.
fn inventory(paths: &[PagePath], images: &[PageImage], objects: &[EditableObject], target: ObjectTarget) -> (Inventory, Inventory) {
    let at = |kind, index| ObjectTarget { kind, index };
    let (own, rest): (Vec<_>, Vec<_>) = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (at(ObjectKind::Path, i), p.rect))
        .chain(images.iter().enumerate().map(|(i, image)| (at(ObjectKind::Image, i), image.rect)))
        .chain(objects.iter().map(|o| (o.target, o.rect)))
        .partition(|(t, _)| *t == target);
    let named = |list: Vec<(ObjectTarget, [f64; 4])>| -> Inventory { list.into_iter().map(|(t, rect)| (t.kind.as_str(), rect)).collect() };
    let mut rest = named(rest);
    rest.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.iter().zip(&b.1).map(|(x, y)| x.total_cmp(y)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)));
    (named(own), rest)
}

/// Whether two inventories list the same objects in the same places, to rounding.
fn same(before: &[(&str, [f64; 4])], after: &[(&str, [f64; 4])]) -> bool {
    before.len() == after.len() && before.iter().zip(after).all(|(a, b)| a.0 == b.0 && near(a.1, b.1))
}

/// Move `target` (a vector path, or an image or Form, from [`editable_objects`]) on `page` in
/// the painting order as one atomic edit, and return where it is listed afterwards (paths and
/// images are listed in painting order). A refusal leaves `doc` as it was.
pub fn arrange_object(doc: &mut Document, page: usize, target: ObjectTarget, order: ZOrder) -> Result<ObjectTarget, EditError> {
    let (work, moved) = arranged(doc, page, target, order)?;
    *doc = work;
    Ok(moved)
}

/// Where [`arrange_object`] would list `target` afterwards, or why it would refuse, without
/// changing `doc`.
pub fn arranged_target(doc: &Document, page: usize, target: ObjectTarget, order: ZOrder) -> Result<ObjectTarget, EditError> {
    arranged(doc, page, target, order).map(|(_, moved)| moved)
}

fn arranged(doc: &Document, page: usize, target: ObjectTarget, order: ZOrder) -> Result<(Document, ObjectTarget), EditError> {
    if matches!(target.kind, ObjectKind::Text | ObjectKind::Added) {
        return Err(invalid("only vector paths, images and grouped artwork can be arranged"));
    }
    let objects = editable_objects(doc, page)?;
    let gone = || invalid("the selected object no longer exists; select it again");
    let rect = objects.iter().find(|o| o.target == target).ok_or_else(gone)?.rect;
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = checked_streams(doc, page)?;
    // PdfCraft's marks, added items and the `q`/`Q` it puts around the page's own content.
    let marked: Vec<bool> = streams.iter().map(|(o, _)| crate::stream_tag(doc, o).is_some()).collect();
    let pieces = Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>());
    let (ops, data) = (pieces.parse().ops, pieces.data());
    let foreign = |i: usize| {
        ops.get(i).is_some_and(|op| {
            let (first, last) = pieces.pieces_of(op);
            marked.get(first) == Some(&true) || marked.get(last) == Some(&true)
        })
    };
    let paths = page_paths(doc, page)?;
    let images = page_images(doc, page)?;
    let keys = image_keys(&images, &ops);
    let core = match target.kind {
        ObjectKind::Path => paths.get(target.index).map(|p| (*p.ops.start(), *p.ops.end())).ok_or_else(gone)?,
        _ => keys.get(target.index).copied().flatten().map(|k| (k, k)).ok_or_else(|| invalid(IRREGULAR))?,
    };
    check_core(&ops, core, target.kind)?;
    let nest = Nesting::new(&ops);
    if nest.depth(BT, core.0) > 0 {
        return Err(invalid("an object inside a text object cannot be arranged"));
    }
    let (s, e) = nest.grow(&ops, core);
    if (s..=e).any(foreign) {
        return Err(invalid(OWN_LAYER));
    }

    let forward = matches!(order, ZOrder::Front | ZOrder::Forward);
    let mut units: Vec<Unit> = paths.iter().map(|p| Unit { key: *p.ops.end(), rect: p.rect, core: Some((*p.ops.start(), *p.ops.end())) }).collect();
    units.extend(images.iter().zip(&keys).filter_map(|(image, &key)| {
        let key = key?;
        Some(Unit { key, rect: image.visible_rect?, core: Some((key, key)) })
    }));
    for line in text_lines(doc, page)? {
        for &key in line.ops() {
            // Invisible text (`3 Tr`, `7 Tr`: an OCR layer) is below and above nothing.
            let shown = ops.get(key).is_some_and(|op| is_show(&op.op));
            if !shown || matches!(nest.tr.get(key), Some(3 | 7)) {
                continue;
            }
            let core = nest.inner(BT, key).and_then(|bt| Some((bt, nest.close(bt)?)));
            units.push(Unit { key, rect: line.rect, core });
        }
    }
    let mut hits: Vec<&Unit> =
        units.iter().filter(|u| if forward { u.key > e } else { u.key < s }).filter(|u| overlaps(rect, u.rect, 1e-3)).collect();
    hits.sort_by_key(|u| u.key);
    let foreign_hits = hits.iter().any(|u| foreign(u.key));
    hits.retain(|u| !foreign(u.key));
    let dest = if matches!(order, ZOrder::Forward | ZOrder::Back) { hits.first() } else { hits.last() };
    // Nothing would look different: don't rewrite the page for it.
    let Some(dest) = dest else {
        return Err(invalid(match (foreign_hits, forward) {
            (true, _) => OWN_LAYER,
            (false, true) => "nothing above this object overlaps it",
            (false, false) => "nothing below this object overlaps it",
        }));
    };
    let unit = nest.grow(&ops, dest.core.ok_or_else(|| invalid(IRREGULAR))?);
    let gaps = nest.gaps(if forward { unit.1.saturating_add(1) } else { unit.0 }, forward, (s, e))?;
    let states = states(&ops, &nest.tr, &gaps.iter().copied().chain([s]).collect())?;
    let a = states.get(&s).ok_or_else(|| invalid(IRREGULAR))?;
    let names = |st: &St| st.list.iter().filter_map(|&(j, _)| ops.get(j).filter(|op| op.is("gs")).map(|op| op.name(0))).collect::<BTreeSet<_>>();
    let mine = names(a);
    // The destination's own level when the same clip applies there, otherwise out of the group.
    let Some((x, at_x)) = gaps.iter().find_map(|g| states.get(g).filter(|st| st.clip == a.clip && names(st).is_subset(&mine)).map(|st| (*g, st)))
    else {
        let clipped = gaps.last().and_then(|g| states.get(g)).is_some_and(|st| st.clip != a.clip);
        return Err(invalid(if clipped {
            "this object cannot move past content that is clipped differently"
        } else {
            "this object cannot move into content with different transparency or blending"
        }));
    };

    // The marked-content sequences it leaves, outermost first.
    let mut reopened = Vec::new();
    let mut open = nest.inner(MC, s);
    while let Some(o) = open {
        let still_open = o < x && nest.close(o).is_none_or(|c| c >= x);
        if !still_open {
            reopened.push(o);
        }
        open = nest.inner(MC, o);
    }
    reopened.reverse();
    let properties = p
        .dict
        .get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|r| r.get(b"Properties")).map(|c| doc.resolve(c)))
        .and_then(|c| c.as_dict().cloned())
        .unwrap_or_default();
    if reopened.iter().any(|&o| ops.get(o).is_some_and(|op| has_mcid(doc, &properties, op))) {
        return Err(invalid("this object is tagged together with other content; arranging it would change the document structure"));
    }

    let anchor =
        (if forward { x.checked_sub(1) } else { Some(x) }).filter(|&i| !foreign(i)).and_then(|i| ops.get(i)).ok_or_else(|| invalid(IRREGULAR))?;
    let at = if forward { anchor.span.end } else { anchor.span.start };
    let range = ops.get(s).zip(ops.get(e)).map(|(first, last)| first.span.start..last.span.end).ok_or_else(|| invalid(IRREGULAR))?;
    let owned = data.get(range.clone()).ok_or_else(|| invalid(IRREGULAR))?;
    // What it leaves behind sets no state, so later content renders as before.
    let removal = (range, Vec::new());
    let insertion = (at..at, payload(&ops, data, &reopened, a, at_x, owned)?);
    let edits = if forward { [removal, insertion] } else { [insertion, removal] };
    let mut work = doc.clone();
    let contents = crate::text::rewritten_contents(&mut work, &streams, pieces.splice(edits));
    work.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;

    // Paths and images are listed in painting order: count those that stay before it.
    let index = match target.kind {
        ObjectKind::Path => paths.iter().enumerate().filter(|(i, p)| *i != target.index && *p.ops.end() < x).count(),
        _ => keys.iter().enumerate().filter(|(i, k)| *i != target.index && k.is_some_and(|k| k < x)).count(),
    };
    let moved = ObjectTarget { kind: target.kind, index };
    let (own, rest) = inventory(&paths, &images, &objects, target);
    let (own_after, rest_after) = inventory(&page_paths(&work, page)?, &page_images(&work, page)?, &editable_objects(&work, page)?, moved);
    if !same(&own, &own_after) || !same(&rest, &rest_after) {
        return Err(invalid("arranging this object would change the page; nothing was changed"));
    }
    Ok((work, moved))
}

#[cfg(test)]
#[path = "arrange_tests.rs"]
mod tests;
