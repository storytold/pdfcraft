//! A symbolic TrueType font draws through its (3, 0) `cmap` subtable before its (1, 0) one
//! (vendored hayro-interpret patch, #4156; ISO 32000-2 §9.6.5.4: "If the font contains a (3, 0)
//! subtable" it is used, "Otherwise, if the font contains a (1, 0) subtable"). SAS writes AMT
//! fonts whose (1, 0) table is wrong, so "8" drew as "p" and "+" as "V".
//!
//! The font is built here, in memory: three glyphs (`.notdef`, an empty one and a filled square),
//! a (1, 0) subtable mapping `A` to the empty glyph and a (3, 0) subtable mapping it (as U+F041)
//! to the square. No font file is involved.

use super::render_first_page;

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A TrueType font whose (1, 0) and (3, 0) subtables disagree about the code `A`.
fn disagreeing_font() -> Vec<u8> {
    // Glyph 2: one contour, the square 0..1000 on both axes (glyph 0 and 1 are empty).
    let mut square = Vec::new();
    for n in [1i16, 0, 0, 1000, 1000] {
        out16(&mut square, n);
    }
    be16(&mut square, 3); // the contour's last point
    be16(&mut square, 0); // no instructions
    square.extend([1u8; 4]); // four on-curve points, two-byte coordinates
    for dx in [0i16, 1000, 0, -1000] {
        out16(&mut square, dx);
    }
    for dy in [0i16, 0, 1000, 0] {
        out16(&mut square, dy);
    }
    let glyf = square.clone();
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

    // (1, 0), format 0: `A` → glyph 1 (empty), and `B`, which the (3, 0) table leaves out, →
    // glyph 2.
    let mut mac = Vec::new();
    be16(&mut mac, 0);
    be16(&mut mac, 262);
    be16(&mut mac, 0);
    let mut ids = [0u8; 256];
    ids[usize::from(b'A')] = 1;
    ids[usize::from(b'B')] = 2;
    mac.extend(ids);
    // (3, 0), format 4: U+F041 → glyph 2 (the square).
    let mut win = Vec::new();
    be16(&mut win, 4);
    be16(&mut win, 32);
    be16(&mut win, 0);
    for n in [4u16, 4, 1, 0] {
        be16(&mut win, n); // segCountX2, searchRange, entrySelector, rangeShift
    }
    for n in [0xF041u16, 0xFFFF, 0, 0xF041, 0xFFFF, 2u16.wrapping_sub(0xF041), 1, 0, 0] {
        be16(&mut win, n); // endCode ×2, pad, startCode ×2, idDelta ×2, idRangeOffset ×2
    }
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 2);
    for (platform, encoding, offset) in [(1u16, 0u16, 20u32), (3, 0, 20 + mac.len() as u32)] {
        be16(&mut cmap, platform);
        be16(&mut cmap, encoding);
        be32(&mut cmap, offset);
    }
    cmap.extend(mac);
    cmap.extend(win);

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

fn out16(out: &mut Vec<u8>, n: i16) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A 40 × 40 pt page that shows `text` at 30 pt from (5, 5) in the font, marked symbolic and
/// with no `/Encoding`, as SAS writes it.
fn page_with_symbolic_font(font: &[u8], text: &str) -> Vec<u8> {
    let content = format!("BT /F1 30 Tf 5 5 Td ({text}) Tj ET").into_bytes();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.clone(), b"\nendstream".to_vec()].concat(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Sym /FirstChar 65 /LastChar 66 /Widths [1000 1000] /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /AAAAAA+Sym /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
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
fn a_symbolic_truetype_font_draws_through_its_3_0_subtable_first() {
    let font = disagreeing_font();
    // The square covers (5, 5)–(35, 35) in page space: row 20 from the top is inside it.
    let square_drawn = |text: &str| {
        let page = render_first_page(page_with_symbolic_font(&font, text));
        let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize).copied().unwrap_or(255);
        assert_eq!(at(2, 2), 255, "nothing is drawn outside the square");
        at(20, 20) < 128
    };
    assert!(square_drawn("A"), "the (3, 0) subtable's square, not the (1, 0) subtable's empty glyph");
    assert!(square_drawn("B"), "a code the (3, 0) subtable leaves out still reads through (1, 0)");
}
