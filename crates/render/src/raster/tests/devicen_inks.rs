//! #4260: a DeviceN colour with more inks than the content parser held operands for, and a
//! colour whose tint transform cannot be evaluated (ISO 32000-2 §8.6.6.5).
//!
//! The content parser kept at most ten operands per operator (vendored hayro-syntax patch 28):
//! the eleventh operand of a twelve-ink `scn` ended the whole content stream, so the page came
//! out blank with no error. A tint transform that cannot be evaluated painted the alternate
//! space's initial colour without a word (vendored hayro-interpret patch 55): it now says so.

use super::{RenderWarning, rgb_at};
use crate::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};
use std::sync::Arc;

/// The page is 200 × 200 pt, drawn at 72 dpi.
const SIDE: usize = 200;

const GREEN: [u8; 3] = [0, 255, 0];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// The twelve inks of the report's print dieline.
const TWELVE_INKS: &str =
    "[/Cyan /Magenta /Yellow /Black /PANTONE156C /PANTONE2227C /PANTONE1565C /PANTONE2975C /PANTONE7463C /Varnish /Dieline /SGKBlack]";

/// The report's tint transform: all inks at 1.0 give CMYK 1 1 1 1.
const TINT: &str = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0 0] /C1 [1 1 1 1] >>";

/// Render the one page `content` draws with the colour space `CS1` defined as `cs1`.
fn render(content: &str, cs1: &str) -> RenderedPage {
    render_with(content, cs1, "")
}

/// [`render`], with `objects` (whole indirect objects from 5 0 obj on) added to the file.
fn render_with(content: &str, cs1: &str, objects: &str) -> RenderedPage {
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /ColorSpace << /CS1 {cs1} >> >> /Contents 4 0 R >> endobj\n\
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

/// The colour at (`x`, `y`) in page points from the bottom left.
fn at(page: &RenderedPage, x: usize, y: usize) -> [u8; 3] {
    rgb_at(&page.rgba, SIDE, x, SIDE - 1 - y)
}

/// CMYK 1 1 1 1 through the renderer's CMYK profile: very dark, whatever the exact profile.
fn is_rich_black(rgb: [u8; 3]) -> bool {
    rgb.iter().all(|c| *c < 48)
}

/// The report's page: twelve inks, all at 1.0, fill the page black. A green square drawn
/// afterwards shows the rest of the content is still read.
#[test]
fn a_twelve_ink_devicen_colour_paints() {
    let content = "/CS1 cs 1 1 1 1 1 1 1 1 1 1 1 1 scn 0 0 200 200 re f 0 1 0 rg 0 0 20 20 re f";
    let page = render(content, &format!("[/DeviceN {TWELVE_INKS} /DeviceCMYK {TINT}]"));
    assert!(is_rich_black(at(&page, 100, 100)), "the twelve-ink fill paints: {:?}", at(&page, 100, 100));
    assert_eq!(at(&page, 10, 10), GREEN, "what follows the twelve-ink colour is drawn");
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
}

/// The control from the report: the same colour with four inks always painted.
#[test]
fn a_four_ink_devicen_colour_paints() {
    let content = "/CS1 cs 1 1 1 1 scn 0 0 200 200 re f 0 1 0 rg 0 0 20 20 re f";
    let page = render(content, &format!("[/DeviceN [/Cyan /Magenta /Yellow /Black] /DeviceCMYK {TINT}]"));
    assert!(is_rich_black(at(&page, 100, 100)), "the four-ink fill paints: {:?}", at(&page, 100, 100));
    assert_eq!(at(&page, 10, 10), GREEN);
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
}

/// An operator with more operands than the parser keeps still reads the ones nearest to it,
/// as one with a few stray operands does, and the content after it is drawn.
#[test]
fn an_operator_with_hundreds_of_operands_does_not_end_the_content() {
    let strays = "0 ".repeat(300);
    let content = format!("1 0 0 rg 0 0 200 200 re f {strays}0 1 0 rg 0 0 20 20 re f");
    let page = render(&content, "/DeviceRGB");
    assert_eq!(at(&page, 100, 100), RED, "the content before the stray operands is drawn");
    assert_eq!(at(&page, 10, 10), GREEN, "the operator after them reads its own three operands");
}

/// A tint transform that fails when it runs (a calculator function that pops more than it is
/// given) paints that one colour in the alternate space's initial colour (black in DeviceRGB)
/// and records a warning; the paints before and after it are drawn as usual.
#[test]
fn a_tint_transform_that_cannot_be_evaluated_draws_the_rest_of_the_page_and_warns() {
    let program = "{ pop pop pop }";
    let function = format!(
        "5 0 obj << /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Length {} >> stream\n{program}\nendstream endobj\n",
        program.len()
    );
    let content = "1 0 0 rg 0 0 100 200 re f /CS1 cs 0.5 0.5 scn 100 0 100 200 re f 0 0 1 rg 0 0 50 50 re f";
    let page = render_with(content, "[/DeviceN [/Spot1 /Spot2] /DeviceRGB 5 0 R]", &function);
    assert_eq!(at(&page, 75, 150), RED, "the paint before the colour is drawn");
    assert_eq!(at(&page, 150, 150), BLACK, "the colour falls back to the alternate space's initial colour");
    assert_eq!(at(&page, 25, 25), BLUE, "the paint after the colour is drawn");
    assert!(page.warnings.contains(&RenderWarning::ColourFallback), "{:?}", page.warnings);
}

/// A tint transform that cannot even be read (an unknown function type) keeps the colour space,
/// so its paints fall back the same way, with the same warning, instead of being drawn in
/// DeviceGray from the first tint (white here).
#[test]
fn a_tint_transform_that_cannot_be_read_falls_back_and_warns() {
    let cs1 = format!("[/DeviceN {TWELVE_INKS} /DeviceCMYK << /FunctionType 7 /Domain [0 1] >>]");
    let content = "/CS1 cs 1 1 1 1 1 1 1 1 1 1 1 1 scn 0 0 200 200 re f 0 1 0 rg 0 0 20 20 re f";
    let page = render(content, &cs1);
    assert!(is_rich_black(at(&page, 100, 100)), "the colour falls back to CMYK black: {:?}", at(&page, 100, 100));
    assert_eq!(at(&page, 10, 10), GREEN, "what follows is drawn");
    assert!(page.warnings.contains(&RenderWarning::ColourFallback), "{:?}", page.warnings);
    // Reported once per page, however many paints fell back.
    assert_eq!(page.warnings.iter().filter(|w| **w == RenderWarning::ColourFallback).count(), 1);
}
