//! Rectangle redaction must not report a pattern-cell secret safely removed (#3647).

use std::sync::Arc;

use pdfcraft_automation::{Automation, Content};
use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_engine::DocId;
use serde_json::{Value, json};

fn tool(automation: &mut Automation, name: &str, args: Value) -> Value {
    match automation.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn fixture(cell: &[u8], partial: bool, compressed: bool) -> Vec<u8> {
    let mut pattern = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Pattern")),
        (b"PatternType".to_vec(), Object::Int(1)),
        (b"PaintType".to_vec(), Object::Int(1)),
        (b"TilingType".to_vec(), Object::Int(1)),
        (b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 500.into(), 300.into()])),
        (b"XStep".to_vec(), Object::Int(500)),
        (b"YStep".to_vec(), Object::Int(300)),
    ]);
    pattern.set(
        b"Resources".to_vec(),
        Object::Dict(Dict::from_iter([(
            b"Font".to_vec(),
            Object::Dict(Dict::from_iter([(b"F1".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(4, 0)))])),
        )])),
    );
    let width = if partial { 400 } else { 170 };
    let content = format!("BT /F1 20 Tf 40 250 Td (PUBLIC) Tj ET /Pattern cs /P0 scn 35 110 {width} 35 re f");
    let mut page_stream = Vec::new();
    pdfcraft_cos::serialize(&Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())), &mut page_stream);
    let mut pattern_stream = Vec::new();
    let stream = if compressed { Stream::flate(pattern, cell) } else { Stream::from_raw(pattern, cell.to_vec()) };
    pdfcraft_cos::serialize(&Object::Stream(stream), &mut pattern_stream);
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 300] /Contents 5 0 R /Resources << /Font << /F1 4 0 R >> /Pattern << /P0 6 0 R >> >> >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
        page_stream,
        pattern_stream,
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend(object);
        bytes.extend(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn saved_has_secret(bytes: Vec<u8>) -> bool {
    let document = Document::open(Arc::new(bytes)).unwrap();
    document.object_numbers().into_iter().any(|num| {
        let object = document.try_get(num).unwrap();
        matches!(&*object, Object::Stream(stream) if stream.decoded().unwrap().windows(10).any(|part| part == b"CELLSECRET"))
    })
}

#[test]
fn pattern_redaction_tools_remove_or_refuse_without_leaking_or_changing_undo() {
    let dir = tempfile::tempdir().unwrap();
    for partial in [false, true] {
        for compressed in [false, true] {
            let cell = b"BT /F1 20 Tf 40 120 Td (CELLSECRET) Tj ET BT /F1 20 Tf 300 120 Td (SAFE) Tj ET";
            let bytes = fixture(cell, partial, compressed);
            assert!(saved_has_secret(bytes.clone()));
            std::fs::write(dir.path().join("input.pdf"), bytes).unwrap();
            let mut automation = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
            let id = tool(&mut automation, "doc_open", json!({"path": "input.pdf"}))["doc"].as_u64().unwrap();
            tool(&mut automation, "redact_mark", json!({"doc": id, "page": 1, "rect": [30, 150, 215, 195]}));
            let before = automation.session().get(DocId(id)).unwrap().bytes.clone();
            let pixels = automation.render_pixels(id, 1, 1.0).unwrap();
            match automation.call("redact_apply", &json!({"doc": id})) {
                Ok(_) => {
                    assert!(!partial, "a retained text-bearing cell must not be reported safe");
                    tool(&mut automation, "doc_save", json!({"doc": id, "path": "redacted.pdf"}));
                    assert!(
                        !saved_has_secret(std::fs::read(dir.path().join("redacted.pdf")).unwrap()),
                        "successful apply/save left the pattern-cell text recoverable"
                    );
                }
                Err(error) => {
                    assert!(partial, "a fully removed pattern should apply: {error}");
                    assert!(error.to_string().contains("tiling pattern"), "{error}");
                    assert!(!error.to_string().contains("CELLSECRET"));
                    let document = automation.session().get(DocId(id)).unwrap();
                    assert_eq!(document.bytes, before);
                    assert_eq!(document.redaction_marks(), 1);
                    assert_eq!(document.can_undo(), Some("Add redaction mark"));
                    assert_eq!(automation.render_pixels(id, 1, 1.0).unwrap(), pixels);
                    tool(&mut automation, "edit_undo", json!({"doc": id}));
                    assert_eq!(automation.session().get(DocId(id)).unwrap().redaction_marks(), 0);
                }
            }
        }
    }
}

#[test]
fn pattern_redaction_vector_control_saves_reopens_and_preserves_outside_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fixture(b"0.2 0.4 0.6 rg 40 120 15 15 re f 300 120 30 20 re f", true, true);
    std::fs::write(dir.path().join("vectors.pdf"), bytes).unwrap();
    let mut automation = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut automation, "doc_open", json!({"path": "vectors.pdf"}))["doc"].as_u64().unwrap();
    let (width, height, original) = automation.render_pixels(id, 1, 1.0).unwrap();
    tool(&mut automation, "redact_mark", json!({"doc": id, "page": 1, "rect": [30, 150, 215, 195]}));
    let applied = tool(&mut automation, "redact_apply", json!({"doc": id}));
    assert_eq!(applied["applied"], 1);
    let save = tool(&mut automation, "doc_save", json!({"doc": id, "path": "vectors-redacted.pdf"}));
    assert_eq!(save["incremental"], false);
    let reopened = tool(&mut automation, "doc_open", json!({"path": "vectors-redacted.pdf"}))["doc"].as_u64().unwrap();
    let (new_width, new_height, actual) = automation.render_pixels(reopened, 1, 1.0).unwrap();
    assert_eq!((new_width, new_height), (width, height));
    for (index, (before, after)) in original.as_chunks::<4>().0.iter().zip(actual.as_chunks::<4>().0).enumerate() {
        let (x, y) = (index % width as usize, index / width as usize);
        if x >= 280 {
            assert_eq!(before, after, "unmarked pattern pixels changed at {x},{y}");
        }
        if (35..205).contains(&x) && (155..190).contains(&y) {
            assert_eq!(after, &[0, 0, 0, 255], "marked interior must be black");
        }
    }
}
