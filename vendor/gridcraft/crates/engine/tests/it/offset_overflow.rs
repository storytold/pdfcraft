use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_engine::Session;
use serde_json::json;

#[test]
fn cell_set_offset_extreme_arguments_return_ref() {
    let mut session = Session::new();
    session.new_workbook();
    for formula in [
        "=OFFSET(B2,1E300,0)",
        "=OFFSET(B2,0,1E300)",
        "=OFFSET(B2,0,0,1E300,1)",
        "=OFFSET(B2,0,0,1,1E300)",
        "=OFFSET(B2,-1E300,0)",
        "=OFFSET(B2,0,-1E300)",
        "=OFFSET(B2,0,0,-1E300,1)",
        "=OFFSET(B2,0,0,1,-1E300)",
        "=OFFSET(B2,1E300,0,-1E300,1)",
        "=OFFSET(B2,0,1E300,1,-1E300)",
    ] {
        session.execute("cell.set", json!({"cell": "D1", "input": formula})).unwrap();
        assert_eq!(session.doc().unwrap().wb.sheet(0).unwrap().value(CellRef::new(0, 3)), Value::Error(CellError::Ref), "{formula}");
    }
}
