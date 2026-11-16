//! `page_render` with `show_invisible_text` paints text in an invisible rendering mode (an OCR
//! layer under a scan), and only that render: the document and later renders are unchanged.

use pdfcraft_automation::{Automation, Content};
use serde_json::json;

/// A 100 × 50 pt page with a block of Helvetica X's in rendering mode 3 at the left, and the
/// same in mode 7 (invisible, and clipping) at the right.
fn invisible_pdf() -> Vec<u8> {
    let content = "BT /F1 40 Tf 3 Tr 2 5 Td (XX) Tj ET BT /F1 40 Tf 7 Tr 52 5 Td (XX) Tj ET";
    format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    )
    .into_bytes()
}

/// The page rendered at 72 dpi, as (red-ish pixels in the left half, in the right half).
fn red_pixels(a: &mut Automation, args: serde_json::Value) -> (usize, usize) {
    let out = a.call("page_render", &args).unwrap();
    let Some(Content::Png { data, .. }) = out.first() else { panic!("expected a PNG: {out:?}") };
    let img = craft_image_codecs::decode(data).unwrap();
    let (w, rgba) = (img.width() as usize, img.to_rgba8());
    let red = |half: usize| {
        rgba.as_chunks::<4>().0.iter().enumerate().filter(|(i, p)| (i % w) * 2 / w == half && p[0] > 200 && p[1] < 80 && p[2] < 80).count()
    };
    (red(0), red(1))
}

#[test]
fn show_invisible_text_paints_modes_3_and_7_in_that_render_only() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-invisible-text-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ocr.pdf"), invisible_pdf()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = match a.call("doc_open", &json!({ "path": "ocr.pdf" })).unwrap().remove(0) {
        Content::Json(v) => v["doc"].as_u64().unwrap(),
        other => panic!("expected JSON: {other:?}"),
    };
    let plain = json!({ "doc": doc, "page": 1, "dpi": 72 });
    assert_eq!(red_pixels(&mut a, plain.clone()), (0, 0), "invisible text paints nothing by default");
    let (mode3, mode7) = red_pixels(&mut a, json!({ "doc": doc, "page": 1, "dpi": 72, "show_invisible_text": "#FF0000" }));
    assert!(mode3 > 100 && mode7 > 100, "both modes are shown in red: {mode3}, {mode7}");
    assert_eq!(red_pixels(&mut a, plain), (0, 0), "the cached view render is unchanged");
    let both = json!({ "doc": doc, "page": 1, "show_invisible_text": "red", "hide_text_block": 1 });
    assert!(a.call("page_render", &both).is_err(), "it can't be combined with an editor background");
    assert!(a.call("page_render", &json!({ "doc": doc, "page": 1, "show_invisible_text": "nope" })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
