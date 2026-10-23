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
