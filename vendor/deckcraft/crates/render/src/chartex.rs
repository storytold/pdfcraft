//! Office 2016 chart layouts the bar renderer can't draw: funnel, treemap, sunburst and the
//! boxes of a box and whisker chart (waterfall bars are drawn by [`crate::chart`], histograms as
//! the column chart of their bins).

use deckcraft_color::Rgba;
use deckcraft_model::chart::{BoxStats, Chart, ChartType, TreeNode};
use deckcraft_model::resolve::Ctx;
use deckcraft_model::text::{Align, Anchor};
use kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use vello_cpu::RenderContext;

use crate::chart::{label, series_color};
use crate::color;

/// Draw `c` inside `area` (the space left by the title and legend).
pub fn draw(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64, text_col: Rgba) {
    let Some(s) = c.series.first() else { return };
    if c.kind == ChartType::RegionMap {
        region_tiles(ctx, rctx, c, m, area, base, text_col);
        return;
    }
    let vals: Vec<f64> = s.values.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect();
    if vals.iter().all(|v| *v <= 0.0) {
        return;
    }
    let value_label = |v: f64| c.data_label(s, v);
    let labels = c.series_data_labels(s) || c.kind == ChartType::Funnel;
    match c.kind {
        ChartType::Funnel => funnel(ctx, rctx, c, m, area, base, text_col, &vals, &value_label),
        ChartType::Treemap if !c.category_levels.is_empty() => tree_treemap(ctx, rctx, c, m, area, base, labels.then_some(&value_label)),
        ChartType::Treemap => treemap(ctx, rctx, c, m, area, base, &vals, labels.then_some(&value_label)),
        _ if !c.category_levels.is_empty() => tree_sunburst(ctx, rctx, c, m, area, base),
        _ => sunburst(ctx, rctx, c, m, area, base, &vals),
    }
}

/// Bars centred on one axis, as wide as their value, widest first as given: category labels on
/// the left, values inside.
#[allow(clippy::too_many_arguments)]
fn funnel(
    ctx: &mut RenderContext,
    rctx: &Ctx,
    c: &Chart,
    m: Affine,
    area: Rect,
    base: f64,
    text_col: Rgba,
    vals: &[f64],
    value_label: &dyn Fn(f64) -> String,
) {
    let n = vals.len().max(1);
    let gutter = c.categories.iter().map(|n| n.chars().count()).max().unwrap_or(0) as f64 * base * 0.55 + base;
    let plot = Rect::new(area.x0 + gutter.min(area.width() * 0.4), area.y0, area.x1, area.y1);
    let max = vals.iter().copied().fold(0.0, f64::max).max(1e-9);
    let band = plot.height() / n as f64;
    let cx = plot.center().x;
    for (i, v) in vals.iter().enumerate() {
        let w = v / max * plot.width();
        let y = plot.y0 + band * i as f64;
        let r = Rect::new(cx - w / 2.0, y + band * 0.05, cx + w / 2.0, y + band * 0.95);
        ctx.set_transform(m);
        ctx.set_paint(color(series_color(rctx, c, 0), 1.0));
        ctx.fill_rect(&r);
        if let Some(name) = c.categories.get(i) {
            label(
                ctx,
                rctx,
                m,
                name,
                Rect::new(area.x0, y, plot.x0 - base * 0.4, y + band),
                crate::chart::LabelStyle { size: base, align: Align::Right, anchor: Anchor::Middle, col: text_col, bold: false },
            );
        }
        label(
            ctx,
            rctx,
            m,
            &value_label(*v),
            Rect::new(cx - 60.0, y, cx + 60.0, y + band),
            crate::chart::LabelStyle { size: base, align: Align::Center, anchor: Anchor::Middle, col: Rgba::WHITE, bold: false },
        );
    }
}

/// Mix `a` towards `b` by `t` (0..1).
fn mix(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round().clamp(0.0, 255.0) as u8;
    Rgba::rgb(f(a.r, b.r), f(a.g, b.g), f(a.b, b.b))
}

/// A region map without its geography: the boundaries PowerPoint caches come from an online
/// map service in an undocumented format and DeckCraft ships no map data, so each region is a
/// tile in a grid, coloured on the map's value scale, with the scale below and a note saying
/// the map itself is not drawn.
fn region_tiles(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64, text_col: Rgba) {
    let Some(s) = c.series.first() else { return };
    let regions: Vec<(String, Option<f64>)> =
        c.categories.iter().enumerate().map(|(i, n)| (n.clone(), s.values.get(i).copied().flatten())).filter(|(n, _)| !n.is_empty()).collect();
    let note_h = base * 1.4;
    let scale_h = base * 2.6;
    let grid = Rect::new(area.x0, area.y0, area.x1, (area.y1 - note_h - scale_h).max(area.y0));
    let hi = series_color(rctx, c, 0);
    let lo = mix(hi, Rgba::WHITE, 0.85);
    let (min, max) = regions.iter().filter_map(|(_, v)| *v).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let span = if max > min { max - min } else { 1.0 };
    let n = regions.len();
    if n > 0 && grid.area() > 0.0 {
        // Columns so that tiles come out about twice as wide as tall.
        let aspect = grid.width() / grid.height().max(1.0);
        let cols = ((n as f64 * aspect / 2.0).sqrt().ceil() as usize).clamp(1, n);
        let rows = n.div_ceil(cols).max(1);
        let (tw, th) = (grid.width() / cols as f64, grid.height() / rows as f64);
        for (i, (name, v)) in regions.iter().enumerate() {
            let (col, row) = (i % cols, i / cols);
            let t = Rect::new(grid.x0 + tw * col as f64, grid.y0 + th * row as f64, grid.x0 + tw * (col + 1) as f64, grid.y0 + th * (row + 1) as f64)
                .inset(-1.5);
            let fill = v.map_or(Rgba::rgb(0xD9, 0xD9, 0xD9), |v| mix(lo, hi, (v - min) / span));
            ctx.set_transform(m);
            ctx.set_paint(color(fill, 1.0));
            ctx.fill_rect(&t);
            let dark = v.is_some_and(|v| (v - min) / span > 0.55);
            let mut text = name.clone();
            if c.series_data_labels(s)
                && let Some(v) = v
            {
                text = format!(
                    "{text}
{}",
                    c.data_label(s, *v)
                );
            }
            let inner = t.inset(-base * 0.25);
            if inner.width() > base && inner.height() > base * 0.8 {
                label(
                    ctx,
                    rctx,
                    m,
                    &text,
                    inner,
                    crate::chart::LabelStyle {
                        size: base * 0.9,
                        align: Align::Center,
                        anchor: Anchor::Middle,
                        col: if dark { Rgba::WHITE } else { text_col },
                        bold: false,
                    },
                );
            }
        }
    }
    // The colour scale: a gradient bar from the lowest to the highest value.
    if min <= max {
        let bar_w = (area.width() * 0.5).min(base * 24.0);
        let y = grid.y1 + base * 0.6;
        let bar = Rect::new(area.center().x - bar_w / 2.0, y, area.center().x + bar_w / 2.0, y + base * 0.8);
        let steps = 48;
        for k in 0..steps {
            let x0 = bar.x0 + bar.width() * k as f64 / steps as f64;
            let x1 = bar.x0 + bar.width() * (k + 1) as f64 / steps as f64 + 0.5;
            ctx.set_transform(m);
            ctx.set_paint(color(mix(lo, hi, k as f64 / (steps - 1) as f64), 1.0));
            ctx.fill_rect(&Rect::new(x0, bar.y0, x1.min(bar.x1), bar.y1));
        }
        let ly = Rect::new(bar.x0 - base * 8.0, bar.y0 - base * 0.2, bar.x0 - base * 0.4, bar.y1 + base * 0.2);
        label(
            ctx,
            rctx,
            m,
            &c.data_label(s, min),
            ly,
            crate::chart::LabelStyle { size: base, align: Align::Right, anchor: Anchor::Middle, col: text_col, bold: false },
        );
        let ry = Rect::new(bar.x1 + base * 0.4, bar.y0 - base * 0.2, bar.x1 + base * 8.0, bar.y1 + base * 0.2);
        label(
            ctx,
            rctx,
            m,
            &c.data_label(s, max),
            ry,
            crate::chart::LabelStyle { size: base, align: Align::Left, anchor: Anchor::Middle, col: text_col, bold: false },
        );
    }
    let note = Rect::new(area.x0, area.y1 - note_h, area.x1, area.y1);
    label(
        ctx,
        rctx,
        m,
        "Map preview unavailable: regions shown as tiles",
        note,
        crate::chart::LabelStyle {
            size: base * 0.85,
            align: Align::Center,
            anchor: Anchor::Middle,
            col: mix(text_col, Rgba::WHITE, 0.4),
            bold: false,
        },
    );
}

/// Squarified treemap (Bruls, Huizing, van Wijk): rows of tiles laid along the short side,
/// each row kept while it improves the worst aspect ratio.
pub fn squarify(vals: &[f64], r: Rect) -> Vec<Rect> {
    let total: f64 = vals.iter().sum();
    let mut out = vec![Rect::ZERO; vals.len()];
    if total <= 0.0 || r.area() <= 0.0 {
        return out;
    }
    let scale = r.area() / total;
    let mut order: Vec<usize> = (0..vals.len()).filter(|&i| vals.get(i).is_some_and(|v| *v > 0.0)).collect();
    order.sort_by(|a, b| vals[*b].total_cmp(&vals[*a]));
    let mut free = r;
    let mut i = 0;
    while i < order.len() {
        let short = free.width().min(free.height());
        if !short.is_finite() || short <= 1e-9 {
            break;
        }
        let s2 = short * short;
        let a0 = vals[order[i]] * scale;
        let mut cur_sum = a0;
        let mut cur_max = a0;
        let mut cur_min = a0;
        let mut cur_worst = ((s2 * cur_max) / (cur_sum * cur_sum)).max((cur_sum * cur_sum) / (s2 * cur_min));
        let mut end = i + 1;
        while end < order.len() {
            let next_a = vals[order[end]] * scale;
            let cand_sum = cur_sum + next_a;
            let cand_max = cur_max.max(next_a);
            let cand_min = cur_min.min(next_a);
            let cand_worst = ((s2 * cand_max) / (cand_sum * cand_sum)).max((cand_sum * cand_sum) / (s2 * cand_min));
            if cand_worst.is_finite() && cand_worst <= cur_worst {
                cur_sum = cand_sum;
                cur_max = cand_max;
                cur_min = cand_min;
                cur_worst = cand_worst;
                end += 1;
            } else {
                break;
            }
        }
        let row = &order[i..end];
        let thick = cur_sum / short;
        if !thick.is_finite() || thick <= 0.0 {
            break;
        }
        let mut at = 0.0;
        for &k in row {
            let len = vals[k] * scale / thick;
            out[k] = if free.width() >= free.height() {
                Rect::new(free.x0, free.y0 + at, free.x0 + thick, free.y0 + at + len)
            } else {
                Rect::new(free.x0 + at, free.y0, free.x0 + at + len, free.y0 + thick)
            };
            at += len;
        }
        free = if free.width() >= free.height() {
            Rect::new(free.x0 + thick, free.y0, free.x1, free.y1)
        } else {
            Rect::new(free.x0, free.y0 + thick, free.x1, free.y1)
        };
        i = end;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn treemap(
    ctx: &mut RenderContext,
    rctx: &Ctx,
    c: &Chart,
    m: Affine,
    area: Rect,
    base: f64,
    vals: &[f64],
    value_label: Option<&dyn Fn(f64) -> String>,
) {
    for (i, t) in squarify(vals, area).into_iter().enumerate() {
        if t.area() <= 0.0 {
            continue;
        }
        ctx.set_transform(m);
        ctx.set_paint(color(series_color(rctx, c, i), 1.0));
        ctx.fill_rect(&t);
        ctx.set_paint(color(Rgba::WHITE, 1.0));
        ctx.set_stroke(kurbo::Stroke::new(1.5));
        ctx.stroke_path(&t.to_path(0.1));
        let mut text = c.categories.get(i).cloned().unwrap_or_default();
        if let (Some(f), Some(v)) = (value_label, vals.get(i)) {
            text = format!("{text}\n{}", f(*v));
        }
        let inner = t.inset(-base * 0.3);
        if inner.width() > base && inner.height() > base {
            label(
                ctx,
                rctx,
                m,
                &text,
                inner,
                crate::chart::LabelStyle { size: base, align: Align::Left, anchor: Anchor::Top, col: Rgba::WHITE, bold: false },
            );
        }
    }
}

/// One ring of the innermost level, with the centre left open.
fn sunburst(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64, vals: &[f64]) {
    let total: f64 = vals.iter().sum();
    let r = area.width().min(area.height()) / 2.0 * 0.95;
    let hole = r * 0.35;
    let cen = area.center();
    let mut a0 = -std::f64::consts::FRAC_PI_2;
    for (i, v) in vals.iter().enumerate() {
        let sweep = v / total * std::f64::consts::TAU;
        if sweep <= 0.0 {
            continue;
        }
        let mut p = BezPath::new();
        p.move_to(Point::new(cen.x + hole * a0.cos(), cen.y + hole * a0.sin()));
        p.line_to(Point::new(cen.x + r * a0.cos(), cen.y + r * a0.sin()));
        p.extend(kurbo::Arc::new(cen, (r, r), a0, sweep, 0.0).append_iter(0.1));
        let a1 = a0 + sweep;
        p.line_to(Point::new(cen.x + hole * a1.cos(), cen.y + hole * a1.sin()));
        p.extend(kurbo::Arc::new(cen, (hole, hole), a1, -sweep, 0.0).append_iter(0.1));
        p.close_path();
        ctx.set_transform(m);
        ctx.set_paint(color(series_color(rctx, c, i), 1.0));
        ctx.fill_path(&p);
        ctx.set_paint(color(Rgba::WHITE, 1.0));
        ctx.set_stroke(kurbo::Stroke::new(1.5));
        ctx.stroke_path(&p);
        if let Some(name) = c.categories.get(i).filter(|_| sweep > 0.25) {
            let mid = a0 + sweep / 2.0;
            let rr = (r + hole) / 2.0;
            let lp = Point::new(cen.x + rr * mid.cos(), cen.y + rr * mid.sin());
            label(
                ctx,
                rctx,
                m,
                name,
                Rect::new(lp.x - 50.0, lp.y - base, lp.x + 50.0, lp.y + base),
                crate::chart::LabelStyle { size: base, align: Align::Center, anchor: Anchor::Middle, col: Rgba::WHITE, bold: false },
            );
        }
        a0 = a1;
    }
}

/// `col` mixed toward white by `k` (0..1): deeper hierarchy levels are lighter.
fn lighten(col: Rgba, k: f64) -> Rgba {
    let k = k.clamp(0.0, 0.8);
    let f = |v: u8| (f64::from(v) + (255.0 - f64::from(v)) * k).round() as u8;
    Rgba::rgb(f(col.r), f(col.g), f(col.b))
}

/// A multi-level treemap: top-level groups squarified by their totals, each with its label in
/// a banner along the top, and their children squarified inside, down to the leaves. A group
/// and everything in it take the group's colour.
#[allow(clippy::too_many_arguments)]
fn tree_treemap(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64, value_label: Option<&dyn Fn(f64) -> String>) {
    let tree = c.category_tree();
    #[allow(clippy::too_many_arguments)]
    fn place(
        ctx: &mut RenderContext,
        rctx: &Ctx,
        m: Affine,
        nodes: &[TreeNode],
        r: Rect,
        base: f64,
        col: &dyn Fn(usize) -> Rgba,
        value_label: Option<&dyn Fn(f64) -> String>,
        depth: usize,
    ) {
        let vals: Vec<f64> = nodes.iter().map(|n| n.value).collect();
        for (i, (n, t)) in nodes.iter().zip(squarify(&vals, r)).enumerate() {
            if t.area() <= 0.0 {
                continue;
            }
            let fill = col(i);
            if n.children.is_empty() {
                ctx.set_transform(m);
                ctx.set_paint(color(fill, 1.0));
                ctx.fill_rect(&t);
                ctx.set_paint(color(Rgba::WHITE, 1.0));
                ctx.set_stroke(kurbo::Stroke::new(1.0));
                ctx.stroke_path(&t.to_path(0.1));
                let mut text = n.name.clone();
                if let Some(f) = value_label {
                    text = format!("{text}\n{}", f(n.value));
                }
                let inner = t.inset(-base * 0.3);
                if inner.width() > base && inner.height() > base {
                    label(
                        ctx,
                        rctx,
                        m,
                        &text,
                        inner,
                        crate::chart::LabelStyle { size: base * 0.9, align: Align::Left, anchor: Anchor::Top, col: Rgba::WHITE, bold: false },
                    );
                }
                continue;
            }
            // A group: its banner, then its children below it.
            let banner = (base * 1.5).min(t.height() * 0.3);
            ctx.set_transform(m);
            ctx.set_paint(color(fill, 1.0));
            ctx.fill_rect(&t);
            if t.width() > base * 2.0 && banner > base * 0.8 {
                let head = Rect::new(t.x0 + base * 0.3, t.y0, t.x1 - base * 0.3, t.y0 + banner);
                label(
                    ctx,
                    rctx,
                    m,
                    &n.name,
                    head,
                    crate::chart::LabelStyle { size: base, align: Align::Left, anchor: Anchor::Middle, col: Rgba::WHITE, bold: true },
                );
            }
            let inner = Rect::new(t.x0 + 1.0, t.y0 + banner, t.x1 - 1.0, t.y1 - 1.0);
            let k = lighten(fill, 0.12 * (depth + 1) as f64);
            place(ctx, rctx, m, &n.children, inner, base, &|_| k, value_label, depth + 1);
            ctx.set_transform(m);
            ctx.set_paint(color(Rgba::WHITE, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(2.0));
            ctx.stroke_path(&t.to_path(0.1));
        }
    }
    place(ctx, rctx, m, &tree, area, base, &|i| series_color(rctx, c, i), value_label, 0);
}

/// A multi-level sunburst: the top level as the inner ring, each deeper level in the next ring
/// out within its parent's angle, coloured by top-level group and lighter further out.
fn tree_sunburst(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64) {
    let tree = c.category_tree();
    fn depth(n: &[TreeNode]) -> usize {
        n.iter().map(|c| 1 + depth(&c.children)).max().unwrap_or(0)
    }
    let levels = depth(&tree).max(1);
    let total: f64 = tree.iter().map(|n| n.value).sum();
    if total <= 0.0 {
        return;
    }
    let r = area.width().min(area.height()) / 2.0 * 0.95;
    let hole = r * 0.2;
    let ring = (r - hole) / levels as f64;
    let cen = area.center();
    #[allow(clippy::too_many_arguments)]
    fn ring_of(
        ctx: &mut RenderContext,
        rctx: &Ctx,
        m: Affine,
        nodes: &[TreeNode],
        a0: f64,
        span: f64,
        d: usize,
        geo: (Point, f64, f64, f64),
        base: f64,
        col: &dyn Fn(usize) -> Rgba,
    ) {
        let (cen, hole, ring, total) = geo;
        let sum: f64 = nodes.iter().map(|n| n.value).sum();
        if sum <= 0.0 {
            return;
        }
        let (ri, ro) = (hole + ring * d as f64, hole + ring * (d + 1) as f64);
        let mut a = a0;
        for (i, n) in nodes.iter().enumerate() {
            let sweep = n.value / sum * span;
            if sweep <= 0.0 {
                continue;
            }
            let fill = col(i);
            let mut p = BezPath::new();
            p.move_to(Point::new(cen.x + ri * a.cos(), cen.y + ri * a.sin()));
            p.line_to(Point::new(cen.x + ro * a.cos(), cen.y + ro * a.sin()));
            p.extend(kurbo::Arc::new(cen, (ro, ro), a, sweep, 0.0).append_iter(0.1));
            let a1 = a + sweep;
            p.line_to(Point::new(cen.x + ri * a1.cos(), cen.y + ri * a1.sin()));
            p.extend(kurbo::Arc::new(cen, (ri, ri), a1, -sweep, 0.0).append_iter(0.1));
            p.close_path();
            ctx.set_transform(m);
            ctx.set_paint(color(fill, 1.0));
            ctx.fill_path(&p);
            ctx.set_paint(color(Rgba::WHITE, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(1.5));
            ctx.stroke_path(&p);
            // Labels where the segment has room for them.
            let mid = a + sweep / 2.0;
            let rr = (ri + ro) / 2.0;
            if sweep * rr > base * 2.5 && !n.name.is_empty() {
                let lp = Point::new(cen.x + rr * mid.cos(), cen.y + rr * mid.sin());
                let half = (ring * 0.48).max(base);
                label(
                    ctx,
                    rctx,
                    m,
                    &n.name,
                    Rect::new(lp.x - half, lp.y - base, lp.x + half, lp.y + base),
                    crate::chart::LabelStyle { size: base * 0.9, align: Align::Center, anchor: Anchor::Middle, col: Rgba::WHITE, bold: false },
                );
            }
            if !n.children.is_empty() {
                let k = lighten(fill, 0.15);
                ring_of(ctx, rctx, m, &n.children, a, sweep, d + 1, (cen, hole, ring, total), base, &|_| k);
            }
            a = a1;
        }
    }
    ring_of(ctx, rctx, m, &tree, -std::f64::consts::FRAC_PI_2, std::f64::consts::TAU, 0, (cen, hole, ring, total), base, &|i| {
        series_color(rctx, c, i)
    });
}

/// A box chart's summaries: per series, per group.
pub struct Boxes {
    pub stats: Vec<Vec<Option<BoxStats>>>,
}

/// A box and whisker chart as the frame the bar renderer draws (value axis, gridlines, one
/// category per group, legend by series), and the boxes to draw in it. The frame's values are
/// each group's extremes, so the value axis covers every whisker and outlier.
pub fn box_view(c: &Chart) -> (Chart, Boxes) {
    let (groups, stats) = c.box_groups();
    let mut v = c.clone();
    v.kind = ChartType::Column;
    v.categories = groups;
    v.raw = None;
    let ext = |b: &BoxStats, hi: bool| {
        let o = b.outliers.iter().copied();
        if hi { o.fold(b.high, f64::max) } else { o.fold(b.low, f64::min) }
    };
    let lowest = stats.iter().flatten().flatten().map(|b| ext(b, false)).fold(f64::INFINITY, f64::min);
    for (s, st) in v.series.iter_mut().zip(&stats) {
        s.values = st.iter().map(|b| b.as_ref().map(|b| ext(b, true))).collect();
    }
    if lowest.is_finite() && lowest < 0.0 && v.y_min.is_none() {
        // The frame's values are the highs; below zero the axis must reach the lowest point too.
        let mag = 10f64.powf(lowest.abs().log10().floor());
        v.y_min = Some((lowest / mag).floor() * mag);
    }
    (v, Boxes { stats })
}

/// Boxes from the first to the third quartile with the median line and a mean marker, whiskers
/// to the furthest points within 1.5 IQR, and outliers as dots; series side by side in each
/// group's band.
#[allow(clippy::too_many_arguments)]
pub fn draw_boxes(
    ctx: &mut RenderContext,
    rctx: &Ctx,
    c: &Chart,
    b: &Boxes,
    m: Affine,
    plot: Rect,
    band: f64,
    horiz: bool,
    to_v: &dyn Fn(f64) -> f64,
) {
    let ns = b.stats.len().max(1);
    let group = band / (1.0 + c.gap_width.clamp(0.0, 5.0));
    let w = group / ns as f64;
    let line = Rgba::rgb(0x40, 0x40, 0x40);
    for (si, per) in b.stats.iter().enumerate() {
        let fill = series_color(rctx, c, si);
        for (gi, st) in per.iter().enumerate() {
            let Some(st) = st else { continue };
            let c0 = if horiz { plot.y0 } else { plot.x0 } + band * gi as f64 + (band - group) / 2.0 + w * si as f64;
            let (a, z) = (c0 + w * 0.1, c0 + w * 0.9);
            let mid = (a + z) / 2.0;
            // Along the category axis `a..z`, along the value axis `to_v`.
            let rect = |p: f64, q: f64, u: f64, v: f64| if horiz { Rect::new(u.min(v), p, u.max(v), q) } else { Rect::new(p, u.min(v), q, u.max(v)) };
            let pt = |along: f64, val: f64| if horiz { Point::new(val, along) } else { Point::new(along, val) };
            ctx.set_transform(m);
            ctx.set_paint(color(fill, 1.0));
            let bx = rect(a, z, to_v(st.q1), to_v(st.q3));
            ctx.fill_rect(&bx);
            let mut p = BezPath::new();
            // Whiskers with caps, and the median.
            for (from, to) in [(st.q3, st.high), (st.q1, st.low)] {
                p.move_to(pt(mid, to_v(from)));
                p.line_to(pt(mid, to_v(to)));
                p.move_to(pt(mid - w * 0.2, to_v(to)));
                p.line_to(pt(mid + w * 0.2, to_v(to)));
            }
            p.move_to(pt(a, to_v(st.median)));
            p.line_to(pt(z, to_v(st.median)));
            ctx.set_paint(color(line, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(1.25));
            ctx.stroke_path(&p);
            ctx.stroke_path(&bx.to_path(0.1));
            // The mean as an x.
            let (mx, r) = (pt(mid, to_v(st.mean)), (w * 0.08).clamp(1.5, 5.0));
            let mut x = BezPath::new();
            x.move_to((mx.x - r, mx.y - r));
            x.line_to((mx.x + r, mx.y + r));
            x.move_to((mx.x - r, mx.y + r));
            x.line_to((mx.x + r, mx.y - r));
            ctx.stroke_path(&x);
            for o in &st.outliers {
                let q = pt(mid, to_v(*o));
                ctx.set_paint(color(fill, 1.0));
                ctx.fill_path(&kurbo::Circle::new(q, r).to_path(0.1));
                ctx.set_paint(color(line, 1.0));
                ctx.stroke_path(&kurbo::Circle::new(q, r).to_path(0.1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_tiles_the_area_in_proportion() {
        let vals = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let r = Rect::new(0.0, 0.0, 600.0, 400.0);
        let tiles = squarify(&vals, r);
        let total: f64 = vals.iter().sum();
        for (t, v) in tiles.iter().zip(vals) {
            assert!((t.area() - v / total * r.area()).abs() < 1e-6, "{t:?} for {v}");
            assert!(t.x0 >= -1e-9 && t.y0 >= -1e-9 && t.x1 <= 600.0 + 1e-6 && t.y1 <= 400.0 + 1e-6, "{t:?}");
            // Squarified: no sliver tiles.
            let aspect = t.width().max(t.height()) / t.width().min(t.height());
            assert!(aspect < 4.0, "{t:?}");
        }
        for (i, a) in tiles.iter().enumerate() {
            for b in &tiles[i + 1..] {
                assert!(a.intersect(*b).area() < 1e-6, "{a:?} overlaps {b:?}");
            }
        }
        assert!(squarify(&[0.0, 0.0], r).iter().all(|t| t.area() == 0.0));
    }

    #[test]
    fn waterfall_bars_float_and_totals_stand() {
        let c = Chart {
            kind: ChartType::Waterfall,
            subtotals: vec![3],
            series: vec![deckcraft_model::chart::Series { values: vec![Some(100.0), Some(40.0), Some(-25.0), Some(115.0)], ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(c.waterfall_bars(), [(0.0, 100.0), (100.0, 140.0), (140.0, 115.0), (0.0, 115.0)]);
        assert_eq!(c.value_range(), (0.0, 140.0));
    }

    #[test]
    fn squarify_handles_skewed_and_exhausted_rect() {
        let vals = vec![1e12, 1e-6, 1e-6, 1e-6];
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        let tiles = squarify(&vals, r);
        assert_eq!(tiles.len(), vals.len());
        assert!(tiles[0].area() > 0.0);
    }
}
