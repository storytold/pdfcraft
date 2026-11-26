use super::day_count::{basis_arg, date, year_frac};
use super::{Arg, CellError, Ctx, R, Value, num, num_val, opt_bool, opt_num};

// ---------------------------------------------------------------------------------------------
// Depreciation
// ---------------------------------------------------------------------------------------------

pub(super) fn sln(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (cost, salvage, life) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if life == 0.0 {
        return Err(CellError::Div0);
    }
    num_val((cost - salvage) / life)
}

pub(super) fn syd(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (cost, salvage, life, per) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?);
    if life <= 0.0 || per <= 0.0 || per > life || salvage < 0.0 {
        return Err(CellError::Num);
    }
    num_val((cost - salvage) * (life - per + 1.0) * 2.0 / (life * (life + 1.0)))
}

pub(super) fn db(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (cost, salvage, life, period) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?.trunc());
    let month = opt_num(a, 4, 12.0)?.trunc();
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 || !(1.0..=12.0).contains(&month) {
        return Err(CellError::Num);
    }
    let max_period = if month == 12.0 { life.trunc() } else { life.trunc() + 1.0 };
    if period > max_period.max(1.0) || period > 1e7 {
        return Err(CellError::Num);
    }
    if cost == 0.0 {
        return num_val(0.0);
    }
    let rate = crate::util::round_half_away(1.0 - (salvage / cost).powf(1.0 / life), 3);
    let mut total = 0.0;
    let mut dep = 0.0;
    let mut p = 1.0;
    while p <= period {
        dep = if p == 1.0 {
            cost * rate * month / 12.0
        } else if p == life.trunc() + 1.0 {
            (cost - total) * rate * (12.0 - month) / 12.0
        } else {
            (cost - total) * rate
        };
        total += dep;
        p += 1.0;
    }
    num_val(dep)
}

pub(super) fn ddb(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (cost, salvage, life, period) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?);
    let factor = opt_num(a, 4, 2.0)?;
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 || factor <= 0.0 || period > life {
        return Err(CellError::Num);
    }
    let mut rate = factor / life;
    let old;
    if rate >= 1.0 {
        rate = 1.0;
        old = if period == 1.0 { cost } else { 0.0 };
    } else {
        old = cost * (1.0 - rate).powf(period - 1.0);
    }
    let new = cost * (1.0 - rate).powf(period);
    let dep = if new < salvage { old - salvage } else { old - new };
    num_val(dep.max(0.0))
}

/// Per-period declining-balance depreciation with optional switch to straight line.
fn vdb_schedule(cost: f64, salvage: f64, life: f64, factor: f64, no_switch: bool, periods: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(periods);
    let mut book = cost;
    let mut sl: Option<f64> = None;
    for i in 0..periods {
        let dep = if let Some(s) = sl {
            s.min((book - salvage).max(0.0))
        } else {
            let ddb = (book * factor / life).min((book - salvage).max(0.0));
            let remaining = life - i as f64;
            let slv = if remaining > 0.0 { (book - salvage) / remaining.max(1.0) } else { 0.0 };
            if !no_switch && slv > ddb {
                sl = Some(slv);
                slv.min((book - salvage).max(0.0))
            } else {
                ddb
            }
        };
        out.push(dep);
        book -= dep;
    }
    out
}

pub(super) fn vdb(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (cost, salvage, life, start, end) = (num(a, 0)?, num(a, 1)?, num(a, 2)?, num(a, 3)?, num(a, 4)?);
    let factor = opt_num(a, 5, 2.0)?;
    let no_switch = opt_bool(a, 6, false)?;
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || start < 0.0 || end < start || end > life || factor <= 0.0 {
        return Err(CellError::Num);
    }
    let periods = end.ceil();
    if periods > 1e7 {
        return Err(CellError::Num);
    }
    let sched = vdb_schedule(cost, salvage, life, factor, no_switch, periods as usize);
    let mut total = 0.0;
    for (i, d) in sched.iter().enumerate() {
        let (p0, p1) = (i as f64, i as f64 + 1.0);
        let overlap = (p1.min(end) - p0.max(start)).max(0.0);
        total += d * overlap;
    }
    num_val(total)
}

pub(super) fn amorlinc(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (cost, purchased, first, salvage) = (num(a, 0)?, date(a, 1)?, date(a, 2)?, num(a, 3)?);
    let (period, rate, basis) = (num(a, 4)?.trunc(), num(a, 5)?, basis_arg(a, 6)?);
    if cost < 0.0 || salvage < 0.0 || salvage > cost || period < 0.0 || rate <= 0.0 || first < purchased {
        return Err(CellError::Num);
    }
    let full = cost * rate;
    let limit = cost - salvage;
    let dep0 = (full * year_frac(c.date_system(), purchased, first, basis)?).min(limit);
    if period == 0.0 {
        return num_val(dep0);
    }
    let remaining = limit - dep0 - (period - 1.0) * full;
    num_val(full.min(remaining.max(0.0)))
}

pub(super) fn amordegrc(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (cost, purchased, first, salvage) = (num(a, 0)?, date(a, 1)?, date(a, 2)?, num(a, 3)?);
    let (period, rate, basis) = (num(a, 4)?.trunc(), num(a, 5)?, basis_arg(a, 6)?);
    if cost < 0.0 || salvage < 0.0 || salvage > cost || period < 0.0 || rate <= 0.0 || first < purchased || period > 1e6 {
        return Err(CellError::Num);
    }
    let life = 1.0 / rate;
    let coef = if life < 3.0 {
        1.0
    } else if life < 5.0 {
        1.5
    } else if life <= 6.0 {
        2.0
    } else {
        2.5
    };
    let r = rate * coef;
    let mut book = cost;
    let mut dep = (year_frac(c.date_system(), purchased, first, basis)? * r * cost).round();
    if period == 0.0 {
        return num_val(dep);
    }
    book -= dep;
    let mut rest = book - salvage;
    let mut n = 1.0;
    while n <= period {
        dep = (r * book).round();
        rest -= dep;
        if rest < 0.0 {
            // The two final periods split what remains; afterwards nothing is left.
            let left = period - n;
            return num_val(if left <= 1.0 { (book * 0.5).round() } else { 0.0 });
        }
        book -= dep;
        n += 1.0;
    }
    num_val(dep)
}
