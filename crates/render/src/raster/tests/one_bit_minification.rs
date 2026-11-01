//! Issue #1700: a 1-bit source shrunk by between 1:1 and 2:1 was drawn by picking single source
//! pixels, so one-pixel strokes (Type 3 image-mask glyphs, fax-era and OCR fonts) vanished. The
//! source is averaged over each target pixel's box, and the box ends are clamped to the source so
//! a floating-point epsilon at the last pixel cannot drop the whole image back to picking.

use super::*;

/// Draws a 10×1 `/BitsPerComponent 1` source with a one-pixel stroke in column 1 across a
/// 6-pixel-wide page (1.67:1): as an `/ImageMask` stencil, or as a DeviceGray image.
#[cfg(not(target_arch = "wasm32"))]
fn draw_one_bit_stroke(mask: bool) -> RenderedPage {
    // Column 1 holds the only ink: for the mask a 0 sample paints (its default decode),
    // for DeviceGray a 0 sample is black.
    let bytes: [u8; 2] = [0xBF, 0xFF];
    let data = hex(&bytes);
    let (mask_dict, space) = if mask { ("/ImageMask true ", "") } else { ("", "/ColorSpace /DeviceGray ") };
    let content = "q 6 0 0 1 0 0 cm /Im0 Do Q";
    let pdf = format!(
        "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 6 1] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /XObject /Subtype /Image /Width 10 /Height 1 {space}{mask_dict}/BitsPerComponent 1 /Filter /ASCIIHexDecode /Length {} >> stream\n{data}>\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF\n",
        content.len(),
        data.len() + 1
    );
    let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (6, 1));
    page
}

/// The stroke in source column 1 of 10 covers 0.4 of device pixel 0 and must draw as a mid-tone,
/// as poppler and Acrobat average it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn one_bit_sources_average_when_minified_between_one_and_two_to_one() {
    for mask in [true, false] {
        let page = draw_one_bit_stroke(mask);
        let gray: Vec<u8> = page.rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
        assert!((64..=191).contains(&gray[0]), "mask={mask}: the stroke is lost: {gray:?}");
        assert_eq!(gray[5], 255, "mask={mask}: the far end stays white: {gray:?}");
    }
}

/// Draws a `width`×1 `/BitsPerComponent 1` `/ImageMask` with its only ink in `ink` (a 0
/// sample, which paints under the mask's default decode) across a `device`-pixel-wide page.
#[cfg(not(target_arch = "wasm32"))]
fn draw_one_bit_mask_row(width: usize, ink: usize, device: u32) -> RenderedPage {
    let mut bytes = vec![0xFFu8; width.div_ceil(8)];
    bytes[ink / 8] &= !(1u8 << (7 - ink % 8));
    let data = hex(&bytes);
    let content = format!("q {device} 0 0 1 0 0 cm /Im0 Do Q");
    let pdf = format!(
        "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {device} 1] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /XObject /Subtype /Image /Width {width} /Height 1 /ImageMask true /BitsPerComponent 1 /Filter /ASCIIHexDecode /Length {} >> stream\n{data}>\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF\n",
        content.len(),
        data.len() + 1
    );
    let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (device, 1));
    page
}

/// Floating-point epsilon: the far end of the last target pixel's box, `(tx + 1) * sx` with
/// `sx = sw / tw`, can round a hair above the source width — 22 × (25 / 22) =
/// 25.000000000000004 — so `ceil` took column `sw`, one past the last source column. That read
/// returned `None` and the whole image silently fell back to nearest picking, which missed the
/// stroke. The box ends are clamped to the source.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn one_bit_minification_clamps_the_last_box_to_the_source() {
    // The smallest case from the review: a 25×1 mask inked only in column 4 drawn 22 px
    // wide. Nearest picking reads columns 0, 1, 2, 3, 5, …, never 4: before the fix the row
    // was white.
    let page = draw_one_bit_mask_row(25, 4, 22);
    let gray: Vec<u8> = page.rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
    assert_eq!(gray[0], 255, "the columns before the stroke stay white: {gray:?}");
    assert!((64..=191).contains(&gray[3]), "the stroke is lost: {gray:?}");
    assert_eq!(gray[21], 255, "the columns after the stroke stay white: {gray:?}");

    // A 300 dpi A4 scan (2480 px) shown as a 300 px thumbnail: the same epsilon at a real
    // size (300 × (2480 / 300) = 2480.0000000000005).
    let page = draw_one_bit_mask_row(2480, 5, 300);
    let gray: Vec<u8> = page.rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
    assert!(gray[0] < 255, "the stroke is lost: {gray:?}");
}
