//! What drawing a large page costs, counted rather than timed (the counts are the same on every
//! machine). PDFBOX-5134's 4032 × 2025 plan paints 84 small mesh shadings with `sh`, and each was
//! painted over the whole canvas (hayro patch 33); PDFJS-9581's 8000 × 5333 CalRGB photo was
//! decoded to 128 million `f32`s and converted with six powers a pixel (hayro-interpret patch
//! 110). Both now draw the same pixels as before at a fraction of the cost.

use super::rgb_at;
use crate::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;
use std::sync::Arc;

/// A one-page PDF of `width` × `height` pt drawing `content`, with `resources` and the extra
/// objects `objects` (numbered from 5, each `(dictionary, stream data)`).
fn pdf(width: u32, height: u32, content: &str, resources: &str, objects: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << {resources} >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n",
        content.len()
    )
    .into_bytes();
    for (i, (dict, data)) in objects.iter().enumerate() {
        out.extend_from_slice(format!("{} 0 obj << {dict} /Length {} >> stream\n", i + 5, data.len()).as_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream endobj\n");
    }
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

fn render(pdf: Vec<u8>) -> crate::RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    page
}

/// Forty one-triangle meshes, each 10 pt across, painted with `sh` on a 2000 × 2000 pt page.
/// Each was painted over the whole canvas, its texture's transparent edge repeated across four
/// million pixels it leaves as they were; now each is painted over its own triangle and the
/// transparent pixel around it, and the triangles are where they were.
#[test]
fn small_mesh_shadings_paint_only_around_their_triangles() {
    const SIDE: u32 = 2000;
    const MESHES: u32 = 40;
    // A red triangle at (0, 0), (10, 0) and (0, 10): flag, x, y and colour for each vertex.
    let triangle = vec![0u8, 0, 0, 255, 0, 0, 0, 255, 0, 255, 0, 0, 0, 0, 255, 255, 0, 0];
    let shading = (
        "/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 \
         /Decode [0 10 0 10 0 1 0 1 0 1]"
            .to_string(),
        triangle,
    );
    let place = |i: u32| (100 + (i % 8) * 220, 100 + (i / 8) * 360);
    let content: String = (0..MESHES)
        .map(|i| {
            let (x, y) = place(i);
            format!("q 1 0 0 1 {x} {y} cm /Sh0 sh Q ")
        })
        .collect();

    let before = hayro::mesh_paint_pixels();
    let page = render(pdf(SIDE, SIDE, &content, "/Shading << /Sh0 5 0 R >>", &[shading]));
    let painted = hayro::mesh_paint_pixels() - before;

    // Each triangle's box is 10 × 10 pixels; with the transparent pixel around it and rounding
    // out to whole pixels, at most 14 × 14 (over the whole canvas, 40 × 4,000,000).
    assert!(painted <= u64::from(MESHES) * 14 * 14, "40 small meshes painted {painted} pixels");
    let width = SIDE as usize;
    for i in 0..MESHES {
        let (x, y) = place(i);
        // A point just inside the triangle by its right angle, and one well outside it (rows
        // are counted from the top).
        let (x, row) = (x as usize, (SIDE - y) as usize);
        assert_eq!(rgb_at(&page.rgba, width, x + 2, row - 3), [255, 0, 0], "mesh {i} is drawn");
        assert_eq!(rgb_at(&page.rgba, width, x + 30, row - 30), [255, 255, 255], "mesh {i} paints only its triangle");
    }
}

/// The size of the CalRGB test image, in samples (and of its page, in points).
const WIDTH: usize = 272;
const HEIGHT: usize = 64;

/// The CalRGB space of the samples below: a gamma, white point and black point other than the
/// defaults, so every step of the conversion is taken.
const CAL_RGB: &str = "[/CalRGB << /Gamma [1.8 2.2 2.6] /Matrix [0.4497 0.2446 0.0252 0.3163 0.672 0.1412 0.1845 0.0833 0.9227] \
                       /WhitePoint [0.9505 1 1.089] /BlackPoint [0.01 0.02 0.03] >>]";

/// The 8-bit RGB samples of a [`WIDTH`] × [`HEIGHT`] image: in each row, every value of every
/// component and many colours, then a run of one colour.
fn samples() -> Vec<u8> {
    let mut out = Vec::with_capacity(WIDTH * HEIGHT * 3);
    for y in 0..HEIGHT as u32 {
        for x in 0..WIDTH as u32 {
            let rgb = if x >= 256 { [200, 17, (y * 4) as u8] } else { [x as u8, ((x * 7 + y * 13) % 256) as u8, ((x * 31 + y * 5) % 256) as u8] };
            out.extend_from_slice(&rgb);
        }
    }
    out
}

/// A page drawing the [`WIDTH`] × [`HEIGHT`] CalRGB image of `data`, at `bpc` bits a sample, at one pixel a
/// sample.
fn cal_rgb_page(data: &[u8], bpc: u8) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    let image = (
        format!("/Type /XObject /Subtype /Image /Width {WIDTH} /Height {HEIGHT} /ColorSpace {CAL_RGB} /BitsPerComponent {bpc} /Filter /FlateDecode"),
        encoder.finish().unwrap(),
    );
    let content = format!("q {WIDTH} 0 0 {HEIGHT} 0 0 cm /Im0 Do Q");
    pdf(WIDTH as u32, HEIGHT as u32, &content, "/XObject << /Im0 5 0 R >>", &[image])
}

/// An 8-bit CalRGB image draws the colours it drew when each pixel was decoded to `f32`s and
/// converted on its own, without decoding a sample to an `f32`. The reference is the same image
/// at 16 bits, every sample `s` stored as `s × 257`: it still takes the per-pixel path, and
/// decodes to the very `f32`s (`s × 257 / 65535` and `s / 255` round alike, both being `s / 255`
/// exactly). The 8-bit path reads a component's power from a table and the sRGB byte off 255
/// thresholds; checked this way for every one of the 2^24 colours under three CalRGB spaces
/// when it was written.
#[test]
fn calrgb_8bit_images_convert_as_each_pixel_did() {
    let eight = samples();
    let sixteen: Vec<u8> = eight.iter().flat_map(|s| (u16::from(*s) * 257).to_be_bytes()).collect();

    let before = hayro::hayro_interpret::f32_image_samples();
    let fast = render(cal_rgb_page(&eight, 8));
    let decoded = hayro::hayro_interpret::f32_image_samples() - before;
    assert_eq!(decoded, 0, "an 8-bit CalRGB image decoded {decoded} samples to f32s");

    let before = hayro::hayro_interpret::f32_image_samples();
    let reference = render(cal_rgb_page(&sixteen, 16));
    let decoded = hayro::hayro_interpret::f32_image_samples() - before;
    assert_eq!(decoded, (WIDTH * HEIGHT * 3) as u64, "the 16-bit reference is decoded to f32s");

    let size = (WIDTH as u32, HEIGHT as u32);
    assert_eq!(((fast.width, fast.height), (reference.width, reference.height)), (size, size));
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            assert_eq!(rgb_at(&fast.rgba, WIDTH, x, y), rgb_at(&reference.rgba, WIDTH, x, y), "pixel ({x}, {y})");
        }
    }
}

/// An 8-bit CalRGB image whose data stops short of its last row, under a colour-key `/Mask`,
/// keeps the missing pixels transparent, as when each pixel was decoded to `f32`s: the key is
/// matched against the samples there are, and the missing pixels' alpha is padded transparent.
/// Matched against the zeros the 8-bit path pads the row with, they fell outside the key and
/// were drawn black. Here a 2 × 1 image holds one white pixel, keyed out, and the page's red
/// fill shows through both.
#[test]
fn a_short_colour_keyed_calrgb_row_keeps_its_missing_pixels_transparent() {
    let image = (
        format!("/Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace {CAL_RGB} /BitsPerComponent 8 /Mask [255 255 255 255 255 255]"),
        vec![255, 255, 255],
    );
    let content = "1 0 0 rg 0 0 20 10 re f q 20 0 0 10 0 0 cm /Im0 Do Q";
    let page = render(pdf(20, 10, content, "/XObject << /Im0 5 0 R >>", &[image]));
    assert_eq!(rgb_at(&page.rgba, 20, 5, 5), [255, 0, 0], "the keyed-out pixel shows the fill");
    assert_eq!(rgb_at(&page.rgba, 20, 15, 5), [255, 0, 0], "the missing pixel shows the fill");
}

/// A mesh pattern filled with `f*` in a knockout group keeps its even-odd hole. The mesh is
/// painted through its extent as a clip (hayro patch 33), which a shading in a knockout group
/// pushes with the non-zero rule; the path was then filled with that rule too, and the inner of
/// two rectangles drawn the same way round was painted over.
#[test]
fn an_even_odd_mesh_fill_in_a_knockout_group_keeps_its_hole() {
    // Two red triangles over the 100 × 100 square: flag, x, y and colour for each vertex.
    let vertex = |x: u8, y: u8| [0, x, y, 255, 0, 0];
    let mesh: Vec<u8> = [(0, 0), (255, 0), (0, 255), (255, 0), (255, 255), (0, 255)].iter().flat_map(|&(x, y)| vertex(x, y)).collect();
    let shading = (
        "/ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 /Decode [0 100 0 100 0 1 0 1 0 1]".to_string(),
        mesh,
    );
    let group = "/Pattern cs /P0 scn 10 10 80 80 re 30 30 40 40 re f*";
    let form = (
        "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Group << /S /Transparency /K true >> \
         /Resources << /Pattern << /P0 << /PatternType 2 /Shading 5 0 R >> >> >>"
            .to_string(),
        group.as_bytes().to_vec(),
    );
    let page = render(pdf(100, 100, "/Fm Do", "/XObject << /Fm 6 0 R >>", &[shading, form]));
    assert_eq!(rgb_at(&page.rgba, 100, 20, 50), [255, 0, 0], "the ring between the rectangles is painted");
    assert_eq!(rgb_at(&page.rgba, 100, 50, 50), [255, 255, 255], "the inner rectangle is a hole");
    assert_eq!(rgb_at(&page.rgba, 100, 5, 50), [255, 255, 255], "outside the outer rectangle is not painted");
}
