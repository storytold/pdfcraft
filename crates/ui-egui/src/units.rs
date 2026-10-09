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
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Region ▸ Measurement system: 0 metric, 1 U.S. The absolute path keeps a `reg` earlier on
        // PATH from running; CREATE_NO_WINDOW keeps a console from flashing up.
        let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let reg = std::path::Path::new(&windir).join("System32").join("reg.exe");
        if let Ok(out) = std::process::Command::new(reg)
            .args(["query", "HKCU\\Control Panel\\International", "/v", "iMeasure"])
            .creation_flags(0x0800_0000)
            .output()
            && out.status.success()
        {
            match String::from_utf8_lossy(&out.stdout).split_whitespace().last() {
                Some("1") => return Some(Unit::Inches),
                Some("0") => return Some(Unit::Millimetres),
                _ => {}
            }
        }
    }
    ["LC_ALL", "LC_MEASUREMENT", "LANG"].iter().find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()).and_then(|v| unit_for_tag(&v)))
}

#[cfg(target_arch = "wasm32")]
fn detect() -> Option<Unit> {
    None
}

#[cfg(test)]
mod tests {
    use super::{Unit, unit_for_tag};

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
