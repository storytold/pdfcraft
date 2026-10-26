//! A non-symbolic TrueType font finds its glyphs by the names in a version 2.0 `post` table
//! whose glyphs all take standard Macintosh names, so the table has no string data (vendored
//! hayro-interpret patch). Chrome writes such subsets, with only a (0, 3) `cmap` subtable; their
//! names were dropped and every glyph drew as `.notdef`, a box.
//!
//! The font is built here, in memory: `.notdef` (empty) and a filled square named `A` (standard
//! name 36). Its one `cmap` subtable maps nothing, so the name is the only way to the square.
//! No font file is involved.

use super::render_first_page;

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn out16(out: &mut Vec<u8>, n: i16) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A TrueType font whose square glyph is reachable only through its `post` name `A`.
fn font_named_by_post_table() -> Vec<u8> {
    // Glyph 1: one contour, the square 0..1000 on both axes (glyph 0 is empty).
    let mut glyf = Vec::new();
    for n in [1i16, 0, 0, 1000, 1000] {
        out16(&mut glyf, n);
    }
    be16(&mut glyf, 3); // the contour's last point
    be16(&mut glyf, 0); // no instructions
    glyf.extend([1u8; 4]); // four on-curve points, two-byte coordinates
    for dx in [0i16, 1000, 0, -1000] {
        out16(&mut glyf, dx);
    }
    for dy in [0i16, 0, 1000, 0] {
        out16(&mut glyf, dy);
    }
    let mut loca = Vec::new();
    for offset in [0u16, 0, (glyf.len() / 2) as u16] {
        be16(&mut loca, offset);
    }

    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f_3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&1000i16.to_be_bytes());
    head[42..44].copy_from_slice(&1000i16.to_be_bytes());
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[4..6].copy_from_slice(&1000i16.to_be_bytes());
    hhea[34..36].copy_from_slice(&2u16.to_be_bytes());
    let mut hmtx = Vec::new();
    for _ in 0..2 {
        be16(&mut hmtx, 1000);
        be16(&mut hmtx, 0);
    }
    let mut maxp = vec![0, 1, 0, 0];
    be16(&mut maxp, 2);
    maxp.resize(32, 0);

    // (0, 3), format 4, with only the closing 0xFFFF segment: it maps no character.
    let mut unicode = Vec::new();
    be16(&mut unicode, 4);
    be16(&mut unicode, 24);
    be16(&mut unicode, 0);
    for n in [2u16, 2, 0, 0] {
        be16(&mut unicode, n); // segCountX2, searchRange, entrySelector, rangeShift
    }
    for n in [0xFFFFu16, 0, 0xFFFF, 1, 0] {
        be16(&mut unicode, n); // endCode, pad, startCode, idDelta, idRangeOffset
    }
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 1);
    be16(&mut cmap, 0);
    be16(&mut cmap, 3);
    be32(&mut cmap, 12);
    cmap.extend(unicode);

    // Version 2.0 with standard names only: `.notdef` (0) and `A` (36), and no string data.
    let mut post = vec![0, 2, 0, 0];
    post.resize(32, 0);
    be16(&mut post, 2);
    be16(&mut post, 0);
    be16(&mut post, 36);

    let tables = [
        (*b"cmap", cmap),
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"post", post),
    ];
    let mut out = vec![0, 1, 0, 0];
    be16(&mut out, tables.len() as u16);
    for n in [128u16, 3, 0] {
        be16(&mut out, n);
    }
    let mut payload = Vec::new();
    let mut offset = 12 + 16 * tables.len();
    for (tag, bytes) in tables {
        out.extend(tag);
        let mut padded = bytes.clone();
        padded.resize(bytes.len().div_ceil(4) * 4, 0);
        let sum = padded.as_chunks::<4>().0.iter().fold(0u32, |s, b| s.wrapping_add(u32::from_be_bytes(*b)));
        be32(&mut out, sum);
        be32(&mut out, offset as u32);
        be32(&mut out, bytes.len() as u32);
        offset += padded.len();
        payload.extend(padded);
    }
    out.extend(payload);
    out
}

/// A 40 × 40 pt page that shows `A` at 30 pt from (5, 5) in the font, marked non-symbolic with
/// `/WinAnsiEncoding`, as Chrome writes it.
fn page_with_nonsymbolic_font(font: &[u8]) -> Vec<u8> {
    let content = b"BT /F1 30 Tf 5 5 Td (A) Tj ET".to_vec();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.clone(), b"\nendstream".to_vec()].concat(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Sans /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [1000] /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /AAAAAA+Sans /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", font.len()).into_bytes(), font.to_vec(), b"\nendstream".to_vec()].concat(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n", i + 1).into_bytes());
        pdf.extend(object);
        pdf.extend(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").into_bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).into_bytes());
    pdf
}

#[test]
fn a_nonsymbolic_truetype_font_finds_glyphs_by_standard_post_names() {
    let page = render_first_page(page_with_nonsymbolic_font(&font_named_by_post_table()));
    let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize).copied().unwrap_or(255);
    // The square covers (5, 5)–(35, 35) in page space: (20, 20) is inside it, (2, 2) outside.
    assert_eq!(at(2, 2), 255, "nothing is drawn outside the square");
    assert!(at(20, 20) < 128, "the glyph named `A` in the post table, not .notdef");
}
