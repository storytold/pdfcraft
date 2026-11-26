//! Search-and-redact of text that follows a `q`/`Q` inside a text object. `Q` restores the text
//! matrices there (ISO 32000-2 §9.4.1 as corrected by the PDF Association's errata, issue 368), so
//! "SECRET" is drawn from the pen position before `q`, over the "B". Find's boxes come from the
//! renderer's text, which places it there; redaction measures the content itself, and while it
//! carried the pen across `Q` the two disagreed and `redact_apply` refused to change anything.
use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// The content, the text that stays after SECRET is redacted, and the text that goes with it.
const CASES: [(&str, &str, &str); 3] = [
    // A larger font inside `q`/`Q`: the font and the pen are both restored, so SECRET lies over
    // the B and the B goes with it.
    ("BT /F1 12 Tf 72 700 Td (AAAA) Tj q /F2 24 Tf (B) Tj Q (SECRET) Tj ET", "AAAA", "B"),
    // A move inside `q`/`Q` is undone.
    ("BT /F1 12 Tf 72 700 Td (AA) Tj q 100 0 Td Q (SECRET) Tj ET", "AA", "SECRET"),
    // The text line matrix is restored too: `T*` starts the next line from the line before `q`.
    ("BT /F1 12 Tf 14 TL 72 700 Td (AA) Tj q 100 0 Td Q T* (SECRET) Tj ET", "AA", "SECRET"),
];

fn pdf(content: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << \
         /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> \
         /F2 << /Type /Font /Subtype /Type1 /BaseFont /Courier >> >> >> >>"
            .to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut out = b"%PDF-2.0\n".to_vec();
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
fn find_and_redact_removes_text_after_a_q_q_inside_its_text_object() {
    for (case, (content, kept, gone)) in CASES.iter().enumerate() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-redact-after-q-{}-{case}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in.pdf"), pdf(content)).unwrap();
        let mut a = Automation::new().with_root(&dir).unwrap();
        let doc = ok(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].clone();
        assert_eq!(ok(&mut a, "redact_mark", json!({ "doc": doc, "find": "SECRET" }))["marked"], 1, "{content}");
        ok(&mut a, "redact_apply", json!({ "doc": doc }));
        ok(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf", "full": true }));
        let saved = ok(&mut a, "doc_open", json!({ "path": "out.pdf" }))["doc"].clone();
        let after = text(&mut a, &saved);
        assert!(!after.contains("SECRET") && !after.contains(gone), "{content}: {after:?}");
        assert!(after.contains(kept), "{content}: the text before the mark stays: {after:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
