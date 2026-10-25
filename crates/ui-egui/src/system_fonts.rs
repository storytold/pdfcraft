//! The last-resort interface font: one face already installed on this machine.
//!
//! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; this one
//! only draws characters none of them has, such as an Arabic file name in a build without
//! craft-fonts, or the Simplified Chinese interface in a build without a `Hans` face (#826). A
//! few more installed faces, [`devanagari_fallback`], [`telugu_fallback`], [`sinhala_fallback`],
//! [`tamil_fallback`], [`hebrew_fallback`] and [`hangul_fallback`], draw Hindi, Telugu, Sinhala,
//! Tamil, Hebrew and Korean when one is installed. It is read at runtime and never embedded or
//! shipped (AGENTS.md §1.4), and `PDFCRAFT_SYSTEM_FONTS=0` turns it off (published screenshots do).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::FontData;

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
mod fontconfig;

/// Larger files are not read: a font path is still untrusted input.
const MAX_BYTES: u64 = 32 << 20;
/// Chinese system faces are big collections (PingFang, STHeiti), so they get a higher cap.
const MAX_HAN_BYTES: u64 = 96 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
/// Arabic letter alef: the face must have it to be worth loading for Arabic script.
const PROBE: char = '\u{0627}';
/// Devanagari letter ka: the face must have it to cover Hindi.
const DEVANAGARI_PROBE: char = '\u{0915}';
/// Telugu letter ka: the face must have it to cover Telugu.
const TELUGU_PROBE: char = '\u{0C15}';
/// Sinhala letter ka (ක): the face must have it to cover Sinhala.
const SINHALA_PROBE: char = '\u{0D9A}';
/// Tamil letter ka (க): the face must have it to cover Tamil.
const TAMIL_PROBE: char = '\u{0B95}';
/// Hebrew letter alef: the face must have it to cover Hebrew.
const HEBREW_PROBE: char = '\u{05D0}';
/// Hangul syllable hih (힣, U+D7A3), the last of the 11,172 modern syllables: a face with it has
/// the whole block. Droid Sans Fallback, for one, has 가 but few other syllables.
const HANGUL_PROBE: char = '\u{D7A3}';
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

/// The installed fallback face for Devanagari (Hindi), read once. `None` when it is turned off
/// or no candidate is installed; the interface then draws what the embedded faces can.
pub fn devanagari_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| script_fallback(&devanagari_candidates(), DEVANAGARI_PROBE)).clone()
}

/// The installed fallback face for Telugu, read once. `None` when it is turned off or no
/// candidate is installed; the interface then draws what the embedded faces can.
pub fn telugu_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| script_fallback(&telugu_candidates(), TELUGU_PROBE)).clone()
}

/// The installed fallback face for Sinhala, read once. `None` when it is turned off or no
/// candidate is installed; the interface then draws what the embedded faces can.
pub fn sinhala_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| script_fallback(&sinhala_candidates(), SINHALA_PROBE)).clone()
}

/// The installed fallback face for Tamil, read once. `None` when it is turned off or no
/// candidate is installed; the interface then draws what the embedded faces can.
pub fn tamil_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| script_fallback(&tamil_candidates(), TAMIL_PROBE)).clone()
}

/// The installed fallback face for Hebrew, read once. `None` when it is turned off or no
/// candidate is installed; the interface then draws what the embedded faces can.
pub fn hebrew_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| script_fallback(&hebrew_candidates(), HEBREW_PROBE)).clone()
}

/// The installed fallback face for Hangul (Korean), read once. `None` when it is turned off or no
/// candidate is installed; the interface then draws what the embedded faces can.
pub fn hangul_fallback() -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    CACHE.get_or_init(|| script_fallback(&hangul_candidates(), HANGUL_PROBE)).clone()
}

fn script_fallback(candidates: &[PathBuf], probe: char) -> Option<Arc<FontData>> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    candidates.iter().find_map(|path| read(path, probe, MAX_BYTES))
}

/// macOS folders that hold Apple's own families, system folder first. On macOS 26.6 (Apple M5
/// Max) `Kohinoor.ttc` is in `/System/Library/Fonts` while the Sangam MN faces are under
/// `Supplemental/`, so looking one family up in both folders stops a move by Apple from silently
/// dropping it — which is what happened to `Supplemental/Kohinoor.ttc`, listed by the code but
/// not present on that OS. `/Library/Fonts` is deliberately not listed here: it holds
/// compatibility symlinks into `Supplemental/`, and the one face wanted from it is named in full
/// below. On that machine each family below lives in exactly one of the two folders, so their
/// order is a deterministic choice rather than a tie-break that was ever exercised.
///
/// Only [`macos_devanagari_candidates`] uses this today: in the Telugu, Sinhala and Tamil lists each
/// family is named once, in a single folder (some of those lists also name `/Library/Fonts` for the
/// Unicode face). Their first choices all exist on macOS 26.6, so nothing is dropping there now — the
/// same two-folder expansion would fit them, and is left for a change that tests it.
const MACOS_FONT_DIRS: [&str; 2] = ["/System/Library/Fonts", "/System/Library/Fonts/Supplemental"];

/// The Devanagari families this list tries, best first. Spelled as the files are on disk —
/// `Devanagari Sangam MN.ttc` has spaces, like the other Sangam MN families below. Apple ships at
/// least one more Devanagari-capable collection on macOS 26.6 (`Supplemental/ITFDevanagari.ttc`,
/// 10 faces), and this list does not try it.
const MACOS_DEVANAGARI_FAMILIES: &[&str] = &["DevanagariMT.ttc", "Devanagari Sangam MN.ttc", "Kohinoor.ttc"];

/// `families`, best first, each looked for in every folder of `dirs`. Split out from
/// [`macos_candidates`] so the macOS layout can be tested on a machine with no Apple fonts.
fn macos_families_in(dirs: &[&str], families: &[&str]) -> Vec<PathBuf> {
    families.iter().flat_map(|family| dirs.iter().map(move |dir| Path::new(dir).join(family))).collect()
}

/// [`macos_families_in`] against the folders Apple actually uses.
fn macos_candidates(families: &[&str]) -> Vec<PathBuf> {
    macos_families_in(&MACOS_FONT_DIRS, families)
}

/// The macOS Devanagari list: Apple's installed families, then the last-resort Unicode face.
/// `Arial Unicode.ttf` is listed from `/Library/Fonts`, where it is a symlink into
/// `Supplemental/` on current macOS, and is read only when no family above covers the script.
fn macos_devanagari_candidates() -> Vec<PathBuf> {
    let mut list = macos_candidates(MACOS_DEVANAGARI_FAMILIES);
    list.push(PathBuf::from("/Library/Fonts/Arial Unicode.ttf"));
    list
}

/// Well-known locations of Devanagari (Hindi) faces, best first.
fn devanagari_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["Nirmala.ttf", "NirmalaB.ttf", "mangal.ttf", "aparaj.ttf", "utsah.ttf"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        macos_devanagari_candidates()
    } else {
        unix_fonts(&[
            "google-noto/NotoSansDevanagariUI-Regular.ttf",
            "google-noto/NotoSansDevanagari-Regular.ttf",
            "google-noto-vf/NotoSansDevanagari[wght].ttf",
            "truetype/noto/NotoSansDevanagari-Regular.ttf",
            "truetype/noto/NotoSansDevanagariUI-Regular.ttf",
            "noto/NotoSansDevanagari-Regular.ttf",
            "noto/NotoSansDevanagariUI-Regular.ttf",
            "lohit-devanagari-fonts/Lohit-Devanagari.ttf",
            "truetype/lohit-devanagari/Lohit-Devanagari.ttf",
            "truetype/deva/Lohit-Devanagari.ttf",
            "lohit-hindi/Lohit-Hindi.ttf",
            "google-droid-sans-fonts/DroidSansDevanagari-Regular.ttf",
            "samyak-fonts/Samyak-Devanagari.ttf",
            "gargi/Gargi.ttf",
        ])
    }
}

/// Well-known locations of Telugu faces, best first.
fn telugu_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["Nirmala.ttc", "Nirmala.ttf", "gautami.ttf"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        [
            "/System/Library/Fonts/KohinoorTelugu.ttc",
            "/System/Library/Fonts/Supplemental/Telugu Sangam MN.ttc",
            "/System/Library/Fonts/Supplemental/Telugu MN.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else {
        unix_fonts(&[
            "google-noto/NotoSansTeluguUI-Regular.ttf",
            "google-noto/NotoSansTelugu-Regular.ttf",
            "google-noto-vf/NotoSansTelugu[wght].ttf",
            "google-noto-vf/NotoSansTelugu-UI[wght].ttf",
            "truetype/noto/NotoSansTelugu-Regular.ttf",
            "truetype/noto/NotoSansTeluguUI-Regular.ttf",
            "noto/NotoSansTelugu-Regular.ttf",
            "noto/NotoSansTeluguUI-Regular.ttf",
            "lohit-telugu-fonts/Lohit-Telugu.ttf",
            "truetype/lohit-telugu/Lohit-Telugu.ttf",
            "truetype/fonts-telu-extra/Pothana2000.ttf",
        ])
    }
}

/// Well-known locations of Sinhala faces, best first. Nirmala UI has Sinhala from Windows 10;
/// Iskoola Pota is the older Windows Sinhala face.
fn sinhala_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["Nirmala.ttc", "Nirmala.ttf", "iskpota.ttf"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        ["/System/Library/Fonts/Supplemental/Sinhala Sangam MN.ttc", "/System/Library/Fonts/Supplemental/Sinhala MN.ttc"]
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        unix_fonts(&[
            "google-noto/NotoSansSinhalaUI-Regular.ttf",
            "google-noto/NotoSansSinhala-Regular.ttf",
            "google-noto-vf/NotoSansSinhala[wght].ttf",
            "truetype/noto/NotoSansSinhala-Regular.ttf",
            "truetype/noto/NotoSansSinhalaUI-Regular.ttf",
            "noto/NotoSansSinhala-Regular.ttf",
            "noto/NotoSansSinhalaUI-Regular.ttf",
        ])
    }
}

/// Well-known locations of Tamil faces, best first. Latha is the older Windows Tamil face.
fn tamil_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["Nirmala.ttc", "Nirmala.ttf", "latha.ttf"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        [
            "/System/Library/Fonts/Supplemental/Tamil Sangam MN.ttc",
            "/System/Library/Fonts/Supplemental/Tamil MN.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else {
        unix_fonts(&[
            "google-noto/NotoSansTamilUI-Regular.ttf",
            "google-noto/NotoSansTamil-Regular.ttf",
            "google-noto-vf/NotoSansTamil[wght].ttf",
            "truetype/noto/NotoSansTamil-Regular.ttf",
            "truetype/noto/NotoSansTamilUI-Regular.ttf",
            "noto/NotoSansTamil-Regular.ttf",
            "noto/NotoSansTamilUI-Regular.ttf",
            "lohit-tamil-fonts/Lohit-Tamil.ttf",
            "truetype/lohit-tamil/Lohit-Tamil.ttf",
        ])
    }
}

/// Well-known locations of Hebrew faces, best first. The general fallback does not cover it:
/// SF Arabic, Geeza Pro and Noto Sans Arabic have no Hebrew.
fn hebrew_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["segoeui.ttf", "arial.ttf", "tahoma.ttf", "david.ttf"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        ["/System/Library/Fonts/SFHebrew.ttf", "/System/Library/Fonts/ArialHB.ttc", "/Library/Fonts/Arial Unicode.ttf"]
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        unix_fonts(&[
            "google-noto/NotoSansHebrew-Regular.ttf",
            "google-noto-vf/NotoSansHebrew[wght].ttf",
            "truetype/noto/NotoSansHebrew-Regular.ttf",
            "noto/NotoSansHebrew-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
            "dejavu/DejaVuSans.ttf",
            "dejavu-sans-fonts/DejaVuSans.ttf",
        ])
    }
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

/// Chinese sans faces the operating system ships, the UI language's own region first.
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

/// Well-known locations of Korean faces, best first.
fn hangul_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["malgun.ttf", "gulim.ttc", "batang.ttc"].iter().map(|f| windows_fonts().join(f)).collect()
    } else if cfg!(target_os = "macos") {
        ["/System/Library/Fonts/AppleSDGothicNeo.ttc", "/System/Library/Fonts/Supplemental/AppleGothic.ttf", "/Library/Fonts/Arial Unicode.ttf"]
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        unix_fonts(&[
            "truetype/nanum/NanumGothic.ttf",
            "nanum/NanumGothic.ttf",
            "naver-nanum/NanumGothic.ttf",
            "nhn-nanum/NanumGothic.ttf",
            "truetype/unfonts-core/UnDotum.ttf",
            "un-core/UnDotum.ttf",
            "truetype/baekmuk/dotum.ttf",
            "baekmuk-ttf-dotum-fonts/dotum.ttf",
        ])
    }
}

fn windows_fonts() -> PathBuf {
    let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    dir.join("Fonts")
}

fn unix_fonts(files: &'static [&'static str]) -> Vec<PathBuf> {
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    static INSTALLED: OnceLock<Vec<PathBuf>> = OnceLock::new();
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    // Read once per process, and not at all when installed fonts are turned off.
    let installed = INSTALLED
        .get_or_init(|| {
            if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
                return Vec::new();
            }
            fontconfig::font_files(&fontconfig::config_path())
        })
        .as_slice();
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_arch = "wasm32"))))]
    let installed: &[PathBuf] = &[];
    unix_candidates(files, installed)
}

fn unix_candidates(files: &[&str], installed: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths: Vec<_> =
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect();
    // Fontconfig may point into /nix/store or a user's font directory. Keep the existing
    // candidate names and their preference order; this does not select arbitrary fonts.
    for file in files {
        let name = Path::new(file).file_name();
        for path in installed.iter().filter(|p| p.file_name() == name) {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
    }
    paths
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
    Some(Arc::new(crate::theme::on_interface_baseline(data)))
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
    fn configured_candidates_preserve_names_order_and_existing_paths() {
        let names = ["noto/NotoSansArabic-Regular.ttf", "truetype/dejavu/DejaVuSans.ttf"];
        let noto = PathBuf::from("/custom/fonts/NotoSansArabic-Regular.ttf");
        let dejavu = PathBuf::from("/nix/store/font/share/fonts/truetype/DejaVuSans.ttf");
        let installed = [dejavu.clone(), PathBuf::from("/custom/fonts/SourceHanSans.ttf"), noto.clone(), dejavu.clone()];
        let mut expected = unix_candidates(&names, &[]);
        expected.extend([noto, dejavu]);
        assert_eq!(unix_candidates(&names, &installed), expected);
    }

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

    /// `script_fallback` honours the documented installed-font switch, under which it returns
    /// nothing before order is ever consulted — so both loader tests and the macOS oracle skip when
    /// it is off, the way `tests/it/fonts.rs` skips for the same reason. Without this the suite goes red in exactly the
    /// mode `cargo xtask screenshots` uses.
    fn installed_faces_switched_off() -> bool {
        std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0")
    }

    /// Check that the first readable fallback candidate wins, so reversing the preference list reverses the
    /// chosen face.
    #[test]
    fn the_first_readable_candidate_wins_not_the_last() {
        // List order *is* the preference, so a loader that scanned back-to-front would pick a
        // different face and nobody would notice. Two bundled faces both cover 'A' and differ in
        // size, so the winner is identifiable by length alone.
        if installed_faces_switched_off() {
            eprintln!("skipping: PDFCRAFT_SYSTEM_FONTS=0 turns the loader off before order matters");
            return;
        }
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts"));
        let (inter, mono) = (dir.join("Inter-Regular.ttf"), dir.join("JetBrainsMono-Regular.ttf"));
        let size = |p: &Path| std::fs::metadata(p).expect("bundled face").len();
        assert_ne!(size(&inter), size(&mono), "the two fixtures must be tellable apart");
        let first = script_fallback(&[inter.clone(), mono.clone()], 'A').expect("Inter covers 'A'");
        assert_eq!(first.bytes().len() as u64, size(&inter), "the first readable candidate must win");
        let reversed = script_fallback(&[mono.clone(), inter], 'A').expect("JetBrains Mono covers 'A'");
        assert_eq!(reversed.bytes().len() as u64, size(&mono), "reversing the list must reverse the pick");
    }

    #[test]
    fn a_script_fallback_is_the_first_candidate_with_the_probe() {
        // This one calls `script_fallback` too, so it fails under the documented switch that
        // published screenshots use — skip with a reason, as tests/it/fonts.rs does.
        if installed_faces_switched_off() {
            eprintln!("skipping: PDFCRAFT_SYSTEM_FONTS=0 turns the loader off");
            return;
        }
        let inter = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Inter-Regular.ttf"));
        let candidates = [PathBuf::from("definitely/not/here.ttf"), inter];
        assert!(script_fallback(&candidates, 'A').is_some());
        assert!(script_fallback(&candidates, TELUGU_PROBE).is_none());
        assert!(script_fallback(&candidates, DEVANAGARI_PROBE).is_none());
        assert!(script_fallback(&candidates, HANGUL_PROBE).is_none());
        assert!(script_fallback(&candidates, SINHALA_PROBE).is_none());
        assert!(script_fallback(&candidates, TAMIL_PROBE).is_none());
        assert!(script_fallback(&candidates, HEBREW_PROBE).is_none());
    }

    #[test]
    fn script_candidates_are_absolute_font_files() {
        for list in [devanagari_candidates(), telugu_candidates(), sinhala_candidates(), tamil_candidates(), hebrew_candidates(), hangul_candidates()]
        {
            assert!(!list.is_empty());
            assert!(list.iter().all(|p| p.is_absolute() || cfg!(windows)), "{list:?}");
            assert!(list.iter().all(|p| p.extension().is_some_and(|e| e == "ttf" || e == "ttc")), "{list:?}");
        }
    }

    #[test]
    fn hangul_candidates_are_font_files() {
        let list = hangul_candidates();
        assert!(!list.is_empty());
        for path in list {
            assert!(path.extension().is_some_and(|e| e == "ttf" || e == "ttc"), "{path:?}");
        }
        // Inter has no Hangul.
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        assert_eq!(face_with(inter, HANGUL_PROBE), None);
    }

    /// Retune tripwires, deliberately separate from `the_macos_devanagari_list_is_apples_layout` below: these assert that three
    /// constants still hold the values this module's contract depends on. They cannot fail on their
    /// own — they exist so that editing one is a decision someone has to make twice. Each was
    /// checked by deleting the pin and watching the matching regression go green again: raising
    /// `MAX_BYTES` above Arial Unicode's ~23 MB, swapping the Devanagari probe for Arabic `U+0627`
    /// (which that face also covers, so every other test stays green), and cutting `MAX_FACES` to 1.
    /// The oracle in the macOS test below also depends on all three, and it is conditional on
    /// installation — so on CI these are the only guard.
    #[test]
    fn the_load_constants_are_the_documented_contract() {
        assert_eq!(MAX_BYTES, 32 << 20, "the non-Han installed-face size cap is this module's contract (Arial Unicode is ~23 MB)");
        assert_eq!(DEVANAGARI_PROBE, '\u{0915}', "Devanagari ka is what the Hindi candidates are probed for, not another script's letter");
        assert_eq!(MAX_FACES, 16, "how deep into a .ttc collection the loader looks; the macOS oracle trusts this depth");
    }

    /// The macOS Devanagari list, exactly: families best first, each tried in the system folder
    /// before `Supplemental/`. This pins the spellings Apple uses on disk and the order that
    /// decides which face wins. A missing space (`DevanagariSangamMN.ttc`), swapping the two
    /// folders, or walking folder-major instead of family-major all fail here — and they fail on
    /// every OS, because this test reads no filesystem.
    #[test]
    fn the_macos_devanagari_list_is_apples_layout() {
        assert_eq!(
            macos_devanagari_candidates(),
            vec![
                PathBuf::from("/System/Library/Fonts/DevanagariMT.ttc"),
                PathBuf::from("/System/Library/Fonts/Supplemental/DevanagariMT.ttc"),
                PathBuf::from("/System/Library/Fonts/Devanagari Sangam MN.ttc"),
                PathBuf::from("/System/Library/Fonts/Supplemental/Devanagari Sangam MN.ttc"),
                PathBuf::from("/System/Library/Fonts/Kohinoor.ttc"),
                PathBuf::from("/System/Library/Fonts/Supplemental/Kohinoor.ttc"),
                PathBuf::from("/Library/Fonts/Arial Unicode.ttf"),
            ]
        );
        // Pin what the application actually consults, not just the helper that builds it.
        #[cfg(target_os = "macos")]
        assert_eq!(devanagari_candidates(), macos_devanagari_candidates(), "the app must consult the list this test pins");
    }

    #[test]
    fn a_family_whose_name_has_spaces_is_still_found() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let root = std::env::temp_dir().join(format!("pdfcraft-macos-fonts-{}-{nanos}", std::process::id()));
        let supplemental = root.join("Supplemental");
        std::fs::create_dir_all(&supplemental).expect("build a temp macOS-shaped font tree");
        let dirs = [root.to_string_lossy().to_string(), supplemental.to_string_lossy().to_string()];
        let names: Vec<&str> = dirs.iter().map(|d| d.as_str()).collect();
        let inter: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let spaced = "Devanagari Sangam MN.ttc";
        std::fs::write(supplemental.join(spaced), inter).expect("place a face at the spaced path");
        let found = macos_families_in(&names, &[spaced]).iter().any(|p| read(p, 'A', MAX_BYTES).is_some());
        let _ = std::fs::remove_dir_all(&root);
        assert!(found, "a family named with spaces did not resolve under {dirs:?}");
    }

    /// On a Mac, where Apple's fonts are installed: every family folder that really exists is
    /// reachable from the candidate list, and something in the list covers Devanagari. A family
    /// Apple no longer ships is reported rather than failed — this list exists precisely for the case
    /// where one of them is missing, with one exception: if *nothing* here covers Devanagari the test
    /// fails loudly, because a machine with no face to check cannot validate the list at all. That
    /// leniency is not a guess: Apple's "Fonts included with macOS Tahoe" page (support.apple.com/122869)
    /// titles its list "Installed or downloadable fonts" and says most install automatically while
    /// others download on demand via Font Book — without marking which is which, so no entry in these
    /// lists can be assumed present on a given Mac. CI has no macOS runner (`.github/workflows/ci.yml`
    /// keeps the org's Mac slots for release builds), so this runs on contributors' and
    /// maintainers' machines.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_installed_devanagari_face_is_reachable_on_this_mac() {
        if installed_faces_switched_off() {
            eprintln!("skipping on this machine: PDFCRAFT_SYSTEM_FONTS=0 turns installed faces off");
            return;
        }
        let list = devanagari_candidates();
        for family in MACOS_DEVANAGARI_FAMILIES {
            // Same rule as the last resort below: an entry that exists counts, even a link whose
            // target has gone — "present but unresolved" is precisely the dead-path defect this change
            // is about, and filtering on `is_file()` would quietly reclassify it as "not installed".
            // Absence is tolerated; a broken entry is not.
            let present: Vec<PathBuf> =
                MACOS_FONT_DIRS.iter().map(|dir| Path::new(dir).join(family)).filter(|p| std::fs::symlink_metadata(p).is_ok()).collect();
            if present.is_empty() {
                eprintln!("{family}: not installed on this machine, so the list offers nothing that exists — fine, and reported only");
                continue;
            }
            for path in &present {
                assert!(
                    path.is_file(),
                    "{family}: {path:?} exists but does not resolve — its target was moved or removed, so this candidate path has gone dead"
                );
                assert!(list.contains(path), "{path:?} is installed here but the candidate list skips it: {list:?}");
            }
        }
        assert!(script_fallback(&devanagari_candidates(), DEVANAGARI_PROBE).is_some(), "no installed face on this Mac covers Devanagari");
        // And the function callers use, not just its inputs: asserting *some* face loads is not
        // enough, because both lists end at the same last-resort Unicode face and would rescue a
        // mix-up. The app-facing function must land on the same bytes, and the same face within
        // them, as this list produces.
        let direct = script_fallback(&devanagari_candidates(), DEVANAGARI_PROBE).expect("the list above resolves");
        let cached = devanagari_fallback().expect("devanagari_fallback() resolves where these faces are installed");
        assert_eq!(cached.bytes().len(), direct.bytes().len(), "devanagari_fallback() must load the face this list points at, not another script's");
        assert_eq!(cached.index, direct.index, "and the same face within that file");
        // The advertised last resort is the biggest file on the list and reaches its target through
        // a compatibility symlink in `/Library/Fonts`, a folder installers and users may modify — so
        // it is optional in the same sense the families above are, and a Mac without it must not go
        // red for an unrelated reason. But `is_file()` follows symlinks: a link whose target has
        // moved reads as "no file", which is exactly the dead-path defect this change exists to
        // catch. So gate on the entry existing (lstat), then require that it resolves, then check
        // size cap and coverage. `MAX_BYTES` is what the loader applies to non-Han scripts; the path
        // comes from the list itself rather than being written out again, so this oracle cannot
        // drift from what production offers.
        let last_resort = list.last().expect("the macOS Devanagari list always ends at the Unicode face").clone();
        assert!(
            last_resort.to_string_lossy().ends_with("Arial Unicode.ttf"),
            "this block checks the Unicode last resort, so the list must still end at it: {last_resort:?}"
        );
        if std::fs::symlink_metadata(&last_resort).is_ok() {
            assert!(
                last_resort.is_file(),
                "{last_resort:?} exists but does not resolve — its target was moved or removed, so this path has gone dead"
            );
            assert!(
                read(&last_resort, DEVANAGARI_PROBE, MAX_BYTES).is_some(),
                "the last-resort Unicode face is installed here but did not resolve and load under MAX_BYTES"
            );
        } else {
            eprintln!("{last_resort:?} is not installed on this machine — optional to the loader, so nothing to check");
        }
    }
}
