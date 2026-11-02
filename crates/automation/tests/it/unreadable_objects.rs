//! `doc_info` lists the objects of a file that could not be read: each reads as null, so the file
//! opens, but what it held is lost and nothing was repaired.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// A one-page PDF whose document information dictionary (object 4) is `info`.
fn with_info(info: &str) -> Vec<u8> {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
        info,
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn info_of(a: &mut Automation, path: &str) -> Value {
    let doc = match a.call("doc_open", &json!({ "path": path })).unwrap().remove(0) {
        Content::Json(v) => v["doc"].as_u64().unwrap(),
        other => panic!("expected JSON: {other:?}"),
    };
    match a.call("doc_info", &json!({ "doc": doc })).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// An unparseable `/Info` is listed with its number and why, the rest of the file reads, and
/// nothing is reported as repaired.
#[test]
fn doc_info_lists_an_object_that_could_not_be_read() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-unreadable-objects-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.pdf"), with_info("<< /Title (Report) /Author ) >>")).unwrap();
    std::fs::write(dir.join("sound.pdf"), with_info("<< /Title (Report) >>")).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();

    let info = info_of(&mut a, "broken.pdf");
    let lines = info["unreadable_objects"].as_array().unwrap();
    assert_eq!(lines.len(), 1, "{info}");
    let line = lines[0].as_str().unwrap();
    assert!(line.starts_with("object 4 0 could not be read: syntax error"), "{line}");
    assert_eq!(info["repairs"], json!([]), "nothing was repaired");
    assert_eq!(info["pages"].as_array().unwrap().len(), 1, "the rest of the file reads");

    let info = info_of(&mut a, "sound.pdf");
    assert_eq!(info["unreadable_objects"], json!([]), "{info}");
    assert_eq!(info["title"], "Report");
    let _ = std::fs::remove_dir_all(dir);
}
