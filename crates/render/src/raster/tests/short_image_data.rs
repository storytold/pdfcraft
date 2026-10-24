//! An image or soft mask whose data stops short of its `/Height` keeps that height (#2780,
//! vendored hayro-interpret patch 67): the rows present are drawn where they belong, and the
//! missing rows are padded as 1-, 2- and 4-bit samples already were. 8-bit data was cut to the
//! rows present, which were then stretched over the whole image, so an image no longer lined up
//! with its `/SMask`. A scanned page's colour layer drew its first rows across the page under a
//! text mask that kept its full height, and the text came out as a faint ghost.

use super::{image_cache_pdf, render_first_page, rgb_at};

/// The width in pixels of the 130 × 70 pt page `image_cache_pdf` makes, rendered at 72 dpi.
const WIDTH: usize = 130;

/// Draws image `/Im` 20 pt square with its top at y 40 of the rendered page, so each of its
/// four rows is 5 px tall: row 0 spans y 40 to 45, row 1 45 to 50, and so on.
const DRAW: &str = "q 20 0 0 20 10 10 cm /Im Do Q";

/// The middle of each of the image's rows, and of its column.
const ROW_Y: [usize; 4] = [42, 47, 52, 57];
const X: usize = 20;

/// The shade of each of the image's four rows when `objects` (5 is `/Im`) are drawn.
fn rows(objects: &[(&str, &[u8])]) -> [&'static str; 4] {
    let page = render_first_page(image_cache_pdf(DRAW, "<< /XObject << /Im 5 0 R >> >>", objects));
    ROW_Y.map(|y| match rgb_at(&page.rgba, WIDTH, X, y) {
        rgb if rgb.iter().all(|&c| c < 64) => "black",
        rgb if rgb.iter().all(|&c| c > 192) => "white",
        _ => "grey",
    })
}

/// A 1 × 4, 8-bit DeviceGray image or mask, `extra` added to its dictionary.
fn gray(extra: &str, length: usize) -> String {
    format!("<< /Type /XObject /Subtype /Image /Width 1 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 {extra}/Length {length} >>")
}

/// The first two rows of an image, black then white: its data stops there.
const SHORT: [u8; 2] = [0x00, 0xff];

/// An image whose data holds only its first two rows draws them as its first two rows and its
/// last two black (sample 0). They were stretched over all four.
#[test]
fn a_short_image_draws_its_rows_where_they_belong() {
    let image = gray("", 2);
    let objects: [(&str, &[u8]); 1] = [(&image, &SHORT)];
    assert_eq!(rows(&objects), ["black", "white", "black", "black"]);
}

/// The case of #2780: under a soft mask of the full height, a short image's rows meet the mask's
/// rows they belong to.
#[test]
fn a_short_image_lines_up_with_its_soft_mask() {
    let (image, mask) = (gray("/SMask 6 0 R ", 2), gray("", 4));
    let objects: [(&str, &[u8]); 2] = [(&image, &SHORT), (&mask, &[0xff; 4])];
    assert_eq!(rows(&objects), ["black", "white", "black", "black"]);
}

/// The same through the colour table a spot-colour image is converted with (patch 28), which
/// follows the same length rule. The tint transform draws tint t as gray t, so sample 0 is black.
#[test]
fn a_short_spot_colour_image_draws_its_rows_where_they_belong() {
    // The table is used when the image has more pixels than its 256 entries: the two rows
    // present (400 pixels) are enough on their own.
    let image = "<< /Type /XObject /Subtype /Image /Width 200 /Height 4 /ColorSpace [/Separation /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>] /BitsPerComponent 8 /Length 400 >>";
    let data = [[0x00; 200], [0xff; 200]].concat();
    let objects: [(&str, &[u8]); 1] = [(image, &data)];
    assert_eq!(rows(&objects), ["black", "white", "black", "black"]);
}

/// A colour-key `/Mask` keys a short image's padded samples like its own: a key range without
/// sample 0 leaves them opaque. The key was tested on the samples present only, and the padded
/// pixels were masked out. A spot-colour image (tint t drawn as gray t) takes the path that keys
/// the decoded samples rather than the padded image data.
#[test]
fn a_colour_key_keys_a_short_images_padded_samples() {
    let image = "<< /Type /XObject /Subtype /Image /Width 1 /Height 4 /ColorSpace [/Separation /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>] /BitsPerComponent 8 /Mask [200 255] /Length 2 >>";
    // Black, then white, which the key masks out.
    let objects: [(&str, &[u8]); 1] = [(image, &SHORT)];
    assert_eq!(rows(&objects), ["black", "white", "black", "black"]);
}

/// A soft mask whose data holds only its first two rows, opaque then clear, masks the first two
/// rows of its image and leaves the rest clear (alpha 0). The opaque row was stretched over the
/// top half.
#[test]
fn a_short_soft_mask_masks_only_the_rows_it_has() {
    let (image, mask) = (gray("/SMask 6 0 R ", 4), gray("", 2));
    let objects: [(&str, &[u8]); 2] = [(&image, &[0x00; 4]), (&mask, &[0xff, 0x00])];
    assert_eq!(rows(&objects), ["black", "white", "white", "white"]);
}

/// An image with no data at all is still left out, not drawn as a block of padding.
#[test]
fn an_image_with_no_data_is_not_drawn() {
    let image = gray("", 0);
    let objects: [(&str, &[u8]); 1] = [(&image, &[])];
    assert_eq!(rows(&objects), ["white"; 4]);
}
