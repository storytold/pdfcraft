use super::descriptive::{arg_nums, arg_pairs, mean, var_s};
use super::{
    Arg, C, CellError, R, Value, arg, as_array, boolean, erf, gamma, has, invert, ln_beta, ln_gamma, norm_cdf, norm_inv, norm_pdf, num, num_val,
    reg_gamma_p, reg_gamma_q, reg_gamma_q_front, reg_inc_beta, reg_inc_beta_front,
};

// ---------------------------------------------------------------------------------------------
// Distributions
// ---------------------------------------------------------------------------------------------

fn ln_comb(n: f64, k: f64) -> f64 {
    ln_gamma(n + 1.0) - ln_gamma(k + 1.0) - ln_gamma(n - k + 1.0)
}

fn t_upper(t: f64, v: f64) -> f64 {
    // P(T > t) for t ≥ 0.
    0.5 * reg_inc_beta(v / (v + t * t), v / 2.0, 0.5)
}

fn t_cdf(t: f64, v: f64) -> f64 {
    if t >= 0.0 { 1.0 - t_upper(t, v) } else { t_upper(-t, v) }
}

fn t_pdf(t: f64, v: f64) -> f64 {
    let z = v / 2.0;
    let log_gamma_ratio = if z >= 16.0 {
        let inv_z = 1.0 / z;
        0.5 * z.ln() - inv_z / 8.0 + inv_z.powi(3) / 192.0 - inv_z.powi(5) / 640.0 + 17.0 * inv_z.powi(7) / 14_336.0
    } else {
        ln_gamma(z + 0.5) - ln_gamma(z)
    };
    (log_gamma_ratio - 0.5 * (v.ln() + std::f64::consts::PI.ln()) - (v + 1.0) / 2.0 * (t * t / v).ln_1p()).exp()
}

/// t ≥ 0 with P(T > t) = q, q in (0, 0.5].
fn t_inv_upper(q: f64, v: f64) -> f64 {
    if q >= 0.5 {
        return 0.0;
    }
    invert(|t| t_upper(t, v), q, 0.0, 1.0, 1e60)
}

fn chi_inv(p: f64, v: f64) -> f64 {
    if p <= 0.0 {
        return 0.0;
    }
    invert(|x| reg_gamma_p(v / 2.0, x / 2.0), p, 0.0, v.max(1.0), 1e60)
}

fn chi_inv_rt(q: f64, v: f64) -> f64 {
    if q >= 1.0 {
        return 0.0;
    }
    invert(|x| reg_gamma_q(v / 2.0, x / 2.0), q, 0.0, v.max(1.0), 1e60)
}

fn f_cdf(x: f64, d1: f64, d2: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x == 1.0 && d1 == d2 {
        return 0.5;
    }
    if d1 >= 1e8 && d2 / d1 <= 1e-8 {
        return reg_gamma_q(d2 / 2.0, d2 / (2.0 * (x + d2 / d1)));
    }
    reg_inc_beta(d1 * x / (d1 * x + d2), d1 / 2.0, d2 / 2.0)
}

fn f_upper(x: f64, d1: f64, d2: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    if d1 >= 1e8 && d2 / d1 <= 1e-8 {
        // With a much larger numerator degree, the scaled beta tail converges to a gamma tail.
        return reg_gamma_p(d2 / 2.0, d2 / (2.0 * (x + d2 / d1)));
    }
    reg_inc_beta(d2 / (d2 + d1 * x), d2 / 2.0, d1 / 2.0)
}

fn f_pdf(x: f64, d1: f64, d2: f64) -> f64 {
    if x == 0.0 {
        return if d1 < 2.0 {
            f64::INFINITY
        } else if d1 == 2.0 {
            1.0
        } else {
            0.0
        };
    }
    ((d1 / 2.0) * (d1 / d2).ln() + (d1 / 2.0 - 1.0) * x.ln() - ((d1 + d2) / 2.0) * (1.0 + d1 * x / d2).ln() - ln_beta(d1 / 2.0, d2 / 2.0)).exp()
}

const MAX_BINOM_TRIALS: f64 = 2_147_483_646.0;

/// Stirling-series error `ln n! - ((n + 1/2) ln n - n + ln(2π)/2)` for integer `n >= 1`: exact
/// table (computed with 60-digit Decimal arithmetic) up to 30, then the series, whose first
/// omitted term is below 1e-19 there.
fn stirlerr(n: f64) -> f64 {
    const TABLE: [f64; 31] = [
        0.0,
        0.08106146679532726,
        0.0413406959554093,
        0.02767792568499834,
        0.020790672103765093,
        0.016644691189821193,
        0.013876128823070748,
        0.01189670994589177,
        0.010411265261972096,
        0.009255462182712733,
        0.00833056343336287,
        0.007573675487951841,
        0.00694284010720953,
        0.006408994188004207,
        0.0059513701127588475,
        0.005554733551962801,
        0.0052076559196096404,
        0.004901395948434738,
        0.004629153749334028,
        0.004385560249232324,
        0.004166319691996922,
        0.00396795421864086,
        0.0037876180684444346,
        0.0036229602246830948,
        0.003472021382978767,
        0.003333155636728093,
        0.003204970228055038,
        0.0030862786826087773,
        0.002976063983550409,
        0.0028734493623524663,
        0.0027776749297526936,
    ];
    if n <= 30.0 {
        return TABLE.get(n as usize).copied().unwrap_or(0.0);
    }
    let r = 1.0 / n;
    let r2 = r * r;
    r * (1.0 / 12.0 - r2 * (1.0 / 360.0 - r2 * (1.0 / 1260.0 - r2 * (1.0 / 1680.0 - r2 * (1.0 / 1188.0)))))
}

/// `x ln(x/np) + np - x` without the cancellation of the direct formula when `x` is near `np`
/// (Loader, 2000).
fn bd0(x: f64, np: f64) -> f64 {
    if (x - np).abs() < 0.1 * (x + np) {
        let v = (x - np) / (x + np);
        let mut s = (x - np) * v;
        if s.abs() < f64::MIN_POSITIVE {
            return s;
        }
        let mut ej = 2.0 * x * v;
        let v2 = v * v;
        for j in 1..1000 {
            ej *= v2;
            let s1 = s + ej / f64::from(2 * j + 1);
            if s1 == s {
                return s1;
            }
            s = s1;
        }
    }
    x * (x / np).ln() + np - x
}

/// `(1 - p)^e` with the rounding error of `1 - p` (it is not exact for `p < 1/2`) folded back in,
/// which `e` would otherwise multiply.
fn pow_one_minus(p: f64, e: f64) -> f64 {
    let hi = 1.0 - p;
    let lo = (1.0 - hi) - p;
    let r = hi.powf(e) * (e * (lo / hi).ln_1p()).exp();
    // An exponent so large that the two factors under- and overflow: the plain form is enough.
    if r.is_nan() { (e * (-p).ln_1p()).exp() } else { r }
}

/// `C(n, k)` for a small `k` as a product of `k` ratios.
fn choose_small(n: f64, k: f64) -> f64 {
    let mut c = 1.0;
    let mut j = 0.0;
    while j < k {
        c *= (n - j) / (j + 1.0);
        j += 1.0;
    }
    c
}

/// Binomial mass at integer `k`, `0 <= k <= n`, by Loader's saddle-point formula: no log-gamma
/// of large numbers, so the relative error stays near 1e-15 for any `n`.
fn binom_pmf(k: f64, n: f64, p: f64) -> f64 {
    if k < 0.0 || k > n {
        return 0.0;
    }
    if p == 0.0 {
        return if k == 0.0 { 1.0 } else { 0.0 };
    }
    if p == 1.0 {
        return if k == n { 1.0 } else { 0.0 };
    }
    if k == 0.0 {
        return if n == 0.0 { 1.0 } else { pow_one_minus(p, n) };
    }
    if k == n {
        return p.powf(n);
    }
    // A few successes or a few failures: the product form has no large cancelling terms.
    // (Not for astronomically large n, or when either power underflows: the
    // saddle-point form below handles those through logs.)
    if n < 1e30 && p.powf(k) > 1e-280 {
        if k < 16.0 {
            let rest = pow_one_minus(p, n - k);
            if rest >= f64::MIN_POSITIVE {
                return choose_small(n, k) * p.powi(k as i32) * rest;
            }
        }
        if n - k < 16.0 {
            return choose_small(n, n - k) * p.powf(k) * pow_one_minus(p, n - k);
        }
    }
    let q = 1.0 - p;
    let lc = stirlerr(n) - stirlerr(k) - stirlerr(n - k) - bd0(k, n * p) - bd0(n - k, n * q);
    let lf = std::f64::consts::TAU.ln() + k.ln() + (-k / n).ln_1p();
    (lc - 0.5 * lf).exp()
}

/// Sum of the binomial masses 0..=k for `n <= 10_000`: starts at the term nearest the mode
/// (accurate by the formula above) and walks away from it by the ratio of neighbouring masses,
/// one multiply per term, stopping once the terms no longer matter. Above the mode it sums the
/// upper tail instead and subtracts it from 1.
fn binom_cdf_sum(k: f64, n: f64, p: f64) -> f64 {
    let q = 1.0 - p;
    if p == 0.0 {
        return 1.0;
    }
    if p == 1.0 {
        return 0.0;
    }
    if k <= n * p {
        // mass(i - 1) = mass(i) * i / (n - i + 1) * q / p
        let mut t = binom_pmf(k, n, p);
        let mut s = t;
        let mut i = k;
        while i >= 1.0 && t > s * 1e-18 {
            t *= i / (n - i + 1.0) * (q / p);
            s += t;
            i -= 1.0;
        }
        s.min(1.0)
    } else {
        // mass(i + 1) = mass(i) * (n - i) / (i + 1) * p / q
        let mut t = binom_pmf(k + 1.0, n, p);
        let mut s = t;
        let mut i = k + 1.0;
        while i < n && t > s * 1e-18 {
            t *= (n - i) / (i + 1.0) * (p / q);
            s += t;
            i += 1.0;
        }
        (1.0 - s).max(0.0)
    }
}

fn binom_cdf(k: f64, n: f64, p: f64) -> f64 {
    if k < 0.0 {
        return 0.0;
    }
    if k >= n {
        return 1.0;
    }
    if n <= 10_000.0 {
        return binom_cdf_sum(k, n, p);
    }
    if p == 0.0 {
        return 1.0;
    }
    if p == 1.0 {
        return 0.0;
    }
    // I_(1-p)(n-k, k+1) has prefactor (n-k) p mass(k).
    reg_inc_beta_front(1.0 - p, n - k, k + 1.0, (n - k) * p * binom_pmf(k, n, p))
}

/// Poisson mass at integer `x >= 0` by Loader's formula.
fn poisson_pmf(x: f64, m: f64) -> f64 {
    if m == 0.0 {
        return if x == 0.0 { 1.0 } else { 0.0 };
    }
    if x == 0.0 {
        return (-m).exp();
    }
    (-stirlerr(x) - bd0(x, m)).exp() / (std::f64::consts::TAU * x).sqrt()
}

fn poisson_cdf(x: f64, m: f64) -> f64 {
    if m == 0.0 {
        return 1.0;
    }
    if x <= 1000.0 {
        if x <= m {
            // mass(i - 1) = mass(i) * i / m, downwards from x while the terms matter
            let mut t = poisson_pmf(x, m);
            let mut s = t;
            let mut i = x;
            while i >= 1.0 && t > s * 1e-18 {
                t *= i / m;
                s += t;
                i -= 1.0;
            }
            return s.min(1.0);
        }
        // Above the mean: 1 minus the upper tail, mass(i + 1) = mass(i) * m / (i + 1).
        let mut t = poisson_pmf(x + 1.0, m);
        let mut s = t;
        let mut i = x + 1.0;
        while t > s * 1e-18 {
            t *= m / (i + 1.0);
            s += t;
            i += 1.0;
        }
        return (1.0 - s).max(0.0);
    }
    // Q(x+1, m) has prefactor m mass(x).
    reg_gamma_q_front(x + 1.0, m, m * poisson_pmf(x, m))
}

pub(super) fn norm_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, m, sd, cum) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, boolean(a, 3)?);
    if sd <= 0.0 {
        return Err(CellError::Num);
    }
    let z = (x - m) / sd;
    num_val(if cum { norm_cdf(z) } else { norm_pdf(z) / sd })
}

pub(super) fn norm_inv_fn(a: &[Arg], _: C) -> R<Value> {
    let (p, m, sd) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if !(p > 0.0 && p < 1.0) || sd <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(m + sd * norm_inv(p))
}

pub(super) fn norm_s_dist(a: &[Arg], _: C) -> R<Value> {
    let z = num(a, 0)?;
    let cum = if a.len() > 1 { boolean(a, 1)? } else { true };
    num_val(if cum { norm_cdf(z) } else { norm_pdf(z) })
}

pub(super) fn norm_s_inv(a: &[Arg], _: C) -> R<Value> {
    let p = num(a, 0)?;
    if !(p > 0.0 && p < 1.0) {
        return Err(CellError::Num);
    }
    num_val(norm_inv(p))
}

fn df_arg(a: &[Arg], i: usize) -> R<f64> {
    let v = num(a, i)?.trunc();
    if v < 1.0 {
        return Err(CellError::Num);
    }
    Ok(v)
}

pub(super) fn t_dist(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let v = num(a, 1)?.trunc();
    let cum = boolean(a, 2)?;
    if v == 0.0 {
        return Err(if cum { CellError::Num } else { CellError::Div0 });
    }
    if v < 0.0 {
        return Err(CellError::Num);
    }
    num_val(if cum { t_cdf(x, v) } else { t_pdf(x, v) })
}

pub(super) fn t_dist_2t(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let v = df_arg(a, 1)?;
    if x < 0.0 {
        return Err(CellError::Num);
    }
    num_val(2.0 * t_upper(x, v))
}

pub(super) fn t_dist_rt(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let v = df_arg(a, 1)?;
    num_val(1.0 - t_cdf(x, v))
}

pub(super) fn tdist_legacy(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let v = df_arg(a, 1)?;
    let tails = num(a, 2)?.trunc();
    if x < 0.0 || !(tails == 1.0 || tails == 2.0) {
        return Err(CellError::Num);
    }
    num_val(tails * t_upper(x, v))
}

pub(super) fn t_inv(a: &[Arg], _: C) -> R<Value> {
    let p = num(a, 0)?;
    let v = df_arg(a, 1)?;
    if p == 0.0 {
        return Err(CellError::Div0);
    }
    if !(p > 0.0 && p < 1.0) {
        return Err(CellError::Num);
    }
    num_val(if p < 0.5 { -t_inv_upper(p, v) } else { t_inv_upper(1.0 - p, v) })
}

pub(super) fn t_inv_2t(a: &[Arg], _: C) -> R<Value> {
    let p = num(a, 0)?;
    let v = df_arg(a, 1)?;
    if !(p > 0.0 && p <= 1.0) {
        return Err(CellError::Num);
    }
    num_val(t_inv_upper(p / 2.0, v))
}

pub(super) fn t_test(a: &[Arg], _: C) -> R<Value> {
    let tails = num(a, 2)?.trunc();
    let ty = num(a, 3)?.trunc();
    if !(tails == 1.0 || tails == 2.0) || !(1.0..=3.0).contains(&ty) {
        return Err(CellError::Num);
    }
    let (t, v) = if ty == 1.0 {
        let (x, y) = arg_pairs(a, 0, 1)?;
        let d: Vec<f64> = x.iter().zip(&y).map(|(p, q)| p - q).collect();
        let n = d.len() as f64;
        let sd = var_s(&d)?.sqrt();
        if sd == 0.0 {
            return Err(CellError::Div0);
        }
        (mean(&d)? / (sd / n.sqrt()), n - 1.0)
    } else {
        let x = arg_nums(a, 0)?;
        let y = arg_nums(a, 1)?;
        let (n1, n2) = (x.len() as f64, y.len() as f64);
        let (v1, v2) = (var_s(&x)?, var_s(&y)?);
        let diff = mean(&x)? - mean(&y)?;
        if ty == 2.0 {
            let df = n1 + n2 - 2.0;
            let sp = ((n1 - 1.0) * v1 + (n2 - 1.0) * v2) / df;
            let se = (sp * (1.0 / n1 + 1.0 / n2)).sqrt();
            if se == 0.0 {
                return Err(CellError::Div0);
            }
            (diff / se, df)
        } else {
            let (a1, a2) = (v1 / n1, v2 / n2);
            let se = (a1 + a2).sqrt();
            if se == 0.0 {
                return Err(CellError::Div0);
            }
            let df = (a1 + a2).powi(2) / (a1 * a1 / (n1 - 1.0) + a2 * a2 / (n2 - 1.0));
            (diff / se, df)
        }
    };
    num_val(tails * t_upper(t.abs(), v))
}

pub(super) fn chisq_dist(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let v = df_arg(a, 1)?;
    let cum = boolean(a, 2)?;
    if x < 0.0 || v > 1e10 {
        return Err(CellError::Num);
    }
    if cum {
        return num_val(reg_gamma_p(v / 2.0, x / 2.0));
    }
    if x == 0.0 {
        return if v == 2.0 {
            num_val(0.5)
        } else if v < 2.0 {
            Err(CellError::Num)
        } else {
            num_val(0.0)
        };
    }
    num_val(((v / 2.0 - 1.0) * x.ln() - x / 2.0 - (v / 2.0) * 2f64.ln() - ln_gamma(v / 2.0)).exp())
}

pub(super) fn chisq_dist_rt(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let v = df_arg(a, 1)?;
    if x < 0.0 || v > 1e10 {
        return Err(CellError::Num);
    }
    num_val(reg_gamma_q(v / 2.0, x / 2.0))
}

pub(super) fn chisq_inv(a: &[Arg], _: C) -> R<Value> {
    let p = num(a, 0)?;
    let v = df_arg(a, 1)?;
    if !(0.0..1.0).contains(&p) || v > 1e10 {
        return Err(CellError::Num);
    }
    num_val(chi_inv(p, v))
}

pub(super) fn chisq_inv_rt(a: &[Arg], _: C) -> R<Value> {
    let p = num(a, 0)?;
    let v = df_arg(a, 1)?;
    if !(p > 0.0 && p <= 1.0) || v > 1e10 {
        return Err(CellError::Num);
    }
    num_val(chi_inv_rt(p, v))
}

pub(super) fn chisq_test(a: &[Arg], _: C) -> R<Value> {
    let act = as_array(&arg(a, 0)?.value).into_owned();
    let exp = as_array(&arg(a, 1)?.value).into_owned();
    if act.rows != exp.rows || act.cols != exp.cols {
        return Err(CellError::NA);
    }
    let mut x2 = 0.0;
    for (o, e) in act.data.iter().zip(&exp.data) {
        if let Value::Error(err) = o {
            return Err(*err);
        }
        if let Value::Error(err) = e {
            return Err(*err);
        }
        if let (Value::Number(o), Value::Number(e)) = (o, e) {
            if *e == 0.0 {
                return Err(CellError::Div0);
            }
            x2 += (o - e).powi(2) / e;
        }
    }
    let df = if act.rows > 1 && act.cols > 1 { ((act.rows - 1) * (act.cols - 1)) as f64 } else { (act.data.len().saturating_sub(1)) as f64 };
    if df < 1.0 {
        return Err(CellError::NA);
    }
    num_val(reg_gamma_q(df / 2.0, x2 / 2.0))
}

fn f_args(a: &[Arg]) -> R<(f64, f64, f64)> {
    let x = num(a, 0)?;
    let d1 = num(a, 1)?.trunc();
    let d2 = num(a, 2)?.trunc();
    if d1 < 1.0 || d2 < 1.0 || d1 > 1e10 || d2 > 1e10 {
        return Err(CellError::Num);
    }
    Ok((x, d1, d2))
}

pub(super) fn f_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, d1, d2) = f_args(a)?;
    let cum = boolean(a, 3)?;
    if x < 0.0 {
        return Err(CellError::Num);
    }
    num_val(if cum { f_cdf(x, d1, d2) } else { f_pdf(x, d1, d2) })
}

pub(super) fn f_dist_rt(a: &[Arg], _: C) -> R<Value> {
    let (x, d1, d2) = f_args(a)?;
    if x < 0.0 {
        return Err(CellError::Num);
    }
    num_val(f_upper(x, d1, d2))
}

pub(super) fn f_inv(a: &[Arg], _: C) -> R<Value> {
    let (p, d1, d2) = f_args(a)?;
    if !(0.0..1.0).contains(&p) {
        return Err(CellError::Num);
    }
    if p == 0.0 {
        return num_val(0.0);
    }
    let value = if p < 0.5 { invert(|x| f_cdf(x, d1, d2), p, 0.0, 1.0, 1e60) } else { invert(|x| f_upper(x, d1, d2), 1.0 - p, 0.0, 1.0, 1e60) };
    num_val(value)
}

pub(super) fn f_inv_rt(a: &[Arg], _: C) -> R<Value> {
    let (p, d1, d2) = f_args(a)?;
    if !(p > 0.0 && p <= 1.0) {
        return Err(CellError::Num);
    }
    if p == 1.0 {
        return num_val(0.0);
    }
    num_val(invert(|x| f_upper(x, d1, d2), p, 0.0, 1.0, 1e60))
}

pub(super) fn f_test(a: &[Arg], _: C) -> R<Value> {
    let x = arg_nums(a, 0)?;
    let y = arg_nums(a, 1)?;
    let (v1, v2) = (var_s(&x)?, var_s(&y)?);
    if v1 == 0.0 || v2 == 0.0 {
        return Err(CellError::Div0);
    }
    let f = v1 / v2;
    let (d1, d2) = ((x.len() - 1) as f64, (y.len() - 1) as f64);
    let c = f_cdf(f, d1, d2);
    num_val((2.0 * c.min(1.0 - c)).min(1.0))
}

pub(super) fn binom_dist(a: &[Arg], _: C) -> R<Value> {
    let k = num(a, 0)?.trunc();
    let n = num(a, 1)?.trunc();
    let p = num(a, 2)?;
    let cum = boolean(a, 3)?;
    if k < 0.0 || n > MAX_BINOM_TRIALS || k > n || !(0.0..=1.0).contains(&p) {
        return Err(CellError::Num);
    }
    num_val(if cum { binom_cdf(k, n, p) } else { binom_pmf(k, n, p) })
}

pub(super) fn binom_dist_range(a: &[Arg], _: C) -> R<Value> {
    let n = num(a, 0)?.trunc();
    let p = num(a, 1)?;
    let s1 = num(a, 2)?.trunc();
    let s2 = if has(a, 3) { num(a, 3)?.trunc() } else { s1 };
    if !(0.0..=MAX_BINOM_TRIALS).contains(&n) || !(0.0..=1.0).contains(&p) || s1 < 0.0 || s1 > n || s2 < s1 || s2 > n {
        return Err(CellError::Num);
    }
    if s2 - s1 > 10_000.0 {
        return num_val(binom_cdf(s2, n, p) - binom_cdf(s1 - 1.0, n, p));
    }
    let mut s = 0.0;
    let mut k = s1;
    while k <= s2 {
        s += binom_pmf(k, n, p);
        k += 1.0;
    }
    num_val(s)
}

pub(super) fn binom_inv(a: &[Arg], _: C) -> R<Value> {
    let n = num(a, 0)?.trunc();
    let p = num(a, 1)?;
    let alpha = num(a, 2)?;
    if n < 0.0 || !(0.0..=1.0).contains(&p) || !(0.0..=1.0).contains(&alpha) {
        return Err(CellError::Num);
    }
    // Smallest k with cdf(k) ≥ alpha, by bisection on the integer k.
    let (mut lo, mut hi) = (0.0f64, n);
    if binom_cdf(0.0, n, p) >= alpha {
        return num_val(0.0);
    }
    while hi - lo > 1.0 {
        let mid = ((lo + hi) / 2.0).floor();
        // Above 2^53 adjacent f64 values are more than 1 apart, so the gap can stop shrinking.
        if mid <= lo || mid >= hi {
            break;
        }
        if binom_cdf(mid, n, p) >= alpha {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    num_val(hi)
}

pub(super) fn poisson_dist(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?.trunc();
    let m = num(a, 1)?;
    let cum = boolean(a, 2)?;
    if x < 0.0 || m < 0.0 {
        return Err(CellError::Num);
    }
    num_val(if cum { poisson_cdf(x, m) } else { poisson_pmf(x, m) })
}

pub(super) fn expon_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, l, cum) = (num(a, 0)?, num(a, 1)?, boolean(a, 2)?);
    if x < 0.0 || l <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(if cum { -(-l * x).exp_m1() } else { l * (-l * x).exp() })
}

pub(super) fn gamma_fn(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let g = gamma(x);
    if !g.is_finite() {
        return Err(CellError::Num);
    }
    num_val(g)
}

pub(super) fn gamma_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, al, be, cum) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, boolean(a, 3)?);
    if x < 0.0 || al <= 0.0 || be <= 0.0 {
        return Err(CellError::Num);
    }
    if cum {
        return num_val(reg_gamma_p(al, x / be));
    }
    if x == 0.0 {
        return if al <= 1.0 { Err(CellError::Num) } else { num_val(0.0) };
    }
    num_val(((al - 1.0) * x.ln() - x / be - al * be.ln() - ln_gamma(al)).exp())
}

pub(super) fn gamma_inv(a: &[Arg], _: C) -> R<Value> {
    let (p, al, be) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if !(0.0..1.0).contains(&p) || al <= 0.0 || be <= 0.0 {
        return Err(CellError::Num);
    }
    if p == 0.0 {
        return num_val(0.0);
    }
    num_val(invert(|x| reg_gamma_p(al, x / be), p, 0.0, (al * be).max(1e-3), 1e60))
}

pub(super) fn gammaln(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    if x <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(ln_gamma(x))
}

fn beta_bounds(a: &[Arg], i: usize) -> R<(f64, f64)> {
    let lo = if has(a, i) { num(a, i)? } else { 0.0 };
    let hi = if has(a, i + 1) { num(a, i + 1)? } else { 1.0 };
    if lo >= hi {
        return Err(CellError::Num);
    }
    Ok((lo, hi))
}

pub(super) fn beta_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, al, be, cum) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, boolean(a, 3)?);
    let (lo, hi) = beta_bounds(a, 4)?;
    if al <= 0.0 || be <= 0.0 || x < lo || x > hi {
        return Err(CellError::Num);
    }
    let z = (x - lo) / (hi - lo);
    if cum {
        return num_val(reg_inc_beta(z, al, be));
    }
    if (z == 0.0 && al <= 1.0) || (z == 1.0 && be <= 1.0) {
        return Err(CellError::Num);
    }
    let lp = if z == 0.0 || z == 1.0 {
        let base = if z == 0.0 { al } else { be };
        if base == 1.0 { -ln_beta(al, be) } else { f64::NEG_INFINITY }
    } else {
        (al - 1.0) * z.ln() + (be - 1.0) * (1.0 - z).ln() - ln_beta(al, be)
    };
    num_val(lp.exp() / (hi - lo))
}

pub(super) fn betadist_legacy(a: &[Arg], _: C) -> R<Value> {
    let (x, al, be) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let (lo, hi) = beta_bounds(a, 3)?;
    if al <= 0.0 || be <= 0.0 || x < lo || x > hi {
        return Err(CellError::Num);
    }
    num_val(reg_inc_beta((x - lo) / (hi - lo), al, be))
}

pub(super) fn beta_inv(a: &[Arg], _: C) -> R<Value> {
    let (p, al, be) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let (lo, hi) = beta_bounds(a, 3)?;
    if al <= 0.0 || be <= 0.0 || !(0.0..=1.0).contains(&p) {
        return Err(CellError::Num);
    }
    let z = if p == 0.0 {
        0.0
    } else if p == 1.0 {
        1.0
    } else {
        invert(|x| reg_inc_beta(x, al, be), p, 0.0, 1.0, 1.0)
    };
    num_val(lo + z * (hi - lo))
}

pub(super) fn lognorm_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, m, sd) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let cum = if a.len() > 3 { boolean(a, 3)? } else { true };
    if x <= 0.0 || sd <= 0.0 {
        return Err(CellError::Num);
    }
    let z = (x.ln() - m) / sd;
    num_val(if cum { norm_cdf(z) } else { norm_pdf(z) / (x * sd) })
}

pub(super) fn lognorm_inv(a: &[Arg], _: C) -> R<Value> {
    let (p, m, sd) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if !(p > 0.0 && p < 1.0) || sd <= 0.0 {
        return Err(CellError::Num);
    }
    num_val((m + sd * norm_inv(p)).exp())
}

pub(super) fn weibull_dist(a: &[Arg], _: C) -> R<Value> {
    let (x, al, be, cum) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, boolean(a, 3)?);
    if x < 0.0 || al <= 0.0 || be <= 0.0 {
        return Err(CellError::Num);
    }
    if x == 0.0 {
        return num_val(0.0);
    }
    let t = (x / be).powf(al);
    num_val(if cum { -(-t).exp_m1() } else { al / be.powf(al) * x.powf(al - 1.0) * (-t).exp() })
}

pub(super) fn hypgeom_dist(a: &[Arg], _: C) -> R<Value> {
    let k = num(a, 0)?.trunc();
    let n = num(a, 1)?.trunc();
    let kk = num(a, 2)?.trunc();
    let nn = num(a, 3)?.trunc();
    let cum = if a.len() > 4 { boolean(a, 4)? } else { false };
    if k < 0.0 || n <= 0.0 || kk <= 0.0 || nn <= 0.0 || n > nn || kk > nn || k > n.min(kk) || k < (n - nn + kk).max(0.0) {
        return Err(CellError::Num);
    }
    let pmf = |i: f64| (ln_comb(kk, i) + ln_comb(nn - kk, n - i) - ln_comb(nn, n)).exp();
    if !cum {
        return num_val(pmf(k));
    }
    let mut s = 0.0;
    let mut i = (n - nn + kk).max(0.0);
    if k - i > 1e6 || i + 1.0 == i {
        return Err(CellError::Num);
    }
    while i <= k {
        s += pmf(i);
        i += 1.0;
    }
    num_val(s.min(1.0))
}

pub(super) fn negbinom_dist(a: &[Arg], _: C) -> R<Value> {
    let f = num(a, 0)?.trunc();
    let s = num(a, 1)?.trunc();
    let p = num(a, 2)?;
    let cum = if a.len() > 3 { boolean(a, 3)? } else { false };
    if f < 0.0 || s < 1.0 || !(0.0..=1.0).contains(&p) {
        return Err(CellError::Num);
    }
    if cum {
        if p == 0.0 {
            return num_val(0.0);
        }
        return num_val(reg_inc_beta(p, s, f + 1.0));
    }
    if p == 0.0 || p == 1.0 {
        return num_val(if p == 1.0 && f == 0.0 { 1.0 } else { 0.0 });
    }
    num_val((ln_comb(f + s - 1.0, s - 1.0) + s * p.ln() + f * (1.0 - p).ln()).exp())
}

pub(super) fn confidence_norm(a: &[Arg], _: C) -> R<Value> {
    let (al, sd, n) = (num(a, 0)?, num(a, 1)?, num(a, 2)?.trunc());
    if !(al > 0.0 && al < 1.0) || sd <= 0.0 || n < 1.0 {
        return Err(CellError::Num);
    }
    num_val(norm_inv(1.0 - al / 2.0) * sd / n.sqrt())
}

pub(super) fn confidence_t(a: &[Arg], _: C) -> R<Value> {
    let (al, sd, n) = (num(a, 0)?, num(a, 1)?, num(a, 2)?.trunc());
    if !(al > 0.0 && al < 1.0) || sd <= 0.0 || n < 1.0 {
        return Err(CellError::Num);
    }
    if n == 1.0 {
        return Err(CellError::Div0);
    }
    num_val(t_inv_upper(al / 2.0, n - 1.0) * sd / n.sqrt())
}

pub(super) fn z_test(a: &[Arg], _: C) -> R<Value> {
    let v = arg_nums(a, 0)?;
    let x = num(a, 1)?;
    if v.is_empty() {
        return Err(CellError::NA);
    }
    let sd = if has(a, 2) { num(a, 2)? } else { var_s(&v)?.sqrt() };
    if sd <= 0.0 {
        return Err(if has(a, 2) { CellError::Num } else { CellError::Div0 });
    }
    let z = (mean(&v)? - x) / (sd / (v.len() as f64).sqrt());
    num_val(1.0 - norm_cdf(z))
}

pub(super) fn fisher(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    if x <= -1.0 || x >= 1.0 {
        return Err(CellError::Num);
    }
    num_val(0.5 * ((1.0 + x) / (1.0 - x)).ln())
}

pub(super) fn gauss(a: &[Arg], _: C) -> R<Value> {
    let z = num(a, 0)?;
    // Φ(z) − 0.5 = erf(z/√2)/2, accurate near zero.
    num_val(0.5 * erf(z / std::f64::consts::SQRT_2))
}
