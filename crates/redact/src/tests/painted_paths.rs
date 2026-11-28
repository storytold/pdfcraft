//! A path is under a mark when what it paints is, not when its bounding box is. A stroked frame
//! round the page has a box over the whole page and paints only its edges; a map's areas are
//! drawn past the clip that keeps them inside the map. Measured by their boxes, both were removed
//! by a mark in an empty margin, and the page lost its frame and its map. A path whose paint does
//! reach a mark still goes whole (F-008).

use super::*;

/// Apply one mark at `area` to a page drawing `src`; the report and the page's content after.
fn redact(src: &[u8], area: [f64; 4]) -> (Report, String) {
    let mut doc = one_page(src, "", vec![]);
    mark(&mut doc, 0, &[area], "");
    let r = apply(&mut doc, None).unwrap();
    let c = content(&doc, 0);
    (r, c)
}

#[test]
fn a_path_whose_box_crosses_a_mark_but_whose_paint_does_not_stays() {
    // In an empty spot inside a frame, an L-shaped area and a clipped-away fill.
    let src = b"0.5 w 10 10 m 10 290 l 290 290 l 290 10 l h S \
                0 g 20 20 m 120 20 l 120 40 l 40 40 l 40 120 l 20 120 l h f \
                q 150 20 50 50 re W n 0 0 1 rg 0 0 300 300 re f Q";
    let (r, c) = redact(src, [80.0, 80.0, 100.0, 100.0]);
    assert_eq!(r.paths_removed, 0, "{r:?}\n{c}");
    // Nothing was removed, so the content is the page's own, untouched.
    assert!(c.contains("10 10 m 10 290 l 290 290 l 290 10 l h S"), "the frame stays: {c}");
    assert!(c.contains("20 20 m 120 20 l 120 40 l 40 40 l 40 120 l 20 120 l h f"), "the L stays: {c}");
    assert!(c.contains("150 20 50 50 re W n 0 0 1 rg 0 0 300 300 re f Q"), "the fill clipped short of the mark stays: {c}");
}

#[test]
fn a_path_whose_paint_reaches_a_mark_still_goes_whole() {
    // A triangle the mark lies across, the frame's edge running through a second mark, and a fill
    // after `Q`, which ends the clip that kept the same fill away before it.
    let src = b"0 g 200 200 m 280 200 l 280 280 l h f \
                0.5 w 10 10 m 10 290 l 290 290 l 290 10 l h S \
                q 150 20 50 50 re W n 0 0 300 300 re f Q 0 0 1 rg 0 0 300 300 re f";
    let mut doc = one_page(src, "", vec![]);
    mark(&mut doc, 0, &[[250.0, 220.0, 270.0, 240.0], [5.0, 140.0, 15.0, 160.0], [100.0, 100.0, 110.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    let c = content(&doc, 0);
    assert_eq!(r.paths_removed, 3, "{r:?}\n{c}");
    assert!(!c.contains("200 200 m") && !c.contains("280 280 l"), "the triangle goes whole: {c}");
    assert!(!c.contains("10 290 l"), "the frame goes whole: {c}");
    assert!(c.contains("150 20 50 50 re\nW\nn\n0 0 300 300 re\nf"), "the clipped fill stays: {c}");
    assert!(!c.contains("1 rg\n0 0 300 300 re"), "the fill after Q goes: {c}");
}

#[test]
fn a_form_draws_nothing_past_its_bbox() {
    // The form's /BBox reaches the mark's lower half; its fill starts just above the /BBox and
    // reaches the mark's upper half, which the /BBox clips away.
    let mut doc =
        one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 300 150]", b"0 g 0 155 300 100 re f")]);
    mark(&mut doc, 0, &[[200.0, 140.0, 220.0, 160.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.paths_removed, 0, "{r:?}");
}

/// After `h` the current point is the closed subpath's start (ISO 32000-2 Table 58), and `v`
/// takes it as its first control point: the curve `0 100 100 0 v` from (0, 0) passes (12.5, 37.5).
#[test]
fn a_curve_after_a_closed_subpath_is_measured_from_its_start() {
    let (r, c) = redact(b"1 j 0 0 m 100 0 l 100 100 l h 0 100 100 0 v S", [11.0, 36.0, 14.0, 39.0]);
    assert_eq!(r.paths_removed, 1, "{r:?}\n{c}");
}

/// The miter limit is tested on a corner's angle in user space (ISO 32000-2 §8.4.3.5). A right
/// angle stretched ten times across is a sharp corner on the page, but it is still mitred: its tip
/// reaches (114.1, 10).
#[test]
fn a_corner_stretched_by_the_ctm_keeps_its_miter() {
    let (r, c) = redact(b"10 0 0 1 0 0 cm 2 w 0 j 10 M 0 0 m 10 10 l 0 20 l S", [112.0, 9.9, 113.0, 10.1]);
    assert_eq!(r.paths_removed, 1, "{r:?}\n{c}");
}

/// A join at a curve's end takes the curve's tangent. Here it turns 120° from the line, a 60°
/// corner whose miter (ratio 2, within the limit of 3) reaches (108.7, 95); the curve's first
/// chord turns back further, a corner that would be bevelled.
#[test]
fn a_join_into_a_curve_is_mitred_by_the_curve_tangent() {
    let src = b"10 w 0 j 3 M 0 100 m 100 100 l 99.5 100.866 0 100 -100 100 c S";
    let (r, c) = redact(src, [107.5, 94.5, 108.5, 95.5]);
    assert_eq!(r.paths_removed, 1, "{r:?}\n{c}");
}
