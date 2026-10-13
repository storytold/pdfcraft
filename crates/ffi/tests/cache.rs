use pdfcraft_ffi::{Engine, FfiRenderRequest, FfiTile, ThumbnailDiskCache};

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
        let page_obj_num = 3 + i * 2;
        let content_obj_num = page_obj_num + 1;
        objs.push(format!("{page_obj_num} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_obj_num} 0 R >>\nendobj\n"));
        let stream = format!("BT /F1 24 Tf 100 700 Td (Cache Test Page {}) Tj ET", i + 1);
        objs.push(format!("{content_obj_num} 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n", stream.len(), stream));
    }
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
fn test_tile_cache_hit_and_eviction() {
    let pdf_bytes = synthetic_pdf(2);
    // 128x128 tile is 128*128*4 = 65,536 bytes.
    // Set budget to 100,000 bytes (holds exactly 1 tile, evicts on 2nd tile).
    let engine = Engine::with_tile_cache_bytes(100_000);
    let doc_id = engine.open_bytes("test_cache.pdf".into(), pdf_bytes, None).unwrap();

    let tile1 = FfiTile { x: 0, y: 0, width: 128, height: 128 };
    let req1 = FfiRenderRequest { page: 0, tile: Some(tile1), scale: 1.0, tag: 1, preview: false, reading_colors: None, hide_comments: false };

    // Initial render: cache miss
    let res1 = engine.render_tile(doc_id, req1.clone()).expect("render tile 1");
    assert_eq!(res1.width, 128);
    assert_eq!(res1.height, 128);
    assert_eq!(engine.cached_tile_count(), 1);
    assert_eq!(engine.cached_tile_bytes(), 65536);

    // Second render: cache hit
    let res1_cached = engine.render_tile(doc_id, req1).expect("render tile 1 from cache");
    assert_eq!(res1_cached.pixels, res1.pixels);
    assert_eq!(engine.cached_tile_count(), 1);

    // Render tile 2: requires 65,536 bytes, exceeding the 100,000 budget when combined with tile 1
    let tile2 = FfiTile { x: 128, y: 0, width: 128, height: 128 };
    let req2 = FfiRenderRequest { page: 0, tile: Some(tile2), scale: 1.0, tag: 2, preview: false, reading_colors: None, hide_comments: false };
    let res2 = engine.render_tile(doc_id, req2).expect("render tile 2");
    assert_eq!(res2.width, 128);

    // Tile 1 should have been evicted to respect the budget
    assert_eq!(engine.cached_tile_count(), 1);
    assert_eq!(engine.cached_tile_bytes(), 65536);

    engine.close(doc_id).unwrap();
    // Cache evicted on close
    assert_eq!(engine.cached_tile_count(), 0);
    assert_eq!(engine.cached_tile_bytes(), 0);
}

#[test]
fn test_tile_cache_memory_trim() {
    let pdf_bytes = synthetic_pdf(1);
    let engine = Engine::new();
    let doc_id = engine.open_bytes("test_trim.pdf".into(), pdf_bytes, None).unwrap();

    let req = FfiRenderRequest {
        page: 0,
        tile: Some(FfiTile { x: 0, y: 0, width: 128, height: 128 }),
        scale: 1.0,
        tag: 1,
        preview: false,
        reading_colors: None,
        hide_comments: false,
    };
    engine.render_tile(doc_id, req).expect("render tile");
    assert_eq!(engine.cached_tile_count(), 1);

    // Memory warning triggered
    engine.trim_memory().unwrap();
    assert_eq!(engine.cached_tile_count(), 0);
    assert_eq!(engine.cached_tile_bytes(), 0);

    engine.close(doc_id).unwrap();
}

#[test]
fn test_thumbnail_disk_cache() {
    let tmp_dir = tempfile::tempdir().expect("create temp dir");
    let disk_cache = ThumbnailDiskCache::new(tmp_dir.path());

    let fp = "doc_hash_12345";
    assert!(disk_cache.get(fp, 0, 160).is_none());

    let dummy_result =
        pdfcraft_ffi::FfiRenderResult { page: 0, width: 10, height: 10, tag: 0, pixels: vec![255; 400], error: None, render_time_ms: 5 };

    disk_cache.put(fp, 0, 160, &dummy_result).expect("write thumb to disk");

    let loaded = disk_cache.get(fp, 0, 160).expect("read thumb from disk");
    assert_eq!(loaded.width, 10);
    assert_eq!(loaded.height, 10);
    assert_eq!(loaded.pixels, dummy_result.pixels);

    disk_cache.clear().expect("clear thumb cache");
    assert!(disk_cache.get(fp, 0, 160).is_none());
}
