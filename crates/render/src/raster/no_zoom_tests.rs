//! The NoZoom annotation flag (ISO 32000-2 §12.5.3, Table 167 bit 4) and text annotations, which
//! behave as if it were set (§12.5.6.4): on screen, at a `RenderRequest::magnification`, they keep
//! their size, pinned at the upper-left corner of `/Rect` (hayro-interpret patch 54).

use super::*;

/// The left edges of the four 20 × 20 annotations, whose upper-left corners all sit on y = 200:
/// A, a note with NoZoom (`/F 28`: Print, NoZoom, NoRotate); B, a note without it (`/F 4`); C, a
/// square with NoZoom (`/F 12`); D, a square without it (`/F 4`).
const LEFT: [f32; 4] = [40.0, 120.0, 200.0, 280.0];
const TOP: f32 = 200.0;
const SIDE: f32 = 20.0;
const PAGE_HEIGHT: f32 = 300.0;

/// A 400 × 300 page holding A, B, C and D, all drawn by one solid black 20 × 20 appearance, plus
/// E, a link, and F, a form field, with NoZoom set (`/F 12`) left of the others' row, below it.
fn four_annotations() -> Vec<u8> {
    let annot = |n: usize, subtype: &str, flags: u32, x: f32, y: f32| {
        format!("{n} 0 obj << /Type /Annot /Subtype /{subtype} /F {flags} /Rect [{x} {} {} {y}] /AP << /N 4 0 R >> >> endobj\n", y - SIDE, x + SIDE)
    };
    let appearance = "0 g 0 0 20 20 re f";
    let mut pdf = String::from(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R] >> endobj
",
    );
    pdf.push_str(&format!(
        "4 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length {} >> stream\n{appearance}\nendstream endobj\n",
        appearance.len()
    ));
    pdf.push_str(&annot(5, "Text", 28, LEFT[0], TOP));
    pdf.push_str(&annot(6, "Text", 4, LEFT[1], TOP));
    pdf.push_str(&annot(7, "Square", 12, LEFT[2], TOP));
    pdf.push_str(&annot(8, "Square", 4, LEFT[3], TOP));
    pdf.push_str(&annot(9, "Link", 12, LEFT[0], 100.0));
    pdf.push_str(&annot(10, "Widget", 12, LEFT[2], 100.0));
    pdf.push_str("trailer << /Root 1 0 R >>\n%%EOF\n");
    pdf.into_bytes()
}

fn render(scale: f32, magnification: Option<f32>) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(four_annotations()), RenderConfig::default());
    let page = renderer.render(RenderRequest { scale, magnification, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    page
}

/// The device-pixel box `[x0, y0, x1, y1)` of the dark pixels in the band of columns that starts
/// at the user-space x `left` and runs right, between the device rows `rows`; `None` if there
/// are none.
fn ink_box(page: &RenderedPage, left: f32, scale: f32, rows: std::ops::Range<u32>) -> Option<[u32; 4]> {
    let x0 = (left * scale) as u32;
    // Up to the next annotation's band, or the page's edge.
    let x1 = ((left + 60.0) * scale).min(page.width as f32) as u32;
    let mut b: Option<[u32; 4]> = None;
    for y in rows.start..rows.end.min(page.height) {
        for x in x0..x1 {
            let at = ((y * page.width + x) * 4) as usize;
            if page.rgba[at] < 128 {
                let r = b.get_or_insert([x, y, x + 1, y + 1]);
                *r = [r[0].min(x), r[1].min(y), r[2].max(x + 1), r[3].max(y + 1)];
            }
        }
    }
    b
}

/// The box the annotation at `left` (upper-left corner on y = 200) covers at `scale`, `side`
/// device pixels a side.
fn expected(left: f32, scale: f32, side: f32) -> [u32; 4] {
    let (x, y) = ((left * scale).round() as u32, ((PAGE_HEIGHT - TOP) * scale).round() as u32);
    [x, y, x + side.round() as u32, y + side.round() as u32]
}

fn close(got: Option<[u32; 4]>, want: [u32; 4]) -> bool {
    got.is_some_and(|g| g.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1))
}

/// The row band of the annotations on y = 200 at `scale` (the link and the field are below it).
fn top_row(scale: f32) -> std::ops::Range<u32> {
    0..((PAGE_HEIGHT - TOP + 3.0 * SIDE) * scale) as u32
}

/// On screen at 100% and at 400% (here one device pixel per point per 100%), A (NoZoom note), B
/// (note without the flag, §12.5.6.4) and C (NoZoom square) stay 20 pixels a side with their
/// upper-left corner where the page puts it; only D grows with the page.
#[test]
fn no_zoom_annotations_and_notes_keep_their_size_on_screen() {
    for m in [1.0f32, 4.0] {
        let page = render(m, Some(m));
        for (i, left) in LEFT.into_iter().enumerate() {
            let side = if i == 3 { SIDE * m } else { SIDE };
            let got = ink_box(&page, left, m, top_row(m));
            assert!(close(got, expected(left, m, side)), "{m}: annotation {i} drawn at {got:?}, want {:?}", expected(left, m, side));
        }
    }
}

/// Without a magnification (print, export, thumbnails, the CLI), and at 100% on a display with
/// more device pixels per point, every annotation scales with the page, as before.
#[test]
fn without_a_magnification_every_annotation_scales_with_the_page() {
    for (scale, magnification) in [(4.0f32, None), (2.0, Some(1.0))] {
        let page = render(scale, magnification);
        for (i, left) in LEFT.into_iter().enumerate() {
            let got = ink_box(&page, left, scale, top_row(scale));
            let want = expected(left, scale, SIDE * scale);
            assert!(close(got, want), "{scale} {magnification:?}: annotation {i} drawn at {got:?}, want {want:?}");
        }
    }
}

/// A link and a form field keep to `/Rect` even with NoZoom: their click targets come from it.
#[test]
fn links_and_form_fields_scale_with_the_page_even_with_no_zoom() {
    let m = 4.0;
    let page = render(m, Some(m));
    let rows = ((PAGE_HEIGHT - 100.0) * m) as u32..page.height;
    for left in [LEFT[0], LEFT[2]] {
        let got = ink_box(&page, left, m, rows.clone());
        let (x, y) = ((left * m) as u32, ((PAGE_HEIGHT - 100.0) * m) as u32);
        let want = [x, y, x + (SIDE * m) as u32, y + (SIDE * m) as u32];
        assert!(close(got, want), "{left}: drawn at {got:?}, want {want:?}");
    }
}

/// A magnification that isn't finite and greater than zero is ignored rather than drawing an
/// infinite or NaN transform; a tiny one draws a huge but finite one. Neither fails the render.
#[test]
fn a_meaningless_magnification_is_ignored() {
    for bad in [0.0f32, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let page = render(2.0, Some(bad));
        for (i, left) in LEFT.into_iter().enumerate() {
            let got = ink_box(&page, left, 2.0, top_row(2.0));
            assert!(close(got, expected(left, 2.0, SIDE * 2.0)), "{bad}: annotation {i} drawn at {got:?}");
        }
    }
    for tiny in [f32::MIN_POSITIVE, 1e-30] {
        render(1.0, Some(tiny));
    }
}

/// The inspector marks the comments the renderer keeps at a fixed size, for hit-testing.
#[test]
fn the_inspector_marks_comments_that_keep_their_size() {
    let info = crate::inspect(Arc::new(four_annotations()), None).expect("opens");
    let marked: Vec<(String, bool)> = info.annotations.iter().map(|a| (a.subtype.clone(), a.no_zoom)).collect();
    for (subtype, flagged) in [("Text", true), ("Square", true), ("Square", false)] {
        assert!(marked.contains(&(subtype.to_string(), flagged)), "{subtype} {flagged}: {marked:?}");
    }
    assert!(!marked.contains(&("Text".to_string(), false)), "{marked:?}");
    assert!(!marked.iter().any(|(s, z)| matches!(s.as_str(), "Link" | "Widget") && *z), "{marked:?}");
}

/// A 400 × 300 page with one annotation, 20 × 20 with the upper-left corner at (`LEFT[2]`, 200):
/// `body` is everything in its dictionary but `/Type` and `/Rect`.
fn one_annotation(body: &str) -> Vec<u8> {
    let (x, y) = (LEFT[2], TOP);
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Annots [5 0 R] >> endobj
4 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 18 >> stream
0 g 0 0 20 20 re f
endstream endobj
5 0 obj << /Type /Annot {body} /Rect [{x} {} {} {y}] >> endobj
trailer << /Root 1 0 R >>
%%EOF
",
        y - SIDE,
        x + SIDE
    )
    .into_bytes()
}

/// The box `pdf`'s one annotation is drawn in at 400% on screen.
fn drawn_at_400(pdf: Vec<u8>) -> Option<[u32; 4]> {
    let m = 4.0;
    let mut renderer = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let page = renderer.render(RenderRequest { scale: m, magnification: Some(m), ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    ink_box(&page, LEFT[2], m, top_row(m))
}

/// A NoZoom highlight without an appearance, drawn from its `/QuadPoints`, keeps its size on
/// the screen as one with an appearance does.
#[test]
fn a_no_zoom_highlight_without_an_appearance_keeps_its_size() {
    let (x0, x1, y0, y1) = (LEFT[2], LEFT[2] + SIDE, TOP - SIDE, TOP);
    let body = format!("/Subtype /Highlight /F 12 /C [0 0 0] /QuadPoints [{x0} {y1} {x1} {y1} {x0} {y0} {x1} {y0}]");
    let got = drawn_at_400(one_annotation(&body));
    assert!(close(got, expected(LEFT[2], 4.0, SIDE)), "drawn at {got:?}");
}

/// `/F` written as a real (`12.0`) is read as the integer flags it holds, as the inspector reads
/// it, so the NoZoom square keeps its size.
#[test]
fn a_real_flags_value_is_read_as_its_integer() {
    let got = drawn_at_400(one_annotation("/Subtype /Square /F 12.0 /AP << /N 4 0 R >>"));
    assert!(close(got, expected(LEFT[2], 4.0, SIDE)), "drawn at {got:?}");
}

/// A NoZoom highlight without an appearance whose `/Rect` corners overflow to infinity (read as
/// `f32`) has no finite pivot to keep its size about: the scale is dropped and the highlight is
/// drawn from its `/QuadPoints` at the page's scale instead of through a NaN transform that
/// painted nothing.
#[test]
fn a_no_zoom_highlight_whose_rect_overflows_is_drawn_unscaled() {
    let m = 4.0;
    let (x0, x1, y0, y1) = (LEFT[2], LEFT[2] + SIDE, TOP - SIDE, TOP);
    let body = format!("/Subtype /Highlight /F 12 /C [0 0 0] /QuadPoints [{x0} {y1} {x1} {y1} {x0} {y0} {x1} {y0}]");
    let pdf = String::from_utf8(one_annotation(&body)).expect("the fixture is ASCII");
    let huge = format!("1{}.0", "0".repeat(40));
    let rect = format!("/Rect [{x0} {y0} {x1} {y1}]");
    assert!(pdf.contains(&rect), "{pdf}");
    let pdf = pdf.replace(&rect, &format!("/Rect [-{huge} {y0} {x1} {huge}]"));
    let mut renderer = PageRenderer::new(Arc::new(pdf.into_bytes()), RenderConfig::default());
    let page = renderer.render(RenderRequest { scale: m, magnification: Some(m), ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    let got = ink_box(&page, LEFT[2], m, top_row(m));
    assert!(close(got, expected(LEFT[2], m, SIDE * m)), "drawn at {got:?}");
}
