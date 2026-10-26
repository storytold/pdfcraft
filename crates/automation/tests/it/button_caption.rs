//! Editing a button's visible label through the CLI/MCP tool table (#2892).

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::{DocId, Edit, NewField, Session};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

#[test]
fn button_caption_can_be_changed_after_creation() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-button-caption-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::new();
    let bytes = session.create_blank(300.0, 400.0, 1).unwrap();
    let id = session.open("buttons.pdf", None, bytes, None).unwrap();
    session
        .apply(
            id,
            Edit::AddField {
                page: 0,
                rect: [40.0, 300.0, 200.0, 330.0],
                kind: NewField::Button { caption: String::new() },
                name: Some("clear_button".into()),
            },
        )
        .unwrap();
    std::fs::write(dir.join("buttons.pdf"), session.save_bytes(id).unwrap().as_slice()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path": "buttons.pdf"}))["doc"].as_u64().unwrap();
    let before = a.session().save_bytes(DocId(id)).unwrap();
    let (_, _, empty_pixels) = a.render_pixels(id, 1, 1.0).unwrap();
    tool(&mut a, "form_set_props", json!({"doc": id, "field": "clear_button", "caption": "Clear Form"}));
    let fields = tool(&mut a, "form_fields", json!({"doc": id}));
    assert_eq!(fields["fields"][0]["caption"], "Clear Form");
    assert_eq!(fields["fields"][0]["captions"], json!([{"widget": 1, "caption": "Clear Form"}]));
    assert_eq!(a.session().get(DocId(id)).unwrap().can_undo(), Some("Change field properties"));
    let (_, _, labelled_pixels) = a.render_pixels(id, 1, 1.0).unwrap();
    assert_ne!(empty_pixels, labelled_pixels, "the label must reach the actual page appearance");
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(a.session().save_bytes(DocId(id)).unwrap(), before);
    tool(&mut a, "edit_redo", json!({"doc": id}));
    tool(&mut a, "doc_save", json!({"doc": id, "path": "labelled.pdf"}));
    let saved = std::sync::Arc::new(std::fs::read(dir.join("labelled.pdf")).unwrap());
    let doc = pdfcraft_cos::Document::open(saved).unwrap();
    let root = doc.get(doc.root().unwrap());
    let form = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(form.as_dict().unwrap().get(b"Fields").unwrap());
    let button = doc.resolve(&fields.as_array().unwrap()[0]);
    let mk = doc.resolve(button.as_dict().unwrap().get(b"MK").unwrap());
    assert_eq!(doc.resolve(mk.as_dict().unwrap().get(b"CA").unwrap()).as_string().unwrap().to_text(), "Clear Form");
    assert_eq!(doc.resolve(button.as_dict().unwrap().get(b"T").unwrap()).as_string().unwrap().to_text(), "clear_button");
    let reopened = tool(&mut a, "doc_open", json!({"path": "labelled.pdf"}))["doc"].as_u64().unwrap();
    let fields = tool(&mut a, "form_fields", json!({"doc": reopened}));
    assert_eq!(fields["fields"][0]["caption"], "Clear Form");
    let (_, _, pixels) = a.render_pixels(reopened, 1, 1.0).unwrap();
    assert_eq!(pixels, labelled_pixels);
    let out = tool(&mut a, "form_set_props", json!({"doc": id, "field": "clear_button", "caption": "提交"}));
    assert!(out["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).any(|w| w.contains("U+63D0")));
    let fields = tool(&mut a, "form_fields", json!({"doc": id}));
    assert_eq!(fields["fields"][0]["caption"], "提交", "unsupported characters are retained and reported");
    tool(&mut a, "form_set_props", json!({"doc": id, "field": "clear_button", "caption": "Clear Form"}));
    for invalid in [
        json!({"doc": id, "field": "clear_button", "caption": "bad", "widget": 0}),
        json!({"doc": id, "field": "clear_button", "caption": "bad", "widget": -1}),
        json!({"doc": id, "field": "clear_button", "caption": "bad", "widget": 2}),
        json!({"doc": id, "field": "clear_button", "caption": "bad", "widget": i64::MAX}),
        json!({"doc": id, "field": "clear_button", "caption": 42}),
        json!({"doc": id, "field": "clear_button", "widget": 1, "tooltip": "bad"}),
        json!({"doc": id, "fields": ["clear_button"], "caption": "bad"}),
    ] {
        let before = a.session().save_bytes(DocId(id)).unwrap();
        assert!(a.call("form_set_props", &invalid).is_err(), "{invalid}");
        assert_eq!(a.session().save_bytes(DocId(id)).unwrap(), before, "{invalid}");
    }
    tool(&mut a, "form_set_props", json!({"doc": id, "field": "clear_button", "locked": true}));
    let before = a.session().save_bytes(DocId(id)).unwrap();
    assert!(a.call("form_set_props", &json!({"doc": id, "field": "clear_button", "caption": "bad"})).is_err());
    assert_eq!(a.session().save_bytes(DocId(id)).unwrap(), before);
    tool(&mut a, "form_set_props", json!({"doc": id, "field": "clear_button", "locked": false}));
    tool(&mut a, "form_set_props", json!({"doc": id, "field": "clear_button", "caption": ""}));
    let fields = tool(&mut a, "form_fields", json!({"doc": id}));
    assert_eq!(fields["fields"][0]["caption"], "");
    let (_, _, pixels) = a.render_pixels(id, 1, 1.0).unwrap();
    assert_eq!(pixels, empty_pixels);
    std::fs::remove_dir_all(dir).unwrap();
}
