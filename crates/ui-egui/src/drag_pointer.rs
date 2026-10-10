//! Where the pointer is while files are dragged over the window. The windowing layer reports
//! that a file hovers, but not where, and sends no pointer moves until the drag ends; the
//! system has to be asked.

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

/// X11 and Windows report the pointer in screen pixels; egui wants points from the window's
/// top-left. Under Wayland the crate can't read the pointer, so this gives `None` there.
#[cfg(any(target_os = "linux", target_os = "freebsd", windows))]
pub(crate) fn in_window(ctx: &egui::Context) -> Option<egui::Pos2> {
    use mouse_position::mouse_position::Mouse;
    // Asks the system for the pointer, since winit sends none during a file drag. It fails where
    // there is no X11 or Win32 to ask (Wayland): no position, so the caller leaves egui's as is.
    let Mouse::Position { x, y } = Mouse::get_mouse_position() else {
        return None;
    };
    // The pointer is in screen coordinates; the window's own origin is needed to make it relative.
    let inner = ctx.input(|i| i.viewport().inner_rect)?;

    // The system gives pixels but `inner_rect` is in egui points, so convert before subtracting
    // (otherwise the position is off on HiDPI screens).
    let scale = ctx.pixels_per_point().max(0.01);
    let pos = egui::pos2(x as f32 / scale - inner.min.x, y as f32 / scale - inner.min.y);
    // Never hand egui a NaN or infinite position.
    (pos.x.is_finite() && pos.y.is_finite()).then_some(pos)
}

// Targets with no way to ask (wasm, other Unixes): no position, as before.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "freebsd", windows)))]
pub(crate) fn in_window(_ctx: &egui::Context) -> Option<egui::Pos2> {
    None
}
