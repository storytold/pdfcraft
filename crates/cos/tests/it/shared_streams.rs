//! Stream data read from a file points into the file's bytes instead of being copied: the
//! document's object cache holds parsed dictionaries, not a second copy of every stream.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_full, write_incremental};

const CONTENT: &[u8] = b"0 0 1 rg 10 10 30 20 re f";

fn fixture() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 50] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", CONTENT.len(), String::from_utf8_lossy(CONTENT)),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
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

const CONTENTS: ObjRef = ObjRef { num: 4, generation: 0 };

fn stream_bytes(doc: &Document, r: ObjRef) -> (Vec<u8>, *const u8) {
    match &*doc.get(r) {
        Object::Stream(s) => (s.raw.to_vec(), s.raw.as_ptr()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cached_streams_are_the_files_own_bytes() {
    let file = Arc::new(fixture());
    let doc = Document::open(file.clone()).unwrap();
    let (data, at) = stream_bytes(&doc, CONTENTS);
    assert_eq!(data, CONTENT);
    assert!(file.as_ptr_range().contains(&at), "not a copy");
    // A full save reads and caches every object; the cache still holds no copy of stream data.
    let full = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(file.as_ptr_range().contains(&stream_bytes(&doc, CONTENTS).1));
    // Saves are unchanged: the stream is written as it was read.
    let back = Document::open(Arc::new(full)).unwrap();
    assert_eq!(stream_bytes(&back, ObjRef::new(4, 0)).0, CONTENT);
}

#[test]
fn edits_within_a_document_keep_pointing_into_its_file() {
    let file = Arc::new(fixture());
    let mut doc = Document::open(file.clone()).unwrap();
    doc.update_dict(CONTENTS, |d| d.set(b"Marked".to_vec(), Object::Bool(true))).unwrap();
    assert!(file.as_ptr_range().contains(&stream_bytes(&doc, CONTENTS).1));
    let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(updated)).unwrap();
    assert_eq!(stream_bytes(&back, CONTENTS).0, CONTENT);
}

#[test]
fn streams_moved_to_another_document_get_their_own_bytes() {
    let file = Arc::new(fixture());
    let source = Document::open(file.clone()).unwrap();
    let moved = (*source.get(CONTENTS)).clone();
    let mut target = Document::new_empty();
    let r = target.add(moved);
    drop(source);
    // The target keeps just the stream's bytes, not the whole source file.
    assert_eq!(Arc::strong_count(&file), 1);
    assert_eq!(stream_bytes(&target, r).0, CONTENT);
}
