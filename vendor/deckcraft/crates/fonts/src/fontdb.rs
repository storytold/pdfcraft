//! Font database: bundled OFL fonts, user fonts, the installed system fonts (cataloged the first
//! time a lookup needs them, native only), outline cache.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use kurbo::BezPath;
use skrifa::instance::{Location, LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::FileRef;
use skrifa::string::StringId;
use skrifa::{GlyphId, MetadataProvider};

/// The family used when a requested family is unknown and has no substitute.
pub const FALLBACK_FAMILY: &str = "Inter";

/// The always-available last-resort face (Ubuntu Light, compiled in through
/// `epaint_default_fonts`; licence in ATTRIBUTION.md).
pub const LAST_RESORT_FAMILY: &str = "Ubuntu";

/// Compiled-in font files: the craft-fonts faces (Latin presentation fonts, then Japanese;
/// empty without `CRAFT_FONTS_DIR`), then the last-resort face.
pub fn bundled() -> Vec<&'static [u8]> {
    let mut v: Vec<&'static [u8]> = crate::CRAFT_FONTS.iter().map(|f| f.bytes).collect();
    v.push(epaint_default_fonts::UBUNTU_LIGHT);
    v
}

/// Open substitutes for families documents often ask for but we can't ship (metric-compatible
/// where one exists), tried in order after the family itself.
pub fn substitutes(family: &str) -> &'static [&'static str] {
    let f = family.to_ascii_lowercase();
    if is_monospace_family(&f) {
        return &["Liberation Mono", "Courier New", "Menlo", "Courier", "DejaVu Sans Mono"];
    }
    match f.as_str() {
        "calibri" | "calibri light" => &["Carlito", "Liberation Sans", "Arial", "Inter"],
        "cambria" | "cambria math" => &["Caladea", "Liberation Serif", "Times New Roman", "Merriweather"],
        "arial" | "helvetica" | "helvetica neue" | "arial nova" => &["Liberation Sans", "Arial", "Helvetica Neue", "Helvetica", "Inter"],
        "times new roman" | "times" => &["Liberation Serif", "Times New Roman", "Times", "Merriweather"],
        "aptos" | "aptos display" | "aptos narrow" | "segoe ui" | "segoe ui light" | "segoe ui semibold" | "tahoma" | "verdana" => {
            &["Inter", "Helvetica Neue", "Arial", "Liberation Sans"]
        }
        "georgia" | "garamond" | "book antiqua" | "palatino" | "palatino linotype" | "constantia" => &["Merriweather", "Liberation Serif", "Georgia"],
        "century gothic" | "avenir" | "avenir next" | "futura" | "gill sans" | "gill sans mt" | "trebuchet ms" | "corbel" | "candara" => {
            &["Montserrat", "Poppins", "Avenir Next", "Inter"]
        }
        "franklin gothic" | "franklin gothic book" | "franklin gothic medium" | "univers" => &["Open Sans", "Inter"],
        "open sans" | "lato" | "roboto" | "montserrat" | "poppins" | "nunito sans" => &["Inter"],
        "playfair display" | "merriweather" | "source serif 4" | "source serif pro" => &["Merriweather", "Liberation Serif", "Georgia"],
        "source sans 3" | "source sans pro" => &["Open Sans", "Inter"],
        "impact" | "arial black" => &["Montserrat", "Inter"],
        _ => &[],
    }
}

/// Families that are monospaced: when not installed, documents keep a monospaced substitute
/// ([`substitutes`]) so code blocks and columns stay aligned instead of falling back to
/// proportional Calibri.
pub fn is_monospace_family(family: &str) -> bool {
    let f = family.to_ascii_lowercase();
    let s = f.trim();
    if matches!(
        s,
        "courier new"
            | "courier"
            | "cousine"
            | "consolas"
            | "menlo"
            | "monaco"
            | "source code pro"
            | "jetbrains mono"
            | "cascadia code"
            | "cascadia code pl"
            | "cascadia mono"
            | "cascadia mono pl"
            | "fira code"
            | "fira mono"
            | "inconsolata"
            | "dejavu sans mono"
            | "sf mono"
            | "roboto mono"
            | "hack"
            | "ubuntu mono"
            | "pt mono"
            | "liberation mono"
            | "andale mono"
            | "lucida console"
            | "lucida sans typewriter"
            | "fixedsys"
            | "terminal"
            | "monospace"
            | "iosevka"
            | "monolisa"
            | "envy code r"
            | "hasklig"
    ) {
        return true;
    }
    s.starts_with("courier")
        || s.starts_with("consolas")
        || s.starts_with("inconsolata")
        || s.starts_with("iosevka")
        || s.ends_with(" mono")
        || s.ends_with(" code")
        || s.ends_with(" console")
        || s.ends_with(" typewriter")
        || s.ends_with(" monospace")
        || s.split_whitespace().any(|w| w == "mono" || w == "terminal" || w == "monospace")
}

/// Families whose open substitutes ([`substitutes`]) keep their metrics, so text drawn with
/// them breaks into the same lines.
fn has_metric_twin(family: &str) -> bool {
    matches!(
        family.to_ascii_lowercase().as_str(),
        "calibri"
            | "calibri light"
            | "cambria"
            | "cambria math"
            | "arial"
            | "helvetica"
            | "times new roman"
            | "times"
            | "courier new"
            | "courier"
            | "cousine"
    )
}

/// The installed family PowerPoint draws a missing `family` with instead of Calibri: Liberation
/// Sans and Serif as Arial and Times New Roman (observed in PowerPoint; Liberation Mono, Carlito,
/// Caladea, Arimo and Tinos are not mapped and draw in Calibri), and Windows' standard font
/// substitutes (Arial CE, Times New Roman CYR…).
fn office_twin(family: &str) -> Option<&'static str> {
    let f = family.to_ascii_lowercase();
    Some(match f.as_str() {
        "liberation sans" | "helvetica" | "helv" => "Arial",
        "liberation serif" | "times" | "tms rmn" => "Times New Roman",
        _ => {
            let base = f
                .strip_suffix(" baltic")
                .or(f.strip_suffix(" ce"))
                .or(f.strip_suffix(" cyr"))
                .or(f.strip_suffix(" greek"))
                .or(f.strip_suffix(" tur"))?;
            match base {
                "arial" => "Arial",
                "courier new" => "Courier New",
                "times new roman" => "Times New Roman",
                _ => return None,
            }
        }
    })
}

/// Office's downloadable (cloud) families: PowerPoint fetches them when a document uses them, so
/// they draw as themselves there, not as Calibri. Without them DeckCraft keeps their closest open
/// substitute ([`substitutes`]; Aptos as Inter).
fn is_office_cloud_font(family: &str) -> bool {
    let f = family.to_ascii_lowercase();
    f.starts_with("aptos") || ["bierstadt", "grandview", "seaford", "skeena", "tenorite"].iter().any(|c| f.starts_with(c))
}

/// The family PowerPoint draws a document's Latin text in when the font it names isn't installed,
/// whatever the theme, `panose` or `pitchFamily` say.
pub const MISSING_FONT_FAMILY: &str = "Calibri";

#[derive(Clone)]
enum FontBytes {
    Static(&'static [u8]),
    Owned(Arc<Vec<u8>>),
}

/// One loaded font face.
pub struct FontFace {
    id: u32,
    /// Typographic family name (e.g. "Source Sans 3").
    pub family: String,
    /// Typographic style name (e.g. "Semibold", "Italic").
    pub style: String,
    /// The Windows (GDI) family and style names (`name` IDs 1 and 2) when the family differs from
    /// the typographic one: "Segoe UI Semibold" / "Regular" for Segoe UI Semibold. Documents name
    /// fonts this way.
    pub legacy: Option<(String, String)>,
    /// usWeightClass-style weight (400 = regular).
    pub weight: f32,
    pub italic: bool,
    bytes: FontBytes,
    index: u32,
    pub upem: f64,
    /// Ascender in font units (positive = up).
    pub ascent: f64,
    /// Descender in font units (positive = down).
    pub descent: f64,
    /// Cap height and x height in font units (estimated from the ascent when the font has no OS/2
    /// values).
    pub cap_height: f64,
    pub x_height: f64,
    /// Where the baseline sits in a line, as a share of the line's height from its top: the
    /// Windows ascent over the Windows ascent plus descent (OS/2 `usWinAscent`/`usWinDescent`),
    /// the split PowerPoint uses (computed by `line_split`).
    pub line_split: f64,
    /// Underline position (top edge, below the baseline) and thickness in font units (`post`
    /// table; 0.1 and 0.05 em when the font has none). PowerPoint draws single underlines there.
    pub underline: (f64, f64),
    /// Strikethrough position (top edge, above the baseline) and thickness in font units (OS/2;
    /// 0.26 and 0.05 em when the font has none). PowerPoint draws single strikethroughs there.
    pub strikeout: (f64, f64),
    pub shaper: harfrust::ShaperData,
    /// Variable fonts: the named instance's axis settings (user units; empty = default instance),
    /// the normalised location and the shaper's view of it.
    pub coords: Vec<([u8; 4], f32)>,
    location: Location,
    pub instance: Option<harfrust::ShaperInstance>,
    /// Basic Multilingual Plane coverage bitset, built on first use.
    bmp: std::sync::OnceLock<Box<[u64]>>,
    /// Colour glyph tables present (`is_color`), looked up on first use.
    color: std::sync::OnceLock<bool>,
}

/// A cheap `Copy` handle to a face. Faces are never unloaded, so the handle lives for the rest of
/// the process; unlike cloning an `Arc`, copying it touches no shared reference count (which made
/// parallel composition scale negatively).
#[derive(Clone, Copy)]
pub struct FaceRef(&'static FontFace);

impl FaceRef {
    /// The handle for a loaded face (memoised per face).
    pub fn of(face: &Arc<FontFace>) -> FaceRef {
        static LEAKED: RwLock<Vec<Option<&FontFace>>> = RwLock::new(Vec::new());
        let id = face.id as usize;
        if let Some(Some(f)) = LEAKED.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(id) {
            return FaceRef(f);
        }
        let mut w = LEAKED.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.len() <= id {
            w.resize(id + 1, None);
        }
        let f: &'static FontFace = w[id].unwrap_or_else(|| {
            let keep: &'static Arc<FontFace> = Box::leak(Box::new(face.clone()));
            keep
        });
        w[id] = Some(f);
        FaceRef(f)
    }
    pub fn get(self) -> &'static FontFace {
        self.0
    }
}

impl std::ops::Deref for FaceRef {
    type Target = FontFace;
    fn deref(&self) -> &FontFace {
        self.0
    }
}

impl std::fmt::Debug for FaceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl PartialEq for FaceRef {
    fn eq(&self, o: &FaceRef) -> bool {
        self.0.id == o.0.id
    }
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontFace({} {})", self.family, self.style)
    }
}

impl FontFace {
    pub fn data(&self) -> &[u8] {
        match &self.bytes {
            FontBytes::Static(b) => b,
            FontBytes::Owned(v) => v.as_slice(),
        }
    }
    pub fn skrifa(&self) -> Option<skrifa::FontRef<'_>> {
        skrifa::FontRef::from_index(self.data(), self.index).ok()
    }
    /// Does the face draw colour glyphs (a `COLR`, `CBDT`, `sbix` or `SVG ` table, as colour emoji
    /// fonts have)?
    pub fn is_color(&self) -> bool {
        *self.color.get_or_init(|| has_color_tables(self.data(), self.index))
    }
    pub fn hb(&self) -> Option<harfrust::FontRef<'_>> {
        harfrust::FontRef::from_index(self.data(), self.index).ok()
    }
    /// Where in the design space glyphs, metrics and outlines come from.
    pub fn location(&self) -> LocationRef<'_> {
        (&self.location).into()
    }
    /// Is this a named instance of a variable font?
    pub fn is_variable(&self) -> bool {
        !self.coords.is_empty()
    }
    /// Face index within the font file (collections); 0 for plain fonts.
    pub fn index(&self) -> u32 {
        self.index
    }
    /// Unique id of this face within the process.
    pub fn id(&self) -> u32 {
        self.id
    }
    /// Does the face map `c` to a glyph?
    pub fn covers(&self, c: char) -> bool {
        let cp = c as u32;
        if cp < 0x1_0000 {
            let bits = self.bmp.get_or_init(|| {
                let mut b = vec![0u64; 1024].into_boxed_slice();
                if let Some(f) = self.skrifa() {
                    for (cp, _) in f.charmap().mappings() {
                        if cp < 0x1_0000 {
                            b[(cp / 64) as usize] |= 1 << (cp % 64);
                        }
                    }
                }
                b
            });
            return bits[(cp / 64) as usize] & (1 << (cp % 64)) != 0;
        }
        self.skrifa().is_some_and(|f| f.charmap().map(c).is_some())
    }
    /// Units per em.
    pub fn units_per_em(&self) -> f64 {
        self.upem
    }
    /// (ascent, descent) in font units, both positive.
    pub fn vertical_metrics(&self) -> (f64, f64) {
        (self.ascent, self.descent)
    }
    /// Every mapped character and its glyph id, sorted by code point (the Glyphs panel).
    pub fn chars(&self) -> Vec<(char, u32)> {
        let Some(f) = self.skrifa() else { return vec![] };
        let mut v: Vec<(char, u32)> = f.charmap().mappings().filter_map(|(cp, g)| char::from_u32(cp).map(|c| (c, g.to_u32()))).collect();
        v.sort_unstable_by_key(|x| x.0);
        v.dedup_by_key(|x| x.0);
        v
    }
    /// Advance width of glyph `gid` in font units.
    pub fn advance(&self, gid: u32) -> f64 {
        self.skrifa()
            .and_then(|f| f.glyph_metrics(Size::unscaled(), self.location()).advance_width(GlyphId::new(gid)))
            .map(|a| a as f64)
            .unwrap_or(self.upem * 0.5)
    }
    /// Glyph id for `c` (0 = .notdef).
    pub fn glyph_for(&self, c: char) -> u32 {
        self.skrifa().and_then(|f| f.charmap().map(c)).map(|g| g.to_u32()).unwrap_or(0)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
struct CatalogEntry {
    family: String,
    style: String,
    path: std::path::PathBuf,
}

/// Process-wide font database.
pub struct FontDb {
    faces: RwLock<Vec<Arc<FontFace>>>,
    outlines: Mutex<OutlineCache>,
    #[cfg(not(target_arch = "wasm32"))]
    catalog: RwLock<Vec<CatalogEntry>>,
    /// The folders the system font scan reads (the platform's font folders for [`FontDb::global`]).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    font_dirs: Vec<std::path::PathBuf>,
    /// Set once the font folders have been scanned. Lookups by family name wait for the first
    /// scan, so what they find doesn't depend on what ran before them.
    #[cfg(not(target_arch = "wasm32"))]
    cataloged: std::sync::OnceLock<()>,
    /// Families (lower case) whose cataloged files have all been loaded: a family's files load
    /// together, so a lookup never settles for the one face another name loaded ("Segoe UI
    /// Semibold" loads a face of the family "Segoe UI").
    #[cfg(not(target_arch = "wasm32"))]
    loaded_families: Mutex<std::collections::HashSet<String>>,
    /// System fallback state: enabled, and characters no system font covers.
    #[cfg(not(target_arch = "wasm32"))]
    sys: Mutex<SysFallback>,
    /// Held while installed fonts are loaded, so threads that need the same family wait for one
    /// load instead of racing it: what a lookup finds doesn't depend on which thread got there
    /// first.
    #[cfg(not(target_arch = "wasm32"))]
    loading: Mutex<()>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct SysFallback {
    enabled: bool,
    misses: std::collections::HashSet<char>,
}

/// Does face `index` of `data` carry colour glyph tables?
fn has_color_tables(data: &[u8], index: u32) -> bool {
    use skrifa::raw::types::Tag;
    let Ok(f) = skrifa::FontRef::from_index(data, index) else { return false };
    [b"COLR", b"CBDT", b"sbix", b"SVG "].iter().any(|t| f.table_data(Tag::new(t)).is_some())
}

/// Families tried (when installed) for characters the loaded fonts lack: CJK, symbols, emoji.
#[cfg(not(target_arch = "wasm32"))]
const SYSTEM_FALLBACKS: &[&str] = &[
    "Helvetica Neue",
    "Arial",
    "Segoe UI",
    "Noto Sans",
    "DejaVu Sans",
    "PingFang SC",
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Apple SD Gothic Neo",
    "Heiti SC",
    "STHeiti",
    "Microsoft YaHei",
    "Yu Gothic",
    "Malgun Gothic",
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Arial Unicode MS",
    "Apple Symbols",
    "Segoe UI Symbol",
    "Noto Sans Symbols",
    "Noto Sans Symbols2",
    "Noto Emoji",
    "Segoe UI Emoji",
    "Apple Color Emoji",
    "Noto Color Emoji",
];

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
const OUTLINE_CACHE_MAX: usize = 50_000;
/// Most path elements one glyph outline may expand to. Composite and `VARC` glyphs can draw a
/// shared outline many times over; a glyph past this draws nothing rather than holding megabytes.
const MAX_OUTLINE_ELEMENTS: usize = 1 << 15;
/// Most path elements the outline cache holds across all glyphs (about 230 MB at most) before it
/// starts over: distinct glyph ids can each expand one dense outline.
const OUTLINE_CACHE_MAX_ELEMENTS: usize = 1 << 22;

/// Glyph outlines by (face, glyph), with the path elements they hold.
#[derive(Default)]
struct OutlineCache {
    paths: HashMap<(u32, u32), Arc<BezPath>>,
    elements: usize,
}

impl OutlineCache {
    fn get(&self, key: &(u32, u32)) -> Option<Arc<BezPath>> {
        self.paths.get(key).cloned()
    }

    fn insert(&mut self, key: (u32, u32), path: Arc<BezPath>) {
        let n = path.elements().len();
        if let Some(old) = self.paths.remove(&key) {
            self.elements = self.elements.saturating_sub(old.elements().len());
        }
        if self.paths.len() >= OUTLINE_CACHE_MAX || self.elements.saturating_add(n) > OUTLINE_CACHE_MAX_ELEMENTS {
            self.paths.clear();
            self.elements = 0;
        }
        self.paths.insert(key, path);
        self.elements = self.elements.saturating_add(n);
    }
}

fn name(font: &skrifa::FontRef<'_>, ids: &[StringId]) -> Option<String> {
    ids.iter().find_map(|id| font.localized_strings(*id).english_or_first().map(|s| s.to_string()).filter(|s| !s.is_empty()))
}

/// A face's family and style names (the default instance's style for a variable font).
fn face_names(f: &skrifa::FontRef<'_>) -> Option<(String, String)> {
    let family = name(f, &[StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME])?;
    let style = name(f, &[StringId::TYPOGRAPHIC_SUBFAMILY_NAME, StringId::SUBFAMILY_NAME]).unwrap_or_else(|| "Regular".into());
    Some((family, style))
}

/// A face's Windows (GDI) family and style (`name` IDs 1 and 2) when that family differs from the
/// typographic `family`.
fn legacy_names(f: &skrifa::FontRef<'_>, family: &str) -> Option<(String, String)> {
    let legacy = name(f, &[StringId::FAMILY_NAME])?;
    if legacy.eq_ignore_ascii_case(family) {
        return None;
    }
    Some((legacy, name(f, &[StringId::SUBFAMILY_NAME]).unwrap_or_else(|| "Regular".into())))
}

/// The Windows family and style a variable font's named instance answers to. Windows lists each
/// instance other than Regular, Italic, Bold and Bold Italic as a family of its own, as the
/// family's static files name themselves: "Montserrat ExtraBold" / "Regular" for the ExtraBold
/// instance, "Montserrat ExtraBold" / "Italic" for ExtraBold Italic.
fn instance_legacy_names(family: &str, style: &str) -> Option<(String, String)> {
    if style.contains('{') {
        return None;
    }
    let weight: Vec<&str> = style.split_whitespace().filter(|w| !matches!(norm(w).as_str(), "italic" | "oblique")).collect();
    let weight = weight.join(" ");
    if matches!(norm(&weight).as_str(), "" | "regular" | "bold") {
        return None;
    }
    Some((format!("{family} {weight}"), if style_italic(style) { "Italic" } else { "Regular" }.into()))
}

/// The tables skrifa reads glyph outlines from: `glyf` (TrueType), `CFF `, `CFF2` and `VARC`. For a
/// face with none of them (bitmaps only, or outlines in Apple's `hvgl` table), [`FontDb::outline`]
/// returns an empty path for every glyph.
#[cfg(not(target_arch = "wasm32"))]
const OUTLINE_TABLES: [&[u8; 4]; 4] = [b"glyf", b"CFF ", b"CFF2", b"VARC"];

/// (family, style) of every face in the font file at `path`, reading only its table directories
/// and `name` tables: a scan opens hundreds of font files, many of them megabytes long. A
/// variable font is cataloged under its default style; its named instances appear once it loads.
/// Faces with none of the [`OUTLINE_TABLES`] are left out.
#[cfg(not(target_arch = "wasm32"))]
fn file_face_names(path: &std::path::Path) -> Vec<(String, String)> {
    use std::io::{Read, Seek, SeekFrom};
    /// Caps on what a (possibly damaged) file can make the scan read.
    const MAX_FACES: u32 = 256;
    const MAX_NAME_TABLE: u32 = 1 << 20;
    let Ok(mut file) = std::fs::File::open(path) else { return vec![] };
    let mut read_at = |offset: u64, len: usize| -> Option<Vec<u8>> {
        let mut buf = vec![0; len];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    let be32 = |b: &[u8], at: usize| b.get(at..at + 4).and_then(|s| s.try_into().ok()).map(u32::from_be_bytes);
    let Some(head) = read_at(0, 12) else { return vec![] };
    // A collection lists where each face's table directory starts.
    let starts: Vec<u32> = if head.starts_with(b"ttcf") {
        let n = be32(&head, 8).unwrap_or(0).min(MAX_FACES) as usize;
        read_at(12, n * 4).map(|b| b.as_chunks::<4>().0.iter().map(|c| u32::from_be_bytes(*c)).collect()).unwrap_or_default()
    } else {
        vec![0]
    };
    starts
        .into_iter()
        .filter_map(|start| {
            let dir = read_at(start.into(), 12)?;
            let tables = u16::from_be_bytes(dir.get(4..6)?.try_into().ok()?) as usize;
            let records = read_at(u64::from(start) + 12, tables * 16)?;
            if !records.as_chunks::<16>().0.iter().any(|r| OUTLINE_TABLES.iter().any(|t| r.starts_with(*t))) {
                return None;
            }
            let rec = records.as_chunks::<16>().0.iter().find(|r| r.starts_with(b"name"))?;
            let (offset, len) = (be32(rec, 8)?, be32(rec, 12)?);
            if len > MAX_NAME_TABLE {
                return None;
            }
            let table = read_at(offset.into(), len as usize)?;
            // A one-table font holding just the `name` table, to read it as the font itself would.
            let mut font = Vec::with_capacity(28 + table.len());
            font.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
            font.extend_from_slice(&[0, 1, 0, 16, 0, 0, 0, 0]);
            font.extend_from_slice(b"name");
            for v in [0, 28, len] {
                font.extend_from_slice(&u32::to_be_bytes(v));
            }
            font.extend_from_slice(&table);
            let f = skrifa::FontRef::new(&font).ok()?;
            let (family, style) = face_names(&f)?;
            // Cataloged under its Windows family too, so "Segoe UI Semibold" finds the file.
            let legacy = legacy_names(&f, &family);
            Some(std::iter::once((family, style)).chain(legacy))
        })
        .flatten()
        .collect()
}

/// Folder depth below a font folder that the system scan descends.
#[cfg(not(target_arch = "wasm32"))]
const MAX_SCAN_DEPTH: usize = 12;
/// Directory entries the system scan looks at in all.
#[cfg(not(target_arch = "wasm32"))]
const MAX_SCAN_ENTRIES: usize = 200_000;
/// The largest font file read (the biggest CJK collections are a few hundred MB).
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_FONT_FILE: u64 = 512 << 20;

/// A regular file of a size a font can have.
#[cfg(not(target_arch = "wasm32"))]
fn font_file_ok(meta: &std::fs::Metadata) -> bool {
    meta.is_file() && meta.len() > 12 && meta.len() <= MAX_FONT_FILE
}

/// The platform's font folders (the system's and the user's), scanned by [`FontDb::global`].
/// Empty on wasm.
pub fn system_font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(target_arch = "wasm32") {
        return dirs;
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(Into::into));
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
            // Fonts Microsoft Office downloads on demand (Aptos…), used where Office put them.
            dirs.push(h.join("Library/Group Containers/UBF8T346G9.Office/FontCache/4/CloudFonts"));
        }
        // Fonts Office installs inside its apps.
        for app in ["Microsoft PowerPoint", "Microsoft Word", "Microsoft Excel"] {
            dirs.push(std::path::PathBuf::from(format!("/Applications/{app}.app/Contents/Resources/DFonts")));
        }
        // Fonts macOS ships as assets (PingFang, Yu Mincho, …): com_apple_MobileAsset_Font<N>.
        for parent in ["/System/Library/AssetsV2", "/System/Library/AssetsV2/PreinstalledAssetsV2/InstallWithOs"] {
            let Ok(entries) = std::fs::read_dir(parent) else { continue };
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with("com_apple_MobileAsset_Font") {
                    dirs.push(e.path());
                }
            }
        }
    } else if cfg!(windows) {
        let root = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(root.join("Fonts"));
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            let l = std::path::PathBuf::from(l);
            dirs.push(l.join("Microsoft\\Windows\\Fonts"));
            // Fonts Microsoft Office downloads on demand (Aptos…), used where Office put them;
            // DeckCraft never ships or needs them.
            dirs.push(l.join("Microsoft\\FontCache\\4\\CloudFonts"));
        }
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(Into::into));
        if let Some(h) = &home {
            dirs.push(h.join(".fonts"));
            dirs.push(h.join(".local/share/fonts"));
        }
        let env = |k: &str| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned());
        dirs.extend(xdg_font_dirs(env("XDG_DATA_HOME").as_deref(), env("XDG_DATA_DIRS").as_deref(), home.as_deref()));
        // Inside Flatpak the host's fonts are mounted here.
        dirs.extend(["/run/host/fonts", "/run/host/local-fonts", "/run/host/user-fonts"].map(Into::into));
        // Folders fontconfig is configured with (NixOS, Guix, custom setups).
        for conf in fontconfig_files() {
            if let Ok(text) = std::fs::read_to_string(&conf) {
                dirs.extend(fontconfig_dirs(&text, env("XDG_DATA_HOME").as_deref(), home.as_deref()));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

/// `<data dir>/fonts` for `XDG_DATA_HOME` (default `~/.local/share`) and each `XDG_DATA_DIRS`
/// entry (default `/usr/local/share:/usr/share`).
fn xdg_font_dirs(data_home: Option<&str>, data_dirs: Option<&str>, home: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut v = vec![];
    match data_home.filter(|d| !d.is_empty()) {
        Some(d) => v.push(std::path::Path::new(d).join("fonts")),
        None => {
            if let Some(h) = home {
                v.push(h.join(".local/share/fonts"));
            }
        }
    }
    let dirs = data_dirs.filter(|d| !d.is_empty()).unwrap_or("/usr/local/share:/usr/share");
    v.extend(dirs.split(':').filter(|d| d.starts_with('/')).take(64).map(|d| std::path::Path::new(d).join("fonts")));
    v
}

/// The fontconfig configuration files: `/etc/fonts/fonts.conf` and `/etc/fonts/conf.d/*.conf`.
fn fontconfig_files() -> Vec<std::path::PathBuf> {
    let mut v = vec![std::path::PathBuf::from("/etc/fonts/fonts.conf"), std::path::PathBuf::from("/etc/fonts/local.conf")];
    if let Ok(entries) = std::fs::read_dir("/etc/fonts/conf.d") {
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "conf")).collect();
        files.sort();
        files.truncate(512);
        v.extend(files);
    }
    v
}

/// The `<dir>` folders of a fontconfig file (`prefix="xdg"` is under `XDG_DATA_HOME`, a leading
/// `~` under home; relative and `cachedir` entries are skipped).
fn fontconfig_dirs(text: &str, data_home: Option<&str>, home: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut v = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("<dir") {
        let after = &rest[start + 4..];
        // `<dir>` or `<dir …>`, not `<dirname>`.
        if !(after.starts_with('>') || after.starts_with(' ')) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        let attrs = &after[..gt];
        let body = &after[gt + 1..];
        let Some(end) = body.find("</dir>") else { break };
        let path = body[..end].trim();
        rest = &body[end..];
        if attrs.trim_end().ends_with('/') || path.is_empty() || v.len() >= 64 {
            continue;
        }
        let p = if attrs.contains("prefix=\"xdg\"") {
            let base = data_home.filter(|d| !d.is_empty()).map(std::path::PathBuf::from).or_else(|| home.map(|h| h.join(".local/share")));
            base.map(|b| b.join(path))
        } else if let Some(r) = path.strip_prefix("~/") {
            home.map(|h| h.join(r))
        } else if path.starts_with('/') {
            Some(std::path::PathBuf::from(path))
        } else {
            None
        };
        v.extend(p);
    }
    v
}

/// One face found in a font file: index, family, style and (variable fonts) the named instance's
/// axis settings.
type Found = (u32, String, String, Vec<([u8; 4], f32)>);

/// Maximum number of faces we enumerate from a single font collection.
const MAX_COLLECTION_FACES: u32 = 256;
/// Maximum collection entries inspected while looking for distinct table directories.
const MAX_COLLECTION_ENTRIES_SCANNED: u32 = 16_384;
/// Maximum named instances read from a single face.
const MAX_NAMED_INSTANCES_PER_FACE: usize = 1024;
/// Maximum faces (named instances counted separately) enumerated from one font file, across all
/// of a collection's faces: the per-collection and per-face limits multiply, and each face found
/// is then built with its own shaper.
const MAX_ENUMERATED_FACES: usize = 4096;

fn distinct_face_indices(entries: impl IntoIterator<Item = (u32, u32)>) -> Vec<u32> {
    let mut offsets = std::collections::HashSet::new();
    let mut indices = Vec::with_capacity(MAX_COLLECTION_FACES as usize);
    for (index, offset) in entries.into_iter().take(MAX_COLLECTION_ENTRIES_SCANNED as usize) {
        if offsets.insert(offset) {
            indices.push(index);
            if indices.len() == MAX_COLLECTION_FACES as usize {
                break;
            }
        }
    }
    indices
}

fn capped_named_instance_count(count: usize) -> usize {
    count.min(MAX_NAMED_INSTANCES_PER_FACE)
}

/// Parse every face in `data` (a font file or collection); a variable font yields one face per
/// named instance (InDesign lists them as styles).
fn enumerate_faces(data: &[u8]) -> Vec<Found> {
    let indices = match FileRef::new(data) {
        Ok(FileRef::Font(_)) => vec![0],
        Ok(FileRef::Collection(c)) => {
            let entries = (0..c.len().min(MAX_COLLECTION_ENTRIES_SCANNED)).filter_map(|i| {
                let start = 12usize.checked_add((i as usize).checked_mul(4)?)?;
                let end = start.checked_add(4)?;
                let offset = u32::from_be_bytes(data.get(start..end)?.try_into().ok()?);
                Some((i, offset))
            });
            distinct_face_indices(entries)
        }
        Err(_) => Vec::new(),
    };
    let mut out = Vec::new();
    // Named instances read and plain faces kept, across the whole file.
    let mut budget = MAX_ENUMERATED_FACES;
    for i in indices {
        if budget == 0 {
            break;
        }
        let Ok(f) = skrifa::FontRef::from_index(data, i) else { continue };
        let Some((family, style)) = face_names(&f) else { continue };
        let axes = f.axes();
        let mut named = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let instances = f.named_instances();
        for ni in instances.iter().take(capped_named_instance_count(instances.len())) {
            let Some(left) = budget.checked_sub(1) else { break };
            budget = left;
            let Some(style) = name(&f, &[ni.subfamily_name_id()]) else { continue };
            if !seen.insert(style.to_ascii_lowercase()) {
                continue;
            }
            let coords: Vec<([u8; 4], f32)> = axes.iter().zip(ni.user_coords()).map(|(a, v)| (a.tag().to_be_bytes(), v)).collect();
            named.push((i, style, coords));
        }
        if named.is_empty() {
            budget = budget.saturating_sub(1);
            out.push((i, family, style, Vec::new()));
        } else {
            out.extend(named.into_iter().map(|(i, s, c)| (i, family.clone(), s, c)));
        }
    }
    out
}

/// The compiled-in last-resort face, for a database that somehow has no usable faces at all.
pub(crate) fn last_resort_face() -> Arc<FontFace> {
    static FACE: std::sync::OnceLock<Arc<FontFace>> = std::sync::OnceLock::new();
    FACE.get_or_init(|| {
        // The font is compiled in (`include_bytes!`), so parsing it can't depend on input; the
        // `last_resort_face_parses` test proves it on every run.
        #[allow(clippy::expect_used)]
        let face = make_face(FontBytes::Static(epaint_default_fonts::UBUNTU_LIGHT), 0, LAST_RESORT_FAMILY.into(), "Regular".into(), Vec::new())
            .expect("the compiled-in last-resort face parses");
        Arc::new(face)
    })
    .clone()
}

/// The share of a line above the baseline (`FontFace::line_split`): OS/2 `usWinAscent` over
/// `usWinAscent + usWinDescent`, else the ascent over ascent plus descent. Fonts whose Windows
/// extent is huge (Gabriola, Cambria Math: over 1.5 em) get 0.81, where PowerPoint puts their
/// baseline (observed).
fn line_split(f: &skrifa::FontRef, ascent: f64, descent: f64, upem: f64) -> f64 {
    use skrifa::raw::TableProvider;
    let (a, d) = match f.os2() {
        Ok(os2) if os2.us_win_ascent() > 0 => (os2.us_win_ascent() as f64, os2.us_win_descent() as f64),
        _ => (ascent, descent),
    };
    let total = a + d;
    if !total.is_finite() || total <= 0.0 || a < 0.0 || d < 0.0 {
        return 0.8;
    }
    if total > upem * 1.5 { 0.81 } else { (a / total).clamp(0.5, 1.0) }
}

fn make_face(bytes: FontBytes, index: u32, family: String, style: String, coords: Vec<([u8; 4], f32)>) -> Option<FontFace> {
    let data: &[u8] = match &bytes {
        FontBytes::Static(b) => b,
        FontBytes::Owned(v) => v.as_slice(),
    };
    let f = skrifa::FontRef::from_index(data, index).ok()?;
    let settings: Vec<(skrifa::Tag, f32)> = coords.iter().map(|(t, v)| (skrifa::Tag::new(t), *v)).collect();
    let location = if coords.is_empty() { Location::default() } else { f.axes().location(settings.iter().copied()) };
    let m = f.metrics(Size::unscaled(), &location);
    let a = f.attributes();
    let hb = harfrust::FontRef::from_index(data, index).ok()?;
    let shaper = harfrust::ShaperData::new(&hb);
    let instance = (!coords.is_empty())
        .then(|| harfrust::ShaperInstance::from_variations(&hb, settings.iter().map(|(t, v)| harfrust::Variation { tag: *t, value: *v })));
    let axis = |tag: &[u8; 4]| coords.iter().find(|(t, _)| t == tag).map(|(_, v)| *v);
    let weight = axis(b"wght").unwrap_or(if coords.is_empty() { a.weight.value() } else { style_weight(&style) });
    let italic = axis(b"ital")
        .map(|v| v >= 0.5)
        .or(axis(b"slnt").map(|v| v.abs() > 0.1))
        .unwrap_or(!matches!(a.style, skrifa::attribute::Style::Normal) || style_italic(&style));
    let legacy = if coords.is_empty() { legacy_names(&f, &family) } else { instance_legacy_names(&family, &style) };
    Some(FontFace {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        family,
        style,
        legacy,
        weight,
        italic,
        upem: m.units_per_em.max(1) as f64,
        ascent: m.ascent as f64,
        descent: -(m.descent as f64),
        cap_height: m.cap_height.map(|v| v as f64).filter(|v| *v > 0.0).unwrap_or(m.ascent as f64 * 0.72),
        x_height: m.x_height.map(|v| v as f64).filter(|v| *v > 0.0).unwrap_or(m.ascent as f64 * 0.5),
        line_split: line_split(&f, m.ascent as f64, -(m.descent as f64), m.units_per_em.max(1) as f64),
        underline: underline_metrics(m.underline, m.units_per_em.max(1) as f64),
        strikeout: strikeout_metrics(m.strikeout, m.units_per_em.max(1) as f64),
        shaper,
        coords,
        location,
        instance,
        bytes,
        index,
        bmp: std::sync::OnceLock::new(),
        color: std::sync::OnceLock::new(),
    })
}

/// The underline's top edge below the baseline and its thickness, font units, from the `post`
/// table (its position is the top edge: Arial's -217/2048 is where PowerPoint's underline starts,
/// 10 px below the baseline at 96 pt); fonts without a usable one get 0.1 em and 0.05 em.
fn underline_metrics(d: Option<skrifa::metrics::Decoration>, upem: f64) -> (f64, f64) {
    match d {
        Some(d) if d.thickness > 0.0 && (d.thickness as f64) < upem * 0.3 && (d.offset as f64).abs() < upem * 0.5 => {
            (-(d.offset as f64), d.thickness as f64)
        }
        _ => (upem * 0.1, upem * 0.05),
    }
}

/// The strikethrough's top edge above the baseline and its thickness, font units, from OS/2 (its
/// position is the top edge: Arial's 530/2048 is where PowerPoint's strikethrough starts, 25 px
/// above the baseline at 96 pt); fonts without a usable one get 0.26 em and 0.05 em.
fn strikeout_metrics(d: Option<skrifa::metrics::Decoration>, upem: f64) -> (f64, f64) {
    match d {
        Some(d) if d.thickness > 0.0 && (d.thickness as f64) < upem * 0.3 && d.offset > 0.0 && (d.offset as f64) < upem => {
            (d.offset as f64, d.thickness as f64)
        }
        _ => (upem * 0.26, upem * 0.05),
    }
}

/// The named style part of a style with axis settings (`Bold {wght:650}` → `Bold`).
pub fn base_style(style: &str) -> &str {
    style.split('{').next().unwrap_or(style).trim()
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Weight implied by a style name.
fn style_weight(style: &str) -> f32 {
    let s = norm(style);
    const TABLE: &[(&str, f32)] = &[
        ("extralight", 200.0),
        ("ultralight", 200.0),
        ("semibold", 600.0),
        ("demibold", 600.0),
        ("extrabold", 800.0),
        ("ultrabold", 800.0),
        ("hairline", 100.0),
        ("thin", 100.0),
        ("light", 300.0),
        ("medium", 500.0),
        ("bold", 700.0),
        ("black", 900.0),
        ("heavy", 900.0),
    ];
    TABLE.iter().find(|(k, _)| s.contains(k)).map(|(_, w)| *w).unwrap_or(400.0)
}

fn style_italic(style: &str) -> bool {
    let s = norm(style);
    s.contains("italic") || s.contains("oblique") || s == "it"
}

impl FontDb {
    /// A database holding the bundled fonts, then the craft-fonts Japanese faces (Mincho first,
    /// so they are the fallback for Japanese text after the requested and bundled fonts; empty
    /// without `CRAFT_FONTS_DIR`), whose system font scan reads `font_dirs`.
    pub fn with_font_dirs(font_dirs: Vec<std::path::PathBuf>) -> Self {
        let mut faces = Vec::new();
        for data in bundled() {
            for (i, family, style, coords) in enumerate_faces(data) {
                if let Some(f) = make_face(FontBytes::Static(data), i, family, style, coords) {
                    faces.push(Arc::new(f));
                }
            }
        }
        Self {
            faces: RwLock::new(faces),
            outlines: Mutex::new(OutlineCache::default()),
            #[cfg(not(target_arch = "wasm32"))]
            catalog: RwLock::new(Vec::new()),
            font_dirs,
            #[cfg(not(target_arch = "wasm32"))]
            cataloged: std::sync::OnceLock::new(),
            #[cfg(not(target_arch = "wasm32"))]
            loaded_families: Mutex::new(std::collections::HashSet::new()),
            #[cfg(not(target_arch = "wasm32"))]
            sys: Mutex::new(SysFallback { enabled: true, ..Default::default() }),
            #[cfg(not(target_arch = "wasm32"))]
            loading: Mutex::new(()),
        }
    }

    /// Enable or disable the lazy system-font fallback for characters the loaded fonts lack
    /// (native only; on by default).
    pub fn set_system_fallback(&self, on: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).enabled = on;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = on;
    }

    /// Whether automatic system-font lookups (fallback by character, symbol and script families)
    /// may touch installed fonts. Lookups of a family the document or host names are not gated.
    pub(crate) fn system_fallback_enabled(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).enabled
        }
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Process-wide database: the bundled fonts, then the installed system fonts, cataloged the
    /// first time a lookup by family name needs them (or ahead of time by
    /// [`FontDb::scan_in_background`]).
    pub fn global() -> &'static FontDb {
        static DB: std::sync::OnceLock<FontDb> = std::sync::OnceLock::new();
        DB.get_or_init(|| FontDb::with_font_dirs(system_font_dirs()))
    }

    pub(crate) fn read_faces(&self) -> std::sync::RwLockReadGuard<'_, Vec<Arc<FontFace>>> {
        self.faces.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The system font catalog, scanned first if it hasn't been yet.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_catalog(&self) -> std::sync::RwLockReadGuard<'_, Vec<CatalogEntry>> {
        self.ensure_catalog();
        self.catalog.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Family names available (loaded plus cataloged system fonts), sorted and deduplicated.
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = self.read_faces().iter().map(|f| f.family.clone()).collect();
        #[cfg(not(target_arch = "wasm32"))]
        v.extend(self.read_catalog().iter().map(|c| c.family.clone()));
        v.sort_by_key(|a| a.to_lowercase());
        v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        v
    }

    /// Style names available for `family` (Regular first, then by weight).
    pub fn styles(&self, family: &str) -> Vec<String> {
        let mut v: Vec<(bool, f32, String)> = self
            .read_faces()
            .iter()
            .filter(|f| f.family.eq_ignore_ascii_case(family) && !f.style.contains('{'))
            .map(|f| (f.italic, f.weight, f.style.clone()))
            .collect();
        #[cfg(not(target_arch = "wasm32"))]
        for c in self.read_catalog().iter() {
            if c.family.eq_ignore_ascii_case(family) && !v.iter().any(|(_, _, s)| s.eq_ignore_ascii_case(&c.style)) {
                v.push((style_italic(&c.style), style_weight(&c.style), c.style.clone()));
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        v.dedup_by(|a, b| a.2 == b.2);
        v.into_iter().map(|t| t.2).collect()
    }

    /// Add a user font (TTF/OTF/TTC bytes). Returns the number of faces added (0 if unparseable or
    /// every face was already present).
    pub fn add_font(&self, bytes: Vec<u8>) -> usize {
        self.add_fonts(vec![bytes])
    }

    /// Add font files together: lookups see all of their faces or none, never part of a family.
    /// Returns the number of faces added.
    fn add_fonts(&self, files: Vec<Vec<u8>>) -> usize {
        let present = |faces: &[Arc<FontFace>], family: &str, style: &str| {
            faces.iter().any(|f| f.family.eq_ignore_ascii_case(family) && f.style.eq_ignore_ascii_case(style))
        };
        let mut new = Vec::new();
        for bytes in files {
            let data = Arc::new(bytes);
            for (i, family, style, coords) in enumerate_faces(&data) {
                if present(&self.read_faces(), &family, &style) {
                    continue;
                }
                if let Some(f) = make_face(FontBytes::Owned(data.clone()), i, family, style, coords) {
                    new.push(Arc::new(f));
                }
            }
        }
        let mut faces = self.faces.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut added = 0;
        for f in new {
            // Checked again under the lock: another thread may have added the same face.
            if !present(&faces, &f.family, &f.style) {
                faces.push(f);
                added += 1;
            }
        }
        added
    }

    /// Catalog the installed fonts unless that has been done: the first caller scans the font
    /// folders, any other waits for that scan to finish.
    #[cfg(not(target_arch = "wasm32"))]
    fn ensure_catalog(&self) {
        self.cataloged.get_or_init(|| {
            self.scan_font_dirs();
        });
    }

    /// Catalog the installed fonts on a background thread, so the first lookup by family name
    /// (opening a file, the font menus) doesn't wait for the scan. A no-op once they are
    /// cataloged, and on wasm.
    pub fn scan_in_background(&'static self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.cataloged.get().is_none() {
            // A failed spawn leaves the scan to the first lookup that needs it.
            let _ = std::thread::Builder::new().name("font-scan".into()).spawn(move || self.ensure_catalog());
        }
    }

    /// Scan the font folders again (fonts installed or removed since), cataloging the faces
    /// found (native only). Font data is loaded when a cataloged family is first resolved.
    /// Returns the number of faces cataloged.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_system_fonts(&self) -> usize {
        // Families load again from the new catalog when next looked up.
        self.loaded_families.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        // The first scan, or a rescan once it is done (never both at once).
        let mut first = None;
        self.cataloged.get_or_init(|| first = Some(self.scan_font_dirs()));
        first.unwrap_or_else(|| self.scan_font_dirs())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn scan_font_dirs(&self) -> usize {
        let mut found = Vec::new();
        let mut stack: Vec<(std::path::PathBuf, usize)> = self.font_dirs.iter().map(|d| (d.clone(), 0)).collect();
        // Each folder once, however links lead back to it.
        let mut visited = std::collections::HashSet::new();
        let mut entries = 0usize;
        while let Some((d, depth)) = stack.pop() {
            if !visited.insert(std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone())) {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                // A link to `/` or a vast tree mustn't turn start-up into a disk crawl.
                entries += 1;
                if entries > MAX_SCAN_ENTRIES {
                    log::warn!("font scan stopped after {MAX_SCAN_ENTRIES} entries");
                    stack.clear();
                    break;
                }
                let p = e.path();
                // Follows links: what matters is what the entry leads to.
                let Ok(meta) = std::fs::metadata(&p) else { continue };
                if meta.is_dir() {
                    if depth < MAX_SCAN_DEPTH {
                        stack.push((p, depth + 1));
                    }
                    continue;
                }
                let ext = p.extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase);
                if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                    continue;
                }
                // Only regular files: opening a FIFO named like a font blocks until something
                // writes to it (start-up hung), and a device never ends.
                if !font_file_ok(&meta) {
                    continue;
                }
                for (family, style) in file_face_names(&p) {
                    found.push(CatalogEntry { family, style, path: p.clone() });
                }
            }
        }
        let n = found.len();
        log::debug!("cataloged {n} system font faces");
        *self.catalog.write().unwrap_or_else(std::sync::PoisonError::into_inner) = found;
        n
    }

    /// Load the files of the installed `family` (faces already loaded are kept). Returns whether
    /// it is loaded.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_cataloged(&self, family: &str) -> bool {
        let loading = self.loading.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.load_family(family, &loading)
    }

    /// [`FontDb::load_cataloged`] for a caller already holding `loading`.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_family(&self, family: &str, _loading: &std::sync::MutexGuard<'_, ()>) -> bool {
        // Not skipped when some face of the family is loaded: a lookup by a Windows family name
        // loads only that style's file, and the family's other files must still load later.
        let paths: Vec<std::path::PathBuf> = {
            let cat = self.read_catalog();
            let mut p: Vec<_> = cat.iter().filter(|c| c.family.eq_ignore_ascii_case(family)).map(|c| c.path.clone()).collect();
            p.sort();
            p.dedup();
            p
        };
        self.add_fonts(paths.iter().filter(|p| std::fs::metadata(p).is_ok_and(|m| font_file_ok(&m))).filter_map(|p| std::fs::read(p).ok()).collect());
        self.is_loaded(family)
    }

    /// Resolve a family + style to a face, falling back to the closest style of the family, then to
    /// an open substitute ([`substitutes`]), then to Inter. Installed system fonts are found by name
    /// whatever ran before.
    pub fn face(&self, family: &str, style: &str) -> Arc<FontFace> {
        if let Some(f) = self.try_family(family, style) {
            return f;
        }
        for sub in substitutes(family) {
            if let Some(f) = self.try_family(sub, style) {
                return f;
            }
        }
        for sub in [FALLBACK_FAMILY, "Helvetica Neue", "Arial", "Liberation Sans", "DejaVu Sans", "Segoe UI"] {
            if let Some(f) = self.try_family(sub, style) {
                return f;
            }
        }
        self.find(LAST_RESORT_FAMILY, "Regular")
            .or_else(|| self.find(FALLBACK_FAMILY, "Regular"))
            .or_else(|| self.read_faces().first().cloned())
            .unwrap_or_else(last_resort_face)
    }

    /// The face a document's text in `family` is drawn with, as PowerPoint draws it: the family
    /// when it is installed or bundled, or a metric-compatible stand-in (Carlito for Calibri,
    /// Liberation Sans for Arial…), or the family PowerPoint maps it to (Liberation Sans as Arial,
    /// see `office_twin`); any other missing family is drawn in Calibri (or Carlito).
    /// Monospaced families keep a monospaced substitute instead of falling back to Calibri.
    /// DeckCraft's own default family keeps [`FontDb::face`]'s fallbacks.
    pub fn document_face(&self, family: &str, style: &str) -> Arc<FontFace> {
        if family.eq_ignore_ascii_case(FALLBACK_FAMILY) || has_metric_twin(family) || is_office_cloud_font(family) || self.has_family(family) {
            return self.face(family, style);
        }
        if let Some(twin) = office_twin(family) {
            return self.face(twin, style);
        }
        if is_monospace_family(family) {
            return self.face(family, style);
        }
        self.face(MISSING_FONT_FAMILY, style)
    }

    fn try_family(&self, family: &str, style: &str) -> Option<Arc<FontFace>> {
        // Every installed file of the family first, so the best style is among the candidates. The
        // lock is held while the files load, so another thread looking up the family waits for
        // them rather than finding none and falling back to another family.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut loaded = self.loaded_families.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = family.to_lowercase();
            if !loaded.contains(&key) {
                self.load_cataloged(family);
                loaded.insert(key);
            }
        }
        self.find(family, style)
    }

    /// Is `family` available (loaded, or installed on the system)?
    pub fn has_family(&self, family: &str) -> bool {
        if self.is_loaded(family) {
            return true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.read_catalog().iter().any(|c| c.family.eq_ignore_ascii_case(family)) {
            return true;
        }
        false
    }

    pub(crate) fn is_loaded(&self, family: &str) -> bool {
        self.read_faces()
            .iter()
            .any(|f| f.family.eq_ignore_ascii_case(family) || f.legacy.as_ref().is_some_and(|(lf, _)| lf.eq_ignore_ascii_case(family)))
    }

    /// A variable font's axes: (tag, name, min, default, max), empty for static fonts.
    pub fn axes(&self, family: &str, style: &str) -> Vec<(String, String, f32, f32, f32)> {
        let face = self.face(family, base_style(style));
        let Some(f) = face.skrifa() else { return vec![] };
        f.axes()
            .iter()
            .map(|a| {
                let tag = String::from_utf8_lossy(&a.tag().to_be_bytes()).to_string();
                let name = f.localized_strings(a.name_id()).english_or_first().map(|s| s.to_string()).unwrap_or_else(|| tag.clone());
                (tag, name, a.min_value(), a.default_value(), a.max_value())
            })
            .collect()
    }

    /// `Style {wght:650,wdth:90}`: the named style's font at those axis values, made on first use.
    fn instance(&self, family: &str, style: &str) -> Option<Arc<FontFace>> {
        let open = style.find('{')?;
        let base = self.find(family, style[..open].trim())?;
        let Some(font) = base.skrifa().filter(|f| !f.axes().is_empty()) else { return Some(base) };
        let axis_tags: Vec<[u8; 4]> = font.axes().iter().map(|a| a.tag().to_be_bytes()).collect();
        let mut coords = base.coords.clone();
        for kv in style[open + 1..].trim_end_matches('}').split(',') {
            let (k, v) = kv.split_once(':')?;
            let k = k.trim().as_bytes();
            let v: f32 = v.trim().parse().ok()?;
            if k.len() != 4 {
                return None;
            }
            let tag = [k[0], k[1], k[2], k[3]];
            // A tag the font has no axis for changes nothing; keeping it would grow the list with
            // every distinct tag in the style text.
            if !axis_tags.contains(&tag) {
                continue;
            }
            match coords.iter_mut().find(|(t, _)| *t == tag) {
                Some(c) => c.1 = v,
                None => coords.push((tag, v)),
            }
        }
        let f = make_face(base.bytes.clone(), base.index, base.family.clone(), style.to_string(), coords)?;
        let f = Arc::new(f);
        self.faces.write().unwrap_or_else(std::sync::PoisonError::into_inner).push(f.clone());
        Some(f)
    }

    pub(crate) fn find(&self, family: &str, style: &str) -> Option<Arc<FontFace>> {
        if style.contains('{') {
            {
                let faces = self.read_faces();
                if let Some(f) = faces.iter().find(|f| f.family.eq_ignore_ascii_case(family) && f.style == style) {
                    return Some(f.clone());
                }
            }
            return self.instance(family, style);
        }
        let faces = self.read_faces();
        let mut cands: Vec<&Arc<FontFace>> = faces.iter().filter(|f| f.family.eq_ignore_ascii_case(family)).collect();
        let ns = norm(style);
        if cands.is_empty() {
            // A Windows family name ("Segoe UI Semibold", "Arial Black"): its faces, matched by
            // their Windows style ("Regular", "Bold"…).
            cands = faces.iter().filter(|f| f.legacy.as_ref().is_some_and(|(lf, _)| lf.eq_ignore_ascii_case(family))).collect();
            if let Some(f) = cands.iter().find(|f| f.legacy.as_ref().is_some_and(|(_, ls)| norm(ls) == ns)) {
                return Some((*f).clone());
            }
            // Bold of a family that is itself a weight (Semibold, Light): the face, made bolder
            // by the caller, as Windows does, rather than a face of another weight.
            if let Some(f) = cands
                .iter()
                .find(|f| f.legacy.as_ref().is_some_and(|(_, ls)| norm(ls) == norm(if style_italic(style) { "Italic" } else { "Regular" })))
            {
                return Some((*f).clone());
            }
        }
        if cands.is_empty() {
            return None;
        }
        if let Some(f) = cands.iter().find(|f| norm(&f.style) == ns) {
            return Some((*f).clone());
        }
        let (tw, ti) = (style_weight(style), style_italic(style));
        cands
            .iter()
            .min_by(|a, b| {
                let sa = (a.weight - tw).abs() + if a.italic != ti { 1000.0 } else { 0.0 };
                let sb = (b.weight - tw).abs() + if b.italic != ti { 1000.0 } else { 0.0 };
                sa.total_cmp(&sb)
            })
            .map(|f| (*f).clone())
    }

    /// First face (fallback family first, then load order) that covers `c`; on native, system
    /// fonts are cataloged and loaded lazily the first time no loaded face covers a character.
    pub fn fallback_for(&self, c: char, exclude: u32) -> Option<Arc<FontFace>> {
        if let Some(f) = self.loaded_fallback(c, exclude) {
            return Some(f);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_fallback(c) {
            return self.loaded_fallback(c, exclude);
        }
        None
    }

    /// A covering fallback accepted by the caller (for example, an interface that cannot use
    /// colour or very large faces). Rejected faces do not stop the search for another face.
    /// A policy-specific miss is not stored as a general document-font miss.
    pub fn fallback_for_filtered(&self, c: char, exclude: u32, accept: impl Fn(&FontFace) -> bool) -> Option<Arc<FontFace>> {
        if let Some(f) = self.loaded_fallback_filtered(c, exclude, &accept) {
            return Some(f);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_fallback_filtered(c, &|f| f.id != exclude && accept(f), false) {
            return self.loaded_fallback_filtered(c, exclude, &accept);
        }
        None
    }

    fn loaded_fallback(&self, c: char, exclude: u32) -> Option<Arc<FontFace>> {
        self.loaded_fallback_filtered(c, exclude, &|_| true)
    }

    fn loaded_fallback_filtered(&self, c: char, exclude: u32, accept: &impl Fn(&FontFace) -> bool) -> Option<Arc<FontFace>> {
        let faces = self.read_faces();
        let mut order: Vec<&Arc<FontFace>> = faces.iter().filter(|f| f.id != exclude).collect();
        order.sort_by_key(|f| (!f.family.eq_ignore_ascii_case(FALLBACK_FAMILY), f.italic, (f.weight - 400.0).abs() as i32));
        order.into_iter().find(|f| f.covers(c) && accept(f)).cloned()
    }

    /// Load a system font covering `c` (preferred fallback families first, then any cataloged
    /// file under 40 MB). Returns true if one was loaded. Misses are remembered.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn system_fallback(&self, c: char) -> bool {
        self.system_fallback_filtered(c, &|_| true, true)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn system_fallback_filtered(&self, c: char, accept: &impl Fn(&FontFace) -> bool, remember_miss: bool) -> bool {
        if c.is_control() || c.is_whitespace() {
            return false;
        }
        let seen = self.read_faces().len();
        {
            let sys = self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if !sys.enabled || sys.misses.contains(&c) {
                return false;
            }
        }
        let covered = |db: &FontDb| db.read_faces().iter().any(|f| f.covers(c) && accept(f));
        let cataloged: Vec<String> = self.read_catalog().iter().map(|e| e.family.clone()).collect();
        let loading = self.loading.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Another thread may have loaded a face for `c` (faces are only ever added), or found
        // there is none, while this one waited.
        if self.read_faces().len() != seen && covered(self) {
            return true;
        }
        if self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).misses.contains(&c) {
            return false;
        }
        for fam in SYSTEM_FALLBACKS {
            if !self.is_loaded(fam) && cataloged.iter().any(|f| f.eq_ignore_ascii_case(fam)) {
                self.load_family(fam, &loading);
                if covered(self) {
                    return true;
                }
            }
        }
        let mut paths: Vec<std::path::PathBuf> =
            self.catalog.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter().map(|e| e.path.clone()).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            if std::fs::metadata(&p).map(|m| m.len() > 40 << 20).unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            let hit =
                enumerate_faces(&data).iter().any(|(i, _, _, _)| skrifa::FontRef::from_index(&data, *i).is_ok_and(|f| f.charmap().map(c).is_some()));
            if hit && self.add_font(data) > 0 && covered(self) {
                return true;
            }
        }
        if remember_miss {
            self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).misses.insert(c);
        }
        false
    }

    /// A loaded colour face drawing all of `cluster`, loading an installed colour font that covers
    /// `key` if none is loaded yet. Misses are remembered.
    pub(crate) fn color_face_for(&self, key: char, cluster: &[char]) -> Option<Arc<FontFace>> {
        let pick = |db: &FontDb| {
            let faces = db.read_faces();
            let mut v: Vec<&Arc<FontFace>> = faces.iter().filter(|f| crate::covers_cluster(f, cluster) && f.is_color()).collect();
            v.sort_by_key(|f| (f.italic, (f.weight - 400.0).abs() as i32));
            v.first().map(|f| (*f).clone())
        };
        if let Some(f) = pick(self) {
            return Some(f);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_color_fallback(key) {
            return pick(self);
        }
        let _ = key;
        None
    }

    /// A loaded plain (not colour) face drawing all of `cluster`, loading an installed plain font
    /// that covers `key` if none is loaded yet. Misses are remembered.
    pub(crate) fn plain_face_for(&self, key: char, cluster: &[char]) -> Option<Arc<FontFace>> {
        // Symbol fonts first (PowerPoint draws ⚠ ♥ ⬆ from the system's symbol font), then the
        // usual fallback families in order, then any other.
        let rank = |f: &FontFace| {
            if f.family.contains("Symbol") {
                return 0;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(i) = SYSTEM_FALLBACKS.iter().position(|s| s.eq_ignore_ascii_case(&f.family)) {
                return i + 1;
            }
            1000
        };
        let pick = |db: &FontDb| {
            let faces = db.read_faces();
            let mut v: Vec<&Arc<FontFace>> = faces.iter().filter(|f| !f.is_color() && crate::covers_cluster(f, cluster)).collect();
            v.sort_by_key(|f| (rank(f), f.italic, (f.weight - 400.0).abs() as i32));
            v.first().map(|f| (*f).clone())
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cataloged: Vec<String> =
                if self.system_fallback_enabled() { self.read_catalog().iter().map(|e| e.family.clone()).collect() } else { Vec::new() };
            for fam in SYSTEM_FALLBACKS.iter().filter(|f| f.contains("Symbol")) {
                if !self.is_loaded(fam) && cataloged.iter().any(|f| f.eq_ignore_ascii_case(fam)) {
                    self.load_cataloged(fam);
                }
            }
        }
        if let Some(f) = pick(self) {
            return Some(f);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.system_plain_fallback(key) {
            return pick(self);
        }
        let _ = key;
        None
    }

    /// Load an installed font without colour tables covering `c`. True if one was loaded.
    #[cfg(not(target_arch = "wasm32"))]
    fn system_plain_fallback(&self, c: char) -> bool {
        static MISSES: Mutex<Option<std::collections::HashSet<char>>> = Mutex::new(None);
        if c.is_control() || c.is_whitespace() {
            return false;
        }
        if !self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).enabled {
            return false;
        }
        if MISSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Default::default).contains(&c) {
            return false;
        }
        let mut paths: Vec<std::path::PathBuf> = self.read_catalog().iter().map(|e| e.path.clone()).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            if std::fs::metadata(&p).map(|m| m.len() > 40 << 20).unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            let hit = enumerate_faces(&data).iter().any(|(i, _, _, _)| {
                !has_color_tables(&data, *i) && skrifa::FontRef::from_index(&data, *i).is_ok_and(|f| f.charmap().map(c).is_some())
            });
            if hit && self.add_font(data) > 0 {
                return true;
            }
        }
        MISSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Default::default).insert(c);
        false
    }

    /// Load an installed colour font covering `c`. True if one was loaded.
    #[cfg(not(target_arch = "wasm32"))]
    fn system_color_fallback(&self, c: char) -> bool {
        static MISSES: Mutex<Option<std::collections::HashSet<char>>> = Mutex::new(None);
        if c.is_control() || c.is_whitespace() {
            return false;
        }
        {
            let sys = self.sys.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if !sys.enabled {
                return false;
            }
        }
        if MISSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Default::default).contains(&c) {
            return false;
        }
        let mut paths: Vec<std::path::PathBuf> = self.read_catalog().iter().map(|e| e.path.clone()).collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            if std::fs::metadata(&p).map(|m| m.len() > 64 << 20).unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            let hit = enumerate_faces(&data).iter().any(|(i, _, _, _)| {
                has_color_tables(&data, *i) && skrifa::FontRef::from_index(&data, *i).is_ok_and(|f| f.charmap().map(c).is_some())
            });
            if hit && self.add_font(data) > 0 {
                return true;
            }
        }
        MISSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Default::default).insert(c);
        false
    }

    /// Glyph outline in font units, y-down (flipped), cached per (face, glyph).
    pub fn outline(&self, face: &FontFace, gid: u32) -> Arc<BezPath> {
        let key = (face.id, gid);
        if let Some(p) = self.outlines.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return p;
        }
        let mut pen = FlipPen::new(MAX_OUTLINE_ELEMENTS);
        if let Some(f) = face.skrifa()
            && let Some(g) = f.outline_glyphs().get(GlyphId::new(gid))
        {
            let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), face.location()), &mut pen);
        }
        let p = Arc::new(pen.finish());
        self.outlines.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, p.clone());
        p
    }
}

/// Collects a glyph outline, y flipped, up to `limit` path elements. Past the limit it ignores
/// what follows and [`FlipPen::finish`] gives an empty path: a cut-off outline would fill wrongly.
struct FlipPen {
    path: BezPath,
    limit: usize,
    over: bool,
}

impl FlipPen {
    fn new(limit: usize) -> Self {
        FlipPen { path: BezPath::new(), limit, over: false }
    }

    /// Room for one more element?
    fn room(&mut self) -> bool {
        if !self.over && self.path.elements().len() >= self.limit {
            self.over = true;
            self.path = BezPath::new();
        }
        !self.over
    }

    fn finish(self) -> BezPath {
        if self.over { BezPath::new() } else { self.path }
    }
}

impl OutlinePen for FlipPen {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.room() {
            self.path.move_to((x as f64, -y as f64));
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if self.room() {
            self.path.line_to((x as f64, -y as f64));
        }
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        if self.room() {
            self.path.quad_to((cx0 as f64, -cy0 as f64), (x as f64, -y as f64));
        }
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        if self.room() {
            self.path.curve_to((cx0 as f64, -cy0 as f64), (cx1 as f64, -cy1 as f64), (x as f64, -y as f64));
        }
    }
    fn close(&mut self) {
        if self.room() {
            self.path.close_path();
        }
    }
}

#[cfg(test)]
mod font_dir_tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn duplicate_collection_offsets_do_not_consume_face_cap() {
        let mut entries = vec![(0, 7); MAX_COLLECTION_FACES as usize];
        entries.extend((0..MAX_COLLECTION_FACES).map(|i| (i + MAX_COLLECTION_FACES, i + 8)));
        let faces = distinct_face_indices(entries);
        assert_eq!(faces.len(), MAX_COLLECTION_FACES as usize);
        assert_eq!(faces.first(), Some(&0));
        assert_eq!(faces.get(1), Some(&MAX_COLLECTION_FACES));
        assert_eq!(faces.last(), Some(&(MAX_COLLECTION_FACES * 2 - 2)));
    }

    #[test]
    fn named_instance_limit_is_bounded_per_face() {
        assert_eq!(capped_named_instance_count(0), 0);
        assert_eq!(capped_named_instance_count(MAX_NAMED_INSTANCES_PER_FACE), MAX_NAMED_INSTANCES_PER_FACE);
        assert_eq!(capped_named_instance_count(MAX_NAMED_INSTANCES_PER_FACE + 1), MAX_NAMED_INSTANCES_PER_FACE);
    }

    #[test]
    fn faces_of_one_file_are_bounded_across_the_collection() {
        // The per-collection and per-face limits together would allow far more.
        assert!((MAX_COLLECTION_FACES as usize) * MAX_NAMED_INSTANCES_PER_FACE > 16 * MAX_ENUMERATED_FACES);
        for data in bundled() {
            assert!(enumerate_faces(data).len() <= MAX_ENUMERATED_FACES);
        }
    }

    #[test]
    fn an_outline_past_the_element_limit_draws_nothing() {
        let mut pen = FlipPen::new(8);
        pen.move_to(0.0, 0.0);
        pen.line_to(1.0, 1.0);
        pen.close();
        assert_eq!(pen.finish().elements().len(), 3);
        // A shared outline drawn over and over (composite and VARC glyphs).
        let mut pen = FlipPen::new(8);
        for i in 0..1_000_000 {
            pen.move_to(0.0, 0.0);
            pen.line_to(i as f32, 1.0);
            pen.close();
        }
        assert_eq!(pen.path.elements().len(), 0);
        assert!(pen.finish().elements().is_empty());
    }

    #[test]
    fn outline_cache_is_bounded_by_elements() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        for i in 1..MAX_OUTLINE_ELEMENTS {
            path.line_to((i as f64, 0.0));
        }
        let path = Arc::new(path);
        let mut cache = OutlineCache::default();
        let glyphs = 4 * OUTLINE_CACHE_MAX_ELEMENTS / MAX_OUTLINE_ELEMENTS;
        for gid in 0..glyphs as u32 {
            cache.insert((1, gid), path.clone());
            assert!(cache.elements <= OUTLINE_CACHE_MAX_ELEMENTS);
            assert_eq!(cache.elements, cache.paths.values().map(|p| p.elements().len()).sum::<usize>());
        }
        assert!(cache.get(&(1, glyphs as u32 - 1)).is_some());
        // Replacing an entry does not count it twice.
        let before = cache.elements;
        cache.insert((1, glyphs as u32 - 1), path.clone());
        assert_eq!(cache.elements, before);
    }

    #[test]
    fn xdg_data_dirs_contribute_their_fonts_folders() {
        let home = Path::new("/home/ann");
        assert_eq!(
            xdg_font_dirs(None, None, Some(home)),
            [home.join(".local/share/fonts"), "/usr/local/share/fonts".into(), "/usr/share/fonts".into()]
        );
        let v = xdg_font_dirs(Some("/data/ann"), Some("/nix/profile/share:relative:/var/lib/flatpak/exports/share"), Some(home));
        assert_eq!(v, [PathBuf::from("/data/ann/fonts"), "/nix/profile/share/fonts".into(), "/var/lib/flatpak/exports/share/fonts".into()]);
    }

    #[test]
    fn fontconfig_dir_entries_are_read() {
        let conf = r#"<?xml version="1.0"?>
<fontconfig>
  <dir>/usr/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>~/.fonts</dir>
  <dir>relative/fonts</dir>
  <cachedir>/var/cache/fontconfig</cachedir>
  <dirname>ignored</dirname>
  <dir>/nix/store/abc-fonts/share/fonts</dir>
</fontconfig>"#;
        let home = Path::new("/home/ann");
        let v = fontconfig_dirs(conf, None, Some(home));
        assert_eq!(
            v,
            [PathBuf::from("/usr/share/fonts"), home.join(".local/share/fonts"), home.join(".fonts"), "/nix/store/abc-fonts/share/fonts".into()]
        );
        assert!(fontconfig_dirs("<dir>/never closed", None, Some(home)).is_empty());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_dirs_include_flatpak_host_fonts_without_duplicates() {
        let d = system_font_dirs();
        assert!(d.contains(&PathBuf::from("/run/host/user-fonts")));
        let mut u = d.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), d.len());
    }
}
