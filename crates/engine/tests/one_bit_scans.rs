//! 1-bit black-and-white scans become PDF pages with the right polarity: black ink stays dark and
//! paper stays light, whatever the file's convention (BMP palette order, TIFF photometric
//! interpretation, PNG gray or palette), orientation (top-down or bottom-up) and row padding.
//!
//! The images are synthetic (no scanner, no fake device). They live in this crate because it can
//! reach both `pdfcraft-create` and `pdfcraft-render`.

use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_create::{ImageResolution, from_images_with_resolution};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// Image size: 13 is neither a multiple of 8 nor of 32, so every row has padding bits and bytes.
const W: u32 = 13;
const H: u32 = 5;
const SCALE: f32 = 8.0;

/// The known pattern: a black bar in columns 3..=6 of the two top rows. Everything else is paper.
fn is_black(x: u32, y: u32) -> bool {
    (3..=6).contains(&x) && y < 2
}

/// Rows are listed top to bottom; each row is packed MSB first and padded to `stride` bytes.
/// `set` says whether a pixel's bit is 1.
fn pack_rows(stride: usize, set: impl Fn(u32, u32) -> bool) -> Vec<Vec<u8>> {
    (0..H)
        .map(|y| {
            let mut row = vec![0u8; stride];
            for x in 0..W {
                if set(x, y) {
                    row[(x / 8) as usize] |= 0x80 >> (x % 8);
                }
            }
            row
        })
        .collect()
}

/// A 1-bpp BMP. `black_first`: the palette is [black, white] (black is index 0), else [white, black].
fn bmp(black_first: bool, top_down: bool) -> Vec<u8> {
    let stride = (W as usize).div_ceil(32) * 4;
    let rows = pack_rows(stride, |x, y| is_black(x, y) != black_first);
    let palette: [[u8; 4]; 2] = if black_first { [[0, 0, 0, 0], [255, 255, 255, 0]] } else { [[255, 255, 255, 0], [0, 0, 0, 0]] };
    let pixels: Vec<u8> = if top_down { rows.concat() } else { rows.iter().rev().flatten().copied().collect() };
    let pixel_at = 14 + 40 + 8;
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((pixel_at + pixels.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(pixel_at as u32).to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(W as i32).to_le_bytes());
    let height = if top_down { -(H as i32) } else { H as i32 };
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // BI_RGB
    out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 8]); // pixels per metre
    out.extend_from_slice(&2u32.to_le_bytes()); // colours used
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&palette.concat());
    out.extend_from_slice(&pixels);
    out
}

/// A little-endian 1-bit TIFF. `min_is_white`: PhotometricInterpretation 0 (0 bits are white, so
/// black pixels are 1 bits), else 1 (0 bits are black). `compression` 1 is uncompressed; other
/// values need `strip` to carry the (already encoded) data.
fn tiff(min_is_white: bool, compression: u16, strip: Option<Vec<u8>>) -> Vec<u8> {
    let stride = (W as usize).div_ceil(8);
    let data = strip.unwrap_or_else(|| pack_rows(stride, |x, y| is_black(x, y) == min_is_white).concat());
    let entries: [(u16, u16, u32); 9] = [
        (256, 4, W),
        (257, 4, H),
        (258, 3, 1),
        (259, 3, u32::from(compression)),
        (262, 3, u32::from(!min_is_white)),
        (273, 4, 8 + 2 + 9 * 12 + 4),
        (277, 3, 1),
        (278, 4, H),
        (279, 4, data.len() as u32),
    ];
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, value) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// A 1-bit PNG: "gray" (0 = black), or indexed with the palette [black, white] / [white, black].
fn png_bytes(kind: &str) -> Vec<u8> {
    let stride = (W as usize).div_ceil(8);
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, W, H);
        enc.set_depth(png::BitDepth::One);
        let bit_set: Box<dyn Fn(u32, u32) -> bool> = match kind {
            "gray" => {
                enc.set_color(png::ColorType::Grayscale);
                Box::new(|x, y| !is_black(x, y))
            }
            "palette-black-first" => {
                enc.set_color(png::ColorType::Indexed);
                enc.set_palette(vec![0, 0, 0, 255, 255, 255]);
                Box::new(|x, y| !is_black(x, y))
            }
            _ => {
                enc.set_color(png::ColorType::Indexed);
                enc.set_palette(vec![255, 255, 255, 0, 0, 0]);
                Box::new(is_black)
            }
        };
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&pack_rows(stride, bit_set).concat()).unwrap();
    }
    out
}

type Raster = (u32, u32, Vec<u8>);

/// Render every page of the images' PDF at SCALE.
fn render(images: &[(&str, Vec<u8>)]) -> Vec<Raster> {
    let list: Vec<(String, Vec<u8>)> = images.iter().map(|(n, b)| (n.to_string(), b.clone())).collect();
    let doc = from_images_with_resolution(&list, ImageResolution::Dpi(72.0)).unwrap();
    let pdf = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    (0..images.len())
        .map(|page| {
            let p = renderer.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: SCALE, tag: 0, preview: false });
            assert!(p.error.is_none(), "{:?}", p.error);
            (p.width, p.height, p.rgba.to_vec())
        })
        .collect()
}

/// Red channel at the centre of source pixel (x, y).
fn at(page: &Raster, x: u32, y: u32) -> u8 {
    let (w, h, rgba) = page;
    let (px, py) = ((x as f32 * SCALE + SCALE / 2.0) as u32, (y as f32 * SCALE + SCALE / 2.0) as u32);
    assert!(px < *w && py < *h);
    rgba[((py * w + px) * 4) as usize]
}

/// Every source pixel must render dark where the pattern is black and light elsewhere. That covers
/// polarity, vertical flips and stride errors (the 13th column and every row).
fn assert_pattern(label: &str, page: &Raster) {
    assert_eq!((page.0, page.1), (W * SCALE as u32, H * SCALE as u32), "{label}: page size");
    for y in 0..H {
        for x in 0..W {
            let v = at(page, x, y);
            if is_black(x, y) {
                assert!(v < 60, "{label}: pixel ({x},{y}) should be black, luma {v}");
            } else {
                assert!(v > 200, "{label}: pixel ({x},{y}) should be paper, luma {v}");
            }
        }
    }
}

#[test]
fn bmp_1bpp_keeps_polarity_for_both_palette_orders_and_row_orders() {
    for black_first in [true, false] {
        for top_down in [true, false] {
            let label = format!("bmp black_first={black_first} top_down={top_down}");
            let pages = render(&[("scan.bmp", bmp(black_first, top_down))]);
            assert_pattern(&label, &pages[0]);
        }
    }
}

#[test]
fn tiff_1bit_keeps_polarity_for_min_is_black_and_min_is_white() {
    for min_is_white in [true, false] {
        let pages = render(&[("scan.tif", tiff(min_is_white, 1, None))]);
        assert_pattern(&format!("tiff min_is_white={min_is_white}"), &pages[0]);
    }
}

#[test]
fn png_1bit_keeps_polarity_for_gray_and_both_palette_orders() {
    for kind in ["gray", "palette-black-first", "palette-white-first"] {
        let pages = render(&[("scan.png", png_bytes(kind))]);
        assert_pattern(&format!("png {kind}"), &pages[0]);
    }
}

#[test]
fn mixed_conventions_in_one_document_agree_page_by_page() {
    // The "one page inverted" scenario: consecutive pages whose files use opposite conventions.
    let pages = render(&[
        ("1.bmp", bmp(true, false)),
        ("2.bmp", bmp(false, false)),
        ("3.tif", tiff(true, 1, None)),
        ("4.tif", tiff(false, 1, None)),
        ("5.png", png_bytes("gray")),
        ("6.png", png_bytes("palette-white-first")),
        ("7.bmp", bmp(false, true)),
    ]);
    assert_eq!(pages.len(), 7);
    for (i, page) in pages.iter().enumerate() {
        assert_pattern(&format!("page {}", i + 1), page);
    }
}

#[test]
fn a_ccitt_tiff_is_decoded_or_refused_never_a_panic() {
    // Not a valid G4 stream (hand-encoding one is out of scope): an error or a page, never a crash.
    let result = std::panic::catch_unwind(|| {
        let list = vec![("fax.tif".to_string(), tiff(true, 4, Some(vec![0x00, 0x10, 0x01])))];
        from_images_with_resolution(&list, ImageResolution::Dpi(72.0)).map(|_| ())
    });
    assert!(result.is_ok(), "CCITT TIFF input panicked");
}

/// A 1-bit file must not grow eightfold: the BMP is embedded as 1-bit DeviceGray, like a 1-bit
/// TIFF, and the PDF stays smaller than the same scan as 8-bit gray.
#[test]
fn one_bit_bmp_stays_one_bit_in_the_pdf() {
    let list = vec![("scan.bmp".to_string(), bmp(true, false))];
    let doc = from_images_with_resolution(&list, ImageResolution::Dpi(72.0)).unwrap();
    let pdf = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/BitsPerComponent 1"), "1-bit BMP was expanded");
    assert!(!text.contains("/BitsPerComponent 8"), "an 8-bit image is left in the PDF");
    assert!(text.contains("/DeviceGray"));
    // The unpacked 8-bit scan would be W*H = 65 bytes before compression; the packed one is
    // ceil(W/8)*H = 10.
    let packed = (W as usize).div_ceil(8) * H as usize;
    let image_len = text
        .split("/Subtype /Image")
        .nth(1)
        .and_then(|s| s.split("/Length ").nth(1))
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse::<usize>().ok());
    assert!(image_len.is_some_and(|n| n < (W * H) as usize), "image stream is {image_len:?} bytes, packed data is {packed}");
}
