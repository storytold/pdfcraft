use super::*;

#[test]
fn annotation_rectangles_do_not_skip_non_numeric_entries() {
    let doc = one_page(b"BT ET", "", vec![]);
    let rect = Object::Array(vec![Object::Int(10), Object::Int(100), Object::name("bad"), Object::Int(40), Object::Int(120)]);
    assert_eq!(rect_of(&doc, Some(&rect)), None);
}

fn with_annotation(rect: Option<Object>, inline: bool) -> Document {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET", "", vec![]);
    let mut annotation = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Annot")),
        (b"Subtype".to_vec(), Object::name("Text")),
        (b"Contents".to_vec(), Object::String(pdfcraft_cos::PdfString::text("PRIVATE-NOTE"))),
    ]);
    if let Some(rect) = rect {
        annotation.set(b"Rect".to_vec(), rect);
    }
    let annotation = if inline { Object::Dict(annotation) } else { Object::Ref(doc.add(Object::Dict(annotation))) };
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(vec![annotation]))).unwrap();
    mark(&mut doc, 0, &[[0.0, 90.0, 100.0, 130.0]], "");
    doc
}

#[test]
fn unreadable_annotation_rectangles_refuse_apply_without_changing_the_document() {
    let number = |n| Object::Int(n);
    let bad = [
        None,
        Some(Object::Null),
        Some(Object::name("not-a-rectangle")),
        Some(Object::Ref(ObjRef::new(999, 0))),
        Some(Object::Array(vec![number(10), number(100), number(40)])),
        Some(Object::Array(vec![number(10), number(100), Object::name("bad"), number(120)])),
        Some(Object::Array(vec![number(10), number(100), Object::name("bad"), number(40), number(120)])),
        Some(Object::Array(vec![number(10), number(100), Object::Real(f64::INFINITY), number(120)])),
    ];
    for inline in [false, true] {
        for rect in &bad {
            let mut doc = with_annotation(rect.clone(), inline);
            // Keep non-finite objects in memory for the check: a writer need not preserve them.
            let before_page = pdfcraft_model::pages(&doc)[0].dict.clone();
            let before_content = content(&doc, 0);
            let before_marks = marks(&doc);
            let result = apply(&mut doc, None);
            assert!(matches!(result, Err(RedactError::UnreadableAnnotation { page: 1, annotation: 1 })), "{rect:?}, inline={inline}: {result:?}");
            assert_eq!(pdfcraft_model::pages(&doc)[0].dict, before_page);
            assert_eq!(content(&doc, 0), before_content);
            assert_eq!(marks(&doc), before_marks);
        }
    }
}

#[test]
fn readable_annotation_rectangles_keep_their_geometry_and_save_without_the_note() {
    for inline in [false, true] {
        for indirect in [false, true] {
            let mut doc = with_annotation(Some(Object::Array(vec![Object::Int(40), Object::Int(120), Object::Int(10), Object::Int(100)])), inline);
            if indirect {
                let coordinates: Vec<Object> = [40, 120, 10, 100].into_iter().map(|n| Object::Ref(doc.add(Object::Int(n)))).collect();
                let rect = doc.add(Object::Array(coordinates));
                let page = pdfcraft_model::pages(&doc)[0].clone();
                let mut annots = annots_of(&doc, &page.dict);
                let mut annotation = doc.resolve(&annots[0]).as_dict().unwrap().clone();
                annotation.set(b"Rect".to_vec(), Object::Ref(rect));
                annots[0] = if inline { Object::Dict(annotation) } else { Object::Ref(doc.add(Object::Dict(annotation))) };
                doc.update_dict(page.obj, |d| d.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();
            }
            assert_eq!(apply(&mut doc, None).unwrap().annotations, 1);
            let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
            assert!(!bytes.windows(b"PRIVATE-NOTE".len()).any(|w| w == b"PRIVATE-NOTE"));
            let saved = Document::open(Arc::new(bytes)).unwrap();
            assert!(annots_of(&saved, &pdfcraft_model::pages(&saved)[0].dict).is_empty());
        }
    }
}

#[test]
fn malformed_annotations_on_unselected_pages_do_not_block_apply() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Annots [<< /Type /Annot /Subtype /Text /Contents (UNSELECTED-NOTE) >>] >>".to_vec(),
        stream("", b"BT ET"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 100.0, 100.0]], "");
    let before = pdfcraft_model::pages(&doc)[1].dict.clone();
    apply(&mut doc, Some(&[0])).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc)[1].dict, before);
}
