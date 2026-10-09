//! The last-resort interface fonts: faces already installed on this machine.
//!
//! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; these
//! only draw characters none of them has, such as an Arabic file name in a build without
//! craft-fonts, or the Hebrew interface (no embedded face has Hebrew). They are read at runtime
//! and never embedded or shipped (AGENTS.md §1.4), and `PDFCRAFT_SYSTEM_FONTS=0` turns them off
//! (published screenshots do).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// One letter per script a fallback face is looked for: Arabic alef, Hebrew alef. Each script
/// gets the first candidate that has it, so a machine whose best Arabic face lacks Hebrew (macOS:
/// SF Arabic) still draws Hebrew.
const PROBES: [char; 2] = ['\u{0627}', '\u{05D0}'];

/// The installed fallback faces, read once, first script first and without repeats. Empty when
/// they are turned off or no candidate fits.
pub fn fallbacks() -> Vec<Arc<FontData>> {
    static CACHE: OnceLock<Vec<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(load).clone()
}

fn load() -> Vec<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return Vec::new();
    }
    let candidates = candidates();
    let mut picked: Vec<(&PathBuf, Arc<FontData>)> = Vec::new();
    for probe in PROBES {
        // A face picked for an earlier script may already cover this one.
        if picked.iter().any(|(_, data)| covers(data, probe)) {
            continue;
        }
        if let Some(found) = candidates.iter().filter(|p| !picked.iter().any(|(q, _)| q == p)).find_map(|p| read(p, probe).map(|d| (p, d))) {
            picked.push(found);
        }
    }
    picked.into_iter().map(|(_, data)| data).collect()
}

/// Well-known locations of faces with broad script coverage, best first.
fn candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        ["segoeui.ttf", "tahoma.ttf", "arial.ttf"].iter().map(|f| dir.join("Fonts").join(f)).collect()
    } else if cfg!(target_os = "macos") {
        [
            "/System/Library/Fonts/SFArabic.ttf",
            "/System/Library/Fonts/GeezaPro.ttc",
            "/System/Library/Fonts/SFHebrew.ttf",
            "/System/Library/Fonts/ArialHB.ttc",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else {
        let files = [
            "truetype/noto/NotoSansArabic-Regular.ttf",
            "noto/NotoSansArabic-Regular.ttf",
            "google-noto/NotoSansArabic-Regular.ttf",
            "truetype/noto/NotoSansHebrew-Regular.ttf",
            "noto/NotoSansHebrew-Regular.ttf",
            "google-noto/NotoSansHebrew-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
            "dejavu/DejaVuSans.ttf",
            "dejavu-sans-fonts/DejaVuSans.ttf",
        ];
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
    }
}

fn read(path: &Path, probe: char) -> Option<Arc<FontData>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let index = face_with(&bytes, probe)?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    log::info!("interface font fallback: {}", path.display());
    Some(Arc::new(data))
}

/// Does the loaded face draw `c`?
fn covers(data: &FontData, c: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(&data.font, data.index).is_ok_and(|font| font.charmap().map(c).is_some())
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

    const ARABIC: char = PROBES[0];

    #[test]
    fn broken_and_missing_files_are_skipped() {
        assert_eq!(face_with(b"", ARABIC), None);
        assert_eq!(face_with(b"not a font at all", ARABIC), None);
        assert_eq!(face_with(&[0u8; 4096], ARABIC), None);
        assert!(read(Path::new("definitely/not/here.ttf"), ARABIC).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir(), ARABIC).is_none());
    }

    #[test]
    fn a_face_without_the_probe_is_rejected() {
        // Inter is Latin, Greek and Cyrillic only.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        for probe in PROBES {
            assert_eq!(face_with(inter, probe), None);
        }
        assert_eq!(face_with(inter, 'A'), Some(0));
    }

    #[test]
    fn candidates_are_absolute_font_files() {
        let list = candidates();
        assert!(!list.is_empty());
        assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")));
    }
}
