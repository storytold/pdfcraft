use super::frame::{Bx, CatAxis, ValAxis, frame, on, value_range};
use super::legend::{cat_label, point_color};
use super::{
    AXIS, Chart, ChartData, Ctx, DOWN, Fmt, GRID, HAlign, LABEL_SIZE, MAX_CATS, MAX_LABELS, MAX_MARKERS, MAX_SERIES, MAX_WEDGES, Prim,
    QuartileMethod, Rgba, STOCK, SeriesData, TAU, TEXT, UP, VAlign, WHITE, clean, nice_scale,
};

// ---------------------------------------------------------------- histogram

pub(super) fn histogram(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let Some(s) = data.series.first() else { return };
    let vals: Vec<f64> = s.values.iter().flatten().copied().collect();
    let Some((lo, hi)) = value_range(vals.iter().copied()) else { return };
    let n = vals.len();
    let span = hi - lo;
    let mut bins = ((n as f64).sqrt().ceil() as usize).clamp(1, 100);
    if !(span.is_finite() && span > 0.0) {
        bins = 1;
    }
    let width = if bins == 1 && span.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) { 1.0 } else { span / bins as f64 };
    // Upper edge of each bin, exactly as shown in the labels below. Bins are right-inclusive (the first is closed on both sides), so a value
    // equal to an edge belongs to the bin that ends there.
    let edges: Vec<f64> = (0..bins).map(|b| if b + 1 == bins { hi.max(lo + b as f64 * width) } else { lo + (b + 1) as f64 * width }).collect();
    let mut counts = vec![0usize; bins];
    for v in &vals {
        let b = edges.partition_point(|e| e < v).min(bins - 1);
        if let Some(c) = counts.get_mut(b) {
            *c += 1;
        }
    }
    let f = Fmt::new(&s.number_format);
    let labels: Vec<String> = (0..bins)
        .map(|b| {
            let a = lo + b as f64 * width;
            let e = edges.get(b).copied().unwrap_or(a);
            let open = if b == 0 { '[' } else { '(' };
            format!("{open}{}, {}]", f.format(a, Some(width / 10.0)), f.format(e, Some(width / 10.0)))
        })
        .collect();
    let maxc = counts.iter().copied().max().unwrap_or(0) as f64;
    let ys = nice_scale(0.0, maxc, true);
    let fc = Fmt::new("General");
    let plot = frame(ctx, area, CatAxis::Labels(&labels, &[]), &ValAxis { scale: ys, fmt: &fc }, None, false, chart.gridlines);
    if !plot.ok() {
        return;
    }
    let cw = plot.w / bins as f32;
    for (b, c) in counts.iter().enumerate() {
        let top = plot.bottom() - ys.frac(*c as f64) * plot.h;
        let x = plot.x + b as f32 * cw;
        ctx.rect(x, top, cw, plot.bottom() - top, s.color, Some((WHITE, 0.75)));
        if chart.data_labels {
            ctx.text(x + cw / 2.0, top - 2.0, &c.to_string(), LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Bottom));
        }
    }
}

// ---------------------------------------------------------------- waterfall

pub(super) fn waterfall(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let Some(s) = data.series.first() else { return };
    let mut ncat = data.categories.len().max(s.values.len()).min(MAX_CATS);
    if ncat == 0 {
        return;
    }
    let mut labels: Vec<String> = (0..ncat).map(|j| cat_label(data, j)).collect();
    let mut vals: Vec<f64> = (0..ncat).map(|j| s.values.get(j).copied().flatten().unwrap_or(0.0)).collect();
    let mut is_total = vec![false; ncat];
    let last_total = labels.last().is_some_and(|l| l.to_lowercase().contains("total"));
    if let Some(t) = is_total.last_mut() {
        *t = last_total;
    }
    if !last_total {
        labels.push("Total".into());
        vals.push(0.0);
        is_total.push(true);
        ncat += 1;
    }
    let mut bars = Vec::with_capacity(ncat);
    let mut cum = 0.0f64;
    for (j, v) in vals.iter().enumerate() {
        if is_total.get(j).copied().unwrap_or(false) {
            let total = if last_total && j + 1 == ncat && *v != 0.0 { *v } else { cum };
            bars.push((0.0, total, 0u8));
            cum = total;
        } else {
            let top = clean(cum + v);
            bars.push((cum, top, if *v >= 0.0 { 1 } else { 2 }));
            cum = top;
        }
    }
    let (lo, hi) = value_range(bars.iter().flat_map(|(b, t, _)| [*b, *t])).unwrap_or((0.0, 0.0));
    let ys = nice_scale(lo, hi, true);
    let f = Fmt::new(&s.number_format);
    let plot = frame(ctx, area, CatAxis::Labels(&labels, &[]), &ValAxis { scale: ys, fmt: &f }, None, false, chart.gridlines);
    if !plot.ok() {
        return;
    }
    let cw = plot.w / ncat as f32;
    let bw = cw / 1.5;
    let vy = |v: f64| plot.bottom() - ys.frac(v) * plot.h;
    for (j, (b, t, kind)) in bars.iter().enumerate() {
        let color = match kind {
            1 => UP,
            2 => DOWN,
            _ => s.color,
        };
        let x = plot.x + (j as f32 + 0.5) * cw - bw / 2.0;
        let (yb, yt) = (vy(*b), vy(*t));
        ctx.rect(x, yb.min(yt), bw, (yb - yt).abs().max(0.5), color, None);
        if j + 1 < bars.len() && ncat <= 500 {
            ctx.out.push(Prim::Line { pts: vec![[x + bw, yt], [x + cw, yt]], color: AXIS, width: 0.75, dash: true });
        }
        if chart.data_labels && ncat <= MAX_LABELS {
            let v = if *kind == 0 { *t } else { t - b };
            ctx.text(x + bw / 2.0, yb.min(yt) - 2.0, &f.format(v, None), LABEL_SIZE, TEXT, (HAlign::Center, VAlign::Bottom));
        }
    }
}

// ---------------------------------------------------------------- funnel

pub(super) fn funnel(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let Some(s) = data.series.first() else { return };
    let n = data.categories.len().max(s.values.len()).min(MAX_CATS);
    if n == 0 {
        return;
    }
    let vals: Vec<f64> = (0..n).map(|j| s.values.get(j).copied().flatten().unwrap_or(0.0).abs()).collect();
    let max = vals.iter().copied().fold(0.0f64, f64::max);
    if max.is_nan() || max <= 0.0 {
        return;
    }
    let labels: Vec<String> = (0..n).map(|j| cat_label(data, j)).collect();
    let lw = (labels.iter().take(2000).map(|t| ctx.tw(t, LABEL_SIZE)).fold(0.0f32, f32::max) + 8.0).min(area.w * 0.35);
    let plot = Bx { x: area.x + lw, w: area.w - lw, ..area };
    if !plot.ok() {
        return;
    }
    let slot = plot.h / n as f32;
    let bh = slot * 0.85;
    let f = Fmt::new(&s.number_format);
    let show = slot >= LABEL_SIZE * 0.9;
    for (j, v) in vals.iter().enumerate() {
        let bw = (*v / max) as f32 * plot.w;
        let x = plot.x + (plot.w - bw) / 2.0;
        let y = plot.y + j as f32 * slot + (slot - bh) / 2.0;
        if bw > 0.0 {
            ctx.rect(x, y, bw, bh, s.color, None);
        }
        if show {
            let yc = y + bh / 2.0;
            if chart.data_labels {
                let t = f.format(s.values.get(j).copied().flatten().unwrap_or(0.0), None);
                let color = if ctx.tw(&t, LABEL_SIZE) + 4.0 <= bw { on(s.color) } else { TEXT };
                ctx.text(plot.cx(), yc, &t, LABEL_SIZE, color, (HAlign::Center, VAlign::Middle));
            }
            let l = ctx.fit(labels.get(j).map(String::as_str).unwrap_or(""), LABEL_SIZE, lw - 8.0);
            ctx.text(plot.x - 6.0, yc, &l, LABEL_SIZE, TEXT, (HAlign::Right, VAlign::Middle));
        }
    }
}

// ---------------------------------------------------------------- pie, doughnut, sunburst

/// At most `cap` of the largest values, in category order. When a ring has more categories than
/// its share of the wedge budget allows, the big slices survive rather than the first ones.
fn largest_slices(vals: &[f64], cap: usize) -> Vec<(usize, f64)> {
    let mut idx: Vec<usize> = (0..vals.len()).collect();
    if idx.len() > cap {
        // Partition descends by value, so the first `cap` entries are the largest ones.
        idx.select_nth_unstable_by(cap - 1, |a, b| vals[*b].total_cmp(&vals[*a]));
        idx.truncate(cap);
        idx.sort_unstable();
    }
    idx.into_iter().map(|j| (j, vals[j])).collect()
}

/// One ring's geometry and separator stroke: the slices of a ring differ only in angle and colour.
struct Ring {
    cx: f32,
    cy: f32,
    outer: f32,
    inner: f32,
    stroke: Option<(Rgba, f32)>,
}

/// A wedge of `ring`, split at 12 o'clock when it crosses the end of the circle.
fn wedge(ctx: &mut Ctx, ring: &Ring, a0: f32, sweep: f32, fill: Rgba) {
    let Ring { cx, cy, outer, inner, stroke } = *ring;
    if sweep <= 1e-4 {
        return;
    }
    // A slice that starts past 12 o'clock still has to be split against 12 o'clock, not against
    // its unwrapped start: otherwise the half after TAU is drawn from 0 over its neighbours.
    let a0 = a0.rem_euclid(TAU);
    let a1 = a0 + sweep;
    if a1 <= TAU {
        ctx.out.push(Prim::Wedge { cx, cy, r_outer: outer, r_inner: inner, a0, a1, fill, stroke });
        return;
    }
    // The halves are filled without a stroke: each would outline its own radial edge at 12
    // o'clock, painting the white separator straight through the slice.
    ctx.out.push(Prim::Wedge { cx, cy, r_outer: outer, r_inner: inner, a0, a1: TAU, fill, stroke: None });
    ctx.out.push(Prim::Wedge { cx, cy, r_outer: outer, r_inner: inner, a0: 0.0, a1: a1 - TAU, fill, stroke: None });
    if let Some((color, width)) = stroke {
        // One outline for the whole slice: the outer and inner arcs run through 12 o'clock, and
        // the two radial edges at the slice's own start and end close the path.
        let arc = |r: f32, from: f32, to: f32| -> Vec<[f32; 2]> {
            let steps = ((r.abs() * (to - from).abs() / 2.0).ceil() as usize).clamp(4, 512);
            let at = |a: f32| [cx + r * a.sin(), cy - r * a.cos()];
            (0..=steps).map(|k| at(from + (to - from) * k as f32 / steps as f32)).collect()
        };
        let mut pts = arc(outer, a0, a1);
        if inner > 0.0 {
            pts.extend(arc(inner, a1, a0));
        } else {
            pts.push([cx, cy]);
        }
        let start = pts[0];
        pts.push(start);
        ctx.out.push(Prim::Line { pts, color, width, dash: false });
    }
}

pub(super) fn pie(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx, inner: f32, names: bool) {
    let Some(s) = data.series.first() else { return };
    let vals: Vec<f64> = s.values.iter().take(MAX_CATS).map(|v| v.map(f64::abs).unwrap_or(0.0)).collect();
    let total: f64 = vals.iter().sum();
    if !(total.is_finite() && total > 0.0) {
        return;
    }
    let (cx, cy) = (area.cx(), area.cy());
    let r = area.w.min(area.h) / 2.0 * 0.92;
    if r < 2.0 {
        return;
    }
    let ri = r * inner;
    let ring = Ring { cx, cy, outer: r, inner: ri, stroke: Some((WHITE, 1.5)) };
    let f = Fmt::new(&s.number_format);
    let mut a = (chart.first_slice_angle % 360) as f32 * TAU / 360.0;
    let mut labels = Vec::new();
    for (j, v) in vals.iter().enumerate() {
        let sweep = (*v / total) as f32 * TAU;
        if sweep > 1e-4 {
            let fill = point_color(data, j);
            wedge(ctx, &ring, a, sweep, fill);
            if (chart.data_labels || names) && vals.len() <= MAX_LABELS {
                let t = if names { cat_label(data, j) } else { f.format(s.values.get(j).copied().flatten().unwrap_or(0.0), None) };
                let mid = a + sweep / 2.0;
                let rr = if ri > 0.0 { (r + ri) / 2.0 } else { r * 0.65 };
                let room = sweep * rr;
                if ctx.tw(&t, LABEL_SIZE) <= room.max(r - ri) && room >= LABEL_SIZE {
                    labels.push((cx + rr * mid.sin(), cy - rr * mid.cos(), t, on(fill)));
                }
            }
        }
        a += sweep;
    }
    for (x, y, t, c) in labels {
        ctx.text(x, y, &t, LABEL_SIZE, c, (HAlign::Center, VAlign::Middle));
    }
}

/// Excel's doughnut: every series is one concentric ring, the first series innermost (measured in
/// Excel: ring outer radii ≈0.83, 0.92 and 1.0 of the plot radius for series 1, 2 and 3). The hole
/// size and first slice angle come from the chart; each ring is scaled to its own total.
pub(super) fn doughnut(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let nser = data.series.len().min(MAX_SERIES);
    if nser == 0 {
        return;
    }
    let (cx, cy) = (area.cx(), area.cy());
    let r = area.w.min(area.h) / 2.0 * 0.92;
    if r < 2.0 {
        return;
    }
    let hole = f32::from(chart.hole_size.clamp(10, 90)) / 100.0;
    let ri = r * hole;
    let thick = (r - ri) / nser as f32;
    // A ring thinner than a pixel would vanish. Rather than suppressing the whole chart when there
    // are more series than the annulus has pixels, keep the outer rings at one pixel each and drop
    // the innermost series that no longer fit: Excel's order makes the first series the innermost,
    // so the outermost rings are the ones worth keeping.
    let band = thick.max(1.0);
    let start = (chart.first_slice_angle % 360) as f32 * TAU / 360.0;
    // Only rings with a pixel of band are drawn, and the wedge budget is shared across them in
    // drawing order: the innermost ring (the first series) gets its slices first, and the outer
    // rings are capped instead of the series × category product growing without bound.
    let rings: Vec<(&SeriesData, f32, f32)> = data
        .series
        .iter()
        .take(nser)
        .enumerate()
        .filter_map(|(si, s)| {
            let outer = (r - (nser - 1 - si) as f32 * band).min(r);
            let inner = (r - (nser - si) as f32 * band).max(ri);
            (outer - inner >= 1.0).then_some((s, outer, inner))
        })
        .collect();
    let mut budget = MAX_WEDGES;
    let mut labels = Vec::new();
    for (k, (s, outer, inner)) in rings.iter().enumerate() {
        // The white separator is 1.5 units wide, wider than a one-pixel band: on a thin ring it
        // would paint over the fill of this ring and its neighbour, so such rings keep only fill.
        let stroke = if outer - inner > 1.0 { Some((WHITE, 1.5)) } else { None };
        let ring = Ring { cx, cy, outer: *outer, inner: *inner, stroke };
        let vals: Vec<f64> = s.values.iter().take(MAX_CATS).map(|v| v.map(f64::abs).unwrap_or(0.0)).collect();
        let cap = (budget / (rings.len() - k)).max(1);
        let slices = largest_slices(&vals, cap);
        let total: f64 = slices.iter().map(|(_, v)| *v).sum();
        if !(total.is_finite() && total > 0.0) {
            continue;
        }
        let n = slices.len();
        budget = budget.saturating_sub(n);
        let f = Fmt::new(&s.number_format);
        let mut a = start;
        for (j, v) in slices {
            let sweep = (v / total) as f32 * TAU;
            if sweep > 1e-4 {
                let fill = point_color(data, j);
                wedge(ctx, &ring, a, sweep, fill);
                if chart.data_labels && n <= MAX_LABELS {
                    let t = f.format(s.values.get(j).copied().flatten().unwrap_or(0.0), None);
                    let mid = a + sweep / 2.0;
                    let rr = (outer + inner) / 2.0;
                    if ctx.tw(&t, LABEL_SIZE) <= (sweep * rr).max(outer - inner) && sweep * rr >= LABEL_SIZE {
                        labels.push((cx + rr * mid.sin(), cy - rr * mid.cos(), t, on(fill)));
                    }
                }
            }
            a += sweep;
        }
    }
    for (x, y, t, c) in labels {
        ctx.text(x, y, &t, LABEL_SIZE, c, (HAlign::Center, VAlign::Middle));
    }
}

// ---------------------------------------------------------------- treemap

/// Worst aspect ratio of a row with the given area sum, smallest and largest item, laid along `side`.
fn worst(s: f64, mn: f64, mx: f64, side: f64) -> f64 {
    if s <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = s * s;
    let w2 = side * side;
    (w2 * mx / s2).max(s2 / (w2 * mn))
}

/// Squarified treemap layout of (index, value) items, sorted descending, into `r`; each block
/// comes back with its index and value.
pub(super) fn squarify(items: &[(usize, f64)], r: Bx) -> Vec<(usize, f64, Bx)> {
    let total: f64 = items.iter().map(|i| i.1).sum();
    let area = (r.w as f64) * (r.h as f64);
    if !(total > 0.0 && area > 0.0) {
        return vec![];
    }
    let scale = area / total;
    let mut out = Vec::with_capacity(items.len());
    let mut rem = r;
    let mut row: Vec<(usize, f64, f64)> = Vec::new();
    let (mut sum, mut mn, mut mx) = (0.0f64, f64::INFINITY, 0.0f64);
    let mut i = 0;
    let layout = |row: &[(usize, f64, f64)], rem: &mut Bx, out: &mut Vec<(usize, f64, Bx)>| {
        let s: f64 = row.iter().map(|x| x.1).sum();
        if s <= 0.0 {
            return;
        }
        if rem.w >= rem.h {
            let cw = (s / rem.h.max(1e-6) as f64) as f32;
            let mut y = rem.y;
            for (j, a, v) in row {
                let h = (*a / cw.max(1e-6) as f64) as f32;
                out.push((*j, *v, Bx { x: rem.x, y, w: cw, h }));
                y += h;
            }
            rem.x += cw;
            rem.w = (rem.w - cw).max(0.0);
        } else {
            let rh = (s / rem.w.max(1e-6) as f64) as f32;
            let mut x = rem.x;
            for (j, a, v) in row {
                let w = (*a / rh.max(1e-6) as f64) as f32;
                out.push((*j, *v, Bx { x, y: rem.y, w, h: rh }));
                x += w;
            }
            rem.y += rh;
            rem.h = (rem.h - rh).max(0.0);
        }
    };
    while let Some(&(j, v)) = items.get(i) {
        let a = v * scale;
        let side = rem.w.min(rem.h) as f64;
        // Sum, smallest and largest item of the open row, kept incrementally (same fold order as
        // summing the row afresh, so the numbers are bit-identical).
        let with = (sum + a, mn.min(a), mx.max(a));
        if row.is_empty() || worst(with.0, with.1, with.2, side) <= worst(sum, mn, mx, side) {
            row.push((j, a, v));
            (sum, mn, mx) = with;
            i += 1;
        } else {
            layout(&row, &mut rem, &mut out);
            row.clear();
            (sum, mn, mx) = (0.0, f64::INFINITY, 0.0);
        }
    }
    if !row.is_empty() {
        layout(&row, &mut rem, &mut out);
    }
    out
}

pub(super) fn treemap(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let Some(s) = data.series.first() else { return };
    let mut items: Vec<(usize, f64)> =
        s.values.iter().take(MAX_MARKERS).enumerate().filter_map(|(j, v)| v.filter(|v| *v > 0.0).map(|v| (j, v))).collect();
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    let f = Fmt::new(&s.number_format);
    for (j, v, b) in squarify(&items, area) {
        let fill = point_color(data, j);
        ctx.rect(b.x, b.y, b.w, b.h, fill, Some((WHITE, 1.5)));
        if b.w > 24.0 && b.h > LABEL_SIZE * 1.6 {
            let name = ctx.fit(&cat_label(data, j), LABEL_SIZE, b.w - 8.0);
            ctx.text(b.x + 4.0, b.y + 4.0, &name, LABEL_SIZE, on(fill), (HAlign::Left, VAlign::Top));
            if chart.data_labels && b.h > LABEL_SIZE * 3.2 {
                let t = ctx.fit(&f.format(v, None), LABEL_SIZE, b.w - 8.0);
                ctx.text(b.x + 4.0, b.y + 4.0 + LABEL_SIZE * 1.4, &t, LABEL_SIZE, on(fill), (HAlign::Left, VAlign::Top));
            }
        }
    }
}

// ---------------------------------------------------------------- radar

pub(super) fn radar(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let ncat = data.categories.len().max(data.series.iter().map(|s| s.values.len()).max().unwrap_or(0)).min(1000);
    if ncat == 0 {
        return;
    }
    let labels: Vec<String> = (0..ncat).map(|j| cat_label(data, j)).collect();
    let lw = labels.iter().map(|t| ctx.tw(t, LABEL_SIZE)).fold(0.0f32, f32::max).min(area.w * 0.2);
    let r = ((area.w - 2.0 * (lw + 8.0)).min(area.h - 2.0 * (LABEL_SIZE * 1.4 + 6.0)) / 2.0).max(0.0);
    if r < 5.0 {
        return;
    }
    let (cx, cy) = (area.cx(), area.cy());
    let (lo, hi) = value_range(data.series.iter().flat_map(|s| s.values.iter().flatten().copied())).unwrap_or((0.0, 0.0));
    let sc = nice_scale(lo, hi, true);
    let f = Fmt::new(data.series.first().map(|s| s.number_format.as_str()).unwrap_or("General"));
    let ang = |j: usize| j as f32 / ncat as f32 * TAU;
    let pt = |a: f32, rr: f32| [cx + rr * a.sin(), cy - rr * a.cos()];
    let ticks = sc.ticks();
    let spacing = r / ticks.len().saturating_sub(1).max(1) as f32;
    let every = ((LABEL_SIZE * 1.3 / spacing.max(1e-3)).ceil() as usize).max(1);
    for (ti, t) in ticks.into_iter().enumerate() {
        let rr = sc.frac(t) * r;
        if rr > 0.5 && chart.gridlines {
            let mut ring: Vec<[f32; 2]> = (0..ncat).map(|j| pt(ang(j), rr)).collect();
            if ncat < 3 {
                ring = (0..=48).map(|k| pt(k as f32 / 48.0 * TAU, rr)).collect();
            } else if let Some(p) = ring.first().copied() {
                ring.push(p);
            }
            ctx.line(ring, GRID, 0.75);
        }
        if ti % every == 0 {
            ctx.text(cx - 3.0, cy - rr, &f.format(t, Some(sc.step)), LABEL_SIZE * 0.9, TEXT, (HAlign::Right, VAlign::Middle));
        }
    }
    let step = ncat.div_ceil(72).max(1);
    for j in (0..ncat).step_by(step) {
        let a = ang(j);
        ctx.line(vec![[cx, cy], pt(a, r)], GRID, 0.75);
        let p = pt(a, r + 6.0);
        let (sa, ca) = (a.sin(), a.cos());
        let align = if sa > 0.1 {
            HAlign::Left
        } else if sa < -0.1 {
            HAlign::Right
        } else {
            HAlign::Center
        };
        let valign = if ca > 0.1 {
            VAlign::Bottom
        } else if ca < -0.1 {
            VAlign::Top
        } else {
            VAlign::Middle
        };
        let t = ctx.fit(labels.get(j).map(String::as_str).unwrap_or(""), LABEL_SIZE, lw.max(20.0));
        ctx.text(p[0], p[1], &t, LABEL_SIZE, TEXT, (align, valign));
    }
    for s in &data.series {
        let mut pts: Vec<[f32; 2]> = (0..ncat).map(|j| pt(ang(j), sc.frac(s.values.get(j).copied().flatten().unwrap_or(sc.min)) * r)).collect();
        if let Some(p) = pts.first().copied() {
            pts.push(p);
        }
        ctx.line(pts, s.color, 2.0);
    }
}

// ---------------------------------------------------------------- box & whisker

fn quantile(sorted: &[f64], q: f64, method: QuartileMethod) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    // Clamp exclusive ranks for short samples so one or two points remain drawable.
    let pos = match method {
        QuartileMethod::Inclusive => q * (n - 1) as f64,
        QuartileMethod::Exclusive => (q * (n as f64 + 1.0) - 1.0).clamp(0.0, (n - 1) as f64),
    };
    let i = pos.floor() as usize;
    let frac = pos - i as f64;
    let a = sorted.get(i).copied().unwrap_or(0.0);
    let b = sorted.get(i + 1).copied().unwrap_or(a);
    a + (b - a) * frac
}

pub(super) fn box_whisker(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let labels: Vec<String> = data.series.iter().map(|s| s.name.clone()).collect();
    let Some((lo, hi)) = value_range(data.series.iter().flat_map(|s| s.values.iter().flatten().copied())) else { return };
    let ys = nice_scale(lo, hi, false);
    let f = Fmt::new(data.series.first().map(|s| s.number_format.as_str()).unwrap_or("General"));
    let plot = frame(ctx, area, CatAxis::Labels(&labels, &[]), &ValAxis { scale: ys, fmt: &f }, None, false, chart.gridlines);
    if !plot.ok() {
        return;
    }
    let n = data.series.len().max(1);
    let cw = plot.w / n as f32;
    let bw = (cw * 0.4).min(80.0);
    let vy = |v: f64| plot.bottom() - ys.frac(v) * plot.h;
    for (i, s) in data.series.iter().enumerate() {
        let mut v: Vec<f64> = s.values.iter().flatten().copied().collect();
        if v.is_empty() {
            continue;
        }
        v.sort_by(f64::total_cmp);
        let method = chart.series.get(i).map(|s| s.quartiles).unwrap_or_default();
        let (q1, med, q3) = (quantile(&v, 0.25, method), quantile(&v, 0.5, method), quantile(&v, 0.75, method));
        let iqr = q3 - q1;
        let (flo, fhi) = (q1 - 1.5 * iqr, q3 + 1.5 * iqr);
        let wlo = v.iter().copied().find(|x| *x >= flo).unwrap_or(q1);
        let whi = v.iter().rev().copied().find(|x| *x <= fhi).unwrap_or(q3);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let c = plot.x + (i as f32 + 0.5) * cw;
        ctx.vline(c, vy(whi), vy(q3), s.color, 1.25);
        ctx.vline(c, vy(q1), vy(wlo), s.color, 1.25);
        ctx.hline(c - bw / 4.0, c + bw / 4.0, vy(whi), s.color, 1.25);
        ctx.hline(c - bw / 4.0, c + bw / 4.0, vy(wlo), s.color, 1.25);
        let (yt, yb) = (vy(q3), vy(q1));
        ctx.rect(c - bw / 2.0, yt, bw, (yb - yt).max(0.5), s.color, None);
        ctx.hline(c - bw / 2.0, c + bw / 2.0, vy(med), WHITE, 1.5);
        let (mx, my, d) = (c, vy(mean), 3.0);
        ctx.line(vec![[mx - d, my - d], [mx + d, my + d]], WHITE, 1.25);
        ctx.line(vec![[mx - d, my + d], [mx + d, my - d]], WHITE, 1.25);
        for o in v.iter().filter(|x| **x < flo || **x > fhi).take(MAX_MARKERS) {
            ctx.out.push(Prim::Circle { cx: c, cy: vy(*o), r: 2.5, fill: None, stroke: Some((s.color, 1.0)) });
        }
    }
}

// ---------------------------------------------------------------- stock

pub(super) fn stock(ctx: &mut Ctx, chart: &Chart, data: &ChartData, area: Bx) {
    let ncat = data.categories.len().max(data.series.iter().map(|s| s.values.len()).max().unwrap_or(0)).min(MAX_CATS);
    if ncat == 0 {
        return;
    }
    let labels: Vec<String> = (0..ncat).map(|j| cat_label(data, j)).collect();
    let Some((lo, hi)) = value_range(data.series.iter().flat_map(|s| s.values.iter().flatten().copied())) else { return };
    let ys = nice_scale(lo, hi, false);
    let f = Fmt::new(data.series.first().map(|s| s.number_format.as_str()).unwrap_or("General"));
    let plot = frame(ctx, area, CatAxis::Labels(&labels, &[]), &ValAxis { scale: ys, fmt: &f }, None, false, chart.gridlines);
    if !plot.ok() {
        return;
    }
    let ser = |k: usize| data.series.get(k);
    let (open, high, low, close) = match data.series.len() {
        0 => return,
        1 => (None, None, None, ser(0)),
        2 => (None, ser(0), ser(1), None),
        3 => (None, ser(0), ser(1), ser(2)),
        _ => (ser(0), ser(1), ser(2), ser(3)),
    };
    let get = |s: Option<&SeriesData>, j: usize| s.and_then(|s| s.values.get(j).copied().flatten());
    let cw = plot.w / ncat as f32;
    let tick = (cw * 0.25).max(1.0);
    let vy = |v: f64| plot.bottom() - ys.frac(v) * plot.h;
    for j in 0..ncat {
        let c = plot.x + (j as f32 + 0.5) * cw;
        if let (Some(h), Some(l)) = (get(high, j), get(low, j)) {
            ctx.vline(c, vy(h), vy(l), STOCK, 1.25);
        }
        match (get(open, j), get(close, j)) {
            (Some(o), Some(cl)) => {
                let (yo, yc) = (vy(o), vy(cl));
                let bw = (cw * 0.5).max(1.0);
                let fill = if cl >= o { WHITE } else { STOCK };
                ctx.rect(c - bw / 2.0, yo.min(yc), bw, (yo - yc).abs().max(0.75), fill, Some((STOCK, 1.0)));
            }
            (None, Some(cl)) => {
                if high.is_some() {
                    ctx.hline(c, c + tick, vy(cl), STOCK, 1.25);
                } else {
                    ctx.out.push(Prim::Circle { cx: c, cy: vy(cl), r: 2.5, fill: Some(STOCK), stroke: None });
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod quartile_tests {
    use super::*;

    #[test]
    fn box_whisker_default_quartiles_are_exclusive() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        assert_eq!(quantile(&values, 0.25, QuartileMethod::default()), 1.75);
        assert_eq!(quantile(&values, 0.5, QuartileMethod::default()), 3.5);
        assert_eq!(quantile(&values, 0.75, QuartileMethod::default()), 5.25);
    }

    #[test]
    fn quartile_methods_and_short_samples() {
        for (values, exclusive, inclusive) in [
            (vec![1., 2., 3.], (1., 3.), (1.5, 2.5)),
            (vec![1., 2., 3., 4.], (1.25, 3.75), (1.75, 3.25)),
            (vec![1., 2., 3., 4., 5.], (1.5, 4.5), (2., 4.)),
            (vec![1., 2., 3., 4., 5., 6., 7.], (2., 6.), (2.5, 5.5)),
            (vec![1., 1., 2., 3., 5., 8.], (1., 5.75), (1.25, 4.5)),
            (vec![1., 2.], (1., 2.), (1.25, 1.75)),
            (vec![1.], (1., 1.), (1., 1.)),
            (vec![], (0., 0.), (0., 0.)),
        ] {
            for (method, expected) in [(QuartileMethod::Exclusive, exclusive), (QuartileMethod::Inclusive, inclusive)] {
                assert_eq!((quantile(&values, 0.25, method), quantile(&values, 0.75, method)), expected);
            }
        }
    }
}
