//! Exercise the complete detection/editing tool with a tiny, contributor-original ONNX
//! model generated at runtime. Model accuracy is evaluated separately with real FFDetr.
use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn setup(tag: &str) -> (Automation, PathBuf, u64) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ffdetr-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut automation = Automation::new().with_root(&dir).unwrap();
    let id = call(&mut automation, "doc_create", json!({"from":"blank","width":400,"height":600}))["doc"].as_u64().unwrap();
    (automation, dir, id)
}

fn write_model(path: &Path) {
    std::fs::write(path, pdfcraft_form_detect::test_model::bytes()).unwrap();
}

#[test]
fn native_model_preview_add_fill_save_reopen_and_undo_through_tools() {
    let (mut a, dir, id) = setup("roundtrip");
    write_model(&dir.join("fields.onnx"));
    let preview = call(&mut a, "form_detect_fields", json!({"doc":id,"model":"fields.onnx","add":false}));
    assert_eq!(preview["backend"], "ffdetr");
    assert_eq!(preview["fields"].as_array().unwrap().len(), 3);
    assert_eq!(call(&mut a, "form_fields", json!({"doc":id}))["fields"].as_array().unwrap().len(), 0);
    let added = call(&mut a, "form_detect_fields", json!({"doc":id,"model":"fields.onnx"}));
    assert_eq!(preview, added, "adding returns the same proposed fields");
    assert_eq!(call(&mut a, "form_fields", json!({"doc":id}))["fields"].as_array().unwrap().len(), 3);
    call(&mut a, "edit_undo", json!({"doc":id}));
    assert_eq!(call(&mut a, "form_fields", json!({"doc":id}))["fields"].as_array().unwrap().len(), 0);
    call(&mut a, "edit_redo", json!({"doc":id}));
    let again = call(&mut a, "form_detect_fields", json!({"doc":id,"model":"fields.onnx","add":false}));
    assert!(again["fields"].as_array().unwrap().is_empty());
    call(&mut a, "form_fill", json!({"doc":id,"values":{"Text1":"Ada","Check Box1":true}}));
    call(&mut a, "doc_save", json!({"doc":id,"path":"saved.pdf"}));
    let reopened = call(&mut a, "doc_open", json!({"path":"saved.pdf"}))["doc"].as_u64().unwrap();
    let fields = call(&mut a, "form_fields", json!({"doc":reopened}));
    assert_eq!(fields["fields"].as_array().unwrap().len(), 3);
    let text = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "Text1").unwrap();
    assert_eq!(text["value"], json!("Ada"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn model_paths_are_confined_before_loading_even_in_preview() {
    let (mut a, dir, id) = setup("confinement");
    let outside = dir.with_extension("outside.onnx");
    write_model(&outside);
    let error = a.call("form_detect_fields", &json!({"doc":id,"model":outside,"add":false})).unwrap_err().to_string();
    assert!(error.contains("outside"), "{error}");
    assert_eq!(call(&mut a, "form_fields", json!({"doc":id}))["fields"].as_array().unwrap().len(), 0);
    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_models_and_options_leave_the_document_unchanged() {
    let (mut a, dir, id) = setup("errors");
    std::fs::write(dir.join("bad.onnx"), b"not an ONNX model").unwrap();
    write_model(&dir.join("fields.onnx"));
    for args in [
        json!({"doc":id,"model":"bad.onnx"}),
        json!({"doc":id,"confidence":0.4}),
        json!({"doc":id,"model":"bad.onnx","confidence":-1}),
        json!({"doc":id,"model":"bad.onnx","pages":[2]}),
        json!({"doc":id,"model":"fields.onnx","confidence":1.00000001}),
        json!({"doc":id,"model":"fields.onnx","confidence":-0.00000001}),
        json!({"doc":id,"model":"fields.onnx","add":"yes"}),
    ] {
        assert!(a.call("form_detect_fields", &args).is_err());
        assert_eq!(call(&mut a, "form_fields", json!({"doc":id}))["fields"].as_array().unwrap().len(), 0);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
