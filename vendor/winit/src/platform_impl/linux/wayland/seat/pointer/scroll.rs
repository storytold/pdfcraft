//! PdfCraft patch (13): Wayland scroll-phase policy, shared with the desktop regression tests.

use wayland_client::protocol::wl_pointer::AxisSource;

use crate::event::TouchPhase;

pub(crate) fn phase(previous: TouchPhase, source: Option<AxisSource>, has_discrete_scroll: bool, stopped: bool) -> TouchPhase {
    // Wayland does not guarantee axis_stop for these sources. A pixel-only wheel must
    // not start a finger gesture: consumers would retain its modifiers indefinitely.
    // https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_pointer-event-axis_source
    if stopped {
        TouchPhase::Ended
    } else if has_discrete_scroll || matches!(source, Some(AxisSource::Wheel | AxisSource::WheelTilt | AxisSource::Continuous)) {
        TouchPhase::Moved
    } else {
        match previous {
            TouchPhase::Started | TouchPhase::Moved => TouchPhase::Moved,
            _ => TouchPhase::Started,
        }
    }
}
