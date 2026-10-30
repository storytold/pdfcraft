use pdfcraft_ffi::Engine;
use tempfile::tempdir;

fn sample_document_with_text() -> Vec<u8> {
    // Generate a clean PDF with known text strings: "CONFIDENTIAL SECRET AGENT" and "PUBLIC RECORD"
    let engine = Engine::new();
    let text_content = "CONFIDENTIAL SECRET AGENT\nPUBLIC RECORD LINE 2\n";
    let dir = tempdir().expect("tempdir");
    let txt_path = dir.path().join("memo.txt");
    std::fs::write(&txt_path, text_content).expect("write text");

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
fn test_redaction_mark_clear_and_audit() {
    let engine = Engine::new();
    let bytes = sample_document_with_text();
    let doc_id = engine.open_bytes("memo.pdf".into(), bytes, None).expect("open bytes");

    // 1. Initial audit should have 0 unapplied marks
    let findings = engine.audit_redactions(doc_id).expect("audit");
    let unapplied_count = findings.iter().filter(|f| f.kind == "unapplied-mark").count();
    assert_eq!(unapplied_count, 0);

    // 2. Mark "CONFIDENTIAL" for redaction
    let marked = engine.mark_redaction_text(doc_id, "CONFIDENTIAL".into(), Some("REDACTED".into())).expect("mark text");
    assert!(marked >= 1);

    // 3. Audit should now detect the unapplied redaction mark
    let findings_after_mark = engine.audit_redactions(doc_id).expect("audit after mark");
    assert!(findings_after_mark.iter().any(|f| f.kind == "unapplied-mark"), "expected unapplied-mark finding, got {findings_after_mark:?}");

    // 4. Clear redactions and verify mark is gone
    engine.clear_redactions(doc_id).expect("clear redactions");
    let findings_after_clear = engine.audit_redactions(doc_id).expect("audit after clear");
    let unapplied_after_clear = findings_after_clear.iter().filter(|f| f.kind == "unapplied-mark").count();
    assert_eq!(unapplied_after_clear, 0);

    engine.close(doc_id).expect("close");
}

#[test]
fn test_redaction_apply_and_audit_verification() {
    let engine = Engine::new();
    let bytes = sample_document_with_text();
    let doc_id = engine.open_bytes("memo.pdf".into(), bytes, None).expect("open bytes");

    // Check initial page text contains the secret
    let initial_text = engine.page_text(doc_id, 0).expect("page text");
    assert!(initial_text.contains("CONFIDENTIAL"));
    assert!(initial_text.contains("PUBLIC RECORD"));

    // 1. Mark "CONFIDENTIAL SECRET AGENT" for redaction
    let marked = engine.mark_redaction_text(doc_id, "CONFIDENTIAL SECRET AGENT".into(), Some("REDACTED".into())).expect("mark text");
    assert!(marked >= 1);

    // 2. Permanently apply redactions
    let apply_result = engine.apply_redactions(doc_id).expect("apply redactions");
    assert!(apply_result.applied_count >= 1);
    assert!(apply_result.audit_passed, "audit failed with findings: {:?}", apply_result.audit_findings);
    assert!(apply_result.audit_findings.is_empty());

    // 3. Verify text extraction after application no longer contains "CONFIDENTIAL"
    let text_after = engine.page_text(doc_id, 0).expect("page text after apply");
    assert!(!text_after.contains("CONFIDENTIAL"), "secret text must be removed from content stream");
    assert!(text_after.contains("PUBLIC RECORD"), "unredacted text must remain untouched");

    // 4. Save and inspect physical bytes
    let dir = tempdir().expect("tempdir");
    let saved_path = dir.path().join("redacted.pdf");
    engine.save_to_path(doc_id, saved_path.to_string_lossy().into()).expect("save to path");

    let saved_bytes = std::fs::read(&saved_path).expect("read saved pdf");
    assert!(!saved_bytes.windows(12).any(|w| w == b"CONFIDENTIAL"), "the physical saved PDF must not contain any trace of the redacted word");

    // 5. Reopen saved PDF and ensure audit passes cleanly
    let doc_reopened = engine.open_bytes("redacted.pdf".into(), saved_bytes, None).expect("reopen");
    let audit_reopened = engine.audit_redactions(doc_reopened).expect("audit reopened");
    assert_eq!(audit_reopened.len(), 0, "reopened redacted document must audit clean");

    engine.close(doc_id).expect("close original");
    engine.close(doc_reopened).expect("close reopened");
}
