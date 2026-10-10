//! Arabic and Thai text shaping with the matching craft-fonts face and bounded glyph outlines.

use std::sync::OnceLock;

use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer, script};
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::{GlyphError, GlyphOutline};

/// One cluster of shaped text: the glyphs that show one character (a letter's body and
/// dots, say), or several characters for a ligature. Em units.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedCluster {
    /// Face that owns the glyph ids.
    pub script: ShapedScript,
    /// Glyph ids, each with its offset from the cluster's pen position (marks sit above or below).
    pub glyphs: Vec<(u32, [f64; 2])>,
    pub advance: f64,
    /// The characters it shows, in logical order.
    pub text: String,
}

/// The face owning a shaped glyph id. IDs from different faces must never share a cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapedScript {
    Arabic,
    Thai,
}

impl ShapedScript {
    pub fn font(self) -> Option<&'static crate::CraftFont> {
        match self {
            Self::Arabic => crate::document_arabic_font(),
            Self::Thai => crate::document_thai_font(),
        }
    }
}

fn face(script: ShapedScript) -> Option<&'static (FontRef<'static>, ShaperData)> {
    static FACE: OnceLock<Option<(FontRef<'static>, ShaperData)>> = OnceLock::new();
    static THAI: OnceLock<Option<(FontRef<'static>, ShaperData)>> = OnceLock::new();
    let cache = match script {
        ShapedScript::Arabic => &FACE,
        ShapedScript::Thai => &THAI,
    };
    cache
        .get_or_init(|| {
            let font = FontRef::new(script.font()?.bytes).ok()?;
            let data = ShaperData::new(&font);
            Some((font, data))
        })
        .as_ref()
}

/// Whether the Arabic document face has a glyph for `c` (false without the face).
pub fn arabic_has(c: char) -> bool {
    face(ShapedScript::Arabic).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape one directional run of `text` with the Arabic document face. The clusters come in
/// drawing order, left to right. [`GlyphError::NoFont`] without the face;
/// [`GlyphError::Missing`] when it lacks a character.
pub fn shape_arabic(text: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, rtl, ShapedScript::Arabic)
}

pub fn thai_has(c: char) -> bool {
    face(ShapedScript::Thai).is_some_and(|(font, _)| font.charmap().map(c).is_some())
}

/// Shape Thai with grapheme clusters so Sara Am decomposition and stacked marks keep their
/// original Unicode text exactly once, including when wrapped or copied from the saved PDF.
pub fn shape_thai(text: &str) -> Result<Vec<ShapedCluster>, GlyphError> {
    shape_script(text, false, ShapedScript::Thai)
}

fn shape_script(text: &str, rtl: bool, kind: ShapedScript) -> Result<Vec<ShapedCluster>, GlyphError> {
    let (font, data) = face(kind).ok_or(GlyphError::NoFont)?;
    let shaper = data.shaper(font).build();
    let scale = 1.0 / f64::from(shaper.units_per_em().max(1));
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
    buffer.set_script(match kind {
        ShapedScript::Arabic => script::ARABIC,
        ShapedScript::Thai => script::THAI,
    });
    // Preserve the historical Arabic per-character mapping; Thai marks stay with their base.
    buffer.set_cluster_level(match kind {
        ShapedScript::Arabic => BufferClusterLevel::Characters,
        ShapedScript::Thai => BufferClusterLevel::MonotoneGraphemes,
    });
    let shaped = shaper.shape(buffer, ShapeOptions::new());
    // Clusters are byte offsets into `text`; a cluster shows the characters up to the next one.
    let mut starts: Vec<usize> = shaped.glyph_infos().iter().map(|g| g.cluster as usize).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut out: Vec<(usize, ShapedCluster)> = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        if info.glyph_id == 0 {
            return Err(GlyphError::Missing);
        }
        let start = info.cluster as usize;
        // A cluster's glyphs are adjacent in the shaped run.
        let cluster = match out.last_mut() {
            Some((s, c)) if *s == start => c,
            _ => {
                let end = starts.get(starts.partition_point(|s| *s <= start)).copied().unwrap_or(text.len());
                let text = text.get(start..end).unwrap_or_default().to_string();
                out.push((start, ShapedCluster { script: kind, glyphs: Vec::new(), advance: 0.0, text }));
                // Just pushed.
                let Some((_, c)) = out.last_mut() else { continue };
                c
            }
        };
        let at = [cluster.advance + f64::from(pos.x_offset) * scale, f64::from(pos.y_offset) * scale];
        cluster.glyphs.push((info.glyph_id, at));
        cluster.advance += f64::from(pos.x_advance) * scale;
    }
    Ok(out.into_iter().map(|(_, c)| c).collect())
}

/// Glyph `id` of the Arabic document face, bounded like [`crate::japanese_glyph`]. Glyphs without
/// outlines (spaces) come back empty.
pub fn arabic_glyph(id: u32) -> Result<GlyphOutline, GlyphError> {
    shaped_glyph(ShapedScript::Arabic, id)
}

/// A bounded outline from the same face that produced the shaped glyph id.
pub fn shaped_glyph(script: ShapedScript, id: u32) -> Result<GlyphOutline, GlyphError> {
    let (font, _) = face(script).ok_or(GlyphError::NoFont)?;
    if font.maxp().map_or(true, |m| id >= u32::from(m.num_glyphs())) {
        return Err(GlyphError::Missing);
    }
    let gid = GlyphId::new(id);
    let loc = LocationRef::default();
    let scale = 1.0 / font.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
    let width = font.glyph_metrics(Size::unscaled(), loc).advance_width(gid).unwrap_or(0.0) as f64 * scale;
    // Marks have no advance; the widest ligatures (U+FDFD) span a few em.
    if !width.is_finite() || !(0.0..=8.0).contains(&width) {
        return Err(GlyphError::Missing);
    }
    if font.outline_glyphs().get(gid).is_none() {
        return Ok(GlyphOutline { contours: Vec::new(), width, bbox: [0.0, 0.0, width, 0.0] });
    }
    crate::script::bounded_outline(font, gid, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face_or_skip() -> bool {
        if crate::document_arabic_font().is_none() {
            eprintln!("skipping the Arabic shaping checks: built without a craft-fonts Arab face (set CRAFT_FONTS_DIR)");
            assert_eq!(shape_arabic("ب", true), Err(GlyphError::NoFont));
            assert_eq!(arabic_glyph(1).map(|_| ()), Err(GlyphError::NoFont));
            assert!(!arabic_has('ب'));
            return false;
        }
        true
    }

    #[test]
    fn letters_take_their_joining_forms() {
        if !face_or_skip() {
            return;
        }
        let ids =
            |s: &str| shape_arabic(s, true).unwrap().iter().flat_map(|c| c.glyphs.iter().map(|g| g.0)).collect::<std::collections::BTreeSet<_>>();
        let (isolated, joined) = (ids("ب"), ids("ببب"));
        // Initial, medial and final forms are glyphs the isolated letter doesn't use.
        assert!(joined.difference(&isolated).count() >= 2, "{isolated:?} {joined:?}");
        // One cluster per letter, however many glyphs the face draws it with (Noto Sans Arabic:
        // body and dots).
        let c = shape_arabic("ببب", true).unwrap();
        assert!(c.len() == 3 && c.iter().all(|c| c.text == "ب" && c.advance > 0.0 && !c.glyphs.is_empty()), "{c:?}");
    }

    #[test]
    fn right_to_left_runs_are_drawn_from_their_last_letter() {
        if !face_or_skip() {
            return;
        }
        let g = shape_arabic("سلام", true).unwrap();
        let drawn: String = g.iter().map(|g| g.text.as_str()).collect();
        assert_eq!(drawn.chars().rev().collect::<String>(), "سلام");
        assert!(shape_arabic("سلام", false).unwrap().iter().map(|g| g.text.as_str()).collect::<String>() == "سلام");
    }

    #[test]
    fn marks_keep_their_own_text_and_no_advance() {
        if !face_or_skip() {
            return;
        }
        let c = shape_arabic("بَ", true).unwrap();
        assert_eq!(c.len(), 2, "{c:?}");
        let mark = c.iter().find(|c| c.text == "\u{064E}").unwrap();
        assert_eq!(mark.advance, 0.0);
        let outline = arabic_glyph(mark.glyphs[0].0).unwrap();
        assert!(!outline.contours.is_empty() && outline.bbox[1] > 0.0, "the fatha sits above the baseline");
    }

    #[test]
    fn spaces_and_missing_characters() {
        if !face_or_skip() {
            return;
        }
        let space = shape_arabic(" ", true).unwrap();
        assert!(space[0].advance > 0.0);
        assert!(arabic_glyph(space[0].glyphs[0].0).unwrap().contours.is_empty());
        assert!(arabic_has('ب') && !arabic_has('日'));
        assert_eq!(shape_arabic("日", true), Err(GlyphError::Missing));
        assert_eq!(arabic_glyph(u32::MAX).map(|_| ()), Err(GlyphError::Missing));
        assert!(shape_arabic("", true).unwrap().is_empty());
    }
}

#[cfg(test)]
mod thai_tests {
    use super::*;

    #[test]
    fn thai_clusters_preserve_sara_am_and_stacked_marks() {
        if crate::document_thai_font().is_none() {
            assert_eq!(shape_thai("น้ำ"), Err(GlyphError::NoFont));
            assert!(!thai_has('ก'));
            return;
        }
        for text in ["น้ำ", "กิ่", "ผู้ใช้", "ปู่", "เก้า", "สวัสดีภาษาไทย", "๑๒๓", ""]
        {
            let clusters = shape_thai(text).unwrap();
            assert_eq!(clusters.iter().map(|c| c.text.as_str()).collect::<String>(), text);
            for c in clusters {
                assert_eq!(c.script, ShapedScript::Thai);
                assert!(c.advance.is_finite() && c.advance >= 0.0);
                for (id, pos) in c.glyphs {
                    assert!(pos.iter().all(|v| v.is_finite()));
                    shaped_glyph(c.script, id).unwrap();
                }
            }
        }
        let stacked = shape_thai("กิ่").unwrap();
        assert_eq!(stacked.len(), 1, "base, vowel and tone must wrap together");
        assert!(stacked[0].glyphs.len() >= 3);
        assert!(stacked[0].glyphs.iter().any(|(_, p)| p[0] != 0.0 || p[1] != 0.0));
        assert_eq!(shape_thai("日"), Err(GlyphError::Missing));
        assert_eq!(shaped_glyph(ShapedScript::Thai, u32::MAX).map(|_| ()), Err(GlyphError::Missing));
    }
}
