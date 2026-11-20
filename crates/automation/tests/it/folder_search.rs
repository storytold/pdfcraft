use std::path::Path;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn pdf(text: &str) -> Vec<u8> {
    Session::new().create_from_text("Title", text).unwrap().as_ref().clone()
}

fn call(auto: &mut Automation, args: Value) -> Value {
    match auto.call("folder_search", &args).unwrap().remove(0) {
        Content::Json(value) => value,
        _ => panic!("expected a report"),
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn a_search_reports_matches_by_file_with_pages_from_one_and_opens_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.pdf"), &pdf("the quick brown fox"));
    write(&dir.path().join("sub/b.pdf"), &pdf("quick again"));
    write(&dir.path().join("c.pdf"), &pdf("nothing"));
    write(&dir.path().join("broken.pdf"), b"not a pdf");
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, json!({ "path": ".", "query": "quick" }));
    assert_eq!(report["found"], 4);
    assert_eq!(report["searched"], 3);
    assert_eq!(report["files_with_matches"], 2);
    assert_eq!(report["matches"], 2);
    assert_eq!(report["stopped"], Value::Null);
    assert_eq!(report["folder_cut_short"], false);
    let first = &report["files"][0];
    assert!(first["path"].as_str().unwrap().ends_with("a.pdf"), "{first}");
    assert_eq!(first["matches"][0]["page"], 1);
    assert_eq!(first["matches"][0]["text"], "quick");
    assert_eq!(first["matches"][0]["after"], "brown fox");
    let skipped = report["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0]["reason"], "damaged");
    assert!(skipped[0]["path"].as_str().unwrap().ends_with("broken.pdf"));
    assert!(skipped[0]["detail"].as_str().unwrap().contains("not a readable PDF"));
    let Content::Json(listed) = auto.call("doc_list", &json!({})).unwrap().remove(0) else { panic!("expected a list") };
    assert_eq!(listed["documents"], json!([]), "the files were read, not opened");
}

#[test]
fn the_options_reach_the_search() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.pdf"), &pdf("Cat category"));
    write(&dir.path().join("sub/b.pdf"), &pdf("cat"));
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let count = |auto: &mut Automation, extra: Value| {
        let mut args = json!({ "path": ".", "query": "cat" });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        call(auto, args)["matches"].as_u64().unwrap()
    };
    assert_eq!(count(&mut auto, json!({})), 3);
    assert_eq!(count(&mut auto, json!({ "subfolders": false })), 2);
    assert_eq!(count(&mut auto, json!({ "case_sensitive": true })), 2);
    assert_eq!(count(&mut auto, json!({ "whole_words": true })), 2);
}

#[test]
fn the_limit_ends_the_search_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("a.pdf"), &pdf("needle"));
    write(&dir.path().join("b.pdf"), &pdf("needle"));
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, json!({ "path": ".", "query": "needle", "limit": 1 }));
    assert_eq!(report["matches"], 1);
    assert_eq!(report["stopped"], "match_limit");
}

#[test]
fn bad_arguments_are_refused_with_what_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    for limit in [0, -1, 10_001] {
        let error = auto.call("folder_search", &json!({ "path": ".", "query": "x", "limit": limit })).unwrap_err().to_string();
        assert!(error.contains("limit"), "{limit}: {error}");
    }
    let error = auto.call("folder_search", &json!({ "path": ".", "query": "   " })).unwrap_err().to_string();
    assert!(error.contains("empty"), "{error}");
    write(&dir.path().join("a.pdf"), b"x");
    let error = auto.call("folder_search", &json!({ "path": "a.pdf", "query": "x" })).unwrap_err().to_string();
    assert!(error.contains("not a folder"), "{error}");
}

#[test]
fn a_search_stays_inside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    write(&root.join("inside.pdf"), &pdf("needle"));
    let outside = dir.path().join("outside");
    write(&outside.join("secret.pdf"), &pdf("needle"));
    let mut auto = Automation::new().with_root(&root).unwrap();
    for path in ["..", "../outside", outside.to_str().unwrap()] {
        let error = auto.call("folder_search", &json!({ "path": path, "query": "needle" })).unwrap_err().to_string();
        assert!(error.contains("outside the allowed directory"), "{path}: {error}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.pdf"), root.join("link.pdf")).unwrap();
        let report = call(&mut auto, json!({ "path": ".", "query": "needle" }));
        assert_eq!(report["matches"], 1, "only the file inside: {report}");
        let reasons: Vec<&str> = report["skipped"].as_array().unwrap().iter().map(|s| s["reason"].as_str().unwrap()).collect();
        assert_eq!(reasons, ["link_or_special_file", "link_or_special_file"]);
    }
}
