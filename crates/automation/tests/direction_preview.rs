//! Preview and Apply share the text selected for implicit paragraph reflow.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn page() -> Vec<u8> {
    let content = "BT /F1 12 Tf 14.4 TL 1 0 0 1 72 700 Tm (Alpha beta gamma delta epsilon) Tj T* (zeta eta theta) Tj T* (Tail) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn call(auto: &mut Automation, name: &str, args: Value) -> Value {
    let result = auto.call(name, &args).unwrap();
    let Content::Json(value) = result.into_iter().next().unwrap() else { panic!("JSON result") };
    value
}

#[test]
fn implicit_direction_reflow_preview_matches_explicit_text_and_apply() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wrapped.pdf"), page()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut auto, "doc_open", json!({"path":"wrapped.pdf"}))["doc"].clone();
    let paragraphs = call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}));
    assert_eq!(paragraphs["paragraphs"].as_array().unwrap().len(), 1);
    let block = &paragraphs["paragraphs"][0];
    let reflow = auto.session().docs().first().unwrap().text_blocks(0).remove(0).reflow_text;
    assert_eq!(reflow.matches('\n').count(), 1, "retain a hard break after a soft wrap");
    assert_eq!(block["editor_text"].as_str().unwrap().matches('\n').count(), 2);
    assert_ne!(block["text"], reflow, "exercise explicit reflow rather than unchanged source text");
    // Forced direction across source rows requires a uniform complete font face; the
    // standard-font fixture isolates text selection without adding that dependency.
    let implicit = call(&mut auto, "text_edit_layout", json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto"}));
    let explicit = call(&mut auto, "text_edit_layout", json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto","text":reflow}));
    assert_eq!(implicit, explicit, "preview must reflow the same text as Apply");
    assert_eq!(call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1})), paragraphs, "preview must not edit the document");
    call(&mut auto, "text_edit", json!({"doc":doc,"page":1,"paragraph":1,"width":400,"direction":"auto"}));
    let applied = call(&mut auto, "text_paragraphs", json!({"doc":doc,"page":1}));
    assert_eq!(applied["paragraphs"][0]["editor_text"], reflow);
}
