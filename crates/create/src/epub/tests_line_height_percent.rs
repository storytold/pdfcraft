//! `line-height: 150%` is a percentage of the element's own font size (CSS 2.1 §10.8.1), so 1.5
//! lines, not the 5.0 ceiling.

use super::dom::{Block, Builder, Cascade};
use super::*;

fn line_height_of(html: &str) -> f64 {
    let doc = package::parse(html).unwrap();
    let cascade = Cascade::new(Vec::new());
    let mut b = Builder::new(&cascade, "OEBPS/c.xhtml");
    b.node(doc.root_element());
    let (blocks, _) = b.finish();
    blocks
        .iter()
        .find_map(|b| match b {
            Block::Text { style, .. } => Some(style.line_height),
            _ => None,
        })
        .expect("a paragraph")
}

#[test]
fn a_percentage_line_height_is_a_multiple_of_the_font_size() {
    let page = |style: &str| format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p style="{style}">Text</p></body></html>"#);
    let lh = line_height_of(&page("font-size: 10pt; line-height: 150%"));
    assert!((lh - 1.5).abs() < 1e-9, "150% should be 1.5 lines, got {lh}");
    let lh = line_height_of(&page("font-size: 20pt; line-height: 120%"));
    assert!((lh - 1.2).abs() < 1e-9, "120% should be 1.2 lines, got {lh}");
    // Other forms are unchanged.
    let lh = line_height_of(&page("font-size: 10pt; line-height: 1.4"));
    assert!((lh - 1.4).abs() < 1e-9, "{lh}");
    let lh = line_height_of(&page("font-size: 10pt; line-height: 2em"));
    assert!((lh - 2.0).abs() < 1e-9, "{lh}");
}
