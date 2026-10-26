//! Installed CJK faces for CID fonts a PDF doesn't embed.
//!
//! Acrobat and other tools often write Chinese, Japanese or Korean text with one of the Adobe
//! collections (Adobe-Japan1, GB1, CNS1, Korea1) and a font name such as `KozGoPr6N-Medium` or
//! `MSung-Light` without embedding the font. hayro maps those CIDs to Unicode itself, but it needs
//! a face to draw them with; without one the text is blank. The craft-fonts Japanese faces come
//! first (see `japanese_fallback` in `raster.rs`); this module is the last resort: one face
//! already installed on this machine, used for the viewer only (`RenderConfig::system_fonts`, off
//! for exports, printing and OCR). It is read at runtime and never embedded or shipped
//! (AGENTS.md §1.4), no font an Adobe product installs is a candidate (§1.1), and
//! `PDFCRAFT_SYSTEM_FONTS=0` turns it off, as for the interface font.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use hayro::hayro_interpret::font::{FallbackFontQuery, FontData};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use skrifa::MetadataProvider as _;
use skrifa::string::StringId;

/// Larger files are not read: a font path is still untrusted input. CJK system faces are big
/// collections (PingFang is tens of megabytes).
const MAX_BYTES: u64 = 96 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 32;
/// Downloaded font assets looked at per kind on macOS.
const MAX_ASSETS: usize = 512;

/// The writing system a character collection is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Script {
    Japanese,
    SimplifiedChinese,
    TraditionalChinese,
    Korean,
}

impl Script {
    fn of(family: &CidFamily) -> Option<Self> {
        match family {
            CidFamily::AdobeJapan1 => Some(Self::Japanese),
            CidFamily::AdobeGB1 => Some(Self::SimplifiedChinese),
            CidFamily::AdobeCNS1 => Some(Self::TraditionalChinese),
            CidFamily::AdobeKorea1 => Some(Self::Korean),
            CidFamily::AdobeIdentity | CidFamily::Custom { .. } => None,
        }
    }

    /// A character the face must have to be worth using. Each is common and not shared by the
    /// other scripts' faces: 物 (Japanese; also Chinese, so it only rules out Latin-only faces),
    /// 欢 (Simplified only), 說 (Traditional only), 한 (Hangul).
    fn probe(self) -> char {
        match self {
            Self::Japanese => '\u{7269}',
            Self::SimplifiedChinese => '\u{6B22}',
            Self::TraditionalChinese => '\u{8AAA}',
            Self::Korean => '\u{D55C}',
        }
    }
}

/// One installed file and the words (in a face's family name) that pick the face for the right
/// region out of a collection that holds several (PingFang SC, TC and HK share one file).
struct Candidate {
    path: PathBuf,
    region: &'static [&'static str],
}

/// The installed face to draw a CID font of `query`'s collection with, as `(data, face index)`.
/// `None` when it is turned off, the collection isn't one of the four, or nothing fits.
pub(crate) fn fallback(query: &FallbackFontQuery) -> Option<(FontData, u32)> {
    if cfg!(target_arch = "wasm32") || std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    let script = Script::of(&query.character_collection.as_ref()?.family)?;
    let bold = query.is_bold || query.font_weight >= 600;
    /// A face file and the index of the chosen face in it, or `None` when nothing fits.
    type Face = Option<(Arc<Vec<u8>>, u32)>;
    static CACHE: Mutex<Option<HashMap<(Script, bool), Face>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let found = cache.get_or_insert_with(HashMap::new).entry((script, bold)).or_insert_with(|| {
        candidates(script)
            .iter()
            .find_map(|c| read(c, script.probe(), bold))
            .inspect(|(_, index)| log::info!("CJK font fallback for {script:?}: face {index}"))
    });
    found.as_ref().map(|(bytes, index)| (Arc::clone(bytes) as FontData, *index))
}

fn read(candidate: &Candidate, probe: char, bold: bool) -> Option<(Arc<Vec<u8>>, u32)> {
    let bytes = file_bytes(&candidate.path)?;
    let index = pick_face(&bytes, probe, candidate.region, bold)?;
    Some((bytes, index))
}

/// The bytes of an installed font file, read once for the whole process and shared: a collection
/// such as `PingFang.ttc` is tens of megabytes and is the candidate of several scripts and both
/// weights, so the per-script cache keeps only a handle and a face index. A file that cannot be
/// used is remembered too, so it is not tried again for every script.
fn file_bytes(path: &Path) -> Option<Arc<Vec<u8>>> {
    /// The bytes of each file tried, or `None` for one that cannot be used.
    type Files = HashMap<PathBuf, Option<Arc<Vec<u8>>>>;
    static FILES: Mutex<Option<Files>> = Mutex::new(None);
    let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
    files.get_or_insert_with(HashMap::new).entry(path.to_path_buf()).or_insert_with(|| read_file(path, MAX_BYTES).map(Arc::new)).clone()
}

/// A regular file of at most `max` bytes, or `None`: a font path is untrusted input.
fn read_file(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    std::fs::read(path).ok()
}

/// The best face of the file that parses and maps `probe`: the region's face when the file has
/// one, then the regular (or, for bold text, the semibold or bold) weight, then the first.
fn pick_face(bytes: &[u8], probe: char, region: &[&str], bold: bool) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None;
    for index in 0..MAX_FACES {
        let Ok(font) = skrifa::FontRef::from_index(bytes, index) else { continue };
        if font.charmap().map(probe).is_none() {
            continue;
        }
        let names = |id: StringId| font.localized_strings(id).map(|s| s.chars().collect::<String>()).collect::<Vec<_>>();
        let family = names(StringId::FAMILY_NAME).join(" ");
        let style = names(StringId::SUBFAMILY_NAME).join(" ").to_ascii_lowercase();
        let mut score = 1;
        if region.iter().any(|r| family.split_whitespace().any(|w| w == *r)) {
            score += 4;
        }
        let weighty = style.contains("bold") || style.contains("semibold") || style.contains("heavy");
        let plain = style.is_empty() || style.contains("regular") || style.contains("normal") || style.contains("medium") && !bold;
        if (bold && weighty) || (!bold && plain && !weighty) {
            score += 2;
        }
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

fn candidates(script: Script) -> Vec<Candidate> {
    let region: &'static [&'static str] = match script {
        Script::Japanese => &["JP", "Japanese"],
        Script::SimplifiedChinese => &["SC", "GB"],
        Script::TraditionalChinese => &["TC", "HK", "CNS"],
        Script::Korean => &["KR", "Korean"],
    };
    let files: Vec<PathBuf> = if cfg!(windows) {
        let dir = windows_fonts();
        let names: &[&str] = match script {
            Script::Japanese => &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"],
            Script::SimplifiedChinese => &["msyh.ttc", "simsun.ttc", "msjh.ttc"],
            Script::TraditionalChinese => &["msjh.ttc", "mingliu.ttc", "msyh.ttc"],
            Script::Korean => &["malgun.ttf", "gulim.ttc"],
        };
        names.iter().map(|f| dir.join(f)).collect()
    } else if cfg!(target_os = "macos") {
        let names: &[&str] = match script {
            Script::Japanese => &["ヒラギノ角ゴシック W3.ttc", "Hiragino Sans GB.ttc", "PingFang.ttc"],
            Script::SimplifiedChinese => &["PingFang.ttc", "Hiragino Sans GB.ttc", "STHeiti Light.ttc"],
            Script::TraditionalChinese => &["PingFang.ttc", "STHeiti Light.ttc", "Hiragino Sans GB.ttc"],
            Script::Korean => &["AppleSDGothicNeo.ttc", "AppleGothic.ttf", "PingFang.ttc"],
        };
        // The system folder first, then the downloaded font assets: on recent macOS a face such as
        // PingFang lives in `/System/Library/AssetsV2/com_apple_MobileAsset_Font*/<id>.asset/AssetData`.
        let mut dirs = vec![PathBuf::from("/System/Library/Fonts")];
        dirs.extend(macos_font_asset_dirs().iter().cloned());
        names.iter().flat_map(|f| dirs.iter().map(move |d| d.join(f))).collect()
    } else {
        let names: &[&str] = match script {
            Script::Korean => {
                &["truetype/nanum/NanumGothic.ttf", "nanum/NanumGothic.ttf", "truetype/baekmuk/gulim.ttf", "truetype/unfonts-core/UnDotum.ttf"]
            }
            Script::Japanese => &[
                "truetype/fonts-japanese-gothic.ttf",
                "ipa-gothic/ipag.ttf",
                "truetype/takao-gothic/TakaoGothic.ttf",
                "truetype/vlgothic/VL-Gothic-Regular.ttf",
            ],
            Script::SimplifiedChinese | Script::TraditionalChinese => &[
                "truetype/wqy/wqy-microhei.ttc",
                "wenquanyi/wqy-microhei/wqy-microhei.ttc",
                "wqy-microhei/wqy-microhei.ttc",
                "truetype/wqy/wqy-zenhei.ttc",
                "wenquanyi/wqy-zenhei/wqy-zenhei.ttc",
                "wqy-zenhei/wqy-zenhei.ttc",
                "truetype/droid/DroidSansFallbackFull.ttf",
                "google-droid-sans-fonts/DroidSansFallbackFull.ttf",
                "truetype/arphic/uming.ttc",
                "arphic-uming/uming.ttc",
            ],
        };
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| names.iter().map(move |f| Path::new(dir).join(f))).collect()
    };
    files.into_iter().map(|path| Candidate { path, region }).collect()
}

/// The `AssetData` folders of the downloaded font assets of macOS, found once. The ids in their
/// names change with each asset version, so they are listed rather than named; at most
/// [`MAX_ASSETS`] are looked at.
fn macos_font_asset_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let root = Path::new("/System/Library/AssetsV2");
        let Ok(kinds) = std::fs::read_dir(root) else { return Vec::new() };
        let mut dirs = Vec::new();
        for kind in kinds.flatten().filter(|k| k.file_name().to_string_lossy().starts_with("com_apple_MobileAsset_Font")) {
            let Ok(assets) = std::fs::read_dir(kind.path()) else { continue };
            for asset in assets.flatten().take(MAX_ASSETS) {
                let data = asset.path().join("AssetData");
                if data.is_dir() {
                    dirs.push(data);
                }
            }
        }
        dirs
    })
}

fn windows_fonts() -> PathBuf {
    let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    dir.join("Fonts")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPTS: [Script; 4] = [Script::Japanese, Script::SimplifiedChinese, Script::TraditionalChinese, Script::Korean];

    #[test]
    fn broken_and_missing_files_are_skipped() {
        assert_eq!(pick_face(b"", '\u{7269}', &[], false), None);
        assert_eq!(pick_face(b"not a font at all", '\u{7269}', &[], false), None);
        assert_eq!(pick_face(&[0u8; 4096], '\u{7269}', &[], false), None);
        let missing = Candidate { path: PathBuf::from("definitely/not/here.ttc"), region: &[] };
        assert!(read(&missing, '\u{7269}', false).is_none());
        // A directory is not a font.
        let dir = Candidate { path: std::env::temp_dir(), region: &[] };
        assert!(read(&dir, '\u{7269}', false).is_none());
    }

    /// The files are bounded by `max`: a larger one is not read.
    #[test]
    fn a_file_over_the_cap_is_not_read() {
        let inter = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Inter-Regular.ttf"));
        assert!(read_file(inter, MAX_BYTES).is_some());
        assert!(read_file(inter, 1024).is_none());
    }

    /// Two faces drawn from one installed file (two scripts, or regular and bold) hold the same
    /// bytes, not a copy each; a file that can't be used is remembered as unusable.
    #[test]
    fn one_installed_file_is_read_once_and_shared() {
        let inter = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Inter-Regular.ttf"));
        let first = file_bytes(inter).expect("readable");
        let second = file_bytes(inter).expect("readable");
        assert!(Arc::ptr_eq(&first, &second), "one allocation for the file");
        let missing = Path::new("definitely/not/here.ttc");
        assert!(file_bytes(missing).is_none() && file_bytes(missing).is_none());
    }

    #[test]
    fn a_latin_face_is_rejected_for_every_script() {
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        for script in SCRIPTS {
            assert_eq!(pick_face(inter, script.probe(), &[], false), None, "{script:?}");
        }
        assert_eq!(pick_face(inter, 'A', &[], false), Some(0));
    }

    #[test]
    fn candidates_are_absolute_font_files_without_adobe_designs() {
        for script in SCRIPTS {
            let list = candidates(script);
            assert!(!list.is_empty(), "{script:?}");
            for c in list {
                assert!(c.path.is_absolute(), "{}", c.path.display());
                assert!(c.path.extension().is_some_and(|e| e == "ttf" || e == "ttc"), "{}", c.path.display());
                let name = c.path.to_string_lossy().to_lowercase();
                assert!(!["adobe", "kozuka", "minion", "myriad"].iter().any(|b| name.contains(b)), "{name}");
            }
        }
    }

    #[test]
    fn only_the_four_adobe_collections_have_a_script() {
        assert_eq!(Script::of(&CidFamily::AdobeCNS1), Some(Script::TraditionalChinese));
        assert_eq!(Script::of(&CidFamily::AdobeGB1), Some(Script::SimplifiedChinese));
        assert_eq!(Script::of(&CidFamily::AdobeKorea1), Some(Script::Korean));
        assert_eq!(Script::of(&CidFamily::AdobeJapan1), Some(Script::Japanese));
        assert_eq!(Script::of(&CidFamily::AdobeIdentity), None);
    }

    /// On a Mac the face for Traditional Chinese is the TC one of PingFang, not the first of the
    /// collection (which is another region's), and it maps a character only Traditional has.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_picks_the_regional_pingfang_face() {
        // Found where this macOS keeps it: the system folder, or the downloaded font assets.
        let path = candidates(Script::TraditionalChinese)
            .into_iter()
            .map(|c| c.path)
            .find(|p| p.file_name().is_some_and(|n| n == "PingFang.ttc") && p.is_file());
        let Some(bytes) = path.and_then(|p| std::fs::read(p).ok()) else {
            eprintln!("PingFang is not installed here: skipped");
            return;
        };
        for (script, region) in [(Script::TraditionalChinese, "TC"), (Script::SimplifiedChinese, "SC")] {
            let index = pick_face(&bytes, script.probe(), &[region], false).expect("PingFang covers it");
            let font = skrifa::FontRef::from_index(&bytes, index).unwrap();
            let family: String = font.localized_strings(StringId::FAMILY_NAME).next().unwrap().chars().collect();
            assert!(family.contains(region), "{script:?} got {family:?}");
        }
    }
    /// A face that this macOS keeps only in its downloaded font assets (PingFang, on recent
    /// versions) is among the candidates. The assets are searched here on their own, not by the
    /// code under test; where there are none, nothing is checked.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_candidates_include_a_pingfang_that_lives_in_the_font_assets() {
        fn find(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
            for entry in std::fs::read_dir(dir).ok()?.flatten() {
                let path = entry.path();
                if path.file_name().is_some_and(|n| n == name) && path.is_file() {
                    return Some(path);
                }
                if depth > 0
                    && path.is_dir()
                    && !path.is_symlink()
                    && let Some(found) = find(&path, name, depth - 1)
                {
                    return Some(found);
                }
            }
            None
        }
        let Some(asset) = find(Path::new("/System/Library/AssetsV2"), "PingFang.ttc", 3) else {
            eprintln!("no PingFang in the font assets here: skipped");
            return;
        };
        for script in [Script::SimplifiedChinese, Script::TraditionalChinese] {
            assert!(candidates(script).iter().any(|c| c.path == asset), "{script:?}: {} is not a candidate", asset.display());
        }
    }
}
