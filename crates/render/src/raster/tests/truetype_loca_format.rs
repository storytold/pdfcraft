//! A TrueType font whose `head.indexToLocFormat` holds neither 0 nor 1 draws through the `loca`
//! format its table is in (vendored hayro-interpret patch 79). The OpenType `head` table allows
//! only 0 (short offsets) and 1 (long); the glyph reader took anything but 1 as short, so a font
//! whose field reads 256 (pdf.js `issue2537r.pdf`) had its long offsets read as short ones, and
//! "LINE UP" drew as boxes. ISO 32000-2 §9.9 has an embedded TrueType program drawn as the
//! font program it is.
//!
//! The font is built here, in memory: two glyphs (`.notdef`, empty, and a filled square) and a
//! (1, 0) `cmap` subtable mapping `A` to the square. No font file is involved.

use super::render_first_page;

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// The font, its `loca` offsets long when `long`, its `head.indexToLocFormat` set to `format`.
fn square_font(long: bool, format: u16) -> Vec<u8> {
    // Glyph 1: one contour, the square 0..1000 on both axes (glyph 0 is empty).
    let mut glyf = Vec::new();
    for n in [1u16, 0, 0, 1000, 1000] {
        be16(&mut glyf, n);
    }
    be16(&mut glyf, 3); // the contour's last point
    be16(&mut glyf, 0); // no instructions
    glyf.extend([1u8; 4]); // four on-curve points, two-byte coordinates
    for dx in [0i16, 1000, 0, -1000] {
        glyf.extend_from_slice(&dx.to_be_bytes());
    }
    for dy in [0i16, 0, 1000, 0] {
        glyf.extend_from_slice(&dy.to_be_bytes());
    }
    let mut loca = Vec::new();
    for offset in [0u32, 0, glyf.len() as u32] {
        if long { be32(&mut loca, offset) } else { be16(&mut loca, (offset / 2) as u16) }
    }

    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f_3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&1000i16.to_be_bytes());
    head[42..44].copy_from_slice(&1000i16.to_be_bytes());
    head[50..52].copy_from_slice(&format.to_be_bytes());
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

    // (1, 0), format 0: `A` → glyph 1 (the square).
    let mut mac = Vec::new();
    be16(&mut mac, 0);
    be16(&mut mac, 262);
    be16(&mut mac, 0);
    let mut ids = [0u8; 256];
    ids[usize::from(b'A')] = 1;
    mac.extend(ids);
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 1);
    be16(&mut cmap, 1);
    be16(&mut cmap, 0);
    be32(&mut cmap, 12);
    cmap.extend(mac);

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

/// A 40 × 40 pt page that shows `A` at 30 pt from (5, 5) in the font.
fn page_with_font(font: &[u8]) -> Vec<u8> {
    let content = b"BT /F1 30 Tf 5 5 Td (A) Tj ET".to_vec();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.clone(), b"\nendstream".to_vec()].concat(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Sq /FirstChar 65 /LastChar 65 /Widths [1000] /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /AAAAAA+Sq /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
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
fn a_truetype_font_with_an_out_of_range_loca_format_draws_its_glyphs() {
    // The square covers (5, 5)–(35, 35) in page space: row 20 from the top is inside it.
    let square_drawn = |long: bool, format: u16| {
        let page = render_first_page(page_with_font(&square_font(long, format)));
        let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize).copied().unwrap_or(255);
        assert_eq!(at(2, 2), 255, "nothing is drawn outside the square");
        at(20, 20) < 128
    };
    assert!(square_drawn(true, 1), "long offsets, marked long");
    assert!(square_drawn(false, 0), "short offsets, marked short");
    assert!(square_drawn(true, 0x0100), "long offsets marked 256: the table's length says long");
    assert!(square_drawn(false, 0x0100), "short offsets marked 256: the table's length says short");
}
