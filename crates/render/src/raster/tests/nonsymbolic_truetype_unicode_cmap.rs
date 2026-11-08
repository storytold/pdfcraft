//! A non-symbolic TrueType font whose `cmap` has only a Unicode-platform (0, x) subtable draws
//! its glyphs (vendored hayro-interpret patch, #6581). Such a font's codes are read as glyph
//! names, then as Unicode, and the Unicode was looked up only in a Windows (3, 1) subtable and,
//! by Mac Roman, a (1, 0) one, before the `post` glyph names, so every code drew `.notdef`. The
//! file in #6581 embeds a font whose only subtable is (0, 1), and its text came out as boxes.
//! The (0, x) subtable is read last, so a code those routes already draw keeps its glyph.
//!
//! The font is built here, in memory: three glyphs (`.notdef`, an empty one and a filled square)
//! and `cmap` subtables mapping `A` to one of them, under the platform and encoding each case
//! names. It has no `post` table, so glyph names give no other route. No font file is involved.

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

/// A format 4 subtable mapping `from` to glyph `to`.
fn format4(from: u16, to: u16) -> Vec<u8> {
    let mut sub = Vec::new();
    be16(&mut sub, 4);
    be16(&mut sub, 32);
    be16(&mut sub, 0);
    for n in [4u16, 4, 1, 0] {
        be16(&mut sub, n); // segCountX2, searchRange, entrySelector, rangeShift
    }
    for n in [from, 0xFFFF, 0, from, 0xFFFF, to.wrapping_sub(from), 1, 0, 0] {
        be16(&mut sub, n); // endCode ×2, pad, startCode ×2, idDelta ×2, idRangeOffset ×2
    }
    sub
}

/// A TrueType font whose only `cmap` subtables are `records`: (platform, encoding, subtable).
fn font_with_cmap(records: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    // Glyph 2: one contour, the square 0..1000 on both axes (glyph 0 and 1 are empty).
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
    for offset in [0u16, 0, 0, (glyf.len() / 2) as u16] {
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
    hhea[34..36].copy_from_slice(&3u16.to_be_bytes());
    let mut hmtx = Vec::new();
    for _ in 0..3 {
        be16(&mut hmtx, 1000);
        be16(&mut hmtx, 0);
    }
    let mut maxp = vec![0, 1, 0, 0];
    be16(&mut maxp, 3);
    maxp.resize(32, 0);

    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, records.len() as u16);
    let mut offset = 4 + 8 * records.len() as u32;
    for (platform, encoding, sub) in records {
        be16(&mut cmap, *platform);
        be16(&mut cmap, *encoding);
        be32(&mut cmap, offset);
        offset += sub.len() as u32;
    }
    for (_, _, sub) in records {
        cmap.extend(sub);
    }

    let tables = [(*b"cmap", cmap), (*b"glyf", glyf), (*b"head", head), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"loca", loca), (*b"maxp", maxp)];
    let mut out = vec![0, 1, 0, 0];
    be16(&mut out, tables.len() as u16);
    for n in [64u16, 2, 48] {
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
/// `/WinAnsiEncoding`, as the file in #6581 has it.
fn page_with_nonsymbolic_font(font: &[u8]) -> Vec<u8> {
    let content = b"BT /F1 30 Tf 5 5 Td (A) Tj ET".to_vec();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.clone(), b"\nendstream".to_vec()].concat(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Plain /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 65 /Widths [1000] /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /AAAAAA+Plain /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
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

/// Whether `A` drew the square: it covers (5, 5)–(35, 35) in page space, so (20, 20) is inside.
fn square_drawn(records: &[(u16, u16, Vec<u8>)]) -> bool {
    let page = render_first_page(page_with_nonsymbolic_font(&font_with_cmap(records)));
    let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize).copied().unwrap_or(255);
    assert_eq!(at(2, 2), 255, "nothing is drawn outside the square");
    at(20, 20) < 128
}

#[test]
fn a_nonsymbolic_truetype_font_reads_its_unicode_platform_cmap() {
    for encoding in [0u16, 1, 3, 4] {
        assert!(square_drawn(&[(0, encoding, format4(0x41, 2))]), "a lone (0, {encoding}) subtable maps U+0041 to the square");
    }
}

/// A format 0 (Mac Roman) subtable mapping byte `from` to glyph `to`.
fn format0(from: u8, to: u8) -> Vec<u8> {
    let mut sub = Vec::new();
    be16(&mut sub, 0);
    be16(&mut sub, 262);
    be16(&mut sub, 0);
    let mut ids = [0u8; 256];
    ids[usize::from(from)] = to;
    sub.extend(ids);
    sub
}

#[test]
fn a_nonsymbolic_truetype_font_keeps_its_1_0_cmap_over_a_unicode_one() {
    // No (3, 1): (0, 3) sends `A` to the empty glyph, (1, 0) to the square. The (1, 0) lookup
    // drew this font before the (0, x) one existed, so it still decides.
    assert!(square_drawn(&[(0, 3, format4(0x41, 1)), (1, 0, format0(b'A', 2))]));
}

#[test]
fn a_nonsymbolic_truetype_font_still_prefers_its_3_1_cmap() {
    // (0, 3) sends `A` to the empty glyph, (3, 1) to the square; the Windows table wins (ISO
    // 32000-2 §9.6.5.4), whatever order the records are listed in.
    assert!(square_drawn(&[(0, 3, format4(0x41, 1)), (3, 1, format4(0x41, 2))]));
}

#[test]
fn a_nonsymbolic_truetype_font_skips_a_0_5_subtable() {
    // (0, 5) holds variation sequences, so it is never read as a character map, even when its
    // record points at a format 4 table that maps `A` to the square.
    assert!(!square_drawn(&[(0, 5, format4(0x41, 2))]));
    // Skipping it does not end the scan: a later (0, 3) still maps `A`, and the (0, 5) one, which
    // sends `A` to the empty glyph, is not read first.
    assert!(square_drawn(&[(0, 5, format4(0x41, 1)), (0, 3, format4(0x41, 2))]));
}
