//! A soft mask's `/Matte` is removed in the image's own colour space, before the colours are
//! converted (ISO 32000-1 §11.6.5.3): #7251.

use crate::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

/// A page with two 80-point squares on blue, each a 1 × 1 image in `space` with a soft mask
/// of `mask`: on the left `straight` with no `/Matte`, on the right `preblended` with
/// `/Matte [matte]` and the extra image entries `extra` (a `/Decode`, say).
fn render(space: &str, straight: u8, preblended: u8, mask: u8, matte: &str, extra: &str) -> RenderedPage {
    let image = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8";
    let content = "0 0 1 rg 0 0 240 120 re f q 80 0 0 80 20 20 cm /Straight Do Q q 80 0 0 80 140 20 cm /Matted Do Q";
    let stream = |n: u32, dict: String, data: &[u8]| {
        let mut out = format!("{n} 0 obj << {dict} /Length {} >> stream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream endobj\n");
        out
    };
    let mut pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 240 120] /Resources << /XObject << /Straight 4 0 R /Matted 6 0 R >> >> /Contents 8 0 R >> endobj
"
    .to_vec();
    let n = straight_len(space);
    pdf.extend(stream(4, format!("{image} /ColorSpace {space} /SMask 5 0 R"), &vec![straight; n]));
    pdf.extend(stream(5, format!("{image} /ColorSpace /DeviceGray"), &[mask]));
    pdf.extend(stream(6, format!("{image} /ColorSpace {space} {extra} /SMask 7 0 R"), &vec![preblended; n]));
    pdf.extend(stream(7, format!("{image} /ColorSpace /DeviceGray /Matte [{matte}]"), &[mask]));
    pdf.extend(stream(8, String::new(), content.as_bytes()));
    pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(p.error.is_none(), "{space}: {:?}", p.error);
    p
}

/// Components per pixel of the test spaces.
fn straight_len(space: &str) -> usize {
    if space.contains("RGB") { 3 } else { 1 }
}

/// The left (straight) and right (preblended) squares' colours.
fn squares(p: &RenderedPage) -> (Vec<u8>, Vec<u8>) {
    let px = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4) as usize..][..3].to_vec();
    (px(50, 60), px(190, 60))
}

fn close(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 2)
}

/// The issue's fixture: CalGray with `/Gamma 4`, white at half alpha on the left, and on the
/// right the same white preblended with a black matte, 128 = 0 + 128/255 × (255 − 0). Both
/// are the same colour; removing the matte after the gamma drew the right one darker
/// (77,77,204 where 128,128,255 is right).
#[test]
fn a_matte_is_removed_before_a_calgray_image_is_converted() {
    let space = "[/CalGray << /WhitePoint [.9505 1 1.089] /Gamma 4 >>]";
    let (left, right) = squares(&render(space, 255, 128, 128, "0", ""));
    assert!(close(&left, &[128, 128, 255]), "straight: {left:?}");
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// The issue's second control: CalGray's default gamma and a white matte,
/// 127 = 255 + 128/255 × (0 − 255), so the right square is black at half alpha too.
#[test]
fn a_white_matte_is_removed_before_a_linear_calgray_image_is_converted() {
    let space = "[/CalGray << /WhitePoint [.9505 1 1.089] >>]";
    let (left, right) = squares(&render(space, 0, 127, 128, "1", ""));
    assert!(close(&left, &[0, 0, 127]), "straight: {left:?}");
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// The matte is removed from the decoded components, after `/Decode`: with `[1 0]` the byte
/// 127 is the component 128/255, white preblended with black at half alpha.
#[test]
fn a_matte_is_removed_after_the_decode_array() {
    let space = "[/CalGray << /WhitePoint [.9505 1 1.089] /Gamma 4 >>]";
    let (left, right) = squares(&render(space, 255, 127, 128, "0", "/Decode [1 0]"));
    assert!(close(&right, &left), "preblended {right:?}, straight {left:?}");
}

/// Device spaces, whose conversion is no conversion, draw as they did.
#[test]
fn a_matte_in_a_device_space_is_removed_as_before() {
    for (space, straight, preblended, matte) in [("/DeviceGray", 255, 128, "0"), ("/DeviceRGB", 255, 128, "0 0 0"), ("/DeviceRGB", 0, 127, "1 1 1")] {
        let (left, right) = squares(&render(space, straight, preblended, 128, matte, ""));
        assert!(close(&right, &left), "{space}: preblended {right:?}, straight {left:?}");
    }
}
