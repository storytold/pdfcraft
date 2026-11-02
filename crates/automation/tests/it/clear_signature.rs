//! Remove Signature through the tools: sign_removable lists, sign_clear removes, doc_save keeps
//! the original and writes a new file. Synthetic PDFs and a digital ID created here.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// Two 200×300 pt pages reading "Page 1" and "Page 2".
fn fixture() -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
        b"<< /Length 34 >>\nstream\nBT /F1 24 Tf 20 150 Td (Page 1) Tj ET\nendstream".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
        b"<< /Length 34 >>\nstream\nBT /F1 24 Tf 20 150 Td (Page 2) Tj ET\nendstream".to_vec(),
    ];
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

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-clear-signature-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), fixture()).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

/// A document with two signatures, the first certifying (form filling allowed): saved as
/// `signed.pdf`. Returns the open automation, the document and the two field names.
fn signed_twice(dir: &Path, certify: Option<&str>) -> (Automation, u64, String, String) {
    let mut a = auto(dir);
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let mut first = json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 1, "rect": [20, 200, 180, 250], "out": "one.pdf" });
    if let Some(c) = certify {
        first["certify"] = json!(c);
    }
    let one = ok(&mut a, "sign_document", first)["signature"]["field"].as_str().unwrap().to_owned();
    let second = json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 2, "rect": [20, 200, 180, 250], "out": "signed.pdf" });
    let two = ok(&mut a, "sign_document", second)["signature"]["field"].as_str().unwrap().to_owned();
    (a, doc, one, two)
}

#[test]
fn removing_a_signature_and_saving_a_new_file_keeps_the_original() {
    let dir = workdir("two");
    let (mut a, doc, one, two) = signed_twice(&dir, None);
    let original = std::fs::read(dir.join("signed.pdf")).unwrap();
    let list = ok(&mut a, "sign_removable", json!({ "doc": doc }));
    assert_eq!(list["count"], 2, "{list}");
    let first = &list["items"][0];
    assert_eq!((first["field"].as_str(), first["kind"].as_str(), first["removable"].as_bool()), (Some(one.as_str()), Some("digital"), Some(true)));
    assert_eq!(first["status"], "unknown", "validated, not trusted: never reported as valid");
    let effects: Vec<String> = serde_json::from_value(first["effects"].clone()).unwrap();
    assert!(effects.iter().any(|e| e.contains(&two) && e.contains("Signed after")), "{effects:?}");
    assert!(effects.iter().any(|e| e.contains("recovered")), "{effects:?}");

    let r = ok(&mut a, "sign_clear", json!({ "doc": doc, "field": two }));
    assert_eq!(r["signed"], true, "the first signature remains");
    assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), original, "nothing is written until saved");
    // A full rewrite would break the remaining signature: refused when asked for, and not what
    // saving to a new path does by default while the document is signed.
    match a.call("doc_save", &json!({ "doc": doc, "path": "unsigned.pdf", "full": true })) {
        Err(ToolError::Failed(m)) => assert!(m.contains("signed"), "{m}"),
        other => panic!("a full rewrite of a signed document must be refused: {other:?}"),
    }
    assert!(!dir.join("unsigned.pdf").exists(), "a refused save writes nothing");
    let saved = ok(&mut a, "doc_save", json!({ "doc": doc, "path": "unsigned.pdf" }));
    assert_eq!(saved["incremental"], true);
    assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), original, "the original is untouched");
    let new = std::fs::read(dir.join("unsigned.pdf")).unwrap();
    assert!(new.starts_with(&original), "the removed signature stays in the earlier revision");

    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "unsigned.pdf" }))["doc"].as_u64().unwrap();
    let sigs = ok(&mut b, "sign_list", json!({ "doc": re }));
    assert_eq!(sigs["count"], 1, "{sigs}");
    assert_eq!(sigs["signatures"][0]["field"].as_str(), Some(one.as_str()));
    assert_ne!(sigs["signatures"][0]["modification"], "none", "the remaining signature reports the change");
}

#[test]
fn the_last_signature_removed_saves_a_clean_copy() {
    let dir = workdir("clean");
    let (mut a, doc, one, two) = signed_twice(&dir, None);
    ok(&mut a, "sign_clear", json!({ "doc": doc, "field": two }));
    let r = ok(&mut a, "sign_clear", json!({ "doc": doc, "item": 0, "keep_field": true }));
    assert_eq!(r["signed"], false);
    let saved = ok(&mut a, "doc_save", json!({ "doc": doc, "path": "clean.pdf" }));
    assert_eq!(saved["incremental"], false);
    let clean = std::fs::read(dir.join("clean.pdf")).unwrap();
    assert!(!clean.windows(10).any(|w| w == b"/ByteRange"), "no signature is left in the clean copy");
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "clean.pdf" }))["doc"].as_u64().unwrap();
    let sigs = ok(&mut b, "sign_list", json!({ "doc": re }));
    assert_eq!((sigs["count"].as_u64(), sigs["signed"].as_u64()), (Some(1), Some(0)), "the kept field is empty: {sigs}");
    assert_eq!(sigs["signatures"][0]["field"].as_str(), Some(one.as_str()));
    // It can be signed again.
    let again = json!({ "doc": re, "id": "ada.p12", "password": "secret12", "field": one, "out": "resigned.pdf" });
    assert_eq!(ok(&mut b, "sign_document", again)["signature"]["signed"], true);
}

#[test]
fn a_certified_documents_other_signature_is_refused_with_the_reason() {
    let dir = workdir("certified");
    let (mut a, doc, one, two) = signed_twice(&dir, Some("form_fill"));
    let list = ok(&mut a, "sign_removable", json!({ "doc": doc }));
    let item = |f: &str| list["items"].as_array().unwrap().iter().find(|i| i["field"] == f).unwrap().clone();
    assert_eq!((item(&one)["kind"].as_str(), item(&one)["certify"].as_u64()), (Some("certification"), Some(2)));
    assert_eq!(item(&two)["removable"], false);
    assert!(item(&two)["blocked"].as_str().unwrap().contains("certifies this document"));
    match a.call("sign_clear", &json!({ "doc": doc, "field": two })) {
        Err(ToolError::Failed(m)) => assert!(m.contains("certifies"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": doc }))["document"]["dirty"], false, "a refusal changes nothing");
}

#[test]
fn bad_arguments_say_what_is_allowed() {
    let dir = workdir("args");
    let (mut a, doc, ..) = signed_twice(&dir, None);
    for args in [json!({ "doc": doc }), json!({ "doc": doc, "field": "Signature1", "item": 0 })] {
        assert!(matches!(a.call("sign_clear", &args), Err(ToolError::InvalidArgs(m)) if m.contains("field or item")));
    }
    assert!(matches!(a.call("sign_clear", &json!({ "doc": doc, "item": 99 })), Err(ToolError::InvalidArgs(m)) if m.contains("not in")));
    assert!(matches!(a.call("sign_clear", &json!({ "doc": doc, "field": "nope" })), Err(ToolError::Failed(m)) if m.contains("no signature")));
}

#[test]
fn a_visual_signature_is_numbered_as_the_comment_tools_number_it() {
    let dir = workdir("visual");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "check", "at": [20, 80] }));
    ok(&mut a, "fill_sign_add", json!({ "doc": doc, "page": 1, "type": "signature", "at": [20, 140], "text": "Ada" }));
    let list = ok(&mut a, "sign_removable", json!({ "doc": doc }));
    assert_eq!(list["count"], 1, "the check mark is not a signature: {list}");
    let at = list["items"][0]["annotation"].clone();
    // The same numbers comment_list gives the signature (the check mark comes first).
    let comments = ok(&mut a, "comment_list", json!({ "doc": doc }));
    let indexes: Vec<&Value> = comments["comments"].as_array().unwrap().iter().map(|c| &c["index"]).collect();
    assert_eq!((at["page"].as_u64(), at["index"].as_u64()), (Some(1), Some(2)), "{comments}");
    assert!(indexes.contains(&&json!(2)), "{comments}");
    ok(&mut a, "sign_clear", json!({ "doc": doc, "item": 0 }));
    assert_eq!(ok(&mut a, "sign_removable", json!({ "doc": doc }))["count"], 0);
    assert_eq!(ok(&mut a, "comment_list", json!({ "doc": doc }))["comments"].as_array().unwrap().len(), 1, "the check mark stays");
}

/// A document secured with "fill-sign" changes, opened without its permissions password: its
/// signature can't be removed, and the listing says so before anything is tried.
#[test]
fn security_that_forbids_changes_is_said_in_the_listing() {
    let dir = workdir("secured");
    let mut a = auto(&dir);
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let field = ok(&mut a, "form_add_field", json!({ "doc": doc, "page": 1, "type": "signature", "rect": [20, 200, 180, 250] }))["field"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(&mut a, "doc_protect", json!({ "doc": doc, "permissions_password": "boss", "changes": "fill-sign" }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "form.pdf" }));
    let mut b = auto(&dir);
    let form = ok(&mut b, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut b, "sign_document", json!({ "doc": form, "id": "ada.p12", "password": "secret12", "field": field, "out": "signed.pdf" }));
    let list = ok(&mut b, "sign_removable", json!({ "doc": form }));
    let item = &list["items"][0];
    assert_eq!(item["removable"], false, "{list}");
    assert!(item["blocked"].as_str().unwrap().contains("security"), "{list}");
    assert!(matches!(b.call("sign_clear", &json!({ "doc": form, "field": field })), Err(ToolError::Failed(m)) if m.contains("security")));
}
