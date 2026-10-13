use pdfcraft_ffi::{Engine, FfiMemoryPressureLevel, FfiRenderRequest, FfiTile};

fn synthetic_pdf(pages: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let mut objs: Vec<String> = Vec::new();
    objs.push("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string());
    let mut kids = String::new();
    for i in 0..pages {
        kids.push_str(&format!("{} 0 R ", 3 + i * 2));
    }
    objs.push(format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {pages} >>\nendobj\n"));
    for i in 0..pages {
        let page_obj_id = 3 + i * 2;
        let content_obj_id = 4 + i * 2;
        objs.push(format!("{page_obj_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents {content_obj_id} 0 R >>\nendobj\n"));
        let stream = "0 0 200 200 re f\n";
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
fn test_stepwise_memory_pressure_degradation() {
    // 200 KB cache budget for testing
    let engine = Engine::with_tile_cache_bytes(200 * 1024);
    let pdf = synthetic_pdf(4);
    let doc_id = engine.open_bytes("pressure.pdf".into(), pdf, None).expect("open");

    // Render 4 tiles to populate the cache
    for p in 0..4 {
        let req = FfiRenderRequest {
            page: p,
            tile: Some(FfiTile { x: 0, y: 0, width: 100, height: 100 }),
            scale: 1.0,
            tag: p as u64 + 1,
            ..Default::default()
        };
        let res = engine.render_tile(doc_id, req).expect("render");
        assert!(res.error.is_none());
    }

    let initial_count = engine.cached_tile_count();
    let initial_bytes = engine.cached_tile_bytes();
    assert_eq!(initial_count, 4);
    assert!(initial_bytes > 0);

    // 1. Moderate pressure: trims cache to at most half budget (100 KB)
    engine.handle_memory_pressure(FfiMemoryPressureLevel::Moderate).expect("moderate pressure");
    let mod_bytes = engine.cached_tile_bytes();
    let mod_count = engine.cached_tile_count();
    assert!(mod_bytes <= 100 * 1024, "moderate pressure must trim to 50% budget");
    assert!(mod_count > 0, "moderate pressure must keep warm entries");

    // 2. Critical pressure: clears all in-memory tile cache
    engine.handle_memory_pressure(FfiMemoryPressureLevel::Critical).expect("critical pressure");
    assert_eq!(engine.cached_tile_count(), 0);
    assert_eq!(engine.cached_tile_bytes(), 0);

    // 3. Engine remains completely usable immediately after critical pressure
    let fresh_req =
        FfiRenderRequest { page: 0, tile: Some(FfiTile { x: 0, y: 0, width: 100, height: 100 }), scale: 1.0, tag: 100, ..Default::default() };
    let fresh_res = engine.render_tile(doc_id, fresh_req).expect("render fresh");
    assert!(fresh_res.error.is_none());
    assert_eq!(engine.cached_tile_count(), 1);
}
