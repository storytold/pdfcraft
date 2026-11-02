//! Perspective (projective) transforms between quadrilaterals: what straightening a photographed
//! page needs. Pure maths in `f64`; every input may be hostile, so degenerate or non-finite
//! quadrilaterals give `None` instead of NaNs.

/// A point `[x, y]`.
pub type Point = [f64; 2];

/// A 3×3 projective transform, row-major, normalised so that the last entry is 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// The transform that takes each of `from` to the point at the same index of `to`. `None`
    /// when three of either set are collinear or a coordinate is not finite.
    pub fn from_points(from: [Point; 4], to: [Point; 4]) -> Option<Self> {
        if from.iter().chain(to.iter()).flatten().any(|v| !v.is_finite()) {
            return None;
        }
        // x' = (h0 x + h1 y + h2) / (h6 x + h7 y + 1), y' likewise with h3..h5: eight unknowns,
        // two equations per point pair.
        let mut m = [[0.0f64; 9]; 8];
        for (i, (&[x, y], &[u, v])) in from.iter().zip(to.iter()).enumerate() {
            m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve8(m)?;
        let out = Self([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0]);
        // Collinear points can leave a solvable but singular system: check that the transform
        // really maps the four points.
        let ok = from.iter().zip(to.iter()).all(|(&p, &q)| {
            out.apply(p).is_some_and(|r| {
                let scale = 1.0 + q[0].abs().max(q[1].abs());
                (r[0] - q[0]).abs() <= 1e-6 * scale && (r[1] - q[1]).abs() <= 1e-6 * scale
            })
        });
        ok.then_some(out)
    }

    /// The image of `p`, or `None` where the transform sends it to infinity.
    pub fn apply(&self, [x, y]: Point) -> Option<Point> {
        let h = &self.0;
        let w = h[6] * x + h[7] * y + h[8];
        if !w.is_finite() || w.abs() < 1e-12 {
            return None;
        }
        let u = (h[0] * x + h[1] * y + h[2]) / w;
        let v = (h[3] * x + h[4] * y + h[5]) / w;
        (u.is_finite() && v.is_finite()).then_some([u, v])
    }
}

/// Gaussian elimination with partial pivoting on an 8×8 system (the 9th column is the right-hand
/// side). `None` when the system is singular.
fn solve8(mut m: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let p = m[col];
        for (r, row) in m.iter_mut().enumerate() {
            if r == col {
                continue;
            }
            let f = row[col] / p[col];
            if f != 0.0 {
                for (a, b) in row.iter_mut().zip(p.iter()) {
                    *a -= f * b;
                }
            }
        }
    }
    let mut out = [0.0; 8];
    for (i, o) in out.iter_mut().enumerate() {
        *o = m[i][8] / m[i][i];
        if !o.is_finite() {
            return None;
        }
    }
    Some(out)
}

/// The four corners in reading order for a y-down space (view or raster pixels): top-left,
/// top-right, bottom-right, bottom-left, whatever order they were given in. `None` unless they
/// form a convex quadrilateral with a non-negligible area.
pub fn order_quad(points: [Point; 4]) -> Option<[Point; 4]> {
    if points.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let cx = points.iter().map(|p| p[0]).sum::<f64>() / 4.0;
    let cy = points.iter().map(|p| p[1]).sum::<f64>() / 4.0;
    let mut q = points;
    // Ascending angle about the centre is clockwise on screen (y down): top, right, bottom, left.
    q.sort_by(|a, b| (a[1] - cy).atan2(a[0] - cx).total_cmp(&(b[1] - cy).atan2(b[0] - cx)));
    let first = (0..4).min_by(|&a, &b| (q[a][0] + q[a][1]).total_cmp(&(q[b][0] + q[b][1])))?;
    q.rotate_left(first);
    // Convex, with every turn the same way, and not a sliver.
    let cross = |o: Point, a: Point, b: Point| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let turns: Vec<f64> = (0..4).map(|i| cross(q[i], q[(i + 1) % 4], q[(i + 2) % 4])).collect();
    let area = turns.iter().sum::<f64>() / 2.0;
    let span = q.iter().map(|p| p[0]).fold(f64::MIN, f64::max) - q.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
    let rise = q.iter().map(|p| p[1]).fold(f64::MIN, f64::max) - q.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
    let size = span.max(rise);
    let convex = turns.iter().all(|&t| t > 1e-9 * size * size);
    (convex && area > 1e-6 * size * size).then_some(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn maps_each_corner_onto_its_target() {
        let from = [[12.0, 30.0], [410.0, 5.0], [430.0, 610.0], [-4.0, 590.0]];
        let to = [[0.0, 0.0], [400.0, 0.0], [400.0, 600.0], [0.0, 600.0]];
        let h = Homography::from_points(from, to).unwrap();
        for (p, q) in from.iter().zip(to.iter()) {
            assert!(close(h.apply(*p).unwrap(), *q), "{p:?} -> {q:?}");
        }
    }

    #[test]
    fn a_rectangle_onto_itself_is_the_identity() {
        let r = [[0.0, 0.0], [10.0, 0.0], [10.0, 20.0], [0.0, 20.0]];
        let h = Homography::from_points(r, r).unwrap();
        assert!(close(h.apply([3.0, 7.0]).unwrap(), [3.0, 7.0]));
    }

    #[test]
    fn the_inverse_takes_points_back() {
        let from = [[50.0, 40.0], [300.0, 70.0], [320.0, 400.0], [30.0, 380.0]];
        let to = [[0.0, 0.0], [200.0, 0.0], [200.0, 300.0], [0.0, 300.0]];
        let (h, back) = (Homography::from_points(from, to).unwrap(), Homography::from_points(to, from).unwrap());
        let p = [120.0, 210.0];
        assert!(close(back.apply(h.apply(p).unwrap()).unwrap(), p));
    }

    #[test]
    fn collinear_or_non_finite_points_have_no_transform() {
        let to = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(Homography::from_points([[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]], to).is_none());
        assert!(Homography::from_points([[0.0, 0.0]; 4], to).is_none());
        assert!(Homography::from_points([[f64::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], to).is_none());
        assert!(Homography::from_points([[f64::INFINITY, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], to).is_none());
    }

    #[test]
    fn corners_come_back_in_reading_order_whatever_the_input_order() {
        let tl = [10.0, 12.0];
        let tr = [200.0, 5.0];
        let br = [210.0, 300.0];
        let bl = [0.0, 290.0];
        for q in [[br, tl, bl, tr], [bl, br, tr, tl], [tl, tr, br, bl], [tr, bl, tl, br]] {
            assert_eq!(order_quad(q), Some([tl, tr, br, bl]), "{q:?}");
        }
    }

    #[test]
    fn concave_degenerate_or_non_finite_quads_are_refused() {
        // A dart: one corner inside the triangle of the other three.
        assert_eq!(order_quad([[0.0, 0.0], [100.0, 0.0], [20.0, 20.0], [0.0, 100.0]]), None);
        assert_eq!(order_quad([[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0]]), None);
        assert_eq!(order_quad([[5.0, 5.0]; 4]), None);
        assert_eq!(order_quad([[f64::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]), None);
    }
}
