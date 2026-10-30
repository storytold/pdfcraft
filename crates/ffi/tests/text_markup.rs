use pdfcraft_ffi::{Engine, FfiError, FfiRenderRequest, FfiTextMarkupKind};

fn synthetic_text_pdf() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let objs: Vec<String> = vec![
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>\nendobj\n"
            .to_string(),
        "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n".to_string(),
        {
            let stream = "BT /F1 24 Tf 100 700 Td (Hello PdfCraft Mobile Text Markup) Tj ET";
            format!("5 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n", stream.len(), stream)
        },
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
fn test_text_selection_and_markup_highlight() {
    let engine = Engine::new();
    let pdf = synthetic_text_pdf();
    let doc_id = engine.open_bytes("markup_test.pdf".into(), pdf, None).expect("open text pdf");

    let text = engine.page_text(doc_id, 0).expect("extract page text");
    assert!(text.contains("PdfCraft"));

    // Find "PdfCraft"
    let matches = engine.find_text(doc_id, 0, "PdfCraft".into(), false).expect("find text");
    assert!(!matches.is_empty());
    let m = &matches[0];

    // Query selection rects
    let rects = engine.text_selection_rects(doc_id, 0, m.start_glyph, m.end_glyph.saturating_sub(1)).expect("selection rects");
    assert!(!rects.is_empty());
    assert!(rects[0].width > 0.0);
    assert!(rects[0].height > 0.0);

    // Query selection string
    let sel_text = engine.text_selection_string(doc_id, 0, m.start_glyph, m.end_glyph.saturating_sub(1)).expect("selection string");
    assert_eq!(sel_text, "PdfCraft");

    // Nearest glyph lookup
    let nearest = engine.nearest_glyph_at(doc_id, 0, rects[0].x + 5.0, rects[0].y + 5.0).expect("nearest glyph");
    assert!(nearest.is_some());

    // Render page tile before markup
    let req_before = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 1, preview: false, reading_colors: None, hide_comments: false };
    let render_before = engine.render_tile(doc_id, req_before).expect("render before");
    assert!(render_before.error.is_none());

    // Add Highlight annotation
    engine
        .add_text_markup_selection(
            doc_id,
            0,
            m.start_glyph,
            m.end_glyph.saturating_sub(1),
            FfiTextMarkupKind::Highlight,
            Some(0xFFFF00),
            Some("Author".into()),
            Some("Comment".into()),
        )
        .expect("add highlight");

    // Render page tile after markup - should reflect the new markup appearance
    let req_after = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 2, preview: false, reading_colors: None, hide_comments: false };
    let render_after = engine.render_tile(doc_id, req_after).expect("render after");
    assert!(render_after.error.is_none());

    // Pixel buffers must differ because the yellow highlight has been drawn
    assert_ne!(render_before.pixels, render_after.pixels);
}

#[test]
fn test_text_markup_rects_and_kinds() {
    let engine = Engine::new();
    let pdf = synthetic_text_pdf();
    let doc_id = engine.open_bytes("markup_kinds.pdf".into(), pdf, None).expect("open text pdf");

    let matches = engine.find_text(doc_id, 0, "Mobile".into(), false).expect("find text");
    assert!(!matches.is_empty());
    let rects = engine.text_selection_rects(doc_id, 0, matches[0].start_glyph, matches[0].end_glyph.saturating_sub(1)).unwrap();

    // Underline
    engine.add_text_markup_rects(doc_id, 0, rects.clone(), FfiTextMarkupKind::Underline, Some(0x00FF00), None, None).expect("add underline");

    // StrikeOut
    engine.add_text_markup_rects(doc_id, 0, rects.clone(), FfiTextMarkupKind::StrikeOut, Some(0xFF0000), None, None).expect("add strikeout");

    // Squiggly
    engine.add_text_markup_rects(doc_id, 0, rects, FfiTextMarkupKind::Squiggly, None, None, None).expect("add squiggly");

    // Out of bounds page returns PageOutOfBounds
    let err = engine.add_text_markup_selection(doc_id, 99, 0, 1, FfiTextMarkupKind::Highlight, None, None, None).unwrap_err();
    assert!(matches!(err, FfiError::PageOutOfBounds { .. }));
}
