//! Typing a bare number into a Percentage-formatted cell enters it as a percent (issue #1967).

use gridcraft_engine::display::cell_text;
use serde_json::json;

fn enter(input: &str) -> String {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("home.numberFormat", json!({"range": "A1", "format": "Percentage"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": input})).unwrap();
    let d = s.doc().unwrap();
    cell_text(&d.wb, d.wb.active().unwrap(), gridcraft_engine::core::CellRef::new(0, 0))
}

#[test]
fn bare_number_in_percent_cell_is_a_percent() {
    assert_eq!(enter("3"), "3.00%");
    assert_eq!(enter("0.03"), "3.00%");
    assert_eq!(enter("3%"), "3.00%");
}
