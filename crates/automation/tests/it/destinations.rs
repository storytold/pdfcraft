//! The named destination tools (`destination_list`, `_add`, `_set`, `_rename`, `_delete`): the
//! Destinations panel without the GUI.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

/// Call `name` and return its JSON result.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap_or_else(|e| panic!("{name}: {e}")).remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Call `name`, which must be refused, and return what it said.
fn refused(a: &mut Automation, name: &str, args: Value) -> String {
    a.call(name, &args).map(|_| ()).expect_err(name).to_string()
}

/// The destinations `destination_list` gives for `doc`.
fn list(a: &mut Automation, doc: u64) -> Value {
    tool(a, "destination_list", json!({ "doc": doc }))["destinations"].clone()
}

/// A two-page 300 × 400 document saved as `dir/a.pdf`, opened; its id.
fn open_blank(a: &mut Automation, dir: &std::path::Path) -> u64 {
    let session = Session::new();
    std::fs::write(dir.join("a.pdf"), session.create_blank(300.0, 400.0, 2).unwrap().as_slice()).unwrap();
    tool(a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap()
}

#[test]
fn destinations_are_added_moved_renamed_deleted_and_saved_through_tools() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = open_blank(&mut a, dir.path());
    assert_eq!(list(&mut a, doc), json!([]));

    tool(&mut a, "destination_add", json!({ "doc": doc, "name": "Intro", "page": 2, "top": 100, "left": 50 }));
    tool(&mut a, "destination_add", json!({ "doc": doc, "name": "Appendix", "page": 1 }));
    // Sorted by name; a destination that says no place goes to the page as the reader has it.
    assert_eq!(
        list(&mut a, doc),
        json!([
            { "name": "Appendix", "page": 1, "destination": { "fit": "XYZ", "left": null, "top": null, "zoom": null } },
            { "name": "Intro", "page": 2, "destination": { "fit": "XYZ", "left": 50.0, "top": 100.0, "zoom": null } },
        ])
    );

    tool(&mut a, "destination_set", json!({ "doc": doc, "name": "Intro", "page": 1, "top": 20 }));
    tool(&mut a, "destination_rename", json!({ "doc": doc, "name": "Intro", "new_name": "Einführung" }));
    let moved = json!({ "name": "Einführung", "page": 1, "destination": { "fit": "XYZ", "left": null, "top": 20.0, "zoom": null } });
    assert_eq!(list(&mut a, doc)[1], moved);

    tool(&mut a, "doc_save", json!({ "doc": doc, "path": "b.pdf" }));
    let again = tool(&mut a, "doc_open", json!({ "path": "b.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(list(&mut a, again)[1], moved);

    tool(&mut a, "destination_delete", json!({ "doc": doc, "name": "Appendix" }));
    assert_eq!(list(&mut a, doc), json!([moved]));
    tool(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(list(&mut a, doc).as_array().map(Vec::len), Some(2));
}

#[test]
fn destination_tools_refuse_what_they_cannot_do_and_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = open_blank(&mut a, dir.path());
    tool(&mut a, "destination_add", json!({ "doc": doc, "name": "Intro", "page": 1 }));
    tool(&mut a, "destination_add", json!({ "doc": doc, "name": "Summary", "page": 2 }));
    let before = list(&mut a, doc);

    assert!(
        refused(&mut a, "destination_add", json!({ "doc": doc, "name": "Intro", "page": 2 })).contains("a destination named Intro already exists")
    );
    assert!(refused(&mut a, "destination_add", json!({ "doc": doc, "name": "Later", "page": 3 })).contains("page 3 is out of range"));
    assert!(refused(&mut a, "destination_add", json!({ "doc": doc, "name": "   ", "page": 1 })).contains("a destination needs a name"));
    assert!(refused(&mut a, "destination_add", json!({ "doc": doc, "name": "Far", "page": 1, "top": 1e9 })).contains("top"));
    assert!(
        refused(&mut a, "destination_set", json!({ "doc": doc, "name": "Nothing", "page": 1 })).contains("there is no destination named Nothing")
    );
    assert!(
        refused(&mut a, "destination_rename", json!({ "doc": doc, "name": "Intro", "new_name": "Summary" }))
            .contains("a destination named Summary already exists")
    );
    assert!(refused(&mut a, "destination_delete", json!({ "doc": doc, "name": "Nothing" })).contains("there is no destination named Nothing"));
    // Over the length limit, and missing arguments, are refused by the schema checks.
    let long = "x".repeat(pdfcraft_organize::MAX_DESTINATION_NAME + 1);
    assert!(a.call("destination_add", &json!({ "doc": doc, "name": long, "page": 1 })).is_err());
    assert!(a.call("destination_rename", &json!({ "doc": doc, "name": "Intro" })).is_err());
    assert_eq!(list(&mut a, doc), before);
}
