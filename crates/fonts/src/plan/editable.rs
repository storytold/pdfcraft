//! Cluster programs for editable OCR, kept separate from the legacy scalar planner.

use skrifa::FontRef;

use super::{Coverage, MAX_PLAN_GLYPHS, TextPlan, document_faces};
use crate::{GlyphOutline, document_latin_fonts, latin_text};

const MAX_OUTLINE_POINTS: usize = 4096;
const MAX_PLAN_POINTS: usize = 262_144;
const MAX_WORD_GLYPHS: usize = 65_534;

impl TextPlan {
    /// Preflight editable output without substitutions. ASCII and non-Latin scalar words
    /// retain their legacy preference; non-ASCII Latin words (including WinAnsi letters)
    /// and marked Latin clusters use Inter together, keeping canonical NFC/NFD identical.
    /// A failed plan is discarded before any document or font resources are created.
    pub fn for_editable_words<'a>(words: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let faces = document_faces();
        Self::editable_with_faces(words, &faces)
    }

    pub(super) fn editable_with_faces<'a>(words: impl IntoIterator<Item = &'a str>, faces: &[&crate::CraftFont]) -> Result<Self, String> {
        let mut coverage = Coverage::default();
        let mut plan = Self::default();
        let mut bytes = 0usize;
        for (i, word) in words.into_iter().enumerate() {
            bytes = bytes.checked_add(word.len()).ok_or("editable OCR exceeds its text budget")?;
            if i >= 65_536 || bytes > 4 * 1024 * 1024 {
                return Err("editable OCR exceeds its text budget".into());
            }
            if latin_text::required(word) || checked_latin_word(word) {
                plan.push_shaped_word(word)?;
            } else {
                plan.push_scalar_word(word, faces, &mut coverage);
            }
            if plan.missing != 0 {
                return Err("editable OCR cannot draw every character within its font budget; use searchable output".into());
            }
        }
        Ok(plan)
    }

    /// Original Unicode for a cluster code. Scalar codes retain their public character label.
    /// Registrars must use this mapping when present instead of the scalar label.
    pub fn cluster_text(&self, font: usize, code: u8) -> Option<&str> {
        self.cluster_sources.get(&(font, code)).map(String::as_str)
    }

    /// Bounds of a cluster-planned word, including ink overhang and horizontal advance.
    /// `None` selects the unchanged legacy scalar geometry.
    pub fn shaped_bounds(&self, word: usize) -> Option<[f64; 4]> {
        self.shaped_words.get(&word).copied()
    }

    fn push_shaped_word(&mut self, word: &str) -> Result<(), String> {
        if word.len() > latin_text::MAX_ROW_BYTES || word.contains(['\r', '\n']) {
            return Err("editable OCR must shape one bounded text row at a time".into());
        }
        let faces = document_latin_fonts(false);
        let face = faces.first().ok_or("the bundled Latin document font is unavailable")?;
        let font = FontRef::new(face.bytes).map_err(|e| e.to_string())?;
        let mut encoding = Vec::new();
        let mut advance = 0.0;
        let mut bounds = [0.0f64; 4];
        let mut glyph_count = 0usize;
        for cluster in latin_text::clusters(word) {
            let (fi, code, count) = match self.cluster_index.get(cluster).copied() {
                Some(code) => code,
                None => {
                    let row = latin_text::plan(cluster, false)?;
                    if self.planned_glyphs() >= MAX_PLAN_GLYPHS {
                        return Err("editable OCR exceeds its Type3 glyph budget".into());
                    }
                    let mut glyph = GlyphOutline { contours: Vec::new(), width: row.advance, bbox: [0.0; 4] };
                    let mut points = 0usize;
                    // A space has advance but no outline. All other glyphs, including
                    // zero-advance marks, must supply their complete bounded outline.
                    if !matches!(cluster, " " | "\t") {
                        for positioned in &row.glyphs {
                            let outline = crate::script::bounded_outline(&font, positioned.id, 0.0)
                                .map_err(|_| "editable OCR cluster outline is missing or too complex")?;
                            for contour in outline.contours {
                                points = points.saturating_add(contour.len());
                                if points > MAX_OUTLINE_POINTS || self.shaped_points.saturating_add(points) > MAX_PLAN_POINTS {
                                    return Err("editable OCR exceeds its cluster outline budget".into());
                                }
                                glyph
                                    .contours
                                    .push(contour.into_iter().map(|p| [p[0] + positioned.position[0], p[1] + positioned.position[1]]).collect());
                            }
                        }
                    }
                    let mut ink = glyph.contours.iter().flatten();
                    if let Some(first) = ink.next() {
                        glyph.bbox = [first[0], first[1], first[0], first[1]];
                        for point in ink {
                            glyph.bbox[0] = glyph.bbox[0].min(point[0]);
                            glyph.bbox[1] = glyph.bbox[1].min(point[1]);
                            glyph.bbox[2] = glyph.bbox[2].max(point[0]);
                            glyph.bbox[3] = glyph.bbox[3].max(point[1]);
                        }
                    }
                    if !glyph.width.is_finite() || glyph.width <= 0.0 || glyph.width > 2.0 || glyph.bbox.iter().any(|v| !v.is_finite()) {
                        return Err("editable OCR cluster has invalid geometry".into());
                    }
                    let label = cluster.chars().next().ok_or("empty OCR cluster")?;
                    let (fi, code) = self.add_glyph(face, label, glyph).ok_or("editable OCR exceeds its font budget")?;
                    self.shaped_points += points;
                    self.cluster_sources.insert((fi, code), cluster.into());
                    let planned = (fi, code, row.glyphs.len());
                    self.cluster_index.insert(cluster.into(), planned);
                    planned
                }
            };
            glyph_count = glyph_count.saturating_add(count);
            if glyph_count > MAX_WORD_GLYPHS {
                return Err("editable OCR word exceeds its shaped glyph budget".into());
            }
            let glyph = self.fonts.get(fi).and_then(|f| f.glyphs.get(usize::from(code).checked_sub(1)?)).ok_or("missing OCR cluster code")?;
            bounds[0] = bounds[0].min(advance + glyph.1.bbox[0]);
            bounds[1] = bounds[1].min(glyph.1.bbox[1]);
            bounds[2] = bounds[2].max(advance + glyph.1.bbox[2]);
            bounds[3] = bounds[3].max(glyph.1.bbox[3]);
            advance += glyph.1.width;
            encoding.push((fi, code));
        }
        bounds[2] = bounds[2].max(advance);
        if !advance.is_finite() || advance <= 0.0 || bounds[3] <= bounds[1] {
            return Err("editable OCR word has no measurable ink".into());
        }
        self.shaped_words.insert(self.words.len(), bounds);
        self.words.push(encoding);
        Ok(())
    }
}

/// The checked editable API makes a deliberate non-ASCII-Latin face choice, including
/// WinAnsi letters. ASCII and non-Latin scripts retain their scalar font preference.
fn checked_latin_word(word: &str) -> bool {
    word.chars().any(|c| !c.is_ascii() && c.is_alphabetic())
        && word.chars().all(|c| !c.is_alphabetic() || matches!(u32::from(c), 0x0041..=0x007a | 0x00c0..=0x024f | 0x1e00..=0x1eff))
        && latin_text::first_unsupported(word, false).is_none()
}
