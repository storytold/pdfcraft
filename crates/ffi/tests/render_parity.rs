use pdfcraft_ffi::{Engine, FfiError, FfiRenderRequest, FfiTile};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, Tile};
use std::sync::Arc;

fn fixture_standard_fonts() -> Vec<u8> {
    let stream = b"BT /F1 16 Tf 50 200 Td (Helvetica Standard Text) Tj ET BT /F2 16 Tf 50 150 Td (Times-Roman Standard Text) Tj ET";
    format!(
        "%PDF-1.4\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> /F2 << /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >> >> >> /Contents 4 0 R >> endobj\n\
4 0 obj << /Length {} >> stream\n{}\nendstream endobj\n\
trailer << /Root 1 0 R >>\n%%EOF",
        stream.len(),
        std::str::from_utf8(stream).unwrap()
    )
    .into_bytes()
}

fn fixture_transparency() -> Vec<u8> {
    let stream = b"/GS1 gs 1 0 0 rg 50 50 100 100 re f /GS2 gs 0 0 1 rg 100 100 100 100 re f";
    format!(
        "%PDF-1.4\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /ExtGState << /GS1 << /Type /ExtGState /ca 0.6 /CA 0.6 >> /GS2 << /Type /ExtGState /ca 0.4 /CA 0.4 /BM /Multiply >> >> >> /Contents 4 0 R >> endobj\n\
4 0 obj << /Length {} >> stream\n{}\nendstream endobj\n\
trailer << /Root 1 0 R >>\n%%EOF",
        stream.len(),
        std::str::from_utf8(stream).unwrap()
    )
    .into_bytes()
}

fn fixture_image() -> Vec<u8> {
    let img_bytes = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
    let stream = b"q 150 0 0 150 50 50 cm /Im1 Do Q";
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    out.extend_from_slice(b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
    out.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
    out.extend_from_slice(
        b"3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >> endobj\n",
    );
    out.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", stream.len()).as_bytes());
    out.extend_from_slice(stream);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(
        format!(
            "5 0 obj << /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >> stream\n",
            img_bytes.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&img_bytes);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

fn fixture_embedded_type3() -> Vec<u8> {
    let glyph_stream = b"1000 0 0 0 1000 1000 d1 0 0 1000 1000 re f";
    let content_stream = b"BT /F1 20 Tf 50 150 Td (A) Tj ET";
    format!(
        "%PDF-1.4\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj\n\
4 0 obj << /Length {} >> stream\n{}\nendstream endobj\n\
5 0 obj << /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /A 6 0 R >> /Encoding << /Type /Encoding /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [1000] >> endobj\n\
6 0 obj << /Length {} >> stream\n{}\nendstream endobj\n\
trailer << /Root 1 0 R >>\n%%EOF",
        content_stream.len(),
        std::str::from_utf8(content_stream).unwrap(),
        glyph_stream.len(),
        std::str::from_utf8(glyph_stream).unwrap()
    )
    .into_bytes()
}

fn fixture_cjk() -> Vec<u8> {
    let stream = b"BT /F1 20 Tf 50 150 Td <30533093306b3061> Tj ET";
    format!(
        "%PDF-1.4\n\
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 << /Type /Font /Subtype /Type0 /BaseFont /HeiseiMin-W3 /Encoding /UniJIS-UTF16-H /DescendantFonts [5 0 R] >> >> >> /Contents 4 0 R >> endobj\n\
4 0 obj << /Length {} >> stream\n{}\nendstream endobj\n\
5 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /HeiseiMin-W3 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 4 >> /FontDescriptor 6 0 R /DW 1000 >> endobj\n\
6 0 obj << /Type /FontDescriptor /FontName /HeiseiMin-W3 /Flags 4 /FontBBox [-100 -200 1100 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 70 >> endobj\n\
trailer << /Root 1 0 R >>\n%%EOF",
        stream.len(),
        std::str::from_utf8(stream).unwrap()
    )
    .into_bytes()
}

fn fixture_encrypted() -> Vec<u8> {
    use pdfcraft_engine::{Edit, Protection, Session};
    let mut session = Session::new();
    let id = session.open("plain.pdf", None, Arc::new(fixture_standard_fonts()), None).expect("open plain");
    session
        .apply(
            id,
            Edit::Protect(Protection { open_password: Some("secret123".into()), permissions_password: Some("owner".into()), ..Default::default() }),
        )
        .expect("protect");
    let saved = session.save_bytes(id).expect("save encrypted copy");
    saved.as_ref().clone()
}

fn assert_ffi_matches_direct(pdf_bytes: Vec<u8>, password: Option<String>, tile: FfiTile, scale: f32) {
    let engine = Engine::new();
    let doc_id = engine.open_bytes("test.pdf".into(), pdf_bytes.clone(), password.clone()).expect("open in engine");

    let ffi_req = FfiRenderRequest { page: 0, tile: Some(tile), scale, tag: 1, preview: false, reading_colors: None, hide_comments: false };

    let ffi_res = engine.render_tile(doc_id, ffi_req).expect("render tile through FFI");
    assert!(ffi_res.error.is_none());
    assert_eq!(ffi_res.width, tile.width);
    assert_eq!(ffi_res.height, tile.height);

    let mut direct_renderer = PageRenderer::new(Arc::new(pdf_bytes), RenderConfig { password: password.map(Arc::from), ..Default::default() });
    let direct_req = RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        tile: Some(Tile { x: tile.x, y: tile.y, w: tile.width, h: tile.height }),
        scale,
        tag: 1,
        preview: false,
    };
    let direct_res = direct_renderer.render(direct_req);

    assert_eq!(ffi_res.width, direct_res.width);
    assert_eq!(ffi_res.height, direct_res.height);
    assert_eq!(ffi_res.pixels, direct_res.rgba.as_ref());

    engine.close(doc_id).expect("close doc");
}

#[test]
fn test_render_parity_standard_non_embedded_fonts() {
    assert_ffi_matches_direct(fixture_standard_fonts(), None, FfiTile { x: 0, y: 0, width: 300, height: 300 }, 1.0);
}

#[test]
fn test_render_parity_transparency() {
    assert_ffi_matches_direct(fixture_transparency(), None, FfiTile { x: 50, y: 50, width: 150, height: 150 }, 1.0);
}

#[test]
fn test_render_parity_images() {
    assert_ffi_matches_direct(fixture_image(), None, FfiTile { x: 0, y: 0, width: 250, height: 250 }, 1.0);
}

#[test]
fn test_render_parity_embedded_type3() {
    assert_ffi_matches_direct(fixture_embedded_type3(), None, FfiTile { x: 0, y: 0, width: 300, height: 300 }, 1.0);
}

#[test]
fn test_render_parity_cjk() {
    assert_ffi_matches_direct(fixture_cjk(), None, FfiTile { x: 0, y: 0, width: 300, height: 300 }, 1.0);
}

#[test]
fn test_render_parity_encrypted_document() {
    let encrypted = fixture_encrypted();
    let engine = Engine::new();

    // Opening with wrong password must fail with WrongPassword error
    let err = engine.open_bytes("enc.pdf".into(), encrypted.clone(), Some("wrongpw".into())).unwrap_err();
    assert!(matches!(err, FfiError::WrongPassword | FfiError::NeedsPassword));

    // Opening without password must fail
    let err_none = engine.open_bytes("enc.pdf".into(), encrypted.clone(), None).unwrap_err();
    assert!(matches!(err_none, FfiError::NeedsPassword | FfiError::WrongPassword));

    // Opening with correct password succeeds and matches direct renderer
    assert_ffi_matches_direct(encrypted, Some("secret123".into()), FfiTile { x: 0, y: 0, width: 200, height: 200 }, 1.0);
}

#[test]
fn test_render_tiles_stitch_equal_to_full_render() {
    let pdf_bytes = fixture_standard_fonts();
    let engine = Engine::new();
    let doc_id = engine.open_bytes("test.pdf".into(), pdf_bytes.clone(), None).unwrap();

    // 1. Full page render (300 x 300)
    let full_req = FfiRenderRequest {
        page: 0,
        tile: Some(FfiTile { x: 0, y: 0, width: 300, height: 300 }),
        scale: 1.0,
        tag: 1,
        preview: false,
        reading_colors: None,
        hide_comments: false,
    };
    let full_res = engine.render_tile(doc_id, full_req).expect("full render");
    assert_eq!(full_res.pixels.len(), 300 * 300 * 4);

    // 2. Render 4 tiles (150 x 150 each)
    let tile_coords = [(0, 0), (150, 0), (0, 150), (150, 150)];
    let mut stitched = vec![0u8; 300 * 300 * 4];

    for (tx, ty) in tile_coords {
        let req = FfiRenderRequest {
            page: 0,
            tile: Some(FfiTile { x: tx, y: ty, width: 150, height: 150 }),
            scale: 1.0,
            tag: (tx + ty) as u64,
            preview: false,
            reading_colors: None,
            hide_comments: false,
        };
        let t_res = engine.render_tile(doc_id, req).expect("tile render");
        assert_eq!(t_res.width, 150);
        assert_eq!(t_res.height, 150);

        // Copy tile rows into stitched canvas
        for row in 0..150 {
            let src_start = (row * 150 * 4) as usize;
            let src_end = src_start + 150 * 4;
            let dst_y = ty + row;
            let dst_start = ((dst_y * 300 + tx) * 4) as usize;
            let dst_end = dst_start + 150 * 4;
            stitched[dst_start..dst_end].copy_from_slice(&t_res.pixels[src_start..src_end]);
        }
    }

    // 3. Stitched tiles must equal full render: across 360,000 bytes, any boundary differences
    // must be <= 1 (subpixel floating-point antialiasing drift) and affect at most a tiny fraction of pixels.
    let mut diff_count = 0;
    for (i, (&s_byte, &f_byte)) in stitched.iter().zip(full_res.pixels.iter()).enumerate() {
        let diff = (s_byte as i32 - f_byte as i32).abs();
        assert!(diff <= 1, "subpixel antialiasing drift must not exceed 1 at byte {i}: stitched={s_byte}, full={f_byte}");
        if diff != 0 {
            diff_count += 1;
        }
    }
    assert!(diff_count <= 20, "excessive difference count across tile boundaries: {diff_count} / {}", stitched.len());

    engine.close(doc_id).unwrap();
}
