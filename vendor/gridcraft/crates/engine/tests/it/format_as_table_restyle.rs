use gridcraft_engine::Session;
use serde_json::json;

fn table() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Item", "Qty"], ["x", "7"], ["y", "8"]]})).unwrap();
    s.execute("insert.table", json!({"range":"A1:B3", "header":true, "name":"T"})).unwrap();
    s
}

fn styles(s: &Session) -> Vec<String> {
    s.doc().unwrap().wb.active().unwrap().tables.iter().map(|t| t.style.clone()).collect()
}

#[test]
fn format_as_table_inside_a_table_restyles_it() {
    let mut s = table();
    s.execute("selection.set", json!({"range":"B2"})).unwrap();
    s.execute("home.formatAsTable", json!({"style":"TableStyleLight9"})).unwrap();
    assert_eq!(styles(&s), ["TableStyleLight9"]);
}

#[test]
fn format_as_table_on_a_range_inside_a_table_restyles_it() {
    let mut s = table();
    s.execute("home.formatAsTable", json!({"range":"A2:B3", "style":"TableStyleDark3"})).unwrap();
    assert_eq!(styles(&s), ["TableStyleDark3"]);
}

#[test]
fn format_as_table_across_a_table_edge_is_rejected() {
    let mut s = table();
    let e = s.execute("home.formatAsTable", json!({"range":"A1:C3", "style":"TableStyleLight9"})).unwrap_err();
    assert!(e.to_string().contains("overlap"), "{e}");
    assert_eq!(styles(&s), ["TableStyleMedium2"]);
}

#[test]
fn format_as_table_inside_a_table_returns_that_table() {
    let mut s = table();
    let r = s.execute("home.formatAsTable", json!({"range":"B2", "style":"TableStyleLight9"})).unwrap();
    assert_eq!(r, json!({"table":"T", "range":"A1:B3"}));
}

#[test]
fn format_as_table_uses_the_active_area_of_a_multi_area_selection() {
    let mut s = table();
    s.execute("selection.set", json!({"range":"D10:E11,B2"})).unwrap();
    s.execute("home.formatAsTable", json!({"style":"TableStyleLight9"})).unwrap();
    assert_eq!(styles(&s), ["TableStyleLight9"]);
}

#[test]
fn format_as_table_inside_a_table_with_data_beside_it_restyles_it() {
    let mut s = table();
    s.execute("range.setValues", json!({"range":"C1", "values":[["Note"], ["a"], ["b"]]})).unwrap();
    s.execute("selection.set", json!({"range":"B2"})).unwrap();
    s.execute("home.formatAsTable", json!({"style":"TableStyleLight9"})).unwrap();
    assert_eq!(styles(&s), ["TableStyleLight9"]);
}
