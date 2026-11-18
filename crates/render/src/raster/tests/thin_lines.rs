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
