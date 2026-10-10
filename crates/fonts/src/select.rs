//! The same embedding rules are used by build.rs and interface glyph tests.

pub fn allowed_family(family: &str) -> bool {
    // AGENTS.md §1.1 excludes Adobe type designs, including Source Han rebranded as Noto CJK.
    !["Source Han", "Source Serif", "Source Sans", "Noto Sans CJK", "Noto Serif CJK"].iter().any(|prefix| family.starts_with(prefix))
}

pub fn on_web(family: &str, style: &str, scripts: &[&str]) -> bool {
    allowed_family(family)
        && ((family == "BIZ UDPGothic" && style == "Regular") || scripts.iter().any(|s| matches!(*s, "Hans" | "Hant" | "Arab" | "Telu")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_keeps_allowed_ui_scripts_without_adobe_faces() {
        assert!(on_web("Droid Sans Fallback", "Regular", &["Hans", "Hant"]));
        assert!(on_web("BIZ UDPGothic", "Regular", &["Jpan"]));
        assert!(!on_web("BIZ UDPGothic", "Bold", &["Jpan"]));
        assert!(!on_web("Shippori Mincho", "Regular", &["Jpan"]));
        assert!(!on_web("Noto Sans CJK SC", "Regular", &["Hans"]));
        assert!(!allowed_family("Source Han Sans"));
        assert!(on_web("Noto Sans Arabic", "Regular", &["Arab"]));
        assert!(on_web("Telugu UI", "Regular", &["Telu"]));
    }
}
