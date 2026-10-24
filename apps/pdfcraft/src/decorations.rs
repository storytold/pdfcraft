//! Who draws the window frame on Linux.
//!
//! Wayland compositors that draw frames for their clients offer `zxdg_decoration_manager_v1`
//! (KDE, Sway, Hyprland, COSMIC). GNOME doesn't: it leaves the frame to the app, and winit then
//! drew an imitation of GNOME's. Without that protocol PdfCraft turns winit's frame off and draws
//! a header bar like GNOME's own apps (`pdfcraft_ui_egui::window_controls`), with the window
//! buttons GNOME's `button-layout` names. X11 and the BSDs draw the header bar too, so the window
//! buttons sit in the tab strip as in Acrobat; the window manager's frame is not painted above it.
//!
//! `PDFCRAFT_TITLEBAR=app` or `=system` overrides the choice.

use pdfcraft_ui_egui::window_controls::Layout;

/// The window buttons to draw in the app, or `None` when the desktop draws the frame.
pub(crate) fn app_drawn() -> Option<Layout> {
    let drawn = match std::env::var("PDFCRAFT_TITLEBAR").ok().as_deref().map(str::trim) {
        Some("app") => true,
        Some("system") => false,
        _ => desktop_leaves_frame_to_app(),
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

/// Whether the session gives no server-side frame: X11 and the BSDs, or a Wayland compositor
/// that draws none (GNOME).
#[cfg(target_os = "linux")]
fn desktop_leaves_frame_to_app() -> bool {
    use wayland_client::Connection;
    use wayland_client::globals::registry_queue_init;
    // No Wayland compositor: X11 (or a bare console). PdfCraft draws the header bar so its window
    // buttons sit in the tab strip, as in Acrobat; `PDFCRAFT_TITLEBAR=system` brings the window
    // manager's frame back.
    let Ok(conn) = Connection::connect_to_env() else { return true };
    let Ok((globals, _queue)) = registry_queue_init::<Registry>(&conn) else { return false };
    let offered = globals.contents().with_list(|list| list.iter().any(|g| g.interface == "zxdg_decoration_manager_v1"));
    if !offered {
        log::info!("the Wayland compositor draws no window frames; PdfCraft draws its own title bar");
    }
    !offered
}

/// The BSDs run X11 desktops, so they get the header bar as well. Wayland detection is Linux-only
/// (the `wayland-client` dependency), and `PDFCRAFT_TITLEBAR=system` still restores the frame.
#[cfg(not(target_os = "linux"))]
fn desktop_leaves_frame_to_app() -> bool {
    cfg!(any(target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))
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
