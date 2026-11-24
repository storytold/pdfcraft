use gridcraft_engine::{EngineError, Session};
use serde_json::json;

fn new_session() -> Session {
    let mut session = Session::new();
    session.new_workbook();
    session
}

#[test]
fn insert_with_an_out_of_range_count_is_a_normal_error() {
    for (id, key, at) in [("home.insertRows", "rows", "3:3"), ("home.insertColumns", "cols", "C:C")] {
        let mut session = new_session();
        let err = session.execute(id, json!({ key: at, "count": u32::MAX })).unwrap_err();
        assert!(matches!(err, EngineError::BadParams { .. }), "{id}: {err:?}");
    }
}

#[test]
fn insert_after_a_formatted_line_with_an_out_of_range_count_is_a_normal_error() {
    // A custom size on the preceding line and no cells: the used-range guard has nothing to check.
    let mut session = new_session();
    session.execute("home.rowHeight", json!({"rows": "2:2", "height": 30})).unwrap();
    let err = session.execute("home.insertRows", json!({"rows": "3:3", "count": u32::MAX})).unwrap_err();
    assert!(matches!(err, EngineError::BadParams { .. }), "{err:?}");

    let mut session = new_session();
    session.execute("home.columnWidth", json!({"cols": "B:B", "width": 100})).unwrap();
    let err = session.execute("home.insertColumns", json!({"cols": "C:C", "count": u32::MAX})).unwrap_err();
    assert!(matches!(err, EngineError::BadParams { .. }), "{err:?}");
}

#[test]
fn delete_with_an_out_of_range_count_does_not_panic() {
    for (id, key, at) in [("home.deleteRows", "rows", "3:3"), ("home.deleteColumns", "cols", "C:C")] {
        let mut session = new_session();
        let res = session.execute(id, json!({ key: at, "count": u32::MAX }));
        assert!(!matches!(res, Err(EngineError::Internal(..))), "{id}: {res:?}");
    }
}

#[test]
fn insert_and_delete_with_a_small_count_still_work() {
    let mut session = new_session();
    session.execute("home.insertRows", json!({"rows": "1:1", "count": 3})).unwrap();
    session.execute("home.deleteRows", json!({"rows": "1:1", "count": 3})).unwrap();
}
