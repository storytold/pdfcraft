//! Through the tools, a file that declares PDF/A-4 keeps its metadata in XMP: `doc_set_info` and
//! `doc_save` write no document information dictionary (ISO 19005-4 §6.1.3), and `doc_info` reads
//! the title back from XMP.

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A one-page PDF 2.0 file whose XMP declares PDF/A-4, with no `/Info`.
fn pdfa4() -> Vec<u8> {
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
               <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"4\" pdfaid:rev=\"2020\"/>\
               </rdf:RDF></x:xmpmeta>";
    format!(
        "%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R /Metadata 4 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Metadata /Subtype /XML /Length {} >> stream
{xmp}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        xmp.len()
    )
    .into_bytes()
}

#[test]
fn doc_set_info_and_doc_save_keep_a_pdfa4_files_metadata_in_xmp() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.pdf"), pdfa4()).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "in.pdf" }))["doc"].as_u64().unwrap();
    tool(&mut a, "doc_set_info", json!({ "doc": doc, "key": "Title", "value": "Annual report" }));
    assert!(a.call("doc_set_info", &json!({ "doc": doc, "key": "Project", "value": "Apollo" })).is_err(), "no custom property in PDF/A-4");
    for (out, full) in [("inc.pdf", false), ("full.pdf", true)] {
        tool(&mut a, "doc_save", json!({ "doc": doc, "path": out, "full": full }));
        let bytes = Arc::new(std::fs::read(dir.path().join(out)).unwrap());
        let saved = pdfcraft_cos::Document::open(bytes).unwrap();
        assert!(saved.trailer().get(b"Info").is_none(), "{out}: no /Info");
        let reopened = tool(&mut a, "doc_open", json!({ "path": out }))["doc"].as_u64().unwrap();
        let info = tool(&mut a, "doc_info", json!({ "doc": reopened }));
        assert_eq!(info["title"].as_str(), Some("Annual report"), "{out}");
    }
}
