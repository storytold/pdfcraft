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
        let dir = std::env::temp_dir().join(format!("pdfcraft-vrr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("compose-window");
        std::fs::write(dir.join("app.ron"), vec![b' '; 9 * 1024 * 1024]).unwrap();
        assert!(!load(&path));
        for (text, expected) in [("on", true), ("off", false), ("broken", false), ("", false), ("onextra", false)] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(load(&path), expected);
        }
        std::fs::remove_dir_all(dir).unwrap();
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
