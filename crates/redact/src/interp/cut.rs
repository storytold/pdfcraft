//! Cutting a path a mark reaches down to what it paints outside the marks (#7300).
//!
//! A path is often many shapes: a spreadsheet export fills every cell of a colour with one
//! path of `re`s and strokes every grid line with one `S`. Removing such a path whole because a
//! mark lies over one cell wiped tables far from the mark. Here the path is rebuilt from what
//! lies outside the marks, and nothing of it stays under one, behind a clip or otherwise (F-008):
//! - a subpath whose box meets no mark is kept as written;
//! - a filled rectangle the mark reaches (under a CTM that keeps axes) becomes the rectangles
//!   around the mark, oriented as it was, so the fill rule encloses the same area outside them;
//! - a stroked straight segment loses the part within reach of a mark (the mark grown by the
//!   stroke's reach); a stroked curve that comes within reach goes whole.
//!
//! Anything else a mark reaches (a filled curve, a filled shape under a rotating CTM, a dashed
//! stroke) returns `None`, as does a cut too large to make, and the caller removes the path
//! whole, as before. The pieces are written as several paths, none of whose boxes meets a mark,
//! so the proof (`verify::geometry`), which measures a path by its box, finds nothing under the
//! marks. Filled pieces whose boxes overlap stay in one
//! path, so their fill rule still sees them together.

use pdfcraft_content::{Matrix, Op, num, overlaps};

/// More path operators than this are not cut: the path goes whole.
const MAX_OPS: usize = 1 << 14;
/// Pieces are grouped pairwise; more than this and the path goes whole.
const MAX_PIECES: usize = 4096;
/// Box tests grouping pieces into paths may make (each piece against each path so far and
/// every region); more and the path goes whole, so many marks over a path can't stall it.
const MAX_WORK: usize = 1 << 26;

type P = (f64, f64);

/// How far a stroke paints from its outline, in device space.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Reach {
    /// From anywhere on a path kept as written: every corner may be a mitred join.
    pub all: f64,
    /// From a straight stretch or its cut end: half the line width (times sqrt 2 for a
    /// projecting cap's corner).
    pub line: f64,
    /// Half the line width, which a round or bevelled join reaches.
    pub half: f64,
    /// The miter limit, when joins are mitred.
    pub miter: Option<f64>,
    /// The stroke is dashed. Each subpath restarts the dashes, so a subpath cut into new ones
    /// would move them along what is kept: such a path goes whole.
    pub dashed: bool,
}

impl Reach {
    /// How far a join from direction `into` to direction `out` (user space) reaches. A mitred
    /// join reaches `half / sin(φ/2)` at an angle φ, and is bevelled past the miter limit
    /// (ISO 32000-2 §8.4.3.5); measured on the page only when the CTM keeps angles and both
    /// sides are straight, it is taken at the limit otherwise.
    fn join(&self, keeps_angles: bool, into: Option<P>, out: Option<P>) -> f64 {
        let Some(limit) = self.miter else { return self.half };
        let (Some(u), Some(v), true) = (into, out, keeps_angles) else { return self.half * limit };
        let (lu, lv) = (u.0.hypot(u.1), v.0.hypot(v.1));
        if !(lu > 0.0 && lv > 0.0) {
            return self.half * limit;
        }
        let cos = -(u.0 * v.0 + u.1 * v.1) / (lu * lv);
        let ratio = 1.0 / ((1.0 - cos) / 2.0).max(0.0).sqrt();
        // Taken as mitred a hair past the limit, where a renderer's own arithmetic may still mitre it.
        if ratio.is_finite() && ratio <= limit * (1.0 + 1e-9) { self.half * ratio } else { self.half }
    }
}

/// Does the CTM keep angles (a rotation and uniform scale, perhaps mirrored)?
fn keeps_angles(m: &Matrix) -> bool {
    let [a, b, c, d, ..] = m.0;
    let s = a.abs().max(b.abs()).max(c.abs()).max(d.abs()) * 1e-9;
    ((a - d).abs() <= s && (b + c).abs() <= s) || ((a + d).abs() <= s && (b - c).abs() <= s)
}

/// What replaces a path a mark reaches: its painting `ops`, and whether they fill and stroke.
pub(super) struct Cut {
    pub ops: Vec<Op>,
    pub fills: bool,
    pub strokes: bool,
}

enum Seg {
    Line(P, P),
    /// A Bézier segment from `.0`, written by `.2` (`c`, `v` or `y`), with control points `.1`.
    Curve(P, Vec<P>, Op),
}

struct Sub {
    /// The subpath's operators as written.
    ops: Vec<Op>,
    /// `re`'s operands (x, y, w, h), when the subpath is one.
    rect: Option<[f64; 4]>,
    start: P,
    segs: Vec<Seg>,
    closed: bool,
    /// Every point it names, control points included, in user space.
    pts: Vec<P>,
}

impl Sub {
    fn new(op: Op, start: P) -> Self {
        Sub { ops: vec![op], rect: None, start, segs: Vec::new(), closed: false, pts: vec![start] }
    }
}

/// Split a path's construction operators into subpaths; `None` if one can't be read.
fn subpaths(body: &[Op]) -> Option<Vec<Sub>> {
    let mut subs: Vec<Sub> = Vec::new();
    let mut at: P = (0.0, 0.0);
    for op in body {
        let o = op.op.as_slice();
        match o {
            b"m" => {
                let [x, y] = op.nums::<2>()?;
                at = (x, y);
                subs.push(Sub::new(op.clone(), at));
            }
            b"re" => {
                let [x, y, w, h] = op.nums::<4>()?;
                let mut s = Sub::new(op.clone(), (x, y));
                let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
                s.segs = (0..4).map(|i| Seg::Line(corners[i], corners[(i + 1) % 4])).collect();
                s.pts = corners.to_vec();
                (s.rect, s.closed, at) = (Some([x, y, w, h]), true, (x, y));
                subs.push(s);
            }
            b"l" | b"c" | b"v" | b"y" | b"h" => {
                // A segment needs an open subpath that `m` began.
                let s = subs.last_mut().filter(|s| s.rect.is_none() && !s.closed)?;
                let end = match o {
                    b"l" => {
                        let [x, y] = op.nums::<2>()?;
                        s.segs.push(Seg::Line(at, (x, y)));
                        (x, y)
                    }
                    b"h" => {
                        s.closed = true;
                        s.segs.push(Seg::Line(at, s.start));
                        s.start
                    }
                    _ => {
                        let ctrl: Vec<P> = match o {
                            b"c" => op.nums::<6>()?.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect(),
                            b"v" => op.nums::<4>().map(|[x2, y2, x3, y3]| vec![at, (x2, y2), (x3, y3)])?,
                            _ => op.nums::<4>().map(|[x1, y1, x3, y3]| vec![(x1, y1), (x3, y3), (x3, y3)])?,
                        };
                        let end = *ctrl.last()?;
                        s.pts.extend(&ctrl);
                        s.segs.push(Seg::Curve(at, ctrl, op.clone()));
                        end
                    }
                };
                s.pts.push(end);
                s.ops.push(op.clone());
                at = end;
            }
            _ => return None,
        }
    }
    Some(subs)
}

fn bbox(pts: impl IntoIterator<Item = P>) -> Option<[f64; 4]> {
    pts.into_iter().fold(None, |b, (u, v)| {
        Some(match b {
            None => [u, v, u, v],
            Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
        })
    })
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn grow(r: [f64; 4], by: f64) -> [f64; 4] {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

/// Does box `b` meet a region? Edges count, so a straight line's flat box through one does.
fn meets(regions: &[[f64; 4]], b: [f64; 4]) -> bool {
    regions.iter().any(|r| b[0].max(r[0]) <= b[2].min(r[2]) && b[1].max(r[1]) <= b[3].min(r[3]))
}

/// Does the CTM map axis-aligned rectangles to axis-aligned rectangles?
fn keeps_axes(m: &Matrix) -> bool {
    let [a, b, c, d, ..] = m.0;
    let s = a.abs().max(b.abs()).max(c.abs()).max(d.abs());
    (b.abs() <= 1e-9 * s && c.abs() <= 1e-9 * s) || (a.abs() <= 1e-9 * s && d.abs() <= 1e-9 * s)
}

/// `d` (device space, normalised) less every region: disjoint rectangles, `None` past
/// `budget` of them (they can grow as the product of the marks crossing it).
fn subtract(d: [f64; 4], regions: &[[f64; 4]], budget: usize) -> Option<Vec<[f64; 4]>> {
    let mut left = vec![d];
    for r in regions {
        let mut next = Vec::with_capacity(left.len());
        for p in left {
            if !overlaps(*r, p, 0.0) {
                next.push(p);
                continue;
            }
            let (y0, y1) = (p[1].max(r[1]), p[3].min(r[3]));
            let strips = [[p[0], p[1], p[2], r[1]], [p[0], r[3], p[2], p[3]], [p[0], y0, r[0], y1], [r[2], y0, p[2], y1]];
            next.extend(strips.into_iter().filter(|q| q[2] - q[0] > 1e-9 && q[3] - q[1] > 1e-9));
            if next.len() > budget {
                return None;
            }
        }
        left = next;
    }
    Some(left)
}

/// The parameter intervals of the segment `a`–`b` (device space) outside every region, `None`
/// once `work` (intervals tested against regions) passes [`MAX_WORK`].
fn outside(a: P, b: P, regions: &[[f64; 4]], work: &mut usize) -> Option<Vec<(f64, f64)>> {
    let mut kept: Vec<(f64, f64)> = vec![(0.0, 1.0)];
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    for r in regions {
        *work += kept.len();
        if *work > MAX_WORK {
            return None;
        }
        // Liang–Barsky: the part of the segment inside `r`, if any.
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut inside = true;
        for (p, q) in [(-dx, a.0 - r[0]), (dx, r[2] - a.0), (-dy, a.1 - r[1]), (dy, r[3] - a.1)] {
            if p == 0.0 {
                inside &= q > 0.0;
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
        if !inside || t0 >= t1 {
            continue;
        }
        kept = kept.into_iter().flat_map(|(s, e)| [(s, e.min(t0)), (s.max(t1), e)]).filter(|(s, e)| e - s > 1e-9).collect();
    }
    Some(kept)
}

/// `p` as [`point`] writes it: a join is decided by the angle the written operands make, and
/// near the miter limit the rounding can turn a bevel into a mitre.
fn rounded(p: P) -> P {
    let r = |v: f64| num(v).as_f64().unwrap_or(v);
    (r(p.0), r(p.1))
}

fn point(op: &str, p: P) -> Op {
    Op::new(op, vec![num(p.0), num(p.1)])
}

/// Put pieces (operators and their device box) into as few paths as can be written without a
/// path's box meeting a region, each ended by `paint`.
fn group(pieces: Vec<(Vec<Op>, [f64; 4])>, regions: &[[f64; 4]], paint: &str, out: &mut Vec<Op>) -> Option<bool> {
    let mut paths: Vec<(Vec<Op>, [f64; 4])> = Vec::new();
    let mut work = 0usize;
    for (ops, b) in pieces {
        work += (paths.len() + 1) * regions.len();
        if meets(regions, b) || work > MAX_WORK {
            return None;
        }
        match paths.iter_mut().find(|(_, pb)| !meets(regions, union(*pb, b))) {
            Some((pops, pb)) => {
                pops.extend(ops);
                *pb = union(*pb, b);
            }
            None => paths.push((ops, b)),
        }
    }
    let any = !paths.is_empty();
    for (ops, _) in paths {
        out.extend(ops);
        out.push(Op::new(paint, vec![]));
    }
    Some(any)
}

/// Rebuild the path `body`, painted by `paint` under `ctm`, from what lies outside `regions`
/// (device space), its stroke reaching `reach` from its outline. `None`: it can't be cut
/// exactly, so it goes whole.
pub(super) fn cut(body: &[Op], paint: &[u8], ctm: &Matrix, regions: &[[f64; 4]], reach: Reach) -> Option<Cut> {
    if body.len() > MAX_OPS {
        return None;
    }
    let rule = match paint {
        b"f" | b"F" | b"B" | b"b" => Some("f"),
        b"f*" | b"B*" | b"b*" => Some("f*"),
        _ => None,
    };
    let strokes = matches!(paint, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
    let mut subs = subpaths(body)?;
    if matches!(paint, b"s" | b"b" | b"b*")
        && let Some(last) = subs.last_mut()
        && !last.closed
    {
        last.closed = true;
        last.ops.push(Op::new("h", vec![]));
        last.segs.push(Seg::Line(last.pts.last().copied().unwrap_or(last.start), last.start));
    }
    let scale = ctm.0[..4].iter().fold(0.0f64, |s, v| s.max(v.abs()));
    if ![reach.all, reach.line, reach.half, reach.miter.unwrap_or(1.0), scale].iter().all(|v| v.is_finite()) {
        return None;
    }
    // A margin the operands' rounding (`num`, 1e-4) can't cross back into a region.
    let margin = 1e-3 + 2e-4 * scale;
    let mut out = Vec::new();
    let fills = match rule {
        Some(rule) => cut_fill(&subs, rule, ctm, regions, margin, &mut out)?,
        None => false,
    };
    let strokes = strokes && cut_stroke(&subs, ctm, regions, reach, margin, &mut out)?;
    Some(Cut { ops: out, fills, strokes })
}

/// The fill of `subs` outside `regions`, appended to `out` ended by `rule`; whether it paints.
fn cut_fill(subs: &[Sub], rule: &str, ctm: &Matrix, regions: &[[f64; 4]], margin: f64, out: &mut Vec<Op>) -> Option<bool> {
    let dev = |p: P| ctm.apply(p.0, p.1);
    // Cut `margin` clear of the regions, and test against half of it: what is cut away
    // then never counts as meeting a region, and the rounding can't take it back there.
    let near: Vec<[f64; 4]> = regions.iter().map(|r| grow(*r, margin / 2.0)).collect();
    let regions: Vec<[f64; 4]> = regions.iter().map(|r| grow(*r, margin)).collect();
    let inverse = ctm.invert()?;
    let mut pieces: Vec<(Vec<Op>, [f64; 4])> = Vec::new();
    for s in subs {
        let b = bbox(s.pts.iter().map(|p| dev(*p)))?;
        if !meets(&near, b) {
            pieces.push((s.ops.clone(), b));
            if pieces.len() > MAX_PIECES {
                return None;
            }
            continue;
        }
        let [x, y, w, h] = s.rect.filter(|_| keeps_axes(ctm))?;
        let d = bbox([dev((x, y)), dev((x + w, y + h))])?;
        for p in subtract(d, &regions, MAX_PIECES.saturating_sub(pieces.len()))? {
            let u = bbox([inverse.apply(p[0], p[1]), inverse.apply(p[2], p[3])])?;
            // Each edge rounded once, so neighbouring pieces write the same edge and no seam
            // opens between them.
            let ((x0, y0), (x1, y1)) = (rounded((u[0], u[1])), rounded((u[2], u[3])));
            // The orientation `re` gave it, so the non-zero rule still counts it the same way.
            let (y, h) = if w * h >= 0.0 { (y0, y1 - y0) } else { (y1, y0 - y1) };
            // Its box as written and painted: under a CTM that only nearly keeps axes, a very
            // wide or tall rectangle's edge leans further than the margin, so it is measured again.
            let b = bbox([(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(dev))?;
            pieces.push((vec![Op::new("re", vec![num(x0), num(y), num(x1 - x0), num(h)])], b));
        }
    }
    if pieces.len() > MAX_PIECES {
        return None;
    }
    // Pieces whose boxes overlap may be inside one another (a hole), so they stay together.
    let mut root: Vec<usize> = (0..pieces.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for i in 0..pieces.len() {
        for j in i + 1..pieces.len() {
            if overlaps(pieces[i].1, pieces[j].1, 0.0) {
                let (a, b) = (find(&mut root, i), find(&mut root, j));
                root[a] = b;
            }
        }
    }
    let mut parts: Vec<(usize, Vec<Op>, [f64; 4])> = Vec::new();
    for (i, (ops, b)) in pieces.into_iter().enumerate() {
        let r = find(&mut root, i);
        match parts.iter_mut().find(|(pr, ..)| *pr == r) {
            Some((_, pops, pb)) => {
                pops.extend(ops);
                *pb = union(*pb, b);
            }
            None => parts.push((r, ops, b)),
        }
    }
    group(parts.into_iter().map(|(_, ops, b)| (ops, b)).collect(), &near, rule, out)
}

/// The stroke of `subs` outside `regions`, appended to `out`; whether it paints.
fn cut_stroke(subs: &[Sub], ctm: &Matrix, regions: &[[f64; 4]], reach: Reach, margin: f64, out: &mut Vec<Op>) -> Option<bool> {
    let dev = |p: P| ctm.apply(p.0, p.1);
    let around = |by: f64| -> Vec<[f64; 4]> { regions.iter().map(|r| grow(*r, by)).collect() };
    // A subpath kept as written keeps all its joins; a cut one keeps a join only at a
    // corner whose miter can't reach a region, and has caps elsewhere.
    let (whole, near) = (around(reach.all + margin / 2.0), around(reach.line + margin / 2.0));
    let marks = regions;
    let regions = around(reach.line + margin);
    let conformal = keeps_angles(ctm);
    // Can a join at `at`, from direction `into` to `out`, stay a join?
    let joinable = |at: P, into: Option<P>, out: Option<P>| {
        let (x, y) = dev(at);
        let by = reach.join(conformal, into, out) + margin / 2.0;
        !marks.iter().any(|r| meets(&[grow(*r, by)], [x, y, x, y]))
    };
    let mut pieces: Vec<(Vec<Op>, [f64; 4])> = Vec::new();
    // Many subpaths crossed by many marks: the budgets are spent as the pieces are made, so
    // the path goes whole before the work is done, not after.
    let mut work = 0usize;
    for s in subs {
        let b = bbox(s.pts.iter().map(|p| dev(*p)))?;
        if !meets(&whole, b) {
            pieces.push((s.ops.clone(), b));
            continue;
        }
        let before = pieces.len();
        if reach.dashed {
            return None;
        }
        // What is kept, in runs: a run goes on through a shared point (keeping its join)
        // while that join can't reach a region and the run's box stays clear of them, and
        // starts again with `m` otherwise. A run holds its operators, its box (device space)
        // and the direction it starts in (`None` for a curve).
        let mut runs: Vec<(Vec<Op>, [f64; 4], Option<P>)> = Vec::new();
        let mut pen: Option<P> = None;
        let mut last: Option<P> = None;
        let mut keep = |from: P, op: Op, pts: &[P], dir: Option<P>, pen: &mut Option<P>, last: &mut Option<P>| -> Option<()> {
            let b = bbox(pts.iter().map(|p| dev(*p)))?;
            let joined = runs.last().map(|(_, rb, _)| union(*rb, b));
            match joined {
                Some(u) if *pen == Some(from) && joinable(from, *last, dir) && !meets(&near, u) => {
                    if let Some((ops, rb, _)) = runs.last_mut() {
                        ops.push(op);
                        *rb = u;
                    }
                }
                _ if before + runs.len() >= MAX_PIECES => return None,
                _ => runs.push((vec![point("m", from), op], b, dir)),
            }
            *pen = pts.last().copied();
            *last = dir;
            Some(())
        };
        for seg in &s.segs {
            match seg {
                Seg::Line(a, e) => {
                    for (t0, t1) in outside(dev(*a), dev(*e), &regions, &mut work)? {
                        let at = |t: f64| rounded(if t == 1.0 { *e } else { (a.0 + t * (e.0 - a.0), a.1 + t * (e.1 - a.1)) });
                        let (p0, p1) = (at(t0), at(t1));
                        // The direction as written, which is what decides a join's miter.
                        let dir = Some((p1.0 - p0.0, p1.1 - p0.1));
                        keep(p0, point("l", p1), &[p0, p1], dir, &mut pen, &mut last)?;
                    }
                    if pen != Some(rounded(*e)) {
                        pen = None;
                    }
                }
                Seg::Curve(a, ctrl, op) => {
                    let cb = bbox(std::iter::once(*a).chain(ctrl.iter().copied()).map(dev))?;
                    if meets(&near, cb) {
                        pen = None;
                    } else {
                        let pts: Vec<P> = std::iter::once(*a).chain(ctrl.iter().copied()).collect();
                        keep(*a, op.clone(), &pts, None, &mut pen, &mut last)?;
                    }
                }
            }
        }
        // A closed subpath comes back to where it began. Kept in one run, it is closed again
        // with `h`, keeping that join; cut somewhere, its last run goes on into its first there,
        // keeping the join too, when it can.
        let start = rounded(s.start);
        let starts_at_start = |run: &(Vec<Op>, [f64; 4], Option<P>)| run.0.first().and_then(|m| m.nums::<2>()) == Some([start.0, start.1]);
        let back =
            s.closed && pen == Some(start) && runs.first().is_some_and(starts_at_start) && joinable(start, last, runs.first().and_then(|r| r.2));
        if back && runs.len() == 1 {
            if let Some((ops, ..)) = runs.first_mut() {
                // The closing segment is drawn by `h` instead of the `l` back to the start.
                if ops.last().is_some_and(|o| o.op.as_slice() == b"l" && o.nums::<2>() == Some([start.0, start.1])) {
                    ops.pop();
                }
                ops.push(Op::new("h", vec![]));
            }
        } else if back && runs.len() > 1 {
            let (first_ops, first_box, first_dir) = runs.remove(0);
            match runs.last_mut() {
                Some((ops, rb, _)) if !meets(&near, union(*rb, first_box)) => {
                    ops.extend(first_ops.into_iter().skip(1));
                    *rb = union(*rb, first_box);
                }
                _ => runs.insert(0, (first_ops, first_box, first_dir)),
            }
        }
        pieces.extend(runs.into_iter().map(|(ops, b, _)| (ops, b)));
    }
    if pieces.len() > MAX_PIECES {
        return None;
    }
    group(pieces, &near, "S", out)
}
