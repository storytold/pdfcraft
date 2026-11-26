//! Contributor-original synthetic pages (#5530): a shrunk image on a page turned by `/Rotate` is
//! drawn exactly as on the upright page, turned. The crop box and scale put every page edge on a
//! whole device pixel, so a turn is a pure permutation of pixels and the turned render, turned
//! back, must match the upright one. The image itself sits at fractional coordinates: an upright
//! image there is snapped to whole pixels and shrunk once, and before the fix a turned one was
//! not, so it was shrunk to a rounded-up size and resampled a second time at a fractional scale
//! and offset (most of its pixels off, by up to 217 levels).
//!
//! Every kind of image that reaches the snap is drawn: an 8-bit and a 1-bit grey image, a stencil
//! mask (a CCITT scan is often one), and images with a soft mask of their own size and of another
//! size. The last needs no `/Interpolate`: a shrunk soft mask of another size is filtered, and so
//! snapped, whatever the image asks (hayro patch 15). The other kinds without `/Interpolate` are
//! not snapped upright or turned, and were already the same both ways; they stay in the test as a
//! guard.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

/// A PDF of the given objects (object 1 the catalog), with a cross-reference table.
fn pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut bytes = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(b"\nendstream");
    bytes
}

/// Image size in samples.
const W: usize = 400;
const H: usize = 500;

/// A busy grey pattern with edges both ways, as a scan has.
fn grey_samples() -> Vec<u8> {
    (0..H).flat_map(|y| (0..W).map(move |x| ((x * 7 + y * 13) % 256) as u8 ^ if (x / 3 + y / 5) % 2 == 0 { 0 } else { 0x80 })).collect()
}

/// One bit a sample, rows padded to whole bytes: a text-like pattern of thin strokes.
fn bit_samples() -> Vec<u8> {
    let row = W.div_ceil(8);
    let mut out = vec![0u8; row * H];
    for y in 0..H {
        for x in 0..W {
            if (x / 2 + y / 3) % 3 == 0 || (x * 5 + y * 3) % 11 < 3 {
                out[y * row + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    out
}

fn alpha_samples(w: usize, h: usize) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).map(move |x| ((x * 3 + y * 5) % 256) as u8)).collect()
}

/// The kinds of image drawn.
const KINDS: [&str; 5] = ["8-bit grey", "1-bit grey", "stencil mask", "soft mask", "smaller soft mask"];

/// The image of `kind` as objects 5 (and 6, its soft mask).
fn image(kind: &str, interpolate: bool) -> Vec<Vec<u8>> {
    let image = |depth: u8, extra: &str, data: &[u8]| {
        let dict = format!(
            "/Type /XObject /Subtype /Image /Width {W} /Height {H} /ColorSpace /DeviceGray /BitsPerComponent {depth} /Interpolate {interpolate}{extra}"
        );
        stream(&dict, data)
    };
    let mask = |w: usize, h: usize| {
        let dict =
            format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate {interpolate}");
        stream(&dict, &alpha_samples(w, h))
    };
    match kind {
        "8-bit grey" => vec![image(8, "", &grey_samples())],
        "1-bit grey" => vec![image(1, "", &bit_samples())],
        "stencil mask" => vec![stream(
            &format!("/Type /XObject /Subtype /Image /Width {W} /Height {H} /ImageMask true /BitsPerComponent 1 /Interpolate {interpolate}"),
            &bit_samples(),
        )],
        "soft mask" => vec![image(8, " /SMask 6 0 R", &grey_samples()), mask(W, H)],
        _ => vec![image(8, " /SMask 6 0 R", &grey_samples()), mask(300, 350)],
    }
}

/// A page whose crop box sits inside a larger media box, as in the report, with the image drawn
/// at fractional coordinates inside it, in a colour (a stencil mask paints it).
fn page(rotate: u32, kind: &str, interpolate: bool) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 794 1224] /CropBox [114 510 680 1210] /Rotate {rotate} /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >>"
        )
        .into_bytes(),
        stream("", b"0.1 0.2 0.6 rg q 500.3 0 0 600.7 150.15 560.45 cm /Im Do Q"),
    ];
    objects.extend(image(kind, interpolate));
    pdf(&objects)
}

/// At half a pixel per point every crop-box edge, upright or turned, is a whole device pixel,
/// and the 400 × 500 image is shrunk to about 250 × 300 pixels.
fn render(rotate: u32, kind: &str, interpolate: bool) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(page(rotate, kind, interpolate)), RenderConfig::default());
    let out = renderer.render(RenderRequest { page: 0, scale: 0.5, ..Default::default() });
    assert!(out.error.is_none(), "{kind}: {:?}", out.error);
    out
}

fn rgb(out: &RenderedPage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * out.width + x) * 4) as usize;
    [out.rgba[i], out.rgba[i + 1], out.rgba[i + 2]]
}

/// The largest channel difference between the turned render and the upright one turned the same
/// way.
fn turned_difference(rotate: u32, kind: &str, interpolate: bool) -> u8 {
    let upright = render(0, kind, interpolate);
    let turned = render(rotate, kind, interpolate);
    let (w, h) = (upright.width, upright.height);
    // `/Rotate` turns the page clockwise.
    let expected = |x: u32, y: u32| match rotate {
        90 => rgb(&upright, y, h - 1 - x),
        180 => rgb(&upright, w - 1 - x, h - 1 - y),
        _ => rgb(&upright, w - 1 - y, x),
    };
    let (tw, th) = if rotate == 180 { (w, h) } else { (h, w) };
    assert_eq!((turned.width, turned.height), (tw, th), "{kind}, /Rotate {rotate}");
    let mut worst = 0;
    for y in 0..th {
        for x in 0..tw {
            let (got, want) = (rgb(&turned, x, y), expected(x, y));
            worst = (0..3).map(|c| got[c].abs_diff(want[c])).fold(worst, u8::max);
        }
    }
    worst
}

#[test]
fn a_shrunk_image_on_a_turned_page_is_the_upright_render_turned() {
    for kind in KINDS {
        for interpolate in [true, false] {
            for rotate in [90, 180, 270] {
                let worst = turned_difference(rotate, kind, interpolate);
                assert!(worst <= 1, "{kind}, /Rotate {rotate}, /Interpolate {interpolate}: off by up to {worst} levels");
            }
        }
    }
}
