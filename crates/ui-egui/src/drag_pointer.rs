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

thread_local! {
    /// Where the pointer is while files are dragged over a window of its own. The root window
    /// gets this as an event from `raw_input_hook`; eframe does not call that hook for the
    /// other windows, so their pass sets it here for the page grid to read.
    static HINT: std::cell::Cell<Option<egui::Pos2>> = const { std::cell::Cell::new(None) };
}

/// Set (or clear) the pointer position the page grid should use during this window's pass.
pub(crate) fn set_hint(pos: Option<egui::Pos2>) {
    HINT.with(|h| h.set(pos));
}

/// The position set for this pass, if any.
pub(crate) fn hint() -> Option<egui::Pos2> {
    HINT.with(std::cell::Cell::get)
}

/// For a window of its own: while files hover over it or are dropped on it, ask the system where
/// the pointer is (egui is not told, see [`in_window`]) and remember it for this pass.
pub(crate) fn note_for_pass(ctx: &egui::Context) {
    let files = ctx.input(|i| !i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty());
    set_hint(if files { in_window(ctx) } else { None });
    if files {
        ctx.request_repaint();
    }
}
