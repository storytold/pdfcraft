//! #7250: a JPEG 2000 image with `/SMaskInData 2` has an opacity channel the colours were
//! premultiplied by (ISO 32000-2 Table 87, T.800 Table I.16). It is the image's soft mask, and the
//! colours are divided by it again (vendored hayro-interpret patch 107). It used to be dropped, so
//! a transparent pixel painted its premultiplied colour, black, over the page.

use super::{jpx_midpoint_pdf, rgb_at};
use crate::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

/// A 40 × 40 pt page painted blue, with `jpx_midpoint_pdf`'s 1 × 1 image (every sample 128, the
/// last component its opacity) of `components` components over its middle, `keys` in the image
/// dictionary.
fn page(components: u16, keys: &str) -> [u8; 3] {
    page_with_soft_mask(components, keys, None)
}

/// [`page`], with an external `/SMask` of one pixel at `external_opacity` and a zero `/Matte` when
/// given.
fn page_with_soft_mask(components: u16, keys: &str, external_opacity: Option<u8>) -> [u8; 3] {
    let raw = jpx_midpoint_pdf(8, components);
    let start = raw.windows(4).position(|w| w == [0xff, 0x4f, 0xff, 0x51]).unwrap();
    let end = raw.windows(2).rposition(|w| w == [0xff, 0xd9]).unwrap() + 2;
    let codestream = &raw[start..end];
    let content = "0 0 1 rg 0 0 40 40 re f q 20 0 0 20 10 10 cm /Im1 Do Q";
    let soft_mask = if external_opacity.is_some() { " /SMask 6 0 R" } else { "" };
    let mut pdf = format!(
        "%PDF-1.7\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >> endobj\n\
4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
5 0 obj << /Type /XObject /Subtype /Image /Width 1 /Height 1 {keys}{soft_mask} /Filter /JPXDecode /Length {} >> stream\n",
        content.len(),
        codestream.len()
    )
    .into_bytes();
    pdf.extend_from_slice(codestream);
    pdf.extend_from_slice(b"\nendstream endobj\n");
    if let Some(opacity) = external_opacity {
        pdf.extend_from_slice(b"6 0 obj << /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Matte [0] /Length 1 >> stream\n");
        pdf.push(opacity);
        pdf.extend_from_slice(b"\nendstream endobj\n");
    }
    pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{keys}: {:?}", page.error);
    assert_eq!((page.width, page.height), (40, 40));
    rgb_at(&page.rgba, 40, 20, 20)
}

fn close(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2)
}

/// Review on #7292: with `/SMaskInData` nonzero an image's `/SMask` shall be ignored (ISO 32000-2
/// Table 87), and for `/SMaskInData 2` the embedded opacity is the soft mask. A same-size `/SMask`
/// with a `/Matte` used to be applied first, dividing the already recovered colours a second time
/// and taking the external mask's opacity instead of the embedded one.
#[test]
fn a_premultiplied_image_ignores_its_external_soft_mask() {
    // The embedded opacity is 128 and the external soft mask's 64. Drawn through the embedded
    // opacity, the recovered white is half opaque over the blue: 128, 128, 255. Through the
    // external mask instead it would be a quarter opaque: 64, 64, 255.
    let actual = page_with_soft_mask(2, "/ColorSpace /DeviceGray /SMaskInData 2", Some(64));
    assert!(close(actual, [128, 128, 255]), "{actual:?}");
}

#[test]
fn a_jpx_image_with_premultiplied_opacity_is_drawn_through_it() {
    for (components, colour_space) in [(4, "/ColorSpace /DeviceRGB"), (2, "/ColorSpace /DeviceGray")] {
        // Premultiplied: colour 128 at opacity 128/255 is white at half opacity over the blue.
        let premultiplied = page(components, &format!("{colour_space} /SMaskInData 2"));
        assert!(close(premultiplied, [128, 128, 255]), "{colour_space}, /SMaskInData 2: {premultiplied:?}");
        // Unchanged: straight opacity is grey 128 at half opacity, and without /SMaskInData the
        // opacity channel is not used.
        let straight = page(components, &format!("{colour_space} /SMaskInData 1"));
        assert!(close(straight, [64, 64, 191]), "{colour_space}, /SMaskInData 1: {straight:?}");
        let opaque = page(components, colour_space);
        assert!(close(opaque, [128, 128, 128]), "{colour_space}, no /SMaskInData: {opaque:?}");
    }
}

/// Review on #7292: the colours are recovered in their own colour space, before `/Decode` and the
/// colour conversion, which are not linear. Dividing the converted RGB instead made a partly
/// transparent CalGray pixel too bright.
#[test]
fn a_premultiplied_colour_is_recovered_before_colour_conversion() {
    // Sample 128 at opacity 128 is white premultiplied. `/Decode [0 0.25]` makes the recovered
    // sample 0.25, which CalGray's gamma-1 conversion draws as grey 146, so half opacity over the
    // blue is 73 and 200. Divided in RGB after conversion instead, the premultiplied 0.125 became
    // 107 and came back as 213: 107 and 234.
    let keys = "/ColorSpace [/CalGray << /WhitePoint [1 1 1] /Gamma 1 >>] /Decode [0 0.25] /SMaskInData 2";
    let actual = page(2, keys);
    assert!(close(actual, [73, 73, 200]), "{actual:?}");
}
