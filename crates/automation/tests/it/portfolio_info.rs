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

/// A one-page portfolio of `notes.txt` and `form.pdf`, a two-page PDF, with a cross-reference
/// table, so the PDF inside isn't read as the portfolio's own objects.
fn portfolio_of_files() -> Vec<u8> {
    let form = "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Collection << /Type /Collection /D (form.pdf) >> \
         /Names << /EmbeddedFiles << /Names [(form.pdf) 7 0 R (notes.txt) 5 0 R] >> >> >>"
            .to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /EmbeddedFile /Length 5 >>\nstream\nhello\nendstream".into(),
        "<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 4 0 R >> >>".into(),
        format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n{form}\nendstream", form.len()),
        "<< /Type /Filespec /F (form.pdf) /UF (form.pdf) /EF << /F 6 0 R >> >>".into(),
    ];
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

/// `attachment_open` opens a portfolio's PDF as a document of its own, by key or by name, and
/// refuses a file that isn't a PDF; the portfolio is left as it was.
#[test]
fn attachment_open_reads_a_portfolios_pdf_as_a_document_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("portfolio.pdf"), portfolio_of_files()).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let portfolio = tool(&mut auto, "doc_open", json!({ "path": "portfolio.pdf" }));
    let doc = portfolio["doc"].as_u64().unwrap();
    for by in [json!({ "doc": doc, "key": "form.pdf" }), json!({ "doc": doc, "name": "form.pdf" })] {
        let opened = tool(&mut auto, "attachment_open", by);
        assert_ne!(opened["doc"], json!(doc), "a document of its own");
        assert_eq!(opened["pages"], json!(2), "the attached PDF's pages, not the cover's");
        assert_eq!(opened["from"], json!({ "doc": doc, "key": "form.pdf" }));
    }
    let refused = auto.call("attachment_open", &json!({ "doc": doc, "key": "notes.txt" })).unwrap_err().to_string();
    assert!(refused.contains("not a PDF"), "{refused}");
    assert!(auto.call("attachment_open", &json!({ "doc": doc })).is_err(), "a key or a name is needed");
    assert_eq!(portfolio["pages"], json!(1), "the portfolio's own page is its cover sheet");
    let list = tool(&mut auto, "doc_list", json!({}));
    let listed = list.as_array().or_else(|| list["documents"].as_array()).cloned().unwrap_or_default();
    let source = listed.iter().find(|d| d["doc"] == json!(doc)).expect("the portfolio stays open");
    assert_eq!(source["dirty"], json!(false), "the portfolio is unchanged");
}
