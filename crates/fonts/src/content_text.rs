//! Shared Syriac PDF text for page marks, annotations, forms and typed signatures.
//! Layout is UAX #9 plus the face's OpenType rules, never a presentation-form table.

use crate::{
    GlyphError, GlyphOutline, ScriptOutline, ShapedCluster, ShapedScript, SyriacFamily, embed_shaped_clusters, shape_syriac_for_family, shaped_glyph,
};
use pdfcraft_cos::{Dict, Document, Object};
use std::collections::HashMap;
use unicode_bidi::{Level, ParagraphBidiInfo};

pub const MAX_TEXT_BYTES: usize = 262_144;
const MAX_LINES: usize = 1024;
const MAX_POINTS: usize = 1_000_000;

/// Whether a content string needs the Syriac document face.
pub fn contains_syriac(text: &str) -> bool {
    text.chars().any(crate::source_shaping::is_syriac)
}

/// Translate shaping failures into actionable missing-font, missing-glyph or budget errors.
fn error(error: GlyphError) -> String {
    match error {
        GlyphError::NoFont => "Syriac text needs a Syrc font in the optional craft-fonts build input".into(),
        GlyphError::Missing => "the Syriac font lacks a glyph needed by this text".into(),
        GlyphError::TooComplex => "Syriac text exceeds its text or outline budget".into(),
    }
}

/// A visual line in em units, retaining its paragraph direction and logical text.
#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    pub rtl: bool,
    pub width: f64,
    clusters: Vec<ShapedCluster>,
}

/// Identify directional controls used for ordering but omitted from painted glyph input.
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Shape a bounded line with a base direction inherited from its paragraph.
pub fn line(text: &str, rtl: bool) -> Result<Line, String> {
    line_for_family(text, rtl, SyriacFamily::default())
}

/// Shape a line using exactly the selected document family.
pub fn line_for_family(text: &str, rtl: bool, family: SyriacFamily) -> Result<Line, String> {
    if text.len() > MAX_TEXT_BYTES || text.contains(['\n', '\r']) {
        return Err("shape one bounded Syriac line at a time".into());
    }
    let mut clusters = Vec::new();
    if !text.is_empty() {
        let bidi = ParagraphBidiInfo::new(text, Some(if rtl { Level::rtl() } else { Level::ltr() }));
        let (levels, runs) = bidi.visual_runs(0..text.len());
        for run in runs {
            let input: String = text.get(run.clone()).unwrap_or_default().chars().filter(|c| !bidi_control(*c)).collect();
            let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
            clusters.extend(shape_syriac_for_family(&input, run_rtl, family).map_err(error)?);
            if clusters.len() > 65_534 {
                return Err("the Syriac line has too many clusters".into());
            }
        }
    }
    let width: f64 = clusters.iter().map(|cluster| cluster.advance).sum();
    if !width.is_finite() || width < 0.0 {
        return Err("the Syriac line has invalid geometry".into());
    }
    Ok(Line { text: text.into(), rtl, width, clusters })
}

/// Shaped width at a point size. Failure remains explicit rather than measuring '?' substitutes.
pub fn width(text: &str, size: f64) -> Result<f64, String> {
    width_for_family(text, size, SyriacFamily::default())
}

/// Return the selected face's em advance multiplied by the caller-supplied scale.
/// Propagate shaping failures; this helper does not validate the scale itself.
pub fn width_for_family(text: &str, size: f64, family: SyriacFamily) -> Result<f64, String> {
    Ok(line_for_family(text, unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl, family)?.width * size)
}

/// Comb cells follow bidi visual order, with Syriac vowel/dot marks kept with their
/// base. Each cell is shaped independently, so letters do not join across cell gaps.
pub fn comb_cells(text: &str) -> Result<Vec<Line>, String> {
    comb_cells_for_family(text, SyriacFamily::default())
}

/// Lay out at most 4096 independent cells in visual order using the selected family.
/// Keep nonspacing marks with their base and refuse multiline or oversized input.
pub fn comb_cells_for_family(text: &str, family: SyriacFamily) -> Result<Vec<Line>, String> {
    if text.len() > MAX_TEXT_BYTES || text.contains(['\n', '\r']) {
        return Err("a comb value must be one bounded line".into());
    }
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let bidi = ParagraphBidiInfo::new(text, None);
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut cells = Vec::new();
    for run in runs {
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        let mut groups: Vec<String> = Vec::new();
        for ch in text.get(run).unwrap_or_default().chars().filter(|ch| !bidi_control(*ch)) {
            if unicode_bidi::bidi_class(ch) == unicode_bidi::BidiClass::NSM
                && let Some(group) = groups.last_mut()
            {
                group.push(ch);
            } else {
                if groups.len().saturating_add(cells.len()) >= 4096 {
                    return Err("the comb value has too many cells".into());
                }
                groups.push(ch.to_string());
            }
        }
        if rtl {
            groups.reverse();
        }
        for group in groups {
            cells.push(line_for_family(&group, rtl, family)?);
        }
        if cells.len() > 4096 {
            return Err("the comb value has too many cells".into());
        }
    }
    Ok(cells)
}

/// Wrap at spaces without breaking a joining word or detaching a vowel mark. Explicit
/// CR/LF/CRLF paragraphs retain their own bidi base direction on every wrapped row.
pub fn wrap(text: &str, size: f64, width: f64) -> Result<Vec<Line>, String> {
    wrap_for_family(text, size, width, SyriacFamily::default())
}

/// Wrap at spaces with the selected face and an independent bidi base per paragraph.
/// `size` and `width` must be positive finite values in points; input is bounded by MAX_TEXT_BYTES
/// and output by 1024 rows. Joining words and attached marks stay intact, so an overlong word
/// can exceed the requested width. Refuse invalid geometry, unavailable glyphs or exceeded budgets.
pub fn wrap_for_family(text: &str, size: f64, width: f64, family: SyriacFamily) -> Result<Vec<Line>, String> {
    if text.len() > MAX_TEXT_BYTES || !size.is_finite() || size <= 0.0 || !width.is_finite() || width <= 0.0 {
        return Err("Syriac layout needs bounded text and positive finite size/width".into());
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = Vec::new();
    for paragraph in normalized.split('\n') {
        let rtl = unicode_bidi::get_base_direction(paragraph) == unicode_bidi::Direction::Rtl;
        let space = line_for_family(" ", rtl, family)?.width * size;
        let (mut current, mut used) = (String::new(), 0.0);
        for (index, word) in paragraph.split(' ').enumerate() {
            let advance = line_for_family(word, rtl, family)?.width * size;
            let gap = if index == 0 { 0.0 } else { space };
            if !current.is_empty() && used + gap + advance > width {
                if output.len() >= MAX_LINES {
                    return Err("the Syriac text has too many wrapped rows".into());
                }
                output.push(line_for_family(&current, rtl, family)?);
                current.clear();
                used = 0.0;
            } else if index != 0 {
                current.push(' ');
                used += space;
            }
            current.push_str(word);
            used += advance;
        }
        output.push(line_for_family(&current, rtl, family)?);
        if output.len() > MAX_LINES {
            return Err("the Syriac text has too many wrapped rows".into());
        }
    }
    Ok(output)
}

type Key = (String, Vec<(u32, u64, u64)>, u64);
/// Identify equivalent clusters by logical text, positioned glyph ids and exact advance bits.
fn key(cluster: &ShapedCluster) -> Key {
    (cluster.text.clone(), cluster.glyphs.iter().map(|(id, xy)| (*id, xy[0].to_bits(), xy[1].to_bits())).collect(), cluster.advance.to_bits())
}

impl Line {
    /// Ink above and below the baseline, including positioned vowel and dot marks.
    pub fn vertical_extents(&self) -> Result<(f64, f64), String> {
        let (mut top, mut bottom) = (0.718f64, -0.207f64);
        let mut boxes = HashMap::new();
        for cluster in &self.clusters {
            for (id, offset) in &cluster.glyphs {
                let bbox = match boxes.get(id) {
                    Some(bbox) => *bbox,
                    None => {
                        let bbox = shaped_glyph(cluster.script, *id).map_err(error)?.bbox;
                        boxes.insert(*id, bbox);
                        bbox
                    }
                };
                top = top.max(bbox[3] + offset[1]);
                bottom = bottom.min(bbox[1] + offset[1]);
            }
        }
        Ok((top, bottom))
    }

    /// Embed bounded Type 3 cluster fonts and emit text with both ToUnicode and
    /// per-line ActualText. `at` is a finite baseline position in PDF points; `size` is in (0, 500].
    /// This mutates `doc` and `fonts`: a later cluster-font chunk can fail after earlier resources
    /// were allocated. The caller must stage those objects when atomic multi-line updates are required.
    pub fn write(&self, doc: &mut Document, fonts: &mut Dict, at: [f64; 2], size: f64) -> Result<Vec<u8>, String> {
        if fonts.len() > 65_534 {
            return Err("the text surface has too many font resources".into());
        }
        if !size.is_finite() || !(0.0..=500.0).contains(&size) || size == 0.0 || at.iter().any(|v| !v.is_finite()) {
            return Err("Syriac text placement needs finite coordinates and a size in (0, 500]".into());
        }
        let mut outlines: HashMap<(ShapedScript, u32), GlyphOutline> = HashMap::new();
        let mut points = 0usize;
        for cluster in &self.clusters {
            for (id, _) in &cluster.glyphs {
                if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((cluster.script, *id)) {
                    let outline = shaped_glyph(cluster.script, *id).map_err(error)?;
                    points = points.saturating_add(outline.contours.iter().map(Vec::len).sum::<usize>());
                    if points > MAX_POINTS {
                        return Err("Syriac glyph outlines exceed their point budget".into());
                    }
                    entry.insert(outline);
                }
            }
        }
        let mut unique = Vec::new();
        let mut indices = HashMap::new();
        for cluster in &self.clusters {
            let next = unique.len();
            if let std::collections::hash_map::Entry::Vacant(entry) = indices.entry(key(cluster)) {
                unique.push(cluster);
                entry.insert(next);
            }
        }
        let mut names = Vec::new();
        for chunk in unique.chunks(240) {
            let font = embed_shaped_clusters(doc, chunk, &outlines)?;
            let name = (0..=fonts.len())
                .map(|i| format!("PCSyrc{}_{}", font.num, i))
                .find(|name| !fonts.contains(name.as_bytes()))
                .ok_or("the Syriac font resource name budget was exhausted")?;
            fonts.set(name.clone().into_bytes(), Object::Ref(font));
            names.push(name);
        }
        let logical: String = self.text.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
        let mut output = format!("/Span <</ActualText <FEFF{logical}>>> BDC\nq\nBT 0 Tc 0 Tw 100 Tz 0 Ts 0 Tr\n").into_bytes();
        let mut pen = at[0];
        for cluster in &self.clusters {
            let index = *indices.get(&key(cluster)).ok_or("the Syriac cluster mapping is missing")?;
            let name = names.get(index / 240).ok_or("the Syriac font resource is missing")?;
            let code = u8::try_from(index % 240 + 1).map_err(|_| "the Syriac character code is out of range")?;
            output.extend(format!("/{name} {size:.6} Tf 1 0 0 1 {pen:.6} {:.6} Tm <{code:02X}> Tj\n", at[1]).bytes());
            pen += cluster.advance * size;
        }
        output.extend_from_slice(b"ET\nQ\nEMC\n");
        Ok(output)
    }

    /// A bounded outline for a typed signature, retaining the same shaping and bidi order.
    pub fn outline(&self) -> Result<ScriptOutline, String> {
        let (mut contours, mut pen, mut points) = (Vec::new(), 0.0, 0usize);
        let (mut ascent, mut descent) = (0.0f64, 0.0f64);
        for cluster in &self.clusters {
            for (id, offset) in &cluster.glyphs {
                let glyph = shaped_glyph(cluster.script, *id).map_err(error)?;
                for contour in glyph.contours {
                    points = points.saturating_add(contour.len());
                    if points > 65_536 {
                        return Err("the Syriac signature outline is too complex".into());
                    }
                    let moved: Vec<_> = contour.into_iter().map(|point| [point[0] + pen + offset[0], point[1] + offset[1]]).collect();
                    for point in &moved {
                        ascent = ascent.max(point[1]);
                        descent = descent.min(point[1]);
                    }
                    contours.push(moved);
                }
            }
            pen += cluster.advance;
        }
        Ok(ScriptOutline { contours, width: self.width, ascent, descent })
    }
}
