//! A tiling pattern's cell blends with what is under it (ISO 32000-2 §11.6.7): the pattern
//! definition is treated as a non-isolated, non-knockout transparency group, "as if the pattern
//! definition were re-executed independently for each tile, taking into account the colour of
//! the backdrop at each point" (NOTE 1). A cell drawn on its own and then composited, the
//! shortcut NOTE 1 allows only for cells painted entirely with Normal, loses every blend mode
//! inside it.
//!
//! Each page paints a backdrop of two greys, 0.2 on the left half and 0.6 on the right, and then
//! fills or strokes over it with a pattern whose cell paints red under `/BM /Multiply`. Red
//! multiplied with grey `g` is `(255 g, 0, 0)`; the cell drawn on its own would leave plain red.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// The page is `SIDE` × `SIDE` points, rendered at one pixel per point.
const SIDE: u32 = 200;
/// The two-grey backdrop.
const BACKDROP: &str = "0.2 g 0 0 100 200 re f 0.6 g 100 0 100 200 re f";

/// One page whose content `content` paints with `/P1`: a tiling pattern with `/PaintType`
/// `paint_type`, `/BBox [0 0 cell cell]`, steps `cell` and content `cell_content`, with
/// `/GM << /BM /Multiply >>` among its graphics states. The page has `/PM`, the same state, and
/// `/CP`, an uncoloured pattern space over `DeviceRGB`.
fn page_pdf(content: &str, paint_type: u8, cell: u32, cell_content: &str) -> Vec<u8> {
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {SIDE} {SIDE}] /Contents 4 0 R /Resources << /ColorSpace << /CP [/Pattern /DeviceRGB] >> /Pattern << /P1 5 0 R >> /ExtGState << /PM << /BM /Multiply >> >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Pattern /PatternType 1 /PaintType {paint_type} /TilingType 1 /BBox [0 0 {cell} {cell}] /XStep {cell} /YStep {cell} /Resources << /ExtGState << /GM << /BM /Multiply >> >> >> /Length {} >> stream
{cell_content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len(),
        cell_content.len()
    )
    .into_bytes()
}

struct Page {
    rgba: Vec<u8>,
    width: u32,
}

impl Page {
    /// The colour at PDF point `(x, y)` (pixel rows run top down).
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (((SIDE - 1 - y) * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }
}

fn render(pdf: Vec<u8>) -> Page {
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    Page { rgba: page.rgba.to_vec(), width: page.width }
}

/// Whether `got` is `want` within two levels a channel.
fn near(got: [u8; 3], want: [u8; 3]) -> bool {
    got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2)
}

/// Red multiplied with grey 0.2 and with grey 0.6.
const DARK: [u8; 3] = [51, 0, 0];
const LIGHT: [u8; 3] = [153, 0, 0];

/// A cell as large as the page, drawn once (the case of a picture fill over shaded artwork).
#[test]
fn a_page_sized_cell_multiplies_with_the_page() {
    let cell = "/GM gs 1 0 0 rg 0 0 200 200 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /Pattern cs /P1 scn 20 20 160 160 re f"), 1, 200, cell));
    assert!(near(page.at(50, 100), DARK), "left half: {:?}", page.at(50, 100));
    assert!(near(page.at(150, 100), LIGHT), "right half: {:?}", page.at(150, 100));
    assert_eq!(page.at(10, 100), [51, 51, 51], "outside the fill the backdrop is untouched");
}

/// A small cell repeated over the fill blends in every tile, on both greys.
#[test]
fn every_repeated_cell_multiplies_with_the_page() {
    // A 20-point cell whose lower-left 10 × 10 square is red.
    let cell = "/GM gs 1 0 0 rg 0 0 10 10 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /Pattern cs /P1 scn 0 0 200 200 re f"), 1, 20, cell));
    for (x, y) in [(5, 5), (45, 85), (85, 165), (105, 5), (145, 105), (185, 185)] {
        let want = if x < 100 { DARK } else { LIGHT };
        assert!(near(page.at(x, y), want), "red square at ({x}, {y}): {:?}", page.at(x, y));
    }
    assert_eq!(page.at(15, 15), [51, 51, 51], "between the squares the backdrop shows");
    assert_eq!(page.at(115, 15), [153, 153, 153], "between the squares the backdrop shows");
}

/// A stroke painted with the pattern blends where the stroke is, and only there.
#[test]
fn a_stroke_with_a_blending_cell_multiplies_with_the_page() {
    let cell = "/GM gs 1 0 0 rg 0 0 200 200 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /Pattern CS /P1 SCN 20 w 20 100 m 180 100 l S"), 1, 200, cell));
    assert!(near(page.at(50, 100), DARK), "stroke on the left: {:?}", page.at(50, 100));
    assert!(near(page.at(150, 100), LIGHT), "stroke on the right: {:?}", page.at(150, 100));
    assert_eq!(page.at(50, 130), [51, 51, 51], "beside the stroke the backdrop is untouched");
}

/// An uncoloured pattern (`/PaintType 2`) takes its colour from `scn` and blends as well.
#[test]
fn an_uncoloured_blending_cell_multiplies_with_the_page() {
    let cell = "/GM gs 0 0 200 200 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /CP cs 1 0 0 /P1 scn 20 20 160 160 re f"), 2, 200, cell));
    assert!(near(page.at(50, 100), DARK), "left half: {:?}", page.at(50, 100));
    assert!(near(page.at(150, 100), LIGHT), "right half: {:?}", page.at(150, 100));
}

/// The fill's own blend mode still composites the pattern: under `/BM /Multiply` set on the page
/// the red cell multiplies with the backdrop too.
#[test]
fn the_fills_own_blend_mode_still_applies() {
    let cell = "/GM gs 1 0 0 rg 0 0 200 200 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /PM gs /Pattern cs /P1 scn 20 20 160 160 re f"), 1, 200, cell));
    assert!(near(page.at(150, 100), LIGHT), "right half: {:?}", page.at(150, 100));
}

/// A cell painted with Normal alone keeps its colour over the backdrop.
#[test]
fn a_normal_cell_covers_the_page() {
    let cell = "1 0 0 rg 0 0 10 10 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /Pattern cs /P1 scn 0 0 200 200 re f"), 1, 20, cell));
    assert_eq!(page.at(45, 85), [255, 0, 0]);
    assert_eq!(page.at(145, 105), [255, 0, 0]);
}

/// A blending cell repeated more often than the renderer draws cells one by one is still drawn
/// (from the cell pixmap).
#[test]
fn a_fill_over_very_many_blending_cells_still_paints() {
    let cell = "/GM gs 1 0 0 rg 0 0 1 1 re f";
    let page = render(page_pdf(&format!("{BACKDROP} /Pattern cs /P1 scn 0 0 200 200 re f"), 1, 2, cell));
    assert_ne!(page.at(150, 100), [153, 153, 153], "the cells are painted");
}
