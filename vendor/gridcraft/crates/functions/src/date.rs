//! Date and time functions.

use gridcraft_core::date::{DateTime, datetime_from_serial, days_in_month, is_leap, serial_from_ymd, serial_in_system};
use gridcraft_core::parse::parse_input;
use gridcraft_core::{CellError, DateSystem, Value};

use crate::util::{R, S, arg, as_array, has, num, num_val, opt_bool, opt_num, scalar, text, to_int};
use crate::{Arg, Ctx, FnSpec};

const MAX_SERIAL: f64 = 2_958_465.0;
/// NETWORKDAYS / WORKDAY: start, end/days scalar; holidays take arrays.
const ND: &[bool] = &[true, true, false];
const NDI: &[bool] = &[true, true, true, false];

/// A date serial argument (truncated to whole days when `whole`). Out of range → `#NUM!`.
fn serial_arg(a: &[Arg], i: usize, sys: DateSystem) -> R<f64> {
    let n = scalar(arg(a, i)?).to_number_in(sys)?;
    if !(0.0..MAX_SERIAL + 1.0).contains(&n) {
        return Err(CellError::Num);
    }
    Ok(n)
}

fn day_arg(a: &[Arg], i: usize, sys: DateSystem) -> R<i64> {
    Ok(serial_arg(a, i, sys)?.floor() as i64)
}

fn dt(sys: DateSystem, serial: f64) -> R<DateTime> {
    datetime_from_serial(sys, serial).ok_or(CellError::Num)
}

fn ymd_serial(sys: DateSystem, y: i64, m: i64, d: i64) -> R<i64> {
    serial_from_ymd(sys, y, m, d).map(|s| s as i64).ok_or(CellError::Num)
}

/// Weekday of a serial, 0 = Sunday.
fn weekday0(sys: DateSystem, serial: i64) -> usize {
    match sys {
        DateSystem::D1900 => (serial + 6).rem_euclid(7) as usize,
        DateSystem::D1904 => (serial + 5).rem_euclid(7) as usize,
    }
}

fn date(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let y = to_int(num(a, 0)?)?;
    let m = to_int(num(a, 1)?)?;
    let d = to_int(num(a, 2)?)?;
    if !(0..10000).contains(&y) || m.abs() > 1_000_000 || d.abs() > 100_000_000 {
        return Err(CellError::Num);
    }
    num_val(ymd_serial(c.date_system(), y, m, d)? as f64)
}

fn time(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let h = num(a, 0)?.trunc();
    let m = num(a, 1)?.trunc();
    let s = num(a, 2)?.trunc();
    if h > 32767.0 || m > 32767.0 || s > 32767.0 {
        return Err(CellError::Num);
    }
    let total = h * 3600.0 + m * 60.0 + s;
    if total < 0.0 {
        return Err(CellError::Num);
    }
    num_val(total.rem_euclid(86400.0) / 86400.0)
}

fn part(a: &[Arg], c: &mut dyn Ctx, f: fn(&DateTime) -> f64) -> R<Value> {
    let sys = c.date_system();
    let s = serial_arg(a, 0, sys)?;
    num_val(f(&dt(sys, s.floor())?))
}

fn year(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    part(a, c, |d| d.year as f64)
}
fn month(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    part(a, c, |d| d.month as f64)
}
fn day(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    part(a, c, |d| d.day as f64)
}

/// Seconds into the day, rounded to the nearest second.
fn day_seconds(a: &[Arg], sys: DateSystem) -> R<i64> {
    let s = serial_arg(a, 0, sys)?;
    let secs = ((s - s.floor()) * 86400.0).round() as i64;
    Ok(if secs >= 86400 { 0 } else { secs })
}

fn hour(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val((day_seconds(a, c.date_system())? / 3600) as f64)
}
fn minute(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val((day_seconds(a, c.date_system())? / 60 % 60) as f64)
}
fn second(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val((day_seconds(a, c.date_system())? % 60) as f64)
}

fn now(_a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(serial_in_system(c.date_system(), c.now_serial()))
}
fn today(_a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    num_val(serial_in_system(c.date_system(), c.now_serial()).floor())
}

fn weekday(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let s = day_arg(a, 0, sys)?;
    let t = to_int(opt_num(a, 1, 1.0)?)?;
    let wd = weekday0(sys, s) as i64;
    let r = match t {
        1 | 17 => wd + 1,
        2 | 11 => (wd + 6) % 7 + 1,
        3 => (wd + 6) % 7,
        12..=16 => (wd + 7 - (t - 10)) % 7 + 1,
        _ => return Err(CellError::Num),
    };
    num_val(r as f64)
}

fn isoweek(sys: DateSystem, s: i64) -> R<i64> {
    let iso_wd = (weekday0(sys, s) + 6) % 7 + 1; // Monday = 1
    let thursday = s - iso_wd as i64 + 4;
    let first = match sys {
        DateSystem::D1900 => 1,
        DateSystem::D1904 => 0,
    };
    let jan1 = if thursday < first {
        // The week belongs to the year before the calendar starts.
        first - 365
    } else {
        ymd_serial(sys, dt(sys, thursday as f64)?.year as i64, 1, 1)?
    };
    Ok((thursday - jan1).div_euclid(7) + 1)
}

fn weeknum(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let s = day_arg(a, 0, sys)?;
    let t = to_int(opt_num(a, 1, 1.0)?)?;
    let start = match t {
        1 | 17 => 0,
        2 | 11 => 1,
        12..=16 => t - 10,
        21 => return num_val(isoweek(sys, s)? as f64),
        _ => return Err(CellError::Num),
    } as usize;
    let y = dt(sys, s as f64)?.year as i64;
    let jan1 = ymd_serial(sys, y, 1, 1).unwrap_or(s.min(1));
    let offset = (weekday0(sys, jan1) + 7 - start) % 7;
    num_val(((s - jan1 + offset as i64).div_euclid(7) + 1) as f64)
}

fn isoweeknum(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    num_val(isoweek(sys, day_arg(a, 0, sys)?)? as f64)
}

fn months_arg(a: &[Arg], i: usize) -> R<i64> {
    let m = to_int(num(a, i)?)?;
    if m.abs() > 200_000 {
        return Err(CellError::Num);
    }
    Ok(m)
}

fn edate(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let d = dt(sys, day_arg(a, 0, sys)? as f64)?;
    let m = months_arg(a, 1)?;
    let total = d.year as i64 * 12 + d.month as i64 - 1 + m;
    let (y, mo) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    if !(1900..=9999).contains(&y) {
        return Err(CellError::Num);
    }
    // Excel keeps serial 0's day 0 and gives February 1900 its real 28 days here, though the
    // sheet's calendar has a 29 February 1900 (serial 60).
    let dd = (d.day as i64).min(days_in_month(y as i32, mo as u32) as i64);
    num_val(ymd_serial(sys, y, mo, dd)? as f64)
}

fn eomonth(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let d = dt(sys, day_arg(a, 0, sys)? as f64)?;
    let m = months_arg(a, 1)?;
    let total = d.year as i64 * 12 + d.month as i64 - 1 + m;
    let (y, mo) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    if !(1900..=9999).contains(&y) {
        return Err(CellError::Num);
    }
    // Like EDATE, February 1900 ends on the 28th (serial 59).
    num_val(ymd_serial(sys, y, mo, days_in_month(y as i32, mo as u32) as i64)? as f64)
}

fn datedif(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let s = day_arg(a, 0, sys)?;
    let e = day_arg(a, 1, sys)?;
    let unit = text(a, 2)?.to_ascii_uppercase();
    if s > e {
        return Err(CellError::Num);
    }
    let d1 = dt(sys, s as f64)?;
    let d2 = dt(sys, e as f64)?;
    let months = (d2.year as i64 - d1.year as i64) * 12 + d2.month as i64 - d1.month as i64 - i64::from(d2.day < d1.day);
    let r = match unit.as_str() {
        "D" => e - s,
        "M" => months,
        "Y" => months / 12,
        "YM" => months % 12,
        "MD" => {
            if d2.day >= d1.day {
                (d2.day - d1.day) as i64
            } else {
                let (py, pm) = if d2.month == 1 { (d2.year - 1, 12) } else { (d2.year, d2.month - 1) };
                days_in_month(py, pm) as i64 - d1.day as i64 + d2.day as i64
            }
        }
        "YD" => {
            let later = (d1.month, d1.day) > (d2.month, d2.day);
            let y = if later { d2.year - 1 } else { d2.year };
            let dd = d1.day.min(days_in_month(y, d1.month).max(1));
            let shifted = ymd_serial(sys, y as i64, d1.month as i64, dd as i64)?;
            e - shifted
        }
        _ => return Err(CellError::Num),
    };
    num_val(r as f64)
}

fn parsed_text(a: &[Arg], c: &mut dyn Ctx) -> R<(f64, &'static str)> {
    let v = scalar(arg(a, 0)?);
    let Value::Text(t) = v else {
        return Err(match v {
            Value::Error(e) => e,
            _ => CellError::Value,
        });
    };
    let p = parse_input(&t, c.date_system());
    match (p.value, p.format) {
        (Value::Number(n), Some(f)) if f.contains(['d', 'y', 'h']) => Ok((n, f)),
        _ => Err(CellError::Value),
    }
}

fn datevalue(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (n, f) = parsed_text(a, c)?;
    if !f.contains(['d', 'y']) {
        return Err(CellError::Value);
    }
    num_val(n.floor())
}

fn timevalue(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let (n, _) = parsed_text(a, c)?;
    num_val(n - n.floor())
}

fn days(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    num_val((day_arg(a, 0, sys)? - day_arg(a, 1, sys)?) as f64)
}

fn last_of_feb(d: &DateTime) -> bool {
    d.month == 2 && d.day == days_in_month(d.year, 2)
}

fn days360(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let d1 = dt(sys, day_arg(a, 0, sys)? as f64)?;
    let d2 = dt(sys, day_arg(a, 1, sys)? as f64)?;
    let european = opt_bool(a, 2, false)?;
    num_val(days360_core(&d1, &d2, european) as f64)
}

fn days360_core(d1: &DateTime, d2: &DateTime, european: bool) -> i64 {
    let (mut a, mut b) = (d1.day as i64, d2.day as i64);
    if european {
        a = a.min(30);
        b = b.min(30);
    } else {
        if a == 31 || last_of_feb(d1) {
            a = 30;
        }
        if b == 31 && a >= 30 {
            b = 30;
        }
    }
    (d2.year as i64 - d1.year as i64) * 360 + (d2.month as i64 - d1.month as i64) * 30 + b - a
}

/// Weekend mask indexed by weekday (0 = Sunday).
fn weekend_mask(v: Option<&Arg>) -> R<[bool; 7]> {
    let Some(arg) = v else { return Ok([true, false, false, false, false, false, true]) };
    match scalar(arg) {
        Value::Empty if !arg.from_ref => Ok([true, false, false, false, false, false, true]),
        Value::Text(t) => {
            let b = t.as_bytes();
            // All seven days marked as the weekend is a valid string: Microsoft documents
            // NETWORKDAYS.INTL as "Using 1111111 will always return 0" and WORKDAY.INTL as
            // rejecting it, which `workday_core`'s `per_week == 0` guard already implements.
            if b.len() != 7 || !b.iter().all(|c| *c == b'0' || *c == b'1') {
                return Err(CellError::Value);
            }
            // String order is Monday..Sunday.
            let mut m = [false; 7];
            for (i, c) in b.iter().enumerate() {
                m[(i + 1) % 7] = *c == b'1';
            }
            Ok(m)
        }
        Value::Error(e) => Err(e),
        other => {
            let k = to_int(other.to_number()?)?;
            let mut m = [false; 7];
            match k {
                1..=7 => {
                    m[((k + 5) % 7) as usize] = true;
                    m[((k + 6) % 7) as usize] = true;
                }
                11..=17 => m[(k - 11) as usize] = true,
                _ => return Err(CellError::Num),
            }
            Ok(m)
        }
    }
}

fn holidays(v: Option<&Arg>, sys: DateSystem) -> R<Vec<i64>> {
    let mut out = Vec::new();
    if let Some(a) = v {
        for x in as_array(&a.value).iter() {
            match x {
                Value::Empty => {}
                Value::Error(e) => return Err(*e),
                other => {
                    let n = other.to_number_in(sys)?;
                    if !(0.0..MAX_SERIAL + 1.0).contains(&n) {
                        return Err(CellError::Num);
                    }
                    out.push(n.floor() as i64);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Working days in [a, b] (a ≤ b) ignoring holidays.
fn count_workdays(sys: DateSystem, a: i64, b: i64, mask: &[bool; 7]) -> i64 {
    let per_week = mask.iter().filter(|w| !**w).count() as i64;
    let total = b - a + 1;
    let weeks = total / 7;
    let mut n = weeks * per_week;
    let mut d = a + weeks * 7;
    while d <= b {
        if !mask[weekday0(sys, d)] {
            n += 1;
        }
        d += 1;
    }
    n
}

fn networkdays_core(a: &[Arg], c: &mut dyn Ctx, mask: [bool; 7], hol: Option<&Arg>) -> R<Value> {
    let sys = c.date_system();
    let s = day_arg(a, 0, sys)?;
    let e = day_arg(a, 1, sys)?;
    let hs = holidays(hol, sys)?;
    let (lo, hi, sign) = if s <= e { (s, e, 1) } else { (e, s, -1) };
    let mut n = count_workdays(sys, lo, hi, &mask);
    n -= hs.iter().filter(|&&h| h >= lo && h <= hi && !mask[weekday0(sys, h)]).count() as i64;
    num_val((n * sign) as f64)
}

fn networkdays(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    networkdays_core(a, c, weekend_mask(None)?, a.get(2))
}

fn networkdays_intl(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let mask = weekend_mask(a.get(2))?;
    networkdays_core(a, c, mask, a.get(3))
}

/// Moves `n` (> 0) working days from `start` in direction `dir`, ignoring holidays.
fn advance(sys: DateSystem, start: i64, n: i64, dir: i64, mask: &[bool; 7], per_week: i64) -> i64 {
    let weeks = (n - 1) / per_week;
    let mut cur = start + dir * 7 * weeks;
    let mut rem = n - weeks * per_week;
    while rem > 0 {
        cur += dir;
        if !mask[weekday0(sys, cur)] {
            rem -= 1;
        }
    }
    cur
}

fn workday_core(a: &[Arg], c: &mut dyn Ctx, mask: [bool; 7], hol: Option<&Arg>) -> R<Value> {
    let sys = c.date_system();
    let start = day_arg(a, 0, sys)?;
    let days = num(a, 1)?.trunc();
    if days.abs() > 1e7 {
        return Err(CellError::Num);
    }
    let days = days as i64;
    let hs = holidays(hol, sys)?;
    let per_week = mask.iter().filter(|w| !**w).count() as i64;
    if per_week == 0 {
        return Err(CellError::Value);
    }
    if days == 0 {
        return num_val(start as f64);
    }
    // The holidays on working days, still sorted (`holidays` sorts them), so each step counts
    // the ones it passed with two binary searches rather than a scan of every holiday: a run of
    // consecutive holidays takes one step per holiday.
    let hs: Vec<i64> = hs.into_iter().filter(|&h| !mask[weekday0(sys, h)]).collect();
    let dir = days.signum();
    let mut cur = start;
    let mut rem = days.abs();
    while rem > 0 {
        let next = advance(sys, cur, rem, dir, &mask, per_week);
        let (lo, hi) = if dir > 0 { (cur + 1, next) } else { (next, cur - 1) };
        rem = hs.partition_point(|&h| h <= hi).saturating_sub(hs.partition_point(|&h| h < lo)) as i64;
        cur = next;
        if !(0..=MAX_SERIAL as i64).contains(&cur) {
            return Err(CellError::Num);
        }
    }
    num_val(cur as f64)
}

fn workday(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    workday_core(a, c, weekend_mask(None)?, a.get(2))
}

fn workday_intl(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let mask = weekend_mask(a.get(2))?;
    workday_core(a, c, mask, a.get(3))
}

/// Excel's YEARFRAC for a basis (0–4) between two whole-day serials.
pub(crate) fn yearfrac_core(sys: DateSystem, s: i64, e: i64, basis: i64) -> R<f64> {
    let (s, e) = if s <= e { (s, e) } else { (e, s) };
    let d1 = dt(sys, s as f64)?;
    let d2 = dt(sys, e as f64)?;
    let days = (e - s) as f64;
    Ok(match basis {
        0 => {
            let (mut a, mut b) = (d1.day as i64, d2.day as i64);
            // Excel tests the end-31 rule against the original start day, before a February end moves to 30.
            let start_30 = a >= 30;
            if last_of_feb(&d1) && last_of_feb(&d2) {
                b = 30;
            }
            if last_of_feb(&d1) || a == 31 {
                a = 30;
            }
            if start_30 && b == 31 {
                b = 30;
            }
            ((d2.year as i64 - d1.year as i64) * 360 + (d2.month as i64 - d1.month as i64) * 30 + b - a) as f64 / 360.0
        }
        1 => {
            let within_year = d1.year == d2.year || (d2.year == d1.year + 1 && (d1.month > d2.month || (d1.month == d2.month && d1.day >= d2.day)));
            if within_year {
                let year_len = if (d1.year == d2.year && is_leap(d1.year)) || feb29_between(sys, s, e, &d1, &d2) || (d2.month == 2 && d2.day == 29) {
                    366.0
                } else {
                    365.0
                };
                days / year_len
            } else {
                let years = (d2.year - d1.year + 1) as f64;
                let span = ymd_serial(sys, d2.year as i64 + 1, 1, 1).unwrap_or(e + 1) - ymd_serial(sys, d1.year as i64, 1, 1).unwrap_or(s);
                days / (span as f64 / years)
            }
        }
        2 => days / 360.0,
        3 => days / 365.0,
        4 => days360_core(&d1, &d2, true) as f64 / 360.0,
        _ => return Err(CellError::Num),
    })
}

fn feb29_between(sys: DateSystem, s: i64, e: i64, d1: &DateTime, d2: &DateTime) -> bool {
    (d1.year..=d2.year).any(|y| is_leap(y) && serial_from_ymd(sys, y as i64, 2, 29).is_some_and(|f| (f as i64) >= s && (f as i64) <= e))
}

fn yearfrac(a: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let sys = c.date_system();
    let s = day_arg(a, 0, sys)?;
    let e = day_arg(a, 1, sys)?;
    let basis = if has(a, 2) { to_int(num(a, 2)?)? } else { 0 };
    num_val(yearfrac_core(sys, s, e, basis)?)
}

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!("DATE", 3, 3, DateTime, S, "DATE(year, month, day)", "Builds a date serial number from a year, month and day.", date),
        f!("TIME", 3, 3, DateTime, S, "TIME(hour, minute, second)", "Builds a time (a fraction of a day) from hours, minutes and seconds.", time),
        f!("YEAR", 1, 1, DateTime, S, "YEAR(serial_number)", "Returns the year of a date.", year),
        f!("MONTH", 1, 1, DateTime, S, "MONTH(serial_number)", "Returns the month (1-12) of a date.", month),
        f!("DAY", 1, 1, DateTime, S, "DAY(serial_number)", "Returns the day of the month of a date.", day),
        f!("HOUR", 1, 1, DateTime, S, "HOUR(serial_number)", "Returns the hour (0-23) of a time.", hour),
        f!("MINUTE", 1, 1, DateTime, S, "MINUTE(serial_number)", "Returns the minute (0-59) of a time.", minute),
        f!("SECOND", 1, 1, DateTime, S, "SECOND(serial_number)", "Returns the second (0-59) of a time.", second),
        f!(volatile; "NOW", 0, 0, DateTime, S, "NOW()", "Returns the current date and time.", now),
        f!(volatile; "TODAY", 0, 0, DateTime, S, "TODAY()", "Returns the current date.", today),
        f!("WEEKDAY", 1, 2, DateTime, S, "WEEKDAY(serial_number, [return_type])", "Returns the day of the week of a date as a number.", weekday),
        f!("WEEKNUM", 1, 2, DateTime, S, "WEEKNUM(serial_number, [return_type])", "Returns the week number of a date within its year.", weeknum),
        f!("ISOWEEKNUM", 1, 1, DateTime, S, "ISOWEEKNUM(date)", "Returns the ISO 8601 week number of a date.", isoweeknum),
        f!("EDATE", 2, 2, DateTime, S, "EDATE(start_date, months)", "Returns the date a number of months before or after a date.", edate),
        f!("EOMONTH", 2, 2, DateTime, S, "EOMONTH(start_date, months)", "Returns the last day of the month a number of months away.", eomonth),
        f!(
            "DATEDIF",
            3,
            3,
            DateTime,
            S,
            "DATEDIF(start_date, end_date, unit)",
            "Counts whole years, months or days between two dates. unit: Y, M, D, MD, YM or YD.",
            datedif
        ),
        f!("DATEVALUE", 1, 1, DateTime, S, "DATEVALUE(date_text)", "Converts a date written as text into a date serial number.", datevalue),
        f!("TIMEVALUE", 1, 1, DateTime, S, "TIMEVALUE(time_text)", "Converts a time written as text into a fraction of a day.", timevalue),
        f!("DAYS", 2, 2, DateTime, S, "DAYS(end_date, start_date)", "Returns the number of days between two dates.", days),
        f!(
            "DAYS360",
            2,
            3,
            DateTime,
            S,
            "DAYS360(start_date, end_date, [method])",
            "Counts days between two dates on a 360-day year (US or European method).",
            days360
        ),
        f!(
            "NETWORKDAYS",
            2,
            3,
            DateTime,
            ND,
            "NETWORKDAYS(start_date, end_date, [holidays])",
            "Counts working days (Monday to Friday) between two dates, excluding holidays.",
            networkdays
        ),
        f!(
            "NETWORKDAYS.INTL",
            2,
            4,
            DateTime,
            NDI,
            "NETWORKDAYS.INTL(start_date, end_date, [weekend], [holidays])",
            "Counts working days between two dates with a custom weekend, excluding holidays.",
            networkdays_intl
        ),
        f!(
            "WORKDAY",
            2,
            3,
            DateTime,
            ND,
            "WORKDAY(start_date, days, [holidays])",
            "Returns the date a number of working days before or after a date.",
            workday
        ),
        f!(
            "WORKDAY.INTL",
            2,
            4,
            DateTime,
            NDI,
            "WORKDAY.INTL(start_date, days, [weekend], [holidays])",
            "Returns the date a number of working days away with a custom weekend.",
            workday_intl
        ),
        f!(
            "YEARFRAC",
            2,
            3,
            DateTime,
            S,
            "YEARFRAC(start_date, end_date, [basis])",
            "Returns the fraction of a year between two dates under a day-count basis.",
            yearfrac
        ),
    ]
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, Value};

    use crate::util::testutil::*;

    fn d(y: f64, m: f64, dd: f64) -> f64 {
        match ev("DATE", vec![n(y), n(m), n(dd)]) {
            Value::Number(x) => x,
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn construct() {
        assert_eq!(d(2026.0, 10.0, 7.0), 46302.0);
        assert_eq!(d(2020.0, 13.0, 1.0), 44197.0);
        assert_eq!(d(2020.0, 3.0, 0.0), 43890.0);
        assert_eq!(d(120.0, 1.0, 1.0), 43831.0);
        assert_eq!(d(2020.0, 1.0, -1.0), 43829.0);
        is_err(ev("DATE", vec![n(10000.0), n(1.0), n(1.0)]), CellError::Num);
        is_err(ev("DATE", vec![n(-1.0), n(1.0), n(1.0)]), CellError::Num);
        is_err(ev("DATE", vec![n(1900.0), n(1.0), n(-5.0)]), CellError::Num);
        close(ev("TIME", vec![n(12.0), n(0.0), n(0.0)]), 0.5);
        close(ev("TIME", vec![n(25.0), n(0.0), n(0.0)]), 1.0 / 24.0);
        is_err(ev("TIME", vec![n(0.0), n(-1.0), n(0.0)]), CellError::Num);
    }

    #[test]
    fn time_negative() {
        is_err(ev("TIME", vec![n(0.0), n(0.0), n(-1.0)]), CellError::Num);
        close(ev("TIME", vec![n(1.0), n(-30.0), n(0.0)]), 1.0 / 48.0);
    }

    #[test]
    fn parts() {
        close(ev("YEAR", vec![n(46302.75)]), 2026.0);
        close(ev("MONTH", vec![n(46302.75)]), 10.0);
        close(ev("DAY", vec![n(46302.75)]), 7.0);
        close(ev("HOUR", vec![n(46302.75)]), 18.0);
        close(ev("MINUTE", vec![n(0.5 + 61.0 / 1440.0)]), 1.0);
        close(ev("SECOND", vec![n(12.5 / 86400.0)]), 13.0);
        close(ev("HOUR", vec![n(0.999_999_9)]), 0.0);
        close(ev("YEAR", vec![t("2020-05-06")]), 2020.0);
        close(ev("DAY", vec![n(0.0)]), 0.0);
        close(ev("MONTH", vec![n(60.0)]), 2.0);
        close(ev("DAY", vec![n(60.0)]), 29.0);
        is_err(ev("YEAR", vec![n(-1.0)]), CellError::Num);
        is_err(ev("YEAR", vec![t("abc")]), CellError::Value);
        close(ev("NOW", vec![]), 46302.5);
        close(ev("TODAY", vec![]), 46302.0);
    }

    #[test]
    fn weekdays() {
        // 2026-10-07 is a Wednesday.
        close(ev("WEEKDAY", vec![n(46302.0)]), 4.0);
        close(ev("WEEKDAY", vec![n(46302.0), n(2.0)]), 3.0);
        close(ev("WEEKDAY", vec![n(46302.0), n(3.0)]), 2.0);
        close(ev("WEEKDAY", vec![n(46302.0), n(13.0)]), 1.0);
        close(ev("WEEKDAY", vec![n(46302.0), n(16.0)]), 5.0);
        close(ev("WEEKDAY", vec![n(46302.0), n(17.0)]), 4.0);
        is_err(ev("WEEKDAY", vec![n(46302.0), n(4.0)]), CellError::Num);
        close(ev("WEEKDAY", vec![n(1.0)]), 1.0);
        // 2024-01-01 Monday.
        let jan1 = d(2024.0, 1.0, 1.0);
        close(ev("WEEKNUM", vec![n(jan1)]), 1.0);
        close(ev("WEEKNUM", vec![n(d(2024.0, 1.0, 7.0))]), 2.0); // Sunday starts week 2
        close(ev("WEEKNUM", vec![n(d(2024.0, 1.0, 7.0)), n(2.0)]), 1.0);
        close(ev("WEEKNUM", vec![n(d(2026.0, 10.0, 7.0))]), 41.0);
        close(ev("WEEKNUM", vec![n(d(2021.0, 1.0, 1.0)), n(21.0)]), 53.0);
        close(ev("ISOWEEKNUM", vec![n(d(2021.0, 1.0, 1.0))]), 53.0);
        close(ev("ISOWEEKNUM", vec![n(d(2026.0, 10.0, 7.0))]), 41.0);
        close(ev("ISOWEEKNUM", vec![n(d(2024.0, 12.0, 30.0))]), 1.0);
        is_err(ev("WEEKNUM", vec![n(1.0), n(3.0)]), CellError::Num);
    }

    #[test]
    fn month_math() {
        close(ev("EDATE", vec![n(d(2020.0, 1.0, 31.0)), n(1.0)]), d(2020.0, 2.0, 29.0));
        close(ev("EDATE", vec![n(d(2020.0, 3.0, 15.0)), n(-14.0)]), d(2019.0, 1.0, 15.0));
        close(ev("EDATE", vec![n(d(2020.0, 3.0, 15.5)), n(1.9)]), d(2020.0, 4.0, 15.0));
        close(ev("EOMONTH", vec![n(d(2020.0, 1.0, 15.0)), n(1.0)]), d(2020.0, 2.0, 29.0));
        close(ev("EOMONTH", vec![n(d(2020.0, 1.0, 15.0)), n(-1.0)]), d(2019.0, 12.0, 31.0));
        close(ev("EOMONTH", vec![n(d(2020.0, 1.0, 15.0)), n(0.0)]), d(2020.0, 1.0, 31.0));
        is_err(ev("EOMONTH", vec![n(1.0), n(-12.0)]), CellError::Num);
        is_err(ev("EDATE", vec![n(1.0), n(1e12)]), CellError::Num);
    }

    #[test]
    fn fictitious_1900_02_29() {
        // Excel's 1900 calendar has 29 February 1900 = serial 60, and DATE counts day and month
        // overflow through it.
        assert_eq!(d(1900.0, 2.0, 29.0), 60.0);
        assert_eq!(d(1900.0, 3.0, 0.0), 60.0);
        assert_eq!(d(1900.0, 2.0, 28.0), 59.0);
        assert_eq!(d(1900.0, 3.0, 1.0), 61.0);
        assert_eq!(d(1900.0, 2.0, 30.0), 61.0);
        assert_eq!(d(1900.0, 1.0, 60.0), 60.0);
        assert_eq!(d(1900.0, 4.0, -31.0), 60.0);
        assert_eq!(d(1901.0, -10.0, 29.0), 60.0);
        assert_eq!(d(0.0, 2.0, 29.0), 60.0);
        assert_eq!(d(1900.0, 1.0, 0.0), 0.0);
        close(ev("DAY", vec![n(d(1900.0, 3.0, 0.0))]), 29.0);
        close(ev("DATEVALUE", vec![t("1900-02-29")]), 60.0);
        close(ev("DATEVALUE", vec![t("2/29/1900")]), 60.0);
        close(ev("DATEVALUE", vec![t("3/1/1900")]), 61.0);
        is_err(ev("DATEVALUE", vec![t("2/29/1901")]), CellError::Value);
        // EDATE and EOMONTH read it as 29 February, but end February 1900 on the 28th.
        close(ev("EOMONTH", vec![n(d(1900.0, 1.0, 15.0)), n(1.0)]), 59.0);
        close(ev("EOMONTH", vec![n(60.0), n(0.0)]), 59.0);
        close(ev("EOMONTH", vec![n(d(1900.0, 3.0, 15.0)), n(-1.0)]), 59.0);
        close(ev("EOMONTH", vec![n(60.0), n(12.0)]), d(1901.0, 2.0, 28.0));
        close(ev("EDATE", vec![n(d(1900.0, 1.0, 31.0)), n(1.0)]), 59.0);
        close(ev("EDATE", vec![n(d(1900.0, 3.0, 31.0)), n(-1.0)]), 59.0);
        close(ev("EDATE", vec![n(60.0), n(1.0)]), d(1900.0, 3.0, 29.0));
        close(ev("EDATE", vec![n(60.0), n(12.0)]), d(1901.0, 2.0, 28.0));
    }

    #[test]
    fn edate_eomonth_early_1900() {
        // Excel 365: serial 0 is 0 January 1900 and February 1900 has 28 days here.
        close(ev("EDATE", vec![n(0.0), n(0.0)]), 0.0);
        close(ev("EDATE", vec![n(0.0), n(1.0)]), 31.0);
        close(ev("EDATE", vec![n(0.0), n(12.0)]), 366.0);
        close(ev("EOMONTH", vec![n(0.0), n(1.0)]), 59.0);
        close(ev("EDATE", vec![n(31.0), n(1.0)]), 59.0);
        close(ev("EOMONTH", vec![n(59.0), n(0.0)]), 59.0);
        // Unchanged, and the same as Excel.
        close(ev("EOMONTH", vec![n(0.0), n(0.0)]), 31.0);
        is_err(ev("EOMONTH", vec![n(0.0), n(-1.0)]), CellError::Num);
        close(ev("EDATE", vec![n(59.0), n(1.0)]), 88.0);
        close(ev("EDATE", vec![n(60.0), n(1.0)]), 89.0);
        close(ev("EDATE", vec![n(61.0), n(-1.0)]), 32.0);
        close(ev("EOMONTH", vec![n(31.0), n(0.0)]), 31.0);
    }

    #[test]
    fn datedif_units() {
        let s = n(d(2001.0, 6.0, 1.0));
        let e = n(d(2002.0, 8.0, 15.0));
        close(ev("DATEDIF", vec![s.clone(), e.clone(), t("Y")]), 1.0);
        close(ev("DATEDIF", vec![s.clone(), e.clone(), t("M")]), 14.0);
        close(ev("DATEDIF", vec![s.clone(), e.clone(), t("D")]), 440.0);
        close(ev("DATEDIF", vec![s.clone(), e.clone(), t("YM")]), 2.0);
        close(ev("DATEDIF", vec![s.clone(), e.clone(), t("MD")]), 14.0);
        close(ev("DATEDIF", vec![s.clone(), e.clone(), t("yd")]), 75.0);
        close(ev("DATEDIF", vec![n(d(2020.0, 1.0, 31.0)), n(d(2020.0, 3.0, 1.0)), t("M")]), 1.0);
        close(ev("DATEDIF", vec![n(d(2020.0, 1.0, 31.0)), n(d(2020.0, 3.0, 1.0)), t("MD")]), -1.0);
        is_err(ev("DATEDIF", vec![e.clone(), s.clone(), t("D")]), CellError::Num);
        is_err(ev("DATEDIF", vec![s, e, t("X")]), CellError::Num);
    }

    #[test]
    fn text_conversion() {
        close(ev("DATEVALUE", vec![t("2020-01-01")]), 43831.0);
        close(ev("DATEVALUE", vec![t("1/2/2020 10:00")]), 43832.0);
        close(ev("DATEVALUE", vec![t("4-Mar-2026")]), d(2026.0, 3.0, 4.0));
        is_err(ev("DATEVALUE", vec![t("hello")]), CellError::Value);
        is_err(ev("DATEVALUE", vec![t("123")]), CellError::Value);
        is_err(ev("DATEVALUE", vec![n(43831.0)]), CellError::Value);
        close(ev("TIMEVALUE", vec![t("6:00 PM")]), 0.75);
        close(ev("TIMEVALUE", vec![t("1/2/2020 12:00")]), 0.5);
        is_err(ev("TIMEVALUE", vec![t("x")]), CellError::Value);
    }

    /// Security: WORKDAY counts the holidays each step passes by binary search, so a long run of
    /// consecutive holidays is linear work, not one scan of every holiday per holiday.
    #[test]
    fn workday_over_many_consecutive_holidays() {
        let start = d(2000.0, 1.0, 3.0);
        let hol: Vec<f64> = (1..=200_000).map(|i| start + f64::from(i)).collect();
        let out = ev("WORKDAY.INTL", vec![n(start), n(1.0), n(11.0), av(col(&hol))]);
        // Sundays are the only weekend (11): 200,000 days of holidays are skipped, landing on the
        // first working day after them.
        let after = start + 200_001.0;
        let sunday = ev("WEEKDAY", vec![n(after), n(2.0)]) == nv(7.0);
        close(out, if sunday { after + 1.0 } else { after });
        // Holidays on a weekend don't push the result out.
        let fri = d(2026.0, 10.0, 9.0);
        close(ev("WORKDAY", vec![n(fri), n(1.0), av(col(&[fri + 1.0, fri + 3.0]))]), fri + 4.0);
    }

    #[test]
    fn day_counts() {
        close(ev("DAYS", vec![n(d(2021.0, 3.0, 15.0)), n(d(2021.0, 2.0, 1.0))]), 42.0);
        close(ev("DAYS", vec![t("2021-02-01"), t("2021-03-15")]), -42.0);
        close(ev("DAYS360", vec![n(d(2011.0, 1.0, 30.0)), n(d(2011.0, 12.0, 31.0))]), 330.0);
        close(ev("DAYS360", vec![n(d(2011.0, 1.0, 29.0)), n(d(2011.0, 12.0, 31.0))]), 332.0);
        close(ev("DAYS360", vec![n(d(2011.0, 1.0, 29.0)), n(d(2011.0, 12.0, 31.0)), b(true)]), 331.0);
        close(ev("DAYS360", vec![n(d(2011.0, 1.0, 30.0)), n(d(2011.0, 12.0, 31.0)), b(true)]), 330.0);
        close(ev("DAYS360", vec![n(d(2011.0, 2.0, 28.0)), n(d(2011.0, 3.0, 31.0))]), 30.0);
        close(ev("DAYS360", vec![n(d(2011.0, 1.0, 1.0)), n(d(2011.0, 1.0, 31.0))]), 30.0);
    }

    #[test]
    fn working_days() {
        let s = d(2012.0, 10.0, 1.0);
        let e = d(2013.0, 3.0, 1.0);
        close(ev("NETWORKDAYS", vec![n(s), n(e)]), 110.0);
        close(ev("NETWORKDAYS", vec![n(s), n(e), av(col(&[d(2012.0, 11.0, 22.0)]))]), 109.0);
        close(ev("NETWORKDAYS", vec![n(s), n(e), av(col(&[d(2012.0, 11.0, 22.0), d(2012.0, 12.0, 4.0), d(2013.0, 1.0, 21.0)]))]), 107.0);
        close(ev("NETWORKDAYS", vec![n(e), n(s)]), -110.0);
        close(ev("NETWORKDAYS.INTL", vec![n(d(2006.0, 1.0, 1.0)), n(d(2006.0, 1.0, 31.0))]), 22.0);
        is_err(ev("NETWORKDAYS.INTL", vec![n(d(2006.0, 1.0, 1.0)), n(d(2006.0, 2.0, 1.0)), n(-1.0)]), CellError::Num);
        close(ev("NETWORKDAYS.INTL", vec![n(d(2006.0, 1.0, 1.0)), n(d(2006.0, 2.0, 1.0)), n(7.0)]), 24.0);
        close(ev("NETWORKDAYS.INTL", vec![n(d(2006.0, 1.0, 1.0)), n(d(2006.0, 2.0, 1.0)), t("0000110")]), 24.0);
        close(ev("NETWORKDAYS.INTL", vec![n(d(2006.0, 1.0, 1.0)), n(d(2006.0, 1.0, 31.0)), n(11.0)]), 26.0);
        // An all-weekend string is valid: no day is a workday, so the count is 0.
        close(ev("NETWORKDAYS.INTL", vec![n(1.0), n(10.0), t("1111111")]), 0.0);
        // WORKDAY.INTL rejects it (there is no working day to move to).
        is_err(ev("WORKDAY.INTL", vec![n(1.0), n(10.0), t("1111111")]), CellError::Value);
        is_err(ev("NETWORKDAYS.INTL", vec![n(1.0), n(10.0), n(8.0)]), CellError::Num);
        // WORKDAY
        close(ev("WORKDAY", vec![n(d(2008.0, 10.0, 1.0)), n(151.0)]), d(2009.0, 4.0, 30.0));
        let hol = av(col(&[d(2008.0, 11.0, 26.0), d(2008.0, 12.0, 4.0), d(2009.0, 1.0, 21.0)]));
        close(ev("WORKDAY", vec![n(d(2008.0, 10.0, 1.0)), n(151.0), hol]), d(2009.0, 5.0, 5.0));
        close(ev("WORKDAY", vec![n(d(2026.0, 10.0, 9.0)), n(1.0)]), d(2026.0, 10.0, 12.0));
        close(ev("WORKDAY", vec![n(d(2026.0, 10.0, 12.0)), n(-1.0)]), d(2026.0, 10.0, 9.0));
        close(ev("WORKDAY", vec![n(d(2026.0, 10.0, 10.0)), n(0.0)]), d(2026.0, 10.0, 10.0));
        is_err(ev("WORKDAY.INTL", vec![n(d(2012.0, 1.0, 1.0)), n(30.0), n(0.0)]), CellError::Num);
        close(ev("WORKDAY.INTL", vec![n(d(2012.0, 1.0, 1.0)), n(90.0), n(11.0)]), d(2012.0, 4.0, 14.0));
        close(ev("WORKDAY.INTL", vec![n(d(2012.0, 1.0, 1.0)), n(30.0), n(17.0)]), d(2012.0, 2.0, 5.0));
        is_err(ev("WORKDAY", vec![n(d(2026.0, 1.0, 1.0)), n(1e9)]), CellError::Num);
        is_err(ev("WORKDAY", vec![n(d(2026.0, 1.0, 1.0)), n(3e6)]), CellError::Num);
    }

    #[test]
    fn yearfrac_bases() {
        let s = n(d(2012.0, 1.0, 1.0));
        let e = n(d(2012.0, 7.0, 30.0));
        close(ev("YEARFRAC", vec![s.clone(), e.clone()]), 0.580_555_555_555_555_6);
        close(ev("YEARFRAC", vec![s.clone(), e.clone(), n(1.0)]), 0.576_502_732_240_437_2);
        close(ev("YEARFRAC", vec![s.clone(), e.clone(), n(2.0)]), 0.586_111_111_111_111_1);
        close(ev("YEARFRAC", vec![s.clone(), e.clone(), n(3.0)]), 0.578_082_191_780_821_9);
        close(ev("YEARFRAC", vec![s.clone(), e.clone(), n(4.0)]), 0.580_555_555_555_555_6);
        close(ev("YEARFRAC", vec![e.clone(), s.clone(), n(1.0)]), 0.576_502_732_240_437_2);
        is_err(ev("YEARFRAC", vec![s.clone(), e, n(5.0)]), CellError::Num);
        // Multi-year actual/actual uses the average year length.
        close(ev("YEARFRAC", vec![n(d(2000.0, 1.0, 1.0)), n(d(2003.0, 1.0, 1.0)), n(1.0)]), 1096.0 / (1461.0 / 4.0));
        close(ev("YEARFRAC", vec![n(d(2011.0, 2.0, 28.0)), n(d(2012.0, 2.0, 29.0)), n(0.0)]), 1.0);
        close(ev("YEARFRAC", vec![n(d(2011.0, 3.0, 1.0)), n(d(2012.0, 2.0, 29.0)), n(1.0)]), 365.0 / 366.0);
        close(ev("YEARFRAC", vec![s, n(d(2012.0, 1.0, 1.0))]), 0.0);
    }

    #[test]
    fn yearfrac_basis0_feb_end_to_31() {
        // Excel 16: a February-end start keeps its original day for the end-31 test, so 31 days.
        close(ev("YEARFRAC", vec![n(d(2021.0, 2.0, 28.0)), n(d(2021.0, 3.0, 31.0)), n(0.0)]), 0.086_111_111_111_111_11);
        close(ev("YEARFRAC", vec![n(d(2020.0, 2.0, 29.0)), n(d(2020.0, 3.0, 31.0)), n(0.0)]), 0.086_111_111_111_111_11);
        close(ev("YEARFRAC", vec![n(d(2021.0, 3.0, 31.0)), n(d(2021.0, 2.0, 28.0)), n(0.0)]), 0.086_111_111_111_111_11);
        close(ev("YEARFRAC", vec![n(d(2021.0, 1.0, 31.0)), n(d(2021.0, 2.0, 28.0)), n(0.0)]), 0.077_777_777_777_777_78);
        close(ev("YEARFRAC", vec![n(d(2021.0, 1.0, 30.0)), n(d(2021.0, 3.0, 31.0)), n(0.0)]), 1.0 / 6.0);
        close(ev("YEARFRAC", vec![n(d(2020.0, 2.0, 29.0)), n(d(2021.0, 2.0, 28.0)), n(0.0)]), 1.0);
        close(ev("YEARFRAC", vec![n(d(2021.0, 2.0, 28.0)), n(d(2021.0, 2.0, 28.0)), n(0.0)]), 0.0);
    }

    #[test]
    fn date_lifting() {
        let v = ev("YEAR", vec![av(row(&[43831.0, 46302.0]))]);
        assert_eq!(rows_of(&v), vec![vec![nv(2020.0), nv(2026.0)]]);
    }

    #[test]
    fn system_1904() {
        let mut ctx = TestCtx { sys: gridcraft_core::DateSystem::D1904, ..TestCtx::default() };
        let spec = crate::lookup("DATE").unwrap();
        close(crate::call(spec, &[n(2000.0), n(1.0), n(1.0)], &mut ctx), 35064.0);
        let spec = crate::lookup("WEEKDAY").unwrap();
        close(crate::call(spec, &[n(0.0)], &mut ctx), 6.0);
        let spec = crate::lookup("YEAR").unwrap();
        close(crate::call(spec, &[n(0.0)], &mut ctx), 1904.0);
        // The clock is 2026-10-07 12:00; in the 1904 system that is serial 44840.5.
        close(crate::call(crate::lookup("NOW").unwrap(), &[], &mut ctx), 44840.5);
        let today = crate::call(crate::lookup("TODAY").unwrap(), &[], &mut ctx);
        close(today.clone(), 44840.0);
        close(crate::call(spec, &[crate::Arg::val(today)], &mut ctx), 2026.0);
        // No fictitious day here: 1904 is a real leap year and 1900 is before the epoch.
        let spec = crate::lookup("DATE").unwrap();
        close(crate::call(spec, &[n(1904.0), n(2.0), n(29.0)], &mut ctx), 59.0);
        close(crate::call(spec, &[n(1904.0), n(3.0), n(0.0)], &mut ctx), 59.0);
        close(crate::call(spec, &[n(1904.0), n(3.0), n(1.0)], &mut ctx), 60.0);
        is_err(crate::call(spec, &[n(1900.0), n(2.0), n(29.0)], &mut ctx), CellError::Num);
        let spec = crate::lookup("EOMONTH").unwrap();
        close(crate::call(spec, &[n(31.0), n(0.0)], &mut ctx), 59.0);
        let spec = crate::lookup("DATEVALUE").unwrap();
        close(crate::call(spec, &[t("2/29/1904")], &mut ctx), 59.0);
        is_err(crate::call(spec, &[t("2/29/1900")], &mut ctx), CellError::Value);
    }

    #[test]
    fn isoweek_at_calendar_start() {
        let iso = crate::lookup("ISOWEEKNUM").unwrap();
        let wk = crate::lookup("WEEKNUM").unwrap();
        let mut ctx = TestCtx::default();
        close(crate::call(iso, &[n(1.0)], &mut ctx), 52.0);
        close(crate::call(wk, &[n(1.0), n(21.0)], &mut ctx), 52.0);
        let mut ctx = TestCtx { sys: gridcraft_core::DateSystem::D1904, ..TestCtx::default() };
        for s in [0.0, 1.0, 2.0] {
            close(crate::call(iso, &[n(s)], &mut ctx), 53.0);
        }
    }

    #[test]
    fn date_text_arguments_use_the_workbook_date_system() {
        let mut ctx = TestCtx { sys: gridcraft_core::DateSystem::D1904, ..TestCtx::default() };
        close(crate::call(crate::lookup("YEAR").unwrap(), &[t("1/1/2020")], &mut ctx), 2020.0);
        close(crate::call(crate::lookup("MONTH").unwrap(), &[t("2/29/1904")], &mut ctx), 2.0);
        close(crate::call(crate::lookup("DAY").unwrap(), &[t("2/29/1904")], &mut ctx), 29.0);
        close(crate::call(crate::lookup("DAYS").unwrap(), &[t("1/2/2020"), t("1/1/2020")], &mut ctx), 1.0);
        let networkdays = crate::lookup("NETWORKDAYS").unwrap();
        close(crate::call(networkdays, &[t("1/6/2020"), t("1/10/2020"), t("1/8/2020")], &mut ctx), 4.0);
    }
}
