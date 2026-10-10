//! Which craft-fonts faces a build embeds. `build.rs` includes this file (`#[path]`), so it has no
//! dependencies; the library compiles it too, so the rules are unit-tested and the tests use the
//! same lists as the build.

/// Family-name prefixes of the faces AGENTS.md §1.1 forbids: Adobe's Source families and Noto CJK,
/// which is Source Han under another name. craft-fonts carries Noto Sans CJK SC for other apps, so
/// a checkout may list one; it is never embedded.
pub const BARRED_FAMILIES: [&str; 5] = ["Source Han", "Source Serif", "Source Sans", "Noto Sans CJK", "Noto Serif CJK"];

/// The faces wasm32 builds keep by name: the Japanese UI face and the Chinese UI face (Simplified
/// and Traditional, 3.9 MB). The web app is one `.wasm` file that every visitor downloads, so the
/// other CJK faces (serif and bold Japanese, any larger Chinese face craft-fonts adds later) stay
/// desktop-only.
pub const WEB_FACES: [(&str, &str); 2] = [("BIZ UDPGothic", "Regular"), ("Droid Sans Fallback", "Regular")];

/// Scripts whose faces wasm32 builds keep whatever their name: they are small.
const WEB_SCRIPTS: [&str; 2] = ["Arab", "Telu"];

/// Whether `family` is one AGENTS.md §1.1 forbids ([`BARRED_FAMILIES`]).
pub fn barred(family: &str) -> bool {
    BARRED_FAMILIES.iter().any(|prefix| family.starts_with(prefix))
}

/// Whether a wasm32 build embeds the face (`scripts` are its ISO 15924 tags).
pub fn on_web(family: &str, style: &str, scripts: &[&str]) -> bool {
    WEB_FACES.contains(&(family, style)) || scripts.iter().any(|s| WEB_SCRIPTS.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adobe_designs_are_barred_by_family() {
        for family in ["Noto Sans CJK SC", "Noto Serif CJK TC", "Source Han Sans SC", "Source Serif 4", "Source Sans 3"] {
            assert!(barred(family), "{family}");
        }
        for family in ["Noto Sans Arabic", "Noto Sans Telugu", "Droid Sans Fallback", "BIZ UDPGothic", "Shippori Mincho"] {
            assert!(!barred(family), "{family}");
        }
    }

    #[test]
    fn the_web_keeps_the_ui_faces_and_small_scripts() {
        assert!(on_web("BIZ UDPGothic", "Regular", &["Jpan", "Latn"]));
        assert!(on_web("Droid Sans Fallback", "Regular", &["Hans", "Hant"]));
        assert!(on_web("Noto Sans Arabic", "Regular", &["Arab"]));
        assert!(on_web("Noto Sans Telugu", "Regular", &["Telu"]));
        // Serif and bold Japanese, and any other Chinese face, stay desktop-only.
        assert!(!on_web("BIZ UDPGothic", "Bold", &["Jpan", "Latn"]));
        assert!(!on_web("Shippori Mincho", "Regular", &["Jpan", "Latn"]));
        assert!(!on_web("Some Large Hans Face", "Regular", &["Hans", "Latn"]));
    }
}
