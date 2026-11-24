//! Clearing a large sparse selection must invalidate every changed cell (issue #1032).

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_engine::Session;
use serde_json::json;

fn value(s: &Session, address: &str) -> Value {
    s.doc().unwrap().wb.active().unwrap().value(CellRef::parse(address).unwrap())
}

#[test]
fn large_clear_updates_distant_direct_and_transitive_dependents() {
    for command in ["edit.clearContents", "edit.clearAll"] {
        let mut s = Session::new();
        s.new_workbook();
        for (cell, input) in [("A1000001", "7"), ("B1", "=A1000001"), ("C1", "=B1*2")] {
            s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
        }
        assert_eq!((value(&s, "B1"), value(&s, "C1")), (Value::Number(7.0), Value::Number(14.0)));

        // B1 stretches the used range to row 1; the changed source lies just past the old cap.
        s.execute(command, json!({"range": "A1:A1000001"})).unwrap();
        assert_eq!(value(&s, "A1000001"), Value::Empty, "{command}");
        assert_eq!((value(&s, "B1"), value(&s, "C1")), (Value::Number(0.0), Value::Number(0.0)), "{command}");

        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(value(&s, "A1000001"), Value::Number(7.0), "{command} undo");
        assert_eq!((value(&s, "B1"), value(&s, "C1")), (Value::Number(7.0), Value::Number(14.0)), "{command} undo");
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(value(&s, "A1000001"), Value::Empty, "{command} redo");
        assert_eq!((value(&s, "B1"), value(&s, "C1")), (Value::Number(0.0), Value::Number(0.0)), "{command} redo");
    }
}

#[test]
fn large_clear_removes_a_distant_formulas_spill_and_updates_its_dependent() {
    for command in ["edit.clearContents", "edit.clearAll"] {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("cell.set", json!({"cell": "A1000001", "input": "=SEQUENCE(2)"})).unwrap();
        s.execute("cell.set", json!({"cell": "B1", "input": "=A1000002"})).unwrap();
        assert_eq!((value(&s, "A1000002"), value(&s, "B1")), (Value::Number(2.0), Value::Number(2.0)));

        // The spill lies outside the selection, but must disappear when its anchor is cleared.
        s.execute(command, json!({"range": "A1:A1000001"})).unwrap();
        assert_eq!(value(&s, "A1000001"), Value::Empty, "{command}");
        assert_eq!(value(&s, "A1000002"), Value::Empty, "{command}");
        assert_eq!(value(&s, "B1"), Value::Number(0.0), "{command}");
    }
}

#[test]
fn clearing_an_empty_merge_unblocks_a_spill() {
    for command in ["edit.clearFormats", "edit.clearAll"] {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
        // Keep the empty merged area inside the used range after the spill becomes blocked.
        s.execute("cell.set", json!({"cell": "C4", "input": "1"})).unwrap();
        s.execute("home.mergeCells", json!({"range": "A2:B3"})).unwrap();
        assert_eq!(value(&s, "A1"), Value::Error(CellError::Spill));

        s.execute(command, json!({"range": "A2:B3"})).unwrap();
        assert_eq!((value(&s, "A1"), value(&s, "A3")), (Value::Number(1.0), Value::Number(3.0)), "{command}");
        assert!(s.doc().unwrap().wb.active().unwrap().merges.is_empty(), "{command}");
    }
}

#[test]
fn clearing_a_distant_styled_blocker_unblocks_a_spill() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(100)"})).unwrap();
    s.execute("cell.set", json!({"cell": "A90", "input": "x"})).unwrap();
    s.execute("home.bold", json!({"range": "A90", "on": true})).unwrap();
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Spill));

    // The cleared blocker stays stored for its style and lies beyond the local 64-row scan.
    s.execute("edit.clearContents", json!({"range": "A2:A100"})).unwrap();
    assert_eq!(value(&s, "A1"), Value::Number(1.0));
    assert_eq!(value(&s, "A90"), Value::Number(90.0));
    assert_eq!(value(&s, "A100"), Value::Number(100.0));
    let wb = &s.doc().unwrap().wb;
    let cell = wb.active().unwrap().cell(CellRef::parse("A90").unwrap()).unwrap();
    assert_eq!(cell.value, Value::Empty);
    assert!(wb.styles.get(cell.style).font.bold);
}
