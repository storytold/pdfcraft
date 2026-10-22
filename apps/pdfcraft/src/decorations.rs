//! Who draws the window frame on Linux.
//!
//! Wayland compositors that draw frames for their clients offer `zxdg_decoration_manager_v1`
//! (KDE, Sway, Hyprland, COSMIC). GNOME doesn't: it leaves the frame to the app, and winit then
//! drew an imitation of GNOME's. Without that protocol PdfCraft turns winit's frame off and draws
//! a header bar like GNOME's own apps (`pdfcraft_ui_egui::window_controls`), with the window
//! buttons GNOME's `button-layout` names. X11 window managers always draw the frame, also for a
//! window opened through Xwayland on a Wayland session (src/pen_display.rs), whatever the
//! Wayland compositor offers.
//!
//! `PDFCRAFT_TITLEBAR=app` or `=system` overrides the choice.

use pdfcraft_ui_egui::window_controls::Layout;

/// The window buttons to draw in the app, or `None` when the desktop draws the frame.
/// `through_xwayland`: the window opens on Xwayland, whose window manager draws the frame.
pub(crate) fn app_drawn(through_xwayland: bool) -> Option<Layout> {
    let titlebar = std::env::var("PDFCRAFT_TITLEBAR").ok();
    app_draws_frame(titlebar.as_deref(), through_xwayland, compositor_leaves_frame_to_app).then(button_layout)
}

/// [`app_drawn`]'s decision. The Wayland compositor is asked only when neither
/// `PDFCRAFT_TITLEBAR` nor Xwayland decides: it says nothing about a window on Xwayland.
fn app_draws_frame(titlebar: Option<&str>, through_xwayland: bool, compositor_leaves_frame: impl FnOnce() -> bool) -> bool {
    match titlebar.map(str::trim) {
        Some("app") => true,
        Some("system") => false,
        _ if through_xwayland => false,
        _ => compositor_leaves_frame(),
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
    use super::app_draws_frame;

    #[test]
    fn a_window_on_xwayland_gets_the_window_managers_frame() {
        // GNOME's compositor leaves Wayland frames to the app, but not an X11 window's.
        assert!(!app_draws_frame(None, true, || panic!("the Wayland compositor isn't asked")));
        assert!(!app_draws_frame(Some("auto"), true, || true));
        assert!(app_draws_frame(None, false, || true), "native Wayland on GNOME");
        assert!(!app_draws_frame(None, false, || false), "native Wayland on KDE");
    }

    #[test]
    fn the_environment_overrides_the_choice() {
        for through_xwayland in [false, true] {
            assert!(app_draws_frame(Some(" app "), through_xwayland, || false));
            assert!(!app_draws_frame(Some("system"), through_xwayland, || true));
        }
    }
}
