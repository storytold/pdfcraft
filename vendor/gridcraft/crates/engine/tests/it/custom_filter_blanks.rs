use gridcraft_engine::Session;
use serde_json::json;

fn visible_rows(op: &str) -> Vec<u32> {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["n"], ["3"], ["10"], [""], ["abc1"], ["20"]]})).unwrap();
    s.execute("selection.set", json!({"range":"A1:A6"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    s.execute("data.filterBy", json!({"column":0, "custom":{"op":op, "value":"5"}})).unwrap();
    let doc = s.doc().unwrap();
    let sh = doc.wb.active().unwrap();
    (1..6u32).filter(|r| !sh.rows.get(r).is_some_and(|i| i.hidden)).collect()
}

#[test]
fn less_than_numeric_criterion_hides_blank_and_text_rows() {
    assert_eq!(visible_rows("<"), vec![1]);
    assert_eq!(visible_rows("<="), vec![1]);
}
