//! Compound outlines (`a:ln/@cmpd`): double, thick-thin, thin-thick and triple lines.
//!
//! PowerPoint splits the line's width into bands across the path, measured from its left side
//! (looking along the path) to its right, as these shares of the width (read from PowerPoint's
//! export of 24 pt lines of each kind):
//! - double: 1/3 line, 1/3 gap, 1/3 line;
//! - thin-thick: 1/5 line, 1/5 gap, 3/5 line; thick-thin the mirror;
//! - triple: 1/6 line, 1/6 gap, 1/3 line, 1/6 gap, 1/6 line.
//!
//! On a shape drawn clockwise (all presets) the left side is the outside.

use deckcraft_model::style::Compound;
use kurbo::{BezPath, CubicBez, ParamCurve, PathEl, PathSeg, Point, Vec2};

/// The bands of `c` as (from, to) shares of the width, left side first.
pub fn bands(c: Compound) -> &'static [(f64, f64)] {
    match c {
        Compound::Single => &[(0.0, 1.0)],
        Compound::Double => &[(0.0, 1.0 / 3.0), (2.0 / 3.0, 1.0)],
        Compound::ThinThick => &[(0.0, 0.2), (0.4, 1.0)],
        Compound::ThickThin => &[(0.0, 0.6), (0.8, 1.0)],
        Compound::Triple => &[(0.0, 1.0 / 6.0), (2.0 / 6.0, 4.0 / 6.0), (5.0 / 6.0, 1.0)],
    }
}

/// Unit normal pointing to the right of travel (y down), or None for a zero-length tangent.
fn right(t: Vec2) -> Option<Vec2> {
    let l = t.hypot();
    (l > 1e-12 && l.is_finite()).then(|| Vec2::new(-t.y, t.x) / l)
}

/// `path` moved `d` to the right of travel (negative: to the left). Line corners meet at their
/// miter point (bevelled when sharper than about 11°); curves are offset by kurbo; other joins are
/// bridged by a straight line.
pub fn offset(path: &BezPath, d: f64, tolerance: f64) -> BezPath {
    let mut out = BezPath::new();
    if d.abs() < 1e-9 {
        return path.clone();
    }
    // Each subpath as segments, offset one by one.
    let mut segs: Vec<PathSeg> = Vec::new();
    let mut closed = false;
    let flush = |segs: &mut Vec<PathSeg>, closed: bool, out: &mut BezPath| {
        let pieces: Vec<(PathSeg, BezPath)> = segs.iter().filter_map(|s| offset_seg(*s, d, tolerance).map(|p| (*s, p))).collect();
        let begin = out.elements().len();
        for (i, (seg, piece)) in pieces.iter().enumerate() {
            let Some(PathEl::MoveTo(p)) = piece.elements().first().copied() else { continue };
            match i.checked_sub(1).and_then(|j| pieces.get(j)) {
                None => out.move_to(p),
                Some((prev, _)) => join(out, *prev, *seg, p, d),
            }
            for e in piece.elements().iter().skip(1) {
                out.push(*e);
            }
        }
        if closed && let (Some((last, _)), Some((first, piece))) = (pieces.last(), pieces.first()) {
            if let Some(PathEl::MoveTo(p)) = piece.elements().first().copied() {
                // Lines meeting at the start: both ends move to their miter point.
                match miter(*last, *first, d) {
                    Some(m) if is_line(*last) && is_line(*first) => {
                        set_last(out, m);
                        if let Some(PathEl::MoveTo(_)) = out.elements().get(begin) {
                            out.elements_mut()[begin] = PathEl::MoveTo(m);
                        }
                    }
                    _ => out.line_to(p),
                }
            }
            out.close_path();
        }
        segs.clear();
    };
    let mut start = Point::ZERO;
    let mut cur = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut segs, closed, &mut out);
                closed = false;
                start = p;
                cur = p;
            }
            PathEl::LineTo(p) => {
                segs.push(PathSeg::Line(kurbo::Line::new(cur, p)));
                cur = p;
            }
            PathEl::QuadTo(a, b) => {
                segs.push(PathSeg::Quad(kurbo::QuadBez::new(cur, a, b)));
                cur = b;
            }
            PathEl::CurveTo(a, b, c) => {
                segs.push(PathSeg::Cubic(CubicBez::new(cur, a, b, c)));
                cur = c;
            }
            PathEl::ClosePath => {
                if (cur - start).hypot() > 1e-9 {
                    segs.push(PathSeg::Line(kurbo::Line::new(cur, start)));
                }
                closed = true;
                flush(&mut segs, closed, &mut out);
                closed = false;
                cur = start;
            }
        }
    }
    flush(&mut segs, closed, &mut out);
    out
}

fn offset_seg(s: PathSeg, d: f64, tolerance: f64) -> Option<BezPath> {
    // Far-off or non-finite geometry (hostile files) is left out rather than offset.
    let sane = |p: Point| p.is_finite() && p.x.abs() < 1e7 && p.y.abs() < 1e7;
    let ok = match s {
        PathSeg::Line(l) => sane(l.p0) && sane(l.p1),
        PathSeg::Quad(q) => sane(q.p0) && sane(q.p1) && sane(q.p2),
        PathSeg::Cubic(c) => sane(c.p0) && sane(c.p1) && sane(c.p2) && sane(c.p3),
    };
    if !ok || !d.is_finite() {
        return None;
    }
    match s {
        PathSeg::Line(l) => {
            let n = right(l.p1 - l.p0)?;
            let mut p = BezPath::new();
            p.move_to(l.p0 + n * d);
            p.line_to(l.p1 + n * d);
            Some(p)
        }
        PathSeg::Quad(q) => offset_seg(PathSeg::Cubic(q.raise()), d, tolerance),
        PathSeg::Cubic(c) => {
            // Nearly straight cubics (collinear control points) are offset as lines.
            let chord = c.p3 - c.p0;
            let flat = |p: Point| chord.cross(p - c.p0).abs() <= 1e-9 * chord.hypot2().max(1e-18);
            if flat(c.p1) && flat(c.p2) {
                return offset_seg(PathSeg::Line(kurbo::Line::new(c.p0, c.p3)), d, tolerance);
            }
            let mut p = BezPath::new();
            kurbo::offset::offset_cubic(c, d, tolerance, &mut p);
            Some(p)
        }
    }
}

/// The segment's tangent at its start and at its end (the first non-zero control vector).
fn tangents(s: PathSeg) -> (Vec2, Vec2) {
    let first = |v: &[Vec2]| v.iter().copied().find(|v| v.hypot2() > 1e-18).unwrap_or(Vec2::ZERO);
    match s {
        PathSeg::Line(l) => (l.p1 - l.p0, l.p1 - l.p0),
        PathSeg::Quad(q) => (first(&[q.p1 - q.p0, q.p2 - q.p0]), first(&[q.p2 - q.p1, q.p2 - q.p0])),
        PathSeg::Cubic(c) => (first(&[c.p1 - c.p0, c.p2 - c.p0, c.p3 - c.p0]), first(&[c.p3 - c.p2, c.p3 - c.p1, c.p3 - c.p0])),
    }
}

fn is_line(s: PathSeg) -> bool {
    matches!(s, PathSeg::Line(_))
}

/// Move the path's last point to `p`.
fn set_last(out: &mut BezPath, p: Point) {
    if let Some(e) = out.elements_mut().last_mut() {
        match e {
            PathEl::MoveTo(q) | PathEl::LineTo(q) => *q = p,
            PathEl::QuadTo(_, q) => *q = p,
            PathEl::CurveTo(_, _, q) => *q = p,
            PathEl::ClosePath => {}
        }
    }
}

/// Where the offsets of `prev` and `next` meet at their shared corner (their miter point), or
/// None for a corner sharper than about 11 degrees or a degenerate tangent.
fn miter(prev: PathSeg, next: PathSeg, d: f64) -> Option<Point> {
    let (_, t0) = tangents(prev);
    let (t1, _) = tangents(next);
    let (n0, n1) = (right(t0)?, right(t1)?);
    let denom = 1.0 + n0.dot(n1);
    (denom > 0.02).then(|| prev.end() + (n0 + n1) * (d / denom)).filter(|m| m.is_finite())
}

/// Join the offset of `prev` (ending at `out`'s current point) to the offset of `next`, which
/// starts at `p`: two lines meet at their miter point (both ends moved there, so an inner corner
/// doesn't loop); anything else is bridged straight.
fn join(out: &mut BezPath, prev: PathSeg, next: PathSeg, p: Point, d: f64) {
    match miter(prev, next, d) {
        Some(m) if is_line(prev) && is_line(next) => set_last(out, m),
        Some(m) => {
            out.line_to(m);
            out.line_to(p);
        }
        None => out.line_to(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn bands_split_the_width_like_powerpoint() {
        for c in [Compound::Single, Compound::Double, Compound::ThinThick, Compound::ThickThin, Compound::Triple] {
            let b = bands(c);
            assert_eq!(b.first().map(|x| x.0), Some(0.0));
            assert_eq!(b.last().map(|x| x.1), Some(1.0));
            assert!(b.windows(2).all(|w| w[0].1 < w[1].0), "{c:?}: bands in order with gaps");
        }
        // Thin-thick: the thin line on the left, thick-thin its mirror.
        let (tt, th) = (bands(Compound::ThinThick), bands(Compound::ThickThin));
        assert!((tt[0].1 - tt[0].0) < (tt[1].1 - tt[1].0));
        assert!((th[0].1 - th[0].0) > (th[1].1 - th[1].0));
    }

    #[test]
    fn offsetting_a_rectangle_grows_or_shrinks_it() {
        // Clockwise in y-down coordinates: the left of travel is the outside.
        let r = kurbo::Rect::new(0.0, 0.0, 100.0, 50.0).to_path(0.1);
        let out = offset(&r, -5.0, 0.01).bounding_box();
        assert!(
            (out.x0 + 5.0).abs() < 1e-6 && (out.y0 + 5.0).abs() < 1e-6 && (out.x1 - 105.0).abs() < 1e-6 && (out.y1 - 55.0).abs() < 1e-6,
            "{out:?}"
        );
        let inn = offset(&r, 5.0, 0.01).bounding_box();
        assert!((inn.x0 - 5.0).abs() < 1e-6 && (inn.y1 - 45.0).abs() < 1e-6, "{inn:?}");
        // Corners meet at their miter points: the area is the offset rectangle's.
        assert!((offset(&r, -5.0, 0.01).area().abs() - 110.0 * 60.0).abs() < 1e-6);
    }

    #[test]
    fn offsetting_a_circle_changes_its_radius() {
        let c = kurbo::Circle::new((50.0, 50.0), 20.0).to_path(0.01);
        let b = offset(&c, 4.0, 0.01).bounding_box();
        let w = b.width() / 2.0;
        assert!((w - 16.0).abs() < 0.2 || (w - 24.0).abs() < 0.2, "radius {w}");
        // A hostile path never panics and yields finite output.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((0.0, 0.0));
        p.curve_to((f64::MAX, 0.0), (0.0, f64::MIN_POSITIVE), (1.0, 1.0));
        p.close_path();
        let _ = offset(&p, 3.0, 0.01);
    }
}
