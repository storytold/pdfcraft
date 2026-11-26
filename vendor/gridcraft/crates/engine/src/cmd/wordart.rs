//! Insert › Text › WordArt: decorative text as a floating shape (`ShapeKind::WordArt`) built on
//! the text-box machinery (anchor, move/resize/delete, undo, XLSX shape text body, printing).
//! The style palette below is original to GridCraft.

use gridcraft_model::{Anchor, Color, Shape, ShapeFormat, ShapeKind, ShapeTextAlign, ShapeTextAnchor};
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, PROTECTED, Result, Session, anchor_mode_param, bad, bool_param, cell_param, cmd, edit, f64_param, has_doc, ok,
    u32_param,
};

/// One entry of the style gallery.
pub struct WordArtStyle {
    pub name: &'static str,
    pub fill: Color,
    pub outline: Option<(Color, f32)>,
    pub shadow: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    /// Font size in points.
    pub size: f32,
}

const fn rgb(v: u32) -> Color {
    Color::Rgb(v)
}

/// GridCraft's own WordArt presets. Theme colours where it makes sense (they follow the theme),
/// fixed colours for the rest.
pub const STYLES: [WordArtStyle; 10] = [
    WordArtStyle { name: "Classic", fill: Color::Theme(4, 0), outline: None, shadow: None, bold: true, italic: false, size: 36.0 },
    WordArtStyle { name: "Midnight", fill: rgb(0x1B2A49), outline: None, shadow: Some(rgb(0x8A94A6)), bold: true, italic: false, size: 40.0 },
    WordArtStyle { name: "Sunrise", fill: rgb(0xF2A33A), outline: Some((rgb(0xB5451B), 1.5)), shadow: None, bold: true, italic: false, size: 40.0 },
    WordArtStyle {
        name: "Lagoon",
        fill: rgb(0x1F9E9A),
        outline: Some((rgb(0x0B4F5C), 1.0)),
        shadow: Some(rgb(0x9CC9C7)),
        bold: true,
        italic: true,
        size: 36.0,
    },
    WordArtStyle { name: "Ember", fill: rgb(0xD64527), outline: None, shadow: Some(rgb(0x5A1A0E)), bold: true, italic: false, size: 44.0 },
    WordArtStyle {
        name: "Mint Outline",
        fill: rgb(0xFFFFFF),
        outline: Some((rgb(0x2E9B6A), 2.0)),
        shadow: None,
        bold: true,
        italic: false,
        size: 40.0,
    },
    WordArtStyle { name: "Ink Stamp", fill: rgb(0x2F3A8F), outline: Some((rgb(0x2F3A8F), 0.5)), shadow: None, bold: true, italic: false, size: 32.0 },
    WordArtStyle { name: "Orchid", fill: rgb(0x8E4FB5), outline: None, shadow: Some(rgb(0xD9C2E8)), bold: false, italic: true, size: 40.0 },
    WordArtStyle {
        name: "Graphite",
        fill: rgb(0x4A4F57),
        outline: Some((rgb(0xFFFFFF), 1.0)),
        shadow: Some(rgb(0x9AA0A8)),
        bold: true,
        italic: false,
        size: 36.0,
    },
    WordArtStyle {
        name: "Lime Pop",
        fill: rgb(0xA6D61C),
        outline: Some((rgb(0x3F5A00), 1.5)),
        shadow: Some(rgb(0x6E8B2C)),
        bold: true,
        italic: true,
        size: 40.0,
    },
];

/// The text of a WordArt inserted without `text`.
pub const DEFAULT_TEXT: &str = "Your text here";
/// Longest WordArt text (characters).
pub const MAX_CHARS: usize = 1000;
const MAX_POINTS: f64 = 400.0;
const MAX_SIDE: f64 = 4000.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "insert.wordArt",
            "WordArt",
            ["Insert", "Text"],
            None,
            "{style?: preset name or 0-based index (Classic, Midnight, Sunrise, Lagoon, Ember, Mint Outline, Ink Stamp, Orchid, Graphite, Lime Pop), text?, at?, width?, height?, fontSize?, bold?, italic?, fill?: hex, outline?: hex|false, outlineWidth?, shadow?: hex|bool}",
            has_doc,
            insert_word_art
        ),
        cmd!(
            "wordArt.set",
            "Format WordArt",
            [],
            None,
            "{id: WordArt id, style?, text?, fontSize?, bold?, italic?, fill?: hex, outline?: hex|false, outlineWidth?, shadow?: hex|bool}",
            has_doc,
            set_word_art
        ),
    ]
}

fn style_param(command: &str, p: &Json) -> Result<Option<&'static WordArtStyle>> {
    match p.get("style") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Number(n)) => n
            .as_u64()
            .and_then(|i| STYLES.get(usize::try_from(i).ok()?))
            .map(Some)
            .ok_or_else(|| bad(command, format!("`style` index must be 0..{}", STYLES.len() - 1))),
        Some(Json::String(name)) => STYLES
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name.trim()))
            .map(Some)
            .ok_or_else(|| bad(command, format!("unknown WordArt style `{name}`"))),
        Some(_) => Err(bad(command, "`style` must be a preset name or index")),
    }
}

fn text_param(command: &str, p: &Json) -> Result<Option<String>> {
    match p.get("text") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(t)) if t.is_empty() => Err(bad(command, "`text` must not be empty")),
        Some(Json::String(t)) if t.chars().nth(MAX_CHARS).is_some() => Err(bad(command, format!("`text` is longer than {MAX_CHARS} characters"))),
        Some(Json::String(t)) => Ok(Some(t.clone())),
        Some(_) => Err(bad(command, "`text` must be a string")),
    }
}

/// Applies a preset, then explicit overrides, to a shape's format and fill.
fn apply(command: &str, shape: &mut Shape, style: Option<&WordArtStyle>, p: &Json) -> Result<()> {
    if let Some(st) = style {
        shape.format.text_color = Some(st.fill);
        shape.format.text_outline = st.outline;
        shape.format.text_shadow = st.shadow;
        shape.format.bold = st.bold;
        shape.format.italic = st.italic;
        shape.format.font_size = Some(st.size);
    }
    let f = &mut shape.format;
    if let Some(c) = p.get("fill").filter(|v| !v.is_null()) {
        f.text_color = Some(super::format::color_param(Some(c)).ok_or_else(|| bad(command, "`fill` must be a hex colour"))?);
    }
    match p.get("outline") {
        None | Some(Json::Null) => {}
        Some(Json::Bool(false)) => f.text_outline = None,
        Some(v) => {
            let c = super::format::color_param(Some(v)).ok_or_else(|| bad(command, "`outline` must be a hex colour or false"))?;
            let w = f64_param(p, "outlineWidth").map_or(f.text_outline.map_or(1.5, |o| o.1), |w| w.clamp(0.25, 20.0) as f32);
            f.text_outline = Some((c, w));
        }
    }
    if let (Some(w), Some((c, _))) = (f64_param(p, "outlineWidth"), f.text_outline) {
        f.text_outline = Some((c, w.clamp(0.25, 20.0) as f32));
    }
    match p.get("shadow") {
        None | Some(Json::Null) => {}
        Some(Json::Bool(false)) => f.text_shadow = None,
        Some(Json::Bool(true)) => f.text_shadow = Some(f.text_shadow.unwrap_or(rgb(0x808080))),
        Some(v) => {
            f.text_shadow = Some(super::format::color_param(Some(v)).ok_or_else(|| bad(command, "`shadow` must be a hex colour or a boolean"))?)
        }
    }
    if let Some(b) = bool_param(p, "bold") {
        f.bold = b;
    }
    if let Some(b) = bool_param(p, "italic") {
        f.italic = b;
    }
    if let Some(sz) = f64_param(p, "fontSize") {
        f.font_size = Some(sz.clamp(1.0, MAX_POINTS) as f32);
    }
    Ok(())
}

fn insert_word_art(s: &mut Session, p: &Json) -> Result<Json> {
    let style = style_param("insert.wordArt", p)?.unwrap_or(&STYLES[0]);
    let text = text_param("insert.wordArt", p)?.unwrap_or_else(|| DEFAULT_TEXT.to_string());
    let d = s.doc()?;
    if d.wb.active().is_some_and(|sh| sh.is_protected()) {
        return Err(EngineError::Other(PROTECTED.into()));
    }
    let id = d.wb.next_object_id();
    let at = cell_param(s, p, "at")?.unwrap_or(d.selection.active);
    let mut shape = Shape {
        id,
        kind: ShapeKind::WordArt,
        anchor: Anchor { cell: at, dx: 0.0, dy: 0.0, width: 0.0, height: 0.0, mode: anchor_mode_param(p).unwrap_or_default() },
        fill: Color::Auto,
        line: Color::Auto,
        text: text.clone(),
        format: ShapeFormat { no_line: true, align: Some(ShapeTextAlign::Center), anchor: Some(ShapeTextAnchor::Middle), ..Default::default() },
        math: Default::default(),
        rotation: 0.0,
    };
    apply("insert.wordArt", &mut shape, Some(style), p)?;
    // Default box: the longest line at the font size (about 0.62 em per character) plus padding.
    let pt = f64::from(shape.format.font_size.unwrap_or(36.0));
    let longest = text.split('\n').map(|l| l.chars().count()).max().unwrap_or(1) as f64;
    let lines = text.split('\n').count() as f64;
    let width = f64_param(p, "width").unwrap_or(longest * pt * 0.62 * 96.0 / 72.0 + 24.0).clamp(8.0, MAX_SIDE);
    let height = f64_param(p, "height").unwrap_or(lines * pt * 1.35 * 96.0 / 72.0 + 12.0).clamp(8.0, MAX_SIDE);
    shape.anchor.width = width as f32;
    shape.anchor.height = height as f32;
    let sheet = d.wb.active_sheet();
    edit(s, |cx| {
        cx.sheet_mut(sheet)?.shapes.push(shape);
        Ok(json!({"shape": id}))
    })
}

fn set_word_art(s: &mut Session, p: &Json) -> Result<Json> {
    let id = u32_param(p, "id").ok_or_else(|| bad("wordArt.set", "`id` must be a WordArt id"))?;
    let style = style_param("wordArt.set", p)?;
    let text = text_param("wordArt.set", p)?;
    let d = s.doc()?;
    let sheet = d.wb.active_sheet();
    let sh = d.wb.active().ok_or_else(|| bad("wordArt.set", "no such WordArt"))?;
    let current = sh.shapes.iter().find(|x| x.id == id).ok_or_else(|| bad("wordArt.set", "no such WordArt"))?;
    if current.kind != ShapeKind::WordArt {
        return Err(bad("wordArt.set", "the shape is not WordArt"));
    }
    if sh.is_protected() {
        return Err(EngineError::Other(PROTECTED.into()));
    }
    let mut updated = current.clone();
    apply("wordArt.set", &mut updated, style, p)?;
    if let Some(t) = text {
        updated.text = t;
    }
    if &updated == current {
        return ok();
    }
    edit(s, |cx| {
        let shape = cx.sheet_mut(sheet)?.shapes.iter_mut().find(|x| x.id == id).ok_or_else(|| bad("wordArt.set", "no such WordArt"))?;
        *shape = updated;
        Ok(Json::Null)
    })
}
