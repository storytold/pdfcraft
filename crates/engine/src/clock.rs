/// Seconds to add to UTC for local time at `at` (seconds since the epoch; now when `None`), which
/// differs with daylight saving; 0 where unknown.
pub(crate) fn local_utc_offset(at: Option<i64>) -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone};
        match at.and_then(|t| Local.timestamp_opt(t, 0).single()) {
            Some(t) => i64::from(t.offset().local_minus_utc()),
            None => i64::from(Local::now().offset().local_minus_utc()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = at;
        0
    }
}

/// (year, month, day) from a PDF date `D:YYYYMMDD…`.
pub(crate) fn parse_ymd(d: &str) -> Option<(i64, u32, u32)> {
    let d = d.strip_prefix("D:").unwrap_or(d);
    Some((d.get(0..4)?.parse().ok()?, d.get(4..6)?.parse().ok()?, d.get(6..8)?.parse().ok()?))
}
