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
pub fn required(text: &str) -> bool {
    text.chars().any(|c| {
        is_myanmar(c)
            || matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
    })
}

/// Myanmar and its Extended-A and Extended-B blocks. Its medial ra (U+103C) and vowel sign e
/// (U+1031) are typed after the consonant they belong to but drawn before it, and a virama
/// (U+1039) stacks the next consonant under the previous one: only the font's GSUB rules put
/// them there, so one glyph per character draws them in the wrong place (#5114).
pub fn is_myanmar(c: char) -> bool {
    matches!(u32::from(c), 0x1000..=0x109f | 0xa9e0..=0xa9ff | 0xaa60..=0xaa7f)
}

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text.chars().any(|c| is_myanmar(c) || matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0fff | 0x1780..=0x17ff))
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
    use super::{required, shape};

    const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

    /// `bytes` without its `tag` table: the table directory is rebuilt without that record and
    /// the remaining tables are copied after it. Checksums are not kept (nothing here reads them).
    fn without_table(bytes: &[u8], tag: &[u8; 4]) -> Vec<u8> {
        let count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        let records: Vec<&[u8]> = (0..count).map(|i| &bytes[12 + 16 * i..28 + 16 * i]).filter(|r| &r[..4] != tag).collect();
        let mut out = bytes[..12].to_vec();
        out[4..6].copy_from_slice(&(records.len() as u16).to_be_bytes());
        let mut data = Vec::new();
        let base = 12 + 16 * records.len();
        for record in &records {
            let offset = u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as usize;
            let length = u32::from_be_bytes([record[12], record[13], record[14], record[15]]) as usize;
            out.extend_from_slice(&record[..8]);
            out.extend_from_slice(&((base + data.len()) as u32).to_be_bytes());
            out.extend_from_slice(&record[12..]);
            data.extend_from_slice(&bytes[offset..offset + length]);
            data.resize(data.len().next_multiple_of(4), 0);
        }
        out.extend_from_slice(&data);
        out
    }

    /// #5114: Myanmar, in its main block and both extensions, is shaped before it is written, and
    /// its neighbours (Georgian follows it) and Latin text are not made to need it.
    #[test]
    fn myanmar_text_is_shaped() {
        for text in ["မြန်မာ", "ကျောင်း", "ဗုဒ္ဓ", "\u{aa60}", "\u{a9e0}", "ABC ေ"] {
            assert!(required(text), "{text}");
        }
        for text in ["Stable text", "\u{10a0}", "\u{0fff}x"] {
            assert_eq!(required(text), text.contains('\u{0fff}'), "{text}");
        }
    }

    /// #5114: a program without GSUB can't put a Myanmar medial ra or vowel sign e before its
    /// consonant, so it is refused (and the full face asked for) rather than drawn in typing order.
    #[test]
    fn myanmar_needs_the_programs_substitution_tables() {
        let stripped = without_table(INTER, b"GSUB");
        assert!(shape(&stripped, "Stable text").is_ok(), "the stripped program is still a usable font");
        let refused = shape(&stripped, "မြန်မာ").unwrap_err();
        assert!(refused.contains("no contextual substitution tables"), "{refused}");
    }
}
