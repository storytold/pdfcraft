use super::{Arg, CellError, Ctx, PI, R, Value, num, num_val};

// ---------------------------------------------------------------------------------------------
// Bessel functions

const MAX_BESSEL_POINTS: f64 = 4_000_000.0;

/// J_n(x) for integer n ≥ 0 via Bessel's integral, J_n(x) = (1/2π)∫₀^{2π} cos(nτ − x sin τ) dτ,
/// evaluated with the trapezoid rule, which converges exponentially for periodic integrands.
fn bessel_j(n: u32, x: f64) -> Option<f64> {
    let pts = (2.0 * (x.abs() + n as f64) + 64.0).ceil();
    if pts > MAX_BESSEL_POINTS {
        return if x.abs() > 1e6 { hankel(n, x.abs()).map(|(j, _)| if x < 0.0 && n % 2 == 1 { -j } else { j }) } else { None };
    }
    let m = pts as usize;
    let h = 2.0 * PI / m as f64;
    let nf = n as f64;
    let mut sum = 0.0;
    for k in 0..m {
        let t = k as f64 * h;
        sum += (nf * t - x * t.sin()).cos();
    }
    Some(sum / m as f64)
}

/// Hankel asymptotic expansion: (J_n(x), Y_n(x)) for large x.
fn hankel(n: u32, x: f64) -> Option<(f64, f64)> {
    let mu = 4.0 * (n as f64) * (n as f64);
    let mut p = 0.0;
    let mut q = 0.0;
    let mut term: f64 = 1.0;
    let mut last = f64::INFINITY;
    for k in 0..200 {
        if term.abs() > last {
            break;
        }
        let t = if k % 4 < 2 { term } else { -term };
        if k % 2 == 0 {
            p += t;
        } else {
            q += t;
        }
        last = term.abs();
        if last < 1e-17 {
            break;
        }
        let kk = (k + 1) as f64;
        let odd = 2.0 * kk - 1.0;
        term *= (mu - odd * odd) / (kk * 8.0 * x);
    }
    let chi = x - (n as f64 / 2.0 + 0.25) * PI;
    let f = (2.0 / (PI * x)).sqrt();
    let j = f * (p * chi.cos() - q * chi.sin());
    let y = f * (p * chi.sin() + q * chi.cos());
    if j.is_finite() && y.is_finite() { Some((j, y)) } else { None }
}

const EULER: f64 = std::f64::consts::EULER_GAMMA;

/// J_0..J_m(x) for 0 < x ≤ ~100 by Miller's backward recurrence, normalised with
/// J_0 + 2ΣJ_{2k} = 1.
fn bessel_j_all(x: f64) -> Vec<f64> {
    let top = x.ceil() as usize + 30 + (40.0 * x.max(1.0)).sqrt() as usize;
    let m = top + top % 2;
    let mut j = vec![0.0f64; m + 2];
    if let Some(v) = j.get_mut(m) {
        *v = 1e-30;
    }
    for k in (1..=m).rev() {
        let jk = j.get(k).copied().unwrap_or(0.0);
        let jk1 = j.get(k + 1).copied().unwrap_or(0.0);
        let mut prev = 2.0 * k as f64 / x * jk - jk1;
        if prev.abs() > 1e250 {
            for v in j.iter_mut() {
                *v *= 1e-250;
            }
            prev *= 1e-250;
        }
        if let Some(v) = j.get_mut(k - 1) {
            *v = prev;
        }
    }
    let norm: f64 = j.first().copied().unwrap_or(0.0) + 2.0 * j.iter().skip(2).step_by(2).sum::<f64>();
    if norm != 0.0 {
        for v in j.iter_mut() {
            *v /= norm;
        }
    }
    j
}

/// Y_0 and Y_1 by Neumann series over J_k (x ≤ 25) or the asymptotic expansion.
fn bessel_y01(x: f64) -> Option<(f64, f64)> {
    if x > 25.0 {
        let (_, y0) = hankel(0, x)?;
        let (_, y1) = hankel(1, x)?;
        return Some((y0, y1));
    }
    let j = bessel_j_all(x);
    let jj = |k: usize| j.get(k).copied().unwrap_or(0.0);
    let l = (x / 2.0).ln() + EULER;
    // Y0 = (2/π)(L J0) − (4/π) Σ (−1)^k J_{2k}/k
    // Y1 = −Y0' = −(2/π)(J0/x − L J1) + (2/π) Σ (−1)^k (J_{2k−1} − J_{2k+1})/k
    let mut s0 = 0.0;
    let mut s1 = 0.0;
    let mut k = 1usize;
    while 2 * k + 1 < j.len() {
        let sign = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
        s0 += sign * jj(2 * k) / k as f64;
        s1 += sign * (jj(2 * k - 1) - jj(2 * k + 1)) / k as f64;
        k += 1;
    }
    let y0 = (2.0 / PI) * l * jj(0) - (4.0 / PI) * s0;
    let y1 = -(2.0 / PI) * (jj(0) / x - l * jj(1)) + (2.0 / PI) * s1;
    if y0.is_finite() && y1.is_finite() { Some((y0, y1)) } else { None }
}

fn bessel_y(n: u32, x: f64) -> Option<f64> {
    let (y0, y1) = bessel_y01(x)?;
    if n == 0 {
        return Some(y0);
    }
    let (mut a, mut b) = (y0, y1);
    for k in 1..n {
        let c = 2.0 * k as f64 / x * b - a;
        a = b;
        b = c;
        if !b.is_finite() {
            return None;
        }
    }
    Some(b)
}

/// I_n(x) by its power series (all terms positive), computed with a log-scaled first term.
fn bessel_i(n: u32, x: f64) -> Option<f64> {
    if x == 0.0 {
        return Some(if n == 0 { 1.0 } else { 0.0 });
    }
    let ax = x.abs();
    let nf = n as f64;
    let ln_t0 = nf * (ax / 2.0).ln() - crate::special::ln_gamma(nf + 1.0);
    let q = ax * ax / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 0.0;
    while k < 1e6 {
        k += 1.0;
        term *= q / (k * (k + nf));
        sum += term;
        if term < 1e-17 * sum {
            break;
        }
        if !sum.is_finite() {
            return None;
        }
    }
    let r = (ln_t0 + sum.ln()).exp();
    let r = if x < 0.0 && n % 2 == 1 { -r } else { r };
    if r.is_finite() { Some(r) } else { None }
}

/// K_n(x) for x > 0: K_0 and K_1 from ∫₀^∞ e^{−x cosh t} cosh(νt) dt (trapezoid rule), then
/// upward recurrence.
fn bessel_k(n: u32, x: f64) -> Option<f64> {
    let k_int = |nu: f64| -> f64 {
        let h: f64 = 0.02;
        let mut sum = 0.5 * (-x).exp();
        let mut t: f64 = h;
        while t < 800.0 {
            let v = (-x * t.cosh() + (nu * t).ln_cosh_safe()).exp();
            sum += v;
            if v < 1e-18 * sum && x * t.cosh() > 50.0 {
                break;
            }
            t += h;
        }
        sum * h
    };
    let k0 = k_int(0.0);
    if n == 0 {
        return Some(k0);
    }
    let k1 = k_int(1.0);
    let (mut a, mut b) = (k0, k1);
    for k in 1..n {
        let c = a + 2.0 * k as f64 / x * b;
        a = b;
        b = c;
        if !b.is_finite() {
            return None;
        }
    }
    if b.is_finite() { Some(b) } else { None }
}

trait LnCosh {
    fn ln_cosh_safe(self) -> f64;
}

impl LnCosh for f64 {
    /// ln(cosh(x)) without overflow.
    fn ln_cosh_safe(self) -> f64 {
        let a = self.abs();
        a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
    }
}

fn bessel_args(a: &[Arg]) -> R<(f64, u32)> {
    let x = num(a, 0)?;
    let n = num(a, 1)?.trunc();
    if n < 0.0 {
        return Err(CellError::Num);
    }
    if n > 100_000.0 {
        return Err(CellError::Num);
    }
    Ok((x, n as u32))
}

pub(super) fn besselj(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, n) = bessel_args(a)?;
    num_val(bessel_j(n, x).ok_or(CellError::Num)?)
}

pub(super) fn bessely(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, n) = bessel_args(a)?;
    if x <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(bessel_y(n, x).ok_or(CellError::Num)?)
}

pub(super) fn besseli(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, n) = bessel_args(a)?;
    num_val(bessel_i(n, x).ok_or(CellError::Num)?)
}

pub(super) fn besselk(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, n) = bessel_args(a)?;
    if x <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(bessel_k(n, x).ok_or(CellError::Num)?)
}
