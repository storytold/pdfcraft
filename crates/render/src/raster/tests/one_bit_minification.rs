//! Issues #1700, #3687, #6654, #7936: a 1-bit source shrunk by between 1:1 and 2:1 was drawn by picking single source
//! pixels, so one-pixel strokes (Type 3 image-mask glyphs, fax-era and OCR fonts) vanished. A
//! 1-bit source is line art: it is area-averaged over the source each target pixel covers, so a
//! stroke the picking grid misses keeps its coverage. `/ImageMask` and `/BitsPerComponent 1`
//! sources and a Type 3 glyph's image mask all take that path, and so does a 1-bit `/SMask`.

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

/// Floating-point epsilon: the far end of the last target pixel's box can sit a hair above the
/// source width — 22 × (25 / 22) = 25.000000000000004 — and an averaging that indexed the box
/// directly read the row and column past the source, found nothing and dropped the whole image
/// back to nearest picking, which missed the stroke. Both pairs must stay averaged.
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

/// An 8-bit colour image under a same-size 1-bit `/SMask`: the mask is line art, so a minified
/// draw averages it instead of picking the source sample under each target pixel's centre. The
/// mask's one-pixel stroke in column 1 of 10 covers 0.4 of device pixel 0 and must stay visible.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_one_bit_soft_mask_on_a_minified_colour_image_keeps_its_stroke() {
    // Black 10×1 picture; the mask is opaque (sample 1) only in column 1 of 10.
    let black = [0u8; 30];
    let mask = [0x40u8, 0x00];
    let page = draw_minified_image("DeviceRGB", (10, 1), None, &black, Some((&mask, 1)), (6, 1));
    let px = page.rgba.as_chunks::<4>().0;
    // 40% of black over the white page: a mid-tone, not the page showing through.
    assert!((64..=191).contains(&px[0][0]), "the one-pixel stroke is lost: {:?}", &px[..2]);
    assert_eq!(px[5], [255, 255, 255, 255], "the far end stays white");
}

/// A Type 3 image-mask glyph shrunk 10:6: the charproc paints a 10×1 `/ImageMask` with its only
/// stroke in column 1. Nearest picking reads column 0 for the first device pixel and drops the
/// stroke; the 1-bit source is area-averaged instead, so 0.4 of that pixel is inked.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_one_bit_image_mask_glyph_keeps_its_stroke_when_minified() {
    // Sample 0 paints (the mask's default decode), so column 1 is the stroke.
    let data = hex(&[0xBFu8, 0xFF]);
    let content = "BT /F1 1 Tf 0 0 Td (A) Tj ET";
    let glyph = "10 0 d0 q 6 0 0 1 0 0 cm /Im0 Do Q";
    let pdf = format!(
        "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 6 1] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 6 1] /FontMatrix [1 0 0 1 0 0] /FirstChar 65 /LastChar 65 /Widths [6] /Encoding << /Differences [65 /a] >> /CharProcs << /a 6 0 R >> /Resources << /XObject << /Im0 7 0 R >> >> >> endobj\n\
         6 0 obj << /Length {} >> stream\n{glyph}\nendstream endobj\n\
         7 0 obj << /Type /XObject /Subtype /Image /Width 10 /Height 1 /ImageMask true /Filter /ASCIIHexDecode /Length {} >> stream\n{data}>\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF\n",
        content.len(),
        glyph.len(),
        data.len() + 1
    );
    let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (6, 1));
    let gray: Vec<u8> = page.rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
    assert!((64..=191).contains(&gray[0]), "the one-pixel stroke is lost: {gray:?}");
    assert_eq!(gray[5], 255, "the far end stays white: {gray:?}");
}

/// A CCITT-encoded stencil-mask glyph keeps the 1-bit path: a stencil mask is 1-bit whatever a
/// filter reports (ISO 32000-2 §8.9.6.2, Table 87: `/BitsPerComponent` shall be 1 when
/// `/ImageMask` is true), but the CCITT filter expands a non-indexed image to 8-bit grey rows and
/// reports 8, which used to send the glyph to the nearest picking of #624 — where the stroke
/// vanished (the fax-era fonts of PDFBOX-2573 and friends stayed broken).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_ccitt_stencil_mask_glyph_keeps_its_stroke_when_minified() {
    // The same 10×1 row as the ASCIIHex test above — white 1, black 1 (the stroke), white 8 —
    // coded one-dimensionally with T.4's terminating codes: 000111 010 10011, two bytes with
    // two fill bits.
    const STROKE_10X1: [u8; 2] = [0x1D, 0x4C];
    let content = "BT /F1 1 Tf 0 0 Td (A) Tj ET";
    let glyph = "10 0 d0 q 6 0 0 1 0 0 cm /Im0 Do Q";
    let mut pdf = format!(
        "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 6 1] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 6 1] /FontMatrix [1 0 0 1 0 0] /FirstChar 65 /LastChar 65 /Widths [6] /Encoding << /Differences [65 /a] >> /CharProcs << /a 6 0 R >> /Resources << /XObject << /Im0 7 0 R >> >> >> endobj\n\
         6 0 obj << /Length {} >> stream\n{glyph}\nendstream endobj\n\
         7 0 obj << /Type /XObject /Subtype /Image /Width 10 /Height 1 /ImageMask true /Filter /CCITTFaxDecode /DecodeParms << /K 0 /Columns 10 /Rows 1 >> /Length {} >> stream\n",
        content.len(),
        glyph.len(),
        STROKE_10X1.len()
    )
    .into_bytes();
    pdf.extend_from_slice(&STROKE_10X1);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n");
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (6, 1));
    let gray: Vec<u8> = page.rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
    assert!((64..=191).contains(&gray[0]), "the one-pixel stroke is lost: {gray:?}");
    assert_eq!(gray[5], 255, "the far end stays white: {gray:?}");
}

/// Issue #1469, PR #7946: Type 3 `/ImageMask` glyphs rendered under `0 Tr` (fill) and `2 Tr` (fill + stroke)
/// at 150 dpi must remain visually equivalent (ink ratio between 0.90 and 1.10).
/// At 10 pt (and fractional device placement), nearest-neighbor sampling of area-averaged
/// pixels preserves line-art stroke coverage without extraneous bilinear interpolation
/// disrupting ink parity across integer and fractional positions.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn type3_imagemask_tr2_parity_at_150_dpi() {
    let proc_l = b"10 0 0 0 10 14 d1\n8 0 0 14 1 0 cm\nBI\n/W 8\n/H 14\n/BPC 1\n/Decode [1 0]\n/IM true\nID \x00\x00\x10\x10\x10\x10\x10\x10\x10\x10\x10\x10\x10\x00\nEI";
    let proc_h = b"10 0 0 0 10 14 d1\n8 0 0 14 1 0 cm\nBI\n/W 8\n/H 14\n/BPC 1\n/Decode [1 0]\n/IM true\nID \x00\x00BBBB~BBBBBB\x00\nEI";
    let proc_o = b"10 0 0 0 10 14 d1\n8 0 0 14 1 0 cm\nBI\n/W 8\n/H 14\n/BPC 1\n/Decode [1 0]\n/IM true\nID \x00\x00\x00\x00\x00<BBBBB<\x00\x00\nEI";

    let content = "\
0 g 0 G\n\
BT 0 Tr /F1 10 Tf 1 0 0 1 260 780 Tm (HolloHlloHoll) Tj 0 Tr ET\n\
BT 2 Tr /F1 10 Tf 1 0 0 1 260 747.0 Tm (HolloHlloHoll) Tj 0 Tr ET\n";

    let mut pdf = format!(
        "%PDF-1.7\n\
1 0 obj << /Length {} >> stream\n",
        proc_l.len()
    )
    .into_bytes();
    pdf.extend_from_slice(proc_l);
    pdf.extend_from_slice(
        format!(
            "\nendstream endobj\n\
2 0 obj << /Length {} >> stream\n",
            proc_h.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(proc_h);
    pdf.extend_from_slice(
        format!(
            "\nendstream endobj\n\
3 0 obj << /Length {} >> stream\n",
            proc_o.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(proc_o);
    pdf.extend_from_slice(
        format!(
            "\nendstream endobj\n\
4 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 10 14] /FontMatrix [0.03 0 0 0.03 0 0] \
/CharProcs << /l 1 0 R /H 2 0 R /o 3 0 R >> /Encoding << /Type /Encoding /Differences [72 /H 108 /l 111 /o] >> \
/FirstChar 72 /LastChar 111 /Widths [10 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 10 0 0 10] \
/Resources << >> >> endobj\n\
5 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
6 0 obj << /Type /Page /Parent 7 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >> endobj\n\
7 0 obj << /Type /Pages /Kids [6 0 R] /Count 1 >> endobj\n\
8 0 obj << /Type /Catalog /Pages 7 0 R >> endobj\n\
trailer << /Size 9 /Root 8 0 R >>\n%%EOF\n",
            content.len()
        )
        .as_bytes(),
    );

    let scale = 150.0 / 72.0;
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);

    let count_ink = |y_base: f64| {
        let scale = scale as f64;
        let x_min = (250.0 * scale) as u32;
        let x_max = (550.0 * scale) as u32;
        let y_top = f64::max(0.0, (842.0 - (y_base + 10.0 * 1.5)) * scale) as u32;
        let y_bot = f64::min(page.height as f64, (842.0 - (y_base - 10.0 * 0.5)) * scale) as u32;
        let mut dark = 0;
        for y in y_top..y_bot {
            for x in x_min..x_max {
                let idx = ((y * page.width + x) * 4) as usize;
                let gray = (page.rgba[idx] as u32 + page.rgba[idx + 1] as u32 + page.rgba[idx + 2] as u32) / 3;
                if gray < 128 {
                    dark += 1;
                }
            }
        }
        dark
    };

    let dark_0_tr = count_ink(780.0);
    let dark_2_tr = count_ink(747.0);
    assert!(dark_0_tr > 0, "0 Tr line must paint ink");
    assert!(dark_2_tr > 0, "2 Tr line must paint ink");

    let ratio = dark_2_tr as f64 / dark_0_tr as f64;
    assert!(
        (0.90..=1.10).contains(&ratio),
        "ink ratio 2 Tr / 0 Tr at 10 pt must be between 0.90 and 1.10, got {ratio:.4} (0 Tr: {dark_0_tr}, 2 Tr: {dark_2_tr})"
    );
}
