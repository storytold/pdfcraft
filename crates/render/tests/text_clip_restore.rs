//! Pending glyph outlines belong to the text object, before they become a graphics-state
//! clip at ET (ISO 32000-1 §9.3.6). q/Q must not discard them or resurrect consumed outlines.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use pdfcraft_testkit::{Pdf, stream_obj};

fn render(body: &str) -> Vec<u8> {
    let bytes = Pdf::new()
        .objs(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            stream_obj("", body.as_bytes()),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ])
        .build();
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (200, 200));
    page.rgba.to_vec()
}

/// Restoring an inner state must retain all outlines accumulated before ET, including
/// outlines shown both before and inside the save, in every clipping rendering mode.
#[test]
fn pending_text_clips_survive_inner_graphics_restores() {
    for mode in 4..=7 {
        for (flat, nested) in [
            ("(MASK) Tj", "q (MASK) Tj Q"),
            ("(MASK) Tj", "q q (MASK) Tj Q Q"),
            ("60 0 Td (MASK) Tj", "q 1 0 0 1 60 0 cm (MASK) Tj Q"),
            ("(A) Tj 40 0 Td (B) Tj", "(A) Tj q 40 0 Td (B) Tj Q"),
        ] {
            let page = |runs| format!("q BT /F1 24 Tf {mode} Tr 20 100 Td {runs} ET 0 0 1 rg 0 0 200 200 re f Q");
            let expected = render(&page(flat));
            let blue = expected.as_chunks::<4>().0.iter().filter(|p| p[2] > 180 && p[0] < 80 && p[1] < 80).count();
            assert!(blue > 0 && blue < 2000, "mode {mode}: a real glyph mask, {blue} blue pixels");
            assert!(render(&page(nested)) == expected, "mode {mode}, {nested}");
        }
    }
}

/// ET consumes pending outlines: a later Q must not bring back its saved copy. The clip
/// installed inside q/Q is restored normally, leaving the later full-page fill visible.
#[test]
fn restoring_after_et_does_not_resurrect_pending_text_clips() {
    let fill = "0 0 1 rg 0 0 200 200 re f";
    let body = format!("BT /F1 24 Tf 7 Tr 20 100 Td (MASK) Tj q ET Q BT ET {fill}");
    assert!(render(&body) == render(fill), "Q resurrected outlines consumed at ET");
}
