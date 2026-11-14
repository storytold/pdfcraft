//! What a painted path really covers, for the hit test of `paint_path`.
//!
//! A path's bounding box is a poor stand-in for what it paints: a stroked frame round a page
//! has a box over the whole page and paints only its four edges, and a map's areas are drawn
//! past the clip that keeps them inside the map. So the current path is also kept as segments
//! in page space, and a path counts as under a mark only when what it paints reaches the mark:
//! for a fill, its outline crosses the mark or the mark lies inside it (by the fill rule); for a
//! stroke, a segment comes within the pen's reach of the mark. Curves are flattened into chords,
//! each grown by how far its curve can stray from it, so the test never answers "no" for paint
//! that is there. Anything it can't follow (an operator without its operands, a coordinate that
//! isn't finite, a path past [`MAX_SEGMENTS`]) leaves the path [`Outline::sure`] false, and the
//! caller falls back to the box, as it does for a path that would cost more than [`MAX_WORK`] to
//! test against the marks its box meets.

use pdfcraft_content::{Matrix, Op};

/// Most segments one path keeps for the test; past it the path is measured by its box.
const MAX_SEGMENTS: usize = 1 << 20;
/// Most point-against-mark tests one path may cost; past it the path is measured by its box.
pub(super) const MAX_WORK: usize = 1 << 26;
/// How far a chord may stray from its curve, in page units, before the curve is split further.
const FLATNESS: f64 = 0.05;
/// Most chords a curve is split into.
const MAX_CHORDS: f64 = 64.0;

/// One subpath: its points in page space, and for each segment from one point to the next how
/// far the painted outline can lie from it (a curve's flattening error; 0 for a line), and the
/// operator that drew it.
#[derive(Default)]
struct Sub {
    pts: Vec<(f64, f64)>,
    pads: Vec<f64>,
    from: Vec<From>,
    /// Closed by `h` (or drawn by `re`): a stroke paints the segment back to its start too.
    closed: bool,
}

/// The operator a segment comes from: its place in the path, and whether it is a chord of a
/// curve. Two chords of one curve meet where the curve is smooth: no join is drawn there.
#[derive(Clone, Copy, PartialEq)]
struct From {
    op: usize,
    curve: bool,
}

/// The current path as segments in page space.
#[derive(Default)]
pub(super) struct Outline {
    subs: Vec<Sub>,
    segments: usize,
    /// How many construction operators the path has had.
    ops: usize,
    broken: bool,
}

impl Outline {
    /// Can the path be measured by its segments? Otherwise the caller uses its box.
    pub(super) fn sure(&self) -> bool {
        !self.broken
    }

    /// How many points the path holds: the cost of one test against a mark.
    pub(super) fn len(&self) -> usize {
        self.segments
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Adds a construction operator, its coordinates taken through `ctm` (taking each operator's
    /// numbers from the end, as `construct_path` and the renderer read them).
    pub(super) fn push(&mut self, op: &Op, ctm: &Matrix) {
        if self.broken {
            return;
        }
        let p = |x: f64, y: f64| ctm.apply(x, y);
        self.ops = self.ops.saturating_add(1);
        match op.op.as_slice() {
            b"m" => match op.nums::<2>() {
                Some([x, y]) => self.move_to(p(x, y)),
                None => self.broken = true,
            },
            b"l" => match op.nums::<2>() {
                Some([x, y]) => self.line_to(p(x, y), 0.0, false),
                None => self.broken = true,
            },
            b"c" => match op.nums::<6>() {
                Some([x1, y1, x2, y2, x3, y3]) => self.curve_to(p(x1, y1), Some(p(x2, y2)), p(x3, y3)),
                None => self.broken = true,
            },
            // `v`: the first control point is the current point.
            b"v" => match op.nums::<4>() {
                Some([x2, y2, x3, y3]) => self.curve_to_v(p(x2, y2), p(x3, y3)),
                None => self.broken = true,
            },
            // `y`: the second control point is the end point.
            b"y" => match op.nums::<4>() {
                Some([x1, y1, x3, y3]) => self.curve_to(p(x1, y1), None, p(x3, y3)),
                None => self.broken = true,
            },
            b"h" => {
                if let Some(s) = self.subs.last_mut() {
                    s.closed = true;
                }
            }
            b"re" => match op.nums::<4>() {
                Some([x, y, w, h]) => {
                    self.move_to(p(x, y));
                    self.line_to(p(x + w, y), 0.0, false);
                    self.line_to(p(x + w, y + h), 0.0, false);
                    self.line_to(p(x, y + h), 0.0, false);
                    if let Some(s) = self.subs.last_mut() {
                        s.closed = true;
                    }
                }
                None => self.broken = true,
            },
            _ => {}
        }
    }

    /// The current point: after `h` (and `re`) the start of the closed subpath (ISO 32000-2
    /// Table 58), otherwise the last point.
    fn current(&self) -> Option<(f64, f64)> {
        let s = self.subs.last()?;
        if s.closed { s.pts.first().copied() } else { s.pts.last().copied() }
    }

    fn add(&mut self, pt: (f64, f64)) -> bool {
        self.segments = self.segments.saturating_add(1);
        if self.segments > MAX_SEGMENTS || !pt.0.is_finite() || !pt.1.is_finite() {
            self.broken = true;
            return false;
        }
        true
    }

    fn move_to(&mut self, pt: (f64, f64)) {
        if self.add(pt) {
            self.subs.push(Sub { pts: vec![pt], ..Sub::default() });
        }
    }

    fn line_to(&mut self, pt: (f64, f64), pad: f64, curve: bool) {
        if !self.add(pt) {
            return;
        }
        let from = From { op: self.ops, curve };
        // After `h` a new segment starts a new subpath at the closed one's start.
        let restart = self.subs.last().and_then(|s| s.closed.then(|| s.pts.first().copied()).flatten());
        match (self.subs.last_mut(), restart) {
            (Some(_), Some(start)) => self.subs.push(Sub { pts: vec![start, pt], pads: vec![pad], from: vec![from], closed: false }),
            (Some(s), None) => {
                s.pts.push(pt);
                s.pads.push(pad);
                s.from.push(from);
            }
            // A segment with no current point: the renderer has nothing to draw it from, so it
            // is taken as a point here (a conservative stand-in).
            (None, _) => self.subs.push(Sub { pts: vec![pt], ..Sub::default() }),
        }
    }

    fn curve_to_v(&mut self, c2: (f64, f64), end: (f64, f64)) {
        let c1 = self.current().unwrap_or(c2);
        self.curve_to(c1, Some(c2), end);
    }

    /// A cubic Bézier from the current point; `c2` is `None` for `y` (the end point).
    fn curve_to(&mut self, c1: (f64, f64), c2: Option<(f64, f64)>, end: (f64, f64)) {
        let c2 = c2.unwrap_or(end);
        // A closed subpath starts again at its start, as `line_to` does.
        let p0 = self.current().unwrap_or(c1);
        // The second differences bound the curve's bend: n even chords stray from it by at
        // most 3/4·M/n² (|B''| ≤ 6·M, a chord over Δt strays by Δt²/8·max|B''|).
        let d = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| (a.0 - 2.0 * b.0 + c.0).hypot(a.1 - 2.0 * b.1 + c.1);
        let bend = d(p0, c1, c2).max(d(c1, c2, end));
        if !bend.is_finite() || !c1.0.is_finite() || !c1.1.is_finite() || !c2.0.is_finite() || !c2.1.is_finite() {
            self.broken = true;
            return;
        }
        // Finite and clamped to [1, 64], so the cast is exact.
        let n = (0.75 * bend / FLATNESS).sqrt().ceil().clamp(1.0, MAX_CHORDS);
        let pad = 0.75 * bend / (n * n);
        let steps = n as usize;
        for i in 1..=steps {
            let t = i as f64 / n;
            let u = 1.0 - t;
            let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            let pt = (a * p0.0 + b * c1.0 + c * c2.0 + e * end.0, a * p0.1 + b * c1.1 + c * c2.1 + e * end.1);
            self.line_to(pt, pad, true);
            if self.broken {
                return;
            }
        }
    }

    /// Every segment of the path with its pad, and for each subpath the segment back to its
    /// start when `close(index, sub)` says it is painted.
    fn segments<'a>(&'a self, close: impl Fn(usize, &Sub) -> bool + 'a) -> impl Iterator<Item = ((f64, f64), (f64, f64), f64)> + 'a {
        self.subs.iter().enumerate().flat_map(move |(i, s)| {
            let open = s.pts.windows(2).zip(s.pads.iter()).filter_map(|(w, &pad)| Some((*w.first()?, *w.get(1)?, pad)));
            // A lone point is a segment of no length: a stroke's cap or a dot can still paint it.
            let lone = (s.pts.len() == 1).then(|| s.pts.first().map(|&p| (p, p, 0.0))).flatten();
            let back = (s.pts.len() > 1 && close(i, s)).then(|| Some((*s.pts.last()?, *s.pts.first()?, 0.0))).flatten();
            open.chain(lone).chain(back)
        })
    }

    /// Does a fill of the path reach `r` (closed, page space)? `even_odd` picks the fill rule.
    pub(super) fn fill_hits(&self, r: [f64; 4], even_odd: bool) -> bool {
        // A fill closes every subpath.
        if self.segments(|_, _| true).any(|(a, b, pad)| meets(a, b, grow(r, pad))) {
            return true;
        }
        // No edge comes near `r`, so all of it is inside the fill or none: its centre decides.
        let c = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let mut winding = 0i64;
        for (a, b, _) in self.segments(|_, _| true) {
            if (a.1 <= c.1) != (b.1 <= c.1) {
                let x = a.0 + (c.1 - a.1) / (b.1 - a.1) * (b.0 - a.0);
                if x > c.0 {
                    winding = winding.saturating_add(if b.1 > a.1 { 1 } else { -1 });
                }
            }
        }
        if even_odd { winding % 2 != 0 } else { winding != 0 }
    }

    /// Does a stroke of the path drawn with `pen` reach `r`? `close_last` is set for `s`, `b` and
    /// `b*`, which close the last subpath before stroking it.
    pub(super) fn stroke_hits(&self, r: [f64; 4], pen: Pen, close_last: bool) -> bool {
        let last = self.subs.len().saturating_sub(1);
        let closed = |i: usize, s: &Sub| s.closed || (close_last && i == last);
        if self.segments(closed).any(|(a, b, pad)| meets(a, b, grow(r, pen.half + pad))) {
            return true;
        }
        // A miter join reaches past the pen at the corner (ISO 32000-2 §8.4.3.4, §8.4.3.5).
        let Some(limit) = pen.miter else { return false };
        self.subs.iter().enumerate().any(|(i, s)| {
            let n = s.pts.len();
            let around = closed(i, s) && n >= 2;
            // The corners: every point between two segments, and where a painted closing
            // segment meets the first and the last.
            (0..n).any(|k| {
                let (prev, next) = match k {
                    0 if !around => return false,
                    0 => (n - 1, 1 % n),
                    k if k + 1 == n && !around => return false,
                    k if k + 1 == n => (k - 1, 0),
                    k => (k - 1, k + 1),
                };
                let (Some(&p), Some(&v), Some(&q)) = (s.pts.get(prev), s.pts.get(k), s.pts.get(next)) else { return false };
                // The segments in and out of the corner (the closing segment, a line, is none).
                let (into, out) = (k.checked_sub(1).and_then(|j| s.from.get(j)), s.from.get(k));
                if into.is_some() && into == out && into.is_some_and(|f| f.curve) {
                    // Inside one curve: no join, and the segments' pads cover the curve.
                    return false;
                }
                // A join at a curve's end takes its angle from the curve's tangent, which the
                // chords only approximate; and an angle in page space isn't the one the miter
                // limit is tested on (in user space) when the CTM doesn't keep angles. Either way
                // the miter may reach as far as the limit lets it.
                let curved = into.is_some_and(|f| f.curve) || out.is_some_and(|f| f.curve);
                let ratio = if curved || !pen.conformal { limit.max(1.0) } else { corner(p, v, q, limit) };
                let pad = s.pads.get(prev).copied().unwrap_or(0.0).max(s.pads.get(k).copied().unwrap_or(0.0));
                meets(v, v, grow(r, ratio * pen.half + pad))
            })
        })
    }
}

/// How the current pen paints round a path, in page space.
#[derive(Clone, Copy)]
pub(super) struct Pen {
    /// How far from a segment the line (its caps included) can paint.
    pub(super) half: f64,
    /// The miter limit when joins are mitred, `None` for round or bevelled joins (which reach no
    /// further than `half`).
    pub(super) miter: Option<f64>,
    /// The CTM keeps angles (a rotation and a uniform scale, perhaps mirrored), so a corner's
    /// angle in page space is its angle in user space, where the miter limit applies.
    pub(super) conformal: bool,
}

/// How far, in half line widths, a miter join at `v` between `p`-`v` and `v`-`q` reaches from
/// `v`: `1/sin(φ/2)` at a corner of angle φ, or 1 where that passes `limit` and the corner is
/// bevelled. A segment of no length gives no direction, so the limit is taken.
fn corner(p: (f64, f64), v: (f64, f64), q: (f64, f64), limit: f64) -> f64 {
    let (ux, uy) = (v.0 - p.0, v.1 - p.1);
    let (wx, wy) = (q.0 - v.0, q.1 - v.1);
    let (lu, lw) = (ux.hypot(uy), wx.hypot(wy));
    if !(lu > 0.0 && lw > 0.0) {
        return limit.max(1.0);
    }
    // The cosine of the turn between the two directions; sin(φ/2) = cos(turn/2) = √((1 + cos)/2).
    let cos = ((ux * wx + uy * wy) / (lu * lw)).clamp(-1.0, 1.0);
    let half_sin = ((1.0 + cos) / 2.0).sqrt();
    let k = if half_sin > 0.0 { 1.0 / half_sin } else { f64::INFINITY };
    if k.is_finite() && k <= limit { k.max(1.0) } else { 1.0 }
}

fn grow(r: [f64; 4], d: f64) -> [f64; 4] {
    [r[0] - d, r[1] - d, r[2] + d, r[3] + d]
}

/// Does the segment `a`–`b` meet the closed box `r`? (Liang–Barsky.)
fn meets(a: (f64, f64), b: (f64, f64), r: [f64; 4]) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a.0 - r[0]), (dx, r[2] - a.0), (-dy, a.1 - r[1]), (dy, r[3] - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            if t > t1 {
                return false;
            }
            t0 = t0.max(t);
        } else {
            if t < t0 {
                return false;
            }
            t1 = t1.min(t);
        }
    }
    t0 <= t1
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_content::num;

    fn pen(half: f64) -> Pen {
        Pen { half, miter: None, conformal: true }
    }

    fn outline(ops: &[(&str, &[f64])]) -> Outline {
        let mut o = Outline::default();
        for (name, args) in ops {
            o.push(&Op::new(name, args.iter().map(|&v| num(v)).collect()), &Matrix::IDENTITY);
        }
        o
    }

    #[test]
    fn a_frame_paints_its_edges_not_its_middle() {
        let o = outline(&[("re", &[10.0, 10.0, 100.0, 100.0])]);
        let middle = [40.0, 40.0, 60.0, 60.0];
        assert!(!o.stroke_hits(middle, pen(1.0), false));
        assert!(o.fill_hits(middle, false) && o.fill_hits(middle, true));
        assert!(o.stroke_hits([105.0, 40.0, 120.0, 60.0], pen(1.0), false), "across the right edge");
        assert!(o.stroke_hits([111.5, 40.0, 120.0, 60.0], pen(2.0), false), "within the pen's reach");
    }

    #[test]
    fn an_open_path_strokes_no_closing_segment_unless_closed() {
        // A U open at the top: its closing segment would run along y = 100.
        let u = [("m", &[0.0, 100.0][..]), ("l", &[0.0, 0.0]), ("l", &[100.0, 0.0]), ("l", &[100.0, 100.0])];
        let o = outline(&u);
        let top = [40.0, 95.0, 60.0, 105.0];
        assert!(!o.stroke_hits(top, pen(1.0), false));
        assert!(o.stroke_hits(top, pen(1.0), true), "`s` closes it");
        assert!(o.fill_hits(top, false), "a fill closes it");
    }

    #[test]
    fn even_odd_leaves_the_hole_and_nonzero_fills_it() {
        let o = outline(&[("re", &[0.0, 0.0, 100.0, 100.0]), ("re", &[20.0, 20.0, 60.0, 60.0])]);
        let hole = [40.0, 40.0, 60.0, 60.0];
        assert!(o.fill_hits(hole, false));
        assert!(!o.fill_hits(hole, true));
    }

    #[test]
    fn a_curve_is_never_missed_by_its_chords() {
        // A quarter circle of radius 100 about the origin, as a Bézier. A small box round the
        // curve's midpoint lies outside the chords there (they cut inside the arc by ~0.04).
        let k = 55.228_475;
        let o = outline(&[("m", &[100.0, 0.0]), ("c", &[100.0, k, k, 100.0, 0.0, 100.0])]);
        let mid = 0.125 * 100.0 + 0.375 * 100.0 + 0.375 * k; // B(1/2): (p0 + 3·c1 + 3·c2 + p3) / 8
        let on_arc = [mid - 0.005, mid - 0.005, mid + 0.005, mid + 0.005];
        assert!(o.stroke_hits(on_arc, pen(0.0), false));
        assert!(!o.stroke_hits([10.0, 10.0, 20.0, 20.0], pen(1.0), false));
    }

    #[test]
    fn a_miter_reaches_past_the_pen_only_at_its_corners() {
        // A right-angled corner at (100, 100): its miter tip is √2 half-widths out, toward the
        // upper right; along the edges the pen reaches one half-width.
        let o = outline(&[("m", &[0.0, 100.0]), ("l", &[100.0, 100.0]), ("l", &[100.0, 0.0])]);
        let mitred = Pen { half: 10.0, miter: Some(10.0), conformal: true };
        let past_the_corner = [111.0, 111.0, 120.0, 120.0];
        assert!(!o.stroke_hits(past_the_corner, pen(10.0), false), "round joins stop at the pen");
        assert!(o.stroke_hits(past_the_corner, mitred, false), "the miter tip reaches 14.1");
        assert!(!o.stroke_hits(past_the_corner, Pen { half: 10.0, miter: Some(1.4), conformal: true }, false), "bevelled past the limit");
        // Beside an edge, away from the corner, the miter adds nothing.
        assert!(!o.stroke_hits([40.0, 111.0, 60.0, 120.0], mitred, false));
        // A hairpin is bevelled at any finite limit.
        let pin = outline(&[("m", &[0.0, 0.0]), ("l", &[100.0, 0.0]), ("l", &[0.0, 1.0])]);
        assert!(!pin.stroke_hits([112.0, -5.0, 120.0, 5.0], mitred, false));
        // A closed square's first corner is a join too.
        let sq = outline(&[("re", &[0.0, 0.0, 100.0, 100.0])]);
        assert!(sq.stroke_hits([-12.0, -12.0, -11.0, -11.0], mitred, false));
        assert!(!sq.stroke_hits([-12.0, -12.0, -11.0, -11.0], pen(10.0), false));
    }

    /// B(1/2) of the cubic `p0`, `c1`, `c2`, `p3`.
    fn mid(p0: (f64, f64), c1: (f64, f64), c2: (f64, f64), p3: (f64, f64)) -> [f64; 4] {
        let x = (p0.0 + 3.0 * c1.0 + 3.0 * c2.0 + p3.0) / 8.0;
        let y = (p0.1 + 3.0 * c1.1 + 3.0 * c2.1 + p3.1) / 8.0;
        [x - 0.01, y - 0.01, x + 0.01, y + 0.01]
    }

    #[test]
    fn a_v_after_a_closed_subpath_starts_at_its_start() {
        // After `h` (and after `re`) the current point is the subpath's start, and `v` takes it
        // as its first control point too.
        let o = outline(&[("m", &[0.0, 0.0]), ("l", &[100.0, 0.0]), ("l", &[100.0, 100.0]), ("h", &[]), ("v", &[0.0, 100.0, 100.0, 0.0])]);
        let on_curve = mid((0.0, 0.0), (0.0, 0.0), (0.0, 100.0), (100.0, 0.0));
        assert!(o.stroke_hits(on_curve, pen(0.5), false), "{on_curve:?}");
        let o = outline(&[("re", &[10.0, 10.0, 80.0, 80.0]), ("v", &[10.0, 200.0, 200.0, 10.0])]);
        let on_curve = mid((10.0, 10.0), (10.0, 10.0), (10.0, 200.0), (200.0, 10.0));
        assert!(o.stroke_hits(on_curve, pen(0.5), false), "{on_curve:?}");
    }

    #[test]
    fn a_join_at_a_curve_or_under_a_stretching_ctm_takes_the_miter_limit() {
        // A line into a curve whose tangent at its start turns back sharply: the chords bend
        // gently there, the curve's tangent does not.
        // Its tangent there turns 120°, a 60° corner whose miter tip reaches (108.66, -5); its
        // first chords turn back further, a corner sharp enough to be bevelled at a limit of 3.
        let o = outline(&[("m", &[0.0, 0.0]), ("l", &[100.0, 0.0]), ("c", &[99.5, 0.866, 0.0, 0.0, -100.0, 0.0])]);
        let beyond = [107.5, -5.5, 108.5, -4.5];
        assert!(o.stroke_hits(beyond, Pen { half: 5.0, miter: Some(3.0), conformal: true }, false));
        // Under a CTM that doesn't keep angles, any corner may reach the limit.
        let l = outline(&[("m", &[0.0, 0.0]), ("l", &[100.0, 0.0]), ("l", &[100.0, 100.0])]);
        let off = [118.0, -19.0, 119.0, -18.0];
        assert!(!l.stroke_hits(off, Pen { half: 2.0, miter: Some(10.0), conformal: true }, false));
        assert!(l.stroke_hits(off, Pen { half: 2.0, miter: Some(10.0), conformal: false }, false));
        // Two chords of one curve meet where it is smooth: no miter there.
        let k = 55.228_475;
        let arc = outline(&[("m", &[100.0, 0.0]), ("c", &[100.0, k, k, 100.0, 0.0, 100.0])]);
        assert!(!arc.stroke_hits([75.0, 75.0, 76.0, 76.0], Pen { half: 1.0, miter: Some(10.0), conformal: true }, false));
    }

    #[test]
    fn what_it_cannot_follow_makes_it_unsure() {
        assert!(!outline(&[("m", &[1.0])]).sure());
        assert!(!outline(&[("m", &[f64::INFINITY, 0.0])]).sure());
        assert!(!outline(&[("m", &[0.0, 0.0]), ("c", &[f64::NAN, 0.0, 1.0, 1.0, 2.0, 2.0])]).sure());
        assert!(outline(&[("m", &[0.0, 0.0]), ("l", &[1.0, 1.0])]).sure());
    }
}
