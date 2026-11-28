//! A file attached as a comment on a page is stored like the same file attached to the document:
//! its embedded file stream carries the same media type (`/Subtype`), and its date.

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// The embedded file stream's dictionary that the file specification `fs` points at.
fn embedded(d: &Document, fs: &Object) -> Dict {
    let ef = dict(d, fs).get(b"EF").unwrap().clone();
    let file = dict(d, &ef).get(b"F").unwrap().clone();
    assert!(matches!(file, Object::Ref(_)), "the embedded file is an indirect stream: {file:?}");
    dict(d, &file)
}

fn dict(d: &Document, o: &Object) -> Dict {
    d.resolve(o).as_dict().unwrap().clone()
}

fn name_of(d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).and_then(Object::as_name).map(|n| String::from_utf8_lossy(n).into_owned())
}

#[test]
fn a_page_attachment_keeps_the_media_type_and_date_a_document_attachment_gets() {
    let dir = tempfile::tempdir().unwrap();
    let session = pdfcraft_engine::Session::new();
    let bytes = session.create_blank(300.0, 400.0, 1).unwrap();
    std::fs::write(dir.path().join("a.pdf"), bytes.as_slice()).unwrap();
    std::fs::write(dir.path().join("table.csv"), b"a,b\n1,2\n").unwrap();

    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut auto, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    tool(&mut auto, "attachment_add", json!({ "doc": doc, "path": "table.csv" }));
    tool(&mut auto, "comment_add", json!({ "doc": doc, "page": 1, "type": "attachment", "at": [40, 300], "path": "table.csv" }));
    tool(&mut auto, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));

    let d = Document::open(Arc::new(std::fs::read(dir.path().join("out.pdf")).unwrap())).unwrap();
    let catalog = dict(&d, &Object::Ref(d.root().unwrap()));
    let names = dict(&d, catalog.get(b"Names").unwrap()).get(b"EmbeddedFiles").unwrap().clone();
    let pairs = dict(&d, &names).get(b"Names").unwrap().clone();
    let doc_fs = d.resolve(&pairs).as_array().unwrap()[1].clone();
    let doc_file = embedded(&d, &doc_fs);

    let page = pdfcraft_model::pages(&d).remove(0);
    let annots = d.resolve(page.dict.get(b"Annots").unwrap()).as_array().unwrap().clone();
    let annot =
        annots.iter().map(|a| dict(&d, a)).find(|a| name_of(a, b"Subtype").as_deref() == Some("FileAttachment")).expect("the comment is on the page");
    let page_file = embedded(&d, annot.get(b"FS").unwrap());

    let doc_type = name_of(&doc_file, b"Subtype");
    assert!(doc_type.is_some(), "the document attachment of a .csv carries a media type: {doc_file:?}");
    assert_eq!(name_of(&page_file, b"Subtype"), doc_type, "the page attachment carries the same one: {page_file:?}");

    // Its date: the embedded file's modification date is the comment's own date.
    let params = |f: &Dict| dict(&d, f.get(b"Params").unwrap());
    let date = |o: Option<&Object>| o.and_then(Object::as_string).map(|s| s.to_text());
    let page_date = date(params(&page_file).get(b"ModDate"));
    assert!(page_date.is_some(), "the page attachment is dated: {page_file:?}");
    assert_eq!(page_date, date(annot.get(b"M")), "by the comment's date");
    assert_eq!(params(&page_file).get(b"Size"), params(&doc_file).get(b"Size"), "and both measure the same file");
}
