#![cfg(feature = "mcp")]

mod connected_support;

use pdfcraft_automation::window::WindowTools;
use pdfcraft_engine::Edit;
use serde_json::json;

#[test]
fn repeated_edits_use_the_same_connections_updated_paragraph_keys() {
    let (mut session, id) = connected_support::session();
    let mut tools = WindowTools::default();
    tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    for text in ["Charlie revised once.", "Charlie revised twice."] {
        tools.call(1, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":3,"text":text})).unwrap();
    }
    let listed = tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    assert!(listed.to_string().contains("Charlie revised twice."));
}

#[test]
fn another_connections_listing_cannot_renumber_the_first_connections_target() {
    let (mut session, id) = connected_support::session();
    let mut tools = WindowTools::default();
    let listed = tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    assert_eq!(listed["content"][0]["value"]["count"], 3);
    tools.call(1, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":2,"text":""})).unwrap();
    let fresh = tools.call(2, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    assert!(fresh["content"][0]["value"]["paragraphs"][1]["text"].as_str().unwrap().starts_with("Charlie"), "{fresh}");
    let before = session.get(id).unwrap().bytes.clone();
    let error =
        tools.call(1, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":2,"text":"must not replace Charlie"})).unwrap_err();
    assert!(error.contains("text_paragraphs"), "{error}");
    assert_eq!(session.get(id).unwrap().bytes, before);
    tools.call(2, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":2,"text":"Charlie by second connection."})).unwrap();
}

#[test]
fn an_outside_edit_and_context_eviction_require_a_fresh_listing() {
    let (mut session, id) = connected_support::session();
    let mut tools = WindowTools::default();
    tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let error = tools.call(1, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":1,"text":"stale"})).unwrap_err();
    assert!(error.contains("text_paragraphs"), "{error}");
    tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    for connection in 2..40 {
        tools.call(connection, &mut session, None, "doc_list", &json!({})).unwrap();
    }
    let error = tools.call(1, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":1,"text":"evicted"})).unwrap_err();
    assert!(error.contains("text_paragraphs"), "{error}");
}

#[test]
fn a_failed_call_does_not_lose_the_connections_listing_or_session() {
    let (mut session, id) = connected_support::session();
    let mut tools = WindowTools::default();
    tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    assert!(tools.call(1, &mut session, None, "doc_info", &json!({"doc":999})).is_err());
    tools.call(1, &mut session, None, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":3,"text":"Charlie after error."})).unwrap();
    assert_eq!(session.docs().len(), 1);
}

#[test]
fn another_document_does_not_inherit_this_connections_listing() {
    let (mut session, first) = connected_support::session();
    let mut tools = WindowTools::default();
    tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":first.0,"page":1})).unwrap();
    let second = session.open("second.pdf", None, std::sync::Arc::new(connected_support::pdf()), None).unwrap();
    let error =
        tools.call(1, &mut session, None, "text_edit", &json!({"doc":second.0,"page":1,"paragraph":1,"text":"wrong document context"})).unwrap_err();
    assert!(error.contains("text_paragraphs"), "{error}");
    tools
        .call(1, &mut session, None, "text_edit", &json!({"doc":first.0,"page":1,"paragraph":3,"text":"Original document remains addressable."}))
        .unwrap();
}
