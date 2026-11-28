//! Installed CJK faces for CID fonts a PDF doesn't embed.
//!
//! Acrobat and other tools often write Chinese, Japanese or Korean text with one of the Adobe
//! collections (Adobe-Japan1, GB1, CNS1, Korea1) and a font name such as `KozGoPr6N-Medium` or
//! `MSung-Light` without embedding the font. hayro maps those CIDs to Unicode itself, but it needs
//! a face to draw them with; without one the text is blank. The craft-fonts Japanese faces come
//! first (see `japanese_fallback` in `raster.rs`); this module is the last resort: one face
//! already installed on this machine, used for the viewer only (`RenderConfig::system_fonts`, off
//! for exports, printing and OCR). It is read at runtime and never embedded or shipped
//! (docs/development.md "Fonts and attribution"), Noto CJK and Source Han are left out on purpose (clean-room.md), and
//! `PDFCRAFT_SYSTEM_FONTS=0` turns it off, as for the interface font.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use craft_pdf::interpret::font::{FallbackFontQuery, FontData};
use craft_pdf::interpret::hayro_cmap::CidFamily;
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
/// prefers a bold face for bold requests. macOS includes bounded downloaded-font locations. Linux
/// also finds those fixed names, and the Windows ones, anywhere in the font folders (bounded).
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
        let fixed = ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| names.iter().map(move |f| Path::new(dir).join(f)));
        let mut files = unix_named(script, family, unix_installed());
        files.extend(fixed);
        files.extend(unix_found(names.iter().filter_map(|n| n.rsplit('/').next()), unix_installed()));
        files.extend(unix_found(windows_names(script).iter().copied(), unix_installed()));
        files
    };
    files.into_iter().map(|path| Candidate { path, region }).collect()
}

/// Windows file names of the faces a Chinese document family names, which Linux users often copy
/// into a font folder (#7285). They come before the generic list, as on Windows.
fn windows_family_names(script: Script, family: ChineseFamily) -> &'static [&'static str] {
    match (script, family) {
        (Script::SimplifiedChinese, ChineseFamily::Song) => &["simsun.ttc", "simsun.ttf"],
        (Script::SimplifiedChinese, ChineseFamily::Hei) => &["simhei.ttf"],
        (Script::SimplifiedChinese, ChineseFamily::Kai) => &["simkai.ttf"],
        (Script::SimplifiedChinese, ChineseFamily::FangSong) => &["simfang.ttf"],
        _ => &[],
    }
}

/// Windows file names of a script's faces, tried on Linux after the distribution's own faces.
fn windows_names(script: Script) -> &'static [&'static str] {
    match script {
        Script::Japanese => &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"],
        Script::SimplifiedChinese => &["msyh.ttc", "simsun.ttc", "simsun.ttf", "simhei.ttf", "simkai.ttf", "simfang.ttf", "msjh.ttc"],
        Script::TraditionalChinese => &["msjh.ttc", "mingliu.ttc", "msyh.ttc"],
        Script::Korean => &["malgun.ttf", "gulim.ttc"],
    }
}

/// The installed files of the named Chinese family, found anywhere in the font folders.
fn unix_named(script: Script, family: ChineseFamily, found: &HashMap<String, Vec<PathBuf>>) -> Vec<PathBuf> {
    unix_found(windows_family_names(script, family).iter().copied(), found)
}

/// The installed files with these names (compared without case), in the order given. A name with
/// several copies yields each copy in search order, so a copy that can't be read falls through to
/// the next.
fn unix_found<'a>(names: impl Iterator<Item = &'a str>, found: &HashMap<String, Vec<PathBuf>>) -> Vec<PathBuf> {
    names.flat_map(|n| found.get(&n.to_lowercase()).into_iter().flatten().cloned()).collect()
}

/// Folders and entries looked at when searching the Unix font folders: they can be large, and a
/// symbolic link can loop back on a folder, so both the depth and the total are bounded.
const MAX_SCAN_DEPTH: u32 = 6;
const MAX_SCAN_ENTRIES: usize = 50_000;

/// The font files in the Unix font folders, searched once: lower-cased file name to its paths,
/// in search order.
fn unix_installed() -> &'static HashMap<String, Vec<PathBuf>> {
    static FOUND: OnceLock<HashMap<String, Vec<PathBuf>>> = OnceLock::new();
    FOUND.get_or_init(|| scan_font_dirs(&unix_font_dirs(), MAX_SCAN_ENTRIES))
}

/// The folders fontconfig's default configuration reads: the system and local ones, and the
/// user's (`$XDG_DATA_HOME/fonts`, else `~/.local/share/fonts`, and `~/.fonts`). Users install
/// fonts by hand into any of them, often in a subfolder of their own (#7285).
fn unix_font_dirs() -> Vec<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let home = std::env::var_os("HOME").and_then(absolute);
    let data = std::env::var_os("XDG_DATA_HOME").and_then(absolute).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
    dirs.extend(data.map(|d| d.join("fonts")));
    dirs.extend(home.map(|h| h.join(".fonts")));
    dirs
}

/// Every `.ttf` and `.ttc` file under `roots`, by lower-cased file name. Paths are kept in search
/// order: roots in order, and each folder's entries sorted, so the choice doesn't depend on disk
/// order and every copy of a name can be tried. `max_entries` is the most entries kept in total;
/// a folder that runs past it keeps its first entries in sorted order, so which ones are kept is
/// deterministic too.
fn scan_font_dirs(roots: &[PathBuf], max_entries: usize) -> HashMap<String, Vec<PathBuf>> {
    /// The greatest remaining depth each real folder has been explored to.
    type Explored = HashMap<PathBuf, u32>;
    /// The entries of each real folder, read once.
    type Listed = HashMap<PathBuf, Vec<PathBuf>>;

    /// Add the font files of `dir` and its subfolders down to `depth` more levels.
    fn walk(dir: &Path, depth: u32, budget: &mut usize, explored: &mut Explored, listed: &mut Listed, found: &mut HashMap<String, Vec<PathBuf>>) {
        // A folder reached through a symbolic link is still a folder, so a link back to one
        // already walked would read its entries again at every level and spend the budget on
        // them, leaving a later root such as the user's font folder unsearched. Each real folder
        // is read once, and explored only as far as it hasn't been already: a folder a link
        // reached with no depth left is still searched from its own root, which has more.
        let Ok(real) = std::fs::canonicalize(dir) else { return };
        if explored.get(&real).is_some_and(|&reached| reached >= depth) {
            return;
        }
        explored.insert(real.clone(), depth);
        let paths = match listed.entry(real) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let mut paths = Vec::new();
                if let Ok(entries) = std::fs::read_dir(dir) {
                    paths.extend(entries.flatten().map(|entry| entry.path()));
                }
                // Read whole, then sorted before the budget is spent, so a folder with more
                // entries than the budget has left keeps its first names in sorted order rather
                // than whichever entries the disk returned first: the same files are found on
                // every run.
                paths.sort();
                let kept = (*budget).min(paths.len());
                *budget -= kept;
                paths.truncate(kept);
                // Each file is added once, on the folder's first visit; a later, deeper visit
                // only continues below it.
                for path in &paths {
                    if let Some(name) = font_file_name(path) {
                        found.entry(name).or_default().push(path.clone());
                    }
                }
                entry.insert(paths.clone());
                paths
            }
        };
        if depth > 0 {
            for path in paths.iter().filter(|p| p.is_dir()) {
                walk(path, depth - 1, budget, explored, listed, found);
            }
        }
    }
    let mut found = HashMap::new();
    let mut explored = Explored::new();
    let mut listed = Listed::new();
    let mut budget = max_entries;
    for root in roots {
        walk(root, MAX_SCAN_DEPTH, &mut budget, &mut explored, &mut listed, &mut found);
    }
    found
}

/// The lower-cased file name of `path` when it is a `.ttf` or `.ttc` file rather than a folder.
fn font_file_name(path: &Path) -> Option<String> {
    if path.is_dir() {
        return None;
    }
    let name = path.file_name()?.to_str()?.to_lowercase();
    (name.ends_with(".ttf") || name.ends_with(".ttc")).then_some(name)
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
                assert!(c.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("ttc")), "{}", c.path.display());
                // The file name, not the whole path: a discovered file sits under the user's home
                // folder, whose name is the user's to choose.
                let name = c.path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                assert!(
                    !["notosanscjk", "notoserifcjk", "sourcehan", "source-han", "adobe", "kozuka", "minion"].iter().any(|b| name.contains(b)),
                    "{name}"
                );
            }
        }
    }

    /// SimSun, SimHei and KaiTi copied by hand into a subfolder of a font folder, with whatever
    /// case their names have, are found and come first for the families a GBK invoice names;
    /// before #7285 only a few fixed distribution paths were tried, so the text drew as boxes.
    #[test]
    fn hand_installed_chinese_families_are_found_in_font_subfolders() {
        let root = std::env::temp_dir().join(format!("pdfcraft-7285-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"font").unwrap();
            path
        };
        let simsun = file("local/windows/SimSun.ttc");
        let simhei = file("local/windows/SIMHEI.TTF");
        let simkai = file("user/zh/simkai.ttf");
        let wqy = file("local/wqy/wqy-zenhei.ttc");
        file("local/readme.txt");
        // Below the depth bound: not looked at.
        file(&format!("user/{}/simfang.ttf", ["d"; MAX_SCAN_DEPTH as usize + 1].join("/")));
        let found = scan_font_dirs(&[root.join("local"), root.join("user"), root.join("missing")], MAX_SCAN_ENTRIES);
        let _ = std::fs::remove_dir_all(&root);

        let gb = Script::SimplifiedChinese;
        assert_eq!(unix_named(gb, ChineseFamily::Song, &found), std::slice::from_ref(&simsun));
        assert_eq!(unix_named(gb, ChineseFamily::Hei, &found), [simhei]);
        assert_eq!(unix_named(gb, ChineseFamily::Kai, &found), [simkai]);
        assert!(unix_named(gb, ChineseFamily::FangSong, &found).is_empty(), "too deep");
        assert!(unix_named(gb, ChineseFamily::Default, &found).is_empty());
        assert_eq!(unix_found(["wqy-zenhei.ttc"].into_iter(), &found), [wqy]);
        assert!(unix_found(windows_names(gb).iter().copied(), &found).contains(&simsun));
        assert!(!found.contains_key("readme.txt"));
    }

    /// A symbolic link back to a folder already walked must not read its entries again at every
    /// level: doing so spends the entry budget, and a font in a later folder is then missed even
    /// though the distinct entries fit within it.
    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_does_not_spend_the_scan_budget() {
        let root = std::env::temp_dir().join(format!("pdfcraft-7285-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let first = root.join("first");
        std::fs::create_dir_all(&first).unwrap();
        for n in 0..4 {
            std::fs::write(first.join(format!("note-{n}.txt")), b"not a font").unwrap();
        }
        std::os::unix::fs::symlink(&first, first.join("loop")).unwrap();
        let later = root.join("later");
        std::fs::create_dir_all(&later).unwrap();
        let simhei = later.join("simhei.ttf");
        std::fs::write(&simhei, b"font").unwrap();
        // Enough for the linked folder to be read five times over, but not for both roots.
        let found = scan_font_dirs(&[first, later], 20);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(unix_named(Script::SimplifiedChinese, ChineseFamily::Hei, &found), [simhei]);
    }

    /// A folder a link reaches with no depth left is still searched from its own root, which has
    /// more: a font one level below it is found. Marking a folder as seen for good would skip it.
    #[cfg(unix)]
    #[test]
    fn a_folder_reached_early_through_a_link_is_still_searched_from_its_own_root() {
        let root = std::env::temp_dir().join(format!("pdfcraft-7285-alias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let second = root.join("second");
        let simhei = second.join("windows/simhei.ttf");
        std::fs::create_dir_all(simhei.parent().unwrap()).unwrap();
        std::fs::write(&simhei, b"font").unwrap();
        // Reached from `first` with no depth left: its own entries are read, but not `windows`.
        let deep = root.join("first/a/b/c/d/e");
        std::fs::create_dir_all(&deep).unwrap();
        std::os::unix::fs::symlink(&second, deep.join("local")).unwrap();
        let found = scan_font_dirs(&[root.join("first"), second.clone()], MAX_SCAN_ENTRIES);
        // The folder's entries were read through the link, so the path found is that one; it is
        // the same file.
        let expected = std::fs::canonicalize(&simhei).unwrap();
        let resolved: Vec<PathBuf> =
            unix_named(Script::SimplifiedChinese, ChineseFamily::Hei, &found).iter().filter_map(|p| std::fs::canonicalize(p).ok()).collect();
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(resolved, [expected], "found below the folder the link reached with no depth left");
    }

    /// A folder with more entries than the budget has left keeps the first names in sorted order,
    /// not whichever entries the disk returned first, so the same files are found on every run.
    #[test]
    fn a_folder_over_the_entry_budget_keeps_the_first_names_in_sorted_order() {
        let root = std::env::temp_dir().join(format!("pdfcraft-7285-budget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Written in an order unlike their sorted order, so a scan that stops at the budget
        // without sorting first keeps a different set.
        for n in (1..40).chain(std::iter::once(0)) {
            std::fs::write(root.join(format!("face-{n:02}.ttf")), b"font").unwrap();
        }
        let found = scan_font_dirs(std::slice::from_ref(&root), 5);
        let _ = std::fs::remove_dir_all(&root);

        let mut names: Vec<&String> = found.keys().collect();
        names.sort();
        assert_eq!(names, ["face-00.ttf", "face-01.ttf", "face-02.ttf", "face-03.ttf", "face-04.ttf"]);
    }

    /// Two copies of a named font keep their search order, so a copy that can't be read falls
    /// through to the next instead of hiding it.
    #[test]
    fn every_copy_of_a_font_name_is_kept_in_search_order() {
        let root = std::env::temp_dir().join(format!("pdfcraft-7285-copies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"font").unwrap();
            path
        };
        let first = file("system/old/simhei.ttf");
        let second = file("user/simhei.ttf");
        let found = scan_font_dirs(&[root.join("system"), root.join("user")], MAX_SCAN_ENTRIES);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(unix_named(Script::SimplifiedChinese, ChineseFamily::Hei, &found), [first, second]);
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
        use craft_pdf::interpret::hayro_cmap::CharacterCollection;
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
