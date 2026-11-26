//! Formulas › Recently Used: the functions most recently entered in a formula, from any command
//! that enters one, kept in the preferences rather than read back from the journal.

use gridcraft_engine::Session;
use serde_json::{Value as Json, json};

fn recent(s: &mut Session) -> Vec<String> {
    let list = s.execute("formulas.recentlyUsed", json!({})).unwrap();
    list.as_array().unwrap().iter().filter_map(Json::as_str).map(String::from).collect()
}

#[test]
fn formulas_entered_by_any_command_lead_the_list() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=NPV(0.1,1,2)"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=IRR({-10,6,6})+ROUND(1,0)"})).unwrap();
    s.execute("range.setValues", json!({"range": "B1", "values": [["=FV(0.1,2,-1)", 4, "text"]]})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "PMT is just text"})).unwrap();
    assert_eq!(&recent(&mut s)[..4], ["FV", "IRR", "ROUND", "NPV"]);
    // A command that fails enters nothing.
    assert!(s.execute("cell.set", json!({"cell": "not a cell", "input": "=DB(1,2,3,4)"})).is_err());
    assert_ne!(recent(&mut s)[0], "DB");
}

#[test]
fn formulas_typed_with_a_plus_or_minus_count() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "+ROUND(1.5,0)"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "-SLN(100,10,5)"})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "-5"})).unwrap();
    assert_eq!(&recent(&mut s)[..2], ["SLN", "ROUND"]);
}

#[test]
fn the_list_does_not_depend_on_the_journal() {
    // The journal drops its oldest entries past its caps (and is public); the list stays.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=NPV(0.1,1,2)"})).unwrap();
    s.journal.clear();
    assert_eq!(recent(&mut s)[0], "NPV");
    s.execute("cell.set", json!({"cell": "A2", "input": "=SLN(100,10,5)"})).unwrap();
    assert_eq!(&recent(&mut s)[..2], ["SLN", "NPV"]);
}

#[test]
fn the_list_is_kept_with_the_preferences() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SYD(100,10,5,1)"})).unwrap();
    let saved = serde_json::to_value(&s.prefs).unwrap();
    let mut next = Session::new();
    next.new_workbook();
    next.prefs = serde_json::from_value(saved).unwrap();
    assert_eq!(recent(&mut next)[0], "SYD");
}
