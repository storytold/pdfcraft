//! Flate images with TIFF predictor 2 (ISO 32000-2 §7.4.4.4, Table 10) at 8 and 16 bits per
//! component are undone a byte row at a time (vendored hayro-syntax patch 42), with the samples
//! the bit-level decoder gave.

use super::rgb_at;
use crate::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;
use std::sync::Arc;

/// The page is 150 × 200 pt, drawn at 72 dpi; a 3 × 4 image fills it.
const WIDTH: usize = 150;
const HEIGHT: usize = 200;

/// Renders a 3 × 4 image whose samples are `rows` (each row's bytes, before prediction),
/// differenced by TIFF predictor 2 with `pixel_len`-byte pixels of `sample_len`-byte samples.
fn render(rows: &[Vec<u8>], colour_space: &str, colors: usize, bpc: usize) -> Vec<u8> {
    let (sample_len, pixel_len) = (bpc / 8, colors * bpc / 8);
    let mut predicted = Vec::new();
    for row in rows {
        let mut out = row.clone();
        for i in (pixel_len..row.len()).step_by(sample_len).rev() {
            if sample_len == 2 {
                let cur = u16::from_be_bytes([row[i], row[i + 1]]);
                let left = u16::from_be_bytes([row[i - pixel_len], row[i - pixel_len + 1]]);
                out[i..i + 2].copy_from_slice(&cur.wrapping_sub(left).to_be_bytes());
            } else {
                out[i] = row[i].wrapping_sub(row[i - pixel_len]);
            }
        }
        predicted.extend(out);
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
         5 0 obj << /Type /XObject /Subtype /Image /Width 3 /Height 4 /ColorSpace /{colour_space} /BitsPerComponent {bpc} \
         /Filter /FlateDecode /DecodeParms << /Predictor 2 /Colors {colors} /BitsPerComponent {bpc} /Columns 3 >> /Length {} >> stream\n",
        content.len(),
        data.len()
    )
    .into_bytes();
    pdf.extend_from_slice(&data);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF");

    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (WIDTH as u32, HEIGHT as u32));
    page.rgba.to_vec()
}

#[test]
fn an_eight_bit_rgb_image_with_tiff_predicted_rows_draws_its_colours() {
    // Each pixel differs from the one to its left, and some differences wrap past 255.
    let pixels: [[[u8; 3]; 3]; 4] = [
        [[255, 0, 0], [0, 255, 0], [0, 0, 255]],
        [[10, 200, 30], [250, 5, 128], [0, 0, 0]],
        [[255, 255, 255], [1, 2, 3], [128, 64, 32]],
        [[0, 128, 255], [255, 128, 0], [64, 64, 64]],
    ];
    let rows: Vec<Vec<u8>> = pixels.iter().map(|row| row.iter().flatten().copied().collect()).collect();
    let rgba = render(&rows, "DeviceRGB", 3, 8);
    for (row, colours) in pixels.iter().enumerate() {
        for (column, rgb) in colours.iter().enumerate() {
            let (x, y) = (column * 50 + 25, row * 50 + 25);
            assert_eq!(rgb_at(&rgba, WIDTH, x, y), *rgb, "image row {row}, column {column}");
        }
    }
}

#[test]
fn a_sixteen_bit_grey_image_with_tiff_predicted_rows_draws_its_levels() {
    // The high byte decides the level drawn; the differences carry across the byte boundary.
    let levels: [[u16; 3]; 4] = [[0xffff, 0x00ff, 0x8000], [0x0100, 0xff00, 0x4080], [0, 0x7fff, 0xc000], [0x2000, 0x2000, 0xe0ff]];
    let rows: Vec<Vec<u8>> = levels.iter().map(|row| row.iter().flat_map(|v| v.to_be_bytes()).collect()).collect();
    let rgba = render(&rows, "DeviceGray", 1, 16);
    for (row, values) in levels.iter().enumerate() {
        for (column, value) in values.iter().enumerate() {
            let (x, y) = (column * 50 + 25, row * 50 + 25);
            let [r, g, b] = rgb_at(&rgba, WIDTH, x, y);
            let expected = i32::from(value >> 8);
            for got in [r, g, b] {
                assert!((i32::from(got) - expected).abs() <= 1, "image row {row}, column {column}: {got} for {value:#06x}");
            }
        }
    }
}
