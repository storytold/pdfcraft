//! `doc_create_portfolio` (Create a PDF ▸ PDF portfolio): files of any type bundled unchanged
//! into one PDF portfolio, read back through `doc_info`.

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// Call tool `name` and return its JSON result.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A one-page PDF.
const MINIMAL_PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A folder with a one-page PDF, a Word file (any bytes: it is never converted) and an email.
fn case_files() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Complaint.pdf"), MINIMAL_PDF).unwrap();
    std::fs::write(dir.path().join("Lease.docx"), b"PK\x03\x04 not really a word file").unwrap();
    std::fs::write(dir.path().join("Notice.eml"), b"Subject: notice\r\n\r\nhello").unwrap();
    dir
}

#[test]
fn a_portfolio_holds_every_file_unchanged_and_presents_the_one_asked_for() {
    let dir = case_files();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let made = tool(
        &mut a,
        "doc_create_portfolio",
        json!({
            "paths": ["Complaint.pdf", "Lease.docx", "Notice.eml"],
            "title": "Smith v. Jones: Trial Exhibits",
            "note": "Case No. 26-CV-0142",
            "descriptions": ["Filed complaint", "Signed lease", null],
            "dates": ["2026-03-01", "3/15/2025", null],
            "view": "tile",
            "initial": 2,
            "out": "exhibits.pdf",
        }),
    );
    assert!(made["bytes"].as_u64().unwrap() > 0);
    let doc = tool(&mut a, "doc_open", json!({ "path": "exhibits.pdf" }))["doc"].as_u64().unwrap();
    let info = tool(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["portfolio"], json!({ "initial": "Lease.docx", "view": "tile" }));
    let attachments = info["attachments"].as_array().unwrap();
    let find = |name: &str| attachments.iter().find(|x| x["name"] == name).cloned().unwrap_or_else(|| panic!("{name} missing: {attachments:?}"));
    assert_eq!(find("Complaint.pdf")["description"], "Filed complaint");
    assert_eq!(find("Lease.docx")["description"], "Signed lease");
    assert_eq!(find("Notice.eml")["description"], Value::Null);
    // Every file is embedded byte for byte: nothing was converted.
    let embedded = embedded_files(&dir.path().join("exhibits.pdf"));
    for name in ["Complaint.pdf", "Lease.docx", "Notice.eml"] {
        let source = std::fs::read(dir.path().join(name)).unwrap();
        assert_eq!(embedded.iter().find(|(n, _)| n == name).map(|(_, b)| b), Some(&source), "{name} changed");
    }
}

/// Each attachment of the PDF at `path` as (name, bytes), read as the app's Save attachment does.
fn embedded_files(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap();
    let info = pdfcraft_render::inspect(std::sync::Arc::new(bytes.clone()), None).unwrap();
    info.attachments.iter().map(|att| (att.name.clone(), pdfcraft_render::attachment_data(&bytes, None, att).unwrap())).collect()
}

#[test]
fn files_with_the_same_name_stay_apart_on_the_cover_and_in_the_portfolio() {
    // Two exhibits named report.pdf from different folders, and one already called "report.pdf (2)".
    let dir = tempfile::tempdir().unwrap();
    for (folder, body) in [("a", "first"), ("b", "second")] {
        std::fs::create_dir(dir.path().join(folder)).unwrap();
        std::fs::write(dir.path().join(folder).join("report.pdf"), body).unwrap();
    }
    std::fs::write(dir.path().join("report.pdf (2)"), "third").unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    tool(&mut a, "doc_create_portfolio", json!({ "paths": ["a/report.pdf", "b/report.pdf", "report.pdf (2)"], "out": "p.pdf" }));
    let embedded = embedded_files(&dir.path().join("p.pdf"));
    let doc = tool(&mut a, "doc_open", json!({ "path": "p.pdf" }))["doc"].as_u64().unwrap();
    let cover = tool(&mut a, "text_extract", json!({ "doc": doc, "pages": [1] })).to_string();
    // Each exhibit's name on the cover sheet is the attachment holding that exhibit's file.
    for (label, body) in [("Exhibit A", "first"), ("Exhibit B", "second"), ("Exhibit C", "third")] {
        let (name, _) = embedded.iter().find(|(_, b)| b == body.as_bytes()).unwrap_or_else(|| panic!("{body} missing"));
        assert!(cover.contains(&format!("{label}: {name}")), "{label} should name {name}: {cover}");
    }
}

#[test]
fn a_portfolio_with_a_blank_title_is_named_like_its_cover_sheet() {
    let dir = case_files();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let made = tool(&mut a, "doc_create_portfolio", json!({ "paths": ["Notice.eml"], "title": "  " }));
    let name = made["document"]["name"].as_str().unwrap_or_default().to_string();
    assert!(name.starts_with("PDF Portfolio"), "{made}");
}

#[test]
fn a_portfolio_opens_on_its_cover_sheet_by_default() {
    let dir = case_files();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let made = tool(&mut a, "doc_create_portfolio", json!({ "paths": ["Notice.eml", "Complaint.pdf"], "exhibits": "numbers", "prefix": "Tab" }));
    let doc = made["document"]["doc"].as_u64().unwrap();
    let info = tool(&mut a, "doc_info", json!({ "doc": doc }));
    assert_eq!(info["portfolio"], json!({ "initial": null, "view": "details" }));
    // The cover sheet indexes the files under their labels.
    let text = tool(&mut a, "text_extract", json!({ "doc": doc, "pages": [1] }));
    let text = text.to_string();
    assert!(text.contains("Tab 1: Notice.eml"), "{text}");
    assert!(text.contains("Tab 2: Complaint.pdf"), "{text}");
}

#[test]
fn a_portfolio_refuses_what_it_cannot_honour() {
    let dir = case_files();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let mut refuses = |args: Value| match a.call("doc_create_portfolio", &args) {
        Err(ToolError::InvalidArgs(_) | ToolError::Failed(_)) => {}
        other => panic!("expected a refusal for {args}: {other:?}"),
    };
    refuses(json!({ "paths": [] }));
    refuses(json!({ "paths": ["Complaint.pdf"], "descriptions": ["a", "b"] }));
    refuses(json!({ "paths": ["Complaint.pdf"], "exhibits": "roman" }));
    refuses(json!({ "paths": ["Complaint.pdf"], "view": "grid" }));
    refuses(json!({ "paths": ["Complaint.pdf"], "initial": 2 }));
    refuses(json!({ "paths": ["Complaint.pdf"], "dates": ["next Tuesday"] }));
    refuses(json!({ "paths": ["Missing.pdf"] }));
}
