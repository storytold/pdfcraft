use pdfcraft_ffi::{Engine, FfiError, FfiPoint, FfiRect, FfiRenderRequest, FfiShapeKind};

fn synthetic_blank_pdf() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let objs: Vec<String> = vec![
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>\nendobj\n".to_string(),
    ];

    for obj in &objs {
        offsets.push(out.len());
        out.extend_from_slice(obj.as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_note_and_textbox_lifecycle() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("notes.pdf".into(), pdf, None).expect("open pdf");

    // Initially no annotations
    let hit_none = engine.hit_test_annotation(doc_id, 0, 100.0, 100.0).expect("hit test");
    assert!(hit_none.is_none());

    // Add sticky note
    engine
        .add_note_annotation(
            doc_id,
            0,
            FfiPoint { x: 100.0, y: 100.0 },
            "Review this paragraph".into(),
            Some("Reviewer".into()),
            Some("Comment".into()),
            Some(0xFFCC00),
        )
        .expect("add note");

    // Hit test note
    let hit_note = engine.hit_test_annotation(doc_id, 0, 102.0, 102.0).expect("hit note").expect("found note");
    assert_eq!(hit_note.subtype, "Text");
    assert_eq!(hit_note.contents.as_deref(), Some("Review this paragraph"));
    assert_eq!(hit_note.author.as_deref(), Some("Reviewer"));

    // Add text box
    let box_rect = FfiRect { x: 200.0, y: 200.0, width: 150.0, height: 50.0 };
    engine.add_text_box_annotation(doc_id, 0, box_rect, "Sample FreeText".into(), 14.0, Some("Editor".into()), Some(0x0000FF)).expect("add text box");

    // Hit test text box
    let hit_box = engine.hit_test_annotation(doc_id, 0, 220.0, 220.0).expect("hit box").expect("found box");
    assert_eq!(hit_box.subtype, "FreeText");
    assert_eq!(hit_box.contents.as_deref(), Some("Sample FreeText"));

    // Move text box
    engine.move_annotation(doc_id, 0, hit_box.index, 30.0, 20.0).expect("move text box");

    // Old location should not hit text box
    let hit_old = engine.hit_test_annotation(doc_id, 0, 205.0, 205.0).expect("hit old");
    assert!(hit_old.map(|h| h.subtype).as_deref() != Some("FreeText"));

    // New location hits
    let hit_moved = engine.hit_test_annotation(doc_id, 0, 240.0, 230.0).expect("hit moved").expect("found moved box");
    assert_eq!(hit_moved.subtype, "FreeText");

    // Resize text box
    let new_rect = FfiRect { x: 230.0, y: 220.0, width: 200.0, height: 80.0 };
    engine.resize_annotation(doc_id, 0, hit_moved.index, new_rect).expect("resize box");

    // Style annotation
    engine.style_annotation(doc_id, 0, hit_moved.index, Some(0xFF0000), Some(0.8), Some(2.5), None).expect("style box");

    // Delete text box
    engine.delete_annotation(doc_id, 0, hit_moved.index).expect("delete box");

    // After deletion, hit test returns None or note
    let after_delete = engine.hit_test_annotation(doc_id, 0, 240.0, 230.0).expect("hit after delete");
    assert!(after_delete.is_none());
}

#[test]
fn test_shape_and_stamp_lifecycle() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("shapes.pdf".into(), pdf, None).expect("open pdf");

    // Add Rectangle shape
    let rect_view = FfiRect { x: 50.0, y: 50.0, width: 100.0, height: 80.0 };
    engine
        .add_shape_annotation(doc_id, 0, FfiShapeKind::Rectangle, rect_view, Some(0x00FF00), Some(0xEEEEEE), Some(2.0), Some("Designer".into()))
        .expect("add rect shape");

    // Add Oval shape
    let oval_view = FfiRect { x: 200.0, y: 50.0, width: 80.0, height: 80.0 };
    engine.add_shape_annotation(doc_id, 0, FfiShapeKind::Oval, oval_view, Some(0xFF00FF), None, Some(1.5), None).expect("add oval shape");

    // Add Line shape
    let line_view = FfiRect { x: 50.0, y: 200.0, width: 100.0, height: 50.0 };
    engine.add_shape_annotation(doc_id, 0, FfiShapeKind::Line, line_view, Some(0x000000), None, Some(1.0), None).expect("add line shape");

    // Add Arrow shape
    let arrow_view = FfiRect { x: 200.0, y: 200.0, width: 120.0, height: 60.0 };
    engine.add_shape_annotation(doc_id, 0, FfiShapeKind::Arrow, arrow_view, Some(0x0000FF), None, Some(2.0), None).expect("add arrow shape");

    // Add Stamp
    let stamp_view = FfiRect { x: 50.0, y: 350.0, width: 160.0, height: 60.0 };
    engine.add_stamp_annotation(doc_id, 0, stamp_view, "Approved".into(), Some("Approver".into())).expect("add stamp");

    // Verify stamp hit test
    let stamp_hit = engine.hit_test_annotation(doc_id, 0, 80.0, 370.0).expect("hit stamp").expect("found stamp");
    assert_eq!(stamp_hit.subtype, "Stamp");

    // Hit test rectangle and style with new fill
    let rect_hit = engine.hit_test_annotation(doc_id, 0, 70.0, 70.0).expect("hit rect").expect("found rect");
    assert_eq!(rect_hit.subtype, "Square");
    engine.style_annotation(doc_id, 0, rect_hit.index, Some(0x0000FF), Some(1.0), Some(3.0), Some(0xFF0000)).expect("style rect");

    // Verify rendering renders annotations
    let render_res = engine
        .render_tile(doc_id, FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 1, preview: false, reading_colors: None, hide_comments: false })
        .expect("render annotations");
    assert!(render_res.error.is_none());
    assert!(!render_res.pixels.is_empty());
}

#[test]
fn test_error_handling_on_invalid_page_or_id() {
    let engine = Engine::new();
    let err = engine.hit_test_annotation(999, 0, 10.0, 10.0).unwrap_err();
    assert!(matches!(err, FfiError::NotFound { .. }));

    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("error_test.pdf".into(), pdf, None).unwrap();
    let err_page = engine.hit_test_annotation(doc_id, 42, 10.0, 10.0).unwrap_err();
    assert!(matches!(err_page, FfiError::PageOutOfBounds { .. }));
}
