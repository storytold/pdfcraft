//! App-managed colour for printing (Acrobat's Color Handling ▸ "app manages colours"): printed
//! pages, rendered in sRGB, are converted into the printer's ICC profile with a rendering intent
//! and optional black point compensation before the driver sees them. The default leaves colour
//! to the printer, as before.
//!
//! The pages go as converted pictures, so the driver's own colour management should be off for
//! the job; otherwise the printer converts them a second time.

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use craft_cms::Intent;
use craft_cms::{Builtin, ColorSpace, Profile, TransformOptions};

/// The largest profile read from disk. Printer profiles with large tables are a few megabytes.
pub const MAX_PROFILE_BYTES: u64 = 32 << 20;

/// How many files the installed-profile listing looks at, at most.
const MAX_LISTED: usize = 2000;

/// An ICC profile pages can be printed into: an RGB or grey profile that can be a destination.
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterProfile {
    profile: Arc<Profile>,
    name: String,
}

impl PrinterProfile {
    /// `bytes` as a printer profile. Refused when they are not an ICC profile, when it has no
    /// path from the PCS to the device, or when its device space is neither RGB nor grey (the
    /// pictures a print carries are RGB).
    pub fn from_bytes(bytes: &[u8]) -> Result<PrinterProfile, String> {
        let profile = Profile::parse(bytes).map_err(|e| format!("not a usable ICC profile: {e}"))?;
        if !matches!(profile.color_space, ColorSpace::Rgb | ColorSpace::Gray) {
            return Err(format!("a {:?} profile can't be printed into: choose an RGB or grey printer profile", profile.color_space));
        }
        if !profile.can_be_destination() {
            return Err("the profile only describes a source: it has no table to convert colours into it".into());
        }
        let name = profile.description.trim().to_string();
        Ok(PrinterProfile { profile: Arc::new(profile), name })
    }

    /// The profile in the file at `path`, at most [`MAX_PROFILE_BYTES`] long.
    pub fn read(path: &Path) -> Result<PrinterProfile, String> {
        let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len > MAX_PROFILE_BYTES {
            return Err(format!("{}: larger than an ICC profile can be here ({} MB)", path.display(), MAX_PROFILE_BYTES >> 20));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        PrinterProfile::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The profile's description (its `desc` tag), possibly empty.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ICC bytes, as read.
    pub fn bytes(&self) -> Arc<Vec<u8>> {
        self.profile.to_bytes()
    }

    /// Whether the printer's device space is grey.
    pub fn is_gray(&self) -> bool {
        self.profile.color_space == ColorSpace::Gray
    }
}

/// App-managed colour: the profile the pages are converted into, how, and whether black point
/// compensation maps the darkest sRGB black onto the printer's darkest black.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagedColour {
    pub profile: PrinterProfile,
    pub intent: Intent,
    pub black_point: bool,
}

impl ManagedColour {
    /// Converts opaque sRGB pixels (RGBA, alpha ignored and kept) into the printer profile in
    /// place. A grey profile's value is written to all three channels.
    pub fn convert(&self, rgba: &mut [u8]) -> Result<(), String> {
        let opts = TransformOptions { intent: self.intent, bpc: self.black_point, ..TransformOptions::default() };
        let t = craft_cms::cached(Builtin::Srgb.profile(), &self.profile.profile, opts)
            .map_err(|e| format!("can't convert into the printer profile: {e}"))?;
        let source = rgba.to_vec();
        if self.profile.is_gray() {
            let mut grey = vec![0u8; source.len() / 4];
            t.convert_u8(&source, 4, &mut grey, 1, false).map_err(|e| format!("can't convert into the printer profile: {e}"))?;
            for (px, g) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(grey) {
                px[..3].fill(g);
            }
            Ok(())
        } else {
            t.convert_u8(&source, 4, rgba, 4, true).map_err(|e| format!("can't convert into the printer profile: {e}"))
        }
    }
}

/// A profile installed on this computer that pages can be printed into.
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledProfile {
    pub path: PathBuf,
    /// Its description, else its file name.
    pub name: String,
    pub gray: bool,
}

/// The folders the system keeps colour profiles in, those that exist.
pub fn profile_folders() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        dirs.push(root.join("System32").join("spool").join("drivers").join("color"));
    } else if cfg!(target_os = "macos") {
        dirs.push("/System/Library/ColorSync/Profiles".into());
        dirs.push("/Library/ColorSync/Profiles".into());
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/ColorSync/Profiles"));
        }
    } else {
        dirs.push("/usr/share/color/icc".into());
        dirs.push("/usr/local/share/color/icc".into());
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(&home).join(".local/share/color/icc"));
            dirs.push(PathBuf::from(home).join(".color/icc"));
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// The RGB and grey profiles in `folders` and their immediate subfolders that pages can be
/// printed into, sorted by name. Files that aren't such a profile are skipped.
pub fn installed_profiles_in(folders: &[PathBuf]) -> Vec<InstalledProfile> {
    installed_profiles_where(folders, &|_| true)
}

/// [`installed_profiles_in`], reading only the files `allow` passes (a caller confined to some
/// folders checks each file, links followed, before it is opened).
pub fn installed_profiles_where(folders: &[PathBuf], allow: &dyn Fn(&Path) -> bool) -> Vec<InstalledProfile> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in folders {
        collect(dir, 1, &mut files);
    }
    let mut out: Vec<InstalledProfile> = files
        .into_iter()
        .filter(|path| allow(path))
        .filter_map(|path| {
            let p = PrinterProfile::read(&path).ok()?;
            let name = if p.name().is_empty() { path.file_name()?.to_string_lossy().into_owned() } else { p.name().to_string() };
            Some(InstalledProfile { gray: p.is_gray(), name, path })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
    out
}

/// The profiles the system has installed (see [`profile_folders`]).
pub fn installed_profiles() -> Vec<InstalledProfile> {
    installed_profiles_in(&profile_folders())
}

/// The `.icc` and `.icm` files in `dir`, descending `depth` more levels, up to [`MAX_LISTED`].
fn collect(dir: &Path, depth: u32, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if files.len() >= MAX_LISTED {
            return;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if depth > 0 {
                collect(&path, depth - 1, files);
            }
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("icc") || e.eq_ignore_ascii_case("icm")) {
            files.push(path);
        }
    }
}

/// The installed profile `name` names: its description or its file name, ignoring case.
pub fn find_installed<'a>(profiles: &'a [InstalledProfile], name: &str) -> Option<&'a InstalledProfile> {
    let name = name.trim();
    profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name)).or_else(|| {
        profiles.iter().find(|p| {
            let file = p.path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default();
            let stem = p.path.file_stem().map(|f| f.to_string_lossy()).unwrap_or_default();
            file.eq_ignore_ascii_case(name) || stem.eq_ignore_ascii_case(name)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_cms::Curve;

    /// A grey printer profile whose darkest black is a light charcoal (8% luminance, L* 34):
    /// what a matte paper or a worn toner gives. Synthetic, from the built-in sGray.
    fn lifted_black_grey() -> Vec<u8> {
        let mut p = Builtin::SGray.profile().clone();
        p.gray_trc = Some(Curve::Table((0..=255).map(|i| 0.08 + 0.92 * (i as f32 / 255.0)).collect()));
        p.black_point = None;
        p.description = "Lifted black grey".into();
        p.with_encoded_bytes().to_bytes().to_vec()
    }

    fn convert(profile: &[u8], bpc: bool, rgba: &[u8]) -> Vec<u8> {
        let colour = ManagedColour { profile: PrinterProfile::from_bytes(profile).unwrap(), intent: Intent::RelativeColorimetric, black_point: bpc };
        let mut out = rgba.to_vec();
        colour.convert(&mut out).unwrap();
        out
    }

    /// Dark sRGB greys below the printer's black: without compensation they all clip to its
    /// black; with it they stay apart and in order (test the shadows, not only mid grey).
    #[test]
    fn black_point_compensation_keeps_dark_shadows_apart() {
        let profile = lifted_black_grey();
        let darks = [20u8, 40, 60].iter().flat_map(|&v| [v, v, v, 255]).collect::<Vec<u8>>();
        let clipped = convert(&profile, false, &darks);
        assert_eq!((clipped[0], clipped[4], clipped[8]), (0, 0, 0), "{clipped:?}");
        let kept = convert(&profile, true, &darks);
        assert!(kept[0] < kept[4] && kept[4] < kept[8], "{kept:?}");
        assert!(kept.as_chunks::<4>().0.iter().all(|&[r, g, b, a]| r == g && g == b && a == 255), "grey written to R, G and B: {kept:?}");
    }

    /// An RGB printer profile changes the values (sRGB's green is not the profile's green) and
    /// keeps alpha.
    #[test]
    fn rgb_profiles_convert_the_values_and_keep_alpha() {
        let wide = Builtin::AdobeRgbCompat.profile().to_bytes().to_vec();
        let out = convert(&wide, true, &[0, 255, 0, 128, 255, 255, 255, 255]);
        assert!(out[0] > 100 && out[1] > 240 && out[2] > 20, "sRGB green inside a wider gamut: {out:?}");
        assert_eq!(out[3], 128);
        assert!(out[4..7].iter().all(|&v| v >= 254), "white stays white: {out:?}");
        let same = convert(&Builtin::Srgb.profile().to_bytes(), true, &[10, 120, 230, 255]);
        assert!(same.iter().zip([10u8, 120, 230, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "sRGB into sRGB: {same:?}");
    }

    #[test]
    fn cmyk_and_other_bytes_are_refused() {
        let cmyk = Builtin::CoatedCmyk.profile().to_bytes();
        assert!(PrinterProfile::from_bytes(&cmyk).is_err_and(|e| e.contains("RGB or grey")));
        assert!(PrinterProfile::from_bytes(b"not a profile").is_err());
        assert!(PrinterProfile::from_bytes(&[]).is_err());
    }

    /// The listing finds RGB and grey profiles one folder deep, by description, and skips CMYK
    /// profiles, other files and broken ones; a name finds one by description or file name.
    #[test]
    fn installed_profiles_are_rgb_and_grey_ones() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-print-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("vendor").join("deeper")).unwrap();
        std::fs::write(dir.join("office.ICC"), Builtin::Srgb.profile().to_bytes().as_slice()).unwrap();
        std::fs::write(dir.join("vendor").join("matte.icm"), lifted_black_grey()).unwrap();
        std::fs::write(dir.join("vendor").join("deeper").join("hidden.icc"), Builtin::Srgb.profile().to_bytes().as_slice()).unwrap();
        std::fs::write(dir.join("press.icc"), Builtin::CoatedCmyk.profile().to_bytes().as_slice()).unwrap();
        std::fs::write(dir.join("broken.icc"), b"acsp?").unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        let found = installed_profiles_in(std::slice::from_ref(&dir));
        let names: Vec<(&str, bool)> = found.iter().map(|p| (p.name.as_str(), p.gray)).collect();
        assert_eq!(names.len(), 2, "{found:?}");
        assert!(names.contains(&("Lifted black grey", true)), "{names:?}");
        assert!(found.iter().any(|p| !p.gray && p.path.ends_with("office.ICC")), "{found:?}");
        assert!(find_installed(&found, "lifted BLACK grey").is_some_and(|p| p.gray));
        assert!(find_installed(&found, "matte").is_some_and(|p| p.gray), "by file stem");
        assert!(find_installed(&found, "office.icc").is_some_and(|p| !p.gray), "by file name");
        assert!(find_installed(&found, "press").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
