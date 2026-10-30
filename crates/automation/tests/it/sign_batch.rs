use std::path::Path;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn pdf(text: &str) -> Vec<u8> {
    Session::new().create_from_text("Doc", text).unwrap().as_ref().clone()
}

fn call(auto: &mut Automation, args: Value) -> Result<Value, String> {
    match auto.call("sign_batch", &args).map_err(|e| e.to_string())?.remove(0) {
        Content::Json(value) => Ok(value),
        _ => panic!("expected a report"),
    }
}

/// A folder with a test digital ID (a low key derivation count keeps opening it fast) and some PDFs.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let id = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sign/tests/data/ec-p256.p12");
    std::fs::copy(id, dir.path().join("id.p12")).unwrap();
    std::fs::create_dir_all(dir.path().join("in/sub")).unwrap();
    std::fs::write(dir.path().join("in/a.pdf"), pdf("first")).unwrap();
    std::fs::write(dir.path().join("in/sub/b.PDF"), pdf("second")).unwrap();
    std::fs::write(dir.path().join("in/notes.txt"), "not a pdf").unwrap();
    dir
}

fn args(extra: Value) -> Value {
    let mut base = json!({"paths": ["in"], "out_dir": "out", "id": "id.p12", "password": "test", "recursive": true});
    base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    base
}

/// The signatures of a written file, validated by opening it in a fresh session.
fn signatures(auto: &mut Automation, path: &Path) -> Vec<Value> {
    let opened = auto.call("doc_open", &json!({"path": path.to_str().unwrap()})).unwrap().remove(0);
    let Content::Json(opened) = opened else { panic!("expected json") };
    let doc = opened["doc"].clone();
    let Content::Json(list) = auto.call("sign_list", &json!({"doc": doc})).unwrap().remove(0) else { panic!("expected json") };
    auto.call("doc_close", &json!({"doc": doc, "discard_changes": true})).unwrap();
    list["signatures"].as_array().unwrap().clone()
}

#[test]
fn signs_every_pdf_keeping_paths_and_leaving_sources_alone() {
    let dir = workspace();
    let before = std::fs::read(dir.path().join("in/a.pdf")).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, args(json!({"reason": "Aprovado", "location": "Brasília"}))).unwrap();
    assert_eq!((report["completed"].clone(), report["failed"].clone(), report["skipped"].clone()), (json!(2), json!(0), json!(1)), "{report}");
    assert_eq!(std::fs::read(dir.path().join("in/a.pdf")).unwrap(), before, "the source must not change");
    assert!(auto.session().docs().is_empty(), "a batch must not touch the caller's tabs");
    for name in ["a.pdf", "sub/b.pdf"] {
        let sigs = signatures(&mut auto, &dir.path().join("out").join(name));
        assert_eq!(sigs.len(), 1, "{name}: {sigs:?}");
        assert_eq!(sigs[0]["signed"], true);
        assert_eq!(sigs[0]["reason"], "Aprovado");
        assert_eq!(sigs[0]["location"], "Brasília");
        assert_eq!(sigs[0]["sub_filter"], "ETSI.CAdES.detached");
        assert_ne!(sigs[0]["status"], "invalid", "{name}: {sigs:?}");
    }
    assert!(!dir.path().join("out/notes.pdf").exists());
}

#[test]
fn never_replaces_an_existing_output_and_a_rerun_signs_the_sources_again() {
    let dir = workspace();
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/a.pdf"), b"precious").unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, args(json!({"paths": ["in/a.pdf"]}))).unwrap();
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(std::fs::read(dir.path().join("out/a.pdf")).unwrap(), b"precious");
    assert_eq!(signatures(&mut auto, &dir.path().join("out/a (2).pdf")).len(), 1);
}

#[test]
fn a_broken_file_fails_alone_unless_stop_on_error_is_set() {
    let dir = workspace();
    std::fs::write(dir.path().join("in/0-broken.pdf"), b"not really a pdf").unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, args(json!({}))).unwrap();
    assert_eq!((report["completed"].clone(), report["failed"].clone()), (json!(2), json!(1)), "{report}");
    assert!(!dir.path().join("out/0-broken.pdf").exists());

    let stopped = call(&mut auto, args(json!({"out_dir": "out2", "stop_on_error": true}))).unwrap();
    assert_eq!((stopped["completed"].clone(), stopped["failed"].clone(), stopped["stopped"].clone()), (json!(0), json!(1), json!(true)), "{stopped}");
    assert!(!dir.path().join("out2/a.pdf").exists(), "files after the error are not signed");
}

#[test]
fn a_wrong_password_signs_nothing() {
    let dir = workspace();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let error = call(&mut auto, args(json!({"password": "wrong"}))).unwrap_err();
    assert!(error.contains("password"), "{error}");
    assert!(!dir.path().join("out").exists());
}

#[test]
fn a_rectangle_makes_visible_signatures_and_a_bad_one_is_refused_per_file() {
    let dir = workspace();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, args(json!({"rect": [50, 50, 250, 110], "page": 9}))).unwrap();
    assert_eq!(report["completed"], 2, "a page past the end falls back to the last page: {report}");
    for name in ["a.pdf", "sub/b.pdf"] {
        let sigs = signatures(&mut auto, &dir.path().join("out").join(name));
        assert_eq!(sigs[0]["visible"], true, "{sigs:?}");
    }
    let tiny = call(&mut auto, args(json!({"out_dir": "out2", "rect": [10, 10, 11, 11]}))).unwrap();
    assert_eq!((tiny["completed"].clone(), tiny["failed"].clone()), (json!(0), json!(2)), "{tiny}");
}

#[test]
fn certify_levels_and_input_checks() {
    let dir = workspace();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let report = call(&mut auto, args(json!({"certify": "form_fill", "paths": ["in/a.pdf"]}))).unwrap();
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(signatures(&mut auto, &dir.path().join("out/a.pdf"))[0]["certify"], 2);
    assert!(call(&mut auto, args(json!({"certify": "everything"}))).is_err());
    assert!(call(&mut auto, args(json!({"paths": []}))).is_err());
    assert!(call(&mut auto, args(json!({"paths": ["../outside"]}))).is_err());
    assert!(call(&mut auto, args(json!({"out_dir": "../outside"}))).is_err());
    assert!(call(&mut auto, args(json!({"id": "missing.p12"}))).is_err());
}
