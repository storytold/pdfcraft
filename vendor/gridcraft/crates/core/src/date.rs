//! Date serial numbers.
//!
//! The 1900 system counts days from 1899-12-31 (serial 1 = 1900-01-01) and, like Lotus 1-2-3 and
//! Excel, treats 1900 as a leap year: serial 60 is the fictitious 1900-02-29. The 1904 system
//! counts from 1904-01-01 (serial 0). Times are fractions of a day.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DateSystem {
    #[default]
    D1900,
    D1904,
}

/// A calendar date and time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Milliseconds (rounded).
    pub milli: u32,
    /// 0 = Sunday.
    pub weekday: u32,
}

/// Days since 1970-01-01 for a proleptic Gregorian date, exact for every `i32` year.
///
/// `NaiveDate` only covers about ±262143 years, but the Gregorian calendar repeats every 400
/// years (146_097 days): chrono converts the year within its cycle and whole cycles are added
/// back as days (an i128 so the i32 extremes can't overflow). 1970-01-01 is `num_days_from_ce`
/// 719_163; the result matches the proleptic Gregorian count the old hand-rolled code gave.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let (cycles, year) = (i128::from(i64::from(y).div_euclid(400)), i64::from(y).rem_euclid(400));
    i32::try_from(year)
        .ok()
        .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, m, d))
        .and_then(|date| {
            let days = i128::from(chrono::Datelike::num_days_from_ce(&date)) - 719_163 + cycles * 146_097;
            i64::try_from(days).ok()
        })
        .unwrap_or(0)
}

/// Milliseconds since the Unix epoch (UTC) from the system clock; `None` when the clock is unavailable.
/// Chrono uses the browser clock on wasm32, where the standard library clock methods trap.
pub fn unix_now_ms() -> Option<f64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(chrono::Utc::now().timestamp_millis() as f64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        Some(js_sys::Date::now())
    }
}

/// Date serial (1900 system, with fraction) for a Unix time in milliseconds.
pub fn serial_from_unix_ms(ms: f64) -> f64 {
    25569.0 + ms / 86_400_000.0
}

/// Converts a 1900-system serial (as the clock supplies it) to the serial of the same instant in `sys`.
pub fn serial_in_system(sys: DateSystem, serial_1900: f64) -> f64 {
    match sys {
        DateSystem::D1900 => serial_1900,
        DateSystem::D1904 => serial_1900 - 1462.0,
    }
}

/// Proleptic Gregorian `(year, month, day)` for a day count `z` since 1970-01-01 (the inverse of
/// [`days_from_civil`]).
///
/// The calendar repeats every 400 years (146_097 days): chrono converts the day within its cycle
/// and whole cycles go back into the year (an i128 so no `z` overflows), which matches the
/// proleptic Gregorian answer the old hand-rolled code gave past `NaiveDate`'s ±262143 range.
/// 1970-01-01 is `num_days_from_ce` 719_163.
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    use chrono::Datelike;
    let (cycles, rest) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    match i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt) {
        Some(d) => {
            let y = i128::from(d.year()) + i128::from(cycles) * 400;
            (i32::try_from(y).unwrap_or(if y < 0 { i32::MIN } else { i32::MAX }), d.month(), d.day())
        }
        // Unreachable: `rest + 719_163` is always a valid CE day.
        None => (1970, 1, 1),
    }
}

const UNIX_1899_12_30: i64 = -25569; // days_from_civil(1899, 12, 30)
const UNIX_1904_01_01: i64 = -24107;

/// Whether `y` is a Gregorian leap year, for every `i32` year.
pub fn is_leap(y: i32) -> bool {
    // A year is a leap year iff 29 February exists in it. Leap years repeat every 400 years, so
    // the year within its Gregorian cycle (`rem_euclid(400)`) decides it and always fits chrono.
    let y = i64::from(y).rem_euclid(400) as i32;
    chrono::NaiveDate::from_ymd_opt(y, 2, 29).is_some()
}

/// Number of days in month `m` of year `y` (1–31), or `0` for an invalid month number, for every
/// `i32` year.
pub fn days_in_month(y: i32, m: u32) -> u32 {
    // The length of a month is the day before the first of the next month. Month lengths repeat
    // every 400 years, so the year within its Gregorian cycle (`rem_euclid(400)`) is enough and
    // always fits chrono (so December can't overflow `y + 1`). An invalid month keeps the old `0`.
    if !(1..=12).contains(&m) {
        return 0;
    }
    let y = i64::from(y).rem_euclid(400) as i32;
    let first = |y: i32, m: u32| chrono::NaiveDate::from_ymd_opt(y, m, 1);
    match (first(y, m), if m == 12 { first(y + 1, 1) } else { first(y, m + 1) }) {
        (Some(a), Some(b)) => u32::try_from((b - a).num_days()).unwrap_or(0),
        _ => 0,
    }
}

/// Days in a month of a date system's calendar: like [`days_in_month`], except that February
/// 1900 has 29 days in the 1900 system (serial 60 is the fictitious 1900-02-29).
pub fn days_in_month_in(sys: DateSystem, y: i32, m: u32) -> u32 {
    if sys == DateSystem::D1900 && y == 1900 && m == 2 { 29 } else { days_in_month(y, m) }
}

/// Last serial that is a valid date in a date system: 9999-12-31 (2958465 in the 1900 system, 2957003 in the 1904 system).
pub fn max_serial(sys: DateSystem) -> i64 {
    match sys {
        DateSystem::D1900 => 2_958_465,
        DateSystem::D1904 => 2_958_465 - 1462,
    }
}

/// Serial for a date. Month and day overflow roll over like `DATE()` (month 13 = January next
/// year, day 0 = last day of the previous month), in the date system's own calendar: in the 1900
/// system February 1900 has 29 days, so `(1900, 2, 29)` and `(1900, 3, 0)` are serial 60.
/// `None` before the epoch or after 9999-12-31.
pub fn serial_from_ymd(sys: DateSystem, year: i64, month: i64, day: i64) -> Option<f64> {
    // DATE(): years 0–1899 are added to 1900.
    let y = if (0..1900).contains(&year) { year + 1900 } else { year };
    let m0 = month.checked_sub(1)?;
    let y = y.checked_add(m0.div_euclid(12))?;
    let m = m0.rem_euclid(12) + 1;
    if !(1900..=9999).contains(&y) && sys == DateSystem::D1900 || !(1904..=9999).contains(&y) && sys == DateSystem::D1904 {
        return None;
    }
    let first = days_from_civil(y as i32, m as u32, 1);
    // Serial of the first of the month; the day (and any overflow) counts on from there.
    let first = match sys {
        // Up to February 1900 the serial is one lower than the day count from 1899-12-30: the
        // fictitious 1900-02-29 (serial 60) comes before 1 March.
        DateSystem::D1900 => first - UNIX_1899_12_30 - i64::from(y == 1900 && m <= 2),
        DateSystem::D1904 => first - UNIX_1904_01_01,
    };
    let serial = first.checked_add(day)?.checked_sub(1)?;
    if !(0..=max_serial(sys)).contains(&serial) {
        return None;
    }
    Some(serial as f64)
}

/// Date/time from a serial. `None` for negative or too-large serials.
pub fn datetime_from_serial(sys: DateSystem, serial: f64) -> Option<DateTime> {
    if !serial.is_finite() || !(0.0..max_serial(sys) as f64 + 1.0).contains(&serial) {
        return None;
    }
    let mut day = serial.floor() as i64;
    let mut frac_ms = ((serial - serial.floor()) * 86_400_000.0).round() as i64;
    if frac_ms >= 86_400_000 {
        frac_ms -= 86_400_000;
        day += 1;
        if day > max_serial(sys) {
            return None;
        }
    }
    let (y, m, d, weekday) = match sys {
        DateSystem::D1900 => {
            // Excel's weekdays continue the fictitious calendar: serial 1 is a Sunday.
            let wd = ((day + 6) % 7) as u32;
            if day == 60 {
                (1900, 2, 29, wd)
            } else if day == 0 {
                (1900, 1, 0, wd)
            } else {
                let real = if day < 60 { day + 1 } else { day };
                let (y, m, d) = civil_from_days(real + UNIX_1899_12_30);
                (y, m, d, wd)
            }
        }
        DateSystem::D1904 => {
            let unix = day + UNIX_1904_01_01;
            let (y, m, d) = civil_from_days(unix);
            (y, m, d, ((unix + 4).rem_euclid(7)) as u32)
        }
    };
    let secs = frac_ms / 1000;
    Some(DateTime {
        year: y,
        month: m,
        day: d,
        hour: (secs / 3600) as u32,
        minute: ((secs / 60) % 60) as u32,
        second: (secs % 60) as u32,
        milli: (frac_ms % 1000) as u32,
        weekday,
    })
}

/// Fraction of a day for a time.
pub fn time_fraction(h: f64, m: f64, s: f64) -> f64 {
    (h * 3600.0 + m * 60.0 + s) / 86400.0
}

pub const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
pub const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_rounding_into_next_day_past_max_is_none() {
        for sys in [DateSystem::D1900, DateSystem::D1904] {
            let serial = max_serial(sys) as f64 + 0.9999999995;
            assert_eq!(datetime_from_serial(sys, serial), None);
        }
    }

    #[test]
    fn leap_and_month_length_outside_chrono_range() {
        // In range, the ordinary Gregorian rule holds.
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert!(is_leap(2024));
        assert!(!is_leap(2023));
        // Outside chrono's NaiveDate range the Gregorian rule is still applied.
        assert!(is_leap(400_000)); // divisible by 400
        assert!(!is_leap(300_100)); // century not divisible by 400
        assert!(is_leap(i32::MAX - 3)); // 2147483644 is divisible by 4, not a century
        assert!(!is_leap(i32::MAX)); // 2147483647 is odd
        // days_in_month keeps calendar lengths out of range and 0 for invalid months.
        assert_eq!(days_in_month(400_000, 2), 29);
        assert_eq!(days_in_month(300_100, 2), 28);
        assert_eq!(days_in_month(400_000, 1), 31);
        assert_eq!(days_in_month(400_000, 4), 30);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2023, 0), 0);
        assert_eq!(days_in_month(2023, 13), 0);
        // December at i32::MAX does not overflow and still reports 31 days.
        assert_eq!(days_in_month(i32::MAX, 12), 31);
        assert_eq!(days_in_month(i32::MAX, 2), 28); // 2147483647 is not a leap year
    }

    #[test]
    fn civil_conversions_exact_beyond_chrono_range() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // Past `NaiveDate`'s ±262143 the 400-year cycles carry the result, exactly, both sides of
        // the epoch; values round-trip through the proleptic Gregorian converters.
        for (y, m, d, z) in [
            (262_144i32, 1u32, 1u32, 95_026_602i64),
            (300_000, 1, 1, 108_853_222),
            (400_000, 2, 29, 145_377_531),
            (-262_145, 3, 1, -96_465_964),
            (-300_000, 6, 15, -110_292_112),
        ] {
            assert_eq!(days_from_civil(y, m, d), z, "{y}-{m}-{d}");
            assert_eq!(civil_from_days(z), (y, m, d), "{z}");
        }
        // The extremes must not overflow or panic (i128 intermediates).
        let _ = (days_from_civil(i32::MAX, 12, 31), days_from_civil(i32::MIN, 1, 1), civil_from_days(i64::MAX), civil_from_days(i64::MIN));
    }

    #[test]
    fn serials_unchanged_through_cycle_conversion() {
        // The 1900 leap bug and 1904 semantics must stay exact; the converters only ever see
        // in-range years here, so the serial outputs are byte-for-byte the pre-chrono ones.
        let s = DateSystem::D1900;
        assert_eq!(serial_from_ymd(s, 1900, 2, 29), Some(60.0)); // Excel's fictitious leap day
        assert_eq!(serial_from_ymd(s, 1900, 3, 1), Some(61.0));
        assert_eq!(serial_from_ymd(s, 9999, 12, 31), Some(2958465.0));
        assert_eq!(days_in_month_in(s, 1900, 2), 29);
        let s4 = DateSystem::D1904;
        assert_eq!(serial_from_ymd(s4, 1904, 1, 1), Some(0.0)); // serial 0 is 1904-01-01
        assert_eq!(serial_from_ymd(s4, 1904, 2, 29), Some(59.0)); // 1904 is a real leap year
        assert_eq!(serial_from_ymd(s4, 1900, 2, 29), None); // no fictitious day in 1904 system
        assert_eq!(days_in_month_in(s4, 1900, 2), 28);
    }

    #[test]
    fn serial_from_unix_time() {
        assert_eq!(serial_from_unix_ms(0.0), 25569.0);
        // 2026-10-10T12:00:00Z
        assert_eq!(serial_from_unix_ms(1_791_633_600_000.0), 46305.5);
        assert!(unix_now_ms().is_some_and(|ms| serial_from_unix_ms(ms) > 46305.0));
    }

    #[test]
    fn known_serials() {
        let s = DateSystem::D1900;
        assert_eq!(serial_from_ymd(s, 1900, 1, 1), Some(1.0));
        assert_eq!(serial_from_ymd(s, 1900, 2, 28), Some(59.0));
        assert_eq!(serial_from_ymd(s, 1900, 3, 1), Some(61.0));
        assert_eq!(serial_from_ymd(s, 2000, 1, 1), Some(36526.0));
        assert_eq!(serial_from_ymd(s, 2026, 10, 7), Some(46302.0));
        assert_eq!(serial_from_ymd(s, 9999, 12, 31), Some(2958465.0));
        assert_eq!(serial_from_ymd(DateSystem::D1904, 9999, 12, 31), Some(2957003.0));
        assert_eq!(serial_from_ymd(DateSystem::D1904, 9999, 12, 32), None);
        assert!(datetime_from_serial(DateSystem::D1904, 2957003.5).is_some());
        assert!(datetime_from_serial(DateSystem::D1904, 2957004.0).is_none());
        assert_eq!(serial_from_ymd(s, 2020, 13, 1), serial_from_ymd(s, 2021, 1, 1));
        assert_eq!(serial_from_ymd(s, 2020, 3, 0), serial_from_ymd(s, 2020, 2, 29));
        assert_eq!(serial_from_ymd(s, 120, 1, 1), serial_from_ymd(s, 2020, 1, 1));
        assert_eq!(serial_from_ymd(DateSystem::D1904, 1904, 1, 1), Some(0.0));
        assert_eq!(serial_from_ymd(DateSystem::D1904, 2000, 1, 1), Some(35064.0));
    }

    #[test]
    fn fictitious_1900_02_29() -> Result<(), std::io::Error> {
        // Excel's 1900 calendar has a 29 February 1900 (serial 60); day and month overflow count
        // through it.
        let s = DateSystem::D1900;
        assert_eq!(serial_from_ymd(s, 1900, 2, 29), Some(60.0));
        assert_eq!(serial_from_ymd(s, 1900, 3, 0), Some(60.0));
        assert_eq!(serial_from_ymd(s, 1900, 2, 30), Some(61.0));
        assert_eq!(serial_from_ymd(s, 1900, 1, 60), Some(60.0));
        assert_eq!(serial_from_ymd(s, 1900, 1, 61), Some(61.0));
        assert_eq!(serial_from_ymd(s, 1900, 1, 400), Some(400.0));
        assert_eq!(serial_from_ymd(s, 1900, 4, -30), Some(61.0));
        assert_eq!(serial_from_ymd(s, 1900, 4, -31), Some(60.0));
        assert_eq!(serial_from_ymd(s, 1900, 3, -1), Some(59.0));
        assert_eq!(serial_from_ymd(s, 1901, -10, 29), Some(60.0));
        assert_eq!(serial_from_ymd(s, 1900, 1, 0), Some(0.0));
        assert_eq!(serial_from_ymd(s, 1900, 1, -1), None);
        assert_eq!(serial_from_ymd(s, 1900, 0, 1), None);
        // Later years keep the real calendar.
        assert_eq!(serial_from_ymd(s, 1901, 2, 29), serial_from_ymd(s, 1901, 3, 1));
        assert_eq!(serial_from_ymd(s, 1904, 2, 29), Some(1521.0));
        // Every serial up to 1900-12-31 comes back from its own year, month and day.
        for serial in 0..=366 {
            let d = datetime_from_serial(s, serial as f64).ok_or_else(|| std::io::Error::other("serial should convert"))?;
            assert_eq!(serial_from_ymd(s, d.year as i64, d.month as i64, d.day as i64), Some(serial as f64), "{d:?}");
        }
        assert_eq!(days_in_month_in(s, 1900, 2), 29);
        assert_eq!(days_in_month_in(s, 1901, 2), 28);
        assert_eq!(days_in_month_in(DateSystem::D1904, 1900, 2), 28);
        // The 1904 system has no fictitious day: 1904 is a real leap year.
        let s = DateSystem::D1904;
        assert_eq!(serial_from_ymd(s, 1904, 2, 29), Some(59.0));
        assert_eq!(serial_from_ymd(s, 1904, 3, 0), Some(59.0));
        assert_eq!(serial_from_ymd(s, 1904, 3, 1), Some(60.0));
        assert_eq!(serial_from_ymd(s, 1904, 1, 0), None);
        assert_eq!(serial_from_ymd(s, 1900, 2, 29), None);
        Ok(())
    }

    #[test]
    fn hostile_month_and_day() {
        let s = DateSystem::D1900;
        assert_eq!(serial_from_ymd(s, 2000, i64::MIN, 1), None);
        assert_eq!(serial_from_ymd(s, 2000, i64::MAX, 1), None);
        assert_eq!(serial_from_ymd(s, 2000, 1, i64::MAX), None);
        assert_eq!(serial_from_ymd(s, 2000, 1, i64::MIN), None);
        assert_eq!(serial_from_ymd(s, i64::MAX, 1, 1), None);
    }

    #[test]
    fn roundtrip() -> Result<(), std::io::Error> {
        let s = DateSystem::D1900;
        for serial in [1.0, 59.0, 60.0, 61.0, 36526.0, 46302.0, 2958465.0] {
            let d = datetime_from_serial(s, serial).ok_or_else(|| std::io::Error::other("serial should convert"))?;
            assert_eq!(serial_from_ymd(s, d.year as i64, d.month as i64, d.day as i64), Some(serial));
        }
        let d = datetime_from_serial(s, 60.0).ok_or_else(|| std::io::Error::other("leap-day serial should convert"))?;
        assert_eq!((d.year, d.month, d.day), (1900, 2, 29));
        let d = datetime_from_serial(s, 46302.75).ok_or_else(|| std::io::Error::other("time serial should convert"))?;
        assert_eq!((d.hour, d.minute, d.weekday), (18, 0, 3)); // 2026-10-07 is a Wednesday
        let d = datetime_from_serial(s, 1.0).ok_or_else(|| std::io::Error::other("first serial should convert"))?;
        assert_eq!(d.weekday, 0); // 1900-01-01 was a Sunday in Excel's calendar
        Ok(())
    }
}
