//! Tiling pattern cells sit at whole multiples of `/XStep` and `/YStep` in pattern space
//! (ISO 32000-2 8.7.3.1, Table 74), at any rendering scale, however far they are from the
//! pattern's origin.
//!
//! Each page is filled (or stroked) with a pattern whose 20 × 20 cell holds one 6 × 6 blue square
//! at (7, 7). At 110 dpi a step is 30.56 device pixels; every probe finds the blue square in a
//! disc around where the pattern matrix puts it and checks that its centroid is within one
//! device pixel of that place.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// The rendering scale for 110 dpi: a step of 20 points is 30.56 pixels, not a whole number.
const SCALE: f64 = 110.0 / 72.0;
/// The page is `SIDE` × `SIDE` points.
const SIDE: f64 = 400.0;

/// One page `SIDE` points square whose content `content` paints with `/P1`: the 20-point cell
/// with its blue square, `/XStep` and `/YStep` 20, under pattern matrix `matrix`.
fn page_pdf(content: &str, matrix: [f64; 6]) -> Vec<u8> {
    let cell = "0 0 1 rg 7 7 6 6 re f";
    let m = matrix.map(|v| format!("{v}")).join(" ");
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {SIDE} {SIDE}] /Contents 4 0 R /Resources << /Pattern << /P1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 /Matrix [{m}] /Length {} >> stream
{cell}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len(),
        cell.len()
    )
    .into_bytes()
}

struct Page {
    rgba: Vec<u8>,
    width: usize,
    height: usize,
}

fn render(content: &str, matrix: [f64; 6]) -> Page {
    let mut r = PageRenderer::new(Arc::new(page_pdf(content, matrix)), RenderConfig::default());
    let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: SCALE as f32, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    Page { rgba: page.rgba.to_vec(), width: page.width as usize, height: page.height as usize }
}

/// The device position of pattern-space point `(x, y)`: the pattern matrix to page space, then
/// page space to device pixels (y down).
fn device(matrix: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    let [a, b, c, d, e, f] = matrix;
    let (px, py) = (a * x + c * y + e, b * x + d * y + f);
    (px * SCALE, (SIDE - py) * SCALE)
}

/// The centroid of blue (weighted by how far each pixel is from white) in the disc of `radius`
/// pixels around `(cx, cy)`, and the total weight. `None` when the disc leaves the page.
fn blue_centroid(page: &Page, cx: f64, cy: f64, radius: f64) -> Option<((f64, f64), f64)> {
    let (x0, x1) = ((cx - radius).floor(), (cx + radius).ceil());
    let (y0, y1) = ((cy - radius).floor(), (cy + radius).ceil());
    if x0 < 0.0 || y0 < 0.0 || x1 >= page.width as f64 || y1 >= page.height as f64 {
        return None;
    }
    let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for j in (y0 as usize)..=(y1 as usize) {
        for i in (x0 as usize)..=(x1 as usize) {
            let (px, py) = (i as f64 + 0.5, j as f64 + 0.5);
            if (px - cx).powi(2) + (py - cy).powi(2) > radius * radius {
                continue;
            }
            let red = *page.rgba.get((j * page.width + i) * 4)?;
            let w = f64::from(255 - red) / 255.0;
            sum += w;
            sx += w * px;
            sy += w * py;
        }
    }
    (sum > 0.0).then(|| ((sx / sum, sy / sum), sum))
}

/// Checks every cell `(k, m)` whose blue square's centre falls in the page rectangle `area`
/// (points: x0, y0, x1, y1): its square is found where the matrix puts it, within one pixel.
/// Returns how many cells were checked.
fn assert_cells_in_place(page: &Page, matrix: [f64; 6], area: [f64; 4], what: &str) -> usize {
    let [a, b, c, d, _, _] = matrix;
    // The square is 6 × 6 at (7, 7); its centre is (10, 10) in the cell. A disc of 6 cell
    // units around it holds the whole square (half-diagonal 4.24) and no neighbour (7 away).
    let units = (a * a + b * b).sqrt().min((c * c + d * d).sqrt());
    let radius = 6.0 * units * SCALE;
    let area_px = 36.0 * units * units * SCALE * SCALE;
    let mut checked = 0;
    for k in -60..60 {
        for m in -60..60 {
            let (cx_cell, cy_cell) = (20.0 * f64::from(k) + 10.0, 20.0 * f64::from(m) + 10.0);
            let [ea, eb, ec, ed, ee, ef] = matrix;
            let (px, py) = (ea * cx_cell + ec * cy_cell + ee, eb * cx_cell + ed * cy_cell + ef);
            if px < area[0] || px > area[2] || py < area[1] || py > area[3] {
                continue;
            }
            let (cx, cy) = device(matrix, cx_cell, cy_cell);
            let Some(((gx, gy), weight)) = blue_centroid(page, cx, cy, radius) else { continue };
            assert!(weight > 0.8 * area_px, "{what}: cell ({k}, {m}) at ({cx:.1}, {cy:.1}) only partly there: {weight:.1} of {area_px:.1}");
            assert!(
                (gx - cx).abs() <= 1.0 && (gy - cy).abs() <= 1.0,
                "{what}: cell ({k}, {m}) should be centred at ({cx:.2}, {cy:.2}) px, found at ({gx:.2}, {gy:.2})"
            );
            checked += 1;
        }
    }
    checked
}

const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// A fill far from the pattern's origin: the cells 300 to 400 points away (15 to 19 steps) are
/// where 15 to 19 steps put them, not 0.44 pixel further for every step.
#[test]
fn far_cells_of_a_fill_sit_at_whole_steps() {
    let page = render("/Pattern cs /P1 scn 0 0 400 400 re f", IDENTITY);
    let near = assert_cells_in_place(&page, IDENTITY, [0.0, 0.0, 100.0, 100.0], "near the origin");
    let far = assert_cells_in_place(&page, IDENTITY, [290.0, 290.0, 400.0, 400.0], "far from the origin");
    assert!(near >= 16 && far >= 16, "{near} near, {far} far");
}

/// The same under a pattern matrix that scales by 1.49 and turns by 30 degrees: a step is
/// 45.5 pixels along each turned axis.
#[test]
fn far_cells_sit_at_whole_steps_under_a_turned_scaled_matrix() {
    let (scale, turn) = (1.49_f64, 30_f64.to_radians());
    let matrix = [scale * turn.cos(), scale * turn.sin(), -scale * turn.sin(), scale * turn.cos(), 3.0, 5.0];
    let page = render("/Pattern cs /P1 scn 0 0 400 400 re f", matrix);
    let far = assert_cells_in_place(&page, matrix, [250.0, 250.0, 390.0, 390.0], "turned and scaled");
    assert!(far >= 6, "{far} cells checked");
}

/// A stroke painted with the pattern: the cells under a wide border far from the origin are
/// in place on every side of it.
#[test]
fn far_cells_of_a_stroke_sit_at_whole_steps() {
    let page = render("/Pattern CS /P1 SCN 40 w 200 200 180 180 re S", IDENTITY);
    let mut checked = 0;
    // The border is 40 points wide around the square from (200, 200) to (380, 380).
    for band in [[180.0, 180.0, 400.0, 220.0], [180.0, 360.0, 400.0, 400.0], [180.0, 180.0, 220.0, 400.0], [360.0, 180.0, 400.0, 400.0]] {
        checked += assert_cells_in_place(&page, IDENTITY, band, "stroke");
    }
    assert!(checked >= 20, "{checked} cells checked");
    // Inside the border nothing is painted.
    let (cx, cy) = device(IDENTITY, 290.0, 290.0);
    let at = ((cy as usize) * page.width + cx as usize) * 4;
    assert_eq!(page.rgba.get(at..at + 3), Some(&[255, 255, 255][..]), "inside the stroked square");
}

/// The cell pixmap's side and the scale its cell is drawn at: one step is exactly the side, the
/// side stays within 1 and the cap whatever the step, and nothing odd reaches the cast.
#[test]
fn a_cell_pixmap_spans_exactly_one_step() {
    // 110 dpi, a step of 20: 30.56 pixels, drawn at 31 / 20.
    assert_eq!(hayro::tiling_cell_pixels(110.0 / 72.0, 20.0, 3000.0), (31, 31.0 / 20.0));
    assert_eq!(hayro::tiling_cell_pixels(110.0 / 72.0, -20.0, 3000.0), (31, 31.0 / 20.0), "a negative step");
    for (scale, step) in [(1.0_f32, 7.3_f32), (2.37, 0.9), (0.013, 1234.5), (3.0, 999.9), (1.0, 2.0e9)] {
        let (side, drawn) = hayro::tiling_cell_pixels(scale, step, 3000.0);
        assert!((1..=3000).contains(&side), "{scale} {step}: {side}");
        let span = drawn * f64::from(step);
        assert!((span - f64::from(side)).abs() < 1e-9 * f64::from(side), "{scale} {step}: a step spans {span}, not {side}");
        if f64::from(scale) * f64::from(step) <= 3000.0 {
            assert!((span - f64::from(scale) * f64::from(step)).abs() <= 0.5 + 1e-9, "{scale} {step}: drawn {drawn}");
        }
    }
    // A step under half a pixel still gets one pixel (a 0 × N pixmap can't be sampled).
    assert_eq!(hayro::tiling_cell_pixels(0.03, 10.0, 3000.0), (1, 0.1));
    // Steps and scales that aren't finite and positive: the side is bounded, the scale kept.
    assert_eq!(hayro::tiling_cell_pixels(2.0, f32::INFINITY, 3000.0), (3000, 2.0));
    assert_eq!(hayro::tiling_cell_pixels(2.0, 0.0, 3000.0), (1, 2.0));
    let (side, drawn) = hayro::tiling_cell_pixels(f32::NAN, 20.0, 3000.0);
    assert!(side == 1 && drawn.is_nan());
    // A cap past u16::MAX is lowered to it, and the scale follows the side the pixmap gets.
    let (side, drawn) = hayro::tiling_cell_pixels(2.0, 1.0e6, 1.0e9);
    assert_eq!(side, u16::MAX, "the cap is at most u16::MAX");
    assert!((drawn * 1.0e6 - f64::from(u16::MAX)).abs() < 1e-6, "a step spans the side: {drawn}");
    // A cap that is not a whole number, or not a number, is a whole number of pixels.
    assert_eq!(hayro::tiling_cell_pixels(1.0, 100.0, 30.7), (30, 0.3));
    assert_eq!(hayro::tiling_cell_pixels(1.0, 100.0, f32::NAN), (1, 0.01));
}
