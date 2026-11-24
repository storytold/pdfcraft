//! The standard Symbol font: its built-in encoding and advance widths.
//!
//! Source: the character codes and glyph advance widths ("C" and "WX" values) of Adobe's Core 14
//! AFM metrics file for Symbol (`Symbol.afm`, version 001.008), and the Unicode values of its glyph
//! names from Unicode, Inc.'s `symbol.txt` ("Adobe Symbol Encoding to Unicode", table version
//! 1.0). Adobe distributes the Core 14 metrics with permission to use, copy and distribute them
//! for any purpose (the notice is quoted in `metrics.rs`); Unicode, Inc. grants the right to use
//! the information in its file in products supporting the Unicode Standard (its notice is
//! recorded in `ATTRIBUTION.md`). Only the numbers are reproduced here, typed in from the public
//! tables; neither file is included. The stretchable
//! bracket, brace and integral pieces (codes 0xE6 to 0xEF and 0xF3 to 0xFE) and the sans-serif
//! duplicates of the registered, copyright and trademark signs are left out of the Unicode map:
//! they have no Unicode counterpart of their own.

/// The Symbol-encoding code for `c`, or `None` when Symbol has no such glyph.
pub fn symbol_code(c: char) -> Option<u8> {
    let u = c as u32;
    // Characters Symbol encodes at their ASCII code.
    if matches!(
        c,
        ' ' | '!' | '#' | '%' | '&' | '(' | ')' | '+' | ',' | '.' | '/' | '0'
            ..='9' | ':' | ';' | '<' | '=' | '>' | '?' | '[' | ']' | '_' | '{' | '|' | '}'
    ) {
        return Some(u as u8);
    }
    Some(match c {
        '\u{2200}' => 0x22, // for all
        '\u{2203}' => 0x24, // there exists
        '\u{220B}' => 0x27, // such that (contains as member)
        '\u{2217}' | '*' => 0x2A,
        '\u{2212}' | '-' => 0x2D, // minus
        '\u{2245}' => 0x40,       // approximately equal (congruent)
        '\u{0391}' => 0x41,
        '\u{0392}' => 0x42,
        '\u{03A7}' => 0x43,
        '\u{0394}' | '\u{2206}' => 0x44,
        '\u{0395}' => 0x45,
        '\u{03A6}' => 0x46,
        '\u{0393}' => 0x47,
        '\u{0397}' => 0x48,
        '\u{0399}' => 0x49,
        '\u{03D1}' => 0x4A, // theta1
        '\u{039A}' => 0x4B,
        '\u{039B}' => 0x4C,
        '\u{039C}' => 0x4D,
        '\u{039D}' => 0x4E,
        '\u{039F}' => 0x4F,
        '\u{03A0}' => 0x50,
        '\u{0398}' => 0x51,
        '\u{03A1}' => 0x52,
        '\u{03A3}' => 0x53,
        '\u{03A4}' => 0x54,
        '\u{03A5}' => 0x55,
        '\u{03C2}' => 0x56, // final sigma (sigma1)
        '\u{03A9}' | '\u{2126}' => 0x57,
        '\u{039E}' => 0x58,
        '\u{03A8}' => 0x59,
        '\u{0396}' => 0x5A,
        '\u{2234}' => 0x5C, // therefore
        '\u{22A5}' => 0x5E, // perpendicular
        '\u{03B1}' => 0x61,
        '\u{03B2}' => 0x62,
        '\u{03C7}' => 0x63,
        '\u{03B4}' => 0x64,
        '\u{03B5}' | '\u{03F5}' => 0x65,
        '\u{03C6}' => 0x66,
        '\u{03B3}' => 0x67,
        '\u{03B7}' => 0x68,
        '\u{03B9}' => 0x69,
        '\u{03D5}' => 0x6A, // phi1
        '\u{03BA}' => 0x6B,
        '\u{03BB}' => 0x6C,
        '\u{03BC}' | '\u{00B5}' => 0x6D,
        '\u{03BD}' => 0x6E,
        '\u{03BF}' => 0x6F,
        '\u{03C0}' => 0x70,
        '\u{03B8}' => 0x71,
        '\u{03C1}' => 0x72,
        '\u{03C3}' => 0x73,
        '\u{03C4}' => 0x74,
        '\u{03C5}' => 0x75,
        '\u{03D6}' => 0x76, // omega1 (pi symbol)
        '\u{03C9}' => 0x77,
        '\u{03BE}' => 0x78,
        '\u{03C8}' => 0x79,
        '\u{03B6}' => 0x7A,
        '\u{223C}' | '~' => 0x7E, // similar
        '\u{20AC}' => 0xA0,       // euro
        '\u{03D2}' => 0xA1,       // Upsilon1
        '\u{2032}' => 0xA2,       // minute (prime)
        '\u{2264}' => 0xA3,
        '\u{2044}' => 0xA4, // fraction slash
        '\u{221E}' => 0xA5,
        '\u{0192}' => 0xA6,
        '\u{2663}' => 0xA7,
        '\u{2666}' => 0xA8,
        '\u{2665}' => 0xA9,
        '\u{2660}' => 0xAA,
        '\u{2194}' => 0xAB,
        '\u{2190}' => 0xAC,
        '\u{2191}' => 0xAD,
        '\u{2192}' => 0xAE,
        '\u{2193}' => 0xAF,
        '\u{00B0}' => 0xB0,
        '\u{00B1}' => 0xB1,
        '\u{2033}' => 0xB2, // second (double prime)
        '\u{2265}' => 0xB3,
        '\u{00D7}' => 0xB4,
        '\u{221D}' => 0xB5,
        '\u{2202}' => 0xB6,
        '\u{2022}' | '\u{2219}' => 0xB7,
        '\u{00F7}' => 0xB8,
        '\u{2260}' => 0xB9,
        '\u{2261}' => 0xBA,
        '\u{2248}' => 0xBB,
        '\u{2026}' => 0xBC,
        '\u{21B5}' => 0xBF,
        '\u{2135}' => 0xC0,
        '\u{2111}' => 0xC1,
        '\u{211C}' => 0xC2,
        '\u{2118}' => 0xC3,
        '\u{2297}' => 0xC4,
        '\u{2295}' => 0xC5,
        '\u{2205}' => 0xC6,
        '\u{2229}' => 0xC7,
        '\u{222A}' => 0xC8,
        '\u{2283}' => 0xC9,
        '\u{2287}' => 0xCA,
        '\u{2284}' => 0xCB,
        '\u{2282}' => 0xCC,
        '\u{2286}' => 0xCD,
        '\u{2208}' => 0xCE,
        '\u{2209}' => 0xCF,
        '\u{2220}' => 0xD0,
        '\u{2207}' => 0xD1,
        '\u{00AE}' => 0xD2,
        '\u{00A9}' => 0xD3,
        '\u{2122}' => 0xD4,
        '\u{220F}' => 0xD5,
        '\u{221A}' => 0xD6,
        '\u{22C5}' | '\u{00B7}' => 0xD7, // dot operator
        '\u{00AC}' => 0xD8,
        '\u{2227}' => 0xD9,
        '\u{2228}' => 0xDA,
        '\u{21D4}' => 0xDB,
        '\u{21D0}' => 0xDC,
        '\u{21D1}' => 0xDD,
        '\u{21D2}' => 0xDE,
        '\u{21D3}' => 0xDF,
        '\u{25CA}' => 0xE0,
        '\u{2329}' | '\u{27E8}' => 0xE1, // angle bracket left
        '\u{2211}' => 0xE5,
        '\u{232A}' | '\u{27E9}' => 0xF1, // angle bracket right
        '\u{222B}' => 0xF2,
        _ => return None,
    })
}

/// Advance widths in 1/1000 em for codes 0x20..=0xFF (0 where Symbol has no glyph).
const WIDTHS: [u16; 224] = [
    // 0x20
    250, 333, 713, 500, 549, 833, 778, 439, 333, 333, 500, 549, 250, 549, 250, 278, //
    // 0x30
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 549, 549, 549, 444, //
    // 0x40
    549, 722, 667, 722, 612, 611, 763, 603, 722, 333, 631, 722, 686, 889, 722, 722, //
    // 0x50
    768, 741, 556, 592, 611, 690, 439, 768, 645, 795, 611, 333, 863, 333, 658, 500, //
    // 0x60
    500, 631, 549, 549, 494, 439, 521, 411, 603, 329, 603, 549, 549, 576, 521, 549, //
    // 0x70
    549, 521, 549, 603, 439, 576, 713, 686, 493, 686, 494, 480, 200, 480, 549, 0, //
    // 0x80
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    // 0x90
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    // 0xA0
    750, 620, 247, 549, 167, 713, 500, 753, 753, 753, 753, 1042, 987, 603, 987, 603, //
    // 0xB0
    400, 549, 411, 549, 549, 713, 494, 460, 549, 549, 549, 549, 1000, 603, 1000, 658, //
    // 0xC0
    823, 686, 795, 987, 768, 768, 823, 768, 768, 713, 713, 713, 713, 713, 713, 713, //
    // 0xD0
    768, 713, 790, 790, 890, 823, 549, 250, 713, 603, 603, 1042, 987, 603, 987, 603, //
    // 0xE0
    494, 329, 790, 790, 786, 713, 384, 384, 384, 384, 384, 384, 494, 494, 494, 494, //
    // 0xF0
    0, 329, 274, 686, 686, 686, 384, 384, 384, 384, 384, 384, 494, 494, 494, 0, //
];

/// The advance width of code `code` in 1/1000 em, or `None` for an empty slot.
pub fn symbol_width(code: u8) -> Option<u16> {
    let w = *WIDTHS.get(usize::from(code).checked_sub(0x20)?)?;
    (w > 0).then_some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maths_symbols_are_encoded_with_their_widths() {
        for (c, code, w) in [
            ('α', 0x61, 631),
            ('π', 0x70, 549),
            ('Σ', 0x53, 592),
            ('∑', 0xE5, 713),
            ('∫', 0xF2, 274),
            ('√', 0xD6, 549),
            ('≤', 0xA3, 549),
            ('≥', 0xB3, 549),
            ('±', 0xB1, 549),
            ('×', 0xB4, 549),
            ('÷', 0xB8, 549),
            ('∞', 0xA5, 713),
            ('∂', 0xB6, 494),
            ('−', 0x2D, 549),
            ('=', 0x3D, 549),
            ('2', 0x32, 500),
        ] {
            assert_eq!(symbol_code(c), Some(code), "{c}");
            assert_eq!(symbol_width(code), Some(w), "{c}");
        }
        assert_eq!(symbol_code('A'), None, "Latin letters belong to the text faces");
        assert_eq!(symbol_code('ℝ'), None);
        assert_eq!(symbol_width(0x7F), None);
        assert_eq!(symbol_width(0x10), None);
        assert_eq!(symbol_width(0xFF), None);
    }

    #[test]
    fn every_encoded_character_has_a_width() {
        for u in 0..0x3000u32 {
            if let Some(code) = char::from_u32(u).and_then(symbol_code) {
                assert!(symbol_width(code).is_some(), "U+{u:04X} -> {code:#x}");
            }
        }
    }
}
