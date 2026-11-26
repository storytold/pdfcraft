//! Math & trigonometry functions, plus the criteria aggregates (SUMIF(S), COUNTIF(S),
//! AVERAGEIF(S), MAXIFS, MINIFS).

use std::f64::consts::PI;

use gridcraft_core::{Array, CellError, Value};

use crate::criteria::Criterion;
use crate::util::{
    A, MAX_CELLS, R, S, array_val, as_array, check_size, has, num, num_val, numbers, opt_num, round_half_away, text, text_val, to_sig_digits,
};
use crate::{Arg, Ctx, FnSpec, VAR};

mod criteria;
mod matrix;
mod roman_base;

use criteria::{averageif, averageifs, countif, countifs, maxifs, minifs, sumif, sumifs};
use matrix::{mdeterm, minverse, mmult, munit};
use roman_base::{arabic, base, decimal, roman};

// ---------------------------------------------------------------------------------------------
// Lifting tables

const fn alt_table(first_crit: usize) -> [bool; 256] {
    // Criteria positions: first_crit, first_crit + 2, …
    let mut t = [false; 256];
    let mut i = first_crit;
    while i < 256 {
        t[i] = true;
        i += 2;
    }
    t
}
/// COUNTIFS(range, crit, range, crit, …)
static COUNTIFS_SCAL: [bool; 256] = alt_table(1);
/// SUMIFS(sum_range, range, crit, …)
static SUMIFS_SCAL: [bool; 256] = alt_table(2);
const IF2: &[bool] = &[false, true, false];

// ---------------------------------------------------------------------------------------------
// Small helpers

const TRIG_LIMIT: f64 = 134_217_728.0; // 2^27: Excel's argument limit for trig functions

fn unary(a: &[Arg], f: impl Fn(f64) -> R<f64>) -> R<Value> {
    num_val(f(num(a, 0)?)?)
}

fn trig_arg(x: f64) -> R<f64> {
    if x.abs() >= TRIG_LIMIT { Err(CellError::Num) } else { Ok(x) }
}

fn nonzero(x: f64) -> R<f64> {
    if x == 0.0 { Err(CellError::Div0) } else { Ok(x) }
}

/// Removes binary noise before ceil/floor (e.g. 2.0000000000000004 → 2).
fn clean(x: f64) -> f64 {
    to_sig_digits(x, 15)
}

fn digits_arg(a: &[Arg], i: usize) -> R<i32> {
    let d = opt_num(a, i, 0.0)?.trunc();
    Ok(d.clamp(-400.0, 400.0) as i32)
}

/// Rounds |x| scaled by 10^digits with `op` (ceil / floor), keeping sign.
fn round_dir(x: f64, digits: i32, up: bool) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let ax = clean(x.abs());
    let (scaled, f, neg) = if digits >= 0 {
        let f = 10f64.powi(digits);
        (ax * f, f, false)
    } else {
        let f = 10f64.powi(-digits);
        (ax / f, f, true)
    };
    if !scaled.is_finite() || scaled >= 1e17 {
        return x;
    }
    let s = clean(scaled);
    let r = if up { s.ceil() } else { s.floor() };
    // Toward zero, r == 0 with an infinite scale would be 0 * inf = NaN (#NUM!); the result is simply
    // zero. Away from zero the true result (10^309 or more) is not a number, so ROUNDUP stays #NUM!.
    let out = if !neg {
        clean(r / f)
    } else if r == 0.0 && !up {
        0.0
    } else {
        r * f
    };
    if x < 0.0 { -out } else { out }
}

// ---------------------------------------------------------------------------------------------
// Sums and products

fn sum(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(numbers(a)?.iter().sum())
}

/// PERCENTOF(data_subset, data_all) = SUM(data_subset) / SUM(data_all).
fn percentof(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let part = a.get(..1).ok_or(CellError::Value)?;
    let all = a.get(1..2).ok_or(CellError::Value)?;
    let (part, all): (f64, f64) = (numbers(part)?.iter().sum(), numbers(all)?.iter().sum());
    if all == 0.0 {
        return Err(CellError::Div0);
    }
    num_val(part / all)
}

fn sumsq(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(numbers(a)?.iter().map(|x| x * x).sum())
}

fn product(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = numbers(a)?;
    if v.is_empty() {
        return Ok(Value::Number(0.0));
    }
    num_val(v.iter().product())
}

/// SUMPRODUCT: non-numeric entries of arrays and multi-cell ranges count as 0, but a scalar
/// argument holding text (a literal or a single-cell reference) is `#VALUE!` (observed:
/// `SUMPRODUCT(G1330:G1330,I1330:I1330)` with G1330 = the text "4" is `#VALUE!`).
fn sumproduct(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let arrays: Vec<_> = a.iter().map(|x| (as_array(&x.value), !matches!(x.value, Value::Array(_)))).collect();
    let Some((first, _)) = arrays.first() else { return Err(CellError::Value) };
    let (rows, cols) = (first.rows, first.cols);
    if arrays.iter().any(|(x, _)| x.rows != rows || x.cols != cols) {
        return Err(CellError::Value);
    }
    let mut total = 0.0;
    for i in 0..rows.saturating_mul(cols) {
        let mut p = 1.0;
        for (arr, scalar) in &arrays {
            match arr.data.get(i) {
                Some(Value::Number(n)) => p *= n,
                Some(Value::Error(e)) => return Err(*e),
                Some(Value::Text(_)) if *scalar => return Err(CellError::Value),
                _ => p = 0.0,
            }
        }
        total += p;
    }
    num_val(total)
}

fn pair_sum(a: &[Arg], f: fn(f64, f64) -> f64) -> R<Value> {
    let x = as_array(&crate::util::arg(a, 0)?.value);
    let y = as_array(&crate::util::arg(a, 1)?.value);
    if x.data.len() != y.data.len() {
        return Err(CellError::NA);
    }
    let mut total = 0.0;
    let mut any = false;
    for (u, v) in x.data.iter().zip(y.data.iter()) {
        if let Value::Error(e) = u {
            return Err(*e);
        }
        if let Value::Error(e) = v {
            return Err(*e);
        }
        if let (Value::Number(p), Value::Number(q)) = (u, v) {
            total += f(*p, *q);
            any = true;
        }
    }
    if !any {
        return Err(CellError::Div0);
    }
    num_val(total)
}

fn sumx2my2(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    pair_sum(a, |x, y| x * x - y * y)
}
fn sumx2py2(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    pair_sum(a, |x, y| x * x + y * y)
}
fn sumxmy2(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    pair_sum(a, |x, y| (x - y) * (x - y))
}

// ---------------------------------------------------------------------------------------------
// Rounding

fn round(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(round_half_away(num(a, 0)?, digits_arg(a, 1)?))
}
fn roundup(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(round_dir(num(a, 0)?, digits_arg(a, 1)?, true))
}
fn rounddown(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(round_dir(num(a, 0)?, digits_arg(a, 1)?, false))
}
fn trunc(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(round_dir(num(a, 0)?, digits_arg(a, 1)?, false))
}
fn int_fn(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.floor()))
}

fn mround(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, m) = (num(a, 0)?, num(a, 1)?);
    if m == 0.0 || x == 0.0 {
        return Ok(Value::Number(0.0));
    }
    if (x > 0.0) != (m > 0.0) {
        return Err(CellError::Num);
    }
    let q = round_half_away(x / m, 0);
    num_val(clean(q * m))
}

fn ceiling(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let s = num(a, 1)?;
    if x == 0.0 || s == 0.0 {
        return Ok(Value::Number(0.0));
    }
    if x > 0.0 && s < 0.0 {
        return Err(CellError::Num);
    }
    num_val(clean(clean(x / s).ceil() * s))
}

fn floor(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let s = num(a, 1)?;
    if s == 0.0 {
        return if x == 0.0 { Ok(Value::Number(0.0)) } else { Err(CellError::Div0) };
    }
    if x == 0.0 {
        return Ok(Value::Number(0.0));
    }
    if x > 0.0 && s < 0.0 {
        return Err(CellError::Num);
    }
    num_val(clean(clean(x / s).floor() * s))
}

/// CEILING.MATH / FLOOR.MATH / *.PRECISE: `up` picks the direction for positive numbers;
/// negative numbers round away from zero when `away`, toward zero otherwise.
fn ceil_floor_math(x: f64, s: f64, up: bool, away: bool) -> R<Value> {
    let s = s.abs();
    if s == 0.0 || x == 0.0 {
        return Ok(Value::Number(0.0));
    }
    let q = clean(x / s);
    let r = if x >= 0.0 {
        if up { q.ceil() } else { q.floor() }
    } else if away {
        q.floor()
    } else {
        q.ceil()
    };
    num_val(clean(r * s))
}

fn ceiling_math(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let s = if has(a, 1) { num(a, 1)? } else { 1.0 };
    let mode = opt_num(a, 2, 0.0)?;
    ceil_floor_math(x, s, true, mode != 0.0)
}
fn floor_math(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let s = if has(a, 1) { num(a, 1)? } else { 1.0 };
    let mode = opt_num(a, 2, 0.0)?;
    // Negative numbers: mode 0 rounds away from zero (down), non-zero rounds toward zero.
    ceil_floor_math(x, s, false, mode == 0.0)
}
fn ceiling_precise(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let s = if has(a, 1) { num(a, 1)? } else { 1.0 };
    ceil_floor_math(x, s, true, false)
}
fn floor_precise(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let s = if has(a, 1) { num(a, 1)? } else { 1.0 };
    ceil_floor_math(x, s, false, true)
}

fn even_odd(x: f64, odd: bool) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = clean(x.abs());
    let mut v = ax.ceil();
    if odd {
        if v % 2.0 == 0.0 {
            v += 1.0;
        }
    } else if v % 2.0 != 0.0 {
        v += 1.0;
    }
    sign * v
}
fn even(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(even_odd(x, false)))
}
fn odd(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(even_odd(x, true)))
}

// ---------------------------------------------------------------------------------------------
// Arithmetic

fn modulo(x: f64, d: f64) -> R<f64> {
    if d == 0.0 {
        return Err(CellError::Div0);
    }
    // Excel returns #NUM! once the quotient magnitude reaches 1.1259E12 (checked against Excel 16).
    if (x / d).abs() >= 1.1259e12 {
        return Err(CellError::Num);
    }
    // `%` is an exact remainder (truncated), so |r| < |d| even for tiny divisors.
    let r = x % d;
    // Keep the sign of the divisor and avoid returning |d| from rounding.
    let r = if r != 0.0 && (r < 0.0) != (d < 0.0) { r + d } else { r };
    if r.abs() >= d.abs() {
        return Ok(0.0);
    }
    Ok(clean(r))
}
fn mod_fn(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(modulo(num(a, 0)?, num(a, 1)?)?)
}
fn quotient(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, d) = (num(a, 0)?, num(a, 1)?);
    num_val(clean(x / nonzero(d)?).trunc())
}
fn abs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.abs()))
}
fn sign(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| {
        Ok(if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        })
    })
}
fn sqrt(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x < 0.0 { Err(CellError::Num) } else { Ok(x.sqrt()) })
}
fn sqrtpi(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x < 0.0 { Err(CellError::Num) } else { Ok((x * PI).sqrt()) })
}

fn power_of(x: f64, y: f64) -> R<f64> {
    if x == 0.0 {
        if y == 0.0 {
            return Err(CellError::Num);
        }
        if y < 0.0 {
            return Err(CellError::Div0);
        }
        return Ok(0.0);
    }
    if x < 0.0 && y.fract() != 0.0 {
        return Err(CellError::Num);
    }
    let r = x.powf(y);
    if r.is_finite() { Ok(r) } else { Err(CellError::Num) }
}
fn power(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(power_of(num(a, 0)?, num(a, 1)?)?)
}
fn exp(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.exp()))
}
fn ln(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x <= 0.0 { Err(CellError::Num) } else { Ok(x.ln()) })
}
fn log10(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x <= 0.0 { Err(CellError::Num) } else { Ok(x.log10()) })
}
fn log(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = num(a, 0)?;
    let base = opt_num(a, 1, 10.0)?;
    if x <= 0.0 || base <= 0.0 {
        return Err(CellError::Num);
    }
    if base == 1.0 {
        return Err(CellError::Div0);
    }
    let r = if base == 10.0 {
        x.log10()
    } else if base == 2.0 {
        x.log2()
    } else {
        x.ln() / base.ln()
    };
    num_val(r)
}
fn pi(_a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(PI)
}
fn rand(_a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(c.random())
}
fn randbetween(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let lo = num(a, 0)?.ceil();
    let hi = num(a, 1)?.floor();
    if lo > hi {
        return Err(CellError::Num);
    }
    let r = c.random().clamp(0.0, 1.0);
    let v = (lo + (r * (hi - lo + 1.0)).floor()).min(hi);
    num_val(v)
}

// ---------------------------------------------------------------------------------------------
// Combinatorics

fn fact_of(n: f64) -> R<f64> {
    let n = n.trunc();
    if n < 0.0 {
        return Err(CellError::Num);
    }
    if n > 170.0 {
        return Err(CellError::Num);
    }
    let mut f = 1.0f64;
    let mut i = 2.0;
    while i <= n {
        f *= i;
        i += 1.0;
    }
    Ok(f)
}
fn fact(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(fact_of(num(a, 0)?)?)
}
fn factdouble(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let n = num(a, 0)?.trunc();
    if n < -1.0 {
        return Err(CellError::Num);
    }
    if n > 300.0 {
        return Err(CellError::Num);
    }
    let mut f = 1.0f64;
    let mut i = n;
    while i > 1.0 {
        f *= i;
        i -= 2.0;
    }
    num_val(f)
}

fn combin_of(n: f64, k: f64) -> R<f64> {
    let (n, k) = (n.trunc(), k.trunc());
    if n < 0.0 || k < 0.0 || k > n {
        return Err(CellError::Num);
    }
    let k = k.min(n - k);
    if k > 1e7 {
        let r = (crate::special::ln_gamma(n + 1.0) - crate::special::ln_gamma(k + 1.0) - crate::special::ln_gamma(n - k + 1.0)).exp();
        return if r.is_finite() { Ok(r) } else { Err(CellError::Num) };
    }
    let mut r = 1.0f64;
    let mut i = 1.0;
    while i <= k {
        r = r * (n - k + i) / i;
        if !r.is_finite() {
            return Err(CellError::Num);
        }
        i += 1.0;
    }
    Ok(if r < 9e15 { r.round() } else { r })
}
fn combin(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(combin_of(num(a, 0)?, num(a, 1)?)?)
}
fn combina(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (n, k) = (num(a, 0)?.trunc(), num(a, 1)?.trunc());
    if n < 0.0 || k < 0.0 || n < k && n == 0.0 && k > 0.0 {
        return Err(CellError::Num);
    }
    if k == 0.0 {
        return Ok(Value::Number(1.0));
    }
    num_val(combin_of(n + k - 1.0, k)?)
}
fn permut(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (n, k) = (num(a, 0)?.trunc(), num(a, 1)?.trunc());
    if n < 0.0 || k < 0.0 || k > n {
        return Err(CellError::Num);
    }
    let mut r = 1.0f64;
    let mut i = 0.0;
    while i < k {
        r *= n - i;
        if !r.is_finite() {
            return Err(CellError::Num);
        }
        i += 1.0;
    }
    num_val(r)
}
fn permutationa(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (n, k) = (num(a, 0)?.trunc(), num(a, 1)?.trunc());
    if n < 0.0 || k < 0.0 {
        return Err(CellError::Num);
    }
    num_val(n.powf(k))
}

fn integer_args(a: &[Arg]) -> R<Vec<u64>> {
    let v = numbers(a)?;
    v.iter()
        .map(|x| {
            let t = x.trunc();
            if !(0.0..9.007_199_254_740_992e15).contains(x) { Err(CellError::Num) } else { Ok(t as u64) }
        })
        .collect()
}
fn gcd2(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}
fn gcd(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = integer_args(a)?;
    num_val(v.iter().fold(0u64, |acc, &x| gcd2(acc, x)) as f64)
}
fn lcm(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = integer_args(a)?;
    let mut acc = 1f64;
    for &x in &v {
        if x == 0 {
            return Ok(Value::Number(0.0));
        }
        let accu = acc as u64;
        let g = gcd2(accu, x);
        acc = acc / g as f64 * x as f64;
        if acc >= 9.007_199_254_740_992e15 {
            return Err(CellError::Num);
        }
    }
    num_val(acc)
}
fn multinomial(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = numbers(a)?;
    let mut total = 0.0;
    let mut r = 1.0f64;
    for x in v {
        let k = x.trunc();
        if k < 0.0 {
            return Err(CellError::Num);
        }
        total += k;
        r *= combin_of(total, k)?;
        if !r.is_finite() {
            return Err(CellError::Num);
        }
    }
    num_val(r)
}

// ---------------------------------------------------------------------------------------------
// Trigonometry

fn sin(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(trig_arg(x)?.sin()))
}
fn cos(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(trig_arg(x)?.cos()))
}
fn tan(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(trig_arg(x)?.tan()))
}
fn asin(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x.abs() > 1.0 { Err(CellError::Num) } else { Ok(x.asin()) })
}
fn acos(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x.abs() > 1.0 { Err(CellError::Num) } else { Ok(x.acos()) })
}
fn atan(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.atan()))
}
fn atan2(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, y) = (num(a, 0)?, num(a, 1)?);
    if x == 0.0 && y == 0.0 {
        return Err(CellError::Div0);
    }
    num_val(y.atan2(x))
}
fn sinh(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.sinh()))
}
fn cosh(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.cosh()))
}
fn tanh(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.tanh()))
}
fn asinh(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x.asinh()))
}
fn acosh(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x < 1.0 { Err(CellError::Num) } else { Ok(x.acosh()) })
}
fn atanh(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x.abs() >= 1.0 { Err(CellError::Num) } else { Ok(x.atanh()) })
}
fn cot(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(1.0 / nonzero(trig_arg(x)?)?.tan()))
}
fn coth(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(1.0 / nonzero(trig_arg(x)?)?.tanh()))
}
fn csc(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(1.0 / nonzero(trig_arg(x)?)?.sin()))
}
fn csch(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(1.0 / nonzero(trig_arg(x)?)?.sinh()))
}
fn sec(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(1.0 / trig_arg(x)?.cos()))
}
fn sech(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(1.0 / trig_arg(x)?.cosh()))
}
fn acot(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(PI / 2.0 - x.atan()))
}
fn acoth(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| if x.abs() <= 1.0 { Err(CellError::Num) } else { Ok(0.5 * ((x + 1.0) / (x - 1.0)).ln()) })
}
fn degrees(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x * 180.0 / PI))
}
fn radians(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    unary(a, |x| Ok(x * PI / 180.0))
}

fn seriessum(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, n, m) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let coeffs = as_array(&crate::util::arg(a, 3)?.value);
    let mut total = 0.0;
    for (i, c) in coeffs.iter().enumerate() {
        let c = match c {
            Value::Number(v) => *v,
            Value::Error(e) => return Err(*e),
            _ => return Err(CellError::Value),
        };
        total += c * power_of(x, n + i as f64 * m)?;
    }
    num_val(total)
}

// ---------------------------------------------------------------------------------------------
// Registry

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!("SUM", 1, VAR, MathTrig, A, "SUM(number1, [number2], ...)", "Adds all the numbers in the arguments.", sum),
        f!(
            "PERCENTOF",
            2,
            2,
            MathTrig,
            A,
            "PERCENTOF(data_subset, data_all)",
            "The sum of a subset as a fraction of the sum of all the values.",
            percentof
        ),
        f!("SUMSQ", 1, VAR, MathTrig, A, "SUMSQ(number1, [number2], ...)", "Adds the squares of the numbers.", sumsq),
        f!(
            "SUMPRODUCT",
            1,
            VAR,
            MathTrig,
            A,
            "SUMPRODUCT(array1, [array2], ...)",
            "Multiplies matching elements of equally sized arrays and adds the products.",
            sumproduct
        ),
        f!("SUMX2MY2", 2, 2, MathTrig, A, "SUMX2MY2(array_x, array_y)", "Sums the differences of squares of matching elements.", sumx2my2),
        f!("SUMX2PY2", 2, 2, MathTrig, A, "SUMX2PY2(array_x, array_y)", "Sums the sums of squares of matching elements.", sumx2py2),
        f!("SUMXMY2", 2, 2, MathTrig, A, "SUMXMY2(array_x, array_y)", "Sums the squared differences of matching elements.", sumxmy2),
        f!("PRODUCT", 1, VAR, MathTrig, A, "PRODUCT(number1, [number2], ...)", "Multiplies all the numbers in the arguments.", product),
        f!("ROUND", 2, 2, MathTrig, S, "ROUND(number, num_digits)", "Rounds a number to a given number of digits, halves away from zero.", round),
        f!("ROUNDUP", 2, 2, MathTrig, S, "ROUNDUP(number, num_digits)", "Rounds a number away from zero to a given number of digits.", roundup),
        f!("ROUNDDOWN", 2, 2, MathTrig, S, "ROUNDDOWN(number, num_digits)", "Rounds a number toward zero to a given number of digits.", rounddown),
        f!("MROUND", 2, 2, MathTrig, S, "MROUND(number, multiple)", "Rounds a number to the nearest multiple.", mround),
        f!("CEILING", 2, 2, Compatibility, S, "CEILING(number, significance)", "Rounds a number up to a multiple of significance.", ceiling),
        f!(
            "ECMA.CEILING",
            2,
            2,
            Compatibility,
            S,
            "ECMA.CEILING(number, significance)",
            "Rounds a number up to a multiple of significance.",
            ceiling
        ),
        f!(
            "CEILING.MATH",
            1,
            3,
            MathTrig,
            S,
            "CEILING.MATH(number, [significance], [mode])",
            "Rounds a number up to the nearest integer or multiple.",
            ceiling_math
        ),
        f!(
            "CEILING.PRECISE",
            1,
            2,
            MathTrig,
            S,
            "CEILING.PRECISE(number, [significance])",
            "Rounds a number toward positive infinity to a multiple.",
            ceiling_precise
        ),
        f!(
            "ISO.CEILING",
            1,
            2,
            MathTrig,
            S,
            "ISO.CEILING(number, [significance])",
            "Rounds a number toward positive infinity to a multiple.",
            ceiling_precise
        ),
        f!("FLOOR", 2, 2, Compatibility, S, "FLOOR(number, significance)", "Rounds a number down to a multiple of significance.", floor),
        f!(
            "FLOOR.MATH",
            1,
            3,
            MathTrig,
            S,
            "FLOOR.MATH(number, [significance], [mode])",
            "Rounds a number down to the nearest integer or multiple.",
            floor_math
        ),
        f!(
            "FLOOR.PRECISE",
            1,
            2,
            MathTrig,
            S,
            "FLOOR.PRECISE(number, [significance])",
            "Rounds a number toward negative infinity to a multiple.",
            floor_precise
        ),
        f!("INT", 1, 1, MathTrig, S, "INT(number)", "Rounds a number down to the nearest integer.", int_fn),
        f!("TRUNC", 1, 2, MathTrig, S, "TRUNC(number, [num_digits])", "Cuts off the fractional part of a number at the given digits.", trunc),
        f!("MOD", 2, 2, MathTrig, S, "MOD(number, divisor)", "Remainder of a division, with the sign of the divisor.", mod_fn),
        f!("QUOTIENT", 2, 2, MathTrig, S, "QUOTIENT(numerator, denominator)", "Integer part of a division.", quotient),
        f!("ABS", 1, 1, MathTrig, S, "ABS(number)", "Absolute value of a number.", abs),
        f!("SIGN", 1, 1, MathTrig, S, "SIGN(number)", "1 for positive, -1 for negative and 0 for zero.", sign),
        f!("SQRT", 1, 1, MathTrig, S, "SQRT(number)", "Positive square root.", sqrt),
        f!("SQRTPI", 1, 1, MathTrig, S, "SQRTPI(number)", "Square root of the number times pi.", sqrtpi),
        f!("POWER", 2, 2, MathTrig, S, "POWER(number, power)", "Raises a number to a power.", power),
        f!("EXP", 1, 1, MathTrig, S, "EXP(number)", "e raised to the given power.", exp),
        f!("LN", 1, 1, MathTrig, S, "LN(number)", "Natural logarithm.", ln),
        f!("LOG", 1, 2, MathTrig, S, "LOG(number, [base])", "Logarithm to the given base (10 by default).", log),
        f!("LOG10", 1, 1, MathTrig, S, "LOG10(number)", "Base-10 logarithm.", log10),
        f!("PI", 0, 0, MathTrig, S, "PI()", "The constant pi to 15 digits.", pi),
        f!(volatile; "RAND", 0, 0, MathTrig, S, "RAND()", "A random number between 0 and 1, recalculated every time.", rand),
        f!(volatile; "RANDBETWEEN", 2, 2, MathTrig, S, "RANDBETWEEN(bottom, top)", "A random integer between two bounds, inclusive.", randbetween),
        f!("FACT", 1, 1, MathTrig, S, "FACT(number)", "Factorial of a number.", fact),
        f!("FACTDOUBLE", 1, 1, MathTrig, S, "FACTDOUBLE(number)", "Double factorial of a number.", factdouble),
        f!("COMBIN", 2, 2, MathTrig, S, "COMBIN(number, number_chosen)", "Number of combinations without repetition.", combin),
        f!("COMBINA", 2, 2, MathTrig, S, "COMBINA(number, number_chosen)", "Number of combinations with repetition.", combina),
        f!("PERMUT", 2, 2, Statistical, S, "PERMUT(number, number_chosen)", "Number of ordered selections without repetition.", permut),
        f!(
            "PERMUTATIONA",
            2,
            2,
            Statistical,
            S,
            "PERMUTATIONA(number, number_chosen)",
            "Number of ordered selections with repetition.",
            permutationa
        ),
        f!("GCD", 1, VAR, MathTrig, A, "GCD(number1, [number2], ...)", "Greatest common divisor of integers.", gcd),
        f!("LCM", 1, VAR, MathTrig, A, "LCM(number1, [number2], ...)", "Least common multiple of integers.", lcm),
        f!("EVEN", 1, 1, MathTrig, S, "EVEN(number)", "Rounds away from zero to the nearest even integer.", even),
        f!("ODD", 1, 1, MathTrig, S, "ODD(number)", "Rounds away from zero to the nearest odd integer.", odd),
        f!("SIN", 1, 1, MathTrig, S, "SIN(number)", "Sine of an angle in radians.", sin),
        f!("COS", 1, 1, MathTrig, S, "COS(number)", "Cosine of an angle in radians.", cos),
        f!("TAN", 1, 1, MathTrig, S, "TAN(number)", "Tangent of an angle in radians.", tan),
        f!("ASIN", 1, 1, MathTrig, S, "ASIN(number)", "Arcsine in radians.", asin),
        f!("ACOS", 1, 1, MathTrig, S, "ACOS(number)", "Arccosine in radians.", acos),
        f!("ATAN", 1, 1, MathTrig, S, "ATAN(number)", "Arctangent in radians.", atan),
        f!("ATAN2", 2, 2, MathTrig, S, "ATAN2(x_num, y_num)", "Angle of the point (x, y) from the x-axis in radians.", atan2),
        f!("SINH", 1, 1, MathTrig, S, "SINH(number)", "Hyperbolic sine.", sinh),
        f!("COSH", 1, 1, MathTrig, S, "COSH(number)", "Hyperbolic cosine.", cosh),
        f!("TANH", 1, 1, MathTrig, S, "TANH(number)", "Hyperbolic tangent.", tanh),
        f!("ASINH", 1, 1, MathTrig, S, "ASINH(number)", "Inverse hyperbolic sine.", asinh),
        f!("ACOSH", 1, 1, MathTrig, S, "ACOSH(number)", "Inverse hyperbolic cosine.", acosh),
        f!("ATANH", 1, 1, MathTrig, S, "ATANH(number)", "Inverse hyperbolic tangent.", atanh),
        f!("COT", 1, 1, MathTrig, S, "COT(number)", "Cotangent of an angle in radians.", cot),
        f!("COTH", 1, 1, MathTrig, S, "COTH(number)", "Hyperbolic cotangent.", coth),
        f!("CSC", 1, 1, MathTrig, S, "CSC(number)", "Cosecant of an angle in radians.", csc),
        f!("CSCH", 1, 1, MathTrig, S, "CSCH(number)", "Hyperbolic cosecant.", csch),
        f!("SEC", 1, 1, MathTrig, S, "SEC(number)", "Secant of an angle in radians.", sec),
        f!("SECH", 1, 1, MathTrig, S, "SECH(number)", "Hyperbolic secant.", sech),
        f!("ACOT", 1, 1, MathTrig, S, "ACOT(number)", "Arccotangent in radians (0 to pi).", acot),
        f!("ACOTH", 1, 1, MathTrig, S, "ACOTH(number)", "Inverse hyperbolic cotangent.", acoth),
        f!("DEGREES", 1, 1, MathTrig, S, "DEGREES(angle)", "Converts radians to degrees.", degrees),
        f!("RADIANS", 1, 1, MathTrig, S, "RADIANS(angle)", "Converts degrees to radians.", radians),
        f!("ROMAN", 1, 2, MathTrig, S, "ROMAN(number, [form])", "Converts a number to Roman numerals in one of five styles.", roman),
        f!("ARABIC", 1, 1, MathTrig, S, "ARABIC(text)", "Converts Roman numerals to a number.", arabic),
        f!("BASE", 2, 3, MathTrig, S, "BASE(number, radix, [min_length])", "Writes a number in another base, padded with zeros.", base),
        f!("DECIMAL", 2, 2, MathTrig, S, "DECIMAL(text, radix)", "Reads text in a given base as a number.", decimal),
        f!("MMULT", 2, 2, MathTrig, A, "MMULT(array1, array2)", "Matrix product of two arrays.", mmult),
        f!("MDETERM", 1, 1, MathTrig, A, "MDETERM(array)", "Determinant of a square matrix.", mdeterm),
        f!("MINVERSE", 1, 1, MathTrig, A, "MINVERSE(array)", "Inverse of a square matrix.", minverse),
        f!("MUNIT", 1, 1, MathTrig, S, "MUNIT(dimension)", "Identity matrix of the given size.", munit),
        f!("SERIESSUM", 4, 4, MathTrig, &[true, true, true, false], "SERIESSUM(x, n, m, coefficients)", "Sum of a power series.", seriessum),
        f!(
            "MULTINOMIAL",
            1,
            VAR,
            MathTrig,
            A,
            "MULTINOMIAL(number1, [number2], ...)",
            "Ratio of the factorial of a sum to the product of factorials.",
            multinomial
        ),
        f!("SUMIF", 2, 3, MathTrig, IF2, "SUMIF(range, criteria, [sum_range])", "Adds the cells that meet a condition.", sumif),
        f!(
            "SUMIFS",
            3,
            VAR,
            MathTrig,
            &SUMIFS_SCAL,
            "SUMIFS(sum_range, criteria_range1, criteria1, ...)",
            "Adds the cells that meet every condition.",
            sumifs
        ),
        f!("COUNTIF", 2, 2, Statistical, NUM_RANGE_REV, "COUNTIF(range, criteria)", "Counts the cells that meet a condition.", countif),
        f!(
            "COUNTIFS",
            2,
            VAR,
            Statistical,
            &COUNTIFS_SCAL,
            "COUNTIFS(criteria_range1, criteria1, ...)",
            "Counts the cells that meet every condition.",
            countifs
        ),
        f!(
            "AVERAGEIF",
            2,
            3,
            Statistical,
            IF2,
            "AVERAGEIF(range, criteria, [average_range])",
            "Average of the cells that meet a condition.",
            averageif
        ),
        f!(
            "AVERAGEIFS",
            3,
            VAR,
            Statistical,
            &SUMIFS_SCAL,
            "AVERAGEIFS(average_range, criteria_range1, criteria1, ...)",
            "Average of the cells that meet every condition.",
            averageifs
        ),
        f!(
            "MAXIFS",
            3,
            VAR,
            Statistical,
            &SUMIFS_SCAL,
            "MAXIFS(max_range, criteria_range1, criteria1, ...)",
            "Largest value among cells that meet every condition.",
            maxifs
        ),
        f!(
            "MINIFS",
            3,
            VAR,
            Statistical,
            &SUMIFS_SCAL,
            "MINIFS(min_range, criteria_range1, criteria1, ...)",
            "Smallest value among cells that meet every condition.",
            minifs
        ),
    ]
}

const NUM_RANGE_REV: &[bool] = &[false, true];

#[cfg(test)]
mod tests;
