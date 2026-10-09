//! A Chinese (Han) face already installed on this machine, read at runtime to draw a page whose
//! Chinese text uses a CID font the PDF doesn't embed. AGENTS.md §1.4 allows the desktop app to
//! read such a face; it is never embedded, shipped, committed or copied, and nothing from it
//! enters the document — it is drawn on screen only. `PDFCRAFT_SYSTEM_FONTS=0` turns it off
//! (published screenshots set it).
//!
//! The interface keeps its own installed fallback (`crates/ui-egui/src/system_fonts.rs`, an
//! Arabic face); this one serves the page renderer, which lives below the UI and can't reach it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use skrifa::{FontRef, MetadataProvider};

/// A face read from this machine. `bytes` is shared with the renderer (`hayro`'s
/// `Arc<dyn AsRef<[u8]>>`) without a second copy.
pub struct SystemFace {
    pub bytes: Arc<Vec<u8>>,
    /// The chosen face inside a font collection (`.ttc`); 0 for a single-face file.
    pub index: u32,
}

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// A common simplified-Chinese hanzi (汉). A face that maps it can draw Simplified Chinese, which a
/// Japanese-only face cannot (欢, 电, 图 … would otherwise be .notdef boxes).
const HAN_PROBE: char = '\u{6c49}';

/// An installed face that covers simplified/traditional Chinese, read once. `None` when it is
/// turned off or no candidate fits.
pub fn han() -> Option<Arc<SystemFace>> {
    static CACHE: OnceLock<Option<Arc<SystemFace>>> = OnceLock::new();
    CACHE.get_or_init(load).clone()
}

fn load() -> Option<Arc<SystemFace>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    candidates().iter().find_map(|path| read(path))
}

/// Well-known Simplified Chinese faces, best first.
fn candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        ["msyh.ttc", "msyhbd.ttc", "simsun.ttc", "simhei.ttf", "msjh.ttc"].iter().map(|f| dir.join("Fonts").join(f)).collect()
    } else if cfg!(target_os = "macos") {
        [
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/Supplemental/Songti.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else {
        let files = [
            "opentype/noto/NotoSansCJK-Regular.ttc",
            "noto-cjk/NotoSansCJK-Regular.ttc",
            "google-noto-cjk/NotoSansCJK-Regular.ttc",
            "truetype/noto/NotoSansCJK-Regular.ttc",
            "opentype/noto/NotoSansCJKsc-Regular.otf",
            "truetype/wqy/wqy-zenhei.ttc",
            "truetype/wqy/wqy-microhei.ttc",
            "wqy-zenhei/wqy-zenhei.ttc",
        ];
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
    }
}

fn read(path: &Path) -> Option<Arc<SystemFace>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let index = face_with(&bytes, HAN_PROBE)?;
    log::info!("installed Chinese fallback font: {} (face {index})", path.display());
    Some(Arc::new(SystemFace { bytes: Arc::new(bytes), index }))
}

/// The first face of the file that parses and maps `c`. hayro parses fonts with the same skrifa, so
/// a face accepted here is one it can load.
fn face_with(bytes: &[u8], c: char) -> Option<u32> {
    (0..MAX_FACES).find(|&index| FontRef::from_index(bytes, index).is_ok_and(|font| font.charmap().map(c).is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_and_missing_files_are_skipped() {
        assert_eq!(face_with(b"", HAN_PROBE), None);
        assert_eq!(face_with(b"not a font at all", HAN_PROBE), None);
        assert_eq!(face_with(&[0u8; 4096], HAN_PROBE), None);
        assert!(read(Path::new("definitely/not/here.ttf")).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir()).is_none());
    }

    #[test]
    fn a_face_without_the_probe_is_rejected() {
        // Inter is Latin, Greek and Cyrillic only: it has no hanzi.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, HAN_PROBE), None);
        assert_eq!(face_with(inter, 'A'), Some(0));
    }

    #[test]
    fn candidates_are_absolute_font_files() {
        let list = candidates();
        assert!(!list.is_empty());
        assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc" || e == "otf")));
    }
}
