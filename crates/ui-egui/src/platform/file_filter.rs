//! Picker filters that list a file whatever the letter case of its extension.
//!
//! On Linux and the BSDs, rfd gives each extension to the XDG portal, GTK or Zenity as the glob
//! `*.<ext>`, and those match it case-sensitively: a "PDF" filter of `pdf` hid `Report.PDF`.
//! Windows, macOS and the browser already ignore case, so there the extension passes unchanged.

/// Whether the platform's pickers match filter globs case-sensitively.
const CASE_SENSITIVE_GLOBS: bool = cfg!(all(unix, not(target_os = "macos")));

/// [`rfd::AsyncFileDialog::add_filter`], with each extension matched in any letter case.
pub(crate) trait AddFilterAnyCase {
    #[must_use]
    fn add_filter_any_case(self, name: impl Into<String>, extensions: &[impl AsRef<str>]) -> Self;
}

impl AddFilterAnyCase for rfd::AsyncFileDialog {
    fn add_filter_any_case(self, name: impl Into<String>, extensions: &[impl AsRef<str>]) -> Self {
        let globs: Vec<String> = extensions.iter().map(|e| any_case(e.as_ref(), CASE_SENSITIVE_GLOBS)).collect();
        self.add_filter(name, &globs)
    }
}

/// `ext` as a glob fragment matching any letter case: `pdf` becomes `[pP][dD][fF]`.
fn any_case(ext: &str, case_sensitive: bool) -> String {
    if !case_sensitive {
        return ext.to_owned();
    }
    ext.chars()
        .map(|c| if c.is_ascii_alphabetic() { format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase()) } else { c.to_string() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::any_case;

    #[test]
    fn a_case_sensitive_picker_gets_a_glob_matching_every_case() {
        assert_eq!(any_case("pdf", true), "[pP][dD][fF]");
        assert_eq!(any_case("PDF", true), "[pP][dD][fF]");
        assert_eq!(any_case("jp2", true), "[jJ][pP]2");
        assert_eq!(any_case("*", true), "*");
    }

    #[test]
    fn a_case_blind_picker_gets_the_extension_unchanged() {
        assert_eq!(any_case("pdf", false), "pdf");
        assert_eq!(any_case("*", false), "*");
    }
}
