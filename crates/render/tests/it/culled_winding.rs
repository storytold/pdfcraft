//! Contributor-original rectangles exercise signed winding outside and across the viewport.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

fn fixture(forward: usize, reverse: usize, even_odd: bool, right: u16) -> Vec<u8> {
    let width = right + 10;
    let mut content = format!("-10 -10 {width} 200 re\n").repeat(forward);
    content.push_str(&format!("-10 190 {width} -200 re\n").repeat(reverse));
    content.push_str(if even_odd { "f*" } else { "f" });
    let mut doc = Document::new_empty();
    let tree = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
    let page = doc.add(Dict::from_iter([
        (b"Type".to_vec(), Object::name("Page")),
        (b"Parent".to_vec(), Object::Ref(tree)),
        (b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 180.into(), 180.into()])),
        (b"Contents".to_vec(), Object::Ref(contents)),
    ]));
    doc.update_dict(tree, |tree| {
        tree.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        tree.set(b"Count".to_vec(), 1);
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn check(forward: usize, reverse: usize, even_odd: bool, right: u16, filled: bool) {
    let mut renderer = PageRenderer::new(Arc::new(fixture(forward, reverse, even_odd, right)), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "forward={forward}, reverse={reverse}, even_odd={even_odd}: {:?}", page.error);
    assert_eq!((page.width, page.height), (180, 180));
    assert_eq!(page.rgba.len(), 180 * 180 * 4);
    for (index, pixel) in page.rgba.as_chunks::<4>().0.iter().enumerate() {
        let expected = if filled && index % 180 < usize::from(right) { [0, 0, 0, 255] } else { [255; 4] };
        assert_eq!(*pixel, expected, "forward={forward}, reverse={reverse}, even_odd={even_odd}, pixel={index}");
    }
}

#[test]
fn offscreen_nonzero_fill_keeps_large_signed_windings() {
    // The final case also exceeds i16 while remaining well inside the processed-line bound.
    for count in [128, 129, 256, 32_769] {
        check(count, 0, false, 190, true);
        check(0, count, false, 190, true);
    }
}

#[test]
fn offscreen_even_odd_fill_keeps_large_winding_parity() {
    for count in [128, 129, 256, 32_769] {
        check(count, 0, true, 190, !count.is_multiple_of(2));
        check(0, count, true, 190, !count.is_multiple_of(2));
    }
}

#[test]
fn offscreen_opposite_windings_cancel_exactly() {
    for count in [200, 32_769] {
        for even_odd in [false, true] {
            check(count, count, even_odd, 190, false);
        }
    }
}

#[test]
fn large_culled_windings_reach_visible_edge_tiles() {
    for even_odd in [false, true] {
        check(32_769, 0, even_odd, 90, true);
        check(0, 32_769, even_odd, 90, true);
        check(32_769, 32_769, even_odd, 90, false);
    }
}
