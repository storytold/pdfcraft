//! Drawing laid-out text through a WordArt warp ([`deckcraft_geom::warp`]).
//!
//! Glyph outlines are placed in layout space, flattened, cut into short segments and every point
//! is mapped through the warp, so straight glyph edges bend with the guides. Underlines and
//! highlights are warped the same way.

use deckcraft_geom::warp::{Warp, WarpLine};
use deckcraft_text::{GlyphRun, TextLayout};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _};
use vello_cpu::{RenderContext, peniko};

use crate::text_draw::LineIndex;
use crate::{ParaState, color, glyph, scale_of, text_transform};

/// Most points one warped glyph may have (a hostile size or zoom can't explode the work).
const MAX_POINTS: usize = 40_000;

/// The line of `l` a glyph of paragraph `para` on baseline `y` belongs to, by scanning.
pub(crate) fn line_of_scan(l: &TextLayout, para: usize, y: f64) -> Option<usize> {
    if let Some(&(first, count)) = l.para_lines.get(para) {
        if count == 0 {
            return None;
        }
        let end = (first + count).min(l.lines.len());
        if first < end {
            let mut best: Option<(usize, f64)> = None;
            for (rel_i, li) in l.lines[first..end].iter().enumerate() {
                let inside = y >= li.top - 0.01 && y <= li.bottom + 0.01;
                let d = if inside { 0.0 } else { (li.baseline - y).abs() };
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((first + rel_i, d));
                }
            }
            return best.map(|(i, _)| i);
        }
    }
    let mut best: Option<(usize, f64)> = None;
    for (i, li) in l.lines.iter().enumerate().filter(|(_, li)| li.para == para) {
        let inside = y >= li.top - 0.01 && y <= li.bottom + 0.01;
        let d = if inside { 0.0 } else { (li.baseline - y).abs() };
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Where a line meets a follow guide, above its baseline: 3/8 of the way up the font's
/// ascent + descent box (measured on Arial, Times New Roman and Courier New in PowerPoint).
fn ref_offset(run: &GlyphRun) -> f64 {
    let f = &run.face;
    let k = run.size / f.upem.max(1.0);
    ((0.375 * (f.ascent + f.descent) - f.descent) * k).clamp(-run.size, run.size)
}

/// `path` flattened with tolerance `tol`, cut into pieces of at most `seg` and mapped by `f`.
fn warp_path(path: &BezPath, tol: f64, seg: f64, f: &dyn Fn(Point) -> Point) -> BezPath {
    let mut out = BezPath::new();
    let mut last: Option<Point> = None;
    let mut start: Option<Point> = None;
    let mut budget = MAX_POINTS;
    let segment = |out: &mut BezPath, a: Point, b: Point, budget: &mut usize| {
        let n = ((b - a).hypot() / seg).ceil();
        let n = if n.is_finite() { (n as usize).clamp(1, 512) } else { 1 };
        for i in 1..=n {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            out.line_to(f(a.lerp(b, i as f64 / n as f64)));
        }
    };
    kurbo::flatten(path.iter(), tol, |el| match el {
        PathEl::MoveTo(p) => {
            if let (Some(a), Some(s)) = (last, start)
                && (a - s).hypot() > 1e-9
            {
                segment(&mut out, a, s, &mut budget);
            }
            out.move_to(f(p));
            last = Some(p);
            start = Some(p);
        }
        PathEl::LineTo(p) => {
            if let Some(a) = last {
                segment(&mut out, a, p, &mut budget);
            }
            last = Some(p);
        }
        PathEl::ClosePath => {
            if let (Some(a), Some(s)) = (last, start) {
                segment(&mut out, a, s, &mut budget);
            }
            out.close_path();
            last = start;
        }
        _ => {}
    });
    out
}

struct Item<'a> {
    run: &'a GlyphRun,
    path: BezPath,
    line: usize,
    state: ParaState,
}

/// Draw `l` (laid out in text rect `tr`, shape transform `m`) through `warp`.
pub(crate) fn draw(ctx: &mut RenderContext, l: &TextLayout, tr: Rect, m: Affine, paras: &[(usize, ParaState)], warp: &Warp) {
    let para = |i: usize| paras.iter().find(|(p, _)| *p == i).map(|(_, s)| *s).unwrap_or_default();
    let m = text_transform(m, tr, l.rotation);
    let n = l.lines.len();
    let lines_of = LineIndex::new(l);
    let mut ink: Vec<Option<Rect>> = vec![None; n];
    let mut off: Vec<Option<f64>> = vec![None; n];
    let mut items: Vec<Item> = Vec::new();
    for run in &l.runs {
        let k = run.size / run.face.upem.max(1.0);
        let skew = if run.fake_italic { Affine::new([1.0, 0.0, -0.2, 1.0, 0.0, 0.0]) } else { Affine::IDENTITY };
        for (gid, x, y) in &run.glyphs {
            let Some(li) = lines_of.line_of(run.para, *y) else { continue };
            if let Some(o) = off.get_mut(li) {
                *o = Some(o.map_or(ref_offset(run), |v| v.max(ref_offset(run))));
            }
            let outline = glyph(&run.face, *gid);
            if outline.elements().is_empty() {
                continue;
            }
            let mut path = (*outline).clone();
            path.apply_affine(Affine::translate((*x, *y)) * skew * Affine::scale(k));
            let b = path.bounding_box();
            if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) {
                continue;
            }
            if let Some(slot) = ink.get_mut(li) {
                *slot = Some(slot.map_or(b, |r| r.union(b)));
            }
            items.push(Item { run, path, line: li, state: para(run.para) });
        }
    }
    let lines: Vec<WarpLine> = l
        .lines
        .iter()
        .enumerate()
        .map(|(i, li)| {
            let o = off.get(i).copied().flatten().unwrap_or(0.18 * (li.bottom - li.top));
            WarpLine { ink: ink.get(i).copied().flatten(), extent: li.extent, ref_y: li.baseline - o }
        })
        .collect();
    let mapper = warp.mapper(&lines, l.inner);
    let px = scale_of(m).max(1e-6) * mapper.zoom();
    let tol = (0.1 / px).clamp(1e-4, 2.0);
    let seg = (2.0 / px).clamp(1e-3, 50.0);
    let deco = |ctx: &mut RenderContext, d: &deckcraft_text::Deco| {
        let ps = para(d.para);
        if !ps.visible || ps.opacity <= 0.001 {
            return;
        }
        let y = (d.rect.y0 + d.rect.y1) / 2.0;
        let Some(li) = lines_of.line_of(d.para, y) else { return };
        ctx.set_transform(m * Affine::translate(ps.offset));
        ctx.set_paint(color(d.color, ps.opacity));
        for r in d.pieces() {
            let path = warp_path(&r.to_path(0.1), tol, seg, &|p| mapper.map(li, p));
            ctx.fill_path(&path);
        }
    };
    for d in l.decos.iter().filter(|d| d.behind) {
        deco(ctx, d);
    }
    for it in &items {
        let (run, ps) = (it.run, it.state);
        if !ps.visible || ps.opacity <= 0.001 {
            continue;
        }
        let alpha = run.alpha * ps.opacity;
        if (alpha <= 0.0 || run.color.a == 0) && run.outline.is_none() {
            continue;
        }
        let path = warp_path(&it.path, tol, seg, &|p| mapper.map(it.line, p));
        ctx.set_transform(m * Affine::translate(ps.offset));
        if alpha > 0.0 && run.color.a > 0 {
            ctx.set_paint(color(run.color, alpha));
            ctx.fill_path(&path);
            if run.fake_bold {
                ctx.set_stroke(kurbo::Stroke::new(run.size * 0.025).with_join(kurbo::Join::Round));
                ctx.stroke_path(&path);
            }
        }
        if let Some((oc, ow)) = run.outline {
            ctx.set_paint(color(oc, ps.opacity));
            ctx.set_stroke(kurbo::Stroke::new(ow.max(0.0)).with_join(kurbo::Join::Round));
            ctx.stroke_path(&path);
        }
    }
    for d in l.decos.iter().filter(|d| !d.behind) {
        deco(ctx, d);
    }
    ctx.set_fill_rule(peniko::Fill::NonZero);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warp_path_subdivides_and_maps_every_point() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        p.close_path();
        let out = warp_path(&p, 0.01, 1.0, &|q| Point::new(q.x, q.y + q.x * q.x * 0.01));
        // 10 + 10 + ~15 pieces of at most 1 pt.
        let n = out.elements().len();
        assert!(n > 30, "{n}");
        // The first edge is bent onto the parabola.
        let mid = out.elements().iter().find_map(|e| if let PathEl::LineTo(q) = e { (q.x == 5.0).then_some(*q) } else { None });
        assert_eq!(mid, Some(Point::new(5.0, 0.25)));
        // A huge path with a tiny segment length stays bounded.
        let mut big = BezPath::new();
        big.move_to((0.0, 0.0));
        for i in 0..1000 {
            big.line_to((1e6 * (i % 2) as f64, i as f64));
        }
        let out = warp_path(&big, 0.01, 1e-3, &|q| q);
        assert!(out.elements().len() <= MAX_POINTS + 2);
    }
}
