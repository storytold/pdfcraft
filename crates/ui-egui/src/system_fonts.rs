//! The last-resort interface font: one face already installed on this machine.
//!
//! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; this one
//! only draws characters none of them has, such as an Arabic file name in a build without
//! craft-fonts. It is read at runtime and never embedded or shipped (AGENTS.md §1.4), and
//! `PDFCRAFT_SYSTEM_FONTS=0` turns it off (published screenshots do).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// Arabic letter alef: the face must have it to be worth loading.
const PROBE: char = '\u{0627}';

/// The installed fallback face, read once. `None` when it is turned off or no candidate fits.
pub fn fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(load).clone()
}

fn load() -> Option<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    candidates().iter().find_map(|path| read(path))
}

/// Well-known locations of faces with broad script coverage, best first.
fn candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        ["segoeui.ttf", "tahoma.ttf", "arial.ttf"].iter().map(|f| dir.join("Fonts").join(f)).collect()
    } else if cfg!(target_os = "macos") {
        ["/System/Library/Fonts/SFArabic.ttf", "/System/Library/Fonts/GeezaPro.ttc", "/System/Library/Fonts/Supplemental/Arial.ttf"]
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        let files = [
            "truetype/noto/NotoSansArabic-Regular.ttf",
            "noto/NotoSansArabic-Regular.ttf",
            "google-noto/NotoSansArabic-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
            "dejavu/DejaVuSans.ttf",
            "dejavu-sans-fonts/DejaVuSans.ttf",
        ];
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
    }
}

fn read(path: &Path) -> Option<Arc<FontData>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let index = face_with(&bytes, PROBE)?;
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
        assert_eq!(face_with(b"", PROBE), None);
        assert_eq!(face_with(b"not a font at all", PROBE), None);
        assert_eq!(face_with(&[0u8; 4096], PROBE), None);
        assert!(read(Path::new("definitely/not/here.ttf")).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir()).is_none());
    }

    #[test]
    fn a_face_without_the_probe_is_rejected() {
        // Inter is Latin, Greek and Cyrillic only.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, PROBE), None);
        assert_eq!(face_with(inter, 'A'), Some(0));
    }

    #[test]
    fn candidates_are_absolute_font_files() {
        let list = candidates();
        assert!(!list.is_empty());
        assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")));
    }
}
