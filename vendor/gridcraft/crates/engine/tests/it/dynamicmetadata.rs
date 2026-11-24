//! A formula opened with dynamic-array metadata keeps it through edits that only move or adjust
//! it, and loses it when it is retyped (the writer then decides from the formula's shape).

use std::sync::Arc;

use gridcraft_engine::core::{CellRef, Value};
use gridcraft_engine::model::{Cell, Formula, Workbook};
use gridcraft_engine::{DocState, Session, io};
use serde_json::json;

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

/// A workbook saved by GridCraft whose D1 formula came from a file with `cm`.
fn opened() -> Session {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    for r in 0..3 {
        sh.set_value(CellRef::new(r, 1), Value::Number(f64::from(r + 1)));
        sh.set_value(CellRef::new(r, 2), Value::Number(f64::from(r + 2)));
    }
    let mut f = Formula::new("SUMPRODUCT(B1:B3*C1:C3)");
    f.dynamic = true;
    sh.cells.set(at("D1"), Cell::formula(f));
    let bytes = io::save_bytes(&wb, "dynamic.xlsx").unwrap();
    let (back, _) = io::open_bytes("dynamic.xlsx", &bytes).unwrap();
    let mut s = Session::new();
    s.add_document(DocState::new(back, None, "dynamic".into(), gridcraft_engine::Trust::FromFile));
    s
}

fn formula(s: &Session, cell: &str) -> Arc<Formula> {
    s.doc().unwrap().wb.sheet(0).unwrap().cell(at(cell)).unwrap().formula.clone().unwrap()
}

/// Whether `cell` of the saved file carries dynamic-array metadata.
fn saved_dynamic(s: &Session, cell: &str) -> bool {
    let bytes = io::save_bytes(&s.doc().unwrap().wb, "dynamic.xlsx").unwrap();
    let (back, _) = io::open_bytes("dynamic.xlsx", &bytes).unwrap();
    back.sheet(0).unwrap().cell(at(cell)).unwrap().formula.as_ref().unwrap().dynamic
}

#[test]
fn opened_dynamic_formula_keeps_its_metadata_on_save() {
    let s = opened();
    assert!(formula(&s, "D1").dynamic);
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().value(at("D1")), Value::Number(20.0));
    assert!(saved_dynamic(&s, "D1"));
}

#[test]
fn inserting_rows_and_moving_keep_the_mark() {
    let mut s = opened();
    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    let f = formula(&s, "D2");
    assert!(f.dynamic);
    assert_eq!(f.text, "SUMPRODUCT(B2:B4*C2:C4)");
    assert!(saved_dynamic(&s, "D2"));
    s.execute("selection.set", json!({"range": "D2"})).unwrap();
    s.execute("edit.cut", json!({})).unwrap();
    s.execute("selection.set", json!({"range": "F2"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert!(formula(&s, "F2").dynamic);
    assert!(saved_dynamic(&s, "F2"));
}

#[test]
fn retyped_formula_drops_the_mark() {
    let mut s = opened();
    s.execute("cell.set", json!({"cell": "D1", "input": "=SUMPRODUCT(B1:B3*C1:C3)"})).unwrap();
    assert!(!formula(&s, "D1").dynamic);
    // SUMPRODUCT evaluates its arguments as arrays anyway: saved as a legacy formula.
    assert!(!saved_dynamic(&s, "D1"));
}

#[test]
fn dynamic_mark_is_serialized_only_when_set() {
    let s = opened();
    let dynamic = serde_json::to_value(&*formula(&s, "D1")).unwrap();
    assert_eq!(dynamic["dynamic"], json!(true));
    let plain = serde_json::to_value(Formula::new("SUM(A1:A3)")).unwrap();
    assert!(plain.get("dynamic").is_none(), "{plain}");
    // Documents written before the field existed still load.
    let old: Formula = serde_json::from_value(json!({"text": "SUM(A1:A3)", "array": null})).unwrap();
    assert!(!old.dynamic);
}
