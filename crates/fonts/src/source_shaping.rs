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
    text.chars()
        .any(|c| matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069))
}

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0fff | 0x1780..=0x17ff)) && face.gsub().is_err() {
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
