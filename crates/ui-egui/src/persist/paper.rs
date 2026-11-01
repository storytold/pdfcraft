//! The paper size the system's region prints on: the size of a new blank document (Create ▸
//! Blank page).

use super::units::region_of;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    Letter,
    A4,
}

impl Paper {
    /// Width and height in points, portrait.
    pub fn points(self) -> (f64, f64) {
        match self {
            Paper::Letter => (612.0, 792.0),
            Paper::A4 => (595.28, 841.89),
        }
    }

    /// The system region's paper: US Letter in the Americas that use it, A4 everywhere else;
    /// Letter when the region can't be told, as before the region was read.
    pub fn system() -> Paper {
        static SYSTEM: std::sync::OnceLock<Paper> = std::sync::OnceLock::new();
        *SYSTEM.get_or_init(|| detect().unwrap_or_default())
    }
}

/// The paper for a locale tag's region (`en_US.UTF-8`, `fr-BE`, `es_MX`); `None` without a region.
/// The Letter regions are CLDR's `paperSize` US-Letter territories.
pub(crate) fn paper_for_tag(tag: &str) -> Option<Paper> {
    Some(match region_of(tag)?.as_str() {
        "US" | "CA" | "MX" | "PH" | "PR" | "BZ" | "CL" | "CO" | "CR" | "GT" | "NI" | "PA" | "SV" | "VE" => Paper::Letter,
        _ => Paper::A4,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn detect() -> Option<Paper> {
    // A Finder-started app gets no LANG, so the system locale's region decides first.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLocale"]).output()
        && out.status.success()
        && let Some(paper) = paper_for_tag(String::from_utf8_lossy(&out.stdout).trim())
    {
        return Some(paper);
    }
    // Region ▸ the paper size Windows keeps per user, read in-process like the measurement system.
    #[cfg(target_os = "windows")]
    if let Some(paper) = windows_paper().and_then(|v| paper_for_ipapersize(&v)) {
        return Some(paper);
    }
    ["LC_ALL", "LC_PAPER", "LANG"].iter().find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()).and_then(|v| paper_for_tag(&v)))
}

/// The paper for the Windows `iPaperSize` value (a `DMPAPER` number): "1" Letter, "9" A4. `None`
/// for anything else, so another size falls through to the locale check.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn paper_for_ipapersize(value: &str) -> Option<Paper> {
    match value.trim() {
        "1" => Some(Paper::Letter),
        "9" => Some(Paper::A4),
        _ => None,
    }
}

/// `HKCU\Control Panel\International\iPaperSize` as text; `None` when it is missing, unreadable
/// or not a string.
#[cfg(target_os = "windows")]
fn windows_paper() -> Option<String> {
    use winsafe::{HKEY, RegistryValue, co::RRF};
    match HKEY::CURRENT_USER.RegGetValue(Some("Control Panel\\International"), Some("iPaperSize"), RRF::RT_REG_SZ).ok()? {
        RegistryValue::Sz(v) => Some(v),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
fn detect() -> Option<Paper> {
    None
}

#[cfg(test)]
mod tests {
    use super::{Paper, paper_for_ipapersize, paper_for_tag};

    #[test]
    fn region_decides_the_paper() {
        assert_eq!(paper_for_tag("en_US.UTF-8"), Some(Paper::Letter));
        assert_eq!(paper_for_tag("fr_CA"), Some(Paper::Letter));
        assert_eq!(paper_for_tag("es-MX"), Some(Paper::Letter));
        assert_eq!(paper_for_tag("fr_BE.UTF-8"), Some(Paper::A4));
        assert_eq!(paper_for_tag("en_GB@euro"), Some(Paper::A4));
        assert_eq!(paper_for_tag("de-DE"), Some(Paper::A4));
        assert_eq!(paper_for_tag("zh-Hans-CN"), Some(Paper::A4));
        assert_eq!(paper_for_tag("C"), None);
        assert_eq!(paper_for_tag("en"), None);
        assert_eq!(paper_for_tag(""), None);
    }

    #[test]
    fn windows_paper_size_decides_the_paper() {
        assert_eq!(paper_for_ipapersize("1"), Some(Paper::Letter));
        assert_eq!(paper_for_ipapersize(" 9\r\n"), Some(Paper::A4));
        assert_eq!(paper_for_ipapersize("5"), None);
        assert_eq!(paper_for_ipapersize(""), None);
    }

    /// The registry read works in-process and gives a value the mapping understands, or nothing.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reads_ipapersize_in_process() {
        if let Some(v) = super::windows_paper() {
            assert!(v.trim().parse::<u32>().is_ok(), "unexpected iPaperSize {v:?}");
        }
    }

    #[test]
    fn a4_and_letter_in_points() {
        assert_eq!(Paper::Letter.points(), (612.0, 792.0));
        let (w, h) = Paper::A4.points();
        assert_eq!(((w * 25.4 / 72.0).round(), (h * 25.4 / 72.0).round()), (210.0, 297.0));
    }
}
