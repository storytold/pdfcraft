use super::{FontFace, UnicodeSegmentation};

/// Is `family` a symbol font (characters at U+F000 + code)?
pub(crate) fn is_symbol_font(family: &str) -> bool {
    let f = family.to_ascii_lowercase();
    f.starts_with("wingdings") || f == "symbol" || f.starts_with("webdings") || f.starts_with("marlett")
}

/// Does `face` draw the symbol-font character `c` (a U+F0xx private-use code)? Either directly at
/// the PUA, or, when the face has no private-use mapping (Apple's "Symbol" maps its glyphs to real
/// Unicode), at the Adobe Symbol encoding's real character, or at the ASCII character itself when
/// `c`'s code is plain printable ASCII (0x20–0x7E with no table entry).
pub(crate) fn covers_symbol(face: &FontFace, c: char) -> bool {
    symbol_font_draw(face, (c as u32) & 0xFF).is_some()
}

/// The character `ch` is drawn as when the item's face is the `a:sym` symbol font `sym`: a U+F0xx
/// private-use code becomes the face's glyph — at the PUA, or at the real Unicode the Symbol font
/// maps it to (Apple's "Symbol"). `None` when `ch` is not such a character or the face is not that
/// font, so the run's other characters pass through unchanged.
pub(crate) fn symbol_map_char(face: &FontFace, ch: char, sym: Option<&str>) -> Option<char> {
    let code = ch as u32;
    if !(0xF000..=0xF0FF).contains(&code) {
        return None;
    }
    let sym = sym?;
    if !is_symbol_font(sym) || !face.family.eq_ignore_ascii_case(sym) {
        return None;
    }
    symbol_font_draw(face, code)
}

/// The real-Unicode character the Symbol font's code `code` (0x00–0xFF) maps to, in the Adobe
/// Symbol encoding (Unicode, Inc.'s `MAPPINGS/VENDORS/ADOBE/symbol.txt`; Apple's Mac OS Symbol
/// shares this layout, with a few alternative Unicode choices — see `symbol_unicode_alts`): the
/// codes a document stores as U+F000 + code draw the same glyphs at these Unicode points. Apple's
/// "Symbol" on macOS has no private-use mapping at all — it maps the glyphs to these real characters
/// instead — so a document's U+F0xx symbol characters must be remapped to them there (PowerPoint
/// does the same against whatever the installed Symbol font covers). Only the non-ASCII, non-CUS
/// entries are listed: printable ASCII passes through as its own code, and the CUS (Corporate Use
/// Subarea) fragment glyphs have no standard Unicode point.
pub(crate) const fn symbol_unicode(code: u32) -> Option<u32> {
    Some(match code {
        // 0x20–0x7E: the few mathematical ones among the ASCII-range characters.
        0x22 => 0x2200,
        0x24 => 0x2203,
        0x27 => 0x220B,
        0x2A => 0x2217,
        0x2D => 0x2212,
        0x40 => 0x2245,
        0x5C => 0x2234,
        0x5E => 0x22A5,
        0x7E => 0x223C,
        // 0x41–0x5A: Greek uppercase.
        0x41 => 0x0391,
        0x42 => 0x0392,
        0x43 => 0x03A7,
        0x44 => 0x0394,
        0x45 => 0x0395,
        0x46 => 0x03A6,
        0x47 => 0x0393,
        0x48 => 0x0397,
        0x49 => 0x0399,
        0x4A => 0x03D1,
        0x4B => 0x039A,
        0x4C => 0x039B,
        0x4D => 0x039C,
        0x4E => 0x039D,
        0x4F => 0x039F,
        0x50 => 0x03A0,
        0x51 => 0x0398,
        0x52 => 0x03A1,
        0x53 => 0x03A3,
        0x54 => 0x03A4,
        0x55 => 0x03A5,
        0x56 => 0x03C2,
        0x57 => 0x03A9,
        0x58 => 0x039E,
        0x59 => 0x03A8,
        0x5A => 0x0396,
        // 0x61–0x7A: Greek lowercase (0x6A is the phi symbol, 0x6D the mu).
        0x61 => 0x03B1,
        0x62 => 0x03B2,
        0x63 => 0x03C7,
        0x64 => 0x03B4,
        0x65 => 0x03B5,
        0x66 => 0x03C6,
        0x67 => 0x03B3,
        0x68 => 0x03B7,
        0x69 => 0x03B9,
        0x6A => 0x03D5,
        0x6B => 0x03BA,
        0x6C => 0x03BB,
        0x6D => 0x03BC,
        0x6E => 0x03BD,
        0x6F => 0x03BF,
        0x70 => 0x03C0,
        0x71 => 0x03B8,
        0x72 => 0x03C1,
        0x73 => 0x03C3,
        0x74 => 0x03C4,
        0x75 => 0x03C5,
        0x76 => 0x03D6,
        0x77 => 0x03C9,
        0x78 => 0x03BE,
        0x79 => 0x03C8,
        0x7A => 0x03B6,
        // 0xA0–0xAF: euro, Greek upsilon with hook, primes, math, suits and arrows.
        0xA0 => 0x20AC,
        0xA1 => 0x03D2,
        0xA2 => 0x2032,
        0xA3 => 0x2264,
        0xA4 => 0x2215,
        0xA5 => 0x221E,
        0xA6 => 0x0192,
        0xA7 => 0x2663,
        0xA8 => 0x2666,
        0xA9 => 0x2665,
        0xAA => 0x2660,
        0xAB => 0x2194,
        0xAC => 0x2190,
        0xAD => 0x2191,
        0xAE => 0x2192,
        0xAF => 0x2193,
        // 0xB0–0xBC: degree, plus-minus, double prime, math and ellipsis.
        0xB0 => 0x00B0,
        0xB1 => 0x00B1,
        0xB2 => 0x2033,
        0xB3 => 0x2265,
        0xB4 => 0x00D7,
        0xB5 => 0x221D,
        0xB6 => 0x2202,
        0xB7 => 0x2022,
        0xB8 => 0x00F7,
        0xB9 => 0x2260,
        0xBA => 0x2261,
        0xBB => 0x2248,
        0xBC => 0x2026,
        0xBF => 0x21B5,
        // 0xC0–0xCF: letterlike symbols, set relations.
        0xC0 => 0x2135,
        0xC1 => 0x2111,
        0xC2 => 0x211C,
        0xC3 => 0x2118,
        0xC4 => 0x2297,
        0xC5 => 0x2295,
        0xC6 => 0x2205,
        0xC7 => 0x2229,
        0xC8 => 0x222A,
        0xC9 => 0x2283,
        0xCA => 0x2287,
        0xCB => 0x2284,
        0xCC => 0x2282,
        0xCD => 0x2286,
        0xCE => 0x2208,
        0xCF => 0x2209,
        // 0xD0–0xDF: angle, nabla, product, radical, not, ands, ors, double arrows.
        0xD0 => 0x2220,
        0xD1 => 0x2207,
        0xD5 => 0x220F,
        0xD6 => 0x221A,
        0xD7 => 0x22C5,
        0xD8 => 0x00AC,
        0xD9 => 0x2227,
        0xDA => 0x2228,
        0xDB => 0x21D4,
        0xDC => 0x21D0,
        0xDD => 0x21D1,
        0xDE => 0x21D2,
        0xDF => 0x21D3,
        // 0xE0–0xF5: lozenge, angle brackets, summation, integral.
        0xE0 => 0x25CA,
        0xE1 => 0x2329,
        0xE5 => 0x2211,
        0xF1 => 0x232A,
        0xF2 => 0x222B,
        0xF3 => 0x2320,
        0xF5 => 0x2321,
        _ => return None,
    })
}

/// Apple's Mac OS Symbol (which shares the Adobe layout) diverges from the Adobe table on a few
/// codes: it maps the fraction slash to U+2044 and the angle brackets to U+3008/U+3009 instead of
/// Adobe's U+2215/U+2329/U+232A. `symbol_font_draw` tries these alternatives after the Adobe
/// mapping when the installed face doesn't cover the Adobe character. The additional Adobe aliases
/// for 0x44/0x57/0x6D (U+2206/U+2126/U+00B5) are not Apple alternatives and are deliberately absent.
pub(crate) const fn symbol_unicode_alts(code: u32) -> &'static [u32] {
    match code {
        0xA4 => &[0x2044],
        0xE1 => &[0x3008],
        0xF1 => &[0x3009],
        _ => &[],
    }
}

/// The character the symbol font draws the code `code` at, when the installed `face` is that font:
/// its private-use code U+F000 + code, or, when the face has no private-use mapping (Apple's
/// "Symbol" maps its glyphs to real Unicode), the Adobe Symbol encoding's real character (then its
/// Apple alternative, then, for printable ASCII with no table entry — the `< = > ?` codes among
/// them — the ASCII character itself) when the face covers it.
pub(crate) fn symbol_font_draw(face: &FontFace, code: u32) -> Option<char> {
    let code = code & 0xFF;
    let pua = char::from_u32(0xF000 | code)?;
    if face.covers(pua) {
        return Some(pua);
    }
    // The table below is the Symbol font's encoding; Wingdings, Webdings and the like are not Symbol.
    if !face.family.eq_ignore_ascii_case("symbol") {
        return None;
    }
    if let Some(real) = symbol_unicode(code) {
        return std::iter::once(real).chain(symbol_unicode_alts(code).iter().copied()).filter_map(char::from_u32).find(|c| face.covers(*c));
    }
    // 0x20–0x7E with no table entry draw the ASCII character itself (the Symbol font's glyphs for
    // "< = > ?" etc. sit at their own ASCII codes, not at a remapped Unicode point).
    if (0x20..=0x7E).contains(&code) {
        let ascii = char::from_u32(code)?;
        if face.covers(ascii) {
            return Some(ascii);
        }
    }
    None
}

/// The character to draw bullet `c` with the installed symbol font `face`, when `font` names a
/// symbol font that `face` is (not a substitute) and it has the glyph.
pub(crate) fn symbol_font_char(face: &FontFace, c: &str, font: Option<&str>) -> Option<char> {
    let font = font?;
    if !is_symbol_font(font) || !face.family.eq_ignore_ascii_case(font) {
        return None;
    }
    let first = c.chars().next()?;
    let cp = first as u32;
    // Only symbol-font codes (low byte, or the U+F000 + code private-use form) take the mapping.
    // A bullet whose character is a plain Unicode codepoint (e.g. U+2022 •) draws its own glyph
    // when the face covers it, never a low-byte remap to an unrelated Symbol code.
    if (cp < 0x100 || (0xF000..=0xF0FF).contains(&cp))
        && let Some(c) = symbol_font_draw(face, cp & 0xFF)
    {
        return Some(c);
    }
    face.covers(first).then_some(first)
}

/// The character that draws `c`, a character of text set in the symbol font `font` (Wingdings,
/// Symbol, Webdings…: its code, or U+F000 + its code), with `face`: the installed symbol font's own
/// glyph when `face` is that font, else a Unicode look-alike as bullets use. None when `font` is not
/// a symbol font or `c` is not a symbol code. Metafile text takes this path.
pub fn symbol_char(face: &FontFace, font: &str, c: char) -> Option<char> {
    let code = c as u32;
    if !is_symbol_font(font) || !(code < 0x100 || (0xF000..=0xF0FF).contains(&code)) {
        return None;
    }
    let plain = char::from_u32(code & 0xFF)?;
    let s = plain.to_string();
    Some(symbol_font_char(face, &s, Some(font)).unwrap_or_else(|| map_symbol_bullet(&s, Some(font)).chars().next().unwrap_or('•')))
}

/// The Unicode bullet for a symbol-font bullet character (Wingdings, Symbol), or the character.
pub fn map_symbol_bullet(c: &str, font: Option<&str>) -> String {
    let f = font.unwrap_or("").to_ascii_lowercase();
    let first = c.chars().next().unwrap_or('•');
    if f.starts_with("webdings") {
        let m = match (first as u32) & 0xff {
            0x6e => '●',
            0x67 => '■',
            0x61 => '✓',
            0x72 => '✕',
            0x33 => '◀',
            0x34 => '▶',
            0x35 => '▲',
            0x36 => '▼',
            _ => '•',
        };
        return m.to_string();
    }
    // Only the legacy symbol fonts: "Noto Sans Symbols" and the like are Unicode fonts whose
    // characters (❑ ❖) are drawn as they are. Private-use codes are mapped only when no font is named.
    if f.contains("wingdings") || f == "symbol" || (font.is_none() && ('\u{f000}'..='\u{f0ff}').contains(&first)) {
        let code = (first as u32) & 0xff;
        // Wingdings 2 and 3 put different shapes at the same codes.
        if f.starts_with("wingdings 2") {
            let m = match code {
                0x97 | 0x98 => '●',
                0x9e..=0xa1 => '■',
                0xa2..=0xa5 => '◆',
                0xea..=0xf3 => '★',
                0xb7..=0xbf => '○',
                0x50 => '✓',
                _ => '•',
            };
            return m.to_string();
        }
        if f.starts_with("wingdings 3") {
            let m = match code {
                0x7d | 0x75 => '►',
                0x7c | 0x74 => '◄',
                0x70 => '▲',
                0x71 => '▼',
                0xc6 | 0xc7 => '➔',
                _ => '▶',
            };
            return m.to_string();
        }
        let m = match code {
            0xa7 => '▪',
            0xd8 => '➢',
            0xfc => '✓',
            0x76 => '❖',
            0x71 => '❑',
            0x6e => '■',
            0x6c => '●',
            0xa8 => '◆',
            // The diamonds: black lozenges and diamonds of several sizes.
            0x73..=0x75 | 0x77 => '◆',
            0x6f => '□',
            0xb7 => '•',
            0x2d => '–',
            _ => '•',
        };
        return m.to_string();
    }
    // PowerPoint draws only the first character of a longer `buChar` ("••" draws one dot).
    c.graphemes(true).next().unwrap_or("•").to_string()
}
