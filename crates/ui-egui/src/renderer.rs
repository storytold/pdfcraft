//! Preferences ▸ Graphics: which renderer the desktop app starts with (#1560, #1719, #1868).
//!
//! On Windows, wgpu presents through a Direct3D 12 (DXGI flip-model) swap chain, and the GPU
//! driver gives the focused window that presents through one the display's variable refresh
//! rate (G-SYNC, FreeSync, Adaptive-Sync). The panel's refresh rate then follows the window's
//! presents, and OLED and VA panels change brightness with their refresh rate, so the window
//! flickers whenever it draws: on every pointer move, as PdfCraft draws only when something
//! changes. Drawing every frame doesn't help (it then flickers all the time), nor do a
//! DirectComposition swap chain or a swap chain without `ALLOW_TEARING`. OpenGL (and Vulkan,
//! which PdfCraft doesn't load on Windows) present through a path that the driver doesn't give
//! variable refresh, and are calm. Observed with an NVIDIA GPU (docs/development.md).
//!
//! The renderer is chosen before the window exists, so `apps/pdfcraft` reads this preference
//! from the saved settings ([`RendererPreference::saved`]) and a change applies at the next start.
//! `PDFCRAFT_RENDERER` still wins over it.

/// The renderer the desktop app starts with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RendererPreference {
    /// wgpu (Direct3D 12 on Windows), and OpenGL when wgpu can't start.
    #[default]
    Auto,
    /// OpenGL from the start.
    Gl,
}

/// The key in the settings that [`crate::PdfCraftApp::persist`] writes.
pub(crate) const KEY: &str = "renderer";

impl RendererPreference {
    pub const ALL: [Self; 2] = [Self::Auto, Self::Gl];

    /// The English label in Preferences.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Gl => "OpenGL",
        }
    }

    /// The name in saved settings and on the control channel.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gl => "gl",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.name().eq_ignore_ascii_case(value.trim()))
    }

    /// The preference in settings written by [`crate::PdfCraftApp::persist`]; `None` when they
    /// don't name one (settings from before it existed) or can't be read. Settings are untrusted.
    pub fn saved(settings: &str) -> Option<Self> {
        Self::in_settings(&serde_json::from_str(settings).ok()?)
    }

    /// The preference in settings already parsed ([`crate::PdfCraftApp::restore`]).
    pub(crate) fn in_settings(settings: &serde_json::Value) -> Option<Self> {
        Self::parse(settings.get(KEY)?.as_str()?)
    }
}

#[cfg(test)]
mod tests {
    use super::RendererPreference;

    #[test]
    fn the_saved_preference_is_read_back_and_anything_else_is_none() {
        assert_eq!(RendererPreference::saved(r#"{"renderer":"gl"}"#), Some(RendererPreference::Gl));
        assert_eq!(RendererPreference::saved(r#"{"theme":"dark","renderer":"auto"}"#), Some(RendererPreference::Auto));
        // Settings from before the preference, a value of another type or name, and a file that isn't JSON.
        for settings in [r#"{"theme":"dark"}"#, r#"{"renderer":1}"#, r#"{"renderer":"vulkan"}"#, "[", "", "\u{0}"] {
            assert_eq!(RendererPreference::saved(settings), None, "{settings:?}");
        }
    }

    /// What `persist` writes is what the desktop app reads before the window exists.
    #[test]
    fn the_preference_is_saved_where_the_desktop_app_reads_it() {
        let mut app = crate::PdfCraftApp::new();
        assert_eq!(app.renderer, RendererPreference::Auto);
        assert_eq!(RendererPreference::saved(&app.persist()), Some(RendererPreference::Auto));
        assert!(app.set_option("renderer", "gl").is_ok());
        assert_eq!(RendererPreference::saved(&app.persist()), Some(RendererPreference::Gl));
        let mut restored = crate::PdfCraftApp::new();
        restored.restore(&app.persist());
        assert_eq!(restored.renderer, RendererPreference::Gl);
        // An unknown value is refused and changes nothing.
        assert!(app.set_option("renderer", "vulkan").is_err());
        assert_eq!(app.renderer, RendererPreference::Gl);
    }

    #[test]
    fn names_round_trip_through_parse() {
        for choice in RendererPreference::ALL {
            assert_eq!(RendererPreference::parse(choice.name()), Some(choice));
        }
        assert_eq!(RendererPreference::parse(" GL "), Some(RendererPreference::Gl));
        assert_eq!(RendererPreference::parse("wgpu"), None);
    }
}
