//! A chart's data as blocks on the sheet, and `chart.setValuesRange`, which dragging a corner of a
//! selected chart's values outline runs (#4389, #4390).

use gridcraft_core::RangeRef;
use gridcraft_model::{Chart, Color};
use serde_json::json;

use super::s;
use crate::Session;
use crate::chart_data::{ChartData, chart_data};

fn r(a: &str) -> RangeRef {
    RangeRef::parse(a).unwrap()
}

fn chart(s: &Session) -> Chart {
    s.doc().unwrap().wb.active().unwrap().charts[0].clone()
}

fn data(s: &Session) -> Option<ChartData> {
    chart_data(&s.doc().unwrap().wb, 0, &chart(s))
}

/// Points each plotted series has, as the chart resolves them.
fn points(s: &Session) -> Vec<usize> {
    gridcraft_chart::resolve(&s.doc().unwrap().wb, 0, &chart(s)).series.iter().map(|se| se.values.len()).collect()
}

/// 10..80 down A1:A8, charted from A1:A5 only.
fn single_column() -> Session {
    let mut s = s();
    let values: Vec<_> = (1..=8).map(|i| json!([i * 10])).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
    s.execute("selection.set", json!({"range": "A1:A5"})).unwrap();
    s.execute("insert.chart", json!({"type": "column"})).unwrap();
    s
}

/// Region names across row 1, quarters down column A, charted from A1:C4.
fn table() -> Session {
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [
            ["", "North", "South", "East"],
            ["Q1", 10, 15, 12],
            ["Q2", 20, 25, 22],
            ["Q3", 30, 35, 32],
            ["Q4", 40, 45, 42],
            ["Q5", 50, 55, 52],
        ]}),
    )
    .unwrap();
    s.execute("selection.set", json!({"range": "A1:C4"})).unwrap();
    s.execute("insert.chart", json!({"type": "column"})).unwrap();
    s
}

#[test]
fn a_plain_column_chart_is_one_values_block() {
    let s = single_column();
    assert_eq!(data(&s), Some(ChartData { sheet: 0, values: r("A1:A5"), by_rows: false, categories: None, names: None }));
}

#[test]
fn a_table_chart_has_names_categories_and_values_blocks() {
    let s = table();
    assert_eq!(data(&s), Some(ChartData { sheet: 0, values: r("B2:C4"), by_rows: false, categories: Some(r("A2:A4")), names: Some(r("B1:C1")) }));
}

#[test]
fn dragging_the_values_down_plots_the_new_rows_and_undo_puts_them_back() {
    let mut s = single_column();
    assert_eq!(points(&s), [5]);
    let out = s.execute("chart.setValuesRange", json!({"range": "A1:A8"})).unwrap();
    assert_eq!(out["values"], "Sheet1!A1:A8");
    assert_eq!(chart(&s).series[0].values, "Sheet1!$A$1:$A$8");
    assert_eq!(points(&s), [8]);
    assert_eq!(data(&s).unwrap().values, r("A1:A8"));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(points(&s), [5]);
    assert_eq!(chart(&s).series[0].values, "Sheet1!$A$1:$A$5");
}

#[test]
fn a_wider_block_adds_series_and_existing_series_keep_their_formatting() {
    let mut s = table();
    s.execute("chart.formatSelection", json!({"series": 1, "color": "#C00000"})).unwrap();
    let south = chart(&s).series[1].color;
    assert!(south.is_some());
    s.execute("chart.setValuesRange", json!({"range": "B2:D6"})).unwrap();
    let c = chart(&s);
    assert_eq!(c.series.len(), 3);
    assert_eq!(c.series[1].color, south, "South keeps its colour");
    assert_eq!(c.series[2].color, None::<Color>, "the new series takes the automatic colour");
    let names: Vec<_> = c.series.iter().map(|se| se.name.clone().unwrap()).collect();
    assert_eq!(names, ["Sheet1!$B$1", "Sheet1!$C$1", "Sheet1!$D$1"], "names follow along the header row");
    assert!(c.series.iter().all(|se| se.categories.as_deref() == Some("Sheet1!$A$2:$A$6")), "labels span the new points");
    assert_eq!(c.series[2].values, "Sheet1!$D$2:$D$6");
    assert_eq!(c.source.as_deref(), Some("Sheet1!A1:D6"), "Switch Row/Column sees the moved blocks");
    assert_eq!(points(&s), [5, 5, 5]);
    assert_eq!(data(&s), Some(ChartData { sheet: 0, values: r("B2:D6"), by_rows: false, categories: Some(r("A2:A6")), names: Some(r("B1:D1")) }));
    // A narrower block drops the last series.
    s.execute("chart.setValuesRange", json!({"range": "B2:B3"})).unwrap();
    let c = chart(&s);
    assert_eq!(c.series.len(), 1);
    assert_eq!(c.series[0].categories.as_deref(), Some("Sheet1!$A$2:$A$3"));
    s.execute("chart.switchRowColumn", json!({})).unwrap();
}

#[test]
fn series_in_rows_grow_across_the_columns() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4, 5, 6, 7], [7, 6, 5, 4, 3, 2, 1]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:E2"})).unwrap();
    s.execute("insert.chart", json!({"type": "line"})).unwrap();
    assert_eq!(data(&s).map(|d| (d.values, d.by_rows)), Some((r("A1:E2"), true)));
    s.execute("chart.setValuesRange", json!({"range": "A1:G2"})).unwrap();
    assert_eq!(points(&s), [7, 7]);
    assert_eq!(chart(&s).series[1].values, "Sheet1!$A$2:$G$2");
}

/// Changes the first chart on the first sheet in place, as a file can leave it.
fn edit_chart(s: &mut Session, f: impl FnOnce(&mut Chart)) {
    let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    f(&mut wb.sheet_mut(0).unwrap().charts[0]);
}

#[test]
fn category_labels_written_with_and_without_equals_are_one_block() {
    let mut s = table();
    edit_chart(&mut s, |c| c.series[1].categories = Some("=Sheet1!$A$2:$A$4".into()));
    assert_eq!(data(&s).unwrap().categories, Some(r("A2:A4")));
    s.execute("chart.setValuesRange", json!({"range": "B2:C6"})).unwrap();
    assert!(chart(&s).series.iter().all(|se| se.categories.as_deref() == Some("Sheet1!$A$2:$A$6")), "both series' labels follow");
}

#[test]
fn series_read_in_rows_stay_resizable_after_shrinking_to_one_cell_each() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4, 5], [5, 4, 3, 2, 1]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:E2"})).unwrap();
    s.execute("insert.chart", json!({"type": "line"})).unwrap();
    // A file can carry series in rows without saying so; the layout is read from the series.
    edit_chart(&mut s, |c| c.by_rows = false);
    assert_eq!(data(&s).map(|d| d.by_rows), Some(true));
    s.execute("chart.setValuesRange", json!({"range": "A1:A2"})).unwrap();
    assert!(chart(&s).by_rows);
    assert_eq!(data(&s).map(|d| (d.values, d.by_rows)), Some((r("A1:A2"), true)), "still one block, still in rows");
    s.execute("chart.setValuesRange", json!({"range": "A1:C2"})).unwrap();
    assert_eq!(points(&s), [3, 3]);
}

#[test]
fn set_values_range_refuses_what_it_cannot_do() {
    let mut s = table();
    let before = chart(&s);
    for (params, why) in [
        (json!({}), "missing `range`"),
        (json!({"range": "not a range"}), "not a range"),
        (json!({"range": "A1:Z100000"}), "too large"),
        (json!({"range": "A1:JA2"}), "too large"),
        (json!({"range": "Sheet2!B2:C4"}), ""),
        (json!({"chart": 999, "range": "B2:C4"}), "chart"),
    ] {
        let err = s.execute("chart.setValuesRange", params.clone()).unwrap_err();
        assert!(err.to_string().contains(why), "{params}: {err}");
        assert_eq!(chart(&s), before, "{params} left the chart as it was");
    }
    // Series with a gap between them are not one block.
    s.execute("selection.set", json!({"range": "A1:D4"})).unwrap();
    s.execute("chart.selectData", json!({"range": "A1:D4"})).unwrap();
    s.execute("chart.selectData", json!({"removeSeries": [1]})).unwrap();
    assert_eq!(data(&s), None);
    let err = s.execute("chart.setValuesRange", json!({"range": "B2:C4"})).unwrap_err();
    assert!(err.to_string().contains("not one block"), "{err}");
}
