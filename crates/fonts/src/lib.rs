//! pdfcraft-fonts — font metrics and encodings for generated appearances (L2).
//!
//! See the README: the metrics are approximations by character class (no vendor metrics files
//! are bundled). The full font subsystem lands in M2.2/M7.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod craft;
mod encodings;
pub mod pdf;
mod script;
pub use craft::{
    CRAFT_FONTS, CraftFont, SHIPPORI_MINCHO, document_japanese_font, ui_arabic_fonts, ui_chinese_fonts, ui_cjk_fonts, ui_japanese_fonts,
    ui_telugu_fonts,
};
pub use script::{GlyphError, GlyphOutline, MAX_SIGNATURE_CHARS, ScriptOutline, japanese_glyph, script_outline};

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

/// Encode text in WinAnsiEncoding (ISO 32000-2 Annex D); unmappable characters become `?`.
pub fn win_ansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{20}'..='\u{7e}' => c as u8,
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            '€' => 0x80,
            '‚' => 0x82,
            '„' => 0x84,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '™' => 0x99,
            '\t' => b' ',
            _ => b'?',
        })
        .collect()
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
}
