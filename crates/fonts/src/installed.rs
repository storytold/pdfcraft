//! The complete program of a font a document names, from the fonts installed on this machine.
//!
//! A PDF usually embeds only a subset of each font: the glyphs its text used. A character the
//! subset lacks can't be drawn from it, so an edit that adds one had to take another face. When
//! the very font the document names is installed here, its complete program can draw the new
//! characters instead, and the edit keeps the document's typeface.
//!
//! Only an exact match counts: the PostScript name the PDF gives, or its family and style
//! (`Arial,Bold`, `TimesNewRoman`). Only a face whose own embedding permission (OS/2 `fsType`)
//! allows editable embedding is used ([`crate::embedding::validate`]), and it is embedded whole,
//! as a font chosen with Load font is.
//!
//! A process reads the installed fonts only once it has been allowed to ([`allow`]). The desktop
//! app allows it at start. A headless run (`pdfcraft-cli run`, `mcp`) does not: it is confined to
//! its `--root`, and the operating system's font folders are outside it. `PDFCRAFT_SYSTEM_FONTS=1`
//! allows it for a headless run whose owner wants it, and `PDFCRAFT_SYSTEM_FONTS=0` turns it off
//! everywhere, as it turns off the other installed-font uses. The web build has no installed fonts.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::CraftFont;

/// Whether this process may read the installed fonts ([`allow`]).
static ALLOWED: AtomicBool = AtomicBool::new(false);

/// Lets this process complete a document's fonts from the fonts installed on the machine. The
/// desktop app calls it at start; nothing headless does, so a run confined to `--root` reads no
/// font folder outside it.
pub fn allow() {
    ALLOWED.store(true, Ordering::Relaxed);
}

/// Whether installed fonts may be read now: `PDFCRAFT_SYSTEM_FONTS` (`0` never, `1` always) over
/// what the process asked for ([`allow`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn allowed(setting: Option<&std::ffi::OsStr>, asked: bool) -> bool {
    match setting.and_then(|v| v.to_str()) {
        Some("0") => false,
        Some("1") => true,
        _ => asked,
    }
}

/// Complete installed programs of the font `name` (a PDF `/BaseFont`, subset tag allowed): the
/// face whose PostScript name it is first, then faces whose family and style it names.
pub fn matching_installed_faces(name: &str) -> Vec<&'static CraftFont> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !allowed(std::env::var_os("PDFCRAFT_SYSTEM_FONTS").as_deref(), ALLOWED.load(Ordering::Relaxed)) {
            return Vec::new();
        }
        native::matching(native::index(), name)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        Vec::new()
    }
}

/// The advance, in thousandths of an em, of `ch` in the font a PDF names `base_font` (subset tag
/// allowed) without embedding it. Such a font's `/Widths` often list only the characters its text
/// used, and a new character can be written in it only once its advance is known. It comes from
/// the same font installed on this machine, else the published metrics of the standard font the
/// name asks for (Arial and Helvetica, Times New Roman and Times, Courier New and Courier), else
/// its metric-compatible counterpart in craft-fonts. Nothing is embedded: the document goes on
/// naming the font. `None` when none of them knows the font or has the character.
pub fn named_font_advance(base_font: &str, ch: char) -> Option<f64> {
    use skrifa::MetadataProvider as _;
    let from_face = |bytes: &[u8]| {
        let face = skrifa::FontRef::new(bytes).ok()?;
        let glyph = face.charmap().map(ch).filter(|g| g.to_u32() != 0)?;
        let metrics = face.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
        let upem = face.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default()).units_per_em;
        let advance = f64::from(metrics.advance_width(glyph)?) * 1000.0 / f64::from(upem);
        (upem > 0 && advance.is_finite() && (0.0..=16_000.0).contains(&advance)).then_some(advance)
    };
    if let Some(advance) = matching_installed_faces(base_font).first().and_then(|face| from_face(face.bytes)) {
        return Some(advance);
    }
    if let Some(face) = standard_face(base_font)
        && let Some(code) = crate::win_ansi_byte(ch)
    {
        let advance = face.width(code);
        if advance > 0.0 {
            return Some(advance);
        }
    }
    crate::craft::metric_counterpart_face(base_font).and_then(|face| from_face(face.bytes))
}

/// The standard-14 face whose published metrics are `base_font`'s: only for the names that ask
/// for exactly that face. `ArialNarrow`, `ArialRoundedMT` or `HelveticaNeue` contain a standard
/// name and have other widths.
fn standard_face(base_font: &str) -> Option<crate::Std14> {
    let (face, how) = crate::Std14::recognise(base_font)?;
    let family = crate::embedding::family_stem(base_font);
    let exact = ["arial", "helvetica", "timesnewroman", "times", "timesroman", "couriernew", "courier"].contains(&family.as_str());
    (exact && how == crate::Std14Match::Alias).then_some(face)
}

#[cfg(test)]
mod named_tests {
    use super::*;

    #[test]
    fn installed_fonts_are_read_only_when_allowed() {
        use std::ffi::OsStr;
        // Headless by default: nothing asked, nothing set.
        assert!(!allowed(None, false));
        // The desktop app asks; the setting overrides either way.
        assert!(allowed(None, true));
        assert!(allowed(Some(OsStr::new("1")), false));
        assert!(!allowed(Some(OsStr::new("0")), true));
        // Anything else leaves it to the process.
        assert!(!allowed(Some(OsStr::new("yes")), false) && allowed(Some(OsStr::new("")), true));
    }

    #[test]
    fn standard_metrics_serve_only_the_names_that_ask_for_that_face() {
        for name in [
            "ArialMT",
            "ABCDEF+Arial-BoldMT",
            "Arial,Italic",
            "Helvetica-BoldOblique",
            "TimesNewRomanPSMT",
            "TimesNewRomanPS-BoldItalicMT",
            "Times-Roman",
            "CourierNewPSMT",
            "Courier",
        ] {
            assert!(standard_face(name).is_some(), "{name}");
        }
        for name in [
            "ArialNarrow",
            "Arial-BlackMT",
            "ArialRoundedMTBold",
            "HelveticaNeue",
            "ArialUnicodeMS",
            "Verdana",
            "TimesTen-Roman",
            "SomeHouseSerif",
            "CourierPrime",
            "",
        ] {
            assert!(standard_face(name).is_none(), "{name}");
        }
        assert_eq!(standard_face("Arial-BoldMT"), Some(crate::Std14::HelveticaBold));
        assert_eq!(standard_face("TimesNewRomanPS-ItalicMT"), Some(crate::Std14::TimesItalic));
    }

    #[test]
    fn a_named_standard_font_gives_the_advance_of_a_character_its_widths_left_out() {
        // With installed fonts off, the published metrics answer: Helvetica's z is 500, Times-Roman's 444.
        // (On a machine that has Arial the installed face gives the same 500.)
        let z = named_font_advance("ArialMT", 'z').unwrap();
        assert!((z - 500.0).abs() < 1.0, "{z}");
        let z = named_font_advance("Times-Roman", 'z').unwrap();
        assert!((z - 444.0).abs() < 1.0, "{z}");
        // A character outside the standard encoding, in a font nothing here knows: no advance.
        assert_eq!(named_font_advance("HouseGroteskThatIsNotInstalledAnywhere", 'z'), None);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::{Path, PathBuf};
    use std::sync::{LazyLock, Mutex, OnceLock};

    use crate::CraftFont;
    use crate::embedding::face_key;

    /// Font files looked at in all, across every folder.
    const MAX_FILES: usize = 8192;
    /// How deep below a font folder files are looked for.
    const MAX_DEPTH: usize = 4;
    /// Faces read from one collection.
    const MAX_FACES: u32 = 64;
    /// Tables one face may list.
    const MAX_TABLES: u16 = 256;
    /// The most of a `name` table read while indexing.
    const MAX_NAME_TABLE: u32 = 256 << 10;
    /// The largest font file loaded when it matches. One face of it must still fit the 16 MiB
    /// [`crate::embedding::validate`] allows.
    const MAX_FILE: u64 = 64 << 20;

    /// One installed face, by the names it answers to.
    #[derive(Clone, Debug)]
    pub(super) struct Entry {
        path: PathBuf,
        /// Where its table directory starts: 0 for a standalone font, else its offset in the collection.
        offset: u32,
        collection: bool,
        postscript: String,
        style: String,
        /// [`face_key`]s of its PostScript name, full name, family and style, and (for a regular
        /// face) its family alone.
        keys: Vec<String>,
    }

    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    /// Each face looked up, by file and offset: loaded once, or refused once.
    type Loaded = HashMap<(PathBuf, u32), Option<&'static CraftFont>>;
    static LOADED: LazyLock<Mutex<Loaded>> = LazyLock::new(|| Mutex::new(HashMap::new()));

    /// The installed faces, read once per process on first use.
    pub(super) fn index() -> &'static [Entry] {
        INDEX.get_or_init(|| index_directories(&font_directories()))
    }

    /// The folders fonts are installed in on this system.
    pub(super) fn font_directories() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if cfg!(target_os = "macos") {
            let mut dirs = vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")];
            dirs.extend(home.map(|h| h.join("Library/Fonts")));
            dirs
        } else if cfg!(windows) {
            let mut dirs: Vec<PathBuf> = std::env::var_os("WINDIR").map(|p| PathBuf::from(p).join("Fonts")).into_iter().collect();
            dirs.extend(std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Microsoft/Windows/Fonts")));
            dirs
        } else {
            let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
            if let Some(home) = home {
                dirs.extend([home.join(".local/share/fonts"), home.join(".fonts")]);
            }
            dirs
        }
    }

    /// Every usable face in `roots`. Links below a root are not followed, so a root can't be
    /// left; unreadable files and faces that forbid editable embedding are skipped.
    pub(super) fn index_directories(roots: &[PathBuf]) -> Vec<Entry> {
        let mut pending: Vec<(PathBuf, usize)> = roots.iter().take(16).filter_map(|r| r.canonicalize().ok()).map(|r| (r, 0)).collect();
        let mut seen = std::collections::HashSet::new();
        let mut files = 0usize;
        let mut entries = Vec::new();
        while let Some((path, depth)) = pending.pop() {
            if files >= MAX_FILES {
                break;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() || !seen.insert(path.clone()) {
                continue;
            }
            if meta.is_dir() {
                if depth < MAX_DEPTH
                    && let Ok(children) = std::fs::read_dir(&path)
                {
                    let mut children: Vec<PathBuf> = children.filter_map(Result::ok).take(MAX_FILES).map(|c| c.path()).collect();
                    children.sort();
                    pending.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
                }
            } else if meta.is_file() && font_extension(&path) {
                files += 1;
                entries.extend(faces_of(&path));
            }
        }
        entries
    }

    fn font_extension(path: &Path) -> bool {
        path.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| ["ttf", "otf", "ttc"].iter().any(|x| e.eq_ignore_ascii_case(x)))
    }

    fn read_at(file: &mut File, offset: u64, len: usize) -> Option<Vec<u8>> {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0; len];
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    fn u16_at(b: &[u8], i: usize) -> Option<u16> {
        Some(u16::from_be_bytes([*b.get(i)?, *b.get(i.checked_add(1)?)?]))
    }

    fn u32_at(b: &[u8], i: usize) -> Option<u32> {
        let end = i.checked_add(4)?;
        Some(u32::from_be_bytes(b.get(i..end)?.try_into().ok()?))
    }

    /// The faces of one font file, read from its table directories and `name` tables only.
    fn faces_of(path: &Path) -> Vec<Entry> {
        let Ok(mut file) = File::open(path) else { return Vec::new() };
        let Some(head) = read_at(&mut file, 0, 12) else { return Vec::new() };
        let (offsets, collection) = if head.get(..4) == Some(b"ttcf") {
            let count = u32_at(&head, 8).unwrap_or(0).min(MAX_FACES);
            let Some(table) = usize::try_from(count).ok().and_then(|n| read_at(&mut file, 12, n.saturating_mul(4))) else { return Vec::new() };
            ((0..count as usize).filter_map(|i| u32_at(&table, i * 4)).collect(), true)
        } else {
            (vec![0], false)
        };
        offsets.into_iter().filter_map(|offset| face_at(&mut file, path, offset, collection)).collect()
    }

    fn face_at(file: &mut File, path: &Path, offset: u32, collection: bool) -> Option<Entry> {
        let header = read_at(file, u64::from(offset), 12)?;
        let tables = u16_at(&header, 4)?.min(MAX_TABLES);
        let records = read_at(file, u64::from(offset) + 12, usize::from(tables) * 16)?;
        let find = |tag: &[u8; 4]| {
            (0..usize::from(tables))
                .find_map(|i| (records.get(i * 16..i * 16 + 4) == Some(tag)).then(|| (u32_at(&records, i * 16 + 8), u32_at(&records, i * 16 + 12))))
        };
        // A variable font can't be embedded as one static face; nor can one without outlines.
        if find(b"fvar").is_some() || (find(b"glyf").is_none() && find(b"CFF ").is_none()) {
            return None;
        }
        if let Some((Some(os2), Some(len))) = find(b"OS/2")
            && len >= 10
        {
            let fs_type = u16_at(&read_at(file, u64::from(os2), 10)?, 8)?;
            // Restricted, preview & print only, or bitmap only: never re-embedded for editing.
            if fs_type & (0x0002 | 0x0004 | 0x0200) != 0 {
                return None;
            }
        }
        let (Some(name_at), Some(name_len)) = find(b"name")? else { return None };
        let names = parse_names(&read_at(file, u64::from(name_at), usize::try_from(name_len.min(MAX_NAME_TABLE)).ok()?)?);
        let get = |id: u16| names.get(&id).cloned().filter(|s| !s.is_empty() && s.len() <= 128);
        let family = get(1)?;
        let style = get(2).unwrap_or_else(|| "Regular".into());
        let postscript = get(6).unwrap_or_else(|| format!("{family}-{style}").replace(' ', ""));
        let mut keys = vec![face_key(&postscript), face_key(&format!("{family} {style}"))];
        keys.extend(get(4).map(|full| face_key(&full)));
        if let (Some(f), Some(s)) = (get(16), get(17)) {
            keys.push(face_key(&format!("{f} {s}")));
            if is_regular(&s) {
                keys.push(face_key(&f));
            }
        }
        if is_regular(&style) {
            keys.push(face_key(&family));
        }
        keys.retain(|k| !k.is_empty());
        keys.dedup();
        Some(Entry { path: path.to_owned(), offset, collection, postscript, style, keys })
    }

    fn is_regular(style: &str) -> bool {
        ["regular", "normal", "roman", "book", "plain", "standard"].iter().any(|s| style.eq_ignore_ascii_case(s))
    }

    /// Name IDs 1, 2, 4, 6, 16 and 17 of a `name` table, preferring US English Windows strings,
    /// then other Windows and Unicode ones, then Macintosh Roman ones.
    pub(super) fn parse_names(table: &[u8]) -> HashMap<u16, String> {
        let mut best: HashMap<u16, (u8, String)> = HashMap::new();
        let (Some(count), Some(storage)) = (u16_at(table, 2), u16_at(table, 4)) else { return HashMap::new() };
        for i in 0..usize::from(count.min(4096)) {
            let at = 6 + i * 12;
            let (Some(platform), Some(encoding), Some(language), Some(id), Some(len), Some(off)) = (
                u16_at(table, at),
                u16_at(table, at + 2),
                u16_at(table, at + 4),
                u16_at(table, at + 6),
                u16_at(table, at + 8),
                u16_at(table, at + 10),
            ) else {
                break;
            };
            if ![1, 2, 4, 6, 16, 17].contains(&id) {
                continue;
            }
            let start = usize::from(storage) + usize::from(off);
            let Some(bytes) = table.get(start..start + usize::from(len)) else { continue };
            let (rank, text) = match (platform, encoding) {
                (3, 1 | 10) | (0, _) => {
                    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
                    (
                        if platform == 3 && language == 0x409 {
                            0
                        } else if platform == 3 {
                            1
                        } else {
                            2
                        },
                        String::from_utf16_lossy(&units),
                    )
                }
                (1, 0) if language == 0 => (3, bytes.iter().map(|&b| if b.is_ascii() { char::from(b) } else { '\u{FFFD}' }).collect()),
                _ => continue,
            };
            if best.get(&id).is_none_or(|(r, _)| rank < *r) {
                best.insert(id, (rank, text.trim().to_owned()));
            }
        }
        best.into_iter().map(|(id, (_, s))| (id, s)).collect()
    }

    /// The faces of `entries` that `name` names, the PostScript match first, each loaded and
    /// checked once per process.
    pub(super) fn matching(entries: &[Entry], name: &str) -> Vec<&'static CraftFont> {
        let key = face_key(name);
        if key.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<&Entry> = entries.iter().filter(|e| e.keys.contains(&key)).collect();
        found.sort_by_key(|e| face_key(&e.postscript) != key);
        let mut faces: Vec<&'static CraftFont> = Vec::new();
        for entry in found.into_iter().take(8) {
            if let Some(face) = loaded(entry)
                && !faces.iter().any(|f| std::ptr::eq(*f, face) || f.bytes == face.bytes)
            {
                faces.push(face);
            }
        }
        faces
    }

    fn loaded(entry: &Entry) -> Option<&'static CraftFont> {
        let key = (entry.path.clone(), entry.offset);
        let mut cache = LOADED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(face) = cache.get(&key) {
            return *face;
        }
        let face = load(entry);
        cache.insert(key, face);
        face
    }

    /// Reads, extracts and checks the face; it is kept for the rest of the process.
    fn load(entry: &Entry) -> Option<&'static CraftFont> {
        let len = std::fs::metadata(&entry.path).ok()?.len();
        if len > MAX_FILE {
            return None;
        }
        let mut data = Vec::new();
        File::open(&entry.path).ok()?.take(len).read_to_end(&mut data).ok()?;
        let bytes = if entry.collection { extract_face(&data, usize::try_from(entry.offset).ok()?)? } else { data };
        crate::embedding::validate(&bytes).ok()?;
        // The name the PDF font takes is the PostScript name, as the document's own subset has it.
        let family: &'static str = Box::leak(entry.postscript.clone().into_boxed_str());
        let style: &'static str = Box::leak(entry.style.clone().into_boxed_str());
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        Some(Box::leak(Box::new(CraftFont { family, style, scripts: &[], bytes })))
    }

    /// One face of a collection (`ttcf`) as a standalone font: its table directory with each
    /// table it lists copied after it.
    pub(super) fn extract_face(data: &[u8], offset: usize) -> Option<Vec<u8>> {
        let version = data.get(offset..offset.checked_add(4)?)?;
        let count = u16_at(data, offset + 4)?;
        if count == 0 || count > MAX_TABLES {
            return None;
        }
        let n = usize::from(count);
        let mut records = Vec::with_capacity(n);
        for i in 0..n {
            let at = offset.checked_add(12 + i * 16)?;
            let tag = data.get(at..at + 4)?;
            let (checksum, start, len) = (u32_at(data, at + 4)?, u32_at(data, at + 8)?, u32_at(data, at + 12)?);
            let start = usize::try_from(start).ok()?;
            let table = data.get(start..start.checked_add(usize::try_from(len).ok()?)?)?;
            records.push((tag, checksum, table));
        }
        let selector = 15 - count.leading_zeros() as u16;
        let range = (1u16 << selector).checked_mul(16)?;
        let mut out = Vec::new();
        out.extend_from_slice(version);
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&range.to_be_bytes());
        out.extend_from_slice(&selector.to_be_bytes());
        out.extend_from_slice(&(count.checked_mul(16)?.checked_sub(range)?).to_be_bytes());
        let mut at = 12 + n * 16;
        let mut body = Vec::new();
        for (tag, checksum, table) in &records {
            out.extend_from_slice(tag);
            out.extend_from_slice(&checksum.to_be_bytes());
            out.extend_from_slice(&u32::try_from(at).ok()?.to_be_bytes());
            out.extend_from_slice(&u32::try_from(table.len()).ok()?.to_be_bytes());
            body.extend_from_slice(table);
            let padded = table.len().checked_add(3)? & !3;
            body.resize(body.len() + (padded - table.len()), 0);
            at = at.checked_add(padded)?;
            if at > (32 << 20) {
                return None;
            }
        }
        out.extend_from_slice(&body);
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        const SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

        /// A collection holding `fonts`, each face's tables copied after the directories.
        fn collection(fonts: &[&[u8]]) -> Vec<u8> {
            let dirs: Vec<usize> = fonts.iter().map(|f| 12 + 16 * usize::from(u16_at(f, 4).unwrap())).collect();
            let mut at = 12 + 4 * fonts.len() + dirs.iter().sum::<usize>();
            let mut out = b"ttcf\x00\x01\x00\x00".to_vec();
            out.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
            let mut offset = 12 + 4 * fonts.len();
            for d in &dirs {
                out.extend_from_slice(&(offset as u32).to_be_bytes());
                offset += d;
            }
            let mut body = Vec::new();
            for f in fonts {
                let n = usize::from(u16_at(f, 4).unwrap());
                out.extend_from_slice(&f[..12]);
                for i in 0..n {
                    let r = 12 + i * 16;
                    let (start, len) = (u32_at(f, r + 8).unwrap() as usize, u32_at(f, r + 12).unwrap() as usize);
                    out.extend_from_slice(&f[r..r + 8]);
                    out.extend_from_slice(&(at as u32).to_be_bytes());
                    out.extend_from_slice(&(len as u32).to_be_bytes());
                    body.extend_from_slice(&f[start..start + len]);
                    let padded = (len + 3) & !3;
                    body.resize(body.len() + padded - len, 0);
                    at += padded;
                }
            }
            out.extend_from_slice(&body);
            out
        }

        fn folder(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
            let root = std::env::temp_dir().join(format!("pdfcraft-installed-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("sub")).unwrap();
            for (file, bytes) in files {
                std::fs::write(root.join(file), bytes).unwrap();
            }
            root
        }

        #[test]
        fn a_face_is_found_by_its_postscript_name_or_its_family_and_style() {
            let root = folder("names", &[("sub/Inter-Regular.ttf", REGULAR), ("Inter-SemiBold.TTF", SEMIBOLD), ("notes.txt", b"not a font")]);
            let entries = index_directories(std::slice::from_ref(&root));
            assert_eq!(entries.len(), 2, "{entries:?}");
            // The subset tag and punctuation are not part of the name.
            let regular = matching(&entries, "ABCDEF+Inter-Regular");
            assert_eq!(regular.len(), 1);
            assert_eq!(regular[0].bytes, REGULAR, "the program is the installed file, unchanged");
            assert_eq!(regular[0].family, "Inter-Regular", "the PDF font is named by its PostScript name");
            // A regular face also answers to its family alone, as `TimesNewRoman` names one.
            assert_eq!(matching(&entries, "Inter")[0].bytes, REGULAR);
            // `Family,Style` names, as Word writes them, find the styled face.
            assert_eq!(matching(&entries, "Inter,SemiBold")[0].bytes, SEMIBOLD);
            assert!(matching(&entries, "Inter-Bold").is_empty(), "never another weight");
            assert!(matching(&entries, "Arial").is_empty(), "never another family");
            assert!(matching(&entries, "").is_empty() && matching(&entries, "ABCDEF+").is_empty());
            // Each face is loaded once: the same face is returned again.
            assert!(std::ptr::eq(matching(&entries, "Inter-Regular")[0], regular[0]));
            std::fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn a_face_of_a_collection_becomes_a_standalone_font() {
            let ttc = collection(&[REGULAR, SEMIBOLD]);
            let root = folder("collection", &[("Inter.ttc", &ttc)]);
            let entries = index_directories(std::slice::from_ref(&root));
            assert_eq!(entries.len(), 2);
            let semibold = matching(&entries, "Inter-SemiBold");
            assert_eq!(semibold.len(), 1);
            let face = skrifa::FontRef::new(semibold[0].bytes).expect("a standalone font, not a collection");
            use skrifa::MetadataProvider;
            assert_eq!(face.charmap().map('A'), skrifa::FontRef::new(SEMIBOLD).unwrap().charmap().map('A'));
            crate::embedding::validate(semibold[0].bytes).unwrap();
            std::fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn faces_that_forbid_editable_embedding_are_not_indexed() {
            // Set fsType to "restricted" in a copy.
            let mut restricted = REGULAR.to_vec();
            let n = usize::from(u16_at(&restricted, 4).unwrap());
            let os2 = (0..n).find(|i| &restricted[12 + i * 16..16 + i * 16] == b"OS/2").unwrap();
            let at = u32_at(&restricted, 12 + os2 * 16 + 8).unwrap() as usize;
            restricted[at + 8..at + 10].copy_from_slice(&2u16.to_be_bytes());
            let root = folder("restricted", &[("Restricted.ttf", &restricted)]);
            assert!(index_directories(std::slice::from_ref(&root)).is_empty());
            std::fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn hostile_files_are_skipped_without_panicking() {
            let mut truncated_ttc = collection(&[REGULAR]);
            truncated_ttc.truncate(400);
            let huge_count = [b"ttcf".as_slice(), &[0, 1, 0, 0], &u32::MAX.to_be_bytes()].concat();
            let root = folder(
                "hostile",
                &[("a.ttf", b""), ("b.ttf", &[0; 11]), ("c.ttc", &truncated_ttc), ("d.ttc", &huge_count), ("e.otf", &REGULAR[..300])],
            );
            let entries = index_directories(std::slice::from_ref(&root));
            for name in ["Inter-Regular", "Inter"] {
                assert!(matching(&entries, name).is_empty(), "{name}");
            }
            assert_eq!(extract_face(&truncated_ttc, 12 + 4), None);
            assert_eq!(extract_face(b"", 0), None);
            assert_eq!(extract_face(&huge_count, usize::MAX - 2), None);
            assert!(parse_names(&[0, 0, 0xff, 0xff, 0xff, 0xff]).is_empty());
            std::fs::remove_dir_all(&root).unwrap();
        }
    }
}
