//! Least-squares trendlines: linear, exponential, logarithmic, polynomial, power, and a
//! moving average. The fit is what the chart drawer plots and what the axis grows to include.

use crate::chart::{ChartType, Series, Trendline, TrendlineType};

/// A fitted trendline. `y` is `None` where the model is undefined (a log of a non-positive x,
/// an exponential that overflowed).
#[derive(Clone, Debug)]
pub struct TrendFit {
    kind: TrendlineType,
    model: Model,
    r2: f64,
    x_min: f64,
    x_max: f64,
}

#[derive(Clone, Debug)]
enum Model {
    /// `y = c[0] + c[1] x + c[2] x² + …`
    Poly(Vec<f64>),
    /// `y = c · e^(b x)`
    Exp { c: f64, b: f64 },
    /// `y = b + c · ln(x)`
    Log { b: f64, c: f64 },
    /// `y = c · x^b`
    Power { c: f64, b: f64 },
    /// Window averages in x order. No equation.
    Moving(Vec<(f64, f64)>),
}

impl Trendline {
    /// Fit `xs`/`ys` (paired). Non-finite pairs are dropped. `None` when the type needs more
    /// points than it was given, or the normal equations have no solution.
    pub fn fit(&self, xs: &[f64], ys: &[f64]) -> Option<TrendFit> {
        let mut pairs: Vec<(f64, f64)> =
            xs.iter().zip(ys).map(|(x, y)| (*x, *y)).filter(|(x, y)| x.is_finite() && y.is_finite()).take(10_000).collect();
        if pairs.len() < 2 {
            return None;
        }
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let (x_min, x_max) = (pairs.first().map(|p| p.0)?, pairs.last().map(|p| p.0)?);
        let intercept = self.intercept.filter(|v| v.is_finite());
        let (model, used_y, yhat) = match self.kind {
            TrendlineType::Linear => {
                let (xs, ys) = split(&pairs);
                let (b, m) = linreg(&xs, &ys, intercept)?;
                let yhat: Vec<f64> = xs.iter().map(|x| b + m * x).collect();
                (Model::Poly(vec![b, m]), ys, yhat)
            }
            TrendlineType::Poly => {
                let order = self.order.unwrap_or(2).clamp(2, 6) as usize;
                if pairs.len() < order + 1 {
                    return None;
                }
                let (xs, ys) = split(&pairs);
                let coeff = polyfit(&xs, &ys, order, intercept)?;
                let yhat: Vec<f64> = xs.iter().map(|x| poly_y(&coeff, *x)).collect();
                (Model::Poly(coeff), ys, yhat)
            }
            TrendlineType::Exp => fit_exp(&pairs, intercept)?,
            TrendlineType::Log => {
                let pairs: Vec<(f64, f64)> = pairs.into_iter().filter(|(x, _)| *x > 0.0).collect();
                if pairs.len() < 2 {
                    return None;
                }
                let (ln_x, ys): (Vec<f64>, Vec<f64>) = pairs.iter().map(|(x, y)| (x.ln(), *y)).unzip();
                let (b, c) = linreg(&ln_x, &ys, intercept)?;
                let yhat: Vec<f64> = ln_x.iter().map(|u| b + c * u).collect();
                (Model::Log { b, c }, ys, yhat)
            }
            TrendlineType::Power => fit_power(&pairs, intercept)?,
            TrendlineType::MovingAvg => {
                let period = self.period.unwrap_or(2).clamp(2, 255) as usize;
                fit_moving_avg(&pairs, period)?
            }
        };
        if yhat.iter().any(|v| !v.is_finite()) {
            return None;
        }
        // A moving average starts at the first full window, not at the first data point.
        let (x_min, x_max) = if let Model::Moving(pts) = &model {
            match (pts.first(), pts.last()) {
                (Some(a), Some(b)) => (a.0, b.0),
                _ => (x_min, x_max),
            }
        } else {
            (x_min, x_max)
        };
        Some(TrendFit { kind: self.kind, model, r2: r_squared(&used_y, &yhat), x_min, x_max })
    }

    /// The equation and R² lines this trendline asks to show. Empty when neither is on. A moving
    /// average has no equation, and PowerPoint does not show its R², so that label is left off
    /// even when the file asks for it.
    pub fn caption(&self, fit: &TrendFit) -> String {
        let mut lines = Vec::new();
        if self.disp_eq
            && let Some(eq) = fit.equation()
        {
            lines.push(eq);
        }
        if self.disp_r_sqr && self.kind != TrendlineType::MovingAvg {
            lines.push(fit.r2_label());
        }
        lines.join("\n")
    }
}

impl TrendFit {
    pub fn y(&self, x: f64) -> Option<f64> {
        if !x.is_finite() {
            return None;
        }
        let y = match &self.model {
            Model::Poly(c) => poly_y(c, x),
            Model::Exp { c, b } => c * (b * x).exp(),
            Model::Log { b, c } => {
                if x <= 0.0 {
                    return None;
                }
                b + c * x.ln()
            }
            Model::Power { c, b } => {
                if x <= 0.0 {
                    return None;
                }
                c * x.powf(*b)
            }
            Model::Moving(_) => return None,
        };
        y.is_finite().then_some(y)
    }

    /// Samples from `x0` to `x1`, forecast included. A moving average returns its window points
    /// and ignores the forecast (Excel does not extend one).
    pub fn samples(&self, x0: f64, x1: f64) -> Vec<(f64, f64)> {
        let (x0, x1) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        if !x0.is_finite() || !x1.is_finite() {
            return Vec::new();
        }
        if let Model::Moving(pts) = &self.model {
            return pts.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).take(10_000).collect();
        }
        if matches!(self.kind, TrendlineType::Linear) {
            return [x0, x1].into_iter().filter_map(|x| self.y(x).map(|y| (x, y))).collect();
        }
        let steps = 48;
        (0..=steps)
            .filter_map(|i| {
                let x = x0 + (x1 - x0) * i as f64 / steps as f64;
                self.y(x).map(|y| (x, y))
            })
            .collect()
    }

    /// Data x plus forecast, in category (or scatter x) units. Forecast is capped so a hostile
    /// value cannot walk off the chart.
    pub fn span(&self, forward: Option<f64>, backward: Option<f64>) -> (f64, f64) {
        let pad = |v: Option<f64>| v.filter(|v| v.is_finite() && *v > 0.0).map(|v| v.min(24.0)).unwrap_or(0.0);
        if matches!(self.kind, TrendlineType::MovingAvg) { (self.x_min, self.x_max) } else { (self.x_min - pad(backward), self.x_max + pad(forward)) }
    }

    pub fn equation(&self) -> Option<String> {
        let body = match &self.model {
            Model::Poly(c) => poly_equation(c),
            Model::Exp { c, b } => Some(format!("y = {}e^({}x)", num(*c), num(*b))),
            Model::Log { b, c } => poly_equation(&[*b, *c]).map(|s| s.replacen("x", "ln(x)", 1)),
            Model::Power { c, b } => {
                let mag = if (c.abs() - 1.0).abs() < 1e-6 { String::new() } else { num(c.abs()) };
                let sign = if *c < 0.0 { "-" } else { "" };
                Some(format!("y = {sign}{mag}{}", x_pow(*b)))
            }
            Model::Moving(_) => None,
        }?;
        Some(body)
    }

    pub fn r2_label(&self) -> String {
        let r = if self.r2.is_finite() { self.r2 } else { 0.0 };
        if (r - 1.0).abs() < 5e-4 { "R² = 1".to_string() } else { format!("R² = {r:.3}") }
    }
}

impl Series {
    /// Trendline y values, including a short forecast, that sit near this series. The axis uses
    /// them so a forecast is not clipped off the plot. A value many times the series' own range
    /// is left out, so a wild polynomial cannot flatten the chart.
    pub fn trendline_ys(&self, scatter: bool) -> Vec<f64> {
        let n = self.values.len().min(10_000);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for i in 0..n {
            let Some(y) = self.values.get(i).copied().flatten().filter(|v| v.is_finite()) else { continue };
            let x = if scatter {
                let Some(x) = self.x_values.get(i).copied().flatten().filter(|v| v.is_finite()) else { continue };
                x
            } else {
                (i + 1) as f64
            };
            xs.push(x);
            ys.push(y);
        }
        let (data_lo, data_hi) = ys.iter().fold((f64::MAX, f64::MIN), |(lo, hi), y| (lo.min(*y), hi.max(*y)));
        if !data_lo.is_finite() || !data_hi.is_finite() {
            return Vec::new();
        }
        let span = (data_hi - data_lo).abs().max(1.0);
        let mid = (data_lo + data_hi) / 2.0;
        let mut out = Vec::new();
        for t in self.trendlines.iter().take(8) {
            let Some(fit) = t.fit(&xs, &ys) else { continue };
            let (x0, x1) = fit.span(t.forward, t.backward);
            for (_, y) in fit.samples(x0, x1) {
                if y.is_finite() && (y - mid).abs() <= span * 8.0 {
                    out.push(y);
                }
            }
        }
        out
    }
}

/// Whether a series is plotted against numeric x values rather than category positions.
pub fn series_is_scatter(kind: ChartType) -> bool {
    matches!(kind, ChartType::Scatter | ChartType::Bubble)
}

fn split(pairs: &[(f64, f64)]) -> (Vec<f64>, Vec<f64>) {
    pairs.iter().map(|(x, y)| (*x, *y)).unzip()
}

/// `y = b + m x`. `intercept` fixes `b`.
fn linreg(xs: &[f64], ys: &[f64], intercept: Option<f64>) -> Option<(f64, f64)> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    if let Some(b) = intercept {
        let mut sxx = 0.0;
        let mut sxy = 0.0;
        for (x, y) in xs.iter().zip(ys) {
            sxx += x * x;
            sxy += x * (y - b);
        }
        if sxx <= 0.0 || !sxx.is_finite() {
            return None;
        }
        let m = sxy / sxx;
        return (m.is_finite() && b.is_finite()).then_some((b, m));
    }
    let n = xs.len() as f64;
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let (mut sxx, mut sxy) = (0.0, 0.0);
    for (x, y) in xs.iter().zip(ys) {
        let dx = x - mx;
        sxx += dx * dx;
        sxy += dx * (y - my);
    }
    // Constant x with a large offset leaves rounding residue in `mx`, so compare against the
    // rounding scale of the mean instead of zero.
    if sxx <= 4.0 * n * (f64::EPSILON * mx.abs()).powi(2) || !sxx.is_finite() {
        return None;
    }
    let m = sxy / sxx;
    let b = my - m * mx;
    (m.is_finite() && b.is_finite()).then_some((b, m))
}

fn poly_y(coeff: &[f64], x: f64) -> f64 {
    let mut acc = 0.0;
    let mut xp = 1.0;
    for c in coeff {
        acc += c * xp;
        if !acc.is_finite() {
            return f64::NAN;
        }
        xp *= x;
    }
    acc
}

fn fit_exp(pairs: &[(f64, f64)], intercept: Option<f64>) -> Option<(Model, Vec<f64>, Vec<f64>)> {
    let pairs: Vec<(f64, f64)> = pairs.iter().copied().filter(|(_, y)| *y > 0.0).collect();
    if pairs.len() < 2 {
        return None;
    }
    let (xs, ln_y): (Vec<f64>, Vec<f64>) = pairs.iter().map(|(x, y)| (*x, y.ln())).unzip();
    let fixed = intercept.filter(|c| *c > 0.0).map(f64::ln);
    let (ln_c, b) = linreg(&xs, &ln_y, fixed)?;
    let c = ln_c.exp();
    if !c.is_finite() || c <= 0.0 {
        return None;
    }
    let ys: Vec<f64> = pairs.iter().map(|(_, y)| *y).collect();
    let yhat: Vec<f64> = xs.iter().map(|x| c * (b * x).exp()).collect();
    Some((Model::Exp { c, b }, ys, yhat))
}

fn fit_power(pairs: &[(f64, f64)], intercept: Option<f64>) -> Option<(Model, Vec<f64>, Vec<f64>)> {
    let pairs: Vec<(f64, f64)> = pairs.iter().copied().filter(|(x, y)| *x > 0.0 && *y > 0.0).collect();
    if pairs.len() < 2 {
        return None;
    }
    let (ln_x, ln_y): (Vec<f64>, Vec<f64>) = pairs.iter().map(|(x, y)| (x.ln(), y.ln())).unzip();
    let fixed = intercept.filter(|c| *c > 0.0).map(f64::ln);
    let (ln_c, b) = linreg(&ln_x, &ln_y, fixed)?;
    let c = ln_c.exp();
    if !c.is_finite() || c <= 0.0 {
        return None;
    }
    let ys: Vec<f64> = pairs.iter().map(|(_, y)| *y).collect();
    let yhat: Vec<f64> = pairs.iter().map(|(x, _)| c * x.powf(b)).collect();
    Some((Model::Power { c, b }, ys, yhat))
}

fn fit_moving_avg(pairs: &[(f64, f64)], period: usize) -> Option<(Model, Vec<f64>, Vec<f64>)> {
    if pairs.len() <= period {
        return None;
    }
    let mut pts = Vec::new();
    let mut ys = Vec::new();
    let mut yhat = Vec::new();
    for i in period - 1..pairs.len() {
        let window = &pairs[i + 1 - period..=i];
        let avg = window.iter().map(|(_, y)| y).sum::<f64>() / period as f64;
        if avg.is_finite() {
            pts.push((pairs[i].0, avg));
            ys.push(pairs[i].1);
            yhat.push(avg);
        }
    }
    if pts.len() < 2 {
        return None;
    }
    Some((Model::Moving(pts), ys, yhat))
}

#[allow(clippy::needless_range_loop)]
fn polyfit_with_intercept(xs: &[f64], ys: &[f64], order: usize, b0: f64) -> Option<Vec<f64>> {
    // Fit `y - b0 = (x / sx) * q(u)` with `u = (x - xm) / su` and `q` of degree `order - 1`. The
    // basis `(x / sx) * u^j` spans the same polynomials as `x^(j + 1)`, but keeps the pivots in
    // `solve` independent of the units and the offset of x.
    let n = xs.len() as f64;
    let sx = xs.iter().fold(0.0_f64, |acc, x| acc.max(x.abs()));
    if sx <= 0.0 || !sx.is_finite() {
        return None;
    }
    let xm = xs.iter().sum::<f64>() / n;
    let su = match xs.iter().fold(0.0_f64, |acc, x| acc.max((x - xm).abs())) {
        s if s > 0.0 && s.is_finite() => s,
        _ => 1.0,
    };
    let m = order;
    let mut ata = vec![vec![0.0; m]; m];
    let mut atb = vec![0.0; m];
    for (x, y) in xs.iter().zip(ys) {
        let v = x / sx;
        let u = (x - xm) / su;
        let dy = y - b0;
        for i in 0..m {
            let xi = v * u.powi(i as i32);
            atb[i] += dy * xi;
            for j in i..m {
                ata[i][j] += xi * v * u.powi(j as i32);
            }
        }
    }
    for i in 0..m {
        for j in 0..i {
            ata[i][j] = ata[j][i];
        }
    }
    let centred: Vec<f64> = solve(ata, atb)?.iter().enumerate().map(|(k, c)| c / su.powi(k as i32)).collect();
    let q = expand_shift(&centred, xm);
    let mut coeff = Vec::with_capacity(order + 1);
    coeff.push(b0);
    coeff.extend(q.iter().map(|c| c / sx));
    coeff.iter().all(|v| v.is_finite()).then_some(coeff)
}

/// Polynomial through the normal equations. `x` is centred when the intercept is free, then
/// expanded back to powers of `x`, so order 6 on category numbers stays solvable.
#[allow(clippy::needless_range_loop)] // the index is the power in the normal equations
fn polyfit(xs: &[f64], ys: &[f64], order: usize, intercept: Option<f64>) -> Option<Vec<f64>> {
    let n = xs.len();
    if n != ys.len() || !(1..=6).contains(&order) || n < order + 1 {
        return None;
    }
    if let Some(b0) = intercept {
        return polyfit_with_intercept(xs, ys, order, b0);
    }
    let xm = xs.iter().sum::<f64>() / n as f64;
    // Scale the centred x to unit range so the pivot threshold in `solve` does not depend on the
    // units of x.
    let s = xs.iter().fold(0.0_f64, |acc, x| acc.max((x - xm).abs()));
    if s <= 0.0 || !s.is_finite() {
        return None;
    }
    let us: Vec<f64> = xs.iter().map(|x| (x - xm) / s).collect();
    let m = order + 1;
    let mut ata = vec![vec![0.0; m]; m];
    let mut atb = vec![0.0; m];
    for (u, y) in us.iter().zip(ys) {
        for i in 0..m {
            let ui = if i == 0 { 1.0 } else { u.powi(i as i32) };
            atb[i] += y * ui;
            for j in i..m {
                let uj = if j == 0 { 1.0 } else { u.powi(j as i32) };
                ata[i][j] += ui * uj;
            }
        }
    }
    for i in 0..m {
        for j in 0..i {
            ata[i][j] = ata[j][i];
        }
    }
    let centred: Vec<f64> = solve(ata, atb)?.iter().enumerate().map(|(k, c)| c / s.powi(k as i32)).collect();
    let coeff = expand_shift(&centred, xm);
    coeff.iter().all(|v| v.is_finite()).then_some(coeff)
}

/// `a[k]` is the coefficient of `(x - xm)^k`. Return coefficients of powers of `x`.
#[allow(clippy::needless_range_loop)] // j is the power of x after the binomial shift
fn expand_shift(a: &[f64], xm: f64) -> Vec<f64> {
    let mut c = vec![0.0; a.len()];
    for (k, ak) in a.iter().enumerate() {
        let mut binom = 1.0;
        for j in 0..=k {
            c[j] += ak * binom * (-xm).powi((k - j) as i32);
            if j < k {
                binom *= (k - j) as f64 / (j + 1) as f64;
            }
        }
    }
    c
}

#[allow(clippy::needless_range_loop)] // Gaussian elimination walks rows and columns by index
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    if n == 0 || n > 8 || a.len() != n {
        return None;
    }
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let div = a[col][col];
        for c in col..n {
            a[col][c] /= div;
        }
        b[col] /= div;
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}

fn r_squared(ys: &[f64], yhat: &[f64]) -> f64 {
    if ys.is_empty() || ys.len() != yhat.len() {
        return 0.0;
    }
    let mean = ys.iter().sum::<f64>() / ys.len() as f64;
    let mut ss_tot = 0.0;
    let mut ss_res = 0.0;
    for (y, h) in ys.iter().zip(yhat) {
        ss_tot += (y - mean) * (y - mean);
        ss_res += (y - h) * (y - h);
    }
    if ss_tot < 1e-18 {
        return if ss_res < 1e-12 { 1.0 } else { 0.0 };
    }
    let r = 1.0 - ss_res / ss_tot;
    if r.is_finite() { r } else { 0.0 }
}

fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let a = v.abs();
    if a == 0.0 {
        return "0".to_string();
    }
    if !(1.0e-3..1.0e6).contains(&a) {
        return format!("{v:.2e}").replace('+', "");
    }
    let digits = if a >= 100.0 {
        1
    } else if a >= 10.0 {
        2
    } else {
        3
    };
    let mut s = format!("{v:.prec$}", prec = digits);
    if let Some(dot) = s.find('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.len() == dot + 1 {
            s.pop();
        }
    }
    s
}

fn x_pow(p: f64) -> String {
    if (p - 1.0).abs() < 1e-6 {
        "x".to_string()
    } else if (p - 2.0).abs() < 1e-6 {
        "x²".to_string()
    } else if (p - 3.0).abs() < 1e-6 {
        "x³".to_string()
    } else {
        format!("x^{}", num(p))
    }
}

fn poly_equation(coeff: &[f64]) -> Option<String> {
    // Drop terms that are negligible relative to the largest coefficient, so tiny-valued data keeps its terms.
    let scale = coeff.iter().map(|c| c.abs()).fold(0.0, f64::max);
    let mut body = String::new();
    let mut first = true;
    for (k, c) in coeff.iter().enumerate().rev() {
        if c.abs() <= scale * 1e-8 {
            continue;
        }
        let pow = match k {
            0 => String::new(),
            1 => "x".to_string(),
            _ => x_pow(k as f64),
        };
        let mag = if pow.is_empty() {
            num(c.abs())
        } else if (c.abs() - 1.0).abs() < scale.max(1.0) * 1e-8 {
            pow
        } else {
            format!("{}{}", num(c.abs()), pow)
        };
        if first {
            if *c < 0.0 {
                body.push('-');
            }
            body.push_str(&mag);
            first = false;
        } else if *c < 0.0 {
            body.push_str(" - ");
            body.push_str(&mag);
        } else {
            body.push_str(" + ");
            body.push_str(&mag);
        }
    }
    if body.is_empty() {
        body.push('0');
    }
    Some(format!("y = {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: TrendlineType) -> Trendline {
        Trendline { kind, ..Default::default() }
    }

    #[test]
    fn linear_fits_tiny_x_values() {
        let xs = [1e-10, 2e-10, 3e-10, 4e-10, 5e-10];
        let ys = [1.0, 2.0, 3.0, 4.0, 5.0];
        let fit = line(TrendlineType::Linear).fit(&xs, &ys).expect("fit of tiny x values");
        assert!((fit.y(3e-10).unwrap() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn poly_fits_tiny_x_values() {
        let xs = [1e-5, 2e-5, 3e-5, 4e-5, 5e-5];
        let f = |x: f64| 1.0 + 2.0 * x + 3.0 * x * x;
        let ys: Vec<f64> = xs.iter().map(|x| f(*x)).collect();
        let fit = line(TrendlineType::Poly).fit(&xs, &ys).expect("poly fit of tiny x values");
        assert!((fit.y(3e-5).unwrap() - f(3e-5)).abs() < 1e-9);
        let big: Vec<f64> = xs.iter().map(|x| x * 1e5).collect();
        let scaled = line(TrendlineType::Poly).fit(&big, &ys).expect("poly fit of scaled x values");
        for x in xs {
            assert!((fit.y(x).unwrap() - scaled.y(x * 1e5).unwrap()).abs() < 1e-9, "{x}");
        }
    }

    #[test]
    fn poly_with_intercept_fits_tiny_x_values() {
        let xs = [1e-5, 2e-5, 3e-5, 4e-5, 5e-5];
        let f = |x: f64| 1.0 + 2.0 * x + 3.0 * x * x;
        let ys: Vec<f64> = xs.iter().map(|x| f(*x)).collect();
        let mut poly = line(TrendlineType::Poly);
        poly.intercept = Some(1.0);
        let fit = poly.fit(&xs, &ys).expect("poly fit with intercept of tiny x values");
        assert!((fit.y(3e-5).unwrap() - f(3e-5)).abs() < 1e-9);
        let big: Vec<f64> = xs.iter().map(|x| x * 1e5).collect();
        let scaled = poly.fit(&big, &ys).expect("poly fit with intercept of scaled x values");
        for x in xs {
            assert!((fit.y(x).unwrap() - scaled.y(x * 1e5).unwrap()).abs() < 1e-9, "{x}");
        }
    }

    #[test]
    fn poly_with_intercept_fits_offset_but_narrow_x_range() {
        let xs = [45000.00, 45000.01, 45000.02, 45000.03, 45000.04];
        let f = |x: f64| 1.0 + x * (1.0 + 100.0 * (x - 45000.02));
        let ys: Vec<f64> = xs.iter().map(|x| f(*x)).collect();
        let mut poly = line(TrendlineType::Poly);
        poly.intercept = Some(1.0);
        let fit = poly.fit(&xs, &ys).expect("poly fit with intercept of offset x values");
        for x in xs {
            assert!((fit.y(x).unwrap() - f(x)).abs() < 1e-2, "{x}");
        }
    }

    #[test]
    fn linear_fits_offset_but_narrow_x_range() {
        let xs = [45000.00, 45000.01, 45000.02, 45000.03, 45000.04];
        let ys = [1.0, 2.0, 3.0, 4.0, 5.0];
        let fit = line(TrendlineType::Linear).fit(&xs, &ys).expect("fit of offset x values");
        assert!((fit.y(45000.02).unwrap() - 3.0).abs() < 1e-4);
        let xs = [1e7, 1e7 + 1.0, 1e7 + 2.0, 1e7 + 3.0, 1e7 + 4.0];
        let fit = line(TrendlineType::Linear).fit(&xs, &ys).expect("fit of 1e7 offset");
        assert!((fit.y(1e7 + 2.0).unwrap() - 3.0).abs() < 1e-4);
    }

    #[test]
    fn linear_fixed_intercept_on_offset_narrow_x_range() {
        let xs = [1e7, 1e7 + 1.0, 1e7 + 2.0, 1e7 + 3.0, 1e7 + 4.0];
        let ys: Vec<f64> = xs.iter().map(|x| 2.0 * x).collect();
        let mut forced = line(TrendlineType::Linear);
        forced.intercept = Some(0.0);
        let fit = forced.fit(&xs, &ys).expect("fixed-intercept fit");
        assert!((fit.y(1e7 + 2.0).unwrap() / (2.0 * (1e7 + 2.0)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn linear_fixed_intercept_rejects_overflowing_x() {
        let xs = [1e160, 2e160, 3e160];
        let ys = [1.0, 2.0, 3.0];
        let mut forced = line(TrendlineType::Linear);
        forced.intercept = Some(0.0);
        assert!(forced.fit(&xs, &ys).is_none());
    }

    #[test]
    fn linear_rejects_constant_x_with_large_offset() {
        let xs = [45000.1; 5];
        let ys = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!(line(TrendlineType::Linear).fit(&xs, &ys).is_none());
    }

    #[test]
    fn linear_recovers_slope_and_a_fixed_intercept() {
        let xs = [1.0, 2.0, 3.0, 4.0];
        let ys = [3.0, 5.0, 7.0, 9.0];
        let fit = line(TrendlineType::Linear).fit(&xs, &ys).expect("fit");
        assert!((fit.y(1.0).unwrap() - 3.0).abs() < 1e-9);
        assert!((fit.y(4.0).unwrap() - 9.0).abs() < 1e-9);
        assert_eq!(fit.equation().as_deref(), Some("y = 2x + 1"));
        assert_eq!(fit.r2_label(), "R² = 1");
        let mut forced = line(TrendlineType::Linear);
        forced.intercept = Some(0.0);
        let fit = forced.fit(&[1.0, 2.0], &[2.0, 4.0]).expect("fit");
        assert!((fit.y(3.0).unwrap() - 6.0).abs() < 1e-6);
        assert_eq!(fit.equation().as_deref(), Some("y = 2x"));
    }

    #[test]
    fn exp_log_power_and_quadratic_round_their_points() {
        let xs = [1.0, 2.0, 3.0, 4.0];
        let exp = line(TrendlineType::Exp).fit(&xs, &[1.0, 2.0, 4.0, 8.0]).expect("exp");
        for (x, y) in xs.iter().zip([1.0, 2.0, 4.0, 8.0]) {
            assert!((exp.y(*x).unwrap() - y).abs() < 1e-6, "{x}");
        }
        assert!(exp.equation().unwrap().contains("e^("));
        let log = line(TrendlineType::Log).fit(&xs, &[1.0_f64.ln(), 2.0_f64.ln(), 3.0_f64.ln(), 4.0_f64.ln()]).expect("log");
        assert!((log.y(2.0).unwrap() - 2.0_f64.ln()).abs() < 1e-6);
        assert!(log.equation().unwrap().contains("ln(x)"));
        let power = line(TrendlineType::Power).fit(&xs, &[1.0, 4.0, 9.0, 16.0]).expect("power");
        assert!((power.y(3.0).unwrap() - 9.0).abs() < 1e-6);
        assert_eq!(power.equation().as_deref(), Some("y = x²"));
        let mut poly = line(TrendlineType::Poly);
        poly.order = Some(2);
        let quad = poly.fit(&xs, &[1.0, 4.0, 9.0, 16.0]).expect("poly");
        assert!((quad.y(3.0).unwrap() - 9.0).abs() < 1e-6);
        assert_eq!(quad.equation().as_deref(), Some("y = x²"));
    }

    #[test]
    fn moving_average_and_forecast_span() {
        let fit = Trendline { kind: TrendlineType::MovingAvg, period: Some(2), ..Default::default() }
            .fit(&[1.0, 2.0, 3.0, 4.0], &[2.0, 8.0, 4.0, 10.0])
            .expect("ma");
        let pts = fit.samples(0.0, 10.0);
        assert_eq!(pts.len(), 3);
        assert!((pts[0].1 - 5.0).abs() < 1e-9);
        assert!((pts[1].1 - 6.0).abs() < 1e-9);
        assert!((pts[2].1 - 7.0).abs() < 1e-9);
        assert!(fit.equation().is_none());
        let linear = line(TrendlineType::Linear).fit(&[1.0, 2.0], &[1.0, 2.0]).expect("line");
        assert_eq!(linear.span(Some(1.5), Some(-3.0)), (1.0, 3.5));
        assert_eq!(fit.span(Some(4.0), Some(4.0)), (2.0, 4.0));
    }

    #[test]
    fn moving_average_shows_neither_equation_nor_r2() {
        let t = Trendline { kind: TrendlineType::MovingAvg, period: Some(2), disp_eq: true, disp_r_sqr: true, ..Default::default() };
        let fit = t.fit(&[1.0, 2.0, 3.0, 4.0], &[2.0, 8.0, 4.0, 10.0]).expect("ma");
        assert!(fit.r2_label().starts_with("R² ="));
        assert!(t.caption(&fit).is_empty(), "{}", t.caption(&fit));
    }

    #[test]
    fn too_few_points_and_non_finite_input_fit_nothing() {
        assert!(line(TrendlineType::Linear).fit(&[1.0], &[2.0]).is_none());
        assert!(line(TrendlineType::Exp).fit(&[1.0, 2.0], &[-1.0, -2.0]).is_none());
        assert!(line(TrendlineType::Poly).fit(&[1.0, 2.0], &[1.0, 2.0]).is_none());
        assert!(line(TrendlineType::Linear).fit(&[f64::NAN, f64::INFINITY], &[1.0, 2.0]).is_none());
        let mut t = line(TrendlineType::Linear);
        t.disp_eq = true;
        t.disp_r_sqr = true;
        let fit = t.fit(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).expect("fit");
        assert_eq!(t.caption(&fit), "y = x\nR² = 1");
    }

    #[test]
    fn equation_keeps_tiny_coefficients() {
        let xs = [1.0, 2.0, 3.0, 4.0];
        let lin = line(TrendlineType::Linear).fit(&xs, &[2e-9, 4e-9, 6e-9, 8e-9]).expect("linear");
        assert_eq!(lin.equation().as_deref(), Some("y = 2.00e-9x"));
        let mut poly = line(TrendlineType::Poly);
        poly.order = Some(2);
        let ys: Vec<f64> = xs.iter().map(|x| 3e-9 * x * x + 1e-9).collect();
        let quad = poly.fit(&xs, &ys).expect("poly");
        assert_eq!(quad.equation().as_deref(), Some("y = 3.00e-9x² + 1.00e-9"));
        let ys: Vec<f64> = xs.iter().map(|x: &f64| 5e-9 * x.ln()).collect();
        let log = line(TrendlineType::Log).fit(&xs, &ys).expect("log");
        assert_eq!(log.equation().as_deref(), Some("y = 5.00e-9ln(x)"));
        let zero = line(TrendlineType::Linear).fit(&xs, &[0.0; 4]).expect("zero");
        assert_eq!(zero.equation().as_deref(), Some("y = 0"));
    }
}
