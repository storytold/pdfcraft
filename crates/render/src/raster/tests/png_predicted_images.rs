//! #5430: Flate images with PNG predictors (ISO 32000-2 §7.4.4.4) are undone a byte row at a
//! time (vendored hayro-syntax patch 35). The bit-level decoder they went through before dropped
//! the last byte of each filtered row that ends part-way through a pixel, so every later row
//! started a byte early and the picture came out sheared. An image whose rows carry no filter-type
//! byte at all, although it declares a PNG predictor, is read as its samples (patch 40).

use super::rgb_at;
use crate::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;
use std::sync::Arc;

/// The page is 150 × 200 pt, drawn at 72 dpi; the image fills it.
const WIDTH: usize = 150;
const HEIGHT: usize = 200;

/// The colour of each of the image's four rows, top first, as 4-bit samples.
const ROWS: [[u8; 3]; 4] = [[15, 0, 0], [0, 15, 0], [0, 0, 15], [15, 15, 0]];

#[test]
fn a_four_bit_rgb_image_of_odd_width_with_up_predicted_rows_is_not_sheared() {
    // Three 4-bit RGB pixels are 4.5 bytes, so each row is five bytes and ends half-way through a
    // two-byte pixel.
    let rows: Vec<Vec<u8>> = ROWS
        .iter()
        .map(|rgb| {
            let nibbles: Vec<u8> = (0..3).flat_map(|_| rgb.iter().copied()).chain([0]).collect();
            nibbles.chunks(2).map(|pair| (pair[0] << 4) | pair[1]).collect()
        })
        .collect();
    let mut predicted = Vec::new();
    let mut above = vec![0u8; 5];
    for row in &rows {
        predicted.push(2);
        predicted.extend(row.iter().zip(&above).map(|(b, a)| b.wrapping_sub(*a)));
        above.clone_from(row);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&predicted).unwrap();
    let data = encoder.finish().unwrap();

    let content = "q 150 0 0 200 0 0 cm /Im0 Do Q";
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 150 200] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /XObject /Subtype /Image /Width 3 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 4 \
         /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 3 /BitsPerComponent 4 /Columns 3 >> /Length {} >> stream\n",
        content.len(),
        data.len()
    )
    .into_bytes();
    pdf.extend_from_slice(&data);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF");

    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page =
        renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false, magnification: None });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (WIDTH as u32, HEIGHT as u32));

    // Each image pixel is 50 pt square; look at the middle of each.
    for (row, rgb) in ROWS.iter().enumerate() {
        let expected = rgb.map(|sample| sample * 17);
        for column in 0..3 {
            let (x, y) = (column * 50 + 25, row * 50 + 25);
            assert_eq!(rgb_at(&page.rgba, WIDTH, x, y), expected, "image row {row}, column {column}");
        }
    }
}

/// PDFBOX-3932: a writer declared `/Predictor 15` for an image and its soft mask without applying
/// it, so their rows carry no PNG filter-type byte (vendored hayro-syntax patch 40). Read with the
/// predictor, each row's first sample was taken for a filter type and the image came out a row
/// short and sheared; PDFium, MuPDF and Poppler draw the samples as they are.
#[test]
fn an_image_and_soft_mask_whose_rows_carry_no_png_tags_draw_their_samples() {
    // Four rows of three 8-bit RGB pixels, every sample above 4 (no byte reads as a filter type
    // that would happen to undo cleanly), and an opaque mask of three grey samples a row.
    let rgb: Vec<u8> = ROWS.iter().flat_map(|px| (0..3).flat_map(move |_| px.map(|s| s * 16 + 8))).collect();
    let mask = [255u8; 12];
    let pdf = image_pdf(&[("/ColorSpace /DeviceRGB /SMask 6 0 R", "/Colors 3", &rgb), ("/ColorSpace /DeviceGray", "/Colors 1", &mask)]);
    let page = render(pdf);
    for (row, px) in ROWS.iter().enumerate() {
        let expected = px.map(|s| s * 16 + 8);
        for column in 0..3 {
            let (x, y) = (column * 50 + 25, row * 50 + 25);
            assert_eq!(rgb_at(&page.rgba, WIDTH, x, y), expected, "image row {row}, column {column}");
        }
    }
}

/// Data one byte short of rows without tags is neither layout: it is decoded with the predictor
/// as before, and does not panic.
#[test]
fn an_image_one_byte_short_of_untagged_rows_still_goes_through_the_predictor() {
    let rgb = [0u8; 35];
    let page = render(image_pdf(&[("/ColorSpace /DeviceRGB", "/Colors 3", &rgb)]));
    assert_eq!((page.width, page.height), (WIDTH as u32, HEIGHT as u32));
}

/// Image objects 5, 6, … of a 3 × 4 8-bit image each, Flate-compressed with `/Predictor 15`
/// declared and their `data` stored as given; the page draws object 5 over all of itself.
fn image_pdf(images: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let content = "q 150 0 0 200 0 0 cm /Im0 Do Q";
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 150 200] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n",
        content.len()
    )
    .into_bytes();
    for (i, (keys, colors, data)) in images.iter().enumerate() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        let data = encoder.finish().unwrap();
        pdf.extend_from_slice(
            format!(
                "{} 0 obj << /Type /XObject /Subtype /Image /Width 3 /Height 4 {keys} /BitsPerComponent 8 \
                 /Filter /FlateDecode /DecodeParms << /Predictor 15 {colors} /BitsPerComponent 8 /Columns 3 >> /Length {} >> stream\n",
                5 + i,
                data.len()
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(&data);
        pdf.extend_from_slice(b"\nendstream endobj\n");
    }
    pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    pdf
}

fn render(pdf: Vec<u8>) -> crate::RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page =
        renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false, magnification: None });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (WIDTH as u32, HEIGHT as u32));
    page
}
