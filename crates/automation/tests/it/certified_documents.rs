//! The tools refuse the changes a certification doesn't permit (ISO 32000-2 §12.8.2.2.1, Table
//! 257) with the engine's error, and still make and save the ones it does.

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

fn failed(a: &mut Automation, tool: &str, args: Value) -> String {
    match a.call(tool, &args) {
        Err(ToolError::Failed(m)) => m,
        other => panic!("{tool} {args}: expected a refusal, got {other:?}"),
    }
}

/// A one-page PDF whose form holds one text field named `name`.
fn form_pdf() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (x) /Rect [60 60 200 80] /P 3 0 R >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    bytes
}

/// A scratch folder holding the form and a digital ID, and the form certified at `level`
/// (`no_changes`, `form_fill` or `comments`) as `certified.pdf`, opened in a fresh session.
fn certified(test: &str, level: &str) -> (std::path::PathBuf, Automation, u64) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-certified-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("form.pdf"), form_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let doc = ok(&mut a, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(
        &mut a,
        "sign_document",
        json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 1, "rect": [20, 300, 180, 350], "certify": level, "out": "certified.pdf" }),
    );
    assert!(r["signature"]["certify"].is_u64(), "{r}");
    let mut b = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut b, "doc_open", json!({ "path": "certified.pdf" }))["doc"].as_u64().unwrap();
    (dir, b, doc)
}

fn certification(a: &mut Automation, doc: u64) -> Value {
    let list = ok(a, "sign_list", json!({ "doc": doc }));
    list["signatures"].as_array().unwrap().iter().find(|s| s["certify"].is_u64()).cloned().unwrap()
}

#[test]
fn a_no_changes_certification_refuses_comments_pages_and_form_fill() {
    let (dir, mut a, doc) = certified("p1", "no_changes");
    let note = json!({ "doc": doc, "page": 1, "type": "note", "at": [20, 20], "contents": "Check", "author": "Ada" });
    let m = failed(&mut a, "comment_add", note);
    assert!(m.contains("certified with no changes allowed") && m.contains("comments"), "{m}");
    assert!(failed(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 90 })).contains("page changes"));
    assert!(failed(&mut a, "form_fill", json!({ "doc": doc, "values": { "name": "Ada" } })).contains("filling in form fields"));
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["pages"][0]["rotation"], 0, "{info}");
    assert_eq!(info["certification"], 1, "doc_info says what the certification allows");
    // Nothing was changed, so a save writes the signed file as it was.
    let before = std::fs::read(dir.join("certified.pdf")).unwrap();
    ok(&mut a, "doc_save", json!({ "doc": doc }));
    assert_eq!(std::fs::read(dir.join("certified.pdf")).unwrap(), before);
    assert_eq!(certification(&mut a, doc)["modification"], "none");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_form_fill_certification_still_fills_and_saves() {
    let (dir, mut a, doc) = certified("p2", "form_fill");
    let note = json!({ "doc": doc, "page": 1, "type": "note", "at": [20, 20], "contents": "Check", "author": "Ada" });
    assert!(failed(&mut a, "comment_add", note).contains("comments"));
    ok(&mut a, "form_fill", json!({ "doc": doc, "values": { "name": "Ada" } }));
    // Saved in place, as an incremental update (a signed file is never rewritten).
    ok(&mut a, "doc_save", json!({ "doc": doc }));
    let mut c = Automation::new().with_root(&dir).unwrap();
    let re = ok(&mut c, "doc_open", json!({ "path": "certified.pdf" }))["doc"].as_u64().unwrap();
    let cert = certification(&mut c, re);
    assert_eq!(cert["modification"], "allowed", "{cert}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_comments_certification_still_takes_comments() {
    let (dir, mut a, doc) = certified("p3", "comments");
    assert!(failed(&mut a, "page_rotate", json!({ "doc": doc, "pages": [1], "degrees": 90 })).contains("page changes"));
    ok(&mut a, "comment_add", json!({ "doc": doc, "page": 1, "type": "note", "at": [20, 20], "contents": "Check", "author": "Ada" }));
    // Saved in place, as an incremental update (a signed file is never rewritten).
    ok(&mut a, "doc_save", json!({ "doc": doc }));
    let mut c = Automation::new().with_root(&dir).unwrap();
    let re = ok(&mut c, "doc_open", json!({ "path": "certified.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(certification(&mut c, re)["modification"], "allowed");
    let _ = std::fs::remove_dir_all(&dir);
}
