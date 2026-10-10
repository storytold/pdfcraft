//! The last-resort interface fonts: faces already installed on this machine.
//!
//! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; these
//! only draw characters none of them has, such as an Arabic or Thai file name in a build without
//! craft-fonts. They are read at runtime and never embedded or shipped (AGENTS.md §1.4), and
//! `PDFCRAFT_SYSTEM_FONTS=0` turns them off (published screenshots do).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;

/// A script the installed fallback probes for: one character a face must draw, one face name.
#[derive(Clone, Copy)]
enum Probe {
    /// Arabic letter alef. Windows picks Segoe UI for this probe, and that face draws no Thai —
    /// which is why Thai needs a probe of its own.
    Arabic,
    /// Thai character ko kai (`0E01`). Segoe UI has no Thai; Leelawadee UI and Tahoma do.
    Thai,
}

impl Probe {
    /// Probed in order; the scripts do not overlap.
    const ALL: [Probe; 2] = [Probe::Arabic, Probe::Thai];

    /// The character a face must be able to draw to be worth loading.
    fn glyph(self) -> char {
        match self {
            Probe::Arabic => '\u{0627}',
            Probe::Thai => '\u{0E01}',
        }
    }

    /// The face name [`crate::theme::installed_font_definitions`] registers it under.
    fn name(self) -> &'static str {
        match self {
            Probe::Arabic => crate::theme::SYSTEM_FALLBACK,
            Probe::Thai => crate::theme::SYSTEM_FALLBACK_THAI,
        }
    }
}

/// The installed fallback faces, read once: at most one per probed script, in probe order.
/// Empty when it is turned off or no candidate fits.
pub fn fallbacks() -> Vec<(&'static str, Arc<FontData>)> {
    static CACHE: OnceLock<Vec<(&'static str, Arc<FontData>)>> = OnceLock::new();
    CACHE.get_or_init(load).clone()
}

fn load() -> Vec<(&'static str, Arc<FontData>)> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return Vec::new();
    }
    let mut faces = Vec::new();
    // (file, face) pairs already read: two probes can land on the same face (Tahoma draws both
    // Arabic and Thai), and one copy of its bytes serves both.
    let mut loaded: Vec<(PathBuf, u32)> = Vec::new();
    for probe in Probe::ALL {
        let found = candidates(probe).into_iter().find_map(|path| read(&path, probe.glyph()).map(|data| (path, data)));
        let Some((path, data)) = found else { continue };
        if loaded.contains(&(path.clone(), data.index)) {
            continue;
        }
        loaded.push((path, data.index));
        faces.push((probe.name(), data));
    }
    faces
}

/// Well-known locations of faces with the probe's script, best first.
fn candidates(probe: Probe) -> Vec<PathBuf> {
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let files: &[&str] = match probe {
            Probe::Arabic => &["segoeui.ttf", "tahoma.ttf", "arial.ttf"],
            // Leelawadee UI, the Windows Thai interface face, then Leelawadee (Windows 7) and
            // Tahoma, which draws Thai on every Windows.
            Probe::Thai => &["leelawui.ttf", "leelawad.ttf", "tahoma.ttf"],
        };
        files.iter().map(|f| dir.join("Fonts").join(f)).collect()
    } else if cfg!(target_os = "macos") {
        let files: &[&str] = match probe {
            Probe::Arabic => {
                &["/System/Library/Fonts/SFArabic.ttf", "/System/Library/Fonts/GeezaPro.ttc", "/System/Library/Fonts/Supplemental/Arial.ttf"]
            }
            Probe::Thai => &[
                "/System/Library/Fonts/Supplemental/Thonburi.ttc",
                "/System/Library/Fonts/Supplemental/Thonburi.ttf",
                "/System/Library/Fonts/Thonburi.ttf",
                "/System/Library/Fonts/Supplemental/Ayuthaya.ttf",
                "/System/Library/Fonts/Supplemental/Tahoma.ttf",
            ],
        };
        files.iter().map(PathBuf::from).collect()
    } else {
        let files: &[&str] = match probe {
            Probe::Arabic => &[
                "truetype/noto/NotoSansArabic-Regular.ttf",
                "noto/NotoSansArabic-Regular.ttf",
                "google-noto/NotoSansArabic-Regular.ttf",
                "truetype/dejavu/DejaVuSans.ttf",
                "TTF/DejaVuSans.ttf",
                "dejavu/DejaVuSans.ttf",
                "dejavu-sans-fonts/DejaVuSans.ttf",
            ],
            Probe::Thai => &[
                "truetype/noto/NotoSansThai-Regular.ttf",
                "noto/NotoSansThai-Regular.ttf",
                "google-noto/NotoSansThai-Regular.ttf",
                // The Thai Linux Working Group fonts, packaged by Debian and Ubuntu.
                "truetype/tlwg/Garuda.ttf",
                "truetype/tlwg/Loma.ttf",
                "truetype/tlwg/Waree.ttf",
            ],
        };
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
    }
}

fn read(path: &Path, glyph: char) -> Option<Arc<FontData>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let index = face_with(&bytes, glyph)?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    log::info!("interface font fallback: {}", path.display());
    Some(Arc::new(data))
}

/// The first face of the file that parses and maps `c`. egui parses fonts with the same skrifa,
/// so a face accepted here is one it can load.
fn face_with(bytes: &[u8], c: char) -> Option<u32> {
    use skrifa::MetadataProvider as _;
    (0..MAX_FACES).find(|&index| skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| font.charmap().map(c).is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_and_missing_files_are_skipped() {
        for probe in Probe::ALL {
            assert_eq!(face_with(b"", probe.glyph()), None);
            assert_eq!(face_with(b"not a font at all", probe.glyph()), None);
            assert_eq!(face_with(&[0u8; 4096], probe.glyph()), None);
            assert!(read(Path::new("definitely/not/here.ttf"), probe.glyph()).is_none());
        }
        // A directory is not a font.
        assert!(read(&std::env::temp_dir(), Probe::Arabic.glyph()).is_none());
    }

    #[test]
    fn a_face_without_the_probe_is_rejected() {
        // Inter is Latin, Greek and Cyrillic only: no Arabic alef, no Thai ko kai.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        for probe in Probe::ALL {
            assert_eq!(face_with(inter, probe.glyph()), None, "{}", probe.name());
        }
        assert_eq!(face_with(inter, 'A'), Some(0));
    }

    #[test]
    fn candidates_are_absolute_font_files() {
        for probe in Probe::ALL {
            let list = candidates(probe);
            assert!(!list.is_empty(), "{}", probe.name());
            assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")), "{}: {list:?}", probe.name());
        }
    }

    #[test]
    fn each_probe_registers_a_distinct_name() {
        // A shared name would make the second face overwrite the first in `font_data`.
        assert_ne!(Probe::Arabic.name(), Probe::Thai.name());
    }
}
