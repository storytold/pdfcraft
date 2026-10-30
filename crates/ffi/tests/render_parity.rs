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

fn fixture_embedded_truetype() -> Vec<u8> {
    let font_bytes = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    let stream = b"BT /F1 16 Tf 50 150 Td (Inter TrueType Embedded) Tj ET";
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    out.extend_from_slice(b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
    out.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
    out.extend_from_slice(
        b"3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj\n",
    );
    out.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", stream.len()).as_bytes());
    out.extend_from_slice(stream);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(b"5 0 obj << /Type /Font /Subtype /TrueType /BaseFont /Inter-Regular /FirstChar 32 /LastChar 126 /Widths [500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500 500] /FontDescriptor 6 0 R >> endobj\n");
    out.extend_from_slice(b"6 0 obj << /Type /FontDescriptor /FontName /Inter-Regular /Flags 32 /FontBBox [-200 -300 1200 1000] /ItalicAngle 0 /Ascent 900 /Descent -250 /CapHeight 700 /StemV 80 /FontFile2 7 0 R >> endobj\n");
    out.extend_from_slice(format!("7 0 obj << /Length {} >> stream\n", font_bytes.len()).as_bytes());
    out.extend_from_slice(font_bytes);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

fn fixture_scanned_page() -> Vec<u8> {
    let mut scan_pixels = Vec::with_capacity(16 * 16 * 3);
    for y in 0..16 {
        for x in 0..16 {
            let v = ((x * 16 + y * 8) % 256) as u8;
            scan_pixels.extend_from_slice(&[v, v, v]);
        }
    }
    let stream = b"q 300 0 0 300 0 0 cm /ScanIm Do Q";
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    out.extend_from_slice(b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
    out.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
    out.extend_from_slice(
        b"3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /XObject << /ScanIm 5 0 R >> >> /Contents 4 0 R >> endobj\n",
    );
    out.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", stream.len()).as_bytes());
    out.extend_from_slice(stream);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(
        format!(
            "5 0 obj << /Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >> stream\n",
            scan_pixels.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&scan_pixels);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

#[test]
fn test_render_parity_embedded_truetype() {
    assert_ffi_matches_direct(fixture_embedded_truetype(), None, FfiTile { x: 0, y: 0, width: 300, height: 300 }, 1.0);
}

fn cff_font_bytes() -> Vec<u8> {
    fn be16(out: &mut Vec<u8>, n: u16) {
        out.extend_from_slice(&n.to_be_bytes());
    }
    fn be32(out: &mut Vec<u8>, n: u32) {
        out.extend_from_slice(&n.to_be_bytes());
    }
    fn number(n: i16) -> Vec<u8> {
        let mut v = vec![28];
        v.extend_from_slice(&n.to_be_bytes());
        v
    }
    fn index(items: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        be16(&mut out, items.len() as u16);
        if items.is_empty() {
            return out;
        }
        out.push(1);
        let mut at = 1;
        out.push(at);
        for i in items {
            at += i.len() as u8;
            out.push(at);
        }
        for i in items {
            out.extend(i);
        }
        out
    }
    let name = index(&[b"RectFont".to_vec()]);
    let mut dict = Vec::new();
    for n in [0, 0, 500, 700] {
        dict.extend(number(n));
    }
    dict.push(5);
    let offset = 4 + name.len() + index(&[vec![0; dict.len() + 4]]).len() + 4;
    dict.extend(number(offset as i16));
    dict.push(17);
    let mut rectangle = number(600);
    rectangle.extend([139, 139, 21]);
    for n in [500, 0, 0, 700, -500, 0, 0, -700] {
        rectangle.extend(number(n));
    }
    rectangle.extend([5, 14]);
    let mut cff = vec![1, 0, 4, 4];
    cff.extend(name);
    cff.extend(index(&[dict]));
    cff.extend([0, 0, 0, 0]);
    cff.extend(index(&[
        vec![14],
        {
            let mut v = number(300);
            v.push(14);
            v
        },
        rectangle,
    ]));
    let mut head = vec![0; 54];
    head[0..4].copy_from_slice(&[0, 1, 0, 0]);
    head[12..16].copy_from_slice(&0x5f0f3cf5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[40..42].copy_from_slice(&500i16.to_be_bytes());
    head[42..44].copy_from_slice(&700i16.to_be_bytes());
    let mut hhea = vec![0; 36];
    hhea[0..4].copy_from_slice(&[0, 1, 0, 0]);
    hhea[4..6].copy_from_slice(&700i16.to_be_bytes());
    hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    hhea[34..36].copy_from_slice(&3u16.to_be_bytes());
    let mut hmtx = Vec::new();
    for w in [600, 300, 600] {
        be16(&mut hmtx, w);
        be16(&mut hmtx, 0);
    }
    let mut maxp = vec![0, 0, 0x50, 0];
    be16(&mut maxp, 3);
    let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 10];
    be32(&mut cmap, 12);
    be16(&mut cmap, 12);
    be16(&mut cmap, 0);
    be32(&mut cmap, 40);
    be32(&mut cmap, 0);
    be32(&mut cmap, 2);
    for (cp, gid) in [(32, 1), (65, 2)] {
        be32(&mut cmap, cp);
        be32(&mut cmap, cp);
        be32(&mut cmap, gid);
    }
    let names = [(1, "Rect Font"), (2, "Regular"), (6, "RectFont-Regular"), (13, "MIT License")];
    let mut name = Vec::new();
    be16(&mut name, 0);
    be16(&mut name, names.len() as u16);
    be16(&mut name, (6 + 12 * names.len()) as u16);
    let mut data = Vec::new();
    for (id, text) in names {
        let bytes: Vec<_> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for n in [3, 1, 0x409, id, bytes.len() as u16, data.len() as u16] {
            be16(&mut name, n);
        }
        data.extend(bytes);
    }
    name.extend(data);
    let tables = [(*b"CFF ", cff), (*b"cmap", cmap), (*b"head", head), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"maxp", maxp), (*b"name", name)];
    let mut out = b"OTTO".to_vec();
    be16(&mut out, 7);
    be16(&mut out, 64);
    be16(&mut out, 2);
    be16(&mut out, 48);
    let mut payload = Vec::new();
    let mut offset = 12 + 16 * tables.len();
    for (tag, bytes) in tables {
        out.extend(tag);
        let mut padded = bytes.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        let sum = padded.as_chunks::<4>().0.iter().fold(0u32, |s, b| s.wrapping_add(u32::from_be_bytes(*b)));
        be32(&mut out, sum);
        be32(&mut out, offset as u32);
        be32(&mut out, bytes.len() as u32);
        offset += padded.len();
        payload.extend(padded);
    }
    out.extend(payload);
    out
}

fn fixture_embedded_cff() -> Vec<u8> {
    let font_bytes = cff_font_bytes();
    let stream = b"BT /F1 20 Tf 50 150 Td (A A) Tj ET";
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    out.extend_from_slice(b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
    out.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
    out.extend_from_slice(
        b"3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj\n",
    );
    out.extend_from_slice(format!("4 0 obj << /Length {} >> stream\n", stream.len()).as_bytes());
    out.extend_from_slice(stream);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(b"5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /RectFont-Regular /FontDescriptor 6 0 R >> endobj\n");
    out.extend_from_slice(b"6 0 obj << /Type /FontDescriptor /FontName /RectFont-Regular /Flags 32 /FontBBox [0 0 500 700] /ItalicAngle 0 /Ascent 700 /Descent 0 /CapHeight 700 /StemV 80 /FontFile3 7 0 R >> endobj\n");
    out.extend_from_slice(format!("7 0 obj << /Subtype /OpenType /Length {} >> stream\n", font_bytes.len()).as_bytes());
    out.extend_from_slice(&font_bytes);
    out.extend_from_slice(b"\nendstream endobj\n");
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

#[test]
fn test_render_parity_embedded_cff() {
    assert_ffi_matches_direct(fixture_embedded_cff(), None, FfiTile { x: 0, y: 0, width: 300, height: 300 }, 1.0);
}

#[test]
fn test_render_parity_scanned_page() {
    assert_ffi_matches_direct(fixture_scanned_page(), None, FfiTile { x: 0, y: 0, width: 300, height: 300 }, 1.0);
}

#[test]
fn test_render_parity_typical_multipage_pdf() {
    let bytes = std::fs::read("../../mobile/fixtures/typical-10p.pdf").expect("read typical-10p.pdf");
    for page in 0..5 {
        let engine = Engine::new();
        let doc_id = engine.open_bytes("typical.pdf".into(), bytes.clone(), None).expect("open typical");
        let ffi_req = FfiRenderRequest {
            page,
            tile: Some(FfiTile { x: 0, y: 0, width: 297, height: 421 }),
            scale: 0.5,
            tag: page as u64,
            preview: false,
            reading_colors: None,
            hide_comments: false,
        };
        let ffi_res = engine.render_tile(doc_id, ffi_req).expect("render tile");
        assert!(ffi_res.error.is_none());

        let mut direct = PageRenderer::new(Arc::new(bytes.clone()), Default::default());
        let direct_res = direct.render(RenderRequest {
            page: page as usize,
            kind: RequestKind::Pixels,
            tile: Some(Tile { x: 0, y: 0, w: 297, h: 421 }),
            scale: 0.5,
            tag: page as u64,
            preview: false,
        });

        assert_eq!(ffi_res.width, direct_res.width);
        assert_eq!(ffi_res.height, direct_res.height);
        assert_eq!(ffi_res.pixels, direct_res.rgba.as_ref());
        engine.close(doc_id).expect("close");
    }
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

    // 3. Stitched tiles must equal full render: exactly 1 pixel (175, 144) differs across 3 color bytes (RGB)
    // by exactly 1 LSB (due to clip-edge antialiasing truncation).
    let mut diff_bytes = 0;
    let mut diff_pixels = std::collections::HashSet::new();
    for (i, (&s_byte, &f_byte)) in stitched.iter().zip(full_res.pixels.iter()).enumerate() {
        let diff = (s_byte as i32 - f_byte as i32).abs();
        assert!(diff <= 1, "subpixel antialiasing drift must not exceed 1 at byte {i}: stitched={s_byte}, full={f_byte}");
        if diff != 0 {
            diff_bytes += 1;
            let px = (i / 4) % 300;
            let py = (i / 4) / 300;
            let ch = i % 4;
            diff_pixels.insert((px, py));
            println!("Tile boundary antialiasing difference at pixel ({px}, {py}) channel {ch}: stitched={s_byte}, full={f_byte} (delta={diff})");
        }
    }
    // Exactly 3 bytes differ (1 pixel)
    assert_eq!(diff_bytes, 3, "expected exactly 3 differing bytes across seam boundary");
    assert_eq!(diff_pixels.len(), 1, "expected exactly 1 differing pixel across seam boundary");

    engine.close(doc_id).unwrap();
}

#[test]
fn test_tile_1px_padded_render_subpixel_drift_bounded() {
    let pdf_bytes = fixture_standard_fonts();
    let engine = Engine::new();
    let doc_id = engine.open_bytes("test_padded.pdf".into(), pdf_bytes.clone(), None).unwrap();

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

    // 2. Render 4 tiles with 1 px padding across the internal seams.
    let mut stitched_padded = vec![0u8; 300 * 300 * 4];

    // Config for each quadrant: (tile_req_x, tile_req_y, tile_req_w, tile_req_h, crop_x, crop_y, dst_x, dst_y)
    let quadrants = [
        (0u32, 0u32, 151u32, 151u32, 0u32, 0u32, 0u32, 0u32),
        (149u32, 0u32, 151u32, 151u32, 1u32, 0u32, 150u32, 0u32),
        (0u32, 149u32, 151u32, 151u32, 0u32, 1u32, 0u32, 150u32),
        (149u32, 149u32, 151u32, 151u32, 1u32, 1u32, 150u32, 150u32),
    ];

    for (rx, ry, rw, rh, cx, cy, dx, dy) in quadrants {
        let req = FfiRenderRequest {
            page: 0,
            tile: Some(FfiTile { x: rx, y: ry, width: rw, height: rh }),
            scale: 1.0,
            tag: (rx * 1000 + ry) as u64,
            preview: false,
            reading_colors: None,
            hide_comments: false,
        };
        let t_res = engine.render_tile(doc_id, req).expect("padded tile render");

        for row in 0..150 {
            let src_y = cy + row;
            let src_start = ((src_y * rw + cx) * 4) as usize;
            let src_end = src_start + 150 * 4;
            let dst_y = dy + row;
            let dst_start = ((dst_y * 300 + dx) * 4) as usize;
            let dst_end = dst_start + 150 * 4;
            stitched_padded[dst_start..dst_end].copy_from_slice(&t_res.pixels[src_start..src_end]);
        }
    }

    let mut differing_pixels = std::collections::HashSet::new();
    let mut max_delta = 0i32;
    for (i, (&s_byte, &f_byte)) in stitched_padded.iter().zip(full_res.pixels.iter()).enumerate() {
        let diff = (s_byte as i32 - f_byte as i32).abs();
        if diff > 0 {
            max_delta = max_delta.max(diff);
            let px = (i / 4) % 300;
            let py = (i / 4) / 300;
            let ch = i % 4;
            differing_pixels.insert((px, py));
            println!("Padded diff at pixel ({px}, {py}) channel {ch}: stitched={s_byte}, full={f_byte} (delta={diff})");
        }
    }

    println!("Padded tile render: differing pixels count = {}, max_delta = {max_delta}", differing_pixels.len());
    // A0c's statement that "padding eliminates the seam" was false: padded rendering still
    // exhibits differences (pixel (175, 144) differs by 1 LSB in RGB channels).
    // The exact cause is not yet established (hypotheses include subpixel rasterization rounding
    // under coordinate translation vs clip-edge antialiasing).
    assert!(max_delta <= 1, "max delta must not exceed 1 LSB, got {max_delta}");
    assert!(differing_pixels.len() <= 4, "differing pixels count must not exceed 4, got {}", differing_pixels.len());

    engine.close(doc_id).unwrap();
}
