//! A `Type1` font whose `FontFile3` has subtype `OpenType` (ISO 32000-2 §9.9) keeps its CFF
//! program inside an OpenType table directory; it is drawn with that program, not with a
//! standard-font stand-in (hayro-interpret patch (69)).
//!
//! A font dictionary whose own `/Subtype` is `/OpenType`, which ISO 32000-2 §9.6.2, Table 109
//! doesn't list, is read by the program it embeds (hayro-interpret patch (111)): a CFF program
//! as a Type 1 font, TrueType outlines as a TrueType font (PDFBOX-5463).
//!
//! The CFF program is the renderer's own bundled fixed-pitch CFF face and the TrueType one the
//! UI's JetBrains Mono, so no font file is added. Drawn right, the same program gives the same
//! pixels however it is wrapped and labelled; the stand-in a serif font falls back to is a
//! different face.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

const TRUE_TYPE: &[u8] = include_bytes!("../../../../assets/fonts/JetBrainsMono-Regular.ttf");

fn foxit_fixed() -> Vec<u8> {
    use craft_pdf::interpret::InterpreterSettings;
    use craft_pdf::interpret::font::{FontQuery, StandardFont};
    let (data, _) =
        (InterpreterSettings::default().font_resolver)(&FontQuery::Standard(StandardFont::Courier)).expect("bundled Courier stand-in font");
    (*data).as_ref().to_vec()
}

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
    font_page("Type1", program.map(|(bytes, subtype)| (bytes, "FontFile3", subtype)))
}

/// A page showing "AgW" at 60 pt in a serif font of `font_subtype` whose descriptor's `key`
/// holds `program` (of stream `subtype`, if not empty), or which embeds no program when `None`.
fn font_page(font_subtype: &str, program: Option<(&[u8], &str, &str)>) -> Vec<u8> {
    let file = program.map(|(_, key, _)| format!(" /{key} 7 0 R")).unwrap_or_default();
    let body = "BT /F1 60 Tf 10 30 Td (AgW) Tj ET";
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 120] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        format!("<< /Type /Font /Subtype /{font_subtype} /BaseFont /Synthetic-Semibold /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 103 /Widths [600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600] /FontDescriptor 6 0 R >>").into_bytes(),
        format!("<< /Type /FontDescriptor /FontName /Synthetic-Semibold /Flags 34 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 120{file} >>").into_bytes(),
    ];
    if let Some((program, _, subtype)) = program {
        let subtype = if subtype.is_empty() { String::new() } else { format!("/Subtype /{subtype} ") };
        let mut stream = format!("<< {subtype}/Length {} >>\nstream\n", program.len()).into_bytes();
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
    let cff = foxit_fixed();
    let bare = pixels(page(Some((&cff, "Type1C"))));
    assert!(inked(&bare) > 200, "the bare CFF program drew no text");
    let wrapped = pixels(page(Some((&open_type(&cff), "OpenType"))));
    assert!(wrapped == bare, "the OpenType-wrapped program drew a different face: the font fell back");
}

/// A wrapper with no `CFF ` table still draws, with the stand-in a font without a program
/// gets, rather than failing the page or reading the program anyway.
#[test]
fn a_wrapper_without_a_cff_table_falls_back() {
    let cff = foxit_fixed();
    let mut broken = open_type(&cff);
    broken.get_mut(12..16).unwrap().copy_from_slice(b"glyf");
    let rgba = pixels(page(Some((&broken, "OpenType"))));
    assert!(inked(&rgba) > 200, "the stand-in drew no text");
    assert!(rgba == pixels(page(None)), "the broken wrapper drew another face than the stand-in");
    assert!(rgba != pixels(page(Some((&cff, "Type1C")))), "the broken wrapper drew the embedded program");
}

/// PDFBOX-5463: a `/Subtype /OpenType` font dictionary with an `OTTO` (CFF) `FontFile3 /OpenType`
/// draws its program, exactly as the `Type1` font with the same program does. It was read as
/// TrueType, which reads only a `FontFile2`, and drew the stand-in.
#[test]
fn an_open_type_font_dictionary_with_a_cff_program_draws_it_as_a_type1_font() {
    let cff = foxit_fixed();
    let wrapped = open_type(&cff);
    let type1 = pixels(font_page("Type1", Some((&wrapped, "FontFile3", "OpenType"))));
    assert!(inked(&type1) > 200, "the Type1 font drew no text");
    let open_type_dict = pixels(font_page("OpenType", Some((&wrapped, "FontFile3", "OpenType"))));
    assert!(open_type_dict == type1, "the /OpenType font drew another face than its CFF program");
    let bare = pixels(font_page("OpenType", Some((&cff, "FontFile3", "Type1C"))));
    assert!(bare == type1, "the /OpenType font drew another face than its bare CFF program");
    assert!(type1 != pixels(font_page("OpenType", None)), "the program and the stand-in look alike");
}

/// A `/Subtype /OpenType` font whose `FontFile3 /OpenType` has TrueType outlines, and a
/// `/Subtype /TrueType` font with the same program (ISO 32000-2 Table 124 allows it there), draw
/// it as the TrueType font with the program in a `FontFile2` does.
#[test]
fn a_font_file3_open_type_program_with_true_type_outlines_draws_as_true_type() {
    let reference = pixels(font_page("TrueType", Some((TRUE_TYPE, "FontFile2", ""))));
    assert!(inked(&reference) > 200, "the TrueType font drew no text");
    assert!(reference != pixels(font_page("TrueType", None)), "the program and the stand-in look alike");
    for font_subtype in ["OpenType", "TrueType"] {
        let rgba = pixels(font_page(font_subtype, Some((TRUE_TYPE, "FontFile3", "OpenType"))));
        assert!(rgba == reference, "/{font_subtype} with FontFile3 /OpenType drew another face than its program");
    }
    let file2 = pixels(font_page("OpenType", Some((TRUE_TYPE, "FontFile2", ""))));
    assert!(file2 == reference, "/OpenType with a FontFile2 drew another face than its program");
}

/// A `FontFile3 /OpenType` program with TrueType outlines behind Flate and a PNG predictor
/// (ISO 32000-2 §7.4.4.4), or behind two filters (§7.3.8.2), draws as TrueType: its sfnt tag is
/// read from the decoded program, where the predictor gives nothing from the first four inflated
/// bytes because a row is 16 bytes long.
#[test]
fn a_predicted_true_type_program_is_still_read_as_true_type() {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    let reference = pixels(font_page("TrueType", Some((TRUE_TYPE, "FontFile2", ""))));
    let mut program = TRUE_TYPE.to_vec();
    program.resize(TRUE_TYPE.len().div_ceil(16) * 16, 0);
    let rows: Vec<u8> = program.chunks(16).flat_map(|row| std::iter::once(0).chain(row.iter().copied())).collect();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&rows).unwrap();
    let encoded = encoder.finish().unwrap();
    let subtype = "OpenType /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 16 >>";
    for font_subtype in ["OpenType", "TrueType"] {
        let rgba = pixels(font_page(font_subtype, Some((&encoded, "FontFile3", subtype))));
        assert!(rgba == reference, "/{font_subtype} with a predicted FontFile3 /OpenType drew another face than its program");
    }
    // A filter after another: four hex digits give two bytes of the zlib stream, too few to inflate.
    let mut hex: Vec<u8> = encoded.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    hex.push(b'>');
    let chain = "OpenType /Filter [/ASCIIHexDecode /FlateDecode] /DecodeParms [null << /Predictor 12 /Columns 16 >>]";
    let rgba = pixels(font_page("OpenType", Some((&hex, "FontFile3", chain))));
    assert!(rgba == reference, "a FontFile3 /OpenType behind two filters drew another face than its program");
}

/// A `/Subtype /OpenType` font with an unreadable CFF program, or with no program, still draws
/// a stand-in rather than failing the page.
#[test]
fn an_open_type_font_dictionary_without_a_usable_program_falls_back() {
    let mut broken = open_type(&foxit_fixed());
    broken.get_mut(12..16).unwrap().copy_from_slice(b"glyf");
    for program in [Some((&broken[..], "FontFile3", "OpenType")), Some((&[][..], "FontFile3", "OpenType")), None] {
        assert!(inked(&pixels(font_page("OpenType", program))) > 200, "the stand-in drew no text");
    }
}
