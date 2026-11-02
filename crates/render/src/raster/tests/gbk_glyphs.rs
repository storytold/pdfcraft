//! A same-machine pixel oracle, independent of GBK decoding and installed-face selection.
//! All font reads and reference rasters stay in memory; nothing is embedded or published.

use super::*;
use hayro::hayro_interpret::font::FontData;
use skrifa::MetadataProvider as _;
use skrifa::string::StringId;

/// Compare the real GBK PDF path with explicit Unicode and a known installed face. Unlike an
/// ink count or extracted text, this fails if drawing chooses another glyph or font family.
pub(super) fn assert_expected_glyphs(pdf: &Arc<Vec<u8>>, base_font: &str) {
    let (expected, file) = match base_font {
        "SimSun" | "ABCDEF+FZSSK--GBK1-0" => ("SimSun", "simsun.ttc"),
        "SimHei" | "ABCDEF+FZHTK--GBK1-0" => ("SimHei", "simhei.ttf"),
        "KaiTi" | "ABCDEF+FZKTK--GBK1-0" => ("KaiTi", "simkai.ttf"),
        "FangSong" | "ABCDEF+FZFSK--GBK1-0" => ("FangSong", "simfang.ttf"),
        other => panic!("no independent GBK oracle for {other}"),
    };
    // Do not use the production candidate list, resolver or face-selection helpers here.
    let directory = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).unwrap_or_else(|| r"C:\Windows".into());
    let path = std::path::PathBuf::from(directory).join("Fonts").join(file);
    if !path.is_file() {
        eprintln!("{base_font}: exact {expected} face unavailable; pixel oracle skipped");
        return;
    }
    let bytes = Arc::new(std::fs::read(path).expect("read installed reference font"));
    let index = (0..32)
        .find(|&index| {
            skrifa::FontRef::from_index(&bytes, index)
                .is_ok_and(|font| font.localized_strings(StringId::POSTSCRIPT_NAME).any(|name| name.chars().collect::<String>() == expected))
        })
        .expect("reference file contains the expected PostScript face");
    let face = skrifa::FontRef::from_index(&bytes, index).unwrap();
    for character in ['\u{4e2d}', '\u{6587}'] {
        assert!(face.charmap().map(character).is_some_and(|id| id.to_u32() != 0), "{expected} must contain {character}");
    }

    // Explicit ToUnicode maps arbitrary reference codes to U+4E2D and U+6587, bypassing GBK
    // and Adobe-GB1 CID lookup. The fixed resolver below bypasses family classification too.
    let cmap = "begincmap /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange \
                2 beginbfchar <0001> <4E2D> <0002> <6587> endbfchar endcmap";
    let source = std::str::from_utf8(pdf).unwrap();
    assert!(source.contains("/Encoding /GBK-EUC-H") && source.contains("<D6D0CEC4>"));
    let reference = source
        .replace("/Encoding /GBK-EUC-H", "/Encoding /Identity-H /ToUnicode 8 0 R")
        .replace("<D6D0CEC4>", "<00010002>")
        .replace("trailer <<", &format!("8 0 obj << /Length {} >> stream\n{cmap}\nendstream endobj\ntrailer <<", cmap.len()));
    let parsed = Pdf::new(Arc::new(reference.into_bytes())).unwrap();
    let data: FontData = bytes;
    let settings = InterpreterSettings { font_resolver: Arc::new(move |_| Some((data.clone(), index))), ..Default::default() };
    let reference = hayro::render(&parsed.pages()[0], &RenderCache::new(), &settings, &RenderSettings { bg_color: WHITE, ..Default::default() });
    let mut renderer = PageRenderer::new(pdf.clone(), RenderConfig::default().on_screen());
    let actual = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
    assert!(actual.error.is_none(), "{base_font}: {:?}", actual.error);
    let expected_pixels = reference.data_as_u8_slice();
    assert_eq!(actual.rgba.len(), expected_pixels.len());
    assert!(expected_pixels.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] < 128).count() > 300, "reference must draw two visible glyphs");
    assert!(actual.rgba.as_ref() == expected_pixels, "{base_font}: GBK raster must draw the exact {expected} glyphs for U+4E2D U+6587");
    eprintln!("{base_font}: exact {expected} Unicode-reference pixels matched");
}
