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
    let raw = jpx_midpoint_pdf(8, components);
    let start = raw.windows(4).position(|w| w == [0xff, 0x4f, 0xff, 0x51]).unwrap();
    let end = raw.windows(2).rposition(|w| w == [0xff, 0xd9]).unwrap() + 2;
    let codestream = &raw[start..end];
    let content = "0 0 1 rg 0 0 40 40 re f q 20 0 0 20 10 10 cm /Im1 Do Q";
    let mut pdf = format!(
        "%PDF-1.7\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >> endobj\n\
4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
5 0 obj << /Type /XObject /Subtype /Image /Width 1 /Height 1 {keys} /Filter /JPXDecode /Length {} >> stream\n",
        content.len(),
        codestream.len()
    )
    .into_bytes();
    pdf.extend_from_slice(codestream);
    pdf.extend_from_slice(b"\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF");
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{keys}: {:?}", page.error);
    assert_eq!((page.width, page.height), (40, 40));
    rgb_at(&page.rgba, 40, 20, 20)
}

fn close(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2)
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
