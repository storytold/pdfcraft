use super::day_count::{basis_arg, coup_daybs, coup_days, coup_daysnc, coupons, date, day_count, freq_arg, shift_months, year_basis, year_frac, ymd};
use super::{Arg, CellError, Ctx, DateSystem, R, Value, days_in_month, has, num, num_val, opt_bool};

// ---------------------------------------------------------------------------------------------
// Securities
// ---------------------------------------------------------------------------------------------

pub(super) fn accrint(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let (issue, first, settle, rate) = (date(a, 0)?, date(a, 1)?, date(a, 2)?, num(a, 3)?);
    let par = if has(a, 4) { num(a, 4)? } else { 1000.0 };
    let freq = freq_arg(a, 5)?;
    let basis = basis_arg(a, 6)?;
    let calc_method = opt_bool(a, 7, true)?;
    if issue >= settle || rate <= 0.0 || par <= 0.0 {
        return Err(CellError::Num);
    }
    let anchor = ymd(sys, first)?;
    let eom = anchor.2 == days_in_month(anchor.0, anchor.1);
    let step = (12.0 / freq) as i64;
    let cd = |j: i64| shift_months(sys, anchor, j * step, eom);
    // Coupon index of the period containing a date: cd(j) <= date < cd(j+1).
    let index_of = |d: f64| -> R<i64> {
        let mut j = (((d - first) / 365.25 * freq).floor() as i64).clamp(-200_000, 200_000);
        let mut guard = 0;
        while cd(j)? > d {
            j -= 1;
            guard += 1;
            if guard > 400_000 {
                return Err(CellError::Num);
            }
        }
        while cd(j + 1)? <= d {
            j += 1;
            guard += 1;
            if guard > 400_000 {
                return Err(CellError::Num);
            }
        }
        Ok(j)
    };
    let start = if calc_method || settle <= first { issue } else { cd(index_of(settle)?)? };
    let mut j = index_of(start)?;
    let mut sum = 0.0;
    let mut guard = 0;
    loop {
        let (p0, p1) = (cd(j)?, cd(j + 1)?);
        if p0 >= settle {
            break;
        }
        let from = p0.max(start);
        let to = p1.min(settle);
        if to > from {
            let nl = match basis {
                1 => p1 - p0,
                3 => 365.0 / freq,
                _ => 360.0 / freq,
            };
            sum += day_count(sys, from, to, basis)? / nl;
        }
        j += 1;
        guard += 1;
        if guard > 100_000 {
            return Err(CellError::Num);
        }
    }
    num_val(par * rate / freq * sum)
}

pub(super) fn accrintm(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (issue, settle, rate) = (date(a, 0)?, date(a, 1)?, num(a, 2)?);
    let par = if has(a, 3) { num(a, 3)? } else { 1000.0 };
    let basis = basis_arg(a, 4)?;
    // Excel accepts equal (whole-day) dates and accrues nothing; only issue after settlement is #NUM!.
    if issue > settle || rate <= 0.0 || par <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(par * rate * year_frac(c.date_system(), issue, settle, basis)?)
}

/// settlement, maturity, value, value, [basis] for discount securities.
fn disc_args(a: &[Arg]) -> R<(f64, f64, f64, f64, u32)> {
    let (s, m, x, y, b) = (date(a, 0)?, date(a, 1)?, num(a, 2)?, num(a, 3)?, basis_arg(a, 4)?);
    if s >= m || x <= 0.0 || y <= 0.0 {
        return Err(CellError::Num);
    }
    Ok((s, m, x, y, b))
}

fn disc_frac(sys: DateSystem, s: f64, m: f64, b: u32) -> R<f64> {
    Ok(day_count(sys, s, m, b)? / year_basis(sys, b, s)?)
}

pub(super) fn disc(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, pr, red, b) = disc_args(a)?;
    num_val((1.0 - pr / red) / disc_frac(c.date_system(), s, m, b)?)
}

pub(super) fn pricedisc(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, d, red, b) = disc_args(a)?;
    num_val(red - d * red * disc_frac(c.date_system(), s, m, b)?)
}

pub(super) fn yielddisc(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, pr, red, b) = disc_args(a)?;
    num_val((red / pr - 1.0) / disc_frac(c.date_system(), s, m, b)?)
}

pub(super) fn intrate(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, inv, red, b) = disc_args(a)?;
    num_val((red - inv) / inv / disc_frac(c.date_system(), s, m, b)?)
}

pub(super) fn received(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, inv, d, b) = disc_args(a)?;
    let denom = 1.0 - d * disc_frac(c.date_system(), s, m, b)?;
    if denom <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(inv / denom)
}

fn tbill_args(a: &[Arg], sys: DateSystem) -> R<(f64, f64)> {
    let (s, m, x) = (date(a, 0)?, date(a, 1)?, num(a, 2)?);
    if s >= m || x <= 0.0 {
        return Err(CellError::Num);
    }
    // At most one year after settlement.
    let (y, mo, d) = ymd(sys, s)?;
    let limit = shift_months(sys, (y, mo, d), 12, false)?;
    if m > limit {
        return Err(CellError::Num);
    }
    Ok((m - s, x))
}

pub(super) fn tbillprice(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (dsm, d) = tbill_args(a, c.date_system())?;
    let p = 100.0 * (1.0 - d * dsm / 360.0);
    if p <= 0.0 {
        return Err(CellError::Num);
    }
    num_val(p)
}

pub(super) fn tbillyield(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (dsm, pr) = tbill_args(a, c.date_system())?;
    num_val((100.0 - pr) / pr * 360.0 / dsm)
}

pub(super) fn tbilleq(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (dsm, d) = tbill_args(a, c.date_system())?;
    if dsm <= 182.0 {
        return num_val(365.0 * d / (360.0 - d * dsm));
    }
    let price = 100.0 * (1.0 - d * dsm / 360.0);
    if price <= 0.0 {
        return Err(CellError::Num);
    }
    let t = dsm / 365.0;
    let disc = t * t - (2.0 * t - 1.0) * (1.0 - 100.0 / price);
    if disc < 0.0 {
        return Err(CellError::Num);
    }
    num_val((-t + disc.sqrt()) / (t - 0.5))
}

/// settlement, maturity, issue, rate, value, [basis].
fn mat_terms(a: &[Arg], sys: DateSystem) -> R<(f64, f64, f64, f64, f64)> {
    let (s, m, issue, rate, x, b) = (date(a, 0)?, date(a, 1)?, date(a, 2)?, num(a, 3)?, num(a, 4)?, basis_arg(a, 5)?);
    if s >= m || issue >= s || rate < 0.0 || x < 0.0 {
        return Err(CellError::Num);
    }
    let dim = year_frac(sys, issue, m, b)?;
    let dsm = year_frac(sys, s, m, b)?;
    let ai = year_frac(sys, issue, s, b)?;
    Ok((dim, dsm, ai, rate, x))
}

pub(super) fn pricemat(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (dim, dsm, ai, rate, yld) = mat_terms(a, c.date_system())?;
    num_val((100.0 + dim * rate * 100.0) / (1.0 + dsm * yld) - ai * rate * 100.0)
}

pub(super) fn yieldmat(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (dim, dsm, ai, rate, pr) = mat_terms(a, c.date_system())?;
    if pr <= 0.0 || dsm == 0.0 {
        return Err(CellError::Num);
    }
    let base = pr / 100.0 + ai * rate;
    num_val(((1.0 + dim * rate) - base) / base / dsm)
}

struct Bond {
    n: f64,
    e: f64,
    a: f64,
    dsc: f64,
    freq: f64,
}

fn bond(sys: DateSystem, settle: f64, mat: f64, freq: f64, basis: u32) -> R<Bond> {
    let c = coupons(sys, settle, mat, freq)?;
    let e = coup_days(&c, freq, basis);
    let a = coup_daybs(sys, &c, settle, basis)?;
    let dsc = coup_daysnc(sys, &c, settle, freq, basis)?;
    if e <= 0.0 {
        return Err(CellError::Num);
    }
    Ok(Bond { n: c.num, e, a, dsc, freq })
}

fn bond_price(b: &Bond, rate: f64, yld: f64, red: f64) -> f64 {
    let f = b.freq;
    let coupon = 100.0 * rate / f;
    let accrued = coupon * b.a / b.e;
    if b.n == 1.0 {
        return (red + coupon) / (1.0 + b.dsc / b.e * yld / f) - accrued;
    }
    let base = 1.0 + yld / f;
    let frac = b.dsc / b.e;
    let mut sum = red / base.powf(b.n - 1.0 + frac);
    let mut k = 1.0;
    while k <= b.n {
        sum += coupon / base.powf(k - 1.0 + frac);
        k += 1.0;
    }
    sum - accrued
}

pub(super) fn price(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, rate, yld, red) = (date(a, 0)?, date(a, 1)?, num(a, 2)?, num(a, 3)?, num(a, 4)?);
    let (f, basis) = (freq_arg(a, 5)?, basis_arg(a, 6)?);
    if rate < 0.0 || yld < 0.0 || red <= 0.0 {
        return Err(CellError::Num);
    }
    let b = bond(c.date_system(), s, m, f, basis)?;
    num_val(bond_price(&b, rate, yld, red))
}

pub(super) fn yield_(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (s, m, rate, pr, red) = (date(a, 0)?, date(a, 1)?, num(a, 2)?, num(a, 3)?, num(a, 4)?);
    let (f, basis) = (freq_arg(a, 5)?, basis_arg(a, 6)?);
    if rate < 0.0 || pr <= 0.0 || red <= 0.0 {
        return Err(CellError::Num);
    }
    let b = bond(c.date_system(), s, m, f, basis)?;
    if b.n == 1.0 {
        let base = pr / 100.0 + b.a / b.e * rate / f;
        return num_val(((red / 100.0 + rate / f) - base) / base * f * b.e / b.dsc);
    }
    // Price is decreasing in yield: bisect on [0, hi].
    let g = |y: f64| bond_price(&b, rate, y, red) - pr;
    let (mut lo, mut hi) = (-0.999_999 * f, 1.0);
    let mut guard = 0;
    while g(hi) > 0.0 && guard < 60 {
        hi *= 2.0;
        guard += 1;
    }
    if !(g(lo) >= 0.0 && g(hi) <= 0.0) {
        return Err(CellError::Num);
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if g(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    num_val(0.5 * (lo + hi))
}

fn duration_calc(a: &[Arg], sys: DateSystem) -> R<(f64, f64, f64)> {
    let (s, m, coupon, yld) = (date(a, 0)?, date(a, 1)?, num(a, 2)?, num(a, 3)?);
    let (f, basis) = (freq_arg(a, 4)?, basis_arg(a, 5)?);
    if coupon < 0.0 || yld < 0.0 {
        return Err(CellError::Num);
    }
    let b = bond(sys, s, m, f, basis)?;
    let c = coupon * 100.0 / f;
    let y = 1.0 + yld / f;
    let frac = b.dsc / b.e;
    let (mut wsum, mut psum) = (0.0, 0.0);
    let mut k = 1.0;
    while k <= b.n {
        let t = k - 1.0 + frac;
        let cf = if k == b.n { c + 100.0 } else { c };
        let pv = cf / y.powf(t);
        wsum += t * pv;
        psum += pv;
        k += 1.0;
    }
    if psum == 0.0 {
        return Err(CellError::Num);
    }
    Ok((wsum / psum / f, yld, f))
}

pub(super) fn duration(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(duration_calc(a, c.date_system())?.0)
}

pub(super) fn mduration(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (d, y, f) = duration_calc(a, c.date_system())?;
    num_val(d / (1.0 + y / f))
}

// ---------------------------------------------------------------------------------------------
// Odd first and last coupon periods
// ---------------------------------------------------------------------------------------------
//
// An odd period is split into quasi-coupon periods: the regular coupon periods the security
// would have had, counted back from the first coupon (ODDF*) or forward from the last interest
// date (ODDL*). For quasi-period i, NL is its normal length, DC the days of it inside the odd
// period, A the accrued days of it before settlement and DSC the days of it from settlement on.

/// Most quasi-coupon periods an odd period may span (the date range allows ~32,400 quarters).
const MAX_QUASI: i64 = 40_000;

/// Normal length of the coupon period `[start, end]` under a basis (COUPDAYS semantics).
fn normal_length(start: f64, end: f64, freq: f64, basis: u32) -> f64 {
    match basis {
        1 => end - start,
        3 => 365.0 / freq,
        _ => 360.0 / freq,
    }
}

/// Days of `[from, to]` under a basis, or 0 when the span is empty.
fn span(sys: DateSystem, from: f64, to: f64, basis: u32) -> R<f64> {
    if to > from { day_count(sys, from, to, basis) } else { Ok(0.0) }
}

/// The odd-first-period terms that ODDFPRICE and ODDFYIELD share.
struct OddFirst {
    /// Coupons after the first coupon up to maturity.
    n: f64,
    /// Whole quasi-coupon periods between settlement and the first coupon.
    nq: f64,
    /// DSC / NL of the quasi-period holding settlement.
    frac: f64,
    /// Σ DCi / NLi over the odd period.
    dc: f64,
    /// Σ Ai / NLi over the odd period.
    acc: f64,
    freq: f64,
}

fn odd_first(sys: DateSystem, settle: f64, mat: f64, issue: f64, first: f64, freq: f64, basis: u32) -> R<OddFirst> {
    if !(mat > first && first > settle && settle > issue) {
        return Err(CellError::Num);
    }
    let anchor = ymd(sys, first)?;
    let eom = anchor.2 == days_in_month(anchor.0, anchor.1);
    let step = (12.0 / freq) as i64;
    let quasi = |k: i64| shift_months(sys, anchor, -k * step, eom);
    let (mut dc, mut acc) = (0.0, 0.0);
    let (mut nq, mut frac) = (None, 0.0);
    // Walk back from the first coupon: period k runs from quasi(k + 1) to quasi(k).
    let mut k = 0;
    loop {
        if k >= MAX_QUASI {
            return Err(CellError::Num);
        }
        let (end, start) = (quasi(k)?, quasi(k + 1)?);
        let nl = normal_length(start, end, freq, basis);
        if nl <= 0.0 {
            return Err(CellError::Num);
        }
        let from = start.max(issue);
        dc += span(sys, from, end, basis)? / nl;
        acc += span(sys, from, settle.min(end), basis)? / nl;
        if start <= settle && settle < end {
            nq = Some(k as f64);
            frac = span(sys, settle, end, basis)? / nl;
        }
        if start <= issue {
            break;
        }
        k += 1;
    }
    let nq = nq.ok_or(CellError::Num)?;
    let n = coupons(sys, first, mat, freq)?.num;
    Ok(OddFirst { n, nq, frac, dc, acc, freq })
}

fn odd_first_price(t: &OddFirst, rate: f64, yld: f64, red: f64) -> f64 {
    let coupon = 100.0 * rate / t.freq;
    let base = 1.0 + yld / t.freq;
    let lead = t.nq + t.frac;
    let mut p = red / base.powf(t.n + lead) + coupon * t.dc / base.powf(lead);
    let mut k = 1.0;
    while k <= t.n {
        p += coupon / base.powf(k + lead);
        k += 1.0;
    }
    p - coupon * t.acc
}

/// settlement, maturity, issue, first_coupon, rate, yld/pr, redemption, frequency, [basis].
fn odd_first_args(a: &[Arg], sys: DateSystem) -> R<(OddFirst, f64, f64, f64)> {
    let (s, m, issue, first) = (date(a, 0)?, date(a, 1)?, date(a, 2)?, date(a, 3)?);
    let (rate, x, red) = (num(a, 4)?, num(a, 5)?, num(a, 6)?);
    let (f, basis) = (freq_arg(a, 7)?, basis_arg(a, 8)?);
    if rate < 0.0 || red <= 0.0 {
        return Err(CellError::Num);
    }
    Ok((odd_first(sys, s, m, issue, first, f, basis)?, rate, x, red))
}

pub(super) fn oddfprice(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (t, rate, yld, red) = odd_first_args(a, c.date_system())?;
    if yld < 0.0 {
        return Err(CellError::Num);
    }
    num_val(odd_first_price(&t, rate, yld, red))
}

pub(super) fn oddfyield(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (t, rate, pr, red) = odd_first_args(a, c.date_system())?;
    if pr <= 0.0 {
        return Err(CellError::Num);
    }
    // The price falls as the yield rises: bracket the root, then bisect.
    let g = |y: f64| odd_first_price(&t, rate, y, red) - pr;
    let (mut lo, mut hi) = (-0.999_999 * t.freq, 1.0);
    let mut guard = 0;
    while g(hi) > 0.0 && guard < 60 {
        hi *= 2.0;
        guard += 1;
    }
    // NaN fails both comparisons.
    if !(g(lo) >= 0.0 && g(hi) <= 0.0) {
        return Err(CellError::Num);
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if g(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    num_val(0.5 * (lo + hi))
}

/// Σ DCi/NLi, Σ Ai/NLi and Σ DSCi/NLi over an odd last period.
fn odd_last(sys: DateSystem, settle: f64, mat: f64, last: f64, freq: f64, basis: u32) -> R<(f64, f64, f64)> {
    if !(mat > settle && settle > last) {
        return Err(CellError::Num);
    }
    let anchor = ymd(sys, last)?;
    let eom = anchor.2 == days_in_month(anchor.0, anchor.1);
    let step = (12.0 / freq) as i64;
    let (mut dc, mut acc, mut dsc) = (0.0, 0.0, 0.0);
    // Walk forward from the last interest date: period k runs from quasi(k - 1) to quasi(k).
    let mut start = last;
    let mut k = 1;
    loop {
        if k > MAX_QUASI {
            return Err(CellError::Num);
        }
        let end = shift_months(sys, anchor, k * step, eom)?;
        let nl = normal_length(start, end, freq, basis);
        if nl <= 0.0 {
            return Err(CellError::Num);
        }
        let stop = end.min(mat);
        dc += span(sys, start, stop, basis)? / nl;
        acc += span(sys, start, settle.min(stop), basis)? / nl;
        dsc += span(sys, settle.max(start), stop, basis)? / nl;
        if end >= mat {
            break;
        }
        start = end;
        k += 1;
    }
    Ok((dc, acc, dsc))
}

/// settlement, maturity, last_interest, rate, yld/pr, redemption, frequency, [basis].
/// `((dc, acc, dsc), rate, yield-or-price, redemption, frequency)` of an odd-last-period call.
type OddLastArgs = ((f64, f64, f64), f64, f64, f64, f64);

fn odd_last_args(a: &[Arg], sys: DateSystem) -> R<OddLastArgs> {
    let (s, m, last) = (date(a, 0)?, date(a, 1)?, date(a, 2)?);
    let (rate, x, red) = (num(a, 3)?, num(a, 4)?, num(a, 5)?);
    let (f, basis) = (freq_arg(a, 6)?, basis_arg(a, 7)?);
    if rate < 0.0 || red <= 0.0 {
        return Err(CellError::Num);
    }
    Ok((odd_last(sys, s, m, last, f, basis)?, rate, x, red, f))
}

pub(super) fn oddlprice(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let ((dc, acc, dsc), rate, yld, red, f) = odd_last_args(a, c.date_system())?;
    if yld < 0.0 {
        return Err(CellError::Num);
    }
    let coupon = 100.0 * rate / f;
    num_val((red + dc * coupon) / (1.0 + dsc * yld / f) - acc * coupon)
}

pub(super) fn oddlyield(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let ((dc, acc, dsc), rate, pr, red, f) = odd_last_args(a, c.date_system())?;
    if pr <= 0.0 {
        return Err(CellError::Num);
    }
    let coupon = 100.0 * rate / f;
    let paid = pr + acc * coupon;
    if paid <= 0.0 || dsc <= 0.0 {
        return Err(CellError::Num);
    }
    num_val((red + dc * coupon - paid) / paid * f / dsc)
}
