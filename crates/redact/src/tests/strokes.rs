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
