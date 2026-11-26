//! Rectangles with smoothed ("squircle") corners, as design tools offer next to the corner radius
//! (#286). A plain rounded corner joins the straight edge with a sudden change of curvature; a
//! smoothed corner starts curving earlier, along a cubic Bézier, and keeps a shorter circular arc
//! in the middle, so the outline flows into the edge.
//!
//! For a corner of radius `r` and smoothing `s` (0 = circular, 1 = no arc left), the corner takes
//! `p = (1 + s) · r` of each edge. The arc keeps `90° · (1 − s)` around the corner's diagonal and
//! each side of it is a cubic whose handles lie on the edge, so the curve is tangent to the edge
//! where it starts and to the arc where it meets it. When `p` would exceed half the shorter side,
//! the smoothing is reduced first, then the radius. The construction is our own derivation from
//! these constraints.

/// The outline of a `w` x `h` rectangle with corners of radius `radius` smoothed by `smoothing`
/// (0..=1), as path data in the rectangle's own coordinates (`M`, `L`, `C`, `Z`; y down).
pub fn path_d(w: f64, h: f64, radius: f64, smoothing: f64) -> String {
    let w = if w.is_finite() { w.max(0.0) } else { 0.0 };
    let h = if h.is_finite() { h.max(0.0) } else { 0.0 };
    let half = w.min(h) / 2.0;
    let r = if radius.is_finite() { radius.clamp(0.0, half) } else { 0.0 };
    let mut s = if smoothing.is_finite() { smoothing.clamp(0.0, 1.0) } else { 0.0 };
    if r <= 0.0 {
        return format!("M 0 0 L {w} 0 L {w} {h} L 0 {h} Z");
    }
    s = s.min(half / r - 1.0).max(0.0);
    let c = corner(r, s);
    // The top-right corner in local terms: `u` runs back along the top edge from the corner,
    // `v` down the right edge. Each corner maps (u, v) to the rectangle by symmetry.
    let corners: [&dyn Fn(f64, f64) -> (f64, f64); 4] = [
        &|u, v| (w - u, v),     // top right: from the top edge to the right edge
        &|u, v| (w - v, h - u), // bottom right: from the right edge to the bottom edge
        &|u, v| (u, h - v),     // bottom left: from the bottom edge to the left edge
        &|u, v| (v, u),         // top left: from the left edge to the top edge
    ];
    let mut d = String::new();
    let pt = |f: &dyn Fn(f64, f64) -> (f64, f64), (u, v): (f64, f64)| {
        let (x, y) = f(u, v);
        format!("{} {}", round(x), round(y))
    };
    for (i, f) in corners.iter().enumerate() {
        let segs = c.segments();
        if i == 0 {
            d.push_str(&format!("M {}", pt(*f, segs[0])));
        } else {
            d.push_str(&format!(" L {}", pt(*f, segs[0])));
        }
        for k in 0..3 {
            let b = 1 + k * 3;
            d.push_str(&format!(" C {} {} {}", pt(*f, segs[b]), pt(*f, segs[b + 1]), pt(*f, segs[b + 2])));
        }
    }
    d.push_str(" Z");
    d
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0 + 0.0
}

/// One corner in (u, v) terms: distance back along the incoming edge, distance along the outgoing.
struct Corner {
    p: f64,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    r: f64,
    arc: f64,
}

fn corner(r: f64, s: f64) -> Corner {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};
    let p = (1.0 + s) * r;
    let arc = FRAC_PI_2 * (1.0 - s);
    // The arc's chord, and where the edge curve meets the arc.
    let arc_chord = (arc / 2.0).sin() * r * std::f64::consts::SQRT_2;
    let alpha = (FRAC_PI_2 - arc) / 2.0;
    let p3_p4 = r * (alpha / 2.0).tan();
    let beta = FRAC_PI_4 * s;
    let c = p3_p4 * beta.cos();
    let d = c * beta.tan();
    let b = ((p - arc_chord - c - d) / 3.0).max(0.0);
    let a = 2.0 * b;
    Corner { p, a, b, c, d, r, arc }
}

impl Corner {
    /// Ten points: the start on the incoming edge, then three cubics (control, control, end):
    /// edge → arc start, the arc, arc end → outgoing edge.
    fn segments(&self) -> [(f64, f64); 10] {
        let Corner { p, a, b, c, d, r, arc } = *self;
        // Edge curve into the arc: handles on the edge, ending where the arc starts.
        let e0 = (p, 0.0);
        let e1 = (p - a, 0.0);
        let e2 = (p - a - b, 0.0);
        let e3 = (p - a - b - c, d);
        // The arc around the centre (r, r) in (u, v), symmetric about the corner's diagonal.
        // Angles from +u towards +v: the corner itself is at 5π/4 from the centre; the arc runs
        // from the incoming edge's side (larger angle) to the outgoing edge's side.
        let start = std::f64::consts::FRAC_PI_4 * 5.0 + arc / 2.0;
        let end = start - arc;
        let at = |t: f64| (r + r * t.cos(), r + r * t.sin());
        let k = 4.0 / 3.0 * (arc / 4.0).tan() * r;
        let a0 = at(start);
        let a3 = at(end);
        // Tangent of the arc travelled towards smaller angles: (sin t, −cos t).
        let a1 = (a0.0 + k * start.sin(), a0.1 - k * start.cos());
        let a2 = (a3.0 - k * end.sin(), a3.1 + k * end.cos());
        // Mirror of the edge curve onto the outgoing edge (swap u and v), reversed.
        let o1 = (e2.1, e2.0);
        let o2 = (e1.1, e1.0);
        let o3 = (e0.1, e0.0);
        let _ = (e3, a0);
        [e0, e1, e2, a0, a1, a2, a3, o1, o2, o3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(d: &str) -> Vec<f64> {
        d.split_whitespace().filter_map(|t| t.parse().ok()).collect()
    }

    #[test]
    fn no_radius_is_a_rectangle() {
        assert_eq!(path_d(100.0, 50.0, 0.0, 0.6), "M 0 0 L 100 0 L 100 50 L 0 50 Z");
    }

    #[test]
    fn edge_curve_meets_the_arc() {
        for s in [0.0, 0.3, 0.6, 1.0] {
            let c = corner(10.0, s);
            let seg = c.segments();
            let e3 = (c.p - c.a - c.b - c.c, c.d);
            // The arc starts on the circle and where the edge curve ends.
            assert!((seg[3].0 - e3.0).abs() < 1e-6 && (seg[3].1 - e3.1).abs() < 1e-6, "s={s}: {:?} vs {:?}", seg[3], e3);
            // The arc is symmetric about the diagonal u = v.
            assert!((seg[3].0 - seg[6].1).abs() < 1e-9 && (seg[3].1 - seg[6].0).abs() < 1e-9);
        }
    }

    #[test]
    fn zero_smoothing_is_a_circular_corner() {
        let c = corner(10.0, 0.0);
        let seg = c.segments();
        assert!((c.p - 10.0).abs() < 1e-9);
        assert!((seg[3].0 - 10.0).abs() < 1e-9 && seg[3].1.abs() < 1e-9);
        assert!((seg[6].0).abs() < 1e-9 && (seg[6].1 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn smoothing_uses_more_of_the_edge() {
        let d0 = numbers(&path_d(200.0, 100.0, 20.0, 0.0));
        let d6 = numbers(&path_d(200.0, 100.0, 20.0, 0.6));
        // The first point is where the top-right corner starts on the top edge.
        assert_eq!((d0[0], d0[1]), (180.0, 0.0));
        assert_eq!((d6[0], d6[1]), (168.0, 0.0));
    }

    #[test]
    fn stays_inside_and_reduces_smoothing_when_the_rectangle_is_small() {
        for (w, h, r, s) in [(100.0, 40.0, 20.0, 1.0), (40.0, 40.0, 50.0, 0.8), (10.0, 300.0, 4.0, 0.6), (300.0, 120.0, 30.0, 0.6)] {
            let n = numbers(&path_d(w, h, r, s));
            assert!(n.chunks(2).all(|p| p[0] >= -1e-6 && p[0] <= w + 1e-6 && p[1] >= -1e-6 && p[1] <= h + 1e-6), "{w}x{h} r{r} s{s}");
            // Corners never cross the middle of an edge.
            assert!(n[0] >= w / 2.0 - 1e-6);
        }
        assert!(numbers(&path_d(f64::NAN, 10.0, f64::INFINITY, f64::NAN)).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn four_symmetric_corners() {
        let n = numbers(&path_d(120.0, 80.0, 16.0, 0.6));
        // 4 corners x 10 points, 2 numbers each.
        assert_eq!(n.len(), 80);
        let pts: Vec<(f64, f64)> = n.chunks(2).map(|p| (p[0], p[1])).collect();
        for &(x, y) in &pts {
            let mirrored = (120.0 - x, 80.0 - y);
            assert!(pts.iter().any(|q| (q.0 - mirrored.0).abs() < 0.01 && (q.1 - mirrored.1).abs() < 0.01), "{x},{y}");
        }
    }
}
