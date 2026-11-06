use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full};
use pdfcraft_edit::{FlattenOptions, flatten_with};

/// A synthetic PDF with a signature-typed parent, a text child with an appearance, and an
/// original terminal signature without a widget. No external document or asset is used.
fn fixture() -> Document {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 8 0 R] >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R /Annots [6 0 R] >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /FT /Sig /T (parent) /Kids [6 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (child) /Parent 5 0 R /Rect [10 10 150 40] /F 4 /AP << /N 7 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 140 30] /Length 0 >>\nstream\n\nendstream",
        "<< /FT /Sig /T (terminal) >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(bytes)).unwrap()
}

#[test]
fn flattening_does_not_turn_an_empty_signature_parent_into_a_terminal_field() {
    let mut doc = fixture();
    assert_eq!(flatten_with(&mut doc, &[0], FlattenOptions { comments: false, fields: true, keep_signature_fields: true }).unwrap(), 1);
    let doc = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let catalog = doc.get(doc.root().unwrap());
    let acroform = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acroform.as_dict().unwrap().get(b"Fields").unwrap());
    let names: Vec<_> = fields
        .as_array()
        .unwrap()
        .iter()
        .map(|field| doc.resolve(field).as_dict().unwrap().get(b"T").unwrap().as_string().unwrap().to_text())
        .collect();
    assert_eq!(names, ["terminal"], "keep the original terminal signature, not a pruned parent");
}
