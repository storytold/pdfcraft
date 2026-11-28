//! Invisible format characters in a page's text: no font has a glyph for them and they take no
//! room, so where nothing is drawn for them the text read for editing leaves them out, and an edit
//! there doesn't ask a font for one. A glyph drawn for one (a Unicode chart's box) keeps its text
//! (`MarkedContent::end`), whatever [`InvisibleMarks`] would leave out.
//!
//! Chrome's PDFs (Google Docs exports included) give every run an `/ActualText` that wraps its
//! text in U+202D LEFT-TO-RIGHT OVERRIDE and U+202C POP DIRECTIONAL FORMATTING. Read as they
//! were, every paragraph and every draft made from it carried them, and every edit failed on a
//! missing glyph for U+202D.

/// Which invisible format characters a text leaves out.
///
/// The zero-width space, the word joiner and invisible operators (U+2060 to U+2064), the byte
/// order mark and the Mongolian vowel separator always go. The bidi marks, embeddings, overrides
/// and isolates go when the text has no right-to-left letter, where they reorder nothing; in
/// right-to-left text they stay, because shaping the paragraph's own font orders the text with
/// them ([`pdfcraft_fonts::source_shaping`]). The joiners (U+200C, U+200D) stay, since shaping
/// reads them, and so do variation selectors, which the fallback draws as empty glyphs (#4861).
#[derive(Clone, Copy)]
pub(crate) struct InvisibleMarks {
    bidi: bool,
}

impl InvisibleMarks {
    /// The marks `text` leaves out. Only a letter counts as right-to-left: not the punctuation of
    /// those scripts' blocks (U+060C ARABIC COMMA in Latin text), nor U+061C ARABIC LETTER MARK,
    /// which is a control.
    pub(crate) fn of(text: &str) -> Self {
        Self { bidi: !text.chars().any(|c| crate::bidi::is_rtl(c) && c.is_alphabetic()) }
    }

    pub(crate) fn drops(self, c: char) -> bool {
        is_zero_width(c) || (self.bidi && crate::bidi::is_bidi_control(c))
    }

    /// `text` without the marks.
    pub(crate) fn strip(self, text: &str) -> std::borrow::Cow<'_, str> {
        if text.chars().any(|c| self.drops(c)) { text.chars().filter(|c| !self.drops(*c)).collect::<String>().into() } else { text.into() }
    }
}

/// The zero-width characters [`InvisibleMarks`] always leaves out.
fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{180E}' | '\u{200B}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Right-to-left text keeps its bidi controls for the reordering; zero-width characters go
    /// whatever the script, and the joiners stay for shaping.
    #[test]
    fn right_to_left_text_keeps_its_bidi_controls() {
        let hebrew = InvisibleMarks::of("\u{200F}שלום\u{202B}");
        assert!(!hebrew.drops('\u{200F}') && !hebrew.drops('\u{202B}'));
        assert!(hebrew.drops('\u{200B}') && hebrew.drops('\u{FEFF}'));
        assert!(InvisibleMarks::of("\u{061C}abc").drops('\u{061C}'), "U+061C is no right-to-left letter");
        // Right-to-left punctuation in Latin text is no right-to-left letter either.
        for punctuation in ['\u{060C}', '\u{061F}', '\u{05BE}'] {
            let text = format!("\u{202D}Hello{punctuation} world\u{202C}");
            assert_eq!(InvisibleMarks::of(&text).strip(&text), format!("Hello{punctuation} world"), "U+{:04X}", u32::from(punctuation));
        }
        let latin = InvisibleMarks::of("\u{202D}abc\u{202C}");
        assert!(latin.drops('\u{202D}') && latin.drops('\u{202C}') && latin.drops('\u{2066}') && latin.drops('\u{200E}'));
        for kept in ['\u{200C}', '\u{200D}', '\u{FE0F}', '\u{00AD}', ' ', 'a'] {
            assert!(!latin.drops(kept), "U+{:04X}", u32::from(kept));
        }
        assert_eq!(latin.strip("\u{202D}abc\u{202C}"), "abc");
    }
}
