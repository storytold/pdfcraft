//! A Type 0 font with `/Identity-H` that the document names but doesn't embed (#6168): its CIDs
//! are the glyph ids of the font it names (ISO 32000-2 §9.7.4.2, `/CIDToGIDMap` /Identity), so
//! given that very face as its substitute, the text draws as it would with the font embedded.
//! Contributor-original synthetic fixtures; the face is the repository's Inter.
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::{FontData, FontQuery};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use hayro::hayro_syntax::Pdf;
use hayro::{RenderCache, RenderSettings, render};
use skrifa::MetadataProvider as _;
use std::sync::Arc;

const INTER: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");

/// The page shows "HI" by Inter's glyph ids in a CIDFontType2 named `Inter-Regular`, with no
/// `/ToUnicode`; `embed` puts Inter in the descriptor as `/FontFile2`.
fn page(embed: bool) -> Vec<u8> {
    let font = skrifa::FontRef::new(INTER).unwrap();
    let gids: String = "HI".chars().map(|c| format!("{:04X}", font.charmap().map(c).unwrap().to_u32())).collect();
    let body = format!("BT /F1 60 Tf 10 20 Td <{gids}> Tj ET");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 160 90] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type0 /BaseFont /Inter-Regular /Encoding /Identity-H /DescendantFonts [6 0 R] >>".into(),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Inter-Regular \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 700 >>"
            .into(),
        format!(
            "<< /Type /FontDescriptor /FontName /Inter-Regular /Flags 32 /FontBBox [-700 -300 2600 1100] \
             /ItalicAngle 0 /Ascent 970 /Descent -240 /CapHeight 730 /StemV 80{} >>",
            if embed { " /FontFile2 8 0 R" } else { "" }
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.drain(..).enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    if embed {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("8 0 obj\n<< /Length {} >>\nstream\n", INTER.len()).as_bytes());
        bytes.extend_from_slice(INTER);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    bytes
}

/// The page's pixels, with a resolver that answers an Identity font's fallback request with
/// Inter when `substitute` is set, and every other request with nothing.
fn pixels(bytes: Vec<u8>, substitute: bool) -> Vec<u8> {
    let settings = InterpreterSettings {
        font_resolver: Arc::new(move |query| match query {
            FontQuery::Fallback(f) if substitute && f.character_collection.as_ref().is_some_and(|cc| cc.family == CidFamily::AdobeIdentity) => {
                Some((Arc::new(INTER.to_vec()) as FontData, 0))
            }
            _ => None,
        }),
        ..InterpreterSettings::default()
    };
    let pdf = Pdf::new(bytes).unwrap();
    let pages = pdf.pages();
    let cache = RenderCache::new();
    render(pages.first().unwrap(), &cache, &settings, &RenderSettings::default()).data_as_u8_slice().to_vec()
}

fn inked(pixels: &[u8]) -> usize {
    pixels.as_chunks::<4>().0.iter().filter(|p| p[3] > 128).count()
}

#[test]
fn an_identity_font_drawn_with_the_face_it_names_matches_the_font_embedded() {
    let embedded = pixels(page(true), false);
    assert!(inked(&embedded) > 200, "the embedded font draws HI");
    assert_eq!(inked(&pixels(page(false), false)), 0, "without the face, nothing is drawn");
    assert!(pixels(page(false), true) == embedded, "the named face draws the same glyphs where the embedded font does");
}
