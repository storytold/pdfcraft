//! Headless acceptance test for barcode/QR overlay replacement logic.
//! Asserts that replacement barcode/QR raster stretches to 100% of user-space bounds
//! without shrinking or aspect ratio distortion.

use pdfcraft_engine::{CodeKind, Document, Edit, ScannedCode, Session};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use std::sync::Arc;

#[test]
fn test_barcode_overlay_exact_dimensions() {
    let mut session = Session::new();
    let blank = session.create_blank(200.0, 200.0, 1).expect("create blank page");
    let id = session.open_new("barcode_overlay_test.pdf", blank).expect("open new");

    // 1. Synthetic PDF page with a 100x100 point square at [50.0, 50.0, 150.0, 150.0]
    let rect = [50.0, 50.0, 150.0, 150.0];
    let code = ScannedCode { id: 0, page: 0, kind: CodeKind::Code128, text: "EXACT100".into(), rect, pixel_size: [100, 100], image_index: None };

    // 2. Generate replacement barcode image matching exact dimensions
    let orig_w = (code.rect[2] - code.rect[0]).abs().round() as u32;
    let orig_h = (code.rect[3] - code.rect[1]).abs().round() as u32;
    let pw = if code.pixel_size[0] > 0 { code.pixel_size[0] } else { orig_w };
    let ph = if code.pixel_size[1] > 0 { code.pixel_size[1] } else { orig_h };
    assert_eq!(pw, 100);
    assert_eq!(ph, 100);

    let png_bytes = Document::generate_replacement_code(code.kind, &code.text, pw, ph).expect("generate code");

    // 3. Apply replacement edit
    session
        .apply(id, Edit::AddImage { page: 0, rect: Some(code.rect), name: "UpdatedCode.png".into(), bytes: Arc::new(png_bytes) })
        .expect("apply add image");

    // 4. Re-render page to RGBA buffer at scale 1.0 (72 DPI, 1 pt = 1 px)
    let doc = session.get(id).expect("get doc");
    let mut renderer = PageRenderer::new(doc.display.clone(), Default::default());
    let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });

    assert!(rendered.error.is_none(), "render failed: {:?}", rendered.error);
    assert_eq!(rendered.width, 200);
    assert_eq!(rendered.height, 200);

    // 5. Assert black pixels of the new barcode start exactly at (50, 50) and end at (150, 150)
    let mut min_x = usize::MAX;
    let mut max_x = 0usize;
    let mut min_y = usize::MAX;
    let mut max_y = 0usize;

    for y in 0..rendered.height as usize {
        for x in 0..rendered.width as usize {
            let idx = (y * rendered.width as usize + x) * 4;
            let r = rendered.rgba[idx];
            let g = rendered.rgba[idx + 1];
            let b = rendered.rgba[idx + 2];
            // Black/dark barcode bar pixel
            if r < 50 && g < 50 && b < 50 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }

    assert_eq!(min_x, 50, "barcode black pixels should start at x=50, found {min_x}");
    assert!(max_x == 149 || max_x == 150, "barcode black pixels should end at x=149/150, found {max_x}");
    assert_eq!(min_y, 50, "barcode black pixels should start at y=50, found {min_y}");
    assert!(max_y == 149 || max_y == 150, "barcode black pixels should end at y=149/150, found {max_y}");
}
