//! Arabic fonts already installed on this machine, for Arabic text written into PDFs.
//!
//! The fonts are read at runtime and never shipped (AGENTS.md §1.4). Their glyph outlines go into
//! the PDF as Type 3 glyphs, so only fonts whose licence allows embedding (OS/2 `fsType`) are
//! listed. `PDFCRAFT_SYSTEM_FONTS=0` turns the lookup off, as for the interface fallback.
//!
//! Scanning reads only each file's table directory and its `name`, `OS/2`, `cmap`, `head` and
//! `post` tables, so listing a few hundred installed fonts stays quick; a face's whole file is
//! read when text is drawn with it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::ArabicFace;

/// One installed face that can draw Arabic and may be embedded.
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledFont {
    /// Family name ("Traditional Arabic"), from the typographic family when the font has one.
    pub family: String,
    /// Style name ("Bold").
    pub style: String,
    pub bold: bool,
    pub italic: bool,
    pub path: PathBuf,
    /// The face's index in a collection (`.ttc`); 0 otherwise.
    pub index: u32,
}

impl InstalledFont {
    /// The face, read from disk (cached for the few faces in use). `None` when the file is gone,
    /// too large or no longer parses.
    pub fn load(&self) -> Option<ArabicFace> {
        let bytes = self.bytes()?;
        ArabicFace::from_shared(self.family.clone(), self.style.clone(), self.bold, bytes, self.index)
    }

    /// The font file's bytes (face [`Self::index`] of it), read from disk (cached like [`Self::load`]).
    pub fn bytes(&self) -> Option<Arc<[u8]>> {
        cached_bytes(&self.path)
    }
}

/// Larger files are not read: a font path is still untrusted input.
const MAX_FILE: u64 = 64 << 20;
/// Largest table read while scanning (a `cmap` with every CJK character stays well below).
const MAX_TABLE: u64 = 8 << 20;
/// Faces tried in a collection (`.ttc`).
const MAX_FACES: u32 = 16;
const MAX_TABLES: usize = 64;
/// Files looked at in the font folders, and how deep their subfolders go.
const MAX_FILES: usize = 6000;
const MAX_DEPTH: usize = 4;
/// Whole font files kept in memory for drawing: at least as many as a font choice tries.
const CACHE: usize = 16;

/// Every installed Arabic face that may be embedded, by family then style. Scanned once; empty
/// when `PDFCRAFT_SYSTEM_FONTS=0` and on the web.
pub fn installed_arabic_fonts() -> &'static [InstalledFont] {
    static FONTS: OnceLock<Vec<InstalledFont>> = OnceLock::new();
    FONTS.get_or_init(scan)
}

/// The installed Arabic families, sorted and without duplicates, for a font menu (worked out once).
pub fn arabic_font_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut families: Vec<String> = installed_arabic_fonts().iter().map(|f| f.family.clone()).collect();
        families.sort_by_key(|f| f.to_lowercase());
        families.dedup();
        families
    })
}

/// The installed Arabic family named `font` (a PDF font name such as `ABCDEF+TraditionalArabic-Bold`
/// gives Traditional Arabic), if there is one.
pub fn arabic_font_family(font: &str) -> Option<&'static str> {
    let name = normalize(font);
    installed_arabic_fonts().iter().find(|f| !name.is_empty() && normalize(&f.family) == name).map(|f| f.family.as_str())
}

/// Installed Arabic faces best first for text that asks for `requested` (a family chosen by the
/// user), sits among fonts named `hints` (`/BaseFont` names of the page or the edited text, such
/// as `ABCDEF+TraditionalArabic-Bold`), and is serif (naskh) or not and bold or not.
pub fn arabic_candidates(requested: Option<&str>, hints: &[&str], serif: bool, bold: bool) -> Vec<&'static InstalledFont> {
    rank(installed_arabic_fonts(), requested, hints, serif, bold)
}

/// Families PdfCraft prefers when nothing names one, best first: the usual Windows, macOS and
/// Linux faces for Arabic body text.
const SERIF_DEFAULTS: &[&str] =
    &["traditionalarabic", "simplifiedarabic", "sakkalmajalla", "timesnewroman", "notonaskharabic", "amiri", "arabictypesetting", "geezapro"];
const SANS_DEFAULTS: &[&str] = &["arial", "tahoma", "segoeui", "notosansarabic", "sfarabic", "geezapro", "dejavusans", "simplifiedarabic"];

fn rank<'a>(fonts: &'a [InstalledFont], requested: Option<&str>, hints: &[&str], serif: bool, bold: bool) -> Vec<&'a InstalledFont> {
    let requested = requested.map(normalize).filter(|r| !r.is_empty());
    let hints: Vec<(String, bool)> = hints.iter().map(|h| (normalize(h), named_bold(h))).filter(|(h, _)| !h.is_empty()).collect();
    let defaults = if serif { SERIF_DEFAULTS } else { SANS_DEFAULTS };
    let score = |f: &InstalledFont| -> (usize, bool) {
        let family = normalize(&f.family);
        // Bold or regular as asked; a hint that names its weight decides for its family.
        let mut want_bold = bold;
        let place = if requested.as_deref() == Some(family.as_str()) {
            0
        } else if let Some(i) = hints.iter().position(|(h, _)| *h == family) {
            want_bold |= hints.get(i).is_some_and(|(_, b)| *b);
            1 + i
        } else if let Some(i) = hints.iter().position(|(h, _)| family.len() >= 4 && (h.contains(&family) || family.contains(h.as_str()))) {
            want_bold |= hints.get(i).is_some_and(|(_, b)| *b);
            100 + i
        } else if let Some(i) = defaults.iter().position(|d| *d == family) {
            200 + i
        } else {
            1000
        };
        (place, f.bold != want_bold || f.italic)
    };
    let mut out: Vec<&InstalledFont> = fonts.iter().collect();
    // Stable: equal faces keep the scan's order.
    out.sort_by_key(|f| score(f));
    out
}

/// A font name reduced for comparison: no subset tag (`ABCDEF+`), no style or foundry suffix, only
/// lower-case letters and digits. "ABCDEF+TraditionalArabic-Bold" and "Traditional Arabic" match.
fn normalize(name: &str) -> String {
    let name = match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.chars().all(|c| c.is_ascii_uppercase()) => rest,
        _ => name,
    };
    // A style after a comma or hyphen ("Arial,Bold", "Arial-BoldMT").
    let name = name.split([',', '-']).next().unwrap_or(name);
    let mut s: String = name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    for suffix in ["psmt", "mt", "ps", "regular", "bolditalic", "bold", "italic", "oblique"] {
        if s.len() > suffix.len() + 2
            && let Some(stem) = s.strip_suffix(suffix)
        {
            s = stem.to_string();
        }
    }
    s
}

fn named_bold(name: &str) -> bool {
    let n = name.to_lowercase();
    ["bold", "black", "heavy", "semibold", "demi"].iter().any(|w| n.contains(w))
}

fn scan() -> Vec<InstalledFont> {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") || cfg!(target_arch = "wasm32") {
        return Vec::new();
    }
    let mut files = Vec::new();
    for dir in font_dirs() {
        walk(&dir, 0, &mut files);
    }
    let mut out = Vec::new();
    for path in files {
        out.extend(faces_in(&path));
    }
    out.sort_by(|a, b| a.family.to_lowercase().cmp(&b.family.to_lowercase()).then(a.bold.cmp(&b.bold)).then(a.italic.cmp(&b.italic)));
    out.dedup_by(|a, b| a.family == b.family && a.style == b.style);
    out
}

fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if cfg!(windows) {
        let windir =
            std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let mut dirs = vec![windir.join("Fonts")];
        // Fonts installed for one user only.
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
        dirs
    } else if cfg!(target_os = "macos") {
        let mut dirs = vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")];
        dirs.extend(home.map(|h| h.join("Library/Fonts")));
        dirs
    } else {
        let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        if let Some(h) = home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
        dirs
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if depth < MAX_DEPTH {
                walk(&path, depth + 1, out);
            }
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc")) {
            out.push(path);
        }
    }
}

/// The Arabic, embeddable faces of one font file.
fn faces_in(path: &Path) -> Vec<InstalledFont> {
    use skrifa::MetadataProvider as _;
    use skrifa::raw::TableProvider as _;
    let Some(bytes) = read_sparse(path) else { return Vec::new() };
    let mut out = Vec::new();
    for index in 0..MAX_FACES {
        let Ok(font) = skrifa::FontRef::from_index(&bytes, index) else { break };
        let charmap = font.charmap();
        // Alef and beh: a face with both is meant for Arabic text.
        if charmap.map('\u{0627}').is_none() || charmap.map('\u{0628}').is_none() {
            continue;
        }
        // Restricted licence (bit 1) or bitmap-only embedding (bit 9): its outlines may not go into a PDF.
        let fs_type = font.os2().map_or(0, |os2| os2.fs_type());
        if fs_type & 0x0002 != 0 || fs_type & 0x0200 != 0 {
            continue;
        }
        let name = |ids: [skrifa::string::StringId; 2]| {
            ids.iter()
                .find_map(|id| font.localized_strings(*id).english_or_first().map(|s| s.chars().collect::<String>()))
                .filter(|s| !s.trim().is_empty())
        };
        use skrifa::string::StringId;
        let Some(family) = name([StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]) else { continue };
        let style = name([StringId::TYPOGRAPHIC_SUBFAMILY_NAME, StringId::SUBFAMILY_NAME]).unwrap_or_else(|| "Regular".into());
        let attrs = font.attributes();
        let bold = attrs.weight.value() >= 600.0;
        let italic = attrs.style != skrifa::attribute::Style::Normal;
        out.push(InstalledFont { family: family.trim().to_string(), style: style.trim().to_string(), bold, italic, path: path.to_path_buf(), index });
    }
    out
}

/// A buffer the size of the file holding only its table directories and the tables the scan
/// reads; the rest stays zero, so a large font costs a few reads, not its whole size.
fn read_sparse(path: &Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let len = std::fs::metadata(path).ok()?.len();
    if !(12..=MAX_FILE).contains(&len) {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; usize::try_from(len).ok()?];
    let mut fill = |buf: &mut [u8], at: u64, n: u64| -> Option<()> {
        let end = at.checked_add(n).filter(|e| *e <= len)?;
        let range = usize::try_from(at).ok()?..usize::try_from(end).ok()?;
        file.seek(SeekFrom::Start(at)).ok()?;
        file.read_exact(buf.get_mut(range)?).ok()
    };
    let u32_at = |buf: &[u8], at: usize| buf.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
    fill(&mut buf, 0, 12)?;
    let offsets: Vec<u64> = if buf.get(0..4) == Some(b"ttcf") {
        let count = u32_at(&buf, 8)?.min(MAX_FACES);
        fill(&mut buf, 12, u64::from(count) * 4)?;
        (0..count as usize).filter_map(|i| u32_at(&buf, 12 + i * 4)).map(u64::from).collect()
    } else {
        vec![0]
    };
    for at in offsets {
        fill(&mut buf, at, 12)?;
        let start = usize::try_from(at).ok()?;
        let tables = buf.get(start + 4..start + 6).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]]))).min(MAX_TABLES);
        fill(&mut buf, at + 12, tables as u64 * 16)?;
        for t in 0..tables {
            let rec = start + 12 + t * 16;
            let Some(tag) = buf.get(rec..rec + 4) else { break };
            if !matches!(tag, b"name" | b"OS/2" | b"cmap" | b"head" | b"post" | b"hhea" | b"maxp") {
                continue;
            }
            let (Some(off), Some(size)) = (u32_at(&buf, rec + 8), u32_at(&buf, rec + 12)) else { break };
            if u64::from(size) <= MAX_TABLE {
                // A table past the end of the file is left zero; the parser then rejects it.
                let _ = fill(&mut buf, u64::from(off), u64::from(size));
            }
        }
    }
    Some(buf)
}

/// Font files read for drawing, by path, oldest first.
type FileCache = Mutex<Vec<(PathBuf, Arc<[u8]>)>>;

/// A whole font file, kept for the next draw with the same face.
fn cached_bytes(path: &Path) -> Option<Arc<[u8]>> {
    static FILES: OnceLock<FileCache> = OnceLock::new();
    let files = FILES.get_or_init(|| Mutex::new(Vec::new()));
    // A poisoned lock only means another thread panicked mid-update; the list is still usable.
    let mut files = files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, bytes)) = files.iter().find(|(p, _)| p == path) {
        return Some(bytes.clone());
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let bytes: Arc<[u8]> = std::fs::read(path).ok()?.into();
    if files.len() >= CACHE {
        files.remove(0);
    }
    files.push((path.to_path_buf(), bytes.clone()));
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(family: &str, bold: bool) -> InstalledFont {
        InstalledFont {
            family: family.into(),
            style: if bold { "Bold".into() } else { "Regular".into() },
            bold,
            italic: false,
            path: PathBuf::from(format!("{family}.ttf")),
            index: 0,
        }
    }

    fn installed() -> Vec<InstalledFont> {
        ["Arial", "Tahoma", "Traditional Arabic", "Simplified Arabic", "Andalus", "Sakkal Majalla"]
            .iter()
            .flat_map(|f| [font(f, false), font(f, true)])
            .collect()
    }

    fn first(requested: Option<&str>, hints: &[&str], serif: bool, bold: bool) -> (String, bool) {
        let fonts = installed();
        let best = rank(&fonts, requested, hints, serif, bold)[0];
        (best.family.clone(), best.bold)
    }

    #[test]
    fn names_from_pdfs_match_installed_families() {
        assert_eq!(normalize("ABCDEF+TraditionalArabic-Bold"), "traditionalarabic");
        assert_eq!(normalize("Arial-BoldMT"), "arial");
        assert_eq!(normalize("ArialMT"), "arial");
        assert_eq!(normalize("Arial,Bold"), "arial");
        assert_eq!(normalize("Traditional Arabic"), "traditionalarabic");
        assert_eq!(normalize("Times New Roman PSMT"), "timesnewroman");
        // Not a subset tag: kept.
        assert_eq!(normalize("Ab+cd"), "abcd");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn the_documents_own_font_comes_first_then_the_defaults() {
        // The page's font, with its weight.
        assert_eq!(first(None, &["XYZABC+SimplifiedArabic-Bold"], false, false), ("Simplified Arabic".into(), true));
        assert_eq!(first(None, &["AndalusRegular"], true, false), ("Andalus".into(), false));
        // A requested family wins over the page's fonts.
        assert_eq!(first(Some("Tahoma"), &["TraditionalArabic"], true, true), ("Tahoma".into(), true));
        // Nothing named: the defaults for the style.
        assert_eq!(first(None, &[], true, false), ("Traditional Arabic".into(), false));
        assert_eq!(first(None, &["Helvetica"], false, true), ("Arial".into(), true));
        // A family that isn't installed falls through to the defaults.
        assert_eq!(first(Some("Not Installed"), &["Unknown-Font"], false, false), ("Arial".into(), false));
    }

    #[test]
    fn every_installed_face_is_ranked_once() {
        let fonts = installed();
        assert_eq!(rank(&fonts, None, &[], false, false).len(), fonts.len());
        assert!(rank(&[], Some("Arial"), &["Arial"], false, false).is_empty());
    }

    #[test]
    fn broken_and_missing_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-sysfont-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in
            [("empty.ttf", &b""[..]), ("junk.ttf", &b"not a font at all, really"[..]), ("ttc.ttc", &b"ttcf\0\x01\0\0\xff\xff\xff\xff"[..])]
        {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            assert!(faces_in(&p).is_empty(), "{name}");
        }
        // A table directory pointing past the end of the file.
        let mut lying = b"\0\x01\0\0\0\x01\0\0\0\0\0\0".to_vec();
        lying.extend_from_slice(b"name\0\0\0\0\xff\xff\xff\x00\xff\xff\xff\xff");
        let p = dir.join("lying.ttf");
        std::fs::write(&p, &lying).unwrap();
        assert!(faces_in(&p).is_empty());
        assert!(faces_in(&dir.join("missing.ttf")).is_empty());
        assert!(cached_bytes(&dir.join("missing.ttf")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_faces_draw_arabic() {
        // Whatever this machine has: every listed face loads and has alef.
        eprintln!("installed Arabic families: {:?}", arabic_font_families());
        for f in installed_arabic_fonts().iter().take(4) {
            let face = f.load().unwrap_or_else(|| panic!("{f:?} doesn't load"));
            assert!(face.shaper().unwrap().has('\u{0627}'), "{f:?}");
        }
    }
}
