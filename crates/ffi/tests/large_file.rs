use pdfcraft_ffi::{Engine, FfiRenderRequest, FfiTile};
use std::time::Instant;

fn generate_large_document(page_count: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let mut objs: Vec<String> = Vec::new();

    // 1: Catalog
    objs.push("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".into());

    // 2: Pages root
    let mut kids = String::new();
    for i in 0..page_count {
        kids.push_str(&format!("{} 0 R ", 3 + i * 2));
    }
    objs.push(format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {page_count} >>\nendobj\n"));

    // Pages & Streams
    for i in 0..page_count {
        let page_obj_id = 3 + i * 2;
        let content_obj_id = 4 + i * 2;
        objs.push(format!("{page_obj_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_obj_id} 0 R >>\nendobj\n"));
        let stream = format!("BT /F1 12 Tf 72 720 Td (Page {i} of large benchmark document) Tj ET\n0 0 100 100 re f\n");
        objs.push(format!("{content_obj_id} 0 obj\n<< /Length {} >>\nstream\n{stream}endstream\nendobj\n", stream.len()));
    }

    for (i, obj) in objs.iter().enumerate() {
        offsets.push((i + 1, out.len()));
        out.extend_from_slice(obj.as_bytes());
    }

    let xref_offset = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for (_, off) in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_large_document_open_and_tile_render() {
    let page_count = 200;
    let data = generate_large_document(page_count);

    let engine = Engine::new();

    let open_start = Instant::now();
    let doc_id = engine.open_bytes("large_doc_spike.pdf".into(), data, None).expect("open large doc");
    let open_elapsed = open_start.elapsed();

    assert!(open_elapsed.as_millis() < 500, "200-page document open took {:?}, exceeding 500ms budget", open_elapsed);

    let count = engine.page_count(doc_id).expect("page count");
    assert_eq!(count as usize, page_count);

    // Render random page tile
    let render_start = Instant::now();
    let req = FfiRenderRequest { page: 150, tile: Some(FfiTile { x: 0, y: 0, width: 512, height: 512 }), scale: 2.0, tag: 1, ..Default::default() };
    let res = engine.render_tile(doc_id, req).expect("render tile");
    let render_elapsed = render_start.elapsed();

    assert!(res.error.is_none());
    assert_eq!(res.width, 512);
    assert_eq!(res.height, 512);
    assert!(render_elapsed.as_millis() < 100, "large doc tile render took {:?}, exceeding 100ms", render_elapsed);

    engine.close(doc_id).expect("close");
}
