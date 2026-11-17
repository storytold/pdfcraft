//! The PDF/A gate through the tools: every call that changes, opens or saves a document that
//! declares PDF/A says what the document's changes break (`pdfa`), and names each rule a change
//! breaks once in `warnings`. A document that declares no PDF/A part gets neither.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// A one-page PDF 2.0 file; `ids` (if any) are the attributes of its XMP identification.
fn file(ids: Option<&str>) -> Vec<u8> {
    let (meta, xmp) = match ids {
        Some(ids) => {
            let xmp = format!(
                "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
                 <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" {ids}/></rdf:RDF></x:xmpmeta>"
            );
            (" /Metadata 4 0 R", format!("4 0 obj << /Type /Metadata /Subtype /XML /Length {} >> stream\n{xmp}\nendstream endobj\n", xmp.len()))
        }
        None => ("", String::new()),
    };
    format!(
        "%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R{meta} >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
{xmp}trailer << /Root 1 0 R >>
%%EOF"
    )
    .into_bytes()
}

const PDFA4: &str = "pdfaid:part=\"4\" pdfaid:rev=\"2020\"";

fn setup(ids: Option<&str>) -> (tempfile::TempDir, Automation, u64, Value) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.pdf"), file(ids)).unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello\n").unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let opened = tool(&mut a, "doc_open", json!({ "path": "in.pdf" }));
    let doc = opened["doc"].as_u64().unwrap();
    (dir, a, doc, opened)
}

/// The result's sentences that come from the PDF/A gate.
fn pdfa_warnings(v: &Value) -> Vec<String> {
    let all = v["warnings"].as_array().cloned().unwrap_or_default();
    all.iter().filter_map(Value::as_str).filter(|w| w.contains("PDF/A")).map(str::to_string).collect()
}

#[test]
fn every_change_to_a_pdfa4_file_says_what_it_breaks() {
    let (dir, mut a, doc, opened) = setup(Some(PDFA4));
    // Opening says what the file declares; nothing is broken yet.
    assert_eq!(opened["pdfa"], json!([{ "doc": doc, "declares": "PDF/A-4", "checked_as": "PDF/A-4", "breaks": [] }]), "{opened}");
    assert!(pdfa_warnings(&opened).is_empty(), "{opened}");

    let rotated = tool(&mut a, "page_rotate", json!({ "doc": doc, "degrees": 90 }));
    assert_eq!(rotated["pdfa"], json!([{ "doc": doc, "declares": "PDF/A-4", "checked_as": "PDF/A-4", "breaks": [] }]), "{rotated}");
    assert!(pdfa_warnings(&rotated).is_empty(), "{rotated}");

    // ISO 19005-4 6.9: an embedded file of a PDF/A-4 file is a PDF/A file itself, and says
    // how it relates to the document.
    let added = tool(&mut a, "attachment_add", json!({ "doc": doc, "path": "note.txt" }));
    let breaks = added["pdfa"][0]["breaks"].as_array().unwrap().clone();
    assert!(!breaks.is_empty() && breaks.iter().all(|b| b["clause"] == "6.9"), "{added}");
    let warned = pdfa_warnings(&added);
    assert_eq!(warned.len(), breaks.len(), "{added}");
    assert!(warned.iter().all(|w| w.starts_with("PDF/A-4: this change breaks ISO 19005-4 6.9: ")), "{warned:?}");

    // A later change that breaks nothing more lists them again, and warns of nothing new.
    let again = tool(&mut a, "page_rotate", json!({ "doc": doc, "degrees": 90 }));
    assert_eq!(again["pdfa"][0]["breaks"].as_array().unwrap().len(), breaks.len(), "{again}");
    assert!(pdfa_warnings(&again).is_empty(), "{again}");

    // The save writes them into the file: said once more.
    let saved = tool(&mut a, "doc_save", json!({ "doc": doc, "path": "out.pdf" }));
    let said = pdfa_warnings(&saved);
    assert_eq!(said.len(), 1, "{saved}");
    assert!(said[0].starts_with(&format!("the saved file declares PDF/A-4 but breaks {} of its rules", breaks.len())), "{said:?}");
    assert!(said[0].ends_with("(ISO 19005-4 6.9)"), "{said:?}");

    // An optimized copy is checked as it is written.
    let copy = tool(&mut a, "doc_optimize", json!({ "doc": doc, "path": "small.pdf" }));
    assert_eq!(copy["pdfa"][0]["copy"], json!(dir.path().join("small.pdf").canonicalize().unwrap().to_string_lossy()), "{copy}");
    assert!(pdfa_warnings(&copy).iter().any(|w| w.starts_with("the copy declares PDF/A-4 but breaks")), "{copy}");

    // Undo takes the attachment back, and with it what it broke.
    tool(&mut a, "edit_undo", json!({ "doc": doc }));
    let undone = tool(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(undone["pdfa"][0]["breaks"], json!([]), "{undone}");
    // Redone, it is new again.
    let redone = tool(&mut a, "edit_redo", json!({ "doc": doc }));
    assert_eq!(pdfa_warnings(&redone).len(), breaks.len(), "{redone}");
}

#[test]
fn a_tool_run_through_a_command_reports_once() {
    let (_dir, mut a, doc, _) = setup(Some(PDFA4));
    let added = tool(&mut a, "command_run", json!({ "id": "edit.attach", "params": { "doc": doc, "path": "note.txt" } }));
    let breaks = added["pdfa"][0]["breaks"].as_array().map(Vec::len).unwrap_or_default();
    assert!(breaks > 0 && pdfa_warnings(&added).len() == breaks, "{added}");
    let saved = tool(&mut a, "command_run", json!({ "id": "file.save", "params": { "doc": doc, "path": "out.pdf" } }));
    assert_eq!(saved["pdfa"].as_array().map(Vec::len), Some(1), "{saved}");
    assert_eq!(pdfa_warnings(&saved).len(), 1, "one save, said once: {saved}");
    let batch = tool(&mut a, "command_batch", json!({ "steps": [{ "id": "file.save", "params": { "doc": doc, "path": "out2.pdf" } }] }));
    assert_eq!(batch["pdfa"].as_array().map(Vec::len), Some(1), "{batch}");
    assert_eq!(pdfa_warnings(&batch).len(), 1, "{batch}");
}

/// The saved-file notice is about the bytes the save wrote, even when the same call changes the
/// document again after saving.
#[test]
fn the_saved_file_notice_is_about_the_file_written() {
    let saved_notice = |v: &Value| pdfa_warnings(v).into_iter().filter(|w| w.starts_with("the saved file")).collect::<Vec<_>>();
    // Saved, then broken: the file written breaks nothing.
    let (_dir, mut a, doc, _) = setup(Some(PDFA4));
    let steps = json!([
        { "id": "file.save", "params": { "doc": doc, "path": "out.pdf" } },
        { "id": "edit.attach", "params": { "doc": doc, "path": "note.txt" } },
    ]);
    let batch = tool(&mut a, "command_batch", json!({ "steps": steps }));
    assert!(!batch["pdfa"][0]["breaks"].as_array().unwrap().is_empty(), "the document breaks rules now: {batch}");
    assert!(saved_notice(&batch).is_empty(), "{batch}");
    // Broken, saved, then undone: the file written breaks them.
    let (_dir, mut a, doc, _) = setup(Some(PDFA4));
    let steps = json!([
        { "id": "edit.attach", "params": { "doc": doc, "path": "note.txt" } },
        { "id": "file.save", "params": { "doc": doc, "path": "out.pdf" } },
        { "id": "edit.undo", "params": { "doc": doc } },
    ]);
    let batch = tool(&mut a, "command_batch", json!({ "steps": steps }));
    assert_eq!(batch["pdfa"][0]["breaks"], json!([]), "the document breaks nothing now: {batch}");
    let said = saved_notice(&batch);
    assert_eq!(said.len(), 1, "{batch}");
    assert!(said[0].ends_with("(ISO 19005-4 6.9)"), "{said:?}");
}

/// The saved-file notice matches an issue on a page to the page it was on when the file was
/// written, not to the pages an undo after the save brought back.
#[test]
fn the_saved_file_is_checked_with_its_own_pages() {
    // A PDF/A-2b file of two pages whose second has additional actions (ISO 19005-2 6.5.2).
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
               <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" pdfaid:part=\"2\" pdfaid:conformance=\"B\"/>\
               </rdf:RDF></x:xmpmeta>";
    let bytes = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Metadata 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /AA << /O << /S /JavaScript /JS (1) >> >> >> endobj
5 0 obj << /Type /Metadata /Subtype /XML /Length {} >> stream\n{xmp}\nendstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        xmp.len()
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.pdf"), bytes).unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap();
    let opened = tool(&mut a, "doc_open", json!({ "path": "in.pdf" }));
    let doc = opened["doc"].as_u64().unwrap();
    // Page 1 deleted, saved, and the deletion undone: the file written has the issue on its page
    // 1, the one that was page 2, so it breaks nothing new.
    let steps = json!([
        { "id": "page.delete", "params": { "doc": doc, "pages": [1] } },
        { "id": "file.save", "params": { "doc": doc, "path": "out.pdf" } },
        { "id": "edit.undo", "params": { "doc": doc } },
    ]);
    let batch = tool(&mut a, "command_batch", json!({ "steps": steps }));
    assert_eq!(batch["pdfa"][0]["declares"], "PDF/A-2b", "{batch}");
    let saved: Vec<String> = pdfa_warnings(&batch).into_iter().filter(|w| w.starts_with("the saved file")).collect();
    assert!(saved.is_empty(), "{batch}");
}

#[test]
fn a_part_pdfcraft_does_not_check_is_said_once() {
    let (_dir, mut a, doc, opened) = setup(Some("pdfaid:part=\"1\" pdfaid:conformance=\"B\""));
    // Said when the file is opened: the first result about it.
    assert_eq!(opened["pdfa"][0]["checked_as"], Value::Null, "{opened}");
    assert_eq!(pdfa_warnings(&opened).len(), 1, "{opened}");
    let first = tool(&mut a, "page_rotate", json!({ "doc": doc, "degrees": 90 }));
    assert_eq!(first["pdfa"][0]["declares"], "PDF/A-1b", "{first}");
    assert!(pdfa_warnings(&first).is_empty(), "said at doc_open already: {first}");
}

#[test]
fn a_file_that_declares_no_pdfa_gets_no_pdfa_results() {
    let (_dir, mut a, doc, opened) = setup(None);
    assert!(opened.get("pdfa").is_none(), "{opened}");
    for (name, args) in [
        ("page_rotate", json!({ "doc": doc, "degrees": 90 })),
        ("attachment_add", json!({ "doc": doc, "path": "note.txt" })),
        ("doc_save", json!({ "doc": doc, "path": "out.pdf" })),
        ("doc_optimize", json!({ "doc": doc, "path": "small.pdf" })),
        ("edit_undo", json!({ "doc": doc })),
    ] {
        let out = tool(&mut a, name, args);
        assert!(out.get("pdfa").is_none() && out.get("document").and_then(|d| d.get("pdfa")).is_none(), "{name}: {out}");
        assert!(pdfa_warnings(&out).is_empty(), "{name}: {out}");
    }
}
