use gridcraft_core::{CellRef, Value};
use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn workbook() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn run(s: &mut Session, command: &str, params: Json) {
    s.execute(command, params).unwrap();
}

fn set(s: &mut Session, cell: &str, input: &str) {
    run(s, "cell.set", json!({"cell": cell, "input": input}));
}

fn activate(s: &mut Session, sheet: &str) {
    run(s, "sheet.activate", json!({"sheet": sheet}));
}

fn value(s: &Session, sheet: &str, cell: &str) -> Value {
    let wb = &s.doc().unwrap().wb;
    wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().value(CellRef::parse(cell).unwrap())
}

fn formula(s: &Session, sheet: &str, cell: &str) -> String {
    let wb = &s.doc().unwrap().wb;
    wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().cell(CellRef::parse(cell).unwrap()).unwrap().formula.as_ref().unwrap().text.clone()
}

fn cut_to(s: &mut Session, source: &str, range: &str, destination: &str, at: &str) {
    activate(s, source);
    run(s, "edit.cut", json!({"range": range}));
    activate(s, destination);
    run(s, "edit.paste", json!({"at": at}));
}

#[test]
fn cross_sheet_cut_preserves_total_through_undo_redo_and_xlsx() {
    let mut s = workbook();
    run(&mut s, "range.setValues", json!({"range": "A1", "values": [[10], [20]]}));
    set(&mut s, "B1", "=SUM(A1:A2)");
    run(&mut s, "home.insertSheet", json!({"name": "Sheet2"}));
    cut_to(&mut s, "Sheet1", "A1:A2", "Sheet2", "C3");

    let assert_moved = |s: &Session| {
        assert_eq!(value(s, "Sheet1", "A1"), Value::Empty);
        assert_eq!(value(s, "Sheet1", "A2"), Value::Empty);
        assert_eq!(value(s, "Sheet2", "C3"), Value::Number(10.0));
        assert_eq!(value(s, "Sheet2", "C4"), Value::Number(20.0));
        assert_eq!(value(s, "Sheet1", "B1"), Value::Number(30.0));
        assert_eq!(formula(s, "Sheet1", "B1"), "SUM(Sheet2!C3:C4)");
    };
    assert_moved(&s);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(value(&s, "Sheet1", "A1"), Value::Number(10.0));
    assert_eq!(value(&s, "Sheet1", "A2"), Value::Number(20.0));
    assert_eq!(value(&s, "Sheet2", "C3"), Value::Empty);
    assert_eq!(value(&s, "Sheet2", "C4"), Value::Empty);
    assert_eq!(value(&s, "Sheet1", "B1"), Value::Number(30.0));
    assert_eq!(formula(&s, "Sheet1", "B1"), "SUM(A1:A2)");
    run(&mut s, "edit.redo", json!({}));
    assert_moved(&s);

    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    run(&mut s, "file.open", json!({"name": "moved.xlsx", "base64": saved["base64"]}));
    assert_moved(&s);
}

#[test]
fn references_on_all_sheets_follow_cut_but_destination_local_refs_do_not() {
    let mut s = workbook();
    run(&mut s, "sheet.rename", json!({"name": "Source Data"}));
    run(&mut s, "range.setValues", json!({"range": "A1", "values": [[10], [20]]}));
    set(&mut s, "B1", "=SUM($A$1:$A$2)");
    run(&mut s, "formulas.defineName", json!({"name": "MovedData", "refersTo": "='Source Data'!$A$1:$A$2"}));
    run(&mut s, "home.insertSheet", json!({"name": "Target's Data"}));
    run(&mut s, "range.setValues", json!({"range": "A1", "values": [[100], [200]]}));
    set(&mut s, "B1", "=SUM('Source Data'!$A$1:$A$2)");
    set(&mut s, "B2", "=SUM(A1:A2)");
    run(&mut s, "home.insertSheet", json!({"name": "Summary"}));
    set(&mut s, "A1", "='Source Data'!$A$1+'Source Data'!A$2");
    set(&mut s, "A2", "=SUM(MovedData)");
    cut_to(&mut s, "Source Data", "A1:A2", "Target's Data", "C3");

    for (sheet, cell) in [("Source Data", "B1"), ("Target's Data", "B1"), ("Summary", "A1"), ("Summary", "A2")] {
        assert_eq!(value(&s, sheet, cell), Value::Number(30.0), "{sheet}!{cell}");
    }
    assert_eq!(formula(&s, "Source Data", "B1"), "SUM('Target''s Data'!$C$3:$C$4)");
    assert_eq!(formula(&s, "Summary", "A1"), "'Target''s Data'!$C$3+'Target''s Data'!C$4");
    assert_eq!(value(&s, "Target's Data", "B2"), Value::Number(300.0));
    assert_eq!(formula(&s, "Target's Data", "B2"), "SUM(A1:A2)");

    // Recalculation must keep tracking the destination after the move, not just retain a cache.
    activate(&mut s, "Target's Data");
    set(&mut s, "C3", "40");
    for (sheet, cell) in [("Source Data", "B1"), ("Target's Data", "B1"), ("Summary", "A1"), ("Summary", "A2")] {
        assert_eq!(value(&s, sheet, cell), Value::Number(60.0), "{sheet}!{cell}");
    }
}

#[test]
fn moved_formulas_keep_external_source_refs_and_move_internal_refs_once() {
    let mut s = workbook();
    run(&mut s, "sheet.rename", json!({"name": "Source Data"}));
    set(&mut s, "A1", "10");
    set(&mut s, "D1", "7");
    set(&mut s, "A2", "=$A$1+$D$1+'Source Data'!A1");
    set(&mut s, "A3", "=A2*2");
    run(&mut s, "home.insertSheet", json!({"name": "Destination"}));
    set(&mut s, "D1", "1000");
    // Overlapping coordinates on different sheets catch accidental double adjustment.
    cut_to(&mut s, "Source Data", "A1:A3", "Destination", "A2");

    assert_eq!(value(&s, "Destination", "A2"), Value::Number(10.0));
    assert_eq!(value(&s, "Destination", "A3"), Value::Number(27.0));
    assert_eq!(value(&s, "Destination", "A4"), Value::Number(54.0));
    assert_eq!(value(&s, "Source Data", "D1"), Value::Number(7.0));
    assert!(formula(&s, "Destination", "A3").contains("'Source Data'!$D$1"));
    for cell in ["A1", "A2", "A3"] {
        assert_eq!(value(&s, "Source Data", cell), Value::Empty);
    }
    activate(&mut s, "Source Data");
    set(&mut s, "D1", "8");
    assert_eq!(value(&s, "Destination", "A3"), Value::Number(28.0));
    assert_eq!(value(&s, "Destination", "A4"), Value::Number(56.0));
}

#[test]
fn same_sheet_overlapping_cut_still_updates_references_once() {
    let mut s = workbook();
    run(&mut s, "range.setValues", json!({"range": "A1", "values": [[10], [20]]}));
    set(&mut s, "B1", "=SUM(A1:A2)");
    set(&mut s, "A3", "=A1+A2");
    run(&mut s, "edit.cut", json!({"range": "A1:A3"}));
    run(&mut s, "edit.paste", json!({"at": "A2"}));
    assert_eq!(value(&s, "Sheet1", "A1"), Value::Empty);
    assert_eq!(value(&s, "Sheet1", "A2"), Value::Number(10.0));
    assert_eq!(value(&s, "Sheet1", "A3"), Value::Number(20.0));
    assert_eq!(value(&s, "Sheet1", "A4"), Value::Number(30.0));
    assert_eq!(value(&s, "Sheet1", "B1"), Value::Number(30.0));
    assert_eq!(formula(&s, "Sheet1", "A4"), "A2+A3");
    assert_eq!(formula(&s, "Sheet1", "B1"), "SUM(A2:A3)");
}
