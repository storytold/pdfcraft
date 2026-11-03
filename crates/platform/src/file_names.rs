//! File-name rules of the operating systems the app writes to.

/// Whether Windows reads the path component `name` as a device rather than a file: `CON`, `PRN`,
/// `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` and the superscript `COM¹`–`LPT³` whatever their
/// extension, in any case and with spaces before the first dot (`nul .txt` writes nowhere, as
/// `NUL.pdf` does; Windows drops only spaces there, not other whitespace). `CLOCK$`, `CONIN$` and
/// `CONOUT$` match the same way, though Windows reads the last two as devices only without an
/// extension: a name matched by mistake is only refused or prefixed.
pub fn is_windows_device_name(name: &str) -> bool {
    let device = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
        || device
            .strip_prefix("COM")
            .or_else(|| device.strip_prefix("LPT"))
            .is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
}

#[cfg(test)]
mod tests {
    use super::is_windows_device_name as device;

    #[test]
    fn device_names_match_in_any_case_with_any_extension() {
        for name in
            ["CON", "prn", "Aux", "nul.pdf", "nul .txt", "NUL  ", "clock$", "CONIN$.x", "conout$", "COM1", "lpt9.tar", "COM¹", "lpt³.pdf", "com²"]
        {
            assert!(device(name), "{name}");
        }
        // Only spaces are dropped before the extension: `NUL\u{a0}.pdf` is a file.
        for name in ["COM10", "COM0", "LPT", "Console", "nul_", "x.nul", "", ".", "CO M1", "COM⁴", "NUL\u{a0}.pdf", "con\u{3000}"] {
            assert!(!device(name), "{name}");
        }
    }
}
