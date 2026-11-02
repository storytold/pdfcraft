//! Deleting the last document attachment when the catalog's `/Names` is an indirect dictionary
//! (as some producers write it) must not leave an empty `/Names` behind.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object};
use pdfcraft_organize::{add_attachment, attachment_keys, delete_attachment};

/// One page, and a catalog whose `/Names` is the indirect (empty) dictionary 4 0 R.
fn indirect_names_pdf() -> Vec<u8> {
    let objs: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R /Names 4 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        b"<< >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn deleting_the_last_attachment_removes_an_indirect_empty_names_dictionary() {
    let mut doc = Document::open(Arc::new(indirect_names_pdf())).unwrap();
    let key = add_attachment(&mut doc, "notes.txt", b"hello", None, None, None).unwrap();
    assert_eq!(attachment_keys(&doc), ["notes.txt"]);
    delete_attachment(&mut doc, &key).unwrap();
    assert!(attachment_keys(&doc).is_empty());
    let root = doc.root().unwrap();
    let catalog = doc.get(root).as_dict().cloned().unwrap();
    assert!(catalog.get(b"Names").is_none(), "the catalog keeps no empty /Names: {:?}", catalog.get(b"Names").map(|n| matches!(n, Object::Ref(_))));
}
