//! A stroke's width is not floored in user space before the one-device-pixel minimum (vendored
//! hayro patch 35). ISO 32000-2 §8.4.3.2 gives the width in user space and makes 0 the thinnest
//! line the device can draw; the renderer took `max(0.01)` first, so under a large matrix a zero
//! or tiny width drew many pixels wide (PDFBOX-1227: 0.0012 under a 543× form matrix, about 6 pt).

use super::*;

/// How much ink a black stroke leaves, per pixel, row by row: 0 for white, 1 for black.
fn ink_page(content: &str) -> (usize, Vec<f32>) {
    let pdf = one_page_pdf(content.as_bytes(), "");
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "{content}: {:?}", page.error);
    let ink = page.rgba.as_chunks::<4>().0.iter().map(|px| f32::from(255 - px[0]) / 255.0).collect();
    (page.width as usize, ink)
}

/// The ink in column `x`: how thick, in device pixels, a horizontal line crossing it is.
fn thickness_across(content: &str, x: usize) -> f32 {
    let (width, ink) = ink_page(content);
    ink.iter().skip(x).step_by(width).sum()
}

/// The ink in row `y`: how wide, in device pixels, a vertical line crossing it is.
fn thickness_along(content: &str, y: usize) -> f32 {
    let (width, ink) = ink_page(content);
    ink.iter().skip(y * width).take(width).sum()
}

fn near(got: f32, want: f32) -> bool {
    (got - want).abs() <= 0.15 + want * 0.05
}

/// Widths around the old floor, at a scale where it was 0.01 × 400 = 4 px: four below it, one
/// above.
const WIDTHS: [f32; 5] = [0.0, 0.001, 0.005, 0.0099, 0.02];

/// Under `400 0 0 400 cm` a width of `w` is `400 w` device pixels, one at least: 1, 1, 2, 3.96
/// and 8. The user-space floor drew the first four 4 px thick.
#[test]
fn a_thin_line_under_a_large_matrix_is_as_wide_as_its_width() {
    for w in WIDTHS {
        // Device y = 400 × 0.05 = 20, from x = 4 to 36.
        let got = thickness_across(&format!("q 400 0 0 400 0 0 cm {w} w 0.01 0.05 m 0.09 0.05 l S Q"), 20);
        let want = (w * 400.0).max(1.0);
        assert!(near(got, want), "width {w}: {got} px thick, want {want}");
    }
}

/// A width of 0 is one device pixel thick however large the matrix is, and is drawn.
#[test]
fn a_zero_width_line_is_one_device_pixel() {
    for scale in [1.0, 40.0, 400.0, 4000.0, 40000.0] {
        let y = 20.0 / scale;
        let (x0, x1) = (4.0 / scale, 36.0 / scale);
        let got = thickness_across(&format!("q {scale} 0 0 {scale} 0 0 cm 0 w {x0} {y} m {x1} {y} l S Q"), 20);
        assert!(near(got, 1.0), "scale {scale}: a zero-width line is {got} px thick");
    }
}

/// Under `160 0 0 400 cm` (the report's synthetic page) a horizontal line is `400 w` pixels
/// thick and a vertical one `160 w` wide, each from the device minimum up. The minimum is taken
/// along the direction the matrix shrinks most, so a vertical line is at least one pixel wide,
/// and a horizontal one at least 400 / 160 = 2.5 pixels thick: the same width in user space.
#[test]
fn a_thin_line_under_a_squeezed_matrix_scales_with_its_width() {
    for w in WIDTHS {
        let got = thickness_across(&format!("q 160 0 0 400 0 0 cm {w} w 0.025 0.05 m 0.225 0.05 l S Q"), 20);
        let want = (w * 400.0).max(2.5);
        assert!(near(got, want), "horizontal, width {w}: {got} px thick, want {want}");
    }
    // Device x = 160 × 0.125 = 20, from y = 4 to 36.
    let along = |w: f32| thickness_along(&format!("q 160 0 0 400 0 0 cm {w} w 0.125 0.01 m 0.125 0.09 l S Q"), 20);
    for w in WIDTHS {
        let (got, want) = (along(w), (w * 160.0).max(1.0));
        assert!(near(got, want), "vertical, width {w}: {got} px wide, want {want}");
    }
}

/// ISO 32000-2 §8.4.3.2: a width of 0 is "the thinnest line that can be rendered at device
/// resolution: 1 device pixel wide", whichever way the line runs. Under `400 0 0 100 cm` a
/// minimum taken along the larger scale alone drew a horizontal zero-width line
/// (1 / 400) × 100 = 0.25 pixels thick, so faint it could vanish; it is one pixel thick, and a
/// vertical one is as wide as the same user-space width is on that axis, 4 pixels.
#[test]
fn a_zero_width_line_is_one_pixel_across_under_an_uneven_matrix() {
    // Device y = 100 × 0.2 = 20, from x = 4 to 36.
    let got = thickness_across("q 400 0 0 100 0 0 cm 0 w 0.01 0.2 m 0.09 0.2 l S Q", 20);
    assert!(near(got, 1.0), "horizontal: {got} px thick, want 1");
    // Device x = 400 × 0.05 = 20, from y = 4 to 36.
    let got = thickness_along("q 400 0 0 100 0 0 cm 0 w 0.05 0.04 m 0.05 0.36 l S Q", 20);
    assert!(near(got, 4.0), "vertical: {got} px wide, want 4");
}

/// Widths at and above the old floor's reach draw as before: one pixel up to a pixel, the width
/// above it, with and without a matrix.
#[test]
fn ordinary_widths_are_unchanged() {
    for (content, want) in [
        ("0 w 4 20 m 36 20 l S", 1.0),
        ("0.3 w 4 20 m 36 20 l S", 1.0),
        ("1 w 4 20 m 36 20 l S", 1.0),
        ("3 w 4 20 m 36 20 l S", 3.0),
        ("6 w 4 20 m 36 20 l S", 6.0),
        ("q 0.5 0 0 0.5 0 0 cm 4 w 8 40 m 72 40 l S Q", 2.0),
        ("q 10 0 0 10 0 0 cm 0.01 w 0.4 2 m 3.6 2 l S Q", 1.0),
        ("q 400 0 0 400 0 0 cm 0.05 w 0.01 0.05 m 0.09 0.05 l S Q", 20.0),
    ] {
        let got = thickness_across(content, 20);
        assert!(near(got, want), "{content}: {got} px thick, want {want}");
    }
}

/// Matrices that collapse, shrink past what `f32` can invert or aren't finite, with widths of
/// 0, below 0 and huge: each page renders, and a zero width under an ordinary matrix after them
/// is still a pixel.
#[test]
fn degenerate_matrices_and_widths_render() {
    // PDF numbers have no exponents: Rust prints these in full.
    let (tiny, tinier, huge) = (1e-30_f32, 1e-45_f32, 1e38_f32);
    for content in [
        "q 0 0 0 0 0 0 cm 0 w 0 0 m 1 1 l S Q".to_string(),
        "q 0 0 0 1 0 0 cm 0 w 0 0 m 1 1 l S Q".to_string(),
        format!("q {tiny} 0 0 {tiny} 0 0 cm 0 w 0 0 m {huge} {huge} l S Q"),
        format!("q {tinier} 0 0 {tinier} 0 0 cm 0 w 0 0 m 1 1 l S Q"),
        format!("q {huge} 0 0 {huge} 0 0 cm 0 w 0 0 m 1 1 l S Q"),
        "q 400 0 0 400 0 0 cm -1 w 0.01 0.05 m 0.09 0.05 l S Q".to_string(),
        format!("q 400 0 0 400 0 0 cm {huge} w 0.01 0.05 m 0.09 0.05 l S Q"),
    ] {
        let (_, ink) = ink_page(&content);
        assert!(ink.iter().all(|v| v.is_finite()), "{content}");
    }
    let got = thickness_across(&format!("q {tiny} 0 0 {tiny} 0 0 cm Q 0 w 4 20 m 36 20 l S"), 20);
    assert!(near(got, 1.0), "after them a zero-width line is {got} px thick");
    let got = thickness_across("q 400 0 0 400 0 0 cm -1 w 0.01 0.05 m 0.09 0.05 l S Q", 20);
    assert!(near(got, 1.0), "a negative width is drawn as 0: {got} px thick");
}
