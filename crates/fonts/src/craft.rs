//! Fonts from the optional craft-fonts build input (<https://github.com/storytold/craft-fonts>).
//!
//! `build.rs` embeds every font in craft-fonts' manifest when PdfCraft is built with
//! `CRAFT_FONTS_DIR=<checkout>`; otherwise [`CRAFT_FONTS`] is empty and everything here returns
//! nothing. Callers must work either way.

use skrifa::MetadataProvider as _;

/// A font from the optional craft-fonts build input (empty unless built with `CRAFT_FONTS_DIR`).
pub struct CraftFont {
    pub family: &'static str,
    pub style: &'static str,
    /// ISO 15924 scripts the font is for, e.g. `"Jpan"`.
    pub scripts: &'static [&'static str],
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/craft_fonts.rs"));

impl CraftFont {
    /// Whether the font is meant for `script` (ISO 15924, e.g. `"Jpan"`).
    pub fn covers(&self, script: &str) -> bool {
        self.scripts.contains(&script)
    }

    /// A unique name for registering the face with a font system ("BIZ UDPGothic Bold").
    pub fn name(&self) -> String {
        format!("{} {}", self.family, self.style)
    }
}

/// The `Jpan` craft-fonts faces in the order the interface prefers them: BIZ UDPGothic (a UI
/// face) first, then the rest in manifest order. Empty without craft-fonts.
pub fn ui_japanese_fonts() -> Vec<&'static CraftFont> {
    let mut fonts: Vec<&CraftFont> = CRAFT_FONTS.iter().filter(|f| f.covers("Jpan")).collect();
    // Stable: manifest order within each group.
    fonts.sort_by_key(|f| f.family != "BIZ UDPGothic");
    fonts
}

/// The `Hans` craft-fonts faces for Simplified Chinese interface text, in manifest order.
/// Empty when built without craft-fonts (Chinese text then shows the font system's
/// replacement glyph, the same degraded mode as Japanese without craft-fonts).
pub fn ui_chinese_fonts() -> Vec<&'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.covers("Hans")).collect()
}

/// The `Arab` craft-fonts faces for Arabic-script interface text (file names, document titles),
/// in manifest order. Empty when built without craft-fonts or when it has no Arabic face.
pub fn ui_arabic_fonts() -> Vec<&'static CraftFont> {
    arabic(CRAFT_FONTS.iter())
}

/// The face for Arabic text written into PDFs: the first `Arab` face. `None` when built without
/// craft-fonts or when its revision has no Arabic face.
pub fn document_arabic_font() -> Option<&'static CraftFont> {
    ui_arabic_fonts().into_iter().next()
}

fn arabic<'a>(faces: impl IntoIterator<Item = &'a CraftFont>) -> Vec<&'a CraftFont> {
    faces.into_iter().filter(|f| f.covers("Arab")).collect()
}

/// The `Telu` craft-fonts faces for Telugu interface text (the Telugu catalog, file names,
/// document titles), in manifest order. Empty when built without craft-fonts or when it has no
/// Telugu face.
pub fn ui_telugu_fonts() -> Vec<&'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.covers("Telu")).collect()
}

/// An entry representing a discovered or loaded Thai font file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThaiFontEntry {
    /// Full display name (e.g. "Courier MonoThai", "TH Sarabun New Bold").
    pub name: String,
    /// Family name from font metadata (e.g. "Courier MonoThai", "TH Sarabun New").
    pub family: String,
    /// Subfamily / style from font metadata (e.g. "Regular", "Bold", "Italic").
    pub style: String,
    /// File name on disk (e.g. "COURMT__.TTF", "THSarabunNew.ttf").
    pub file_name: String,
    /// Path to the font file.
    pub path: std::path::PathBuf,
}

static ACTIVE_THAI_FONT: std::sync::RwLock<Option<&'static CraftFont>> = std::sync::RwLock::new(None);
static ACTIVE_THAI_ENTRY: std::sync::RwLock<Option<ThaiFontEntry>> = std::sync::RwLock::new(None);
static LOADED_THAI_CACHE: std::sync::RwLock<Option<std::collections::HashMap<std::path::PathBuf, &'static CraftFont>>> =
    std::sync::RwLock::new(None);
static DISCOVERED_THAI_FONTS: std::sync::RwLock<Option<Vec<ThaiFontEntry>>> = std::sync::RwLock::new(None);

/// Candidate directories where font files may reside.
/// Checks runtime environment variables, executable-relative paths, working directory paths,
/// and manifest directories to ensure robust discovery across dev and packaged distributions.
pub fn font_directories() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();

    // 1. Explicit environment variable overrides
    if let Some(env_dir) = std::env::var_os("PDFCRAFT_FONTS_DIR")
        .or_else(|| std::env::var_os("PDFCRAFT_THAI_FONTS_DIR"))
        .or_else(|| std::env::var_os("CRAFT_FONTS_DIR"))
    {
        dirs.push(std::path::PathBuf::from(env_dir));
    }

    // 2. Executable-relative paths (next to executable, or in ancestor directories for target/debug builds)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("fonts"));
            if let Some(grand) = parent.parent() {
                dirs.push(grand.join("fonts"));
                if let Some(great) = grand.parent() {
                    dirs.push(great.join("fonts"));
                }
            }
        }
    }

    // 3. Current working directory and relative paths
    for rel in ["fonts", "../fonts", "../../fonts"] {
        dirs.push(std::path::PathBuf::from(rel));
    }

    // 4. Manifest directory at compile/build time (for cargo test and local dev execution)
    let manifest = env!("CARGO_MANIFEST_DIR");
    dirs.push(std::path::Path::new(manifest).join("../../fonts"));
    dirs.push(std::path::Path::new(manifest).join("../fonts"));
    dirs.push(std::path::Path::new(manifest).join("fonts"));

    // Filter to existing directories and deduplicate
    let mut unique = Vec::new();
    for d in dirs {
        if d.is_dir() {
            let canonical = d.canonicalize().unwrap_or_else(|_| d.clone());
            if !unique.iter().any(|u: &std::path::PathBuf| u == &canonical) {
                unique.push(canonical);
            }
        }
    }
    unique
}

/// Inspect font bytes and extract family name, style, and whether Thai script ('\u{0E01}') is covered.
fn inspect_font_metadata(bytes: &[u8]) -> Option<(String, String, bool)> {
    use skrifa::MetadataProvider as _;
    let f = skrifa::FontRef::new(bytes).ok()?;
    let get_name = |id| {
        f.localized_strings(id)
            .find(|s| s.language() == Some("en"))
            .or_else(|| f.localized_strings(id).next())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty() && s.len() <= 128)
    };
    let family = get_name(skrifa::string::StringId::FAMILY_NAME).unwrap_or_else(|| "Thai Font".into());
    let style = get_name(skrifa::string::StringId::SUBFAMILY_NAME).unwrap_or_else(|| "Regular".into());

    let mut has_thai = f.charmap().map('\u{0E01}').is_some();
    if !has_thai {
        for idx in 1..16 {
            if let Ok(sub) = skrifa::FontRef::from_index(bytes, idx) {
                if sub.charmap().map('\u{0E01}').is_some() {
                    has_thai = true;
                    break;
                }
            } else {
                break;
            }
        }
    }
    Some((family, style, has_thai))
}

fn scan_thai_fonts_from_dirs() -> Vec<ThaiFontEntry> {
    let mut entries = Vec::new();
    let dirs = font_directories();

    for dir in dirs {
        let Ok(read_dir) = std::fs::read_dir(&dir) else { continue };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let is_font_ext = path.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf")
            });
            if !is_font_ext {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || meta.len() == 0 || meta.len() > (64 << 20) {
                continue;
            }
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if entries.iter().any(|e: &ThaiFontEntry| e.file_name.eq_ignore_ascii_case(&file_name)) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Some((family, style, has_thai)) = inspect_font_metadata(&bytes) else { continue };
            if !has_thai {
                continue;
            }
            let name = if style.eq_ignore_ascii_case("Regular") {
                family.clone()
            } else {
                format!("{family} {style}")
            };
            entries.push(ThaiFontEntry {
                name,
                family,
                style,
                file_name,
                path,
            });
        }
    }

    // Sort entries:
    // 1. COURMT__.TTF (Courier MonoThai Regular) as the default first choice
    // 2. Other Courier MonoThai styles
    // 3. Alphabetical by family, then style (Regular before Bold/Italic)
    entries.sort_by(|a, b| {
        let a_is_default = a.file_name.eq_ignore_ascii_case("COURMT__.TTF");
        let b_is_default = b.file_name.eq_ignore_ascii_case("COURMT__.TTF");
        if a_is_default != b_is_default {
            return b_is_default.cmp(&a_is_default);
        }
        let a_is_courier = a.family.eq_ignore_ascii_case("Courier MonoThai");
        let b_is_courier = b.family.eq_ignore_ascii_case("Courier MonoThai");
        if a_is_courier != b_is_courier {
            return b_is_courier.cmp(&a_is_courier);
        }
        let fam_cmp = a.family.to_lowercase().cmp(&b.family.to_lowercase());
        if fam_cmp != std::cmp::Ordering::Equal {
            return fam_cmp;
        }
        let a_is_reg = a.style.eq_ignore_ascii_case("Regular");
        let b_is_reg = b.style.eq_ignore_ascii_case("Regular");
        if a_is_reg != b_is_reg {
            return b_is_reg.cmp(&a_is_reg);
        }
        a.style.cmp(&b.style)
    });

    entries
}

/// Scan all font directories for Thai `.ttf` and `.otf` fonts and return a list of available entries.
/// Results are sorted with `COURMT__.TTF` (Courier MonoThai Regular) as the default first option.
pub fn available_thai_fonts() -> Vec<ThaiFontEntry> {
    if let Ok(lock) = DISCOVERED_THAI_FONTS.read() {
        if let Some(cached) = lock.as_ref() {
            return cached.clone();
        }
    }
    let list = scan_thai_fonts_from_dirs();
    if let Ok(mut lock) = DISCOVERED_THAI_FONTS.write() {
        *lock = Some(list.clone());
    }
    list
}

/// Force a rescan of font directories (useful if new fonts were added to the directory).
pub fn rescan_thai_fonts() -> Vec<ThaiFontEntry> {
    let list = scan_thai_fonts_from_dirs();
    if let Ok(mut lock) = DISCOVERED_THAI_FONTS.write() {
        *lock = Some(list.clone());
    }
    list
}

/// Load and leak font bytes with specific family and style names into a `'static CraftFont`.
pub fn load_custom_thai_font_named(
    bytes: Vec<u8>,
    family: String,
    style: String,
) -> Result<&'static CraftFont, crate::GlyphError> {
    if bytes.len() > (64 << 20) || bytes.is_empty() {
        return Err(crate::GlyphError::NoFont);
    }
    let mut found = false;
    for idx in 0..16 {
        if let Ok(f) = skrifa::FontRef::from_index(&bytes, idx) {
            if f.charmap().map('\u{0E01}').is_some() {
                found = true;
                break;
            }
        } else {
            break;
        }
    }
    if !found {
        return Err(crate::GlyphError::Missing);
    }
    let family_str = Box::leak(family.into_boxed_str()) as &'static str;
    let style_str = Box::leak(style.into_boxed_str()) as &'static str;
    let leaked_bytes = Box::leak(bytes.into_boxed_slice());
    let font = Box::leak(Box::new(CraftFont {
        family: family_str,
        style: style_str,
        scripts: &["Thai"],
        bytes: leaked_bytes,
    }));
    Ok(font)
}

/// Set the active Thai font using a `ThaiFontEntry`.
pub fn set_active_thai_font_entry(entry: &ThaiFontEntry) -> Result<&'static CraftFont, crate::GlyphError> {
    if let Ok(cache) = LOADED_THAI_CACHE.read() {
        if let Some(map) = cache.as_ref() {
            if let Some(&font) = map.get(&entry.path) {
                if let Ok(mut lock) = ACTIVE_THAI_FONT.write() {
                    *lock = Some(font);
                }
                if let Ok(mut elock) = ACTIVE_THAI_ENTRY.write() {
                    *elock = Some(entry.clone());
                }
                crate::shaping::invalidate_thai_cache();
                return Ok(font);
            }
        }
    }

    let bytes = std::fs::read(&entry.path).map_err(|_| crate::GlyphError::NoFont)?;
    let font = load_custom_thai_font_named(bytes, entry.family.clone(), entry.style.clone())?;

    if let Ok(mut cache) = LOADED_THAI_CACHE.write() {
        let map = cache.get_or_insert_with(std::collections::HashMap::new);
        map.insert(entry.path.clone(), font);
    }
    if let Ok(mut lock) = ACTIVE_THAI_FONT.write() {
        *lock = Some(font);
    }
    if let Ok(mut elock) = ACTIVE_THAI_ENTRY.write() {
        *elock = Some(entry.clone());
    }
    crate::shaping::invalidate_thai_cache();
    Ok(font)
}

/// Set active Thai font by matching font name, family, or file name (case-insensitive).
pub fn set_active_thai_font_by_name(name_or_file: &str) -> Result<&'static CraftFont, crate::GlyphError> {
    let list = available_thai_fonts();
    let lower = name_or_file.to_lowercase();
    let entry = list
        .iter()
        .find(|e| {
            e.file_name.to_lowercase() == lower
                || e.name.to_lowercase() == lower
                || e.family.to_lowercase() == lower
        })
        .or_else(|| {
            list.iter().find(|e| {
                e.file_name.to_lowercase().contains(&lower)
                    || e.name.to_lowercase().contains(&lower)
                    || e.family.to_lowercase().contains(&lower)
            })
        })
        .ok_or(crate::GlyphError::NoFont)?;
    set_active_thai_font_entry(entry)
}

/// Set active Thai font by file path.
pub fn set_active_thai_font_by_path(path: impl AsRef<std::path::Path>) -> Result<&'static CraftFont, crate::GlyphError> {
    let path_ref = path.as_ref();
    let list = available_thai_fonts();
    if let Some(entry) = list.iter().find(|e| e.path == path_ref) {
        return set_active_thai_font_entry(entry);
    }
    let bytes = std::fs::read(path_ref).map_err(|_| crate::GlyphError::NoFont)?;
    let (family, style, has_thai) = inspect_font_metadata(&bytes).ok_or(crate::GlyphError::NoFont)?;
    if !has_thai {
        return Err(crate::GlyphError::Missing);
    }
    let file_name = path_ref.file_name().and_then(|n| n.to_str()).unwrap_or("custom.ttf").to_string();
    let name = if style.eq_ignore_ascii_case("Regular") {
        family.clone()
    } else {
        format!("{family} {style}")
    };
    let entry = ThaiFontEntry {
        name,
        family,
        style,
        file_name,
        path: path_ref.to_path_buf(),
    };
    set_active_thai_font_entry(&entry)
}

/// Get the metadata of the currently active Thai font (auto-initializing if needed).
pub fn active_thai_font_entry() -> Option<ThaiFontEntry> {
    if let Ok(lock) = ACTIVE_THAI_ENTRY.read() {
        if let Some(entry) = lock.as_ref() {
            return Some(entry.clone());
        }
    }
    let _ = document_thai_font();
    ACTIVE_THAI_ENTRY.read().ok().and_then(|lock| lock.clone())
}

/// Get the currently active Thai font object (equivalent to [`document_thai_font`]).
pub fn active_thai_font() -> Option<&'static CraftFont> {
    document_thai_font()
}

/// Register a custom Thai font from static slices.
pub fn register_custom_thai_font(family: &'static str, style: &'static str, bytes: &'static [u8]) -> Result<(), crate::GlyphError> {
    let font = Box::leak(Box::new(CraftFont {
        family,
        style,
        scripts: &["Thai"],
        bytes,
    }));
    if let Ok(mut lock) = ACTIVE_THAI_FONT.write() {
        *lock = Some(font);
    }
    if let Ok(mut elock) = ACTIVE_THAI_ENTRY.write() {
        *elock = Some(ThaiFontEntry {
            name: format!("{family} {style}"),
            family: family.to_string(),
            style: style.to_string(),
            file_name: String::new(),
            path: std::path::PathBuf::new(),
        });
    }
    crate::shaping::invalidate_thai_cache();
    Ok(())
}

/// Load a custom Thai TrueType/OpenType font (.ttf/.otf) from owned bytes.
pub fn load_custom_thai_font(bytes: Vec<u8>) -> Result<&'static CraftFont, crate::GlyphError> {
    let (family, style) = match inspect_font_metadata(&bytes) {
        Some((fam, sty, true)) => (fam, sty),
        Some((_, _, false)) => return Err(crate::GlyphError::Missing),
        None => ("Thai Font".to_string(), "Regular".to_string()),
    };
    let font = load_custom_thai_font_named(bytes, family.clone(), style.clone())?;
    if let Ok(mut lock) = ACTIVE_THAI_FONT.write() {
        *lock = Some(font);
    }
    if let Ok(mut elock) = ACTIVE_THAI_ENTRY.write() {
        *elock = Some(ThaiFontEntry {
            name: format!("{family} {style}"),
            family,
            style,
            file_name: String::new(),
            path: std::path::PathBuf::new(),
        });
    }
    crate::shaping::invalidate_thai_cache();
    Ok(font)
}

/// Load a custom Thai TrueType font from a filesystem path (.ttf).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_custom_thai_font_file(path: impl AsRef<std::path::Path>) -> Result<&'static CraftFont, crate::GlyphError> {
    set_active_thai_font_by_path(path)
}

#[cfg(not(target_arch = "wasm32"))]
fn auto_init_thai_font() -> Option<&'static CraftFont> {
    // 1. Check environment variable override
    if let Some(p) = std::env::var_os("PDFCRAFT_THAI_FONT").or_else(|| std::env::var_os("PDFCRAFT_CUSTOM_FONT")) {
        let path = std::path::PathBuf::from(p);
        if path.is_file() {
            if let Ok(font) = set_active_thai_font_by_path(&path) {
                return Some(font);
            }
        }
    }

    // 2. Discover available fonts from font directories
    let list = available_thai_fonts();

    // Default to COURMT__.TTF (Courier MonoThai) as required
    if let Some(entry) = list.iter().find(|e| e.file_name.eq_ignore_ascii_case("COURMT__.TTF")) {
        if let Ok(font) = set_active_thai_font_entry(entry) {
            return Some(font);
        }
    }

    // Next try any Courier MonoThai
    if let Some(entry) = list.iter().find(|e| e.family.eq_ignore_ascii_case("Courier MonoThai")) {
        if let Ok(font) = set_active_thai_font_entry(entry) {
            return Some(font);
        }
    }

    // Next try the first discovered Thai font
    if let Some(entry) = list.first() {
        if let Ok(font) = set_active_thai_font_entry(entry) {
            return Some(font);
        }
    }

    // 3. Fallback to host system font
    discover_host_thai_font()
}

#[cfg(not(target_arch = "wasm32"))]
fn discover_host_thai_font() -> Option<&'static CraftFont> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("PDFCRAFT_THAI_FONT").or_else(|| std::env::var_os("PDFCRAFT_CUSTOM_FONT")) {
        candidates.push(std::path::PathBuf::from(p));
    }
    // Local workspace or app fonts directory:
    for base in [std::path::PathBuf::from("fonts"), std::path::PathBuf::from("../fonts")] {
        candidates.push(base.join("COURMT__.TTF"));
        candidates.push(base.join("THSarabunNew.ttf"));
        candidates.push(base.join("THSarabun.ttf"));
        candidates.push(base.join("Sarabun-Regular.ttf"));
    }
    #[cfg(windows)]
    {
        let win = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from)
            .join("Fonts");
        if let Some(user) = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from) {
            candidates.push(user.join(r"OneDrive\Documents\fonts\Sarabun-Regular.ttf"));
            candidates.push(user.join(r"OneDrive\Documents\fonts\THSarabunNew.ttf"));
            candidates.push(user.join(r"Documents\fonts\Sarabun-Regular.ttf"));
            candidates.push(user.join(r"Documents\fonts\THSarabunNew.ttf"));
            candidates.push(user.join(r"fonts\Sarabun-Regular.ttf"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) {
            candidates.push(local.join(r"Microsoft\Windows\Fonts\THSarabunNew.ttf"));
            candidates.push(local.join(r"Microsoft\Windows\Fonts\Sarabun-Regular.ttf"));
        }
        for file in [
            "COURMT__.TTF",
            "THSarabunNew.ttf",
            "THSarabun.ttf",
            "Sarabun-Regular.ttf",
            "LeelawUI.ttf",
            "LeelaUIb.ttf",
            "leelawad.ttf",
            "leelawdb.ttf",
            "tahoma.ttf",
            "angsana.ttc",
            "cordia.ttc",
        ] {
            candidates.push(win.join(file));
        }
    }
    #[cfg(target_os = "macos")]
    {
        for p in [
            "/System/Library/Fonts/Supplemental/Thonburi.ttc",
            "/System/Library/Fonts/Supplemental/Ayuthaya.ttf",
            "/System/Library/Fonts/Thonburi.ttc",
            "/System/Library/Fonts/Supplemental/Sathu.ttf",
            "/System/Library/Fonts/Supplemental/Silom.ttf",
            "/System/Library/Fonts/Supplemental/Krungthep.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
        ] {
            candidates.push(std::path::PathBuf::from(p));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for p in [
            "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansThai-Regular.ttf",
            "/usr/share/fonts/truetype/tlwg/Loma.ttf",
            "/usr/share/fonts/truetype/tlwg/Waree.ttf",
            "/usr/share/fonts/truetype/tlwg/Garuda.ttf",
            "/usr/share/fonts/truetype/thai/Sarabun-Regular.ttf",
        ] {
            candidates.push(std::path::PathBuf::from(p));
        }
    }

    for path in candidates {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if !meta.is_file() || meta.len() > (32 << 20) || meta.len() == 0 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if let Ok(font) = load_custom_thai_font(bytes) {
            return Some(font);
        }
    }
    None
}

/// Thai faces shared by the interface and PDF text writer.
/// Returns the active Thai font first, followed by any other loaded Thai fonts.
pub fn ui_thai_fonts() -> Vec<&'static CraftFont> {
    let mut fonts: Vec<&'static CraftFont> = CRAFT_FONTS.iter().filter(|f| f.covers("Thai")).collect();
    if fonts.is_empty() {
        if let Some(font) = document_thai_font() {
            fonts.push(font);
        }
        if let Ok(cache) = LOADED_THAI_CACHE.read() {
            if let Some(map) = cache.as_ref() {
                for font in map.values() {
                    if !fonts.iter().any(|f| std::ptr::eq(*f, *font)) {
                        fonts.push(*font);
                    }
                }
            }
        }
    }
    fonts
}

pub fn document_thai_font() -> Option<&'static CraftFont> {
    if let Some(f) = CRAFT_FONTS.iter().find(|f| f.covers("Thai")) {
        return Some(f);
    }
    if let Ok(lock) = ACTIVE_THAI_FONT.read() {
        if let Some(f) = *lock {
            return Some(f);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static INITIALIZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !INITIALIZING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let res = auto_init_thai_font();
            INITIALIZING.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(f) = res {
                return Some(f);
            }
        }
    }
    None
}

/// The `Hebr` craft-fonts faces for Hebrew interface text (file names, document titles, text
/// being edited), in manifest order. Empty when built without craft-fonts or when it has no
/// Hebrew face.
pub fn ui_hebrew_fonts() -> Vec<&'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.covers("Hebr")).collect()
}

/// The face for Hebrew text written into PDFs: the `Hebr` face of the asked weight, else its
/// Regular, else any `Hebr` face. `None` without craft-fonts or without a Hebrew face.
pub fn document_hebrew_font(bold: bool) -> Option<&'static CraftFont> {
    hebrew_face(CRAFT_FONTS, bold)
}

fn hebrew_face(faces: &[CraftFont], bold: bool) -> Option<&CraftFont> {
    let hebr = || faces.iter().filter(|f| f.covers("Hebr"));
    let style = if bold { "Bold" } else { "Regular" };
    hebr().find(|f| f.style == style).or_else(|| hebr().find(|f| f.style == "Regular")).or_else(|| hebr().next())
}

/// Interface CJK faces in fallback order for the UI language: Simplified Chinese first when
/// `prefer_hans`, otherwise Japanese first (the historical default).
///
/// The order matters beyond glyph shapes. egui renders each character with the FIRST face that
/// has its glyph, so with Japanese first a Simplified-only character (e.g. U+6B22 欢, absent
/// from Japanese faces) lands in a different face than its neighbours; the mixed vertical
/// metrics then sink it below the line. Preferring the UI language's face keeps one line in
/// one face with one baseline.
pub fn ui_cjk_fonts(prefer_hans: bool) -> Vec<&'static CraftFont> {
    order_cjk(CRAFT_FONTS.iter(), prefer_hans)
}

fn order_cjk<'a>(faces: impl IntoIterator<Item = &'a CraftFont>, prefer_hans: bool) -> Vec<&'a CraftFont> {
    let mut out: Vec<&'a CraftFont> = faces.into_iter().filter(|f| f.covers("Hans") || f.covers("Jpan")).collect();
    let first = if prefer_hans { "Hans" } else { "Jpan" };
    // The preferred script's faces first (a face covering both counts as preferred); within each
    // group the Japanese UI face (BIZ UDPGothic, not a serif) leads, otherwise manifest order
    // (the sort is stable).
    out.sort_by_key(|f| (!f.covers(first), f.family != "BIZ UDPGothic"));
    out
}

/// The face for Japanese text written into PDFs (serif document text): Shippori Mincho, then
/// BIZ UDMincho, then any other regular `Jpan` face. `None` without craft-fonts.
pub fn document_japanese_font() -> Option<&'static CraftFont> {
    document_face(CRAFT_FONTS, true, false)
}

/// A real Japanese document face matching serif/sans and weight where available.
/// Sans text prefers BIZ UDPGothic Bold or Regular. Missing weights fall back to Regular;
/// serif text keeps the document Mincho preference. No synthetic weight or slant is applied.
/// `None` without craft-fonts. The small web input currently has only Gothic Regular.
pub fn document_japanese_font_for_style(serif: bool, bold: bool) -> Option<&'static CraftFont> {
    document_face(CRAFT_FONTS, serif, bold)
}

/// Every `Jpan` face that can stand in for the document text, the best match for the style first
/// (the face [`document_japanese_font_for_style`] returns), then the others. Not every face has
/// every glyph (their coverage of e.g. Cyrillic differs), so a caller can move on to the next one.
/// Empty without craft-fonts.
pub fn document_japanese_fonts_for_style(serif: bool, bold: bool) -> Vec<&'static CraftFont> {
    document_faces(CRAFT_FONTS, serif, bold)
}

fn document_face(faces: &[CraftFont], serif: bool, bold: bool) -> Option<&CraftFont> {
    document_faces(faces, serif, bold).into_iter().next()
}

fn document_faces(faces: &[CraftFont], serif: bool, bold: bool) -> Vec<&CraftFont> {
    let jpan = || faces.iter().filter(|f| f.covers("Jpan"));
    let mut preferred = Vec::new();
    if !serif {
        let style = if bold { "Bold" } else { "Regular" };
        preferred.extend(jpan().find(|f| f.family == "BIZ UDPGothic" && f.style == style));
        preferred.extend(jpan().find(|f| f.family == "BIZ UDPGothic" && f.style == "Regular"));
    }
    for family in ["Shippori Mincho", "BIZ UDMincho"] {
        preferred.extend(jpan().find(|f| f.family == family && f.style == "Regular"));
    }
    let mut out: Vec<&CraftFont> = Vec::new();
    for face in preferred.into_iter().chain(jpan().filter(|f| f.style == "Regular")).chain(jpan()) {
        if !out.iter().any(|f| std::ptr::eq(*f, face)) {
            out.push(face);
        }
    }
    out
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn find(family: &str, style: &str) -> Option<&'static [u8]> {
    let mut i = 0;
    while i < CRAFT_FONTS.len() {
        let f = &CRAFT_FONTS[i];
        if str_eq(f.family, family) && str_eq(f.style, style) {
            return Some(f.bytes);
        }
        i += 1;
    }
    None
}

/// Shippori Mincho Regular from craft-fonts, the preferred face for Japanese document text.
/// `None` when PdfCraft was built without craft-fonts.
pub static SHIPPORI_MINCHO: Option<&[u8]> = find("Shippori Mincho", "Regular");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_fallback_order_follows_the_ui_language() {
        // Synthetic faces: order_cjk must not depend on the real build input.
        static BYTES: &[u8] = b"fake";
        static LATN: &[&str] = &["Latn"];
        static JPAN_LATN: &[&str] = &["Jpan", "Latn"];
        static HANS_LATN: &[&str] = &["Hans", "Latn"];
        let biz_bold = CraftFont { family: "BIZ UDPGothic", style: "Bold", scripts: JPAN_LATN, bytes: BYTES };
        let biz = CraftFont { family: "BIZ UDPGothic", style: "Regular", scripts: JPAN_LATN, bytes: BYTES };
        let hans = CraftFont { family: "FakeHans", style: "Regular", scripts: HANS_LATN, bytes: BYTES };
        let inter = CraftFont { family: "Inter", style: "Regular", scripts: LATN, bytes: BYTES };
        let faces = [biz_bold, biz, hans, inter];
        // Chinese mode: the Hans group first (manifest order), then Japanese.
        let zh: Vec<&str> = order_cjk([&faces[0], &faces[2], &faces[1]], true).iter().map(|f| f.family).collect();
        assert_eq!(zh, ["FakeHans", "BIZ UDPGothic", "BIZ UDPGothic"]);
        // Japanese mode keeps the historical default: the Japanese UI face first.
        let ja: Vec<&str> = order_cjk([&faces[0], &faces[2], &faces[1]], false).iter().map(|f| f.family).collect();
        assert_eq!(ja, ["BIZ UDPGothic", "BIZ UDPGothic", "FakeHans"]);
        // Latin-only faces are never interface CJK fallbacks.
        assert!(order_cjk(&faces, true).iter().all(|f| f.covers("Hans") || f.covers("Jpan")));
        // A serif listed first in the manifest never leads the Japanese group, in either mode.
        let mincho = CraftFont { family: "Shippori Mincho", style: "Regular", scripts: JPAN_LATN, bytes: BYTES };
        let zh: Vec<&str> = order_cjk([&mincho, &faces[2], &faces[1]], true).iter().map(|f| f.family).collect();
        assert_eq!(zh, ["FakeHans", "BIZ UDPGothic", "Shippori Mincho"]);
        let ja: Vec<&str> = order_cjk([&mincho, &faces[2], &faces[1]], false).iter().map(|f| f.family).collect();
        assert_eq!(ja, ["BIZ UDPGothic", "Shippori Mincho", "FakeHans"]);
    }

    #[test]
    fn document_faces_match_style_without_inventing_missing_weights() {
        let faces = [
            CraftFont { family: "Shippori Mincho", style: "Regular", scripts: &["Jpan"], bytes: b"serif" },
            CraftFont { family: "BIZ UDPGothic", style: "Regular", scripts: &["Jpan"], bytes: b"sans" },
            CraftFont { family: "BIZ UDPGothic", style: "Bold", scripts: &["Jpan"], bytes: b"bold" },
            CraftFont { family: "BIZ UDPGothic", style: "Bold", scripts: &["Latn"], bytes: b"not-japanese" },
        ];
        assert_eq!(document_face(&faces, false, false).unwrap().bytes, b"sans");
        assert_eq!(document_face(&faces, false, true).unwrap().bytes, b"bold");
        assert_eq!(document_face(&faces, true, false).unwrap().bytes, b"serif");
        assert_eq!(document_face(&faces, true, true).unwrap().bytes, b"serif");
        assert_eq!(document_face(&faces[..2], false, true).unwrap().bytes, b"sans");
        assert_eq!(document_face(&faces[..1], false, true).unwrap().bytes, b"serif");
        // Every Japanese face is a candidate, the style's match first and none twice.
        let order: Vec<&[u8]> = document_faces(&faces, false, true).iter().map(|f| f.bytes).collect();
        assert_eq!(order, [b"bold".as_slice(), b"sans", b"serif"]);
        let order: Vec<&[u8]> = document_faces(&faces, true, false).iter().map(|f| f.bytes).collect();
        assert_eq!(order, [b"serif".as_slice(), b"sans", b"bold"]);
        assert!(document_face(&faces[3..], false, true).is_none());
        assert!(document_face(&[], false, true).is_none());
    }

    #[test]
    fn arabic_faces_are_picked_by_script_in_manifest_order() {
        // Synthetic faces: the filter must not depend on the real build input.
        static BYTES: &[u8] = b"fake";
        static ARAB_LATN: &[&str] = &["Arab", "Latn"];
        static JPAN: &[&str] = &["Jpan"];
        let naskh = CraftFont { family: "FakeNaskh", style: "Regular", scripts: ARAB_LATN, bytes: BYTES };
        let biz = CraftFont { family: "BIZ UDPGothic", style: "Regular", scripts: JPAN, bytes: BYTES };
        let kufi = CraftFont { family: "FakeKufi", style: "Regular", scripts: ARAB_LATN, bytes: BYTES };
        let faces = [naskh, biz, kufi];
        let ar: Vec<&str> = arabic(&faces).iter().map(|f| f.family).collect();
        assert_eq!(ar, ["FakeNaskh", "FakeKufi"]);
        // An Arabic face is never a CJK fallback, and the other way round.
        assert!(order_cjk(&faces, false).iter().all(|f| f.family == "BIZ UDPGothic"));
        assert_eq!(ui_arabic_fonts().len(), CRAFT_FONTS.iter().filter(|f| f.covers("Arab")).count());
    }

    /// Family-name prefixes `build.rs` never embeds (its own `BARRED_FAMILIES`): Adobe's type
    /// designs, which includes Noto CJK (Source Han under another name).
    const BARRED_FAMILIES: [&str; 5] = ["Source Han", "Source Serif", "Source Sans", "Noto Sans CJK", "Noto Serif CJK"];

    /// Whatever the craft-fonts checkout holds, no Adobe type design is embedded (AGENTS.md §1.1).
    #[test]
    fn barred_families_are_never_embedded() {
        for face in CRAFT_FONTS {
            assert!(!BARRED_FAMILIES.iter().any(|barred| face.family.starts_with(barred)), "{} is embedded", face.name());
        }
        assert!(BARRED_FAMILIES.iter().any(|barred| "Noto Sans CJK SC".starts_with(barred)));
        assert!(!BARRED_FAMILIES.iter().any(|barred| "Noto Sans Arabic".starts_with(barred)));
    }

    #[test]
    fn hebrew_document_faces_follow_the_weight() {
        static HEBR_LATN: &[&str] = &["Hebr", "Latn"];
        let faces = [
            CraftFont { family: "BIZ UDPGothic", style: "Bold", scripts: &["Jpan"], bytes: b"not-hebrew" },
            CraftFont { family: "FakeHebrew", style: "Regular", scripts: HEBR_LATN, bytes: b"regular" },
            CraftFont { family: "FakeHebrew", style: "Bold", scripts: HEBR_LATN, bytes: b"bold" },
        ];
        assert_eq!(hebrew_face(&faces, false).map(|f| f.bytes), Some(&b"regular"[..]));
        assert_eq!(hebrew_face(&faces, true).map(|f| f.bytes), Some(&b"bold"[..]));
        // No bold face: Regular, never a face of another script.
        assert_eq!(hebrew_face(&faces[..2], true).map(|f| f.bytes), Some(&b"regular"[..]));
        assert!(hebrew_face(&faces[..1], false).is_none());
        assert_eq!(ui_hebrew_fonts().len(), CRAFT_FONTS.iter().filter(|f| f.covers("Hebr")).count());
        assert_eq!(document_hebrew_font(false).is_some(), CRAFT_FONTS.iter().any(|f| f.covers("Hebr")));
    }

    #[test]
    fn craft_fonts_are_optional_and_consistent() {
        // Holds with or without CRAFT_FONTS_DIR.
        assert_eq!(SHIPPORI_MINCHO.is_some(), CRAFT_FONTS.iter().any(|f| f.family == "Shippori Mincho" && f.style == "Regular"));
        assert_eq!(document_japanese_font().is_some(), CRAFT_FONTS.iter().any(|f| f.covers("Jpan")));
        let ui = ui_japanese_fonts();
        assert_eq!(ui.len(), CRAFT_FONTS.iter().filter(|f| f.covers("Jpan")).count());
        let ui_zh = ui_chinese_fonts();
        assert_eq!(ui_zh.len(), CRAFT_FONTS.iter().filter(|f| f.covers("Hans")).count());
        if CRAFT_FONTS.is_empty() {
            eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset): no Japanese faces, as expected");
            assert!(SHIPPORI_MINCHO.is_none() && ui.is_empty() && ui_zh.is_empty());
            return;
        }
        assert!(CRAFT_FONTS.iter().all(|f| !f.bytes.is_empty()));
        if CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic") {
            assert_eq!(ui.first().map(|f| f.family), Some("BIZ UDPGothic"));
        }
        assert_eq!(document_japanese_font().map(|f| f.family), Some("Shippori Mincho"));
    }

    #[test]
    fn thai_font_discovery_and_registration() {
        static FAKE_THAI_BYTES: &[u8] = b"fake-thai-font";
        let prev = ACTIVE_THAI_FONT.read().map(|l| *l).unwrap_or(None);
        assert!(register_custom_thai_font("Custom Thai Test", "Regular", FAKE_THAI_BYTES).is_ok());
        let thai = document_thai_font();
        assert!(thai.is_some());
        let f = thai.unwrap();
        assert_eq!(f.family, "Custom Thai Test");
        assert_eq!(f.bytes, FAKE_THAI_BYTES);
        assert!(ui_thai_fonts().iter().any(|f| f.family == "Custom Thai Test"));
        if let Ok(mut lock) = ACTIVE_THAI_FONT.write() {
            *lock = prev;
        }
        crate::shaping::invalidate_thai_cache();
    }

    #[test]
    fn thai_font_directory_loading_and_selection() {
        let dirs = font_directories();
        assert!(!dirs.is_empty(), "font_directories() should resolve candidates");

        let fonts = available_thai_fonts();
        if !fonts.is_empty() {
            // First font should be COURMT__.TTF (Courier MonoThai) if present in fonts folder
            if fonts.iter().any(|f| f.file_name.eq_ignore_ascii_case("COURMT__.TTF")) {
                assert_eq!(fonts[0].file_name.to_uppercase(), "COURMT__.TTF");
                assert_eq!(fonts[0].family, "Courier MonoThai");
            }
            // Auto-initialized font should be available
            let active = document_thai_font();
            assert!(active.is_some(), "document_thai_font() should auto-initialize");

            // Test switching font if multiple are available
            if let Some(other) = fonts.iter().find(|f| !f.file_name.eq_ignore_ascii_case("COURMT__.TTF")) {
                let switched = set_active_thai_font_by_name(&other.file_name);
                assert!(switched.is_ok(), "switching Thai font to {} should succeed", other.file_name);
                let current = active_thai_font_entry();
                assert_eq!(current.as_ref().map(|c| c.file_name.as_str()), Some(other.file_name.as_str()));
            }
        }
    }
}
