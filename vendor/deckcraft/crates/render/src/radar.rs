//! Radar charts: one spoke per category, the value axis running up the first spoke, a polygon
//! of gridlines per major unit and each series a closed polygon (filled for a filled radar).

use deckcraft_color::Rgba;
use deckcraft_model::chart::{Chart, ChartType};
use deckcraft_model::resolve::Ctx;
use deckcraft_model::text::{Align, Anchor};
use kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use vello_cpu::RenderContext;

use crate::chart::{label, series_fill, series_line};
use crate::color;

/// The point at `frac` of the radius along spoke `i` of `n` (the first spoke points up).
fn spoke(cen: Point, r: f64, i: usize, n: usize, frac: f64) -> Point {
    let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * i as f64 / n.max(1) as f64;
    Point::new(cen.x + r * frac * a.cos(), cen.y + r * frac * a.sin())
}

pub fn draw(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, area: Rect, base: f64, text_col: Rgba) {
    let n = c.categories.len().max(c.series.iter().map(|s| s.values.len()).max().unwrap_or(0));
    if n < 3 {
        // Fewer than three spokes make no polygon; PowerPoint draws a line, we draw nothing.
        return;
    }
    let grid_col = Rgba::rgb(0xD9, 0xD9, 0xD9);
    // Room for the category labels around the outside.
    let r = (area.width().min(area.height()) / 2.0 - base * 1.9).max(base);
    let cen = area.center();
    let (min, max, step) = c.axis_scale(false, 5).unwrap_or((0.0, 1.0, 0.2));
    let span = crate::chart::positive_span(max - min);
    let ticks = if step > 0.0 { (((max - min) / step).round() as i64).clamp(1, 20) } else { 1 };
    ctx.set_transform(m);
    ctx.set_stroke(kurbo::Stroke::new(0.75));
    ctx.set_paint(color(grid_col, 1.0));
    if c.gridlines {
        for k in 1..=ticks {
            let frac = k as f64 / ticks as f64;
            let mut p = BezPath::new();
            for i in 0..n {
                let q = spoke(cen, r, i, n, frac);
                if i == 0 { p.move_to(q) } else { p.line_to(q) }
            }
            p.close_path();
            ctx.stroke_path(&p);
        }
    }
    for i in 0..n {
        let mut p = BezPath::new();
        p.move_to(cen);
        p.line_to(spoke(cen, r, i, n, 1.0));
        ctx.set_transform(m);
        ctx.set_paint(color(grid_col, 1.0));
        ctx.stroke_path(&p);
        if let Some(name) = c.categories.get(i) {
            let q = spoke(cen, r + base * 1.3, i, n, 1.0);
            let w = name.chars().count() as f64 * base * 0.55 + base;
            let align = if (q.x - cen.x).abs() < base {
                Align::Center
            } else if q.x > cen.x {
                Align::Left
            } else {
                Align::Right
            };
            let x0 = match align {
                Align::Left => q.x - base * 0.3,
                Align::Right => q.x - w + base * 0.3,
                _ => q.x - w / 2.0,
            };
            label(
                ctx,
                rctx,
                m,
                name,
                Rect::new(x0, q.y - base * 0.7, x0 + w, q.y + base * 0.7),
                crate::chart::LabelStyle { size: base, align, anchor: Anchor::Middle, col: text_col, bold: false },
            );
        }
    }
    // Value labels up the first spoke.
    if !c.y_hidden {
        for k in 0..=ticks {
            let v = min + step * k as f64;
            let q = spoke(cen, r, 0, n, k as f64 / ticks as f64);
            let t = c.axis_label(false, v);
            let w = t.chars().count() as f64 * base * 0.55 + base * 0.4;
            label(
                ctx,
                rctx,
                m,
                &t,
                Rect::new(q.x - w - base * 0.3, q.y - base * 0.6, q.x - base * 0.3, q.y + base * 0.6),
                crate::chart::LabelStyle { size: base * 0.9, align: Align::Right, anchor: Anchor::Middle, col: text_col, bold: false },
            );
        }
    }
    let filled = c.kind == ChartType::FilledRadar;
    for (si, s) in c.series.iter().enumerate() {
        let pts: Vec<Point> = (0..n)
            .map(|i| {
                let v = s.values.get(i).copied().flatten().unwrap_or(min);
                spoke(cen, r, i, n, ((v - min) / span).clamp(0.0, 1.2))
            })
            .collect();
        let mut p = BezPath::new();
        for (i, q) in pts.iter().enumerate() {
            if i == 0 { p.move_to(*q) } else { p.line_to(*q) }
        }
        p.close_path();
        ctx.set_transform(m);
        if filled {
            ctx.set_paint(color(series_fill(rctx, c, si, Some(s)), 0.75));
            ctx.fill_path(&p);
        } else {
            let col = series_line(rctx, c, si, s);
            let lw = s.line.as_ref().and_then(|l| l.width).filter(|w| w.is_finite() && *w > 0.0).unwrap_or(2.25).clamp(0.25, 20.0);
            ctx.set_paint(color(col, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(lw).with_join(kurbo::Join::Round));
            ctx.stroke_path(&p);
            if s.marker.is_some() {
                for q in &pts {
                    ctx.fill_path(&kurbo::Circle::new(*q, 3.5).to_path(0.1));
                }
            }
        }
        if c.series_data_labels(s) {
            for (i, q) in pts.iter().enumerate() {
                if let Some(v) = s.values.get(i).copied().flatten() {
                    let t = c.data_label(s, v);
                    label(
                        ctx,
                        rctx,
                        m,
                        &t,
                        Rect::new(q.x - 40.0, q.y - base * 1.4, q.x + 40.0, q.y - base * 0.2),
                        crate::chart::LabelStyle { size: base * 0.9, align: Align::Center, anchor: Anchor::Bottom, col: text_col, bold: false },
                    );
                }
            }
        }
    }
}

/// A stock chart's marks at one category: the high-low line, then the up/down bar between the
/// first (open) and last (close) series, else a dash at the close.
#[allow(clippy::too_many_arguments)]
pub fn stock_marks(ctx: &mut RenderContext, rctx: &Ctx, c: &Chart, m: Affine, x: f64, band: f64, i: usize, to_y: &dyn Fn(f64) -> f64) {
    let vals: Vec<f64> = c.series.iter().filter_map(|s| s.values.get(i).copied().flatten()).collect();
    if vals.len() < 2 {
        return;
    }
    let hi = vals.iter().copied().fold(f64::MIN, f64::max);
    let lo = vals.iter().copied().fold(f64::MAX, f64::min);
    let ink = Rgba::rgb(0x40, 0x40, 0x40);
    ctx.set_transform(m);
    ctx.set_paint(color(ink, 1.0));
    ctx.set_stroke(kurbo::Stroke::new(1.0));
    let mut p = BezPath::new();
    p.move_to((x, to_y(hi)));
    p.line_to((x, to_y(lo)));
    ctx.stroke_path(&p);
    let open = c.series.first().and_then(|s| s.values.get(i).copied().flatten());
    let close = c.series.last().and_then(|s| s.values.get(i).copied().flatten());
    if c.up_down_bars && c.series.len() >= 4 {
        let (Some(o), Some(cl)) = (open, close) else { return };
        let w = band * 0.4;
        let (y0, y1) = (to_y(o.max(cl)), to_y(o.min(cl)));
        let r = Rect::new(x - w / 2.0, y0, x + w / 2.0, y1.max(y0 + 1.0));
        // PowerPoint's default: white when the price rose, dark when it fell.
        ctx.set_paint(color(if cl >= o { Rgba::WHITE } else { ink }, 1.0));
        ctx.fill_rect(&r);
        ctx.set_paint(color(ink, 1.0));
        ctx.stroke_path(&r.to_path(0.1));
        return;
    }
    if let Some(cl) = close {
        let col = c.series.last().map(|s| series_line(rctx, c, c.series.len() - 1, s)).unwrap_or(ink);
        let y = to_y(cl);
        let mut d = BezPath::new();
        d.move_to((x - band * 0.12, y));
        d.line_to((x + band * 0.12, y));
        ctx.set_paint(color(col, 1.0));
        ctx.set_stroke(kurbo::Stroke::new(2.5));
        ctx.stroke_path(&d);
    }
}
