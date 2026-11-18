//! Bleed marks start at the bleed box, so the room made for the marks counts the bleed too.

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_edit::{PrinterMarks, add_printer_marks};

fn rect(r: [f64; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

fn page_with(boxes: &[(&[u8], [f64; 4])]) -> Document {
    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    for (key, r) in boxes {
        page.set(key.to_vec(), rect(*r));
    }
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    doc
}

fn page_box(doc: &Document, key: &[u8]) -> Option<Vec<f64>> {
    let page = pdfcraft_model::pages(doc).first().cloned()?;
    let a = page.dict.get(key).map(|o| doc.resolve(o))?;
    Some(a.as_array()?.iter().filter_map(|v| doc.resolve(v).as_f64()).collect())
}

/// A 9 pt bleed: the bleed marks run out to 9 + 6 + 18 = 33 pt from the trim box, further than the
/// usual 24, so the crop box grows by that much.
#[test]
fn crop_box_grows_to_hold_the_bleed_marks() {
    let mut doc = page_with(&[
        (b"MediaBox", [0.0, 0.0, 1000.0, 1000.0]),
        (b"CropBox", [200.0, 200.0, 800.0, 800.0]),
        (b"TrimBox", [200.0, 200.0, 800.0, 800.0]),
        (b"BleedBox", [191.0, 191.0, 809.0, 809.0]),
    ]);
    let marks = PrinterMarks::default();
    assert_eq!(marks.reach(), 24.0);
    let widened = add_printer_marks(&mut doc, &[0], &marks, false).unwrap();
    assert_eq!(widened, vec![0]);
    assert_eq!(page_box(&doc, b"CropBox"), Some(vec![167.0, 167.0, 833.0, 833.0]));
    assert_eq!(page_box(&doc, b"TrimBox"), Some(vec![200.0, 200.0, 800.0, 800.0]));
}
