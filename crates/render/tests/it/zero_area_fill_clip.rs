//! Zero-area fills and clips mark the pixels they touch (ISO 32000-2 §10.7.4: "A zero-width or
//! zero-height rectangle paints a line 1 pixel wide", and "the clipping region consists of the
//! set of pixels that would be included by a fill operation").
//!
//! Which row (or column) a line on a pixel boundary marks is not left open. §10.7.4, "Scan
//! conversion rules", in device space:
//!
//! > Pixel boundaries always fall on integer coordinates in device space. A pixel is a square
//! > region identified by the location of its corner with minimum horizontal and vertical
//! > coordinates. The region is half-open, meaning that it includes its lower but not its upper
//! > boundaries. More precisely, for any point whose real-number coordinates are (x, y), let
//! > i = floor(x) and j = floor(y). The pixel that contains this point is the one identified as
//! > (i, j).
//!
//! > Like pixels, shapes to be painted by filling (8.5.3.3, "Filling") and stroking (8.5.3.2,
//! > "Stroking") operations are also treated as half-open regions that include the boundaries
//! > along their "floor" sides, but not along their "ceiling" sides.
//!
//! > A shape shall be scan-converted by painting any pixel whose half-open square region
//! > intersects the shape, no matter how small the intersection is.
//!
//! So a line at device y = 150 lies in pixel row floor(150) = 150 and not in row 149, whose
//! region stops short of 150; a line from device x = 10 to x = 110 marks columns 10 to 109, as
//! its ceiling end at 110 is not part of it. Off a boundary, a line at y = 149.5 marks row
//! floor(149.5) = 149. The rule is stated in device space, whose y axis here points down the
//! page, so row 150 is the one just below the line as the page is seen.
//!
//! Every page is 200 × 200 pt rendered at scale 1, so user `(x, y)` lands on device
//! `(x, 200 - y)`: a line at user `y = 50` lies on the boundary between device rows 149 and
//! 150, and by the half-open rule marks row 150.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

const SIZE: u32 = 200;

fn build_pdf(content: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {SIZE} {SIZE}] /Contents 4 0 R >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let size = objs.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for off in offs {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

/// The page drawn by `content`, as RGBA rows of `SIZE` pixels.
fn render(content: &str) -> Vec<u8> {
    let pdf = Arc::new(build_pdf(content));
    let mut renderer = PageRenderer::new(pdf, RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(page.error.is_none(), "render error: {:?}", page.error);
    assert_eq!((page.width, page.height), (SIZE, SIZE));
    page.rgba.to_vec()
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * SIZE + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

fn is_red(p: [u8; 3]) -> bool {
    p[0] > 240 && p[1] < 15 && p[2] < 15
}

fn is_white(p: [u8; 3]) -> bool {
    p.iter().all(|&c| c > 250)
}

/// The red pixels of the page, as `(x, y)`.
fn red_pixels(rgba: &[u8]) -> Vec<(u32, u32)> {
    let mut red = Vec::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            if is_red(px(rgba, x, y)) {
                red.push((x, y));
            }
        }
    }
    red
}

/// The pixels that are neither white nor full red: partly covered ones.
fn tinted_pixels(rgba: &[u8]) -> usize {
    let mut n = 0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = px(rgba, x, y);
            if !is_red(p) && !is_white(p) {
                n += 1;
            }
        }
    }
    n
}

/// Asserts the page is exactly a full-red run of device pixels `x0..x1` × `y0..y1` on white.
fn assert_only_red_run(rgba: &[u8], (x0, x1): (u32, u32), (y0, y1): (u32, u32), what: &str) {
    let mut expected = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            expected.push((x, y));
        }
    }
    assert_eq!(red_pixels(rgba), expected, "{what}: the red pixels");
    assert_eq!(tinted_pixels(rgba), 0, "{what}: partly covered pixels");
}

const FLOOD: &str = "1 0 0 rg 0 0 200 200 re f";
/// A fill that covers the whole page under any of the CTMs below.
const BIG_FLOOD: &str = "1 0 0 rg -400 -400 800 800 re f";

#[test]
fn zero_height_clip_shows_the_fill_as_one_device_row() {
    // User y = 50 is device y = 150, a pixel boundary: row 150 and nothing else.
    let rgba = render(&format!("q 10 50 100 0 re W n {FLOOD} Q"));
    assert_only_red_run(&rgba, (10, 110), (150, 151), "clip on a pixel boundary");
    // Off the boundary (device y = 149.5) the line marks the row it runs through.
    let rgba = render(&format!("q 10 50.5 100 0 re W n {FLOOD} Q"));
    assert_only_red_run(&rgba, (10, 110), (149, 150), "clip inside a row");
    // Inside a translated CTM, as the line is usually drawn.
    let rgba = render(&format!("q 1 0 0 1 20 30 cm 10 10 100 0 re W n {FLOOD} Q"));
    assert_only_red_run(&rgba, (30, 130), (160, 161), "clip under a translation");
}

#[test]
fn zero_width_clip_shows_the_fill_as_one_device_column() {
    let rgba = render(&format!("q 50 10 0 100 re W n {FLOOD} Q"));
    assert_only_red_run(&rgba, (50, 51), (90, 190), "zero-width clip");
}

#[test]
fn zero_height_and_zero_width_fills_paint_one_pixel_lines() {
    let rgba = render("1 0 0 rg 10 50 100 0 re f");
    assert_only_red_run(&rgba, (10, 110), (150, 151), "zero-height fill");
    let rgba = render("1 0 0 rg 50 10 0 100 re f");
    assert_only_red_run(&rgba, (50, 51), (90, 190), "zero-width fill");
    // The same line drawn as a fill and seen through a clip marks the same pixels.
    let fill = render("1 0 0 rg 10 50.5 100 0 re f");
    let clip = render(&format!("q 10 50.5 100 0 re W n {FLOOD} Q"));
    assert_eq!(red_pixels(&fill), red_pixels(&clip));
}

/// Each of the four cases (zero-height and zero-width, fill and clip) on a pixel boundary and
/// off one, by the half-open rule quoted above: a line on a boundary marks the row or column on
/// its floor side, one off it marks the row or column it runs through, and its ends mark every
/// pixel they reach into but not one they only meet at the ceiling side.
#[test]
fn zero_height_and_zero_width_fills_and_clips_on_and_off_a_pixel_boundary() {
    let cases = [
        // (the line, its device columns, its device rows, what)
        ("10 50 100 0 re", (10, 110), (150, 151), "zero-height on a boundary"),
        ("10 50.5 100 0 re", (10, 110), (149, 150), "zero-height off a boundary"),
        ("10.5 50.5 99 0 re", (10, 110), (149, 150), "zero-height, ends inside pixels"),
        ("50 10 0 100 re", (50, 51), (90, 190), "zero-width on a boundary"),
        ("50.5 10 0 100 re", (50, 51), (90, 190), "zero-width off a boundary"),
        ("50.5 10.5 0 99 re", (50, 51), (90, 190), "zero-width, ends inside pixels"),
    ];
    for (line, columns, rows, what) in cases {
        let fill = render(&format!("1 0 0 rg {line} f"));
        assert_only_red_run(&fill, columns, rows, &format!("{what}: fill"));
        let clip = render(&format!("q {line} W n {FLOOD} Q"));
        assert_only_red_run(&clip, columns, rows, &format!("{what}: clip"));
    }
}

#[test]
fn a_quarter_turn_maps_a_zero_height_clip_to_one_device_column() {
    // `0 1 -1 0 100 20 cm` turns user x into device -y: the line runs up column 100.
    let rgba = render(&format!("q 0 1 -1 0 100 20 cm 10 0 100 0 re W n {BIG_FLOOD} Q"));
    assert_only_red_run(&rgba, (100, 101), (70, 170), "quarter-turned clip");
}

#[test]
fn a_rotated_zero_height_clip_and_fill_show_a_one_pixel_band() {
    // 45 degrees: the line runs diagonally, so it shows as a band about one pixel wide.
    let clip = render(&format!("q 0.70710678 0.70710678 -0.70710678 0.70710678 50 50 cm 0 0 100 0 re W n {BIG_FLOOD} Q"));
    let fill = render("q 0.70710678 0.70710678 -0.70710678 0.70710678 50 50 cm 1 0 0 rg 0 0 100 0 re f Q");
    for (rgba, what) in [(&clip, "clip"), (&fill, "fill")] {
        // Along the diagonal from device (50, 150) to (120.7, 79.3), the pixel the line crosses
        // corner to corner in each column is mostly red, and the pixels well off it are white.
        for step in 5..65 {
            let (x, y) = (50 + step, 149 - step);
            let p = px(rgba, x, y);
            assert!(p[0] > 240 && p[1] < 160, "{what}: pixel ({x}, {y}) on the line is {p:?}");
            assert!(is_white(px(rgba, x + 3, y)), "{what}: pixel off the line");
            assert!(is_white(px(rgba, x - 3, y)), "{what}: pixel off the line");
        }
        // The band is one pixel wide: its ink adds up to its length, 100 pixels.
        let ink: f64 = rgba.as_chunks::<4>().0.iter().map(|p| 1.0 - f64::from(p[1]) / 255.0).sum();
        assert!((90.0..=110.0).contains(&ink), "{what}: ink {ink}");
    }
}

#[test]
fn a_thin_rectangle_with_area_keeps_its_partial_coverage() {
    // 0.3 pt high is not degenerate: it covers 30 % of device row 149 and is antialiased as
    // before, not widened to a whole pixel.
    let rgba = render("1 0 0 rg 10 50.2 100 0.3 re f");
    assert!(red_pixels(&rgba).is_empty(), "a 0.3 pt rectangle is not painted solid");
    for x in 10..110 {
        let p = px(&rgba, x, 149);
        assert!(p[0] > 240, "row 149 at {x}: {p:?}");
        assert!((150..=210).contains(&p[1]), "row 149 at {x} is 30 % covered: {p:?}");
        assert!(is_white(px(&rgba, x, 148)) && is_white(px(&rgba, x, 150)));
    }
    // The same rectangle as a clip keeps the same partial coverage.
    let clip = render(&format!("q 10 50.2 100 0.3 re W n {FLOOD} Q"));
    assert_eq!(clip, rgba);
}

#[test]
fn a_zero_area_clip_with_nothing_painted_on_it_stays_blank() {
    // The fill misses the line the clip keeps.
    let rgba = render("q 10 50 100 0 re W n 1 0 0 rg 0 100 200 100 re f Q");
    assert!(rgba.as_chunks::<4>().0.iter().all(|p| is_white([p[0], p[1], p[2]])), "page not blank");
    // Nothing painted at all.
    let rgba = render("q 10 50 100 0 re W n Q");
    assert!(rgba.as_chunks::<4>().0.iter().all(|p| is_white([p[0], p[1], p[2]])), "page not blank");
    // A clip that is a point keeps nothing, as before.
    let rgba = render(&format!("q 10 50 0 0 re W n {FLOOD} Q"));
    assert!(red_pixels(&rgba).is_empty() && tinted_pixels(&rgba) == 0, "a point clip shows the fill");
    // So does a point a little off zero both ways.
    let rgba = render(&format!("q 10 50 0.001 0.001 re W n {FLOOD} Q"));
    assert!(red_pixels(&rgba).is_empty() && tinted_pixels(&rgba) == 0, "a tiny clip shows the fill");
}

#[test]
fn the_clip_is_restored_after_q() {
    let rgba = render(&format!("q 10 50 100 0 re W n Q {FLOOD}"));
    assert_eq!(red_pixels(&rgba).len(), (SIZE * SIZE) as usize);
}

#[test]
fn a_thin_rectangle_scaled_up_to_whole_pixels_keeps_its_area() {
    // 1/1024 pt high is under the rasteriser's "nearly zero" threshold, but the rectangle has
    // area: scaled 4096 times vertically it covers device y 164..168, four whole rows, and is
    // painted as the rectangle it is, not as a line across its bounding box.
    let fill = render("q 1 0 0 4096 0 0 cm 1 0 0 rg 10 0.0078125 100 0.0009765625 re f Q");
    assert_only_red_run(&fill, (10, 110), (164, 168), "scaled-up thin fill");
    let clip = render(&format!("q 1 0 0 4096 0 0 cm 10 0.0078125 100 0.0009765625 re W n {FLOOD} Q"));
    assert_only_red_run(&clip, (10, 110), (164, 168), "scaled-up thin clip");
}

#[test]
fn a_slanted_zero_height_line_crossing_a_row_marks_each_row_only_where_it_runs() {
    // A slight skew lifts the line from device y 149.52 at x = 10 to 148.74 at x = 110: under a
    // pixel high in all, but crossing the boundary between rows 148 and 149 near x = 75. Each
    // row is marked where the line runs through it, not along the whole length.
    let skew = "1 0.008 0 1 0 0 cm";
    let fill = render(&format!("q {skew} 1 0 0 rg 10 50.4 100 0 re f Q"));
    let clip = render(&format!("q {skew} 10 50.4 100 0 re W n {FLOOD} Q"));
    for (rgba, what) in [(&fill, "fill"), (&clip, "clip")] {
        for x in [12, 15, 20] {
            assert!(px(rgba, x, 149)[1] < 128, "{what}: ({x}, 149) on the line is {:?}", px(rgba, x, 149));
            assert!(px(rgba, x, 148)[1] > 128, "{what}: ({x}, 148) off the line is {:?}", px(rgba, x, 148));
        }
        for x in [100, 105, 108] {
            assert!(px(rgba, x, 148)[1] < 128, "{what}: ({x}, 148) on the line is {:?}", px(rgba, x, 148));
            assert!(px(rgba, x, 149)[1] > 128, "{what}: ({x}, 149) off the line is {:?}", px(rgba, x, 149));
        }
        // One pixel wide: its ink adds up to its length, about 100 pixels, not two rows' worth.
        let ink: f64 = rgba.as_chunks::<4>().0.iter().map(|p| 1.0 - f64::from(p[1]) / 255.0).sum();
        assert!((90.0..=110.0).contains(&ink), "{what}: ink {ink}");
    }
}

#[test]
fn disconnected_subpaths_on_one_line_leave_the_gap_between_them_unpainted() {
    // Two zero-height subpaths on user y = 50 (device row 150), x 10..30 and 60..90: the fill
    // and the clip mark the pixels each subpath touches, not the gap between them.
    let mut expected = Vec::new();
    for x in (10..30).chain(60..90) {
        expected.push((x, 150));
    }
    let fill = render("1 0 0 rg 10 50 m 30 50 l 60 50 m 90 50 l f");
    let clip = render(&format!("q 10 50 m 30 50 l 60 50 m 90 50 l W n {FLOOD} Q"));
    for (rgba, what) in [(&fill, "fill"), (&clip, "clip")] {
        assert_eq!(red_pixels(rgba), expected, "{what}: the red pixels");
        assert_eq!(tinted_pixels(rgba), 0, "{what}: partly covered pixels");
    }
    // Overlapping subpaths drawn in opposite directions still paint their union under either rule.
    let fill = render("1 0 0 rg 10 50 m 60 50 l 90 50 m 40 50 l f*");
    assert_only_red_run(&fill, (10, 90), (150, 151), "overlapping subpaths");
}
