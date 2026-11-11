//! A form's `/Matrix` places what it draws. The redaction walk reads it as the proof does, the
//! array and each entry direct or by reference: read as the identity, a form moved onto a mark
//! was tested against the wrong place, never walked, and its text stayed.

use super::*;

/// One page drawing a form with resources of its own, whose content shows SECRET at (20, 60) in a
/// `/BBox` of [0 0 300 100], with `matrix` in its dictionary; object 8 is `extra`.
fn moved_form(matrix: &str, extra: &[u8]) -> Document {
    one_page(
        b"/Fm Do",
        "/XObject << /Fm 7 0 R >>",
        vec![
            stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {matrix} /Resources << /Font << /F1 5 0 R >> >>"),
                b"BT /F1 10 Tf 20 60 Td (SECRET) Tj ET",
            ),
            extra.to_vec(),
        ],
    )
}

/// Over the top third of the 300 × 300 page, where a move of 200 puts the text.
const MARK: [f64; 4] = [0.0, 200.0, 300.0, 300.0];

#[test]
fn a_form_moved_onto_a_mark_is_redacted_however_its_matrix_is_given() {
    for (matrix, extra) in [
        ("/Matrix [1 0 0 1 0 200]", &b"null"[..]),
        ("/Matrix 8 0 R", b"[1 0 0 1 0 200]"),
        ("/Matrix [1 0 0 1 0 8 0 R]", b"200"),
        ("/Matrix [8 0 R 0 0 1 0 200]", b"1"),
    ] {
        let mut doc = moved_form(matrix, extra);
        mark(&mut doc, 0, &[MARK], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6), "{matrix} with 8 0 obj {}", String::from_utf8_lossy(extra));
    }
}

/// A `/Matrix` of null, given directly or by a reference to null, is no `/Matrix` (ISO 32000-2
/// §7.3.9): the identity, under which the text lies below the mark and stays.
#[test]
fn a_form_whose_matrix_is_null_draws_where_it_stands() {
    for (matrix, extra) in [("/Matrix null", &b"null"[..]), ("/Matrix 8 0 R", b"null")] {
        let mut doc = moved_form(matrix, extra);
        mark(&mut doc, 0, &[MARK], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(0), "{matrix}");
        let mut doc = moved_form(matrix, extra);
        mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 100.0]], "");
        assert_eq!(apply(&mut doc, None).map(|r| r.glyphs), Ok(6), "{matrix}, marked where it stands");
    }
}

/// A `/Matrix` that isn't six numbers: where the form draws can't be told, so neither can whether
/// it reaches the mark, and the page is refused.
#[test]
fn a_form_whose_matrix_is_not_six_numbers_is_refused() {
    for (matrix, extra) in [
        ("/Matrix [1 0 0 1 0 (up)]", &b"null"[..]),
        ("/Matrix [1 0 0 1 0]", b"null"),
        ("/Matrix [1 0 0 1 0 8 0 R]", b"(up)"),
        ("/Matrix 8 0 R", b"(up)"),
    ] {
        let mut doc = moved_form(matrix, extra);
        mark(&mut doc, 0, &[MARK], "");
        let result = apply(&mut doc, None);
        assert!(
            matches!(&result, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail, .. })) if detail.contains("/Matrix")),
            "{matrix} with 8 0 obj {}: {result:?}",
            String::from_utf8_lossy(extra)
        );
    }
}

/// A reference to a reference, for the matrix or one of its entries: Poppler doesn't follow it (it
/// reads such a matrix as the identity, such an entry as 0), the app's renderer follows it and
/// reads the move, so where the form draws can't be told, and the page is refused, as for a matrix that isn't six numbers. Read as either,
/// a mark where the other reader shows the text would leave it in the file.
#[test]
fn a_form_whose_matrix_goes_through_a_reference_to_a_reference_is_refused() {
    for (matrix, target) in [("/Matrix [1 0 0 1 0 8 0 R]", &b"200"[..]), ("/Matrix 8 0 R", b"[1 0 0 1 0 200]")] {
        for mark_at in [MARK, [0.0, 0.0, 300.0, 100.0]] {
            let form = stream(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 100] {matrix} /Resources << /Font << /F1 5 0 R >> >>"),
                b"BT /F1 10 Tf 20 60 Td (SECRET) Tj ET",
            );
            let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form, b"9 0 R".to_vec(), target.to_vec()]);
            mark(&mut doc, 0, &[mark_at], "");
            let result = apply(&mut doc, None);
            assert!(
                matches!(&result, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail, .. })) if detail.contains("/Matrix")),
                "{matrix}, 8 0 obj 9 0 R, 9 0 obj {}, mark at {mark_at:?}: {result:?}",
                String::from_utf8_lossy(target)
            );
        }
    }
}

/// The scans count such a form as content that may remain: the verify pass under a mark, and
/// Remove Hidden Information's scan, whose removal then refuses the page.
#[test]
fn a_form_whose_matrix_is_not_six_numbers_counts_in_the_scans() {
    use crate::sanitize::{Hidden, remove_hidden, scan};
    let mut doc = moved_form("/Matrix [1 0 0 1 0 (up)]", b"null");
    assert_eq!(under(&mut doc, 0, &[MARK]), 1);
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::HiddenText], 1);
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenText]);
    assert!(matches!(&removed, Err(RedactError::Cos(pdfcraft_cos::CosError::Syntax { detail, .. })) if detail.contains("/Matrix")), "{removed:?}");
}

/// Removing hidden layers alone never asks where content lands, so such a form is walked as it
/// stands: nothing is counted or refused over it in a document without layers.
#[test]
fn a_form_whose_matrix_is_not_six_numbers_leaves_removing_hidden_layers_alone() {
    use crate::sanitize::{Hidden, remove_hidden, scan};
    let mut doc = moved_form("/Matrix [1 0 0 1 0 (up)]", b"null");
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::HiddenLayers], 0);
    assert_eq!(remove_hidden(&mut doc, &[Hidden::HiddenLayers]), Err(RedactError::NothingToApply));
}

/// Remove Hidden Information reads it the same way: text a matrix entry by reference moves off the
/// page is hidden text, and goes.
#[test]
fn text_a_matrix_entry_by_reference_moves_off_the_page_is_hidden_text() {
    use crate::sanitize::{Hidden, remove_hidden};
    let mut doc = moved_form("/Matrix [1 0 0 1 0 8 0 R]", b"1000");
    let removed = remove_hidden(&mut doc, &[Hidden::HiddenText]);
    assert!(removed.as_ref().is_ok_and(|r| r.iter().any(|(h, n)| *h == Hidden::HiddenText && *n > 0)), "{removed:?}");
}
