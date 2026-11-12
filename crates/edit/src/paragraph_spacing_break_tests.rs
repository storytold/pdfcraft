//! The paragraph spacing goes before a row whose line break the edit inserted, and the rows
//! below it keep their own source steps.

use crate::paragraph_spacing_tests::{close, page, steps};
use crate::text;

/// Source rows `One`, `TwoThree` and `Four` of 10 pt text at baselines 700, 688 and 676: a
/// 12 pt row step and no paragraph gap. The draft breaks `TwoThree` into `Two` and `Three` and
/// sets a 10 pt spacing. `Three` starts the only new paragraph: it goes 12 + 10 below `Two`,
/// at 688 - 22 = 666, and `Four` keeps its 12 pt step below it, at 654. Steps 12, 22, 12.
/// Before, draft rows were paired with source rows by position: `Three` took `Four`'s row
/// (676, no gap there), and `Four`, past the source's rows, got the spacing: 12, 12, 22.
#[test]
fn spacing_goes_before_an_inserted_break() {
    let mut doc = page("BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (TwoThree) Tj 0 -12 Td (Four) Tj ET");
    let style = text::BlockStyle { paragraph_spacing: Some(10.0), ..Default::default() };
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree\nFour"), &style).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 22.0, 12.0]), "{s:?}");
}

/// A range style takes the styled path (`text_regions`), whose rows are placed by source row.
fn styled(spacing: f64) -> text::BlockStyle {
    let range =
        text::RangeStyle { range: 0..3, family: None, resource: None, size: None, color: None, underline: None, char_spacing: None, scale: None };
    text::BlockStyle { paragraph_spacing: Some(spacing), ranges: vec![range], ..Default::default() }
}

/// The styled path: `TwoThree` is split into `Two` and `Three`. `Three` starts the new paragraph,
/// 12 + 10 below `Two`, and `Four` keeps its 12 pt step below it: 12, 22, 12. The source's own
/// breaks, with no extra space above them, get none.
#[test]
fn styled_rows_take_the_spacing_before_an_inserted_break() {
    let source = "BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (TwoThree) Tj 0 -12 Td (Four) Tj ET";
    let mut doc = page(source);
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree\nFour"), &styled(10.0)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 22.0, 12.0]), "{s:?}");
    let mut doc = page(source);
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwoThree\nFour"), &styled(10.0)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 12.0]), "{s:?}");
}

/// The styled path: a source row set apart by a 4 pt gap (step 16) has that gap replaced by a
/// 10 pt spacing (step 22), and the row below keeps its step.
#[test]
fn styled_rows_replace_the_gap_a_row_already_had() {
    let mut doc = page("BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (Two) Tj 0 -16 Td (Three) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &styled(10.0)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 22.0]), "{s:?}");
}

fn rows(names: &[&str]) -> String {
    let body: String =
        names.iter().enumerate().map(|(i, n)| format!("{}({n}) Tj ", if i == 0 { "".to_string() } else { "0 -12 Td ".to_string() })).collect();
    format!("BT /F1 10 Tf 72 700 Td {body}ET")
}

fn steps_after(source: &[&str], draft: &str) -> Vec<f64> {
    let mut doc = page(&rows(source));
    text::rewrite_block(&mut doc, 0, 0, Some(draft), &styled(10.0)).unwrap();
    steps(&doc)
}

/// The styled path: a whole row inserted exactly between two source rows takes the spacing,
/// not the row after it. Source `One`, `Two`, `Three`; the draft adds `New` after `Two`.
/// Aligned by character prefix and suffix the added line could not be told from the break
/// after `Two` and the spacing went before `Three`: 12, 12, 22.
#[test]
fn styled_spacing_goes_before_a_row_inserted_at_a_row_boundary() {
    let s = steps_after(&["One", "Two", "Three"], "One\nTwo\nNew\nThree");
    assert!(close(&s, &[12.0, 22.0, 12.0]), "{s:?}");
}

/// A row inserted after the last row, and one before the first.
#[test]
fn styled_spacing_for_a_row_inserted_at_the_end_and_at_the_start() {
    let s = steps_after(&["One", "Two", "Three"], "One\nTwo\nThree\nNew");
    assert!(close(&s, &[12.0, 12.0, 22.0]), "{s:?}");
    let s = steps_after(&["One", "Two", "Three"], "New\nOne\nTwo\nThree");
    assert!(close(&s, &[22.0, 12.0, 12.0]), "{s:?}");
}

/// Rows that repeat: a third `B` among `A`, `B`, `B`, `C` is one more paragraph, set apart once,
/// and the rows below it keep their steps.
#[test]
fn styled_spacing_for_a_row_inserted_among_identical_rows() {
    let s = steps_after(&["A", "B", "B", "C"], "A\nB\nB\nB\nC");
    assert!(close(&s, &[12.0, 12.0, 22.0, 12.0]), "{s:?}");
    // Only one paragraph is added, whichever of the identical rows is the new one.
    let total: f64 = s.iter().sum();
    assert!((total - 58.0).abs() < 0.5, "{s:?}");
}

/// The default is half the size, for a paragraph the page did not have: a 10 pt row broken in two
/// is a 12 pt row step and 5 pt apart, 17. Rows the page already had stay one 12 pt step apart
/// whatever the draft does around them (below).
#[test]
fn a_new_styled_paragraph_gets_half_the_size_when_no_spacing_is_set() {
    let range =
        text::RangeStyle { range: 0..3, family: None, resource: None, size: None, color: None, underline: None, char_spacing: None, scale: None };
    let style = text::BlockStyle { ranges: vec![range], ..Default::default() };
    let mut doc = page(&rows(&["One", "Three"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &style).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[17.0, 12.0]), "{s:?}");
    // Asking for none takes it away again.
    let none = text::BlockStyle { paragraph_spacing: Some(0.0), ..style };
    let mut doc = page(&rows(&["One", "Three"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &none).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 12.0]), "{s:?}");
}

/// Rows the page had keep their steps when no spacing is set: two paragraphs of 10 pt text stay
/// one 12 pt row step apart.
#[test]
fn styled_source_rows_keep_their_steps_when_no_spacing_is_set() {
    let range =
        text::RangeStyle { range: 0..3, family: None, resource: None, size: None, color: None, underline: None, char_spacing: None, scale: None };
    let style = text::BlockStyle { ranges: vec![range], ..Default::default() };
    let mut doc = page(&rows(&["One", "Two"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo"), &style).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0]), "{s:?}");
}

/// Choosing a family takes the single-style path, which lays every row out afresh.
fn single(spacing: Option<f64>) -> text::BlockStyle {
    text::BlockStyle { family: Some((crate::Family::Times, false, false)), paragraph_spacing: spacing, ..Default::default() }
}

/// The single-style path, like the styled one: unset, a paragraph the draft's break adds gets half
/// the size (a 12 pt step and 5 pt apart, 17), and asking for none takes it away again.
#[test]
fn a_new_single_style_paragraph_gets_half_the_size_when_no_spacing_is_set() {
    let mut doc = page(&rows(&["One", "Three"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &single(None)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[17.0, 12.0]), "{s:?}");
    let mut doc = page(&rows(&["One", "Three"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &single(Some(0.0))).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 12.0]), "{s:?}");
}

/// Rows the page had keep their steps when no spacing is set, whichever way the draft breaks
/// around them, and a gap a source row already had is kept, not added to.
#[test]
fn single_style_source_rows_keep_their_steps_when_no_spacing_is_set() {
    let mut doc = page(&rows(&["One", "Two"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo"), &single(None)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0]), "{s:?}");
    // `Three` sits 16 pt below `Two`: a 4 pt gap, kept.
    let mut doc = page("BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (Two) Tj 0 -16 Td (Three) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &single(None)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 16.0]), "{s:?}");
    // A row added at the end is the only one set apart.
    let mut doc = page(&rows(&["One", "Two"]));
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nNew"), &single(None)).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[12.0, 17.0]), "{s:?}");
}

/// A set spacing replaces the gap a single-style row already had (never adds to it).
#[test]
fn single_style_spacing_that_is_set_replaces_the_gap_a_row_had() {
    let mut doc = page("BT /F1 10 Tf 72 700 Td (One) Tj 0 -12 Td (Two) Tj 0 -16 Td (Three) Tj ET");
    text::rewrite_block(&mut doc, 0, 0, Some("One\nTwo\nThree"), &single(Some(10.0))).unwrap();
    let s = steps(&doc);
    assert!(close(&s, &[22.0, 22.0]), "{s:?}");
}
