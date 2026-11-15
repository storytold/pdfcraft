//! A rectangular clip (`re W n`) whose edges fall between device pixels clips to every pixel it
//! reaches into (ISO 32000-2 §10.7.4: "the clipping region consists of the set of pixels that
//! would be included by a fill operation"), not to an anti-aliased fraction of its edge pixels
//! (vendored hayro patch 36), and `sh` paints the whole pixels of its clip box (vendored
//! hayro-interpret patch 114). A photo set as strips, each under its own clip, showed a light
//! line at every boundary (PDFBOX-956), and a shading clipped to a box filled underneath left
//! the box's colour as a frame (PDFBOX-833).

use super::rgb_at;
use crate::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

/// The page is 200 × 200 pt, drawn at 72 dpi: one pixel per point, rows counted from the top.
const SIDE: usize = 200;

/// A one-shade grey shading (0.93, sRGB 237) over the whole page, so what shows through its
/// edge pixels is what lies under it.
const FLAT_SHADING: &str = "/ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 200 0] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [0.93] /C1 [0.93] /N 1 >>";

/// Render the page `content` draws on white, with `resources` as its resource dictionary's
/// entries and `objects` (whole indirect objects from 5 0 obj on) added to the file.
fn render(content: &str, resources: &str, objects: &str) -> RenderedPage {
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << {resources} >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         {objects}trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    );
    let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (SIDE as u32, SIDE as u32));
    page
}

/// The colour of the pixel in column `x`, row `y` (from the top).
fn px(page: &RenderedPage, x: usize, y: usize) -> [u8; 3] {
    rgb_at(&page.rgba, SIDE, x, y)
}

/// One strip of a black picture, `y0` to `y0 + h` in points, columns 20 to 180, drawn under a
/// clip of its own rectangle; `image` is the image operator(s) that paint the unit square.
fn strip(y0: f64, h: f64, image: &str) -> String {
    format!("q 20 {y0:.3} 160 {h:.3} re W n 160 0 0 {h:.3} 20 {y0:.3} cm {image} Q ")
}

/// PDFBOX-956: a photo cut into strips, each drawn under its own clip, whose shared edges fall
/// between device pixels. The strips meet at y = 100.3 pt, row 99.7: the lower strip covers
/// 0.3 of row 99 and the upper one 0.7, and with both clips anti-aliased the row kept 0.21 of
/// the white page (grey 54). Both clips now take the whole row, so it is black. The pictures
/// are a magnified 1 × 1 inline image and a shrunk 400 × 100 one (snapped to whole pixels).
#[test]
fn abutting_clipped_image_strips_leave_no_seam() {
    let big = format!(
        "5 0 obj << /Type /XObject /Subtype /Image /Width 400 /Height 100 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 40000 >> stream\n{}\nendstream endobj\n",
        "\0".repeat(40_000)
    );
    for (name, image, resources, objects) in [
        ("1 x 1 inline image", "BI /W 1 /H 1 /BPC 8 /CS /G ID \0 EI", "", String::new()),
        ("400 x 100 image", "/Im0 Do", "/XObject << /Im0 5 0 R >>", big),
    ] {
        let content = format!("{}{}", strip(60.3, 40.0, image), strip(100.3, 40.2, image));
        let page = render(&content, resources, &objects);
        for y in 60..140 {
            assert_eq!(px(&page, 100, y), [0, 0, 0], "{name}: row {y} of the strips is black");
        }
        // Row 99.7, where the strips meet, is in that range. Outside the clips the page stays
        // white (row 59 is the shrunk picture's to paint or not: its top, 59.5, is snapped).
        assert_eq!(px(&page, 100, 58), [255; 3], "{name}: above the strips");
        assert_eq!(px(&page, 100, 140), [255; 3], "{name}: below the strips");
    }
}

/// PDFBOX-833: a box is filled blue, then the same rectangle is clipped and a shading painted
/// in it. The shading must cover the box's edge pixels as fully as the fill did: with the clip
/// (and `sh`'s own box) anti-aliased, the blue showed as a frame. Checked with no shading
/// `/BBox` and with one equal to the box, which is a clip of its own.
#[test]
fn a_shading_clipped_to_a_filled_box_covers_the_fill() {
    let rect = "50.3 100.3 99.4 49.6 re";
    for bbox in ["", "/BBox [50.3 100.3 149.7 149.9]"] {
        let content = format!("0.35 0.4 0.7 rg {rect} f q {rect} W n /Sh1 sh Q");
        let objects = format!("5 0 obj << {FLAT_SHADING} {bbox} >> endobj\n");
        let page = render(&content, "/Shading << /Sh1 5 0 R >>", &objects);
        // Columns 50.3–149.7 and rows 50.1–99.7: every pixel the box reaches into.
        for y in 50..100 {
            for x in 50..150 {
                let [r, g, b] = px(&page, x, y);
                assert!(
                    r == g && g == b && (235..=239).contains(&r),
                    "{bbox:?}: ({x}, {y}) is the shading's grey, not blended with the fill: {:?}",
                    [r, g, b]
                );
            }
        }
        assert_eq!(px(&page, 49, 75), [255; 3], "{bbox:?}: left of the box stays white");
        assert_eq!(px(&page, 150, 75), [255; 3], "{bbox:?}: right of the box stays white");
    }
}

/// A clip that is not an axis-aligned rectangle keeps its anti-aliased edge: there is no set of
/// whole pixels that matches a turned rectangle, so it is not snapped to its bounding box.
#[test]
fn a_turned_rectangular_clip_still_anti_aliases() {
    // A 120 × 60 rectangle turned by 15° about (80, 40), and a black picture over the page.
    let content = "q 0.9659 0.2588 -0.2588 0.9659 80 40 cm 0 0 120 60 re W n \
                   0.9659 -0.2588 0.2588 0.9659 -80 -40 cm 200 0 0 200 0 0 cm \
                   BI /W 1 /H 1 /BPC 8 /CS /G ID \0 EI Q";
    let page = render(content, "", "");
    let pixels = page.rgba.as_chunks::<4>().0;
    let partial = pixels.iter().filter(|p| p[0] > 0 && p[0] < 255).count();
    // The four edges are about 360 pixels long; nearly every pixel they cross is partly covered.
    assert!(partial > 250, "the turned clip's edges are anti-aliased: {partial} partial pixels");
    let black = pixels.iter().filter(|p| p[0] == 0).count();
    // The rectangle's area, 7,200 pt², less about half its edge pixels.
    assert!((6_700..7_200).contains(&black), "the clip is the turned rectangle, not its bounding box: {black} black pixels");
}

/// A clip edge within a 256th of a pixel of a pixel boundary counts as on it, so rounding noise
/// does not add a row; a clip thinner than a pixel still clips to the whole row it crosses.
#[test]
fn clip_rectangles_snap_outward_but_not_for_rounding_noise() {
    let image = "BI /W 1 /H 1 /BPC 8 /CS /G ID \0 EI";
    // The clip's top, 140 pt, lifted 0.0001 pt by the matrix: device row 59.9999.
    let content = format!("q 1 0 0 1 0 0.0001 cm 20 100 160 40 re W n 1 0 0 1 0 -0.0001 cm 200 0 0 200 0 0 cm {image} Q");
    let page = render(&content, "", "");
    assert_eq!(px(&page, 100, 59), [255; 3], "a ten-thousandth of a point above row 60 adds no row");
    assert_eq!(px(&page, 100, 60), [0; 3], "the clip's first row is wholly painted");
    assert_eq!(px(&page, 100, 99), [0; 3], "the clip's last row is wholly painted");
    assert_eq!(px(&page, 100, 100), [255; 3], "and the row below it is not");

    // 0.3 pt tall, rows 99.5–99.8: row 99, whole.
    let content = format!("q 20 100.2 160 0.3 re W n 200 0 0 200 0 0 cm {image} Q");
    let page = render(&content, "", "");
    assert_eq!(px(&page, 100, 99), [0; 3], "a clip thinner than a pixel clips to the whole row it crosses");
    assert_eq!(px(&page, 100, 98), [255; 3]);
    assert_eq!(px(&page, 100, 100), [255; 3]);
}

/// A clip path whose points sit on corners of its bounding box and whose lines run along the
/// axes, but that retraces itself instead of going round the box, encloses nothing, so it clips
/// everything away (ISO 32000-2 §10.7.4: the pixels a fill of it would include, and a fill of it
/// includes none under either rule). It is not taken for the rectangle of its bounding box,
/// which would paint a 90 × 90 pt square the clip excludes.
#[test]
fn a_retraced_clip_path_is_not_taken_for_its_bounding_box() {
    let image = "BI /W 1 /H 1 /BPC 8 /CS /G ID \0 EI";
    for (rule, path) in
        [("W", "10 10 m 100 10 l 10 10 l 10 100 l h"), ("W*", "10 10 m 100 10 l 10 10 l 10 100 l h"), ("W", "10 10 m 100 10 l 100 100 l 100 10 l h")]
    {
        let content = format!("q {path} {rule} n 200 0 0 200 0 0 cm {image} Q");
        let page = render(&content, "", "");
        let painted = page.rgba.as_chunks::<4>().0.iter().filter(|p| p[0] < 128).count();
        assert_eq!(painted, 0, "{path} {rule}: the clip keeps nothing, not its bounding box");
    }
}
