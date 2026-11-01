// Copyright 2025 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Flattening filled and stroked paths.

use crate::flatten_simd::{Callback, LinePathEl};
use crate::geometry::RectU16;
use crate::kurbo::{self, Affine, BezPath, PathEl, Stroke, StrokeCtx, StrokeOpts};
use alloc::vec::Vec;
use fearless_simd::{Level, Simd, dispatch};
use log::warn;

pub use crate::flatten_simd::FlattenCtx;

// The current tolerance is set to 0.25. Since `sqrt` doesn't work in const contexts, we instead
// hardcode the squared tolerance and derive the others from that.
pub(crate) const SQRT_TOL: f64 = 0.5;
pub(crate) const TOL: f64 = SQRT_TOL * SQRT_TOL;
pub(crate) const TOL_2: f64 = TOL * TOL;

/// A point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The x coordinate of the point.
    pub x: f32,
    /// The y coordinate of the point.
    pub y: f32,
}

impl Point {
    /// The point `(0, 0)`.
    pub const ZERO: Self = Self::new(0., 0.);

    /// Create a new point.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<kurbo::Point> for Point {
    #[inline(always)]
    fn from(value: kurbo::Point) -> Self {
        Self {
            x: value.x as f32,
            y: value.y as f32,
        }
    }
}

impl core::ops::Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl core::ops::Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl core::ops::Mul<f32> for Point {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

/// A line.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    /// The start point of the line.
    pub p0: Point,
    /// The end point of the line.
    pub p1: Point,
}

impl Line {
    /// Create a new line.
    pub fn new(p0: Point, p1: Point) -> Self {
        Self { p0, p1 }
    }
}

/// Flatten a filled Bézier path into line segments.
///
/// # Open subpaths and culling
///
/// Open subpaths in the input path get closed by connecting the last endpoint in the subpath to
/// the starting point. The output lines in `line_buf` describe the flattened path, but these lines
/// may describe open subpaths, as some path elements may have been culled.
///
/// For example, consider the following, where the box describes the viewport, a path is marked by
/// `*`, and the region to be filled in the viewport is shaded. For ease of drawing the ASCII art,
/// the path elements are all lines ([`PathEl::LineTo`]), but the same also holds for Bézier path
/// elements.
///
/// ```text
///    ---> winding scan direction
///
///                   * * * * *
///                 *         *
///    ---------- * -----     *
///    |        *░░░░░░░|     *
///    |      *░░░░░░░░░|     *
///    |    *░░░░░░░░░░░|     *
///    |  *░░░░░░░░░░░░░|     *
///    |*░░░░░░░░░░░░░░░|     *
///   *|░░░░░░░░░░░░░░░░|     *
/// *  |░░░░░░░░░░░░░░░░|     *
/// *  |░░░░░░░░░░░░░░░░|     *
/// *  ------------------     *
/// *                         *
/// *                         *
/// *                         *
/// *                         *
/// *                         *
/// *                         *
/// *                         *
/// * * * * * * * * * * * * * *
/// ```
///
/// Because the winding scan direction is from left to right, only the left-of-viewport and
/// diagonal lines matter in later stages of rendering for the winding number and pixel coverage.
/// The other three lines can be culled.
///
/// ```text
///                   *
///                 *
///    ---------- * -----
///    |        *░░░░░░░|
///    |      *░░░░░░░░░|
///    |    *░░░░░░░░░░░|
///    |  *░░░░░░░░░░░░░|
///    |*░░░░░░░░░░░░░░░|
///   *|░░░░░░░░░░░░░░░░|
/// *  |░░░░░░░░░░░░░░░░|
/// *  |░░░░░░░░░░░░░░░░|
/// *  ------------------
/// *
/// *
/// *
/// *
/// *
/// *
/// *
/// ```
///
/// It is important to keep these flattened subpaths open after culling, as closing the subpaths
/// might yield different geometry like the following.
///
/// ```text
///                   *
///                 **
///    ---------- * *----
///    |        *░░*    |
///    |      *░░░*     |
///    |    *░░░░*      |
///    |  *░░░░░*       |
///    |*░░░░░░*        |
///   *|░░░░░░*         |
/// *  |░░░░░*          |
/// *  |░░░░*           |
/// *  --- * ------------
/// *     *
/// *    *
/// *   *
/// *  *
/// * *
/// **
/// *
/// ```
pub fn fill(
    level: Level,
    path: impl IntoIterator<Item = PathEl>,
    affine: Affine,
    line_buf: &mut Vec<Line>,
    ctx: &mut FlattenCtx,
    cull_bbox: RectU16,
) {
    dispatch!(level, simd => fill_impl(simd, path, affine, line_buf, ctx, cull_bbox));
}

/// Flatten a filled bezier path into line segments.
///
/// See the note about open subpaths and culling on [`fill`].
#[inline(always)]
pub fn fill_impl<S: Simd>(
    simd: S,
    path: impl IntoIterator<Item = PathEl>,
    affine: Affine,
    line_buf: &mut Vec<Line>,
    flatten_ctx: &mut FlattenCtx,
    cull_bbox: RectU16,
) {
    line_buf.clear();
    let mut lb = FlattenerCallback {
        line_buf,
        start: Point::ZERO,
        p0: Point::ZERO,
        is_nan: false,
    };

    crate::flatten_simd::flatten(simd, path, affine, &mut lb, flatten_ctx, cull_bbox);

    // A path that contains NaN is ill-defined, so ignore it.
    if lb.is_nan {
        warn!("A path contains NaN, ignoring it.");

        line_buf.clear();
    }
}
/// Flatten a stroked Bézier path into line segments.
///
/// See the note about open subpaths and culling on [`fill`].
pub fn stroke(
    level: Level,
    // PdfCraft patch: `Clone` so the path can be stroked a second time.
    path: impl IntoIterator<Item = PathEl> + Clone,
    style: &Stroke,
    affine: Affine,
    line_buf: &mut Vec<Line>,
    flatten_ctx: &mut FlattenCtx,
    stroke_ctx: &mut StrokeCtx,
    cull_bbox: RectU16,
) {
    // TODO: Temporary hack to ensure that strokes are scaled properly by the transform.
    let tolerance = TOL
        / affine.as_coeffs()[0]
            .abs()
            .max(affine.as_coeffs()[3].abs())
            .max(1.);

    expand_stroke(path.clone(), style, tolerance, stroke_ctx);
    // PdfCraft patch: kurbo's stroker can put NaN into the outline of a curve that is nearly a
    // line (all control points within a rounding error of the chord), and `fill` then drops the
    // whole outline. Stroke it again with such curves as lines. A finite outline is unchanged.
    if !stroke_ctx.output().is_finite() {
        let path = near_line_curves_as_lines(path, tolerance);
        expand_stroke(&path, style, tolerance, stroke_ctx);
    }
    fill(
        level,
        stroke_ctx.output(),
        affine,
        line_buf,
        flatten_ctx,
        cull_bbox,
    );
}

// PdfCraft patch.
/// `path` with each quadratic or cubic whose control points all lie within
/// `tolerance` of its chord replaced by a line to its end point. One element out per element in.
fn near_line_curves_as_lines(path: impl IntoIterator<Item = PathEl>, tolerance: f64) -> BezPath {
    let mut out = BezPath::new();
    let mut start = kurbo::Point::ZERO;
    let mut last = start;
    for el in path {
        let el = match el {
            PathEl::QuadTo(p1, p2) if near_segment(p1, last, p2, tolerance) => PathEl::LineTo(p2),
            PathEl::CurveTo(p1, p2, p3)
                if near_segment(p1, last, p3, tolerance)
                    && near_segment(p2, last, p3, tolerance) =>
            {
                PathEl::LineTo(p3)
            }
            el => el,
        };
        match el {
            PathEl::MoveTo(p) => {
                start = p;
                last = p;
            }
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => last = p,
            PathEl::ClosePath => last = start,
        }
        out.push(el);
    }
    out
}

/// Whether `p` is within `tolerance` of the segment from `a` to `b`. False for non-finite input.
fn near_segment(p: kurbo::Point, a: kurbo::Point, b: kurbo::Point, tolerance: f64) -> bool {
    let ab = b - a;
    let len2 = ab.hypot2();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).hypot() <= tolerance
}

/// Expand a stroked path to a filled path.
pub fn expand_stroke(
    path: impl IntoIterator<Item = PathEl>,
    style: &Stroke,
    tolerance: f64,
    stroke_ctx: &mut StrokeCtx,
) {
    kurbo::stroke_with(path, style, &StrokeOpts::default(), tolerance, stroke_ctx);
}

struct FlattenerCallback<'a> {
    line_buf: &'a mut Vec<Line>,
    start: Point,
    p0: Point,
    is_nan: bool,
}

impl Callback for FlattenerCallback<'_> {
    #[inline(always)]
    fn callback(&mut self, el: LinePathEl) {
        match el {
            LinePathEl::MoveTo(p) => {
                self.is_nan |= p.is_nan();

                let p = p.into();
                self.start = p;
                self.p0 = p;
            }
            LinePathEl::LineTo(p) => {
                self.is_nan |= p.is_nan();

                let p = p.into();
                self.line_buf.push(Line::new(self.p0, p));
                self.p0 = p;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FlattenCtx, Line, TOL, expand_stroke, stroke};
    use crate::geometry::RectU16;
    use crate::kurbo::{Affine, BezPath, Point, Stroke, StrokeCtx};
    use alloc::vec::Vec;
    use fearless_simd::Level;

    fn stroke_lines(path: &BezPath, width: f64, ctx: &mut StrokeCtx) -> Vec<Line> {
        let mut lines = Vec::new();
        stroke(
            Level::baseline(),
            path,
            &Stroke::new(width),
            Affine::IDENTITY,
            &mut lines,
            &mut FlattenCtx::default(),
            ctx,
            RectU16::new(0, 0, 200, 200),
        );
        lines
    }

    // An outline in an Illustrator file (#4907) has a cubic whose control points are all equal
    // and 0.001 from its start. kurbo's stroker gives that curve a NaN join, which made the whole
    // outline disappear. The trigger depends on these exact coordinates.
    #[test]
    fn a_tiny_degenerate_cubic_does_not_drop_the_stroke() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((73.763, -0.536));
        let p = Point::new(73.764, -0.536);
        path.curve_to(p, p, p);

        let mut ctx = StrokeCtx::default();
        let lines = stroke_lines(&path, 0.34, &mut ctx);
        assert!(!lines.is_empty(), "the stroke was dropped");
        assert!(ctx.output().is_finite());
        let max_x = lines
            .iter()
            .map(|l| l.p0.x.max(l.p1.x))
            .fold(f32::MIN, f32::max);
        assert!(max_x > 73.0, "the stroke ends at x = {max_x}");
    }

    #[test]
    fn a_path_with_nan_is_still_dropped() {
        let mut path = BezPath::new();
        path.move_to((10.0, 10.0));
        path.curve_to((f64::NAN, 10.0), (10.0, 60.0), (60.0, 60.0));
        let lines = stroke_lines(&path, 2.0, &mut StrokeCtx::default());
        assert!(lines.is_empty());
    }

    // A regression guard: it passes with or without the patch.
    #[test]
    fn a_finite_outline_is_stroked_as_before() {
        let mut path = BezPath::new();
        path.move_to((10.0, 10.0));
        path.curve_to((60.0, 10.0), (10.0, 60.0), (60.0, 60.0));
        let style = Stroke::new(2.0);
        let mut expected = StrokeCtx::default();
        expand_stroke(&path, &style, TOL, &mut expected);

        let mut ctx = StrokeCtx::default();
        let lines = stroke_lines(&path, 2.0, &mut ctx);
        assert!(!lines.is_empty());
        assert_eq!(ctx.output(), expected.output());
    }
}
