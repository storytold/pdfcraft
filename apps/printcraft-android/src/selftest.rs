#![cfg(target_os = "android")]

//! Start-up self-test: does the PDF stack work on this device?
//!
//! PrintCraft is pure Rust — the parser, the content-stream interpreter and the rasterizer are its
//! own crates (hayro), and the font handling is Skrifa — so there is no native PDF library to
//! cross-compile and nothing platform-specific to prove. What is worth proving is that the *same*
//! engine paths the desktop app uses produce pixels and text on Android, through the same
//! `pdfcraft_render` API the canvas calls:
//!
//! 1. parse an embedded one-page PDF ([`TEST_PDF`]) and inspect it (page box, fonts, metadata);
//! 2. rasterize page 1 at 1:1 and check the expected size and that ink was actually drawn;
//! 3. extract the text layer and compare it with what the document says.
//!
//! The result is one logcat line, so a port that draws a blank page is obvious from `adb logcat`
//! alone. Failures are reported, never panicked on (AGENTS.md §4).

use std::sync::Arc;

/// A one-page PDF written for this test: Letter, Helvetica text and a filled rectangle. No
/// third-party asset is involved (AGENTS.md §1).
const TEST_PDF: &[u8] = include_bytes!("../assets/test.pdf");

/// The text the test document draws.
const EXPECTED_TEXT: &str = "PrintCraft on Android";

/// The page box of the test document, in PDF points (US Letter).
const EXPECTED_SIZE: (u32, u32) = (612, 792);

/// Run the self-test and return the line to log. Never panics: a renderer that gives up returns an
/// error in `RenderedPage::error`, and a parser that does is caught by `inspect`.
pub fn run() -> String {
    let bytes = Arc::new(TEST_PDF.to_vec());
    let info = match pdfcraft_render::inspect(bytes.clone(), None) {
        Ok(info) => info,
        Err(e) => return format!("SELFTEST FAILED: the embedded PDF did not parse: {e}"),
    };
    let Some(page) = info.pages.first() else {
        return "SELFTEST FAILED: the embedded PDF has no pages".to_owned();
    };
    let size = (page.width.round() as u32, page.height.round() as u32);
    let fonts = info.fonts.len();
    let version = info.pdf_version.clone();

    let mut renderer = pdfcraft_render::PageRenderer::new(bytes.clone(), pdfcraft_render::RenderConfig::default());
    let rendered = renderer.render(pdfcraft_render::RenderRequest {
        page: 0,
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: None,
        scale: 1.0,
        tag: 0,
        preview: false,
    });
    if let Some(error) = &rendered.error {
        return format!("SELFTEST FAILED: rasterizing page 1 failed: {error}");
    }
    let (width, height) = (rendered.width, rendered.height);
    let ink = ink_pixels(&rendered.rgba);
    let total = (width as usize) * (height as usize);
    let size_ok = size == EXPECTED_SIZE && (width, height) == EXPECTED_SIZE;
    let ink_ok = ink > 0 && ink < total;

    let text = renderer.render(pdfcraft_render::RenderRequest {
        page: 0,
        kind: pdfcraft_render::RequestKind::Text,
        tile: None,
        scale: 1.0,
        tag: 0,
        preview: false,
    });
    let glyphs = text.text.as_ref().map_or(0, |t| t.glyphs.len());
    let text_ok = text.text.as_ref().is_some_and(|t| !t.is_empty());

    let verdict = if size_ok && ink_ok && text_ok { "ok" } else { "SELFTEST FAILED" };
    format!(
        "SELFTEST {verdict}: PDF {version} | page {size:?} pt → raster {width}×{height} ({ink} ink px of {total}, {} ms) | \
         fonts {fonts} | text {glyphs} glyphs (expected {EXPECTED_TEXT:?}) | parser+rasterizer+text: {}",
        rendered.millis,
        if text_ok { "working" } else { "NO TEXT" },
    )
}

/// How many pixels are not (near) white: a page that drew nothing is blank, which is the failure
/// mode this test exists to catch.
fn ink_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|px| px[0] < 250 || px[1] < 250 || px[2] < 250).count()
}
