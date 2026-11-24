//! OpenType shaping with HarfRust.
//!
//! [`shape_range`] shapes one item of a paragraph (one face, one direction, one script) with the
//! rest of the paragraph as pre/post context, so Arabic letters at an item boundary (a colour
//! change inside a word, a bold letter) still take their joining forms. Glyphs come back in
//! visual order, exactly as the shaper produced them: right-to-left items are not reversed.

use std::ops::Range;

use harfrust::{Direction, Feature, Language, Script, ShapeOptions, Tag, UnicodeBuffer};
use skrifa::MetadataProvider;
use skrifa::instance::Size;

use crate::FontFace;
use crate::script::ScriptTag;

/// One shaped glyph, in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub gid: u32,
    /// Byte offset (in the shaped string) of the cluster this glyph belongs to.
    pub cluster: usize,
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

/// How to shape an item.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShapeParams<'a> {
    pub features: &'a [Feature],
    /// Right-to-left (odd bidi level). Mirrored characters (brackets) take their mirrored glyphs.
    pub rtl: bool,
    /// ISO 15924 script (`*b"Arab"`); `None` lets the shaper guess from the text.
    pub script: Option<ScriptTag>,
    /// BCP 47 language (`"ar"`, `"ar-MA"`); `None` = the script's default.
    pub language: Option<&'a str>,
}

/// Most bytes of text given to the shaper at once. The shaper lets one call grow its buffers to
/// 256 glyphs per character (a font's substitutions decide how many), so a long item is shaped in
/// pieces of this size, each with the rest of the paragraph as context.
const MAX_SHAPE_CHUNK: usize = 1024;
/// Most glyphs kept per character of a piece: a piece the font expands further is laid out one
/// glyph per character from the cmap instead.
const MAX_GLYPHS_PER_CHAR: usize = 32;

/// Shape `para[range]` with `face`, the rest of `para` as context. Glyphs are in visual order
/// (left to right) and clusters are byte offsets into `para`. `map` substitutes characters (All
/// Caps) without moving clusters. An empty or out-of-bounds range shapes nothing.
pub fn shape_range(face: &FontFace, para: &str, range: Range<usize>, params: &ShapeParams, map: impl Fn(char) -> char) -> Vec<ShapedGlyph> {
    let (Some(_), Some(text), Some(_)) = (para.get(..range.start), para.get(range.clone()), para.get(range.end..)) else {
        return Vec::new();
    };
    let chunks = chunks(text, MAX_SHAPE_CHUNK);
    let mut pieces: Vec<Vec<ShapedGlyph>> = Vec::with_capacity(chunks.len());
    let shaped = face.hb().map(|hb| {
        let shaper = face.shaper.shaper(&hb).instance(face.instance.as_ref()).build();
        // Pieces of one item share the script the whole item would be guessed to have.
        let item_script = (chunks.len() > 1 && params.script.is_none()).then(|| {
            let mut probe = UnicodeBuffer::new();
            for c in text.chars() {
                probe.add(map(c), 0);
            }
            probe.guess_segment_properties();
            probe.script()
        });
        for c in &chunks {
            let r = (range.start + c.start)..(range.start + c.end);
            let (Some(pre), Some(piece), Some(post)) = (para.get(..r.start), para.get(r.clone()), para.get(r.end..)) else { continue };
            let mut buf = UnicodeBuffer::new();
            buf.set_pre_context(pre);
            buf.set_post_context(post);
            for (i, c) in piece.char_indices() {
                buf.add(map(c), (r.start + i) as u32);
            }
            buf.guess_segment_properties();
            if let Some(s) = params.script.and_then(|t| Script::from_iso15924_tag(Tag::new(&t))) {
                buf.set_script(s);
            } else if let Some(s) = item_script {
                buf.set_script(s);
            }
            if let Some(l) =
                params.language.or_else(|| params.script.and_then(crate::script::default_language)).and_then(|l| l.parse::<Language>().ok())
            {
                buf.set_language(l);
            }
            buf.set_direction(if params.rtl { Direction::RightToLeft } else { Direction::LeftToRight });
            let gb = shaper.shape(buf, ShapeOptions::new().features(params.features));
            let budget = piece.chars().count().saturating_mul(MAX_GLYPHS_PER_CHAR);
            if gb.len() > budget {
                // Font-controlled expansion past anything real text needs: keep it bounded.
                pieces.push(cmap_glyphs(face, para, r, params.rtl, &map));
                continue;
            }
            pieces.push(
                gb.glyph_infos()
                    .iter()
                    .zip(gb.glyph_positions())
                    .map(|(info, pos)| ShapedGlyph {
                        gid: info.glyph_id,
                        cluster: info.cluster as usize,
                        x_advance: pos.x_advance,
                        x_offset: pos.x_offset,
                        y_offset: pos.y_offset,
                    })
                    .collect(),
            );
        }
    });
    if shaped.is_none() {
        // No shaper for this face: one glyph per character from the cmap, in visual order.
        return cmap_glyphs(face, para, range, params.rtl, &map);
    }
    // Each piece is in visual order; right to left, the later pieces come first.
    if params.rtl {
        pieces.reverse();
    }
    let mut out = Vec::with_capacity(pieces.iter().map(Vec::len).sum());
    for p in pieces {
        out.extend(p);
    }
    out
}

/// One glyph per character of `para[range]` from the cmap, in visual order.
fn cmap_glyphs(face: &FontFace, para: &str, range: Range<usize>, rtl: bool, map: &impl Fn(char) -> char) -> Vec<ShapedGlyph> {
    let (Some(text), Some(f)) = (para.get(range.clone()), face.skrifa()) else { return Vec::new() };
    let cmap = f.charmap();
    let gm = f.glyph_metrics(Size::unscaled(), face.location());
    let mut out = Vec::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        let g = cmap.map(map(c)).unwrap_or_default();
        let adv = gm.advance_width(g).unwrap_or(face.upem as f32 * 0.5);
        out.push(ShapedGlyph { gid: g.to_u32(), cluster: range.start + i, x_advance: adv.round() as i32, x_offset: 0, y_offset: 0 });
    }
    if rtl {
        out.reverse();
    }
    out
}

/// `text` cut into byte ranges of at most `max` bytes (or one character, if longer), at the last
/// space of a piece where there is one, else before a character that starts a cluster (not a
/// combining mark or joiner, not after a joiner). Text that fits is one piece.
fn chunks(text: &str, max: usize) -> Vec<Range<usize>> {
    use unicode_bidi::BidiClass::{BN, NSM};
    let mut out = Vec::new();
    let mut start = 0;
    while text.len() - start > max {
        let window = text.get(start..).unwrap_or_default();
        let mut space = None;
        let mut boundary = None;
        let mut any = None;
        let mut prev: Option<char> = None;
        for (i, c) in window.char_indices() {
            if i > max {
                break;
            }
            if i > 0 {
                any = Some(i);
                let joined = prev.is_some_and(|p| matches!(unicode_bidi::bidi_class(p), BN));
                if !joined && !matches!(unicode_bidi::bidi_class(c), NSM | BN) {
                    boundary = Some(i);
                    if prev.is_some_and(char::is_whitespace) {
                        space = Some(i);
                    }
                }
            }
            prev = Some(c);
        }
        // A piece always moves forward by at least one character.
        let first = window.chars().next().map_or(window.len(), char::len_utf8);
        let cut = space.filter(|&i| i >= max / 2).or(boundary).or(any).unwrap_or(first).max(first);
        out.push(start..start + cut);
        start += cut;
    }
    if start < text.len() || out.is_empty() {
        out.push(start..text.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_FAMILY, FontDb};

    #[test]
    fn rtl_items_come_in_visual_order_and_mirror() {
        let face = FontDb::global().face(DEFAULT_FAMILY, "Regular");
        let text = "(ab)";
        let p = ShapeParams { rtl: true, ..Default::default() };
        let g = shape_range(&face, text, 0..text.len(), &p, |c| c);
        // Visual order: last cluster first.
        assert_eq!(g.iter().map(|g| g.cluster).collect::<Vec<_>>(), vec![3, 2, 1, 0]);
        // L4: in a right-to-left run "(" is drawn with the ")" glyph and vice versa.
        if face.covers('(') && face.covers(')') {
            assert_eq!(g[3].gid, face.glyph_for(')'));
            assert_eq!(g[0].gid, face.glyph_for('('));
        }
    }

    #[test]
    fn clusters_point_into_the_whole_paragraph() {
        let face = FontDb::global().face(DEFAULT_FAMILY, "Regular");
        let para = "Hello world";
        let g = shape_range(&face, para, 6..11, &ShapeParams::default(), |c| c);
        assert_eq!(g.first().map(|g| g.cluster), Some(6));
        assert!(shape_range(&face, para, 6..99, &ShapeParams::default(), |c| c).is_empty());
        // Not a char boundary.
        assert!(shape_range(&face, "سنة", 1..2, &ShapeParams::default(), |c| c).is_empty());
    }

    #[test]
    fn long_items_are_shaped_in_bounded_pieces() {
        for text in ["word ".repeat(2_000), "e\u{301}".repeat(2_000), "x".repeat(5_000), "\u{1f469}\u{200d}\u{1f4bb} ".repeat(500)] {
            let pieces = chunks(&text, MAX_SHAPE_CHUNK);
            assert_eq!(pieces.first().map(|r| r.start), Some(0));
            assert_eq!(pieces.last().map(|r| r.end), Some(text.len()));
            for w in pieces.windows(2) {
                assert_eq!(w[0].end, w[1].start);
            }
            for r in &pieces {
                assert!(!r.is_empty() && r.len() <= MAX_SHAPE_CHUNK);
                // Never before a combining mark, never after a joiner.
                let rest = &text[r.end..];
                assert!(!rest.starts_with('\u{301}'));
                assert!(!text[..r.end].ends_with('\u{200d}') || rest.is_empty());
            }
        }
        assert_eq!(chunks("short", MAX_SHAPE_CHUNK), vec![0..5]);
        assert_eq!(chunks("", MAX_SHAPE_CHUNK), vec![0..0]);
    }

    #[test]
    fn long_items_keep_every_cluster_in_visual_order() {
        let face = FontDb::global().face(DEFAULT_FAMILY, "Regular");
        let text = "ab cd ".repeat(1_000);
        let g = shape_range(&face, &text, 0..text.len(), &ShapeParams::default(), |c| c);
        assert_eq!(g.len(), text.len());
        assert!(g.windows(2).all(|w| w[0].cluster < w[1].cluster));
        let rtl = shape_range(&face, &text, 0..text.len(), &ShapeParams { rtl: true, ..Default::default() }, |c| c);
        assert_eq!(rtl.len(), text.len());
        assert!(rtl.windows(2).all(|w| w[0].cluster > w[1].cluster));
    }
}
