//! Startup-only Windows composition preference. Adapted from LightCraft PR #2421.
use std::{io::Read, path::Path};

/// Read the dedicated startup flag, independently of large stamp libraries in app.ron.
pub fn load(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let mut text = String::new();
    file.take(4).read_to_string(&mut text).is_ok() && text == "on"
}

/// Explicit renderer/presentation overrides and fallback OpenGL keep their normal window.
pub fn enabled(windows: bool, preference: bool, wgpu: bool, backend_override: bool, presentation_override: bool) -> bool {
    windows && preference && wgpu && !backend_override && !presentation_override
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_composition_loads_a_small_flag_beside_large_settings() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path();
        let path = dir.join("compose-window");
        std::fs::write(dir.join("app.ron"), vec![b' '; 9 * 1024 * 1024]).unwrap();
        assert!(!load(&path));
        for (text, expected) in [("on", true), ("off", false), ("broken", false), ("", false), ("onextra", false)] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(load(&path), expected);
        }
    }

    #[test]
    fn composition_requires_opt_in_windows_wgpu_without_overrides() {
        assert!(enabled(true, true, true, false, false), "opted-in default Windows DX12");
        assert!(!enabled(true, false, true, false, false), "existing installations stay opaque");
        assert!(!enabled(false, true, true, false, false), "other platforms are unchanged");
        assert!(!enabled(true, true, false, false, false), "OpenGL retry stays opaque");
        assert!(!enabled(true, true, true, true, false), "explicit backend wins");
        assert!(!enabled(true, true, true, false, true), "explicit presentation wins");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod launch_tests {
    #[test]
    fn legacy_choice_migrates_only_for_a_later_launch() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = r#"{"compose_window":true}"#;
        let map = std::collections::HashMap::from([("pdfcraft", legacy)]);
        std::fs::write(dir.path().join("app.ron"), ron::to_string(&map).unwrap()).unwrap();
        let mut app = pdfcraft_ui_egui::PdfCraftApp::new();
        let saved: std::collections::HashMap<String, String> = ron::from_str(&std::fs::read_to_string(dir.path().join("app.ron")).unwrap()).unwrap();
        app.restore(saved.get("pdfcraft").unwrap());
        assert!(app.compose_window);
        let first = super::super::native_options_in(false, false, eframe::Renderer::Wgpu, Some(dir.path().to_path_buf()));
        assert_eq!(first.viewport.transparent, Some(false));
        std::fs::write(dir.path().join("compose-window"), "on").unwrap();
        let later = super::super::native_options_in(false, false, eframe::Renderer::Wgpu, Some(dir.path().to_path_buf()));
        let overridden = std::env::var_os("WGPU_BACKEND").is_some() || std::env::var_os("WGPU_DX12_PRESENTATION_SYSTEM").is_some();
        assert_eq!(later.viewport.transparent, Some(!overridden));
        assert_eq!(first.viewport.transparent, Some(false));
    }
}
