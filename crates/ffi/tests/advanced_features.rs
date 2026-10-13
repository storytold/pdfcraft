use pdfcraft_ffi::Engine;
use tempfile::tempdir;

fn sample_pdf_with_text(text: &str) -> Vec<u8> {
    let engine = Engine::new();
    let dir = tempdir().expect("tempdir");
    let txt_path = dir.path().join("text.txt");
    std::fs::write(&txt_path, text).expect("write text");

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
#[cfg(feature = "js")]
fn test_form_javascript_controls_and_execution() {
    let engine = Engine::new();
    let bytes = sample_pdf_with_text("Document for JavaScript testing\n");
    let doc_id = engine.open_bytes("js_test.pdf".into(), bytes, None).expect("open bytes");

    // 1. Check default state is enabled
    assert!(engine.is_form_javascript_enabled().expect("is_js_enabled"));

    // 2. Toggle off
    engine.set_form_javascript_enabled(false).expect("disable js");
    assert!(!engine.is_form_javascript_enabled().expect("is_js_enabled"));

    // 3. Execution when disabled should error cleanly
    let res_disabled = engine.run_form_javascript(doc_id, "console.println('test');".into(), None);
    assert!(res_disabled.is_err());

    // 4. Re-enable and run script
    engine.set_form_javascript_enabled(true).expect("enable js");
    let res_enabled = engine.run_form_javascript(doc_id, "console.println('hello from mobile js');".into(), None).expect("run js");
    assert!(res_enabled.success);
    assert!(res_enabled.output.contains("hello from mobile js"));

    // 5. Take output for document edits
    let output = engine.take_form_javascript_output(doc_id).expect("take output");
    assert!(output.errors.is_empty());

    engine.close(doc_id).expect("close");
}

#[test]
#[cfg(feature = "xfa")]
fn test_xfa_status_and_datasets() {
    let engine = Engine::new();
    let standard_bytes = sample_pdf_with_text("Standard non-XFA PDF\n");
    let doc_id = engine.open_bytes("standard.pdf".into(), standard_bytes, None).expect("open bytes");

    let status = engine.get_xfa_status(doc_id).expect("get_xfa_status");
    assert_eq!(status.kind, "none");
    assert_eq!(status.layout_pages, 0);
    assert_eq!(status.layout_fields, 0);

    engine.close(doc_id).expect("close");
}

#[test]
#[cfg(feature = "preflight")]
fn test_pdfa_preflight_and_conversion() {
    let engine = Engine::new();
    let bytes = sample_pdf_with_text("PDF/A Preflight Document\n");
    let doc_id = engine.open_bytes("preflight.pdf".into(), bytes, None).expect("open bytes");

    // 1. Declared standards
    let declared = engine.declared_standards(doc_id).expect("declared_standards");
    assert_eq!(declared.pdfa_part, None); // Not yet PDF/A declared

    // 2. Verify PDF/A-2b compliance
    let issues = engine.verify_pdfa(doc_id, "pdfa2b".into()).expect("verify_pdfa");
    // Standard generated doc lacks PDF/A XMP metadata and output intent
    assert!(!issues.is_empty(), "expected preflight issues on standard PDF");
    assert!(issues.iter().any(|i| i.clause.contains("6.1.2") || i.message.contains("metadata")));

    // 3. Convert to PDF/A-2b
    let remaining_unfixable = engine.convert_to_pdfa(doc_id, "pdfa2b".into()).expect("convert_to_pdfa");

    // 4. Standards should now declare PDF/A-2b
    let declared_after = engine.declared_standards(doc_id).expect("declared after");
    assert_eq!(declared_after.pdfa_part, Some(2));
    assert_eq!(declared_after.pdfa_conformance, Some("B".into()));

    // Verify remaining count matches fresh verification
    let issues_after = engine.verify_pdfa(doc_id, "pdfa2b".into()).expect("verify after");
    assert_eq!(issues_after.len() as u32, remaining_unfixable);

    engine.close(doc_id).expect("close");
}

#[test]
fn test_document_compare_text_visual_and_marks() {
    let engine = Engine::new();
    let old_bytes = sample_pdf_with_text("The quick brown fox jumps over the lazy dog\n");
    let new_bytes = sample_pdf_with_text("The fast brown fox leaps over the sleepy dog\n");

    let old_id = engine.open_bytes("old.pdf".into(), old_bytes, None).expect("open old");
    let new_id = engine.open_bytes("new.pdf".into(), new_bytes, None).expect("open new");

    // 1. Text compare
    let changes = engine.compare_documents(old_id, new_id).expect("compare_documents");
    assert!(!changes.is_empty(), "expected differences between old and new text");
    assert!(changes.iter().any(|c| c.old_side.text.contains("quick") || c.new_side.text.contains("fast")));

    // 2. Visual compare
    let visual_diffs = engine.compare_documents_visual(old_id, new_id, Some(72.0)).expect("compare visual");
    assert!(!visual_diffs.is_empty(), "expected visual differences");
    assert!(visual_diffs[0].rect.width > 0.0);

    // 3. Mark differences as annotations in the newer document
    let marked = engine.mark_document_differences(old_id, new_id).expect("mark differences");
    assert!(marked >= 1, "expected comments added for differences");

    // Verify comments were added to newer document
    let comments = engine.page_comments(new_id, 0).expect("page comments");
    assert!(comments.iter().any(|c| c.author.as_deref() == Some("Compare")));

    engine.close(old_id).expect("close old");
    engine.close(new_id).expect("close new");
}

#[test]
fn test_accessibility_checker_all_32_rules() {
    let engine = Engine::new();
    let bytes = sample_pdf_with_text("Accessible document testing heading\n");
    let doc_id = engine.open_bytes("a11y.pdf".into(), bytes, None).expect("open bytes");

    let report = engine.check_accessibility(doc_id).expect("check_accessibility");
    assert_eq!(report.results.len(), 32, "all 32 Acrobat rules must be evaluated");
    let sum = report.passed_count + report.failed_count + report.manual_check_count + report.skipped_count;
    assert_eq!(sum, 32);

    // Verify key categories exist
    assert!(report.results.iter().any(|r| r.category == "Document"));
    assert!(report.results.iter().any(|r| r.category == "Page Content"));
    assert!(report.results.iter().any(|r| r.category == "Forms"));
    assert!(report.results.iter().any(|r| r.category == "Alternate Text"));

    engine.close(doc_id).expect("close");
}

#[test]
fn test_cjk_font_status_and_fallback() {
    let engine = Engine::new();
    let status = engine.get_cjk_font_status();

    // Verify consistency with underlying pdfcraft_fonts state
    assert_eq!(status.has_japanese_font, pdfcraft_fonts::document_japanese_font().is_some());
    assert_eq!(status.has_chinese_font, !pdfcraft_fonts::ui_chinese_fonts().is_empty());
    assert_eq!(status.craft_fonts_count as usize, pdfcraft_fonts::CRAFT_FONTS.len());

    let bytes = sample_pdf_with_text("CJK fallback test doc\n");
    let doc_id = engine.open_bytes("cjk.pdf".into(), bytes, None).expect("open bytes");

    let rect = pdfcraft_ffi::FfiRect { x: 72.0, y: 72.0, width: 200.0, height: 50.0 };
    let res = engine.add_text_box_annotation(doc_id, 0, rect, "日本語テスト".into(), 12.0, None, None);
    assert!(res.is_ok(), "adding Japanese annotation must never panic or crash");

    engine.close(doc_id).expect("close");
}
