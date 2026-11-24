use super::{Arg, CellError, Ctx, R, Value, array_numbers, as_array, flatten, num, num_val, numbers, opt_num, type_arg};

// ---------------------------------------------------------------------------------------------
// Time value of money
// ---------------------------------------------------------------------------------------------

/// The payment per period. `(1 + rate)^nper` comes from `nper * ln_1p(rate)` and `p - 1` from
/// `exp_m1`: rounding `1 + rate` first throws away the low bits of a small rate, which left PMT up to
/// about 1000 ULPs from Excel (observed over 400 synthetic probes; this form is within 4). A loan
/// schedule's last `IF(p - SUM(principal) > 0, ...)` turns on those bits. Excel gives #NUM! for a
/// rate of -1 or less, and -pv*rate (no divide) once `(1 + rate)^nper` overflows.
fn pmt_calc(rate: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> f64 {
    if rate == 0.0 {
        return -(pv + fv) / nper;
    }
    if rate <= -1.0 {
        return f64::NAN;
    }
    let l = nper * rate.ln_1p();
    let m = l.exp_m1();
    let k = 1.0 + rate * typ;
    if m > 0.0 {
        // (fv + pv*p) / (p - 1) = pv + (pv + fv) / (p - 1), without the cancellation in p - 1.
        // rate / m (about 1 / nper for a tiny rate) is taken first so (pv + fv) / m can't overflow.
        -(pv * rate + (pv + fv) * (rate / m)) / k
    } else if m > -0.5 {
        // p near 1 from below: fv + pv*p = (pv + fv) + pv*m keeps the small terms that rounding p would drop.
        -(rate * (pv + fv + pv * m)) / (k * m)
    } else {
        // p well below 1: m nears -1 and pv*m would swallow pv + fv, so use p itself.
        -(rate * (fv + pv * l.exp())) / (k * m)
    }
}

fn fv_calc(rate: f64, nper: f64, pmt: f64, pv: f64, typ: f64) -> f64 {
    if rate == 0.0 {
        return -(pv + pmt * nper);
    }
    let p = (1.0 + rate).powf(nper);
    -(pv * p + pmt * (1.0 + rate * typ) * (p - 1.0) / rate)
}

fn pv_calc(rate: f64, nper: f64, pmt: f64, fv: f64, typ: f64) -> f64 {
    if rate == 0.0 {
        return -(fv + pmt * nper);
    }
    let p = (1.0 + rate).powf(nper);
    -(fv + pmt * (1.0 + rate * typ) * (p - 1.0) / rate) / p
}

pub(super) fn pmt(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, nper, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let (fv, typ) = (opt_num(a, 3, 0.0)?, type_arg(a, 4)?);
    if nper == 0.0 {
        return Err(CellError::Num);
    }
    num_val(pmt_calc(rate, nper, pv, fv, typ))
}

pub(super) fn fv(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, nper, pmt) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    num_val(fv_calc(rate, nper, pmt, opt_num(a, 3, 0.0)?, type_arg(a, 4)?))
}

pub(super) fn pv(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, nper, pmt) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    num_val(pv_calc(rate, nper, pmt, opt_num(a, 3, 0.0)?, type_arg(a, 4)?))
}

pub(super) fn nper(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, pmt, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let (fv, typ) = (opt_num(a, 3, 0.0)?, type_arg(a, 4)?);
    if rate == 0.0 {
        if pmt == 0.0 {
            return Err(CellError::Num);
        }
        return num_val(-(pv + fv) / pmt);
    }
    let numer = pmt * (1.0 + rate * typ) - fv * rate;
    let denom = pv * rate + pmt * (1.0 + rate * typ);
    if denom == 0.0 {
        return Err(CellError::Num);
    }
    let q = numer / denom;
    if q <= 0.0 || rate <= -1.0 {
        return Err(CellError::Num);
    }
    num_val(q.ln() / (1.0 + rate).ln())
}

fn ipmt_calc(rate: f64, per: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> f64 {
    let p = pmt_calc(rate, nper, pv, fv, typ);
    if typ == 1.0 && per == 1.0 {
        return 0.0;
    }
    let bal = fv_calc(rate, per - 1.0, p, pv, typ);
    let i = bal * rate;
    if typ == 1.0 { i / (1.0 + rate) } else { i }
}

pub(super) fn ipmt(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, per, np, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?);
    let (fv, typ) = (opt_num(a, 4, 0.0)?, type_arg(a, 5)?);
    if per < 1.0 || per > np || np == 0.0 {
        return Err(CellError::Num);
    }
    num_val(ipmt_calc(rate, per, np, pv, fv, typ))
}

pub(super) fn ppmt(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, per, np, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?);
    let (fv, typ) = (opt_num(a, 4, 0.0)?, type_arg(a, 5)?);
    if per < 1.0 || per > np || np == 0.0 {
        return Err(CellError::Num);
    }
    num_val(pmt_calc(rate, np, pv, fv, typ) - ipmt_calc(rate, per, np, pv, fv, typ))
}

/// The validated (rate, periods, pv, start, end, type) of CUMIPMT/CUMPRINC: every check that
/// refuses a call before its period loop, shared with the lifted-call work preflight.
pub(crate) fn cumulative_args(a: &[Arg]) -> R<(f64, f64, f64, f64, f64, f64)> {
    let (rate, np, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let (start, end, typ) = (num(a, 3)?.trunc(), num(a, 4)?.trunc(), num(a, 5)?);
    let np_i = np.trunc();
    if rate <= 0.0 || np_i <= 0.0 || pv <= 0.0 || start < 1.0 || end < start || end > np_i || (typ != 0.0 && typ != 1.0) {
        return Err(CellError::Num);
    }
    if end - start > 1e7 || np_i > 1e15 {
        return Err(CellError::Num);
    }
    Ok((rate, np_i, pv, start, end, typ))
}

fn cumulative(a: &[Arg], principal: bool) -> R<Value> {
    let (rate, np_i, pv, start, end, typ) = cumulative_args(a)?;
    let p = pmt_calc(rate, np_i, pv, 0.0, typ);
    let mut sum = 0.0;
    let mut per = start;
    while per <= end {
        let i = ipmt_calc(rate, per, np_i, pv, 0.0, typ);
        sum += if principal { p - i } else { i };
        per += 1.0;
    }
    num_val(sum)
}

pub(super) fn cumipmt(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    cumulative(a, false)
}

pub(super) fn cumprinc(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    cumulative(a, true)
}

/// Newton iteration with a numeric derivative, keeping the rate above -1.
fn newton(f: impl Fn(f64) -> f64, guess: f64) -> Option<f64> {
    let mut x = guess;
    for _ in 0..200 {
        let y = f(x);
        if !y.is_finite() {
            return None;
        }
        let h = 1e-6 * x.abs().max(1e-3);
        let d = (f(x + h) - f(x - h)) / (2.0 * h);
        if d == 0.0 || !d.is_finite() {
            return None;
        }
        let mut nx = x - y / d;
        if nx <= -1.0 {
            nx = (x - 1.0) / 2.0;
            if nx <= -1.0 {
                nx = -0.999_999_999;
            }
        }
        if (nx - x).abs() < 1e-12 * nx.abs().max(1.0) {
            return Some(nx);
        }
        x = nx;
    }
    None
}

/// Tries Newton from the guess and a few fallback starting points; then bisection over a
/// bracket if one can be found.
fn solve_rate(f: impl Fn(f64) -> f64, guess: f64) -> R<f64> {
    let check = |r: f64| f(r).is_finite() && f(r).abs() < 1e-7 * (1.0 + f(guess).abs().min(1e12));
    for g in [guess, 0.1, 0.01, 0.5, -0.5, 1.0, 0.001, 5.0, -0.9, 10.0] {
        if let Some(r) = newton(&f, g)
            && r > -1.0
            && check(r)
        {
            return Ok(r);
        }
    }
    // Scan for a sign change and bisect.
    let grid: Vec<f64> = [-0.999_999, -0.99, -0.9, -0.5, -0.2, 0.0, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 100.0, 1000.0].to_vec();
    for w in grid.windows(2) {
        let (mut lo, mut hi) = match w {
            [a, b] => (*a, *b),
            _ => continue,
        };
        let (flo, fhi) = (f(lo), f(hi));
        if !(flo.is_finite() && fhi.is_finite()) || flo * fhi > 0.0 {
            continue;
        }
        let lo_neg = flo < 0.0;
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            let fm = f(mid);
            if (fm < 0.0) == lo_neg {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        return Ok(0.5 * (lo + hi));
    }
    Err(CellError::Num)
}

/// RATE's equation at rate `r`: the loan and the payments grown to the end of the term, plus
/// `fv`. RATE returns the rate where it is zero. The second value is the size of its terms, to
/// judge how close to zero the first can get in floating point.
fn rate_equation(r: f64, np: f64, pmt: f64, pv: f64, fv: f64, typ: f64) -> (f64, f64) {
    if r == 0.0 {
        return (pv + pmt * np + fv, pv.abs() + (pmt * np).abs() + fv.abs());
    }
    let p = (1.0 + r).powf(np);
    let (a, b) = (pv * p, pmt * (1.0 / r + typ) * (p - 1.0));
    (a + b + fv, a.abs() + b.abs() + fv.abs())
}

/// Excel works with subnormal numbers flushed to zero: a secant step that underflows doesn't
/// move. Observed with RATE(3,0,-1E-305,0), which is 0.101 (the second starting point).
fn flush_subnormal(x: f64) -> f64 {
    if x.abs() < f64::MIN_POSITIVE { 0.0 } else { x }
}

/// Excel's RATE solver, reconstructed by observing Excel with synthetic inputs (not from its
/// code; its documentation says only "iteration", 20 steps and 0.0000001):
///
/// - A secant iteration starting at the guess and at the guess moved 0.001 toward a larger
///   value of the equation, for at most 200 steps.
/// - It stops at the first iterate that moved less than 1e-7 and where the equation is within
///   1e-7 of zero (or as close as its terms allow, for very large amounts). Excel returns that
///   iterate, not the exact root, so a slowly converging case such as no payments and a zero
///   future value (exact answer -100%) gives a rate just above -1.
/// - The result must be above -1 by more than the tolerance; otherwise it is #NUM!.
/// - After 200 steps without that, Excel still returns the last iterate if it is within a
///   looser 1e-4 (step, equation value and distance above -1); otherwise #NUM!. So a root the
///   iteration can't reach (a very large rate) is #NUM! even though it exists.
fn rate_solve(f: impl Fn(f64) -> (f64, f64), guess: f64) -> R<f64> {
    const STEPS: usize = 200;
    const TOL: f64 = 1e-7;
    const LOOSE: f64 = 1e-4;
    let eval = |x: f64| {
        let (y, size) = f(x);
        if y.is_finite() { Ok((y, size)) } else { Err(CellError::Num) }
    };
    let near_zero = |y: f64, size: f64| y.abs() < TOL || y.abs() <= 1e-12 * size;
    let (mut x0, (mut f0, _)) = (guess, eval(guess)?);
    let mut x1 = if f0 < 0.0 { guess + 0.001 } else { guess - 0.001 };
    let (mut f1, mut size1) = eval(x1)?;
    for _ in 0..STEPS {
        let den = flush_subnormal(f1 - f0);
        if den == 0.0 {
            // The secant is flat: done if the iteration had already settled.
            if (x1 - x0).abs() < TOL && near_zero(f1, size1) && x1 - TOL > -1.0 {
                return Ok(x1);
            }
            return Err(CellError::Num);
        }
        let x2 = x1 - flush_subnormal(f1 * (x1 - x0)) / den;
        if !x2.is_finite() {
            return Err(CellError::Num);
        }
        let (f2, size2) = eval(x2)?;
        if (x2 - x1).abs() < TOL && near_zero(f2, size2) {
            return if x2 - TOL > -1.0 { Ok(x2) } else { Err(CellError::Num) };
        }
        (x0, f0, x1, f1, size1) = (x1, f1, x2, f2, size2);
    }
    if (x1 - x0).abs() < LOOSE && f1.abs() < LOOSE && x1 - LOOSE > -1.0 {
        return Ok(x1);
    }
    Err(CellError::Num)
}

pub(super) fn rate(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (np, pmt, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    let (fv, typ, guess) = (opt_num(a, 3, 0.0)?, type_arg(a, 4)?, opt_num(a, 5, 0.1)?);
    if np <= 0.0 {
        return Err(CellError::Num);
    }
    // Excel: a guess at or below -100% is #VALUE!, not #NUM!.
    if guess <= -1.0 {
        return Err(CellError::Value);
    }
    num_val(rate_solve(|r| rate_equation(r, np, pmt, pv, fv, typ), guess)?)
}

pub(super) fn npv(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let rate = num(a, 0)?;
    if rate == -1.0 {
        return Err(CellError::Div0);
    }
    let vals = numbers(a.get(1..).unwrap_or(&[]))?;
    let mut sum = 0.0;
    let mut d = 1.0;
    for v in vals {
        d *= 1.0 + rate;
        sum += v / d;
    }
    num_val(sum)
}

/// Values and dates for XNPV/XIRR: both must be numeric and of equal length.
fn dated_flows(values: &Value, dates: &Value) -> R<(Vec<f64>, Vec<f64>)> {
    let va = as_array(values);
    let da = as_array(dates);
    if va.data.len() != da.data.len() {
        return Err(CellError::Num);
    }
    let mut v = Vec::with_capacity(va.data.len());
    let mut d = Vec::with_capacity(da.data.len());
    for (x, y) in va.data.iter().zip(da.data.iter()) {
        match x {
            Value::Number(n) => v.push(*n),
            Value::Error(e) => return Err(*e),
            _ => return Err(CellError::Value),
        }
        match y {
            Value::Number(n) => d.push(n.trunc()),
            Value::Error(e) => return Err(*e),
            Value::Text(_) => d.push(y.to_number()?.trunc()),
            _ => return Err(CellError::Value),
        }
    }
    let d0 = *d.first().ok_or(CellError::Num)?;
    if d.iter().any(|&x| x < d0 || x < 0.0) {
        return Err(CellError::Num);
    }
    Ok((v, d))
}

fn xnpv_calc(rate: f64, v: &[f64], d: &[f64]) -> f64 {
    let d0 = d.first().copied().unwrap_or(0.0);
    v.iter().zip(d).map(|(x, t)| x / (1.0 + rate).powf((t - d0) / 365.0)).sum()
}

pub(super) fn xnpv(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let rate = num(a, 0)?;
    if rate <= -1.0 {
        return Err(CellError::Num);
    }
    let (v, d) = dated_flows(&a.get(1).ok_or(CellError::Value)?.value, &a.get(2).ok_or(CellError::Value)?.value)?;
    num_val(xnpv_calc(rate, &v, &d))
}

pub(super) fn xirr(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (v, d) = dated_flows(&a.first().ok_or(CellError::Value)?.value, &a.get(1).ok_or(CellError::Value)?.value)?;
    let guess = opt_num(a, 2, 0.1)?;
    if !v.iter().any(|x| *x > 0.0) || !v.iter().any(|x| *x < 0.0) {
        return Err(CellError::Num);
    }
    num_val(solve_rate(|r| xnpv_calc(r, &v, &d), guess)?)
}

pub(super) fn irr(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = array_numbers(&a.first().ok_or(CellError::Value)?.value)?;
    let guess = opt_num(a, 1, 0.1)?;
    if !v.iter().any(|x| *x > 0.0) || !v.iter().any(|x| *x < 0.0) {
        return Err(CellError::Num);
    }
    let f = |r: f64| {
        let mut s = 0.0;
        let mut d = 1.0;
        for x in &v {
            s += x / d;
            d *= 1.0 + r;
        }
        s
    };
    num_val(solve_rate(f, guess)?)
}

pub(super) fn mirr(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = array_numbers(&a.first().ok_or(CellError::Value)?.value)?;
    let (fr, rr) = (num(a, 1)?, num(a, 2)?);
    let n = v.len() as f64;
    if v.len() < 2 {
        return Err(CellError::Div0);
    }
    let (mut pos, mut neg) = (0.0, 0.0);
    for (i, x) in v.iter().enumerate() {
        if *x > 0.0 {
            pos += x / (1.0 + rr).powi(i as i32);
        } else if *x < 0.0 {
            neg += x / (1.0 + fr).powi(i as i32);
        }
    }
    // No positive flow is not an error in Excel: the formula below gives -1.
    if neg == 0.0 {
        return Err(CellError::Div0);
    }
    let r = (-pos * (1.0 + rr).powf(n - 1.0) / neg).powf(1.0 / (n - 1.0)) - 1.0;
    num_val(r)
}

pub(super) fn fvschedule(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let mut p = num(a, 0)?;
    for v in flatten(a.get(1..2).unwrap_or(&[])) {
        let r = match v {
            Value::Number(n) => n,
            Value::Empty => 0.0,
            Value::Error(e) => return Err(e),
            _ => return Err(CellError::Value),
        };
        p *= 1.0 + r;
    }
    num_val(p)
}

pub(super) fn effect(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (nom, n) = (num(a, 0)?, num(a, 1)?.trunc());
    if nom <= 0.0 || n < 1.0 {
        return Err(CellError::Num);
    }
    num_val((1.0 + nom / n).powf(n) - 1.0)
}

pub(super) fn nominal(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (eff, n) = (num(a, 0)?, num(a, 1)?.trunc());
    if eff <= 0.0 || n < 1.0 {
        return Err(CellError::Num);
    }
    num_val(n * ((1.0 + eff).powf(1.0 / n) - 1.0))
}

pub(super) fn ispmt(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, per, np, pv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?);
    if np == 0.0 {
        return Err(CellError::Div0);
    }
    num_val(pv * rate * (per / np - 1.0))
}

pub(super) fn pduration(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (rate, pv, fv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if rate <= 0.0 || pv <= 0.0 || fv <= 0.0 {
        return Err(CellError::Num);
    }
    num_val((fv.ln() - pv.ln()) / (1.0 + rate).ln())
}

pub(super) fn rri(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (np, pv, fv) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if np <= 0.0 || pv == 0.0 {
        return Err(CellError::Num);
    }
    num_val((fv / pv).powf(1.0 / np) - 1.0)
}

fn dollar_digits(fraction: f64) -> R<f64> {
    if fraction < 0.0 {
        return Err(CellError::Num);
    }
    if fraction < 1.0 {
        return Err(CellError::Div0);
    }
    Ok(10f64.powf(fraction.log10().ceil()))
}

pub(super) fn dollarde(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, f) = (num(a, 0)?, num(a, 1)?.trunc());
    let p = dollar_digits(f)?;
    let int = x.trunc();
    num_val(int + (x - int) * p / f)
}

pub(super) fn dollarfr(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (x, f) = (num(a, 0)?, num(a, 1)?.trunc());
    let p = dollar_digits(f)?;
    let int = x.trunc();
    num_val(int + (x - int) * f / p)
}
