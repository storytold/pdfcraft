//! A document whose `/Encrypt` is there but can't be read holds data nothing decrypts. Its pages
//! aren't copied into another document, where that data would read as content.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_organize::{OrganizeError, combine, extract_pages};

/// One page whose content is "encrypted" under an `/Encrypt` that doesn't parse (`E<` for `<<`).
fn damaged() -> Vec<u8> {
    let content: &[u8] = b"\x8a\x11\xf0 ciphertext";
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content, b"\nendstream"].concat(),
        b"E< /Filter /Standard /V 4 /R 4 /Length 128 /P -4 /O <00> /U <00> >>".to_vec(),
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
    let id = "<00112233445566778899aabbccddeeff>";
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /Encrypt 5 0 R /ID [{id} {id}] >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
    );
    out
}

#[test]
fn pages_of_a_document_whose_encryption_cant_be_read_are_not_copied() {
    let src = Document::open(Arc::new(damaged())).unwrap();
    assert!(src.encryption_unreadable());
    let refused = |r: Result<Document, OrganizeError>| matches!(r, Err(OrganizeError::Invalid(why)) if why.contains("/Encrypt"));
    assert!(refused(extract_pages(&src, &[0])), "Extract copied the encrypted content as content");
    assert!(refused(combine(&[("damaged.pdf", &src)])), "Combine copied the encrypted content as content");
}
