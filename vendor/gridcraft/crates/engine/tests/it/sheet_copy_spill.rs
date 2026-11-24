use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use gridcraft_engine::calc::Calc;
use serde_json::{Value as Json, json};

fn run(s: &mut Session, command: &str, params: Json) {
    s.execute(command, params).unwrap();
}

fn value(s: &Session, sheet: &str, cell: &str) -> Value {
    let wb = &s.doc().unwrap().wb;
    wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().value(CellRef::parse(cell).unwrap())
}

/// Issue #2053: the copy of a sheet holding a spill keeps the spill's values, in memory and after a full recalc.
#[test]
fn copied_sheet_keeps_spill_values_through_full_recalc() {
    let mut s = Session::new();
    run(&mut s, "file.new", json!({"sample": "grades"}));
    run(&mut s, "home.mergeCells", json!({"range": "F:H"}));
    run(&mut s, "cell.set", json!({"cell": "J54", "input": "=SEQUENCE(5)*C6"}));
    run(&mut s, "selection.set", json!({"range": "15:15"}));
    run(&mut s, "sheet.move", json!({"copy": true, "sheet": 0, "to": 1}));
    run(&mut s, "cell.set", json!({"array": true, "cell": "D3", "input": "=SUM('Grades (2)'!J54#)"}));

    let before = value(&s, "Grades (2)", "J55");
    assert!(matches!(before, Value::Number(_)), "J55 should hold a spilled number, got {before:?}");

    let mut fresh = (*s.doc().unwrap().wb).clone();
    Calc::new().recalc_all(&mut fresh);
    let after = fresh.sheet(fresh.sheet_index("Grades (2)").unwrap()).unwrap().value(CellRef::parse("J55").unwrap());
    assert_eq!(before, after);
}
