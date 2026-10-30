use pdfcraft_ffi::Engine;
use tempfile::tempdir;

fn sample_pdf() -> Vec<u8> {
    b"%PDF-1.7
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>
endobj
xref
0 4
0000000000 65535 f 
0000000009 00000 n 
000000058 00000 n 
000000115 00000 n 
trailer
<< /Size 4 /Root 1 0 R >>
startxref
193
%%EOF
"
    .to_vec()
}

fn sample_png() -> Vec<u8> {
    // 1x1 white PNG
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG magic
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, // IDAT
        0x08, 0xD7, 0x63, 0xF8, 0xFF, 0xFF, 0x3F, 0x00, 0x05, 0xFE, 0x02, 0xFE, 0xDC, 0xCC, 0x59, 0xE7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
        0x44, // IEND
        0xAE, 0x42, 0x60, 0x82,
    ]
}

#[test]
fn test_create_from_images_and_scan_pipeline() {
    let engine = Engine::new();
    let img = sample_png();
    let doc_id = engine.create_from_images("scan.pdf".into(), vec![img.clone(), img]).expect("create from images");

    assert_eq!(engine.page_count(doc_id).expect("page count"), 2);
    let geom = engine.page_geometry(doc_id, 0).expect("geometry");
    assert!(geom.width > 0.0);
    assert!(geom.height > 0.0);

    // Run enhance_camera tool on page 1 with explicit corners
    let enhance_args = serde_json::json!({
        "doc": doc_id,
        "page": 1,
        "corners": [0, 0, 1, 0, 1, 1, 0, 1],
        "mode": "colour",
        "level": 50
    })
    .to_string();
    let res = engine.invoke_tool("page_enhance_camera".into(), enhance_args).expect("enhance camera");
    assert!(res.success, "error: {:?}", res.error);

    engine.close(doc_id).expect("close");
}

#[test]
fn test_page_organization_operations() {
    let engine = Engine::new();
    let doc_id = engine.open_bytes("test.pdf".into(), sample_pdf(), None).expect("open");
    assert_eq!(engine.page_count(doc_id).expect("pages"), 1);

    // 1. Insert blank page
    let insert_res = engine.invoke_tool("page_insert_blank".into(), serde_json::json!({ "doc": doc_id, "at": 2 }).to_string()).expect("insert blank");
    assert!(insert_res.success);
    assert_eq!(engine.page_count(doc_id).expect("pages after insert"), 2);

    // 2. Rotate page 2
    let rotate_res =
        engine.invoke_tool("page_rotate".into(), serde_json::json!({ "doc": doc_id, "pages": [2], "degrees": 180 }).to_string()).expect("rotate");
    assert!(rotate_res.success);
    assert_eq!(engine.page_geometry(doc_id, 1).expect("geom").rotation, 180);

    // 3. Duplicate page 1
    let dup_res = engine.invoke_tool("page_duplicate".into(), serde_json::json!({ "doc": doc_id, "pages": [1] }).to_string()).expect("duplicate");
    assert!(dup_res.success);
    assert_eq!(engine.page_count(doc_id).expect("pages after dup"), 3);

    // 4. Delete page 3
    let del_res = engine.invoke_tool("page_delete".into(), serde_json::json!({ "doc": doc_id, "pages": [3] }).to_string()).expect("delete");
    assert!(del_res.success);
    assert_eq!(engine.page_count(doc_id).expect("pages after del"), 2);

    engine.close(doc_id).expect("close");
}

#[test]
fn test_watermark_header_footer_and_flatten() {
    let engine = Engine::new();
    let doc_id = engine.open_bytes("test.pdf".into(), sample_pdf(), None).expect("open");

    // 1. Add watermark
    let wm_res = engine
        .invoke_tool(
            "doc_watermark".into(),
            serde_json::json!({
                "doc": doc_id,
                "text": "CONFIDENTIAL",
                "opacity": 0.5,
                "rotation": 45
            })
            .to_string(),
        )
        .expect("watermark");
    assert!(wm_res.success);

    // 2. Add header & footer
    let hf_res = engine
        .invoke_tool(
            "doc_header_footer".into(),
            serde_json::json!({
                "doc": doc_id,
                "header_center": "Annual Report",
                "footer_right": "<<Page 1 of n>>"
            })
            .to_string(),
        )
        .expect("header footer");
    assert!(hf_res.success);

    // 3. Flatten
    let flat_res = engine.invoke_tool("doc_flatten".into(), serde_json::json!({ "doc": doc_id }).to_string()).expect("flatten");
    assert!(flat_res.success);

    engine.close(doc_id).expect("close");
}

#[test]
fn test_combine_and_split_documents() {
    let dir = tempdir().expect("tempdir");
    let file1 = dir.path().join("doc1.pdf");
    let file2 = dir.path().join("doc2.pdf");
    std::fs::write(&file1, sample_pdf()).expect("write doc1");
    std::fs::write(&file2, sample_pdf()).expect("write doc2");

    let combined_path = dir.path().join("combined.pdf");
    let combine_res = pdfcraft_ffi::invoke_tool(
        "doc_combine".into(),
        serde_json::json!({
            "paths": [file1.to_string_lossy(), file2.to_string_lossy()],
            "out": combined_path.to_string_lossy()
        })
        .to_string(),
    )
    .expect("combine");
    assert!(combine_res.success);
    assert!(combined_path.exists());

    // Split combined file
    let split_out_dir = dir.path().join("split_out");
    std::fs::create_dir(&split_out_dir).expect("create split dir");
    let engine = Engine::new();
    let combined_bytes = std::fs::read(&combined_path).expect("read combined");
    let doc_id = engine.open_bytes("combined.pdf".into(), combined_bytes, None).expect("open combined");
    assert_eq!(engine.page_count(doc_id).expect("pages"), 2);

    let split_res = engine
        .invoke_tool(
            "doc_split".into(),
            serde_json::json!({
                "doc": doc_id,
                "every": 1,
                "out_dir": split_out_dir.to_string_lossy()
            })
            .to_string(),
        )
        .expect("split");
    assert!(split_res.success);

    let parts: Vec<_> = std::fs::read_dir(&split_out_dir).expect("read split dir").filter_map(Result::ok).collect();
    assert_eq!(parts.len(), 2);

    engine.close(doc_id).expect("close");
}
