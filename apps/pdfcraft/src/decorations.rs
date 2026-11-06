//! Selects PdfCraft's optional app-drawn window header on Linux and BSD.
//!
//! The default is the toolkit frame. On GNOME/Wayland, winit provides Adwaita-style
//! client-side controls when the compositor has no server-side decorations; X11 window
//! managers and other Wayland compositors can provide their own frame.
//!
//! `PDFCRAFT_TITLEBAR=app` explicitly opts into the integrated PdfCraft header;
//! `PDFCRAFT_TITLEBAR=system` (or an unset variable) keeps the toolkit frame.

use pdfcraft_ui_egui::window_controls::Layout;

/// The window buttons to draw in the app, or `None` to keep the toolkit frame.
pub(crate) fn app_drawn() -> Option<Layout> {
    let titlebar = std::env::var("PDFCRAFT_TITLEBAR").ok();
    wants_app_frame(titlebar.as_deref()).then(button_layout)
}

/// Only an explicit `app` override opts into custom window decorations.
fn wants_app_frame(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("app"))
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

#[cfg(test)]
mod tests {
    use super::wants_app_frame;

    #[test]
    fn toolkit_frame_is_the_default() {
        assert!(!wants_app_frame(None));
        assert!(!wants_app_frame(Some("")));
        assert!(!wants_app_frame(Some("system")));
        assert!(!wants_app_frame(Some("unexpected")));
    }

    #[test]
    fn app_frame_requires_explicit_opt_in() {
        assert!(wants_app_frame(Some("app")));
        assert!(wants_app_frame(Some(" app ")));
        assert!(!wants_app_frame(Some("APP")));
    }
}
