//! A CID that is its character's vertical form draws a substitute's vertical glyph (vendored
//! hayro-interpret patch 107, #7167). Adobe-Japan1 gives "…" CID 668 and its vertical form CID
//! 7897; a font the PDF doesn't embed draws both through their character, U+2026, so a vertical
//! column showed the horizontal ellipsis, and 「」、。ー（） and the small kana the same way.
//!
//! The substitute here is the bundled Inter face with an OpenType `vert` lookup of our own that
//! replaces its ellipsis with its vertical bar, so which glyph is drawn tells the two apart.

use hayro::hayro_interpret::font::{FontData, Glyph};
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use kurbo::{Affine, BezPath, Rect};
use skrifa::MetadataProvider as _;
use std::sync::Arc;

/// The glyph indexes drawn, in order.
#[derive(Default)]
struct Glyphs(Vec<u32>);

impl<'a> Device<'a> for Glyphs {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        if let Glyph::Outline(glyph) = glyph {
            self.0.push(glyph.glyph_id().to_u32());
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Inter's glyph for `c`.
fn inter_glyph(c: char) -> u16 {
    let font = skrifa::FontRef::new(pdfcraft_fonts::INTER_REGULAR).expect("Inter parses");
    let glyph = font.charmap().map(c).expect("Inter has the character");
    u16::try_from(glyph.to_u32()).expect("Inter's glyph indexes are 16-bit")
}

fn u16s(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// How the `vert` lookup's single substitution is written.
#[derive(Clone, Copy, Debug)]
enum Subtable {
    /// Format 2, a list of substitutes.
    List,
    /// Format 1, a delta, reached through an extension lookup (type 7), as large CJK faces have it.
    DeltaInExtension,
}

/// A GSUB table whose one feature, tagged `feature`, is in the `DFLT` script's default language
/// system and replaces glyph `from` with glyph `to`.
fn gsub(feature: &[u8; 4], from: u16, to: u16, subtable: Subtable) -> Vec<u8> {
    // DFLT script; its default language system lists feature 0.
    let mut scripts = u16s(&[1]);
    scripts.extend(b"DFLT");
    scripts.extend(u16s(&[8, 4, 0, 0, 0xFFFF, 1, 0]));
    // Feature 0: `feature`, lookup 0.
    let mut features = u16s(&[1]);
    features.extend(feature);
    features.extend(u16s(&[8, 0, 1, 0]));
    // Coverage of `from` only.
    let coverage = u16s(&[1, 1, from]);
    let single = match subtable {
        Subtable::List => [u16s(&[2, 8, 1, to]), coverage].concat(),
        Subtable::DeltaInExtension => [u16s(&[1, 6, to.wrapping_sub(from)]), coverage].concat(),
    };
    let (lookup_type, body) = match subtable {
        Subtable::List => (1, single),
        // Extension format 1: the type it holds, then a 32-bit offset to it.
        Subtable::DeltaInExtension => (7, [u16s(&[1, 1, 0, 8]), single].concat()),
    };
    let lookups = [u16s(&[1, 4, lookup_type, 0, 1, 8]), body].concat();
    let script_list = 10u16;
    let feature_list = script_list + u16::try_from(scripts.len()).unwrap();
    let lookup_list = feature_list + u16::try_from(features.len()).unwrap();
    [u16s(&[1, 0, script_list, feature_list, lookup_list]), scripts, features, lookups].concat()
}

/// The font file `font` with its table `tag` replaced by `table` (or added).
fn with_table(font: &[u8], tag: &[u8; 4], table: &[u8]) -> Vec<u8> {
    let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let mut tables: Vec<([u8; 4], Vec<u8>)> = (0..count)
        .map(|i| {
            let record = &font[12 + 16 * i..28 + 16 * i];
            let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
            let length = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
            (record[..4].try_into().unwrap(), font[offset..offset + length].to_vec())
        })
        .filter(|(t, _)| t != tag)
        .collect();
    tables.push((*tag, table.to_vec()));
    tables.sort_by_key(|(t, _)| *t);
    let count = tables.len();
    let power = 1usize << count.ilog2();
    let mut out = font[..4].to_vec();
    out.extend(u16s(&[count as u16, (power * 16) as u16, power.ilog2() as u16, ((count - power) * 16) as u16]));
    let mut data = Vec::new();
    let start = 12 + 16 * count;
    for (t, bytes) in &tables {
        out.extend(t);
        out.extend([0; 4]);
        out.extend(((start + data.len()) as u32).to_be_bytes());
        out.extend((bytes.len() as u32).to_be_bytes());
        data.extend(bytes);
        data.resize(data.len().next_multiple_of(4), 0);
    }
    out.extend(data);
    out
}

/// A page showing `code` in a Ryumin-Light of Adobe-Japan1 that the PDF doesn't embed.
fn pdf(encoding: &str, code: &str) -> Vec<u8> {
    let content = format!("BT /F1 40 Tf 20 60 Td <{code}> Tj ET");
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type0 /BaseFont /Ryumin-Light /Encoding /{encoding} /DescendantFonts [6 0 R] >> endobj
6 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /Ryumin-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >>
  /FontDescriptor 7 0 R /DW 1000 >> endobj
7 0 obj << /Type /FontDescriptor /FontName /Ryumin-Light /Flags 6 /FontBBox [-170 -331 1024 903] /ItalicAngle 0 /Ascent 752 /Descent -271 /CapHeight 742 /StemV 54 >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

/// The glyphs drawn for `pdf`'s page with `face` as the substitute for every font.
fn glyphs(pdf: Vec<u8>, face: &Arc<Vec<u8>>) -> Vec<u32> {
    let data: FontData = face.clone();
    let settings = InterpreterSettings { font_resolver: Arc::new(move |_| Some((data.clone(), 0))), ..Default::default() };
    let parsed = Pdf::new(Arc::new(pdf)).expect("parses");
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, 100.0, 100.0), &cache, page.xref(), settings);
    let mut device = Glyphs::default();
    interpret_page(page, &mut ctx, &mut device);
    device.0
}

/// "…" by its vertical form's CID, through a `-V` CMap or as `Identity-H` CID 7897 (0x1ED9, an
/// old Japanese document's column, #7167), draws the substitute's vertical glyph. By its
/// horizontal CID, 668 (0x029C), it draws the ordinary one.
#[test]
fn a_vertical_cid_draws_the_substitutes_vertical_glyph() {
    let (ellipsis, bar) = (inter_glyph('…'), inter_glyph('|'));
    for subtable in [Subtable::List, Subtable::DeltaInExtension] {
        let face = Arc::new(with_table(pdfcraft_fonts::INTER_REGULAR, b"GSUB", &gsub(b"vert", ellipsis, bar, subtable)));
        for (encoding, code, expected, form) in [
            ("UniJIS-UCS2-V", "2026", bar, "vertical"),
            ("Identity-H", "1ED9", bar, "vertical"),
            ("Identity-V", "1ED9", bar, "vertical"),
            ("UniJIS-UCS2-H", "2026", ellipsis, "horizontal"),
            ("Identity-H", "029C", ellipsis, "horizontal"),
            ("Identity-V", "029C", ellipsis, "horizontal"),
        ] {
            assert_eq!(glyphs(pdf(encoding, code), &face), [u32::from(expected)], "{subtable:?}: {encoding} <{code}> is the {form} ellipsis");
        }
    }
}

/// A face whose vertical forms are under `vrt2` alone substitutes them too; a face with no
/// vertical substitution, or one whose `vert` covers other glyphs, draws the character's
/// ordinary glyph, as before.
#[test]
fn a_face_without_a_vertical_glyph_draws_the_ordinary_one() {
    let (ellipsis, bar, a) = (inter_glyph('…'), inter_glyph('|'), inter_glyph('a'));
    let vrt2 = Arc::new(with_table(pdfcraft_fonts::INTER_REGULAR, b"GSUB", &gsub(b"vrt2", ellipsis, bar, Subtable::List)));
    assert_eq!(glyphs(pdf("UniJIS-UCS2-V", "2026"), &vrt2), [u32::from(bar)], "vrt2 stands in for vert");
    let other = Arc::new(with_table(pdfcraft_fonts::INTER_REGULAR, b"GSUB", &gsub(b"vert", a, bar, Subtable::List)));
    let other_feature = Arc::new(with_table(pdfcraft_fonts::INTER_REGULAR, b"GSUB", &gsub(b"smcp", ellipsis, bar, Subtable::List)));
    let plain = Arc::new(pdfcraft_fonts::INTER_REGULAR.to_vec());
    for (face, which) in [(&other, "a vert lookup for other glyphs"), (&other_feature, "another feature"), (&plain, "Inter's own GSUB")] {
        assert_eq!(glyphs(pdf("UniJIS-UCS2-V", "2026"), face), [u32::from(ellipsis)], "{which}: the ordinary ellipsis");
    }
}
