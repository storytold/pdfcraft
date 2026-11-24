//! Trendline maths: least-squares fits behind chart trendlines and moving averages.
//!
//! Exponential, logarithmic and power trendlines are linear fits of transformed data
//! (`ln y` against `x`, `y` against `ln x`, `ln y` against `ln x`), the way Excel documents them;
//! points where the transform is undefined (non-positive values) are skipped and R² is reported
//! for the transformed fit. Polynomials solve the normal equations on centred, scaled x.

use gridcraft_model::{Trendline, TrendlineKind};

/// A fitted regression trendline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrendFit {
    kind: TrendlineKind,
    /// Coefficients `c0..=cd` of `Y = Σ cₖ·Xᵏ` in the transformed variables, for the equation.
    pub coef: Vec<f64>,
    /// The same polynomial in `u = (X - center) / scale`, for stable evaluation.
    center: f64,
    scale: f64,
    a: Vec<f64>,
    /// Coefficient of determination; `None` when undefined (all Y equal).
    pub r2: Option<f64>,
}

/// Polynomial degree in the transformed variable; `None` for moving averages.
fn degree(k: TrendlineKind) -> Option<usize> {
    match k {
        TrendlineKind::Linear | TrendlineKind::Exponential | TrendlineKind::Logarithmic | TrendlineKind::Power => Some(1),
        TrendlineKind::Polynomial(o) => Some(usize::from(o.clamp(2, 6))),
        TrendlineKind::MovingAverage(_) => None,
    }
}

fn log_x(k: TrendlineKind) -> bool {
    matches!(k, TrendlineKind::Logarithmic | TrendlineKind::Power)
}

fn log_y(k: TrendlineKind) -> bool {
    matches!(k, TrendlineKind::Exponential | TrendlineKind::Power)
}

/// Fits `t` to the points `(x, y)`. `None` for moving averages and degenerate data (too few
/// usable points, all x equal, a singular system, non-finite results).
pub(crate) fn fit_trendline(t: &Trendline, pts: &[(f64, f64)]) -> Option<TrendFit> {
    let kind = t.kind;
    let d = degree(kind)?;
    let data: Vec<(f64, f64)> = pts
        .iter()
        .filter(|(x, y)| x.is_finite() && y.is_finite() && (!log_x(kind) || *x > 0.0) && (!log_y(kind) || *y > 0.0))
        .map(|&(x, y)| (if log_x(kind) { x.ln() } else { x }, if log_y(kind) { y.ln() } else { y }))
        .collect();
    if data.len() < 2 || data.len() <= d {
        return None;
    }
    // Set Intercept applies to linear, exponential and polynomial trendlines; it fixes Y at x = 0.
    let fixed = match kind {
        TrendlineKind::Linear | TrendlineKind::Polynomial(_) => t.intercept,
        TrendlineKind::Exponential => t.intercept.filter(|b| *b > 0.0).map(f64::ln),
        _ => None,
    }
    .filter(|b| b.is_finite());
    let n = data.len() as f64;
    let center = if fixed.is_some() { 0.0 } else { data.iter().map(|p| p.0).sum::<f64>() / n };
    let scale = data.iter().map(|p| (p.0 - center).abs()).fold(0.0f64, f64::max);
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let first = usize::from(fixed.is_some());
    let b0 = fixed.unwrap_or(0.0);
    // Normal equations over the unknown powers `first..=d` of u.
    let m = d + 1 - first;
    let mut ata = vec![vec![0.0f64; m]; m];
    let mut aty = vec![0.0f64; m];
    for &(x, y) in &data {
        let u = (x - center) / scale;
        // `d` is at most 6, so the powers `first..=d` always fit the array (no allocation per point).
        let mut buf = [0.0f64; 7];
        for (slot, k) in buf.iter_mut().zip(first..=d) {
            *slot = u.powi(k as i32);
        }
        let pows = buf.get(..m)?;
        for (i, pi) in pows.iter().enumerate() {
            if let Some(v) = aty.get_mut(i) {
                *v += pi * (y - b0);
            }
            for (j, pj) in pows.iter().enumerate() {
                if let Some(v) = ata.get_mut(i).and_then(|r| r.get_mut(j)) {
                    *v += pi * pj;
                }
            }
        }
    }
    let sol = solve(ata, aty)?;
    let mut a = vec![b0; d + 1];
    for (k, v) in sol.into_iter().enumerate() {
        if let Some(slot) = a.get_mut(k + first) {
            *slot = v;
        }
    }
    let coef = expand(&a, center, scale)?;
    let fit = TrendFit { kind, coef, center, scale, a, r2: None };
    let mean = data.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut ss_res, mut ss_tot) = (0.0f64, 0.0f64);
    for &(x, y) in &data {
        ss_res += (y - fit.poly(x)).powi(2);
        // A set intercept is LINEST(y - b, x, FALSE): the total sum of squares is not centred.
        ss_tot += if fixed.is_some() { (y - b0).powi(2) } else { (y - mean).powi(2) };
    }
    let r2 = (ss_tot > 0.0 && ss_tot.is_finite() && ss_res.is_finite()).then(|| 1.0 - ss_res / ss_tot);
    Some(TrendFit { r2, ..fit })
}

/// Gaussian elimination with partial pivoting; `None` when (nearly) singular.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    let big = a.iter().flatten().fold(0.0f64, |m, v| m.max(v.abs()));
    if !(big.is_finite() && big > 0.0) {
        return None;
    }
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| {
            let vi = a.get(i).and_then(|r| r.get(col)).map_or(0.0, |v| v.abs());
            let vj = a.get(j).and_then(|r| r.get(col)).map_or(0.0, |v| v.abs());
            vi.total_cmp(&vj)
        })?;
        a.swap(col, piv);
        b.swap(col, piv);
        let p = *a.get(col)?.get(col)?;
        if !p.is_finite() || p.abs() <= big * 1e-13 {
            return None;
        }
        let prow = a.get(col)?.clone();
        let pb = *b.get(col)?;
        for r in col + 1..n {
            let row = a.get_mut(r)?;
            let f = *row.get(col)? / p;
            for (v, pv) in row.iter_mut().zip(&prow).skip(col) {
                *v -= f * pv;
            }
            *b.get_mut(r)? -= f * pb;
        }
    }
    let mut x = vec![0.0f64; n];
    for r in (0..n).rev() {
        let row = a.get(r)?;
        let s: f64 = (r + 1..n).map(|c| row.get(c).copied().unwrap_or(0.0) * x.get(c).copied().unwrap_or(0.0)).sum();
        let v = (b.get(r)? - s) / row.get(r)?;
        *x.get_mut(r)? = v;
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Coefficients in X of `Σ aₖ·((X - c) / s)ᵏ` (binomial expansion).
fn expand(a: &[f64], c: f64, s: f64) -> Option<Vec<f64>> {
    let mut out = vec![0.0f64; a.len()];
    for (k, ak) in a.iter().enumerate() {
        let lead = ak / s.powi(k as i32);
        let mut binom = 1.0f64;
        for j in 0..=k {
            // C(k, j) · (-c)^(k-j) · X^j
            if let Some(o) = out.get_mut(j) {
                *o += lead * binom * (-c).powi((k - j) as i32);
            }
            binom = binom * (k - j) as f64 / (j + 1) as f64;
        }
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

impl TrendFit {
    /// The fitted polynomial at transformed `x` (Horner in u).
    fn poly(&self, x: f64) -> f64 {
        let u = (x - self.center) / self.scale;
        self.a.iter().rev().fold(0.0, |acc, c| acc * u + c)
    }

    /// The trendline's y at `x`; `None` outside its domain (x ≤ 0 for log/power) or when not finite.
    pub(crate) fn eval(&self, x: f64) -> Option<f64> {
        let xx = if log_x(self.kind) { if x > 0.0 { x.ln() } else { return None } } else { x };
        let p = self.poly(xx);
        let y = if log_y(self.kind) { p.exp() } else { p };
        y.is_finite().then_some(y)
    }

    /// Equation label text, e.g. `y = 2x + 1`, `y = 0.5x² - 2x + 1`, `y = 3e^(0.2x)`.
    pub(crate) fn equation(&self) -> String {
        let c0 = self.coef.first().copied().unwrap_or(0.0);
        let c1 = self.coef.get(1).copied().unwrap_or(0.0);
        match self.kind {
            TrendlineKind::Exponential => format!("y = {}e^({}x)", num_text(c0.exp()), num_text(c1)),
            TrendlineKind::Power => format!("y = {}x^{}", num_text(c0.exp()), num_text(c1)),
            TrendlineKind::Logarithmic => terms(&[(c1, "ln(x)"), (c0, "")]),
            _ => {
                let parts: Vec<(f64, &str)> = self.coef.iter().enumerate().rev().map(|(k, c)| (*c, POWERS.get(k).copied().unwrap_or("x"))).collect();
                terms(&parts)
            }
        }
    }

    /// R² label text, e.g. `R² = 0.9963`.
    pub(crate) fn r_squared(&self) -> String {
        match self.r2 {
            Some(r) => format!("R² = {}", num_text(r)),
            None => "R² = #N/A".into(),
        }
    }
}

const POWERS: [&str; 7] = ["", "x", "x²", "x³", "x⁴", "x⁵", "x⁶"];

/// `y = a·t1 ± b·t2 …`, skipping terms whose coefficient shows as 0.
fn terms(parts: &[(f64, &str)]) -> String {
    let mut s = String::from("y = ");
    let mut first = true;
    for (c, t) in parts {
        let txt = num_text(c.abs());
        if txt == "0" {
            continue;
        }
        let neg = *c < 0.0;
        if first {
            if neg {
                s.push('-');
            }
        } else {
            s.push_str(if neg { " - " } else { " + " });
        }
        // A unit coefficient on a power of x is left out: `y = x² - x + 1`.
        if txt != "1" || t.is_empty() {
            s.push_str(&txt);
        }
        s.push_str(t);
        first = false;
    }
    if first {
        s.push('0');
    }
    s
}

/// A coefficient as trendline labels show it: up to four decimals, scientific when tiny or huge.
pub(crate) fn num_text(v: f64) -> String {
    if !v.is_finite() {
        return "#NUM!".into();
    }
    let a = v.abs();
    if a == 0.0 {
        return "0".into();
    }
    if !(1e-4..1e11).contains(&a) {
        let s = format!("{v:.4E}");
        let (mant, exp) = s.split_once('E').unwrap_or((s.as_str(), "0"));
        let mant = trim_zeros(mant);
        let e: i32 = exp.parse().unwrap_or(0);
        return format!("{mant}E{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs());
    }
    let s = trim_zeros(&format!("{v:.4}"));
    if s == "-0" { "0".into() } else { s }
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
}

/// Moving average of `period` consecutive points in plotted order: point `i` (from
/// `period - 1`) is `(xᵢ, mean(yᵢ₋ₚ₊₁..=yᵢ))`. Empty when there are fewer points than `period`.
///
/// Linear in the number of points: the points are cut into blocks of `period`, and a window is
/// the tail of one block plus the head of the next, taken from running sums that only ever add
/// (a running sum that also subtracts would lose small values after a huge one passed through).
pub(crate) fn moving_average(pts: &[(f64, f64)], period: u32) -> Vec<(f64, f64)> {
    let p = period.clamp(2, 255) as usize;
    let pts: Vec<(f64, f64)> = pts.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
    let n = pts.len();
    if n < p {
        return vec![];
    }
    // `head[i]`: sum of the block's points up to and including `i`; `tail[i]`: from `i` to the block's end.
    let mut head = vec![0.0f64; n];
    let mut tail = vec![0.0f64; n];
    for (b, block) in pts.chunks(p).enumerate() {
        let (mut h, mut t) = (0.0f64, 0.0f64);
        for k in 0..block.len() {
            h += block.get(k).map_or(0.0, |q| q.1);
            if let Some(v) = head.get_mut(b * p + k) {
                *v = h;
            }
            let r = block.len() - 1 - k;
            t += block.get(r).map_or(0.0, |q| q.1);
            if let Some(v) = tail.get_mut(b * p + r) {
                *v = t;
            }
        }
    }
    (p - 1..n)
        .filter_map(|i| {
            let s = i + 1 - p;
            let mut sum = tail.get(s)? + if s.is_multiple_of(p) { 0.0 } else { *head.get(i)? };
            if !sum.is_finite() {
                // A block sum overflowed although the window itself may not: sum it directly.
                sum = pts.get(s..=i)?.iter().map(|q| q.1).sum();
            }
            Some((pts.get(i)?.0, sum / p as f64))
        })
        .collect()
}

/// Excel's automatic trendline name for the legend, e.g. `Linear (Sales)`.
pub(crate) fn auto_name(kind: TrendlineKind, series: &str) -> String {
    let k = match kind {
        TrendlineKind::Linear => "Linear".to_string(),
        TrendlineKind::Exponential => "Expon.".into(),
        TrendlineKind::Logarithmic => "Log.".into(),
        TrendlineKind::Polynomial(_) => "Poly.".into(),
        TrendlineKind::Power => "Power".into(),
        TrendlineKind::MovingAverage(p) => format!("{} per. Mov. Avg.", p.clamp(2, 255)),
    };
    format!("{k} ({series})")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64 for repeatable pseudo-random data.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn unit(&mut self) -> f64 {
            (self.next() % 1_000_000) as f64 / 1_000_000.0 - 0.5
        }
    }

    /// The window-by-window moving average this module used to compute.
    fn moving_average_naive(pts: &[(f64, f64)], period: u32) -> Vec<(f64, f64)> {
        let p = period.clamp(2, 255) as usize;
        let pts: Vec<(f64, f64)> = pts.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
        pts.windows(p).filter_map(|w| w.last().map(|(x, _)| (*x, w.iter().map(|q| q.1).sum::<f64>() / p as f64))).collect()
    }

    #[test]
    fn linear_moving_average_matches_the_window_sums() {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        for case in 0..60 {
            let n = (r.next() % 700) as usize;
            let pts: Vec<(f64, f64)> = (0..n)
                .map(|i| {
                    let y = match case % 5 {
                        0 => r.unit() * 100.0,
                        1 => 1e9 + r.unit(),
                        // A huge value passing through must not leave rounding residue behind.
                        2 => {
                            if r.next().is_multiple_of(50) {
                                1e300 * r.unit().signum()
                            } else {
                                r.unit()
                            }
                        }
                        3 => {
                            if i % 2 == 0 {
                                1e15
                            } else {
                                -1e15 + r.unit()
                            }
                        }
                        _ => {
                            if r.next().is_multiple_of(9) {
                                f64::NAN
                            } else {
                                r.unit() * 1e-3
                            }
                        }
                    };
                    (i as f64, y)
                })
                .collect();
            for period in [0, 2, 3, 7, 50, 255, 400] {
                let fast = moving_average(&pts, period);
                let slow = moving_average_naive(&pts, period);
                assert_eq!(fast.len(), slow.len(), "case {case} period {period}");
                let p = period.clamp(2, 255) as usize;
                let finite: Vec<(f64, f64)> = pts.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
                for (i, ((fx, fy), (sx, sy))) in fast.iter().zip(&slow).enumerate() {
                    assert_eq!(fx, sx);
                    let mass: f64 = finite.iter().skip(i).take(p).map(|q| q.1.abs()).sum::<f64>() / p as f64;
                    assert!((fy - sy).abs() <= 1e-12 * mass, "case {case} period {period} point {i}: {fy} vs {sy}");
                }
            }
        }
    }

    #[test]
    fn moving_average_recomputes_a_window_whose_block_sums_overflow() {
        let pts: Vec<(f64, f64)> = [0.0, -1e308, 1e308, 1e308, -1e308].iter().enumerate().map(|(i, y)| (i as f64, *y)).collect();
        let fast = moving_average(&pts, 4);
        let slow = moving_average_naive(&pts, 4);
        assert_eq!(fast, slow);
        assert!(fast.iter().all(|q| q.1.is_finite()));
    }

    /// The trendline fit as it was written with a `Vec` of powers per data point.
    fn fit_reference(t: &Trendline, pts: &[(f64, f64)]) -> Option<TrendFit> {
        let kind = t.kind;
        let d = degree(kind)?;
        let data: Vec<(f64, f64)> = pts
            .iter()
            .filter(|(x, y)| x.is_finite() && y.is_finite() && (!log_x(kind) || *x > 0.0) && (!log_y(kind) || *y > 0.0))
            .map(|&(x, y)| (if log_x(kind) { x.ln() } else { x }, if log_y(kind) { y.ln() } else { y }))
            .collect();
        if data.len() < 2 || data.len() <= d {
            return None;
        }
        let fixed = match kind {
            TrendlineKind::Linear | TrendlineKind::Polynomial(_) => t.intercept,
            TrendlineKind::Exponential => t.intercept.filter(|b| *b > 0.0).map(f64::ln),
            _ => None,
        }
        .filter(|b| b.is_finite());
        let n = data.len() as f64;
        let center = if fixed.is_some() { 0.0 } else { data.iter().map(|p| p.0).sum::<f64>() / n };
        let scale = data.iter().map(|p| (p.0 - center).abs()).fold(0.0f64, f64::max);
        if !(scale.is_finite() && scale > 0.0) {
            return None;
        }
        let first = usize::from(fixed.is_some());
        let b0 = fixed.unwrap_or(0.0);
        let m = d + 1 - first;
        let mut ata = vec![vec![0.0f64; m]; m];
        let mut aty = vec![0.0f64; m];
        for &(x, y) in &data {
            let u = (x - center) / scale;
            let pows: Vec<f64> = (first..=d).map(|k| u.powi(k as i32)).collect();
            for (i, pi) in pows.iter().enumerate() {
                aty[i] += pi * (y - b0);
                for (j, pj) in pows.iter().enumerate() {
                    ata[i][j] += pi * pj;
                }
            }
        }
        let sol = solve(ata, aty)?;
        let mut a = vec![b0; d + 1];
        for (k, v) in sol.into_iter().enumerate() {
            a[k + first] = v;
        }
        let coef = expand(&a, center, scale)?;
        let fit = TrendFit { kind, coef, center, scale, a, r2: None };
        let mean = data.iter().map(|p| p.1).sum::<f64>() / n;
        let (mut ss_res, mut ss_tot) = (0.0f64, 0.0f64);
        for &(x, y) in &data {
            ss_res += (y - fit.poly(x)).powi(2);
            ss_tot += if fixed.is_some() { (y - b0).powi(2) } else { (y - mean).powi(2) };
        }
        let r2 = (ss_tot > 0.0 && ss_tot.is_finite() && ss_res.is_finite()).then(|| 1.0 - ss_res / ss_tot);
        Some(TrendFit { r2, ..fit })
    }

    #[test]
    fn fit_without_a_vec_per_point_is_bit_identical() {
        let mut r = Rng(0x1234_5678_9ABC_DEF1);
        let kinds = [
            TrendlineKind::Linear,
            TrendlineKind::Exponential,
            TrendlineKind::Logarithmic,
            TrendlineKind::Power,
            TrendlineKind::Polynomial(2),
            TrendlineKind::Polynomial(3),
            TrendlineKind::Polynomial(6),
            TrendlineKind::MovingAverage(3),
        ];
        for case in 0..40 {
            let n = 2 + (r.next() % 60) as usize;
            let pts: Vec<(f64, f64)> = (0..n)
                .map(|i| {
                    let x = i as f64 * (1.0 + r.unit()) + 1.0;
                    let y = if case % 4 == 0 { f64::NAN } else { (r.unit() + 0.6) * 10f64.powi((case % 7) - 2) };
                    (x, if case % 4 == 0 && i % 3 != 0 { r.unit() + 1.0 } else { y })
                })
                .collect();
            for kind in kinds {
                for intercept in [None, Some(0.5), Some(-1.0)] {
                    let t = Trendline { kind, intercept, ..Trendline::default() };
                    assert_eq!(fit_trendline(&t, &pts), fit_reference(&t, &pts), "case {case} {kind:?} {intercept:?}");
                }
            }
        }
    }
}
