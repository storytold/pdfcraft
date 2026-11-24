use super::geometry::{color, scale_of};
use super::*;

/// Stroke `path` with `line`. A gradient line is laid out over `extent` (the untrimmed outline the
/// line belongs to; `path` itself when `None`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke(
    ctx: &mut RenderContext,
    rctx: &Ctx,
    line: &Line,
    ph: Option<Rgba>,
    path: &BezPath,
    extent: Option<&BezPath>,
    m: Affine,
    lw: f64,
    alpha: f64,
) {
    use deckcraft_model::style::{LineCap, LineJoin};
    let cap = match line.cap.unwrap_or_default() {
        LineCap::Flat => kurbo::Cap::Butt,
        LineCap::Round => kurbo::Cap::Round,
        LineCap::Square => kurbo::Cap::Square,
    };
    let join = match line.join.unwrap_or_default() {
        LineJoin::Round => kurbo::Join::Round,
        LineJoin::Bevel => kurbo::Join::Bevel,
        LineJoin::Miter => kurbo::Join::Miter,
    };
    // Hairlines stay visible at any zoom.
    let min_w = 0.5 / scale_of(m).max(1e-6);
    let sw = lw.max(min_w);
    // PowerPoint's default miter limit is 8 widths (`a:miter lim="800000"`).
    let mut sk = kurbo::Stroke::new(sw).with_caps(cap).with_join(join).with_miter_limit(8.0);
    // Square caps on a dashed line square off only the line's own two ends (where a dash is
    // drawn): each dash itself ends flat, as PowerPoint draws them.
    let mut square_ends = None;
    if let Some(d) = &line.dash {
        let pat: Vec<f64> = d.pattern().iter().map(|v| v * lw.max(0.75)).collect();
        let pat = dash_with_caps(pat, cap, sw);
        let pat = limit_dash_count(path, pat);
        if !pat.is_empty() && pat.iter().all(|v| *v > 0.0) {
            if cap == kurbo::Cap::Square {
                sk = sk.with_caps(kurbo::Cap::Butt);
                square_ends = Some(square_dash_ends(path, &pat, sw));
            }
            sk = sk.with_dashes(0.0, pat);
        }
    }
    ctx.set_transform(m);
    match &line.fill {
        Some(Fill::Solid { color: c }) => {
            ctx.set_paint(color(rctx.color(c, ph), alpha));
        }
        Some(Fill::Gradient(g)) if !g.stops.is_empty() => ctx.set_paint(line_gradient(rctx, g, ph, extent.unwrap_or(path), lw, alpha)),
        _ => ctx.set_paint(color(Rgba::BLACK, alpha)),
    }
    // Compound lines: each band stroked along the path moved to its place across the width.
    let compound = line.compound.unwrap_or_default();
    if compound != deckcraft_model::style::Compound::Single && lw * scale_of(m) >= 1.5 {
        let tolerance = 0.1 / scale_of(m).max(1e-6);
        for &(a, b) in compound::bands(compound) {
            let d = (-0.5 + (a + b) / 2.0) * lw;
            let mut band = sk.clone();
            band.width = (b - a) * lw;
            ctx.set_stroke(band);
            ctx.stroke_path(&compound::offset(path, d, tolerance));
        }
        return;
    }
    ctx.set_stroke(sk);
    ctx.stroke_path(path);
    if let Some(ends) = square_ends {
        ctx.fill_path(&ends);
    }
}

/// The square caps of a dashed line drawn `width` wide with dash `pattern`: a half-width square
/// past the start of each open sub-path (every dash pattern starts with a dash) and past its end
/// when the pattern is in a dash there.
fn square_dash_ends(path: &BezPath, pattern: &[f64], width: f64) -> BezPath {
    use kurbo::{ParamCurve, ParamCurveArclen};
    let mut out = BezPath::new();
    let period: f64 = pattern.iter().sum();
    let mut cap = |at: Point, dir: Vec2| {
        let n = Vec2::new(-dir.y, dir.x) * (width / 2.0);
        let back = dir * (width / 2.0);
        out.move_to(at + n);
        out.line_to(at + n + back);
        out.line_to(at - n + back);
        out.line_to(at - n);
        out.close_path();
    };
    let dir = |seg: &kurbo::PathSeg, t0: f64, t1: f64| {
        let d = seg.eval(t1) - seg.eval(t0);
        let d = if d.hypot() > 1e-9 { d } else { seg.end() - seg.start() };
        if d.hypot() > 1e-9 { Some(d.normalize()) } else { None }
    };
    let mut subs: Vec<(Vec<kurbo::PathSeg>, bool)> = vec![];
    let mut cur: Vec<kurbo::PathSeg> = vec![];
    let mut closed = false;
    let mut last = Point::ZERO;
    let mut start = Point::ZERO;
    for el in path.elements() {
        let seg = match *el {
            PathEl::MoveTo(p) => {
                if !cur.is_empty() {
                    subs.push((std::mem::take(&mut cur), closed));
                }
                closed = false;
                last = p;
                start = p;
                continue;
            }
            PathEl::LineTo(p) => kurbo::PathSeg::Line(kurbo::Line::new(last, p)),
            PathEl::QuadTo(a, p) => kurbo::PathSeg::Quad(kurbo::QuadBez::new(last, a, p)),
            PathEl::CurveTo(a, b, p) => kurbo::PathSeg::Cubic(kurbo::CubicBez::new(last, a, b, p)),
            PathEl::ClosePath => {
                closed = true;
                last = start;
                continue;
            }
        };
        last = seg.end();
        cur.push(seg);
    }
    if !cur.is_empty() {
        subs.push((cur, closed));
    }
    for segs in subs.iter().filter(|(s, c)| !c && !s.is_empty()).map(|(s, _)| s) {
        let (Some(first), Some(end)) = (segs.first(), segs.last()) else { continue };
        if let Some(d) = dir(first, 1e-3, 0.0) {
            cap(first.start(), d);
        }
        let len: f64 = segs.iter().map(|s| s.arclen(1e-3)).sum();
        if !(period > 0.0 && len.is_finite()) {
            continue;
        }
        let mut pos = len % period;
        // Ending exactly on a period boundary is the end of a gap.
        let mut on = pos > period * 1e-6;
        for v in pattern.iter().cycle().take(pattern.len()) {
            if pos <= *v {
                break;
            }
            pos -= v;
            on = !on;
        }
        if on && let Some(d) = dir(end, 1.0 - 1e-3, 1.0) {
            cap(end.end(), d);
        }
    }
    out
}

/// A gradient line's paint: laid out, as PowerPoint lays it, over the box the stroke covers (the
/// path's bounds grown by half the line width), so a horizontal line's vertical gradient runs
/// across its width.
fn line_gradient(rctx: &Ctx, g: &deckcraft_model::style::Gradient, ph: Option<Rgba>, path: &BezPath, lw: f64, alpha: f64) -> peniko::Gradient {
    let bounds = path.bounding_box().inflate(lw / 2.0, lw / 2.0);
    let bounds = paint::tile_box(bounds, g.tile);
    let stops: Vec<(f64, Rgba)> = g.stops.iter().map(|s| (s.pos, rctx.color(&s.color, ph))).collect();
    paint::gradient(&g.shape, stops, bounds, alpha)
}

/// A dash pattern (on, off, on, off… lengths) for a line drawn `width` wide with `cap`. As
/// PowerPoint draws them, round caps sit inside each dash's length rather than adding to it: a
/// "round dot" (`sysDot`, one width on and one off) is a row of separate circles, not a solid
/// line. So each dash loses a width (half at each end, where the caps go) and each gap gains it; a
/// dash no longer than the width becomes just its two caps. (Square-capped dashes end flat; see
/// `square_dash_ends`.)
pub(crate) fn dash_with_caps(mut pat: Vec<f64>, cap: kurbo::Cap, width: f64) -> Vec<f64> {
    if cap != kurbo::Cap::Round || !pat.len().is_multiple_of(2) || !(width.is_finite() && width > 0.0) {
        return pat;
    }
    for (i, v) in pat.iter_mut().enumerate() {
        *v = if i.is_multiple_of(2) { (*v - width).max(width * 1e-3) } else { *v + width };
    }
    pat
}

/// The most dashes one stroke is cut into. Stroking expands every dash into its own outline, so a
/// tiny dash on a huge shape (a 1 EMU-wide `custDash` on a 2147483647 EMU rectangle) would
/// otherwise need gigabytes and abort the app when the slide is drawn. Past this count the
/// pattern is scaled up evenly, which looks the same: the dashes are far below a pixel by then.
pub(crate) const MAX_DASHES: f64 = 20_000.0;

pub(crate) fn limit_dash_count(path: &BezPath, pat: Vec<f64>) -> Vec<f64> {
    use kurbo::ParamCurveArclen;
    let period: f64 = pat.iter().sum();
    if !(period.is_finite() && period > 0.0) {
        return pat;
    }
    let len: f64 = path.segments().map(|s| s.arclen(1e-2)).sum();
    // Every dash of the pattern counts, not each repeat of it: a custom pattern can hold many.
    let dashes = len / period * pat.len().div_ceil(2) as f64;
    if !len.is_finite() {
        return vec![];
    }
    if dashes <= MAX_DASHES {
        return pat;
    }
    let k = dashes / MAX_DASHES;
    pat.into_iter().map(|v| v * k).collect()
}

fn end_points(path: &BezPath) -> Option<((Point, Point), (Point, Point))> {
    let els: Vec<PathEl> = path.elements().to_vec();
    let mut pts: Vec<Point> = vec![];
    for e in &els {
        match e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => pts.push(*p),
            PathEl::QuadTo(a, b) => {
                pts.push(*a);
                pts.push(*b);
            }
            PathEl::CurveTo(a, b, c) => {
                pts.push(*a);
                pts.push(*b);
                pts.push(*c);
            }
            PathEl::ClosePath => {}
        }
    }
    if pts.len() < 2 {
        return None;
    }
    let first = *pts.first()?;
    let second = pts.iter().skip(1).find(|p| (**p - first).hypot() > 1e-6).copied().unwrap_or(*pts.get(1)?);
    let last = *pts.last()?;
    let prev = pts.iter().rev().skip(1).find(|p| (**p - last).hypot() > 1e-6).copied().unwrap_or(*pts.get(pts.len() - 2)?);
    Some(((first, second), (last, prev)))
}

/// An arrowhead's width and length for a line `lw` wide. PowerPoint sizes them in multiples of
/// the line width (small 2, medium 3, large 5), but never smaller than for a 2 pt line.
pub(crate) fn head_size(e: &deckcraft_model::style::LineEnd, lw: f64) -> (f64, f64) {
    let sz = |s: &str| match s {
        "sm" => 2.0,
        "lg" => 5.0,
        _ => 3.0,
    };
    let k = lw.max(2.0);
    (sz(&e.w) * k, sz(&e.len) * k)
}

/// How far back from the tip an open arrowhead's legs (`lw` wide) meet along their centre lines:
/// PowerPoint puts the outside of the point, not the legs' centre lines, on the line's end.
fn open_arrow_setback(wl: f64, ll: f64, lw: f64) -> f64 {
    let half = (wl / 2.0).atan2(ll);
    if half.sin() > 1e-6 { (lw / 2.0 / half.sin()).min(ll) } else { 0.0 }
}

/// How much of a `lw`-wide line to leave off at its start and end, under its arrowheads: up to
/// a triangle's base, into a stealth arrow's notch far enough for its barbs to cover the line's
/// corners, and to where an open arrow's legs meet. Ovals and diamonds sit on the end itself.
pub(crate) fn head_trims(line: &Line, lw: f64) -> (f64, f64) {
    // A square cap reaches half a width past the line's end: leave that off too.
    let cap = if line.cap == Some(deckcraft_model::style::LineCap::Square) { lw / 2.0 } else { 0.0 };
    let trim = |e: &Option<deckcraft_model::style::LineEnd>| {
        let Some(e) = e else { return 0.0 };
        let (wl, ll) = head_size(e, lw);
        match e.kind.as_str() {
            "triangle" => ll + cap,
            "stealth" => (ll * (0.6 + 0.4 * lw / wl.max(1e-9))).min(ll) + cap,
            "arrow" => open_arrow_setback(wl, ll, lw.max(0.75)) + cap,
            _ => 0.0,
        }
    };
    (trim(&line.head), trim(&line.tail))
}

/// `path` with `start` left off the beginning of its first sub-path and `end` off the end of its
/// last (by arc length), where those are open. A sub-path shorter than its trims disappears.
pub(crate) fn trim_ends(path: &BezPath, start: f64, end: f64) -> BezPath {
    use kurbo::{ParamCurve, ParamCurveArclen};
    const ACC: f64 = 1e-3;
    let mut subs: Vec<(Vec<kurbo::PathSeg>, bool)> = vec![];
    let (mut last, mut first) = (Point::ZERO, Point::ZERO);
    for el in path.elements() {
        let seg = match *el {
            PathEl::MoveTo(p) => {
                subs.push((vec![], false));
                last = p;
                first = p;
                continue;
            }
            PathEl::ClosePath => {
                if let Some(s) = subs.last_mut() {
                    s.1 = true;
                }
                last = first;
                continue;
            }
            PathEl::LineTo(p) => kurbo::PathSeg::Line(kurbo::Line::new(last, p)),
            PathEl::QuadTo(a, p) => kurbo::PathSeg::Quad(kurbo::QuadBez::new(last, a, p)),
            PathEl::CurveTo(a, b, p) => kurbo::PathSeg::Cubic(kurbo::CubicBez::new(last, a, b, p)),
        };
        last = seg.end();
        if subs.is_empty() {
            subs.push((vec![], false));
        }
        if let Some(s) = subs.last_mut() {
            s.0.push(seg);
        }
    }
    if let Some((segs, false)) = subs.first_mut()
        && start > 0.0
    {
        let mut left = start;
        let mut drop_count = 0;
        for (i, seg) in segs.iter().enumerate() {
            let l = seg.arclen(ACC);
            if l > left {
                let partial = seg.subsegment(seg.inv_arclen(left, ACC)..1.0);
                segs.drain(..i);
                segs[0] = partial;
                drop_count = 0;
                break;
            }
            left -= l;
            drop_count = i + 1;
        }
        if drop_count > 0 {
            segs.drain(..drop_count);
        }
    }
    if let Some((segs, false)) = subs.last_mut()
        && end > 0.0
    {
        let mut left = end;
        while let Some(seg) = segs.last().copied() {
            let l = seg.arclen(ACC);
            if l > left {
                if let Some(s1) = segs.last_mut() {
                    *s1 = seg.subsegment(0.0..seg.inv_arclen(l - left, ACC));
                }
                break;
            }
            left -= l;
            segs.pop();
        }
    }
    let mut out = BezPath::new();
    for (segs, closed) in subs {
        let Some(s0) = segs.first() else { continue };
        out.move_to(s0.start());
        for seg in &segs {
            match seg {
                kurbo::PathSeg::Line(l) => out.line_to(l.p1),
                kurbo::PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                kurbo::PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
            }
        }
        if closed {
            out.close_path();
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn arrowheads(ctx: &mut RenderContext, rctx: &Ctx, line: &Line, ph: Option<Rgba>, geo: &Geometry, rel: Affine, m: Affine, lw: f64) {
    let Some(sp) = geo.paths.iter().find(|p| p.stroke) else { return };
    let Some(((a, a2), (b, b2))) = end_points(&sp.path) else { return };
    // The outline's ends in the space the arrowheads are sized and drawn in.
    let (a, a2, b, b2) = (rel * a, rel * a2, rel * b, rel * b2);
    let col = match &line.fill {
        Some(Fill::Solid { color: c }) => rctx.color(c, ph),
        _ => Rgba::BLACK,
    };
    // A gradient line's heads take the gradient where they are.
    let grad = match &line.fill {
        Some(Fill::Gradient(g)) if !g.stops.is_empty() => {
            let mut moved = sp.path.clone();
            moved.apply_affine(rel);
            Some(line_gradient(rctx, g, ph, &moved, lw, 1.0))
        }
        _ => None,
    };
    let set_paint = |ctx: &mut RenderContext| match &grad {
        Some(g) => ctx.set_paint(g.clone()),
        None => ctx.set_paint(color(col, 1.0)),
    };
    for (end, tip, from) in [(&line.head, a, a2), (&line.tail, b, b2)] {
        let Some(e) = end else { continue };
        if e.kind == "none" {
            continue;
        }
        let (wl, ll) = head_size(e, lw);
        let dir = (tip - from).normalize();
        if !dir.x.is_finite() {
            continue;
        }
        let n = Vec2::new(-dir.y, dir.x);
        let base = tip - dir * ll;
        let mut p = BezPath::new();
        match e.kind.as_str() {
            "oval" => {
                let c = kurbo::Ellipse::new(tip, (ll / 2.0, wl / 2.0), dir.y.atan2(dir.x));
                p = c.to_path(0.05);
            }
            "diamond" => {
                p.move_to(tip + dir * ll / 2.0);
                p.line_to(tip + n * wl / 2.0);
                p.line_to(tip - dir * ll / 2.0);
                p.line_to(tip - n * wl / 2.0);
                p.close_path();
            }
            "stealth" => {
                p.move_to(tip);
                p.line_to(base + n * wl / 2.0);
                p.line_to(tip - dir * ll * 0.6);
                p.line_to(base - n * wl / 2.0);
                p.close_path();
            }
            "arrow" => {
                // Legs the line's width with round ends, meeting in a sharp point whose outside is
                // on the line's end.
                let leg = lw.max(0.75);
                let back = dir * open_arrow_setback(wl, ll, leg);
                let mut o = BezPath::new();
                o.move_to(base + n * wl / 2.0 - back);
                o.line_to(tip - back);
                o.line_to(base - n * wl / 2.0 - back);
                ctx.set_transform(m);
                set_paint(ctx);
                ctx.set_stroke(kurbo::Stroke::new(leg).with_join(kurbo::Join::Miter).with_miter_limit(100.0).with_caps(kurbo::Cap::Round));
                ctx.stroke_path(&o);
                continue;
            }
            _ => {
                p.move_to(tip);
                p.line_to(base + n * wl / 2.0);
                p.line_to(base - n * wl / 2.0);
                p.close_path();
            }
        }
        ctx.set_transform(m);
        set_paint(ctx);
        ctx.fill_path(&p);
    }
}
