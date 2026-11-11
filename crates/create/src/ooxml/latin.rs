//! Latin letters beyond WinAnsiEncoding that the twelve standard-14 text faces still draw.
//!
//! The standard Helvetica, Times and Courier faces carry the Central European, Turkish, Baltic
//! and Romanian letters under their Adobe Glyph List names, but WinAnsiEncoding has no codes for
//! them. Each face is therefore written a second time with an encoding of its own, whose
//! `/Differences` give these glyph names codes from [`FIRST_CODE`] up, and whose `/ToUnicode`
//! says what each code means so the text stays searchable. Without this, Turkish `ğ ı İ ş Ş Ğ`
//! were written as `[U+XXXX]`.
//!
//! The list is the letters every viewer can draw from a font that isn't embedded: those whose
//! glyph is a base letter plus one plain accent, which is what PdfCraft's own renderer (and the
//! Foxit stand-ins other viewers use) builds when the stand-in lacks the glyph; see
//! `ACCENTED_LETTERS` in `vendor/hayro-interpret/src/font/standard_font.rs`. `ď ľ Ľ ť` (a caron
//! drawn as an apostrophe beside the letter), `ģ` (a turned comma above) and `Đ đ` (a stroke)
//! would draw as nothing there, so they stay `[U+XXXX]`.
//!
//! pdfcraft-fonts has exact widths for the WinAnsi glyphs only, so each letter here is measured as
//! the WinAnsi letter it is built on (`ğ` as `g`), which is also how the composites are built.

/// The code of the first letter in [`LETTERS`]; codes run on from it in table order.
pub(crate) const FIRST_CODE: u8 = 0x21;

/// `(letter, glyph name, the WinAnsi letter its advance is taken from)`.
pub(crate) const LETTERS: [(char, &str, char); 75] = [
    ('\u{0100}', "Amacron", 'A'),
    ('\u{0101}', "amacron", 'a'),
    ('\u{0102}', "Abreve", 'A'),
    ('\u{0103}', "abreve", 'a'),
    ('\u{0104}', "Aogonek", 'A'),
    ('\u{0105}', "aogonek", 'a'),
    ('\u{0106}', "Cacute", 'C'),
    ('\u{0107}', "cacute", 'c'),
    ('\u{010C}', "Ccaron", 'C'),
    ('\u{010D}', "ccaron", 'c'),
    ('\u{010E}', "Dcaron", 'D'),
    ('\u{0112}', "Emacron", 'E'),
    ('\u{0113}', "emacron", 'e'),
    ('\u{0116}', "Edotaccent", 'E'),
    ('\u{0117}', "edotaccent", 'e'),
    ('\u{0118}', "Eogonek", 'E'),
    ('\u{0119}', "eogonek", 'e'),
    ('\u{011A}', "Ecaron", 'E'),
    ('\u{011B}', "ecaron", 'e'),
    ('\u{011E}', "Gbreve", 'G'),
    ('\u{011F}', "gbreve", 'g'),
    ('\u{0122}', "Gcommaaccent", 'G'),
    ('\u{012A}', "Imacron", 'I'),
    ('\u{012B}', "imacron", 'i'),
    ('\u{012E}', "Iogonek", 'I'),
    ('\u{012F}', "iogonek", 'i'),
    ('\u{0130}', "Idotaccent", 'I'),
    ('\u{0131}', "dotlessi", 'i'),
    ('\u{0136}', "Kcommaaccent", 'K'),
    ('\u{0137}', "kcommaaccent", 'k'),
    ('\u{0139}', "Lacute", 'L'),
    ('\u{013A}', "lacute", 'l'),
    ('\u{013B}', "Lcommaaccent", 'L'),
    ('\u{013C}', "lcommaaccent", 'l'),
    ('\u{0141}', "Lslash", 'L'),
    ('\u{0142}', "lslash", 'l'),
    ('\u{0143}', "Nacute", 'N'),
    ('\u{0144}', "nacute", 'n'),
    ('\u{0145}', "Ncommaaccent", 'N'),
    ('\u{0146}', "ncommaaccent", 'n'),
    ('\u{0147}', "Ncaron", 'N'),
    ('\u{0148}', "ncaron", 'n'),
    ('\u{014C}', "Omacron", 'O'),
    ('\u{014D}', "omacron", 'o'),
    ('\u{0150}', "Ohungarumlaut", 'O'),
    ('\u{0151}', "ohungarumlaut", 'o'),
    ('\u{0154}', "Racute", 'R'),
    ('\u{0155}', "racute", 'r'),
    ('\u{0156}', "Rcommaaccent", 'R'),
    ('\u{0157}', "rcommaaccent", 'r'),
    ('\u{0158}', "Rcaron", 'R'),
    ('\u{0159}', "rcaron", 'r'),
    ('\u{015A}', "Sacute", 'S'),
    ('\u{015B}', "sacute", 's'),
    ('\u{015E}', "Scedilla", 'S'),
    ('\u{015F}', "scedilla", 's'),
    // The glyph list gives Ţ ţ this name; Romanian Ț ț below share it, as the standard fonts do.
    ('\u{0162}', "Tcommaaccent", 'T'),
    ('\u{0163}', "tcommaaccent", 't'),
    ('\u{0164}', "Tcaron", 'T'),
    ('\u{016A}', "Umacron", 'U'),
    ('\u{016B}', "umacron", 'u'),
    ('\u{016E}', "Uring", 'U'),
    ('\u{016F}', "uring", 'u'),
    ('\u{0170}', "Uhungarumlaut", 'U'),
    ('\u{0171}', "uhungarumlaut", 'u'),
    ('\u{0172}', "Uogonek", 'U'),
    ('\u{0173}', "uogonek", 'u'),
    ('\u{0179}', "Zacute", 'Z'),
    ('\u{017A}', "zacute", 'z'),
    ('\u{017B}', "Zdotaccent", 'Z'),
    ('\u{017C}', "zdotaccent", 'z'),
    // Romanian comma-below letters.
    ('\u{0218}', "Scommaaccent", 'S'),
    ('\u{0219}', "scommaaccent", 's'),
    ('\u{021A}', "Tcommaaccent", 'T'),
    ('\u{021B}', "tcommaaccent", 't'),
];

fn index(c: char) -> Option<usize> {
    // The table is sorted by letter.
    LETTERS.binary_search_by(|(l, _, _)| l.cmp(&c)).ok()
}

/// The code `c` has in the extra encoding, if it is one of [`LETTERS`].
pub(crate) fn code(c: char) -> Option<u8> {
    // At most 0x21 + 74, so the conversion always succeeds.
    index(c).and_then(|i| u8::try_from(i).ok()).and_then(|i| FIRST_CODE.checked_add(i))
}

/// The WinAnsi letter whose advance stands in for `c`, or `c` itself.
pub(crate) fn measured_as(c: char) -> char {
    index(c).and_then(|i| LETTERS.get(i)).map_or(c, |(_, _, base)| *base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_stays_off_winansi() {
        assert!(LETTERS.windows(2).all(|w| w[0].0 < w[1].0), "binary search needs the letters in order");
        for (c, name, base) in LETTERS {
            assert!(pdfcraft_fonts::win_ansi_byte(c).is_none(), "{c} {name}");
            assert!(pdfcraft_fonts::win_ansi_byte(base).is_some(), "{c} is measured as a WinAnsi letter");
        }
        let gbreve = code('ğ').and_then(|c| LETTERS.get(usize::from(c - FIRST_CODE)));
        assert_eq!(gbreve.map(|l| l.1), Some("gbreve"));
        // Letters no stand-in can build are left out, to be written as [U+XXXX].
        assert_eq!([code('ť'), code('đ'), code('ģ')], [None; 3]);
        assert_eq!(code('g'), None);
        assert_eq!(measured_as('ı'), 'i');
        assert_eq!(measured_as('x'), 'x');
        let last = u32::from(FIRST_CODE) + LETTERS.len() as u32 - 1;
        assert!(last <= 0xFF);
    }
}
