//! PdfCraft's plug-in rules (RFC-0046): the allowlist against the tool table, file parameters and
//! settings limits.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{ALLOWED, FILE_KEYS, MAX_SETTINGS_BYTES, base64_decode, policy, refusal, set_setting, tool_named};
use crate::tools;

fn manifest(commands: &[&str]) -> craft_plugin::Manifest {
    craft_plugin::Manifest::from_value(&json!({
        "id": "org.example.t", "name": "T", "version": "1.0.0", "main": "t.wasm",
        "apps": {"pdfcraft": {"api": 6}}, "permissions": {"access": "write", "commands": commands},
    }))
    .unwrap()
}

#[test]
fn every_allowed_tool_exists_and_names_no_other_document_or_file_it_could_reach() {
    for name in ALLOWED {
        let def = tools::find(name).unwrap_or_else(|| panic!("{name} is not a tool"));
        let props = def.input_schema["properties"].as_object().cloned().unwrap_or_default();
        for (key, spec) in &props {
            let text = spec.to_string();
            // A document id other than `doc` would let a run reach another document.
            if text.contains("Document id") {
                assert_eq!(key, "doc", "{name}.{key} names a document");
            }
            // A property that takes a file path is one `refusal` refuses.
            let file = spec["description"].as_str().is_some_and(|d| d.starts_with("A file path")) || matches!(key.as_str(), "file" | "image");
            if file {
                assert!(FILE_KEYS.contains(&key.as_str()), "{name}.{key} takes a file and isn't refused");
            }
        }
    }
}

#[test]
fn the_policy_admits_the_allowlist_and_its_registry_ids_only() {
    let p = policy();
    for name in ALLOWED {
        assert!(!p.refuses(name), "{name}");
    }
    for refused in
        ["doc_save", "doc_open", "doc_close", "edit_undo", "command_run", "js_run", "redact_apply", "doc_print", "plugin_run", "sign_document"]
    {
        assert!(p.refuses(refused), "{refused}");
    }
    // Registry ids reach the tool they name, and are admitted only when it is.
    assert_eq!(tool_named("page.rotate").map(|t| t.name), Some("page_rotate"));
    assert!(!p.refuses("page.rotate"));
    for id in ["file.save", "file.open", "edit.undo", "tools.js_console", "page.extract", "sign.digital"] {
        assert!(p.refuses(id), "{id}");
    }
}

#[test]
fn text_under_a_file_key_is_refused_at_any_depth_but_bookmark_paths_are_not() {
    let m = manifest(&[]);
    for params in [json!({"path": "/etc/passwd"}), json!({"x": [{"file": "a.pdf"}]}), json!({"image": "p.png"}), json!({"paths": ["a"]})] {
        assert!(refusal(&m, "comment_add", &params).is_some_and(|w| w.contains("can't name files")), "{params}");
    }
    assert_eq!(refusal(&m, "bookmark_rename", &json!({"path": [1, 2], "title": "x"})), None);
    assert!(refusal(&manifest(&["bookmark_*"]), "page_rotate", &json!({})).is_some_and(|w| w.contains("asked permission")));
}

#[test]
fn settings_are_bounded() {
    let mut all: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    set_setting(&mut all, "p", "k", Some("1")).unwrap();
    assert_eq!(all["p"]["k"], json!(1));
    assert!(set_setting(&mut all, "p", &"k".repeat(129), Some("1")).is_err());
    assert!(set_setting(&mut all, "p", "big", Some(&format!("\"{}\"", "x".repeat(MAX_SETTINGS_BYTES)))).is_err());
    assert!(set_setting(&mut all, "p", "bad", Some("not json")).is_err());
    set_setting(&mut all, "p", "k", None).unwrap();
    assert!(all.is_empty());
}

#[test]
fn base64_reads_standard_and_url_safe_text() {
    assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
    assert_eq!(base64_decode("aGVs\nbG8").as_deref(), Some(&b"hello"[..]));
    assert_eq!(base64_decode("a$"), None);
    assert_eq!(base64_decode("aG=V"), None);
}
