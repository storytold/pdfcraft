use gridcraft_engine::Session;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Region", "Sales"], ["East", "10"], ["East", "2"], ["West", "4"]]})).unwrap();
    s
}

#[test]
fn insert_pivot_table_rejects_unknown_func() {
    for func in ["stdev", "sumx"] {
        let mut s = session();
        let r = s.execute(
            "insert.pivotTable",
            json!({"source":"Sheet1!A1:B4", "destination":"Sheet1!F1", "rows":["Region"], "values":[{"field":"Sales", "func":func}]}),
        );
        assert!(r.is_err(), "unknown func `{func}` accepted silently");
        assert!(s.doc().unwrap().wb.active().unwrap().pivots.is_empty());
    }
}

#[test]
fn insert_pivot_table_accepts_known_func() {
    let mut s = session();
    s.execute(
        "insert.pivotTable",
        json!({"source":"Sheet1!A1:B4", "destination":"Sheet1!F1", "rows":["Region"], "values":[{"field":"Sales", "func":"stddev"}]}),
    )
    .unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().pivots.len(), 1);
}
