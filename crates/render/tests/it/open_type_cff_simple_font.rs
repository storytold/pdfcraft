//! A `Type1` font whose `FontFile3` has subtype `OpenType` (ISO 32000-2 §9.9) keeps its CFF
//! program inside an OpenType table directory; it is drawn with that program, not with a
//! standard-font stand-in (hayro-interpret patch (69)).
//!
//! The program is the renderer's own bundled fixed-pitch CFF face, so no font file is added.
//! Drawn right, the bare program and the same program in an OpenType wrapper give the same
//! pixels; the stand-in a serif font falls back to is a different face.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");

/// `cff` in the smallest OpenType wrapper: an `OTTO` table directory with one `CFF ` table.
fn open_type(cff: &[u8]) -> Vec<u8> {
    let mut out = b"OTTO".to_vec();
    out.extend_from_slice(&[0, 1, 0, 16, 0, 0, 0, 0]);
    out.extend_from_slice(b"CFF ");
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&28u32.to_be_bytes());
    out.extend_from_slice(&u32::try_from(cff.len()).unwrap().to_be_bytes());
    out.extend_from_slice(cff);
    out
}

/// A page showing "AgW" at 60 pt in a serif `Type1` font whose `FontFile3` is `program` of
/// `subtype`, or which embeds no program when `None`.
fn page(program: Option<(&[u8], &str)>) -> Vec<u8> {
    let file = if program.is_some() { " /FontFile3 7 0 R" } else { "" };
    let body = "BT /F1 60 Tf 10 30 Td (AgW) Tj ET";
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 120] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Synthetic-Semibold /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 103 /Widths [600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600] /FontDescriptor 6 0 R >>".to_vec(),
        format!("<< /Type /FontDescriptor /FontName /Synthetic-Semibold /Flags 34 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 120{file} >>").into_bytes(),
    ];
    if let Some((program, subtype)) = program {
        let mut stream = format!("<< /Subtype /{subtype} /Length {} >>\nstream\n", program.len()).into_bytes();
        stream.extend_from_slice(program);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
    }

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
fn an_open_type_wrapped_cff_program_draws_like_the_bare_program() {
    let bare = pixels(page(Some((CFF, "Type1C"))));
    assert!(inked(&bare) > 200, "the bare CFF program drew no text");
    let wrapped = pixels(page(Some((&open_type(CFF), "OpenType"))));
    assert!(wrapped == bare, "the OpenType-wrapped program drew a different face: the font fell back");
}

/// A wrapper with no `CFF ` table still draws, with the stand-in a font without a program
/// gets, rather than failing the page or reading the program anyway.
#[test]
fn a_wrapper_without_a_cff_table_falls_back() {
    let mut broken = open_type(CFF);
    broken.get_mut(12..16).unwrap().copy_from_slice(b"glyf");
    let rgba = pixels(page(Some((&broken, "OpenType"))));
    assert!(inked(&rgba) > 200, "the stand-in drew no text");
    assert!(rgba == pixels(page(None)), "the broken wrapper drew another face than the stand-in");
    assert!(rgba != pixels(page(Some((CFF, "Type1C")))), "the broken wrapper drew the embedded program");
}
