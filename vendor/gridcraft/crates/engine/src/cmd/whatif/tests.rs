use gridcraft_core::{CellRef, Value};
use serde_json::json;

use crate::Session;

fn s() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}
fn v(s: &Session, a: &str) -> Value {
    let d = s.doc().unwrap();
    let (sh, body) = super::split_sheet(a);
    let si = sh.map(|n| d.wb.sheet_index(&n).unwrap()).unwrap_or(d.wb.active_sheet());
    d.wb.sheet(si).unwrap().value(CellRef::parse(body).unwrap())
}
fn n(s: &Session, a: &str) -> f64 {
    v(s, a).as_f64().unwrap_or(f64::NAN)
}

#[test]
fn goal_seek_linear_and_nonlinear() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B5", "input": "=B2*3+4"})).unwrap();
    let r = s.execute("data.goalSeek", json!({"set": "B5", "to": 25, "changing": "B2"})).unwrap();
    assert_eq!(r["converged"], true);
    assert!((n(&s, "B2") - 7.0).abs() < 0.001, "{r}");
    assert!((n(&s, "B5") - 25.0).abs() < 0.001);
    // One undo step restores the input.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(n(&s, "B2"), 1.0);
    // Nonlinear: x^2 = 2 from x = 1.
    s.execute("cell.set", json!({"cell": "C1", "input": "=B2^2"})).unwrap();
    let r = s.execute("data.goalSeek", json!({"set": "C1", "to": 2, "changing": "B2"})).unwrap();
    assert_eq!(r["converged"], true);
    assert!((n(&s, "C1") - 2.0).abs() <= 0.001);
    // Loan payment: what rate makes PMT = -500?
    s.execute("range.setValues", json!({"range": "E1", "values": [[0.01], [10000], [24]]})).unwrap();
    s.execute("cell.set", json!({"cell": "E4", "input": "=PMT(E1/12,E3,E2)"})).unwrap();
    let r = s.execute("data.goalSeek", json!({"set": "E4", "to": -500, "changing": "E1"})).unwrap();
    assert_eq!(r["converged"], true, "{r}");
    assert!((n(&s, "E4") + 500.0).abs() <= 0.001);
    // Starting at zero.
    s.execute("cell.set", json!({"cell": "G1", "input": "0"})).unwrap();
    s.execute("cell.set", json!({"cell": "G2", "input": "=G1*G1*G1-27"})).unwrap();
    let r = s.execute("data.goalSeek", json!({"set": "G2", "to": 0, "changing": "G1"})).unwrap();
    assert_eq!(r["converged"], true, "{r}");
    assert!((n(&s, "G1") - 3.0).abs() < 0.01);
}

#[test]
fn goal_seek_errors() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=A1"})).unwrap();
    assert!(s.execute("data.goalSeek", json!({"set": "A1", "to": 1, "changing": "A1"})).is_err());
    assert!(s.execute("data.goalSeek", json!({"set": "A2", "to": 1, "changing": "A2"})).is_err());
    assert!(s.execute("data.goalSeek", json!({"set": "A2", "changing": "A1"})).is_err());
    // Unreachable target: not converged, but no error and no crash.
    s.execute("cell.set", json!({"cell": "A3", "input": "=A1^2"})).unwrap();
    let r = s.execute("data.goalSeek", json!({"set": "A3", "to": -4, "changing": "A1"})).unwrap();
    assert_eq!(r["converged"], false);
    assert!(r["iterations"].as_u64().unwrap() <= 100);
}

#[test]
fn seek_unit() {
    let mut f = |x: f64| (x - 1e6) / 3.0;
    let r = super::seek(0.0, &mut f, 100, 0.001);
    assert!(r.converged, "{r:?}");
    let mut g = |x: f64| if x < 0.0 { f64::NAN } else { x.sqrt() - 5.0 };
    let r = super::seek(1.0, &mut g, 100, 0.001);
    assert!(r.converged, "{r:?}");
    let mut h = |_: f64| f64::NAN;
    let r = super::seek(1.0, &mut h, 100, 0.001);
    assert!(!r.converged);
}

#[test]
fn scenario_show_restores_a_percentage_cells_stored_number() {
    let mut s = s();
    s.execute("home.numberFormat", json!({"range": "B2", "format": "Percentage"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "5"})).unwrap();
    s.execute("data.scenarioAdd", json!({"name": "Low", "changing": "B2"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "9"})).unwrap();
    s.execute("data.scenarioShow", json!({"name": "Low"})).unwrap();
    assert_eq!(n(&s, "B2"), 0.05);
}

#[test]
fn scenarios() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "B2", "values": [[10], [20]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B10", "input": "=B2*B3"})).unwrap();
    s.execute("data.scenarioAdd", json!({"name": "Base", "changing": "B2:B3"})).unwrap();
    s.execute("data.scenarioAdd", json!({"name": "Best", "changing": "B2:B3", "values": [100, 3], "comment": "optimistic"})).unwrap();
    assert!(s.execute("data.scenarioAdd", json!({"name": "best", "changing": "B2:B3", "values": [1, 2]})).is_err());
    assert!(s.execute("data.scenarioAdd", json!({"name": "Bad", "changing": "B2:B3", "values": [1]})).is_err());
    let l = s.execute("data.scenarioManager", json!({})).unwrap();
    assert_eq!(l["scenarios"].as_array().unwrap().len(), 2);
    assert_eq!(l["scenarios"][1]["comment"], "optimistic");
    s.execute("data.scenarioShow", json!({"name": "Best"})).unwrap();
    assert_eq!(n(&s, "B10"), 300.0);
    s.execute("data.scenarioShow", json!({"name": "Base"})).unwrap();
    assert_eq!(n(&s, "B10"), 200.0);
    // Stored in a hidden workbook name.
    let d = s.doc().unwrap();
    assert!(d.wb.names.iter().any(|n| n.name == super::SCENARIO_NAME && n.hidden));
    assert_eq!(super::load_scenarios(&d.wb).len(), 2);
    // Long data is split into 250-character literals and survives an xlsx round trip.
    s.execute("data.scenarioAdd", json!({"name": "Long", "changing": "B2", "values": [1], "comment": "x".repeat(600)})).unwrap();
    let f = s.doc().unwrap().wb.names.iter().find(|n| n.name == super::SCENARIO_NAME).unwrap().formula.clone();
    assert!(f.matches("\"&\"").count() >= 2, "{f}");
    let b64 = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap()["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "sc.xlsx", "base64": b64})).unwrap();
    let l = s.execute("data.scenarioManager", json!({})).unwrap();
    assert_eq!(l["scenarios"].as_array().unwrap().len(), 3, "{l}");
    assert_eq!(l["scenarios"][2]["comment"].as_str().unwrap().len(), 600);
    s.execute("data.scenarioDelete", json!({"name": "Long"})).unwrap();
    let r = s.execute("data.scenarioSummary", json!({"resultCells": "B10"})).unwrap();
    assert_eq!(r["sheet"], "Scenario Summary");
    assert_eq!(s.doc().unwrap().wb.active().unwrap().name, "Scenario Summary");
    assert_eq!(v(&s, "'Scenario Summary'!D3"), Value::from("Base"));
    assert_eq!(v(&s, "'Scenario Summary'!B5"), Value::from("$B$2"));
    assert_eq!(n(&s, "'Scenario Summary'!E5"), 100.0);
    // Result row: current, Base, Best.
    assert_eq!(n(&s, "'Scenario Summary'!C8"), 200.0);
    assert_eq!(n(&s, "'Scenario Summary'!E8"), 300.0);
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("data.scenarioDelete", json!({"name": "Best"})).unwrap();
    assert_eq!(s.execute("data.scenarioManager", json!({})).unwrap()["scenarios"].as_array().unwrap().len(), 1);
    s.execute("data.scenarioDelete", json!({"name": "Base"})).unwrap();
    assert!(s.doc().unwrap().wb.names.iter().all(|n| n.name != super::SCENARIO_NAME));
    assert!(s.execute("data.scenarioSummary", json!({})).is_err());
}

#[test]
fn data_tables() {
    let mut s = s();
    // Loan: rate B1, amount B2; payment formula.
    s.execute("range.setValues", json!({"range": "B1", "values": [[0.05], [1000]]})).unwrap();
    // One-variable, column oriented: amounts down D3:D5, formula in E2.
    s.execute("range.setValues", json!({"range": "D3", "values": [[100], [200], [300]]})).unwrap();
    s.execute("cell.set", json!({"cell": "E2", "input": "=B2*(1+B1)"})).unwrap();
    s.execute("cell.set", json!({"cell": "F2", "input": "=B2*2"})).unwrap();
    let r = s.execute("data.dataTable", json!({"range": "D2:F5", "columnInput": "B2"})).unwrap();
    assert_eq!(r["cells"], 6);
    assert!((n(&s, "E3") - 105.0).abs() < 1e-9);
    assert_eq!(n(&s, "F5"), 600.0);
    // The input cell is untouched.
    assert_eq!(n(&s, "B2"), 1000.0);
    // Row oriented: rates across H2:J2, formula in G3.
    s.execute("range.setValues", json!({"range": "H2", "values": [[0.1, 0.2, 0.3]]})).unwrap();
    s.execute("cell.set", json!({"cell": "G3", "input": "=B2*(1+B1)"})).unwrap();
    s.execute("data.dataTable", json!({"range": "G2:J3", "rowInput": "B1"})).unwrap();
    assert!((n(&s, "J3") - 1300.0).abs() < 1e-9);
    // Two-variable: formula at L1, rates across, amounts down.
    s.execute("cell.set", json!({"cell": "L1", "input": "=B2*(1+B1)"})).unwrap();
    s.execute("range.setValues", json!({"range": "M1", "values": [[0.5, 1]]})).unwrap();
    s.execute("range.setValues", json!({"range": "L2", "values": [[10], [20]]})).unwrap();
    let r = s.execute("data.dataTable", json!({"range": "L1:N3", "rowInput": "B1", "columnInput": "B2"})).unwrap();
    assert_eq!(r["mode"], "twoVariable");
    assert_eq!(n(&s, "M2"), 15.0);
    assert_eq!(n(&s, "N3"), 40.0);
    assert!(s.execute("data.dataTable", json!({"range": "L1:N3"})).is_err());
    assert!(s.execute("data.dataTable", json!({"range": "A1:XFD100000", "columnInput": "B2"})).is_err());
    assert!(s.execute("data.dataTable", json!({"range": "L1:N3", "columnInput": "M2"})).is_err());
}

#[test]
fn advanced_filter_in_place_and_copy() {
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["Region", "Sales"], ["East", 50], ["West", 150], ["East", 200], ["North", 120], ["East", 200]]}),
    )
    .unwrap();
    // East AND Sales > 100, OR North.
    s.execute("range.setValues", json!({"range": "H1", "values": [["Region", "Sales"], ["East", ">100"], ["North", null]]})).unwrap();
    let r = s.execute("data.advancedFilter", json!({"range": "A1:B6", "criteria": "H1:I3"})).unwrap();
    assert_eq!(r["matched"], 3);
    let sh = || s.doc().unwrap().wb.active().unwrap().clone();
    assert!(sh().is_row_hidden(1));
    assert!(sh().is_row_hidden(2));
    assert!(!sh().is_row_hidden(3));
    assert!(!sh().is_row_hidden(4));
    // Unique, copied.
    let r = s.execute("data.advancedFilter", json!({"range": "A1:B6", "criteria": "H1:I3", "copyTo": "K1", "unique": true})).unwrap();
    assert_eq!(r["matched"], 2);
    assert_eq!(r["range"], "K1:L3");
    assert_eq!(v(&s, "K1"), Value::from("Region"));
    assert_eq!(v(&s, "K2"), Value::from("East"));
    assert_eq!(n(&s, "L2"), 200.0);
    assert_eq!(v(&s, "K3"), Value::from("North"));
    assert_eq!(v(&s, "K4"), Value::Empty);
    // Wildcards and exact text.
    s.execute("range.setValues", json!({"range": "H5", "values": [["Region"], ["*st"]]})).unwrap();
    let r = s.execute("data.advancedFilter", json!({"range": "A1:B6", "criteria": "H5:H6"})).unwrap();
    assert_eq!(r["matched"], 4);
    // Computed criterion (blank header): Sales above 100.
    s.execute("cell.set", json!({"cell": "J8", "input": "=B2>100"})).unwrap();
    let r = s.execute("data.advancedFilter", json!({"range": "A1:B6", "criteria": "J7:J8"})).unwrap();
    assert_eq!(r["matched"], 4);
    // Overlap is rejected.
    assert!(s.execute("data.advancedFilter", json!({"range": "A1:B6", "criteria": "H1:I3", "copyTo": "B2"})).is_err());
}

#[test]
fn advanced_filter_copy_preserves_cells_beyond_result() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Score"], ["A", 1], ["B", 2], ["C", 3]]})).unwrap();
    s.execute("range.setValues", json!({"range": "H1", "values": [["Name"], ["B"]]})).unwrap();
    s.execute("cell.set", json!({"cell": "K4", "input": "Keep me"})).unwrap();
    let r = s.execute("data.advancedFilter", json!({"range": "A1:B4", "criteria": "H1:H2", "copyTo": "K1"})).unwrap();
    assert_eq!(r["range"], "K1:L2");
    assert_eq!(v(&s, "K2"), Value::from("B"));
    assert_eq!(v(&s, "K4"), Value::from("Keep me"));
}

#[test]
fn advanced_filter_copy_rejects_overflow_without_mutation() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Score"], ["A", 1]]})).unwrap();
    s.execute("range.setValues", json!({"range": "H1", "values": [["Name"], ["A"]]})).unwrap();
    for dest in ["XFD1", "K1048576"] {
        s.execute("cell.set", json!({"cell": dest, "input": "Keep me"})).unwrap();
        assert!(s.execute("data.advancedFilter", json!({"range": "A1:B2", "criteria": "H1:H2", "copyTo": dest})).is_err());
        assert_eq!(v(&s, dest), Value::from("Keep me"));
    }
}

#[test]
fn advanced_filter_copy_allows_output_above_source_when_only_old_clear_area_overlaps() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "C4", "values": [["Name", "Score"], ["A", 1], ["B", 2], ["C", 3]]})).unwrap();
    s.execute("range.setValues", json!({"range": "H1", "values": [["Name"], ["A"]]})).unwrap();
    let r = s.execute("data.advancedFilter", json!({"range": "C4:D7", "criteria": "H1:H2", "copyTo": "B1"})).unwrap();
    assert_eq!(r["range"], "B1:C2");
    assert_eq!(v(&s, "C4"), Value::from("Name"));
    assert_eq!(v(&s, "C2"), Value::Number(1.0));
}

#[test]
fn advanced_filter_unique_without_criteria() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name"], ["A"], ["A"], ["B"]]})).unwrap();
    let r = s.execute("data.advancedFilter", json!({"range": "A1:A4", "copyTo": "C1", "unique": true})).unwrap();
    assert_eq!(r["matched"], 2);
    assert_eq!(r["range"], "C1:C3");
    assert_eq!(v(&s, "C2"), Value::from("A"));
    assert_eq!(v(&s, "C3"), Value::from("B"));
}

#[test]
fn criteria_semantics() {
    use super::criterion_matches as m;
    let t = |x: &str| Value::from(x);
    assert!(m(&Value::Number(150.0), "150", &t(">100")));
    assert!(!m(&Value::Number(50.0), "50", &t(">100")));
    assert!(!m(&t("abc"), "abc", &t(">100")));
    assert!(m(&t("Apple"), "Apple", &t("a")));
    assert!(m(&t("apple pie"), "apple pie", &t("=apple*")));
    assert!(!m(&t("apple pie"), "apple pie", &t("=apple")));
    assert!(m(&Value::Empty, "", &t("=")));
    assert!(m(&t("x"), "x", &t("<>")));
    assert!(m(&Value::Number(5.0), "5", &Value::Number(5.0)));
    assert!(m(&t("b"), "b", &t(">a")));
    assert!(m(&t("x"), "x", &t("<>y")));
}

#[test]
fn consolidate_by_position_and_label() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.rename", json!({"name": "East"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [["", "Q1", "Q2"], ["Apples", 10, 20], ["Pears", 5, 6]]})).unwrap();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.rename", json!({"name": "West"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [["", "Q2", "Q3"], ["Pears", 1, 2], ["Plums", 7, 8]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let r = s
        .execute("data.consolidate", json!({"sources": ["East!A1:C3", "West!A1:C3"], "destination": "A1", "topRow": true, "leftColumn": true}))
        .unwrap();
    assert_eq!(r["range"], "A1:D4");
    assert_eq!(v(&s, "Sheet1!B1"), Value::from("Q1"));
    assert_eq!(v(&s, "Sheet1!D1"), Value::from("Q3"));
    assert_eq!(v(&s, "Sheet1!A3"), Value::from("Pears"));
    assert_eq!(n(&s, "Sheet1!C3"), 7.0);
    assert_eq!(n(&s, "Sheet1!D4"), 8.0);
    assert_eq!(v(&s, "Sheet1!B4"), Value::Empty);
    // By position, average.
    let r = s.execute("data.consolidate", json!({"sources": ["East!B2:C3", "West!B2:C3"], "destination": "F1", "function": "average"})).unwrap();
    assert_eq!(r["range"], "F1:G2");
    assert_eq!(n(&s, "Sheet1!F1"), 5.5);
    assert_eq!(n(&s, "Sheet1!G2"), 7.0);
    assert!(s.execute("data.consolidate", json!({"sources": []})).is_err());
    assert!(s.execute("data.consolidate", json!({"sources": ["A1"], "function": "median"})).is_err());
}

#[test]
fn advanced_filter_refuses_a_list_times_criteria_product_over_the_work_limit() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name"], ["A"]]})).unwrap();
    s.execute("range.setValues", json!({"range": "H1", "values": [["Name"], ["A"]]})).unwrap();
    // A sparse million-row list against 9,999 blank criteria rows would be ~10 billion tests.
    assert!(s.execute("data.advancedFilter", json!({"range": "A2:A1048576", "criteria": "H1:H10000"})).is_err());
}

/// A hundred thousand positional rows take one key lookup each; a tall source and a wide one
/// whose keys would meet in a table of 160 million cells are refused before it is built.
#[test]
fn consolidate_keys_are_indexed_and_the_output_is_bounded() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2]]})).unwrap();
    let r = s.execute("data.consolidate", json!({"sources": ["A1:A100000"], "destination": "C1"})).unwrap();
    assert_eq!(r["rows"], 100_000);
    assert_eq!(n(&s, "Sheet1!C2"), 2.0);
    let r = s.execute("data.consolidate", json!({"sources": ["A1:A10000", "A1:XFC1"], "destination": "A1"}));
    assert!(r.unwrap_err().to_string().contains("too large"));
}

#[test]
fn scenario_summary_refuses_a_matrix_over_its_cell_limit() {
    let mut s = s();
    // 200 scenarios of 32 disjoint cells: 6,400 changing cells x 200 scenarios = 1.28 million cells.
    for i in 0..200 {
        let (a, b) = (i * 32 + 1, i * 32 + 32);
        s.execute("data.scenarioAdd", json!({"name": format!("S{i}"), "changing": format!("A{a}:A{b}")})).unwrap();
    }
    assert!(s.execute("data.scenarioSummary", json!({})).is_err());
}

#[test]
fn scenario_summary_counts_result_rows_in_its_cell_limit() {
    let mut s = s();
    // 900 scenarios of one distinct cell each are 810,000 cells; 255 result rows take it past the limit.
    for i in 0..900 {
        s.execute("data.scenarioAdd", json!({"name": format!("S{i}"), "changing": format!("A{}", i + 1)})).unwrap();
    }
    let err = s.execute("data.scenarioSummary", json!({"resultCells": "C1:C255"})).unwrap_err().to_string();
    assert!(err.contains("too many cells"), "{err}");
}
