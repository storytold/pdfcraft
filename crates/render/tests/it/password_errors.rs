//! A renderer opened without the user password, or with a wrong one, says so in the words
//! `inspect` uses (ISO 32000-2 §7.6.4.1: a reader that fails to authenticate prompts for a
//! password), not that the document could not be parsed. Contributor-original, in-memory
//! fixtures.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, NewEncryption, SaveOptions, write_full};
use pdfcraft_render::text::TextFilter;
use pdfcraft_render::{OpenError, PageRenderer, RenderConfig, RenderRequest, inspect};

/// A one-page PDF encrypted with `algorithm`, user password "user" and owner password "owner".
fn protected(algorithm: Algorithm) -> Arc<Vec<u8>> {
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
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    doc.set_encryption_legacy(&NewEncryption {
        algorithm,
        user_password: "user",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())
}

fn renderer(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> PageRenderer {
    PageRenderer::new(bytes.clone(), RenderConfig { password: password.map(Arc::from), ..Default::default() })
}

#[test]
fn rendering_without_the_password_or_with_a_wrong_one_reports_the_password() {
    for algorithm in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let bytes = protected(algorithm);
        for (password, expected) in [(None, OpenError::NeedsPassword), (Some("wrong"), OpenError::WrongPassword)] {
            let what = format!("{algorithm:?} with {password:?}");
            // The same words `inspect` (and so `info` and `doc_open`) gives.
            let inspected = inspect(bytes.clone(), password).err().map(|e| e.to_string());
            assert_eq!(inspected.as_deref(), Some(expected.to_string().as_str()), "{what}");
            let mut r = renderer(&bytes, password);
            assert_eq!(r.page_count(), 0, "{what}");
            let page = r.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
            assert_eq!(page.error, Some(expected.to_string()), "{what}: render");
            assert_eq!(r.filtered_text(0, TextFilter::default()).err(), Some(expected.to_string()), "{what}: text");
        }
        let mut r = renderer(&bytes, Some("user"));
        assert_eq!(r.page_count(), 1, "{algorithm:?}");
        let page = r.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert_eq!(page.error, None, "{algorithm:?} with the user password");
        assert_eq!((page.width, page.height), (100, 100), "{algorithm:?}");
    }
}

#[test]
fn a_file_that_is_not_a_pdf_still_reports_a_parse_failure() {
    let page = renderer(&Arc::new(b"not a pdf".to_vec()), Some("user")).render(RenderRequest::default());
    assert_eq!(page.error.as_deref(), Some("the document could not be parsed"));
}
