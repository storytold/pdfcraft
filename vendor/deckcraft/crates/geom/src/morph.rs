//! Morph between two outlines: one shape turns into another (a circle into a star).
//!
//! The approach is manim's: both outlines get the same number of contours and each contour pair the
//! same number of cubic curves, then every control point moves in a straight line. On top of that,
//! closed contours are turned and reversed so that their points travel as short a way as possible
//! (a triangle does not twist on its way into a circle), and extra points go to the longest curves.

use kurbo::{BezPath, PathEl, Point, Rect, Vec2};

use crate::preset::{FillMode, Geometry, SubPath};

/// A contour as cubic curves: the start anchor, then three points per curve (handle, handle,
/// anchor).
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    pub closed: bool,
    pub pts: Vec<Point>,
}

impl Contour {
    pub fn curves(&self) -> usize {
        self.pts.len().saturating_sub(1) / 3
    }
}

fn line_to(p: &mut Vec<Point>, to: Point) {
    let Some(&from) = p.last() else { return };
    p.extend([from.lerp(to, 1.0 / 3.0), from.lerp(to, 2.0 / 3.0), to]);
}

fn close(c: &mut Contour) {
    if let (Some(&first), Some(&last)) = (c.pts.first(), c.pts.last())
        && first != last
    {
        line_to(&mut c.pts, first);
    }
    c.closed = true;
}

fn pen(cur: &mut Option<Contour>, start: Point) -> &mut Contour {
    cur.get_or_insert_with(|| Contour { closed: false, pts: vec![start] })
}

fn keep(c: Contour, out: &mut Vec<Contour>) {
    if c.curves() > 0 && c.pts.iter().all(|p| p.is_finite()) {
        out.push(c);
    }
}

/// The contours of a path, every segment as a cubic curve. Contours without a curve or with
/// non-finite points are left out.
pub fn contours(path: &BezPath) -> Vec<Contour> {
    let mut out = Vec::new();
    let mut cur: Option<Contour> = None;
    let mut start = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if let Some(c) = cur.take() {
                    keep(c, &mut out);
                }
                start = p;
                cur = Some(Contour { closed: false, pts: vec![p] });
            }
            PathEl::LineTo(p) => line_to(&mut pen(&mut cur, start).pts, p),
            PathEl::QuadTo(q, p) => {
                let c = pen(&mut cur, start);
                if let Some(&a) = c.pts.last() {
                    c.pts.extend([a.lerp(q, 2.0 / 3.0), p.lerp(q, 2.0 / 3.0), p]);
                }
            }
            PathEl::CurveTo(a, b, p) => pen(&mut cur, start).pts.extend([a, b, p]),
            PathEl::ClosePath => {
                if let Some(mut c) = cur.take()
                    && c.curves() > 0
                {
                    close(&mut c);
                    keep(c, &mut out);
                }
            }
        }
    }
    if let Some(c) = cur {
        keep(c, &mut out);
    }
    out
}

/// Contours back to a path.
pub fn to_path(cs: &[Contour]) -> BezPath {
    let mut bp = BezPath::new();
    for c in cs {
        let Some(&s) = c.pts.first() else { continue };
        bp.move_to(s);
        for [a, b, p] in c.pts.get(1..).unwrap_or(&[]).as_chunks::<3>().0 {
            bp.curve_to(*a, *b, *p);
        }
        if c.closed {
            bp.close_path();
        }
    }
    bp
}

// --- alignment ---------------------------------------------------------------------------------

/// Rough curve length: mean of chord and control polygon.
fn curve_length(c: &[Point]) -> f64 {
    let [p0, p1, p2, p3] = c else { return 0.0 };
    (p0.distance(*p3) + p0.distance(*p1) + p1.distance(*p2) + p2.distance(*p3)) / 2.0
}

/// Appends curve `c` (four points) split into `n` pieces of equal parameter length (de Casteljau),
/// without its start anchor.
fn split_curve(c: &[Point], n: usize, out: &mut Vec<Point>) {
    let [a, b, d, e] = c else { return };
    let (mut p0, mut p1, mut p2, p3) = (*a, *b, *d, *e);
    for j in (2..=n).rev() {
        let t = 1.0 / j as f64;
        let (ab, bc, cd) = (p0.lerp(p1, t), p1.lerp(p2, t), p2.lerp(p3, t));
        let (abc, bcd) = (ab.lerp(bc, t), bc.lerp(cd, t));
        let m = abc.lerp(bcd, t);
        out.extend([ab, abc, m]);
        (p0, p1, p2) = (m, bcd, cd);
    }
    out.extend([p1, p2, p3]);
}

/// A piece length ordered totally, so it can sit in a heap.
#[derive(PartialEq)]
struct Share(f64);
impl Eq for Share {}
impl PartialOrd for Share {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Share {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0)
    }
}

/// Brings a contour to `n` curves. Unlike manim (which splits every curve equally often), the extra
/// splits go to the longest pieces, so a long side gets more points than a short one.
fn refine(c: &Contour, n: usize) -> Contour {
    let have = c.curves();
    if have >= n || have == 0 {
        return c.clone();
    }
    let curves: Vec<&[Point]> = (0..have).filter_map(|k| c.pts.get(3 * k..3 * k + 4)).collect();
    let len: Vec<f64> = curves.iter().map(|q| curve_length(q) + 1e-9).collect();
    let mut parts = vec![1usize; curves.len()];
    // The curve with the longest piece gets the next split (the lowest index on a tie); a heap keeps
    // this O(n log n) instead of rescanning every curve for each split.
    let mut heap: std::collections::BinaryHeap<(Share, std::cmp::Reverse<usize>)> =
        len.iter().enumerate().map(|(k, l)| (Share(*l), std::cmp::Reverse(k))).collect();
    for _ in 0..n - have {
        let Some((_, std::cmp::Reverse(best))) = heap.pop() else { break };
        if let Some(p) = parts.get_mut(best) {
            *p += 1;
            heap.push((Share(len.get(best).copied().unwrap_or(0.0) / *p as f64), std::cmp::Reverse(best)));
        }
    }
    let mut out = Vec::with_capacity(1 + 3 * n);
    out.extend(c.pts.first().copied());
    for (q, p) in curves.iter().zip(&parts) {
        split_curve(q, *p, &mut out);
    }
    Contour { closed: c.closed, pts: out }
}

const MIN_CURVES: usize = 12;
const MAX_CURVES: usize = 60;
/// Start points tried per closed contour; longer contours are sampled evenly.
const MAX_TRIES: usize = 128;

/// How many curves a contour pair gets: a common multiple of both counts while that stays small
/// (then a triangle meets a circle at matching places and the morph stays symmetric), and at least
/// [`MIN_CURVES`] for a smooth middle.
fn curves_for(na: usize, nb: usize) -> usize {
    fn gcd(x: usize, y: usize) -> usize {
        if y == 0 { x } else { gcd(y, x % y) }
    }
    let hi = na.max(nb).max(1);
    let mut n = if na > 0 && nb > 0 { (na / gcd(na, nb)).saturating_mul(nb) } else { hi };
    if n > MAX_CURVES {
        n = hi;
    }
    n.saturating_mul(MIN_CURVES.div_ceil(n).max(1))
}

/// Box around all points, handles included.
fn bounds<'a>(pts: impl IntoIterator<Item = &'a Point>) -> Option<Rect> {
    pts.into_iter().fold(None, |r: Option<Rect>, p| Some(r.map_or(Rect::from_points(*p, *p), |r| r.union_pt(*p))))
}

fn center(pts: &[Point]) -> Point {
    bounds(pts).map(|r| r.center()).unwrap_or(Point::ZERO)
}

/// The same contour, starting at anchor `s`, optionally run backwards.
fn turn(pts: &[Point], s: usize, back: bool) -> Vec<Point> {
    let q: Vec<Point> = if back { pts.iter().rev().copied().collect() } else { pts.to_vec() };
    if s == 0 {
        return q;
    }
    let body = q.get(1..).unwrap_or(&[]);
    let cut = (3 * s).min(body.len());
    let mut out = Vec::with_capacity(q.len());
    out.extend(cut.checked_sub(1).and_then(|i| body.get(i)).copied());
    out.extend_from_slice(body.get(cut..).unwrap_or(&[]));
    out.extend_from_slice(body.get(..cut).unwrap_or(&[]));
    out
}

/// One contour pair of equal curve count: `b` runs backwards if `back` is set, then the closed side
/// (`b` if it is closed, else `a`) gets the start that lets the points travel least. Both sides are
/// compared around their own centre, so where the shapes are does not matter.
fn match_pair(a: &Contour, b: &Contour, back: bool) -> (Contour, Contour, f64) {
    let bp = if back { turn(&b.pts, 0, true) } else { b.pts.clone() };
    let turn_a = !b.closed && a.closed;
    let (fixed, free) = if turn_a { (&bp, &a.pts) } else { (&a.pts, &bp) };
    let (fc, gc) = (center(fixed), center(free));
    let tries = if turn_a || b.closed { free.len().saturating_sub(1) / 3 } else { 1 };
    let mut best = free.clone();
    let mut best_cost = f64::INFINITY;
    for s in (0..tries.max(1)).step_by(tries.div_ceil(MAX_TRIES).max(1)) {
        let q = turn(free, s, false);
        let mut cost = 0.0;
        for (p, f) in q.iter().zip(fixed.iter()) {
            cost += ((*p - gc) - (*f - fc)).hypot2();
            if cost >= best_cost {
                break;
            }
        }
        if cost < best_cost {
            best_cost = cost;
            best = q;
        }
    }
    let (pa, pb) = if turn_a { (best, bp) } else { (a.pts.clone(), best) };
    (Contour { closed: a.closed, pts: pa }, Contour { closed: b.closed, pts: pb }, best_cost)
}

/// Where a contour without partner shrinks to (or grows from): the spot at the same place relative
/// to the other outline's box, or its own centre without one.
fn place(c: &Contour, from: Option<Rect>, to: Option<Rect>) -> Point {
    let p = center(&c.pts);
    let (Some(from), Some(to)) = (from, to) else { return p };
    let fx = if from.width() > 0.0 { (p.x - from.x0) / from.width() } else { 0.5 };
    let fy = if from.height() > 0.0 { (p.y - from.y0) / from.height() } else { 0.5 };
    Point::new(to.x0 + to.width() * fx, to.y0 + to.height() * fy)
}

/// Two contour lists brought to the same number of contours and the same number of curves per
/// contour pair. Contours pair up in order; one without partner meets a point (see [`place`];
/// `ba`/`bb` are the boxes of the whole outlines). Whether `b` runs backwards is decided once for all
/// its contours: turning single ones around would change which areas a nonzero fill covers.
pub fn align(a: &[Contour], b: &[Contour], ba: Option<Rect>, bb: Option<Rect>) -> (Vec<Contour>, Vec<Contour>) {
    // (a, b, either side a point).
    let mut pairs: Vec<(Contour, Contour, bool)> = Vec::new();
    for k in 0..a.len().max(b.len()) {
        let (ca, cb) = (a.get(k), b.get(k));
        let n = curves_for(ca.map_or(0, Contour::curves), cb.map_or(0, Contour::curves));
        let point = |c: &Contour, from, to, closed| Contour { closed, pts: vec![place(c, from, to); 1 + 3 * n] };
        match (ca, cb) {
            (Some(x), Some(y)) => pairs.push((refine(x, n), refine(y, n), false)),
            (Some(x), None) => pairs.push((refine(x, n), point(x, ba, bb, x.closed), true)),
            (None, Some(y)) => pairs.push((point(y, bb, ba, y.closed), refine(y, n), true)),
            (None, None) => {}
        }
    }
    let mut best: Option<(Vec<Contour>, Vec<Contour>, f64)> = None;
    for back in [false, true] {
        let (mut xs, mut ys, mut cost) = (Vec::new(), Vec::new(), 0.0);
        for (x, y, point) in &pairs {
            if *point {
                xs.push(x.clone());
                ys.push(if back { Contour { closed: y.closed, pts: turn(&y.pts, 0, true) } } else { y.clone() });
            } else {
                let (mx, my, c) = match_pair(x, y, back);
                xs.push(mx);
                ys.push(my);
                cost += c;
            }
        }
        if best.as_ref().is_none_or(|b| cost < b.2) {
            best = Some((xs, ys, cost));
        }
    }
    best.map(|(x, y, _)| (x, y)).unwrap_or_default()
}

// --- whole geometries ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    fill: FillMode,
    stroke: bool,
    even_odd: bool,
}

#[derive(Clone, Debug)]
struct Part {
    a: Option<Look>,
    b: Option<Look>,
    ca: Vec<Contour>,
    cb: Vec<Contour>,
}

/// Two shape geometries brought to the same structure once, then blended per frame with
/// [`GeometryMorph::at`]. Sub-paths pair up in order; one without partner grows out of (or shrinks
/// into) a point. When a pair differs in fill mode, outline or fill rule, the two looks cross-fade
/// (see [`Fade`]).
#[derive(Clone, Debug)]
pub struct GeometryMorph {
    parts: Vec<Part>,
    text: (Rect, Rect),
}

fn look(s: &SubPath) -> Look {
    Look { fill: s.fill, stroke: s.stroke, even_odd: s.even_odd }
}

/// How strongly a sub-path of a morphed geometry shows: its fill and its outline, 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub fill: f64,
    pub stroke: f64,
}

/// The layers a pair of looks is drawn with at `t`, bottom first. A fill or an outline only one side
/// has fades out (or in). When both fill, the side with the even-odd rule lies opaque underneath and
/// the other fades over it, so a donut's hole fills smoothly; with the same rule both cover the same
/// area, so this is a plain cross-fade (shading of 3-D faces).
fn layers(a: Option<Look>, b: Option<Look>, t: f64) -> Vec<(Look, Fade)> {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) if a != b => (a, b),
        (Some(x), _) | (None, Some(x)) => return vec![(x, Fade { fill: 1.0, stroke: 1.0 })],
        (None, None) => return vec![],
    };
    let side = |on_a: bool, on_b: bool| match (on_a, on_b) {
        (true, true) => 1.0,
        (true, false) => 1.0 - t,
        (false, true) => t,
        (false, false) => 0.0,
    };
    let (stroke, line) = (a.stroke || b.stroke, side(a.stroke, b.stroke));
    let (fa, fb) = (a.fill != FillMode::None, b.fill != FillMode::None);
    if fa && fb {
        let (under, over, alpha) = if a.even_odd || !b.even_odd { (a, b, t) } else { (b, a, 1.0 - t) };
        vec![(Look { stroke, ..under }, Fade { fill: 1.0, stroke: line }), (Look { stroke: false, ..over }, Fade { fill: alpha, stroke: 0.0 })]
    } else {
        let filled = if fa { a } else { b };
        vec![(Look { stroke, ..filled }, Fade { fill: side(fa, fb), stroke: line })]
    }
}

impl GeometryMorph {
    /// `a` and `b` each in their own shape-local space.
    pub fn new(a: &Geometry, b: &Geometry) -> Self {
        let ca: Vec<Vec<Contour>> = a.paths.iter().map(|s| contours(&s.path)).collect();
        let cb: Vec<Vec<Contour>> = b.paths.iter().map(|s| contours(&s.path)).collect();
        let ba = bounds(ca.iter().flatten().flat_map(|c| &c.pts));
        let bb = bounds(cb.iter().flatten().flat_map(|c| &c.pts));
        let mut parts = Vec::new();
        for k in 0..a.paths.len().max(b.paths.len()) {
            let (xa, xb) = (ca.get(k).map(Vec::as_slice).unwrap_or(&[]), cb.get(k).map(Vec::as_slice).unwrap_or(&[]));
            let (pa, pb) = align(xa, xb, ba, bb);
            parts.push(Part { a: a.paths.get(k).map(look), b: b.paths.get(k).map(look), ca: pa, cb: pb });
        }
        GeometryMorph { parts, text: (a.text_rect, b.text_rect) }
    }

    /// The geometry at `t` (0..1), with a [`Fade`] for each of its sub-paths. The points of `a` are
    /// first scaled by `sa` and those of `b` by `sb`, to bring both into the current box.
    pub fn at(&self, t: f64, sa: Vec2, sb: Vec2) -> (Geometry, Vec<Fade>) {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        let scale = |p: Point, s: Vec2| Point::new(p.x * s.x, p.y * s.y);
        let (mut paths, mut fades) = (Vec::new(), Vec::new());
        for part in &self.parts {
            let cs: Vec<Contour> = part
                .ca
                .iter()
                .zip(&part.cb)
                .map(|(x, y)| Contour {
                    closed: x.closed && y.closed,
                    pts: x.pts.iter().zip(&y.pts).map(|(p, q)| scale(*p, sa).lerp(scale(*q, sb), t)).collect(),
                })
                .collect();
            let path = to_path(&cs);
            for (lk, fade) in layers(part.a, part.b, t) {
                paths.push(SubPath { path: path.clone(), fill: lk.fill, stroke: lk.stroke, even_odd: lk.even_odd });
                fades.push(fade);
            }
        }
        let r = |r: Rect, s: Vec2| Rect::new(r.x0 * s.x, r.y0 * s.y, r.x1 * s.x, r.y1 * s.y);
        let (ta, tb) = (r(self.text.0, sa), r(self.text.1, sb));
        let text_rect = Rect::from_points(ta.origin().lerp(tb.origin(), t), Point::new(ta.x1, ta.y1).lerp(Point::new(tb.x1, tb.y1), t));
        (Geometry { paths, text_rect, handles: vec![], sites: vec![] }, fades)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::build;
    use kurbo::Shape as _;

    fn bbox(g: &Geometry) -> Rect {
        g.outline().bounding_box()
    }

    fn near(a: Rect, b: Rect) -> bool {
        (a.x0 - b.x0).abs() < 1e-6 && (a.y0 - b.y0).abs() < 1e-6 && (a.x1 - b.x1).abs() < 1e-6 && (a.y1 - b.y1).abs() < 1e-6
    }

    #[test]
    fn contours_turn_lines_and_quads_into_cubics() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((3.0, 0.0));
        p.quad_to((3.0, 3.0), (0.0, 3.0));
        p.close_path();
        // After a close, drawing goes on from the start point.
        p.line_to((5.0, 5.0));
        p.move_to((9.0, 9.0));
        let cs = contours(&p);
        assert_eq!(cs.len(), 2, "{cs:?}");
        assert!(cs[0].closed && !cs[1].closed);
        // line, quad and the closing line.
        assert_eq!(cs[0].curves(), 3);
        assert_eq!(cs[0].pts[1], Point::new(1.0, 0.0));
        assert_eq!(cs[0].pts.last(), cs[0].pts.first());
        let end = Point::new(5.0, 5.0);
        assert_eq!(cs[1].pts, vec![Point::ZERO, Point::ZERO.lerp(end, 1.0 / 3.0), Point::ZERO.lerp(end, 2.0 / 3.0), end]);
        // The lone move_to at the end makes no contour, and the path comes back the same.
        assert_eq!(contours(&to_path(&cs)), cs);
    }

    #[test]
    fn refine_many_curves_finishes_at_the_asked_count() {
        let mut c = Contour { closed: false, pts: vec![Point::ZERO] };
        for i in 1..=20_000 {
            line_to(&mut c.pts, Point::new(f64::from(i), f64::from(i % 7)));
        }
        let r = refine(&c, 40_000);
        assert_eq!(r.curves(), 40_000);
    }

    #[test]
    fn refine_splits_the_longest_curves_and_keeps_the_shape() {
        let c = Contour { closed: false, pts: vec![Point::ZERO, Point::new(1.0, 0.0), Point::new(2.0, 0.0), Point::new(3.0, 0.0)] };
        let mut long = c.clone();
        line_to(&mut long.pts, Point::new(3.0, 30.0));
        let r = refine(&long, 5);
        assert_eq!(r.curves(), 5);
        // The short first side stays one curve, the long one gets the other four.
        assert_eq!(r.pts[3], Point::new(3.0, 0.0));
        assert_eq!(r.pts.last(), Some(&Point::new(3.0, 30.0)));
        assert!(r.pts.iter().all(|p| (p.x - 3.0).abs() < 1e-9 || p.y == 0.0));
        assert_eq!(refine(&c, 1), c);
        assert_eq!(curves_for(3, 4), 12);
        assert_eq!(curves_for(4, 4), 12);
        // The common multiple 77 is too many: the larger count, doubled to reach the minimum.
        assert_eq!(curves_for(7, 11), 22);
        assert_eq!(curves_for(0, 0), 12);
    }

    #[test]
    fn ellipse_turns_into_a_star_and_lands_exactly() {
        let (a, b) = (build("ellipse", 100.0, 100.0, &[]).unwrap(), build("star5", 200.0, 100.0, &[]).unwrap());
        let m = GeometryMorph::new(&a, &b);
        let one = Vec2::new(1.0, 1.0);
        assert!(near(bbox(&m.at(0.0, one, one).0), bbox(&a)));
        assert!(near(bbox(&m.at(1.0, one, one).0), bbox(&b)));
        let mid = m.at(0.5, one, one).0;
        assert_eq!(mid.paths.len(), 1);
        assert!(mid.paths[0].path.elements().iter().all(|e| e.end_point().is_none_or(|p| p.is_finite())));
        // Non-finite progress is the start.
        assert!(near(bbox(&m.at(f64::NAN, one, one).0), bbox(&a)));
    }

    #[test]
    fn triangle_does_not_twist_into_a_circle() {
        let (a, b) = (build("triangle", 100.0, 100.0, &[]).unwrap(), build("ellipse", 100.0, 100.0, &[]).unwrap());
        let ca = contours(&a.paths[0].path);
        let cb = contours(&b.paths[0].path);
        let (x, y) = align(&ca, &cb, None, None);
        // Every point travels less than the shapes are wide: no point crosses to the other side.
        let far = x[0].pts.iter().zip(&y[0].pts).map(|(p, q)| p.distance(*q)).fold(0.0, f64::max);
        assert!(far < 60.0, "{far}");
    }

    #[test]
    fn extra_sub_paths_grow_out_of_a_point() {
        let (a, b) = (build("rect", 100.0, 100.0, &[]).unwrap(), build("cube", 100.0, 100.0, &[]).unwrap());
        assert!(b.paths.len() > 1);
        let m = GeometryMorph::new(&a, &b);
        let one = Vec2::new(1.0, 1.0);
        let start = m.at(0.0, one, one).0;
        assert_eq!(start.paths.len(), b.paths.len());
        for sp in start.paths.iter().skip(1) {
            let r = sp.path.bounding_box();
            assert!(r.width() < 1e-9 && r.height() < 1e-9, "{r:?}");
        }
        // Looks switch at the midpoint: the cube's shaded faces show from there on.
        assert_eq!(m.at(1.0, one, one).0.paths.iter().map(|s| s.fill).collect::<Vec<_>>(), b.paths.iter().map(|s| s.fill).collect::<Vec<_>>());
        // And the other way round, the faces shrink away.
        let back = GeometryMorph::new(&b, &a).at(1.0, one, one).0;
        assert!(back.paths.iter().skip(1).all(|sp| sp.path.bounding_box().area() < 1e-9));
    }

    #[test]
    fn differing_looks_cross_fade() {
        let one = Vec2::new(1.0, 1.0);
        let fades = |a: &Geometry, b: &Geometry, t: f64| {
            let (g, f) = GeometryMorph::new(a, b).at(t, one, one);
            g.paths.iter().zip(f).map(|(s, f)| (s.fill, s.even_odd, s.stroke, f.fill, f.stroke)).collect::<Vec<_>>()
        };
        // A filled rectangle into a line: the fill fades out, the outline stays.
        let (r, line) = (build("rect", 100.0, 100.0, &[]).unwrap(), build("line", 100.0, 0.0, &[]).unwrap());
        assert_eq!(fades(&r, &line, 0.25), [(FillMode::Norm, false, true, 0.75, 1.0)]);
        assert_eq!(fades(&line, &r, 0.25), [(FillMode::Norm, false, true, 0.25, 1.0)]);
        // A donut into a circle: the donut (even-odd) lies underneath, the circle fades in over its hole.
        let (d, e) = (build("donut", 100.0, 100.0, &[]).unwrap(), build("ellipse", 100.0, 100.0, &[]).unwrap());
        assert!(d.paths[0].even_odd && !e.paths[0].even_odd);
        assert_eq!(fades(&d, &e, 0.25), [(FillMode::Norm, true, true, 1.0, 1.0), (FillMode::Norm, false, false, 0.25, 0.0)]);
        assert_eq!(fades(&e, &d, 0.25), [(FillMode::Norm, true, true, 1.0, 1.0), (FillMode::Norm, false, false, 0.75, 0.0)]);
        // Same looks: one layer, fully there.
        assert_eq!(fades(&e, &r, 0.5), [(FillMode::Norm, false, true, 1.0, 1.0)]);
    }

    #[test]
    fn scales_bring_both_sides_into_the_current_box() {
        let (a, b) = (build("rect", 10.0, 10.0, &[]).unwrap(), build("ellipse", 40.0, 20.0, &[]).unwrap());
        let m = GeometryMorph::new(&a, &b);
        // Current box 20 x 20: a scaled up twice, b to half the width.
        let g = m.at(0.0, Vec2::new(2.0, 2.0), Vec2::new(0.5, 1.0)).0;
        assert!(near(bbox(&g), Rect::new(0.0, 0.0, 20.0, 20.0)));
        assert!(near(g.text_rect, Rect::new(a.text_rect.x0 * 2.0, a.text_rect.y0 * 2.0, a.text_rect.x1 * 2.0, a.text_rect.y1 * 2.0)));
        let g = m.at(1.0, Vec2::new(2.0, 2.0), Vec2::new(0.5, 1.0)).0;
        assert!(near(bbox(&g), Rect::new(0.0, 0.0, 20.0, 20.0)));
    }

    #[test]
    fn empty_and_degenerate_geometries_do_not_crash() {
        let empty = Geometry { paths: vec![], text_rect: Rect::ZERO, handles: vec![], sites: vec![] };
        let r = build("rect", 10.0, 10.0, &[]).unwrap();
        let line = build("line", 10.0, 0.0, &[]).unwrap();
        let mut nan = BezPath::new();
        nan.move_to((f64::NAN, 0.0));
        nan.line_to((1.0, 1.0));
        let bad = Geometry { paths: vec![SubPath { path: nan, fill: FillMode::Norm, stroke: true, even_odd: false }], ..empty.clone() };
        for (x, y) in [(&empty, &empty), (&empty, &r), (&r, &empty), (&r, &line), (&line, &r), (&bad, &r), (&r, &bad)] {
            for t in [0.0, 0.5, 1.0, f64::INFINITY] {
                let g = GeometryMorph::new(x, y).at(t, Vec2::new(1.0, 1.0), Vec2::new(0.0, 1e300)).0;
                // At most two layers per sub-path pair.
                assert!(g.paths.len() <= 2 * x.paths.len().max(y.paths.len()));
            }
        }
        // A rectangle into a line: the line is open, so the morph ends open.
        let g = GeometryMorph::new(&r, &line).at(1.0, Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0)).0;
        assert!(!g.paths[0].path.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
    }

    #[test]
    fn long_contours_sample_their_start_points() {
        // A polygon with many corners still aligns (and quickly): the start search is capped.
        let mut p = BezPath::new();
        for k in 0..2000 {
            let a = k as f64 / 2000.0 * std::f64::consts::TAU;
            let q = (50.0 + 50.0 * a.cos(), 50.0 + 50.0 * a.sin());
            if k == 0 { p.move_to(q) } else { p.line_to(q) }
        }
        p.close_path();
        let many = contours(&p);
        let e = contours(&build("ellipse", 100.0, 100.0, &[]).unwrap().paths[0].path);
        let (x, y) = align(&many, &e, None, None);
        assert_eq!(x[0].curves(), y[0].curves());
        assert_eq!(x[0].curves(), 2000);
    }
}
