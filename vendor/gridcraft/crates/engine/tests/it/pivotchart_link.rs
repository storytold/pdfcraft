//! #1447: a PivotChart is bound to its PivotTable behind the workbook's name.

use std::io::Read;

use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn run(s: &mut Session, command: &str, params: Json) -> Json {
    s.execute(command, params).unwrap()
}

fn source() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    run(
        &mut s,
        "range.setValues",
        json!({"range": "A1", "values": [["Region ", " Sales ", "Qty"], ["East", 10, 1], ["West", 20, 2], ["East", 5, 3]]}),
    );
    s
}

#[test]
fn pivot_chart_link_names_the_workbook_in_brackets() {
    let mut s = source();
    run(&mut s, "selection.set", json!({"range": "A1"}));
    run(&mut s, "insert.pivotTable", json!({"rows": ["Region"], "values": ["Sales"]}));
    run(&mut s, "insert.pivotChart", json!({}));
    let bytes = gridcraft_engine::io::save_bytes(&s.doc().unwrap().wb, "Q1 Report.xlsx").unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut chart = String::new();
    zip.by_name("xl/charts/chart1.xml").unwrap().read_to_string(&mut chart).unwrap();
    assert!(chart.contains("<c:name>[Q1 Report.xlsx]Sheet2!PivotTable1</c:name>"), "{chart}");

    // Without a file name, a placeholder workbook is named; reading drops the brackets again.
    let saved = run(&mut s, "file.saveBytes", json!({"format": "xlsx"}));
    let mut reopened = Session::new();
    run(&mut reopened, "file.open", json!({"name": "q.xlsx", "base64": saved["base64"]}));
    let link = reopened.doc().unwrap().wb.sheets.iter().flat_map(|sh| sh.charts.iter()).find_map(|c| c.pivot.clone());
    assert_eq!(link.as_deref(), Some("Sheet2!PivotTable1"));
}
