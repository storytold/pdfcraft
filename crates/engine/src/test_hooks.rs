//! Test-only hooks: injected refresh panics and counts of the secondary page scans.

thread_local! {
    static SECONDARY_SCAN_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static REFRESH_PANICS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn panic_on_refresh(count: usize) {
    REFRESH_PANICS.with(|n| n.set(count));
}

pub(crate) fn check_refresh_panic() {
    REFRESH_PANICS.with(|n| {
        let rem = n.get();
        if rem > 0 {
            n.set(rem - 1);
            panic!("injected refresh panic");
        }
    });
}

pub(crate) fn reset_secondary_scan_counts() {
    SECONDARY_SCAN_COUNTS.with(|counts| counts.set((0, 0)));
}

pub(crate) fn secondary_scan_counts() -> (usize, usize) {
    SECONDARY_SCAN_COUNTS.with(std::cell::Cell::get)
}

pub(crate) fn record_marker_scan() {
    SECONDARY_SCAN_COUNTS.with(|counts| {
        let (markers, links) = counts.get();
        counts.set((markers + 1, links));
    });
}

pub(crate) fn record_link_scan() {
    SECONDARY_SCAN_COUNTS.with(|counts| {
        let (markers, links) = counts.get();
        counts.set((markers, links + 1));
    });
}
