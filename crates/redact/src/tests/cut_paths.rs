//! A mark over part of a path takes that part, not the whole path (#7300). A spreadsheet export
//! fills the cells of one colour with one path of `re`s and strokes its grid lines in one go;
//! a mark over one cell wiped the whole table. Rectangles and straight segments are now cut
//! around the mark; what can't be cut exactly (a filled curve, a filled shape under a rotation,
//! a dashed stroke, a path that paints and clips) still goes whole, and nothing of a cut path
//! paints under a mark (F-008).

use pdfcraft_content::{Matrix, overlaps, parse};

use super::*;
use crate::verify::{ProofOptions, Snapshot};

/// What still paints within reach of `mark` on page 0, outside the overlay boxes, measured
/// independently of the cutter: each stroked segment sampled against its painted half-width,
/// each filled subpath by its box.
pub(super) fn painted_under(doc: &Document, overlays: &[pdfcraft_cos::ObjRef], mark: [f64; 4]) -> Vec<String> {
    let p = &pdfcraft_model::pages(doc)[0];
    let (refs, data) = page_streams(doc, &p.dict, 0).unwrap();
    let mut found = Vec::new();
    let mut stack: Vec<(Matrix, f64)> = Vec::new();
    let (mut ctm, mut width) = (Matrix::IDENTITY, 1.0);
    let mut subs: Vec<Vec<(f64, f64)>> = Vec::new();
    for (r, d) in refs.iter().zip(&data) {
        if overlays.iter().any(|o| *r == Object::Ref(*o)) {
            continue;
        }
        for op in parse(d).ops {
            let n: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
            let dev = |x: f64, y: f64| ctm.apply(x, y);
            match op.op.as_slice() {
                b"q" => stack.push((ctm, width)),
                b"Q" => (ctm, width) = stack.pop().unwrap_or((ctm, width)),
                b"cm" => ctm = Matrix([n[0], n[1], n[2], n[3], n[4], n[5]]).then(&ctm),
                b"w" => width = *n.last().unwrap(),
                b"m" => subs.push(vec![dev(n[0], n[1])]),
                b"l" => subs.last_mut().unwrap().push(dev(n[0], n[1])),
                b"c" | b"v" | b"y" => subs.last_mut().unwrap().extend(n.chunks(2).map(|p| dev(p[0], p[1]))),
                b"h" => {
                    let s = subs.last_mut().unwrap();
                    s.push(s[0]);
                }
                b"re" => {
                    let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
                    subs.push(vec![dev(x, y), dev(x + w, y), dev(x + w, y + h), dev(x, y + h), dev(x, y)]);
                }
                o @ (b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*") => {
                    let [a, b, c, d, ..] = ctm.0;
                    let reach = (width / 2.0 * (a * a + b * b).sqrt().max((c * c + d * d).sqrt())).max(0.5);
                    let dist = |(x, y): (f64, f64)| (mark[0] - x).max(x - mark[2]).max(0.0).hypot((mark[1] - y).max(y - mark[3]).max(0.0));
                    for s in &subs {
                        if o != b"S" && o != b"s" {
                            let (x0, x1) = s.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
                            let (y0, y1) = s.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
                            if overlaps([x0, y0, x1, y1], mark, 0.0) {
                                found.push(format!("fill {s:?}"));
                            }
                        }
                        if matches!(o, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*") {
                            for w in s.windows(2) {
                                let near = (0..=400)
                                    .map(|k| k as f64 / 400.0)
                                    .any(|t| dist((w[0].0 + t * (w[1].0 - w[0].0), w[0].1 + t * (w[1].1 - w[0].1))) < reach);
                                if near {
                                    found.push(format!("stroke {:?}–{:?}", w[0], w[1]));
                                }
                            }
                        }
                    }
                    subs.clear();
                }
                b"n" => subs.clear(),
                _ => {}
            }
        }
    }
    found
}

/// Mark `rect`, apply, and check nothing paints under it, by [`painted_under`] and by the proof.
fn cut_under(src: &[u8], rect: [f64; 4]) -> (Report, String) {
    cut_doc(one_page(src, "", vec![]), rect)
}

/// [`cut_under`] on a document of its own.
fn cut_doc(mut doc: Document, rect: [f64; 4]) -> (Report, String) {
    mark(&mut doc, 0, &[rect], "");
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    let r = apply(&mut doc, None).unwrap();
    snap.set_overlay_streams(&doc, &r.overlays).unwrap();
    let proof = snap.prove(&doc, None, &ProofOptions::default());
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(painted_under(&doc, &r.overlays, rect), Vec::<String>::new(), "{}", content(&doc, 0));
    let c = content(&doc, 0);
    (r, c)
}

#[test]
fn a_mark_over_one_cell_keeps_the_rest_of_the_table() {
    // Three cells filled by one path (the last given upside down), and a grid line across them.
    let src = b"0 0 1 rg 10 200 50 20 re 60 200 50 20 re 110 220 50 -20 re f 0 G 10 100 m 290 100 l S";
    let (r, c) = cut_under(src, [120.0, 90.0, 140.0, 210.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 2), "{r:?}");
    assert!(c.contains("10 200 50 20 re\n60 200 50 20 re"), "the cells the mark misses are kept as written: {c}");
    // The marked cell keeps its strip above the mark and its sides, upside down as it was given.
    assert!(c.contains("110 220 50 -9.9988 re"), "{c}");
    assert!(c.contains("110 210.0012 9.9988 -10.0012 re"), "{c}");
    assert!(c.contains("140.0012 210.0012 19.9988 -10.0012 re"), "{c}");
    // The line keeps both ends, short of the mark by its painted half-width.
    assert!(c.contains("10 100 m\n119.2917 100 l\nS"), "{c}");
    assert!(c.contains("140.7083 100 m\n290 100 l\nS"), "{c}");
}

#[test]
fn a_stroked_border_keeps_the_sides_the_mark_misses() {
    // A cell border; the mark lies on its left side.
    let (r, c) = cut_under(b"2 w 100 100 50 50 re S", [95.0, 120.0, 105.0, 130.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(!c.contains("100 100 50 50 re"), "{c}");
    // The left side is cut by half the line width around the mark, not by the miter limit:
    // only a join can reach that far, and its right-angle corners (√2 half-widths) stay joined.
    // The right side is a stroke of its own, as a piece holding it and a left corner would
    // have a box over the mark.
    assert!(c.contains("150 150 m\n100 150 l\n100 131.4154 l\nS"), "{c}");
    assert!(c.contains("100 118.5846 m\n100 100 l\n150 100 l\nS"), "{c}");
    assert!(c.contains("150 100 m\n150 150 l\nS"), "{c}");
}

/// A sharp corner's miter reaches far past half the line width: the join is broken there (two
/// butt ends) when the miter could reach the mark, though the sides stay clear of it.
#[test]
fn a_sharp_corner_near_the_mark_is_not_left_mitred() {
    // A 4-point-wide chevron pointing right at (150, 100), φ ≈ 11.4°: its miter would be
    // 1 / sin(φ/2) ≈ 10.05 half-widths long. The mark is 6 points past the tip, clear of the sides.
    let mark = [156.0, 98.0, 160.0, 102.0];
    // Past the default miter limit (10) the tip is bevelled and reaches no further than the
    // sides: it stays a join.
    let (r, c) = cut_under(b"4 w 50 90 m 150 100 l 50 110 l S", mark);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(c.contains("50 90 m\n150 100 l\n50 110 l"), "the bevelled tip stays a join: {c}");
    // Within a limit of 20 it is mitred and its spike would reach the mark: the join is broken.
    let (r, c) = cut_under(b"20 M 4 w 50 90 m 150 100 l 50 110 l S", mark);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(!c.contains("50 90 m\n150 100 l\n50 110 l"), "the mitred tip is no longer a join: {c}");
    // Just past the limit as given, the tip is bevelled; as written (four decimals) it is
    // within it, mitred, and its spike would reach the mark: the join is decided as written.
    let (_, c) = cut_under(b"4 w 50 89.94964 m 150 100 l 50 110.05036 l S", mark);
    assert!(!c.contains("150 100 l\n50 110.0504 l"), "the tip mitred as written is no longer a join: {c}");
}

#[test]
fn a_frame_around_the_mark_stays() {
    // The mark is inside the frame, clear of its sides: nothing of it is under the mark, but
    // its box is, so it is written again as pieces whose boxes aren't.
    let (r, c) = cut_under(b"1 w 50 50 200 200 re S", [140.0, 140.0, 160.0, 160.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    for side in ["50 50 m", "250 50 l", "250 250 l", "50 250 l"] {
        assert!(c.contains(side), "{side}: {c}");
    }
}

#[test]
fn a_line_under_a_scaled_ctm_is_cut_in_its_own_space() {
    let (r, c) = cut_under(b"q 2 0 0 2 0 0 cm 5 50 m 145 50 l S Q", [120.0, 90.0, 140.0, 110.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(c.contains("5 50 m\n59.2922 50 l\nS") && c.contains("70.7078 50 m\n145 50 l\nS"), "{c}");
}

#[test]
fn a_filled_and_stroked_box_keeps_both_outside_the_mark() {
    let (r, c) = cut_under(b"0 0 1 rg 10 10 100 50 re B", [50.0, 0.0, 60.0, 100.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(c.contains("\nf\n") && c.contains("\nS\n"), "filled, then stroked: {c}");
}

#[test]
fn what_cannot_be_cut_exactly_still_goes_whole() {
    for (src, why) in [
        (&b"0 g 100 100 m 150 150 l 200 100 l h f"[..], "a filled triangle"),
        (b"0 g 100 100 m 150 150 160 120 200 100 c f", "a filled curve"),
        (b"q 0.7071 0.7071 -0.7071 0.7071 150 150 cm 0 g -20 -20 40 40 re f Q", "a filled square under a rotation"),
        (b"0 g 100 100 100 100 re W f", "a clip that also paints"),
        (b"q 10 w [6 4] 0 d 100 145 m 200 145 l S Q", "a dashed stroke"),
        (b"q /Dash gs 10 w 100 145 m 200 145 l S Q", "a stroke dashed by its ExtGState"),
        (b"q /Half gs 4 w 0 g 100 100 100 100 re B Q", "a box filled and stroked half see-through"),
        (b"q /Multiply gs 4 w 0 g 100 100 100 100 re B Q", "a box filled and stroked with a blend mode"),
    ] {
        let mut doc = one_page(src, "/ExtGState << /Dash << /D [[6 4] 0] >> /Half << /CA 0.5 >> /Multiply << /BM /Multiply >> >>", vec![]);
        mark(&mut doc, 0, &[[130.0, 130.0, 160.0, 160.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert_eq!((r.paths_removed, r.paths_cut), (1, 0), "{why}: {r:?}");
        let c = content(&doc, 0);
        assert!(!c.contains("100 100 100 100 re"), "{why}: nothing of it stays: {c}");
        assert!(painted_under(&doc, &r.overlays, [130.0, 130.0, 160.0, 160.0]).is_empty(), "{why}: {c}");
    }
}

/// A clip that paints nothing (`W n`) is no paint for the mark to take: it stays as written.
#[test]
fn a_clip_that_paints_nothing_stays() {
    let mut doc = one_page(b"q 100 100 100 100 re W n 0 g 10 10 20 20 re f Q", "", vec![]);
    mark(&mut doc, 0, &[[130.0, 130.0, 160.0, 160.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.paths_removed, r.paths_cut), (0, 0), "{r:?}");
    assert!(content(&doc, 0).contains("100 100 100 100 re W n"), "{}", content(&doc, 0));
}

/// A path may begin in one `/Contents` stream and end in the next. What the first holds of it
/// goes too, or the cut's paint (or the next path's) would paint it under the mark.
#[test]
fn a_path_split_between_content_streams_is_cut_whole() {
    let two = |head: &[u8], tail: &[u8]| {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 5 0 R] >>".to_vec(),
            stream("", head),
            stream("", tail),
        ])
    };
    let mark = [110.0, 110.0, 140.0, 140.0];
    let (r, c) = cut_doc(two(b"0 g 100 100 50 50 re", b"200 100 50 50 re f"), mark);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(!c.contains("100 100 50 50 re") && c.contains("200 100 50 50 re"), "{c}");
    // Removed whole, the curve's start must not run on into the stroke after it.
    let (r, c) = cut_doc(two(b"0 g 100 100 m 110 160 140 160 150 100 c", b"h f 0 G 10 10 m 20 10 l S"), mark);
    assert_eq!((r.paths_removed, r.paths_cut), (1, 0), "{r:?}");
    assert!(!c.contains(" c\n") && c.contains("10 10 m\n20 10 l\nS"), "{c}");
}

/// 400 marks across one rectangle and 400 down it would cut it into 160,000 pieces, each
/// strip checked against all made so far: past the budget it goes whole, at once.
#[test]
fn a_rectangle_under_a_grid_of_marks_goes_whole() {
    let mut doc = one_page(b"0 g 0 0 300 300 re f", "", vec![]);
    let grid: Vec<[f64; 4]> = (0..400).map(|i| 0.25 + 0.75 * i as f64).flat_map(|v| [[v, 0.0, v + 0.25, 300.0], [0.0, v, 300.0, v + 0.25]]).collect();
    mark(&mut doc, 0, &grid, "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.paths_removed, r.paths_cut), (1, 0), "{r:?}");
}

#[test]
fn a_stroked_curve_the_mark_reaches_goes_and_its_straight_neighbours_stay() {
    let (r, c) = cut_under(b"10 140 m 100 140 l 120 160 160 160 180 140 c 290 140 l S", [130.0, 150.0, 150.0, 170.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(c.contains("10 140 m\n100 140 l") && c.contains("180 140 m\n290 140 l"), "{c}");
    assert!(!c.contains(" c\n"), "the curve under the mark goes: {c}");
}

#[test]
fn a_ring_keeps_its_hole_when_both_are_cut() {
    // Even-odd: the hole stays a hole outside the mark, as the pieces that overlap there (an
    // outer piece and a hole piece) are filled together.
    let (r, c) = cut_under(b"0 g 100 100 100 100 re 120 120 60 60 re f*", [130.0, 110.0, 160.0, 210.0]);
    assert_eq!((r.paths_removed, r.paths_cut), (0, 1), "{r:?}");
    assert!(c.contains("f*") && !c.contains("\nf\n"), "{c}");
}
