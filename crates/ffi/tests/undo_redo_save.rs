use pdfcraft_ffi::{Engine, FfiPoint, FfiRect, FfiRenderRequest, FfiShapeKind};
use tempfile::tempdir;

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
fn test_undo_redo_and_dirty_tracking() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("test.pdf".into(), pdf, None).expect("open pdf");

    // Baseline: clean state
    assert_eq!(engine.can_undo(doc_id).expect("can_undo"), None);
    assert_eq!(engine.can_redo(doc_id).expect("can_redo"), None);
    assert!(!engine.is_dirty(doc_id).expect("is_dirty"));

    // Render baseline
    let render_req = FfiRenderRequest { page: 0, tile: None, scale: 1.0, tag: 1, preview: false, reading_colors: None, hide_comments: false };
    let baseline_render = engine.render_tile(doc_id, render_req.clone()).expect("render baseline");

    // 1. Add sticky note
    engine.add_note_annotation(doc_id, 0, FfiPoint { x: 100.0, y: 150.0 }, "First edit".into(), Some("Author".into()), None, None).expect("add note");

    assert!(engine.is_dirty(doc_id).expect("is_dirty"));
    assert!(engine.can_undo(doc_id).expect("can_undo").is_some());
    assert_eq!(engine.can_redo(doc_id).expect("can_redo"), None);

    // 2. Add shape
    let rect = FfiRect { x: 200.0, y: 200.0, width: 80.0, height: 60.0 };
    engine.add_shape_annotation(doc_id, 0, FfiShapeKind::Rectangle, rect, None, None, None, Some("Author".into())).expect("add shape");

    let edited_render = engine.render_tile(doc_id, render_req.clone()).expect("render edited");
    assert_ne!(baseline_render.pixels, edited_render.pixels);

    // 3. Undo shape
    let undone1 = engine.undo(doc_id).expect("undo shape");
    assert!(!undone1.is_empty());
    assert!(engine.can_undo(doc_id).expect("can_undo").is_some());
    assert!(engine.can_redo(doc_id).expect("can_redo").is_some());

    // 4. Undo sticky note back to initial
    let undone2 = engine.undo(doc_id).expect("undo note");
    assert!(!undone2.is_empty());
    assert_eq!(engine.can_undo(doc_id).expect("can_undo"), None);
    assert!(engine.can_redo(doc_id).expect("can_redo").is_some());

    // Verify rendered output equals baseline
    let back_render = engine.render_tile(doc_id, render_req.clone()).expect("render after undo all");
    assert_eq!(baseline_render.pixels, back_render.pixels);

    // 5. Redo sticky note and shape forward
    let redone1 = engine.redo(doc_id).expect("redo note");
    assert!(!redone1.is_empty());
    let redone2 = engine.redo(doc_id).expect("redo shape");
    assert!(!redone2.is_empty());
    assert_eq!(engine.can_redo(doc_id).expect("can_redo"), None);

    // Verify rendered output matches fully edited state
    let forward_render = engine.render_tile(doc_id, render_req).expect("render after redo all");
    assert_eq!(edited_render.pixels, forward_render.pixels);
}

#[test]
fn test_atomic_save_and_reopen() {
    let dir = tempdir().expect("tempdir");
    let target_path = dir.path().join("document.pdf");
    let target_str = target_path.to_str().expect("to_str").to_string();

    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("orig.pdf".into(), pdf, None).expect("open bytes");

    // Apply an edit
    engine
        .add_note_annotation(doc_id, 0, FfiPoint { x: 50.0, y: 80.0 }, "Persistent comment".into(), Some("Reviewer".into()), None, None)
        .expect("add note");
    assert!(engine.is_dirty(doc_id).expect("dirty"));

    // Save bytes directly
    let bytes = engine.save_bytes(doc_id).expect("save_bytes");
    assert!(bytes.starts_with(b"%PDF-"));

    // Save atomically to disk
    engine.save_to_path(doc_id, target_str.clone()).expect("save_to_path");
    assert!(!engine.is_dirty(doc_id).expect("not dirty after save"));

    // Ensure target file exists and no temporary files were left behind
    assert!(target_path.exists());
    let dir_entries: Vec<_> =
        std::fs::read_dir(dir.path()).expect("read_dir").filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(dir_entries, vec!["document.pdf"]);

    // Reopen saved file in a new engine and verify comment was persisted
    let engine2 = Engine::new();
    let reopened_id = engine2.open_file(target_str, None).expect("open saved file");
    let comments = engine2.page_comments(reopened_id, 0).expect("page_comments");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].contents.as_deref(), Some("Persistent comment"));
    assert_eq!(comments[0].author.as_deref(), Some("Reviewer"));
}

#[test]
fn test_autosave_snapshots_and_recovery() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("session_doc.pdf".into(), pdf, None).expect("open");

    // Initially clean -> 0 snapshots
    let snaps0 = engine.autosave_snapshots().expect("autosave 0");
    assert!(snaps0.is_empty());

    // Edit document
    engine
        .add_note_annotation(doc_id, 0, FfiPoint { x: 120.0, y: 120.0 }, "Unsaved work".into(), Some("Worker".into()), None, None)
        .expect("add note");

    // First call after edit -> 1 snapshot
    let snaps1 = engine.autosave_snapshots().expect("autosave 1");
    assert_eq!(snaps1.len(), 1);
    assert_eq!(snaps1[0].doc_id, doc_id);
    assert_eq!(snaps1[0].name, "session_doc.pdf");
    assert!(!snaps1[0].encrypted);
    assert!(snaps1[0].bytes.starts_with(b"%PDF-"));

    // Immediate second call without new edits -> 0 snapshots (already captured)
    let snaps2 = engine.autosave_snapshots().expect("autosave 2");
    assert!(snaps2.is_empty());

    // Apply second edit -> snapshot taken again
    engine
        .add_shape_annotation(
            doc_id,
            0,
            FfiShapeKind::Oval,
            FfiRect { x: 100.0, y: 100.0, width: 50.0, height: 50.0 },
            None,
            None,
            None,
            Some("Worker".into()),
        )
        .expect("add oval");

    let snaps3 = engine.autosave_snapshots().expect("autosave 3");
    assert_eq!(snaps3.len(), 1);

    // Test mark_recovered
    engine.mark_recovered(doc_id, Some("/recovered/path.pdf".into())).expect("mark_recovered");
}
