#![cfg(feature = "mcp")]

mod connected_support;

use std::sync::Arc;

use pdfcraft_automation::window::call_for_window;
use pdfcraft_engine::{Edit, NewAnnotation, Session, Shape, Style};
use serde_json::json;

#[test]
fn an_unlisted_connected_paragraph_is_refused_instead_of_treated_as_a_position() {
    let (mut session, id) = connected_support::session();
    let before = session.get(id).unwrap().bytes.clone();
    let error = call_for_window(&mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":2,"text":"wrong paragraph"})).unwrap_err();
    assert!(error.contains("text_paragraphs"), "{error}");
    assert_eq!(session.get(id).unwrap().bytes, before);
}

#[test]
fn a_pathless_connected_save_checks_the_original_destination_before_preparation() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("allowed");
    std::fs::create_dir(&root).unwrap();
    let path = folder.path().join("outside.pdf");
    let original = connected_support::pdf();
    std::fs::write(&path, &original).unwrap();
    let mut session = Session::new();
    let id = session.open("outside.pdf", Some(path.to_str().unwrap().into()), Arc::new(original.clone()), None).unwrap();
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let before = session.get(id).unwrap().edit_generation();
    let error = call_for_window(&mut session, Some(&root), "doc_save", &json!({"doc":id.0,"flatten_fill_sign":true})).unwrap_err();
    assert!(error.contains("outside the allowed directory"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(session.get(id).unwrap().edit_generation(), before, "refusal precedes any save-preparation edit");
    assert!(session.get(id).unwrap().dirty);
    call_for_window(&mut session, Some(&root), "doc_save", &json!({"doc":id.0,"path":"inside.pdf"})).unwrap();
    assert!(root.join("inside.pdf").is_file());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn a_pathless_save_of_a_relative_window_path_remains_incremental() {
    let folder = tempfile::tempdir_in(".").unwrap();
    let absolute = std::path::absolute(folder.path().join("relative.pdf")).unwrap();
    let current = std::env::current_dir().unwrap();
    let relative = absolute.strip_prefix(&current).unwrap();
    let original = connected_support::pdf();
    std::fs::write(&absolute, &original).unwrap();
    let mut session = Session::new();
    let id = session.open("relative.pdf", Some(relative.to_str().unwrap().into()), Arc::new(original.clone()), None).unwrap();
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let saved = call_for_window(&mut session, None, "doc_save", &json!({"doc":id.0})).unwrap();
    assert_eq!(saved["content"][0]["value"]["incremental"], true);
    assert!(std::fs::read(&absolute).unwrap().starts_with(&original));
}

#[test]
fn connected_redo_cannot_restore_the_persons_undone_signature() {
    let (mut session, id) = connected_support::session();
    let shape = Shape::Signature { strokes: vec![vec![[10.0, 40.0], [40.0, 70.0], [70.0, 40.0]]] };
    let style = Style::default_for(&shape);
    session
        .apply(id, Edit::AddAnnotation(NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Synthetic person".into() }))
        .unwrap();
    assert_eq!(session.get(id).unwrap().info.annotations.len(), 1);
    session.undo(id).unwrap();
    assert!(session.get(id).unwrap().info.annotations.is_empty());
    for (name, arguments) in [
        ("edit_redo", json!({"doc":id.0})),
        ("command_run", json!({"id":"edit.redo","params":{"doc":id.0}})),
        ("command_batch", json!({"steps":[{"id":"edit.redo","params":{"doc":id.0}}]})),
    ] {
        let error = call_for_window(&mut session, None, name, &arguments).unwrap_err();
        assert!(error.contains("HUMAN_HISTORY_REQUIRED"), "{name}: {error}");
        assert!(session.get(id).unwrap().info.annotations.is_empty());
    }
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().info.annotations.len(), 1, "the person's native Redo still works");
}
