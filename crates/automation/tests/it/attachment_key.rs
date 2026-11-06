//! `attachment_add` reports the name-tree key of the file it added, even when a file attached to
//! a page as a comment already has the same name.

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::{AttachIcon, Edit, NewAnnotation, Session, Shape, Style};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

#[test]
fn attachment_add_reports_its_own_key_when_a_page_comment_has_the_same_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::new();
    let bytes = session.create_blank(300.0, 400.0, 1).unwrap();
    let id = session.open("a.pdf", None, bytes, None).unwrap();
    let shape = Shape::Attachment { at: [20.0, 300.0], icon: AttachIcon::Paperclip, file: "notes.txt".into(), data: b"on the page".to_vec() };
    session
        .apply(id, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style: Style::default(), contents: String::new(), author: "Ada".into() }))
        .unwrap();
    std::fs::write(dir.path().join("a.pdf"), session.save_bytes(id).unwrap().as_slice()).unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"for the document").unwrap();

    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut auto, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let added = tool(&mut auto, "attachment_add", json!({ "doc": doc, "path": "notes.txt" }));
    assert_eq!(added["name"], "notes.txt", "{added}");
    let again = tool(&mut auto, "attachment_add", json!({ "doc": doc, "path": "notes.txt" }));
    assert_eq!(again["name"], "notes.txt (2)", "{again}");
}
