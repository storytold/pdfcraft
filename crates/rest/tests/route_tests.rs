//! route_tests.rs — HTTP Route and RFC 7807 Problem Details tests.
//! ====================================================================
//!
//! Purpose:
//!     Verifies HTTP routing, synchronous vs asynchronous job lifecycles,
//!     bearer authentication gates, and RFC 7807 problem details serialization.
//!
//! Layer:
//!     Test Suite / REST Route Tests

use std::sync::Arc;
use std::time::Duration;

use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::models::{ContentDto, JobResponse, JobState};
use pdfcraft_rest::problem::ProblemDetails;
use pdfcraft_rest::routes::{HttpRequest, RestService};
use pdfcraft_rest::storage::InMemoryJobRepository;
use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, OperationExecutor, RenderOptions, SdkError, SplitMode, SplitResult};

/// Simple mock executor for fast route testing.
struct FastMockExecutor;

impl OperationExecutor for FastMockExecutor {
    fn execute_merge(&self, _sources: &[DocumentSource], _options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        Ok(DocumentResult::memory(b"%PDF-1.7 merged mock".to_vec(), "application/pdf"))
    }

    fn execute_split(&self, _source: &DocumentSource, _mode: &SplitMode) -> Result<SplitResult, SdkError> {
        Ok(SplitResult { files: vec![] })
    }

    fn execute_render(&self, _source: &DocumentSource, page: usize, _options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page 0 is invalid".into()));
        }
        Ok(DocumentResult::memory(b"mock-png-bytes".to_vec(), "image/png"))
    }

    fn execute_call_tool(&self, name: &str, args: &serde_json::Value) -> Result<Vec<pdfcraft_sdk::ToolContent>, SdkError> {
        match name {
            "unknown_tool" => Err(SdkError::UnsupportedOperation(format!("Unknown tool: {name}"))),
            "render_preview" => Ok(vec![pdfcraft_sdk::ToolContent::Png { data: b"mock-tool-png".to_vec(), width: 1, height: 1 }]),
            _ => Ok(vec![pdfcraft_sdk::ToolContent::Json(serde_json::json!({ "tool": name, "args": args }))]),
        }
    }

    fn execute_batch(&self, steps: &[pdfcraft_sdk::ports::BatchStep]) -> Result<pdfcraft_sdk::ports::BatchReport, SdkError> {
        let results = steps
            .iter()
            .enumerate()
            .map(|(i, s)| pdfcraft_sdk::ports::BatchStepResult {
                step: i + 1,
                tool: s.tool.clone(),
                status: "ok".into(),
                result: Some(serde_json::json!({ "executed": s.tool })),
                error: None,
            })
            .collect();
        Ok(pdfcraft_sdk::ports::BatchReport { ok: true, steps: results, output_files: None })
    }
}

fn create_test_service(token: Option<&str>) -> Arc<RestService> {
    let mock = Arc::new(FastMockExecutor);
    let config = ExecutorConfig::default();
    let executor = Arc::new(BlockingExecutor::new(mock, config));
    let repository = Arc::new(InMemoryJobRepository::new());
    Arc::new(RestService::new(executor, repository, token.map(str::to_string)))
}

#[test]
fn test_sync_merge_route_success() {
    let service = create_test_service(None);

    let body = serde_json::json!({
        "files": [
            { "data_base64": "JVBERi0xLjQK", "name": "doc1.pdf" }
        ],
        "async_job": false
    });

    let req =
        HttpRequest::new("POST", "/v1/merged-pdf").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.headers.get("content-type"), Some(&"application/pdf".to_string()));
    assert_eq!(resp.body, b"%PDF-1.7 merged mock");
}

#[test]
fn test_async_merge_job_lifecycle() {
    let service = create_test_service(None);

    let body = serde_json::json!({
        "files": [
            { "data_base64": "JVBERi0xLjQK", "name": "doc1.pdf" }
        ],
        "async_job": true
    });

    let req =
        HttpRequest::new("POST", "/v1/merged-pdf").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 202);
    let job: JobResponse = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(job.status, JobState::Queued);

    // Give background worker a brief moment to finish
    std::thread::sleep(Duration::from_millis(50));

    // Poll GET /v1/jobs/{id}
    let get_req = HttpRequest::new("GET", format!("/v1/jobs/{}", job.id));
    let get_resp = service.dispatch(&get_req);
    assert_eq!(get_resp.status, 200);
    let finished_job: JobResponse = serde_json::from_slice(&get_resp.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(finished_job.status, JobState::Succeeded);
    assert_eq!(finished_job.progress, 100);

    // Fetch result: GET /v1/jobs/{id}/result
    let res_req = HttpRequest::new("GET", format!("/v1/jobs/{}/result", job.id));
    let res_resp = service.dispatch(&res_req);
    assert_eq!(res_resp.status, 200);
    assert_eq!(res_resp.headers.get("content-type"), Some(&"application/pdf".to_string()));
    assert_eq!(res_resp.body, b"%PDF-1.7 merged mock");
}

#[test]
fn test_page_0_rejection_rfc7807() {
    let service = create_test_service(None);

    let body = serde_json::json!({
        "file": { "data_base64": "JVBERi0xLjQK", "name": "doc.pdf" },
        "page": 0,
        "async_job": false
    });

    let req =
        HttpRequest::new("POST", "/v1/page-preview").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 400);
    assert_eq!(resp.headers.get("content-type"), Some(&"application/problem+json".to_string()));

    let problem: ProblemDetails = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(problem.status, 400);
    assert_eq!(problem.code, "BAD_REQUEST");
    assert!(problem.detail.contains("Page index must be >= 1"));
}

#[test]
fn test_bearer_authentication_gate() {
    let service = create_test_service(Some("secret-token-123"));

    let body = serde_json::json!({
        "files": [{ "data_base64": "JVBERi0xLjQK" }],
        "async_job": false
    });

    // 1. Without Authorization header -> 401 Unauthorized
    let unauth_req = HttpRequest::new("POST", "/v1/merged-pdf").with_body(serde_json::to_vec(&body).unwrap());
    let resp1 = service.dispatch(&unauth_req);
    assert_eq!(resp1.status, 401);
    let prob1: ProblemDetails = serde_json::from_slice(&resp1.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(prob1.code, "UNAUTHORIZED");

    // 2. With valid Authorization header -> 200 OK
    let auth_req = HttpRequest::new("POST", "/v1/merged-pdf")
        .with_header("authorization", "Bearer secret-token-123")
        .with_body(serde_json::to_vec(&body).unwrap());
    let resp2 = service.dispatch(&auth_req);
    assert_eq!(resp2.status, 200);
}

#[test]
fn test_call_tool_json_result() {
    let service = create_test_service(None);

    let body = serde_json::json!({ "doc": 1, "page": 1 });
    let req = HttpRequest::new("POST", "/v1/operations/doc_info").with_body(serde_json::to_vec(&body).unwrap());
    let resp = service.dispatch(&req);

    assert_eq!(resp.status, 200);
    assert_eq!(resp.headers.get("content-type"), Some(&"application/json".to_string()));
    let dtos: Vec<ContentDto> = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(dtos.len(), 1);
    match &dtos[0] {
        ContentDto::Json { data } => assert_eq!(data["tool"], "doc_info"),
        ContentDto::Png { .. } => panic!("expected Json content"),
    }
}

#[test]
fn test_call_tool_png_result() {
    let service = create_test_service(None);

    let req = HttpRequest::new("POST", "/v1/operations/render_preview").with_body(b"{}".to_vec());
    let resp = service.dispatch(&req);

    assert_eq!(resp.status, 200);
    assert_eq!(resp.headers.get("content-type"), Some(&"image/png".to_string()));
    assert_eq!(resp.body, b"mock-tool-png");
}

#[test]
fn test_call_tool_unknown_tool_rfc7807() {
    let service = create_test_service(None);

    let req = HttpRequest::new("POST", "/v1/operations/unknown_tool").with_body(b"{}".to_vec());
    let resp = service.dispatch(&req);

    assert_eq!(resp.status, 501);
    let problem: ProblemDetails = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(problem.code, "UNSUPPORTED_OPERATION");
}

#[test]
fn test_call_tool_empty_name_not_found() {
    let service = create_test_service(None);

    let req = HttpRequest::new("POST", "/v1/operations/").with_body(b"{}".to_vec());
    let resp = service.dispatch(&req);

    assert_eq!(resp.status, 404);
}

#[test]
fn test_call_tool_empty_body_defaults_to_empty_object() {
    let service = create_test_service(None);

    let req = HttpRequest::new("POST", "/v1/operations/doc_info");
    let resp = service.dispatch(&req);

    assert_eq!(resp.status, 200);
    let dtos: Vec<ContentDto> = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    match &dtos[0] {
        ContentDto::Json { data } => assert_eq!(data["args"], serde_json::json!({})),
        ContentDto::Png { .. } => panic!("expected Json content"),
    }
}

#[test]
fn test_batch_execution_route() {
    let service = create_test_service(None);

    let batch_steps = serde_json::json!([
        { "tool": "command_list", "args": {} },
        { "tool": "doc_info", "args": { "doc": "doc_1" } }
    ]);

    let req =
        HttpRequest::new("POST", "/v1/batch").with_header("content-type", "application/json").with_body(serde_json::to_vec(&batch_steps).unwrap());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 200);

    let report: pdfcraft_sdk::ports::BatchReport = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    assert!(report.ok);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[0].tool, "command_list");
    assert_eq!(report.steps[1].tool, "doc_info");
}

#[test]
fn test_batch_execution_empty_steps_rejected() {
    let service = create_test_service(None);

    let req = HttpRequest::new("POST", "/v1/batch").with_header("content-type", "application/json").with_body(b"[]".to_vec());

    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 400);
}

struct MissingSplitFileMockExecutor;
impl OperationExecutor for MissingSplitFileMockExecutor {
    fn execute_merge(&self, _sources: &[DocumentSource], _options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        unimplemented!()
    }
    fn execute_split(&self, _source: &DocumentSource, _mode: &SplitMode) -> Result<SplitResult, SdkError> {
        Ok(SplitResult { files: vec!["/nonexistent/path/chunk1.pdf".into()] })
    }
    fn execute_render(&self, _source: &DocumentSource, _page: usize, _options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        unimplemented!()
    }
    fn execute_call_tool(&self, _name: &str, _args: &serde_json::Value) -> Result<Vec<pdfcraft_sdk::ToolContent>, SdkError> {
        unimplemented!()
    }
}

#[test]
fn test_split_missing_file_fails_closed() {
    let executor = Arc::new(BlockingExecutor::new(Arc::new(MissingSplitFileMockExecutor), ExecutorConfig::default()));
    let repository = Arc::new(InMemoryJobRepository::new());
    let service = RestService::new(executor, repository, None);

    let body = serde_json::json!({
        "file": { "data_base64": "JVBERi0xLjQK" },
        "mode": { "EveryNPages": 1 },
        "async_job": false
    });
    let req = HttpRequest::new("POST", "/v1/split-pdf").with_header("content-type", "application/json").with_body(serde_json::to_vec(&body).unwrap());
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 500);
    let problem: ProblemDetails = serde_json::from_slice(&resp.body.try_to_bytes().unwrap()).unwrap();
    assert_eq!(problem.code, "INTERNAL_SERVER_ERROR");
    assert!(problem.detail.contains("Failed to read split output"));
}

#[test]
fn server_local_sources_are_rejected_before_execution() {
    let service = create_test_service(None);
    for async_job in [false, true] {
        let body = serde_json::json!({"files": [{"path": "/etc/passwd"}], "async_job": async_job});
        let response = service.dispatch(&HttpRequest::new("POST", "/v1/merged-pdf").with_body(serde_json::to_vec(&body).unwrap()));
        assert_eq!(response.status, 400);
        let problem: ProblemDetails = serde_json::from_slice(&response.body.to_bytes().unwrap()).unwrap();
        assert!(problem.detail.contains("Server-local paths are not accepted over REST"));
    }
}
