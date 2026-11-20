//! An octal escape above `\377` in a shown string draws and extracts its low eight bits
//! (vendored hayro-syntax patch 54; ISO 32000-2 §7.3.4.2, Table 3: "high-order overflow shall be
//! ignored"). PDFBOX-142's form buttons write `(Abschlie\737en)`, `(L\766schen)` and
//! `(Pr\774fsumme)`; the escapes were dropped, so they read "Abschlieen", "Lschen" and
//! "Prfsumme". In WinAnsi 0o737 & 0xFF is ß, 0o766 & 0xFF ö and 0o774 & 0xFF ü.

use super::*;

/// One page showing `string` (a literal string as written in the content stream) in
/// WinAnsi-encoded Helvetica at 24 pt.
fn page(string: &str) -> Vec<u8> {
    let content = format!("BT /F1 24 Tf 10 20 Td {string} Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 240 60] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", i + 1).into_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").into_bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).into_bytes());
    pdf
}

fn extracted(string: &str) -> String {
    let mut r = PageRenderer::new(Arc::new(page(string)), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    out.text.map(|t| t.plain_text()).unwrap_or_default()
}

#[test]
fn an_octal_escape_past_0o377_extracts_its_low_eight_bits() {
    assert_eq!(extracted(r"(Abschlie\737en)").trim(), "Abschließen");
    assert_eq!(extracted(r"(L\766schen)").trim(), "Löschen");
    assert_eq!(extracted(r"(Pr\774fsumme)").trim(), "Prüfsumme");
    assert_eq!(extracted(r"(Abschlie\337en)").trim(), "Abschließen", "control: the escape in range");
}

#[test]
fn an_octal_escape_past_0o377_draws_its_low_eight_bits() {
    let overflowing = render_first_page(page(r"(Abschlie\737en)"));
    let in_range = render_first_page(page(r"(Abschlie\337en)"));
    let dropped = render_first_page(page("(Abschlieen)"));
    assert!(overflowing.rgba == in_range.rgba, "\\737 draws the ß that \\337 draws");
    assert!(in_range.rgba != dropped.rgba, "control: the ß draws ink");
}
