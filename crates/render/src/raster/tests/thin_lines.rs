//! Strokes thinner than a device pixel are drawn one pixel wide, on whole pixels (#7097, hayro
//! patch 32): a drawing's thin lines stay sharp at every zoom instead of turning into grey bands.

use super::*;

/// Page 1 of a 130 × 70 pt page drawing `content`, at `scale`: its pixels and width.
fn draw(content: &str, scale: f32) -> (Vec<u8>, usize) {
    let pdf = image_cache_pdf(content, "<< >>", &[]);
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview: false });
    assert!(out.error.is_none(), "{:?}", out.error);
    (out.rgba.to_vec(), out.width as usize)
}

/// Four 0.1 pt vertical lines and four horizontal ones, 2 pt apart, on whole points: at 100 %
/// their centres ran along pixel edges and each covered two pixels by half, so the four drew as
/// one 50 % grey band 8 pixels wide; a zoom that moved them onto pixel centres showed them sharp.
/// At every zoom each now inks whole pixels: black or white, nothing between.
#[test]
fn hairlines_on_pixel_edges_ink_whole_pixels_at_every_zoom() {
    let content = "0.1 w 10 5 m 10 65 l S 12 5 m 12 65 l S 14 5 m 14 65 l S 16 5 m 16 65 l S \
                   30 30 m 120 30 l S 30 32 m 120 32 l S 30 34 m 120 34 l S 30 36 m 120 36 l S";
    for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
        let (rgba, w) = draw(content, scale);
        let s = f64::from(scale);
        // Across the vertical lines, halfway up the page.
        let y = (35.0 * s) as usize;
        let row: Vec<u8> = ((8.0 * s) as usize..=(18.0 * s) as usize).map(|x| rgb_at(&rgba, w, x, y)[0]).collect();
        assert!(row.iter().all(|v| *v == 0 || *v == 255), "at {scale}: vertical hairlines blur into grey: {row:?}");
        assert_eq!(row.iter().filter(|v| **v == 0).count(), 4, "at {scale}: each vertical hairline inks one column: {row:?}");
        // Down the horizontal lines (y up from the bottom of the page).
        let x = (75.0 * s) as usize;
        let top = rgba.len() / 4 / w;
        let col: Vec<u8> = (top - (38.0 * s) as usize..=top - (28.0 * s) as usize).map(|y| rgb_at(&rgba, w, x, y)[0]).collect();
        assert!(col.iter().all(|v| *v == 0 || *v == 255), "at {scale}: horizontal hairlines blur into grey: {col:?}");
        assert_eq!(col.iter().filter(|v| **v == 0).count(), 4, "at {scale}: each horizontal hairline inks one row: {col:?}");
    }
}

/// A mark smaller than a pixel still draws: snapping would put all its points on one pixel
/// centre, and a stroke of no length with butt caps draws nothing, so it keeps its own (a
/// drawing's stroked lettering at a low zoom is made of such strokes). Lines a pixel wide or
/// wider are not moved.
#[test]
fn small_marks_still_draw_and_wide_lines_keep_their_place() {
    for (mark, content) in [
        ("a stroke 0.4 pt long", "0.1 w 40.3 39.5 m 40.7 39.5 l S"),
        (
            "a circle 0.4 pt across",
            "0.1 w 40.7 39.5 m 40.7 39.61 40.61 39.7 40.5 39.7 c 40.39 39.7 40.3 39.61 40.3 39.5 c 40.3 39.39 40.39 39.3 40.5 39.3 c 40.61 39.3 40.7 39.39 40.7 39.5 c S",
        ),
    ] {
        // Inside the pixel at (40, 30).
        let (rgba, w) = draw(content, 1.0);
        let inked = (28..=32).flat_map(|y| (38..=42).map(move |x| (x, y))).filter(|&(x, y)| rgb_at(&rgba, w, x, y)[0] < 255).count();
        assert!(inked > 0, "{mark} is drawn");
    }
    // A 1 pt line along a pixel edge covers the two columns beside it by half each, as before.
    let (rgba, w) = draw("1 w 10 5 m 10 65 l S", 1.0);
    let row: Vec<u8> = (8..=11).map(|x| rgb_at(&rgba, w, x, 35)[0]).collect();
    assert_eq!(row, [255, 127, 127, 255]);
}

/// A glyph's content is not snapped, and stays so after the glyph shows text in another Type 3
/// font: the flag that marks a glyph's content is restored, not cleared, when the nested glyph
/// ends. A body ending in a thin stroke draws the same pixels whether or not it nests one.
#[test]
fn a_type3_glyph_that_nests_one_keeps_its_thin_strokes_unsnapped() {
    // The body's stroke runs down the glyph's origin, a pixel edge at font size 20, and is 0.2
    // device pixels wide: widened to a pixel and snapped unless the glyph is recognised.
    let pdf = |body: &str| {
        let page = "BT /F1 20 Tf 10 10 Td (A) Tj ET";
        // The nested font's glyph paints nothing, so it can only change the page through the
        // state its drawing leaves behind.
        let nested_glyph = "0 0 0 0 re f";
        let doc = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 7 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{page}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /FirstChar 65 /LastChar 65 /Widths [1]
  /Encoding << /Differences [65 /a] >> /CharProcs << /a 6 0 R >> >> endobj
6 0 obj << /Length {} >> stream
{body}
endstream endobj
7 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /FirstChar 66 /LastChar 66 /Widths [1]
  /Encoding << /Differences [66 /b] >> /CharProcs << /b 8 0 R >> >> endobj
8 0 obj << /Length {} >> stream
{nested_glyph}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            page.len(),
            body.len(),
            nested_glyph.len()
        );
        doc.into_bytes()
    };
    let render = |doc: Vec<u8>| {
        let mut r = PageRenderer::new(Arc::new(doc), RenderConfig::default());
        let out = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
        assert!(out.error.is_none(), "{:?}", out.error);
        out.rgba.to_vec()
    };
    let stroke = "0.01 w 0 0 m 0 1 l S";
    let plain = render(pdf(stroke));
    let nested = render(pdf(&format!("BT /F2 1 Tf 0 0 Td (B) Tj ET {stroke}")));
    assert!(plain.iter().any(|v| *v < 255), "the glyph's stroke is drawn");
    assert_eq!(plain, nested, "nesting a Type 3 glyph leaves the stroke it draws alone");
}

/// Hairlines on the page's right and bottom edges stay visible instead of snapping outside the
/// canvas, while interior hairlines and tile seams stay snapped. Partial overhangs keep their
/// partial edge coverage without jumping inward, and tiled renders remain pixel-identical to
/// whole-page renders.
#[test]
fn hairlines_on_page_edges_stay_visible_while_interior_hairlines_stay_snapped() {
    let content = "0.1 w 130 5 m 130 65 l S 10 0 m 120 0 l S \
                   0 5 m 0 65 l S 10 70 m 120 70 l S \
                   50 5 m 50 65 l S 10 30 m 120 30 l S";
    for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
        let (rgba, w) = draw(content, scale);
        let h = rgba.len() / 4 / w;
        assert!(rgb_at(&rgba, w, w - 1, h / 2)[0] < 255, "at {scale}: right-edge hairline disappeared");
        assert!(rgb_at(&rgba, w, w / 2, h - 1)[0] < 255, "at {scale}: bottom-edge hairline disappeared");
        if scale != 1.25 {
            assert_eq!(rgb_at(&rgba, w, w - 1, h / 2)[0], 0, "at {scale}: right-edge hairline should ink whole pixel");
            assert_eq!(rgb_at(&rgba, w, w - 2, h / 2)[0], 255, "at {scale}: right-edge neighbour should stay white");
            assert_eq!(rgb_at(&rgba, w, w / 2, h - 1)[0], 0, "at {scale}: bottom-edge hairline should ink whole pixel");
            assert_eq!(rgb_at(&rgba, w, w / 2, h - 2)[0], 255, "at {scale}: bottom-edge neighbour should stay white");
            assert_eq!(rgb_at(&rgba, w, 0, h / 2)[0], 0, "at {scale}: left-edge hairline should ink whole pixel");
            assert_eq!(rgb_at(&rgba, w, 1, h / 2)[0], 255, "at {scale}: left-edge neighbour should stay white");
            assert_eq!(rgb_at(&rgba, w, w / 2, 0)[0], 0, "at {scale}: top-edge hairline should ink whole pixel");
            assert_eq!(rgb_at(&rgba, w, w / 2, 1)[0], 255, "at {scale}: top-edge neighbour should stay white");
        }
        let s = f64::from(scale);
        let x = (50.0 * s) as usize;
        let y_mid = (35.0 * s) as usize;
        let interior_col: Vec<u8> = (x - 1..=x + 1).map(|px| rgb_at(&rgba, w, px, y_mid)[0]).collect();
        assert_eq!(interior_col, [255, 0, 255], "at {scale}: interior vertical hairline should stay snapped");
        let y = ((70.0 - 30.0) * s) as usize;
        let x_mid = (75.0 * s) as usize;
        let interior_row: Vec<u8> = (y - 1..=y + 1).map(|py| rgb_at(&rgba, w, x_mid, py)[0]).collect();
        assert_eq!(interior_row, [255, 0, 255], "at {scale}: interior horizontal hairline should stay snapped");
    }

    // A full-page border rectangle inks crisp black on all four edges.
    let (rgba, w) = draw("0.1 w 0 0 130 70 re S", 1.0);
    assert_eq!(rgb_at(&rgba, w, 0, 35)[0], 0);
    assert_eq!(rgb_at(&rgba, w, 129, 35)[0], 0);
    assert_eq!(rgb_at(&rgba, w, 65, 0)[0], 0);
    assert_eq!(rgb_at(&rgba, w, 65, 69)[0], 0);

    // Partial overhangs stay unmoved and retain partial coverage on the last column/row.
    let (rgba, w) = draw("0.1 w 130.2 5 m 130.2 65 l S 10 -0.2 m 120 -0.2 l S", 1.0);
    let right_overhang = rgb_at(&rgba, w, 129, 35)[0];
    let bottom_overhang = rgb_at(&rgba, w, 65, 69)[0];
    assert!((1..255).contains(&right_overhang), "expected partial coverage on right overhang, got {right_overhang}");
    assert!((1..255).contains(&bottom_overhang), "expected partial coverage on bottom overhang, got {bottom_overhang}");

    // Tiled rendering matches full-page rendering pixel-for-pixel across tile seams and outer page edges.
    let pdf = image_cache_pdf(content, "<< >>", &[]);
    let mut r = PageRenderer::new(Arc::new(pdf), RenderConfig::default());
    let full = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
    assert!(full.error.is_none(), "{:?}", full.error);
    for tile in [
        Tile { x: 0, y: 0, w: 50, h: 40 },
        Tile { x: 50, y: 0, w: 80, h: 40 },
        Tile { x: 0, y: 40, w: 50, h: 30 },
        Tile { x: 50, y: 40, w: 80, h: 30 },
    ] {
        let part = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: Some(tile), scale: 1.0, tag: 0, preview: false });
        assert!(part.error.is_none(), "{:?}", part.error);
        assert_eq!((part.width, part.height), (tile.w, tile.h));
        for py in 0..tile.h {
            for px in 0..tile.w {
                let a = &full.rgba[(((py + tile.y) * full.width + px + tile.x) * 4) as usize..][..4];
                let b = &part.rgba[((py * tile.w + px) * 4) as usize..][..4];
                assert_eq!(a, b, "tile {tile:?} mismatch at ({px}, {py})");
            }
        }
    }
}
