//! A link's JavaScript action through the tool table (#2081, ported): link_list shows the
//! script, link_run_javascript runs it with ordered answers and reports requests, never
//! carrying them out.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn pdf_of(objs: &[String]) -> Vec<u8> {
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
fn link_run_javascript_answers_menus_and_reports_requests() {
    let script = "var c = app.popUpMenuEx\\({cName:'Red', cReturn:'r'}, {cName:'Blue', cReturn:'b'}\\); getField\\('log'\\).value = c + ':' + event.type; app.openDoc\\('other.pdf'\\);";
    let pdf = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Annot /Subtype /Link /Rect [20 20 180 60] /Border [0 0 0] /A << /S /JavaScript /JS ({script}) >> >>"),
        "<< /Type /Annot /Subtype /Link /Rect [20 80 180 120] /Border [0 0 0] /A << /S /JavaScript /JS (missing\\(\\);) >> >>".into(),
    ]);
    let dir = std::env::temp_dir().join(format!("pdfcraft-link-js-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("link.pdf"), pdf).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "link.pdf" }))["doc"].as_u64().unwrap();
    let info = tool(&mut a, "doc_info", json!({ "doc": doc }));
    assert!(info.to_string().contains("\"javascript\""), "links show their script: {info}");
    let log = |a: &mut Automation| {
        let f = tool(a, "form_fields", json!({ "doc": doc }));
        f["fields"].as_array().unwrap().iter().find(|f| f["name"] == "log").map(|f| f["value"].clone()).unwrap()
    };

    let out = tool(&mut a, "link_run_javascript", json!({ "doc": doc, "page": 1, "index": 2, "answers": ["b"] }));
    assert_eq!(out["error"], Value::Null, "{out}");
    assert_eq!(out["requests"], json!([{ "open_doc": "other.pdf" }]), "{out}");
    assert_eq!(log(&mut a), "b:Link");

    let failed = tool(&mut a, "link_run_javascript", json!({ "doc": doc, "page": 1, "index": 3 }));
    assert!(failed["error"].is_string(), "{failed}");
    assert!(a.call("link_run_javascript", &json!({ "doc": doc, "page": 1, "index": 1 })).is_err(), "a widget is no JavaScript link");
    assert!(a.call("link_run_javascript", &json!({ "doc": doc, "page": 0, "index": 2 })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
