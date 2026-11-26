//! Whole-row and whole-column formatting must re-fit row heights (issue #833).

use serde_json::json;

fn sheet_mut(s: &mut gridcraft_engine::Session) -> &mut gridcraft_engine::model::Sheet {
    std::sync::Arc::make_mut(&mut s.active_mut().unwrap().wb).sheet_mut(0).unwrap()
}

#[test]
fn whole_row_font_format_grows_row_height_and_undoes() {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A5", "values": [["plain big bold"]]})).unwrap();
    let before = s.doc().unwrap().wb.active().unwrap().row_height(4);
    s.execute("home.fontSize", json!({"range": "5:5", "size": 20})).unwrap();
    let after = s.doc().unwrap().wb.active().unwrap().row_height(4);
    assert!(after > before + 10.0, "20pt whole-row format should grow the row: before={before}, after={after}");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(4), before, "undo must restore the height");
}

#[test]
fn whole_column_font_format_grows_only_rows_with_cells() {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a"]]})).unwrap();
    s.execute("range.setValues", json!({"range": "A50", "values": [["b"]]})).unwrap();
    s.execute("home.fontSize", json!({"range": "A:A", "size": 20})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert!(sh.row_height(0) > 30.0 && sh.row_height(49) > 30.0, "rows with cells grow");
    assert!(sh.rows.len() <= 3, "full-column format must fit only rows that have cells, rows map has {} entries", sh.rows.len());
}

#[test]
fn whole_row_format_does_not_resize_custom_height_rows() {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A5", "values": [["plain big bold"]]})).unwrap();
    {
        let sh = sheet_mut(&mut s);
        let e = sh.rows.entry(4).or_default();
        e.custom = true;
        e.size = Some(25.0);
    }
    s.execute("home.fontSize", json!({"range": "5:5", "size": 20})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(4), 25.0, "custom-height rows keep their height");
}

#[test]
fn whole_row_format_keeps_hidden_rows_hidden() {
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A5", "values": [["plain big bold"]]})).unwrap();
    let before = s.doc().unwrap().wb.active().unwrap().row_height(4);
    sheet_mut(&mut s).rows.entry(4).or_default().hidden = true;
    s.execute("home.fontSize", json!({"range": "5:5", "size": 20})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    let info = sh.rows.get(&4).unwrap();
    assert!(info.size.unwrap_or(before) > before + 10.0, "hidden row still grows to its font: before={before}, size={:?}", info.size);
    assert!(info.hidden, "formatting a hidden row must not unhide it");
}

#[test]
fn many_merges_fit_rows_the_same_and_skip_merged_cells() {
    // Rows are fitted with one sweep over the sheet's merges; a merged cell still never grows
    // its row, and rows next to thousands of merges still fit.
    let mut s = gridcraft_engine::Session::new();
    s.new_workbook();
    let values: Vec<_> = (0..2_000).map(|i| json!([format!("v{i}")])).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
    {
        let sh = sheet_mut(&mut s);
        sh.merges.push(gridcraft_engine::core::RangeRef::parse("A3:B4").unwrap());
        for i in 0..20_000u32 {
            let row = 4 + i % 1_990;
            sh.merges.push(gridcraft_engine::core::RangeRef::new(
                gridcraft_engine::core::CellRef::new(row, 3 + (i % 7) * 2),
                gridcraft_engine::core::CellRef::new(row + 3, 4 + (i % 7) * 2),
            ));
        }
    }
    s.execute("home.fontSize", json!({"range": "A:A", "size": 20})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    let default = sh.default_row_height;
    assert!(sh.row_height(0) > 30.0 && sh.row_height(1_999) > 30.0, "rows with big text grow");
    assert_eq!(sh.row_height(2), default, "a merged cell does not grow its row");
    assert_eq!(sh.row_height(3), default, "nor any row of the merge");
    assert!(sh.row_height(4) > 30.0 && sh.row_height(1_000) > 30.0, "rows beside other merges grow");
}
