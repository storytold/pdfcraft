//! A contributor-original synthetic page: a wide stroke of a path built with `y` (the second
//! control point on the end point, ISO 32000-2 §8.5.2.1 Table 58) was not drawn at all at some widths,
//! because the stroker put NaN into its outline and the rasterizer dropped the whole path. A file
//! in the PDFBox issue tracker lost the 32 pt outline of its "XX" this way.
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

/// A one-page 200 × 200 pt PDF drawing `content`, with a cross-reference table.
fn pdf(content: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".to_string(),
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
fn a_wide_stroke_of_a_path_with_y_curves_is_drawn() {
    // The path's exact coordinates are the trigger; `cm` moves the point where its two curves
    // meet, (18.9, 236.3), to about (98.9, 99.7) from the top left, inside every stroke width
    // below whatever the caps. The `v` path is the same two curves run backwards, a guard that
    // draws with or without the fix.
    let y = "21.4166 234.5928 m 19.9009 236.0303 18.9478 236.3428 y 19.2603 238.4209 20.8697 238.4209 y S";
    let v = "20.8697 238.4209 m 19.2603 238.4209 18.9478 236.3428 v 19.9009 236.0303 21.4166 234.5928 v S";
    let mut dropped = Vec::new();
    for path in [y, v] {
        for width in [4, 8, 16, 24, 32, 48, 63, 64] {
            for (join, cap) in [(0, 0), (1, 1), (2, 2)] {
                let content = format!("0 1 0 RG {width} w {join} j {cap} J 1 0 0 1 80 -136 cm {path}");
                let red = red_at(&content, 98, 99);
                if red > 64 {
                    dropped.push(content);
                }
            }
        }
    }
    assert!(dropped.is_empty(), "strokes not drawn: {dropped:#?}");
}
