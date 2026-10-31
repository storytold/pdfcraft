//! A colour-key `/Mask` is tested against an image's raw samples (ISO 32000-2 §8.9.6.4), also
//! when its `/Decode` is inverted (vendored hayro-interpret patch 69). The 8-bit fast path
//! inverts such an image's data in place, and the key was tested against the inverted bytes, so
//! it masked the wrong samples: a gray image with `/Decode [1 0]` and `/Mask [0 0]` hid its raw
//! 255 rows and drew its raw 0 rows white, where Chrome hides the 0 rows and draws the rest black.

use super::{image_cache_pdf, render_first_page, rgb_at};

/// The width in pixels of the 130 × 70 pt page `image_cache_pdf` makes, rendered at 72 dpi.
const WIDTH: usize = 130;

/// A blue square, then image `/Im` over it, 20 pt square with its top at y 40 of the rendered
/// page: each of its four rows is 5 px tall, and a masked-out pixel shows the blue.
const DRAW: &str = "0 0 1 rg 10 10 20 20 re f q 20 0 0 20 10 10 cm /Im Do Q";

/// The middle of each of the image's rows, and of its column.
const ROW_Y: [usize; 4] = [42, 47, 52, 57];
const X: usize = 20;

/// What each of the image's four rows shows when `image` (dictionary, data) is drawn.
fn rows(image: &str, data: &[u8]) -> [&'static str; 4] {
    let page = render_first_page(image_cache_pdf(DRAW, "<< /XObject << /Im 5 0 R >> >>", &[(image, data)]));
    ROW_Y.map(|y| match rgb_at(&page.rgba, WIDTH, X, y) {
        [r, g, b] if r < 64 && g < 64 && b > 192 => "masked",
        rgb if rgb.iter().all(|&c| c < 64) => "black",
        rgb if rgb.iter().all(|&c| c > 192) => "white",
        _ => "other",
    })
}

/// A 1 × 4, 8-bit image in `space` with `extra` in its dictionary.
fn image(space: &str, extra: &str, length: usize) -> String {
    format!("<< /Type /XObject /Subtype /Image /Width 1 /Height 4 /ColorSpace {space} /BitsPerComponent 8 {extra}/Length {length} >>")
}

/// Under `/Decode [1 0]` the key hides the raw 0 rows, and the raw 255 rows decode to black.
#[test]
fn a_colour_key_keys_the_raw_samples_of_an_inverted_gray_image() {
    let gray = image("/DeviceGray", "/Decode [1 0] /Mask [0 0] ", 4);
    assert_eq!(rows(&gray, &[0x00, 0xff, 0x00, 0xff]), ["masked", "black", "masked", "black"]);
}

/// The same for RGB, whose fast path keeps the inverted bytes in the decoded data rather than
/// in the image.
#[test]
fn a_colour_key_keys_the_raw_samples_of_an_inverted_rgb_image() {
    let rgb = image("/DeviceRGB", "/Decode [1 0 1 0 1 0] /Mask [0 0 0 0 0 0] ", 12);
    let data = [[0x00; 3], [0xff; 3], [0x00; 3], [0xff; 3]].concat();
    assert_eq!(rows(&rgb, &data), ["masked", "black", "masked", "black"]);
}

/// With the default `/Decode` nothing is inverted, and the key keeps hiding the raw 0 rows.
#[test]
fn a_colour_key_on_an_image_with_the_default_decode_is_unchanged() {
    let gray = image("/DeviceGray", "/Mask [0 0] ", 4);
    assert_eq!(rows(&gray, &[0x00, 0xff, 0x00, 0xff]), ["masked", "white", "masked", "white"]);
}
