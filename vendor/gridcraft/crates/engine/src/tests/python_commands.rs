//! The python.* commands.

use gridcraft_core::{CellError, Value};
use serde_json::json;

use super::{s, v};

fn set(s: &mut crate::Session, cell: &str, code: &str) -> serde_json::Value {
    s.execute("python.set", json!({"cell": cell, "code": code})).unwrap()
}

#[test]
fn set_stores_the_code_and_blocks_the_cell_until_it_can_run() {
    let mut s = s();
    let r = set(&mut s, "B1", "xl('A1') + 1");
    assert_eq!(r["cell"], "B1");
    assert_eq!(v(&s, "B1"), Value::Error(CellError::Blocked));
    let got = s.execute("cell.get", json!({"cell": "B1"})).unwrap();
    assert_eq!(got["python"]["code"], "xl('A1') + 1");
    assert_eq!(got["python"]["output"], "value");
    assert_eq!(got["input"], "xl('A1') + 1");
    // The active cell by default.
    s.execute("selection.set", json!({"cell": "D4"})).unwrap();
    s.execute("python.set", json!({"code": "2"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "D4"})).unwrap()["python"]["code"], "2");
    // One undo step takes the cell back.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.execute("cell.get", json!({"cell": "D4"})).unwrap()["python"].is_null());
}

#[test]
fn unchanged_code_keeps_its_cached_result() {
    let mut s = s();
    set(&mut s, "A1", "21 * 2");
    // As a file would have cached it.
    {
        let d = s.doc_mut().unwrap();
        let wb = std::sync::Arc::make_mut(&mut d.wb);
        let sh = wb.sheet_mut(0).unwrap();
        let cell = sh.cells.get_mut(gridcraft_core::CellRef::new(0, 0)).unwrap();
        let py = std::sync::Arc::make_mut(cell.py.as_mut().unwrap());
        py.output = Some(gridcraft_model::PyOutput { value: Value::Number(42.0), type_name: "int".into(), preview: "42".into(), convertible: true });
        cell.value = Value::Number(42.0);
    }
    set(&mut s, "A1", "21 * 2");
    assert_eq!(v(&s, "A1"), Value::Number(42.0), "same code, same result");
    s.execute("python.set", json!({"cell": "A1", "code": "21 * 2", "output": "object"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["python"]["output"], "object");
    set(&mut s, "A1", "21 * 3");
    assert_eq!(v(&s, "A1"), Value::Error(CellError::Blocked), "new code has no result yet");
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["python"]["output"], "object", "the return type stays");
}

#[test]
fn set_refuses_bad_params_long_code_headers_and_protected_cells() {
    let mut s = s();
    for p in [
        json!({"cell": "A1"}),
        json!({"cell": "A1", "code": 5}),
        json!({"cell": "not a cell", "code": "1"}),
        json!({"cell": "A1", "code": "1", "output": "sideways"}),
    ] {
        assert!(s.execute("python.set", p.clone()).is_err(), "{p}");
    }
    // The limit is the stored PY(…) formula's 8,192 characters.
    assert!(s.execute("python.set", json!({"cell": "A1", "code": "y".repeat(8_080)})).is_err());
    s.execute("python.set", json!({"cell": "A1", "code": "y".repeat(8_079)})).unwrap();
    let quoted = format!("{}{}", "\"".repeat(1_200), "y".repeat(5_800));
    let e = s.execute("python.set", json!({"cell": "A2", "code": quoted})).unwrap_err();
    assert!(e.to_string().contains("8192") || e.to_string().contains("8,192"), "{e}");
    // A table header is the column's name.
    s.execute("range.setValues", json!({"range": "C1", "values": [["Name", "Score"], ["a", 1]]})).unwrap();
    s.execute("insert.table", json!({"range": "C1:D2", "headers": true})).unwrap();
    assert!(s.execute("python.set", json!({"cell": "D1", "code": "1"})).is_err());
    assert_eq!(v(&s, "D1"), Value::text("Score"));
    set(&mut s, "D2", "1");
    // A locked cell on a protected sheet refuses it.
    s.execute("review.protectSheet", json!({})).unwrap();
    let e = s.execute("python.set", json!({"cell": "F1", "code": "1"})).unwrap_err();
    assert!(e.to_string().contains("protected"), "{e}");
}

#[test]
fn set_over_a_spilling_formula_takes_the_spill_away() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
    assert_eq!(v(&s, "A3"), Value::Number(3.0));
    set(&mut s, "A1", "[1, 2, 3]");
    assert_eq!(v(&s, "A3"), Value::Empty);
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().spill_ranges.is_empty());
}

/// A cached result as a file would have kept it on the cell: `[[1], [2], [3]]`, a list.
fn cache_list(s: &mut crate::Session, cell: &str) {
    let at = gridcraft_core::CellRef::parse(cell).unwrap();
    let d = s.doc_mut().unwrap();
    let wb = std::sync::Arc::make_mut(&mut d.wb);
    let sh = wb.sheet_mut(0).unwrap();
    let py = std::sync::Arc::make_mut(sh.cells.get_mut(at).unwrap().py.as_mut().unwrap());
    let arr = gridcraft_core::Array::new(3, 1, vec![1.0.into(), 2.0.into(), 3.0.into()]).map(Value::from).unwrap();
    py.output = Some(gridcraft_model::PyOutput { value: arr, type_name: "list".into(), preview: "[1, 2, 3]".into(), convertible: true });
}

/// A1 holds `[1, 2, 3]` with its cached list spilled over A1:A3 as Excel values; B1 reads A3.
fn cached_spill_with_a_reader() -> crate::Session {
    let mut s = s();
    set(&mut s, "A1", "[1, 2, 3]");
    cache_list(&mut s, "A1");
    s.execute("formulas.calculateNow", json!({})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=SUM(A3)"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    assert_eq!(v(&s, "B1"), Value::Number(3.0));
    s
}

fn output_of(s: &mut crate::Session, cell: &str) -> serde_json::Value {
    s.execute("cell.get", json!({"cell": cell})).unwrap()["python"]["output"].clone()
}

fn undo_depth(s: &mut crate::Session) -> Option<usize> {
    s.execute("history.list", json!({})).unwrap()["undo"].as_array().map(Vec::len)
}

#[test]
fn output_switches_the_return_type_and_drops_a_result_of_the_other_type() {
    let mut s = cached_spill_with_a_reader();
    // The mode it already has changes nothing and adds no undo step.
    let undo = undo_depth(&mut s);
    let r = s.execute("python.output", json!({"cell": "A1", "mode": "value"})).unwrap();
    assert_eq!(r["output"], "value");
    assert_eq!(undo_depth(&mut s), undo);
    assert_eq!(v(&s, "A3"), Value::Number(3.0), "the kept result still spills");
    // Without `mode` it switches. The cached list is an Excel value's result: as with python.set
    // and a new return type, the cell has no result until it runs, and its old spill goes with
    // its readers recalculated.
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    let r = s.execute("python.output", json!({})).unwrap();
    assert_eq!(r["output"], "object");
    assert_eq!(v(&s, "A1"), Value::Error(CellError::Blocked));
    assert_eq!(v(&s, "A3"), Value::Empty, "the old spill goes");
    assert_eq!(v(&s, "B1"), Value::Number(0.0), "and what read it is recalculated");
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["python"]["code"], "[1, 2, 3]", "the code stays");
    // The same as python.set with the same code and the other return type.
    let mut by_set = cached_spill_with_a_reader();
    by_set.execute("python.set", json!({"cell": "A1", "code": "[1, 2, 3]", "output": "object"})).unwrap();
    for c in ["A1", "A3", "B1"] {
        assert_eq!(v(&s, c), v(&by_set, c), "{c}");
    }
    assert_eq!(output_of(&mut s, "A1"), output_of(&mut by_set, "A1"));
    // Switching back brings no result back: the dropped one belonged to the old type.
    s.execute("python.output", json!({"cell": "A1", "mode": "value"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Error(CellError::Blocked));
    assert_eq!(v(&s, "A3"), Value::Empty);
    // Each switch is one undo step, and undoing the first brings the result and its spill back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(output_of(&mut s, "A1"), "object");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(output_of(&mut s, "A1"), "value");
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    assert_eq!(v(&s, "A3"), Value::Number(3.0));
    assert_eq!(v(&s, "B1"), Value::Number(3.0));
    // A cell with no result keeps showing #BLOCKED!.
    set(&mut s, "C1", "x = 1");
    s.execute("python.output", json!({"cell": "C1"})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Error(CellError::Blocked));
    assert_eq!(output_of(&mut s, "C1"), "object");
}

#[test]
fn output_refuses_table_headers_and_locked_cells_as_set_does() {
    let mut s = s();
    // A Python cell in a table's header row (as a file could hold one): python.set refuses to
    // put code there, and python.output refuses to change it.
    s.execute("range.setValues", json!({"range": "C1", "values": [["Name", "Score"], ["a", 1]]})).unwrap();
    s.execute("insert.table", json!({"range": "C1:D2", "headers": true})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        let wb = std::sync::Arc::make_mut(&mut d.wb);
        let cell = wb.sheet_mut(0).unwrap().cells.get_mut(gridcraft_core::CellRef::parse("D1").unwrap()).unwrap();
        cell.py = Some(std::sync::Arc::new(gridcraft_model::PyCode::new("'Score'")));
    }
    let e = s.execute("python.output", json!({"cell": "D1"})).unwrap_err();
    assert!(e.to_string().contains("table header"), "{e}");
    assert_eq!(output_of(&mut s, "D1"), "value");
    // A locked cell on a protected sheet refuses it; an unlocked one formatted Hidden takes it,
    // as it takes python.set.
    set(&mut s, "F1", "1");
    set(&mut s, "G1", "2");
    s.execute("home.formatCells", json!({"range": "G1", "style": {"protection": {"locked": false, "hidden": true}}})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    let e = s.execute("python.output", json!({"cell": "F1"})).unwrap_err();
    assert!(e.to_string().contains("protected"), "{e}");
    assert_eq!(output_of(&mut s, "F1"), "value");
    s.execute("python.set", json!({"cell": "G1", "code": "2"})).unwrap();
    s.execute("python.output", json!({"cell": "G1"})).unwrap();
    assert_eq!(output_of(&mut s, "G1"), "object");
}

#[test]
fn output_refuses_cells_without_python_and_bad_modes() {
    let mut s = s();
    // Empty params: the active cell, which holds no Python code.
    let e = s.execute("python.output", json!({})).unwrap_err();
    assert!(e.to_string().contains("isn't a Python cell"), "{e}");
    s.execute("cell.set", json!({"cell": "B1", "input": "=1+1"})).unwrap();
    assert!(s.execute("python.output", json!({"cell": "B1"})).is_err());
    set(&mut s, "A1", "1");
    for p in [json!({"cell": "A1", "mode": "sideways"}), json!({"cell": "A1", "mode": 1}), json!({"cell": "nowhere"})] {
        assert!(s.execute("python.output", p.clone()).is_err(), "{p}");
    }
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["python"]["output"], "value");
    // python.set refuses a return type that isn't text too, rather than ignoring it.
    assert!(s.execute("python.set", json!({"cell": "A1", "code": "1", "output": true})).is_err());
}

#[test]
fn a_recorded_switch_keeps_the_return_type_it_set() {
    // python.output without `mode` switches, so a recording keeps the return type the switch set
    // (as Bold records `on`): played back on a cell that already has it, nothing switches back.
    let mut s = s();
    set(&mut s, "A1", "1");
    s.execute("macro.record", json!({"relative": false})).unwrap();
    s.execute("python.output", json!({"cell": "A1"})).unwrap();
    s.execute("macro.record", json!({"stop": true, "name": "Rec"})).unwrap();
    assert_eq!(output_of(&mut s, "A1"), "object");
    let m = s.execute("macro.get", json!({"name": "Rec"})).unwrap();
    let m = crate::macros::parse_macro(&m).unwrap();
    let steps = crate::macros::command_steps(&m.steps);
    let step = steps.iter().find(|(_, id, _)| *id == "python.output").map(|(_, _, p)| (*p).clone());
    assert_eq!(step.and_then(|p| p.get("mode").cloned()), Some(json!("object")));
    for _ in 0..2 {
        s.execute("macro.run", json!({"name": "Rec", "onConflict": "overwrite"})).unwrap();
        assert_eq!(output_of(&mut s, "A1"), "object");
    }
    // A run inside a script is recorded the same way.
    s.execute("automate.newScript", json!({"name": "Switch", "commands": [{"command": "python.output", "params": {"cell": "A1"}}]})).unwrap();
    s.execute("macro.record", json!({"relative": false})).unwrap();
    s.execute("automate.runScript", json!({"name": "Switch"})).unwrap();
    s.execute("macro.record", json!({"stop": true, "name": "Rec2"})).unwrap();
    assert_eq!(output_of(&mut s, "A1"), "value");
    for _ in 0..2 {
        s.execute("macro.run", json!({"name": "Rec2", "onConflict": "overwrite"})).unwrap();
        assert_eq!(output_of(&mut s, "A1"), "value");
    }
}
