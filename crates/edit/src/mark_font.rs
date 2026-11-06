//! Text the standard fonts can't draw (Vietnamese, Cyrillic, Greek, Latin-extended) in headers,
//! footers and watermarks. Each glyph comes from the bundled Inter face (`assets/fonts`, OFL) as a
//! Type 3 outline, the way Edit text already draws its fallback text: no font file is embedded, so
//! the PDF doesn't grow by the size of a font, and a ToUnicode map keeps the text selectable.
//!
//! The text is composed to NFC first. A PDF text show positions no combining marks, so Telex-typed
//! (decomposed) Vietnamese would otherwise draw its accents apart from their letters.

use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;

use super::{Dict, Document, EditError, Object};
use crate::text::fallback::{MAX_TYPE3_GLYPHS, Type3Glyph, type3_glyph, write_type3_glyphs};

/// `text` composed to NFC.
fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// `text` as the marks draw it: composed to NFC, with Windows line endings and the spacing
/// characters a PDF text show can't position (tab, no-break space) written as plain spaces.
///
/// Every mark path runs its text through this before deciding on a face or measuring it, so the
/// preview and applying see the same characters. A lone `\r` is left alone: it is still refused
/// (it would be drawn as `?`), as it always was.
pub fn sanitize_text(text: &str) -> String {
    nfc(text).replace("\r\n", "\n").replace('\t', "    ").replace('\u{00a0}', " ")
}

/// The lines of a whole mark's `texts` (every header and footer box, or the watermark), sanitized.
pub(crate) fn sanitized_lines<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut lines = Vec::new();
    for text in texts {
        let text = sanitize_text(text);
        lines.extend(text.lines().map(str::to_owned));
    }
    lines
}

fn inter() -> Option<&'static pdfcraft_fonts::CraftFont> {
    pdfcraft_fonts::document_latin_fonts(false).into_iter().next()
}

fn drawn_in_inter(c: char) -> bool {
    if c == '\r' {
        return false;
    }
    let Some(face) = inter() else { return false };
    type3_glyph(face, c).ok().flatten().is_some()
}

/// The character of a whole mark's `lines` to name when its text is refused: the first one the
/// face that [`MarkFont`] chooses for the block can't draw. `None` if every character is drawable.
pub(crate) fn refused_char<'a>(lines: impl IntoIterator<Item = &'a str> + Clone) -> Option<char> {
    // The face is chosen once for the whole block, exactly as `MarkFont` does it. Checking line by
    // line instead would miss a character the block's face can't draw when another line chose it.
    let use_inter = needs_inter(lines.clone());
    lines.into_iter().find_map(|line| {
        sanitize_text(line).chars().find(|&c| {
            if c == '\n' {
                return false;
            }
            // A lone `\r` is refused in either face, as it always was (Inter doesn't draw it).
            if c == '\r' {
                return true;
            }
            if use_inter {
                // Every character of the block is shown in Inter.
                !drawn_in_inter(c)
            } else {
                // The standard fonts draw WinAnsi; anything else is only drawable in Inter, and
                // Inter was not chosen because some character of the block is missing from it.
                !pdfcraft_fonts::win_ansi_encodable(c) && !drawn_in_inter(c)
            }
        })
    })
}

/// Whether `lines` need Inter: something WinAnsi can't encode, and all of it drawable in Inter.
pub(crate) fn needs_inter<'a>(lines: impl IntoIterator<Item = &'a str>) -> bool {
    let mut any = false;
    for line in lines {
        for c in sanitize_text(line).chars() {
            if !pdfcraft_fonts::win_ansi_encodable(c) {
                if !drawn_in_inter(c) {
                    return false;
                }
                any = true;
            }
        }
    }
    any
}

/// The Type 3 fonts for the text of one run of marks, made once and shared by every page.
pub(crate) struct MarkFont {
    /// The font objects (references), in order.
    fonts: Vec<Object>,
    /// Where each character is shown: the index of its font, and its code there.
    codes: HashMap<char, (usize, u8)>,
    /// Every character's advance, in em.
    widths: HashMap<char, f64>,
}

impl MarkFont {
    /// The fonts for every character of `lines`. All outlines are read before anything is added
    /// to the document, so a refusal leaves it as it was.
    pub(crate) fn new<'a>(doc: &mut Document, lines: impl IntoIterator<Item = &'a str>) -> Result<Self, EditError> {
        let face = inter().ok_or_else(|| EditError::Invalid("the bundled Inter face is missing".into()))?;
        let mut chars: Vec<char> = Vec::new();
        for line in lines {
            for c in sanitize_text(line).chars() {
                if c != '\n' && c != '\r' && !chars.contains(&c) {
                    chars.push(c);
                }
            }
        }
        let mut glyphs = Vec::with_capacity(chars.len());
        let mut widths = HashMap::new();
        for (k, ch) in chars.into_iter().enumerate() {
            let (path, width) = type3_glyph(face, ch)?.ok_or_else(|| crate::place::undrawable(ch))?;
            // Codes in a font run 1 to 240; `write_type3_glyphs` starts a new font after that many.
            let code = u8::try_from(k % MAX_TYPE3_GLYPHS + 1).map_err(|_| EditError::Invalid("too many different characters".into()))?;
            widths.insert(ch, width);
            glyphs.push(Type3Glyph { code, ch, path, width });
        }
        // The fonts are written into a scratch dictionary: each becomes one object every page shares.
        let mut scratch = Dict::new();
        let made = write_type3_glyphs(doc, &mut scratch, face, false, glyphs, MAX_TYPE3_GLYPHS);
        let fonts =
            made.names.iter().filter_map(|name| scratch.get(name.as_bytes()).cloned()).map(|font| Object::Ref(doc.add(font))).collect::<Vec<_>>();
        if fonts.len() != made.names.len() {
            return Err(EditError::Invalid("the Inter glyph fonts went missing".into()));
        }
        Ok(Self { fonts, codes: made.codes, widths })
    }

    /// The width of `line` at `size`.
    pub(crate) fn width(&self, line: &str, size: f64) -> f64 {
        let w: f64 = sanitize_text(line).chars().map(|c| self.widths.get(&c).copied().unwrap_or(0.0)).sum();
        if w.is_finite() { w * size } else { 0.0 }
    }

    /// Put the fonts in `page`'s own `/Font` resources (shared dictionaries stay untouched), under
    /// names it doesn't use, and return those names.
    pub(crate) fn add_to_page(&self, doc: &mut Document, page: &pdfcraft_model::Page) -> Result<Vec<String>, EditError> {
        let mut res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let mut fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
        let mut names = Vec::with_capacity(self.fonts.len());
        for font in &self.fonts {
            let mut k = 0usize;
            let name = loop {
                let name = format!("PCMk{k}");
                if !fonts.contains(name.as_bytes()) {
                    break name;
                }
                k = k.saturating_add(1);
            };
            fonts.set(name.clone().into_bytes(), font.clone());
            names.push(name);
        }
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        Ok(names)
    }

    /// The operators that show `line` with its origin at (`x`, `y`): one `Tm`, then a font switch
    /// only where the text moves to another font. `names` are what [`MarkFont::add_to_page`] returned.
    pub(crate) fn show(&self, names: &[String], size: f64, x: f64, y: f64, line: &str) -> Vec<u8> {
        let mut out = format!("1 0 0 1 {} {} Tm\n", super::n(x), super::n(y)).into_bytes();
        let mut run: Option<(usize, Vec<u8>)> = None;
        let flush = |run: &mut Option<(usize, Vec<u8>)>, out: &mut Vec<u8>| {
            if let Some((font, bytes)) = run.take()
                && let Some(name) = names.get(font)
            {
                out.extend(format!("/{name} {} Tf ", super::n(size)).bytes());
                out.extend(super::literal(&bytes));
                out.extend_from_slice(b" Tj\n");
            }
        };
        for c in sanitize_text(line).chars() {
            let Some(&(font, code)) = self.codes.get(&c) else { continue };
            match &mut run {
                Some((f, bytes)) if *f == font => bytes.push(code),
                _ => {
                    flush(&mut run, &mut out);
                    run = Some((font, vec![code]));
                }
            }
        }
        flush(&mut run, &mut out);
        out
    }
}
