//! Search-and-redact of page text drawn after a form XObject with more `Q` than `q`. Find's boxes
//! come from the renderer's text and redaction measures the content itself, so the two must agree
//! that the form's stray `Q`s do not undo the page's `cm` (vendored hayro-interpret patch 68).

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

const CONTENT: &str = "q 1 0 0 1 20 -100 cm BT /F1 12 Tf 72 700 Td (AAAA) Tj ET /X Do \
                       BT /F1 12 Tf 72 680 Td (SECRET) Tj ET Q";
const FORM: &str = "BT /F1 12 Tf 200 0 Td ET Q Q Q";

fn pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << \
         /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /XObject << /X 5 0 R >> >> >>"
            .to_owned(),
        format!("<< /Length {} >>\nstream\n{CONTENT}\nendstream", CONTENT.len()),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Length {} >>\nstream\n{FORM}\nendstream", FORM.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
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

fn text(a: &mut Automation, doc: &Value) -> String {
    ok(a, "text_extract", json!({ "doc": doc }))["pages"][0]["text"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn find_and_redact_removes_text_drawn_after_a_form_with_stray_q() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redact-stray-q-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("in.pdf"), pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].clone();
    assert_eq!(ok(&mut a, "redact_mark", json!({ "doc": doc, "find": "SECRET" }))["marked"], 1);
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf", "full": true }));
    let saved = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].clone();
    let after = text(&mut a, &saved);
    assert!(!after.contains("SECRET"), "{after:?}");
    assert!(after.contains("AAAA"), "the text before the mark stays: {after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
