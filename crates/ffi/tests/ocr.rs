#![cfg(feature = "ocr")]
use pdfcraft_ffi::{Engine, FfiOcrWord, FfiRect, FfiRenderRequest};
use tempfile::tempdir;

fn sample_document() -> Vec<u8> {
    let engine = Engine::new();
    let dir = tempdir().expect("tempdir");
    let txt_path = dir.path().join("doc.txt");
    std::fs::write(&txt_path, "First line of sample page\n").expect("write text");

    let create_args = serde_json::json!({
        "from": "text",
        "path": txt_path.to_string_lossy(),
    })
    .to_string();

    let res = engine.invoke_tool("doc_create".into(), create_args).expect("invoke doc_create");
    assert!(res.success, "doc_create failed: {:?}", res.error);
    let parsed: serde_json::Value = serde_json::from_str(&res.output_json).expect("parse json");
    let doc_id = parsed[0]["doc"].as_u64().expect("doc id");

    let out_pdf = dir.path().join("out.pdf");
    let save_res =
        engine.invoke_tool("doc_save".into(), serde_json::json!({ "doc": doc_id, "path": out_pdf.to_string_lossy() }).to_string()).expect("save doc");
    assert!(save_res.success);

    let bytes = std::fs::read(&out_pdf).expect("read generated pdf");
    engine.close(doc_id).expect("close doc");
    bytes
}

#[test]
fn test_ocr_status_and_custom_models_dir() {
    let engine = Engine::new();

    // Query status
    let status = engine.ocr_status().expect("ocr_status");
    assert!(!status.languages.is_empty(), "languages should list English");
    assert!(status.languages.iter().any(|l| l.contains("en: English")));
    assert!(!status.search_dirs.is_empty(), "search_dirs should contain default dirs");

    // Setting an invalid models dir should fail with InvalidArgs
    let err = engine.set_ocr_models_dir("/nonexistent/directory/for/ocr/models".into());
    assert!(err.is_err(), "nonexistent dir must error");

    // Setting a valid directory should succeed and appear in search_dirs
    let temp_models_dir = tempdir().expect("tempdir");
    engine.set_ocr_models_dir(temp_models_dir.path().to_string_lossy().into_owned()).expect("set_ocr_models_dir");

    let updated_status = engine.ocr_status().expect("updated status");
    assert!(
        updated_status.search_dirs.iter().any(|d| d == &temp_models_dir.path().to_string_lossy()),
        "custom models dir must appear in search_dirs"
    );
}

#[test]
fn test_apply_ocr_text_layer_and_searchability() {
    let engine = Engine::new();
    let bytes = sample_document();
    let doc_id = engine.open_bytes("scanned.pdf".into(), bytes, None).expect("open bytes");

    // Inject externally recognized words into page 0 (e.g. from platform Apple Vision / ML Kit)
    let words = vec![
        FfiOcrWord { text: "PATENT".into(), rect: FfiRect { x: 50.0, y: 100.0, width: 80.0, height: 20.0 } },
        FfiOcrWord { text: "INSPECTION".into(), rect: FfiRect { x: 140.0, y: 100.0, width: 110.0, height: 20.0 } },
    ];

    let applied_count = engine.apply_ocr_text_layer(doc_id, 0, words).expect("apply_ocr_text_layer");
    assert_eq!(applied_count, 2);

    // Verify text extraction includes the newly injected searchable words
    let page_text = engine.page_text(doc_id, 0).expect("page_text");
    assert!(page_text.contains("PATENT"), "extracted text should contain PATENT: got {}", page_text);
    assert!(page_text.contains("INSPECTION"), "extracted text should contain INSPECTION: got {}", page_text);

    // Verify find_text locates the OCR word
    let matches = engine.find_text(doc_id, 0, "PATENT".into(), false).expect("find_text");
    assert_eq!(matches.len(), 1);
    assert!(matches[0].rect.width > 0.0);

    // Verify rendering succeeds without error after OCR text layer injection
    let render_res = engine
        .render_tile(doc_id, FfiRenderRequest { page: 0, scale: 1.0, tile: None, reading_colors: None, hide_comments: false, preview: false, tag: 0 })
        .expect("render_tile");
    assert_eq!(render_res.error, None);
    assert!(!render_res.pixels.is_empty());

    engine.close(doc_id).expect("close");
}

#[test]
fn test_recognize_page_text_fails_gracefully_when_models_missing() {
    let engine = Engine::new();
    if !engine.is_ocr_available() {
        let bytes = sample_document();
        let doc_id = engine.open_bytes("sample.pdf".into(), bytes, None).expect("open bytes");
        let res = engine.recognize_page_text(doc_id, 0, Some(300.0), true);
        assert!(res.is_err(), "must return Unsupported error when models are not installed");
        engine.close(doc_id).expect("close");
    }
}
