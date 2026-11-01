//! The existing date-language preference reaches dynamic stamps through the headless tools.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn call(automation: &mut Automation, tool: &str, args: Value) -> Value {
    automation
        .call(tool, &args)
        .unwrap()
        .into_iter()
        .find_map(|content| match content {
            Content::Json(value) => Some(value),
            _ => None,
        })
        .expect("tool returns JSON")
}

fn page_text(automation: &mut Automation, doc: &Value) -> String {
    call(automation, "text_extract", json!({ "doc": doc }))["pages"][0]["text"].as_str().unwrap().to_string()
}

#[test]
fn dynamic_stamps_follow_the_date_language_through_save_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let mut automation = Automation::new().with_root(directory.path()).unwrap().with_wall_clock(|| 1_792_024_200, || -3 * 3600);
    let doc = call(&mut automation, "doc_create", json!({ "from": "blank" }))["doc"].clone();
    call(&mut automation, "fill_sign_date_format", json!({ "language": "pt-br", "format": "yyyy.mm.dd." }));
    call(
        &mut automation,
        "comment_add",
        json!({ "doc": doc, "page": 1, "type": "stamp", "stamp": "reviewed", "dynamic": true, "at": [300, 200], "author": "Ana" }),
    );
    let expected = "Por Ana às 21:30, 14/10/2026";
    let text = page_text(&mut automation, &doc);
    assert!(text.contains(expected), "{text}");
    assert!(!text.contains("By Ana") && !text.contains("pm"), "{text}");
    // Changing the language affects only the next stamp, never the appearance already placed.
    call(&mut automation, "fill_sign_date_format", json!({ "language": "en" }));
    call(
        &mut automation,
        "comment_add",
        json!({ "doc": doc, "page": 1, "type": "stamp", "stamp": "reviewed", "dynamic": true, "at": [300, 300], "author": "Ada" }),
    );
    let english = "By Ada at 9:30 pm, Oct 14, 2026";
    let text = page_text(&mut automation, &doc);
    assert!(text.contains(expected) && text.contains(english), "{text}");
    // Rendering the same appearance is part of the headless route, not just metadata inspection.
    let image = automation.call("page_render", &json!({ "doc": doc, "page": 1, "dpi": 72 })).unwrap();
    assert!(image.iter().any(|content| matches!(content, Content::Png { width, height, .. } if *width > 0 && *height > 0)));
    call(&mut automation, "doc_save", json!({ "doc": doc, "path": "stamp.pdf" }));
    let reopened = call(&mut automation, "doc_open", json!({ "path": "stamp.pdf" }))["doc"].clone();
    let text = page_text(&mut automation, &reopened);
    assert!(text.contains(expected) && text.contains(english), "{text}");
    call(&mut automation, "edit_undo", json!({ "doc": doc }));
    let text = page_text(&mut automation, &doc);
    assert!(text.contains(expected) && !text.contains(english), "{text}");
    call(&mut automation, "edit_redo", json!({ "doc": doc }));
    let text = page_text(&mut automation, &doc);
    assert!(text.contains(expected) && text.contains(english), "{text}");
}
