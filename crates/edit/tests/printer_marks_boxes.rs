//! Printer marks and the page boxes: the marks must show inside the crop box, and running
//! again must not move the trim box or grow the page.

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_edit::{PrinterMarks, add_printer_marks};

fn rect(r: [f64; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

/// One empty page with these boxes set on it.
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

/// A crop box cut to the trim inside a large media box: the marks fit the media box but
/// not the crop box, so the crop box grows to show them and the trim box stays.
#[test]
fn printer_marks_widen_crop_inside_media() {
    let mut doc = page_with(&[(b"MediaBox", [0.0, 0.0, 1000.0, 1000.0]), (b"CropBox", [200.0, 200.0, 800.0, 800.0])]);
    let marks = PrinterMarks::default();
    let reach = marks.reach();
    let widened = add_printer_marks(&mut doc, &[0], &marks, false).unwrap();
    assert_eq!(widened, vec![0]);
    assert_eq!(page_box(&doc, b"MediaBox"), Some(vec![0.0, 0.0, 1000.0, 1000.0]), "the media box already had room");
    let crop = page_box(&doc, b"CropBox").unwrap();
    assert_eq!(crop, vec![200.0 - reach, 200.0 - reach, 800.0 + reach, 800.0 + reach]);
    assert_eq!(page_box(&doc, b"TrimBox"), Some(vec![200.0, 200.0, 800.0, 800.0]));
}

/// No trim box given: it defaults to the crop box, which widening moves. Running again with
/// replace must keep the trim box and the page size from the first run.
#[test]
fn printer_marks_rerun_keeps_trim() {
    let mut doc = page_with(&[(b"MediaBox", [0.0, 0.0, 612.0, 792.0])]);
    let marks = PrinterMarks::default();
    add_printer_marks(&mut doc, &[0], &marks, false).unwrap();
    let media = page_box(&doc, b"MediaBox");
    let crop = page_box(&doc, b"CropBox");
    assert_eq!(page_box(&doc, b"TrimBox"), Some(vec![0.0, 0.0, 612.0, 792.0]));
    assert_eq!(page_box(&doc, b"BleedBox"), Some(vec![0.0, 0.0, 612.0, 792.0]));
    let widened = add_printer_marks(&mut doc, &[0], &marks, true).unwrap();
    assert!(widened.is_empty(), "the page already has room");
    assert_eq!(page_box(&doc, b"MediaBox"), media, "the page did not grow again");
    assert_eq!(page_box(&doc, b"CropBox"), crop);
    assert_eq!(page_box(&doc, b"TrimBox"), Some(vec![0.0, 0.0, 612.0, 792.0]));
}
