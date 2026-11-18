//! Who draws the window frame on Linux.
//!
//! Wayland compositors that draw frames for their clients offer `zxdg_decoration_manager_v1`
//! (KDE, Sway, Hyprland, COSMIC). GNOME doesn't: it leaves the frame to the app, and winit then
//! drew an imitation of GNOME's. Without that protocol PdfCraft turns winit's frame off and draws
//! a header bar like GNOME's own apps (`pdfcraft_ui_egui::window_controls`), with the window
//! buttons GNOME's `button-layout` names. X11 window managers always draw the frame, including
//! under XWayland, where PdfCraft runs on Wayland desktops for file drops (src/display_backend.rs).
//!
//! `PDFCRAFT_TITLEBAR=app` or `=system` overrides the choice.

use pdfcraft_ui_egui::window_controls::Layout;

/// The window buttons to draw in the app, or `None` when the desktop draws the frame. `on_x11`:
/// the window opens on X11 (also through XWayland), whatever the session is.
pub(crate) fn app_drawn(on_x11: bool) -> Option<Layout> {
    frame_drawn(std::env::var("PDFCRAFT_TITLEBAR").ok().as_deref(), on_x11, compositor_leaves_frame_to_app).then(button_layout)
}

/// Whether the app draws its frame, from `PDFCRAFT_TITLEBAR` and the backend the window opens on.
/// The compositor is asked only when the window is going to be a Wayland one.
fn frame_drawn(titlebar: Option<&str>, on_x11: bool, compositor_leaves_frame_to_app: impl FnOnce() -> bool) -> bool {
    match titlebar.map(str::trim) {
        Some("app") => true,
        Some("system") => false,
        // The X window manager draws it, even when the session's Wayland compositor wouldn't.
        _ if on_x11 => false,
        _ => compositor_leaves_frame_to_app(),
    }
}

/// GNOME's button order, or minimize, maximize and close on the right when it names none.
fn button_layout() -> Layout {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
    if let Some(value) = crate::text_scale::gsettings(&["get", "org.gnome.desktop.wm.preferences", "button-layout"]) {
        let layout = Layout::parse(&value);
        // A layout without a single button would leave no way to close the window by mouse.
        if !layout.left.is_empty() || !layout.right.is_empty() {
            return layout;
        }
    }
    Layout::default()
}

/// Whether this is a Wayland session whose compositor draws no frames.
#[cfg(target_os = "linux")]
fn compositor_leaves_frame_to_app() -> bool {
    use wayland_client::Connection;
    use wayland_client::globals::registry_queue_init;
    // No Wayland compositor: X11, whose window manager draws the frame (winit falls back to it).
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, _queue)) = registry_queue_init::<Registry>(&conn) else { return false };
    let offered = globals.contents().with_list(|list| list.iter().any(|g| g.interface == "zxdg_decoration_manager_v1"));
    if !offered {
        log::info!("the Wayland compositor draws no window frames; PdfCraft draws its own title bar");
    }
    !offered
}

#[cfg(not(target_os = "linux"))]
fn compositor_leaves_frame_to_app() -> bool {
    false
}

#[cfg(target_os = "linux")]
struct Registry;

#[cfg(target_os = "linux")]
impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, wayland_client::globals::GlobalListContents> for Registry {
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &wayland_client::Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::frame_drawn;

    #[test]
    fn the_app_draws_its_frame_only_on_a_wayland_window_without_server_frames() {
        assert!(frame_drawn(None, false, || true));
        assert!(!frame_drawn(None, false, || false));
        // Under XWayland on GNOME the Wayland compositor draws no frames, but the X window
        // manager does: the app must not draw a second one (or none, if it turned the frame off).
        assert!(!frame_drawn(None, true, || panic!("the Wayland compositor isn't asked for an X11 window")));
        assert!(!frame_drawn(Some("auto"), true, || true));
        // The explicit override still wins either way.
        assert!(frame_drawn(Some(" app "), true, || false));
        assert!(!frame_drawn(Some("system"), false, || true));
    }
}
