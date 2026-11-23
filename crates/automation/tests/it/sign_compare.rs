//! `sign_compare_signed_version`: the version a signature signed compared with the document as it
//! is now, through the tools, and the signatures it refuses.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// A fresh directory for one test.
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-sign-compare-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
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

/// The message of a refused call, which must be `InvalidArgs` (`args`) or `Failed`.
fn refused(a: &mut Automation, tool: &str, args: Value, invalid_args: bool) -> String {
    match a.call(tool, &args) {
        Err(ToolError::InvalidArgs(m)) if invalid_args => m,
        Err(ToolError::Failed(m)) if !invalid_args => m,
        other => panic!("{tool} {args}: {other:?}"),
    }
}

/// Two pages, page n showing "Page n" in Helvetica.
fn two_pages() -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for i in 0..2 {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// `signed` with its `/ByteRange` numbers replaced by `numbers`, padded to the same width.
fn with_byte_range(signed: &[u8], numbers: &str) -> Vec<u8> {
    let key = b"/ByteRange [";
    let at = signed.windows(key.len()).position(|w| w == key).unwrap() + key.len();
    let width = signed[at..].iter().position(|b| *b == b']').unwrap();
    assert!(numbers.len() <= width);
    let mut out = signed.to_vec();
    let mut patch = numbers.as_bytes().to_vec();
    patch.resize(width, b' ');
    out[at..at + width].copy_from_slice(&patch);
    out
}

/// Sign `two_pages` invisibly with a fresh P-256 ID into `signed.pdf`; returns the open document.
fn signed_doc(a: &mut Automation, dir: &Path) -> u64 {
    std::fs::write(dir.join("unsigned.pdf"), two_pages()).unwrap();
    ok(a, "sign_id_create", json!({ "name": "Test Signer", "key": "p256", "password": "secret12", "path": "signer.p12" }));
    let doc = ok(a, "doc_open", json!({ "path": "unsigned.pdf" }))["doc"].as_u64().unwrap();
    ok(a, "sign_document", json!({ "doc": doc, "id": "signer.p12", "password": "secret12", "reason": "Approved", "out": "signed.pdf" }));
    doc
}

#[test]
fn comparing_a_signed_version_with_text_added_after_signing() {
    let dir = tmp("added-text");
    let mut a = auto(&dir);
    let doc = signed_doc(&mut a, &dir);
    let list = ok(&mut a, "sign_list", json!({ "doc": doc }));
    let field = list["signatures"][0]["field"].as_str().unwrap().to_string();
    let signed_len = std::fs::read(dir.join("signed.pdf")).unwrap().len();
    // Straight after signing the signature covers the file: nothing differs.
    let r = ok(&mut a, "sign_compare_signed_version", json!({ "doc": doc, "field": field }));
    assert_eq!(r["identical"], true, "{r}");
    assert_eq!(r["signature"]["covers_current"], true, "{r}");
    assert_eq!((r["signature"]["revision"].as_u64(), r["signature"]["signed_bytes"].as_u64()), (Some(2), Some(signed_len as u64)));
    // Text added after signing is what the comparison finds, on the page it was added to.
    ok(&mut a, "page_add_text", json!({ "doc": doc, "page": 2, "at": [20, 60], "text": "Added later" }));
    let docs_before = ok(&mut a, "doc_list", json!({}));
    let r = ok(&mut a, "sign_compare_signed_version", json!({ "doc": doc, "index": 1, "visual": true }));
    assert_eq!(ok(&mut a, "doc_list", json!({})), docs_before, "the signed version is closed again");
    assert_eq!(
        (r["identical"].as_bool(), r["inserted"].as_u64(), r["replaced"].as_u64(), r["deleted"].as_u64()),
        (Some(false), Some(1), Some(0), Some(0)),
        "{r}"
    );
    assert_eq!((r["changes"][0]["kind"].as_str(), r["changes"][0]["new"]["text"].as_str()), (Some("inserted"), Some("Added later")));
    assert_eq!(r["changes"][0]["new"]["page"], 2);
    assert!(r["visual"].as_array().unwrap().iter().all(|v| v["page"] == 2) && !r["visual"].as_array().unwrap().is_empty(), "{r}");
    let sig = &r["signature"];
    assert_eq!((sig["field"].as_str(), sig["index"].as_u64(), sig["covers_current"].as_bool()), (Some(field.as_str()), Some(1), Some(false)));
    assert!(sig["bytes"].as_u64().unwrap() > sig["signed_bytes"].as_u64().unwrap(), "{sig}");
    // The same report shape doc_compare gives two open documents.
    let kept = ok(&mut a, "sign_compare_signed_version", json!({ "doc": doc, "field": field, "keep_open": true, "limit": 1 }));
    let old = kept["signed_doc"].as_u64().unwrap();
    let mut plain = ok(&mut a, "doc_compare", json!({ "doc": doc, "other": old, "limit": 1 }));
    let mut kept_report = kept.clone();
    for v in [&mut plain, &mut kept_report] {
        let o = v.as_object_mut().unwrap();
        o.remove("signature");
        o.remove("signed_doc");
    }
    assert_eq!(plain, kept_report);
    // The kept signed version is the file as signed, and the existing compare tools take it.
    let list = ok(&mut a, "doc_list", json!({}));
    let kept_doc = list["documents"].as_array().unwrap().iter().find(|d| d["doc"] == old).unwrap();
    assert!(kept_doc["name"].as_str().unwrap().ends_with(" (signed version).pdf"), "{kept_doc}");
    assert_eq!(ok(&mut a, "sign_list", json!({ "doc": old }))["signatures"][0]["field"], field.as_str());
    let report = ok(&mut a, "doc_compare_report", json!({ "doc": doc, "other": old, "path": "report.pdf" }));
    assert!(report["bytes"].as_u64().unwrap() > 0 && dir.join("report.pdf").exists());
}

#[test]
fn signatures_without_a_signed_version_are_refused() {
    let dir = tmp("refused");
    let mut a = auto(&dir);
    let doc = signed_doc(&mut a, &dir);
    let field = ok(&mut a, "sign_list", json!({ "doc": doc }))["signatures"][0]["field"].as_str().unwrap().to_string();
    let tool = "sign_compare_signed_version";
    // Naming the signature: one way, and one that exists.
    assert!(refused(&mut a, tool, json!({ "doc": doc }), true).contains("pass field"));
    assert!(refused(&mut a, tool, json!({ "doc": doc, "field": field, "index": 1 }), true).contains("not both"));
    assert!(refused(&mut a, tool, json!({ "doc": doc, "index": 2 }), true).contains("1 signature"));
    assert!(refused(&mut a, tool, json!({ "doc": doc, "index": 0 }), true).contains("out of range"));
    assert!(refused(&mut a, tool, json!({ "doc": doc, "field": "Nope" }), true).contains(&field));
    let plain = ok(&mut a, "doc_open", json!({ "path": "unsigned.pdf" }))["doc"].as_u64().unwrap();
    assert!(refused(&mut a, tool, json!({ "doc": plain, "index": 1 }), true).contains("0 signature"));
    // Byte ranges that are malformed, overflow or don't fit the file: refused, naming the
    // signature, and nothing is left open.
    let signed = std::fs::read(dir.join("signed.pdf")).unwrap();
    let docs_before = ok(&mut a, "doc_list", json!({}))["documents"].as_array().map(Vec::len);
    for (i, numbers) in ["0 10 20 999999999", "0 10 99999999999999999999 5", "0 10 20", "0 -10 20 5"].iter().enumerate() {
        let name = format!("broken{i}.pdf");
        std::fs::write(dir.join(&name), with_byte_range(&signed, numbers)).unwrap();
        let broken = ok(&mut a, "doc_open", json!({ "path": name }))["doc"].as_u64().unwrap();
        assert_eq!(ok(&mut a, "sign_list", json!({ "doc": broken }))["signatures"][0]["signed"], true, "{numbers}");
        let m = refused(&mut a, tool, json!({ "doc": broken, "field": field, "keep_open": true }), false);
        assert!(m.contains("byte range") && m.contains(&field), "{numbers}: {m}");
        ok(&mut a, "doc_close", json!({ "doc": broken }));
        assert_eq!(ok(&mut a, "doc_list", json!({}))["documents"].as_array().map(Vec::len), docs_before, "{numbers}");
    }
}
