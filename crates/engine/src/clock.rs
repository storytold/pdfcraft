/// Seconds to add to UTC for local time (0 where unknown).
pub(crate) fn local_utc_offset() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    return i64::from(chrono::Local::now().offset().local_minus_utc());
    #[cfg(target_arch = "wasm32")]
    return 0;
}

/// (year, month, day) from a PDF date `D:YYYYMMDD…`.
pub(crate) fn parse_ymd(d: &str) -> Option<(i64, u32, u32)> {
    let d = d.strip_prefix("D:").unwrap_or(d);
    Some((d.get(0..4)?.parse().ok()?, d.get(4..6)?.parse().ok()?, d.get(6..8)?.parse().ok()?))
}
