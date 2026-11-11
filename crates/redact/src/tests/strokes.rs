//! A stroke paints its line width around its path: half of it on each side, more at a projecting
//! cap. A stroke is under a mark when that paint is, not only its path: a horizontal or vertical
//! line, whose path has no height or width, under a mark, and a thick line beside one whose paint
//! reaches under it.

use super::*;

/// How many paths applying a mark over `[90, 90, 150, 150]` removes from a page drawing `content`,
/// with `extra_res` in its resources and a form `/Fm` (object 7) drawing a thin line beside it.
fn removed(content: &[u8], extra_res: &str) -> usize {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"100 160 m 140 160 l S");
    let mut doc = one_page(content, &format!("{extra_res} /XObject << /Fm 7 0 R >>"), vec![form]);
    mark(&mut doc, 0, &[[90.0, 90.0, 150.0, 150.0]], "");
    apply(&mut doc, None).unwrap().paths_removed
}

/// A horizontal or vertical line under the mark, of any width, the thinnest included (`0 w`), and a
/// fill of a path with no height or width, which the renderer paints as a line: its path has no
/// height (or width), and was taken for one that paints nowhere.
#[test]
fn a_line_under_a_mark_is_removed_however_thin() {
    for content in [
        &b"100 120 m 140 120 l S"[..],
        b"120 100 m 120 140 l S",
        b"0 w 100 120 m 140 120 l S",
        b"100 120 m 140 120 l s",
        b"100 120 40 0 re f",
        b"100 120 m 140 120 l f",
        b"120 100 0 40 re f*",
    ] {
        assert_eq!(removed(content, ""), 1, "{}", String::from_utf8_lossy(content));
    }
}

/// A line beside the mark whose width reaches under it, however the width is set: by `w` (after
/// a surplus operand too, as the renderer reads it, and a negative one at its size), by an ExtGState's `/LW`, in a user space
/// scaled up (each way, the width scaled across the line), or in a form drawn with it. A
/// projecting cap's corner reaches `√2` times half the width from the end of a diagonal line. A
/// thin line there stays, a width restored by `Q` is the earlier one, and a fill paints no width:
/// one that only touches the mark stays.
#[test]
fn a_stroke_whose_width_reaches_under_a_mark_is_removed() {
    let wide = "/ExtGState << /GS1 << /LW 30 >> /GSN << /LW -30 >> >>";
    for (content, gone) in [
        (&b"30 w 100 160 m 140 160 l S"[..], 1),
        (b"(x) 30 w 100 160 m 140 160 l S", 1),
        (b"-30 w 100 160 m 140 160 l S", 1),
        (b"/GS1 gs 100 160 m 140 160 l S", 1),
        (b"/GSN gs 100 160 m 140 160 l S", 1),
        (b"q 10 0 0 10 0 0 cm 3 w 10 16 m 14 16 l S Q", 1),
        (b"q 1 0 0 10 0 0 cm 3 w 100 16 m 140 16 l S Q", 1),
        (b"q 10 0 0 1 0 0 cm 3 w 16 100 m 16 140 l S Q", 1),
        (b"q 0 10 1 0 0 0 cm 3 w 16 100 m 16 140 l S Q", 1),
        (b"2 J 20 w 160 145 m 200 185 l S", 1),
        (b"30 w /Fm Do", 1),
        (b"/Fm Do", 0),
        (b"30 w 100 160 m 140 160 l 140 170 l 100 170 l h B", 1),
        (b"30 w 100 160 m 140 160 l 140 170 l 100 170 l h B*", 1),
        (b"30 w 100 160 m 140 160 l 140 170 l 100 170 l h b", 1),
        (b"30 w 100 160 m 140 160 l 140 170 l 100 170 l h b*", 1),
        (b"30 w 100 160 m 140 160 l s", 1),
        (b"100 160 m 140 160 l S", 0),
        (b"q 30 w Q 100 160 m 140 160 l S", 0),
        (b"30 w 100 160 m 140 160 l 140 170 l 100 170 l h f", 0),
        (b"100 150 40 10 re f", 0),
        (b"150 100 10 40 re f", 0),
    ] {
        assert_eq!(removed(content, wide), gone, "{}", String::from_utf8_lossy(content));
    }
}

/// A width given by reference is read through it: an ExtGState given by reference, the whole
/// `/ExtGState` by reference, and `/LW` itself; and a form's own ExtGState sets the width inside it.
#[test]
fn a_width_given_by_reference_reaches_under_a_mark() {
    let line = "100 160 m 140 160 l S";
    let form =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /ExtGState << /GS1 << /LW 30 >> >> >>", format!("/GS1 gs {line}"));
    for (content, res, extra) in [
        (format!("/GS1 gs {line}"), "/ExtGState << /GS1 7 0 R >>", b"<< /LW 30 >>".to_vec()),
        (format!("/GS1 gs {line}"), "/ExtGState 7 0 R", b"<< /GS1 << /LW 30 >> >>".to_vec()),
        (format!("/GS1 gs {line}"), "/ExtGState << /GS1 << /LW 7 0 R >> >>", b"30".to_vec()),
        ("/Fm Do".to_string(), "/XObject << /Fm 7 0 R >>", form),
    ] {
        let mut doc = one_page(content.as_bytes(), res, vec![extra]);
        mark(&mut doc, 0, &[[90.0, 90.0, 150.0, 150.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().paths_removed, 1, "{content} with {res}");
    }
}

/// A mitered corner reaches past half the width, `1/sin(φ/2)` times it at a corner of angle φ, up
/// to the miter limit; past it the corner is bevelled. Here a 20-wide stroke turns at (120, 175),
/// 25 points above the mark, through a corner whose miter (4.6 times half the width) reaches 46
/// points below it, where half the width at a projecting cap's corner reaches 14. Joined round
/// (`1 j`, an ExtGState's `/LJ 1`) or bevelled (`2 j`, or a limit below 4.6 by `M` or `/ML`) it
/// stays; any other join is mitered, as the renderer reads it (by its integer part: `0.9 j` is
/// mitered). A curve meets the line at its tangent: arriving or leaving across the page, the
/// corner is blunt.
/// Closed by `h`, `s`, `b` or `b*`, the subpath joins at its start too; drawn back to its start and not
/// closed, it has ends there, not a corner; and a line after `h` or a rectangle starts a subpath
/// of its own, and has no corner with the segment before. Where the renderer takes two points for
/// one, the corner is as it draws it: a curve's control point a ten-millionth of a unit from its
/// end, leaving by `c` or arriving by `y`, gives it no tangent, nor does one written with a last
/// digit it doesn't read (`120.00000000000001`), and a line that short, closing or mid-path, is
/// none. Blunt corners on either side of a line stay, however sharp the turn across it, and so
/// does a pentagon, whose first side is where it leaves its start, not its second. A line of more
/// such steps than the model weighs at a join (`stroke::MAX_DIRECTIONS`, 16) is taken to reach as far as
/// the limit lets. A form takes the join of the content that draws
/// it. The renderer keeps the limit as a 32-bit float, 4.3 as 4.30000019: a corner whose miter is
/// 4.3000001 times half the width is mitered within a limit of 4.3, by `M` or `/ML`, and goes;
/// within 4.29 it is bevelled.
#[test]
fn a_mitered_corner_reaching_under_a_mark_is_removed() {
    // A corner at (120, 175) whose miter is 4.3000001 times half the width.
    const SHARP: &str = "98.4797360354395 265 m 120 175 l 141.5202639645605 265 l S";
    // A straight line 25 points above the mark, `n` steps of a ten-millionth of a unit first.
    let tiny_steps = |n: u32| {
        let steps: String = (1..=n).map(|k| format!("{:.7} 175 l ", 100.0 + f64::from(k) * 1e-7)).collect();
        format!("20 w 100 175 m {steps}140 175 l S")
    };
    let corner = "20 w 100 265 m 120 175 l 140 265 l S";
    let res =
        "/ExtGState << /GSR << /LJ 1 >> /GSM << /ML 4 >> /GSJ << /LJ 0 >> /GSL << /ML 8 0 R >> /GS43 << /ML 4.3 >> >> /XObject << /FmC 7 0 R >>";
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", corner.as_bytes());
    for (content, gone) in [
        (corner.to_string(), 1),
        (format!("1 j {corner}"), 0),
        (format!("2 j {corner}"), 0),
        (format!("7 j {corner}"), 1),
        (format!("0.9 j {corner}"), 1),
        (format!("4 M {corner}"), 0),
        (format!("5 M {corner}"), 1),
        (format!("/GSR gs {corner}"), 0),
        (format!("/GSM gs {corner}"), 0),
        (format!("/GSL gs {corner}"), 0),
        (format!("1 j /GSJ gs {corner}"), 1),
        (format!("q 1 j Q {corner}"), 1),
        ("20 w 120 175 m 140 265 l 100 265 l h S".into(), 1),
        ("20 w 120 175 m 140 265 l 100 265 l s".into(), 1),
        ("20 w 120 175 m 140 265 l 100 265 l b".into(), 1),
        ("20 w 120 175 m 140 265 l 100 265 l b*".into(), 1),
        ("20 w 120 175 m 140 265 l 100 265 l 120 175 l h S".into(), 1),
        ("20 w 120 175 m 140 265 l 100 265 l 120 175 l S".into(), 0),
        ("20 w 120 175 m 100 265 l h 140 265 l S".into(), 0),
        ("20 w 100 265 m 120 175 l 200 200 1 1 re 140 265 l S".into(), 0),
        ("20 w 100 265 m 105 242.5 115 197.5 120 175 c 140 265 l S".into(), 1),
        ("20 w 100 265 m 115 197.5 120 175 v 140 265 l S".into(), 1),
        ("20 w 100 265 m 110 220 120 175 y 140 265 l S".into(), 1),
        ("20 w 100 265 m 120 175 l 125 197.5 135 242.5 140 265 c S".into(), 1),
        ("20 w 100 265 m 100 220 110 175 120 175 c 140 265 l S".into(), 0),
        ("20 w 100 265 m 110 175 120 175 v 140 265 l S".into(), 0),
        ("20 w 100 265 m 110 175 120 175 y 140 265 l S".into(), 0),
        ("20 w 100 265 m 120 175 l 130 175 140 220 140 265 c S".into(), 0),
        ("20 w 100 265 m 120 175 l 120.0000001 175 140 265 140 265 c S".into(), 1),
        ("20 w 100 265 m 119.9999999 175 120 175 y 140 265 l S".into(), 1),
        ("20 w 100 265 m 120 175 l 120.00000000000001 175 140 265 140 265 c S".into(), 1),
        ("20 w 120 175 m 140 265 l 100 265 l 120.00000000000001 175 l s".into(), 1),
        ("20 w 100 265 m 119.99999999999999 175 l 120 175 l 140 265 l S".into(), 1),
        ("20 w 100 265 m 120 175 l 140 175 l 160 265 l S".into(), 0),
        ("20 w 120 175 m 148.5 195.7 l 137.6 229.3 l 102.4 229.3 l 91.5 195.7 l h S".into(), 0),
        (tiny_steps(10), 0),
        (tiny_steps(20), 1),
        (format!("20 w 4.3 M {SHARP}"), 1),
        (format!("20 w /GS43 gs {SHARP}"), 1),
        (format!("20 w 4.29 M {SHARP}"), 0),
        ("/FmC Do".into(), 1),
        ("1 j /FmC Do".into(), 0),
    ] {
        let mut doc = one_page(content.as_bytes(), res, vec![form.clone(), b"2".to_vec()]);
        mark(&mut doc, 0, &[[90.0, 90.0, 150.0, 150.0]], "");
        assert_eq!(apply(&mut doc, None).unwrap().paths_removed, gone, "{content}");
    }
}

/// A fill whose box has a height (or width) only in a last digit the renderer doesn't read
/// (`120.00000000000001` is 120 to it) paints a line a pixel wide, as one of no height does: under
/// a mark whose top (or side) is less than half a point from it. A fill a point high there stays.
#[test]
fn a_fill_flat_to_the_renderer_is_taken_for_flat() {
    let (below, beside) = ([90.0, 100.0, 150.0, 119.7], [120.3, 90.0, 150.0, 150.0]);
    for (content, at, gone) in [
        (&b"100 120 m 140 120.00000000000001 l f"[..], below, 1),
        (b"100 120 m 140 120 l f", below, 1),
        (b"100 120 40 1 re f", below, 0),
        (b"120 100 m 120.00000000000001 110 l f", beside, 1),
    ] {
        let mut doc = one_page(content, "", vec![]);
        mark(&mut doc, 0, &[at], "");
        assert_eq!(apply(&mut doc, None).unwrap().paths_removed, gone, "{}", String::from_utf8_lossy(content));
    }
}
