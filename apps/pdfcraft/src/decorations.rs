//! Who draws the window frame on Linux.
//!
//! PdfCraft integrates its window buttons into the tab strip on KDE Plasma, even though KWin can
//! draw a separate title bar. On other desktops the app draws its own header on Wayland only when
//! the compositor doesn't offer `zxdg_decoration_manager_v1`, as on GNOME. Otherwise the desktop
//! draws the frame.
//!
//! `PDFCRAFT_TITLEBAR=app` or `=system` overrides the choice.

use pdfcraft_ui_egui::window_controls::{Layout, Style};

/// The window buttons to draw in the app, or `None` when the desktop draws the frame.
pub(crate) fn app_drawn() -> Option<Layout> {
    let titlebar = std::env::var("PDFCRAFT_TITLEBAR").ok();
    let desktop = if cfg!(target_os = "linux") { std::env::var("XDG_CURRENT_DESKTOP").ok() } else { None };
    app_drawn_for(titlebar.as_deref(), desktop.as_deref(), compositor_leaves_frame_to_app, button_layout)
}

fn app_drawn_for(
    titlebar: Option<&str>,
    desktop: Option<&str>,
    compositor_leaves_frame_to_app: impl FnOnce() -> bool,
    button_layout: impl FnOnce() -> Layout,
) -> Option<Layout> {
    // XDG_CURRENT_DESKTOP is a colon-separated list of desktop names.
    let plasma = desktop.is_some_and(|names| names.split(':').any(|name| name.eq_ignore_ascii_case("KDE")));
    let drawn = match titlebar.map(str::trim) {
        Some("app") => true,
        Some("system") => false,
        _ => plasma || compositor_leaves_frame_to_app(),
    };
    // Plasma's default supported buttons are on the right. GNOME's setting is unrelated there.
    drawn.then(|| if plasma { Layout { style: Style::Breeze, ..Layout::default() } } else { button_layout() })
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
    use super::{Layout, Style, app_drawn_for};

    #[test]
    fn plasma_integrates_the_header_even_when_kwin_offers_a_frame() {
        let gnome_layout = || Layout::parse("close:");
        let breeze = Some(Layout { style: Style::Breeze, ..Layout::default() });
        assert_eq!(app_drawn_for(None, Some("KDE"), || false, gnome_layout), breeze);
        assert_eq!(app_drawn_for(None, Some("Example:kde"), || false, gnome_layout), breeze);
        assert_eq!(app_drawn_for(Some(" app "), Some("KDE"), || false, gnome_layout), breeze);
    }

    #[test]
    fn explicit_system_frame_overrides_plasma_and_other_defaults_stay_intact() {
        let gnome_layout = || Layout::parse("close:");
        assert_eq!(app_drawn_for(Some(" system "), Some("KDE"), || true, gnome_layout), None);
        assert_eq!(app_drawn_for(None, Some("GNOME"), || true, gnome_layout), Some(gnome_layout()));
        for desktop in [None, Some("sway"), Some("NotKDE")] {
            assert_eq!(app_drawn_for(None, desktop, || false, gnome_layout), None);
        }
        assert_eq!(app_drawn_for(Some("app"), Some("sway"), || false, gnome_layout), Some(gnome_layout()));
    }
}
