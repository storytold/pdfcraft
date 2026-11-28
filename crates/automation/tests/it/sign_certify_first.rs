//! sign_document refuses to certify a document that already holds a signature (ISO 32000-2
//! §12.8.1: at most one certification, and approval signatures follow it), and writes no file.

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

/// A one-page PDF with no form.
fn page_pdf() -> Vec<u8> {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>", "<< /Type /Page /Parent 2 0 R >>"];
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

#[test]
fn sign_document_certifies_only_a_document_with_no_signature() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-certify-signed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("page.pdf"), page_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    ok(&mut a, "sign_id_create", json!({ "name": "Ada Lovelace", "key": "p256", "password": "secret12", "path": "ada.p12" }));
    let sign = |doc: u64, certify: Option<&str>, out: &str| {
        let mut args = json!({ "doc": doc, "id": "ada.p12", "password": "secret12", "page": 1, "rect": [20, 300, 180, 350], "out": out });
        if let Some(c) = certify {
            args["certify"] = json!(c);
        }
        args
    };
    // An unsigned document is certified.
    let doc = ok(&mut a, "doc_open", json!({ "path": "page.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "sign_document", sign(doc, Some("comments"), "certified.pdf"));
    assert_eq!(r["signature"]["certify"], 3, "{r}");
    // A second certification is refused, and no file is written.
    match a.call("sign_document", &sign(doc, Some("form_fill"), "twice.pdf")) {
        Err(ToolError::Failed(m)) => assert!(m.contains("already signed, so it can't be certified"), "{m}"),
        other => panic!("a second certification must be refused: {other:?}"),
    }
    assert!(!dir.join("twice.pdf").exists());
    // So is certifying after an approval signature.
    let doc = ok(&mut a, "doc_open", json!({ "path": "page.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "sign_document", sign(doc, None, "approved.pdf"));
    match a.call("sign_document", &sign(doc, Some("no_changes"), "late.pdf")) {
        Err(ToolError::Failed(m)) => assert!(m.contains("a certification has to be the document's first signature"), "{m}"),
        other => panic!("certifying a signed document must be refused: {other:?}"),
    }
    assert!(!dir.join("late.pdf").exists());
    let list = ok(&mut a, "sign_list", json!({ "doc": doc }));
    assert!(list["signatures"].as_array().unwrap().iter().all(|s| s["certify"].is_null()), "{list}");
    let _ = std::fs::remove_dir_all(&dir);
}
