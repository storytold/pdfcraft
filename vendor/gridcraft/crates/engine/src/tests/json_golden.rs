//! Byte-for-byte checks on the native JSON workbook (`.json`, `.scjson`). The files in
//! `crates/engine/tests/golden` were written before the format moved to `craft-gridcraft-format`
//! (storytold/craft-libs). Saving the same workbook must give the same bytes, and opening a
//! golden file and saving it again must give it back.
//! `GRIDCRAFT_BLESS_GOLDEN=1` rewrites them. Only do that for a deliberate format change.

use std::path::PathBuf;

use serde_json::json;

use super::s;
use crate::Session;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name)
}

fn check(name: &str, actual: &[u8]) {
    let path = golden(name);
    if std::env::var_os("GRIDCRAFT_BLESS_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let expected = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(expected == actual, "{name} changed ({} bytes, golden has {})", actual.len(), expected.len());
}

fn save(s: &mut Session) -> Vec<u8> {
    let r = s.execute("file.saveBytes", json!({"format": "json"})).unwrap();
    crate::io::base64_decode(r["base64"].as_str().unwrap()).unwrap()
}

fn open(s: &mut Session, name: &str, bytes: &[u8]) {
    s.execute("file.open", json!({"name": name, "base64": crate::io::base64_encode(bytes)})).unwrap();
}

/// A workbook with text, numbers, a formula, a style, a number format, a second sheet and a macro.
fn book() -> Session {
    let mut s = s();
    for (cell, input) in [("A1", "Item"), ("B1", "Qty"), ("A2", "Äpfel ✓"), ("B2", "3.5"), ("A3", "Pears"), ("B3", "2"), ("B4", "=SUM(B2:B3)")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    s.execute("home.bold", json!({"range": "A1:B1"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "B2:B4", "format": "0.00"})).unwrap();
    s.execute("macro.save", json!({"name": "Fill", "steps": [{"command": "cell.set", "params": {"cell": "C1", "input": "x"}}]})).unwrap();
    s
}

#[test]
fn a_save_writes_the_golden_workbook_bytes() {
    let mut s = book();
    check("book.json", &save(&mut s));
}

#[test]
fn the_golden_workbook_opens_and_saves_back_unchanged() {
    let bytes = std::fs::read(golden("book.json")).unwrap();
    for name in ["book.json", "book.scjson"] {
        let mut s = s();
        open(&mut s, name, &bytes);
        assert_eq!(super::v(&s, "B4"), gridcraft_core::Value::Number(5.5), "{name}");
        assert!(save(&mut s) == bytes, "{name} saved back differently");
    }
}

#[test]
fn macros_in_a_newer_format_are_written_back_as_the_file_held_them() {
    // The save that splices kept macros back into `macros` goes through a JSON value.
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(golden("book.json")).unwrap()).unwrap();
    v["macros"].as_array_mut().unwrap().push(json!({"name": "Future", "v": gridcraft_model::MACRO_VERSION + 1, "extra": [1, 2]}));
    let mut s = s();
    open(&mut s, "kept.json", &serde_json::to_vec_pretty(&v).unwrap());
    check("kept.json", &save(&mut s));
}

#[test]
fn what_is_not_a_json_workbook_says_so() {
    let err = crate::io::open_bytes("bad.json", b"{\"sheets\": 7}").unwrap_err().to_string();
    check("bad.error.txt", err.as_bytes());
}
