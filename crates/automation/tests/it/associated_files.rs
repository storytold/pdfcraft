//! PDF 2.0 associated files (`/AF`, ISO 32000-2 §14.13): `doc_info` lists those of the document,
//! its pages and their annotations, with the entries it leaves out and why, and
//! `associated_file_save` writes one to a file inside the root.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

/// Call tool `name` and return its JSON result.
fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

/// Call tool `name`, which must fail, and return its message.
fn refusal(a: &mut Automation, name: &str, args: Value) -> String {
    a.call(name, &args).expect_err("the call is refused").to_string()
}

/// Two pages: the catalog and page 1 share one file by object, page 2's comment writes one
/// directly, and the catalog's `/AF` also holds a dictionary without `/EF`, a reference to itself
/// and a file whose stream uses a filter nothing decodes.
const PDF: &[u8] = b"%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R /AF [10 0 R 11 0 R 12 0 R 13 0 R] >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /AF [10 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 10 10] /AF [<< /Type /Filespec /UF (data.csv) /AFRelationship /Alternative /EF << /F 21 0 R >> >>] >> endobj
10 0 obj << /Type /Filespec /F (invoice.xml) /Desc (Invoice data) /AFRelationship /Data /EF << /F 20 0 R >> >> endobj
11 0 obj << /Type /Filespec /F (external.xml) >> endobj
12 0 obj 12 0 R endobj
13 0 obj << /Type /Filespec /F (packed.bin) /AFRelationship /Schema /EF << /F 22 0 R >> >> endobj
20 0 obj << /Type /EmbeddedFile /Subtype /text#2Fxml /Params << /Size 9 >> /Filter /ASCIIHexDecode /Length 19 >> stream
3C696E766F6963653E>
endstream endobj
21 0 obj << /Type /EmbeddedFile /Subtype /text#2Fcsv /Length 5 >> stream
a,b,c
endstream endobj
22 0 obj << /Type /EmbeddedFile /Filter /NoSuchDecode /Length 4 >> stream
data
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn associated_files_are_listed_and_saved_through_the_tools() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("af.pdf"), PDF).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut auto, "doc_open", json!({ "path": "af.pdf" }))["doc"].as_u64().unwrap();
    let info = tool(&mut auto, "doc_info", json!({ "doc": doc }));
    assert_eq!(
        info["associated_files"],
        json!([
            { "file": 1, "name": "invoice.xml", "relationship": "Data", "standard_relationship": true, "description": "Invoice data",
              "mime": "text/xml", "size": 9, "owners": [{ "owner": "document" }, { "owner": "page", "page": 1 }] },
            { "file": 2, "name": "packed.bin", "relationship": "Schema", "standard_relationship": true, "description": null,
              "mime": null, "size": null, "owners": [{ "owner": "document" }] },
            { "file": 3, "name": "data.csv", "relationship": "Alternative", "standard_relationship": true, "description": null,
              "mime": "text/csv", "size": null, "owners": [{ "owner": "annotation", "page": 2, "annotation": 1 }] },
        ])
    );
    assert_eq!(
        info["associated_file_notes"],
        json!([
            "The document, /AF entry 2: it has no embedded file (/EF), so it was skipped.",
            "The document, /AF entry 3: its reference leads to no object, so it was skipped.",
        ])
    );

    let saved = tool(&mut auto, "associated_file_save", json!({ "doc": doc, "file": 1, "path": "out/invoice.xml" }));
    assert_eq!(
        (saved["file"].clone(), saved["name"].clone(), saved["relationship"].clone(), saved["bytes"].clone()),
        (json!(1), json!("invoice.xml"), json!("Data"), json!(9))
    );
    assert_eq!(std::fs::read(dir.path().join("out/invoice.xml")).unwrap(), b"<invoice>");
    tool(&mut auto, "associated_file_save", json!({ "doc": doc, "file": 3, "path": "data.csv" }));
    assert_eq!(std::fs::read(dir.path().join("data.csv")).unwrap(), b"a,b,c");

    // A stream nothing decodes is an error that says so, and writes nothing.
    let e = refusal(&mut auto, "associated_file_save", json!({ "doc": doc, "file": 2, "path": "packed.bin" }));
    assert!(e.contains("could not be decoded"), "{e}");
    assert!(!dir.path().join("packed.bin").exists());
    for n in [0, 4, -1] {
        let e = refusal(&mut auto, "associated_file_save", json!({ "doc": doc, "file": n, "path": "x.bin" }));
        assert!(e.contains("out of range") && e.contains("3 associated files"), "{n}: {e}");
    }
    let e = refusal(&mut auto, "associated_file_save", json!({ "doc": doc, "file": 1, "path": "../outside.xml" }));
    assert!(e.contains("outside the allowed directory"), "{e}");
    assert!(!dir.path().join("x.bin").exists() && !dir.path().parent().unwrap().join("outside.xml").exists());

    // Reading changed nothing: the document has no unsaved changes.
    let listed = tool(&mut auto, "doc_list", json!({}));
    assert_eq!((listed["documents"][0]["dirty"].clone(), listed["documents"][0]["undo"].clone()), (json!(false), Value::Null), "{listed}");
}

/// `/AF` arrays that list themselves, or each other, end: each entry is read once per owner and
/// a shared array is noted once.
#[test]
fn cyclic_associated_file_arrays_are_read_once() {
    let pdf = b"%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R /AF 8 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /AF 9 0 R >> endobj
8 0 obj [8 0 R 9 0 R 10 0 R] endobj
9 0 obj [9 0 R 8 0 R 10 0 R] endobj
10 0 obj << /Type /Filespec /F (a.txt) /EF << /F 11 0 R >> >> endobj
11 0 obj << /Type /EmbeddedFile /Length 1 >> stream
a
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cyclic.pdf"), pdf).unwrap();
    let mut auto = Automation::new().with_root(dir.path()).unwrap();
    let doc = tool(&mut auto, "doc_open", json!({ "path": "cyclic.pdf" }))["doc"].as_u64().unwrap();
    let info = tool(&mut auto, "doc_info", json!({ "doc": doc }));
    let files = info["associated_files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{info}");
    assert_eq!(files[0]["relationship"], "Unspecified");
    assert_eq!(files[0]["owners"], json!([{ "owner": "document" }, { "owner": "page", "page": 1 }]));
    assert_eq!(
        info["associated_file_notes"],
        json!([
            "The document, /AF entry 1: it is not a file specification dictionary, so it was skipped.",
            "The document, /AF entry 2: it is not a file specification dictionary, so it was skipped.",
        ])
    );
    tool(&mut auto, "associated_file_save", json!({ "doc": doc, "file": 1, "path": "a.txt" }));
    assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"a");
}
