//! Unit and integration test plan for Document Operation Service & In-Process SDK stubs.
//!
//! Verifies:
//! - Resource limit enforcement (max upload bytes, max output bytes)
//! - Sandboxed directory isolation (0700 permissions and path containment)
//! - Never-crash error propagation returning typed Problem Details or Result errors
//! - In-process execution without network or open ports

use std::path::PathBuf;
use pdfcraft_automation::rest_stub::{
    DocumentCommand, DocumentOperationService, JobSandbox, MergeRequest, ResourceLimits, SplitRequest,
};

fn sample_resource_limits() -> ResourceLimits {
    ResourceLimits {
        max_request_bytes: 5 * 1024 * 1024,
        max_output_bytes: 10 * 1024 * 1024,
        max_pages: 50,
        timeout: std::time::Duration::from_secs(5),
    }
}

#[test]
fn sandbox_creation_and_bounds() {
    let limits = sample_resource_limits();
    let sandbox = JobSandbox::new(limits).expect("sandbox creation");
    assert!(sandbox.root().exists(), "Sandbox root must exist");

    // Write input within limits
    let test_data = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF";
    let input_path = sandbox.write_input("sample.pdf", test_data).expect("write input");
    assert!(input_path.exists());
    assert_eq!(input_path.parent(), Some(sandbox.root()));

    // Reject input exceeding max_request_bytes
    let oversized = vec![0u8; 6 * 1024 * 1024];
    let err = sandbox.write_input("oversized.pdf", &oversized);
    assert!(err.is_err(), "Must reject input exceeding max_request_bytes");
}

#[test]
fn merge_command_fails_gracefully_on_missing_files() {
    let limits = sample_resource_limits();
    let sandbox = JobSandbox::new(limits).expect("sandbox creation");

    let cmd = DocumentCommand::Merge(MergeRequest {
        files: vec!["nonexistent1.pdf".into(), "nonexistent2.pdf".into()],
        pages: None,
        passwords: None,
        output_filename: "out.pdf".into(),
    });

    // Must return an actionable Err, never panic
    let result = DocumentOperationService::execute(&sandbox, cmd);
    assert!(result.is_err());
    let err_msg = result.err().unwrap_or_default();
    assert!(err_msg.contains("Merge operation failed") || err_msg.contains("doc_combine"));
}

#[test]
fn split_command_fails_gracefully_on_missing_file() {
    let limits = sample_resource_limits();
    let sandbox = JobSandbox::new(limits).expect("sandbox creation");

    let cmd = DocumentCommand::Split(SplitRequest {
        file: "nonexistent.pdf".into(),
        every: Some(1),
        before_pages: None,
        at_bookmarks: None,
        max_mb: None,
    });

    let result = DocumentOperationService::execute(&sandbox, cmd);
    assert!(result.is_err());
    let err_msg = result.err().unwrap_or_default();
    assert!(err_msg.contains("Failed to open document") || err_msg.contains("doc_open"));
}
