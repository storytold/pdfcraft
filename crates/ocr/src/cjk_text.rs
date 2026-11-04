//! The text layer for a page whose text is Chinese.
//!
//! The layer is invisible text (rendering mode 3) over each recognised line, so it needs no
//! glyph outlines: the characters are written as codes in a CID font that carries a `/ToUnicode`
//! map, which is what a reader uses to extract the text. The font's widths come from the same
//! measure the text matrix is scaled by, so the text lands exactly on the words it covers.
//!
//! The text layer of a Latin page keeps using the standard fonts: see [`crate::text_layer`].

use crate::PlacedWord;
use pdfcraft_fonts::cjk::CjkFont;

/// The ascent and descent of a Chinese glyph, in em fractions: its ink fills the em square.
const ASCENT: f64 = 0.88;
const DESCENT: f64 = 0.12;

/// Page content that writes `words` as invisible text, and the CID font it needs: each
/// character, the code it is written as, and its advance.
pub fn text_layer(words: &[PlacedWord]) -> (Vec<u8>, CjkFont) {
    // One code per character, in the order the characters first appear.
    let mut chars: Vec<(char, f64)> = Vec::new();
    for word in words {
        for ch in word.text.chars() {
            if chars.iter().any(|(c, _)| *c == ch) {
                continue;
            }
            chars.push((ch, pdfcraft_fonts::cjk_width(&ch.to_string(), 1.0)));
        }
    }
    let font = CjkFont::new(chars);

    let mut out = format!("/OCR BMC\nq\nBT\n3 Tr\n/{} 1 Tf\n", pdfcraft_fonts::cjk::RESOURCE).into_bytes();
    for word in words {
        let width = pdfcraft_fonts::cjk_width(&word.text, 1.0);
        let height = word.up[0].hypot(word.up[1]);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        // The em square: the word's ink spans the box, so the text is as tall as the scanned
        // word; its width is stretched to fit the word.
        let em = [word.up[0] / (ASCENT - DESCENT), word.up[1] / (ASCENT - DESCENT)];
        let ax = [word.across[0] / width, word.across[1] / width];
        let o = [word.origin[0] - em[0] * DESCENT, word.origin[1] - em[1] * DESCENT];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
        let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
        // The layer is invisible, so a reader that follows the marked content reads the word
        // from its actual text rather than from the glyphs it cannot see.
        out.extend_from_slice(b"/Span << /ActualText <");
        out.extend_from_slice(&actual_text(&word.text));
        out.extend_from_slice(b"> >> BDC\n");
        out.extend_from_slice(nums.join(" ").as_bytes());
        out.extend_from_slice(b" Tm ");
        out.extend_from_slice(b"<");
        for ch in word.text.chars() {
            let code = font.code_of(ch).unwrap_or(0);
            out.extend_from_slice(format!("{code:04X}").as_bytes());
        }
        out.extend_from_slice(b"> Tj\n");
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    (out, font)
}

/// The actual text of a word, as the hex string of its UTF-16BE code units: a PDF text
/// string holds UTF-16BE with a byte-order mark, so a reader decodes the Chinese characters.
fn actual_text(text: &str) -> Vec<u8> {
    let mut out = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        hex.push_str(&format!("{b:02X}"));
    }
    hex.into_bytes()
}

/// A number for a text matrix: no exponent, and no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{:.4}", if v.abs() < 1e-9 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}
