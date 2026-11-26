use gridcraft_engine::Session;
use serde_json::json;

const NOT_OBJECTS: [&str; 4] = ["\"[1,2]\"", "\"7\"", "\"true\"", "\"\"\"hi\"\"\""];

fn with_name(name: &str, refers_to: &str) -> Session {
    let mut s = Session::new();
    s.new_workbook();
    // `force`: the name commands refuse GridCraft's own data names otherwise.
    s.execute("formulas.defineName", json!({"name": name, "refersTo": refers_to, "force": true})).unwrap();
    s
}

#[test]
fn saving_a_view_works_when_stored_list_is_not_an_object() {
    for text in NOT_OBJECTS {
        let mut s = with_name("_sc_custom_views", text);
        let out = s.execute("view.customViews", json!({"save": "v"})).unwrap();
        assert!(out.get("v").is_some(), "{text}: {out}");
    }
}

#[test]
fn saving_a_view_keeps_the_keys_of_a_stored_object() {
    let mut s = with_name("_sc_custom_views", "\"{\"\"a\"\":1}\"");
    let out = s.execute("view.customViews", json!({"save": "v"})).unwrap();
    assert!(out.get("a").is_some() && out.get("v").is_some(), "{out}");
}

#[test]
fn creating_a_script_works_when_stored_list_is_not_an_object() {
    for text in NOT_OBJECTS {
        let mut s = with_name("_sc_scripts", text);
        s.execute("automate.newScript", json!({"name": "s1", "commands": []})).unwrap();
        let list = s.execute("automate.allScripts", json!({})).unwrap();
        assert!(list.to_string().contains("s1"), "{text}: {list}");
    }
}

#[test]
fn stopping_a_recording_works_when_stored_list_is_not_an_object() {
    for text in NOT_OBJECTS {
        let mut s = with_name("_sc_scripts", text);
        s.execute("automate.recordActions", json!({})).unwrap();
        s.execute("automate.recordActions", json!({"stop": true, "name": "s1"})).unwrap();
        let list = s.execute("automate.allScripts", json!({})).unwrap();
        assert!(list.to_string().contains("s1"), "{text}: {list}");
    }
}
