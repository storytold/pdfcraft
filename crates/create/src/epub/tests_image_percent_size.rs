//! `width: 100%` on a picture is not a size we can resolve here: it must leave the picture its
//! natural size (fitted to the page later), not make it a zero-sized box.

use super::dom::{Block, Builder, Cascade};
use super::*;

fn image_size(style: &str) -> [Option<f64>; 2] {
    let html = format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="p.png" alt="" style="{style}"/></body></html>"#);
    let doc = package::parse(&html).unwrap();
    let cascade = Cascade::new(Vec::new());
    let mut b = Builder::new(&cascade, "OEBPS/c.xhtml");
    b.node(doc.root_element());
    let (blocks, _) = b.finish();
    blocks
        .iter()
        .find_map(|b| match b {
            Block::Image { style, .. } => Some(style.size),
            _ => None,
        })
        .expect("an image")
}

#[test]
fn percentage_sizes_are_left_to_the_natural_size() {
    assert_eq!(image_size("width: 100%; height: 50%"), [None, None]);
    assert_eq!(image_size("width: 100%"), [None, None]);
}

#[test]
fn absolute_sizes_still_apply() {
    let [w, h] = image_size("width: 100pt; height: 40pt");
    assert_eq!((w, h), (Some(100.0), Some(40.0)));
    assert_eq!(image_size("width: auto"), [None, None]);
}
