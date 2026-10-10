//! A Han (Chinese) face already installed on this machine, read at runtime.
//!
//! Two things use it: the interface, for Chinese labels and file names, and the page renderer, for
//! a CID font the PDF doesn't embed (Adobe-GB1/Adobe-CNS1) when no embedded face can draw it.
//! AGENTS.md §1.4 allows the desktop app to read such a face: it is never embedded, shipped,
//! committed or copied, and nothing from it enters the document — the renderer draws it on screen
//! only. `PDFCRAFT_SYSTEM_FONTS=0` turns it off (published screenshots set it).
//!
//! There is no embedded Chinese face to prefer instead: the craft-fonts build input's only `Hans`
//! face is Noto CJK, which AGENTS.md §1.1 rules out, so this is the whole of Chinese support on
//! desktop until an allowed face arrives. The web build reads nothing from the machine, so Chinese
//! there still shows the replacement glyph.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use skrifa::instance::{LocationRef, Size};
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
/// Han characters the interface actually draws. All of them must map in the SAME face: a
/// Japanese-only face maps 中 and 文 but not 欢, 编 or 导, and a face that misses one of them would
/// split one Chinese line across faces with different vertical metrics, or draw boxes.
const HAN_PROBES: &str = "中文欢迎编辑导出";

/// An installed face that can draw Chinese, read once. `None` when it is turned off or no
/// candidate fits.
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

/// Chinese faces the platform installs, best first. Candidates are names rather than a search:
/// an absent or oversized file is skipped when it is read, so one list can hold faces that not
/// every machine has. Only system directories: a user's own fonts would need CoreText or
/// fontconfig to find in order of preference, and reading a directory of them could mean reading
/// tens of megabytes just to answer a fallback question.
fn candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        // Microsoft YaHei is the Windows interface Chinese face; JhengHei (msjh) is its
        // Traditional counterpart.
        let dir = windows_font_dir().join("Fonts");
        ["msyh.ttc", "msyhbd.ttc", "simhei.ttf", "simsun.ttc", "msjh.ttc"].iter().map(|f| dir.join(f)).collect()
    } else if cfg!(target_os = "macos") {
        // PingFang is the system Chinese face up to macOS 15; macOS 26 no longer ships it at this
        // path. Hiragino Sans GB is there on both and covers both Chinese catalogs. Songti and
        // STHeiti are over MAX_BYTES today, listed last in case the cap changes.
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
        // Not Noto CJK / Source Han, which AGENTS.md §1.1 rules out by name: these are the
        // community Chinese faces Linux distributions install for it instead.
        let files = [
            "truetype/wqy/wqy-zenhei.ttc",
            "truetype/wqy/wqy-microhei.ttc",
            "wqy-zenhei/wqy-zenhei.ttc",
            "opentype/arphic/uming.ttc",
            "opentype/arphic/ukai.ttc",
        ];
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
    }
}

fn windows_font_dir() -> PathBuf {
    std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

fn read(path: &Path) -> Option<Arc<SystemFace>> {
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
    let index = face_with(&bytes, HAN_PROBES)?;
    // A face the interface cannot be measured against is not worth loading either: it would draw
    // Chinese text out of line, which is the thing this module exists to avoid.
    metrics(&bytes, index)?;
    log::info!("installed Chinese fallback font: {} (face {index})", path.display());
    Some(Arc::new(SystemFace { bytes: Arc::new(bytes), index }))
}

/// The first face of the file that parses and maps every probe. hayro and egui parse fonts with the
/// same skrifa, so a face accepted here is one they can load.
fn face_with(bytes: &[u8], probes: &str) -> Option<u32> {
    (0..MAX_FACES).find(|&index| FontRef::from_index(bytes, index).is_ok_and(|font| probes.chars().all(|c| font.charmap().map(c).is_some())))
}

/// `(ascent, row height)` of one face as fractions of its em, or `None` when the file states no
/// usable metrics.
fn metrics(bytes: &[u8], index: u32) -> Option<(f32, f32)> {
    let font = FontRef::from_index(bytes, index).ok()?;
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    let em = f32::from(m.units_per_em.max(1));
    let row = m.ascent - m.descent + m.leading;
    (m.ascent > 0.0 && row > 0.0 && (m.ascent + row).is_finite()).then(|| (m.ascent / em, row / em))
}

/// The `FontTweak::y_offset_factor` that puts [`han`]'s glyphs on the same baseline as the
/// interface's primary face (the bytes of `primary`), which is what Chinese text shares a line with.
///
/// egui places a glyph `face ascent + (primary row height - face row height) / 2` below the top of
/// its row, so a face whose own row is much taller than the primary's — every Chinese face is —
/// lands above the Latin beside it, and out of the top of the row box. Shifting the glyphs down by
/// the difference of those two expressions cancels it; the shift is visual only, so no layout moves.
/// `0.0` when either face cannot be measured, which is the unfixed behaviour and never a worse one.
pub fn ui_y_offset_factor(primary: &[u8]) -> f32 {
    let (Some(han), Some(primary)) = (han().and_then(|f| metrics(&f.bytes, f.index)), metrics(primary, 0)) else {
        return 0.0;
    };
    (primary.0 - han.0) + 0.5 * (han.1 - primary.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_and_missing_files_are_skipped() {
        assert_eq!(face_with(b"", HAN_PROBES), None);
        assert_eq!(face_with(b"not a font at all", HAN_PROBES), None);
        assert_eq!(face_with(&[0u8; 4096], HAN_PROBES), None);
        assert!(read(Path::new("definitely/not/here.ttf")).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir()).is_none());
    }

    #[test]
    fn a_face_must_map_every_probe() {
        // Inter is Latin, Greek and Cyrillic only: it has no hanzi.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, HAN_PROBES), None);
        assert_eq!(face_with(inter, "A文"), None);
        assert_eq!(face_with(inter, "AB"), Some(0));
    }

    #[test]
    fn oversized_font_files_are_skipped() {
        let path = std::env::temp_dir().join(format!("pdfcraft-oversized-han-{}.ttf", std::process::id()));
        let file = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).expect("a temp file");
        file.set_len(MAX_BYTES + 1).expect("a sparse temp file");
        drop(file);
        let result = read(&path);
        std::fs::remove_file(&path).expect("the temp file is removed");
        assert!(result.is_none());
    }

    #[test]
    fn candidates_are_font_files() {
        let list = candidates();
        assert!(!list.is_empty());
        assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc" || e == "otf")), "{list:?}");
    }
}
