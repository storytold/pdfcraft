//! Value-axis scaling ("nice numbers") and label formatting.

use gridcraft_core::{DateSystem, Value, number_to_text};
use gridcraft_numfmt::{NumberFormat, format_value};

/// Largest magnitude we plot; bigger values are clamped so ranges stay finite.
pub(crate) const MAX_ABS: f64 = 1e300;

/// A linear axis scale: `min..=max` with gridlines every `step`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl Scale {
    /// Tick values from `min` to `max` (at most 51).
    pub fn ticks(&self) -> Vec<f64> {
        let span = self.max - self.min;
        if !(span.is_finite() && self.step.is_finite() && self.step > 0.0 && span > 0.0) {
            return vec![self.min];
        }
        let n = ((span / self.step).round() as i64).clamp(1, 50);
        (0..=n)
            .map(|i| {
                let v = self.min + i as f64 * self.step;
                // Snap away accumulated float error, but only onto a multiple of the step that is
                // really that close: a `min` off the step grid must keep its own ticks.
                let snapped = (v / self.step).round() * self.step;
                let v = if (snapped - v).abs() < self.step * 1e-9 { snapped } else { v };
                if v.abs() < self.step * 1e-9 { 0.0 } else { v }
            })
            .collect()
    }
    /// Position of `v` along the axis, 0 at `min`, 1 at `max` (clamped to a sane band).
    pub fn frac(&self, v: f64) -> f32 {
        let span = self.max - self.min;
        if !(span.is_finite() && span > 0.0 && v.is_finite()) {
            return 0.0;
        }
        (((v - self.min) / span).clamp(-0.5, 1.5)) as f32
    }
}

pub(crate) fn clean(v: f64) -> f64 {
    if v.is_finite() { v.clamp(-MAX_ABS, MAX_ABS) } else { 0.0 }
}

/// Picks a "nice" scale (steps of 1, 2 or 5 × 10ⁿ, 4–9 gridlines) covering `lo..=hi`.
///
/// `force_zero` always includes 0 (columns, bars, areas). Otherwise 0 is still included when the
/// data sits reasonably close to it (min below 5/6 of max), like common spreadsheet behaviour.
/// A little headroom is added past the data so the extreme value doesn't touch the frame.
pub fn nice_scale(lo: f64, hi: f64, force_zero: bool) -> Scale {
    let (mut lo, mut hi) = (clean(lo), clean(hi));
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    if force_zero || (lo > 0.0 && lo < hi * 5.0 / 6.0) || lo == hi {
        lo = lo.min(0.0);
    }
    if force_zero || (hi < 0.0 && hi > lo * 5.0 / 6.0) || lo == hi {
        hi = hi.max(0.0);
    }
    if lo == hi {
        // Both zero.
        return Scale { min: 0.0, max: 1.0, step: 0.2 };
    }
    let range = hi - lo;
    if hi > 0.0 {
        hi += range * 0.05;
    }
    if lo < 0.0 {
        lo -= range * 0.05;
    }
    let range = hi - lo;
    if !range.is_finite() || range <= 0.0 {
        return Scale { min: 0.0, max: 1.0, step: 0.2 };
    }
    let e = (range / 8.0).log10().floor();
    let mut chosen = None;
    'outer: for k in [e - 1.0, e, e + 1.0, e + 2.0] {
        let p = 10f64.powf(k);
        for m in [1.0, 2.0, 5.0] {
            let s = m * p;
            if !(s.is_finite() && s > 0.0) {
                continue;
            }
            let intervals = (hi / s - 1e-9).ceil() - (lo / s + 1e-9).floor();
            if intervals <= 8.0 {
                chosen = Some(s);
                break 'outer;
            }
        }
    }
    let step = chosen.unwrap_or(range);
    let mut min = (lo / step + 1e-9).floor() * step;
    let mut max = (hi / step - 1e-9).ceil() * step;
    if min.abs() < step * 1e-9 {
        min = 0.0;
    }
    if max.abs() < step * 1e-9 {
        max = 0.0;
    }
    if !(min.is_finite() && max.is_finite()) || max <= min {
        return Scale { min: lo, max: hi, step: range };
    }
    Scale { min, max, step }
}

/// A number format prepared once and reused for many labels.
pub(crate) struct Fmt {
    general: bool,
    fmt: NumberFormat,
}

impl Fmt {
    pub fn new(code: &str) -> Fmt {
        let t = code.trim();
        let general = t.is_empty() || t.eq_ignore_ascii_case("general");
        Fmt { general, fmt: NumberFormat::parse_cached(if general { "General" } else { t }) }
    }
    /// Formats `v`; `step` (axis labels) fixes the decimals for General.
    pub fn format(&self, v: f64, step: Option<f64>) -> String {
        let mut v = clean(v);
        if let Some(s) = step
            && s > 0.0
            && (v / s).abs() < 1e-9
        {
            v = 0.0;
        }
        if self.general {
            return general_text(v, step);
        }
        let mut t = format_value(&Value::Number(v), &self.fmt, DateSystem::D1900).text;
        if t.chars().count() > 40 {
            t = t.chars().take(40).collect();
        }
        t
    }
}

fn general_text(v: f64, step: Option<f64>) -> String {
    if v == 0.0 {
        return "0".into();
    }
    match step {
        Some(s) if s.is_finite() && s > 0.0 && v.abs() < 1e11 && v.abs() >= 1e-9 => {
            let d = (-s.log10().floor()).clamp(0.0, 10.0) as usize;
            let t = format!("{:.*}", d, v);
            if t.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { "0".into() } else { t }
        }
        _ => number_to_text(v),
    }
}

/// Formats a number with a format code (General shows up to the precision implied by `step`).
pub fn format_number(v: f64, code: &str, step: Option<f64>) -> String {
    Fmt::new(code).format(v, step)
}
