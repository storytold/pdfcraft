//! Calendars a format section can show dates in: the `B1` (Gregorian) and `B2` (Hijri) prefixes.

use gridcraft_core::date::{DateTime, MONTHS, WEEKDAYS, days_from_civil, days_in_month};

/// The calendar a section's year, month and day parts count in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Calendar {
    #[default]
    Gregorian,
    /// The tabular (arithmetic) Islamic calendar.
    Hijri,
}

/// Arabic names of the Hijri months, Muharram first. Two-word names are joined by a no-break
/// space (U+00A0), as Excel writes them.
const HIJRI_MONTHS: [&str; 12] = [
    "محرم",
    "صفر",
    "ربيع\u{a0}الأول",
    "ربيع\u{a0}الثاني",
    "جمادى\u{a0}الأولى",
    "جمادى\u{a0}الثانية",
    "رجب",
    "شعبان",
    "رمضان",
    "شوال",
    "ذو\u{a0}القعدة",
    "ذو\u{a0}الحجة",
];

/// Arabic weekday names, Sunday first, shown for `ddd` and `dddd` under `B2`.
const ARABIC_WEEKDAYS: [&str; 7] = ["الأحد", "الإثنين", "الثلاثاء", "الأربعاء", "الخميس", "الجمعة", "السبت"];

/// Year, month and day of a date in a section's calendar, with the month and weekday names.
pub(crate) struct CalendarDate {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    /// `mmmm`.
    pub month_name: &'static str,
    /// `mmm`.
    pub month_short: &'static str,
    /// `dddd`.
    pub day_name: &'static str,
    /// `ddd`.
    pub day_short: &'static str,
}

impl Calendar {
    /// The date of a day in this calendar, `None` when it has none: under `B2`, the 1900 system's
    /// fictitious 29 February 1900 (serial 60, which Excel shows as `#VALUE!`) and days before
    /// 1 Muharram 1 AH.
    pub(crate) fn date(self, dt: &DateTime) -> Option<CalendarDate> {
        match self {
            Calendar::Hijri => hijri_date(dt),
            Calendar::Gregorian => Some(gregorian_date(dt)),
        }
    }
}

fn gregorian_date(dt: &DateTime) -> CalendarDate {
    let month_name = MONTHS.get((dt.month as usize).wrapping_sub(1)).copied().unwrap_or("");
    let day_name = WEEKDAYS.get(dt.weekday as usize).copied().unwrap_or("");
    CalendarDate {
        year: dt.year.into(),
        month: dt.month,
        day: dt.day,
        month_name,
        month_short: month_name.get(..3).unwrap_or(month_name),
        day_name,
        day_short: day_name.get(..3).unwrap_or(day_name),
    }
}

/// Days from 1 Muharram 1 AH to the first of a Hijri month. Leap years (30 days in the last
/// month) are years 2, 5, 7, 10, 13, 15, 18, 21, 24, 26 and 29 of each 30-year cycle (10,631
/// days), which is what Excel shows; odd months have 30 days and even months 29.
fn days_before_month(year: i64, month: i64) -> i64 {
    // `(4 + 11 * year) / 30` is the number of leap years before `year`.
    (year - 1) * 354 + (4 + 11 * year).div_euclid(30) + 29 * (month - 1) + month / 2
}

/// Hijri year, month and day of the day `days` after 1 Muharram 1 AH (`days >= 0`).
fn hijri_from_days(days: i64) -> Option<(i64, i64, i64)> {
    if days < 0 {
        return None;
    }
    // The estimate is the mean year length rounded; it can be one year off at a year boundary.
    // Estimate and `days_before_month` both move by 30 years per 10,631 days, so one step is
    // enough everywhere if it is within one cycle (`every_day_round_trips` checks many).
    let mut year = (30 * days + 10646) / 10631;
    if days < days_before_month(year, 1) {
        year -= 1;
    } else if days >= days_before_month(year + 1, 1) {
        year += 1;
    }
    let month = ((11 * (days - days_before_month(year, 1)) + 330) / 325).clamp(1, 12);
    let day = days - days_before_month(year, month) + 1;
    Some((year, month, day))
}

/// 1 Muharram 1 AH is 18 July 622 (proleptic Gregorian), so that 1 January 2001 is 6 Shawwal 1421.
fn hijri_date(dt: &DateTime) -> Option<CalendarDate> {
    // 29 February 1900 (serial 60) is no real day and has no Hijri date. The 1900 system's day 0
    // (`1900-01-00`, serial 0) shows as 1 January 1900, as Excel shows it.
    if dt.day > days_in_month(dt.year, dt.month) {
        return None;
    }
    let (year, month, day) = hijri_from_days(days_from_civil(dt.year, dt.month, dt.day.max(1)) - days_from_civil(622, 7, 18))?;
    let month_name = HIJRI_MONTHS.get(usize::try_from(month - 1).ok()?).copied()?;
    // Excel shows the full Arabic name for `ddd` as well as `dddd`, as it does for `mmm`.
    let day_name = ARABIC_WEEKDAYS.get(dt.weekday as usize).copied()?;
    Some(CalendarDate {
        year,
        month: u32::try_from(month).ok()?,
        day: u32::try_from(day).ok()?,
        month_name,
        month_short: month_name,
        day_name,
        day_short: day_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_core::DateSystem;
    use gridcraft_core::date::{datetime_from_serial, max_serial};

    /// Leap years of the 30-year cycle (Excel's set, per the reviewer's 43,512-serial comparison).
    const LEAP: [i64; 11] = [2, 5, 7, 10, 13, 15, 18, 21, 24, 26, 29];

    fn is_leap(year: i64) -> bool {
        LEAP.contains(&((year - 1).rem_euclid(30) + 1))
    }

    fn month_len(year: i64, month: i64) -> i64 {
        if month == 12 { 29 + i64::from(is_leap(year)) } else { 30 - (month + 1) % 2 }
    }

    /// Years 1 to 10,000 (every cycle up to and past 31 December 9999, which is 9666 AH): year,
    /// month and cycle lengths follow the leap-year set.
    #[test]
    fn year_and_month_lengths_follow_the_leap_set() {
        assert_eq!(days_before_month(1, 1), 0);
        for year in 1..=10_000 {
            let start = days_before_month(year, 1);
            let len = days_before_month(year + 1, 1) - start;
            assert_eq!(len, if is_leap(year) { 355 } else { 354 }, "year {year}");
            for month in 1..=12 {
                let next = if month == 12 { days_before_month(year + 1, 1) } else { days_before_month(year, month + 1) };
                assert_eq!(next - days_before_month(year, month), month_len(year, month), "{year}-{month}");
            }
            assert_eq!(days_before_month(year + 30, 1) - start, 10_631, "cycle from {year}");
        }
        // One cycle: exactly the eleven leap years, 10,631 days.
        let cycle: Vec<i64> = (1..=30).filter(|&y| days_before_month(y + 1, 1) - days_before_month(y, 1) == 355).collect();
        assert_eq!(cycle, LEAP);
    }

    /// Every day from 1 Muharram 1 AH to one cycle past 31 December 9999: day -> Hijri -> day
    /// round-trips, the day is within its month, and consecutive days follow each other.
    #[test]
    fn every_day_round_trips() {
        let last = days_from_civil(9999, 12, 31) - days_from_civil(622, 7, 18) + 10_631;
        let mut prev: Option<(i64, i64, i64)> = None;
        for days in 0..=last {
            let (y, m, d) = hijri_from_days(days).unwrap();
            assert!((1..=12).contains(&m) && (1..=month_len(y, m)).contains(&d), "{days}: {y}-{m}-{d}");
            assert_eq!(days_before_month(y, m) + d - 1, days, "{y}-{m}-{d}");
            if let Some((py, pm, pd)) = prev {
                let expected = if pd < month_len(py, pm) {
                    (py, pm, pd + 1)
                } else if pm < 12 {
                    (py, pm + 1, 1)
                } else {
                    (py + 1, 1, 1)
                };
                assert_eq!((y, m, d), expected, "after {py}-{pm}-{pd}");
            }
            prev = Some((y, m, d));
        }
        assert_eq!(hijri_from_days(-1), None);
    }

    /// Every serial of the 1900 system: the Hijri date names the serial's day, one day apart.
    #[test]
    fn every_serial_has_the_right_hijri_day() {
        let epoch = days_from_civil(622, 7, 18);
        let mut prev = None;
        for serial in 1..=max_serial(DateSystem::D1900) {
            if serial == 60 {
                continue;
            }
            let dt = datetime_from_serial(DateSystem::D1900, serial as f64).unwrap();
            let h = Calendar::Hijri.date(&dt).unwrap();
            assert!((1..=month_len(h.year, i64::from(h.month))).contains(&i64::from(h.day)), "serial {serial}");
            let days = days_before_month(h.year, i64::from(h.month)) + i64::from(h.day) - 1;
            assert_eq!(days, days_from_civil(dt.year, dt.month, dt.day) - epoch, "serial {serial}");
            if let Some(p) = prev {
                assert_eq!(days, p + 1, "serial {serial}");
            }
            prev = Some(days);
        }
    }
}
