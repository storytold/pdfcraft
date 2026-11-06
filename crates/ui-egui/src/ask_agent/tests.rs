use super::{Action, Status, handle_action, target};
use crate::PdfCraftApp;
use serde_json::{Value, json};

#[test]
fn selection_uses_each_word_and_real_quads_instead_of_a_bounding_hull() {
    let selection = json!({"doc":1,"pages":[1],"rects":[{"page":1,"rect":[0,0,100,100]}],"quads":[
        {"page":1,"quad":[0,10,20,10,0,0,20,0]},
        {"page":1,"quad":[80,100,100,100,80,90,100,90]}
    ]});
    let proof = |rects: Value| json!({"doc":1,"pages":[1],"rects":rects});
    assert!(target::contains(&selection, &proof(json!([{"page":1,"rect":[1,1,19,9]}]))));
    assert!(!target::contains(&selection, &proof(json!([{"page":1,"rect":[40,40,60,60]}]))));
    assert!(!target::contains(&selection, &proof(json!([{"page":1,"rect":[1,1,19,9]},{"page":1,"rect":[21,1,30,9]}]))));
    assert!(!target::contains(&selection, &json!({"doc":2,"pages":[1],"rects":[{"page":1,"rect":[1,1,19,9]}]})));
    assert!(!target::contains(&selection, &json!({"doc":1,"pages":[2],"rects":[{"page":2,"rect":[1,1,19,9]}]})));
}

#[test]
fn advancing_selection_uses_new_ink_and_drops_the_old_extent() {
    let old = json!({"doc":1,"pages":[1],"rects":[{"page":1,"rect":[0,0,100,100]}],"quads":[{"page":1,"quad":[0,100,100,100,0,0,100,0]}]});
    let edited = json!({"doc":1,"pages":[1],"textTarget":{"text":"after"},"rects":[{"page":1,"rect":[10,20,30,40]}]});
    let selection = target::after(&old, &edited);
    assert!(target::contains(&selection, &edited));
    assert!(!target::contains(&selection, &json!({"doc":1,"pages":[1],"rects":[{"page":1,"rect":[50,50,70,70]}]})));
    assert_eq!(selection["text"], "after");
}

fn pending() -> (PdfCraftApp, u64, u64) {
    let mut app = PdfCraftApp::new();
    let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
    app.open_bytes("synthetic.pdf", None, bytes.as_ref().clone()).unwrap();
    let id = app.ask_agent_command(&json!({"instruction":"rotate the page"})).unwrap()["id"].as_u64().unwrap();
    let request = app.ask_agent.hub.listen(7, Some("worker"), std::time::Duration::ZERO, || true).unwrap().unwrap();
    assert_eq!(request.id, id);
    let doc = app.views[0].id.0;
    app.tool_resolver = Some(Box::new(|session, _, _, args| {
        let doc = pdfcraft_engine::DocId(args["doc"].as_u64().unwrap());
        Ok(json!({"doc":doc.0,"revision":session.get(doc).unwrap().edit_generation(),"pages":[1],"rects":[]}))
    }));
    (app, id, doc)
}

fn partial_failure(app: &mut PdfCraftApp, id: u64, doc: u64) {
    app.tool_runner = Some(Box::new(|session, _, _, args, _| {
        let doc = pdfcraft_engine::DocId(args["doc"].as_u64().unwrap());
        for _ in 0..2 {
            session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        }
        Err("synthetic failure after two committed steps".into())
    }));
    assert!(
        app.ask_agent_tool(7, Some(id), "page_rotate", &json!({"doc":doc,"pages":[1],"degrees":90}), None).unwrap_err().contains("synthetic failure")
    );
    assert_eq!(app.ask_agent.open.as_ref().unwrap().edits_count, 2);
}

#[test]
fn actual_history_includes_partial_failure_and_excludes_a_noop_save() {
    let (mut app, id, doc) = pending();
    app.tool_runner = Some(Box::new(|_, _, _, _, _| Ok(json!({"saved":true}))));
    app.ask_agent_tool(7, Some(id), "doc_save", &json!({"doc":doc}), None).unwrap();
    assert_eq!(app.ask_agent.open.as_ref().unwrap().edits_count, 0);
    partial_failure(&mut app, id, doc);
    handle_action(&mut app, Action::Keep);
    let doc = pdfcraft_engine::DocId(doc);
    assert_eq!(app.session.get(doc).unwrap().undo_depth(), 1);
    app.session.undo(doc).unwrap();
    assert_eq!(app.session.get(doc).unwrap().info.pages[0].rotation, 0);
}

#[test]
fn keep_and_undo_never_group_or_remove_an_interleaved_human_edit() {
    for action in [Action::Keep, Action::Undo] {
        let (mut app, id, doc) = pending();
        partial_failure(&mut app, id, doc);
        let doc = pdfcraft_engine::DocId(doc);
        app.session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let before = app.session.get(doc).unwrap().bytes.clone();
        handle_action(&mut app, action);
        assert_eq!(app.session.get(doc).unwrap().undo_depth(), 3);
        assert_eq!(app.session.get(doc).unwrap().bytes, before);
    }
}

#[test]
fn a_disconnect_after_mutation_preserves_review_checkpoint_and_invalidates_the_request() {
    let (mut app, id, doc) = pending();
    let hub = app.ask_agent.hub.clone();
    app.tool_runner = Some(Box::new(move |session, _, _, args, _| {
        let doc = pdfcraft_engine::DocId(args["doc"].as_u64().unwrap());
        for _ in 0..2 {
            session.apply(doc, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        }
        hub.disconnect(7);
        hub.listen(8, Some("replacement"), std::time::Duration::ZERO, || true).unwrap().unwrap();
        Ok(json!({"steps":2}))
    }));
    assert!(app.ask_agent_tool(7, Some(id), "page_rotate", &json!({"doc":doc,"pages":[1],"degrees":90}), None).unwrap_err().contains("NOT_OWNER"));
    assert_eq!(app.ask_agent.open.as_ref().unwrap().edits_count, 2);
    assert_eq!(app.ask_agent.hub.snapshot(id).unwrap().status, Status::Failed);
    assert_eq!(app.ask_agent.author(id).as_deref(), Some("Agent (worker)"));
    assert!(app.ask_agent_command(&json!({"instruction":"do not reuse old selection"})).unwrap_err().contains("STALE_TARGET"));
    handle_action(&mut app, Action::Send("Keep my typed retry".into()));
    assert_eq!(app.ask_agent.draft, "Keep my typed retry");
    assert_eq!(app.ask_agent.open.as_ref().unwrap().id, id);
    handle_action(&mut app, Action::Keep);
    let doc = pdfcraft_engine::DocId(doc);
    assert_eq!(app.session.get(doc).unwrap().undo_depth(), 1);
    app.session.undo(doc).unwrap();
    assert_eq!(app.session.get(doc).unwrap().info.pages[0].rotation, 0);
}
