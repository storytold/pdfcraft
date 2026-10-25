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

/// Returns the active or monospace Thai font if available.
pub fn bundled_tlwg_typewriter_font() -> Option<&'static CraftFont> {
    let list = available_thai_fonts();
    if let Some(entry) = list.iter().find(|e| {
        e.family.eq_ignore_ascii_case("TlwgTypewriter")
            || e.family.eq_ignore_ascii_case("Tlwg Typewriter")
            || e.file_name.eq_ignore_ascii_case("TlwgTypewriter.ttf")
    })
        && let Ok(font) = set_active_thai_font_entry(entry)
    {
        return Some(font);
    }
    document_thai_font()
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
    /// Full display name (e.g. "TlwgTypewriter", "TH Sarabun New Bold").
    pub name: String,
    /// Family name from font metadata (e.g. "TlwgTypewriter", "TH Sarabun New").
    pub family: String,
    /// Subfamily / style from font metadata (e.g. "Regular", "Bold", "Italic").
    pub style: String,
    /// File name on disk (e.g. "TlwgTypewriter.ttf", "THSarabunNew.ttf").
    pub file_name: String,
    /// Path to the font file.
    pub path: std::path::PathBuf,
}

static ACTIVE_THAI_FONT: std::sync::RwLock<Option<&'static CraftFont>> = std::sync::RwLock::new(None);
static ACTIVE_THAI_ENTRY: std::sync::RwLock<Option<ThaiFontEntry>> = std::sync::RwLock::new(None);
static LOADED_THAI_CACHE: std::sync::RwLock<Option<std::collections::HashMap<std::path::PathBuf, &'static CraftFont>>> = std::sync::RwLock::new(None);
static DISCOVERED_THAI_FONTS: std::sync::RwLock<Option<Vec<ThaiFontEntry>>> = std::sync::RwLock::new(None);
static THAI_DISCOVERY_ATTEMPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn get_active_thai_font() -> Option<&'static CraftFont> {
    *ACTIVE_THAI_FONT.read().unwrap_or_else(|e| e.into_inner())
}

fn set_active_thai_font(font: Option<&'static CraftFont>) {
    *ACTIVE_THAI_FONT.write().unwrap_or_else(|e| e.into_inner()) = font;
}

fn get_active_thai_entry() -> Option<ThaiFontEntry> {
    ACTIVE_THAI_ENTRY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_active_thai_entry(entry: Option<ThaiFontEntry>) {
    *ACTIVE_THAI_ENTRY.write().unwrap_or_else(|e| e.into_inner()) = entry;
}

fn with_loaded_thai_cache_read<R>(f: impl FnOnce(&std::collections::HashMap<std::path::PathBuf, &'static CraftFont>) -> R) -> Option<R> {
    let lock = LOADED_THAI_CACHE.read().unwrap_or_else(|e| e.into_inner());
    lock.as_ref().map(f)
}

fn with_loaded_thai_cache_write(f: impl FnOnce(&mut std::collections::HashMap<std::path::PathBuf, &'static CraftFont>)) {
    let mut lock = LOADED_THAI_CACHE.write().unwrap_or_else(|e| e.into_inner());
    let map = lock.get_or_insert_with(std::collections::HashMap::new);
    f(map);
}

fn get_discovered_thai_fonts() -> Option<Vec<ThaiFontEntry>> {
    DISCOVERED_THAI_FONTS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_discovered_thai_fonts(list: Vec<ThaiFontEntry>) {
    *DISCOVERED_THAI_FONTS.write().unwrap_or_else(|e| e.into_inner()) = Some(list);
}

/// Candidate directories where font files may reside.
/// Checks runtime environment variables, executable-relative paths, working directory paths,
/// and manifest directories to ensure robust discovery across dev and packaged distributions.
pub fn font_directories() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();

    // 1. Explicit environment variable overrides
    if let Some(env_dir) =
        std::env::var_os("PDFCRAFT_FONTS_DIR").or_else(|| std::env::var_os("PDFCRAFT_THAI_FONTS_DIR")).or_else(|| std::env::var_os("CRAFT_FONTS_DIR"))
    {
        dirs.push(std::path::PathBuf::from(env_dir));
    }

    // 2. Executable-relative paths (next to executable, or in ancestor directories for target/debug builds)
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("assets").join("fonts"));
        if let Some(grand) = parent.parent() {
            dirs.push(grand.join("assets").join("fonts"));
            if let Some(great) = grand.parent() {
                dirs.push(great.join("assets").join("fonts"));
            }
        }
    }

    // 3. Current working directory and relative paths
    for rel in ["assets/fonts", "../assets/fonts"] {
        dirs.push(std::path::PathBuf::from(rel));
    }

    // 4. Manifest directory at compile/build time (for cargo test and local dev execution)
    let manifest = env!("CARGO_MANIFEST_DIR");
    dirs.push(std::path::Path::new(manifest).join("../../assets/fonts"));
    dirs.push(std::path::Path::new(manifest).join("../assets/fonts"));

    // 5. System font directories across platforms
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from);
        dirs.push(windir.join("Fonts"));

        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(std::path::PathBuf::from(local_app_data).join(r"Microsoft\Windows\Fonts"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        dirs.push(std::path::PathBuf::from("/Library/Fonts"));
        dirs.push(std::path::PathBuf::from("/System/Library/Fonts"));
        dirs.push(std::path::PathBuf::from("/System/Library/Fonts/Supplemental"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(std::path::PathBuf::from(home).join("Library/Fonts"));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for base in ["/usr/share/fonts", "/usr/local/share/fonts"] {
            dirs.push(std::path::PathBuf::from(base));
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(std::path::PathBuf::from(home).join(".local/share/fonts"));
        }
    }

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

fn scan_dir_recursive(dir: &std::path::Path, depth: usize, entries: &mut Vec<ThaiFontEntry>) {
    if depth > 2 {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else { return };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 2 {
                scan_dir_recursive(&path, depth + 1, entries);
            }
            continue;
        }
        let is_font_ext =
            path.extension().and_then(|e| e.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf"));
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
        let name = if style.eq_ignore_ascii_case("Regular") { family.clone() } else { format!("{family} {style}") };
        entries.push(ThaiFontEntry { name, family, style, file_name, path });
    }
}

fn scan_thai_fonts_from_dirs() -> Vec<ThaiFontEntry> {
    let mut entries = Vec::new();
    let dirs = font_directories();

    for dir in dirs {
        scan_dir_recursive(&dir, 0, &mut entries);
    }

    // Sort entries:
    // 1. Sarabun Regular (bundled default font)
    // 2. TlwgTypewriter.ttf (TLWG Typewriter monospace)
    // 3. Other monospace styles
    // 4. Alphabetical by family, then style (Regular before Bold/Italic)
    entries.sort_by(|a, b| {
        let a_is_sarabun = a.family.eq_ignore_ascii_case("Sarabun") && a.style.eq_ignore_ascii_case("Regular");
        let b_is_sarabun = b.family.eq_ignore_ascii_case("Sarabun") && b.style.eq_ignore_ascii_case("Regular");
        if a_is_sarabun != b_is_sarabun {
            return b_is_sarabun.cmp(&a_is_sarabun);
        }
        let a_is_default = a.file_name.eq_ignore_ascii_case("TlwgTypewriter.ttf");
        let b_is_default = b.file_name.eq_ignore_ascii_case("TlwgTypewriter.ttf");
        if a_is_default != b_is_default {
            return b_is_default.cmp(&a_is_default);
        }
        let a_is_mono = a.family.eq_ignore_ascii_case("TlwgTypewriter")
            || a.family.eq_ignore_ascii_case("Tlwg Typewriter")
            || a.family.contains("Mono")
            || a.family.contains("Typewriter");
        let b_is_mono = b.family.eq_ignore_ascii_case("TlwgTypewriter")
            || b.family.eq_ignore_ascii_case("Tlwg Typewriter")
            || b.family.contains("Mono")
            || b.family.contains("Typewriter");
        if a_is_mono != b_is_mono {
            return b_is_mono.cmp(&a_is_mono);
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
/// Results are sorted with `Sarabun-Regular.ttf` as the default first option, followed by `TlwgTypewriter.ttf`.
pub fn available_thai_fonts() -> Vec<ThaiFontEntry> {
    if let Some(cached) = get_discovered_thai_fonts() {
        return cached;
    }
    let list = scan_thai_fonts_from_dirs();
    set_discovered_thai_fonts(list.clone());
    list
}

/// Force a rescan of font directories (useful if new fonts were added to the directory).
pub fn rescan_thai_fonts() -> Vec<ThaiFontEntry> {
    #[cfg(not(target_arch = "wasm32"))]
    THAI_DISCOVERY_ATTEMPTED.store(false, std::sync::atomic::Ordering::Relaxed);
    let list = scan_thai_fonts_from_dirs();
    set_discovered_thai_fonts(list.clone());
    list
}

/// Load and leak font bytes with specific family and style names into a `'static CraftFont`.
pub fn load_custom_thai_font_named(bytes: Vec<u8>, family: String, style: String) -> Result<&'static CraftFont, crate::GlyphError> {
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
    let font = Box::leak(Box::new(CraftFont { family: family_str, style: style_str, scripts: &["Thai"], bytes: leaked_bytes }));
    Ok(font)
}

/// Set the active Thai font using a `ThaiFontEntry`.
pub fn set_active_thai_font_entry(entry: &ThaiFontEntry) -> Result<&'static CraftFont, crate::GlyphError> {
    if let Some(Some(font)) = with_loaded_thai_cache_read(|map| map.get(&entry.path).copied()) {
        set_active_thai_font(Some(font));
        set_active_thai_entry(Some(entry.clone()));
        crate::shaping::invalidate_thai_cache();
        return Ok(font);
    }

    let bytes = std::fs::read(&entry.path).map_err(|_| crate::GlyphError::NoFont)?;
    let font = load_custom_thai_font_named(bytes, entry.family.clone(), entry.style.clone())?;

    with_loaded_thai_cache_write(|map| {
        map.insert(entry.path.clone(), font);
    });
    set_active_thai_font(Some(font));
    set_active_thai_entry(Some(entry.clone()));
    crate::shaping::invalidate_thai_cache();
    Ok(font)
}

/// Set active Thai font by matching font name, family, or file name (case-insensitive).
pub fn set_active_thai_font_by_name(name_or_file: &str) -> Result<&'static CraftFont, crate::GlyphError> {
    let list = available_thai_fonts();
    let lower = name_or_file.to_lowercase();
    let entry = list
        .iter()
        .find(|e| e.file_name.to_lowercase() == lower || e.name.to_lowercase() == lower || e.family.to_lowercase() == lower)
        .or_else(|| {
            list.iter().find(|e| {
                e.file_name.to_lowercase().contains(&lower) || e.name.to_lowercase().contains(&lower) || e.family.to_lowercase().contains(&lower)
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
    let name = if style.eq_ignore_ascii_case("Regular") { family.clone() } else { format!("{family} {style}") };
    let entry = ThaiFontEntry { name, family, style, file_name, path: path_ref.to_path_buf() };
    set_active_thai_font_entry(&entry)
}

/// Get the metadata of the currently active Thai font (auto-initializing if needed).
pub fn active_thai_font_entry() -> Option<ThaiFontEntry> {
    if let Some(entry) = get_active_thai_entry() {
        return Some(entry);
    }
    let _ = document_thai_font();
    get_active_thai_entry()
}

/// Get the currently active Thai font object (equivalent to [`document_thai_font`]).
pub fn active_thai_font() -> Option<&'static CraftFont> {
    document_thai_font()
}

/// Register a custom Thai font from static slices.
pub fn register_custom_thai_font(family: &'static str, style: &'static str, bytes: &'static [u8]) -> Result<(), crate::GlyphError> {
    let font = Box::leak(Box::new(CraftFont { family, style, scripts: &["Thai"], bytes }));
    set_active_thai_font(Some(font));
    set_active_thai_entry(Some(ThaiFontEntry {
        name: format!("{family} {style}"),
        family: family.to_string(),
        style: style.to_string(),
        file_name: String::new(),
        path: std::path::PathBuf::new(),
    }));
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
    set_active_thai_font(Some(font));
    set_active_thai_entry(Some(ThaiFontEntry {
        name: format!("{family} {style}"),
        family,
        style,
        file_name: String::new(),
        path: std::path::PathBuf::new(),
    }));
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
        if path.is_file()
            && let Ok(font) = set_active_thai_font_by_path(&path)
        {
            return Some(font);
        }
    }

    // 2. Discover available fonts from font directories
    let list = available_thai_fonts();

    // Default to Sarabun (OFL-1.1 bundled font)
    if let Some(entry) = list.iter().find(|e| e.family.eq_ignore_ascii_case("Sarabun") && e.style.eq_ignore_ascii_case("Regular"))
        && let Ok(font) = set_active_thai_font_entry(entry)
    {
        return Some(font);
    }

    // Next try TlwgTypewriter.ttf as monospace option
    if let Some(entry) = list.iter().find(|e| e.file_name.eq_ignore_ascii_case("TlwgTypewriter.ttf"))
        && let Ok(font) = set_active_thai_font_entry(entry)
    {
        return Some(font);
    }

    // Next try any TlwgTypewriter
    if let Some(entry) = list.iter().find(|e| e.family.eq_ignore_ascii_case("TlwgTypewriter") || e.family.eq_ignore_ascii_case("Tlwg Typewriter"))
        && let Ok(font) = set_active_thai_font_entry(entry)
    {
        return Some(font);
    }

    // Next try each discovered Thai font until one loads successfully
    for entry in &list {
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
    for base in [std::path::PathBuf::from("assets/fonts"), std::path::PathBuf::from("../assets/fonts")] {
        candidates.push(base.join("Sarabun-Regular.ttf"));
        candidates.push(base.join("TlwgTypewriter.ttf"));
        candidates.push(base.join("THSarabunNew.ttf"));
        candidates.push(base.join("THSarabun.ttf"));
    }
    #[cfg(windows)]
    {
        let win = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from)
            .join("Fonts");
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) {
            candidates.push(local.join(r"Microsoft\Windows\Fonts\THSarabunNew.ttf"));
            candidates.push(local.join(r"Microsoft\Windows\Fonts\Sarabun-Regular.ttf"));
            candidates.push(local.join(r"Microsoft\Windows\Fonts\TlwgTypewriter.ttf"));
        }
        for file in [
            "THSarabunNew.ttf",
            "THSarabun.ttf",
            "Sarabun-Regular.ttf",
            "TlwgTypewriter.ttf",
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
        with_loaded_thai_cache_read(|map| {
            for font in map.values() {
                if !fonts.iter().any(|f| std::ptr::eq(*f, *font)) {
                    fonts.push(*font);
                }
            }
        });
    }
    fonts
}

pub fn document_thai_font() -> Option<&'static CraftFont> {
    if let Some(f) = get_active_thai_font() {
        return Some(f);
    }
    if let Some(f) = CRAFT_FONTS.iter().find(|f| f.covers("Thai")) {
        return Some(f);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if THAI_DISCOVERY_ATTEMPTED.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        static INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _init_guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = get_active_thai_font() {
            return Some(f);
        }
        if THAI_DISCOVERY_ATTEMPTED.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        if let Some(f) = auto_init_thai_font() {
            return Some(f);
        }
        THAI_DISCOVERY_ATTEMPTED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    None
}

/// The `Deva` craft-fonts faces for Devanagari interface text (the Hindi catalog, file names,
/// document titles), in manifest order. Empty when built without craft-fonts or when its revision
/// has no Devanagari face; a desktop build then falls back to one font already installed on the
/// machine (`SYSTEM_DEVANAGARI_FALLBACK`), and the web build shows replacement boxes.
pub fn ui_devanagari_fonts() -> Vec<&'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.covers("Deva")).collect()
}

/// The face for Devanagari text written into PDFs: the first `Deva` face. `None` when built
/// without craft-fonts or when its revision has no Devanagari face.
pub fn document_devanagari_font() -> Option<&'static CraftFont> {
    ui_devanagari_fonts().into_iter().next()
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

/// The scripts the bundled Inter faces are for.
static LATIN_SCRIPTS: &[&str] = &["Latn", "Cyrl", "Grek"];

/// The bundled Latin document faces (`assets/fonts`, OFL): every build has them, unlike
/// craft-fonts. The Japanese faces carry only part of the Cyrillic (`привет` yes, Uzbek `қизил`
/// and Kazakh `әже` no), so letters like қ, ғ, ҳ, ә, ө, ү, ұ and һ have no face at all without
/// these.
static INTER_REGULAR_FACE: CraftFont =
    CraftFont { family: "Inter", style: "Regular", scripts: LATIN_SCRIPTS, bytes: include_bytes!("../../../assets/fonts/Inter-Regular.ttf") };
static INTER_SEMIBOLD_FACE: CraftFont =
    CraftFont { family: "Inter", style: "SemiBold", scripts: LATIN_SCRIPTS, bytes: include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf") };

/// The face for Latin, Greek and Cyrillic text written into PDFs: the bundled Inter Regular, or
/// SemiBold for bold text. Present in every build, crafted ones included.
pub fn document_latin_fonts(bold: bool) -> Vec<&'static CraftFont> {
    vec![if bold { &INTER_SEMIBOLD_FACE } else { &INTER_REGULAR_FACE }]
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
pub(crate) static THAI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// `build.rs` embeds the Japanese CJK faces whole and any other CJK face only up to
    /// `MAX_OTHER_CJK_BYTES`, so a checkout with a large Chinese or Korean face (craft-fonts has the
    /// 16.4 MB Noto Sans CJK SC) leaves it out. The four Japanese faces of the pinned craft-fonts
    /// are 24.1 MB.
    #[test]
    fn cjk_faces_follow_the_size_rule() {
        use crate::cjk_scripts::{MAX_OTHER_CJK_BYTES, WHOLE_CJK_FAMILIES, is_cjk_script};
        // A tag can carry a region after the code: craft-fonts tags Noto Sans HK UI `Hant-HK`.
        assert!(["Hant-HK", "Hans", "Jpan", "Hang"].iter().all(|s| is_cjk_script(s)));
        assert!(!["Latn", "Arab", "Hebr"].iter().any(|s| is_cjk_script(s)));
        let cjk = CRAFT_FONTS.iter().filter(|f| f.scripts.iter().any(|s| is_cjk_script(s)));
        let (whole, other): (Vec<_>, Vec<_>) = cjk.partition(|f| WHOLE_CJK_FAMILIES.contains(&f.family));
        for face in &other {
            let len = face.bytes.len();
            assert!(u64::try_from(len).is_ok_and(|n| n <= MAX_OTHER_CJK_BYTES), "{} is embedded at {len} bytes", face.name());
        }
        let bytes: usize = whole.iter().map(|f| f.bytes.len()).sum();
        assert!(bytes < 25_000_000, "{bytes} bytes of Japanese faces");
    }

    /// The bundled Inter faces carry the Latin, Greek and Cyrillic letters the craft-fonts faces
    /// lack: Uzbek қ ғ ҳ, Kazakh ә ө ү ұ һ, Ukrainian ї є ґ, Greek. This one runs with or without
    /// CRAFT_FONTS_DIR (#2593).
    #[test]
    fn latin_faces_carry_the_letters_the_japanese_faces_lack() {
        for face in document_latin_fonts(false).into_iter().chain(document_latin_fonts(true)) {
            for ch in ['қ', 'ғ', 'ҳ', 'ә', 'ө', 'ү', 'ұ', 'һ', 'ї', 'є', 'ґ', 'ё', 'α', 'β', 'γ', '≥', '≈'] {
                assert!(crate::japanese_glyph_from(face, ch).is_ok(), "{} has no {ch}", face.name());
            }
        }
        assert_eq!(document_latin_fonts(false)[0].style, "Regular");
        assert_eq!(document_latin_fonts(true)[0].style, "SemiBold");
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
        let _guard = THAI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        static FAKE_THAI_BYTES: &[u8] = b"fake-thai-font";
        let prev_font = get_active_thai_font();
        let prev_entry = get_active_thai_entry();
        assert!(register_custom_thai_font("Custom Thai Test", "Regular", FAKE_THAI_BYTES).is_ok());
        let thai = document_thai_font();
        assert!(thai.is_some());
        let f = thai.unwrap();
        assert_eq!(f.family, "Custom Thai Test");
        assert_eq!(f.bytes, FAKE_THAI_BYTES);
        assert!(ui_thai_fonts().iter().any(|f| f.family == "Custom Thai Test"));
        set_active_thai_font(prev_font);
        set_active_thai_entry(prev_entry);
        crate::shaping::invalidate_thai_cache();
    }

    #[test]
    fn thai_font_directory_loading_and_selection() {
        let _guard = THAI_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dirs = font_directories();
        assert!(!dirs.is_empty(), "font_directories() should resolve candidates");

        let fonts = available_thai_fonts();
        if !fonts.is_empty() {
            // Auto-initialized font should be available
            let active = document_thai_font();
            assert!(active.is_some(), "document_thai_font() should auto-initialize");

            // Test switching font if multiple are available
            if fonts.len() > 1 {
                let prior_font = active;
                let prior_entry = active_thai_font_entry();
                let other = &fonts[1];
                let switched = set_active_thai_font_by_name(&other.file_name);
                assert!(switched.is_ok(), "switching Thai font to {} should succeed", other.file_name);
                let current = active_thai_font_entry();
                assert_eq!(current.as_ref().map(|c| c.file_name.as_str()), Some(other.file_name.as_str()));
                set_active_thai_font(prior_font);
                set_active_thai_entry(prior_entry);
                crate::shaping::invalidate_thai_cache();
            }
        }
    }
}
