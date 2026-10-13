use pdfcraft_ffi::{Engine, FfiPrefetchPriority, FfiRenderRequest, FfiScrollDirection, RequestCoalescer, TileCacheKey, plan_prefetch_pages};

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
fn test_direction_aware_prefetch_planner() {
    // Empty document
    assert!(plan_prefetch_pages(0, 0, FfiScrollDirection::Forward, 2).is_empty());

    // 10 pages forward from page 0
    let plans_fwd = plan_prefetch_pages(0, 10, FfiScrollDirection::Forward, 2);
    assert_eq!(plans_fwd[0].page, 0);
    assert_eq!(plans_fwd[0].priority, FfiPrefetchPriority::Visible);
    assert!(!plans_fwd[0].preview);

    // Adjacent forward: pages 1, 2
    assert_eq!(plans_fwd[1].page, 1);
    assert_eq!(plans_fwd[1].priority, FfiPrefetchPriority::Adjacent);
    assert_eq!(plans_fwd[2].page, 2);
    assert_eq!(plans_fwd[2].priority, FfiPrefetchPriority::Adjacent);

    // Thumbnails further ahead: pages 3, 4, 5, 6
    assert_eq!(plans_fwd[3].page, 3);
    assert_eq!(plans_fwd[3].priority, FfiPrefetchPriority::Thumbnail);
    assert!(plans_fwd[3].preview);

    // Backward from page 5
    let plans_bwd = plan_prefetch_pages(5, 10, FfiScrollDirection::Backward, 2);
    assert_eq!(plans_bwd[0].page, 5);
    assert_eq!(plans_bwd[0].priority, FfiPrefetchPriority::Visible);
    // Behind pages: 4, 3
    assert_eq!(plans_bwd[1].page, 4);
    assert_eq!(plans_bwd[1].priority, FfiPrefetchPriority::Adjacent);
    assert_eq!(plans_bwd[2].page, 3);
    assert_eq!(plans_bwd[2].priority, FfiPrefetchPriority::Adjacent);
    // Page ahead in case of bounce: 6
    assert_eq!(plans_bwd[3].page, 6);
    assert_eq!(plans_bwd[3].priority, FfiPrefetchPriority::Adjacent);
    // Thumbnails further behind: 2, 1, 0
    assert_eq!(plans_bwd[4].page, 2);
    assert_eq!(plans_bwd[4].priority, FfiPrefetchPriority::Thumbnail);
    assert!(plans_bwd[4].preview);

    // Idle at page 3
    let plans_idle = plan_prefetch_pages(3, 10, FfiScrollDirection::Idle, 1);
    assert_eq!(plans_idle[0].page, 3);
    assert_eq!(plans_idle[0].priority, FfiPrefetchPriority::Visible);
    assert_eq!(plans_idle[1].page, 4);
    assert_eq!(plans_idle[1].priority, FfiPrefetchPriority::Adjacent);
    assert_eq!(plans_idle[2].page, 2);
    assert_eq!(plans_idle[2].priority, FfiPrefetchPriority::Adjacent);
}

#[test]
fn test_request_coalescer() {
    let coalescer = RequestCoalescer::new();
    let req = FfiRenderRequest { page: 0, scale: 1.0, ..Default::default() };
    let key = TileCacheKey::from_request(1, 1, &req);

    assert_eq!(coalescer.current_generation(), 1);
    assert!(coalescer.try_acquire(&key));
    // Second attempt for same key is coalesced
    assert!(!coalescer.try_acquire(&key));
    assert_eq!(coalescer.in_flight_count(), 1);

    coalescer.release(&key);
    assert_eq!(coalescer.in_flight_count(), 0);
    // Can acquire again after release
    assert!(coalescer.try_acquire(&key));

    // Generation advance
    let next_gen = coalescer.next_generation();
    assert_eq!(next_gen, 2);
    assert_eq!(coalescer.current_generation(), 2);
}

#[test]
fn test_engine_prefetch_and_generation_cancellation() {
    let engine = Engine::new();
    let pdf = synthetic_pdf(5);
    let doc_id = engine.open_bytes("prefetch_test.pdf".into(), pdf, None).expect("open");

    // Compute prefetch plan
    let plan = engine.plan_prefetch(0, 5, FfiScrollDirection::Forward, 2);
    assert_eq!(plan.len(), 5);

    // Generation tags
    let tag1 = engine.current_render_tag();
    let req1 = FfiRenderRequest { page: 0, tag: tag1, ..Default::default() };
    let res1 = engine.render_tile(doc_id, req1).expect("render");
    assert!(res1.error.is_none());
    assert_eq!(res1.tag, tag1);

    // User zooms/scrolls fast: advance generation and cancel older tags
    let tag2 = engine.next_render_tag();
    assert!(tag2 > tag1);
    engine.cancel_renders(tag2);

    // Render with old tag1 should be cancelled
    let stale_req = FfiRenderRequest { page: 1, tag: tag1, ..Default::default() };
    let stale_res = engine.render_tile(doc_id, stale_req).expect("render stale");
    assert_eq!(stale_res.error.as_deref(), Some("request cancelled"));
    assert!(stale_res.pixels.is_empty());

    // Render with fresh tag2 succeeds
    let fresh_req = FfiRenderRequest { page: 1, tag: tag2, ..Default::default() };
    let fresh_res = engine.render_tile(doc_id, fresh_req).expect("render fresh");
    assert!(fresh_res.error.is_none());
    assert!(!fresh_res.pixels.is_empty());
}
