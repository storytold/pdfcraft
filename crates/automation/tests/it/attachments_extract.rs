//! `attachments_extract` writes a portfolio's files (a document's own attachments) into a folder,
//! never replacing a file there and never writing outside it, and says which it couldn't write.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A one-page portfolio of `files` (name, bytes), plus `gone.txt`, a file specification whose
/// file isn't embedded.
fn portfolio(files: &[(&str, &[u8])]) -> Vec<u8> {
    let gone = 4 + 2 * files.len();
    let mut names: String = files.iter().enumerate().map(|(i, (name, _))| format!("({name}) {} 0 R ", 5 + 2 * i)).collect();
    names.push_str(&format!("(gone.txt) {gone} 0 R"));
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Collection << /View /D >> /Names << /EmbeddedFiles << /Names [{names}] >> >> >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_vec(),
    ];
    for (i, (name, data)) in files.iter().enumerate() {
        objs.push([format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat());
        objs.push(format!("<< /Type /Filespec /F ({name}) /UF ({name}) /EF << /F {} 0 R >> >>", 4 + 2 * i).into_bytes());
    }
    objs.push(b"<< /Type /Filespec /F (gone.txt) /UF (gone.txt) >>".to_vec());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The written files, by name, in `dir`.
fn listing(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap()))
        .collect();
    files.sort();
    files
}

#[test]
fn every_file_lands_in_the_folder_without_replacing_one_and_an_unembedded_one_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = portfolio(&[("brief.pdf", b"%PDF-1.4 brief"), ("notes.txt", b"from the portfolio"), ("../escape.txt", b"climbs out")]);
    std::fs::write(dir.path().join("p.pdf"), pdf).unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("notes.txt"), b"already here").unwrap();

    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut auto, "doc_open", json!({ "path": "p.pdf" }))["doc"].as_u64().unwrap();
    let r = tool(&mut auto, "attachments_extract", json!({ "doc": doc, "folder": "out" }));
    assert_eq!((r["saved"].as_u64(), r["failed"].as_u64()), (Some(3), Some(1)), "{r}");
    let gone = r["files"].as_array().unwrap().iter().find(|f| f["key"] == "gone.txt").unwrap();
    assert!(gone["error"].as_str().is_some_and(|e| !e.is_empty()) && gone.get("path").is_none(), "{gone}");

    let files = listing(&out);
    assert!(files.contains(&("notes.txt".into(), b"already here".to_vec())), "a file already in the folder is kept: {files:?}");
    let contents: Vec<&[u8]> = files.iter().map(|(_, b)| b.as_slice()).collect();
    for wanted in [&b"%PDF-1.4 brief"[..], b"from the portfolio", b"climbs out"] {
        assert!(contents.contains(&wanted), "{:?} was written: {files:?}", String::from_utf8_lossy(wanted));
    }
    assert_eq!(files.len(), 4, "nothing else is in the folder: {files:?}");
    assert!(!dir.path().join("escape.txt").exists(), "a name with a path stays in the folder");

    // Again: every file is written once more beside the first copies, none of which changes.
    let again = tool(&mut auto, "attachments_extract", json!({ "doc": doc, "folder": "out" }));
    assert_eq!(again["saved"].as_u64(), Some(3), "{again}");
    let after = listing(&out);
    assert!(files.iter().all(|f| after.contains(f)) && after.len() == files.len() + 3, "{after:?}");
}

#[test]
fn chosen_keys_only_and_an_unknown_key_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("p.pdf"), portfolio(&[("a.txt", b"first"), ("b.txt", b"second")])).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut auto, "doc_open", json!({ "path": "p.pdf" }))["doc"].as_u64().unwrap();

    let r = tool(&mut auto, "attachments_extract", json!({ "doc": doc, "folder": "one", "keys": ["b.txt"] }));
    assert_eq!(r["saved"].as_u64(), Some(1), "{r}");
    assert_eq!(listing(&dir.path().join("one")), vec![("b.txt".to_string(), b"second".to_vec())]);

    let refused = auto.call("attachments_extract", &json!({ "doc": doc, "folder": "none", "keys": ["a.txt", "missing.txt"] }));
    assert!(refused.is_err(), "an unknown key is refused");
    assert!(!dir.path().join("none").exists(), "and nothing is written for the keys that exist");
}
