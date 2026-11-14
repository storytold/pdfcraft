//! A TrueType font whose last `loca` offset is smaller than the one before it, or past the end of
//! its `glyf` table, draws its last glyph, and the composite glyphs built on it, as if that offset
//! were the `glyf` table's length (vendored hayro-interpret patch 109). The OpenType `loca` table
//! holds `numGlyphs + 1` offsets, the last one the end of the last glyph; subsets written with a
//! final offset of 0 had their last glyph read as a negative length, so it was dropped, and with
//! it every composite that uses it: an `é` built from `e` and an acute accent that was the
//! subset's last glyph drew as nothing (PDFBOX-5917 `test.pdf`, "par t l copieur").
//!
//! The font is built here, in memory: glyph 1 a lower bar, glyph 3 (the last) an upper bar and
//! glyph 2 a composite of the two, with a (1, 0) `cmap` subtable mapping `A` to the composite and
//! `B` to the upper bar alone. No font file is involved.

use super::render_first_page;

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A simple glyph: one rectangular contour from (0, `y0`) to (1000, `y1`).
fn bar(y0: i16, y1: i16) -> Vec<u8> {
    let mut glyf = Vec::new();
    for n in [1i16, 0, y0, 1000, y1] {
        glyf.extend_from_slice(&n.to_be_bytes());
    }
    be16(&mut glyf, 3); // the contour's last point
    be16(&mut glyf, 0); // no instructions
    glyf.extend([1u8; 4]); // four on-curve points, two-byte coordinates
    for dx in [0i16, 1000, 0, -1000] {
        glyf.extend_from_slice(&dx.to_be_bytes());
    }
    for dy in [y0, 0, y1 - y0, 0] {
        glyf.extend_from_slice(&dy.to_be_bytes());
    }
    glyf
}

/// A composite glyph of glyphs 1 and 3, both at the origin.
fn composite() -> Vec<u8> {
    let mut glyf = Vec::new();
    for n in [-1i16, 0, 0, 1000, 1000] {
        glyf.extend_from_slice(&n.to_be_bytes());
    }
    // ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES, then MORE_COMPONENTS on the first.
    for (flags, glyph) in [(0x0023u16, 1u16), (0x0003, 3)] {
        be16(&mut glyf, flags);
        be16(&mut glyf, glyph);
        be32(&mut glyf, 0);
    }
    glyf
}

/// The font, its `loca` offsets long when `long`, the final offset given by `end` from the `glyf`
/// table's length.
fn font(long: bool, end: impl Fn(u32) -> u32) -> Vec<u8> {
    let mut glyf = Vec::new();
    let mut offsets = Vec::new();
    for glyph in [Vec::new(), bar(0, 450), composite(), bar(600, 1000)] {
        offsets.push(glyf.len() as u32);
        glyf.extend(glyph);
    }
    offsets.push(end(glyf.len() as u32));
    let mut loca = Vec::new();
    for offset in offsets {
        if long { be32(&mut loca, offset) } else { be16(&mut loca, (offset / 2) as u16) }
    }

    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f_3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&1000i16.to_be_bytes());
    head[42..44].copy_from_slice(&1000i16.to_be_bytes());
    head[50..52].copy_from_slice(&u16::from(long).to_be_bytes());
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[4..6].copy_from_slice(&1000i16.to_be_bytes());
    hhea[34..36].copy_from_slice(&4u16.to_be_bytes());
    let mut hmtx = Vec::new();
    for _ in 0..4 {
        be16(&mut hmtx, 1000);
        be16(&mut hmtx, 0);
    }
    let mut maxp = vec![0, 1, 0, 0];
    be16(&mut maxp, 4);
    maxp.resize(32, 0);

    // (1, 0), format 0: `A` → glyph 2 (the composite), `B` → glyph 3 (the upper bar).
    let mut mac = Vec::new();
    be16(&mut mac, 0);
    be16(&mut mac, 262);
    be16(&mut mac, 0);
    let mut ids = [0u8; 256];
    ids[usize::from(b'A')] = 2;
    ids[usize::from(b'B')] = 3;
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

/// A 70 × 40 pt page that shows `AB` at 30 pt from (5, 5) in the font.
fn page_with_font(font: &[u8]) -> Vec<u8> {
    let content = b"BT /F1 30 Tf 5 5 Td (AB) Tj ET".to_vec();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 70 40] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.clone(), b"\nendstream".to_vec()].concat(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Bars /FirstChar 65 /LastChar 66 /Widths [1000 1000] /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /AAAAAA+Bars /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile2 7 0 R >>".to_vec(),
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

/// Whether, in order, `A`'s lower bar, `A`'s upper bar and `B`'s upper bar are drawn.
fn drawn(font: &[u8]) -> [bool; 3] {
    let page = render_first_page(page_with_font(font));
    let at = |x: u32, y: u32| page.rgba.get(((y * page.width + x) * 4) as usize).copied().unwrap_or(255);
    // Page space: `A` spans x 5–35 and `B` 35–65; the lower bars y 5–18.5, the upper ones 23–35.
    // Rows count from the top of the 40 pt page.
    assert_eq!(at(20, 19), 255, "nothing is drawn between the bars");
    [at(20, 28) < 128, at(20, 10) < 128, at(50, 10) < 128]
}

#[test]
fn a_last_glyph_whose_loca_end_is_zero_draws_and_so_do_composites_built_on_it() {
    assert_eq!(drawn(&font(true, |len| len)), [true; 3], "a sane loca draws everything");
    assert_eq!(drawn(&font(false, |len| len)), [true; 3], "so does a sane short loca");
    assert_eq!(drawn(&font(true, |_| 0)), [true; 3], "long offsets, the last one 0");
    assert_eq!(drawn(&font(false, |_| 0)), [true; 3], "short offsets, the last one 0");
    // The upper bar is 34 bytes long, so its start is 34 bytes before the table's end.
    assert_eq!(drawn(&font(true, |len| len - 35)), [true; 3], "the last offset just before the last glyph's start");
    assert_eq!(drawn(&font(true, |len| len + 400)), [true; 3], "the last offset past the glyf table");
}
