//! Startup-only Windows composition preference. Adapted from LightCraft PR #2421.
use std::{collections::HashMap, io::Read, path::Path};

/// Read eframe's RON map before the renderer creates its window. Corrupt settings are harmless.
pub fn load(path: &Path) -> bool {
    let mut text = String::new();
    let Ok(file) = std::fs::File::open(path) else { return false };
    // Preferences are untrusted; never allocate without a bound for a startup flag.
    if file.take(8 * 1024 * 1024 + 1).read_to_string(&mut text).is_err() || text.len() > 8 * 1024 * 1024 {
        return false;
    }
    saved(&text)
}

fn saved(text: &str) -> bool {
    let Ok(map) = ron::from_str::<HashMap<String, String>>(text) else { return false };
    map.get("pdfcraft")
        .or_else(|| map.get(super::LEGACY_STORAGE_KEY))
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|v| v.get("compose_window").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Explicit renderer/presentation overrides and fallback OpenGL keep their normal window.
pub fn enabled(windows: bool, preference: bool, wgpu: bool, backend_override: bool, presentation_override: bool) -> bool {
    windows && preference && wgpu && !backend_override && !presentation_override
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_composition_reads_current_and_legacy_storage_safely() {
        for key in ["pdfcraft", "printcraft"] {
            for (json, expected) in [
                (r#"{"compose_window":true}"#, true),
                (r#"{"compose_window":false}"#, false),
                ("{}", false),
                (r#"{"compose_window":"true"}"#, false),
                ("broken", false),
            ] {
                let text = ron::to_string(&HashMap::from([(key, json)])).unwrap();
                assert_eq!(saved(&text), expected);
            }
        }
        assert!(!saved("broken"));
        let text = ron::to_string(&HashMap::from([("pdfcraft", "{}"), ("printcraft", r#"{"compose_window":true}"#)])).unwrap();
        assert!(!saved(&text), "current key takes precedence");
        assert!(!load(Path::new("missing-vrr-settings/app.ron")));
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
