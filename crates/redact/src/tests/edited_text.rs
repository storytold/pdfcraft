//! Text that Edit text wrote is redacted like any other.

use super::*;

/// The entry `name` of page 0's `/Font` resources, as the page holds it.
fn font_entry(doc: &Document, name: &[u8]) -> Option<Object> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources")?).as_dict()?.clone();
    let fonts = doc.resolve(res.get(b"Font")?).as_dict()?.clone();
    fonts.get(name).cloned()
}

/// Edit text draws a character the page's font lacks (here `қ`) in a Type 3 font it generates.
/// Apply Redactions cleans a Type 3 font's glyphs through its reference and refuses a page that
/// holds one directly, so the generated font must be an object of its own. A second edit of the
/// line draws with that same font.
#[test]
fn text_edited_in_a_generated_fallback_font_can_be_redacted() {
    let mut doc = one_page(b"BT /F1 10 Tf 20 20 Td (Name) Tj ET", "", vec![]);
    let edit = pdfcraft_edit::replace_line(&mut doc, 0, 0, "SECRET қ").unwrap();
    assert_eq!(edit.substituted.as_deref(), Some("Inter Type3"), "{edit:?}");
    pdfcraft_edit::replace_line(&mut doc, 0, 0, "қ SECRET").unwrap();
    assert!(content(&doc, 0).contains("/PCLat "), "the second edit draws with the first one's font: {}", content(&doc, 0));
    assert!(font_entry(&doc, b"PCLat1").is_none(), "the second edit added another font");
    let entry = font_entry(&doc, b"PCLat");
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 60.0]], "");
    let report = apply(&mut doc, None).unwrap_or_else(|e| panic!("Apply Redactions refused the edited page: {e}"));
    assert!(report.glyphs > 0, "{report:?}");
    assert!(!pdfcraft_edit::text_lines(&doc, 0).unwrap().iter().any(|l| l.text.contains("SECRET")), "the marked text stays");
    assert!(matches!(entry, Some(Object::Ref(_))), "the generated font is not an object of its own: {entry:?}");
}
