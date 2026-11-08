//! A symbolic TrueType font with no `/Encoding`, no `/ToUnicode` and no glyph names reads a code
//! as Mac OS Roman when its (1, 0) `cmap` subtable selects the glyph the code draws and that glyph
//! sits where the standard Macintosh glyph order puts the code's Mac OS Roman character (vendored
//! hayro-interpret patch 101; ISO 32000-2 §9.6.5.4: single bytes "look up the associated glyph
//! descriptions" in the (1, 0) subtable, and a (3, 0) subtable maps "the single-byte codes in the
//! (1, 0) subtable" prepended with 0xF0). pdf.js's issue2017r.pdf has such a font; its text read as
//! WinAnsi, `ˆ` for `à`.
//!
//! The fonts are built here, in memory: [`GLYPHS`] glyphs, those the tests show filled squares, no
//! `post` table, and the `cmap` subtables each test asks for. No font file is involved.

use super::*;

/// Glyphs in each font: past the standard Macintosh order's `quoteright` (183).
const GLYPHS: u16 = 200;

/// The standard Macintosh order's glyphs for `agrave`, `eacute`, `endash` and `quoteright`.
const AGRAVE: u8 = 106;
const EACUTE: u8 = 112;
const ENDASH: u8 = 178;
const QUOTERIGHT: u8 = 183;

fn be16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}

/// A (1, 0) format 0 subtable mapping each code to its glyph.
fn mac_subtable(map: &[(u8, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    for n in [0u16, 262, 0] {
        be16(&mut out, n); // format, length, language
    }
    let mut ids = [0u8; 256];
    for &(code, glyph) in map {
        ids[usize::from(code)] = glyph;
    }
    out.extend(ids);
    out
}

/// A (3, 0) format 4 subtable mapping each code at 0xF000 to its glyph, one segment each.
fn windows_subtable(map: &[(u8, u8)]) -> Vec<u8> {
    let segments: Vec<(u16, u16)> = map.iter().map(|&(code, glyph)| (0xF000 | u16::from(code), u16::from(glyph))).chain([(0xFFFF, 0)]).collect();
    let count = segments.len() as u16;
    let mut out = Vec::new();
    for n in [4u16, 16 + 8 * count, 0, 2 * count, 2, 0, 0] {
        be16(&mut out, n); // format, length, language, segCountX2, searchRange, entrySelector, rangeShift
    }
    for &(code, _) in &segments {
        be16(&mut out, code); // endCode
    }
    be16(&mut out, 0); // reservedPad
    for &(code, _) in &segments {
        be16(&mut out, code); // startCode
    }
    for &(code, glyph) in &segments {
        be16(&mut out, if code == 0xFFFF { 1 } else { glyph.wrapping_sub(code) }); // idDelta
    }
    for _ in &segments {
        be16(&mut out, 0); // idRangeOffset
    }
    out
}

/// A TrueType font of [`GLYPHS`] glyphs, the `inked` ones squares, whose (3, 0) subtable maps
/// `windows` and whose (1, 0) subtable, if any, maps `mac`. A `named` font has a version 2.0 `post`
/// table naming its `inked` glyphs `bullet0`, `bullet1` and so on.
fn font(inked: &[u8], mac: Option<&[(u8, u8)]>, windows: &[(u8, u8)], named: bool) -> Vec<u8> {
    // One contour, the square 0..1000 on both axes.
    let mut square = Vec::new();
    for n in [1u16, 0, 0, 1000, 1000] {
        be16(&mut square, n);
    }
    be16(&mut square, 3); // the contour's last point
    be16(&mut square, 0); // no instructions
    square.extend([1u8; 4]); // four on-curve points, two-byte coordinates
    for dx in [0i16, 1000, 0, -1000] {
        square.extend_from_slice(&dx.to_be_bytes());
    }
    for dy in [0i16, 0, 1000, 0] {
        square.extend_from_slice(&dy.to_be_bytes());
    }
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for glyph in 0..GLYPHS {
        be16(&mut loca, (glyf.len() / 2) as u16);
        if inked.iter().any(|g| u16::from(*g) == glyph) {
            glyf.extend_from_slice(&square);
        }
    }
    be16(&mut loca, (glyf.len() / 2) as u16);

    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f_3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&1000i16.to_be_bytes());
    head[42..44].copy_from_slice(&1000i16.to_be_bytes());
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[4..6].copy_from_slice(&1000i16.to_be_bytes());
    hhea[34..36].copy_from_slice(&GLYPHS.to_be_bytes());
    let mut hmtx = Vec::new();
    for _ in 0..GLYPHS {
        be16(&mut hmtx, 1000);
        be16(&mut hmtx, 0);
    }
    let mut maxp = vec![0, 1, 0, 0];
    be16(&mut maxp, GLYPHS);
    maxp.resize(32, 0);

    let subtables: Vec<((u16, u16), Vec<u8>)> =
        mac.map(|m| ((1, 0), mac_subtable(m))).into_iter().chain([((3, 0), windows_subtable(windows))]).collect();
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, subtables.len() as u16);
    let mut offset = 4 + 8 * subtables.len() as u32;
    for ((platform, encoding), bytes) in &subtables {
        be16(&mut cmap, *platform);
        be16(&mut cmap, *encoding);
        be32(&mut cmap, offset);
        offset += bytes.len() as u32;
    }
    for (_, bytes) in &subtables {
        cmap.extend(bytes);
    }

    let mut tables =
        vec![(*b"cmap", cmap), (*b"glyf", glyf), (*b"head", head), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"loca", loca), (*b"maxp", maxp)];
    if named {
        let mut post = vec![0, 2, 0, 0];
        post.resize(32, 0);
        be16(&mut post, GLYPHS);
        for glyph in 0..GLYPHS {
            // Index 258 and on are the table's own names, in order.
            let own = inked.iter().position(|g| u16::from(*g) == glyph);
            be16(&mut post, own.map_or(0, |i| 258 + i as u16));
        }
        for i in 0..inked.len() {
            let name = format!("bullet{i}");
            post.push(name.len() as u8);
            post.extend(name.bytes());
        }
        tables.push((*b"post", post));
    }
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

/// The text extracted from a page that shows `codes` in `font`, marked symbolic and with no
/// `/Encoding` and no `/ToUnicode`.
fn extracted(font: &[u8], codes: &[u8]) -> String {
    extracted_with(font, codes, "AAAAAA+Sym", "")
}

/// [`extracted`] with the font dictionary's `/BaseFont` named `base_font` and the `extra` entries.
fn extracted_with(font: &[u8], codes: &[u8], base_font: &str, extra: &str) -> String {
    let hex: String = codes.iter().map(|c| format!("{c:02X}")).collect();
    let content = format!("BT /F1 30 Tf 5 5 Td <{hex}> Tj ET").into_bytes();
    let widths = vec!["1000"; 256].join(" ");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 80 40] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.clone(), b"\nendstream".to_vec()].concat(),
        format!("<< /Type /Font /Subtype /TrueType /BaseFont /{base_font} /FirstChar 0 /LastChar 255 /Widths [{widths}] /FontDescriptor 6 0 R {extra}>>").into_bytes(),
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
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    out.text.map(|t| t.plain_text()).unwrap_or_default()
}

#[test]
fn a_symbolic_truetype_font_in_the_macintosh_order_reads_its_codes_as_mac_roman() {
    // As issue2017r.pdf's font: code c and 0xF000 + c select the glyph the standard Macintosh
    // order gives Mac OS Roman c.
    let map = [(0x88, AGRAVE), (0x8E, EACUTE)];
    let font = font(&[AGRAVE, EACUTE], Some(&map), &map, false);
    assert_eq!(extracted(&font, &[0x88, 0x8E]), "àé", "Mac OS Roman 0x88 is à and 0x8E é");
}

#[test]
fn winansi_codes_of_a_font_in_the_macintosh_order_read_as_before() {
    // A Windows subset in the same order: code 0x92 draws `quoteright`, WinAnsi 0x92, where Mac OS
    // Roman 0x92 is `iacute`.
    let map = [(0x92, QUOTERIGHT), (0x96, ENDASH)];
    let font = font(&[ENDASH, QUOTERIGHT], Some(&map), &map, false);
    assert_eq!(extracted(&font, &[0x92, 0x96]), "\u{2019}\u{2013}", "WinAnsi, not Mac OS Roman's íñ");
}

#[test]
fn a_symbolic_truetype_font_without_a_1_0_subtable_reads_as_before() {
    let font = font(&[AGRAVE, EACUTE], None, &[(0x88, AGRAVE), (0x8E, EACUTE)], false);
    assert_eq!(extracted(&font, &[0x88, 0x8E]), "ˆŽ", "with no (1, 0) subtable the codes read as WinAnsi");
}

#[test]
fn a_1_0_subtable_selecting_other_glyphs_than_the_drawn_ones_is_not_read() {
    // The (3, 0) subtable is used first (#4156); a (1, 0) one that disagrees doesn't say what the
    // drawn glyphs are.
    let font = font(&[AGRAVE, EACUTE], Some(&[(0x88, EACUTE), (0x8E, AGRAVE)]), &[(0x88, AGRAVE), (0x8E, EACUTE)], false);
    assert_eq!(extracted(&font, &[0x88, 0x8E]), "ˆŽ", "the codes read as WinAnsi, as before");
}

#[test]
fn a_font_that_names_its_glyphs_is_not_read_by_their_order() {
    // A Wingdings subset draws code 0x83 with glyph 101, `Eacute` in the order, and names it
    // `threesans`: a font with glyph names needn't keep the order.
    let map = [(0x88, AGRAVE), (0x8E, EACUTE)];
    let font = font(&[AGRAVE, EACUTE], Some(&map), &map, true);
    assert_eq!(extracted(&font, &[0x88, 0x8E]), "ˆŽ", "the codes read as WinAnsi, as before");
}

#[test]
fn a_symbol_or_zapfdingbats_font_in_the_macintosh_order_gets_no_mac_roman_reading() {
    // Symbol and ZapfDingbats have built-in encodings that aren't Latin (ISO 32000-2 §9.6.6.1), so their codes get
    // no implicit Mac OS Roman reading, as the editor's `unicode_map` gives them none.
    let map = [(0x88, AGRAVE), (0x8E, EACUTE)];
    let font = font(&[AGRAVE, EACUTE], Some(&map), &map, false);
    for name in ["ZapfDingbats", "AAAAAA+Symbol"] {
        let text = extracted_with(&font, &[0x88, 0x8E], name, "");
        assert_eq!(text, "ˆŽ", "{name}: the codes read as WinAnsi, as before");
    }
}

#[test]
fn differences_naming_notdef_clear_the_mac_roman_reading_for_its_code() {
    // `/Differences` without a base encoding: code 0x8E names `.notdef`, and code 0x88 keeps the Mac OS Roman reading.
    let map = [(0x88, AGRAVE), (0x8E, EACUTE)];
    let font = font(&[AGRAVE, EACUTE], Some(&map), &map, false);
    let text = extracted_with(&font, &[0x88, 0x8E], "AAAAAA+Sym", "/Encoding << /Differences [142 /.notdef] >> ");
    assert_eq!(text, "à", "0x8E reads as nothing");
}

#[test]
fn a_base_encoding_name_this_reader_does_not_know_keeps_the_mac_roman_reading_off() {
    // An unknown `/Encoding` name is read as the built-in encoding, but it names a base encoding all the same, so the
    // editor gives it no Mac OS Roman reading and neither does the renderer: the codes read as WinAnsi, as before.
    let map = [(0x88, AGRAVE), (0x8E, EACUTE)];
    let font = font(&[AGRAVE, EACUTE], Some(&map), &map, false);
    assert_eq!(extracted_with(&font, &[0x88, 0x8E], "AAAAAA+Sym", "/Encoding /NoSuchEncoding "), "ˆŽ");
}
