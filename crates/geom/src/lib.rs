//! pdfcraft-geom — L0 geometry primitives shared by every PdfCraft crate.
//!
//! PDF user space is y-up with the origin at the bottom-left of the page; view space is y-down.
//! `PageRect` is always in PDF user space (points, 1/72 inch).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod perspective;

/// An axis-aligned rectangle in PDF user space (points).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PageRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl PageRect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// Page rotation in quarter turns, as in the PDF `/Rotate` key (normalised to 0/90/180/270).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn from_degrees(deg: i64) -> Self {
        match deg.rem_euclid(360) {
            90 => Self::R90,
            180 => Self::R180,
            270 => Self::R270,
            _ => Self::R0,
        }
    }
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }
}

/// The axis-aligned box `[x0, y0, x1, y1]` around a quadrilateral given as its four corners
/// `[x1, y1, x2, y2, x3, y3, x4, y4]` (one quad of a markup or redaction annotation's `/QuadPoints`),
/// in any corner order. A NaN coordinate is passed over by the min/max, and an infinite one is kept
/// (every x at `+inf` gives `x0 = x1 = +inf`). When every x (or y) is NaN that axis comes out
/// inverted and infinite, `+inf` to `-inf`: a caller that must not get such a box checks that the
/// coordinates are finite, or `x0 <= x1` and `y0 <= y1`. This is not validation: a quad with one
/// NaN corner still gets a box from the other three, so decide with [`finite_quad_bounds`].
pub fn quad_bounds(q: &[f64; 8]) -> [f64; 4] {
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    [
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// `rect` with its corners ordered (`x0 <= x1`, `y0 <= y1`), or `None` when any of the four
/// numbers is not finite. Every original value is checked before ordering, so a NaN can't be
/// ordered away. A rectangle with no width or height is still a rectangle here; callers that
/// need area check for it.
pub fn normalize_rect(rect: [f64; 4]) -> Option<[f64; 4]> {
    let [x0, y0, x1, y1] = rect;
    rect.iter().all(|v| v.is_finite()).then(|| [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// [`quad_bounds`] of a quad whose eight numbers are all finite; `None` otherwise. Unlike
/// [`quad_bounds`], which passes over a NaN corner and so can give a box smaller than the quad,
/// this is safe to decide with.
pub fn finite_quad_bounds(quad: &[f64; 8]) -> Option<[f64; 4]> {
    quad.iter().all(|v| v.is_finite()).then(|| quad_bounds(quad))
}

/// The smallest `f32` rectangle that contains `rect` (normalized first): a low edge that `f32`
/// rounds up moves one step down, a high edge it rounds down one step up, so the result never
/// leaves out any of `rect` and is no wider than it must be. `None` when `rect` isn't finite or
/// its covering edges aren't finite `f32`s.
pub fn rect_f32_cover(rect: [f64; 4]) -> Option<[f32; 4]> {
    let [x0, y0, x1, y1] = normalize_rect(rect)?;
    let low = |v: f64| {
        let c = v as f32;
        let c = if f64::from(c) > v { c.next_down() } else { c };
        c.is_finite().then_some(c)
    };
    let high = |v: f64| {
        let c = v as f32;
        let c = if f64::from(c) < v { c.next_up() } else { c };
        c.is_finite().then_some(c)
    };
    Some([low(x0)?, low(y0)?, high(x1)?, high(y1)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_normalises() {
        let r = PageRect::new(10.0, 20.0, 0.0, 5.0);
        assert_eq!((r.x0, r.y0, r.width(), r.height()), (0.0, 5.0, 10.0, 15.0));
    }

    #[test]
    fn rotation_normalises_negative_and_large_values() {
        assert_eq!(Rotation::from_degrees(-90), Rotation::R270);
        assert_eq!(Rotation::from_degrees(450), Rotation::R90);
        assert!(Rotation::R90.swaps_axes());
    }

    #[test]
    fn quad_bounds_is_the_box_around_the_corners() {
        // A square turned 45 degrees, its corners in no particular order.
        assert_eq!(quad_bounds(&[10.0, 20.0, 20.0, 30.0, 0.0, 30.0, 10.0, 40.0]), [0.0, 20.0, 20.0, 40.0]);
        assert_eq!(quad_bounds(&[5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0]), [5.0, 5.0, 5.0, 5.0]);
        assert_eq!(quad_bounds(&[f64::NAN, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]), [2.0, 1.0, 6.0, 7.0], "a NaN is passed over");
        let all_nan = quad_bounds(&[f64::NAN, 0.0, f64::NAN, 0.0, f64::NAN, 0.0, f64::NAN, 0.0]);
        assert_eq!((all_nan[0], all_nan[2]), (f64::INFINITY, f64::NEG_INFINITY));
        // An axis that is all `+inf` stays there: not `f64::MAX`, which would pass for finite.
        let inf = f64::INFINITY;
        assert_eq!(quad_bounds(&[inf, 0.0, inf, 1.0, inf, 2.0, inf, 3.0]), [inf, 0.0, inf, 3.0]);
    }

    #[test]
    fn normalize_rect_orders_finite_corners_only() {
        assert_eq!(normalize_rect([10.0, 20.0, 0.0, 5.0]), Some([0.0, 5.0, 10.0, 20.0]));
        assert_eq!(normalize_rect([1.0, 1.0, 1.0, 1.0]), Some([1.0, 1.0, 1.0, 1.0]), "no area is still a rectangle");
        for (i, bad) in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY].into_iter().enumerate() {
            let mut r = [0.0, 0.0, 10.0, 10.0];
            r[i % 4] = bad;
            assert_eq!(normalize_rect(r), None, "{r:?}");
        }
        // Idempotent once accepted.
        for r in [[3.0, -2.0, -7.5, 4.0], [0.0, -0.0, -0.0, 0.0], [f64::MAX, f64::MIN, 1.0, 2.0]] {
            let n = normalize_rect(r).unwrap();
            assert_eq!(normalize_rect(n), Some(n));
        }
    }

    #[test]
    fn finite_quad_bounds_rejects_any_bad_corner() {
        let q = [10.0, 20.0, 20.0, 30.0, 0.0, 30.0, 10.0, 40.0];
        assert_eq!(finite_quad_bounds(&q), Some([0.0, 20.0, 20.0, 40.0]));
        for i in 0..8 {
            for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let mut b = q;
                b[i] = bad;
                assert_eq!(finite_quad_bounds(&b), None, "corner value {i} = {bad}");
            }
        }
        // An accepted box contains every corner.
        let b = finite_quad_bounds(&q).unwrap();
        for c in q.as_chunks::<2>().0 {
            assert!(b[0] <= c[0] && c[0] <= b[2] && b[1] <= c[1] && c[1] <= b[3]);
        }
    }

    /// The covering `f32` rectangle contains the source, is ordered and finite, and each edge is
    /// the nearest `f32` on the outside (one step in would leave part of the source out).
    fn assert_tight_cover(r: [f64; 4]) {
        let n = normalize_rect(r).unwrap();
        let c = rect_f32_cover(r).unwrap_or_else(|| panic!("{r:?} has a cover"));
        assert!(c.iter().all(|v| v.is_finite()) && c[0] <= c[2] && c[1] <= c[3], "{c:?}");
        for (i, (&e, &s)) in c.iter().zip(n.iter()).enumerate() {
            if i < 2 {
                assert!(f64::from(e) <= s && f64::from(e.next_up()) > s, "low edge {i}: {e} for {s}");
            } else {
                assert!(f64::from(e) >= s && f64::from(e.next_down()) < s, "high edge {i}: {e} for {s}");
            }
        }
    }

    #[test]
    fn rect_f32_cover_is_the_tightest_enclosing_f32_rect() {
        // Exactly representable values stay put.
        assert_eq!(rect_f32_cover([20.0, 95.0, 40.0, 110.0]), Some([20.0, 95.0, 40.0, 110.0]));
        assert_eq!(rect_f32_cover([40.0, 110.0, 20.0, 95.0]), Some([20.0, 95.0, 40.0, 110.0]), "normalized");
        // 0.1 is not an f32: the low edge rounds down, the high edge up.
        let c = rect_f32_cover([0.1, 0.1, 0.1, 0.1]).unwrap();
        assert!(f64::from(c[0]) < 0.1 && f64::from(c[2]) > 0.1 && c[2] == c[0].next_up());
        let tiny = f64::from(f32::from_bits(1)) / 4.0; // below the smallest f32 subnormal
        for r in [
            [0.1, 0.2, 0.3, 0.7],
            [-0.0, 0.0, 0.0, -0.0],
            [-tiny, -tiny, tiny, tiny],
            [1e-300, -1e-300, 2e-300, 1e-300],
            [-1234.5678901, 987.654321, 1e6 + 0.3, 1e7 + 0.1],
            [f64::from(f32::MIN), f64::from(f32::MIN), f64::from(f32::MAX), f64::from(f32::MAX)],
            [16_777_217.0, -16_777_217.0, 16_777_219.0, 0.5],
        ] {
            assert_tight_cover(r);
        }
        // A subnormal high edge is covered by the smallest positive f32, not by zero.
        assert_eq!(rect_f32_cover([0.0, 0.0, tiny, tiny]).unwrap()[2], f32::from_bits(1));
    }

    #[test]
    fn rect_f32_cover_refuses_what_f32_cannot_cover() {
        let past = f64::from(f32::MAX) * 1.000_001;
        assert_eq!(rect_f32_cover([0.0, 0.0, past, 1.0]), None);
        assert_eq!(rect_f32_cover([-past, 0.0, 1.0, 1.0]), None);
        // Just over f32::MAX (within half a step) rounds down to it, and the step up is infinite.
        assert_eq!(rect_f32_cover([0.0, 0.0, f64::from(f32::MAX) + 1e30, 1.0]), None);
        assert_eq!(rect_f32_cover([f64::NAN, 0.0, 1.0, 1.0]), None);
        assert_eq!(rect_f32_cover([0.0, 0.0, f64::INFINITY, 1.0]), None);
    }
}
