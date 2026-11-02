//! Inline image data ends where the renderer ends it, so the operators after it are the ones the
//! page draws.

use super::*;

/// The image data `7FEI 1 0 0 1 200 0 cm` holds an `EI` straight after `7F`. The renderer ends
/// the image there (text follows, so what comes after reads as content) and runs the `cm`,
/// drawing SECRET at x = 210, not 10.
const FAKE_EI: &[u8] = b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7FEI 1 0 0 1 200 0 cm EI BT /F1 10 Tf 10 100 Td (SECRET) Tj ET";

#[test]
fn text_after_an_inline_image_is_redacted_where_the_renderer_draws_it() {
    let mut doc = one_page(FAKE_EI, "", vec![]);
    assert_eq!(under(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]]), 0, "nothing is drawn at x = 10");
    mark(&mut doc, 0, &[[205.0, 95.0, 245.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("SECRET"));
}

#[test]
fn an_inline_image_with_one_possible_end_is_redacted() {
    let mut doc = one_page(b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI BT /F1 10 Tf 10 100 Td (SECRET) Tj ET", "", vec![]);
    mark(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 6);
    assert!(!content(&doc, 0).contains("SECRET"));
}

/// Without an `EI` the renderer accepts, where the operators resume is a guess: refused.
#[test]
fn a_page_whose_inline_image_the_renderer_cannot_end_is_refused() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F", "", vec![]);
    mark(&mut doc, 0, &[[5.0, 95.0, 45.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}
