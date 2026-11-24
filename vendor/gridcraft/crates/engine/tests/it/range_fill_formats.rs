//! Ctrl+Enter (`range.fill`) gives every cell what typing the input there gives it: a General
//! cell gets the input's automatic number format, a formatted cell keeps its own.

use gridcraft_engine::Session;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn get(s: &mut Session, a: &str) -> Value {
    s.execute("cell.get", json!({"cell": a})).unwrap()
}

fn fmt_of(s: &Session, a: &str) -> String {
    let wb = &s.doc().unwrap().wb;
    let sh = wb.active().unwrap();
    wb.styles.get(sh.style_id(gridcraft_engine::core::CellRef::parse(a).unwrap())).num_fmt.as_str().to_string()
}

/// Selects `range` with A1 active and fills it with `input`, as Ctrl+Enter does.
fn fill(s: &mut Session, range: &str, input: &str) {
    s.execute("selection.set", json!({"range": range, "active": "A1"})).unwrap();
    s.execute("range.fill", json!({"input": input})).unwrap();
}

#[test]
fn filled_cells_that_already_hold_a_value_get_the_automatic_format() {
    for input in ["5%", "$5", "3/4/2026", "1:30 PM", "1.5E+3"] {
        let mut s = session();
        // What typing the input into a General cell gives.
        s.execute("cell.set", json!({"cell": "C1", "input": input})).unwrap();
        let typed = get(&mut s, "C1");
        // A1 blank, A2 holding a number: both are General.
        s.execute("cell.set", json!({"cell": "A2", "input": "1"})).unwrap();
        fill(&mut s, "A1:A2", input);
        for a in ["A1", "A2"] {
            let filled = get(&mut s, a);
            assert_eq!(filled["text"], typed["text"], "{input} in {a}");
            assert_eq!(filled["value"], typed["value"], "{input} in {a}");
            assert_eq!(fmt_of(&s, a), fmt_of(&s, "C1"), "{input} in {a}");
        }
    }
}

#[test]
fn filled_cells_with_their_own_format_keep_it() {
    let mut s = session();
    s.execute("cell.set", json!({"cell": "A2", "input": "1"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A2", "style": {"num_fmt": "0.000"}})).unwrap();
    // Column B is formatted but has no cells yet; row 3 is Text.
    s.execute("home.formatCells", json!({"range": "B:B", "style": {"num_fmt": "0.00"}})).unwrap();
    s.execute("home.formatCells", json!({"range": "3:3", "style": {"num_fmt": "@"}})).unwrap();
    fill(&mut s, "A1:B3", "5%");
    assert_eq!(fmt_of(&s, "A1"), "0%");
    assert_eq!(get(&mut s, "A1")["text"], "5%");
    assert_eq!(fmt_of(&s, "A2"), "0.000");
    assert_eq!(get(&mut s, "A2")["text"], "0.050");
    assert_eq!(fmt_of(&s, "B1"), "0.00");
    assert_eq!(get(&mut s, "B1")["text"], "0.05");
    // A Text cell keeps what was typed, as text.
    for a in ["A3", "B3"] {
        assert_eq!(get(&mut s, a)["type"], "text", "{a}");
        assert_eq!(get(&mut s, a)["text"], "5%", "{a}");
    }
}

#[test]
fn filled_formulas_still_shift_and_get_their_date_format() {
    let mut s = session();
    s.execute("cell.set", json!({"cell": "A2", "input": "1"})).unwrap();
    fill(&mut s, "A1:A2", "=DATE(2026,3,ROW())");
    assert_eq!(get(&mut s, "A2")["formula"], "=DATE(2026,3,ROW())");
    for (a, text) in [("A1", "3/1/2026"), ("A2", "3/2/2026")] {
        assert_eq!(fmt_of(&s, a), "m/d/yyyy", "{a}");
        assert_eq!(get(&mut s, a)["text"], text, "{a}");
    }
    fill(&mut s, "A1:A2", "=C1*2");
    assert_eq!(get(&mut s, "A2")["formula"], "=C2*2");
}

#[test]
fn a_fill_undoes_in_one_step() {
    let mut s = session();
    s.execute("cell.set", json!({"cell": "A2", "input": "1"})).unwrap();
    fill(&mut s, "A1:A2", "5%");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(get(&mut s, "A1")["type"], "empty");
    assert_eq!(get(&mut s, "A2")["value"].as_f64(), Some(1.0));
    assert_eq!(fmt_of(&s, "A2"), "General");
}
