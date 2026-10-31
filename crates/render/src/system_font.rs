//! Installed CJK faces for CID fonts a PDF doesn't embed.
//!
//! Acrobat and other tools often write Chinese, Japanese or Korean text with one of the Adobe
//! collections (Adobe-Japan1, GB1, CNS1, Korea1) and a font name such as `KozGoPr6N-Medium` or
//! `MSung-Light` without embedding the font. hayro maps those CIDs to Unicode itself, but it needs
//! a face to draw them with; without one the text is blank. A craft-fonts face for the font's
//! script comes first (`raster/substitute.rs`); this module comes next, before a craft-fonts face
//! for the nearest Han script, and before hayro's Latin standard faces: one face
//! already installed on this machine, used for the viewer only (`RenderConfig::system_fonts`, off
//! for exports, printing and OCR). It is read at runtime and never embedded or shipped
//! (AGENTS.md §1.4), Noto CJK and Source Han are left out on purpose (§1.1), and
//! `PDFCRAFT_SYSTEM_FONTS=0` turns it off, as for the interface font.

use std::collections::HashMap;
use std::io::Read as _;
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
    /// Map the four supported Adobe CJK collections to their writing systems. Identity and
    /// custom collections provide no regional hint and cannot select an installed face here.
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

/// Keep named Windows body, heading, calligraphic and FangSong faces distinct. A cache keyed only
/// by script and weight would make the first Chinese font on a page stand in for all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ChineseFamily {
    Default,
    Song,
    Hei,
    Kai,
    FangSong,
}

impl ChineseFamily {
    /// The family a font name asks for: the Windows and Founder (方正) names that Chinese PDFs,
    /// CNKI papers in particular, use without embedding the font. FangSong is tested first, as
    /// its names contain "song" too. Founder PostScript names are matched by prefix (`FZSSK--GBK1-0`
    /// is 方正书宋, `FZXBSK--GBK1-0` 方正小标宋, `FZHTK--GBK1-0` 方正黑体, `FZKTK--GBK1-0` 方正楷体,
    /// `FZFSK--GBK1-0` 方正仿宋).
    fn of(name: &str) -> Self {
        // PDF subset names prepend six uppercase ASCII letters and '+'. Match the actual
        // PostScript name so Founder prefixes work for subsets too.
        let name = match name.split_once('+') {
            Some((tag, family)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => family,
            _ => name,
        };
        let name = name.to_ascii_lowercase();
        let has = |hints: &[&str]| hints.iter().any(|hint| name.contains(hint));
        let starts = |prefixes: &[&str]| prefixes.iter().any(|p| name.starts_with(p));
        if has(&["fangsong", "simfang", "仿宋"]) || starts(&["fzfs"]) {
            Self::FangSong
        } else if has(&["simsun", "stsong", "songti", "song", "宋"]) || starts(&["fzss", "fzbs", "fzxbs", "fzcs"]) {
            Self::Song
        } else if has(&["simhei", "stheiti", "stxihei", "heiti", "黑"]) || starts(&["fzht", "fzhei", "fzlth", "fzdh"]) {
            Self::Hei
        } else if has(&["kaiti", "stkaiti", "kai", "楷"]) || starts(&["fzkt", "fzkai"]) {
            Self::Kai
        } else {
            Self::Default
        }
    }
}

/// The installed face to draw a CID font of `query`'s collection with, as `(data, face index)`.
/// `None` when it is turned off, the collection isn't one of the four, or nothing fits.
pub(crate) fn fallback(query: &FallbackFontQuery) -> Option<(FontData, u32)> {
    if cfg!(target_arch = "wasm32") || std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        return None;
    }
    let script = Script::of(&query.character_collection.as_ref()?.family)?;
    let bold = query.is_bold || query.font_weight >= 600;
    let family = ChineseFamily::of(query.post_script_name.as_deref().unwrap_or_default());
    /// A face file and the index of the chosen face in it, or `None` when nothing fits.
    type Face = Option<(Arc<Vec<u8>>, u32)>;
    type Faces = HashMap<(Script, bool, ChineseFamily), Face>;
    static CACHE: Mutex<Option<Faces>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let found = cache.get_or_insert_with(HashMap::new).entry((script, bold, family)).or_insert_with(|| {
        candidates(script, family, bold).iter().find_map(|c| {
            read(c, script.probe(), bold).inspect(|(_, index)| {
                log::info!("CJK font fallback for {script:?}/{family:?}: {} (face {index})", c.path.display());
            })
        })
    });
    found.as_ref().map(|(bytes, index)| (Arc::clone(bytes) as FontData, *index))
}

/// Read a candidate through the bounded file cache and select a face that maps `probe`,
/// preferring the requested region and weight. Unreadable files and unsuitable faces return
/// `None`, allowing the caller to try the next candidate without changing the PDF.
fn read(candidate: &Candidate, probe: char, bold: bool) -> Option<(Arc<Vec<u8>>, u32)> {
    let bytes = file_bytes(&candidate.path)?;
    let index = pick_face(&bytes, probe, candidate.region, bold)?;
    Some((bytes, index))
}

/// The bytes of an installed font file, read once for the whole process and shared: a collection
/// such as `PingFang.ttc` is tens of megabytes and is the candidate of several scripts and both
/// weights, so the per-script cache keeps only a handle and a face index. A file that cannot be
/// used is remembered too, so it is not tried again for every script or family.
fn file_bytes(path: &Path) -> Option<Arc<Vec<u8>>> {
    /// The bytes of each file tried, or `None` for one that cannot be used.
    type Files = HashMap<PathBuf, Option<Arc<Vec<u8>>>>;
    static FILES: Mutex<Option<Files>> = Mutex::new(None);
    let mut files = FILES.lock().unwrap_or_else(PoisonError::into_inner);
    files.get_or_insert_with(HashMap::new).entry(path.to_path_buf()).or_insert_with(|| read_file(path, MAX_BYTES).map(Arc::new)).clone()
}

/// A regular file of at most `max` bytes, or `None`: a font path is untrusted input.
fn read_file(path: &Path, max: u64) -> Option<Vec<u8>> {
    // Reject special files before opening them (in particular a FIFO can block on open).
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    // Bound the actual read too: the file can grow after the metadata check.
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
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

/// List installed-font paths in preference order, with regional face-name hints for collections.
/// Windows Simplified Chinese first tries the named document family; its generic list also
/// prefers a bold face for bold requests. macOS includes bounded downloaded-font locations.
/// Only fixed platform filenames are used: a PDF font name never becomes a filesystem path.
fn candidates(script: Script, family: ChineseFamily, bold: bool) -> Vec<Candidate> {
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
            Script::SimplifiedChinese => match family {
                ChineseFamily::Song => &["simsun.ttc", "msyh.ttc", "msjh.ttc"],
                ChineseFamily::Hei => &["simhei.ttf", "msyh.ttc", "simsun.ttc", "msjh.ttc"],
                ChineseFamily::Kai => &["simkai.ttf", "simsun.ttc", "msyh.ttc", "msjh.ttc"],
                ChineseFamily::FangSong => &["simfang.ttf", "simsun.ttc", "msyh.ttc", "msjh.ttc"],
                ChineseFamily::Default if bold => &["msyhbd.ttc", "msyh.ttc", "simsun.ttc", "msjh.ttc"],
                ChineseFamily::Default => &["msyh.ttc", "simsun.ttc", "msjh.ttc"],
            },
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

/// Locate the Windows Fonts directory using `WINDIR`, then `SystemRoot`, then `C:\Windows`.
/// This only constructs the path; candidate reads still enforce regular-file and size limits.
fn windows_fonts() -> PathBuf {
    let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    dir.join("Fonts")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPTS: [Script; 4] = [Script::Japanese, Script::SimplifiedChinese, Script::TraditionalChinese, Script::Korean];

    /// Standard, localized and subset-prefixed names retain their Chinese family hint;
    /// unrelated Latin names and generic YaHei requests use the default candidate list.
    #[test]
    fn chinese_family_hints_include_subset_and_localized_names() {
        for name in ["SimSun", "ABCDEF+SimSun", "STSong-Light", "宋体"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::Song, "{name}");
        }
        for name in ["SimHei", "STHeiti", "黑体"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::Hei, "{name}");
        }
        for name in ["KaiTi", "STKaiti", "楷体"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::Kai, "{name}");
        }
        assert_eq!(ChineseFamily::of("Helvetica"), ChineseFamily::Default);
        assert_eq!(ChineseFamily::of("MicrosoftYaHei"), ChineseFamily::Default);
    }

    /// FangSong names contain "song" and must not become Song; Founder (方正) names in CNKI papers
    /// pick the matching installed family instead of YaHei for everything (#3587).
    #[test]
    fn fangsong_and_founder_names_pick_their_family() {
        for name in ["FangSong", "FangSong_GB2312", "STFangsong", "SimFang", "仿宋", "FZFSK--GBK1-0", "ABCDEF+FZFSK--GBK1-0"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::FangSong, "{name}");
        }
        for name in ["FZSSK--GBK1-0", "FZBSK--GBK1-0", "FZXBSK--GBK1-0", "FZShuSong-Z01", "STZhongsong", "NSimSun", "ABCDEF+FZSSK--GBK1-0"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::Song, "{name}");
        }
        for name in ["FZHTK--GBK1-0", "FZLTHJW--GB1-0", "STXihei", "ABCDEF+FZHTK--GBK1-0"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::Hei, "{name}");
        }
        for name in ["FZKTK--GBK1-0", "KaiTi_GB2312", "ABCDEF+FZKTK--GBK1-0"] {
            assert_eq!(ChineseFamily::of(name), ChineseFamily::Kai, "{name}");
        }
    }

    /// Every family has a Windows list that ends in faces any Chinese Windows has, so a missing
    /// named face still draws the text.
    #[cfg(windows)]
    #[test]
    fn every_windows_chinese_family_falls_back_to_common_faces() {
        for family in [ChineseFamily::Default, ChineseFamily::Song, ChineseFamily::Hei, ChineseFamily::Kai, ChineseFamily::FangSong] {
            let list = candidates(Script::SimplifiedChinese, family, false);
            assert!(list.iter().any(|c| c.path.ends_with("msyh.ttc")), "{family:?}");
        }
        let fangsong = candidates(Script::SimplifiedChinese, ChineseFamily::FangSong, false);
        assert!(fangsong.first().is_some_and(|c| c.path.ends_with("simfang.ttf")));
    }

    /// Invalid font bytes, missing paths and directories produce no candidate face rather
    /// than preventing the resolver from trying a later installed font.
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

    /// A Latin-only face fails every CJK probe even though its Latin glyph remains readable,
    /// distinguishing unsupported script coverage from an unparseable font file.
    #[test]
    fn a_latin_face_is_rejected_for_every_script() {
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        for script in SCRIPTS {
            assert_eq!(pick_face(inter, script.probe(), &[], false), None, "{script:?}");
        }
        assert_eq!(pick_face(inter, 'A', &[], false), Some(0));
    }

    /// Platform candidates are absolute TrueType paths and exclude the font families barred
    /// by the asset policy, independently of whether those files are installed locally.
    #[test]
    fn candidates_are_absolute_font_files_without_adobe_designs() {
        for script in SCRIPTS {
            let list = candidates(script, ChineseFamily::Default, false);
            assert!(!list.is_empty(), "{script:?}");
            for c in list {
                assert!(c.path.is_absolute(), "{}", c.path.display());
                assert!(c.path.extension().is_some_and(|e| e == "ttf" || e == "ttc"), "{}", c.path.display());
                let name = c.path.to_string_lossy().to_lowercase();
                assert!(
                    !["notosanscjk", "notoserifcjk", "sourcehan", "source-han", "adobe", "kozuka", "minion"].iter().any(|b| name.contains(b)),
                    "{name}"
                );
            }
        }
    }

    /// The four regional Adobe collections select their corresponding scripts; Identity
    /// carries no regional information and must not acquire an installed fallback by guess.
    #[test]
    fn only_the_four_adobe_collections_have_a_script() {
        assert_eq!(Script::of(&CidFamily::AdobeCNS1), Some(Script::TraditionalChinese));
        assert_eq!(Script::of(&CidFamily::AdobeGB1), Some(Script::SimplifiedChinese));
        assert_eq!(Script::of(&CidFamily::AdobeKorea1), Some(Script::Korean));
        assert_eq!(Script::of(&CidFamily::AdobeJapan1), Some(Script::Japanese));
        assert_eq!(Script::of(&CidFamily::AdobeIdentity), None);
    }

    /// A real Windows GBK book names these faces. Replacing both with YaHei hides the
    /// distinction between serif body text and sans headings even when the exact face exists.
    #[cfg(windows)]
    #[test]
    fn windows_preserves_installed_chinese_document_families() {
        use hayro::hayro_interpret::hayro_cmap::CharacterCollection;
        for (name, expected, file) in [
            ("SimSun", "SimSun", "simsun.ttc"),
            ("SimHei", "SimHei", "simhei.ttf"),
            ("KaiTi", "KaiTi", "simkai.ttf"),
            ("FangSong", "FangSong", "simfang.ttf"),
            ("ABCDEF+FZSSK--GBK1-0", "SimSun", "simsun.ttc"),
            ("ABCDEF+FZHTK--GBK1-0", "SimHei", "simhei.ttf"),
            ("ABCDEF+FZKTK--GBK1-0", "KaiTi", "simkai.ttf"),
            ("ABCDEF+FZFSK--GBK1-0", "FangSong", "simfang.ttf"),
        ] {
            if !windows_fonts().join(file).is_file() {
                continue;
            }
            let query = FallbackFontQuery {
                post_script_name: Some(name.into()),
                character_collection: Some(CharacterCollection { family: CidFamily::AdobeGB1, supplement: 4 }),
                ..FallbackFontQuery::default()
            };
            let (bytes, index) = fallback(&query).expect("installed Chinese font");
            let font = skrifa::FontRef::from_index(bytes.as_ref().as_ref(), index).unwrap();
            let names: Vec<String> = font.localized_strings(StringId::POSTSCRIPT_NAME).map(|n| n.chars().collect()).collect();
            assert!(names.iter().any(|n| n.eq_ignore_ascii_case(expected)), "{name} resolved to {names:?}, expected {expected}");
        }
    }

    /// On a Mac the face for Traditional Chinese is the TC one of PingFang, not the first of the
    /// collection (which is another region's), and it maps a character only Traditional has.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_picks_the_regional_pingfang_face() {
        // Found where this macOS keeps it: the system folder, or the downloaded font assets.
        let path = candidates(Script::TraditionalChinese, ChineseFamily::Default, false)
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
        /// Independently locate an installed asset by filename within the test's depth limit,
        /// providing an oracle for the production candidate list without using its discovery.
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
            assert!(
                candidates(script, ChineseFamily::Default, false).iter().any(|c| c.path == asset),
                "{script:?}: {} is not a candidate",
                asset.display()
            );
        }
    }
}
