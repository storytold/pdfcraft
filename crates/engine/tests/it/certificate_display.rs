//! A certificate-protected document (`/Adobe.PubSec`) is shown from a decrypted copy: the copies
//! made from it for the renderer (missing comment appearances, the redaction check) must be
//! decrypted too, or the renderer cannot open them.
use crate::support;

use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style, rect_quad};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use support::certificate::{contains, one_page, open_with_identity, protected};

/// A FreeText comment with no `/AP`: its appearance is drawn for display only.
const FREE_TEXT: &str =
    "<< /Type /Annot /Subtype /FreeText /Rect [20 200 180 260] /Contents (Box) /DA (/Helv 10 Tf 0 0 1 rg) /Border [0 0 1] /C [.85 .47 .02] >>";

#[test]
fn unappearanced_comment_on_certificate_document_renders() {
    let (s, id) = open_with_identity(protected(one_page(&[FREE_TEXT])));
    let doc = s.get(id).unwrap();
    let mut r = PageRenderer::new(doc.display.clone(), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(p.error.is_none(), "the page view renders: {:?}", p.error);
    // y down: the box spans rows 40..100.
    let at = ((90 * p.width + 100) * 4) as usize;
    assert_eq!(p.rgba[at..at + 4], [217, 120, 5, 255], "the comment's background is drawn");
}

#[test]
fn redactions_apply_on_certificate_protected_document() {
    let (mut s, id) = open_with_identity(protected(one_page(&[])));
    // "Page 1" at 24 pt from x 20: the "1" starts near x 82.7 (Helvetica widths).
    let shape = Shape::Redact { quads: vec![rect_quad([80.0, 140.0, 100.0, 180.0])], overlay: String::new(), look: Default::default() };
    let mark = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Reviewer".into() };
    s.apply(id, Edit::AddAnnotation(mark)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let doc = s.get(id).unwrap();
    assert_eq!(doc.redaction_marks(), 0);
    let mut r = PageRenderer::new(doc.bytes.clone(), RenderConfig::default());
    let text = r.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..Default::default() }).text.map(|t| t.plain_text());
    assert_eq!(text.as_deref().map(str::trim), Some("Page"), "the redacted digit is gone");
    // What Save writes stays certificate-protected.
    assert!(contains(&s.save_bytes(id).unwrap(), b"Adobe.PubSec"));
}

/// `file` encrypted for the test identity without opening it in a session (which would apply an
/// XFA form's data and keep the result).
fn encrypted_as_it_stands(file: Vec<u8>) -> std::sync::Arc<Vec<u8>> {
    let mut cos = pdfcraft_cos::Document::open(std::sync::Arc::new(file)).unwrap();
    let seed = [7u8; 20];
    let certificate = support::certificate::identity().certificate;
    let content = [&seed[..], &pdfcraft_cos::permission_bytes(-1)].concat();
    let envelope = pdfcraft_engine::sign::envelope::envelope(&certificate, &content).unwrap();
    let params = pdfcraft_cos::PublicKeyEncryption {
        algorithm: pdfcraft_engine::Algorithm::Aes256,
        recipients: vec![envelope],
        seed,
        permissions: -1,
        encrypt_metadata: true,
    };
    cos.set_public_key_encryption(&params).unwrap();
    std::sync::Arc::new(pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap())
}

/// Opening applies a form's XFA data by rewriting the file as one more revision and reopening it:
/// that reopening needs the document's key, and the renderer reads a decrypted copy of the result.
#[test]
fn xfa_data_applies_on_certificate_protected_document() {
    let xfa = pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name><agree>1</agree></page1></form1>");
    let file = encrypted_as_it_stands(xfa);
    assert!(contains(&file, b"Adobe.PubSec"));
    let (s, id) = open_with_identity(file);
    let doc = s.get(id).unwrap();
    assert!(!doc.info.warnings.iter().any(|w| w.contains("could not be applied")), "{:?}", doc.info.warnings);
    assert!(doc.info.warnings.iter().any(|w| w.contains("taken from this form's XFA data")), "{:?}", doc.info.warnings);
    assert_eq!(doc.form.iter().find(|f| f.name == "form1[0].page1[0].name[0]").map(|f| f.value.clone()), Some(vec!["Ada".to_string()]));
    assert!(!contains(&doc.bytes, b"Adobe.PubSec"), "the renderer reads the decrypted copy");
    assert!(contains(&s.save_bytes(id).unwrap(), b"Adobe.PubSec"), "what Save writes stays certificate-protected");
}
