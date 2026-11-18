//! Who draws the window frame.
//!
//! Linux: Wayland compositors that draw frames for their clients offer
//! `zxdg_decoration_manager_v1` (KDE, Sway, Hyprland, COSMIC). GNOME doesn't: it leaves the frame
//! to the app, and winit then drew an imitation of GNOME's. Without that protocol PdfCraft turns
//! winit's frame off and draws a header bar like GNOME's own apps
//! (`pdfcraft_ui_egui::window_controls`), with the window buttons GNOME's `button-layout` names.
//! X11 window managers always draw the frame. `PDFCRAFT_TITLEBAR=app` or `=system` overrides the
//! choice.
//!
//! Windows: the app may draw the frame too, with the caption buttons at the strip's right, but
//! only when Preferences ▸ Use system title bar is turned off; Windows' own title bar is the
//! default, for upgrades too (older builds saved the preference on Windows without offering it).
//! The saved preference is read from `app.ron` before the window is created, so the window opens
//! with the right frame instead of switching on its first frame.

use pdfcraft_ui_egui::window_controls::{self, Layout};

/// The window buttons to draw in the app, or `None` when the desktop draws the frame.
pub(crate) fn app_drawn() -> Option<Layout> {
    let drawn = match std::env::var("PDFCRAFT_TITLEBAR").ok().as_deref().map(str::trim) {
        Some("app") => true,
        Some("system") => false,
        _ => compositor_leaves_frame_to_app(),
    };
    drawn.then(button_layout)
}

/// Whether the window opens without winit's frame: the app has buttons to draw, and the saved
/// Use system title bar preference (or, unsaved, the platform's default) doesn't ask for the
/// system's frame.
pub(crate) fn app_frame_at_launch(window_controls: bool, saved_system_title_bar: Option<bool>) -> bool {
    window_controls && !saved_system_title_bar.unwrap_or(window_controls::SYSTEM_TITLE_BAR_DEFAULT)
}

/// The saved Use system title bar preference, if `app.ron` in `settings_dir` holds one.
pub(crate) fn saved_system_title_bar(settings_dir: Option<&std::path::Path>) -> Option<bool> {
    let text = std::fs::read_to_string(settings_dir?.join("app.ron")).ok()?;
    system_title_bar_in(&text)
}

/// eframe's `app.ron` is a map of strings; PdfCraft's settings are the JSON under `pdfcraft`
/// (or the app's former name), read the way the app reads them. Anything unreadable means "not
/// saved", never an error: the app reads the same file again through eframe and reports problems
/// there.
fn system_title_bar_in(ron_text: &str) -> Option<bool> {
    let kv: std::collections::HashMap<String, String> = ron::from_str(ron_text).ok()?;
    let json = kv.get("pdfcraft").or_else(|| kv.get(crate::LEGACY_STORAGE_KEY))?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    window_controls::saved_system_title_bar(&value)
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
    use super::{app_frame_at_launch, system_title_bar_in};

    #[test]
    fn the_saved_preference_is_read_from_app_ron() {
        let ron = r#"{"pdfcraft":"{\"title_bar\":\"app\",\"theme\":\"Dark\"}","egui":"{}"}"#;
        assert_eq!(system_title_bar_in(ron), Some(false));
        let ron = r#"{"pdfcraft":"{\"title_bar\":\"system\"}"}"#;
        assert_eq!(system_title_bar_in(ron), Some(true));
        // The app's former name still counts.
        let ron = r#"{"printcraft":"{\"title_bar\":\"system\"}"}"#;
        assert_eq!(system_title_bar_in(ron), Some(true));
        // The boolean older builds wrote: a choice where the checkbox was, not on Windows
        // (`window_controls::saved_system_title_bar`).
        let ron = r#"{"pdfcraft":"{\"system_title_bar\":false}"}"#;
        assert_eq!(system_title_bar_in(ron), if cfg!(target_os = "windows") { None } else { Some(false) });
        // Not saved, not JSON, not RON: no preference, never an error.
        assert_eq!(system_title_bar_in(r#"{"pdfcraft":"{\"theme\":\"Dark\"}"}"#), None);
        assert_eq!(system_title_bar_in(r#"{"pdfcraft":"not json"}"#), None);
        assert_eq!(system_title_bar_in("not ron"), None);
        assert_eq!(system_title_bar_in(""), None);
    }

    #[test]
    fn the_window_opens_with_the_frame_the_preference_asks_for() {
        // Nothing to draw: always the system's frame.
        assert!(!app_frame_at_launch(false, None));
        assert!(!app_frame_at_launch(false, Some(false)));
        // A saved choice wins.
        assert!(app_frame_at_launch(true, Some(false)));
        assert!(!app_frame_at_launch(true, Some(true)));
        // Unsaved: the platform's default, which is the system's title bar only on Windows.
        assert_eq!(app_frame_at_launch(true, None), !cfg!(target_os = "windows"));
    }
}
