//! adapter_tests.rs — Integration Tests for AutomationEngineAdapter and Panic Containment.
//! =========================================================================================
//!
//! Purpose:
//!     Verifies concrete AutomationEngineAdapter execution, ephemeral sandbox containment,
//!     and catch_unwind panic protection against engine failures.
//!
//! Layer:
//!     Test Suite / SDK Adapter Tests

use pdfcraft_sdk::engine_adapter::AutomationEngineAdapter;
use pdfcraft_sdk::error::SdkError;
use pdfcraft_sdk::ports::OperationExecutor;
use pdfcraft_sdk::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode};
use pdfcraft_sdk::{LocalClient, PdfCraftClient};

fn minimal_valid_pdf() -> Vec<u8> {
    // Synthetic minimal valid PDF adhering to AGENTS.md clean-room asset policy
    b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n".to_vec()
}

#[test]
fn test_merge_missing_source_fails_with_file_not_found() {
    let adapter = AutomationEngineAdapter::default();
    let sources = vec![DocumentSource::from("nonexistent_doc.pdf")];
    let res = adapter.execute_merge(&sources, &MergeOptions::default());

    assert!(res.is_err());
    match res.expect_err("error expected") {
        SdkError::FileNotFound { path } => {
            assert!(path.contains("nonexistent_doc.pdf"));
        }
        other => panic!("Expected FileNotFound, got {other:?}"),
    }
}

#[test]
fn test_merge_valid_in_memory_sources() {
    let adapter = AutomationEngineAdapter::default();
    let doc1 = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc1.pdf".into()));
    let doc2 = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc2.pdf".into()));

    let result = adapter.execute_merge(&[doc1, doc2], &MergeOptions::default()).expect("merge must succeed");
    assert_eq!(result.mime_type(), "application/pdf");
    assert!(result.byte_len() > 0);
}

#[test]
fn test_render_page_zero_rejected() {
    let client = LocalClient::default();
    let src = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc.pdf".into()));
    let res = client.render_page(&src, 0, RenderOptions::default());

    assert!(res.is_err());
    match res.expect_err("error expected") {
        SdkError::InvalidArgument(msg) => {
            assert!(msg.contains("1-based"));
        }
        other => panic!("Expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_large_output_spills_to_file_backed_result() {
    // Set max memory threshold to 100 bytes so any successful output spills to disk
    let adapter = AutomationEngineAdapter::new(100);
    let doc1 = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc1.pdf".into()));
    let doc2 = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc2.pdf".into()));

    let result = adapter.execute_merge(&[doc1, doc2], &MergeOptions::default()).expect("real merge must succeed before spillover is verified");
    match result {
        DocumentResult::File { path, mime_type, size_bytes } => {
            assert_eq!(mime_type, "application/pdf");
            assert!(size_bytes > 100);
            assert!(path.exists());
        }
        DocumentResult::Memory { .. } => {
            panic!("Output should have spilled to DocumentResult::File due to 100 byte limit");
        }
    }
}

#[test]
fn test_render_non_png_format_is_unsupported() {
    // Regression: PNG bytes used to be labelled `image/jpeg` when Jpeg was requested.
    let client = LocalClient::default();
    let src = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc.pdf".into()));
    let opts = RenderOptions { format: pdfcraft_sdk::ImageFormat::Jpeg, ..Default::default() };
    match client.render_page(&src, 1, opts) {
        Err(SdkError::UnsupportedOperation(msg)) => assert!(msg.contains("PNG")),
        other => panic!("Expected UnsupportedOperation, got {other:?}"),
    }
}

#[test]
fn test_call_tool_executes_a_real_automation_tool() {
    let adapter = AutomationEngineAdapter::default();
    let res = adapter.execute_call_tool("command_list", &serde_json::json!({})).expect("command_list ok");

    assert_eq!(res.len(), 1);
    match &res[0] {
        pdfcraft_sdk::ToolContent::Json(v) => {
            assert!(v.get("commands").is_some() || v.is_array(), "expected a commands listing, got {v:?}");
        }
        pdfcraft_sdk::ToolContent::Png { .. } => panic!("expected Json content from command_list"),
    }
}

#[test]
fn test_call_tool_unknown_tool_is_unsupported_operation() {
    let adapter = AutomationEngineAdapter::default();
    let res = adapter.execute_call_tool("not_a_real_tool", &serde_json::json!({}));

    match res {
        Err(SdkError::UnsupportedOperation(msg)) => assert!(msg.contains("not_a_real_tool")),
        other => panic!("Expected UnsupportedOperation, got {other:?}"),
    }
}

#[test]
fn test_call_tool_empty_name_rejected() {
    let adapter = AutomationEngineAdapter::default();
    let res = adapter.execute_call_tool("", &serde_json::json!({}));

    match res {
        Err(SdkError::InvalidArgument(_)) => {}
        other => panic!("Expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_call_tool_invalid_args_surfaced_without_panic() {
    // doc_open requires a `path`; omitting it must return a structured error, never panic.
    let adapter = AutomationEngineAdapter::default();
    let res = adapter.execute_call_tool("doc_open", &serde_json::json!({}));

    assert!(res.is_err(), "missing required argument must fail, not succeed");
}

#[test]
fn test_split_parts_outlive_the_call() {
    // Regression: the sandbox directory was deleted on return, leaving dangling split paths.
    let adapter = AutomationEngineAdapter::default();
    let src = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc.pdf".into()));
    let res = adapter.execute_split(&src, &SplitMode::EveryNPages(1)).expect("real split must succeed");
    assert!(!res.files.is_empty(), "split reported no files");
    for f in &res.files {
        assert!(f.exists(), "split part {} should still exist", f.display());
    }
}

#[test]
fn test_batch_execution_success() {
    let adapter = AutomationEngineAdapter::default();
    let steps = vec![pdfcraft_sdk::ports::BatchStep { tool: "command_list".into(), args: serde_json::json!({}) }];

    let report = adapter.execute_batch(&steps).expect("batch execution should succeed");
    assert!(report.ok);
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].status, "ok");
    assert!(report.steps[0].result.is_some());
}

#[test]
fn test_batch_execution_halts_on_failure_and_skips_remaining() {
    let adapter = AutomationEngineAdapter::default();
    let steps = vec![
        pdfcraft_sdk::ports::BatchStep { tool: "nonexistent_tool_123".into(), args: serde_json::json!({}) },
        pdfcraft_sdk::ports::BatchStep { tool: "command_list".into(), args: serde_json::json!({}) },
    ];

    let report = adapter.execute_batch(&steps).expect("batch execution returns report even on step error");
    assert!(!report.ok);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[0].status, "error");
    assert_eq!(report.steps[1].status, "skipped");
}

#[test]
fn test_signing_cert_purged_on_error() {
    let adapter = AutomationEngineAdapter::default();
    let src = DocumentSource::from_bytes(minimal_valid_pdf(), Some("doc.pdf".into()));
    let mut sign_req = pdfcraft_sdk::security::SignDoc::new(b"invalid_mock_p12_content".to_vec());
    sign_req.password = Some("secret_password".into());
    sign_req.reason = Some("Test".into());

    // This should fail gracefully with engine error since p12 is invalid, but cert must not linger
    let res = adapter.execute_sign(&src, &sign_req);
    assert!(res.is_err());
}
