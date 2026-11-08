//! File-name rules of the operating systems the app writes to.

/// Whether Windows reads the path component `name` as a device rather than a file: `CON`, `PRN`,
/// `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` and the superscript `COM¹`–`LPT³` whatever their
/// extension or a colon after it (`NUL:`, or `NUL:x`, a stream of the device), in any case. The
/// device part ends at the first `.` or `:`, and spaces before it are ignored (`nul .txt` and
/// `con :` write nowhere, as `NUL.pdf` does; Windows drops only spaces there, not other
/// whitespace). `CONIN$` and `CONOUT$` match the same way (Windows 10 opens `conin$.txt` as the
/// console too), and so does `CLOCK$`, which Windows 10 writes as an ordinary file: a name matched
/// by mistake is only refused or prefixed.
pub fn is_windows_device_name(name: &str) -> bool {
    let device = name.split(['.', ':']).next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$")
        || device
            .strip_prefix("COM")
            .or_else(|| device.strip_prefix("LPT"))
            .is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
}

#[cfg(test)]
mod tests {
    use super::is_windows_device_name as device;

    /// Checked on Windows 10 by writing to each name: `COM¹` and `lpt³.pdf` open `\\.\COM¹` and
    /// `\\.\lpt³`, and `nul:.txt` opens `NUL`; `COM0`, `COM10`, `CONIN` and a no-break space before
    /// the dot make ordinary files.
    #[test]
    fn device_names_match_in_any_case_with_any_extension() {
        for name in [
            "CON",
            "prn",
            "Aux",
            "nul.pdf",
            "con.pdf.pdf",
            "nul .txt",
            "NUL  ",
            "clock$",
            "CONIN$.x",
            "conout$",
            "COM1",
            "lpt9.tar",
            "COM¹",
            "lpt³.pdf",
            "com²",
            "NUL:",
            "nul:.pdf",
            "com1:x",
            "con :",
        ] {
            assert!(device(name), "{name}");
        }
        // Only spaces are dropped before the extension: `NUL\u{a0}.pdf` is a file.
        for name in
            ["COM10", "COM0", "LPT", "Console", "CONIN", "nullx.pdf", "nul_", "x.nul", "", ".", "CO M1", "COM⁴", "NUL\u{a0}.pdf", "con\u{3000}"]
        {
            assert!(!device(name), "{name}");
        }
    }
}
