use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS};
use gridcraft_engine::Session;
use serde_json::json;

#[test]
fn selection_move_clamps_extreme_displacements() {
    let mut session = Session::new();
    session.new_workbook();
    for (dr, dc, expected) in [
        (i64::MAX, 0, CellRef::new(MAX_ROWS - 1, 1)),
        (0, i64::MAX, CellRef::new(1, MAX_COLS - 1)),
        (i64::MAX, i64::MAX, CellRef::new(MAX_ROWS - 1, MAX_COLS - 1)),
        (i64::MIN, i64::MIN, CellRef::new(0, 0)),
        (i64::MAX, i64::MIN, CellRef::new(MAX_ROWS - 1, 0)),
        (i64::MIN, i64::MAX, CellRef::new(0, MAX_COLS - 1)),
        (2, -1, CellRef::new(3, 0)),
    ] {
        session.execute("selection.set", json!({"cell": "B2"})).unwrap();
        session.execute("selection.move", json!({"dr": dr, "dc": dc})).unwrap();
        assert_eq!(session.doc().unwrap().selection.active, expected);
    }
}

#[test]
fn selection_move_from_a_merged_cell_clamps_extreme_displacements() {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("selection.set", json!({"range": "B2:C3"})).unwrap();
    session.execute("home.mergeCells", json!({})).unwrap();
    for (dr, dc, expected) in [
        (i64::MAX, 0, CellRef::new(MAX_ROWS - 1, 1)),
        (0, i64::MAX, CellRef::new(1, MAX_COLS - 1)),
        (4_294_967_297, 0, CellRef::new(MAX_ROWS - 1, 1)),
        (1, 0, CellRef::new(3, 1)),
    ] {
        session.execute("selection.set", json!({"cell": "B2"})).unwrap();
        session.execute("selection.move", json!({"dr": dr, "dc": dc})).unwrap();
        assert_eq!(session.doc().unwrap().selection.active, expected);
    }
}
