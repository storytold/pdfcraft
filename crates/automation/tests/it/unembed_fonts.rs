//! `doc_unembed_fonts` end to end, on a PDF built here with a stand-in font program.

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions, Stream, write_full};
use serde_json::{Value, json};

/// Bytes of the stand-in program: nothing parses them, so text is drawn with a substitute
/// before and after.
const PROGRAM: usize = 60_000;

fn fixture() -> Vec<u8> {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let data: Vec<u8> = (0..PROGRAM).map(|i| (i * 13 % 251) as u8).collect();
    let program = doc.add(Object::Stream(Stream::from_raw(Dict::new(), data)));
    let descriptor = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("FontDescriptor")),
        (b"FontName".to_vec(), Object::name("KLMNOP+Bookish")),
        (b"Flags".to_vec(), 32.into()),
        (b"FontBBox".to_vec(), Object::Array(vec![0.into(), (-200).into(), 1000.into(), 900.into()])),
        (b"ItalicAngle".to_vec(), 0.into()),
        (b"Ascent".to_vec(), 900.into()),
        (b"Descent".to_vec(), (-200).into()),
        (b"CapHeight".to_vec(), 700.into()),
        (b"StemV".to_vec(), 80.into()),
        (b"FontFile2".to_vec(), Object::Ref(program)),
    ])));
    let widths: Vec<Object> = (32..=126).map(|_| 500.into()).collect();
    let font = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("TrueType")),
        (b"BaseFont".to_vec(), Object::name("KLMNOP+Bookish")),
        (b"FirstChar".to_vec(), 32.into()),
        (b"LastChar".to_vec(), 126.into()),
        (b"Widths".to_vec(), Object::Array(widths)),
        (b"Encoding".to_vec(), Object::name("WinAnsiEncoding")),
        (b"FontDescriptor".to_vec(), Object::Ref(descriptor)),
    ])));
    let content = doc.add(Object::Stream(Stream::flate(Dict::new(), b"BT /F1 24 Tf 72 700 Td (Stays searchable) Tj ET")));
    let page = doc.add(Object::Dict(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(pages)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()])),
        (
            b"Resources".to_vec(),
            Object::Dict(Dict::from_iter([(b"Font".to_vec(), Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Ref(font))])))])),
        ),
        (b"Contents".to_vec(), Object::Ref(content)),
    ])));
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::from(1i64));
    })
    .unwrap();
    // An output intent, as a PDF/X file carries: unembedding a font breaks that conformance.
    let intent = Dict::from_iter([
        (b"Type".to_vec(), Object::name("OutputIntent")),
        (b"S".to_vec(), Object::name("GTS_PDFX")),
        (b"OutputConditionIdentifier".to_vec(), Object::String(PdfString::text("Test press"))),
    ]);
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"OutputIntents".to_vec(), Object::Array(vec![Object::Dict(intent)]))).unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    let result = a.call(tool, &args).unwrap();
    let Content::Json(value) = &result[0] else { panic!("expected JSON from {tool}") };
    value.clone()
}

fn text(a: &mut Automation, doc: u64) -> String {
    call(a, "text_extract", json!({ "doc": doc }))["pages"][0]["text"].as_str().unwrap_or_default().to_string()
}

#[test]
fn unembedding_shrinks_the_saved_file_and_keeps_the_text() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.pdf");
    std::fs::write(&input, fixture()).unwrap();
    let before = std::fs::metadata(&input).unwrap().len();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "input.pdf" }))["doc"].as_u64().unwrap();
    let words = text(&mut a, doc);

    let preview = call(&mut a, "doc_unembed_fonts", json!({ "doc": doc, "dry_run": true }));
    assert_eq!(preview["unembedded"], 1, "{preview}");
    assert_eq!(preview["fonts"][0]["renamed_to"], "Bookish", "{preview}");
    let loses = preview["loses_conformance"].as_array().map(Vec::len).unwrap_or_default();
    assert!(loses > 0, "the output intent's conformance is not reported as lost: {preview}");
    let again = call(&mut a, "doc_unembed_fonts", json!({ "doc": doc, "dry_run": true }));
    assert_eq!(again["unembedded"], 1, "a dry run changed the document: {again}");

    let missing = a.call("doc_unembed_fonts", &json!({ "doc": doc, "fonts": ["Missing"] }));
    assert!(matches!(missing, Err(ToolError::Failed(_))), "a name no font has must fail: {missing:?}");

    let done = call(&mut a, "doc_unembed_fonts", json!({ "doc": doc, "fonts": ["Bookish"] }));
    assert_eq!(done["unembedded"], 1, "{done}");
    assert_eq!(text(&mut a, doc), words, "unembedding changed the page's text");
    let left = call(&mut a, "doc_unembed_fonts", json!({ "doc": doc, "dry_run": true }));
    assert_eq!(left["fonts"][0]["outcome"], "not-embedded", "{left}");

    // Saving over the file must rewrite it: an incremental update would keep the program.
    call(&mut a, "doc_save", json!({ "doc": doc }));
    let after = std::fs::metadata(&input).unwrap().len();
    assert!(after + (PROGRAM as u64) / 2 < before, "the saved file kept the program: {before} -> {after} bytes");

    call(&mut a, "edit_undo", json!({ "doc": doc }));
    let undone = call(&mut a, "doc_unembed_fonts", json!({ "doc": doc, "dry_run": true }));
    assert_eq!(undone["unembedded"], 1, "undo did not bring the program back: {undone}");
}
