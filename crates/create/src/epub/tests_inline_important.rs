//! An inline `!important` declaration beats a stylesheet's `!important` one; a normal inline
//! declaration still loses to it.

use super::dom::{Block, Builder, Cascade};
use super::*;

fn colour_of(css_text: &str, style: &str) -> [f64; 3] {
    let page = format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p style="{style}">Text</p></body></html>"#);
    let doc = package::parse(&page).unwrap();
    let cascade = Cascade::new(vec![css::parse(css_text, "OEBPS/s.css")]);
    let mut b = Builder::new(&cascade, "OEBPS/c.xhtml");
    b.node(doc.root_element());
    let (blocks, _) = b.finish();
    blocks
        .iter()
        .find_map(|b| match b {
            Block::Text { style, .. } => Some(style.run.color),
            _ => None,
        })
        .expect("a paragraph")
}

#[test]
fn inline_important_follows_the_sheets_important() {
    let red = [1.0, 0.0, 0.0];
    let blue = [0.0, 0.0, 1.0];
    assert_eq!(colour_of("p { color: blue !important }", "color: red !important"), red);
    assert_eq!(colour_of("p { color: blue !important }", "color: red"), blue);
    assert_eq!(colour_of("p { color: blue }", "color: red"), red);
}
