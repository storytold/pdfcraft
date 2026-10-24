//! Stroke geometry the renderer gets from the graphics state: degenerate subpaths and their line
//! caps (ISO 32000-2 §8.5.3.2), dash arrays that are all zero or otherwise invalid (§8.4.3.6) and
//! line widths below zero (§8.4.3.2). Vendored hayro-interpret patch 61.

use std::sync::Arc;

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, StrokeProps,
    TransformExt, interpret_page,
};
use kurbo::{Affine, BezPath, Rect};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// Renders `content` on a 40 × 40 pt page at 1 px per pt, with `gs1` as the ExtGState `/GS1`.
fn render(content: &str, gs1: &str) -> RenderedPage {
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /ExtGState << /GS1 {gs1} >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    );
    let mut r = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (40, 40));
    page
}

/// The pixel at device (x, y), y down.
fn px(page: &RenderedPage, x: usize, y: usize) -> [u8; 4] {
    let i = (y * 40 + x) * 4;
    let mut out = [0; 4];
    out.copy_from_slice(&page.rgba[i..i + 4]);
    out
}

/// How many pixels are not white.
fn inked(page: &RenderedPage) -> usize {
    page.rgba.as_chunks::<4>().0.iter().filter(|p| **p != WHITE).count()
}

/// §8.5.3.2: a degenerate subpath (a single-point closed path, or points that all coincide) is
/// painted as a filled circle of the line width when the caps are round, and not at all with butt
/// or projecting square caps. User-space point (20, 20) is device pixel (20, 20); a 10 pt round
/// dot covers (20 ± 4, 20) and (20, 20 ± 4) but not the square's corner (24, 24).
#[test]
fn degenerate_subpaths_draw_a_round_dot_or_nothing() {
    let round = [
        ("m l to the same point", "0 0 1 RG 10 w 1 J 20 20 m 20 20 l S"),
        ("m h, a single-point closed path", "0 0 1 RG 10 w 1 J 20 20 m h S"),
        ("m h by s", "0 0 1 RG 10 w 1 J 20 20 m s"),
        ("m c with every point the same", "0 0 1 RG 10 w 1 J 20 20 m 20 20 20 20 20 20 c S"),
        ("m l l h at one point", "0 0 1 RG 10 w 1 J 20 20 m 20 20 l 20 20 l h S"),
        ("a 0 × 0 re", "0 0 1 RG 10 w 1 J 20 20 0 0 re S"),
    ];
    for (what, content) in round {
        let page = render(content, "<< >>");
        for (x, y) in [(20, 20), (16, 20), (23, 20), (20, 16), (20, 23)] {
            assert_eq!(px(&page, x, y), BLUE, "{what}, round caps: ({x}, {y}) is inside the dot");
        }
        assert_eq!(px(&page, 24, 24), WHITE, "{what}, round caps: the dot is round, not square");
        assert_eq!(px(&page, 27, 20), WHITE, "{what}, round caps: the dot is the line width across");
    }
    for cap in ["0 J", "2 J"] {
        for (what, content) in round {
            let content = content.replace("1 J", cap);
            assert_eq!(inked(&render(&content, "<< >>")), 0, "{what} with {cap}: nothing is painted");
        }
    }
    // A single-point open subpath (a trailing m) paints nothing whatever the cap.
    assert_eq!(inked(&render("0 0 1 RG 10 w 1 J 20 20 m S", "<< >>")), 0, "a lone m");
}

/// Only the degenerate subpaths of a path change: the line drawn with them keeps its caps, and a
/// line whose ends differ by a hair is not degenerate.
#[test]
fn degenerate_subpaths_leave_the_rest_of_the_path_alone() {
    // Projecting square caps: the line from x 10 to 30 at y 10 reaches x 7 and x 33; the
    // degenerate subpath at (20, 30) is dropped.
    let page = render("0 0 1 RG 6 w 2 J 10 30 m 30 30 l 20 10 m h S", "<< >>");
    for x in [8, 20, 32] {
        assert_eq!(px(&page, x, 10), BLUE, "({x}, 10) on the line with its square caps");
    }
    assert_eq!(px(&page, 20, 30), WHITE, "the degenerate subpath is dropped");
    // Round caps: the degenerate subpath becomes a dot beside the line.
    let page = render("0 0 1 RG 6 w 1 J 10 30 m 30 30 l 20 10 m h S", "<< >>");
    assert_eq!(px(&page, 20, 10), BLUE, "the line");
    assert_eq!(px(&page, 20, 30), BLUE, "the dot");
    // `b` (close, fill and stroke) of a single point strokes the dot too.
    let page = render("0 0 1 RG 1 0 0 rg 6 w 1 J 20 20 m b", "<< >>");
    assert_eq!(px(&page, 20, 20), BLUE, "b of one point strokes a round dot");
    // A curve back to its start through other control points is a loop, not a point.
    let page = render("0 0 1 RG 2 w 2 J 20 10 m 35 35 5 35 20 10 c S", "<< >>");
    assert!(inked(&page) > 20, "a curve that returns to its start is stroked");
    // A hairline dot is still drawn, about a device pixel across.
    let page = render("0 0 1 RG 0 w 1 J 20 20 m h S", "<< >>");
    assert!((1..=9).contains(&inked(&page)), "a 0 w round dot: {} pixels", inked(&page));
    // Not degenerate: the ends differ, so the square caps are drawn along the hairline segment.
    let page = render("0 0 1 RG 10 w 2 J 20 20 m 20.01 20 l S", "<< >>");
    assert_eq!(px(&page, 20, 20), BLUE, "a segment a hundredth of a point long is stroked with its caps");
}

/// §8.4.3.6 asks for dash numbers that are nonnegative and not all zero, but not how to recover
/// from others. They draw a solid line, as an empty dash array does; before, `[0 0]` was cut into
/// hundredth-of-a-point dashes and came out a faint, half-covered line.
#[test]
fn all_zero_dash_arrays_draw_a_solid_line() {
    for (what, content, gs1) in [
        ("[0 0]", "0 0 1 RG 4 w [0 0] 0 d 10 20 m 30 20 l S", "<< >>"),
        ("[0]", "0 0 1 RG 4 w [0] 0 d 10 20 m 30 20 l S", "<< >>"),
        ("[0 0 0] with a phase", "0 0 1 RG 4 w [0 0 0] 3 d 10 20 m 30 20 l S", "<< >>"),
        ("[0 0] from an ExtGState", "0 0 1 RG 4 w /GS1 gs 10 20 m 30 20 l S", "<< /D [[0 0] 0] >>"),
        ("[0 -1], negative", "0 0 1 RG 4 w [0 -1] 0 d 10 20 m 30 20 l S", "<< >>"),
        ("[3 -1] from an ExtGState", "0 0 1 RG 4 w /GS1 gs 10 20 m 30 20 l S", "<< /D [[3 -1] 0] >>"),
    ] {
        let page = render(content, gs1);
        for x in 11..30 {
            assert_eq!(px(&page, x, 20), BLUE, "{what}: ({x}, 20) is solid and opaque");
        }
    }
}

/// Patterns that are valid keep their dashes: `[0 12]` with round caps is a row of dots, `[4 4]`
/// leaves gaps, an odd `[4]` repeats as `[4 4]`.
#[test]
fn valid_dash_patterns_still_dash() {
    let page = render("0 0 1 RG 4 w 1 J [0 12] 0 d 4 20 m 36 20 l S", "<< >>");
    for x in [4, 16, 28] {
        assert_eq!(px(&page, x, 20), BLUE, "[0 12] round caps: a dot at x {x}");
    }
    for x in [10, 22, 34] {
        assert_eq!(px(&page, x, 20), WHITE, "[0 12] round caps: a gap at x {x}");
    }
    for (what, content) in [("[4 4]", "0 0 1 RG 4 w [4 4] 0 d 0 20 m 40 20 l S"), ("[4]", "0 0 1 RG 4 w [4] 0 d 0 20 m 40 20 l S")] {
        let page = render(content, "<< >>");
        assert_eq!(px(&page, 2, 20), BLUE, "{what}: the first dash");
        assert_eq!(px(&page, 6, 20), WHITE, "{what}: the first gap");
        assert_eq!(px(&page, 10, 20), BLUE, "{what}: the second dash");
    }
}

/// The stroke properties and paints every `draw_path` of `content` reaches a device with.
fn strokes(content: &str, gs1: &str) -> Vec<(StrokeProps, Paint<'static>)> {
    #[derive(Default)]
    struct Strokes(Vec<(StrokeProps, Paint<'static>)>);
    impl<'a> Device<'a> for Strokes {
        fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
        fn set_blend_mode(&mut self, _: BlendMode) {}
        fn draw_path(&mut self, _: &BezPath, _: Affine, paint: &Paint<'a>, mode: &PathDrawMode) {
            if let (PathDrawMode::Stroke(props), Paint::Color(colour)) = (mode, paint) {
                self.0.push((props.clone(), Paint::Color(colour.clone())));
            }
        }
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
        fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {}
        fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
        fn pop_clip_path(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /ExtGState << /GS1 {gs1} >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    );
    let parsed = Pdf::new(Arc::new(pdf.into_bytes())).expect("parses");
    let page = &parsed.pages()[0];
    let cache = InterpreterCache::new();
    let mut ctx =
        Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, 40.0, 40.0), &cache, page.xref(), InterpreterSettings::default());
    let mut device = Strokes::default();
    interpret_page(page, &mut ctx, &mut device);
    device.0
}

/// §8.4.3.2: a line width of 0 is the thinnest line the device can draw. A negative or
/// non-finite one is invalid and kept as 0, so no device sees a width below zero; the stroke
/// keeps the current stroke colour.
#[test]
fn negative_line_widths_reach_a_device_as_zero() {
    for (what, content, gs1, width) in [
        ("-6 w", "0 0 1 RG -6 w 10 20 m 30 20 l S", "<< >>", 0.0),
        ("/LW -6", "0 0 1 RG /GS1 gs 10 20 m 30 20 l S", "<< /LW -6 >>", 0.0),
        ("/LW 1e40, beyond f32", "0 0 1 RG /GS1 gs 10 20 m 30 20 l S", "<< /LW 10000000000000000000000000000000000000000 >>", 0.0),
        ("2.5 w", "0 0 1 RG 2.5 w 10 20 m 30 20 l S", "<< >>", 2.5),
        ("0 w", "0 0 1 RG 0 w 10 20 m 30 20 l S", "<< >>", 0.0),
    ] {
        let seen = strokes(content, gs1);
        assert_eq!(seen.len(), 1, "{what}: one stroke");
        let (props, paint) = &seen[0];
        assert_eq!(props.line_width, width, "{what}: line width");
        let Paint::Color(colour) = paint else { unreachable!() };
        assert_eq!(colour.to_rgba().to_rgba8(), [0, 0, 255, 255], "{what}: the stroke colour");
    }
}

/// The dash array a device is given: `[]` (solid) for one that is all zero or has a negative or
/// non-finite entry, from `d` or from `/D`; a valid one keeps its entries, a zero entry turned
/// into a hundredth of a point, as before.
#[test]
fn invalid_dash_arrays_reach_a_device_as_solid() {
    let line = "10 20 m 30 20 l S";
    for (what, setup, gs1, dashes) in [
        ("[0 0]", "[0 0] 0 d", "<< >>", &[][..]),
        ("[0]", "[0] 0 d", "<< >>", &[]),
        ("[0 0 0] with a phase", "[0 0 0] 4 d", "<< >>", &[]),
        ("/D [[0 0] 0]", "/GS1 gs", "<< /D [[0 0] 0] >>", &[]),
        ("[3 -1]", "[3 -1] 0 d", "<< >>", &[]),
        ("/D [[0 -1] 0]", "/GS1 gs", "<< /D [[0 -1] 0] >>", &[]),
        ("[3 1e40], beyond f32", "[3 10000000000000000000000000000000000000000] 0 d", "<< >>", &[]),
        ("[]", "[] 0 d", "<< >>", &[]),
        ("[0 12]", "[0 12] 0 d", "<< >>", &[0.01, 12.0]),
        ("/D [[0 12] 0]", "/GS1 gs", "<< /D [[0 12] 0] >>", &[0.01, 12.0]),
        ("[20 10]", "[20 10] 5 d", "<< >>", &[20.0, 10.0]),
        ("[10]", "[10] 0 d", "<< >>", &[10.0]),
    ] {
        let seen = strokes(&format!("{setup} {line}"), gs1);
        assert_eq!(seen.len(), 1, "{what}: one stroke");
        assert_eq!(seen[0].0.dash_array.as_slice(), dashes, "{what}");
    }
}

/// The renderer already drew a negative width as its thinnest line (it widens anything under a
/// device pixel); that stays so, in the stroke colour.
#[test]
fn negative_line_widths_draw_the_thinnest_line_in_the_stroke_colour() {
    for (what, content, gs1, reference) in [
        ("-6 w", "0 0 1 RG -6 w 10 20 m 30 20 l S", "<< >>", "0 0 1 RG 0 w 10 20 m 30 20 l S"),
        ("/LW -6", "0 0 1 RG /GS1 gs 10 20 m 30 20 l S", "<< /LW -6 >>", "0 0 1 RG 0 w 10 20 m 30 20 l S"),
        ("-6 w with round caps", "0 0 1 RG 1 J -6 w 10 20 m 30 20 l S", "<< >>", "0 0 1 RG 1 J 0 w 10 20 m 30 20 l S"),
    ] {
        let hairline = render(reference, "<< >>");
        let page = render(content, gs1);
        assert!(page.rgba[..] == hairline.rgba[..], "{what}: drawn as 0 w");
        let ink: Vec<[u8; 4]> = page.rgba.as_chunks::<4>().0.iter().filter(|p| **p != WHITE).copied().collect();
        assert!(!ink.is_empty(), "{what}: the line is drawn");
        assert!(ink.iter().all(|p| p[2] == 255 && p[0] == p[1]), "{what}: every inked pixel is a tint of the blue stroke colour: {ink:?}");
        assert!(ink.len() <= 2 * 22, "{what}: about one pixel thick, {} pixels inked", ink.len());
    }
}
