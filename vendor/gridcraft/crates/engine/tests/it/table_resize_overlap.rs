use gridcraft_engine::Session;
use serde_json::json;

fn table() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Item", "Qty"], ["x", "7"]]})).unwrap();
    s.execute("insert.table", json!({"range":"A1:B2", "header":true, "name":"T"})).unwrap();
    s
}

fn table_range(s: &Session) -> String {
    s.doc().unwrap().wb.active().unwrap().tables[0].range.a1()
}

#[test]
fn resize_to_disjoint_range_is_rejected_and_keeps_table() {
    let mut s = table();
    assert!(s.execute("table.resize", json!({"table":"T", "range":"D1:E2"})).is_err());
    assert_eq!(table_range(&s), "A1:B2");
    assert_eq!(s.doc().unwrap().wb.active().unwrap().tables[0].columns[1].name, "Qty");
}

#[test]
fn resize_to_overlapping_range_still_works() {
    let mut s = table();
    s.execute("table.resize", json!({"table":"T", "range":"B1:C2"})).unwrap();
    assert_eq!(table_range(&s), "B1:C2");
}

fn two_tables() -> Session {
    let mut s = table();
    s.execute("range.setValues", json!({"range":"D1", "values":[["P", "Q"], ["y", "8"]]})).unwrap();
    s.execute("insert.table", json!({"range":"D1:E2", "header":true, "name":"U"})).unwrap();
    s
}

fn ranges(s: &Session) -> Vec<String> {
    s.doc().unwrap().wb.active().unwrap().tables.iter().map(|t| t.range.a1()).collect()
}

#[test]
fn resize_over_another_table_is_rejected() {
    let mut s = two_tables();
    assert!(s.execute("table.resize", json!({"table":"T", "range":"A1:E2"})).is_err());
    assert_eq!(ranges(&s), ["A1:B2", "D1:E2"]);
    assert!(s.execute("table.resize", json!({"table":"U", "range":"A1:E2"})).is_err());
    assert_eq!(ranges(&s), ["A1:B2", "D1:E2"]);
}

#[test]
fn resize_next_to_another_table_still_works() {
    let mut s = two_tables();
    s.execute("table.resize", json!({"table":"T", "range":"A1:C3"})).unwrap();
    assert_eq!(ranges(&s), ["A1:C3", "D1:E2"]);
}
