//! Bounded extraction clips. Only single convex quadrilaterals are recognized: these include
//! transformed form BBoxes. Unknown paths keep the previous constraint and a matching pop.
use kurbo::{BezPath, PathEl, Point, Vec2};
use std::sync::Arc;

// Forms already have a nesting limit; explicit path clips need their own bounds.
// Each intersection has four passes over at most MAX_VERTICES points, bounding work per
// push. MAX_DEPTH also bounds retained polygons to about 4 MiB. Restored sibling clips
// do not increase live complexity, so neither their count nor the glyph count is limited.
const MAX_DEPTH: usize = 256;
const MAX_VERTICES: usize = 4 * (MAX_DEPTH + 1);
pub(super) const LIMIT_ERROR: &str = "text clipping limit exceeded";

#[derive(Default)]
pub(super) struct TextClips {
    stack: Vec<Option<Arc<[Point]>>>,
    pub(super) exceeded: bool,
}

impl TextClips {
    pub(super) fn push(&mut self, path: &BezPath) {
        if self.exceeded || self.stack.len() >= MAX_DEPTH {
            self.exceeded = true;
            return;
        }
        let previous = self.stack.last().cloned().flatten();
        let Some(quad) = quadrilateral(path) else {
            self.stack.push(previous);
            return;
        };
        let region = match previous.as_deref() {
            None => Some(quad),
            Some(old) if old.is_empty() || quad.is_empty() => Some(Vec::new()),
            Some(old) => match intersect(old, &quad) {
                Ok(region) => region,
                Err(()) => {
                    self.exceeded = true;
                    return;
                }
            },
        };
        // Nonfinite arithmetic is uncertain, not evidence of an empty intersection.
        self.stack.push(region.map(|p| Arc::from(p.into_boxed_slice())).or(previous));
    }

    pub(super) fn pop(&mut self) {
        self.stack.pop();
    }

    pub(super) fn visible(&self, quad: &[Point; 4]) -> bool {
        if self.exceeded {
            return false;
        }
        let Some(Some(region)) = self.stack.last().cloned() else {
            return true;
        };
        if region.is_empty() {
            return false;
        }
        // Separating axes of both convex polygons distinguish the real transformed geometry
        // from its enclosing rectangle. Strict separation retains boundary contact and lines.
        if edges(&region).any(|(a, b)| outside(a, b, quad, 1.0)) {
            return false;
        }
        let winding = winding(quad);
        if winding.is_finite() && winding != 0.0 {
            let sign = winding.signum();
            if edges(quad).any(|(a, b)| outside(a, b, &region, sign)) {
                return false;
            }
        }
        true
    }
}

fn edges(points: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    points.iter().copied().zip(points.iter().copied().cycle().skip(1)).take(points.len())
}

fn normalized_edge(a: Point, b: Point) -> Vec2 {
    let edge = b - a;
    let scale = edge.x.abs().max(edge.y.abs());
    if scale == 0.0 {
        return edge;
    }
    let normalized = edge / scale;
    // Do not erase a direction component or give it subnormal relative precision.
    // Raw products remain usable for extreme axis ratios; callers reject nonfinite results.
    if (edge.x != 0.0 && normalized.x.abs() < f64::MIN_POSITIVE) || (edge.y != 0.0 && normalized.y.abs() < f64::MIN_POSITIVE) {
        edge
    } else {
        normalized
    }
}

// Normalize the edge once. Signed distances then retain tiny finite bounds instead of
// multiplying two tiny lengths. The error bound follows those products, not page units.
fn side(edge: Vec2, offset: Vec2) -> (f64, f64) {
    let left = edge.x * offset.y;
    let right = edge.y * offset.x;
    (left - right, 32.0 * f64::EPSILON * (left.abs() + right.abs()))
}

fn cross(a: Point, b: Point, p: Point) -> f64 {
    side(normalized_edge(a, b), p - a).0
}

fn outside(a: Point, b: Point, points: &[Point], sign: f64) -> bool {
    let edge = normalized_edge(a, b);
    points.iter().all(|&p| {
        let (distance, tolerance) = side(edge, p - a);
        let distance = sign * distance;
        distance.is_finite() && distance < -tolerance
    })
}

// Only the winding sign is needed. Convex triangle heights share that sign, without
// the extra length factor that makes the usual area sum underflow for tiny polygons.
fn winding(points: &[Point]) -> f64 {
    let Some(&origin) = points.first() else {
        return 0.0;
    };
    edges(points).map(|(a, b)| cross(origin, a, b)).sum()
}

fn quadrilateral(path: &BezPath) -> Option<Vec<Point>> {
    use PathEl::{ClosePath, LineTo, MoveTo};
    let [a, b, c, d] = match path.elements() {
        [MoveTo(a), LineTo(b), LineTo(c), LineTo(d), ClosePath] => [*a, *b, *c, *d],
        [MoveTo(a), LineTo(b), LineTo(c), LineTo(d), LineTo(end), ClosePath] if a == end => [*a, *b, *c, *d],
        _ => return None,
    };
    let points = [a, b, c, d];
    if !points.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        return None;
    }
    let turns = [cross(a, b, c), cross(b, c, d), cross(c, d, a), cross(d, a, b)];
    if !turns.iter().all(|v| v.is_finite()) {
        return None;
    }
    if !(turns.iter().all(|&v| v >= 0.0) || turns.iter().all(|&v| v <= 0.0)) {
        return None;
    }
    if turns.iter().all(|&v| v == 0.0) {
        // A collapsed form rectangle repeats its endpoints. Other apparent collinearity can
        // be floating-point underflow; do not mistake that uncertainty for an empty clip.
        let collapsed = (a == b && c == d) || (a == d && b == c) || points.iter().all(|p| p.x == a.x) || points.iter().all(|p| p.y == a.y);
        return collapsed.then(Vec::new);
    }
    let signed = winding(&points);
    if !signed.is_finite() || signed == 0.0 {
        return None;
    }
    let mut result = points.to_vec();
    if signed < 0.0 {
        result.reverse();
    }
    Some(result)
}

fn intersect(subject: &[Point], clip: &[Point]) -> Result<Option<Vec<Point>>, ()> {
    let mut result = subject.to_vec();
    for (a, b) in edges(clip) {
        let edge = normalized_edge(a, b);
        let mut next = Vec::with_capacity(result.len().saturating_add(1));
        for (start, end) in edges(&result) {
            let (s, s_tolerance) = side(edge, start - a);
            let (e, e_tolerance) = side(edge, end - a);
            if !s.is_finite() || !e.is_finite() {
                return Ok(None);
            }
            let inside_start = s >= -s_tolerance;
            let inside_end = e >= -e_tolerance;
            if inside_start != inside_end {
                // Interpolate from the nearer endpoint to avoid cancelling a tiny boundary
                // coordinate when the other endpoint is many orders of magnitude farther away.
                let (origin, other, near, far) = if s.abs() <= e.abs() { (start, end, s, e) } else { (end, start, e, s) };
                let fraction = near / (near - far);
                if !fraction.is_finite() {
                    return Ok(None);
                }
                let p = origin + (other - origin) * fraction.clamp(0.0, 1.0);
                if !p.x.is_finite() || !p.y.is_finite() {
                    return Ok(None);
                }
                next.push(p);
            }
            if inside_end {
                next.push(end);
            }
        }
        next.dedup();
        if next.len() > 1 && next.first() == next.last() {
            next.pop();
        }
        // Roundoff must not invalidate the storage bound even if it creates extra vertices.
        if next.len() > MAX_VERTICES {
            return Err(());
        }
        result = next;
    }
    let signed = winding(&result);
    if !signed.is_finite() {
        return Ok(None);
    }
    if result.len() < 3 {
        result.clear();
    } else if signed == 0.0 {
        return Ok(None);
    }
    Ok(Some(result))
}
