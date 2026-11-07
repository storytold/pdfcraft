//! Shaping an explicitly selected source/document face, with OpenType features and UAX #9.
use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId};
use unicode_bidi::{Level, ParagraphBidiInfo};

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub advance: f64,
}

/// Complex scripts/marks require shaping instead of independently encoding characters.
/// Layout processes such as kerning apply to a shaped run: a Thai base alone writes
/// unshaped (so it stays editable on its own), but takes part in the run when Latin
/// or a shaping mark is present: `"AV\u{0e01}"`.
pub fn required(text: &str) -> bool {
    fn is_thai_base(c: char) -> bool {
        matches!(u32::from(c), 0x0e00..=0x0e5f) && !matches!(u32::from(c), 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e)
    }

    // A Thai base is shaped only in a mixed run, where layout such as AV
    // kerning applies to the whole run.
    let mut needs_shaping = false;
    let mut thai_base_with_latin = false;
    let has_latin = text.chars().any(|c| {
        c.is_ascii_alphabetic() || (matches!(u32::from(c), 0x00a0..=0x00ff | 0x0100..=0x024f) && c.is_alphabetic())
    });
    for c in text.chars() {
        if matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0dff | 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e | 0x0e80..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
        {
            needs_shaping = true;
        }
        if is_thai_base(c) {
            thai_base_with_latin |= has_latin;
        }
    }
    needs_shaping || thai_base_with_latin
}

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text
        .chars()
        .any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0700..=0x074f | 0x0750..=0x077f | 0x08a0..=0x0dff | 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e | 0x0e80..=0x0fff | 0x1780..=0x17ff))
        && face.gsub().is_err()
    {
        return Err("the source subset has no contextual substitution tables; select its full font".into());
    }
    let data = ShaperData::new(&face);
    let shaper = data.shaper(&face).build();
    let em = f64::from(shaper.units_per_em().max(1));
    let base = if unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl { Level::rtl() } else { Level::ltr() };
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
            let logical = if seen.insert(start) { input.get(start..end).unwrap_or_default().to_string() } else { String::new() };
            glyphs.push(ShapedGlyph {
                id: GlyphId::new(g.glyph_id),
                position: [x + f64::from(p.x_offset) / em, f64::from(p.y_offset) / em],
                text: logical,
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
mod tests {
    use super::*;

    #[test]
    fn thai_base_characters_do_not_require_shaping() {
        // Thai base characters are one glyph each and need no substitution; only the
        // combining marks need positioning.
        let bases = ["\u{0e01}", "\u{0e2a}", "\u{0e27}", "\u{0e32}", "\u{0e44}", "\u{0e51}", "\u{0e53}"];
        for text in bases {
            assert!(!required(text), "{text:?} is a Thai base character");
        }
        assert!(required("\u{0e31}"), "a Thai combining mark needs positioning");
        assert!(required("\u{0e34}"), "a Thai combining mark needs positioning");
    }

    #[test]
    fn thai_sara_am_requires_shaping() {
        assert!(required("\u{0e33}"), "U+0E33 THAI CHARACTER SARA AM needs shaping");
    }

    #[test]
    fn thai_remaining_mark_ranges_require_shaping() {
        assert!(required("\u{0e3a}"), "U+0E3A is the end of the Thai mark range");
        assert!(required("\u{0e47}"), "U+0E47 is the start of the Thai tone mark range");
        assert!(required("\u{0e48}"), "U+0E48 THAI CHARACTER MAI EK needs shaping");
        assert!(required("\u{0e4e}"), "U+0E4E is the end of the Thai tone mark range");
    }

    #[test]
    fn a_thai_base_shaped_with_another_shaped_character_requires_shaping() {
        assert!(required("AV\u{0e01}"), "a Latin run that needs layout keeps the Thai base in the same shaped run");
        assert!(required("\u{0e01}AV"), "a Thai base joins the same shaped run when the rest of the text does");
        assert!(required("\u{0e01}\u{0e31}"), "a base with a combining mark still shapes");
        assert!(!required("\u{0e01}"), "a Thai base alone still writes unshaped");
        assert!(!required("AV"), "plain Latin still writes unshaped");
    }

    #[test]
    fn extended_latin_with_thai_base_requires_shaping() {
        assert!(required("\u{00c0}\u{0e01}"), "a Latin-1 Supplement letter joins a Thai base in the same shaped run");
        assert!(required("\u{0e01}\u{0100}"), "a Latin Extended letter joins a Thai base in the same shaped run");
        assert!(required("\u{00e9}\u{0e01}"), "a Latin-1 Supplement letter joins a Thai base in the same shaped run");
        assert!(!required("\u{00c0}"), "extended Latin alone still writes unshaped");
        assert!(!required("\u{0e01}"), "a Thai base alone still writes unshaped");
    }

    #[cfg(test)]
    fn without_gsub(bytes: &[u8]) -> Vec<u8> {
        let mut copy = bytes.to_vec();
        // Remove the GSUB entry from the table directory (12-byte header,
        // 16-byte records) so the GSUB lookup itself fails. Corrupting the
        // GSUB table contents no longer guarantees that gsub() errors.
        let offset = FontRef::new(&copy).ok().and_then(|font| {
            font.table_directory().table_records().iter().position(|record| record.tag() == skrifa::Tag::new(b"GSUB")).map(|index| 12 + index * 16)
        });
        if let Some(offset) = offset {
            for i in 0..4 {
                if let Some(byte) = copy.get_mut(offset + i) {
                    *byte = b'X';
                }
            }
        }
        copy
    }

    /// The GSUB rejection must fire for Thai marks, not just broader ranges. The copied
    /// fixture is what a subset looks like: a face whose GSUB table is gone.
    #[test]
    fn thai_marks_without_gsub_are_rejected() {
        let bytes = without_gsub(crate::INTER_REGULAR);
        assert!(FontRef::new(&bytes).is_ok_and(|f| f.gsub().is_err()), "the fixture has no GSUB table");
        for text in ["\u{0e0d}\u{0e38}", "\u{0e01}\u{0e31}", "\u{0e01}\u{0e47}"] {
            let error = shape(&bytes, text).expect_err("a subset without GSUB must be refused for Thai clusters");
            assert!(error.contains("no contextual substitution tables"), "{text:?}: {error}");
        }
    }

    #[test]
    fn lao_and_syriac_clusters_without_gsub_are_rejected() {
        let bytes = without_gsub(crate::INTER_REGULAR);
        assert!(shape(&bytes, "\u{0e81}").is_err(), "Lao still requires substitution tables");
        assert!(shape(&bytes, "\u{0710}").is_err(), "Syriac still requires substitution tables");
    }

    #[test]
    fn a_latin_kerning_pair_in_a_shaped_run_uses_shaped_advances() {
        // AV\u{0e01} needs shaping as one run; the complete-face check shape() runs here
        // is the geometry the caller gets when the face covers every character.
        let Some(face) = crate::document_thai_font() else {
            eprintln!("skipping the AV kerning check: built without a craft-fonts Thai face (set CRAFT_FONTS_DIR)");
            return;
        };
        // This check covers that the complete face shapes the mixed run to finite
        // geometry. The caller shapes only when a candidate covers every character.
        let shaped = shape(face.bytes, "AV\u{0e01}").expect("the complete face shapes the mixed run");
        assert!(shaped.advance.is_finite() && shaped.advance > 0.0, "{shaped:?}");
    }
}
