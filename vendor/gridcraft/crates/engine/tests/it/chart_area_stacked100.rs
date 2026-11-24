//! The 100% stacked area chart: `insert.chart` and `chart.set` take type `area` with subtype
//! `stacked100`.

use gridcraft_engine::Session;
use gridcraft_model::ChartKind;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Quarter", "North", "South"], ["Q1", "10", "30"], ["Q2", "30", "10"]]})).unwrap();
    s
}

fn kinds(s: &Session) -> Vec<ChartKind> {
    s.doc().unwrap().wb.sheet(0).unwrap().charts.iter().map(|c| c.kind).collect()
}

#[test]
fn insert_chart_makes_a_hundred_percent_stacked_area() {
    let mut s = session();
    s.execute("insert.chart", json!({"range": "A1:C3", "type": "area", "subtype": "stacked100"})).unwrap();
    assert_eq!(kinds(&s), [ChartKind::AreaStacked100]);
}

#[test]
fn chart_set_changes_a_chart_to_a_hundred_percent_stacked_area_and_back() {
    let mut s = session();
    s.execute("insert.chart", json!({"range": "A1:C3", "type": "area", "subtype": "stacked"})).unwrap();
    assert_eq!(kinds(&s), [ChartKind::AreaStacked]);
    s.execute("chart.set", json!({"type": "area", "subtype": "stacked100"})).unwrap();
    assert_eq!(kinds(&s), [ChartKind::AreaStacked100]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(kinds(&s), [ChartKind::AreaStacked]);
}
