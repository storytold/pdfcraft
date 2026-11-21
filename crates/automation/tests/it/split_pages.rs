//! Integration tests for splitting pages into parts through the automation tool table.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn pdf_of(objs: &[&str]) -> Vec<u8> {
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

#[test]
fn split_parts_vertical_horizontal_and_undo() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-split-pages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 600 400] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ]);
    std::fs::write(dir.join("test.pdf"), bytes).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "test.pdf" }))["doc"].as_u64().unwrap();

    // Split page 1 vertically into 2 parts: now we have 3 pages total (2 from page 1, plus page 2).
    let r = tool(&mut a, "page_split_parts", json!({ "doc": doc, "pages": [1], "direction": "vertical", "parts": 2 }));
    assert_eq!(r["pages"], 3);

    // Undo: back to 2 pages.
    tool(&mut a, "edit_undo", json!({ "doc": doc }));
    let info = tool(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["pages"].as_array().unwrap().len(), 2);

    // Split all pages into 2x2 grid (4 parts each -> total 8 pages).
    let r = tool(&mut a, "page_split_parts", json!({ "doc": doc, "direction": "grid", "rows": 2, "columns": 2 }));
    assert_eq!(r["pages"], 8);

    // Rejects invalid / out-of-bound bounds.
    assert!(a.call("page_split_parts", &json!({ "doc": doc, "parts": -1 })).is_err());
    assert!(a.call("page_split_parts", &json!({ "doc": doc, "parts": 999 })).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}
