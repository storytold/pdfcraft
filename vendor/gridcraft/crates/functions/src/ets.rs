//! The FORECAST.ETS family: additive-error, additive-trend, additive-seasonality (AAA)
//! exponential smoothing over an evenly spaced timeline.
//!
//! Model (Hyndman, Koehler, Ord & Snyder, "Forecasting with Exponential Smoothing: The State
//! Space Approach", 2008, ETS(A,A,A), component form):
//!
//! ```text
//! ŷ_t = l_{t-1} + b_{t-1} + s_{t-m}        e_t = y_t − ŷ_t
//! l_t = l_{t-1} + b_{t-1} + α·e_t
//! b_t = b_{t-1} + α·β·e_t
//! s_t = s_{t-m} + γ·e_t
//! ```
//!
//! Initial states follow the book's heuristic (§2.6.1): seasonal indices from a classical
//! decomposition (centred moving average) of the first seasons, level and trend from a straight
//! line fitted to the first ten seasonally adjusted values. α, β and γ (each in [0, 1]) minimise
//! the sum of squared one-step errors: a coarse grid, then a Nelder–Mead simplex clamped to the
//! box. Excel does not document its optimiser, so fitted parameters (and the last digits of
//! forecasts on noisy data) can differ from Excel's; exact data (straight lines, exactly
//! repeating seasons) forecast exactly either way.
//!
//! Seasonality detection: the autocorrelation of the linearly detrended series; the highest local
//! peak above the 95% white-noise bound (2/√n) at a lag of at most n/2 (and at most 8,760).
//!
//! Prediction intervals use the model's h-step forecast variance (the book's Table 6.1, class 1):
//! `σ²·(1 + Σ_{j=1}^{h−1} (α + αβ·j + γ·[j mod m = 0])²)`, σ² = SSE / n, with the normal quantile.

use gridcraft_core::date::{datetime_from_serial, days_in_month_in, serial_from_ymd};
use gridcraft_core::{CellError, DateSystem, Value};

use crate::special::norm_inv;
use crate::util::{R, arg, as_array, has, num, num_val};
use crate::{Arg, Ctx, FnSpec};

type C<'a> = &'a mut dyn Ctx;

/// Longest timeline (after completing missing points) the functions accept.
pub(crate) const MAX_POINTS: usize = 200_000;
/// Longest seasonality Excel supports (hours in a year).
pub(crate) const MAX_SEASON: usize = 8_760;
/// Share of the timeline's points that may be missing.
const MAX_MISSING: f64 = 0.3;
/// Seasonally adjusted values the initial level and trend are fitted to.
const INIT_POINTS: usize = 10;
const NM_ITERATIONS: usize = 400;

// ---------------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Season {
    Detect,
    None,
    Fixed(usize),
}

/// Duplicate time stamps are combined with this function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Aggregate {
    Average,
    Count,
    CountA,
    Max,
    Median,
    Min,
    Sum,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub seasonality: Season,
    /// `true`: missing points are interpolated from their neighbours; `false`: they are zero.
    pub interpolate: bool,
    pub aggregate: Aggregate,
}

impl Default for Options {
    fn default() -> Self {
        Options { seasonality: Season::Detect, interpolate: true, aggregate: Aggregate::Average }
    }
}

fn whole(a: &[Arg], i: usize) -> R<Option<f64>> {
    if !has(a, i) {
        return Ok(None);
    }
    let v = num(a, i)?;
    if !v.is_finite() {
        return Err(CellError::Num);
    }
    Ok(Some(v.trunc()))
}

/// `[seasonality]`: 1 (default) detects, 0 means none, 2–8,760 is the season length.
pub(crate) fn season_arg(a: &[Arg], i: usize) -> R<Season> {
    match whole(a, i)? {
        None => Ok(Season::Detect),
        Some(1.0) => Ok(Season::Detect),
        Some(0.0) => Ok(Season::None),
        Some(v) if v >= 2.0 && v <= MAX_SEASON as f64 => Ok(Season::Fixed(v as usize)),
        Some(_) => Err(CellError::Num),
    }
}

/// `[data_completion]`: 1 (default) interpolates, 0 fills zeros.
fn completion_arg(a: &[Arg], i: usize) -> R<bool> {
    match whole(a, i)? {
        None => Ok(true),
        Some(1.0) => Ok(true),
        Some(0.0) => Ok(false),
        Some(_) => Err(CellError::Num),
    }
}

/// `[aggregation]`: 0 or 1 AVERAGE (default), 2 COUNT, 3 COUNTA, 4 MAX, 5 MEDIAN, 6 MIN, 7 SUM.
fn aggregate_arg(a: &[Arg], i: usize) -> R<Aggregate> {
    Ok(match whole(a, i)? {
        None => Aggregate::Average,
        Some(v) => match v as i64 {
            0 | 1 => Aggregate::Average,
            2 => Aggregate::Count,
            3 => Aggregate::CountA,
            4 => Aggregate::Max,
            5 => Aggregate::Median,
            6 => Aggregate::Min,
            7 => Aggregate::Sum,
            _ => return Err(CellError::Num),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------------------------

/// How timeline values map to point positions (0 = first point).
#[derive(Clone, Copy, Debug)]
enum Axis {
    /// A constant numeric step.
    Linear { t0: f64, step: f64 },
    /// Whole-day dates a constant number of calendar months apart, all on the same day of the
    /// month (`day = Some(d)`) or all on the last day of their month (`day = None`).
    Months { sys: DateSystem, month0: i64, step: i64, day: Option<u32> },
}

fn month_parts(sys: DateSystem, t: f64) -> Option<(i64, f64)> {
    let d = datetime_from_serial(sys, t.floor())?;
    if d.day == 0 {
        return None;
    }
    Some((i64::from(d.year) * 12 + i64::from(d.month) - 1, f64::from(d.day)))
}

impl Axis {
    fn position(&self, t: f64) -> Option<f64> {
        match *self {
            Axis::Linear { t0, step } => Some((t - t0) / step),
            Axis::Months { sys, month0, step, day } => {
                let d = datetime_from_serial(sys, t.floor())?;
                if d.day == 0 {
                    return None;
                }
                let month = i64::from(d.year) * 12 + i64::from(d.month) - 1;
                let dim = f64::from(days_in_month_in(sys, d.year, d.month));
                if dim <= 0.0 {
                    return None;
                }
                let anchor = day.map_or(dim, |a| f64::from(a).min(dim));
                let offset = (f64::from(d.day) - anchor + (t - t.floor())) / dim;
                Some(((month - month0) as f64 + offset) / step as f64)
            }
        }
    }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.abs()
}

/// The axis of sorted, distinct time stamps: a constant step (missing points allowed) or, for
/// dates, a constant number of calendar months.
fn detect_axis(times: &[f64], sys: DateSystem) -> R<Axis> {
    let (Some(&t0), Some(&t1)) = (times.first(), times.get(1)) else { return Err(CellError::Num) };
    let step = times.windows(2).map(|w| w.get(1).copied().unwrap_or(t1) - w.first().copied().unwrap_or(t0)).fold(f64::INFINITY, f64::min);
    if step.is_finite() && step > 0.0 {
        let even = times.windows(2).all(|w| match (w.first(), w.get(1)) {
            (Some(a), Some(b)) => {
                let r = (b - a) / step;
                (r - r.round()).abs() <= 1e-6
            }
            _ => true,
        });
        if even {
            return Ok(Axis::Linear { t0, step });
        }
    }
    // Calendar months: whole days on one day of the month (or all month ends).
    let mut months = Vec::with_capacity(times.len());
    let mut same_day: Option<u32> = None;
    let mut all_same = true;
    let mut all_end = true;
    for &t in times {
        if t.fract() != 0.0 {
            return Err(CellError::Num);
        }
        let d = datetime_from_serial(sys, t).ok_or(CellError::Num)?;
        if d.day == 0 {
            return Err(CellError::Num);
        }
        months.push(i64::from(d.year) * 12 + i64::from(d.month) - 1);
        match same_day {
            None => same_day = Some(d.day),
            Some(x) if x != d.day => all_same = false,
            _ => {}
        }
        if d.day != days_in_month_in(sys, d.year, d.month) {
            all_end = false;
        }
    }
    let day = if all_same {
        same_day
    } else if all_end {
        None
    } else {
        return Err(CellError::Num);
    };
    let mut g = 0i64;
    for w in months.windows(2) {
        if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
            let d = b - a;
            if d <= 0 {
                return Err(CellError::Num);
            }
            g = gcd(g, d);
        }
    }
    let month0 = month_parts(sys, t0).ok_or(CellError::Num)?.0;
    if g <= 0 {
        return Err(CellError::Num);
    }
    Ok(Axis::Months { sys, month0, step: g, day })
}

/// An evenly spaced series built from a values/timeline pair.
#[derive(Clone, Debug)]
pub(crate) struct Series {
    pub y: Vec<f64>,
    axis: Axis,
    /// The timeline's step in timeline units (the average step for calendar months).
    pub step: f64,
}

impl Series {
    /// Position of a time stamp relative to the last point (0 = last point, 1 = next step).
    pub fn horizon(&self, t: f64) -> R<f64> {
        let p = self.axis.position(t).ok_or(CellError::Num)?;
        let h = p - (self.y.len() as f64 - 1.0);
        if !h.is_finite() || h < -1e-9 {
            return Err(CellError::Num);
        }
        Ok(h.max(0.0))
    }
}

fn aggregate(kind: Aggregate, xs: &mut [f64]) -> Option<f64> {
    let n = xs.len() as f64;
    match kind {
        Aggregate::Count | Aggregate::CountA => Some(n),
        _ if xs.is_empty() => None,
        Aggregate::Average => Some(xs.iter().sum::<f64>() / n),
        Aggregate::Sum => Some(xs.iter().sum()),
        Aggregate::Max => xs.iter().copied().reduce(f64::max),
        Aggregate::Min => xs.iter().copied().reduce(f64::min),
        Aggregate::Median => {
            xs.sort_by(f64::total_cmp);
            let k = xs.len() / 2;
            let hi = xs.get(k).copied()?;
            if xs.len() % 2 == 1 { Some(hi) } else { Some((xs.get(k.checked_sub(1)?).copied()? + hi) / 2.0) }
        }
    }
}

/// Builds the series: pairs values with time stamps, sorts, aggregates duplicates, checks the
/// step and completes missing points.
pub(crate) fn series(values: &Value, timeline: &Value, opts: &Options, sys: DateSystem) -> R<Series> {
    let va = as_array(values);
    let ta = as_array(timeline);
    if va.data.len() != ta.data.len() {
        return Err(CellError::NA);
    }
    if va.data.len() > MAX_POINTS {
        return Err(CellError::Num);
    }
    let mut points: Vec<(f64, Option<f64>)> = Vec::with_capacity(va.data.len());
    for (t, v) in ta.data.iter().zip(va.data.iter()) {
        if let Value::Error(e) = t {
            return Err(*e);
        }
        if let Value::Error(e) = v {
            return Err(*e);
        }
        let value = match v {
            Value::Number(x) => Some(*x),
            Value::Empty => None,
            _ => return Err(CellError::Value),
        };
        let time = match t {
            Value::Number(x) if x.is_finite() => *x,
            // A blank row (no time stamp, no value) is ignored.
            Value::Empty if value.is_none() => continue,
            _ => return Err(CellError::Value),
        };
        points.push((time, value));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut times: Vec<f64> = points.iter().map(|p| p.0).collect();
    times.dedup();
    if times.len() < 2 {
        return Err(CellError::Num);
    }
    let axis = detect_axis(&times, sys)?;
    let (Some(&first), Some(&last)) = (times.first(), times.last()) else { return Err(CellError::Num) };
    let span = axis.position(last).ok_or(CellError::Num)?.round();
    if !(span >= 1.0 && span < MAX_POINTS as f64) {
        return Err(CellError::Num);
    }
    let len = span as usize + 1;
    // Group the (sorted) points by position.
    let mut slots: Vec<Option<f64>> = vec![None; len];
    let mut group: Vec<f64> = Vec::new();
    let mut group_at: Option<usize> = None;
    let flush = |at: Option<usize>, group: &mut Vec<f64>, slots: &mut Vec<Option<f64>>| {
        if let Some(i) = at
            && let Some(s) = slots.get_mut(i)
        {
            *s = aggregate(opts.aggregate, group);
        }
        group.clear();
    };
    for &(t, v) in &points {
        let p = axis.position(t).ok_or(CellError::Num)?.round();
        if !(p >= 0.0 && p < len as f64) {
            return Err(CellError::Num);
        }
        let p = p as usize;
        if group_at != Some(p) {
            flush(group_at, &mut group, &mut slots);
            group_at = Some(p);
        }
        if let Some(x) = v {
            group.push(x);
        }
    }
    flush(group_at, &mut group, &mut slots);
    let known = slots.iter().filter(|s| s.is_some()).count();
    if known == 0 || (len - known) as f64 > MAX_MISSING * len as f64 {
        return Err(CellError::Num);
    }
    let y = if opts.interpolate { interpolate(&slots) } else { slots.iter().map(|s| s.unwrap_or(0.0)).collect() };
    let step = match axis {
        Axis::Linear { step, .. } => step,
        Axis::Months { .. } => (last - first) / span,
    };
    Ok(Series { y, axis, step })
}

/// Missing points become the straight line between their known neighbours (the nearest known
/// value at either end).
fn interpolate(slots: &[Option<f64>]) -> Vec<f64> {
    let mut out = Vec::with_capacity(slots.len());
    // Keep the next known point until it is reached, scanning each slot only once for lookahead.
    let mut known = slots.iter().enumerate().filter_map(|(j, s)| {
        #[cfg(test)]
        tests::INTERPOLATION_VISITS.with(|visits| visits.set(visits.get() + 1));
        s.map(|v| (j, v))
    });
    let mut next = known.next();
    let mut prev: Option<(usize, f64)> = None;
    for (i, s) in slots.iter().enumerate() {
        if let Some(v) = s {
            out.push(*v);
            prev = Some((i, *v));
            next = known.next();
            continue;
        }
        out.push(match (prev, next) {
            (Some((a, va)), Some((b, vb))) => va + (vb - va) * (i - a) as f64 / (b - a) as f64,
            (Some((_, v)), None) | (None, Some((_, v))) => v,
            (None, None) => 0.0,
        });
    }
    out
}

/// A timeline's step, for continuing it past its last point (the Forecast Sheet's future rows).
#[derive(Clone, Copy, Debug)]
pub struct Timeline {
    axis: Axis,
    /// Position of the last time stamp.
    last: i64,
}

impl Timeline {
    /// The step of a set of time stamps (any order, duplicates allowed): a constant numeric step
    /// (missing points allowed) or, for whole-day dates on one day of the month, calendar months.
    /// `None` without at least two distinct, finite time stamps or without a constant step.
    pub fn detect(times: &[f64], sys: DateSystem) -> Option<Timeline> {
        if times.iter().any(|t| !t.is_finite()) {
            return None;
        }
        let mut sorted = times.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted.dedup();
        let axis = detect_axis(&sorted, sys).ok()?;
        let last = axis.position(*sorted.last()?)?.round();
        // Like the functions, at most 30% of the points may be missing.
        if !(0.0..MAX_POINTS as f64).contains(&last) || (last + 1.0 - sorted.len() as f64) > MAX_MISSING * (last + 1.0) {
            return None;
        }
        Some(Timeline { axis, last: last as i64 })
    }

    /// Steps from the last time stamp to `t` (fractional between steps, negative before it).
    pub fn steps_after_last(&self, t: f64) -> Option<f64> {
        let p = self.axis.position(t)? - self.last as f64;
        p.is_finite().then_some(p)
    }

    /// The time stamp `k` steps after the last one.
    pub fn after_last(&self, k: u32) -> Option<f64> {
        let pos = self.last.checked_add(i64::from(k))?;
        match self.axis {
            Axis::Linear { t0, step } => {
                let t = t0 + pos as f64 * step;
                t.is_finite().then_some(t)
            }
            Axis::Months { sys, month0, step, day } => {
                let month = month0.checked_add(pos.checked_mul(step)?)?;
                let (y, m) = (month.div_euclid(12), month.rem_euclid(12) + 1);
                let dim = i64::from(days_in_month_in(sys, i32::try_from(y).ok()?, u32::try_from(m).ok()?));
                let d = day.map_or(dim, |d| i64::from(d).min(dim));
                if y < 1900 {
                    return None;
                }
                serial_from_ymd(sys, y, m, d)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Seasonality detection
// ---------------------------------------------------------------------------------------------

/// Residuals of the least-squares line through `y` (x = 0, 1, 2…).
fn detrend(y: &[f64]) -> Vec<f64> {
    let (a, b) = line(y.iter().enumerate().map(|(i, v)| (i as f64, *v)));
    y.iter().enumerate().map(|(i, v)| v - (a + b * i as f64)).collect()
}

/// (intercept, slope) of the least-squares line through the points (slope 0 for one point).
fn line(points: impl Iterator<Item = (f64, f64)> + Clone) -> (f64, f64) {
    let (mut n, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for (x, y) in points.clone() {
        n += 1.0;
        sx += x;
        sy += y;
    }
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let (mx, my) = (sx / n, sy / n);
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (x, y) in points {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
    }
    let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    (my - b * mx, b)
}

/// The detected season length, 0 when the series shows none.
pub(crate) fn detect_season(y: &[f64]) -> usize {
    let n = y.len();
    if n < 4 {
        return 0;
    }
    let r = detrend(y);
    let var: f64 = r.iter().map(|v| v * v).sum();
    let scale: f64 = y.iter().map(|v| v * v).sum::<f64>();
    if !(var.is_finite() && var > 1e-20 * scale.max(f64::MIN_POSITIVE)) {
        return 0;
    }
    let max_lag = (n / 2).min(MAX_SEASON);
    let acf: Vec<f64> = (0..=max_lag.saturating_add(1).min(n.saturating_sub(1)))
        .map(|k| r.iter().skip(k).zip(r.iter()).map(|(a, b)| a * b).sum::<f64>() / var)
        .collect();
    let bound = 2.0 / (n as f64).sqrt();
    let mut best = 0usize;
    let mut best_r = bound;
    for k in 2..=max_lag {
        let (Some(&prev), Some(&here)) = (acf.get(k - 1), acf.get(k)) else { break };
        let next = acf.get(k + 1).copied().unwrap_or(f64::NEG_INFINITY);
        if here > prev && here >= next && here > best_r {
            best = k;
            best_r = here;
        }
    }
    best
}

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Init {
    level: f64,
    trend: f64,
    /// Seasonal state for positions 0..m (empty without seasonality).
    season: Vec<f64>,
}

/// Initial states (Hyndman et al. 2008, §2.6.1).
fn initial_states(y: &[f64], m: usize) -> Init {
    let n = y.len();
    let mut season = vec![0.0; m];
    if m >= 2 {
        if n >= 2 * m {
            // Classical decomposition of the first seasons: centred moving average as the trend.
            let len = n.min(4 * m).max(2 * m);
            let w = y.get(..len).unwrap_or(y);
            let mut prefix = Vec::with_capacity(w.len() + 1);
            prefix.push(0.0);
            let mut acc = 0.0;
            for v in w {
                acc += v;
                prefix.push(acc);
            }
            let sum = |a: usize, b: usize| prefix.get(b).copied().unwrap_or(0.0) - prefix.get(a).copied().unwrap_or(0.0);
            let h = m / 2;
            let mut totals = vec![0.0; m];
            let mut counts = vec![0usize; m];
            for t in h..w.len().saturating_sub(h) {
                let ma = if m % 2 == 1 {
                    sum(t - h, t + h + 1) / m as f64
                } else {
                    let ends = w.get(t - h).copied().unwrap_or(0.0) + w.get(t + h).copied().unwrap_or(0.0);
                    (sum(t - h + 1, t + h) + ends / 2.0) / m as f64
                };
                if let (Some(tot), Some(c), Some(v)) = (totals.get_mut(t % m), counts.get_mut(t % m), w.get(t)) {
                    *tot += v - ma;
                    *c += 1;
                }
            }
            for ((s, tot), c) in season.iter_mut().zip(&totals).zip(&counts) {
                *s = if *c > 0 { tot / *c as f64 } else { 0.0 };
            }
        } else {
            // Under two seasons: deviations from a straight line through the data.
            let r = detrend(y);
            let mut counts = vec![0usize; m];
            for (t, v) in r.iter().enumerate() {
                if let (Some(s), Some(c)) = (season.get_mut(t % m), counts.get_mut(t % m)) {
                    *s += v;
                    *c += 1;
                }
            }
            for (s, c) in season.iter_mut().zip(&counts) {
                if *c > 0 {
                    *s /= *c as f64;
                }
            }
        }
        let mean = season.iter().sum::<f64>() / m as f64;
        for s in &mut season {
            *s -= mean;
        }
    }
    let k = n.min(INIT_POINTS);
    let adjusted = y.iter().take(k).enumerate().map(|(t, v)| {
        let s = if m >= 2 { season.get(t % m).copied().unwrap_or(0.0) } else { 0.0 };
        ((t + 1) as f64, v - s)
    });
    let (level, trend) = line(adjusted);
    Init { level, trend, season }
}

/// Smoothing parameters (α, β, γ).
type Params = [f64; 3];

#[derive(Clone, Debug)]
struct State {
    level: f64,
    trend: f64,
    season: Vec<f64>,
    sse: f64,
}

/// Runs the filter over the data; `errors` collects the one-step errors when given.
fn filter(y: &[f64], init: &Init, p: Params, mut errors: Option<&mut Vec<f64>>) -> State {
    let [alpha, beta, gamma] = p;
    let m = init.season.len();
    let (mut l, mut b) = (init.level, init.trend);
    let mut season = init.season.clone();
    let mut sse = 0.0;
    for (t, v) in y.iter().enumerate() {
        let slot = if m > 0 { season.get_mut(t % m) } else { None };
        let s = slot.as_ref().map_or(0.0, |s| **s);
        let e = v - (l + b + s);
        if let Some(slot) = slot {
            *slot = s + gamma * e;
        }
        l = l + b + alpha * e;
        b += alpha * beta * e;
        sse += e * e;
        if let Some(errs) = errors.as_deref_mut() {
            errs.push(e);
        }
    }
    State { level: l, trend: b, season, sse }
}

fn objective(y: &[f64], init: &Init, p: Params) -> f64 {
    let sse = filter(y, init, p, None).sse;
    if sse.is_finite() { sse } else { f64::INFINITY }
}

fn clamp(p: Params, active: usize) -> Params {
    let mut q = [0.0; 3];
    for (i, (o, v)) in q.iter_mut().zip(p).enumerate() {
        *o = if i < active { v.clamp(0.0, 1.0) } else { 0.0 };
    }
    q
}

/// Minimises the SSE over the active parameters (α, β and, with seasonality, γ) in [0, 1].
fn optimise(y: &[f64], init: &Init) -> Params {
    let active = if init.season.is_empty() { 2 } else { 3 };
    let f = |p: Params| objective(y, init, clamp(p, active));
    // Coarse grid.
    const GRID: [f64; 5] = [0.05, 0.25, 0.5, 0.75, 0.95];
    let mut best: (Params, f64) = ([0.5, 0.1, 0.1], f64::INFINITY);
    for a in GRID {
        for b in GRID {
            for g in if active == 3 { &GRID[..] } else { &GRID[..1] } {
                let p = clamp([a, b, *g], active);
                let v = f(p);
                if v < best.1 {
                    best = (p, v);
                }
            }
        }
    }
    // Nelder–Mead from the best grid point, clamped to the box.
    let mut simplex: Vec<(Params, f64)> = vec![best];
    for i in 0..active {
        let mut p = best.0;
        if let Some(x) = p.get_mut(i) {
            *x = if *x > 0.5 { *x - 0.1 } else { *x + 0.1 };
        }
        let p = clamp(p, active);
        simplex.push((p, f(p)));
    }
    let lerp = |a: Params, b: Params, t: f64| -> Params {
        let mut o = [0.0; 3];
        for ((o, x), y) in o.iter_mut().zip(a).zip(b) {
            *o = x + t * (y - x);
        }
        clamp(o, active)
    };
    for _ in 0..NM_ITERATIONS {
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (Some(&lo), Some(&hi)) = (simplex.first(), simplex.last()) else { break };
        let second = simplex.len().checked_sub(2).and_then(|i| simplex.get(i)).map_or(hi.1, |s| s.1);
        let size = simplex.iter().map(|s| s.0.iter().zip(lo.0).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max)).fold(0.0, f64::max);
        if (hi.1 - lo.1).abs() <= 1e-12 * lo.1.abs() + 1e-300 || size < 1e-7 {
            break;
        }
        let k = simplex.len().saturating_sub(1).max(1) as f64;
        let mut c = [0.0; 3];
        for s in simplex.iter().take(simplex.len().saturating_sub(1)) {
            for (ci, x) in c.iter_mut().zip(s.0) {
                *ci += x / k;
            }
        }
        let reflected = lerp(c, hi.0, -1.0);
        let fr = f(reflected);
        let replace = if fr < lo.1 {
            let expanded = lerp(c, hi.0, -2.0);
            let fe = f(expanded);
            Some(if fe < fr { (expanded, fe) } else { (reflected, fr) })
        } else if fr < second {
            Some((reflected, fr))
        } else {
            let contracted = lerp(c, hi.0, 0.5);
            let fc = f(contracted);
            if fc < hi.1 { Some((contracted, fc)) } else { None }
        };
        match replace {
            Some(v) => {
                if let Some(last) = simplex.last_mut() {
                    *last = v;
                }
            }
            None => {
                for s in simplex.iter_mut().skip(1) {
                    let p = lerp(lo.0, s.0, 0.5);
                    *s = (p, f(p));
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    clamp(simplex.first().map_or(best.0, |s| s.0), active)
}

/// A fitted model.
#[derive(Clone, Debug)]
pub(crate) struct Fit {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    pub m: usize,
    n: usize,
    state: State,
    pub errors: Vec<f64>,
}

pub(crate) fn fit(y: &[f64], m: usize) -> Fit {
    let m = if m >= 2 { m } else { 0 };
    let init = initial_states(y, m);
    let p = optimise(y, &init);
    let mut errors = Vec::with_capacity(y.len());
    let state = filter(y, &init, p, Some(&mut errors));
    let [alpha, beta, gamma] = p;
    Fit { alpha, beta, gamma, m, n: y.len(), state, errors }
}

impl Fit {
    fn at(&self, k: f64) -> f64 {
        let s = if self.m > 0 {
            let idx = ((self.n as f64 - 1.0) + k).rem_euclid(self.m as f64);
            self.state.season.get(idx as usize).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        self.state.level + k * self.state.trend + s
    }

    /// Point forecast `h` steps after the last point (fractional steps interpolate).
    pub fn forecast(&self, h: f64) -> f64 {
        let lo = h.floor();
        let a = self.at(lo);
        if h == lo { a } else { a + (h - lo) * (self.at(lo + 1.0) - a) }
    }

    /// Standard deviation of the forecast error `h` steps ahead.
    pub fn forecast_sd(&self, h: f64) -> f64 {
        let sigma2 = self.state.sse / self.n.max(1) as f64;
        let k = (h.ceil().max(1.0)) - 1.0;
        let (a, b, g) = (self.alpha, self.alpha * self.beta, self.gamma);
        // Σ_{j=1}^{k} (a + b·j)² + seasonal terms, in closed form.
        let mut sum = k * a * a + a * b * k * (k + 1.0) + b * b * k * (k + 1.0) * (2.0 * k + 1.0) / 6.0;
        if self.m > 0 {
            let m = self.m as f64;
            let q = (k / m).floor();
            sum += 2.0 * g * (q * a + b * m * q * (q + 1.0) / 2.0) + g * g * q;
        }
        (sigma2 * (1.0 + sum)).max(0.0).sqrt()
    }
}

// ---------------------------------------------------------------------------------------------
// Worksheet functions
// ---------------------------------------------------------------------------------------------

struct Prepared {
    series: Series,
    m: usize,
}

fn prepare(a: &[Arg], c: C, values: usize, timeline: usize, opts: Options) -> R<Prepared> {
    let series = series(&arg(a, values)?.value, &arg(a, timeline)?.value, &opts, c.date_system())?;
    let m = match opts.seasonality {
        Season::Detect => detect_season(&series.y),
        Season::None => 0,
        Season::Fixed(m) => m,
    };
    Ok(Prepared { series, m })
}

fn options(a: &[Arg], season: Option<usize>, completion: usize, aggregation: usize) -> R<Options> {
    Ok(Options {
        seasonality: match season {
            Some(i) => season_arg(a, i)?,
            None => Season::Detect,
        },
        interpolate: completion_arg(a, completion)?,
        aggregate: aggregate_arg(a, aggregation)?,
    })
}

fn forecast_ets(a: &[Arg], c: C) -> R<Value> {
    let target = num(a, 0)?;
    let opts = options(a, Some(3), 4, 5)?;
    let p = prepare(a, c, 1, 2, opts)?;
    let h = p.series.horizon(target)?;
    num_val(fit(&p.series.y, p.m).forecast(h))
}

fn forecast_ets_confint(a: &[Arg], c: C) -> R<Value> {
    let target = num(a, 0)?;
    let level = if has(a, 3) { num(a, 3)? } else { 0.95 };
    if !(level > 0.0 && level < 1.0) {
        return Err(CellError::Num);
    }
    let opts = options(a, Some(4), 5, 6)?;
    let p = prepare(a, c, 1, 2, opts)?;
    let h = p.series.horizon(target)?;
    let z = norm_inv((1.0 + level) / 2.0);
    num_val(z * fit(&p.series.y, p.m).forecast_sd(h))
}

fn forecast_ets_seasonality(a: &[Arg], c: C) -> R<Value> {
    let opts = options(a, None, 2, 3)?;
    let p = prepare(a, c, 0, 1, opts)?;
    num_val(p.m as f64)
}

fn forecast_ets_stat(a: &[Arg], c: C) -> R<Value> {
    let kind = num(a, 2)?;
    if !kind.is_finite() || !(1.0..9.0).contains(&kind.trunc()) {
        return Err(CellError::Num);
    }
    let kind = kind.trunc() as u8;
    let opts = options(a, Some(3), 4, 5)?;
    let p = prepare(a, c, 0, 1, opts)?;
    if kind == 8 {
        return num_val(p.series.step);
    }
    let y = &p.series.y;
    let f = fit(y, p.m);
    let n = f.errors.len().max(1) as f64;
    let mae = f.errors.iter().map(|e| e.abs()).sum::<f64>() / n;
    num_val(match kind {
        1 => f.alpha,
        2 => f.beta,
        3 => f.gamma,
        4 => {
            let naive: Vec<f64> = y.windows(2).filter_map(|w| Some((w.get(1)? - w.first()?).abs())).collect();
            let d = naive.iter().sum::<f64>() / naive.len().max(1) as f64;
            if d == 0.0 {
                return Err(CellError::Div0);
            }
            mae / d
        }
        5 => {
            let terms: Vec<f64> = y
                .iter()
                .zip(&f.errors)
                .filter_map(|(v, e)| {
                    let denom = (v.abs() + (v - e).abs()) / 2.0;
                    (denom > 0.0).then(|| e.abs() / denom)
                })
                .collect();
            terms.iter().sum::<f64>() / terms.len().max(1) as f64
        }
        6 => mae,
        7 => (f.errors.iter().map(|e| e * e).sum::<f64>() / n).sqrt(),
        _ => return Err(CellError::Num),
    })
}

const S_ETS: &[bool] = &[true, false, false, true, true, true, true];
const S_ETS_SEASON: &[bool] = &[false, false, true, true];
const S_ETS_STAT: &[bool] = &[false, false, true, true, true, true];

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!(
            "FORECAST.ETS",
            3,
            6,
            Statistical,
            S_ETS,
            "FORECAST.ETS(target_date, values, timeline, [seasonality], [data_completion], [aggregation])",
            "Forecasts a value by additive (AAA) exponential smoothing.",
            forecast_ets
        ),
        f!(
            "FORECAST.ETS.CONFINT",
            3,
            7,
            Statistical,
            S_ETS,
            "FORECAST.ETS.CONFINT(target_date, values, timeline, [confidence_level], [seasonality], [data_completion], [aggregation])",
            "Half-width of the confidence interval around a FORECAST.ETS value.",
            forecast_ets_confint
        ),
        f!(
            "FORECAST.ETS.SEASONALITY",
            2,
            4,
            Statistical,
            S_ETS_SEASON,
            "FORECAST.ETS.SEASONALITY(values, timeline, [data_completion], [aggregation])",
            "Length of the repeating pattern detected in a time series.",
            forecast_ets_seasonality
        ),
        f!(
            "FORECAST.ETS.STAT",
            3,
            6,
            Statistical,
            S_ETS_STAT,
            "FORECAST.ETS.STAT(values, timeline, statistic_type, [seasonality], [data_completion], [aggregation])",
            "A statistic of the FORECAST.ETS model: 1 alpha, 2 beta, 3 gamma, 4 MASE, 5 SMAPE, 6 MAE, 7 RMSE, 8 step size.",
            forecast_ets_stat
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::*;

    thread_local! {
        pub(super) static INTERPOLATION_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn interpolation_preserves_gaps_and_endpoint_values() {
        for (slots, expected) in [
            (
                vec![None, Some(4.0), None, None, Some(13.0), Some(-2.0), None, Some(8.0), None, None],
                vec![4.0, 4.0, 7.0, 10.0, 13.0, -2.0, 3.0, 8.0, 8.0, 8.0],
            ),
            (vec![Some(-7.0), None, Some(4.0)], vec![-7.0, -1.5, 4.0]),
            (vec![Some(3.0), Some(-7.0), Some(2.0)], vec![3.0, -7.0, 2.0]),
            (vec![None, None, None], vec![0.0, 0.0, 0.0]),
            (vec![Some(6.0)], vec![6.0]),
            (vec![], vec![]),
        ] {
            assert_eq!(interpolate(&slots), expected, "{slots:?}");
        }
    }

    #[test]
    fn interpolation_lookahead_work_is_linear() {
        let mut measured = Vec::new();
        for len in [2_000, 4_000] {
            let missing = len * 3 / 10;
            let gap = (len - missing) / 2..(len + missing) / 2;
            let expected: Vec<f64> = (0..len).map(|i| (3 * i + 5) as f64).collect();
            let slots: Vec<Option<f64>> = expected.iter().enumerate().map(|(i, &v)| (!gap.contains(&i)).then_some(v)).collect();
            INTERPOLATION_VISITS.with(|visits| visits.set(0));
            let actual = interpolate(&slots);
            let visits = INTERPOLATION_VISITS.with(|visits| visits.get());
            assert_eq!(actual, expected);
            measured.push((len, visits));
        }
        // Record both sizes before asserting the work bound, including on the old algorithm.
        eprintln!("interpolation lookahead visits (slots, visits): {measured:?}");
        for (len, visits) in measured {
            assert!(visits > 0 && visits <= 2 * len, "{len} slots required {visits} lookahead visits");
        }
    }

    #[test]
    fn series_completes_a_long_gap_at_the_missing_point_limit() {
        let len = 2_000;
        let gap = 700..1_300; // Exactly 30% of the completed timeline.
        let mut times: Vec<f64> = (0..len).filter(|i| !gap.contains(i)).map(|i| (i + 1) as f64).collect();
        let mut values: Vec<f64> = times.iter().map(|t| 3.0 * t + 2.0).collect();
        let options = Options::default();
        let completed = series(&col(&values), &col(&times), &options, DateSystem::D1900).unwrap();
        let expected: Vec<f64> = (0..len).map(|i| (3 * i + 5) as f64).collect();
        assert_eq!(completed.y, expected);
        assert_eq!(completed.step, 1.0);
        assert_eq!(completed.horizon((len + 1) as f64), Ok(1.0));

        let zeroes = series(&col(&values), &col(&times), &Options { interpolate: false, ..options }, DateSystem::D1900).unwrap();
        for (i, &value) in zeroes.y.iter().enumerate() {
            assert_eq!(value, if gap.contains(&i) { 0.0 } else { expected[i] });
        }

        // Remove the first known point after the gap without changing either timeline endpoint.
        times.remove(gap.start);
        values.remove(gap.start);
        assert!(matches!(series(&col(&values), &col(&times), &options, DateSystem::D1900), Err(CellError::Num)));
    }

    fn range(xs: &[f64]) -> Arg {
        rf(col(xs))
    }

    fn timeline(n: usize) -> Vec<f64> {
        (1..=n).map(|t| t as f64).collect()
    }

    /// y = 3 + 2t plus a four-step season, over `n` points.
    fn seasonal(n: usize) -> Vec<f64> {
        let pattern = [4.0, -2.0, 7.0, -9.0];
        (1..=n).map(|t| 3.0 + 2.0 * t as f64 + pattern[(t - 1) % 4]).collect()
    }

    #[test]
    fn straight_line_forecasts_exactly() {
        let ys: Vec<f64> = (1..=20).map(|t| 3.0 + 2.0 * t as f64).collect();
        let ts = timeline(20);
        close_tol(ev("FORECAST.ETS", vec![n(25.0), range(&ys), range(&ts)]), 53.0, 1e-9);
        close_tol(ev("FORECAST.ETS", vec![n(20.5), range(&ys), range(&ts), n(0.0)]), 44.0, 1e-9);
        close(ev("FORECAST.ETS.SEASONALITY", vec![range(&ys), range(&ts)]), 0.0);
        close_tol(ev("FORECAST.ETS.CONFINT", vec![n(25.0), range(&ys), range(&ts)]), 0.0, 1e-9);
        close(ev("FORECAST.ETS.STAT", vec![range(&ys), range(&ts), n(8.0)]), 1.0);
        close_tol(ev("FORECAST.ETS.STAT", vec![range(&ys), range(&ts), n(6.0)]), 0.0, 1e-9);
        // Target dates are lifted over arrays.
        let v = ev("FORECAST.ETS", vec![av(row(&[21.0, 22.0])), range(&ys), range(&ts)]);
        let r = rows_of(&v);
        close_tol(r[0][0].clone(), 45.0, 1e-9);
        close_tol(r[0][1].clone(), 47.0, 1e-9);
    }

    #[test]
    fn seasonal_series_is_detected_and_continued() {
        let ys = seasonal(24);
        let ts = timeline(24);
        close(ev("FORECAST.ETS.SEASONALITY", vec![range(&ys), range(&ts)]), 4.0);
        // t = 27 is the third position of the pattern: 3 + 54 + 7.
        close_tol(ev("FORECAST.ETS", vec![n(27.0), range(&ys), range(&ts)]), 64.0, 1e-7);
        close_tol(ev("FORECAST.ETS", vec![n(28.0), range(&ys), range(&ts), n(4.0)]), 50.0, 1e-7);
        // Timeline order does not matter.
        let mut rev_y = ys.clone();
        let mut rev_t = ts.clone();
        rev_y.reverse();
        rev_t.reverse();
        close_tol(ev("FORECAST.ETS", vec![n(27.0), range(&rev_y), range(&rev_t)]), 64.0, 1e-7);
    }

    #[test]
    fn noisy_series_statistics_are_consistent() {
        // A seasonal series with deterministic pseudo-noise.
        let ys: Vec<f64> = seasonal(48).iter().enumerate().map(|(i, v)| v + ((i * 37 % 11) as f64 - 5.0) * 0.4).collect();
        let ts = timeline(48);
        let stat = |k: f64| match ev("FORECAST.ETS.STAT", vec![range(&ys), range(&ts), n(k)]) {
            Value::Number(x) => x,
            other => panic!("statistic {k}: {other:?}"),
        };
        for k in [1.0, 2.0, 3.0] {
            assert!((0.0..=1.0).contains(&stat(k)), "parameter {k} out of range");
        }
        let (mase, smape, mae, rmse) = (stat(4.0), stat(5.0), stat(6.0), stat(7.0));
        assert!(mae > 0.0 && rmse >= mae && mase > 0.0 && smape > 0.0 && smape < 2.0);
        let conf = |t: f64, c: f64| match ev("FORECAST.ETS.CONFINT", vec![n(t), range(&ys), range(&ts), n(c)]) {
            Value::Number(x) => x,
            other => panic!("confint: {other:?}"),
        };
        assert!(conf(49.0, 0.95) > 0.0);
        assert!(conf(60.0, 0.95) > conf(49.0, 0.95), "the interval widens with the horizon");
        assert!(conf(49.0, 0.99) > conf(49.0, 0.95));
        // The forecast stays close to the noise-free continuation.
        match ev("FORECAST.ETS", vec![n(51.0), range(&ys), range(&ts)]) {
            Value::Number(x) => assert!((x - (3.0 + 102.0 + 7.0)).abs() < 3.0, "forecast {x}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn monthly_dates_missing_points_and_duplicates() {
        // First of each month, 2024-01 … 2025-12 (serials of 1 Jan 2024 = 45292).
        let mut ts = Vec::new();
        let mut serial = 45292.0;
        for i in 0..24 {
            ts.push(serial);
            let (y, m) = (2024 + i / 12, (i % 12) as u32 + 1);
            serial += f64::from(gridcraft_core::date::days_in_month(y, m));
        }
        let ys: Vec<f64> = (0..24).map(|i| 100.0 + 5.0 * i as f64).collect();
        // 2026-01-01 = 46023, 2026-03-01 = 46082.
        close_tol(ev("FORECAST.ETS", vec![n(46023.0), range(&ys), range(&ts), n(0.0)]), 220.0, 1e-9);
        close_tol(ev("FORECAST.ETS", vec![n(46082.0), range(&ys), range(&ts), n(0.0)]), 230.0, 1e-9);
        // Missing points are interpolated (or zero with data_completion 0).
        let gaps_t: Vec<f64> = [1.0, 2.0, 4.0, 5.0, 6.0, 8.0, 9.0, 10.0].to_vec();
        let gaps_y: Vec<f64> = gaps_t.iter().map(|t| 10.0 * t).collect();
        close_tol(ev("FORECAST.ETS", vec![n(12.0), range(&gaps_y), range(&gaps_t), n(0.0)]), 120.0, 1e-9);
        assert_ne!(ev("FORECAST.ETS", vec![n(12.0), range(&gaps_y), range(&gaps_t), n(0.0), n(0.0)]), Value::Number(120.0));
        // Duplicate time stamps are averaged by default, summed with aggregation 7.
        let dup_t = [1.0, 2.0, 2.0, 3.0, 4.0, 5.0];
        let dup_y = [2.0, 3.0, 5.0, 6.0, 8.0, 10.0];
        close_tol(ev("FORECAST.ETS", vec![n(6.0), range(&dup_y), range(&dup_t), n(0.0)]), 12.0, 1e-9);
        close_tol(ev("FORECAST.ETS.STAT", vec![range(&dup_y), range(&dup_t), n(8.0)]), 1.0, 1e-12);
        assert_ne!(ev("FORECAST.ETS", vec![n(6.0), range(&dup_y), range(&dup_t), n(0.0), n(1.0), n(7.0)]), Value::Number(12.0));
    }

    #[test]
    fn documented_errors() {
        let ys = seasonal(12);
        let ts = timeline(12);
        let ets = |args: Vec<Arg>| ev("FORECAST.ETS", args);
        // Target before the end of the timeline.
        is_err(ets(vec![n(5.0), range(&ys), range(&ts)]), CellError::Num);
        // Timeline and values of different sizes.
        is_err(ets(vec![n(13.0), range(&ys), range(&ts[..11])]), CellError::NA);
        // No constant step.
        is_err(ets(vec![n(5.0), range(&[1.0, 2.0, 3.0]), range(&[1.0, 2.0, 4.5])]), CellError::Num);
        // Seasonality, completion, aggregation out of range.
        for (i, bad) in [(3, 8761.0), (3, -1.0), (4, 2.0), (5, 8.0)] {
            let mut args = vec![n(13.0), range(&ys), range(&ts), n(1.0), n(1.0), n(0.0)];
            args[i] = n(bad);
            is_err(ets(args), CellError::Num);
        }
        // More than 30% of the points missing.
        is_err(ets(vec![n(30.0), range(&[1.0, 2.0, 3.0]), range(&[1.0, 2.0, 20.0])]), CellError::Num);
        // Text time stamps; errors propagate.
        is_err(ets(vec![n(13.0), range(&[1.0, 2.0]), rf(arr(vec![vec![nv(1.0)], vec![tv("x")]]))]), CellError::Value);
        is_err(ets(vec![n(13.0), rf(arr(vec![vec![nv(1.0)], vec![Value::Error(CellError::Div0)]])), range(&[1.0, 2.0])]), CellError::Div0);
        // Confidence level outside (0, 1); statistic type outside 1–8.
        is_err(ev("FORECAST.ETS.CONFINT", vec![n(13.0), range(&ys), range(&ts), n(1.0)]), CellError::Num);
        is_err(ev("FORECAST.ETS.CONFINT", vec![n(13.0), range(&ys), range(&ts), n(0.0)]), CellError::Num);
        is_err(ev("FORECAST.ETS.STAT", vec![range(&ys), range(&ts), n(9.0)]), CellError::Num);
        is_err(ev("FORECAST.ETS.STAT", vec![range(&ys), range(&ts), n(0.0)]), CellError::Num);
        // Hostile spans: a huge gap is rejected before anything is allocated.
        is_err(ets(vec![n(1e15), range(&[1.0, 2.0, 3.0]), range(&[0.0, 1.0, 1e12])]), CellError::Num);
        is_err(ets(vec![n(1e308), range(&ys), range(&ts)]), CellError::Num);
    }

    #[test]
    fn timelines_continue_past_their_last_point() {
        let tl = Timeline::detect(&[3.0, 1.0, 2.0, 2.0, 5.0], DateSystem::D1900).expect("numeric step");
        assert_eq!(tl.after_last(1), Some(6.0));
        assert_eq!(tl.steps_after_last(8.5), Some(3.5));
        // Month ends: 2024-01-31 (45322), 2024-02-29 (45351), 2024-03-31 (45382).
        let tl = Timeline::detect(&[45322.0, 45351.0, 45382.0], DateSystem::D1900).expect("months");
        assert_eq!(tl.after_last(1), Some(45412.0)); // 2024-04-30
        assert_eq!(tl.steps_after_last(45412.0), Some(1.0));
        assert!(Timeline::detect(&[1.0, 2.0, 4.5], DateSystem::D1900).is_none());
        assert!(Timeline::detect(&[1.0, 2.0, 9.0], DateSystem::D1900).is_none(), "too many missing points");
        assert!(Timeline::detect(&[1.0], DateSystem::D1900).is_none());
        assert!(Timeline::detect(&[1.0, f64::NAN], DateSystem::D1900).is_none());
    }
}
