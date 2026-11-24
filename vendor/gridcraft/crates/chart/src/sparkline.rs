//! In-cell sparklines.

use gridcraft_model::SparklineKind;

use crate::render::{DOWN, finalize};
use crate::resolve::MAX_POINTS;
use crate::{Prim, Rgba};

/// Draws a sparkline into a `w`×`h` box at the origin. Gaps (`None`) break lines; column and
/// win/loss sparklines draw negatives in our "down" red.
pub fn render_sparkline(kind: SparklineKind, values: &[Option<f64>], color: Rgba, markers: bool, w: f32, h: f32) -> Vec<Prim> {
    if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 || w > 1e6 || h > 1e6 {
        return vec![];
    }
    let vals: Vec<Option<f64>> = values.iter().take(MAX_POINTS).map(|v| v.filter(|x| x.is_finite()).map(|x| x.clamp(-1e300, 1e300))).collect();
    let n = vals.len();
    if n == 0 {
        return vec![];
    }
    let pad = (h * 0.1).clamp(0.5, 2.0);
    let (x0, y0, iw, ih) = (pad, pad, (w - 2.0 * pad).max(0.5), (h - 2.0 * pad).max(0.5));
    let mut out = Vec::new();
    let (lo, hi) = vals.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
    if !lo.is_finite() {
        return out;
    }
    match kind {
        SparklineKind::Line => {
            let span = hi - lo;
            let fy = |v: f64| {
                let f = if span > 0.0 { ((v - lo) / span) as f32 } else { 0.5 };
                y0 + ih - f * ih
            };
            let fx = |j: usize| if n > 1 { x0 + j as f32 / (n - 1) as f32 * iw } else { x0 + iw / 2.0 };
            let mut run: Vec<[f32; 2]> = Vec::new();
            let flush = |run: &mut Vec<[f32; 2]>, out: &mut Vec<Prim>| {
                if run.len() >= 2 {
                    let pts = if run.len() > 4000 { run.iter().step_by(run.len() / 2000 + 1).copied().collect() } else { std::mem::take(run) };
                    out.push(Prim::Line { pts, color, width: 1.25, dash: false });
                }
                run.clear();
            };
            for (j, v) in vals.iter().enumerate() {
                match v {
                    Some(v) => run.push([fx(j), fy(*v)]),
                    None => flush(&mut run, &mut out),
                }
            }
            flush(&mut run, &mut out);
            if markers || n == 1 {
                let r = (h * 0.08).clamp(1.0, 3.0);
                let step = (n / 500).max(1);
                for (j, v) in vals.iter().enumerate().step_by(step) {
                    if let Some(v) = v {
                        out.push(Prim::Circle { cx: fx(j), cy: fy(*v), r, fill: Some(color), stroke: None });
                    }
                }
            }
        }
        SparklineKind::Column | SparklineKind::WinLoss => {
            let slot = iw / n as f32;
            let bw = (slot * 0.8).max(0.25);
            let win = kind == SparklineKind::WinLoss;
            let (lo0, hi0) = if win { (-1.0, 1.0) } else { (lo.min(0.0), hi.max(0.0)) };
            let span = hi0 - lo0;
            let fy = |v: f64| if span > 0.0 { y0 + ih - ((v - lo0) / span) as f32 * ih } else { y0 + ih };
            let base = fy(0.0);
            for (j, v) in vals.iter().enumerate() {
                let Some(v) = v else { continue };
                let v = if win { v.signum() * (*v != 0.0) as i32 as f64 } else { *v };
                if v == 0.0 && win {
                    continue;
                }
                let top = fy(v);
                let (y, hh) = (top.min(base), (top - base).abs().max(0.5));
                let fill = if v < 0.0 { DOWN } else { color };
                out.push(Prim::Rect { x: x0 + j as f32 * slot + (slot - bw) / 2.0, y, w: bw, h: hh, fill: Some(fill), stroke: None, radius: 0.0 });
            }
        }
    }
    finalize(out, w, h)
}
