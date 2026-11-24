//! #1458: sheet names are quoted in references the way Excel quotes them, in formulas, saved
//! files, PivotTable sources and ADDRESS.

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use serde_json::{Value as Json, json};

fn run(s: &mut Session, command: &str, params: Json) -> Json {
    s.execute(command, params).unwrap()
}

fn formula(s: &Session, cell: &str) -> String {
    s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse(cell).unwrap()).unwrap().formula.as_ref().unwrap().text.clone()
}

#[test]
fn formulas_keep_the_quotes_excel_needs_through_a_save() {
    let mut s = Session::new();
    s.new_workbook();
    for name in [".x", "R1C1", "A1", "Bob's"] {
        run(&mut s, "home.insertSheet", json!({"name": name}));
    }
    run(&mut s, "sheet.activate", json!({"sheet": "Sheet1"}));
    let input = "='.x'!A1+'R1C1'!A1+'A1'!B1+'Bob''s'!A1";
    run(&mut s, "cell.set", json!({"cell": "A1", "input": input}));
    assert_eq!(formula(&s, "A1"), input.trim_start_matches('='));

    let saved = run(&mut s, "file.saveBytes", json!({"format": "xlsx"}));
    let mut reopened = Session::new();
    run(&mut reopened, "file.open", json!({"name": "q.xlsx", "base64": saved["base64"]}));
    run(&mut reopened, "sheet.activate", json!({"sheet": "Sheet1"}));
    assert_eq!(formula(&reopened, "A1"), input.trim_start_matches('='));
}

#[test]
fn address_and_pivot_source_quote_a_sheet_named_like_a_cell() {
    let mut s = Session::new();
    s.new_workbook();
    run(&mut s, "home.insertSheet", json!({"name": "A1"}));
    run(&mut s, "sheet.activate", json!({"sheet": "A1"}));
    for (cell, input) in [("A1", "K"), ("B1", "V"), ("A2", "a"), ("B2", "1"), ("A3", "b"), ("B3", "2")] {
        run(&mut s, "cell.set", json!({"cell": cell, "input": input}));
    }
    run(&mut s, "cell.set", json!({"cell": "D1", "input": "=ADDRESS(1,1,1,TRUE,\"A1\")"}));
    let d1 = s.doc().unwrap().wb.active().unwrap().value(CellRef::parse("D1").unwrap());
    assert_eq!(d1, Value::from("'A1'!$A$1"));

    run(&mut s, "selection.set", json!({"range": "A2"}));
    let pivot = run(&mut s, "insert.pivotTable", json!({"rows": ["K"], "values": ["V"]}));
    assert_eq!(pivot["source"], json!("'A1'!$A$1:$B$3"));
}
