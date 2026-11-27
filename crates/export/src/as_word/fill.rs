//! Compound PDF fills (several subpaths, or the even-odd rule) resolved before they reach readers
//! with different path fill rules: trapezoids that don't overlap, each a closed path of one
//! freeform. The original outline is stroked separately.

use super::layout_number;
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
}

fn charge(work: &mut usize, n: usize) -> Option<()> {
    *work = work.checked_sub(n)?;
    Some(())
}

/// Split at vertices and intersections, where the left-to-right edge order can change.
/// Bounded work and output prevent hostile compound paths growing quadratically without limit.
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
            let (a, b) = (a.map(layout_number), b.map(layout_number));
            if a[1] != b[1] {
                edges.push(Edge { a, b });
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
    levels.sort_by(f64::total_cmp);
    levels.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
    let mut out = Vec::new();
    for pair in levels.windows(2) {
        let [low, high] = pair else { continue };
        let middle = (low + high) / 2.0;
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
                && edge.x(middle) - left.x(middle) > 1e-8
            {
                if out.len() >= max_points / 4 {
                    return None;
                }
                out.push([[left.x(*low), *low], [edge.x(*low), *low], [edge.x(*high), *high], [left.x(*high), *high]]);
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
}
