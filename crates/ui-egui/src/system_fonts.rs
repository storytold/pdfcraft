//! Runtime interface fallbacks, read from fonts already installed on this machine.
//!
//! The existing Arabic-script face remains the last fallback. Windows can additionally
//! load one Chinese face for its catalog and file names, without losing Arabic coverage.
//! These faces are UI-only: never embedded, copied or shipped (AGENTS.md §1.4).
//! `PDFCRAFT_SYSTEM_FONTS=0` disables both (published screenshots do).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// Arabic letter alef: preserve the existing fallback selection.
const PROBE: &str = "\u{0627}";
/// Shared and Simplified-only Han characters, not just a glyph a Japanese face may have.
const CHINESE_PROBES: &str = "中文欢迎编辑导出";

/// The existing installed fallback, read once. None when disabled or no candidate fits.
pub fn fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| load(&candidates(), PROBE)).clone()
}

/// One installed Chinese face on Windows, independent of the UI language.
pub fn chinese_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| load(&chinese_candidates(), CHINESE_PROBES)).clone()
}

fn load(paths: &[PathBuf], probes: &str) -> Option<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    paths.iter().find_map(|path| read(path, probes))
}

fn windows_font_dir() -> PathBuf {
    std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from).join("Fonts")
}

fn chinese_candidates() -> Vec<PathBuf> {
    if cfg!(windows) { ["msyh.ttc", "simhei.ttf", "simsun.ttc"].iter().map(|f| windows_font_dir().join(f)).collect() } else { Vec::new() }
}

/// Existing Arabic-script locations, best first; unchanged on every platform.
fn candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["segoeui.ttf", "tahoma.ttf", "arial.ttf"].iter().map(|f| windows_font_dir().join(f)).collect()
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

/// All probes must map in the same collection face; egui uses the same skrifa parser.
fn face_with(bytes: &[u8], probes: &str) -> Option<u32> {
    use skrifa::MetadataProvider as _;
    (0..MAX_FACES).find(|&index| skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| probes.chars().all(|c| font.charmap().map(c).is_some())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_and_missing_files_are_skipped() {
        for probes in [PROBE, CHINESE_PROBES] {
            assert_eq!(face_with(b"", probes), None);
            assert_eq!(face_with(b"not a font at all", probes), None);
            assert_eq!(face_with(&[0u8; 4096], probes), None);
            assert!(read(Path::new("definitely/not/here.ttf"), probes).is_none());
            assert!(read(&std::env::temp_dir(), probes).is_none());
        }
    }

    #[test]
    fn a_face_must_cover_every_probe() {
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, PROBE), None);
        assert_eq!(face_with(inter, CHINESE_PROBES), None);
        assert_eq!(face_with(inter, "AB"), Some(0));
        assert_eq!(face_with(inter, "A欢"), None);
    }

    #[test]
    fn oversized_font_files_are_skipped() {
        let path = std::env::temp_dir().join(format!("pdfcraft-oversized-font-{}.ttf", std::process::id()));
        let file = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
        file.set_len(MAX_BYTES + 1).unwrap();
        drop(file);
        let result = read(&path, CHINESE_PROBES);
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn candidates_are_font_files() {
        let list = candidates();
        assert!(!list.is_empty());
        assert!(list.iter().chain(chinese_candidates().iter()).all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")));
        assert_eq!(chinese_candidates().is_empty(), !cfg!(windows));
    }
}
