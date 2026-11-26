//! Shift+arrows and Enter/Tab on merged cells, as Excel does them.

use gridcraft_engine::Session;
use serde_json::json;

/// A new workbook with B2:C3 merged and nothing selected but A1.
fn merged_book() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("selection.set", json!({"range": "B2:C3"})).unwrap();
    s.execute("home.mergeCells", json!({})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s
}

fn shift(s: &mut Session, dr: i64, dc: i64) -> String {
    s.execute("selection.move", json!({"dr": dr, "dc": dc, "extend": true})).unwrap();
    s.doc().unwrap().selection.a1()
}

fn enter(s: &mut Session, forward: bool, by_row: bool) -> (String, String) {
    s.execute("selection.next", json!({"forward": forward, "byRow": by_row})).unwrap();
    let sel = &s.doc().unwrap().selection;
    (sel.active.a1(), sel.a1())
}

#[test]
fn shift_arrows_from_a_merged_cell_widen_past_each_edge() {
    let mut s = merged_book();
    for ((dr, dc), want) in [((0, -1), "A2:C3"), ((-1, 0), "B1:C3"), ((0, 1), "B2:D3"), ((1, 0), "B2:C4")] {
        s.execute("selection.set", json!({"cell": "B2"})).unwrap();
        assert_eq!(s.doc().unwrap().selection.a1(), "B2:C3");
        assert_eq!(shift(&mut s, dr, dc), want, "Shift+({dr},{dc}) from the merged cell B2:C3");
        assert_eq!(s.doc().unwrap().selection.active.a1(), "B2", "the active cell stays");
    }
}

#[test]
fn shift_arrows_keep_going_and_come_back_to_the_merged_cell() {
    let mut s = merged_book();
    s.execute("selection.set", json!({"cell": "B2"})).unwrap();
    assert_eq!(shift(&mut s, 1, 0), "B2:C4");
    assert_eq!(shift(&mut s, 1, 0), "B2:C5");
    assert_eq!(shift(&mut s, 1, 0), "B2:C6");
    assert_eq!(shift(&mut s, -1, 0), "B2:C5");
    s.execute("selection.set", json!({"cell": "B2"})).unwrap();
    assert_eq!(shift(&mut s, 0, -1), "A2:C3");
    assert_eq!(shift(&mut s, 0, 1), "B2:C3", "back to just the merged cell");
    assert_eq!(shift(&mut s, 0, 1), "B2:D3");
    assert_eq!(shift(&mut s, -1, 0), "B1:D3");
    assert_eq!(shift(&mut s, 1, 0), "B2:D3");
}

#[test]
fn shift_arrows_shrink_past_a_merged_cell_they_pulled_in() {
    let mut s = merged_book();
    assert_eq!(shift(&mut s, 0, 1), "A1:B1");
    assert_eq!(shift(&mut s, 1, 0), "A1:C3", "B2 pulls in the whole merged cell");
    assert_eq!(shift(&mut s, 0, -1), "A1:A3", "one step back leaves the merged cell out");
    assert_eq!(shift(&mut s, 0, 1), "A1:C3");
    assert_eq!(shift(&mut s, -1, 0), "A1:C1");
    assert_eq!(shift(&mut s, 1, 0), "A1:C3");
    assert_eq!(shift(&mut s, 1, 0), "A1:C4");
}

#[test]
fn enter_and_tab_leave_a_selected_merged_cell() {
    let mut s = merged_book();
    for (forward, by_row, want) in [(true, false, "B4"), (false, false, "B1"), (true, true, "D2"), (false, true, "A2")] {
        s.execute("selection.set", json!({"cell": "B2"})).unwrap();
        assert_eq!(enter(&mut s, forward, by_row), (want.to_string(), want.to_string()), "forward {forward} byRow {by_row}");
    }
}

#[test]
fn enter_and_tab_out_of_a_merged_cell_step_over_hidden_lines() {
    let mut s = merged_book();
    s.execute("selection.set", json!({"range": "A4"})).unwrap();
    s.execute("home.hideRows", json!({})).unwrap();
    s.execute("selection.set", json!({"range": "D1"})).unwrap();
    s.execute("home.hideColumns", json!({})).unwrap();
    s.execute("selection.set", json!({"cell": "B2"})).unwrap();
    assert_eq!(enter(&mut s, true, false).0, "B5", "row 4 is hidden");
    s.execute("selection.set", json!({"cell": "B2"})).unwrap();
    assert_eq!(enter(&mut s, true, true).0, "E2", "column D is hidden");
    s.execute("selection.set", json!({"cell": "B2"})).unwrap();
    s.execute("selection.move", json!({"dr": 1})).unwrap();
    assert_eq!(s.doc().unwrap().selection.a1(), "B5", "the Down arrow too");
}

#[test]
fn enter_inside_a_selection_visits_a_merged_cell_once() {
    let mut s = merged_book();
    s.execute("selection.set", json!({"range": "A1:C3", "active": "A1"})).unwrap();
    let seen: Vec<String> = (0..6).map(|_| enter(&mut s, true, false).0).collect();
    assert_eq!(seen, ["A2", "A3", "B1", "B2", "C1", "A1"]);
    assert_eq!(s.doc().unwrap().selection.a1(), "A1:C3", "the selection stays");
    let back: Vec<String> = (0..2).map(|_| enter(&mut s, false, false).0).collect();
    assert_eq!(back, ["C1", "B2"]);
}

#[test]
fn shift_arrow_that_cannot_shrink_the_area_grows_it() {
    // A2:B3 merged, anchor B1: Shift+Down pulls the area out to column A, past the anchor.
    // Shift+Right can't shrink it from the left (the merged cell would come back), so it grows.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("selection.set", json!({"range": "A2:B3"})).unwrap();
    s.execute("home.mergeCells", json!({})).unwrap();
    s.execute("selection.set", json!({"cell": "B1"})).unwrap();
    assert_eq!(shift(&mut s, 1, 0), "A1:B3");
    assert_eq!(shift(&mut s, 0, 1), "A1:C3");
    assert_eq!(s.doc().unwrap().selection.active.a1(), "B1");
}

#[test]
fn shift_left_shrinks_past_a_chain_of_merged_cells_in_one_step() {
    // B2:C3 and D3:E4 are both inside A1:E4. Shift+Left from E can't stop in column D (D3:E4
    // would come back) and stops in C, where B2:C3 is whole.
    let mut s = merged_book();
    s.execute("selection.set", json!({"range": "D3:E4"})).unwrap();
    s.execute("home.mergeCells", json!({})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    assert_eq!(shift(&mut s, 3, 4), "A1:E4");
    assert_eq!(shift(&mut s, 0, -1), "A1:C4");
    assert_eq!(shift(&mut s, -1, 0), "A1:C3", "row 4 holds nothing merged once column D is out");
    assert_eq!(shift(&mut s, 0, -1), "A1:A3", "C can't stop inside B2:C3");
}

#[test]
fn shift_up_over_a_tall_merged_cell_takes_one_step() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("selection.set", json!({"range": "B2:C500000"})).unwrap();
    s.execute("home.mergeCells", json!({})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    assert_eq!(shift(&mut s, 0, 1), "A1:B1");
    assert_eq!(shift(&mut s, 1, 0), "A1:C500000");
    assert_eq!(shift(&mut s, -1, 0), "A1:C1", "row 1 is the last row above the merged cell");
    assert_eq!(shift(&mut s, 1, 0), "A1:C500000");
    assert_eq!(shift(&mut s, 0, -1), "A1:A500000");
}

#[test]
fn shift_arrows_change_only_the_last_area() {
    let mut s = merged_book();
    s.execute("selection.set", json!({"range": "E5:F6,B2"})).unwrap();
    assert_eq!(s.doc().unwrap().selection.a1(), "E5:F6,B2:C3");
    assert_eq!(shift(&mut s, 0, -1), "E5:F6,A2:C3");
}

#[test]
fn enter_skips_a_merged_cell_whose_start_is_outside_the_selection() {
    // A whole column isn't widened to the merged cells it touches. B1 is covered by A1:B1, whose
    // top-left cell is outside column B, so Enter skips it.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("selection.set", json!({"range": "A1:B1"})).unwrap();
    s.execute("home.mergeCells", json!({})).unwrap();
    s.execute("selection.set", json!({"range": "B:B", "active": "B2"})).unwrap();
    assert_eq!(enter(&mut s, false, false).0, "B1048576", "B1 is skipped, going up wraps to the bottom");
}
