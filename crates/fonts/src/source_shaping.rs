//! Shaping an explicitly selected source/document face, with OpenType features and UAX #9.
use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::raw::TableProvider;
use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    instance::{LocationRef, Size},
};
use unicode_bidi::{Level, ParagraphBidiInfo};

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    pub text: String,
    /// Original input UTF-8 byte range of this glyph's shaping cluster, including
    /// auxiliary marks whose `text` is empty. The range is logical, not visual order.
    pub logical_range: std::ops::Range<usize>,
}
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub advance: f64,
}

impl ShapedRun {
    /// PDF Unicode ownership is separate from shaping/spacing ownership. A zero-advance
    /// dot may precede its letter body in a cluster. Put ToUnicode on the first advancing
    /// body, retaining all-zero-advance clusters and every original glyph position.
    pub fn pdf_mappings(&self, bytes: &[u8]) -> Result<Vec<(String, GlyphId)>, String> {
        if self.glyphs.len() > 65_534 || self.glyphs.iter().fold(0usize, |n, g| n.saturating_add(g.text.len())) > 262_144 {
            return Err("shaped font mapping exceeds its budget".into());
        }
        let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
        let metrics = face.glyph_metrics(Size::unscaled(), LocationRef::default());
        let mut mappings: Vec<_> = self.glyphs.iter().map(|g| (g.text.clone(), g.id)).collect();
        let mut owner = None;
        for (i, glyph) in self.glyphs.iter().enumerate() {
            if !glyph.text.is_empty() {
                owner = Some(i);
            }
            if metrics.advance_width(glyph.id).is_some_and(|w| w > 0.0)
                && let Some(start) = owner.take()
                && start != i
            {
                let text = mappings.get_mut(start).map(|m| std::mem::take(&mut m.0)).unwrap_or_default();
                if let Some(mapping) = mappings.get_mut(i) {
                    mapping.0 = text;
                }
            }
        }
        Ok(mappings)
    }
}

/// Complex scripts/marks require shaping instead of independently encoding characters.
pub fn required(text: &str) -> bool {
    text.chars().any(|c| {
        is_arabic(c)
            || unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM
            || matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
    })
}

/// Arabic block routing shared by source shaping and the optional Arabic document-face path.
/// Includes Extended-B/C, presentation forms and mathematical letters; this conservative route is
/// not a glyph-coverage test. Syriac, Thaana, NKo and the common U+FEFF control remain separate.
pub fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefc | 0x10ec0..=0x10eff | 0x1ee00..=0x1eeff)
}

/// Joining Arabic and the existing complex-script paths need contextual substitution tables.
/// Encoded Arabic presentation forms and mathematical letters still need bidi/glyph lookup,
/// but do not gain a joining-table requirement merely by sharing the Arabic font route.
fn needs_contextual_substitution(c: char) -> bool {
    (is_arabic(c) && !matches!(u32::from(c), 0xfb50..=0xfdff | 0xfe70..=0xfefc | 0x1ee00..=0x1eeff))
        || is_syriac(c)
        || matches!(u32::from(c), 0x08a0..=0x0fff | 0x1780..=0x17ff)
}

/// Syriac and its supplement share joining behavior, even when a selected face
/// lacks a supplement glyph (which remains an explicit missing-glyph error).
pub fn is_syriac(c: char) -> bool {
    matches!(u32::from(c), 0x0700..=0x074f | 0x0860..=0x086f)
}

/// Shape a bounded row from explicit font bytes with per-run bidi order and logical text.
/// Refuse missing contextual tables, missing glyphs and excessive output geometry.
pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_with_direction(bytes, text, crate::TextDirection::Auto)
}

/// Shape logical text with an explicit paragraph base, without inserting controls.
pub fn shape_with_direction(bytes: &[u8], text: &str, direction: crate::TextDirection) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text.chars().any(needs_contextual_substitution) && face.gsub().is_err() {
        return Err("the source subset has no contextual substitution tables; select its full font".into());
    }
    let data = ShaperData::new(&face);
    let shaper = data.shaper(&face).build();
    let em = f64::from(shaper.units_per_em().max(1));
    let base = direction.level(text);
    let bidi = ParagraphBidiInfo::new(text, Some(base));
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut glyphs = Vec::new();
    let mut x = 0.0;
    for run in runs {
        let Some(input) = text.get(run.clone()) else { continue };
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(input);
        buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
        buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
        buffer.guess_segment_properties();
        let output = shaper.shape(buffer, ShapeOptions::new());
        let mut starts: Vec<_> = output.glyph_infos().iter().map(|g| g.cluster as usize).collect();
        starts.sort_unstable();
        starts.dedup();
        let mut seen = std::collections::HashSet::new();
        for (g, p) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            if g.glyph_id == 0 {
                return Err("the full font lacks a required contextual glyph; choose the complete source face".into());
            }
            let start = g.cluster as usize;
            let end = starts.get(starts.partition_point(|i| *i <= start)).copied().unwrap_or(input.len());
            let cluster_text =
                input.get(start..end).filter(|_| start < end).ok_or_else(|| "the shaper returned an invalid logical cluster boundary".to_string())?;
            // The buffer receives the original bidi-run substring without removing controls.
            // HarfRust cluster offsets are UTF-8 byte positions in that substring. Translate
            // them directly; repeated cluster text cannot safely recover this provenance.
            let logical_start = run.start.checked_add(start).ok_or_else(|| "logical cluster offset overflow".to_string())?;
            let logical_end = run.start.checked_add(end).ok_or_else(|| "logical cluster offset overflow".to_string())?;
            if logical_end > run.end || text.get(logical_start..logical_end).is_none() {
                return Err("the shaper returned a cluster outside its original text run".into());
            }
            let logical = if seen.insert(start) { cluster_text.to_string() } else { String::new() };
            glyphs.push(ShapedGlyph {
                id: GlyphId::new(g.glyph_id),
                position: [x + f64::from(p.x_offset) / em, f64::from(p.y_offset) / em],
                text: logical,
                logical_range: logical_start..logical_end,
            });
            x += f64::from(p.x_advance) / em;
        }
    }
    if !x.is_finite() || glyphs.len() > 65_534 {
        return Err("shaped row exceeds its geometry budget".into());
    }
    Ok(ShapedRun { glyphs, advance: x })
}

#[cfg(test)]
mod direction_tests {
    use super::*;
    #[test]
    /// Verify a forced RTL base reorders source glyphs without changing total advance or logical input.
    fn forced_paragraph_base_reorders_the_same_source_glyphs() {
        let bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let logical = "123 ABC";
        let auto = shape(bytes, logical).unwrap();
        let rtl = shape_with_direction(bytes, logical, crate::TextDirection::RightToLeft).unwrap();
        let text = |run: &ShapedRun| run.glyphs.iter().map(|glyph| glyph.text.as_str()).collect::<String>();
        assert_eq!(text(&auto), "123 ABC");
        assert_eq!(text(&rtl), "ABC 123");
        assert!((auto.advance - rtl.advance).abs() < 1e-9);
        assert_eq!(auto.glyphs.len(), rtl.glyphs.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extended-B and supplementary joining letters must not bypass the source-face GSUB guard.
    #[test]
    fn arabic_extended_source_rows_need_contextual_tables() {
        let mut bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf").to_vec();
        let count = usize::from(u16::from_be_bytes(bytes.get(4..6).unwrap().try_into().unwrap()));
        let mut removed = false;
        for entry in bytes.get_mut(12..12 + count * 16).unwrap().chunks_exact_mut(16) {
            if entry.get(..4) == Some(b"GSUB") {
                entry.get_mut(..4).unwrap().copy_from_slice(b"TEST");
                removed = true;
            }
        }
        assert!(removed);
        for text in ["\u{0870}", "\u{10ec2}"] {
            let error = shape(&bytes, text).unwrap_err();
            assert!(error.contains("contextual substitution tables"), "{text:?}: {error}");
        }
        for text in ["\u{fe8f}", "\u{1ee00}", "A"] {
            assert!(!needs_contextual_substitution(text.chars().next().unwrap()));
        }
    }

    /// Arabic presentation and supplementary letters use bidi/shaping rather than scalar encoding.
    #[test]
    fn arabic_extended_and_presentation_rows_require_shaping() {
        for text in ["\u{0870}", "\u{10ec2}", "\u{1ee00}", "\u{fe8f}"] {
            assert!(required(text), "{text:?} bypassed Arabic shaping");
        }
        assert!(!required("Latin 123 ()"));
        for c in ['\u{0710}', '\u{0860}', '\u{0780}', '\u{07ca}', '\u{feff}', 'A'] {
            assert!(!is_arabic(c), "{c:?} is not in the Arabic routing policy");
        }
    }

    #[test]
    /// Require each Arabic cluster's logical text to belong to its advancing glyph rather than a zero-advance mark.
    fn arabic_cluster_unicode_is_carried_by_its_advancing_body() {
        let Some(font) = crate::document_arabic_font() else {
            eprintln!("set CRAFT_FONTS_DIR to exercise Arabic cluster Unicode ownership");
            return;
        };
        let face = FontRef::new(font.bytes).unwrap();
        let metrics = face.glyph_metrics(Size::unscaled(), LocationRef::default());
        for text in ["ج", "ف", "ي", "ب", "ة", "ش", "اً"] {
            let shaped = shape(font.bytes, text).unwrap();
            let mappings = shaped.pdf_mappings(font.bytes).unwrap();
            let mapped: Vec<_> = mappings.iter().filter(|(text, _)| !text.is_empty()).collect();
            assert_eq!(mapped.iter().map(|(text, _)| text.as_str()).collect::<String>(), text);
            assert_eq!(shaped.glyphs[0].text, text, "PDF mappings must not move the original shaping/spacing ownership");
            assert!(
                mapped.iter().all(|(_, id)| metrics.advance_width(*id).unwrap_or(0.0) > 0.0),
                "{text:?}: Unicode belongs on the body, not a zero-advance dot or mark"
            );
        }
    }
}
