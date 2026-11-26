//! Page templates through the tools (#1429): list them, add a page from one, show or hide one.

use std::path::PathBuf;

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// One visible page and a hidden template `More` holding the text field `Extra`.
fn form() -> Vec<u8> {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /Templates 6 0 R >> /AcroForm << /Fields [7 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 9 0 R >> >> >>",
        "<< /Length 40 >>\nstream\nBT /F1 12 Tf 20 20 Td (Visible) Tj ET\nendstream",
        "<< /Type /Template /Contents 8 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [7 0 R] >>",
        "<< /Names [(More) 5 0 R] >>",
        "<< /FT /Tx /T (Extra) /Subtype /Widget /Rect [20 100 180 120] /P 5 0 R >>",
        "<< /Length 41 >>\nstream\nBT /F1 12 Tf 20 20 Td (Template) Tj ET\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn setup(test: &str) -> (PathBuf, Automation) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-page-templates-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("form.pdf"), form()).unwrap();
    let a = Automation::new().with_root(&dir).unwrap();
    (dir, a)
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap_or_else(|e| panic!("{tool}: {e:?}")).remove(0) {
        Content::Json(v) => v,
        other => panic!("{tool}: expected JSON: {other:?}"),
    }
}

fn field_names(a: &mut Automation, doc: u64) -> Vec<String> {
    let fields = ok(a, "form_fields", json!({ "doc": doc }));
    let mut names: Vec<String> = fields["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    names
}

#[test]
fn templates_are_listed_spawned_shown_and_hidden_through_tools() {
    let (dir, mut a) = setup("tools");
    let doc = ok(&mut a, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "page_templates", json!({ "doc": doc })), json!({ "templates": [{ "name": "More", "hidden": true }] }));

    // Without `at`, after the last page; renamed unless asked otherwise.
    let spawned = ok(&mut a, "page_template_spawn", json!({ "doc": doc, "name": "More" }));
    assert_eq!(spawned["pages"], 2, "{spawned}");
    assert_eq!(field_names(&mut a, doc), ["Extra", "P1.More.Extra"]);
    ok(&mut a, "page_template_spawn", json!({ "doc": doc, "name": "More", "at": 1, "rename": false }));
    assert_eq!(field_names(&mut a, doc), ["Extra", "P1.More.Extra"], "an unrenamed copy shares Extra");

    let shown = ok(&mut a, "page_template_set_hidden", json!({ "doc": doc, "name": "More", "hidden": false }));
    assert_eq!(shown["pages"], 4, "{shown}");
    assert_eq!(ok(&mut a, "page_templates", json!({ "doc": doc })), json!({ "templates": [{ "name": "More", "hidden": false }] }));
    let hidden = ok(&mut a, "page_template_set_hidden", json!({ "doc": doc, "name": "More", "hidden": true }));
    assert_eq!(hidden["pages"], 3, "{hidden}");

    assert!(matches!(a.call("page_template_spawn", &json!({ "doc": doc, "name": "Nope" })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("page_template_spawn", &json!({ "doc": doc, "name": "More", "at": 0 })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("page_template_set_hidden", &json!({ "doc": doc, "name": "More" })), Err(ToolError::InvalidArgs(_))));
    let _ = std::fs::remove_dir_all(dir);
}

/// `js_run` applies a script's spawn with its other changes, and reports nothing left over.
#[test]
fn a_script_run_through_the_tools_adds_the_page() {
    let (dir, mut a) = setup("js");
    let doc = ok(&mut a, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    let out = ok(&mut a, "js_run", json!({ "doc": doc, "script": "getTemplate('More').spawn(1, false, false);" }));
    assert_eq!(out["error"], Value::Null, "{out}");
    assert_eq!(out["requests"], json!([]), "{out}");
    let info = ok(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["pages"].as_array().map(Vec::len), Some(2), "{info}");
    let _ = std::fs::remove_dir_all(dir);
}
