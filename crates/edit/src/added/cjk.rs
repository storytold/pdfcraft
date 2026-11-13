use std::collections::{HashMap, HashSet};

use pdfcraft_fonts::CraftFont;

use super::layout::latin_width;
use super::{AddedText, Dict, Document, EditError};
use crate::text::{Type3Fallback, Type3Glyph, fallback_face, type3_faces, type3_glyph, unique_font_name, write_type3_glyphs};

/// Han, kana and their full-width punctuation; other scripts keep their shaping paths.
pub(super) fn is_cjk(ch: char) -> bool {
    matches!(u32::from(ch), 0x3000..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

fn missing(ch: char) -> EditError {
    EditError::Invalid(format!(
        "the bundled CJK fonts can't draw \"{ch}\" (U+{:04X}); use a build with a covering craft-fonts face \
         (Simplified Chinese needs a Hans face in CRAFT_FONTS_DIR)",
        u32::from(ch)
    ))
}

/// One bounded set of outlines supplies validation, measurement and writing for the item.
#[derive(Default)]
pub(super) struct CjkText {
    face: Option<&'static CraftFont>,
    glyphs: Vec<Type3Glyph>,
    widths: HashMap<char, f64>,
}

type CjkRun<'a> = (&'a str, Vec<u8>, f64, usize);

impl CjkText {
    pub(super) fn new(t: &AddedText) -> Result<Self, (EditError, char)> {
        let mut chars = Vec::new();
        let mut seen = HashSet::new();
        for ch in t.text.chars().filter(|ch| is_cjk(*ch)) {
            if seen.insert(ch) {
                // Bound retained outline data, as the existing Unicode text planner does.
                if chars.len() >= pdfcraft_fonts::MAX_PLAN_GLYPHS {
                    return Err((EditError::Invalid("too many distinct CJK glyphs; use a smaller text box".into()), ch));
                }
                chars.push(ch);
            }
        }
        let Some(&first) = chars.first() else { return Ok(Self::default()) };
        let text: String = chars.iter().collect();
        let (faces, _) = type3_faces(&text, t.family, t.bold).map_err(|e| (e, first))?;
        let face = fallback_face(&faces, &chars, |face, ch| pdfcraft_fonts::face_has(face.bytes, ch)).ok_or_else(|| (missing(first), first))?;
        let mut glyphs = Vec::with_capacity(chars.len());
        let mut widths = HashMap::with_capacity(chars.len());
        for (i, ch) in chars.into_iter().enumerate() {
            let (path, width) = type3_glyph(face, ch).map_err(|e| (e, ch))?.ok_or_else(|| (missing(ch), ch))?;
            let code = u8::try_from(i % pdfcraft_fonts::MAX_TYPE3_GLYPHS + 1).map_err(|_| (crate::undrawable(ch), ch))?;
            widths.insert(ch, (width * 1000.0).round() / 1000.0);
            glyphs.push(Type3Glyph { code, ch, path, width });
        }
        Ok(Self { face: Some(face), glyphs, widths })
    }

    pub(super) fn first_undrawable(&self, s: &str) -> Option<char> {
        s.chars().find(|ch| !pdfcraft_fonts::win_ansi_encodable(*ch) && !self.widths.contains_key(ch))
    }

    pub(super) fn width(&self, t: &AddedText, s: &str) -> f64 {
        if self.widths.is_empty() {
            return latin_width(t, s);
        }
        let mut buf = [0; 4];
        s.chars()
            .map(|ch| self.widths.get(&ch).map_or_else(|| latin_width(t, ch.encode_utf8(&mut buf)), |width| width * t.size + t.char_spacing))
            .sum()
    }

    pub(super) fn register(&mut self, doc: &mut Document, taken: &Dict, fonts: &mut Dict) -> Option<Type3Fallback> {
        let face = self.face?;
        let mut reserved = taken.clone();
        for (key, value) in fonts.iter() {
            reserved.set(key.clone(), value.clone());
        }
        let mut written = write_type3_glyphs(doc, &mut reserved, face, false, std::mem::take(&mut self.glyphs), pdfcraft_fonts::MAX_TYPE3_GLYPHS);
        for name in &mut written.names {
            if let Some(font) = reserved.get(name.as_bytes()) {
                let font = font.clone();
                let owned = unique_font_name(&reserved, "PCCjk");
                reserved.set(owned.as_bytes().to_vec(), font.clone());
                fonts.set(owned.as_bytes().to_vec(), font);
                *name = owned;
            }
        }
        Some(written)
    }

    pub(super) fn runs<'a>(
        &self,
        t: &AddedText,
        text: &str,
        latin: &'a str,
        fallback: Option<&'a Type3Fallback>,
    ) -> Result<Vec<CjkRun<'a>>, EditError> {
        let mut runs: Vec<CjkRun<'_>> = Vec::new();
        let mut buf = [0; 4];
        for ch in text.chars() {
            let drawn = fallback.and_then(|f| f.codes.get(&ch).and_then(|(i, code)| f.names.get(*i).map(|name| (name.as_str(), *code))));
            let (name, code) = match drawn {
                Some(drawn) => drawn,
                None => (latin, if ch == '\t' { b' ' } else { pdfcraft_fonts::win_ansi_byte(ch).ok_or_else(|| crate::undrawable(ch))? }),
            };
            let width = self.width(t, ch.encode_utf8(&mut buf));
            match runs.last_mut() {
                Some((font, bytes, advance, spaces)) if *font == name => {
                    bytes.push(code);
                    *advance += width;
                    *spaces += usize::from(ch == ' ');
                }
                _ => runs.push((name, vec![code], width, usize::from(ch == ' '))),
            }
        }
        Ok(runs)
    }
}
