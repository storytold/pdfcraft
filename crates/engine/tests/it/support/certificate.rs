//! Certificate-protected (`/Adobe.PubSec`) test documents, made with the sign crate's test
//! identity (`rsa-aes.p12`, password `test`).

use std::sync::Arc;

use pdfcraft_engine::sign::DigitalId;
use pdfcraft_engine::{Algorithm, CertificateProtection, CertificateRecipient, DocId, Edit, Session};

/// The identity the documents are encrypted for.
pub fn identity() -> DigitalId {
    let p12 = std::fs::read(format!("{}/../sign/tests/data/rsa-aes.p12", env!("CARGO_MANIFEST_DIR"))).unwrap();
    pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap()
}

/// One 200 x 300 page showing "Page 1" (Helvetica 24 pt at 20, 150), with `annots` as its
/// annotations.
pub fn one_page(annots: &[&str]) -> Vec<u8> {
    let refs: Vec<String> = (0..annots.len()).map(|i| format!("{} 0 R", 6 + i)).collect();
    let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> /Annots [{}] >>", refs.join(" ")),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    objs.extend(annots.iter().map(|a| a.to_string()));
    let mut out = b"%PDF-1.7\n".to_vec();
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

/// `bytes` encrypted for [`identity`] with every permission, as Save writes it after
/// Protect with certificates.
pub fn protected(bytes: Vec<u8>) -> Arc<Vec<u8>> {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("plain.pdf", None, Arc::new(bytes), None).unwrap();
    let protection = CertificateProtection {
        recipients: vec![CertificateRecipient { certificate: identity().certificate.raw, permissions: -1 }],
        algorithm: Algorithm::Aes256,
        encrypt_metadata: true,
        seed: s.new_seed().unwrap(),
    };
    s.apply(id, Edit::ProtectWithCertificates(protection)).unwrap();
    let saved = s.save_bytes(id).unwrap();
    assert!(contains(&saved, b"Adobe.PubSec"), "the saved file is certificate-protected");
    saved
}

/// `file` opened in a new session that holds [`identity`].
pub fn open_with_identity(file: Arc<Vec<u8>>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    s.add_identity(identity());
    let id = s.open("secret.pdf", None, file, None).unwrap();
    (s, id)
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
