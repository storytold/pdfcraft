//! A CID-keyed CFF font (`FontFile3` of subtype `CIDFontType0C`, ISO 32000-2 §9.9 Table 124)
//! selects a font dict for each glyph through its FDSelect, and a glyph's charstring needs the
//! Private DICT of that font dict (Adobe TN #5176). A subsetter can empty the font dicts no kept
//! glyph uses, leaving them without a Private DICT. Those font dicts make only their own glyphs
//! missing; the glyphs of the other font dicts still draw (hayro-interpret patch (93)).
//!
//! The font is built here: `.notdef` and two 500-unit squares, CID 1 in the first font dict and
//! CID 2 in the second, which is empty.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

/// A CFF INDEX with one-byte offsets.
fn index(items: &[&[u8]]) -> Vec<u8> {
    let mut out = u16::try_from(items.len()).unwrap().to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    out.push(1);
    let mut offset = 1u8;
    out.push(offset);
    for item in items {
        offset += u8::try_from(item.len()).unwrap();
        out.push(offset);
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// A DICT integer operand in the 5-byte form, so every DICT has a fixed size.
fn int(value: usize) -> Vec<u8> {
    let mut out = vec![29];
    out.extend_from_slice(&u32::try_from(value).unwrap().to_be_bytes());
    out
}

fn top_dict(charset: usize, fd_select: usize, char_strings: usize, fd_array: usize) -> Vec<u8> {
    let mut dict = Vec::new();
    // ROS comes first in a CID font's Top DICT: (Adobe) (Identity) 0.
    for operand in [391, 392, 0] {
        dict.extend(int(operand));
    }
    dict.extend([12, 30]);
    dict.extend(int(charset));
    dict.push(15);
    dict.extend(int(char_strings));
    dict.push(17);
    dict.extend(int(fd_array));
    dict.extend([12, 36]);
    dict.extend(int(fd_select));
    dict.extend([12, 37]);
    dict
}

fn font_dict(private: Option<(usize, usize)>) -> Vec<u8> {
    let mut dict = Vec::new();
    if let Some((size, offset)) = private {
        dict.extend(int(size));
        dict.extend(int(offset));
        dict.push(18);
    }
    dict
}

/// The CID-keyed CFF program. The second font dict is always empty; the first has a Private
/// DICT only when `first_has_private`.
fn cid_cff(first_has_private: bool) -> Vec<u8> {
    let header = vec![1, 0, 4, 1];
    let names = index(&[b"T"]);
    let strings = index(&[b"Adobe", b"Identity"]);
    let global_subrs = index(&[]);
    // Format 0: the CIDs of glyphs 1 and 2.
    let charset = vec![0, 0, 1, 0, 2];
    // Format 0: the font dict of each glyph.
    let fd_select = vec![0, 0, 0, 1];
    // 0 0 rmoveto 500 0 rlineto 0 500 rlineto -500 0 rlineto endchar.
    let square: &[u8] = &[139, 139, 21, 28, 1, 244, 139, 5, 139, 28, 1, 244, 5, 28, 254, 12, 139, 5, 14];
    let char_strings = index(&[&[14], square, square]);
    // defaultWidthX 0.
    let mut private = int(0);
    private.push(20);

    let top_len = index(&[&top_dict(0, 0, 0, 0)]).len();
    let charset_at = header.len() + names.len() + top_len + strings.len() + global_subrs.len();
    let fd_select_at = charset_at + charset.len();
    let char_strings_at = fd_select_at + fd_select.len();
    let fd_array_at = char_strings_at + char_strings.len();
    let first_len = font_dict(first_has_private.then_some((0, 0))).len();
    let private_at = fd_array_at + index(&[&vec![0; first_len], &[]]).len();
    let first = font_dict(first_has_private.then_some((private.len(), private_at)));
    let fd_array = index(&[&first, &[]]);
    let top = index(&[&top_dict(charset_at, fd_select_at, char_strings_at, fd_array_at)]);

    let mut out = header;
    for part in [names, top, strings, global_subrs, charset, fd_select, char_strings, fd_array, private] {
        out.extend(part);
    }
    out
}

/// A page showing the hex string `codes` at 60 pt in an Identity-H Type0 font over `program`
/// (no `/FontFile3` when `program` is empty).
fn page(program: &[u8], codes: &str) -> Vec<u8> {
    let font_file = if program.is_empty() { "" } else { " /FontFile3 8 0 R" };
    let body = format!("BT /F1 60 Tf 20 30 Td <{codes}> Tj ET");
    let mut stream = format!("<< /Subtype /CIDFontType0C /Length {} >>\nstream\n", program.len()).into_bytes();
    stream.extend_from_slice(program);
    stream.extend_from_slice(b"\nendstream");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 120] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+Synthetic /Encoding /Identity-H /DescendantFonts [6 0 R] >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /ABCDEF+Synthetic /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 1000 >>".to_vec(),
        format!("<< /Type /FontDescriptor /FontName /ABCDEF+Synthetic /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80{font_file} >>").into_bytes(),
        stream,
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

fn inked(rgba: &[u8]) -> usize {
    rgba.chunks(4).filter(|p| p.first().is_some_and(|r| *r < 128)).count()
}

#[test]
fn glyphs_of_a_usable_font_dict_draw_beside_an_unusable_one() {
    let first = pixels(page(&cid_cff(true), "0001"));
    // The square is 30 pt on a side at 60 pt: 900 pixels at scale 1.
    assert!(inked(&first) >= 800, "the glyph of the usable font dict drew {} pixels", inked(&first));
    let both = pixels(page(&cid_cff(true), "00010002"));
    assert!(both == first, "the glyph of the empty font dict drew something, or the first did not draw");
}

/// With no usable font dict the program is not read, as before, and the page still renders.
#[test]
fn a_program_with_no_usable_font_dict_draws_no_square() {
    let rgba = pixels(page(&cid_cff(false), "00010002"));
    let square = pixels(page(&cid_cff(true), "0001"));
    assert!(rgba != square, "a font dict without a Private DICT drew its glyph");
    // The page renders exactly as it does with no embedded program at all.
    assert!(rgba == pixels(page(&[], "00010002")), "the program was kept although no font dict is usable");
}
