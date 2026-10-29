//! The face header, footer and text watermark lines are drawn in when the standard faces can't
//! draw them (#2013): the bundled Inter (`pdfcraft_fonts::document_latin_fonts`), which every build
//! has and which carries the Latin, Greek and Cyrillic letters WinAnsiEncoding lacks. A line the
//! standard faces can draw keeps them, so Latin-only marks are drawn exactly as before.
//!
//! The face is embedded once per run, and every page the run marks draws that one font object.

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;

use super::EditError;
use super::marks::n;
use super::place::page_fonts;

/// The bundled face, Regular (marks have no bold).
fn face() -> Option<&'static pdfcraft_fonts::CraftFont> {
    pdfcraft_fonts::document_latin_fonts(false).into_iter().next()
}

/// Whether `line` is drawn in the bundled face: the standard faces can't draw all of it.
pub(crate) fn wanted(line: &str) -> bool {
    pdfcraft_fonts::first_non_win_ansi(line).is_some()
}

/// The character the bundled face draws for `c`. A tab is drawn as a space and a soft hyphen as a
/// hyphen, as the standard faces draw them (Inter has no glyph for either).
fn drawn_as(c: char) -> char {
    match c {
        '\t' => ' ',
        '\u{ad}' => '-',
        c => c,
    }
}

/// The first character of `line` that applying refuses: none when the standard faces draw all of
/// it, otherwise the first one the bundled face can't draw either. A control character other
/// than a tab (a lone `\r`) is refused.
pub(crate) fn first_undrawable(line: &str) -> Option<char> {
    let first = pdfcraft_fonts::first_non_win_ansi(line)?;
    let Some(face) = face() else { return Some(first) };
    let draws = |s: &str| !s.chars().any(char::is_control) && pdfcraft_fonts::embedding::covers(face.bytes, s);
    // The whole line at once; the character only for a line the face can't draw.
    if draws(&line.chars().map(drawn_as).collect::<String>()) {
        return None;
    }
    let mut buf = [0u8; 4];
    line.chars().find(|&c| !draws(drawn_as(c).encode_utf8(&mut buf)))
}

/// The refusal for header, footer or watermark text with `c`, which neither the standard faces
/// nor the bundled one can draw.
pub(crate) fn undrawable(c: char) -> EditError {
    EditError::Invalid(undrawable_message(c))
}

/// The wording of [`undrawable`].
pub(crate) fn undrawable_message(c: char) -> String {
    format!(
        "neither the standard fonts nor the bundled Inter face can draw \"{}\" (U+{:04X}); headers, footers and watermarks can be written in Latin, Greek and Cyrillic letters for now",
        c.escape_debug(),
        u32::from(c)
    )
}

/// The bundled face, embedded in a document for one run of headers and footers or watermarks.
pub(crate) struct MarkFont {
    font: ObjRef,
    metrics: Metrics,
}

impl MarkFont {
    /// The face, embedded when any of `lines` needs it (`None` when none does, or when the run
    /// marks no page). An embedding of it that `first` (the run's first page) already draws, from
    /// an earlier run, is reused, so Update adds no second copy. Ask after every check that can
    /// refuse the run.
    pub(crate) fn for_lines<'a>(
        doc: &mut Document,
        first: Option<&pdfcraft_model::Page>,
        mut lines: impl Iterator<Item = &'a str>,
    ) -> Result<Option<Self>, EditError> {
        let Some(first) = first else { return Ok(None) };
        if !lines.any(wanted) {
            return Ok(None);
        }
        let face = face().ok_or_else(|| EditError::Invalid("this build has no bundled Latin, Greek and Cyrillic face".into()))?;
        let mut fonts = page_fonts(doc, first);
        let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, face.bytes, face.family).map_err(EditError::Invalid)?;
        let font = match fonts.get(name.as_bytes()) {
            Some(Object::Ref(r)) => *r,
            // Another writer put the font in the page directly: the run shares a copy of it.
            Some(other) => doc.add(other.clone()),
            None => return Err(EditError::Invalid("the bundled face could not be added".into())),
        };
        let dict = doc.dict(&Object::Ref(font)).ok_or_else(|| EditError::Invalid("the bundled face could not be read".into()))?;
        Ok(Some(Self { font, metrics: Metrics::from_dict(doc, &dict) }))
    }

    /// The codes that show `line` in the face (a tab as a space, a soft hyphen as a hyphen), and
    /// its advance per point of font size. A character the face lacks is refused, never drawn as
    /// something else.
    pub(crate) fn encode(&self, line: &str) -> Result<(Vec<u8>, f64), EditError> {
        let line: String = line.chars().map(drawn_as).collect();
        let mut buf = [0u8; 4];
        let bytes = self.metrics.encode(&line).ok_or_else(|| {
            line.chars()
                .find(|c| self.metrics.encode(c.encode_utf8(&mut buf)).is_none())
                .map_or_else(|| EditError::Invalid(format!("the bundled Inter face can't draw \"{}\"", line.escape_debug())), undrawable)
        })?;
        let advance = self.metrics.codes(&bytes).iter().map(|(code, _)| self.metrics.width(*code)).sum();
        Ok((bytes, advance))
    }

    /// The name `fonts` (a page's) draws the face by: the one they already give it, else the first
    /// free `PCFull<k>`, the name the embedding gives a full face, so later embeddings on the
    /// page recognise it. `true` when the name still has to be added.
    pub(crate) fn name_in(&self, fonts: &Dict) -> (String, bool) {
        if let Some((key, _)) = fonts.iter().find(|(_, v)| matches!(v, Object::Ref(r) if *r == self.font)) {
            return (String::from_utf8_lossy(key).into_owned(), false);
        }
        // One of the first `fonts.len() + 1` names is always free.
        let free = (0..=fonts.len()).map(|k| format!("PCFull{k}")).find(|name| !fonts.contains(name.as_bytes()));
        (free.unwrap_or_else(|| format!("PCFull{}", fonts.len().saturating_add(1))), true)
    }

    /// Adds the face to `page`'s own resources as `name` (a shared dictionary stays untouched).
    pub(crate) fn add_to_page(&self, doc: &mut Document, page: &pdfcraft_model::Page, name: &str) -> Result<(), EditError> {
        let mut res = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let mut fonts = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
        fonts.set(name.as_bytes().to_vec(), Object::Ref(self.font));
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        Ok(())
    }
}

/// Shows `bytes` (from [`MarkFont::encode`]) in the face named `name` at `size`, its baseline
/// starting at (`x`, `y`), then selects `back` again, so the lines after it keep their face.
pub(crate) fn show(name: &str, back: &str, size: f64, x: f64, y: f64, bytes: &[u8]) -> Vec<u8> {
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!("/{name} {size} Tf\n1 0 0 1 {} {} Tm <{hex}> Tj\n/{back} {size} Tf\n", n(x), n(y), size = n(size)).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the standard faces draw, the bundled face draws too, so a line is refused
    /// only for a character neither can draw (and the refusal says so truthfully).
    #[test]
    fn the_bundled_face_draws_everything_the_standard_faces_draw() {
        let standard: String = (0..=0x2122).filter_map(char::from_u32).filter(|c| pdfcraft_fonts::win_ansi_encodable(*c)).collect();
        assert!(standard.chars().count() > 200, "{standard}");
        assert_eq!(first_undrawable(&format!("Ж {standard} Ω ł")), None);
        let mut doc = Document::new_empty();
        let page = pdfcraft_model::Page { obj: doc.add(Object::Dict(Dict::new())), dict: Dict::new() };
        assert!(MarkFont::for_lines(&mut doc, None, ["Ж"].into_iter()).unwrap().is_none(), "no page, nothing embedded");
        let font = MarkFont::for_lines(&mut doc, Some(&page), ["Ж"].into_iter()).unwrap().unwrap();
        let (bytes, _) = font.encode(&standard).unwrap();
        assert_eq!(font.metrics.decode(&bytes), standard.replace('\t', " ").replace('\u{ad}', "-"));
    }

    #[test]
    fn lines_the_standard_faces_draw_are_left_to_them() {
        for (line, wants, refused) in [
            ("Page 1 of 3, café 5€ ™", false, None),
            ("Страница 1 от 3", true, None),
            ("Σελίδα 1", true, None),
            ("Strona 1 z 3, Łódź", true, None),
            ("Tab\tand Ж", true, None),
            ("Ж\rB", true, Some('\r')),
            ("Бележка: 签名", true, Some('签')),
            ("مرحبا", true, Some('م')),
        ] {
            assert_eq!(wanted(line), wants, "{line:?}");
            assert_eq!(first_undrawable(line), refused, "{line:?}");
        }
    }
}
