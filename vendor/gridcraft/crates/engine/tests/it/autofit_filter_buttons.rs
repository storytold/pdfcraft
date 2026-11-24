use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use serde_json::json;

fn table() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range":"A1", "values":[["Quarterly Revenue"], ["1"]]})).unwrap();
    s.execute("insert.table", json!({"range":"A1:A2", "header":true})).unwrap();
    s
}

fn fit(s: &mut Session) -> f32 {
    s.execute("home.autofitColumnWidth", json!({"cols":"A:A"})).unwrap();
    s.doc().unwrap().wb.active().unwrap().col_width(0)
}

#[test]
fn autofit_reserves_room_for_table_filter_button() {
    let mut s = table();
    s.execute("selection.set", json!({"cell":"A1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    let plain = fit(&mut s);
    s.execute("data.filter", json!({})).unwrap();
    let filtered = fit(&mut s);
    assert!(filtered >= plain + 16.0, "filter button obscures heading: plain={plain}, filtered={filtered}");
    assert_eq!(s.doc().unwrap().wb.active().unwrap().value(CellRef::new(0, 0)).display(), "Quarterly Revenue");
}

fn sheet_mut(s: &mut Session) -> &mut gridcraft_engine::model::Sheet {
    std::sync::Arc::make_mut(&mut s.active_mut().unwrap().wb).sheet_mut(0).unwrap()
}

#[test]
fn autofit_skips_disabled_buttons_and_hidden_table_headers() {
    let mut s = table();
    sheet_mut(&mut s).tables[0].filter_button = false;
    let plain = fit(&mut s);
    sheet_mut(&mut s).tables[0].filter_button = true;
    sheet_mut(&mut s).tables[0].header_row = false;
    assert_eq!(fit(&mut s), plain);
    sheet_mut(&mut s).tables[0].header_row = true;
    assert!(fit(&mut s) > plain);
}

#[test]
fn autofit_matches_excel_filter_allowance_across_zoom_and_row_heights() {
    for (zoom, allowance) in [(50, 12.5 * 96.0 / 72.0 * 2.0), (100, 12.5 * 96.0 / 72.0), (200, 12.5 * 96.0 / 72.0)] {
        for height in [12.0_f32, 20.0, 60.0] {
            let mut s = table();
            s.execute("view.zoom", json!({"percent":zoom})).unwrap();
            s.execute("home.rowHeight", json!({"rows":"1:1", "height":height})).unwrap();
            sheet_mut(&mut s).tables[0].filter_button = false;
            let plain = fit(&mut s);
            sheet_mut(&mut s).tables[0].filter_button = true;
            let filtered = fit(&mut s);
            assert!((filtered - plain - allowance).abs() < 1.0, "zoom={zoom}, height={height}, plain={plain}, filtered={filtered}");
        }
    }
}

#[test]
fn autofit_supports_ordinary_autofilters_without_double_counting_table_buttons() {
    let mut s = table();
    let table_width = fit(&mut s);
    sheet_mut(&mut s).autofilter =
        Some(gridcraft_engine::model::AutoFilter { range: gridcraft_engine::core::RangeRef::parse("A1:A2").unwrap(), criteria: vec![] });
    assert_eq!(fit(&mut s), table_width, "overlapping filters occupy one button slot");
    sheet_mut(&mut s).tables.clear();
    assert_eq!(fit(&mut s), table_width, "ordinary filter headers reserve the same space");
    sheet_mut(&mut s).autofilter = None;
    assert!(fit(&mut s) < table_width);
}

#[test]
fn autofit_leaves_longer_body_cells_and_unfiltered_columns_unchanged() {
    let mut s = table();
    s.execute("cell.set", json!({"cell":"A2", "input":"A body value much longer than the short table header above"})).unwrap();
    let filtered = fit(&mut s);
    sheet_mut(&mut s).tables[0].filter_button = false;
    assert_eq!(fit(&mut s), filtered);
    s.execute("cell.set", json!({"cell":"B1", "input":"Quarterly Revenue"})).unwrap();
    s.execute("home.autofitColumnWidth", json!({"cols":"B:B"})).unwrap();
    let plain = s.doc().unwrap().wb.active().unwrap().col_width(1);
    sheet_mut(&mut s).tables[0].filter_button = true;
    s.execute("home.autofitColumnWidth", json!({"cols":"B:B"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().col_width(1), plain);
}

#[test]
fn autofit_filter_width_is_undoable_without_changing_selection() {
    let mut s = table();
    s.execute("home.columnWidth", json!({"cols":"A:A", "width":80})).unwrap();
    s.execute("selection.set", json!({"range":"A:A"})).unwrap();
    let selection = s.doc().unwrap().selection.clone();
    s.active_mut().unwrap().undo.clear();
    fit(&mut s);
    assert_eq!(s.doc().unwrap().selection, selection);
    assert_eq!(s.doc().unwrap().undo.len(), 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().col_width(0), 80.0);
}

#[test]
fn autofit_measures_each_distinct_text_once_and_quickly_past_the_limit() {
    use std::sync::{Arc, Mutex};
    let mut s = Session::new();
    s.new_workbook();
    // 2,500 distinct texts, each in two cells.
    let values: Vec<_> = (0..5000).map(|i| json!([format!("v{}", i % 2500)])).collect();
    s.execute("range.setValues", json!({"range":"A1", "values": values})).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    s.text_width = Some(Arc::new(move |text: &str, _: &gridcraft_engine::model::Font, exact: bool| {
        seen.lock().unwrap().push((text.to_owned(), exact));
        if text == "v2499" { 300.0 } else { 10.0 }
    }));
    s.execute("home.autofitColumnWidth", json!({"cols":"A:A"})).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2500, "one measure per distinct text");
    assert_eq!(calls.iter().filter(|(_, exact)| *exact).count(), 2000);
    assert!(calls.iter().skip(2000).all(|(_, exact)| !exact));
    // The quick measures still size the column.
    assert_eq!(s.doc().unwrap().wb.active().unwrap().col_width(0), 306.0);
}
