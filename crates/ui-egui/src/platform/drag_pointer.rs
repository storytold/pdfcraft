//! Where the pointer is when the windowing layer doesn't say: while files are dragged over the
//! window (it reports that a file hovers, but not where, and sends no pointer moves until the drag
//! ends), and when the wheel turns after switching back to the window (no move is sent until the
//! mouse moves, #1518). The system has to be asked.

/// The pointer in this window's coordinates (egui points), when the system can tell.
#[cfg(target_os = "macos")]
pub(crate) fn in_window(ctx: &egui::Context) -> Option<egui::Pos2> {
    use objc2_app_kit::{NSEvent, NSScreen};
    // AppKit's screen list is only for the main thread (where egui runs).
    let mtm = objc2::MainThreadMarker::new()?;
    // Screen coordinates start at the bottom-left of the first screen; windows at their top-left.
    let top = NSScreen::screens(mtm).firstObject()?.frame().size.height;
    let pointer = NSEvent::mouseLocation();
    let inner = ctx.input(|i| i.viewport().inner_rect)?;
    // egui's points are the system's, divided by the zoom factor.
    let zoom = ctx.zoom_factor().max(0.01) as f64;
    let (x, y) = (pointer.x / zoom, (top - pointer.y) / zoom);
    let pos = egui::pos2(x as f32 - inner.min.x, y as f32 - inner.min.y);
    (pos.x.is_finite() && pos.y.is_finite()).then_some(pos)
}

/// The pointer in this window's coordinates (egui points), when the system can tell.
#[cfg(windows)]
pub(crate) fn in_window(ctx: &egui::Context) -> Option<egui::Pos2> {
    // Physical screen pixels, whatever this thread's DPI awareness.
    let pointer = winsafe::GetPhysicalCursorPos().ok()?;
    let inner = ctx.input(|i| i.viewport().inner_rect)?;
    // egui places the window in points: physical pixels over the pixels per point (zoom included).
    let ppp = ctx.pixels_per_point();
    if !(ppp.is_finite() && ppp > 0.0) {
        return None;
    }
    let pos = egui::pos2(pointer.x as f32 / ppp - inner.min.x, pointer.y as f32 / ppp - inner.min.y);
    (pos.x.is_finite() && pos.y.is_finite()).then_some(pos)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn in_window(_ctx: &egui::Context) -> Option<egui::Pos2> {
    None
}

/// Wheel input with no known pointer: after Alt+Tab back to the window, the windowing layer sends
/// no pointer move until the mouse moves, so egui doesn't know what is under the pointer and the
/// wheel scrolls nothing (#1518). When `raw` turns the wheel, has no pointer move of its own and
/// egui knows no pointer position (`known`), the pointer from `system` is put first, if it is
/// inside the window's `size` (egui points).
pub(crate) fn locate_wheel(raw: &mut egui::RawInput, known: bool, size: egui::Vec2, system: impl FnOnce() -> Option<egui::Pos2>) {
    let wheel = raw.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }));
    let moved = raw.events.iter().any(|e| matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerGone));
    if !wheel || moved || known {
        return;
    }
    if let Some(pos) = system().filter(|p| egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains(*p)) {
        raw.events.insert(0, egui::Event::PointerMoved(pos));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel() -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        }
    }

    fn located(events: Vec<egui::Event>, known: bool, system: Option<egui::Pos2>) -> Vec<egui::Event> {
        let mut raw = egui::RawInput { events, ..Default::default() };
        locate_wheel(&mut raw, known, egui::vec2(800.0, 600.0), || system);
        raw.events
    }

    #[test]
    fn a_wheel_with_no_known_pointer_asks_the_system_where_it_is() {
        let at = egui::pos2(300.0, 200.0);
        // After Alt+Tab back: the wheel turns, nothing moved, egui knows no pointer.
        assert_eq!(located(vec![wheel()], false, Some(at)), vec![egui::Event::PointerMoved(at), wheel()]);
        // Otherwise nothing is added: egui knows the pointer, the frame moves it itself, there is
        // no wheel, the system can't tell, or the pointer is outside the window.
        assert_eq!(located(vec![wheel()], true, Some(at)), vec![wheel()]);
        let moved = egui::Event::PointerMoved(egui::pos2(1.0, 1.0));
        assert_eq!(located(vec![moved.clone(), wheel()], false, Some(at)), vec![moved, wheel()]);
        assert_eq!(located(vec![egui::Event::PointerGone, wheel()], false, Some(at)), vec![egui::Event::PointerGone, wheel()]);
        assert_eq!(
            located(
                vec![egui::Event::Key { key: egui::Key::A, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
                false,
                Some(at)
            )
            .len(),
            1
        );
        assert_eq!(located(vec![wheel()], false, None), vec![wheel()]);
        assert_eq!(located(vec![wheel()], false, Some(egui::pos2(900.0, 200.0))), vec![wheel()]);
        assert_eq!(located(vec![wheel()], false, Some(egui::pos2(f32::NAN, 0.0))), vec![wheel()]);
    }
}
