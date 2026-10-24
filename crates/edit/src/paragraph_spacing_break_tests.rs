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
