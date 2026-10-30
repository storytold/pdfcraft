use pdfcraft_ffi::{Engine, FfiInkStroke, FfiPoint, FfiRenderRequest};

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
fn test_ink_stroke_commit_and_erase() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("ink_test.pdf".into(), pdf, None).expect("open pdf");

    // Render baseline page tile before any ink
    let req0 = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 1, preview: false, reading_colors: None, hide_comments: false };
    let render0 = engine.render_tile(doc_id, req0).expect("render baseline");
    assert!(render0.error.is_none());

    // 1. Commit a single ink stroke (pen-up)
    let stroke = FfiInkStroke {
        page: 0,
        points: vec![
            FfiPoint { x: 100.0, y: 150.0 },
            FfiPoint { x: 120.0, y: 170.0 },
            FfiPoint { x: 150.0, y: 180.0 },
            FfiPoint { x: 200.0, y: 190.0 },
        ],
        color_rgb: 0x0000FF, // Blue
        width: 3.0,
    };
    engine.add_ink_stroke(doc_id, stroke).expect("add single stroke");

    // Render page tile after ink: should have updated pixels showing ink stroke
    let req1 = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 2, preview: false, reading_colors: None, hide_comments: false };
    let render1 = engine.render_tile(doc_id, req1).expect("render after stroke");
    assert_ne!(render0.pixels, render1.pixels);

    // Hit test ink stroke
    let hit_ink = engine.hit_test_annotation(doc_id, 0, 150.0, 180.0).expect("hit test ink");
    assert!(hit_ink.is_some());
    let annot = hit_ink.unwrap();
    assert_eq!(annot.subtype, "Ink");

    // 2. Erase ink along a cutting path intersecting the stroke
    let eraser_path = vec![FfiPoint { x: 140.0, y: 170.0 }, FfiPoint { x: 160.0, y: 190.0 }];
    let erased = engine.erase_ink_at(doc_id, 0, eraser_path, 15.0).expect("erase ink");
    assert!(erased);

    // Render page tile after erase
    let req2 = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 3, preview: false, reading_colors: None, hide_comments: false };
    let render2 = engine.render_tile(doc_id, req2).expect("render after erase");
    assert_ne!(render1.pixels, render2.pixels);
}

#[test]
fn test_multi_stroke_ink_annotation() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("multi_stroke.pdf".into(), pdf, None).expect("open pdf");

    let strokes = vec![
        vec![FfiPoint { x: 50.0, y: 50.0 }, FfiPoint { x: 80.0, y: 90.0 }, FfiPoint { x: 100.0, y: 50.0 }],
        vec![FfiPoint { x: 60.0, y: 70.0 }, FfiPoint { x: 90.0, y: 70.0 }],
    ];

    engine.add_ink_annotation(doc_id, 0, strokes, Some(0xFF0000), Some(2.5), Some(0.9), Some("StylusUser".into())).expect("add multi-stroke");

    let hit = engine.hit_test_annotation(doc_id, 0, 75.0, 70.0).expect("hit multi-stroke").expect("found ink");
    assert_eq!(hit.subtype, "Ink");
    assert_eq!(hit.author.as_deref(), Some("StylusUser"));
}
