//! Sheet navigation must never activate hidden sheets when it reaches a workbook boundary.

use std::sync::Arc;

use gridcraft_engine::{Session, model::Visibility};
use serde_json::json;

fn workbook(hidden: &[usize], active: usize) -> Session {
    let mut session = Session::new();
    session.new_workbook();
    for name in ["Second", "Third", "Fourth"] {
        session.execute("home.insertSheet", json!({"name": name})).unwrap();
    }
    for sheet in hidden {
        session.execute("sheet.hide", json!({"sheet": sheet})).unwrap();
    }
    session.execute("sheet.activate", json!({"sheet": active})).unwrap();
    session.execute("selection.set", json!({"cell": "C7"})).unwrap();
    session
}

#[test]
fn next_stays_on_the_last_visible_sheet() {
    let mut session = workbook(&[2, 3], 1);
    for _ in 0..2 {
        session.execute("sheet.next", json!({})).unwrap();
        let doc = session.active().unwrap();
        assert_eq!(doc.wb.active_sheet(), 1);
        assert_eq!(doc.selection.active.a1(), "C7");
    }
}

#[test]
fn previous_stays_on_the_first_visible_sheet() {
    let mut session = workbook(&[0, 1], 2);
    for _ in 0..2 {
        session.execute("sheet.previous", json!({})).unwrap();
        let doc = session.active().unwrap();
        assert_eq!(doc.wb.active_sheet(), 2);
        assert_eq!(doc.selection.active.a1(), "C7");
    }
}

#[test]
fn navigation_skips_hidden_sheets_in_both_directions() {
    let mut session = workbook(&[1, 2], 0);
    session.execute("sheet.next", json!({})).unwrap();
    assert_eq!(session.active().unwrap().wb.active_sheet(), 3);
    session.execute("sheet.previous", json!({})).unwrap();
    let doc = session.active().unwrap();
    assert_eq!(doc.wb.active_sheet(), 0);
    assert_eq!(doc.selection.active.a1(), "C7");
}

#[test]
fn navigation_with_one_visible_sheet_is_a_noop() {
    let mut session = workbook(&[0, 2, 3], 1);
    for command in ["sheet.previous", "sheet.next"] {
        session.execute(command, json!({})).unwrap();
        let doc = session.active().unwrap();
        assert_eq!(doc.wb.active_sheet(), 1);
        assert_eq!(doc.selection.active.a1(), "C7");
    }
}

#[test]
fn very_hidden_sheets_are_skipped_at_interior_and_boundary_positions() {
    let mut session = workbook(&[1, 3], 0);
    // VeryHidden is an imported XLSX state; sheet.hide only creates Hidden sheets.
    let wb = Arc::make_mut(&mut session.active_mut().unwrap().wb);
    for index in [1, 3] {
        Arc::make_mut(&mut wb.sheets[index]).visibility = Visibility::VeryHidden;
    }
    for (command, expected) in [("sheet.next", 2), ("sheet.next", 2), ("sheet.previous", 0)] {
        session.execute(command, json!({})).unwrap();
        assert_eq!(session.active().unwrap().wb.active_sheet(), expected);
    }
}
