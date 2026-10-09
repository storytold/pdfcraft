//! pdfcraft-fonts — font metrics and encodings for generated appearances (L2).
//!
//! See the README: the metrics are approximations by character class (no vendor metrics files
//! are bundled). The full font subsystem lands in M2.2/M7.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod arabic;
mod craft;
mod encodings;
pub mod pdf;
mod script;
pub use arabic::{ShapedCluster, arabic_glyph, arabic_has, shape_arabic};
pub use craft::{
    CRAFT_FONTS, CraftFont, SHIPPORI_MINCHO, document_arabic_font, document_japanese_font, document_japanese_font_for_style,
    document_japanese_fonts_for_style, ui_arabic_fonts, ui_chinese_fonts, ui_cjk_fonts, ui_japanese_fonts, ui_telugu_fonts,
};
pub use script::{GlyphError, GlyphOutline, MAX_SIGNATURE_CHARS, ScriptOutline, japanese_glyph, japanese_glyph_from, script_outline};

/// Approximate advance of `s` in Helvetica (or Arial) at `size` points.
pub fn helvetica_width(s: &str, size: f64) -> f64 {
    let units: f64 = s
        .chars()
        .map(|c| match c {
            ' ' | 'i' | 'j' | 'l' | '\'' | '!' | '|' | '.' | ',' | ':' | ';' | 'I' => 260.0,
            'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '/' | '-' | '"' => 333.0,
            'm' => 833.0,
            'w' => 722.0,
            'M' => 833.0,
            'W' => 944.0,
            'J' | 'c' | 'k' | 's' | 'v' | 'x' | 'y' | 'z' => 500.0,
            '0'..='9' | 'a'..='z' | '$' | '#' | '?' | '_' => 556.0,
            'A'..='Z' => 680.0,
            '@' => 1015.0,
            _ if c.is_whitespace() => 260.0,
            _ => 584.0,
        })
        .sum();
    units * size / 1000.0
}

/// Greedy line breaking within `width` points (paragraphs split on newlines; words longer
/// than a line are broken by character).
pub fn wrap(text: &str, size: f64, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split(['\n', '\r']) {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if helvetica_width(&candidate, size) <= width || line.is_empty() && helvetica_width(word, size) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for ch in word.chars() {
                if !line.is_empty() && helvetica_width(&format!("{line}{ch}"), size) > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        lines.push(line);
    }
    lines
}

/// The WinAnsiEncoding (ISO 32000-2 Annex D) code for `c`, if the encoding has one.
///
/// Codes 0x80–0x9F come from the same table the text extractor decodes with, so encoding and
/// decoding agree (Š š Ž ž Œ œ Ÿ ƒ † ‡ ˆ ˜ ‰ ‹ › as well as quotes, dashes, € and ™).
pub fn win_ansi_byte(c: char) -> Option<u8> {
    match c {
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).ok(),
        _ => {
            let high = encodings::WIN_ANSI.get(0x80..0xa0)?;
            let i = high.iter().position(|&u| u != 0 && u == u32::from(c))?;
            u8::try_from(0x80 + i).ok()
        }
    }
}

/// Encode text in WinAnsiEncoding (ISO 32000-2 Annex D); tabs become spaces and unmappable
/// characters become `?`.
pub fn win_ansi(s: &str) -> Vec<u8> {
    s.chars().map(|c| if c == '\t' { b' ' } else { win_ansi_byte(c).unwrap_or(b'?') }).collect()
}

/// Bytes as a PDF literal string, `(` … `)`, with delimiters escaped.
pub fn literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'(');
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            _ => out.push(b),
        }
    }
    out.push(b')');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_wrap_and_encode() {
        assert!(helvetica_width("MMMM", 10.0) > helvetica_width("iiii", 10.0) * 2.0);
        assert_eq!(helvetica_width("", 12.0), 0.0);
        let lines = wrap("the quick brown fox jumps over the lazy dog", 12.0, 80.0);
        assert!(lines.len() > 2 && lines.iter().all(|l| helvetica_width(l, 12.0) <= 80.0));
        assert_eq!(wrap("a\nb", 12.0, 100.0), ["a", "b"]);
        let long = wrap("Supercalifragilisticexpialidocious", 12.0, 40.0);
        assert!(long.len() > 3 && long.concat() == "Supercalifragilisticexpialidocious");
        assert_eq!(win_ansi("Café — 5€ ☃"), b"Caf\xe9 \x97 5\x80 ?");
        assert_eq!(literal(b"a(b)\\c"), b"(a\\(b\\)\\\\c)");
    }

    #[test]
    fn win_ansi_covers_the_0x80_to_0x9f_glyphs() {
        // Every character in the WinAnsiEncoding table encodes to its code and back (#347).
        for (code, &u) in encodings::WIN_ANSI.iter().enumerate() {
            let Some(c) = char::from_u32(u).filter(|_| u != 0) else { continue };
            assert_eq!(win_ansi_byte(c), Some(code as u8), "{c:?} (U+{u:04X})");
            assert_eq!(win_ansi(&c.to_string()), [code as u8]);
        }
        assert_eq!(
            win_ansi("Šta? žaba, œuvre — „ok“ ‰ ƒ † ‡ ˆ ˜ ‹›Ÿ"),
            b"\x8ata? \x9eaba, \x9cuvre \x97 \x84ok\x93 \x89 \x83 \x86 \x87 \x88 \x98 \x8b\x9b\x9f"
        );
        // Not in WinAnsi: č ć still need the fallback font; undefined codes are never produced.
        assert_eq!(win_ansi("čć\u{81}\u{8d}\t"), b"???? ");
        assert_eq!(win_ansi_byte('\0'), None);
    }
}
