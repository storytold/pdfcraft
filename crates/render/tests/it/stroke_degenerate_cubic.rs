//! A contributor-original synthetic page reproducing #4907: an outline that ends in a cubic whose
//! control points all sit 0.001 pt from its start was not drawn at all, because the stroker put
//! NaN into its outline and the rasterizer dropped the whole path.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

/// A one-page 100 × 20 pt PDF drawing `content`, with a cross-reference table.
fn pdf(content: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 20] /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

/// The red channel of the pixel at (`x`, `y`) pt from the top left of `content` rendered at 1×.
fn red_at(content: &str, x: usize, y: usize) -> u8 {
    let mut renderer = PageRenderer::new(Arc::new(pdf(content)), RenderConfig::default());
    let out = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let at = (y * out.width as usize + x) * 4;
    *out.rgba.get(at).expect("the pixel lies on the page")
}

#[test]
fn a_stroke_ending_in_a_tiny_degenerate_cubic_is_drawn() {
    // The line runs from (10, 10) to about (84, 9.5) in page space; (50, 10) from the top left is
    // inside its 2 pt width. The cubic's exact coordinates are what trigger the NaN. The control,
    // without the cubic, is the baseline: it draws with or without the fix.
    let path = "0 0 m 73.763 -0.536 l 73.764 -0.536 73.764 -0.536 73.764 -0.536 c S";
    let control = "0 0 m 73.763 -0.536 l S";
    for content in [control, path] {
        let content = format!("0 G 2 w 1 0 0 1 10 10 cm {content}");
        let red = red_at(&content, 50, 10);
        assert!(red < 64, "`{content}` left the line's pixel at red {red}, want black");
    }
}
