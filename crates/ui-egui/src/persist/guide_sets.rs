//! Guide sets: guides saved under a name in a plain-text file that people edit by hand, pass to
//! each other and switch on with a shortcut (Ctrl+Shift+1 … 9, ⇧⌘ on macOS).
//!
//! The file is TOML, one per set, in the guide-set folder (`…/PdfCraft/Guide sets`, beside the
//! recovery folder; `PdfCraftData/Guide sets` in portable mode). The same schema is accepted as
//! JSON by the control channel (`ui.set document-guides` and `guide-set-file`), so an agent can
//! write, load and check guides without a file dialog. A set is positioned from page edges, so it
//! fits pages of other sizes:
//!
//! ```toml
//! format = "pdfcraft-guides"
//! version = 1
//! name = "A4 type area"
//! unit = "mm"            # pt, mm, cm or in; every distance below is in it
//! shortcut = 1           # Ctrl+Shift+1 shows and hides the set
//!
//! [style]                # the set's look; each guide may override it
//! color = "#00A0E6"      # #RRGGBB or #RRGGBBAA
//! width = 1.0            # in screen points
//!
//! [[guide]]
//! axis = "horizontal"    # horizontal: a distance from the top or bottom edge
//! at = 20
//! from = "top"           # top or bottom; left or right for vertical guides
//! pages = "all"          # all, odd, even, or a list of page numbers: [1, 3]
//! label = "Header"
//!
//! [[guide]]
//! axis = "vertical"
//! at = 15
//! from = "right"
//! color = "#E6005C"
//! width = 2
//!
//! [grid]                 # optional: the set brings its own grid
//! width = 10             # distance between vertical lines, in `unit`
//! height = 10
//! offset_left = 0
//! offset_top = 0
//! subdivisions = 2
//! color = "#608CD26E"
//! line_width = 1
//! ```
//!
//! A set is a complete setup: shown with its shortcut it draws its guides and, if it has one,
//! its grid; Edit in document also makes its grid and unit the current ones.
//!
//! Reading is strict about what it cannot use (an unknown key, a distance that is not a finite
//! number, a page number of 0) and says where, so a hand edit that went wrong is reported rather
//! than half applied.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::guides::{Edge, Guide, GuideAxis, Guides, Style};
use crate::units::Unit;

pub const FORMAT: &str = "pdfcraft-guides";
pub const VERSION: u32 = 1;
pub const EXTENSION: &str = "toml";
/// At most this many guides in one set, and this many bytes in one file.
pub const MAX_GUIDES: usize = 1_024;
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// At most this many sets are read from the folder.
pub const MAX_SETS: usize = 256;
/// Widths a guide may be drawn at, in screen points.
pub const MIN_WIDTH: f32 = 0.5;
pub const MAX_WIDTH: f32 = 8.0;

/// Which pages a guide of a set is shown on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Pages {
    #[default]
    All,
    Odd,
    Even,
    /// Zero-based page indexes.
    List(Vec<usize>),
}

impl Pages {
    pub fn includes(&self, page: usize) -> bool {
        match self {
            Pages::All => true,
            // Page 1 (index 0) is odd.
            Pages::Odd => page.is_multiple_of(2),
            Pages::Even => !page.is_multiple_of(2),
            Pages::List(list) => list.contains(&page),
        }
    }
}

/// One guide of a set.
#[derive(Clone, Debug, PartialEq)]
pub struct SetGuide {
    pub guide: Guide,
    pub pages: Pages,
}

/// A named set of guides, as read from its file.
#[derive(Clone, Debug, PartialEq)]
pub struct GuideSet {
    pub name: String,
    /// 1 to 9: Ctrl+Shift+digit shows and hides it.
    pub shortcut: Option<u8>,
    pub style: Style,
    pub guides: Vec<SetGuide>,
    /// The unit the file was written in; kept so a save writes it back in the same one.
    pub unit: Unit,
    /// The set's own grid, in points.
    pub grid: Option<crate::guides::GridSettings>,
}

impl GuideSet {
    /// The guides shown on `page`, in points from the page's top-left corner as displayed.
    pub fn on_page(&self, page: usize) -> impl Iterator<Item = &Guide> {
        self.guides.iter().filter(move |g| g.pages.includes(page)).map(|g| &g.guide)
    }
}

// ---------------------------------------------------------------------------------------- file

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSet {
    format: String,
    version: u32,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shortcut: Option<u8>,
    #[serde(default, skip_serializing_if = "FileStyle::is_empty")]
    style: FileStyle,
    #[serde(default, rename = "guide")]
    guides: Vec<FileGuide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grid: Option<FileGrid>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileGrid {
    width: f64,
    height: f64,
    #[serde(default)]
    offset_left: f64,
    #[serde(default)]
    offset_top: f64,
    #[serde(default = "one")]
    subdivisions: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_width: Option<f32>,
}

fn one() -> u8 {
    1
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
}

impl FileStyle {
    fn is_empty(&self) -> bool {
        self.color.is_none() && self.width.is_none()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileGuide {
    axis: String,
    at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pages: Option<FilePages>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    width: Option<f32>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum FilePages {
    Word(String),
    List(Vec<u64>),
}

pub fn unit_from_name(name: &str) -> Option<Unit> {
    match name.trim().to_ascii_lowercase().as_str() {
        "pt" | "point" | "points" => Some(Unit::Points),
        "mm" | "millimetre" | "millimetres" | "millimeter" | "millimeters" => Some(Unit::Millimetres),
        "cm" | "centimetre" | "centimetres" | "centimeter" | "centimeters" => Some(Unit::Centimetres),
        "in" | "inch" | "inches" => Some(Unit::Inches),
        _ => None,
    }
}

/// `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.is_ascii() || !(hex.len() == 6 || hex.len() == 8) {
        return None;
    }
    let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
    Some([byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 255 }])
}

pub fn color_hex(c: [u8; 4]) -> String {
    if c[3] == 255 { format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]) } else { format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3]) }
}

fn parse_style(color: Option<&str>, width: Option<f32>, at: &str) -> Result<Style, String> {
    let color = match color {
        Some(c) => Some(parse_color(c).ok_or_else(|| format!("{at}: color {c:?} is not #RRGGBB or #RRGGBBAA"))?),
        None => None,
    };
    let width = match width {
        Some(w) if w.is_finite() && (MIN_WIDTH..=MAX_WIDTH).contains(&w) => Some(w),
        Some(w) => return Err(format!("{at}: width {w} must be between {MIN_WIDTH} and {MAX_WIDTH}")),
        None => None,
    };
    Ok(Style { color, width })
}

fn from_file(f: FileSet) -> Result<GuideSet, String> {
    if f.format != FORMAT {
        return Err(format!("format must be {FORMAT:?}, not {:?}", f.format));
    }
    if f.version != VERSION {
        return Err(format!("version {} is not supported (this PdfCraft reads version {VERSION})", f.version));
    }
    let name = f.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err("name must be 1 to 120 characters".into());
    }
    let unit = match f.unit.as_deref() {
        Some(u) => unit_from_name(u).ok_or_else(|| format!("unit {u:?} must be pt, mm, cm or in"))?,
        None => Unit::Points,
    };
    let shortcut = match f.shortcut {
        Some(n @ 1..=9) => Some(n),
        Some(n) => return Err(format!("shortcut {n} must be 1 to 9")),
        None => None,
    };
    let style = parse_style(f.style.color.as_deref(), f.style.width, "style")?;
    if f.guides.len() > MAX_GUIDES {
        return Err(format!("{} guides; a set holds at most {MAX_GUIDES}", f.guides.len()));
    }
    let to_points = 1.0 / unit.per_point();
    let mut guides = Vec::with_capacity(f.guides.len());
    for (i, g) in f.guides.into_iter().enumerate() {
        let at_ = format!("guide {}", i + 1);
        let axis = match g.axis.trim().to_ascii_lowercase().as_str() {
            "horizontal" | "h" => GuideAxis::Horizontal,
            "vertical" | "v" => GuideAxis::Vertical,
            other => return Err(format!("{at_}: axis {other:?} must be horizontal or vertical")),
        };
        let from = match (axis, g.from.as_deref().map(|s| s.trim().to_ascii_lowercase())) {
            (_, None) => Edge::Start,
            (GuideAxis::Horizontal, Some(s)) if s == "top" => Edge::Start,
            (GuideAxis::Horizontal, Some(s)) if s == "bottom" => Edge::End,
            (GuideAxis::Vertical, Some(s)) if s == "left" => Edge::Start,
            (GuideAxis::Vertical, Some(s)) if s == "right" => Edge::End,
            (GuideAxis::Horizontal, Some(s)) => return Err(format!("{at_}: a horizontal guide is measured from top or bottom, not {s:?}")),
            (GuideAxis::Vertical, Some(s)) => return Err(format!("{at_}: a vertical guide is measured from left or right, not {s:?}")),
        };
        let at = g.at * to_points;
        if !at.is_finite() || at.abs() > 1e6 {
            return Err(format!("{at_}: at {} is not a usable distance", g.at));
        }
        let pages = match g.pages {
            None => Pages::All,
            Some(FilePages::Word(w)) => match w.trim().to_ascii_lowercase().as_str() {
                "all" => Pages::All,
                "odd" => Pages::Odd,
                "even" => Pages::Even,
                other => return Err(format!("{at_}: pages {other:?} must be all, odd, even or a list of page numbers")),
            },
            Some(FilePages::List(list)) => {
                if list.len() > MAX_GUIDES {
                    return Err(format!("{at_}: too many page numbers"));
                }
                let mut pages = Vec::with_capacity(list.len());
                for n in list {
                    let n = usize::try_from(n).ok().and_then(|n| n.checked_sub(1)).ok_or_else(|| format!("{at_}: page numbers start at 1"))?;
                    pages.push(n);
                }
                Pages::List(pages)
            }
        };
        let label = g.label.map(|l| l.chars().take(120).collect::<String>()).filter(|l| !l.trim().is_empty());
        let style = parse_style(g.color.as_deref(), g.width, &at_)?;
        guides.push(SetGuide { guide: Guide { axis, at: at as f32, from, style, label }, pages });
    }
    let grid = match f.grid {
        None => None,
        Some(g) => {
            let num = |v: f64, what: &str| {
                let pt = v * to_points;
                if pt.is_finite() && pt.abs() <= f64::from(crate::guides::MAX_GRID_SPACING) {
                    Ok(pt as f32)
                } else {
                    Err(format!("grid: {what} {v} is not a usable distance"))
                }
            };
            let (width, height) = (num(g.width, "width")?, num(g.height, "height")?);
            if f64::from(width.min(height)) < f64::from(crate::guides::MIN_GRID_SPACING) {
                return Err(format!("grid: lines must be at least {} pt apart", crate::guides::MIN_GRID_SPACING));
            }
            if !(1..=crate::guides::MAX_SUBDIVISIONS).contains(&g.subdivisions) {
                return Err(format!("grid: subdivisions must be 1 to {}", crate::guides::MAX_SUBDIVISIONS));
            }
            let look = parse_style(g.color.as_deref(), g.line_width, "grid")?;
            let d = crate::guides::GridSettings::default();
            Some(crate::guides::GridSettings {
                width,
                height,
                offset_left: num(g.offset_left, "offset_left")?,
                offset_top: num(g.offset_top, "offset_top")?,
                subdivisions: g.subdivisions,
                grid_color: look.color.unwrap_or(d.grid_color),
                grid_line_width: look.width.unwrap_or(d.grid_line_width),
                ..d
            })
        }
    };
    Ok(GuideSet { name, shortcut, style, guides, unit, grid })
}

fn to_file(set: &GuideSet) -> FileSet {
    let k = f64::from(set.unit.per_point() as f32);
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    FileSet {
        format: FORMAT.into(),
        version: VERSION,
        name: set.name.clone(),
        unit: Some(set.unit.label().into()),
        shortcut: set.shortcut,
        style: FileStyle { color: set.style.color.map(color_hex), width: set.style.width },
        guides: set
            .guides
            .iter()
            .map(|g| FileGuide {
                axis: if g.guide.axis == GuideAxis::Horizontal { "horizontal" } else { "vertical" }.into(),
                at: round(f64::from(g.guide.at) * k),
                from: match (g.guide.axis, g.guide.from) {
                    (_, Edge::Start) => None,
                    (GuideAxis::Horizontal, Edge::End) => Some("bottom".into()),
                    (GuideAxis::Vertical, Edge::End) => Some("right".into()),
                },
                pages: match &g.pages {
                    Pages::All => None,
                    Pages::Odd => Some(FilePages::Word("odd".into())),
                    Pages::Even => Some(FilePages::Word("even".into())),
                    Pages::List(l) => Some(FilePages::List(l.iter().map(|p| *p as u64 + 1).collect())),
                },
                label: g.guide.label.clone(),
                color: g.guide.style.color.map(color_hex),
                width: g.guide.style.width,
            })
            .collect(),
        grid: set.grid.map(|g| FileGrid {
            width: round(f64::from(g.width) * k),
            height: round(f64::from(g.height) * k),
            offset_left: round(f64::from(g.offset_left) * k),
            offset_top: round(f64::from(g.offset_top) * k),
            subdivisions: g.subdivisions,
            color: Some(color_hex(g.grid_color)),
            line_width: Some(g.grid_line_width),
        }),
    }
}

const HEADER: &str = "# PdfCraft guide set. Edit freely: PdfCraft reloads the folder when Preferences ▸ Units and guides\n\
# opens or Reload is clicked. Keys: name, unit (pt|mm|cm|in), shortcut (1-9, Ctrl+Shift+digit),\n\
# [style] color (#RRGGBB[AA]) and width (screen points), and one [[guide]] per guide with\n\
# axis (horizontal|vertical), at, from (top|bottom, left|right), pages (all|odd|even|[1, 3]),\n\
# label, color and width. An optional [grid] (width, height, offset_left, offset_top,\n\
# subdivisions, color, line_width) makes the set bring its own grid.\n";

/// The set as the text of its file.
pub fn write_toml(set: &GuideSet) -> Result<String, String> {
    toml::to_string_pretty(&to_file(set)).map(|body| format!("{HEADER}\n{body}")).map_err(|e| e.to_string())
}

/// The set as JSON, the same schema as the file (for the control channel).
pub fn to_json(set: &GuideSet) -> serde_json::Value {
    serde_json::to_value(to_file(set)).unwrap_or_default()
}

pub fn read_toml(text: &str) -> Result<GuideSet, String> {
    from_file(toml::from_str(text).map_err(|e| e.to_string())?)
}

/// A set given as JSON (an object) or as the text of a TOML file.
pub fn read_any(text: &str) -> Result<GuideSet, String> {
    if text.trim_start().starts_with('{') { from_file(serde_json::from_str(text).map_err(|e| e.to_string())?) } else { read_toml(text) }
}

// ---------------------------------------------------------------------------------------- folder

/// Where guide sets are kept. `None` on the web, which has no folder.
pub fn default_dir() -> Option<PathBuf> {
    #[cfg(target_arch = "wasm32")]
    return None;
    #[cfg(not(target_arch = "wasm32"))]
    crate::recovery::RecoveryStore::default_dir().and_then(|d| d.parent().map(|p| p.join("Guide sets")))
}

/// One file of the folder: its set, or why it could not be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub path: PathBuf,
    pub set: Result<GuideSet, String>,
}

impl Loaded {
    /// The name the set is shown and switched by: its own name, or the file's when it failed to read.
    pub fn name(&self) -> String {
        match &self.set {
            Ok(s) => s.name.clone(),
            Err(_) => self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        }
    }
}

/// The sets of `sets` named in `shown` that read without errors, in folder order.
pub fn shown<'a>(sets: &'a [Loaded], shown: &std::collections::BTreeSet<String>) -> Vec<&'a GuideSet> {
    sets.iter().filter_map(|l| l.set.as_ref().ok()).filter(|s| shown.contains(&s.name)).collect()
}

/// Every `*.toml` in `dir`, by file name. A missing folder is no sets.
pub fn load_dir(dir: &Path) -> Vec<Loaded> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION)) && p.is_file())
        .take(MAX_SETS)
        .collect();
    paths.sort();
    paths.into_iter().map(|path| Loaded { set: read_file(&path), path }).collect()
}

pub fn read_file(path: &Path) -> Result<GuideSet, String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path).and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_string(&mut text)).map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{}: larger than {MAX_FILE_BYTES} bytes", path.display()));
    }
    read_toml(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A file name for `name`: letters, digits, spaces, `-` and `_` kept, anything else dropped.
pub fn file_name(name: &str) -> String {
    let stem: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(80).collect();
    let stem = stem.trim();
    format!("{}.{EXTENSION}", if stem.is_empty() { "guides" } else { stem })
}

/// Write `set` to `dir` under its name, replacing an earlier version of the same set. Returns
/// the path. Different names can give the same file name ("Plan/A" and "PlanA"); a file that
/// holds another set, or that doesn't read, is never replaced.
pub fn save(dir: &Path, set: &GuideSet) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file_name(&set.name));
    if path.exists() {
        match read_file(&path) {
            Ok(existing) if existing.name == set.name.trim() => {}
            Ok(existing) => return Err(format!("{} already holds the guide set {:?}; choose another name", path.display(), existing.name)),
            Err(e) => return Err(format!("{e}; fix or remove that file, or choose another name")),
        }
    }
    let text = write_toml(set)?;
    let tmp = path.with_extension("toml.part");
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------------------- document

/// The document's guides as a set: guides on every page stay on every page, a page's own guides
/// name that page.
pub fn from_document(name: &str, guides: &Guides, unit: Unit, shortcut: Option<u8>, grid: Option<crate::guides::GridSettings>) -> GuideSet {
    let mut out: Vec<SetGuide> = guides.all().iter().map(|g| SetGuide { guide: g.clone(), pages: Pages::All }).collect();
    for (page, list) in guides.by_page() {
        out.extend(list.iter().map(|g| SetGuide { guide: g.clone(), pages: Pages::List(vec![page]) }));
    }
    out.truncate(MAX_GUIDES);
    GuideSet { name: name.trim().to_string(), shortcut, style: Style::default(), guides: out, unit, grid }
}

/// Replace the document's guides with the set's (`page_count` pages). Guides for all pages stay
/// guides for all pages; odd, even and listed ones are put on each such page. Returns how many
/// guides the document has afterwards.
pub fn apply_to_document(set: &GuideSet, guides: &mut Guides, page_sizes: &[[f32; 2]]) -> usize {
    guides.clear();
    for g in &set.guides {
        let mut guide = g.guide.clone();
        guide.style = guide.style.or(set.style);
        match &g.pages {
            Pages::All => {
                guides.add_all(guide);
            }
            pages => {
                for (page, size) in page_sizes.iter().enumerate().filter(|(p, _)| pages.includes(*p)) {
                    guides.add(page, guide.clone(), *size);
                }
            }
        }
    }
    guides.count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
format = "pdfcraft-guides"
version = 1
name = "A4 type area"
unit = "mm"
shortcut = 2

[style]
color = "#00A0E6"
width = 1.5

[[guide]]
axis = "horizontal"
at = 25.4
from = "top"
label = "Header"

[[guide]]
axis = "vertical"
at = 10
from = "right"
pages = "even"
color = "#FF000080"
width = 2

[[guide]]
axis = "v"
at = 72
pages = [1, 3]
"##;

    #[test]
    fn a_hand_written_file_reads_in_points_with_its_edges_pages_and_styles() {
        let set = read_toml(SAMPLE).unwrap();
        assert_eq!(set.name, "A4 type area");
        assert_eq!(set.shortcut, Some(2));
        assert_eq!(set.style, Style { color: Some([0, 160, 230, 255]), width: Some(1.5) });
        let g = &set.guides;
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].guide.axis, g[0].guide.from, g[0].pages.clone()), (GuideAxis::Horizontal, Edge::Start, Pages::All));
        assert!((g[0].guide.at - 72.0).abs() < 1e-3, "25.4 mm is 72 pt");
        assert_eq!(g[0].guide.label.as_deref(), Some("Header"));
        assert_eq!((g[1].guide.from, g[1].pages.clone()), (Edge::End, Pages::Even));
        assert_eq!(g[1].guide.style, Style { color: Some([255, 0, 0, 128]), width: Some(2.0) });
        assert_eq!(g[2].pages, Pages::List(vec![0, 2]), "page numbers start at 1");
        assert!((g[2].guide.at - 72.0 * 72.0 / 25.4).abs() < 1e-2);
    }

    #[test]
    fn writing_and_reading_back_gives_the_same_set() {
        let set = read_toml(SAMPLE).unwrap();
        let text = write_toml(&set).unwrap();
        assert!(text.starts_with("# PdfCraft guide set"), "{text}");
        let back = read_toml(&text).unwrap();
        assert_eq!(back.name, set.name);
        assert_eq!(back.guides.len(), 3);
        for (a, b) in back.guides.iter().zip(&set.guides) {
            assert!((a.guide.at - b.guide.at).abs() < 0.01, "{a:?} {b:?}");
            assert_eq!((a.guide.axis, a.guide.from, &a.pages, a.guide.style), (b.guide.axis, b.guide.from, &b.pages, b.guide.style));
        }
        let json = serde_json::to_string(&to_json(&set)).unwrap();
        assert_eq!(read_any(&json).unwrap().guides.len(), 3, "JSON uses the same schema");
    }

    #[test]
    fn mistakes_are_reported_with_where_they_are() {
        let bad = |edit: &str| read_toml(&format!("format = \"pdfcraft-guides\"\nversion = 1\nname = \"x\"\n{edit}")).unwrap_err();
        assert!(bad("[[guide]]\naxis = \"diagonal\"\nat = 1").contains("guide 1"));
        assert!(bad("[[guide]]\naxis = \"horizontal\"\nat = 1\nfrom = \"left\"").contains("top or bottom"));
        assert!(bad("[[guide]]\naxis = \"vertical\"\nat = 1\npages = [0]").contains("start at 1"));
        assert!(bad("[[guide]]\naxis = \"vertical\"\nat = 1\nwidth = 99").contains("between"));
        assert!(bad("[[guide]]\naxis = \"vertical\"\nat = 1\ncolor = \"red\"").contains("#RRGGBB"));
        assert!(bad("shortcut = 0").contains("1 to 9"));
        assert!(bad("unit = \"furlong\"").contains("pt, mm"));
        assert!(bad("colour = \"#000000\"").contains("unknown field"), "a misspelt key is not silently ignored");
        assert!(read_toml("format = \"other\"\nversion = 1\nname = \"x\"").unwrap_err().contains("format"));
        assert!(read_toml("format = \"pdfcraft-guides\"\nversion = 2\nname = \"x\"").unwrap_err().contains("version 2"));
        assert!(read_toml("not toml at all [").is_err());
    }

    #[test]
    fn a_set_can_bring_its_own_grid() {
        let text = format!("{SAMPLE}\n[grid]\nwidth = 10\nheight = 5\nsubdivisions = 2\ncolor = \"#112233\"\n");
        let set = read_toml(&text).unwrap();
        let g = set.grid.unwrap();
        assert!((g.width - 28.346).abs() < 0.01 && (g.height - 14.173).abs() < 0.01, "mm to points: {g:?}");
        assert_eq!((g.subdivisions, g.grid_color), (2, [0x11, 0x22, 0x33, 255]));
        let back = read_toml(&write_toml(&set).unwrap()).unwrap().grid.unwrap();
        assert!((back.width - g.width).abs() < 0.01);
        let bad = |grid: &str| read_toml(&format!("{SAMPLE}\n[grid]\n{grid}\n")).unwrap_err();
        assert!(bad("width = 0\nheight = 5").contains("at least"));
        assert!(bad("width = 5\nheight = 5\nsubdivisions = 0").contains("subdivisions"));
        assert!(bad("width = 5").contains("height"), "height is required");
    }

    #[test]
    fn colours_parse_with_and_without_alpha() {
        assert_eq!(parse_color("#00a0e6"), Some([0, 160, 230, 255]));
        assert_eq!(parse_color(" #00A0E680 "), Some([0, 160, 230, 128]));
        assert_eq!(parse_color("00A0E6"), None);
        assert_eq!(parse_color("#00A0E"), None);
        assert_eq!(parse_color("#ÄÖÜÄÖÜ"), None);
        assert_eq!(color_hex([0, 160, 230, 255]), "#00A0E6");
        assert_eq!(color_hex([1, 2, 3, 4]), "#01020304");
    }

    #[test]
    fn odd_and_even_count_pages_from_one() {
        assert!(Pages::Odd.includes(0) && !Pages::Odd.includes(1));
        assert!(Pages::Even.includes(1) && !Pages::Even.includes(2));
    }

    #[test]
    fn file_names_keep_only_safe_characters() {
        assert_eq!(file_name("A4 type area"), "A4 type area.toml");
        assert_eq!(file_name("../../etc/passwd"), "etcpasswd.toml");
        assert_eq!(file_name("///"), "guides.toml");
        assert_eq!(file_name("Größe"), "Größe.toml");
    }

    #[test]
    fn a_document_round_trips_through_a_set() {
        let mut doc = Guides::default();
        doc.add_all(Guide::new(GuideAxis::Horizontal, 36.0));
        doc.add(1, Guide::new(GuideAxis::Vertical, 50.0), [200.0, 100.0]);
        let set = from_document("Mine", &doc, Unit::Millimetres, Some(3), None);
        assert_eq!(set.guides.len(), 2);
        assert_eq!(set.guides[1].pages, Pages::List(vec![1]));
        let mut other = Guides::default();
        other.add(0, Guide::new(GuideAxis::Vertical, 1.0), [200.0, 100.0]);
        assert_eq!(apply_to_document(&set, &mut other, &[[200.0, 100.0]; 3]), 2, "the old guides are replaced");
        assert_eq!(other.all().len(), 1);
        assert_eq!(other.on(1).len(), 1);
        assert!(other.on(0).is_empty());
    }

    #[test]
    fn the_folder_lists_sets_and_reports_broken_files() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-guide-sets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_dir(&dir).is_empty(), "a missing folder is no sets");
        let set = read_toml(SAMPLE).unwrap();
        let path = save(&dir, &set).unwrap();
        assert_eq!(path.file_name().unwrap(), "A4 type area.toml");
        std::fs::write(dir.join("broken.toml"), "name = ").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a set").unwrap();
        let loaded = load_dir(&dir);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name(), "A4 type area");
        assert!(loaded[1].set.is_err() && loaded[1].name() == "broken");
        assert_eq!(save(&dir, &set).unwrap(), path, "the same set saves over itself");
        let other = GuideSet { name: "A4: type area".into(), ..set.clone() };
        assert!(save(&dir, &other).unwrap_err().contains("already holds"), "another set with the same file name");
        let broken = GuideSet { name: "broken".into(), ..set };
        assert!(save(&dir, &broken).is_err(), "a file that doesn't read stays");
        assert_eq!(std::fs::read_to_string(dir.join("broken.toml")).unwrap(), "name = ");
        std::fs::remove_dir_all(&dir).ok();
    }
}
