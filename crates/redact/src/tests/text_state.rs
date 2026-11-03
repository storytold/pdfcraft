//! Glyphs are placed with the whole text state, wherever it was set.

use super::*;

const SECRET_AT_10_100: [f64; 4] = [5.0, 95.0, 45.0, 110.0];

#[test]
fn a_font_set_by_an_extgstate_places_its_glyphs() {
    let mut doc = one_page(b"BT /GS1 gs 10 100 Td (SECRET) Tj ET", "/ExtGState << /GS1 << /Type /ExtGState /Font [5 0 R 10] >> >>", vec![]);
    // "ET" spans x 30–40.
    assert_eq!(under(&mut doc, 0, &[[30.0, 95.0, 40.0, 110.0]]), 2);
    mark(&mut doc, 0, &[SECRET_AT_10_100], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("SECRET"));
}

#[test]
fn a_form_inherits_the_text_state_of_the_page_that_draws_it() {
    // The form shows text with the font, size and spacing the page set before `Do`.
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT 10 100 Td (SECRET) Tj ET");
    let mut doc = one_page(b"/F1 10 Tf 1 Tc /Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    // Glyphs 5 pt wide, 1 pt apart: the last one spans x 40–45.
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 46.0, 110.0]]), 1);
    mark(&mut doc, 0, &[SECRET_AT_10_100], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_rewritten), (6, 1), "{r:?}");
}

/// A Type 3 font whose matrix moves its glyphs up 5 pt (at 10 pt): the renderer draws the `A` at
/// y 105–115, but the box from the matrix's scale alone put it at y 99–110, so a mark over the
/// top of the drawn glyph neither removed it nor saw it left behind.
#[test]
fn a_type3_glyph_is_placed_with_its_whole_font_matrix() {
    let font = b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0.5] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Differences [65 /A] >> /CharProcs << /A 8 0 R >> >>";
    let mut doc = one_page(b"BT /T3 10 Tf 10 100 Td (A) Tj ET", "/Font << /T3 7 0 R >>", vec![font.to_vec(), stream("", b"0 0 1000 1000 re f")]);
    let top = [5.0, 111.0, 20.0, 116.0];
    assert_eq!(under(&mut doc, 0, &[top]), 1, "the verifier sees the drawn glyph");
    mark(&mut doc, 0, &[top], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
}
