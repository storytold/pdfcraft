#![cfg(feature = "mcp")]
mod connected_support;
use pdfcraft_automation::window::WindowTools;
use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_full};
use pdfcraft_engine::Session;
use serde_json::json;
use std::sync::Arc;

#[test]
fn footprints_use_pdf_coordinates_on_rotated_offset_crops() {
    for rotation in [0, 90, 180, 270] {
        let mut cos = Document::open(Arc::new(connected_support::pdf())).unwrap();
        cos.update_dict(ObjRef::new(3, 0), |page| {
            page.set(b"CropBox".to_vec(), Object::Array(vec![Object::Int(10), Object::Int(20), Object::Int(190), Object::Int(280)]));
            page.set(b"Rotate".to_vec(), Object::Int(rotation));
        })
        .unwrap();
        let mut session = Session::new();
        let id = session.open("turned.pdf", None, Arc::new(write_full(&cos, &SaveOptions::default()).unwrap()), None).unwrap();
        let mut tools = WindowTools::default();
        tools.call(4, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
        let proof = tools.resolve_target(4, &session, "text_edit", &json!({"doc":id.0,"page":1,"paragraph":2,"text":"Bravo"})).unwrap();
        let regions = proof["rects"].as_array().unwrap();
        assert!(regions.len() > 5, "prove every rendered glyph, not only a paragraph/word hull");
        assert_eq!(proof["textTarget"]["text"], "Bravo paragraph, a single row.");
        for region in regions {
            let r: Vec<_> = region["rect"].as_array().unwrap().iter().map(|n| n.as_f64().unwrap()).collect();
            assert!(r[0] >= 9.999 && r[2] <= 173.405, "rotation {rotation}: {r:?}");
            assert!((r[1] - 197.6).abs() < 0.001 && (r[3] - 209.6).abs() < 0.001, "rotation {rotation}: {r:?}");
            assert_eq!(region["quad"].as_array().unwrap().len(), 8);
        }
        let region = tools.resolve_target(4, &session, "comment_add", &json!({"doc":id.0,"page":1,"type":"rectangle","rect":[12,25,53,72]})).unwrap();
        let expected = match rotation {
            90 => [35.0, 32.0, 82.0, 73.0],
            180 => [137.0, 45.0, 178.0, 92.0],
            270 => [118.0, 227.0, 165.0, 268.0],
            _ => [22.0, 208.0, 63.0, 255.0],
        };
        for (actual, expected) in region["rects"][0]["rect"].as_array().unwrap().iter().zip(expected) {
            assert!((actual.as_f64().unwrap() - expected).abs() < 0.0001, "rotation {rotation}: {region}");
        }
    }
}

#[test]
fn proof_resolution_uses_only_the_same_connections_live_paragraph_keys() {
    let (mut session, id) = connected_support::session();
    let mut tools = WindowTools::default();
    tools.call(1, &mut session, None, "text_paragraphs", &json!({"doc":id.0,"page":1})).unwrap();
    let target = json!({"doc":id.0,"page":1,"paragraph":3,"text":"Charlie"});
    assert!(tools.resolve_target(2, &session, "text_edit", &target).is_err());
    for text in ["Charlie once.", "Charlie twice."] {
        let args = json!({"doc":id.0,"page":1,"paragraph":3,"text":text});
        tools.resolve_target(1, &session, "text_edit", &args).unwrap();
        tools.call(1, &mut session, None, "text_edit", &args).unwrap();
        assert!(tools.resolve_target(1, &session, "text_edit", &args).unwrap()["textTarget"]["text"].as_str().unwrap().contains(text));
    }
    session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(tools.resolve_target(1, &session, "text_edit", &target).is_err());
}
