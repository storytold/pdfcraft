//! Runtime interface fallbacks, read from fonts already installed on this machine.
//!
//! The Arabic-script face is the last fallback in every family. The Chinese face comes from
//! [`pdfcraft_fonts::han`], the same installed face the page renderer falls back to, so one search
//! answers for both. These faces are UI-only: never embedded, copied or shipped (AGENTS.md §1.4).
//! `PDFCRAFT_SYSTEM_FONTS=0` disables both (published screenshots do).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// Arabic letter alef: the face must have it to be worth loading.
const PROBE: &str = "\u{0627}";

/// The installed Arabic-script fallback, read once. `None` when it is turned off or no candidate
/// fits.
pub fn fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| load(&candidates(), PROBE)).clone()
}

/// One installed Chinese face, independent of the interface language: a Chinese file name in an
/// English interface needs it too. `None` in a web build and when the machine has no Chinese face.
pub fn chinese_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let face = pdfcraft_fonts::han()?;
            let mut data = FontData::from_owned((*face.bytes).clone());
            data.index = face.index;
            data.tweak.y_offset_factor = pdfcraft_fonts::ui_y_offset_factor(PRIMARY_UI_FONT);
            Some(Arc::new(data))
        })
        .clone()
}

/// The face Chinese text shares a line with: every family starts with an Inter, and Inter's three
/// weights have the same vertical metrics. Its tall row makes egui lift a Chinese face's glyphs
/// above it, which [`pdfcraft_fonts::ui_y_offset_factor`] cancels.
const PRIMARY_UI_FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

fn load(paths: &[PathBuf], probes: &str) -> Option<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    paths.iter().find_map(|path| read(path, probes))
}

fn windows_font_dir() -> PathBuf {
    std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// Well-known locations of faces with broad script coverage, best first.
fn candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["segoeui.ttf", "tahoma.ttf", "arial.ttf"].iter().map(|f| windows_font_dir().join("Fonts").join(f)).collect()
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

fn read(path: &Path, probes: &str) -> Option<Arc<FontData>> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    // Bound the read too: the file may grow after the metadata check.
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let index = face_with(&bytes, probes)?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    log::info!("interface font fallback: {} (face {index})", path.display());
    Some(Arc::new(data))
}

/// All probes must map in the same collection face; egui parses fonts with the same skrifa, so a
/// face accepted here is one it can load.
fn face_with(bytes: &[u8], probes: &str) -> Option<u32> {
    use skrifa::MetadataProvider as _;
    (0..MAX_FACES).find(|&index| skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| probes.chars().all(|c| font.charmap().map(c).is_some())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Chinese face is found by `pdfcraft_fonts::han`, not by this module's Arabic list, so it is
    /// not empty on any desktop platform — and empty in a web build.
    #[test]
    fn arabic_candidates_are_absolute_font_files() {
        let list = candidates();
        assert!(!list.is_empty());
        assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")));
    }

    #[test]
    fn broken_and_missing_files_are_skipped() {
        assert_eq!(face_with(b"", PROBE), None);
        assert_eq!(face_with(b"not a font at all", PROBE), None);
        assert_eq!(face_with(&[0u8; 4096], PROBE), None);
        assert!(read(Path::new("definitely/not/here.ttf"), PROBE).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir(), PROBE).is_none());
    }

    #[test]
    fn a_face_must_cover_every_probe() {
        // Inter is Latin, Greek and Cyrillic only.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, PROBE), None);
        assert_eq!(face_with(inter, "AB"), Some(0));
    }

    #[test]
    fn oversized_font_files_are_skipped() {
        let path = std::env::temp_dir().join(format!("pdfcraft-oversized-font-{}.ttf", std::process::id()));
        let file = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).expect("a temp file");
        file.set_len(MAX_BYTES + 1).expect("a sparse temp file");
        drop(file);
        let result = read(&path, PROBE);
        std::fs::remove_file(&path).expect("the temp file is removed");
        assert!(result.is_none());
    }
}
