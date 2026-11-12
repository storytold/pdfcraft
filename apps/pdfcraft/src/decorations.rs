//! Who draws the window frame on Linux.
//!
//! Wayland compositors that draw frames for their clients offer `zxdg_decoration_manager_v1`
//! (KDE, Sway, Hyprland, COSMIC). GNOME doesn't: it leaves the frame to the app, and winit then
//! drew an imitation of GNOME's. Without that protocol PdfCraft turns winit's frame off and draws
//! a header bar like GNOME's own apps (`pdfcraft_ui_egui::window_controls`), with the window
//! buttons GNOME's `button-layout` names. X11 window managers always draw the frame.
//!
//! `PDFCRAFT_TITLEBAR=app` or `=system` overrides the choice.

use pdfcraft_ui_egui::window_controls::Layout;

/// The window buttons to draw in the app, or `None` when the desktop draws the frame.
pub(crate) fn app_drawn() -> Option<Layout> {
    let drawn = match std::env::var("PDFCRAFT_TITLEBAR").ok().as_deref().map(str::trim) {
        Some("app") => true,
        Some("system") => false,
        _ => compositor_leaves_frame_to_app(),
    };
    drawn.then(button_layout)
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

/// Apply an explicit app-frame choice after restoring the user's saved preference.
pub(crate) fn apply_app_override(app: &mut pdfcraft_ui_egui::PdfCraftApp, force_app: bool) {
    if force_app && app.window_controls.is_some() {
        app.system_title_bar = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_ui_egui::PdfCraftApp;

    #[test]
    fn explicit_app_titlebar_overrides_a_restored_system_frame() {
        let mut app = PdfCraftApp::new();
        app.restore(r#"{"system_title_bar":true}"#);
        assert!(app.system_title_bar);
        app.window_controls = Some(Layout::default());
        apply_app_override(&mut app, true);
        assert!(app.draws_window_frame(), "the explicit app override must win over the saved system frame");
    }

    #[test]
    fn automatic_titlebar_preserves_the_restored_preference() {
        for saved in [false, true] {
            let mut app = PdfCraftApp::new();
            app.restore(&format!(r#"{{"system_title_bar":{saved}}}"#));
            app.window_controls = Some(Layout::default());
            apply_app_override(&mut app, false);
            assert_eq!(app.draws_window_frame(), !saved);
        }
    }

    #[test]
    fn an_app_override_without_app_controls_keeps_the_system_frame() {
        let mut app = PdfCraftApp::new();
        app.restore(r#"{"system_title_bar":true}"#);
        apply_app_override(&mut app, true);
        assert!(app.system_title_bar);
        assert!(!app.draws_window_frame());
    }
}
