//! An Arial the PDF doesn't embed draws the glyphs its codes name (vendored hayro-interpret
//! patch 76, #5889). Word and PDFMaker write such a font as a Type 0 `CIDFontType2` with
//! `Identity-H`, often without `/ToUnicode`, so each code is a glyph index in the author's Arial.
//! Arial numbers its first 258 glyphs in the standard Macintosh order of the TrueType `post`
//! table (`L` is 47, `o` 82, `w` 90). The standard substitute numbers its glyphs another way, and
//! "Low Power" drew as "@ck 'Dck Yf".
//!
//! Each document is drawn twice, once as such a file has it and once with a `/ToUnicode` that
//! names each code's character, which draws through the substitute's glyph for that character.
//! The two must be the same pixels.

use super::render_first_page;

/// "Low É" as Arial's glyph indexes: L, o, w, space, Eacute.
const LOW_E_ACUTE: [(u16, char); 5] = [(47, 'L'), (82, 'o'), (90, 'w'), (3, ' '), (101, 'É')];

struct Font<'a> {
    /// The `/BaseFont` of both the Type 0 font and its descendant.
    base_font: &'a str,
    /// The descendant's `/Subtype`.
    subtype: &'a str,
    /// `true`: the codes are 1, 2, … and a `/CIDToGIDMap` stream gives each its Arial glyph.
    /// `false`: `/CIDToGIDMap /Identity`, and the codes are the glyph indexes themselves.
    mapped: bool,
    to_unicode: bool,
}

/// A page showing [`LOW_E_ACUTE`] at 40 pt in `font`, which has no font program.
fn pdf(font: &Font<'_>) -> Vec<u8> {
    let codes: Vec<u16> = if font.mapped { (1..=LOW_E_ACUTE.len() as u16).collect() } else { LOW_E_ACUTE.iter().map(|(gid, _)| *gid).collect() };
    let hex: String = codes.iter().map(|code| format!("{code:04X}")).collect();
    let content = format!("BT /F1 40 Tf 5 15 Td <{hex}> Tj ET");
    let bfchar: String = codes.iter().zip(LOW_E_ACUTE).map(|(code, (_, c))| format!("<{code:04X}> <{:04X}>\n", u32::from(c))).collect();
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def /CMapType 2 def
1 begincodespacerange <0000> <FFFF> endcodespacerange
{} beginbfchar
{bfchar}endbfchar
endcmap CMapName currentdict /CMap defineresource pop end end",
        codes.len()
    );
    // CID 0 is .notdef; CIDs 1… give the glyphs in order.
    let cid_to_gid: Vec<u8> = std::iter::once(0).chain(LOW_E_ACUTE.iter().map(|(gid, _)| *gid)).flat_map(u16::to_be_bytes).collect();
    let Font { base_font, subtype, .. } = font;
    let to_unicode = if font.to_unicode { " /ToUnicode 8 0 R" } else { "" };
    let map = if font.mapped { "9 0 R" } else { "/Identity" };

    let mut out = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 220 60] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type0 /BaseFont /{base_font} /Encoding /Identity-H /DescendantFonts [6 0 R]{to_unicode} >> endobj
6 0 obj << /Type /Font /Subtype /{subtype} /BaseFont /{base_font} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>
  /FontDescriptor 7 0 R /CIDToGIDMap {map} /DW 1000 >> endobj
7 0 obj << /Type /FontDescriptor /FontName /{base_font} /Flags 32 /FontBBox [-665 -325 2000 1040] /ItalicAngle 0 /Ascent 728 /Descent -210 /CapHeight 716 /StemV 80 >> endobj
8 0 obj << /Length {} >> stream
{cmap}
endstream endobj
9 0 obj << /Length {} >> stream
",
        content.len(),
        cmap.len(),
        cid_to_gid.len()
    )
    .into_bytes();
    out.extend_from_slice(&cid_to_gid);
    out.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF");
    out
}

/// `font` drawn without and with a `/ToUnicode`; the second must draw ink.
fn without_and_with_unicode(font: Font<'_>) -> (Vec<u8>, Vec<u8>) {
    let without = render_first_page(pdf(&Font { to_unicode: false, ..font }));
    let with = render_first_page(pdf(&Font { to_unicode: true, ..font }));
    assert!(with.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[0] < 128), "{}: the control must draw ink", font.base_font);
    (without.rgba.to_vec(), with.rgba.to_vec())
}

/// Each Arial face, as Windows exporters name it and as Word names a face it doesn't embed, draws
/// "Low É" from its glyph indexes, with `/CIDToGIDMap /Identity` and with a map stream. The name
/// is read without regard to case, `MT` included.
#[test]
fn an_arial_the_pdf_does_not_embed_draws_the_glyphs_its_codes_name() {
    for base_font in [
        "ArialMT",
        "Arial-BoldMT",
        "Arial-ItalicMT",
        "Arial-BoldItalicMT",
        "Arial",
        "Arial,Bold",
        "Arial,BoldItalic",
        "ABCDEF+Arial-BoldMT",
        "arialmt",
        "Arial-Boldmt",
    ] {
        for mapped in [false, true] {
            let (without, with) = without_and_with_unicode(Font { base_font, subtype: "CIDFontType2", mapped, to_unicode: false });
            assert!(without == with, "{base_font} (CIDToGIDMap {}): the codes draw \"Low É\"", if mapped { "stream" } else { "/Identity" });
        }
    }
}

/// Only Arial's own faces are read in its order: Arial Narrow, Arial Black and Arial Unicode MS
/// are other fonts, and a `CIDFontType0` font's CIDs aren't TrueType glyph indexes.
#[test]
fn codes_of_other_fonts_are_not_read_in_arials_order() {
    for (base_font, subtype) in [
        ("ArialNarrow", "CIDFontType2"),
        ("ArialNarrow-Bold", "CIDFontType2"),
        ("Arial-Black", "CIDFontType2"),
        ("ArialUnicodeMS", "CIDFontType2"),
        ("ArialMT", "CIDFontType0"),
    ] {
        let (without, with) = without_and_with_unicode(Font { base_font, subtype, mapped: false, to_unicode: false });
        assert!(without != with, "{base_font} ({subtype}) is not read as Arial");
    }
}
