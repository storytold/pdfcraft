//! `doc_info` reports a PDF portfolio (`/Collection`, ISO 32000-2 §7.11.6): the attachment it
//! presents first and how its file list first shows.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Call tool `name` and return its JSON result.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A one-page portfolio of `cover.txt` and `form.pdf` with `collection` as its `/Collection`
/// entries, or none at all.
fn portfolio(collection: Option<&str>) -> Vec<u8> {
    let collection = collection.map(|c| format!("/Collection << /Type /Collection {c} >>")).unwrap_or_default();
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R {collection} /Names << /EmbeddedFiles << /Names [(cover.txt) 5 0 R (form.pdf) 7 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /EmbeddedFile /Length 5 >> stream
hello
endstream endobj
5 0 obj << /Type /Filespec /F (cover.txt) /UF (cover.txt) /EF << /F 4 0 R >> >> endobj
6 0 obj << /Type /EmbeddedFile /Length 8 >> stream
%PDF-1.7
endstream endobj
7 0 obj << /Type /Filespec /F (form.pdf) /UF (form.pdf) /EF << /F 6 0 R >> >> endobj
trailer << /Root 1 0 R >>
%%EOF"
    )
    .into_bytes()
}

/// `doc_info` names a portfolio's initial document and view, and reports none for a plain file.
#[test]
fn doc_info_names_the_document_a_portfolio_presents_first() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tile.pdf"), portfolio(Some("/D (form.pdf) /View /T"))).unwrap();
    std::fs::write(dir.path().join("cover.pdf"), portfolio(Some(""))).unwrap();
    std::fs::write(dir.path().join("plain.pdf"), portfolio(None)).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let mut info = |path: &str| {
        let doc = tool(&mut auto, "doc_open", json!({ "path": path }))["doc"].as_u64().unwrap();
        tool(&mut auto, "doc_info", json!({ "doc": doc }))["portfolio"].clone()
    };
    assert_eq!(info("tile.pdf"), json!({ "initial": "form.pdf", "view": "tile" }));
    // Without `/D` the cover sheet is the initial document.
    assert_eq!(info("cover.pdf"), json!({ "initial": null, "view": "details" }));
    assert_eq!(info("plain.pdf"), Value::Null);
}
