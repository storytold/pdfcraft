//! Spell checking comment text through the tool table (#5025).

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn call(a: &mut Automation, name: &str, args: Value) -> Result<Value, ToolError> {
    a.call(name, &args).map(|mut c| match c.remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    })
}

fn open_with_comments(dir: &std::path::Path) -> (Automation, u64) {
    let session = Session::new();
    let bytes = session.create_blank(300.0, 400.0, 2).unwrap();
    std::fs::write(dir.join("a.pdf"), &*bytes).unwrap();
    let mut a = Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000);
    let doc = call(&mut a, "doc_open", json!({"path": "a.pdf"})).unwrap()["doc"].as_u64().unwrap();
    for (page, text) in [(1, "Please chek this"), (2, "the cat sat, Zorblax")] {
        call(&mut a, "comment_add", json!({"doc": doc, "page": page, "type": "note", "at": [20, 20], "contents": text, "author": "Ada"})).unwrap();
    }
    (a, doc)
}

#[test]
fn comments_are_checked_against_a_word_list_and_the_users_words() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-spell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("words.txt"), "# list\nplease\ncheck\nthis\nthe\ncat\nsat\n").unwrap();
    let (mut a, doc) = open_with_comments(&dir);
    let undo_before = call(&mut a, "doc_info", json!({"doc": doc})).unwrap()["undo"].clone();

    let r = call(&mut a, "spell_check_comments", json!({"doc": doc, "dictionary": "words.txt"})).unwrap();
    assert_eq!((r["checked"].as_u64(), r["count"].as_u64()), (Some(2), Some(2)));
    let first = &r["misspellings"][0];
    assert_eq!(
        (first["page"].as_u64(), first["word"].as_str(), first["start"].as_u64(), first["end"].as_u64()),
        (Some(1), Some("chek"), Some(7), Some(11))
    );
    assert_eq!(first["suggestions"], json!(["check"]));
    assert_eq!(r["misspellings"][1]["word"], "Zorblax");

    let r = call(&mut a, "spell_check_comments", json!({"doc": doc, "dictionary": "words.txt", "words": ["Zorblax"], "page": 2})).unwrap();
    assert_eq!(r["count"], 0);
    assert_eq!(call(&mut a, "doc_info", json!({"doc": doc})).unwrap()["undo"], undo_before);
}

#[test]
fn a_missing_empty_or_binary_dictionary_is_refused_in_words() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-spell-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("empty.txt"), "# nothing\n").unwrap();
    std::fs::write(dir.join("binary.bin"), [0xff, 0xfe, 0x00]).unwrap();
    let (mut a, doc) = open_with_comments(&dir);
    for file in ["nope.txt", "empty.txt", "binary.bin"] {
        let e = call(&mut a, "spell_check_comments", json!({"doc": doc, "dictionary": file})).unwrap_err();
        assert!(matches!(e, ToolError::Failed(_)), "{file}: {e}");
    }
}
