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
