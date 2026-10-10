//! The last-resort interface font: one face already installed on this machine.
//!
//! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; this one
//! only draws characters none of them has, such as an Arabic file name in a build without
//! craft-fonts, or the Simplified Chinese interface in a build without a `Hans` face (#826). It is
//! read at runtime and never embedded or shipped (AGENTS.md §1.4), and `PDFCRAFT_SYSTEM_FONTS=0`
//! turns it off (published screenshots do).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Chinese system faces are big collections (PingFang, STHeiti), so they get a higher cap.
const MAX_HAN_BYTES: u64 = 96 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// Arabic letter alef: the face must have it to be worth loading.
const PROBE: char = '\u{0627}';
/// 欢 (U+6B22): Simplified only, absent from the Japanese craft-fonts faces.
const HANS_PROBE: char = '\u{6B22}';
/// 說 (U+8AAA): Traditional only, absent from the Japanese craft-fonts faces.
const HANT_PROBE: char = '\u{8AAA}';

/// What the installed face is wanted for, from the interface language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Need {
    /// Scripts no embedded face has (Arabic in a build without craft-fonts).
    #[default]
    Other,
    /// Simplified Chinese interface text.
    Hans,
    /// Traditional Chinese interface text.
    Hant,
}

/// The installed fallback face for `need`, read once per need. `None` when it is turned off or no
/// candidate fits.
pub fn fallback(need: Need) -> Option<Arc<FontData>> {
    static OTHER: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    static HANS: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    static HANT: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    let cache = match need {
        Need::Other => &OTHER,
        Need::Hans => &HANS,
        Need::Hant => &HANT,
    };
    cache.get_or_init(|| load(need)).clone()
}

fn load(need: Need) -> Option<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    let (probe, max) = match need {
        Need::Other => (PROBE, MAX_BYTES),
        Need::Hans => (HANS_PROBE, MAX_HAN_BYTES),
        Need::Hant => (HANT_PROBE, MAX_HAN_BYTES),
    };
    candidates(need).iter().find_map(|path| read(path, probe, max))
}

/// Well-known locations of faces with broad script coverage, best first.
fn candidates(need: Need) -> Vec<PathBuf> {
    match need {
        Need::Other => other_candidates(),
        Need::Hans | Need::Hant => han_candidates(need == Need::Hant),
    }
}

fn other_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["segoeui.ttf", "tahoma.ttf", "arial.ttf"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        ["/System/Library/Fonts/SFArabic.ttf", "/System/Library/Fonts/GeezaPro.ttc", "/System/Library/Fonts/Supplemental/Arial.ttf"]
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        unix_fonts(&[
            "truetype/noto/NotoSansArabic-Regular.ttf",
            "noto/NotoSansArabic-Regular.ttf",
            "google-noto/NotoSansArabic-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
            "dejavu/DejaVuSans.ttf",
            "dejavu-sans-fonts/DejaVuSans.ttf",
        ])
    }
}

/// Chinese sans faces the operating system ships, the UI language's own region first. Noto CJK
/// and Source Han are left out on purpose (AGENTS.md §1.1).
fn han_candidates(traditional: bool) -> Vec<PathBuf> {
    if cfg!(windows) {
        // Microsoft YaHei / JhengHei, then the older SimSun / MingLiU collections.
        let files: &[&str] =
            if traditional { &["msjh.ttc", "msyh.ttc", "mingliu.ttc", "simsun.ttc"] } else { &["msyh.ttc", "msjh.ttc", "simsun.ttc", "mingliu.ttc"] };
        files.iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        let files: &[&str] = if traditional {
            &["PingFang.ttc", "STHeiti Light.ttc", "STHeiti Medium.ttc", "Hiragino Sans GB.ttc"]
        } else {
            &["PingFang.ttc", "Hiragino Sans GB.ttc", "STHeiti Light.ttc", "STHeiti Medium.ttc"]
        };
        files.iter().map(|f| Path::new("/System/Library/Fonts").join(f)).collect()
    } else {
        unix_fonts(&[
            "truetype/wqy/wqy-microhei.ttc",
            "wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "wqy-microhei/wqy-microhei.ttc",
            "truetype/wqy/wqy-zenhei.ttc",
            "wenquanyi/wqy-zenhei/wqy-zenhei.ttc",
            "wqy-zenhei/wqy-zenhei.ttc",
            "truetype/droid/DroidSansFallbackFull.ttf",
            "google-droid-sans-fonts/DroidSansFallbackFull.ttf",
            "droid/DroidSansFallbackFull.ttf",
            "truetype/arphic/uming.ttc",
            "arphic-uming/uming.ttc",
        ])
    }
}

fn windows_fonts() -> PathBuf {
    let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    dir.join("Fonts")
}

fn unix_fonts(files: &'static [&'static str]) -> Vec<PathBuf> {
    ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
}

fn read(path: &Path, probe: char, max: u64) -> Option<Arc<FontData>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let index = face_with(&bytes, probe)?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    log::info!("interface font fallback: {} (face {index})", path.display());
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
        assert!(read(Path::new("definitely/not/here.ttf"), PROBE, MAX_BYTES).is_none());
        // A directory is not a font.
        assert!(read(&std::env::temp_dir(), PROBE, MAX_BYTES).is_none());
    }

    #[test]
    fn a_face_without_the_probe_is_rejected() {
        // Inter is Latin, Greek and Cyrillic only.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, PROBE), None);
        assert_eq!(face_with(inter, HANS_PROBE), None);
        assert_eq!(face_with(inter, HANT_PROBE), None);
        assert_eq!(face_with(inter, 'A'), Some(0));
    }

    #[test]
    fn a_file_over_the_cap_is_not_read() {
        let inter = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Inter-Regular.ttf"));
        assert!(read(inter, 'A', MAX_BYTES).is_some());
        assert!(read(inter, 'A', 1024).is_none());
    }

    #[test]
    fn candidates_are_absolute_font_files() {
        for need in [Need::Other, Need::Hans, Need::Hant] {
            let list = candidates(need);
            assert!(!list.is_empty(), "{need:?}");
            assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")), "{need:?}");
        }
    }

    #[test]
    fn han_candidates_skip_adobe_designs() {
        for need in [Need::Hans, Need::Hant] {
            for path in candidates(need) {
                let name = path.to_string_lossy().to_lowercase();
                assert!(!["notosanscjk", "notoserifcjk", "sourcehan", "source-han"].iter().any(|b| name.contains(b)), "{name}");
            }
        }
    }
}
