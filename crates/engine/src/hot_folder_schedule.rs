//! Calendar schedules in the machine's local timezone. Nonexistent DST times move to the
//! next valid local minute; repeated times run once at their earlier occurrence.
use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Schedule {
    #[default]
    Interval,
    Daily {
        hour: u32,
        minute: u32,
    },
    Weekly {
        weekday: u32,
        hour: u32,
        minute: u32,
    },
    Monthly {
        day: u32,
        hour: u32,
        minute: u32,
    },
}

impl Schedule {
    pub fn validate(self) -> Result<(), String> {
        let (hour, minute) = match self {
            Self::Interval => return Ok(()),
            Self::Daily { hour, minute } => (hour, minute),
            Self::Weekly { weekday, hour, minute } if weekday < 7 => (hour, minute),
            Self::Monthly { day, hour, minute } if (1..=31).contains(&day) => (hour, minute),
            _ => return Err("weekday must be 0..6 (Monday..Sunday); month day must be 1..31".into()),
        };
        if hour > 23 || minute > 59 {
            return Err("schedule time must be 00:00..23:59".into());
        }
        Ok(())
    }

    pub fn next_after(self, after: u64) -> Result<u64, String> {
        self.next_in(after, Local)
    }

    fn next_in<T: TimeZone>(self, after: u64, zone: T) -> Result<u64, String> {
        self.validate()?;
        let origin = i64::try_from(after).ok().and_then(|v| zone.timestamp_opt(v, 0).single()).ok_or("invalid schedule timestamp")?;
        let mut date = origin.date_naive();
        for _ in 0..370 {
            let (matches, hour, minute) = match self {
                Self::Interval => return Ok(after.saturating_add(1)),
                Self::Daily { hour, minute } => (true, hour, minute),
                Self::Weekly { weekday, hour, minute } => (date.weekday().num_days_from_monday() == weekday, hour, minute),
                Self::Monthly { day, hour, minute } => {
                    let last = date.succ_opt().is_some_and(|d| d.month() != date.month());
                    (date.day() == day || last && date.day() < day, hour, minute)
                }
            };
            if matches {
                let mut local = date.and_hms_opt(hour, minute, 0).ok_or("invalid schedule time")?;
                // A DST gap is bounded to this date; do not drift a daily task into tomorrow.
                while local.date() == date {
                    if let Some(candidate) = zone.from_local_datetime(&local).earliest() {
                        if candidate.timestamp() > origin.timestamp() {
                            return u64::try_from(candidate.timestamp()).map_err(|e| e.to_string());
                        }
                        break;
                    }
                    local = local.checked_add_signed(chrono::Duration::minutes(1)).ok_or("schedule overflow")?;
                    if local.hour() == 0 && local.minute() == 0 {
                        break;
                    }
                }
            }
            date = date.succ_opt().ok_or("schedule date overflow")?;
        }
        Err("no valid schedule occurrence within the next year".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_schedules_preserve_time_and_clamp_short_months() {
        let tz = chrono::Utc;
        let stamp = |year, month, day, hour, minute| {
            u64::try_from(tz.with_ymd_and_hms(year, month, day, hour, minute, 0).single().unwrap().timestamp()).unwrap()
        };
        let start = stamp(2026, 1, 31, 8, 0);
        assert_eq!(Schedule::Daily { hour: 8, minute: 0 }.next_in(start, tz).unwrap(), stamp(2026, 2, 1, 8, 0));
        assert_eq!(Schedule::Monthly { day: 31, hour: 8, minute: 0 }.next_in(start, tz).unwrap(), stamp(2026, 2, 28, 8, 0));
        assert_eq!(Schedule::Weekly { weekday: 0, hour: 9, minute: 30 }.next_in(start, tz).unwrap(), stamp(2026, 2, 2, 9, 30));
        assert!(Schedule::Weekly { weekday: 7, hour: 9, minute: 0 }.validate().is_err());
    }
}
