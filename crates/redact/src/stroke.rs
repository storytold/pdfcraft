//! How far a stroke's mitered joins reach past its path. Shared by the walk and the proof, as the
//! content parser and `Matrix` are: it is geometry, not a reading of the page.

use pdfcraft_content::Op;

/// The initial miter limit (ISO 32000-2 Table 51).
pub(crate) const INITIAL_MITER_LIMIT: f64 = 10.0;

/// Whether the line join `j` (or an ExtGState's `/LJ`) sets is mitered: 0, and any value but the
/// round 1 and the bevelled 2, as the renderer reads it (its integer part).
pub(crate) fn join_miters(j: f64) -> bool {
    !matches!(j as i64, 1 | 2)
}

/// How far the mitered joins of the path `ops` reach from their corners, as a multiple of half the
/// line width: `1/sin(φ/2)` at a corner of angle φ, where that is within the miter `limit`. A
/// corner past the limit is bevelled (ISO 32000-2 §8.4.3.5), and reaches no further than half the
/// width. A join is where two segments of a subpath meet, by the tangents at the end of one and the
/// start of the next, and where `h` closes one (`closes`: the painting operator closes the last,
/// as `s`, `b` and `b*` do). A curve has none inside it, and a segment of no length none of its
/// own: the segments on either side of it are joined. Where the renderer may take two points for
/// one ([`near`]), each tangent it could take is weighed, and segments on either side of one it
/// could take for none are joined too: the reach is as far as any of them. 1 when no corner is
/// mitered.
pub(crate) fn miter_reach(ops: &[Op], closes: bool, limit: f64) -> f64 {
    // The renderer keeps the limit as a 32-bit float, which can round it up past the value written
    // by up to a part in sixteen million (4.3 is 4.30000019 to it), and reckons the corner its own
    // way: a corner a millionth past the limit is taken for mitered.
    let limit = limit * (1.0 + 1e-6);
    let mut joins = Joins { limit, reach: 1.0, start: (0.0, 0.0), at: (0.0, 0.0), first: Vec::new(), last: Vec::new(), opening: true };
    // After `re` the renderer starts a segment from the point before the rectangle, not from its
    // corner (vendored `hayro-interpret`, until #6968): the corners such a segment makes aren't
    // the ones read here, so a path with one may mitre anywhere, as far as the limit lets.
    let mut after_rectangle = false;
    for op in ops {
        if after_rectangle && matches!(op.op.as_slice(), b"l" | b"c" | b"v" | b"y") {
            joins.reach = joins.reach.max(limit);
        }
        // Each operator's points are its last operands, as the renderer takes them; one whose
        // operands aren't numbers is skipped. Where the content parser reads a number the renderer
        // doesn't (`1.75e2`), the renderer drops the operands before it and skips the operator
        // after it, which this still reads: the two lexers disagree there, as everywhere on a page.
        match op.op.as_slice() {
            b"m" => {
                if let Some([x, y]) = op.nums::<2>() {
                    joins.begin((x, y));
                    after_rectangle = false;
                }
            }
            b"l" => {
                if let Some([x, y]) = op.nums::<2>() {
                    joins.segment(&[(x, y)]);
                }
            }
            b"c" => {
                if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>() {
                    joins.segment(&[(x1, y1), (x2, y2), (x3, y3)]);
                }
            }
            // `v` and `y` give one control point, the other being where the curve starts (`v`) or
            // ends (`y`): it leaves towards the one given and arrives from it.
            b"v" | b"y" => {
                if let Some([x, y, x3, y3]) = op.nums::<4>() {
                    joins.segment(&[(x, y), (x3, y3)]);
                }
            }
            // A rectangle's corners are right angles, which reach no further than a projecting cap;
            // it ends where it started, and a segment after it starts a new subpath there.
            b"re" => {
                if let Some([x, y, _, _]) = op.nums::<4>() {
                    joins.begin((x, y));
                    after_rectangle = true;
                }
            }
            b"h" => joins.close(),
            _ => {}
        }
    }
    if closes {
        joins.close();
    }
    joins.reach
}

/// How far apart two coordinates may be and still be one to the renderer, in the units of the
/// space they are in. It reads a coordinate's digits only so far (`100.00000000000001` is 100 to
/// it), and takes a curve's control point closer than about a millionth of a unit to its end for
/// that end (both measured): this is ten times that.
const NEAR: f64 = 1e-5;

/// Whether the renderer may take the points `p` and `q` for one ([`NEAR`]).
fn near(p: (f64, f64), q: (f64, f64)) -> bool {
    (p.0 - q.0).abs().max((p.1 - q.1).abs()) <= NEAR
}

/// Whether the renderer may take an extent from `lo` to `hi` for none ([`NEAR`]).
pub(crate) fn flat(lo: f64, hi: f64) -> bool {
    hi - lo <= NEAR
}

/// The most directions a join weighs on either side. Each segment the renderer may take for none
/// adds those of the next to them; past this many, every corner there is taken to reach as far as
/// the limit lets one.
const MAX_DIRECTIONS: usize = 16;

/// The joins of a path, met one segment at a time.
struct Joins {
    limit: f64,
    /// The furthest reach of a mitered join so far.
    reach: f64,
    /// Where the subpath started, which `h` closes it to, and the current point.
    start: (f64, f64),
    at: (f64, f64),
    /// The directions the subpath may leave its start in, and arrive at the current point in, as
    /// [`Joins::segment`] finds them: none until a segment has a length.
    first: Vec<(f64, f64)>,
    last: Vec<(f64, f64)>,
    /// Whether the renderer may take every segment of the subpath so far for none, so that where
    /// the next one leaves to is where the subpath may leave its start.
    opening: bool,
}

impl Joins {
    fn begin(&mut self, p: (f64, f64)) {
        (self.start, self.at, self.opening) = (p, p, true);
        self.first.clear();
        self.last.clear();
    }

    /// A segment from the current point through the control points `to` to the last of them: a
    /// line gives that one point, a curve its control points.
    fn segment(&mut self, to: &[(f64, f64)]) {
        let Some(&end) = to.last() else { return };
        let from = std::mem::replace(&mut self.at, end);
        // A curve leaves towards its first control point that isn't where it starts, and arrives
        // from its last that isn't where it ends; where one is near, the renderer may take it or
        // the next, so each is weighed, up to one that isn't near.
        let mut leaves = Vec::new();
        for p in to {
            leaves.extend(away(from, *p));
            if !near(from, *p) {
                break;
            }
        }
        let mut arrives = Vec::new();
        for p in to.iter().rev().chain([&from]) {
            arrives.extend(away(*p, end));
            if !near(*p, end) {
                break;
            }
        }
        if leaves.is_empty() || arrives.is_empty() {
            return;
        }
        self.join(&leaves);
        // One the renderer may take for none leaves the directions before it standing, and the
        // subpath may still leave its start by the next.
        let none = to.iter().all(|p| near(from, *p));
        if self.opening {
            self.first.extend(&leaves);
            self.opening = none;
        }
        if !none {
            self.last.clear();
        }
        self.last.extend(arrives);
        for directions in [&mut self.first, &mut self.last] {
            if directions.len() > MAX_DIRECTIONS {
                directions.truncate(MAX_DIRECTIONS);
                self.reach = self.reach.max(self.limit);
            }
        }
    }

    /// The joins of each direction the path may arrive in with each of `leaves`.
    fn join(&mut self, leaves: &[(f64, f64)]) {
        for arrives in &self.last {
            for leaves in leaves {
                let (a, b) = (arrives.0.hypot(arrives.1), leaves.0.hypot(leaves.1));
                let cos = arrives.0 / a * (leaves.0 / b) + arrives.1 / a * (leaves.1 / b);
                // The corner's angle φ is π less the turn between the two, so sin(φ/2) is
                // √((1 + cos)/2). A turn right back has no miter: its ratio, infinite (or no
                // number, rounded past it), is within no limit.
                let ratio = 1.0 / ((1.0 + cos) / 2.0).sqrt();
                if ratio <= self.limit {
                    self.reach = self.reach.max(ratio);
                }
            }
        }
    }

    /// `h`: a line back to the start, if it has a length, joined to where the subpath left from. A
    /// segment after it starts a new subpath there.
    fn close(&mut self) {
        let start = self.start;
        self.segment(&[start]);
        let first = std::mem::take(&mut self.first);
        self.join(&first);
        self.begin(start);
    }
}

/// The direction from `a` to `b`, if they differ.
fn away(a: (f64, f64), b: (f64, f64)) -> Option<(f64, f64)> {
    Some((b.0 - a.0, b.1 - a.1)).filter(|d| d.0 != 0.0 || d.1 != 0.0)
}
