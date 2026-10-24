//! The unit lengths are shown in: page sizes (Document Properties, Print) and the margins typed
//! in Header & Footer and Set Page Boxes. Preferences ▸ Documents and view ▸ Units.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Inches,
    Millimetres,
    Centimetres,
    Points,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::Millimetres, Unit::Centimetres, Unit::Inches, Unit::Points];

    /// Units per PDF point.
    pub fn per_point(self) -> f64 {
        match self {
            Unit::Points => 1.0,
            Unit::Inches => 1.0 / 72.0,
            Unit::Millimetres => 25.4 / 72.0,
            Unit::Centimetres => 2.54 / 72.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Unit::Points => "pt",
            Unit::Inches => "in",
            Unit::Millimetres => "mm",
            Unit::Centimetres => "cm",
        }
    }

    /// Decimals worth showing: a tenth of a millimetre or point, a hundredth of an inch or centimetre.
    pub fn decimals(self) -> usize {
        match self {
            Unit::Millimetres | Unit::Points => 1,
            Unit::Inches | Unit::Centimetres => 2,
        }
    }

    /// Drag speed for a value field in this unit.
    pub fn speed(self) -> f64 {
        match self {
            Unit::Inches | Unit::Centimetres => 0.01,
            Unit::Millimetres | Unit::Points => 0.5,
        }
    }

    /// The unit the system's region measures in: inches in the United States, Liberia and
    /// Myanmar, millimetres everywhere else; inches when the region can't be told. Used on first
    /// start, before the user picks one in Preferences.
    pub fn system() -> Unit {
        static SYSTEM: std::sync::OnceLock<Unit> = std::sync::OnceLock::new();
        *SYSTEM.get_or_init(|| detect().unwrap_or_default())
    }

    /// A page size given in points, e.g. "210.0 × 297.0 mm".
    pub fn size(self, width_pt: f64, height_pt: f64) -> String {
        let (k, d) = (self.per_point(), self.decimals());
        format!("{:.d$} × {:.d$} {}", width_pt * k, height_pt * k, self.label())
    }
}

/// The unit for a locale tag's region: `en_US.UTF-8`, `en-US`, `bg-BG`, `zh-Hans-CN`, `en_GB@euro`.
/// `None` without a region (`C`, `POSIX`, `en`).
#[allow(dead_code)]
pub(crate) fn unit_for_tag(tag: &str) -> Option<Unit> {
    let tag = tag.split(['.', '@']).next().unwrap_or_default();
    let region = tag.split(['-', '_']).skip(1).filter(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic())).last()?;
    Some(match region.to_ascii_uppercase().as_str() {
        "US" | "LR" | "MM" => Unit::Inches,
        _ => Unit::Millimetres,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn detect() -> Option<Unit> {
    // The system's own measurement setting first: a terminal's LANG is often en_US whatever the region.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleMeasurementUnits"]).output()
        && out.status.success()
    {
        match String::from_utf8_lossy(&out.stdout).trim() {
            "Inches" => return Some(Unit::Inches),
            "Centimeters" => return Some(Unit::Millimetres),
            _ => {}
        }
    }
    // AppleMeasurementUnits only exists once someone changed Measurement system by hand, and apps
    // started from the Finder get no LANG, so the region of the system locale decides next.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLocale"]).output()
        && out.status.success()
        && let Some(unit) = unit_for_tag(String::from_utf8_lossy(&out.stdout).trim())
    {
        return Some(unit);
    }
    // Region ▸ Measurement system, read in-process: a child `reg.exe` is a process-creation event
    // that endpoint protection watches on managed PCs, and a preference doesn't warrant one.
    #[cfg(target_os = "windows")]
    if let Some(unit) = windows_measure().and_then(|v| unit_for_imeasure(&v)) {
        return Some(unit);
    }
    ["LC_ALL", "LC_MEASUREMENT", "LANG"].iter().find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()).and_then(|v| unit_for_tag(&v)))
}

/// The unit for the Windows `iMeasure` value (Region ▸ Measurement system): "0" metric, "1" U.S.
/// `None` for anything else, so a missing or odd value falls through to the locale check.
#[cfg(any(test, target_os = "windows"))]
pub(crate) fn unit_for_imeasure(value: &str) -> Option<Unit> {
    match value.trim() {
        "1" => Some(Unit::Inches),
        "0" => Some(Unit::Millimetres),
        _ => None,
    }
}

/// `HKCU\Control Panel\International\iMeasure` as text; `None` when it is missing, unreadable or
/// not a string.
#[cfg(target_os = "windows")]
fn windows_measure() -> Option<String> {
    use winsafe::{HKEY, RegistryValue, co::RRF};
    match HKEY::CURRENT_USER.RegGetValue(Some("Control Panel\\International"), Some("iMeasure"), RRF::RT_REG_SZ).ok()? {
        RegistryValue::Sz(v) => Some(v),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
fn detect() -> Option<Unit> {
    None
}

#[cfg(test)]
mod tests {
    use super::{Unit, unit_for_imeasure, unit_for_tag};

    #[test]
    fn region_decides_the_unit() {
        assert_eq!(unit_for_tag("en_US.UTF-8"), Some(Unit::Inches));
        assert_eq!(unit_for_tag("en-US"), Some(Unit::Inches));
        assert_eq!(unit_for_tag("en_GB@euro"), Some(Unit::Millimetres));
        assert_eq!(unit_for_tag("bg-BG"), Some(Unit::Millimetres));
        assert_eq!(unit_for_tag("de_DE.UTF-8"), Some(Unit::Millimetres));
        assert_eq!(unit_for_tag("zh-Hans-CN"), Some(Unit::Millimetres));
        assert_eq!(unit_for_tag("my_MM"), Some(Unit::Inches));
        assert_eq!(unit_for_tag("C"), None);
        assert_eq!(unit_for_tag("POSIX"), None);
        assert_eq!(unit_for_tag("en"), None);
        assert_eq!(unit_for_tag(""), None);
    }

    #[test]
    fn windows_measurement_system_decides_the_unit() {
        assert_eq!(unit_for_imeasure("1"), Some(Unit::Inches));
        assert_eq!(unit_for_imeasure("0"), Some(Unit::Millimetres));
        assert_eq!(unit_for_imeasure(" 1\r\n"), Some(Unit::Inches));
        assert_eq!(unit_for_imeasure(""), None);
        assert_eq!(unit_for_imeasure("2"), None);
        assert_eq!(unit_for_imeasure("metric"), None);
    }

    /// The registry read works in-process (no `reg.exe`) and gives a value the mapping understands.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reads_imeasure_in_process() {
        let value = super::windows_measure();
        // Every Windows user profile has Region settings; if a sandbox ever lacks them, the read must
        // simply be None rather than failing.
        if let Some(v) = value {
            assert!(unit_for_imeasure(&v).is_some(), "unexpected iMeasure {v:?}");
        }
    }

    #[test]
    fn a4_in_every_unit() {
        assert_eq!(Unit::Millimetres.size(595.276, 841.89), "210.0 × 297.0 mm");
        assert_eq!(Unit::Centimetres.size(595.276, 841.89), "21.00 × 29.70 cm");
        assert_eq!(Unit::Inches.size(612.0, 792.0), "8.50 × 11.00 in");
        assert_eq!(Unit::Points.size(612.0, 792.0), "612.0 × 792.0 pt");
    }

    #[test]
    fn saved_names_round_trip() {
        for u in Unit::ALL {
            let v = serde_json::to_value(u).unwrap();
            assert_eq!(serde_json::from_value::<Unit>(v).unwrap(), u);
        }
        assert_eq!(serde_json::to_value(Unit::Millimetres).unwrap(), "millimetres");
    }
}
