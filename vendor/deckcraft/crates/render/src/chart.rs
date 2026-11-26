//! Charts: title, plot area with axes and gridlines, series marks, legend.

use std::collections::HashMap;

use deckcraft_color::{ColorTransform, Rgba, SchemeSlot};
use deckcraft_model::Shape;
use deckcraft_model::chart::{Chart, ChartType, TrendlineType, axis_scale};
use deckcraft_model::resolve::Ctx;
use deckcraft_model::style::{ColorRef, Fill};
use deckcraft_model::text::{Align, Anchor, Bullet, TextBody};
use kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use vello_cpu::RenderContext;

use crate::color;

const SLOTS: [SchemeSlot; 6] =
    [SchemeSlot::Accent1, SchemeSlot::Accent2, SchemeSlot::Accent3, SchemeSlot::Accent4, SchemeSlot::Accent5, SchemeSlot::Accent6];

/// Colour of series/point `i`: accents, then darker and lighter rounds.
pub fn series_color(ctx: &Ctx, c: &Chart, i: usize) -> Rgba {
    let slot = SLOTS.get(i % 6).copied().unwrap_or(SchemeSlot::Accent1);
    let base = ctx.color(&ColorRef::scheme(slot), None);
    let mono = c.palette.starts_with("monochrome");
    let base = if mono { ctx.color(&ColorRef::scheme(SchemeSlot::Accent1), None) } else { base };
    let round = if mono { i } else { i / 6 };
    match round {
        0 => base,
        1 => deckcraft_color::apply(base, &[ColorTransform::Shade(60000)]),
        2 => deckcraft_color::apply(base, &[ColorTransform::Tint(60000)]),
        n => deckcraft_color::apply(base, &[ColorTransform::LumMod(100000 - (n as i32 % 5) * 12000)]),
    }
}

/// A series' one colour (lines, areas, markers): its solid fill, a pattern's foreground, else the
/// palette's.
pub(crate) fn series_fill(ctx: &Ctx, c: &Chart, i: usize, s: Option<&deckcraft_model::chart::Series>) -> Rgba {
    match s.and_then(|s| s.fill.as_ref()) {
        Some(Fill::Solid { color: cc }) => ctx.color(cc, None),
        Some(Fill::Pattern(p)) => ctx.color(&p.fg, None),
        _ => series_color(ctx, c, i),
    }
}

/// How a bar, or its legend key, is painted.
#[derive(Clone, Debug, PartialEq)]
enum Paint {
    Color(Rgba),
    /// A preset pattern: name, foreground, background.
    Pattern(String, Rgba, Rgba),
    /// `a:noFill`: only the outline, if any, shows.
    Nothing,
}

/// The paint of fill `f` as a bar or legend key draws it; the palette's `fallback` when the file
/// gives none (or one a bar can't show).
fn paint_of(ctx: &Ctx, f: Option<&Fill>, fallback: Rgba) -> Paint {
    match f {
        Some(Fill::Solid { color: cc }) => Paint::Color(ctx.color(cc, None)),
        Some(Fill::Pattern(p)) => Paint::Pattern(p.preset.clone(), ctx.color(&p.fg, None), ctx.color(&p.bg, None)),
        Some(Fill::None) => Paint::Nothing,
        _ => Paint::Color(fallback),
    }
}

/// Fill `r` (in the space `m` maps to the output) with `p`.
fn fill_with(ctx: &mut RenderContext, p: &Paint, r: Rect, m: Affine) {
    match p {
        Paint::Color(c) => {
            ctx.set_transform(m);
            ctx.set_paint(color(*c, 1.0));
            ctx.fill_rect(&r);
        }
        Paint::Pattern(name, fg, bg) => crate::pattern::fill(ctx, name, *fg, *bg, 1.0, &r.to_path(0.1), m),
        Paint::Nothing => {}
    }
}

/// A bar series' outline (`c:spPr/a:ln`) where the file gives it a solid colour: colour and width.
fn bar_outline(ctx: &Ctx, s: &deckcraft_model::chart::Series) -> Option<(Rgba, f64)> {
    let l = s.line.as_ref()?;
    match l.fill.as_ref()? {
        Fill::Solid { color: cc } => Some((ctx.color(cc, None), l.width.unwrap_or(0.75).clamp(0.0, 20.0))),
        _ => None,
    }
}

/// Paint of one bar: a `<c:dPt>` fill wins, and a single-series chart that varies colours by point uses one accent per point
/// (not waterfalls, which are never read from a `<c:barChart>`). Pattern fills draw as patterns and
/// `a:noFill` leaves the bar empty, as in PowerPoint.
fn bar_paint(ctx: &Ctx, c: &Chart, si: usize, pi: usize, s: &deckcraft_model::chart::Series, fills: Option<&HashMap<usize, &Fill>>) -> Paint {
    if let Some(f) = fills.and_then(|m| m.get(&pi)) {
        return paint_of(ctx, Some(*f), series_fill(ctx, c, si, Some(s)));
    }
    if c.vary_colors && c.series.len() == 1 && s.fill.is_none() && c.kind != ChartType::Waterfall {
        return Paint::Color(series_color(ctx, c, pi));
    }
    paint_of(ctx, s.fill.as_ref(), series_color(ctx, c, si))
}

fn polyline(pts: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    for (k, q) in pts.iter().enumerate() {
        if k == 0 {
            p.move_to(*q);
        } else {
            p.line_to(*q);
        }
    }
    p
}

/// A smoothed line (`c:smooth`) through `pts`: a Catmull-Rom spline as cubic Béziers, the curve
/// PowerPoint draws, passing through every point.
pub(crate) fn smooth_path(pts: &[Point]) -> BezPath {
    let mut p = BezPath::new();
    let Some(first) = pts.first() else { return p };
    p.move_to(*first);
    for k in 0..pts.len() - 1 {
        let p0 = pts[k.saturating_sub(1)];
        let (p1, p2) = (pts[k], pts[k + 1]);
        let p3 = pts[(k + 2).min(pts.len() - 1)];
        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        p.curve_to(c1, c2, p2);
    }
    p
}

/// A line series' colour: its line's own (where PowerPoint keeps it), else its fill's, else the
/// palette's.
pub(crate) fn series_line(ctx: &Ctx, c: &Chart, i: usize, s: &deckcraft_model::chart::Series) -> Rgba {
    match s.line.as_ref().and_then(|l| l.fill.as_ref()) {
        Some(Fill::Solid { color: cc }) => ctx.color(cc, None),
        _ => series_fill(ctx, c, i, Some(s)),
    }
}

/// A category label on two lines, split at its middle space or dash ("03.11.2025 - 09.11.2025").
fn split_label(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mid = chars.len() / 2;
    let best = chars.iter().enumerate().filter(|(_, c)| c.is_whitespace()).map(|(i, _)| i).min_by_key(|i| i.abs_diff(mid));
    match best {
        Some(i) if i > 0 && i + 1 < chars.len() => {
            let (a, b) = chars.split_at(i);
            format!("{}\n{}", a.iter().collect::<String>().trim_end(), b.iter().collect::<String>().trim_start())
        }
        _ => name.to_string(),
    }
}

pub(crate) struct LabelStyle {
    pub(crate) size: f64,
    pub(crate) align: Align,
    pub(crate) anchor: Anchor,
    pub(crate) col: Rgba,
    pub(crate) bold: bool,
}

pub(crate) fn label(ctx: &mut RenderContext, rctx: &Ctx, m: Affine, text: &str, r: Rect, style: LabelStyle) {
    let LabelStyle { size, align, anchor, col, bold } = style;
    let mut body = TextBody::from_text(text);
    body.body.inset_l = Some(0.0);
    body.body.inset_r = Some(0.0);
    body.body.inset_t = Some(0.0);
    body.body.inset_b = Some(0.0);
    body.body.anchor = Some(anchor);
    body.body.wrap = Some(false);
    for p in &mut body.paragraphs {
        p.props.align = Some(align);
        p.props.bullet = Some(Bullet::None);
        p.props.margin_left = Some(0.0);
        p.props.indent = Some(0.0);
        for run in &mut p.runs {
            run.props.size = Some(size);
            run.props.bold = Some(bold);
            run.props.fill = Some(Fill::solid(ColorRef::rgb(col)));
        }
    }
    let tmp = Shape { text: Some(body.clone()), ..Default::default() };
    crate::draw_text(
        ctx,
        rctx,
        &tmp,
        &body,
        m,
        &deckcraft_text::Opts { rect: r, fields: &deckcraft_text::NoFields, prompt_color: None, no_shrink: false },
    );
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v:.1}") }
}

/// A scatter or bubble chart's value x axis (min, max, major unit): one scale over every series'
/// x values, from zero when they're all positive. `None` for other charts and when there are no x values.
fn value_x_scale(c: &Chart, max_intervals: usize) -> Option<(f64, f64, f64)> {
    if c.series.is_empty() || !c.series.iter().all(|s| matches!(c.series_kind(s), ChartType::Scatter | ChartType::Bubble)) {
        return None;
    }
    let xs = c.series.iter().flat_map(|s| s.x_values.iter().flatten().copied()).filter(|v| v.is_finite());
    let (lo, hi) = xs.fold(None, |r: Option<(f64, f64)>, v| Some(r.map_or((v, v), |(a, b)| (a.min(v), b.max(v)))))?;
    Some(axis_scale(lo.min(0.0), hi, None, None, None, max_intervals, false))
}

/// Where the legend goes for `c:legendPos`, and the plot area left over.
/// `size` is the legend block (width, height); the result is its top-left corner and the plot area.
/// `r` (also the OOXML default and the fallback for unknown values) and `l` are vertical stacks centred
/// on the area, `tr` is a vertical stack in the top-right corner, `t` and `b` are centred rows.
/// An overlaid legend leaves the plot area as it was.
fn legend_layout(pos: &str, size: (f64, f64), area: Rect, base: f64, overlay: bool) -> (Point, Rect) {
    let (lw, lh) = size;
    let gap = base * 0.5;
    let cx = (area.x0 + area.x1) / 2.0;
    let cy = ((area.y0 + area.y1) / 2.0 - lh / 2.0).max(area.y0);
    let mut plot = area;
    let origin = match pos {
        "b" => {
            plot.y1 = area.y1 - lh - gap;
            Point::new(cx - lw / 2.0, area.y1 - lh)
        }
        "t" => {
            plot.y0 = area.y0 + lh + gap;
            Point::new(cx - lw / 2.0, area.y0)
        }
        "l" => {
            plot.x0 = area.x0 + lw + gap;
            Point::new(area.x0, cy)
        }
        "tr" => {
            plot.x1 = area.x1 - lw - gap;
            Point::new(area.x1 - lw, area.y0)
        }
        _ => {
            plot.x1 = area.x1 - lw - gap;
            Point::new(area.x1 - lw, cy)
        }
    };
    (origin, if overlay { area } else { plot })
}

/// An axis span to divide by: the span itself when positive, else 1 (no absolute floor, so tiny data keeps its scale).
pub(crate) fn positive_span(s: f64) -> f64 {
    if s > 0.0 && s.is_finite() { s } else { 1.0 }
}

/// The tick values of an axis (min, max, major step), one per step (at most 51).
fn axis_ticks((lo, hi, st): (f64, f64, f64)) -> impl Iterator<Item = f64> {
    let n = if st > 0.0 { (((hi - lo) / st).round() as i64).clamp(1, 50) } else { 1 };
    (0..=n).map(move |k| lo + k as f64 * st)
}

/// Most category-by-series cells one chart draws: Excel's 255 series x 32,000 points per series
/// (8,160,000); ordinary charts hold a few hundred.
const MAX_CHART_CELLS: usize = 255 * 32_000;

/// Whether the independent category and series lists make the drawing loops multiply past the budget.
fn over_cell_budget(c: &Chart) -> bool {
    let ncat = c.categories.len().max(c.series.iter().map(|s| s.values.len()).max().unwrap_or(0));
    cells_over_budget(ncat, c.series.len())
}

fn cells_over_budget(categories: usize, series: usize) -> bool {
    categories.saturating_mul(series.max(1)) > MAX_CHART_CELLS
}

pub fn draw(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, w: f64, h: f64) {
    if over_cell_budget(c) {
        return;
    }
    match c.kind {
        // Histograms and Pareto charts are drawn as the column (and line) chart of their bins.
        ChartType::Histogram | ChartType::Pareto => {
            draw_inner(ctx, rctx, &c.histogram_view(), ChartFrame { transform: m, width: w, height: h, boxes: None })
        }
        ChartType::BoxWhisker => {
            let (view, boxes) = crate::chartex::box_view(c);
            draw_inner(ctx, rctx, &view, ChartFrame { transform: m, width: w, height: h, boxes: Some(&boxes) });
        }
        _ => draw_inner(ctx, rctx, c, ChartFrame { transform: m, width: w, height: h, boxes: None }),
    }
}

struct ChartFrame<'a> {
    transform: Affine,
    width: f64,
    height: f64,
    boxes: Option<&'a crate::chartex::Boxes>,
}

/// PowerPoint's default chart text colour: `tx1` with `lumMod` 65000 and `lumOff` 35000, which
/// is 595959 on a light theme and a light grey on a dark one.
pub fn default_text_color() -> ColorRef {
    use deckcraft_color::ColorTransform;
    ColorRef::scheme(SchemeSlot::Tx1).with(ColorTransform::LumMod(65000)).with(ColorTransform::LumOff(35000))
}

fn draw_inner(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, frame: ChartFrame<'_>) {
    let ChartFrame { transform: m, width: w, height: h, boxes } = frame;
    // Text without a colour of its own takes PowerPoint's default, the theme's text colour at
    // lumMod 65 % / lumOff 35 % (595959 on a light theme, light grey on a dark one), so a chart
    // on a dark theme is readable (a fixed grey used to vanish there).
    let text_col = rctx.color(c.text_color.as_ref().unwrap_or(&default_text_color()), None);
    let grid_col = Rgba::rgb(0xD9, 0xD9, 0xD9);
    if let Some(f) = &c.chart_fill {
        crate::paint::fill_path(
            ctx,
            rctx,
            f,
            None,
            &Rect::new(0.0, 0.0, w, h).to_path(0.1),
            Rect::new(0.0, 0.0, w, h),
            m,
            deckcraft_geom::preset::FillMode::Norm,
            1.0,
            rctx.pres,
        );
    }
    // The file's text sizes when it has them (PowerPoint draws chart text at them); otherwise a size
    // that suits the chart.
    let base = c.text_size.unwrap_or_else(|| (h.min(w) / 22.0).clamp(7.0, 18.0));
    let title_size = c.title_size.unwrap_or(base * 1.4);
    let mut area = Rect::new(w * 0.04, h * 0.04, w * 0.96, h * 0.96);
    if let Some(t) = &c.title {
        label(
            ctx,
            rctx,
            m,
            t,
            Rect::new(0.0, area.y0, w, area.y0 + title_size * 1.3),
            LabelStyle { size: title_size, align: Align::Center, anchor: Anchor::Top, col: text_col, bold: false },
        );
        area.y0 += title_size * 1.7;
    }
    let pie = matches!(c.kind, ChartType::Pie | ChartType::Doughnut);
    // Legend placed by `c:legendPos`.
    let waterfall = c.kind == ChartType::Waterfall;
    // Each legend entry: its name, its key's paint and the key's outline.
    let legend_items = build_legend_items(rctx, c, pie, waterfall);
    area = draw_legend(ctx, &LegendDraw { rctx, chart: c, transform: m, items: &legend_items, base, text_color: text_col, width: w }, area);
    if pie {
        draw_pie(ctx, rctx, c, m, area, base, text_col);
        return;
    }
    if matches!(c.kind, ChartType::Funnel | ChartType::Treemap | ChartType::Sunburst | ChartType::RegionMap) {
        crate::chartex::draw(ctx, rctx, c, m, area, base, text_col);
        return;
    }
    if matches!(c.kind, ChartType::Radar | ChartType::FilledRadar) {
        crate::radar::draw(ctx, rctx, c, m, area, base, text_col);
        return;
    }
    let combo = c.kind == ChartType::Combo;
    // Bars are drawn first and lines over them; a combo chart has both.
    let bar_kind = c.series.iter().map(|s| c.series_kind(s)).find(|k| k.is_bar_like());
    let horiz = if combo { bar_kind.is_some_and(ChartType::horizontal) } else { c.kind.horizontal() };
    area = draw_axis_titles(ctx, rctx, &AxisTitleDrawing { chart: c, transform: m, base, text_color: text_col, horiz }, area);
    // As many major units as the value axis has room for labels (PowerPoint's automatic unit).
    let axis_len = if horiz { area.width() - base * 4.5 } else { area.height() - base * 1.8 };
    let max_intervals = (axis_len / if horiz { base * 6.0 } else { base * 1.9 }).floor().clamp(2.0, 12.0) as usize;
    // A logarithmic axis is scaled in exponents of its base.
    let log = c.log_axis_scale(max_intervals).map(|sc| (c.y_log_base.unwrap_or(10.0), sc));
    let (min, max, step) = match log {
        Some((_, sc)) => sc,
        None => c.axis_scale(false, max_intervals).unwrap_or_else(|| axis_scale(0.0, 1.0, None, None, None, max_intervals, true)),
    };
    let to_e = |v: f64| match log {
        Some((b, _)) => v.max(f64::MIN_POSITIVE).log(b).max(min - 1.0),
        None => v,
    };
    // A combo chart's secondary value axis, on the right (vertical value axes only).
    let second = if horiz { None } else { c.axis_scale(true, max_intervals) };
    let ticks = axis_ticks;
    // Label gutters as wide as the longest value label.
    let primary_ticks: Vec<f64> = ticks((min, max, step)).map(|k| log.map_or(k, |(b, _)| b.powf(k))).collect();
    let label_w = |sec: bool, sc: (f64, f64, f64)| {
        let vals: Vec<f64> = if sec { ticks(sc).collect() } else { primary_ticks.clone() };
        vals.iter().map(|v| c.axis_label(sec, *v).chars().count()).max().unwrap_or(1) as f64 * base * 0.55 + base * 0.6
    };
    let gutter = if horiz {
        base * 4.5
    } else if c.y_hidden {
        base * 0.5
    } else {
        clamp_soft(label_w(false, (min, max, step)), base * 1.5, area.width() * 0.3)
    };
    let gutter_r = second.filter(|_| !c.y2_hidden).map(|sc| clamp_soft(label_w(true, sc), base * 1.5, area.width() * 0.3)).unwrap_or(0.0);
    let plot = if horiz {
        Rect::new(area.x0 + gutter, area.y0, area.x1, area.y1 - base * 1.6)
    } else {
        Rect::new(area.x0 + gutter, area.y0, area.x1 - gutter_r, area.y1 - base * 1.8)
    };
    if let Some(f) = &c.plot_fill {
        crate::paint::fill_path(ctx, rctx, f, None, &plot.to_path(0.1), plot, m, deckcraft_geom::preset::FillMode::Norm, 1.0, rctx.pres);
    }
    let span = positive_span(max - min);
    // Values past the axis limits (fixed ones, or zero on an axis that starts above it) stop at
    // the plot's edge.
    let to_v = |v: f64| {
        let v = to_e(v).max(min).min(max);
        if horiz { plot.x0 + (v - min) / span * plot.width() } else { plot.y1 - (v - min) / span * plot.height() }
    };
    // A series' value as a position on its own axis.
    let to_axis = |sec: bool, v: f64| match (sec, second) {
        (true, Some((min2, max2, _))) => plot.y1 - (v.max(min2).min(max2) - min2) / positive_span(max2 - min2) * plot.height(),
        _ => to_v(v),
    };
    draw_gridlines(
        ctx,
        &GridlineDrawing {
            rctx,
            chart: c,
            transform: m,
            area,
            plot,
            horiz,
            base,
            text_color: text_col,
            grid_color: grid_col,
            min,
            max,
            step,
            log,
            primary_ticks: &primary_ticks,
            second,
            to_v: &to_v,
            to_axis: &to_axis,
        },
    );
    let ncat = c.categories.len().max(c.series.iter().map(|s| s.values.len()).max().unwrap_or(0)).max(1);
    // Forecast reserves empty category slots so the line can run past the data. Charts with no
    // forecast keep the old band.
    let (back, fwd) = forecast_pad(c);
    let nspan = ncat as f64 + back + fwd;
    let band = if horiz { plot.height() / nspan } else { plot.width() / nspan };
    // Where category `i`'s band starts along its axis: left to right, or bottom to top on a
    // horizontal bar chart, unless the axis is reversed. The forecast's empty slots stay on
    // their own side of the data.
    let backwards = horiz != c.cat_reversed;
    let slot = |i: usize| if backwards { ncat.saturating_sub(i + 1) as f64 + fwd } else { i as f64 + back };
    // Scatter and bubble charts have a value x axis: one scale over every series' x values, with its
    // own tick labels in place of category names.
    let xscale = if horiz { None } else { value_x_scale(c, (plot.width() / (base * 2.5)).floor().clamp(2.0, 12.0) as usize) };
    let to_x = |v: f64| xscale.map(|(lo, hi, _)| plot.x0 + (v - lo) / positive_span(hi - lo) * plot.width());
    draw_category_labels(
        ctx,
        &CategoryLabelDrawing { rctx, chart: c, transform: m, area, plot, horiz, band, slot: &slot, xscale, base, text_color: text_col },
    );
    if let Some(b) = boxes {
        crate::chartex::draw_boxes(ctx, rctx, c, b, m, plot, band, horiz, &to_v);
        return;
    }
    let zero = match log {
        Some((b, _)) => to_v(b.powf(min)),
        None => to_v(clamp_soft(0.0, min, max)),
    };
    // Error bars, drawn over every series: (position across the value axis, from, to, cap length, cap).
    let mut err_marks = draw_bars(
        ctx,
        &BarDrawing {
            rctx,
            chart: c,
            transform: m,
            plot,
            band,
            horiz,
            ncat,
            bar_kind,
            waterfall,
            slot: &slot,
            to_v: &to_v,
            to_axis: &to_axis,
            base,
            text_color: text_col,
        },
    );
    // A stock chart's high-low lines and up/down bars (or close dashes) stand in for its series.
    if c.kind == ChartType::Stock {
        for i in 0..ncat {
            crate::radar::stock_marks(ctx, rctx, c, m, plot.x0 + band * (i as f64 + 0.5), band, i, &|v| to_axis(false, v));
        }
    }
    draw_lines(
        ctx,
        &mut LineDrawing {
            chart: c,
            rctx,
            transform: m,
            plot,
            category_count: ncat,
            band,
            slot: &slot,
            to_x: &to_x,
            to_axis: &to_axis,
            zero,
            base,
            text_color: text_col,
            error_marks: &mut err_marks,
        },
    );
    // A category position (0-based, fractional) as a slot, the way `slot` places the bars.
    let slot_at = |x: f64| if backwards { ncat as f64 - 1.0 - x + fwd } else { x + back };
    draw_trendlines(ctx, rctx, c, m, TrendlineLayout { plot, band, slot: &slot_at, xscale, ncat, horiz, base, text_col, to_axis: &to_axis });
    if !err_marks.is_empty() {
        let mut p = BezPath::new();
        for &(mid, a, b, cap, end_cap) in &err_marks {
            // A horizontal bar chart's error bars run across, its value axis being horizontal.
            let pt = |along: f64, across: f64| if horiz { Point::new(along, across) } else { Point::new(across, along) };
            p.move_to(pt(a, mid));
            p.line_to(pt(b, mid));
            if end_cap {
                for e in [a, b] {
                    p.move_to(pt(e, mid - cap));
                    p.line_to(pt(e, mid + cap));
                }
            }
        }
        ctx.set_transform(m);
        ctx.set_paint(color(text_col, 1.0));
        ctx.set_stroke(kurbo::Stroke::new(0.75));
        ctx.stroke_path(&p);
    }
    // Axis line.
    let mut ax = BezPath::new();
    if horiz {
        ax.move_to((zero, plot.y0));
        ax.line_to((zero, plot.y1));
    } else {
        ax.move_to((plot.x0, zero));
        ax.line_to((plot.x1, zero));
    }
    ctx.set_transform(m);
    ctx.set_paint(color(Rgba::rgb(0xBF, 0xBF, 0xBF), 1.0));
    ctx.set_stroke(kurbo::Stroke::new(0.75));
    ctx.stroke_path(&ax);
}

struct LegendDraw<'a, 'ctx> {
    rctx: &'a Ctx<'ctx>,
    chart: &'a Chart,
    transform: Affine,
    items: &'a [(String, Paint, Option<(Rgba, f64)>)],
    base: f64,
    text_color: Rgba,
    width: f64,
}

fn draw_legend(ctx: &mut RenderContext, draw: &LegendDraw<'_, '_>, mut area: Rect) -> Rect {
    let rctx = draw.rctx;
    let c = draw.chart;
    let m = draw.transform;
    let legend_items = draw.items;
    let base = draw.base;
    let text_col = draw.text_color;
    let width = draw.width;
    let Some(pos) = c.legend.as_ref().filter(|_| !legend_items.is_empty()) else { return area };
    let vertical = matches!(pos.as_str(), "l" | "r" | "tr");
    // Each entry's text: its own size, weight and colour (`c:legendEntry`) over the legend's.
    let own = |i: usize| c.legend_entries.iter().find(|e| usize::try_from(e.idx).ok() == Some(i));
    let styles: Vec<(f64, bool, Rgba)> = (0..legend_items.len())
        .map(|i| {
            let e = own(i);
            let size = e.and_then(|e| e.size).filter(|v| v.is_finite() && *v > 0.0).map_or(base, |v| v.min(400.0));
            let col = e.and_then(|e| e.color.as_ref()).map_or(text_col, |cr| rctx.color(cr, None));
            (size, e.and_then(|e| e.bold).unwrap_or(false), col)
        })
        .collect();
    let item_ws: Vec<f64> = legend_items.iter().zip(&styles).map(|((n, _, _), (sz, _, _))| n.chars().count() as f64 * sz * 0.55 + sz * 1.6).collect();
    let tallest = styles.iter().map(|s| s.0).fold(base, f64::max);
    let size = if vertical {
        (item_ws.iter().copied().fold(0.0, f64::max).min(width * 0.4), styles.iter().map(|s| s.0 * 1.5).sum::<f64>())
    } else {
        (item_ws.iter().sum::<f64>().min(width * 0.9), tallest * 1.4)
    };
    let (origin, plot_area) = legend_layout(pos, size, area, base, c.legend_overlay);
    area = plot_area;
    let (mut x, mut y) = (origin.x, origin.y);
    for ((name, paint, outline), (sz, bold, col)) in legend_items.iter().zip(&styles) {
        let (sz, bold, col) = (*sz, *bold, *col);
        let key = Rect::new(x, y + sz * 0.3, x + sz * 0.7, y + sz);
        fill_with(ctx, paint, key, m);
        if let Some((oc, ow)) = outline.filter(|o| o.1 > 0.0) {
            ctx.set_transform(m);
            ctx.set_paint(color(oc, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(ow.min(sz * 0.2)));
            ctx.stroke_path(&key.to_path(0.1));
        }
        let tw = name.chars().count() as f64 * sz * 0.55 + sz;
        label(
            ctx,
            rctx,
            m,
            name,
            Rect::new(x + sz, y, x + sz + tw, y + sz * 1.3),
            LabelStyle { size: sz, align: Align::Left, anchor: Anchor::Middle, col, bold },
        );
        if vertical {
            y += sz * 1.5;
        } else {
            x += tw + sz * 0.6;
        }
    }
    area
}

fn build_legend_items(rctx: &Ctx, c: &Chart, pie: bool, waterfall: bool) -> Vec<(String, Paint, Option<(Rgba, f64)>)> {
    if waterfall {
        ["Increase", "Decrease", "Total"].iter().enumerate().map(|(i, n)| (n.to_string(), Paint::Color(series_color(rctx, c, i)), None)).collect()
    } else if matches!(c.kind, ChartType::Treemap | ChartType::Sunburst) {
        // A hierarchy's legend names its top-level groups.
        c.category_tree().iter().enumerate().map(|(i, n)| (n.name.clone(), Paint::Color(series_color(rctx, c, i)), None)).collect()
    } else if matches!(c.kind, ChartType::Funnel | ChartType::RegionMap) {
        // A map draws its own colour scale.
        vec![]
    } else if pie {
        c.categories.iter().enumerate().map(|(i, n)| (n.clone(), Paint::Color(series_color(rctx, c, i)), None)).collect()
    } else {
        c.series
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let k = c.series_kind(s);
                let line = !k.is_bar_like() && !matches!(k, ChartType::Area | ChartType::StackedArea | ChartType::FilledRadar);
                if line {
                    (s.name.clone(), Paint::Color(series_line(rctx, c, i, s)), None)
                } else if k.is_bar_like() {
                    let paint = if c.vary_colors && c.series.len() == 1 && s.fill.is_none() {
                        Paint::Color(series_color(rctx, c, i))
                    } else {
                        paint_of(rctx, s.fill.as_ref(), series_color(rctx, c, i))
                    };
                    (s.name.clone(), paint, bar_outline(rctx, s))
                } else {
                    (s.name.clone(), Paint::Color(series_fill(rctx, c, i, Some(s))), None)
                }
            })
            .collect()
    }
}

struct GridlineDrawing<'a, 'ctx> {
    rctx: &'a Ctx<'ctx>,
    chart: &'a Chart,
    transform: Affine,
    area: Rect,
    plot: Rect,
    horiz: bool,
    base: f64,
    text_color: Rgba,
    grid_color: Rgba,
    min: f64,
    max: f64,
    step: f64,
    log: Option<(f64, (f64, f64, f64))>,
    primary_ticks: &'a [f64],
    second: Option<(f64, f64, f64)>,
    to_v: &'a dyn Fn(f64) -> f64,
    to_axis: &'a dyn Fn(bool, f64) -> f64,
}

fn draw_gridlines(ctx: &mut RenderContext, draw: &GridlineDrawing<'_, '_>) {
    let rctx = draw.rctx;
    let c = draw.chart;
    let m = draw.transform;
    let area = draw.area;
    let plot = draw.plot;
    let horiz = draw.horiz;
    let base = draw.base;
    let text_col = draw.text_color;
    let grid_col = draw.grid_color;
    let min = draw.min;
    let max = draw.max;
    let step = draw.step;
    let log = draw.log;
    let primary_ticks = draw.primary_ticks;
    let second = draw.second;
    let to_v = draw.to_v;
    let to_axis = draw.to_axis;
    let ticks = axis_ticks;
    // Minor gridlines, under the major ones: five to a major unit.
    if c.minor_gridlines {
        let minor = step / 5.0;
        let n = if minor > 0.0 { (((max - min) / minor).round() as i64).clamp(1, 500) } else { 1 };
        let mut g = BezPath::new();
        for k in (0..=n).filter(|k| k % 5 != 0) {
            // On a logarithmic axis `min`, `max` and `step` are exponents of the base.
            let e = min + k as f64 * minor;
            let pos = to_v(log.map_or(e, |(b, _)| b.powf(e)));
            if horiz {
                g.move_to((pos, plot.y0));
                g.line_to((pos, plot.y1));
            } else {
                g.move_to((plot.x0, pos));
                g.line_to((plot.x1, pos));
            }
        }
        ctx.set_transform(m);
        ctx.set_paint(color(Rgba::rgb(0xF2, 0xF2, 0xF2), 1.0));
        ctx.set_stroke(kurbo::Stroke::new(0.75));
        ctx.stroke_path(&g);
    }
    // Gridlines and value labels.
    for &v in primary_ticks {
        let pos = to_v(v);
        let text = if c.y_hidden { String::new() } else { c.axis_label(false, v) };
        let mut g = BezPath::new();
        if horiz {
            g.move_to((pos, plot.y0));
            g.line_to((pos, plot.y1));
            label(
                ctx,
                rctx,
                m,
                &text,
                Rect::new(pos - 30.0, plot.y1 + 2.0, pos + 30.0, plot.y1 + base * 1.5),
                LabelStyle { size: base, align: Align::Center, anchor: Anchor::Top, col: text_col, bold: false },
            );
        } else {
            g.move_to((plot.x0, pos));
            g.line_to((plot.x1, pos));
            label(
                ctx,
                rctx,
                m,
                &text,
                Rect::new(area.x0, pos - base, plot.x0 - base * 0.4, pos + base),
                LabelStyle { size: base, align: Align::Right, anchor: Anchor::Middle, col: text_col, bold: false },
            );
        }
        if c.gridlines {
            ctx.set_transform(m);
            ctx.set_paint(color(grid_col, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(0.75));
            ctx.stroke_path(&g);
        }
    }
    // The secondary axis' labels, on the right, without gridlines of their own.
    if let Some(sc) = second.filter(|_| !c.y2_hidden) {
        for v in ticks(sc) {
            let pos = to_axis(true, v);
            label(
                ctx,
                rctx,
                m,
                &c.axis_label(true, v),
                Rect::new(plot.x1 + base * 0.4, pos - base, area.x1, pos + base),
                LabelStyle { size: base, align: Align::Left, anchor: Anchor::Middle, col: text_col, bold: false },
            );
        }
    }
}

struct CategoryLabelDrawing<'a, 'ctx> {
    rctx: &'a Ctx<'ctx>,
    chart: &'a Chart,
    transform: Affine,
    area: Rect,
    plot: Rect,
    horiz: bool,
    band: f64,
    slot: &'a dyn Fn(usize) -> f64,
    xscale: Option<(f64, f64, f64)>,
    base: f64,
    text_color: Rgba,
}

fn draw_category_labels(ctx: &mut RenderContext, draw: &CategoryLabelDrawing<'_, '_>) {
    let rctx = draw.rctx;
    let c = draw.chart;
    let m = draw.transform;
    let area = draw.area;
    let plot = draw.plot;
    let horiz = draw.horiz;
    let band = draw.band;
    let slot = draw.slot;
    let xscale = draw.xscale;
    let base = draw.base;
    let text_col = draw.text_color;
    let ticks = axis_ticks;
    if let Some(sc) = xscale {
        for v in ticks(sc) {
            let pos = plot.x0 + (v - sc.0) / positive_span(sc.1 - sc.0) * plot.width();
            label(
                ctx,
                rctx,
                m,
                &fmt_num(v),
                Rect::new(pos - 30.0, plot.y1 + 3.0, pos + 30.0, plot.y1 + base * 1.6),
                LabelStyle { size: base, align: Align::Center, anchor: Anchor::Top, col: text_col, bold: false },
            );
        }
    }
    // Category labels: wrapped onto two lines when they don't fit their band (long date ranges).
    let wraps = !horiz && c.categories.iter().any(|n| n.chars().count() as f64 * base * 0.5 > band);
    for (i, name) in c.categories.iter().enumerate().filter(|_| xscale.is_none()) {
        let mid = if horiz { plot.y0 + band * (slot(i) + 0.5) } else { plot.x0 + band * (slot(i) + 0.5) };
        if horiz {
            label(
                ctx,
                rctx,
                m,
                name,
                Rect::new(area.x0, mid - base, plot.x0 - base * 0.4, mid + base),
                LabelStyle { size: base, align: Align::Right, anchor: Anchor::Middle, col: text_col, bold: false },
            );
        } else {
            let text = if wraps { split_label(name) } else { name.clone() };
            label(
                ctx,
                rctx,
                m,
                &text,
                Rect::new(mid - band / 2.0, plot.y1 + 3.0, mid + band / 2.0, plot.y1 + base * if wraps { 2.8 } else { 1.6 }),
                LabelStyle { size: base, align: Align::Center, anchor: Anchor::Top, col: text_col, bold: false },
            );
        }
    }
}

struct AxisTitleDrawing<'a> {
    chart: &'a Chart,
    transform: Affine,
    base: f64,
    text_color: Rgba,
    horiz: bool,
}

fn draw_axis_titles(ctx: &mut RenderContext, rctx: &Ctx, draw: &AxisTitleDrawing<'_>, mut area: Rect) -> Rect {
    let c = draw.chart;
    let m = draw.transform;
    let base = draw.base;
    let text_col = draw.text_color;
    let horiz = draw.horiz;
    // Axis titles: the left one turned to read upwards, the bottom one centred under the axis.
    // On a horizontal bar chart the category axis is the left one.
    let (left_title, bottom_title) =
        if horiz { (c.axis_titles.0.as_deref(), c.axis_titles.1.as_deref()) } else { (c.axis_titles.1.as_deref(), c.axis_titles.0.as_deref()) };
    let band_h = base * 1.6;
    if let Some(t) = bottom_title.filter(|t| !t.is_empty()) {
        let r = Rect::new(area.x0, area.y1 - band_h, area.x1, area.y1);
        label(ctx, rctx, m, t, r, LabelStyle { size: base, align: Align::Center, anchor: Anchor::Bottom, col: text_col, bold: true });
        area.y1 -= band_h + base * 0.3;
    }
    if let Some(t) = left_title.filter(|t| !t.is_empty()) {
        let (cx, cy) = (area.x0 + band_h / 2.0, (area.y0 + area.y1) / 2.0);
        let turned = m * Affine::translate((cx, cy)) * Affine::rotate(-std::f64::consts::FRAC_PI_2);
        let half = area.height() / 2.0;
        label(
            ctx,
            rctx,
            turned,
            t,
            Rect::new(-half, -band_h / 2.0, half, band_h / 2.0),
            LabelStyle { size: base, align: Align::Center, anchor: Anchor::Middle, col: text_col, bold: true },
        );
        area.x0 += band_h + base * 0.3;
    }
    area
}

struct BarDrawing<'a, 'ctx> {
    rctx: &'a Ctx<'ctx>,
    chart: &'a Chart,
    transform: Affine,
    plot: Rect,
    band: f64,
    horiz: bool,
    ncat: usize,
    bar_kind: Option<ChartType>,
    waterfall: bool,
    slot: &'a dyn Fn(usize) -> f64,
    to_v: &'a dyn Fn(f64) -> f64,
    to_axis: &'a dyn Fn(bool, f64) -> f64,
    base: f64,
    text_color: Rgba,
}

struct LineDrawing<'a, 'ctx> {
    chart: &'a Chart,
    rctx: &'a Ctx<'ctx>,
    transform: Affine,
    plot: Rect,
    category_count: usize,
    band: f64,
    slot: &'a dyn Fn(usize) -> f64,
    to_x: &'a dyn Fn(f64) -> Option<f64>,
    to_axis: &'a dyn Fn(bool, f64) -> f64,
    zero: f64,
    base: f64,
    text_color: Rgba,
    error_marks: &'a mut Vec<(f64, f64, f64, f64, bool)>,
}

fn draw_lines(ctx: &mut RenderContext, draw: &mut LineDrawing<'_, '_>) {
    let c = draw.chart;
    let rctx = draw.rctx;
    let m = draw.transform;
    let plot = draw.plot;
    let ncat = draw.category_count;
    let band = draw.band;
    let slot = draw.slot;
    let to_x = draw.to_x;
    let to_axis = draw.to_axis;
    let zero = draw.zero;
    let base = draw.base;
    let err_marks = &mut draw.error_marks;
    let mut acc = vec![0.0; ncat];
    for (si, s) in c.series.iter().enumerate() {
        let kind = c.series_kind(s);
        if kind.is_bar_like() || kind == ChartType::Stock {
            continue;
        }
        let sec = c.on_secondary(s);
        let stacked = kind.stacked();
        let area_kind = matches!(kind, ChartType::Area | ChartType::StackedArea | ChartType::FilledRadar);
        let col = if area_kind { series_fill(rctx, c, si, Some(s)) } else { series_line(rctx, c, si, s) };
        let series_stats = s.error_bars.as_ref().map(|_| deckcraft_model::chart::SeriesStats::of(&s.values));
        let mut pts = vec![];
        // Each point's own value (not the stacked total), for its data label.
        let mut values = vec![];
        // Whether the series has any x value, found once rather than per point.
        let any_x = s.x_values.iter().flatten().next().is_some();
        for i in 0..ncat {
            let Some(v) = s.values.get(i).copied().flatten() else { continue };
            let own = v;
            let v = if stacked {
                let base_v = acc.get(i).copied().unwrap_or(0.0);
                if let Some(a) = acc.get_mut(i) {
                    *a += v;
                }
                base_v + v
            } else {
                v
            };
            let x = match kind {
                ChartType::Scatter | ChartType::Bubble => {
                    match (s.x_values.get(i).copied().flatten().and_then(to_x), any_x) {
                        (Some(x), _) => x,
                        // No x values at all: the points are numbered 1, 2, 3...
                        (None, false) => to_x(i as f64 + 1.0).unwrap_or(plot.x0 + band * (slot(i) + 0.5)),
                        (None, true) => continue,
                    }
                }
                _ => plot.x0 + band * (slot(i) + 0.5),
            };
            if let Some((lo, hi)) = s.error_bars.as_ref().and_then(|e| series_stats.as_ref().and_then(|st| e.range_in(st, i, v))) {
                err_marks.push((x, to_axis(sec, lo), to_axis(sec, hi), base * 0.4, s.error_bars.as_ref().is_some_and(|e| e.end_cap)));
            }
            pts.push(Point::new(x, to_axis(sec, v)));
            values.push(own);
        }
        if pts.is_empty() {
            continue;
        }
        let p = if s.smooth && !area_kind && pts.len() > 2 { smooth_path(&pts) } else { polyline(&pts) };
        ctx.set_transform(m);
        if area_kind {
            let mut a = p.clone();
            if let (Some(l), Some(f)) = (pts.last(), pts.first()) {
                a.line_to((l.x, zero));
                a.line_to((f.x, zero));
                a.close_path();
            }
            ctx.set_paint(color(col, 0.85));
            ctx.fill_path(&a);
        } else if kind != ChartType::Scatter {
            let lw = s.line.as_ref().and_then(|l| l.width).filter(|w| w.is_finite() && *w > 0.0).unwrap_or(2.25).clamp(0.25, 20.0);
            ctx.set_paint(color(col, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(lw).with_join(kurbo::Join::Round).with_caps(kurbo::Cap::Round));
            ctx.stroke_path(&p);
        }
        if matches!(kind, ChartType::LineMarkers | ChartType::Scatter | ChartType::Bubble) {
            ctx.set_paint(color(col, 1.0));
            for q in &pts {
                ctx.fill_path(&kurbo::Circle::new(*q, 3.5).to_path(0.1));
            }
        }
        // A line series that asks for value labels (`c:dLbls/c:showVal`) shows each point's value
        // just above it, as bars do theirs (#2066). Area series keep none (theirs sit inside the
        // area), and scatter and bubble series, which also come here, are left as they were.
        if matches!(kind, ChartType::Line | ChartType::LineMarkers | ChartType::StackedLine) && c.series_data_labels(s) {
            let gap = base * 0.3 + 3.5;
            for (q, v) in pts.iter().zip(&values) {
                label(
                    ctx,
                    rctx,
                    m,
                    &c.data_label(s, *v),
                    Rect::new(q.x - 40.0, q.y - gap - base * 1.4, q.x + 40.0, q.y - gap),
                    LabelStyle { size: base * 0.9, align: Align::Center, anchor: Anchor::Bottom, col: draw.text_color, bold: false },
                );
            }
        }
    }
}

fn draw_bars(ctx: &mut RenderContext, draw: &BarDrawing<'_, '_>) -> Vec<(f64, f64, f64, f64, bool)> {
    let c = draw.chart;
    let rctx = draw.rctx;
    let m = draw.transform;
    let plot = draw.plot;
    let band = draw.band;
    let horiz = draw.horiz;
    let ncat = draw.ncat;
    let bar_kind = draw.bar_kind;
    let waterfall = draw.waterfall;
    let slot = draw.slot;
    let to_v = draw.to_v;
    let to_axis = draw.to_axis;
    let base = draw.base;
    let text_col = draw.text_color;
    let mut err_marks: Vec<(f64, f64, f64, f64, bool)> = vec![];
    if let Some(bk) = bar_kind {
        let bars: Vec<(usize, &deckcraft_model::chart::Series)> =
            c.series.iter().enumerate().filter(|(_, s)| c.series_kind(s).is_bar_like()).collect();
        let ns = bars.len().max(1);
        // Gap width and overlap are fractions of one bar's width: a category's band holds its bars
        // (overlapping by `overlap` of a bar between neighbours) plus one gap.
        let gap = c.gap_width.clamp(0.0, 5.0);
        let stacked = bk.stacked();
        let ov = if stacked { 1.0 } else { c.overlap.clamp(-1.0, 1.0) };
        let n = ns as f64;
        let bar = band / (n - (n - 1.0) * ov + gap);
        let step = bar * (1.0 - ov);
        let group = bar + step * (n - 1.0);
        // Error bars need each series' mean and spread: worked out once, not per point.
        let stats: Vec<deckcraft_model::chart::SeriesStats> = bars.iter().map(|(_, s)| deckcraft_model::chart::SeriesStats::of(&s.values)).collect();
        // Each series' per-point fills by point (the first for a point wins), not searched per bar.
        let point_fills: Vec<HashMap<usize, &Fill>> = bars
            .iter()
            .map(|(_, s)| {
                let mut m = HashMap::new();
                for (k, f) in &s.point_fills {
                    m.entry(*k).or_insert(f);
                }
                m
            })
            .collect();
        // A waterfall's running bars, worked out once for all its categories.
        let waterfall_bars = if waterfall { c.waterfall_bars() } else { Vec::new() };
        for i in 0..ncat {
            let mut pos_acc = 0.0;
            let mut neg_acc = 0.0;
            let total: f64 = if bk.percent() {
                bars.iter().map(|(_, s)| s.values.get(i).copied().flatten().unwrap_or(0.0).abs()).sum::<f64>().max(1e-9)
            } else {
                1.0
            };
            for (bi, (si, s)) in bars.iter().enumerate() {
                let sec = c.on_secondary(s);
                let v = s.values.get(i).copied().flatten().unwrap_or(0.0) / total;
                let start = band * slot(i) + (band - group) / 2.0 + if stacked { 0.0 } else { step * bi as f64 };
                let (a, b) = if waterfall {
                    waterfall_bars.get(i).copied().unwrap_or_default()
                } else if stacked {
                    if v >= 0.0 {
                        let r = (pos_acc, pos_acc + v);
                        pos_acc += v;
                        r
                    } else {
                        let r = (neg_acc + v, neg_acc);
                        neg_acc += v;
                        r
                    }
                } else {
                    (0.0f64.min(v), 0.0f64.max(v))
                };
                let r = if horiz {
                    Rect::new(to_v(a), plot.y0 + start, to_v(b), plot.y0 + start + bar)
                } else {
                    Rect::new(plot.x0 + start, to_axis(sec, b), plot.x0 + start + bar, to_axis(sec, a))
                }
                .abs();
                // A waterfall colours increases, decreases and totals, and joins each bar's end
                // to the next bar's start.
                let paint = if waterfall {
                    let slot = if c.subtotals.contains(&i) {
                        2
                    } else if b < a {
                        1
                    } else {
                        0
                    };
                    Paint::Color(series_color(rctx, c, slot))
                } else {
                    bar_paint(rctx, c, *si, i, s, point_fills.get(bi))
                };
                fill_with(ctx, &paint, r.abs(), m);
                if !waterfall && let Some((oc, ow)) = bar_outline(rctx, s).filter(|o| o.1 > 0.0) {
                    ctx.set_transform(m);
                    ctx.set_paint(color(oc, 1.0));
                    ctx.set_stroke(kurbo::Stroke::new(ow));
                    ctx.stroke_path(&r.abs().to_path(0.1));
                }
                if waterfall && i + 1 < ncat && !horiz {
                    let y = to_v(b);
                    let mut j = BezPath::new();
                    j.move_to((r.x1, y));
                    j.line_to((r.x1 + (band - bar), y));
                    ctx.set_paint(color(Rgba::rgb(0xA6, 0xA6, 0xA6), 1.0));
                    ctx.set_stroke(kurbo::Stroke::new(0.75));
                    ctx.stroke_path(&j);
                }
                if let Some(e) = &s.error_bars {
                    let (end, mid) = if horiz { (if v >= 0.0 { b } else { a }, r.center().y) } else { (if v >= 0.0 { b } else { a }, r.center().x) };
                    if let Some((lo, hi)) = stats.get(bi).and_then(|st| e.range_in(st, i, end)) {
                        let (p0, p1) = if horiz { (to_v(lo), to_v(hi)) } else { (to_axis(sec, lo), to_axis(sec, hi)) };
                        err_marks.push((mid, p0, p1, bar.min(base * 0.8) * 0.5, e.end_cap));
                    }
                }
                if let Some(t) = bar_data_label(c, s, i) {
                    let lr = if horiz {
                        Rect::new(r.x1 + 2.0, r.y0, r.x1 + 40.0, r.y1)
                    } else {
                        Rect::new(r.x0 - 10.0, r.y0 - base * 1.4, r.x1 + 10.0, r.y0)
                    };
                    label(
                        ctx,
                        rctx,
                        m,
                        &t,
                        lr,
                        LabelStyle {
                            size: base * 0.9,
                            align: if horiz { Align::Left } else { Align::Center },
                            anchor: if horiz { Anchor::Middle } else { Anchor::Bottom },
                            col: text_col,
                            bold: false,
                        },
                    );
                }
            }
        }
    }
    err_marks
}

fn bar_data_label(c: &Chart, s: &deckcraft_model::chart::Series, i: usize) -> Option<String> {
    if !c.series_data_labels(s) {
        return None;
    }
    s.values.get(i).copied().flatten().map(|value| c.data_label(s, value))
}

#[cfg(test)]
mod bar_label_tests {
    use super::bar_data_label;
    use deckcraft_model::chart::{Chart, Series};

    #[test]
    fn missing_bar_values_have_no_label_but_explicit_zero_does() {
        let chart = Chart { data_labels: true, ..Default::default() };
        let series = Series { values: [Some(1.0), None, Some(3.0)].into(), ..Default::default() };

        let labels: Vec<_> = (0..series.values.len()).filter_map(|i| bar_data_label(&chart, &series, i)).collect();

        assert_eq!(labels, ["1", "3"]);
        let zero = Series { values: [Some(0.0)].into(), ..Default::default() };
        assert_eq!(bar_data_label(&chart, &zero, 0).as_deref(), Some("0"));
    }
}

fn draw_pie(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64, text_col: Rgba) {
    let Some(s) = c.series.first() else { return };
    let vals: Vec<f64> = s.values.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect();
    let total: f64 = vals.iter().sum();
    if total <= 0.0 {
        return;
    }
    let r = (area.width().min(area.height()) / 2.0) * 0.95;
    let cen = area.center();
    let mut a0 = -std::f64::consts::FRAC_PI_2;
    for (i, v) in vals.iter().enumerate() {
        let sweep = v / total * std::f64::consts::TAU;
        let mut p = BezPath::new();
        p.move_to(cen);
        let arc = kurbo::Arc::new(cen, (r, r), a0, sweep, 0.0);
        p.line_to(Point::new(cen.x + r * a0.cos(), cen.y + r * a0.sin()));
        p.extend(arc.append_iter(0.1));
        p.close_path();
        ctx.set_transform(m);
        let col = s
            .point_fills
            .iter()
            .find(|(k, _)| *k == i)
            .and_then(|(_, f)| if let Fill::Solid { color: cc } = f { Some(rctx.color(cc, None)) } else { None })
            .unwrap_or_else(|| series_color(rctx, c, i));
        ctx.set_paint(color(col, 1.0));
        ctx.fill_path(&p);
        ctx.set_paint(color(Rgba::WHITE, 1.0));
        ctx.set_stroke(kurbo::Stroke::new(1.0));
        ctx.stroke_path(&p);
        if c.series_data_labels(s) {
            let mid = a0 + sweep / 2.0;
            let lp = Point::new(cen.x + r * 0.65 * mid.cos(), cen.y + r * 0.65 * mid.sin());
            label(
                ctx,
                rctx,
                m,
                &c.data_label(s, *v),
                Rect::new(lp.x - 30.0, lp.y - 10.0, lp.x + 30.0, lp.y + 10.0),
                LabelStyle {
                    size: base,
                    align: Align::Center,
                    anchor: Anchor::Middle,
                    // On the slices themselves: white, unless the chart's text was given a colour.
                    col: if c.text_color.is_some() { text_col } else { Rgba::WHITE },
                    bold: false,
                },
            );
        }
        a0 += sweep;
    }
    if c.kind == ChartType::Doughnut {
        ctx.set_transform(m);
        ctx.set_paint(color(Rgba::WHITE, 1.0));
        ctx.fill_path(&kurbo::Circle::new(cen, r * c.hole_size.clamp(0.1, 0.9)).to_path(0.1));
    }
}

/// `v` kept within `lo..=hi` without `f64::clamp`'s panic when `lo > hi` (a chart smaller than
/// its label font) or a bound is NaN: the upper bound wins.
pub(crate) fn clamp_soft(v: f64, lo: f64, hi: f64) -> f64 {
    let v = if v < lo { lo } else { v };
    if v > hi { hi } else { v }
}

/// A corner of `plot` whose box stays clear of `pts` (the drawn trendline).
fn caption_at(plot: Rect, box_w: f64, box_h: f64, pts: &[Point], n: usize) -> (f64, f64) {
    let pad = 4.0;
    let shift = n as f64 * (box_h + 2.0);
    let x_right = (plot.x1 - box_w - pad).max(plot.x0 + pad);
    let y_bottom = (plot.y1 - box_h - pad - shift).max(plot.y0 + pad);
    let y_top = (plot.y0 + pad + shift).min(y_bottom);
    let corners = [(plot.x0 + pad, y_top), (x_right, y_top), (plot.x0 + pad, y_bottom), (x_right, y_bottom)];
    let mut best = corners[0];
    let mut best_d = -1.0;
    for (x, y) in corners {
        let mut min_d = f64::MAX;
        for p in pts {
            let dx = if p.x < x {
                x - p.x
            } else if p.x > x + box_w {
                p.x - (x + box_w)
            } else {
                0.0
            };
            let dy = if p.y < y {
                y - p.y
            } else if p.y > y + box_h {
                p.y - (y + box_h)
            } else {
                0.0
            };
            min_d = min_d.min((dx * dx + dy * dy).sqrt());
        }
        if pts.is_empty() {
            min_d = 1.0;
        }
        if min_d > best_d {
            best_d = min_d;
            best = (x, y);
        }
    }
    best
}

fn pad_units(v: Option<f64>) -> f64 {
    v.filter(|v| v.is_finite() && *v > 0.0).map(|v| v.min(3.0)).unwrap_or(0.0)
}

/// Empty category slots reserved for a trendline forecast, capped so the data still fills the plot.
fn forecast_pad(c: &Chart) -> (f64, f64) {
    if matches!(c.kind, ChartType::Scatter | ChartType::Bubble) {
        return (0.0, 0.0);
    }
    let mut back = 0.0_f64;
    let mut fwd = 0.0_f64;
    for s in &c.series {
        if matches!(c.series_kind(s), ChartType::Scatter | ChartType::Bubble) {
            continue;
        }
        for t in s.trendlines.iter().take(8) {
            if t.kind == TrendlineType::MovingAvg {
                continue;
            }
            back = back.max(pad_units(t.backward));
            fwd = fwd.max(pad_units(t.forward));
        }
    }
    (back, fwd)
}

const TREND_INK: [Rgba; 4] = [Rgba::rgb(0x22, 0x22, 0x22), Rgba::rgb(0xB0, 0x30, 0x30), Rgba::rgb(0x1A, 0x5C, 0x7A), Rgba::rgb(0xA0, 0x68, 0x00)];

struct XYMapper<'a> {
    plot: Rect,
    band: f64,
    slot: &'a dyn Fn(f64) -> f64,
    horiz: bool,
    scatter: bool,
    scatter_x: (f64, f64),
    secondary: bool,
    to_axis: &'a dyn Fn(bool, f64) -> f64,
}

fn map_xy(mapper: &XYMapper<'_>, x: f64, y: f64) -> Option<Point> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let value = (mapper.to_axis)(mapper.secondary, y);
    if !value.is_finite() {
        return None;
    }
    let along = if mapper.scatter && mapper.scatter_x.1 > mapper.scatter_x.0 {
        mapper.plot.x0 + (x - mapper.scatter_x.0) / (mapper.scatter_x.1 - mapper.scatter_x.0) * mapper.plot.width()
    } else {
        let origin = if mapper.horiz { mapper.plot.y0 } else { mapper.plot.x0 };
        origin + mapper.band * ((mapper.slot)(x - 1.0) + 0.5)
    };
    if !along.is_finite() {
        return None;
    }
    Some(if mapper.horiz && !mapper.scatter { Point::new(value, along) } else { Point::new(along, value) })
}

/// The part of segment `a`–`b` that lies inside `r`.
fn clip_seg(a: Point, b: Point, r: Rect) -> Option<(Point, Point)> {
    let (mut t0, mut t1) = (0.0, 1.0);
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let p = [-dx, dx, -dy, dy];
    let q = [a.x - r.x0, r.x1 - a.x, a.y - r.y0, r.y1 - a.y];
    for i in 0..4 {
        let (pi, qi) = (p[i], q[i]);
        if pi.abs() < 1e-12 {
            if qi < 0.0 {
                return None;
            }
        } else {
            let t = qi / pi;
            if pi < 0.0 {
                if t > t1 {
                    return None;
                } else if t > t0 {
                    t0 = t;
                }
            } else if t < t0 {
                return None;
            } else if t < t1 {
                t1 = t;
            }
        }
    }
    Some((Point::new(a.x + t0 * dx, a.y + t0 * dy), Point::new(a.x + t1 * dx, a.y + t1 * dy)))
}

struct TrendlineLayout<'a> {
    plot: Rect,
    band: f64,
    slot: &'a dyn Fn(f64) -> f64,
    xscale: Option<(f64, f64, f64)>,
    ncat: usize,
    horiz: bool,
    base: f64,
    text_col: Rgba,
    to_axis: &'a dyn Fn(bool, f64) -> f64,
}

fn draw_trendlines(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, layout: TrendlineLayout<'_>) {
    let TrendlineLayout { plot, band, slot, xscale, ncat, horiz, base, text_col, to_axis } = layout;
    let mut label_n = 0usize;
    for s in &c.series {
        if s.trendlines.is_empty() {
            continue;
        }
        let kind = c.series_kind(s);
        let scatter = matches!(kind, ChartType::Scatter | ChartType::Bubble);
        let sec = c.on_secondary(s);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for i in 0..ncat {
            let Some(y) = s.values.get(i).copied().flatten().filter(|v| v.is_finite()) else { continue };
            let x = if scatter {
                let Some(x) = s.x_values.get(i).copied().flatten().filter(|v| v.is_finite()) else { continue };
                x
            } else {
                (i + 1) as f64
            };
            xs.push(x);
            ys.push(y);
        }
        // The same x extent the scatter marks use, so the fit sits on the points: the chart's
        // value x axis where it has one, else the series' own x values.
        let (sx0, sx1) = if let (true, Some((lo, hi, _))) = (scatter, xscale) {
            (lo, hi)
        } else if scatter {
            let mut lo = f64::MAX;
            let mut hi = f64::MIN;
            for v in s.x_values.iter().copied().flatten().filter(|v| v.is_finite()) {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            (lo, hi)
        } else {
            (0.0, 0.0)
        };
        let mapper = XYMapper { plot, band, slot, horiz, scatter, scatter_x: (sx0, sx1), secondary: sec, to_axis };
        for (ti, t) in s.trendlines.iter().take(8).enumerate() {
            let Some(fit) = t.fit(&xs, &ys) else { continue };
            let (x0, x1) = fit.span(t.forward, t.backward);
            let samples = fit.samples(x0, x1);
            let mut path = BezPath::new();
            let mut prev: Option<Point> = None;
            let mut ink_pts = Vec::new();
            let mut any = false;
            for w in samples.windows(2) {
                let (Some(a), Some(b)) = (map_xy(&mapper, w[0].0, w[0].1), map_xy(&mapper, w[1].0, w[1].1)) else {
                    prev = None;
                    continue;
                };
                let Some((ca, cb)) = clip_seg(a, b, plot) else {
                    prev = None;
                    continue;
                };
                let cont = prev.is_some_and(|p| (p.x - ca.x).abs() < 0.6 && (p.y - ca.y).abs() < 0.6);
                if cont {
                    path.line_to(cb);
                } else {
                    path.move_to(ca);
                    path.line_to(cb);
                }
                prev = Some(cb);
                if ink_pts.len() < 64 {
                    ink_pts.push(ca);
                    ink_pts.push(cb);
                }
                any = true;
            }
            if !any {
                continue;
            }
            ctx.set_transform(m);
            let ink = TREND_INK.get(ti % TREND_INK.len()).copied().unwrap_or(Rgba::rgb(0x22, 0x22, 0x22));
            ctx.set_paint(color(ink, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(1.6).with_dashes(0.0, [6.0, 3.5]).with_caps(kurbo::Cap::Round));
            ctx.stroke_path(&path);
            let caption = t.caption(&fit);
            if caption.is_empty() {
                continue;
            }
            let lines = caption.chars().filter(|ch| *ch == '\n').count() + 1;
            let box_h = base * 1.3 * lines as f64 + 2.0;
            let widest = caption.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
            let box_w = clamp_soft(widest * base * 0.62, 48.0, plot.width() * 0.72);
            let (x, y) = caption_at(plot, box_w, box_h, &ink_pts, label_n);
            ctx.set_transform(m);
            ctx.set_paint(color(Rgba::WHITE, 1.0));
            ctx.fill_rect(&Rect::new(x, y, x + box_w, y + box_h));
            label(
                ctx,
                rctx,
                m,
                &caption,
                Rect::new(x, y, x + box_w, y + box_h),
                LabelStyle { size: (base * 0.9).clamp(8.0, 13.0), align: Align::Left, anchor: Anchor::Top, col: text_col, bold: false },
            );
            label_n += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0.0, 0.0, 400.0, 300.0);

    #[test]
    fn category_by_series_cells_are_budgeted() {
        assert!(!over_cell_budget(&Chart::sample(ChartType::Column)));
        // Excel's maximum chart (255 series x 32,000 points) is within the budget; one more point is not.
        assert_eq!(MAX_CHART_CELLS, 8_160_000);
        assert!(!cells_over_budget(32_000, 255));
        assert!(cells_over_budget(32_001, 255));
        assert!(cells_over_budget(usize::MAX, 2));
    }

    #[test]
    fn tiny_data_gets_several_ticks() {
        let sc = deckcraft_model::chart::axis_scale(1e-13, 5e-13, None, None, None, 8, true);
        assert!(axis_ticks(sc).count() >= 3, "{sc:?}");
    }

    #[test]
    fn scatter_x_is_a_value_scale_over_all_series() {
        let mut c = Chart::sample(ChartType::Scatter);
        c.series[0].x_values = vec![Some(0.5), Some(7.2), None];
        assert_eq!(value_x_scale(&c, 8), Some((0.0, 8.0, 1.0)));
        assert_eq!(value_x_scale(&Chart::sample(ChartType::Column), 8), None);
    }

    #[test]
    fn right_legend_is_a_centred_column_and_narrows_the_plot() {
        let (o, plot) = legend_layout("r", (60.0, 100.0), AREA, 10.0, false);
        assert_eq!((o.x, o.y), (340.0, 100.0));
        assert_eq!((plot.x0, plot.x1, plot.y0, plot.y1), (0.0, 335.0, 0.0, 300.0));
    }

    #[test]
    fn left_top_and_corner_positions() {
        let (o, plot) = legend_layout("l", (60.0, 100.0), AREA, 10.0, false);
        assert_eq!((o.x, o.y, plot.x0, plot.x1), (0.0, 100.0, 65.0, 400.0));
        let (o, plot) = legend_layout("t", (100.0, 14.0), AREA, 10.0, false);
        assert_eq!((o.x, o.y, plot.y0, plot.y1), (150.0, 0.0, 19.0, 300.0));
        let (o, plot) = legend_layout("tr", (60.0, 100.0), AREA, 10.0, false);
        assert_eq!((o.x, o.y, plot.x1), (340.0, 0.0, 335.0));
    }

    #[test]
    fn bottom_legend_keeps_the_old_row_layout() {
        let (o, plot) = legend_layout("b", (100.0, 14.0), AREA, 10.0, false);
        assert_eq!((o.x, o.y, plot.y1, plot.x1), (150.0, 286.0, 281.0, 400.0));
    }

    #[test]
    fn overlay_does_not_shrink_the_plot() {
        let (o, plot) = legend_layout("r", (60.0, 100.0), AREA, 10.0, true);
        assert_eq!(plot, AREA);
        assert_eq!(o.x, 340.0);
    }

    #[test]
    fn clip_keeps_the_part_inside_the_plot() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let (a, b) = clip_seg(Point::new(-50.0, 25.0), Point::new(150.0, 25.0), r).expect("clipped");
        assert!((a.x - 0.0).abs() < 1e-6 && (b.x - 100.0).abs() < 1e-6);
        assert!((a.y - 25.0).abs() < 1e-6);
        assert!(clip_seg(Point::new(-10.0, -10.0), Point::new(-1.0, -1.0), r).is_none());
        let (a, b) = clip_seg(Point::new(10.0, 10.0), Point::new(40.0, 20.0), r).expect("inside");
        assert!((a.x - 10.0).abs() < 1e-6 && (b.x - 40.0).abs() < 1e-6);
    }

    #[test]
    fn caption_keeps_clear_of_a_line_into_the_top_right() {
        let plot = Rect::new(0.0, 0.0, 200.0, 100.0);
        let pts = [Point::new(20.0, 80.0), Point::new(180.0, 15.0)];
        let (x, y) = caption_at(plot, 60.0, 20.0, &pts, 0);
        assert!(x > 100.0 && y > 40.0, "label on the line at {x} {y}");
    }

    #[test]
    fn forecast_reserves_at_most_three_category_slots() {
        use deckcraft_model::chart::Trendline;
        let mut c = Chart::sample(ChartType::Line);
        c.series[0].trendlines.push(Trendline { forward: Some(10.0), backward: Some(0.5), ..Default::default() });
        assert_eq!(forecast_pad(&c), (0.5, 3.0));
        c.series[0].trendlines[0].kind = TrendlineType::MovingAvg;
        assert_eq!(forecast_pad(&c), (0.0, 0.0));
        c.kind = ChartType::Scatter;
        c.series[0].trendlines[0].kind = TrendlineType::Linear;
        assert_eq!(forecast_pad(&c), (0.0, 0.0));
    }
}
