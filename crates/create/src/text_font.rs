//! The font a text document is set in: standard Helvetica, or, for text Helvetica can't draw, the
//! bundled Inter face (`pdfcraft_fonts::document_latin_fonts`), which every build has and which
//! carries the Latin, Greek and Cyrillic letters WinAnsiEncoding lacks. Text Helvetica draws is
//! set in Helvetica exactly as before.

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{helvetica_width, literal, win_ansi};

use crate::CreateError;

/// The bundled face, Regular.
fn face() -> Option<&'static pdfcraft_fonts::CraftFont> {
    pdfcraft_fonts::document_latin_fonts(false).into_iter().next()
}

/// Line breaks, tabs and form feeds lay the text out; they are not drawn.
fn layout(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\t' | '\u{c}')
}

/// The character the bundled face draws for `c`: a soft hyphen as a hyphen, as Helvetica draws it
/// (Inter has no glyph for it).
pub(crate) fn drawn_as(c: char) -> char {
    if c == '\u{ad}' { '-' } else { c }
}

/// The line (1-based, lines split at `\n`) and character of the first one the bundled face can't
/// draw, or `None` when it draws all of `text`.
pub(crate) fn first_missing(text: &str) -> Option<(usize, char)> {
    let face = face();
    let draws = |s: &str| !s.chars().any(char::is_control) && face.is_some_and(|f| pdfcraft_fonts::embedding::covers(f.bytes, s));
    let mut buf = [0u8; 4];
    text.split('\n').enumerate().find_map(|(i, line)| {
        // A whole line at once; the character only for a line the face can't draw.
        let drawn: String = line.chars().filter(|c| !layout(*c)).map(drawn_as).collect();
        if draws(&drawn) {
            return None;
        }
        line.chars().find(|&c| !layout(c) && !draws(drawn_as(c).encode_utf8(&mut buf))).map(|c| (i + 1, c))
    })
}

/// The refusal for text with `c` on `line`, which neither Helvetica nor the bundled face can draw.
pub(crate) fn undrawable_message(line: usize, c: char) -> String {
    format!(
        "neither Helvetica nor the bundled Inter face can draw \"{}\" (U+{:04X}) in the text (line {line}); only Latin, Greek and Cyrillic letters can be written there for now",
        c.escape_debug(),
        u32::from(c)
    )
}

/// The font a text document is set in.
pub(crate) struct TextFont {
    font: ObjRef,
    /// The bundled face's metrics; `None` for Helvetica.
    inter: Option<Metrics>,
}

impl TextFont {
    /// Standard Helvetica in WinAnsiEncoding, added to `doc`.
    pub(crate) fn helvetica(doc: &mut Document) -> Self {
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        Self { font: doc.add(font), inter: None }
    }

    /// The bundled face, embedded in `doc` once for every page.
    pub(crate) fn inter(doc: &mut Document) -> Result<Self, CreateError> {
        let face = face().ok_or_else(|| CreateError::Invalid("this build has no bundled Latin, Greek and Cyrillic face".into()))?;
        let mut fonts = Dict::new();
        let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, face.bytes, face.family).map_err(CreateError::Invalid)?;
        let font = fonts.reference(name.as_bytes()).ok_or_else(|| CreateError::Invalid("the bundled face could not be added".into()))?;
        let dict = doc.dict(&Object::Ref(font)).ok_or_else(|| CreateError::Invalid("the bundled face could not be read".into()))?;
        Ok(Self { font, inter: Some(Metrics::from_dict(doc, &dict)) })
    }

    /// The font object every page names.
    pub(crate) fn font(&self) -> ObjRef {
        self.font
    }

    /// The advance of `s` at `size`; characters the face lacks count as nothing ([`Self::show`]
    /// refuses them).
    pub(crate) fn width(&self, s: &str, size: f64) -> f64 {
        match &self.inter {
            None => helvetica_width(s, size),
            Some(m) => m.encode(s).map_or(0.0, |codes| m.codes(&codes).iter().map(|(code, _)| m.width(*code)).sum::<f64>() * size),
        }
    }

    /// The string operand that shows `line`. Helvetica writes what WinAnsiEncoding can't hold as
    /// `?`, so its callers check first; the bundled face refuses a character it lacks.
    pub(crate) fn show(&self, line: &str) -> Result<Vec<u8>, CreateError> {
        let Some(m) = &self.inter else { return Ok(literal(&win_ansi(line))) };
        let codes = m.encode(line).ok_or_else(|| CreateError::Invalid(format!("the bundled Inter face can't draw \"{}\"", line.escape_debug())))?;
        let mut out = Vec::with_capacity(codes.len().saturating_mul(2).saturating_add(2));
        out.push(b'<');
        for b in codes {
            out.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        out.push(b'>');
        Ok(out)
    }
}
