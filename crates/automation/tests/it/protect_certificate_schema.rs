//! `doc_protect_certificates` says what each partial restriction defaults to.

use pdfcraft_automation::tools;

#[test]
fn each_restriction_states_its_default() {
    let def = tools().into_iter().find(|t| t.name == "doc_protect_certificates").expect("the tool exists");
    for (key, default) in [("printing", "Default high"), ("changes", "Default none"), ("copy", "default false"), ("accessibility", "default true")] {
        let desc = def.input_schema["properties"][key]["description"].as_str().unwrap_or_else(|| panic!("{key} has no description"));
        assert!(desc.contains(default), "{key}: {desc}");
    }
}
