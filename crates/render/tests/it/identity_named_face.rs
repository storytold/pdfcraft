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
    let gids: Vec<u32> = "HI".chars().map(|c| font.charmap().map(c).unwrap().to_u32()).collect();
    identity_page("Inter-Regular", &gids, embed.then_some(INTER))
}

/// A one-page document showing the glyph ids `gids` at 60 pt in a Type 0 font with
/// `/Identity-H`, its descendant a CIDFontType2 of the Adobe-Identity collection named `name`,
/// without `/ToUnicode`, and with `file` as its `/FontFile2` when given.
fn identity_page(name: &str, gids: &[u32], file: Option<&[u8]>) -> Vec<u8> {
    let codes: String = gids.iter().map(|g| format!("{g:04X}")).collect();
    let body = format!("BT /F1 60 Tf 10 20 Td <{codes}> Tj ET");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 160 90] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [6 0 R] >>"),
        format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 700 >>"
        ),
        format!(
            "<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [-700 -300 2600 1100] \
             /ItalicAngle 0 /Ascent 970 /Descent -240 /CapHeight 730 /StemV 80{} >>",
            if file.is_some() { " /FontFile2 8 0 R" } else { "" }
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.drain(..).enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    if let Some(file) = file {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("8 0 obj\n<< /Length {} >>\nstream\n", file.len()).as_bytes());
        bytes.extend_from_slice(file);
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

/// Through the viewer's own font resolver, on Windows: a SimSun Identity font the document
/// doesn't embed draws with the installed SimSun on screen, and the export configuration, which
/// uses no installed face, still draws nothing (#6168). Skipped where SimSun isn't installed or
/// installed faces are turned off.
#[cfg(windows)]
#[test]
fn windows_draws_an_identity_simsun_font_on_screen_only() {
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
    use skrifa::string::StringId;
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        eprintln!("installed faces are turned off: skipped");
        return;
    }
    let fonts = std::env::var_os("WINDIR").map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from).join("Fonts");
    let Ok(simsun) = std::fs::read(fonts.join("simsun.ttc")) else {
        eprintln!("SimSun is not installed here: skipped");
        return;
    };
    let face = (0..8)
        .filter_map(|index| skrifa::FontRef::from_index(&simsun, index).ok())
        .find(|font| font.localized_strings(StringId::POSTSCRIPT_NAME).any(|n| n.chars().eq("SimSun".chars())))
        .expect("simsun.ttc holds SimSun");
    let gids: Vec<u32> = "\u{4E2D}\u{6587}".chars().map(|c| face.charmap().map(c).expect("SimSun maps it").to_u32()).collect();
    let bytes = Arc::new(identity_page("ABCDEF+SimSun", &gids, None));
    let ink = |config: RenderConfig| {
        let mut renderer = PageRenderer::new(bytes.clone(), config);
        let image = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(image.error.is_none(), "{:?}", image.error);
        image.rgba.as_chunks::<4>().0.iter().filter(|p| p[0] < 128).count()
    };
    assert!(ink(RenderConfig::default().on_screen()) > 100, "the installed SimSun draws it on screen");
    assert_eq!(ink(RenderConfig::default()), 0, "exports use no installed face");
}
