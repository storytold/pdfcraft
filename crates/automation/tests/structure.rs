//! `structure_validate`: the Arlington PDF Model check through the tool table.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// One page whose catalog says `/PageLayout /Spiral` and carries a key the model doesn't define.
fn document() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /Spiral /PdfCraftNote 1 >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
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

#[test]
fn structure_validate_reports_findings_and_filters() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-structure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.pdf"), document()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = ok(&mut a, "doc_open", json!({ "path": "s.pdf" }))["doc"].as_u64().unwrap();

    let r = ok(&mut a, "structure_validate", json!({ "doc": doc }));
    assert_eq!((r["valid"].clone(), r["errors"].clone(), r["infos"].clone()), (json!(false), json!(1), json!(1)), "{r}");
    assert_eq!(r["version"], "1.7");
    assert!(r["model"].as_str().unwrap().contains("d37f4c8b"), "{r}");
    let layout = r["findings"].as_array().unwrap().iter().find(|f| f["key"] == "PageLayout").expect("the layout finding");
    assert_eq!(layout["severity"], "error");
    assert_eq!(layout["rule"], "value");
    assert_eq!(layout["type"], "Catalog");
    assert_eq!(layout["object"], "1 0 R");
    assert_eq!(layout["path"], "trailer/Root");
    assert!(layout["message"].as_str().unwrap().contains("/Spiral"), "{layout}");
    assert!(r["not_checked"].as_array().unwrap().iter().all(|n| n["count"].as_u64().unwrap() > 0));

    // Filters: the list narrows, the counts stay.
    let errors_only = ok(&mut a, "structure_validate", json!({ "doc": doc, "severity": "error" }));
    assert!(errors_only["findings"].as_array().unwrap().iter().all(|f| f["severity"] == "error"));
    assert_eq!(errors_only["infos"], 1);
    let no_unknown = ok(&mut a, "structure_validate", json!({ "doc": doc, "unknown_keys": false }));
    assert_eq!(no_unknown["infos"], 0);
    let capped = ok(&mut a, "structure_validate", json!({ "doc": doc, "max_findings": 1 }));
    assert_eq!(capped["errors"], 1);

    // Arguments an agent can get wrong come back as errors it can act on.
    for bad in [
        json!({ "doc": doc, "severity": "loud" }),
        json!({ "doc": doc, "max_findings": 0 }),
        json!({ "doc": doc, "max_findings": 9999 }),
        json!({ "doc": 99 }),
    ] {
        assert!(a.call("structure_validate", &bad).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
