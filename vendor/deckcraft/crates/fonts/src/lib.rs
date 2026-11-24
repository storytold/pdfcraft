//! DeckCraft fonts: the font database (craft-fonts families, user and system fonts, and open
//! substitutes for the Office fonts presentations ask for), vertical metrics, glyph outlines and
//! OpenType shaping, script/slot classification and per-cluster fallback.
//!
//! Shaping here is style-agnostic: [`shape`] and [`shape_range`] turn text in one face into glyph
//! ids, clusters and advances in font units. `deckcraft-text` applies sizes, tracking, scaling and
//! justification on top.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod fallback;
mod fontdb;
pub mod script;
mod shape;

pub use fallback::{bundled_families, covers_cluster, is_emoji_cluster, ligature_carets};
pub use fontdb::{
    FALLBACK_FAMILY, FaceRef, FontDb, FontFace, LAST_RESORT_FAMILY, MISSING_FONT_FAMILY, base_style, bundled, is_monospace_family, substitutes,
    system_font_dirs,
};
pub use harfrust::Feature;
use harfrust::Tag;
pub use kurbo::BezPath;
pub use script::{FontSlot, ScriptTag};
pub use shape::{ShapeParams, ShapedGlyph, shape_range};

/// A font from the optional craft-fonts build input (https://github.com/storytold/craft-fonts;
/// empty unless built with `CRAFT_FONTS_DIR`, see `build.rs`).
pub struct CraftFont {
    pub family: &'static str,
    pub style: &'static str,
    /// ISO 15924 scripts the font is for, e.g. `"Jpan"`.
    pub scripts: &'static [&'static str],
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/craft_fonts.rs"));

/// The craft-fonts faces for Japanese (`"Jpan"`), in manifest order. Empty without craft-fonts.
pub fn japanese_fonts() -> impl Iterator<Item = &'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.scripts.contains(&"Jpan"))
}

/// The TrueType or OpenType font inside a PresentationML embedded-font part
/// (`application/x-fontdata`).
///
/// The part is the font itself, or an Embedded OpenType wrapper around it (the `0x504C` magic
/// at byte 34, from the W3C Embedded OpenType submission). A MicroType Express subset is neither,
/// so this returns none; the original part stays in the file for the round trip.
pub fn embedded_sfnt(part: &[u8]) -> Option<&[u8]> {
    eot_payload(part).or_else(|| sfnt_or_collection(part).then_some(part))
}

fn sfnt_or_collection(b: &[u8]) -> bool {
    b.len() >= 12
        && (b.starts_with(&[0x00, 0x01, 0x00, 0x00])
            || b.starts_with(b"OTTO")
            || b.starts_with(b"true")
            || b.starts_with(b"typ1")
            || b.starts_with(b"ttcf"))
}

/// Font data at the end of an Embedded OpenType blob, when the header is one.
fn eot_payload(b: &[u8]) -> Option<&[u8]> {
    if b.len() < 82 || u16::from_le_bytes([b[34], b[35]]) != 0x504C {
        return None;
    }
    let version = u32::from_le_bytes(b.get(8..12)?.try_into().ok()?);
    if !matches!(version, 0x0001_0000 | 0x0002_0001 | 0x0002_0002) {
        return None;
    }
    let eot_size = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let font_size = u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) as usize;
    if font_size < 12 || eot_size < font_size.saturating_add(82) || eot_size > b.len() {
        return None;
    }
    let font = b.get(eot_size - font_size..eot_size)?;
    sfnt_or_collection(font).then_some(font)
}

/// Japanese faces in document-fallback order: Mincho (serif) families first, matching the
/// serif default text font, then the others; Regular before other styles.
pub fn japanese_document_fonts() -> Vec<&'static CraftFont> {
    let mut v: Vec<_> = japanese_fonts().collect();
    v.sort_by_key(|f| (!f.family.contains("Mincho"), f.style != "Regular"));
    v
}

/// Japanese faces in UI order: BIZ UDPGothic first (the UI face), then the others; `bold` puts
/// bold styles before regular ones.
pub fn japanese_ui_fonts(bold: bool) -> Vec<&'static CraftFont> {
    let mut v: Vec<_> = japanese_fonts().collect();
    v.sort_by_key(|f| (f.family != "BIZ UDPGothic", (f.style == "Bold") != bold));
    v
}

/// The default theme's font.
pub const DEFAULT_FAMILY: &str = "Inter";

/// An OpenType feature setting: `"liga"`, `"-kern"`, `"ss01"`.
pub fn feature(tag: &str) -> Option<Feature> {
    let (on, t) = match tag.strip_prefix('-') {
        Some(r) => (false, r),
        None => (true, tag.strip_prefix('+').unwrap_or(tag)),
    };
    let b = t.as_bytes();
    if b.len() != 4 {
        return None;
    }
    Some(Feature::new(Tag::new(&[b[0], b[1], b[2], b[3]]), on as u32, ..))
}

/// A strong right-to-left character (Hebrew, Arabic, …)?
pub fn is_rtl(c: char) -> bool {
    use unicode_bidi::BidiClass::{AL, R};
    matches!(unicode_bidi::bidi_class(c), R | AL)
}

/// Shape `text` with `face`. `chars` lets callers substitute characters (e.g. uppercase for All
/// Caps) while keeping clusters pointing into the original string. Glyphs come in logical
/// order: each direction run is shaped with [`shape_range`] (right-to-left runs right to left,
/// with the whole string as context), then its clusters put back in text order (each cluster's
/// glyphs keep the shaper's order). Paragraph layout uses [`shape_range`] directly and keeps the
/// visual order.
pub fn shape(face: &FontFace, text: &str, features: &[Feature], map: impl Fn(char) -> char) -> Vec<ShapedGlyph> {
    let mut out = Vec::with_capacity(text.len());
    for (r, rtl) in direction_runs(text) {
        let mut g = shape_range(face, text, r, &ShapeParams { features, rtl, ..Default::default() }, &map);
        if rtl {
            // Visual (clusters descending) → logical; the sort is stable, so each cluster's
            // glyphs keep the shaper's order.
            g.sort_by_key(|x| x.cluster);
        }
        out.extend(g);
    }
    out
}

/// Maximal runs of one direction (neutrals join the run they're in): (byte range, right-to-left).
fn direction_runs(text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut runs: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
    let mut cur: Option<bool> = None;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        let strong = if is_rtl(c) {
            Some(true)
        } else if unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::L {
            Some(false)
        } else {
            None
        };
        match (cur, strong) {
            (None, Some(d)) => cur = Some(d),
            (Some(a), Some(d)) if a != d => {
                runs.push((start..i, a));
                start = i;
                cur = Some(d);
            }
            _ => {}
        }
    }
    runs.push((start..text.len(), cur.unwrap_or(false)));
    runs
}

/// Glyph id of the first of `chars` the face has (0 = .notdef).
pub fn first_glyph(face: &FontFace, chars: &[char]) -> u32 {
    chars.iter().map(|c| face.glyph_for(*c)).find(|g| *g != 0).unwrap_or(0)
}

#[cfg(test)]
mod tests;
