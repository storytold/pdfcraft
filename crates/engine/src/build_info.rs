//! What this build calls itself. A local or patched build sets `PDFCRAFT_BUILD_LABEL` at compile
//! time (`PDFCRAFT_BUILD_LABEL=titlebar cargo build`, or through `packaging/linux/package.sh`), so
//! it can be told from the release of the same version. The version itself stays the release's:
//! the update check compares it with published releases.

/// The release version from the workspace `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The compile-time build label, if any.
pub const LABEL: Option<&str> = option_env!("PDFCRAFT_BUILD_LABEL");

/// The version as shown to people: `0.7.1`, or `0.7.1 (titlebar)` for a labelled build.
pub fn version() -> String {
    describe(VERSION, LABEL)
}

fn describe(version: &str, label: Option<&str>) -> String {
    match label.map(str::trim).filter(|l| !l.is_empty()) {
        Some(label) => format!("{version} ({label})"),
        None => version.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn a_label_follows_the_version_and_a_blank_one_is_ignored() {
        assert_eq!(describe("0.7.1", None), "0.7.1");
        assert_eq!(describe("0.7.1", Some("")), "0.7.1");
        assert_eq!(describe("0.7.1", Some("  ")), "0.7.1");
        assert_eq!(describe("0.7.1", Some("titlebar")), "0.7.1 (titlebar)");
    }
}
