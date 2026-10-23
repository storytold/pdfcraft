//! Standard-14 font metrics (ISO 32000-2 Annex D; the Adobe Core14 AFM metrics), as advance
//! widths in glyph space — thousandths of an em.
//!
//! A PDF may name any of the fourteen standard fonts without embedding a program, and the viewer
//! has to supply the metrics. These are those metrics, exactly, replacing the character-class
//! estimate that generated appearances used before: that estimate had Times at 0.9 x Helvetica,
//! which makes a capital A 600 where the specification says 722.
//!
//! **Generated.** The tables are data, not typefaces, and are the item AGENTS.md §1.1 permits as
//! Adobe-authored non-visual technical data; `ATTRIBUTION.toml` carries the entry with
//! `adobe_data = true`. Each row is indexed by character **code**, not by Unicode: the twelve text
//! fonts are indexed by WinAnsiEncoding, which is what PdfCraft writes into the appearance streams
//! it generates, and Symbol and ZapfDingbats by their own built-in encodings. A zero means the
//! encoding leaves that code undefined.

/// One of the fourteen standard fonts (ISO 32000-2 §9.6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Std14 {
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
    Symbol,
    ZapfDingbats,
}

/// How [`Std14::recognise`] matched a `/BaseFont` name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// A standard-14 face or a well-known alias of one (Arial for Helvetica, CourierNew for
    /// Courier). The document is asking for this face.
    Alias,
    /// Only a family hint matched -- "Serif", "Roman" or "Mono" in an otherwise unknown name.
    /// The face is a guess from the name, not the one the document asked for.
    FamilyHint,
}

impl Std14 {
    /// The font a `/BaseFont` name selects, by the spec's names and the common aliases viewers
    /// accept (`Arial` for `Helvetica`, `TimesNewRoman` for `Times`, `CourierNew` for `Courier`).
    /// A subset prefix such as `ABCDEF+Helvetica` is ignored.
    pub fn from_base_font(base: &str) -> Option<Self> {
        Self::recognise(base).map(|(face, _)| face)
    }

    /// [`Std14::from_base_font`], also saying *how* the name matched.
    ///
    /// Reporting needs the difference. "Arial" really is a request for Helvetica, but
    /// "SomeHouseSerif" is a house font whose name merely contains "Serif"; choosing Times for it
    /// is a reasonable guess from the name, not the face the document asked for, and a substitution
    /// report that called it a standard-14 name would be telling the reader something untrue.
    pub fn recognise(base: &str) -> Option<(Self, Match)> {
        let base = base.rsplit('+').next().unwrap_or(base);
        let flat: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect();
        let has = |w: &str| flat.contains(w);
        let (bold, italic) = (has("bold"), has("italic") || has("oblique"));
        // Two bools index a four-element array, so the subscript is provably 0..=3.
        let styled = |faces: [Self; 4]| faces[usize::from(bold) | usize::from(italic) << 1];
        let courier = || styled([Self::Courier, Self::CourierBold, Self::CourierOblique, Self::CourierBoldOblique]);
        let times = || styled([Self::TimesRoman, Self::TimesBold, Self::TimesItalic, Self::TimesBoldItalic]);
        if has("symbol") {
            return Some((Self::Symbol, Match::Alias));
        }
        if has("zapf") || has("dingbat") {
            return Some((Self::ZapfDingbats, Match::Alias));
        }
        if has("courier") {
            return Some((courier(), Match::Alias));
        }
        if has("mono") {
            return Some((courier(), Match::FamilyHint));
        }
        if has("times") {
            return Some((times(), Match::Alias));
        }
        if (has("serif") && !has("sans")) || has("roman") {
            return Some((times(), Match::FamilyHint));
        }
        if has("helvetica") || has("arial") {
            return Some((styled([Self::Helvetica, Self::HelveticaBold, Self::HelveticaOblique, Self::HelveticaBoldOblique]), Match::Alias));
        }
        None
    }

    /// The advance of one character `code`, in thousandths of an em. Zero where the font's
    /// encoding leaves the code undefined.
    pub fn width(self, code: u8) -> f64 {
        f64::from(crate::std14_metrics::WIDTHS[self as usize][code as usize])
    }

    /// The advance of `code` at `size` points.
    pub fn advance(self, code: u8, size: f64) -> f64 {
        self.width(code) * size / 1000.0
    }

    /// The advance of `text` at `size` points, encoded as the twelve text fonts are written:
    /// WinAnsiEncoding, with characters it cannot represent replaced as `win_ansi` replaces them.
    ///
    /// Symbol and ZapfDingbats have their own built-in encodings and are not WinAnsi, so this
    /// measures them as if they were; pass codes to [`Std14::width`] for those.
    pub fn text_width(self, text: &str, size: f64) -> f64 {
        crate::win_ansi(text).iter().map(|&c| self.width(c)).sum::<f64>() * size / 1000.0
    }

    /// Courier is the only fixed-pitch family of the fourteen.
    pub fn is_fixed_pitch(self) -> bool {
        matches!(self, Self::Courier | Self::CourierBold | Self::CourierOblique | Self::CourierBoldOblique)
    }

    /// The `/BaseFont` name for this font, as it is written into a PDF.
    pub fn base_font(self) -> &'static str {
        crate::std14_metrics::BASE_FONTS[self as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sans_serif_names_are_not_mapped_to_times() {
        assert!(!matches!(Std14::recognise("SansSerif"), Some((Std14::TimesRoman, _))));
        assert_eq!(Std14::recognise("Arial-SansSerif"), Some((Std14::Helvetica, Match::Alias)));
    }

    /// Widths from the published Core14 metrics, typed in here by hand so the table is checked
    /// against the specification rather than against whatever produced it.
    #[test]
    fn widths_match_the_published_metrics() {
        let cases: &[(Std14, char, u16)] = &[
            // Helvetica
            (Std14::Helvetica, ' ', 278),
            (Std14::Helvetica, 'A', 667),
            (Std14::Helvetica, 'W', 944),
            (Std14::Helvetica, 'i', 222),
            (Std14::Helvetica, 'm', 833),
            (Std14::Helvetica, '0', 556),
            (Std14::Helvetica, '@', 1015),
            (Std14::HelveticaBold, 'A', 722),
            (Std14::HelveticaBold, 'i', 278),
            // Times: not Helvetica scaled by 0.9, which is what the old estimate assumed.
            (Std14::TimesRoman, ' ', 250),
            (Std14::TimesRoman, 'A', 722),
            (Std14::TimesRoman, 'W', 944),
            (Std14::TimesRoman, 'i', 278),
            (Std14::TimesRoman, 'm', 778),
            (Std14::TimesBold, 'A', 722),
            (Std14::TimesItalic, 'A', 611),
            // Courier is fixed pitch at 600 for every glyph.
            (Std14::Courier, ' ', 600),
            (Std14::Courier, 'A', 600),
            (Std14::Courier, 'm', 600),
            (Std14::CourierBoldOblique, 'W', 600),
        ];
        for &(font, ch, want) in cases {
            let code = crate::win_ansi(&ch.to_string());
            assert_eq!(code.len(), 1, "{ch:?} is one WinAnsi byte");
            assert_eq!(font.width(code[0]) as u16, want, "{font:?} {ch:?}");
        }
    }

    #[test]
    fn courier_is_fixed_pitch_everywhere_it_is_defined() {
        for font in [Std14::Courier, Std14::CourierBold, Std14::CourierOblique, Std14::CourierBoldOblique] {
            assert!(font.is_fixed_pitch());
            for code in 0..=255u8 {
                let w = font.width(code);
                assert!(w == 0.0 || w == 600.0, "{font:?} code {code} has width {w}");
            }
        }
        assert!(!Std14::Helvetica.is_fixed_pitch());
    }

    #[test]
    fn every_text_font_covers_printable_ascii_and_latin1() {
        for font in [Std14::Helvetica, Std14::TimesRoman, Std14::Courier, Std14::TimesBoldItalic] {
            for code in 0x20..=0x7Eu8 {
                assert!(font.width(code) > 0.0, "{font:?} has no width for ASCII {code}");
            }
            for code in 0xA0..=0xFFu8 {
                assert!(font.width(code) > 0.0, "{font:?} has no width for Latin-1 {code}");
            }
        }
    }

    #[test]
    fn symbol_and_dingbats_have_their_own_encodings() {
        // Symbol's code 0x61 is alpha, not 'a'; ZapfDingbats' is a scissors glyph.
        assert!(Std14::Symbol.width(0x61) > 0.0);
        assert!(Std14::ZapfDingbats.width(0x61) > 0.0);
        // Neither defines the Latin-1 upper half the text fonts do.
        assert_eq!(Std14::Symbol.width(0xFF), 0.0);
    }

    #[test]
    fn base_font_names_round_trip() {
        for i in 0..14usize {
            let font = [
                Std14::Helvetica,
                Std14::HelveticaBold,
                Std14::HelveticaOblique,
                Std14::HelveticaBoldOblique,
                Std14::TimesRoman,
                Std14::TimesBold,
                Std14::TimesItalic,
                Std14::TimesBoldItalic,
                Std14::Courier,
                Std14::CourierBold,
                Std14::CourierOblique,
                Std14::CourierBoldOblique,
                Std14::Symbol,
                Std14::ZapfDingbats,
            ][i];
            assert_eq!(Std14::from_base_font(font.base_font()), Some(font), "{}", font.base_font());
        }
    }

    #[test]
    fn base_font_aliases_and_subset_prefixes_resolve() {
        assert_eq!(Std14::from_base_font("ABCDEF+Helvetica-Bold"), Some(Std14::HelveticaBold));
        assert_eq!(Std14::from_base_font("Arial"), Some(Std14::Helvetica));
        assert_eq!(Std14::from_base_font("Arial,BoldItalic"), Some(Std14::HelveticaBoldOblique));
        assert_eq!(Std14::from_base_font("TimesNewRoman,Bold"), Some(Std14::TimesBold));
        assert_eq!(Std14::from_base_font("CourierNew-Oblique"), Some(Std14::CourierOblique));
        assert_eq!(Std14::from_base_font("ZapfDingbats"), Some(Std14::ZapfDingbats));
        assert_eq!(Std14::from_base_font("Wingdings"), None);
        assert_eq!(Std14::from_base_font(""), None);
    }

    #[test]
    fn text_width_sums_the_glyphs_and_scales_with_size() {
        let h = Std14::Helvetica;
        // "AW" = 667 + 944 = 1611 thousandths of an em.
        assert!((h.text_width("AW", 1000.0) - 1611.0).abs() < 1e-9);
        assert!((h.text_width("AW", 12.0) - 1611.0 * 12.0 / 1000.0).abs() < 1e-9);
        assert_eq!(h.text_width("", 12.0), 0.0);
        // Courier: every character is 600, so the width is purely a count.
        assert!((Std14::Courier.text_width("hello", 10.0) - 5.0 * 6.0).abs() < 1e-9);
    }

    #[test]
    fn hostile_text_does_not_panic_and_stays_finite() {
        for font in [Std14::Helvetica, Std14::Symbol, Std14::Courier] {
            for s in ["", "\0", "\u{202E}", "💥", &"a".repeat(10_000), "\u{FFFD}"] {
                let w = font.text_width(s, 12.0);
                assert!(w.is_finite() && w >= 0.0, "{font:?} {s:?} -> {w}");
            }
            assert!(font.advance(0, 12.0).is_finite());
            assert!(font.advance(255, 0.0).is_finite());
        }
    }
}
