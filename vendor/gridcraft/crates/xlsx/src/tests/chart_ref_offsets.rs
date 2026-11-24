//! Chart caches keep the position of every point inside the referenced range.

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Sheet, Workbook};
use std::sync::Arc;

use crate::chart::ref_values;

fn book() -> Workbook {
    let mut s = Sheet::new("Data");
    s.set_value(CellRef::new(3, 0), Value::Number(5.0));
    Workbook { sheets: vec![Arc::new(s)], ..Workbook::default() }
}

#[test]
fn explicit_range_keeps_leading_and_trailing_blanks() {
    let vals = ref_values(&book(), 0, "Data!$A$3:$A$6").unwrap();
    assert_eq!(vals, vec![Value::Empty, Value::Number(5.0), Value::Empty, Value::Empty]);
}

#[test]
fn whole_column_still_stops_at_used_range_and_starts_at_top() {
    let vals = ref_values(&book(), 0, "Data!$A:$A").unwrap();
    assert_eq!(vals, vec![Value::Empty, Value::Empty, Value::Empty, Value::Number(5.0)]);
}
