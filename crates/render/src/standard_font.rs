//! The font programs PdfCraft draws the standard 14 fonts with when a document names one without
//! embedding it.

use hayro::hayro_interpret::font::StandardFont;
use pdfcraft_fonts::Std14;

/// The program PdfCraft draws `face` with when a document doesn't embed it: one of the
/// BSD-3-Clause Foxit substitutes bundled with the vendored renderer (`ATTRIBUTION.toml`), as a
/// bare CFF font (what a `/FontFile3` of subtype `/Type1C` holds, ISO 32000-2 §9.9). A document
/// that embeds it shows its text in the glyphs PdfCraft already draws it with.
pub fn standard_font_program(face: Std14) -> Vec<u8> {
    let font = match face {
        Std14::Helvetica => StandardFont::Helvetica,
        Std14::HelveticaBold => StandardFont::HelveticaBold,
        Std14::HelveticaOblique => StandardFont::HelveticaOblique,
        Std14::HelveticaBoldOblique => StandardFont::HelveticaBoldOblique,
        Std14::TimesRoman => StandardFont::TimesRoman,
        Std14::TimesBold => StandardFont::TimesBold,
        Std14::TimesItalic => StandardFont::TimesItalic,
        Std14::TimesBoldItalic => StandardFont::TimesBoldItalic,
        Std14::Courier => StandardFont::Courier,
        Std14::CourierBold => StandardFont::CourierBold,
        Std14::CourierOblique => StandardFont::CourierOblique,
        Std14::CourierBoldOblique => StandardFont::CourierBoldOblique,
        Std14::Symbol => StandardFont::Symbol,
        Std14::ZapfDingbats => StandardFont::ZapfDingBats,
    };
    let (data, _) = font.get_font_data();
    (*data).as_ref().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_font_has_a_cff_program() {
        for face in [Std14::Helvetica, Std14::TimesBoldItalic, Std14::CourierOblique, Std14::Symbol, Std14::ZapfDingbats] {
            let program = standard_font_program(face);
            // A CFF header: major version 1, then its own size.
            assert_eq!(program.get(..2), Some(&[1u8, 0][..]), "{face:?}");
        }
    }
}
