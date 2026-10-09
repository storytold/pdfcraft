//! Rendering checks for form fields. They live here, not in pdfcraft-forms, because forms and
//! render are both layer 3 and may not depend on each other, not even for tests.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, write_incremental};
use pdfcraft_forms::{FieldProps, FieldValue, NewField, add_field, set_props, set_value};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

fn one_page() -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    let r = doc.add(p);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    doc
}

/// Width and height of the dark pixels on page 1.
fn ink_span(doc: &Document) -> (u32, u32) {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    let mut r = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let p = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 2.0, tag: 0 });
    assert!(p.error.is_none(), "{:?}", p.error);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    let mut ink = false;
    for y in 0..p.height {
        for x in 0..p.width {
            let i = ((y * p.width + x) * 4) as usize;
            if p.rgba[i] < 200 {
                ink = true;
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    assert!(ink, "the field drew no text");
    (x1 - x0, y1 - y0)
}

#[test]
fn a_rotated_field_draws_along_the_vertical_edge() {
    let mut doc = one_page();
    add_field(&mut doc, 0, [10.0, 40.0, 90.0, 60.0], &NewField::Text { multiline: false }, Some("wide")).unwrap();
    set_value(&mut doc, "wide", &FieldValue::Text("MMMMMM".into())).unwrap();
    set_props(&mut doc, "wide", &FieldProps { font_size: Some(12.0), ..FieldProps::default() }).unwrap();
    let (wide, tall) = ink_span(&doc);
    assert!(wide > tall, "upright text is a horizontal run: {wide}x{tall}");
    set_props(&mut doc, "wide", &FieldProps { rotation: Some((0, 90)), ..FieldProps::default() }).unwrap();
    let (wide, tall) = ink_span(&doc);
    assert!(tall > wide, "rotated text is a vertical run: {wide}x{tall}");
}
