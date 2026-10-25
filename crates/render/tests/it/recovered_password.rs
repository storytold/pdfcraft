//! Contributor-original, in-memory fixtures: no fonts or binary assets.
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, SaveOptions, write_full};
use pdfcraft_render::{OpenError, check_pages, inspect};

fn fixture(algorithm: Algorithm) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let mut doc = Document::open(Arc::new(bytes)).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm,
        user_password: "user",
        owner_password: "owner",
        permissions: -1852,
        encrypt_metadata: true,
        seed: [39; 32],
    })
    .unwrap();
    write_full(&doc, &SaveOptions { object_streams: false, ..Default::default() }).unwrap()
}

fn without_startxref(mut bytes: Vec<u8>) -> Vec<u8> {
    let at = bytes.windows(b"startxref".len()).rposition(|s| s == b"startxref").unwrap();
    bytes.truncate(at);
    bytes.extend_from_slice(b"%%EOF\n");
    bytes
}

#[test]
fn recovered_encrypted_pdfs_request_password_and_accept_retry() {
    for algorithm in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let original = fixture(algorithm);
        let missing = without_startxref(original.clone());
        let mut wrong = missing.clone();
        wrong.truncate(wrong.len() - b"%%EOF\n".len());
        wrong.extend_from_slice(b"startxref\n0\n%%EOF\n");
        let mut prefixed = b"Synthetic transport header\n".to_vec();
        prefixed.extend_from_slice(&original);
        for (layout, bytes) in [("intact", original), ("missing", missing), ("wrong", wrong), ("prefixed", prefixed)] {
            let bytes = Arc::new(bytes);
            assert!(matches!(inspect(bytes.clone(), None), Err(OpenError::NeedsPassword)), "{algorithm:?}/{layout}: missing password");
            assert!(matches!(inspect(bytes.clone(), Some("wrong")), Err(OpenError::WrongPassword)), "{algorithm:?}/{layout}: wrong password");
            assert!(matches!(check_pages(bytes.clone(), None, 1), Err(OpenError::NeedsPassword)), "{algorithm:?}/{layout}: page check");
            assert!(
                matches!(check_pages(bytes.clone(), Some("wrong"), 1), Err(OpenError::WrongPassword)),
                "{algorithm:?}/{layout}: page check retry"
            );
            let info = inspect(bytes.clone(), Some("user")).unwrap();
            assert_eq!(info.pages.len(), 1, "{algorithm:?}/{layout}");
            check_pages(bytes, Some("user"), 1).unwrap();
        }
    }
}

#[test]
fn recovered_unsupported_encryption_is_not_reported_as_invalid_pdf() {
    let mut bytes = without_startxref(fixture(Algorithm::Rc4_128));
    let at = bytes.windows(b"/Standard".len()).position(|s| s == b"/Standard").unwrap();
    bytes.splice(at..at + b"/Standard".len(), b"/UnknownX".iter().copied());
    assert!(matches!(inspect(Arc::new(bytes), None), Err(OpenError::Unsupported(_))));
}

#[test]
fn unrecoverable_pdf_remains_invalid() {
    let bytes = Arc::new(b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n".to_vec());
    assert!(matches!(inspect(bytes, None), Err(OpenError::Invalid(_))));
}
