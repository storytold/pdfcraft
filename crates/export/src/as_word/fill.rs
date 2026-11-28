//! Compound PDF fills (several subpaths, or the even-odd rule) resolved before they reach readers
//! with different path fill rules: trapezoids that don't overlap, each a closed path of one
//! freeform. The original outline is stroked separately.

use super::LAYOUT_LIMIT;
use crate::layout::Shape;

#[derive(Clone, Copy)]
struct Edge {
    a: [f64; 2],
    b: [f64; 2],
}
impl Edge {
    fn x(self, y: f64) -> f64 {
        self.a[0] + (y - self.a[1]) / (self.b[1] - self.a[1]) * (self.b[0] - self.a[0])
    }
    fn low(self) -> f64 {
        self.a[1].min(self.b[1])
    }
    fn high(self) -> f64 {
        self.a[1].max(self.b[1])
    }
    /// Where the edge crosses the vertical line at `x`, if it does between its ends.
    fn y_at(self, x: f64) -> Option<f64> {
        let (da, db) = (self.a[0] - x, self.b[0] - x);
        (da * db < 0.0).then(|| self.a[1] + da / (da - db) * (self.b[1] - self.a[1])).filter(|y| y.is_finite())
    }
}

fn charge(work: &mut usize, n: usize) -> Option<()> {
    *work = work.checked_sub(n)?;
    Some(())
}

/// Split at vertices and intersections, where the left-to-right edge order can change.
/// Bounded work and output prevent hostile compound paths growing quadratically without limit.
///
/// The fill is worked out from the path's own points and then cut to what a shape can place
/// (0 to [`LAYOUT_LIMIT`] each way, which holds every page): moving a far-off vertex into that
/// range first would turn the edges that lead to it, and change the fill on the page. The bands
/// are also split where an edge crosses the range's sides, so each trapezoid is cut exactly.
/// `None` for a path it can't work out within the work, the output or finite numbers.
pub(super) fn trapezoids(shape: &Shape, work: &mut usize, max_points: usize) -> Option<Vec<[[f64; 2]; 4]>> {
    let count = shape.paths.iter().try_fold(0usize, |n, (p, _)| n.checked_add(p.len()))?;
    charge(work, count)?;
    if count > max_points {
        return None;
    }
    let mut edges = Vec::new();
    let mut levels = Vec::new();
    for (points, _) in &shape.paths {
        if points.len() < 3 {
            continue;
        }
        if !points.iter().flatten().all(|n| n.is_finite()) {
            return None;
        }
        for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
            if a[1] != b[1] {
                edges.push(Edge { a: *a, b: *b });
                levels.extend([a[1], b[1]]);
            }
        }
    }
    for (i, a) in edges.iter().enumerate() {
        for b in edges.iter().skip(i + 1) {
            charge(work, 1)?;
            let (low, high) = (a.low().max(b.low()), a.high().min(b.high()));
            if low >= high {
                continue;
            }
            let (dl, dh) = (a.x(low) - b.x(low), a.x(high) - b.x(high));
            if dl * dh < 0.0 {
                let y = low + (high - low) * dl / (dl - dh);
                if y.is_finite() {
                    levels.push(y);
                }
                if levels.len() > max_points {
                    return None;
                }
            }
        }
    }
    // The range's own sides, and where an edge crosses its left or right one: within a band an
    // edge is then wholly inside or wholly outside, and holding it to the range moves nothing.
    charge(work, edges.len())?;
    levels.extend([0.0, LAYOUT_LIMIT]);
    levels.extend(edges.iter().flat_map(|e| [e.y_at(0.0), e.y_at(LAYOUT_LIMIT)]).flatten());
    levels.sort_by(f64::total_cmp);
    levels.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
    let inside = |x: f64| x.clamp(0.0, LAYOUT_LIMIT);
    let mut out = Vec::new();
    for pair in levels.windows(2) {
        let [low, high] = pair else { continue };
        let middle = (low + high) / 2.0;
        if !(0.0..=LAYOUT_LIMIT).contains(&middle) {
            continue;
        }
        charge(work, edges.len())?;
        let mut active: Vec<_> = edges.iter().filter(|e| e.low() < middle && middle < e.high()).collect();
        active.sort_by(|a, b| a.x(middle).total_cmp(&b.x(middle)));
        let mut winding = 0i64;
        let mut left: Option<&Edge> = None;
        for edge in active {
            let before = if shape.even_odd { winding % 2 != 0 } else { winding != 0 };
            winding += if edge.b[1] > edge.a[1] { 1 } else { -1 };
            let after = if shape.even_odd { winding % 2 != 0 } else { winding != 0 };
            if !before && after {
                left = Some(edge);
            }
            if before
                && !after
                && let Some(left) = left.take()
            {
                let corners = [[left.x(*low), *low], [edge.x(*low), *low], [edge.x(*high), *high], [left.x(*high), *high]];
                if !(corners.iter().flatten().all(|n| n.is_finite()) && left.x(middle).is_finite() && edge.x(middle).is_finite()) {
                    return None;
                }
                // Nothing of a span left or right of the range is on a page.
                if inside(edge.x(middle)) - inside(left.x(middle)) <= 1e-8 {
                    continue;
                }
                if out.len() >= max_points / 4 {
                    return None;
                }
                out.push(corners.map(|[x, y]| [inside(x), y]));
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn square(min: f64, max: f64) -> (Vec<[f64; 2]>, bool) {
        (vec![[min, min], [max, min], [max, max], [min, max]], true)
    }
    #[test]
    fn review_followup_compound_fill_respects_winding_and_intersections() {
        for (even_odd, opposite, area) in [(true, false, 84.0), (false, true, 84.0), (false, false, 100.0)] {
            let mut inner = square(3.0, 7.0);
            if opposite {
                inner.0.reverse();
            }
            let shape = Shape { paths: vec![square(0.0, 10.0), inner], fill: Some([0.0; 3]), stroke: None, even_odd };
            let polys = trapezoids(&shape, &mut 10000, 1000).unwrap();
            let painted: f64 = polys.iter().map(|p| ((p[1][0] - p[0][0]) + (p[2][0] - p[3][0])) * (p[2][1] - p[1][1]) / 2.0).sum();
            assert!((painted - area).abs() < 0.001, "{even_odd}, {opposite}: {painted}");
            assert!(trapezoids(&shape, &mut 1, 1000).is_none());
            assert!(trapezoids(&shape, &mut 10000, 4).is_none());
        }
        let bow = Shape {
            paths: vec![(vec![[0.0, 0.0], [10.0, 10.0], [0.0, 10.0], [10.0, 0.0]], true)],
            fill: Some([0.0; 3]),
            stroke: None,
            even_odd: true,
        };
        let polys = trapezoids(&bow, &mut 10000, 1000).unwrap();
        assert_eq!(polys.len(), 2, "intersection splits the two lobes");
        assert_eq!(polys[0][2][1], 5.0);
    }

    /// The fill's right edge (its widest point) at height `y`.
    fn reach(polys: &[[[f64; 2]; 4]], y: f64) -> f64 {
        polys
            .iter()
            .filter(|p| p[0][1] <= y && y <= p[2][1])
            .map(|p| {
                let t = (y - p[1][1]) / (p[2][1] - p[1][1]);
                p[1][0] + t * (p[2][0] - p[1][0])
            })
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// A fill that reaches far off the page keeps its edges where the PDF draws them on the
    /// page: bringing its far corner into Word's range first turned the edge that leads to it.
    #[test]
    fn review_a_fill_reaching_off_the_page_keeps_its_edges_on_the_page() {
        let far = 3000.0;
        let shape = Shape { paths: vec![(vec![[0.0, 0.0], [far, 100.0], [0.0, 100.0]], true)], fill: Some([0.0; 3]), stroke: None, even_odd: true };
        let polys = trapezoids(&shape, &mut 10000, 1000).unwrap();
        // The slanted edge passes through (150, 5) and (1500, 50), as in the PDF.
        for y in [5.0, 50.0] {
            let expected = far * y / 100.0;
            assert!((reach(&polys, y) - expected).abs() < 0.001, "at {y}: {} for {expected}", reach(&polys, y));
        }
        // Past Word's range the fill is cut, not squeezed in.
        assert!((reach(&polys, 90.0) - LAYOUT_LIMIT).abs() < 0.001);
        assert!(polys.iter().flatten().all(|p| (0.0..=LAYOUT_LIMIT).contains(&p[0])), "{polys:?}");
    }

    /// A fill wholly off the page draws nothing, and numbers too large to work with are an
    /// omission the page reports, never a wrong shape.
    #[test]
    fn review_fills_off_the_page_or_beyond_finite_numbers_draw_nothing() {
        let square = |x: f64, size: f64| (vec![[x, 10.0], [x + size, 10.0], [x + size, 20.0], [x, 20.0]], true);
        let mut shape = Shape { paths: vec![square(-500.0, 100.0), square(-300.0, 100.0)], fill: Some([0.0; 3]), stroke: None, even_odd: false };
        assert_eq!(trapezoids(&shape, &mut 10000, 1000), Some(Vec::new()));
        shape.paths = vec![(vec![[-1e308, 0.0], [1e308, 50.0], [-1e308, 100.0]], true), square(10.0, 10.0)];
        assert_eq!(trapezoids(&shape, &mut 10000, 1000), None);
    }
}
