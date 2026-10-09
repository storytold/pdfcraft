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

#[cfg(not(target_os = "macos"))]
pub(crate) fn in_window(_ctx: &egui::Context) -> Option<egui::Pos2> {
    None
}
