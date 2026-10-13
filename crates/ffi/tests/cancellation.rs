use pdfcraft_ffi::{CancellationTracker, Engine};

#[test]
fn test_cancellation_tracker() {
    let tracker = CancellationTracker::new();
    assert!(!tracker.is_cancelled(10));

    tracker.retire_older_than(15);
    assert!(tracker.is_cancelled(10));
    assert!(tracker.is_cancelled(14));
    assert!(!tracker.is_cancelled(15));
    assert!(!tracker.is_cancelled(20));

    tracker.retire_older_than(25);
    assert!(tracker.is_cancelled(20));
    assert!(!tracker.is_cancelled(25));
}

#[test]
fn test_engine_cancel_renders() {
    let engine = Engine::new();
    engine.cancel_renders(100);
    // Should not panic or fail
    engine.cancel_renders(200);
}

#[test]
fn test_render_cancelled_by_tag() {
    use pdfcraft_ffi::{FfiRenderRequest, FfiTile};

    let engine = Engine::new();
    let pdf = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >> endobj
4 0 obj << /Length 21 >> stream
1 0 0 rg 0 0 50 50 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";

    let doc_id = engine.open_bytes("test.pdf".into(), pdf.to_vec(), None).expect("open bytes");

    // Request with tag 10
    let req_stale = FfiRenderRequest {
        page: 0,
        tile: Some(FfiTile { x: 0, y: 0, width: 50, height: 50 }),
        scale: 1.0,
        tag: 10,
        preview: false,
        reading_colors: None,
        hide_comments: false,
    };

    // Cancel all renders older than tag 15
    engine.cancel_renders(15);

    // Stale render must be cancelled
    let res_stale = engine.render_tile(doc_id, req_stale).expect("render stale tile");
    assert_eq!(res_stale.error.as_deref(), Some("request cancelled"));
    assert!(res_stale.pixels.is_empty());
    assert_eq!(res_stale.width, 0);
    assert_eq!(res_stale.height, 0);

    // Request with tag 20 (not retired)
    let req_fresh = FfiRenderRequest {
        page: 0,
        tile: Some(FfiTile { x: 0, y: 0, width: 50, height: 50 }),
        scale: 1.0,
        tag: 20,
        preview: false,
        reading_colors: None,
        hide_comments: false,
    };

    let res_fresh = engine.render_tile(doc_id, req_fresh).expect("render fresh tile");
    assert!(res_fresh.error.is_none());
    assert_eq!(res_fresh.width, 50);
    assert_eq!(res_fresh.height, 50);
    assert_eq!(res_fresh.pixels.len(), 50 * 50 * 4);

    engine.close(doc_id).expect("close doc");
}
