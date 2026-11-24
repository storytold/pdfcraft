//! Ctrl/Cmd+Shift+arrow searches the active cell's column (or row) from the moving edge (#3484).

use gridcraft_engine::Session;
use serde_json::json;

fn table() -> Session {
    // Headers in A2:K2, a key column in A3:A13, nothing else filled.
    let mut s = Session::new();
    s.new_workbook();
    let header: Vec<i64> = (1..=11).collect();
    s.execute("range.setValues", json!({"range": "A2:K2", "values": [header]})).unwrap();
    let keys: Vec<Vec<i64>> = (1..=11).map(|i| vec![i]).collect();
    s.execute("range.setValues", json!({"range": "A3:A13", "values": keys})).unwrap();
    s
}

fn ctrl_shift(s: &mut Session, dr: i64, dc: i64) -> String {
    s.execute("selection.move", json!({"dr": dr, "dc": dc, "extend": true, "jump": true})).unwrap();
    s.doc().unwrap().selection.a1()
}

#[test]
fn ctrl_shift_down_after_ctrl_shift_right_follows_the_active_column() {
    let mut s = table();
    s.execute("selection.set", json!({"cell": "A2"})).unwrap();
    assert_eq!(ctrl_shift(&mut s, 0, 1), "A2:K2");
    assert_eq!(ctrl_shift(&mut s, 1, 0), "A2:K13", "column A's data ends on row 13; column K's ends on row 2");
    // From the end of column A's data the next press runs to the sheet's last row.
    assert_eq!(ctrl_shift(&mut s, 1, 0), "A2:K1048576");
}

#[test]
fn ctrl_shift_right_after_ctrl_shift_down_follows_the_active_row() {
    let mut s = table();
    s.execute("selection.set", json!({"cell": "A2"})).unwrap();
    assert_eq!(ctrl_shift(&mut s, 1, 0), "A2:A13");
    assert_eq!(ctrl_shift(&mut s, 0, 1), "A2:K13", "row 2's data ends in column K; row 13's ends in column A");
}

#[test]
fn ctrl_shift_down_after_ctrl_shift_left_follows_the_active_column() {
    // The active cell is on the right: column K, empty below the header, decides, not column A.
    let mut s = table();
    s.execute("selection.set", json!({"cell": "K2"})).unwrap();
    assert_eq!(ctrl_shift(&mut s, 0, -1), "A2:K2");
    assert_eq!(ctrl_shift(&mut s, 1, 0), "A2:K1048576");
}

#[test]
fn ctrl_shift_right_after_ctrl_shift_up_follows_the_active_row() {
    // The active cell is at the bottom: row 13, empty right of the key, decides, not row 2.
    let mut s = table();
    s.execute("selection.set", json!({"cell": "A13"})).unwrap();
    assert_eq!(ctrl_shift(&mut s, -1, 0), "A2:A13");
    assert_eq!(ctrl_shift(&mut s, 0, 1), "2:13", "row 13's data ends in column A, so the rows extend to the last column");
}
