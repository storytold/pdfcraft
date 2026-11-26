//! Path gradients (`a:path`) that follow a rectangle or the shape's own outline, which no vello
//! gradient draws: they're rasterised into a small image of the bounds and painted with it.
//!
//! Measured on PowerPoint's export (black to grey to white from the centre):
//! - `rect`: the colour position is the larger of the horizontal and vertical distances from the
//!   focus, each as a share of the distance to that side's edge (X-shaped contours);
//! - `shape`: contours are the outline scaled about the focus (an ellipse gives ellipses, a
//!   triangle triangles): the share of the way from the focus to the outline along the same ray.
//!
//! `circle` stays a radial vello gradient (see `paint::gradient`).

use std::sync::Arc;

use deckcraft_color::Rgba;
use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use vello_cpu::{Pixmap, RenderContext, peniko};

/// Longest side of the rasterised gradient, in pixels (bilinear sampling smooths the rest).
const MAX_SIDE: f64 = 384.0;

/// Most pixel × outline-edge tests one `shape` gradient may cost; a denser outline is simplified
/// to fit (each pixel tests every edge).
const MAX_EDGE_WORK: u64 = 64_000_000;

/// Fewest edges a simplified polygon keeps, so a coarse outline still has a shape.
const MIN_POLY_EDGES: usize = 8;

/// Most points `polygons` keeps from one path; a path flattening past this (a huge finite curve) is
/// not drawn rather than held in memory first.
const MAX_POINTS: usize = 2_000_000;

/// Quads beyond one per cubic that a path's cubics may subdivide into before [`polygons`] refuses it.
const MAX_EXTRA_QUADS: f64 = 200_000.0;

/// Flattening tolerance of [`polygons`], in path units.
const FLATTEN_TOLERANCE: f64 = 0.25;

/// The edge budget for a `pw` x `ph` raster where each pixel tests every edge.
pub(crate) fn edge_budget(pw: u32, ph: u32) -> usize {
    let pixels = u64::from(pw).saturating_mul(u64::from(ph)).max(1);
    usize::try_from(MAX_EDGE_WORK / pixels).unwrap_or(usize::MAX).max(MIN_POLY_EDGES)
}

/// `pw` x `ph` cut down (aspect kept) until every pixel testing all of `edges` stays within
/// [`MAX_EDGE_WORK`]; the raster is stretched when painted, so a many-contour outline keeps every
/// contour at a coarser sampling.
pub(crate) fn fit_raster(edges: usize, pw: u32, ph: u32) -> (u32, u32) {
    let work = u64::from(pw).saturating_mul(u64::from(ph)).saturating_mul(edges.max(1) as u64);
    if work <= MAX_EDGE_WORK {
        return (pw, ph);
    }
    let k = (MAX_EDGE_WORK as f64 / work as f64).sqrt();
    let w = ((f64::from(pw) * k) as u32).max(1);
    let h = ((f64::from(ph) * k) as u32).max(1);
    // A side held at 1 pixel leaves the other to take the rest of the cut.
    let room = |side: u32| u32::try_from(MAX_EDGE_WORK / u64::from(side).saturating_mul(edges.max(1) as u64).max(1)).unwrap_or(u32::MAX).max(1);
    let w = w.min(room(h));
    (w, h.min(room(w)))
}

/// Colour position (0 at the focus, 1 at the edge) of `p` for path `kind` in `bounds` with the
/// focus rect `focus` (fractions inset from left, top, right, bottom), `outline` being the
/// shape's flattened polygon(s) for `shape`.
pub fn position(kind: &str, bounds: Rect, focus: [f64; 4], outline: &[Vec<Point>], p: Point) -> f64 {
    let f = Rect::new(
        bounds.x0 + bounds.width() * focus[0].clamp(0.0, 1.0),
        bounds.y0 + bounds.height() * focus[1].clamp(0.0, 1.0),
        bounds.x1 - bounds.width() * focus[2].clamp(0.0, 1.0),
        bounds.y1 - bounds.height() * focus[3].clamp(0.0, 1.0),
    );
    let f = if f.x0 <= f.x1 && f.y0 <= f.y1 { f } else { Rect::from_center_size(f.center(), (0.0, 0.0)) };
    match kind {
        "shape" if !outline.is_empty() => {
            // A focus rectangle (not a point) is the first colour throughout, as PowerPoint draws
            // it; outside, the colour runs from the rectangle's edge to the outline along the ray
            // from its centre.
            if f.width() > 1e-9 && f.height() > 1e-9 {
                if f.contains(p) {
                    return 0.0;
                }
                let c = f.center();
                let to_outline = shape_position(outline, c, p);
                let d = (p - c).hypot();
                if to_outline <= 0.0 || d <= 0.0 {
                    return 1.0;
                }
                // Where the ray leaves the focus rectangle, as a share of the way to `p`.
                let v = p - c;
                let kx = if v.x.abs() > 1e-12 { (f.width() / 2.0) / v.x.abs() } else { f64::INFINITY };
                let ky = if v.y.abs() > 1e-12 { (f.height() / 2.0) / v.y.abs() } else { f64::INFINITY };
                let rect_share = kx.min(ky).min(1.0);
                // p is `to_outline` of the way to the outline; the rectangle edge `rect_share` of
                // the way to p.
                let outline_share = 1.0 / to_outline; // outline distance in units of |p - c|
                return ((1.0 - rect_share) / (outline_share - rect_share).max(1e-9)).clamp(0.0, 1.0);
            }
            shape_position(outline, f.center(), p)
        }
        _ => {
            // Distance outside the focus rect along each axis, over the room to that side's edge.
            let axis = |v: f64, lo: f64, hi: f64, edge_lo: f64, edge_hi: f64| {
                if v < lo {
                    (lo - v) / (lo - edge_lo).max(1e-9)
                } else if v > hi {
                    (v - hi) / (edge_hi - hi).max(1e-9)
                } else {
                    0.0
                }
            };
            axis(p.x, f.x0, f.x1, bounds.x0, bounds.x1).max(axis(p.y, f.y0, f.y1, bounds.y0, bounds.y1)).clamp(0.0, 1.0)
        }
    }
}

/// The share of the way from `c` to the outline along the ray through `p` (1 when the ray meets
/// no edge).
fn shape_position(outline: &[Vec<Point>], c: Point, p: Point) -> f64 {
    let d = p - c;
    let len = d.hypot();
    if len < 1e-9 {
        return 0.0;
    }
    // The outline scaled about `c` by t reaches `p` first (smallest t) where the ray's farthest
    // crossing does: the only crossing for a focus inside a convex outline, and the one that
    // still works for a focus off the shape (a corner of an ellipse's box).
    let mut best = f64::INFINITY;
    for poly in outline {
        for w in poly.windows(2) {
            let (a, b) = (w[0], w[1]);
            let e = b - a;
            let den = d.cross(e);
            if den.abs() < 1e-12 {
                continue;
            }
            let ac = a - c;
            let s = ac.cross(e) / den;
            let u = ac.cross(d) / den;
            if s > 1e-9 && (-1e-9..=1.0 + 1e-9).contains(&u) {
                best = if best.is_finite() { best.max(s) } else { s };
            }
        }
    }
    if best.is_finite() { (1.0 / best).clamp(0.0, 1.0) } else { 1.0 }
}

/// The outline flattened into closed polygons.
pub fn polygons(path: &BezPath) -> Vec<Vec<Point>> {
    // `kurbo::flatten` buffers all of a cubic's quads before calling back, and cannot be stopped
    // once running, so bound the quads the cubics will make first (kurbo's own count: it splits
    // cubics into quads at a tenth of the tolerance). An ordinary curve makes a few; only those
    // past the first of each count.
    let mut extra_quads = 0.0f64;
    let mut at = Point::ZERO;
    let mut start = Point::ZERO;
    for el in path.iter() {
        match el {
            PathEl::MoveTo(p) => {
                at = p;
                start = p;
            }
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) => at = p,
            PathEl::CurveTo(a, b, p) => {
                let err = ((b.to_vec2() * 3.0 - p.to_vec2()) - (a.to_vec2() * 3.0 - at.to_vec2())).hypot2();
                extra_quads += (err / (432.0 * (FLATTEN_TOLERANCE * 0.1).powi(2))).powf(1.0 / 6.0).ceil().max(1.0) - 1.0;
                // A non-finite estimate is over the limit too.
                if extra_quads.is_nan() || extra_quads > MAX_EXTRA_QUADS {
                    return Vec::new();
                }
                at = p;
            }
            PathEl::ClosePath => at = start,
        }
    }
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let mut total = 0usize;
    kurbo::flatten(path.iter(), FLATTEN_TOLERANCE, |el| {
        total += 1;
        if total > MAX_POINTS {
            return;
        }
        match el {
            PathEl::MoveTo(p) => {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![p];
            }
            PathEl::LineTo(p) => cur.push(p),
            PathEl::ClosePath => {
                if let Some(f) = cur.first().copied() {
                    cur.push(f);
                }
            }
            _ => {}
        }
    });
    if total > MAX_POINTS {
        return Vec::new();
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    for poly in &mut out {
        if let (Some(f), Some(l)) = (poly.first().copied(), poly.last().copied())
            && (f - l).hypot() > 1e-9
        {
            poly.push(f);
        }
    }
    out.retain(|p| p.len() > 2 && p.len() < 100_000);
    out
}

/// `outline` cut down to at most about `max_edges` edges in total: first by dropping points
/// closer than `tol` to the last kept one (detail finer than a raster pixel), then, if still too
/// many, by keeping every k-th point (a polygon too small to thin stays whole, and none is
/// dropped). Each polygon stays closed. Returns it unchanged when it fits.
pub(crate) fn simplify(outline: Vec<Vec<Point>>, tol: f64, max_edges: usize) -> Vec<Vec<Point>> {
    let edges = |o: &[Vec<Point>]| o.iter().map(|p| p.len().saturating_sub(1)).sum::<usize>();
    if edges(&outline) <= max_edges {
        return outline;
    }
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { 0.0 };
    let mut out: Vec<Vec<Point>> = outline
        .into_iter()
        .map(|poly| {
            let mut kept: Vec<Point> = Vec::with_capacity(poly.len());
            let last = poly.last().copied();
            for p in poly {
                if kept.last().is_none_or(|q| (p - *q).hypot() >= tol) {
                    kept.push(p);
                }
            }
            if let (Some(l), Some(k)) = (last, kept.last().copied())
                && k != l
            {
                kept.push(l);
            }
            kept
        })
        .filter(|p| p.len() > 2)
        .collect();
    let total = edges(&out);
    if total > max_edges {
        let k = total.div_ceil(max_edges.max(1)).max(2);
        for poly in &mut out {
            let n = poly.len();
            if n <= MIN_POLY_EDGES + 1 {
                continue;
            }
            let k = k.min((n - 1) / MIN_POLY_EDGES).max(1);
            let last = poly.last().copied();
            let mut kept: Vec<Point> = poly.iter().step_by(k).copied().collect();
            if let (Some(l), Some(q)) = (last, kept.last().copied())
                && q != l
            {
                kept.push(l);
            }
            *poly = kept;
        }
        // Polygons too small to thin stay whole, so the total can still exceed `max_edges`: the
        // caller shrinks its raster ([`fit_raster`]) rather than lose contours.
    }
    out
}

/// A 256-entry colour ramp for the sorted, already expanded `stops` (linear in sRGB between them).
pub fn ramp(stops: &[(f64, Rgba)]) -> Vec<Rgba> {
    (0..256)
        .map(|i| {
            let t = i as f64 / 255.0;
            let after = stops.iter().position(|s| s.0 >= t);
            match after {
                Some(0) | None => stops.get(after.unwrap_or(stops.len().saturating_sub(1))).map(|s| s.1).unwrap_or(Rgba::BLACK),
                Some(j) => {
                    let (Some(a), Some(b)) = (stops.get(j - 1), stops.get(j)) else { return Rgba::BLACK };
                    let k = if b.0 - a.0 > 1e-12 { (t - a.0) / (b.0 - a.0) } else { 1.0 };
                    a.1.lerp(b.1, k)
                }
            }
        })
        .collect()
}

/// Fill `path` with a `rect` or `shape` path gradient of `stops` (sorted and expanded, see
/// `ramp::expand`) over `bounds`, faded by `alpha`; `m` maps the path to the output.
#[allow(clippy::too_many_arguments)]
pub fn fill(ctx: &mut RenderContext, kind: &str, focus: [f64; 4], stops: &[(f64, Rgba)], bounds: Rect, path: &BezPath, m: Affine, alpha: f64) {
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 || !bounds.is_finite() || stops.is_empty() {
        return;
    }
    let k = crate::scale_of(m);
    let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
    let longest = bounds.width().max(bounds.height()) * k;
    let s = (MAX_SIDE / longest).min(1.0) * k;
    let (pw, ph) = (((bounds.width() * s).ceil() as u32).clamp(1, 4096), ((bounds.height() * s).ceil() as u32).clamp(1, 4096));
    let (outline, pw, ph) = if kind == "shape" {
        let max_edges = edge_budget(pw, ph);
        let pixel = (bounds.width() / f64::from(pw)).min(bounds.height() / f64::from(ph));
        let o = simplify(polygons(path), pixel, max_edges);
        let (pw, ph) = fit_raster(o.iter().map(|p| p.len().saturating_sub(1)).sum(), pw, ph);
        (o, pw, ph)
    } else {
        (Vec::new(), pw, ph)
    };
    // An outline `polygons` refused (too many points) has no contours: skip the shape rather than
    // paint it with the rectangle fallback's different gradient.
    if kind == "shape" && outline.is_empty() {
        return;
    }
    let lut = ramp(stops);
    let a = alpha.clamp(0.0, 1.0);
    let mut data = Vec::with_capacity((pw * ph) as usize);
    for j in 0..ph {
        for i in 0..pw {
            let p = Point::new(bounds.x0 + (i as f64 + 0.5) / pw as f64 * bounds.width(), bounds.y0 + (j as f64 + 0.5) / ph as f64 * bounds.height());
            let t = position(kind, bounds, focus, &outline, p);
            let c = lut.get((t * 255.0).round().clamp(0.0, 255.0) as usize).copied().unwrap_or(Rgba::BLACK);
            let ca = (c.a as f64 * a).round().clamp(0.0, 255.0);
            let mul = |v: u8| (v as f64 * ca / 255.0).round().clamp(0.0, 255.0) as u8;
            data.push(vello_cpu::color::PremulRgba8 { r: mul(c.r), g: mul(c.g), b: mul(c.b), a: ca as u8 });
        }
    }
    let pm = Arc::new(Pixmap::from_parts(data, pw as u16, ph as u16));
    ctx.set_transform(m);
    ctx.set_paint(vello_cpu::Image {
        image: vello_cpu::ImageSource::Pixmap(pm),
        sampler: peniko::ImageSampler { quality: peniko::ImageQuality::Medium, ..Default::default() },
    });
    ctx.set_paint_transform(
        Affine::translate(bounds.origin().to_vec2()) * Affine::scale_non_uniform(bounds.width() / pw as f64, bounds.height() / ph as f64),
    );
    ctx.fill_path(path);
    ctx.reset_paint_transform();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    const CENTRE: [f64; 4] = [0.5, 0.5, 0.5, 0.5];

    #[test]
    fn rect_paths_take_the_larger_axis_share() {
        // PowerPoint, 280x200 box from the centre: 20 px across is 1/7, 20 px down is 1/5.
        let b = Rect::new(0.0, 0.0, 280.0, 200.0);
        let at = |x: f64, y: f64| position("rect", b, CENTRE, &[], Point::new(x, y));
        assert!((at(160.0, 100.0) - 20.0 / 140.0).abs() < 1e-9);
        assert!((at(140.0, 120.0) - 20.0 / 100.0).abs() < 1e-9);
        assert!((at(168.0, 120.0) - 0.2).abs() < 1e-9, "the diagonal takes the larger share");
        assert_eq!(at(0.0, 0.0), 1.0);
        // From a corner (fillToRect 0,0,100%,100%): the far corner is the end.
        let corner = [0.0, 0.0, 1.0, 1.0];
        assert!((position("rect", b, corner, &[], Point::new(140.0, 50.0)) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn shape_paths_follow_the_outline() {
        let b = Rect::new(0.0, 0.0, 280.0, 200.0);
        let ellipse = polygons(&kurbo::Ellipse::from_rect(b).to_path(0.01));
        let at = |x: f64, y: f64| position("shape", b, CENTRE, &ellipse, Point::new(x, y));
        assert!((at(160.0, 100.0) - 20.0 / 140.0).abs() < 0.01);
        assert!((at(140.0, 120.0) - 0.2).abs() < 0.01);
        // Half way to the outline along any ray is 0.5.
        let (dx, dy) = (140.0 * 0.6, 100.0 * 0.8);
        assert!((at(140.0 + dx * 0.5, 100.0 + dy * 0.5) - 0.5).abs() < 0.01);
        // A triangle: half way from its centre to its apex is 0.5.
        let mut tri = BezPath::new();
        tri.move_to((140.0, 0.0));
        tri.line_to((280.0, 200.0));
        tri.line_to((0.0, 200.0));
        tri.close_path();
        let polys = polygons(&tri);
        assert!((position("shape", b, CENTRE, &polys, Point::new(140.0, 50.0)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shape_paths_from_off_the_shape_or_a_focus_box() {
        let b = Rect::new(0.0, 0.0, 280.0, 200.0);
        let ellipse = polygons(&kurbo::Ellipse::from_rect(b).to_path(0.01));
        // Focus on the box's corner, outside the ellipse: contours are the ellipse shrunk towards
        // the corner, so its far edge is the end and its middle part way (it was all the end
        // colour).
        let corner = [0.0, 0.0, 1.0, 1.0];
        let mid = position("shape", b, corner, &ellipse, Point::new(140.0, 100.0));
        assert!(mid > 0.3 && mid < 0.8, "{mid}");
        assert!(position("shape", b, corner, &ellipse, Point::new(279.0, 100.0)) > 0.95);
        // A focus rectangle is the first colour inside, and the colour runs from its edge out.
        let boxed = [0.25, 0.25, 0.25, 0.25];
        assert_eq!(position("shape", b, boxed, &ellipse, Point::new(150.0, 110.0)), 0.0);
        let half = position("shape", b, boxed, &ellipse, Point::new(140.0 + 70.0 + 35.0, 100.0));
        assert!((half - 0.5).abs() < 0.02, "{half}");
    }

    fn many_segment_square(n: usize) -> BezPath {
        // A 200x200 square whose sides are split into `n` segments each.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        let corners = [(200.0, 0.0), (200.0, 200.0), (0.0, 200.0), (0.0, 0.0)];
        let mut from = (0.0, 0.0);
        for to in corners {
            for i in 1..=n {
                let t = i as f64 / n as f64;
                p.line_to((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
            }
            from = to;
        }
        p.close_path();
        p
    }

    #[test]
    fn dense_outlines_are_simplified_to_the_work_budget() {
        // Below the per-polygon flatten cap, so the simplifier has to do the work.
        let polys = polygons(&many_segment_square(20_000));
        assert!(polys.iter().map(|p| p.len() - 1).sum::<usize>() >= 80_000);
        let b = Rect::new(0.0, 0.0, 200.0, 200.0);
        let simple = simplify(polys, 200.0 / 384.0, 400);
        let edges: usize = simple.iter().map(|p| p.len() - 1).sum();
        assert!((MIN_POLY_EDGES..=400).contains(&edges), "{edges}");
        // Still the square: half way to a side or a corner is 0.5.
        for q in [Point::new(150.0, 100.0), Point::new(150.0, 150.0), Point::new(100.0, 50.0)] {
            let t = position("shape", b, CENTRE, &simple, q);
            assert!((t - 0.5).abs() < 0.01, "{q:?} {t}");
        }
        // Small outlines pass through untouched.
        let tri = vec![vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(0.0, 1.0), Point::new(0.0, 0.0)]];
        assert_eq!(simplify(tri.clone(), 10.0, 400), tri);
    }

    #[test]
    fn a_dense_shape_gradient_renders_promptly() {
        let path = many_segment_square(20_000);
        let mut ctx = RenderContext::new(400, 400);
        let stops = [(0.0, Rgba::rgb(0, 0, 0)), (1.0, Rgba::rgb(255, 255, 255))];
        let t = std::time::Instant::now();
        fill(&mut ctx, "shape", CENTRE, &stops, Rect::new(0.0, 0.0, 200.0, 200.0), &path, Affine::scale(2.0), 1.0);
        // Unbudgeted this is 147k pixels × 80k edges ≈ 1.2e10 tests (minutes); budgeted, well under a second in release.
        assert!(t.elapsed() < std::time::Duration::from_secs(20), "{:?}", t.elapsed());
    }

    #[test]
    fn a_shape_gradient_with_no_usable_outline_paints_nothing() {
        // One polygon over the per-polygon cap is dropped by `polygons`.
        let path = many_segment_square(26_000);
        assert!(polygons(&path).is_empty());
        let mut ctx = RenderContext::new(64, 64);
        let stops = [(0.0, Rgba::rgb(0, 0, 0)), (1.0, Rgba::rgb(255, 255, 255))];
        fill(&mut ctx, "shape", CENTRE, &stops, Rect::new(0.0, 0.0, 64.0, 64.0), &path, Affine::IDENTITY, 1.0);
        ctx.flush();
        let mut px = vec![0u8; 64 * 64 * 4];
        ctx.render(vello_cpu::PixmapMut::new(64, 64, &mut px).unwrap(), &mut vello_cpu::Resources::new());
        assert!(px.chunks(4).all(|p| p[3] == 0), "nothing was painted");
    }

    #[test]
    fn the_ramp_runs_through_the_stops() {
        let stops = [(0.0, Rgba::rgb(0, 0, 0)), (0.5, Rgba::rgb(128, 128, 128)), (1.0, Rgba::rgb(255, 255, 255))];
        let r = ramp(&stops);
        assert_eq!(r.len(), 256);
        assert_eq!(r[0], Rgba::rgb(0, 0, 0));
        assert_eq!(r[255], Rgba::rgb(255, 255, 255));
        assert!((r[64].r as i32 - 64).abs() <= 2);
        assert!(ramp(&[(0.3, Rgba::rgb(9, 9, 9))]).iter().all(|c| *c == Rgba::rgb(9, 9, 9)));
    }

    #[test]
    fn huge_cubics_are_refused_before_flattening() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        // About 250,000 quads each: over the limit.
        for _ in 0..3 {
            p.curve_to((1e15, -1e15), (-1e15, 1e15), (1.0, 1.0));
        }
        p.close_path();
        assert!(polygons(&p).is_empty());
        // A cubic of ordinary size still flattens.
        let mut q = BezPath::new();
        q.move_to((0.0, 0.0));
        q.curve_to((100.0, 0.0), (100.0, 100.0), (0.0, 100.0));
        q.close_path();
        assert_eq!(polygons(&q).len(), 1);
    }

    #[test]
    fn many_contours_keep_every_contour_at_a_coarser_raster() {
        let square = |x: f64| vec![Point::new(x, 0.0), Point::new(x + 1.0, 0.0), Point::new(x + 1.0, 1.0), Point::new(x, 1.0), Point::new(x, 0.0)];
        let polys: Vec<Vec<Point>> = (0..100).map(|i| square(f64::from(i) * 2.0)).collect();
        let simple = simplify(polys, 0.001, 244);
        assert_eq!(simple.len(), 100, "no contour is dropped");
        let (pw, ph) = fit_raster(400, 512, 512);
        assert!(u64::from(pw) * u64::from(ph) * 400 <= MAX_EDGE_WORK && pw > 100, "{pw}x{ph}");
        assert_eq!(fit_raster(200, 512, 512), (512, 512));
    }
}
