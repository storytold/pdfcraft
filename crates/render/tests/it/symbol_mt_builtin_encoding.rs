//! A non-embedded font named `SymbolMT` (the name Symbol has as a TrueType font) or a style
//! variant such as `Symbol,Bold` is the standard Symbol font, and a non-embedded Symbol draws
//! through its built-in encoding (ISO 32000-2 §9.6.2.2), even when the font dictionary
//! declares `/WinAnsiEncoding` and the nonsymbolic flag: code 0x6D (`m`) is `mu`, as for
//! `/Symbol` itself (hayro-interpret patch (98)).

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

/// A page showing "abmp" at 40 pt in a non-embedded font `font` (the inside of its dictionary).
fn page(font: &str) -> Vec<u8> {
    let body = "BT /F1 40 Tf 10 30 Td (abmp) Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        format!("<< /Type /Font {font} >>").into_bytes(),
    ];

    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn pixels(bytes: Vec<u8>) -> Vec<u8> {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let image = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(image.error.is_none(), "{:?}", image.error);
    image.rgba.to_vec()
}

/// A TrueType `name` with WinAnsiEncoding, nonsymbolic flags and no font program, as written
/// by spreadsheet exports.
fn win_ansi_true_type(name: &str) -> String {
    format!(
        "/Subtype /TrueType /BaseFont /{name} /Encoding /WinAnsiEncoding /FontDescriptor << /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [0 -216 1113 693] /ItalicAngle 0 /Ascent 1005 /Descent -216 /CapHeight 693 /StemV 60 >>"
    )
}

#[test]
fn symbol_mt_draws_symbols_through_the_built_in_encoding() {
    let symbol = pixels(page("/Subtype /Type1 /BaseFont /Symbol"));
    let latin = pixels(page("/Subtype /Type1 /BaseFont /Helvetica"));
    assert!(symbol != latin, "the Symbol stand-in drew the Latin letters");
    for name in ["SymbolMT", "ABCDEF+SymbolMT", "SymbolMT,Bold", "Symbol,Italic", "Symbol-Bold"] {
        assert!(pixels(page(&win_ansi_true_type(name))) == symbol, "{name} did not draw as Symbol");
    }
    let type1 = "/Subtype /Type1 /BaseFont /SymbolMT /Encoding /WinAnsiEncoding";
    assert!(pixels(page(type1)) == symbol, "a Type1 SymbolMT did not draw as Symbol");
}

/// Only the Symbol name itself counts: a font whose name merely starts with it keeps its
/// declared encoding and stand-in.
#[test]
fn a_name_that_only_starts_with_symbol_is_not_symbol() {
    let symbol = pixels(page("/Subtype /Type1 /BaseFont /Symbol"));
    for name in ["Symbolic", "SymbolPS"] {
        assert!(pixels(page(&win_ansi_true_type(name))) != symbol, "{name} drew as Symbol");
    }
}
