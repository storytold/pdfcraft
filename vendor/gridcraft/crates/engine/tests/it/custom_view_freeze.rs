use gridcraft_engine::Session;
use serde_json::json;

fn freeze(s: &Session) -> Option<(u32, u32)> {
    s.doc().unwrap().wb.active().unwrap().freeze
}

#[test]
fn showing_a_custom_view_restores_its_frozen_panes() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("view.freezePanes", json!({"cell": "B2"})).unwrap();
    s.execute("view.customViews", json!({"save": "Frozen"})).unwrap();
    s.execute("view.unfreezePanes", json!({})).unwrap();
    assert_eq!(freeze(&s), None);
    s.execute("view.customViews", json!({"show": "Frozen"})).unwrap();
    assert_eq!(freeze(&s), Some((1, 1)));
}

#[test]
fn showing_an_unfrozen_custom_view_clears_current_panes() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("view.customViews", json!({"save": "Plain"})).unwrap();
    s.execute("view.freezePanes", json!({"cell": "B2"})).unwrap();
    s.execute("view.customViews", json!({"show": "Plain"})).unwrap();
    assert_eq!(freeze(&s), None);
}
