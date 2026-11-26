use super::frame::Bx;
use super::trend::supports_trendline;
use super::{
    Chart, ChartData, ChartKind, Ctx, DOWN, HAlign, LABEL_SIZE, MAX_CATS, MAX_SERIES, MAX_TRENDLINES, Prim, Rgba, TEXT, Theme, UP, VAlign, auto_name,
    series_color,
};
use gridcraft_model::LegendPos;

/// How a legend entry's swatch is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swatch {
    Box,
    Line,
    LineMarker,
    Marker,
}

/// Colour of point `j` for charts that colour by point (pie, doughnut, treemap, sunburst).
pub(super) fn point_color(data: &ChartData, j: usize) -> Rgba {
    match (j, data.series.first()) {
        (0, Some(s)) => s.color,
        _ => j.checked_rem(data.palette.len()).and_then(|k| data.palette.get(k)).copied().unwrap_or_else(|| series_color(&Theme::default(), j)),
    }
}

fn first_len(data: &ChartData) -> usize {
    data.series.first().map(|s| s.values.len()).unwrap_or(0)
}

pub(super) fn cat_label(data: &ChartData, j: usize) -> String {
    data.categories.get(j).cloned().unwrap_or_else(|| (j + 1).to_string())
}

/// Legend entries (text, colour, swatch) `render` draws for this chart; empty when hidden.
pub fn legend_entries(chart: &Chart, data: &ChartData) -> Vec<(String, Rgba, Swatch)> {
    if chart.legend == LegendPos::None {
        return vec![];
    }
    match chart.kind {
        // Pie, doughnut (Excel's default `varyColors=1`, so every ring repeats the category colours
        // and the legend names categories rather than series), treemap and sunburst.
        ChartKind::Pie | ChartKind::Doughnut | ChartKind::Treemap | ChartKind::Sunburst => {
            let n = first_len(data).min(MAX_CATS);
            (0..n).map(|j| (cat_label(data, j), point_color(data, j), Swatch::Box)).collect()
        }
        ChartKind::Histogram | ChartKind::Funnel | ChartKind::Stock => vec![],
        ChartKind::Waterfall => {
            if data.series.is_empty() {
                return vec![];
            }
            let total = data.series.first().map(|s| s.color).unwrap_or(TEXT);
            vec![("Increase".into(), UP, Swatch::Box), ("Decrease".into(), DOWN, Swatch::Box), ("Total".into(), total, Swatch::Box)]
        }
        _ => {
            let series = data.series.iter().take(MAX_SERIES);
            let mut out: Vec<(String, Rgba, Swatch)> = series
                .clone()
                .map(|s| {
                    let sw = match s.kind {
                        ChartKind::Line | ChartKind::LineStacked | ChartKind::Radar | ChartKind::ScatterLines => Swatch::Line,
                        ChartKind::LineMarkers => Swatch::LineMarker,
                        ChartKind::Scatter | ChartKind::Bubble => Swatch::Marker,
                        _ => Swatch::Box,
                    };
                    (s.name.clone(), s.color, sw)
                })
                .collect();
            // Trendlines follow the series, named "Linear (Sales)" unless given a name.
            for s in series.filter(|s| supports_trendline(s.kind)) {
                for t in s.trendlines.iter().take(MAX_TRENDLINES) {
                    out.push((t.spec.name.clone().unwrap_or_else(|| auto_name(t.spec.kind, &s.name)), t.color, Swatch::Line));
                }
            }
            out
        }
    }
}

// ---------------------------------------------------------------- legend

pub(super) fn swatch_w(sw: Swatch, s: f32) -> f32 {
    if sw == Swatch::Box { s } else { s * 2.0 }
}

pub(super) fn draw_swatch(ctx: &mut Ctx, sw: Swatch, x: f32, yc: f32, s: f32, color: Rgba) {
    match sw {
        Swatch::Box => ctx.rect(x, yc - s / 2.0, s, s, color, None),
        Swatch::Line => ctx.hline(x, x + 2.0 * s, yc, color, 2.0),
        Swatch::LineMarker => {
            ctx.hline(x, x + 2.0 * s, yc, color, 2.0);
            ctx.out.push(Prim::Circle { cx: x + s, cy: yc, r: s * 0.35, fill: Some(color), stroke: None });
        }
        Swatch::Marker => ctx.out.push(Prim::Circle { cx: x + s, cy: yc, r: s * 0.4, fill: Some(color), stroke: None }),
    }
}

pub(super) fn draw_legend(ctx: &mut Ctx, entries: &[(String, Rgba, Swatch)], pos: LegendPos, area: Bx) -> Bx {
    let size = LABEL_SIZE;
    let row_h = size * 1.6;
    let s = size * 0.8;
    let gap = 4.0;
    let spacing = size * 1.3;
    let item_w = |ctx: &Ctx, e: &(String, Rgba, Swatch)| swatch_w(e.2, s) + gap + ctx.tw(&e.0, size);
    match pos {
        LegendPos::None => area,
        LegendPos::Top | LegendPos::Bottom => {
            let max_rows = ((area.h * 0.3 / row_h).floor() as usize).max(1);
            let mut rows: Vec<Vec<(usize, f32)>> = vec![vec![]];
            let mut cur = 0.0f32;
            for (i, e) in entries.iter().enumerate() {
                let iw = item_w(ctx, e).min(area.w);
                let need = if cur > 0.0 { spacing + iw } else { iw };
                if cur > 0.0 && cur + need > area.w {
                    if rows.len() >= max_rows {
                        break;
                    }
                    rows.push(vec![]);
                    cur = 0.0;
                }
                cur += if cur > 0.0 { spacing + iw } else { iw };
                if let Some(r) = rows.last_mut() {
                    r.push((i, iw));
                }
            }
            let lh = rows.len() as f32 * row_h;
            let y0 = if pos == LegendPos::Bottom { area.bottom() - lh } else { area.y };
            for (ri, row) in rows.iter().enumerate() {
                let total: f32 = row.iter().map(|(_, w)| w).sum::<f32>() + spacing * row.len().saturating_sub(1) as f32;
                let mut x = area.x + (area.w - total).max(0.0) / 2.0;
                let yc = y0 + ri as f32 * row_h + row_h / 2.0;
                for &(i, iw) in row {
                    if let Some(e) = entries.get(i) {
                        draw_swatch(ctx, e.2, x, yc, s, e.1);
                        let sx = swatch_w(e.2, s) + gap;
                        let t = ctx.fit(&e.0, size, (iw - sx).max(0.0) + 0.5);
                        ctx.text(x + sx, yc, &t, size, TEXT, (HAlign::Left, VAlign::Middle));
                    }
                    x += iw + spacing;
                }
            }
            let used = lh + 4.0;
            if pos == LegendPos::Bottom { Bx { h: area.h - used, ..area } } else { Bx { y: area.y + used, h: area.h - used, ..area } }
        }
        LegendPos::Left | LegendPos::Right => {
            // Widest entry, capped at 35% of the area: once the cap is reached the rest cannot change it.
            let cap = area.w * 0.35;
            let mut colw = 0.0f32;
            for e in entries {
                if colw >= cap {
                    break;
                }
                colw = colw.max(item_w(ctx, e));
            }
            let colw = colw.min(cap);
            let max_rows = ((area.h / row_h).floor() as usize).max(1);
            let n = entries.len().min(max_rows);
            let lh = n as f32 * row_h;
            let y0 = area.y + (area.h - lh).max(0.0) / 2.0;
            let x0 = if pos == LegendPos::Right { area.right() - colw } else { area.x };
            for (i, e) in entries.iter().take(n).enumerate() {
                let yc = y0 + i as f32 * row_h + row_h / 2.0;
                draw_swatch(ctx, e.2, x0, yc, s, e.1);
                let sx = swatch_w(e.2, s) + gap;
                let t = ctx.fit(&e.0, size, (colw - sx).max(0.0) + 0.5);
                ctx.text(x0 + sx, yc, &t, size, TEXT, (HAlign::Left, VAlign::Middle));
            }
            let used = colw + 8.0;
            if pos == LegendPos::Right { Bx { w: area.w - used, ..area } } else { Bx { x: area.x + used, w: area.w - used, ..area } }
        }
    }
}
