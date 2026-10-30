//! parity_tests.rs — Semantic Parity Test Harness between LocalClient and RestClient.
//! ===================================================================================
//!
//! Purpose:
//!     Verifies 1:1 semantic parity between LocalClient (in-process) and RestClient (HTTP loopback)
//!     ensuring identical output bytes, MIME types, page counts, and error semantics.
//!
//! Layer:
//!     Test Suite / Integration Parity Tests

use std::sync::Arc;

use pdfcraft_rest::client::RestClient;
use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::routes::RestService;
use pdfcraft_rest::server::{HttpServer, ServerConfig};
use pdfcraft_rest::storage::InMemoryJobRepository;
use pdfcraft_sdk::{
    DocumentResult, DocumentSource, LocalClient, MergeOptions, OperationExecutor, PdfCraftClient, RenderOptions, SdkError, SplitMode, SplitResult,
};

/// Deterministic mock engine shared between LocalClient and RestClient for parity testing.
struct DeterministicMockEngine;

impl OperationExecutor for DeterministicMockEngine {
    fn execute_merge(&self, sources: &[DocumentSource], _options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        let mut combined = b"%PDF-1.7\n".to_vec();
        for src in sources {
            match src {
                DocumentSource::Bytes { data, .. } => combined.extend_from_slice(data),
                DocumentSource::Path(p) => combined.extend_from_slice(p.to_string_lossy().as_bytes()),
            }
        }
        combined.extend_from_slice(b"\n%%EOF");
        Ok(DocumentResult::memory(combined, "application/pdf"))
    }

    fn execute_split(&self, _source: &DocumentSource, _mode: &SplitMode) -> Result<SplitResult, SdkError> {
        let temp_dir = tempfile::Builder::new().prefix("parity-split-mock-").tempdir().map_err(SdkError::Io)?;
        let chunk1 = temp_dir.path().join("chunk1.pdf");
        let chunk2 = temp_dir.path().join("chunk2.pdf");
        std::fs::write(&chunk1, b"%PDF-1.7 chunk 1").map_err(SdkError::Io)?;
        std::fs::write(&chunk2, b"%PDF-1.7 chunk 2").map_err(SdkError::Io)?;
        let _ = temp_dir.keep();
        Ok(SplitResult { files: vec![chunk1, chunk2] })
    }

    fn execute_render(&self, _source: &DocumentSource, page: usize, options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page 0 is invalid".into()));
        }
        let rendered = format!("RENDER_PAGE_{page}_DPI_{}", options.dpi).into_bytes();
        Ok(DocumentResult::memory(rendered, "image/png"))
    }

    fn execute_call_tool(&self, name: &str, args: &serde_json::Value) -> Result<Vec<pdfcraft_sdk::ToolContent>, SdkError> {
        Ok(vec![pdfcraft_sdk::ToolContent::Json(serde_json::json!({ "tool": name, "args": args }))])
    }
}

#[test]
fn test_local_vs_rest_direct_semantic_parity() {
    let engine: Arc<dyn OperationExecutor> = Arc::new(DeterministicMockEngine);

    // 1. Instantiate LocalClient
    let local_client = LocalClient::new(Arc::clone(&engine));

    // 2. Instantiate RestClient (direct mode)
    let executor = Arc::new(BlockingExecutor::new(Arc::clone(&engine), ExecutorConfig::default()));
    let repository = Arc::new(InMemoryJobRepository::new());
    let service = Arc::new(RestService::new(executor, repository, None));
    let rest_client = RestClient::direct(service);

    // Prepare identical inputs
    let input1 = DocumentSource::from_bytes(b"%PDF-doc1-content".to_vec(), Some("doc1.pdf".into()));
    let input2 = DocumentSource::from_bytes(b"%PDF-doc2-content".to_vec(), Some("doc2.pdf".into()));
    let inputs = vec![input1, input2];

    // Merge Parity Check
    let local_merge = local_client.merge(&inputs, MergeOptions::default()).unwrap();
    let rest_merge = rest_client.merge(&inputs, MergeOptions::default()).unwrap();

    assert_eq!(local_merge.mime_type(), rest_merge.mime_type());
    assert_eq!(local_merge.byte_len(), rest_merge.byte_len());
    match (local_merge, rest_merge) {
        (DocumentResult::Memory { data: d1, .. }, DocumentResult::Memory { data: d2, .. }) => {
            assert_eq!(d1, d2);
        }
        _ => panic!("Expected Memory results"),
    }

    // Split Parity Check
    let split_input = DocumentSource::from_bytes(b"%PDF-doc-split".to_vec(), None);
    let local_split = local_client.split(&split_input, SplitMode::EveryNPages(1)).unwrap();
    let rest_split = rest_client.split(&split_input, SplitMode::EveryNPages(1)).unwrap();
    let local_names: Vec<_> = local_split.files.iter().filter_map(|p| p.file_name()).collect();
    let rest_names: Vec<_> = rest_split.files.iter().filter_map(|p| p.file_name()).collect();
    assert_eq!(local_names, rest_names);

    let local_b1 = std::fs::read(&local_split.files[0]).unwrap();
    let rest_b1 = std::fs::read(&rest_split.files[0]).unwrap();
    assert_eq!(local_b1, rest_b1);
    assert_eq!(local_b1, b"%PDF-1.7 chunk 1");

    let local_b2 = std::fs::read(&local_split.files[1]).unwrap();
    let rest_b2 = std::fs::read(&rest_split.files[1]).unwrap();
    assert_eq!(local_b2, rest_b2);
    assert_eq!(local_b2, b"%PDF-1.7 chunk 2");

    // Render Page Parity Check
    let render_opt = RenderOptions { dpi: 300.0, ..Default::default() };
    let local_render = local_client.render_page(&split_input, 1, render_opt.clone()).unwrap();
    let rest_render = rest_client.render_page(&split_input, 1, render_opt).unwrap();
    assert_eq!(local_render.byte_len(), rest_render.byte_len());

    // Invariant Error Parity Check (Page 0 rejection)
    let local_err = local_client.render_page(&split_input, 0, RenderOptions::default()).unwrap_err();
    let rest_err = rest_client.render_page(&split_input, 0, RenderOptions::default()).unwrap_err();
    assert_eq!(local_err.http_status(), rest_err.http_status());
    assert_eq!(local_err.error_code(), rest_err.error_code());
}

#[test]
fn test_local_vs_rest_over_http_loopback_parity() {
    let engine: Arc<dyn OperationExecutor> = Arc::new(DeterministicMockEngine);
    let local_client = LocalClient::new(Arc::clone(&engine));

    // Spin up actual HTTP server on loopback
    let executor = Arc::new(BlockingExecutor::new(Arc::clone(&engine), ExecutorConfig::default()));
    let repository = Arc::new(InMemoryJobRepository::new());
    let service = Arc::new(RestService::new(executor, repository, Some("test-token".into())));

    let server = HttpServer::start(service, ServerConfig { bind_addr: "127.0.0.1:0".parse().unwrap(), allow_public_bind: false }).unwrap();

    let server_url = format!("http://127.0.0.1:{}", server.port());
    let rest_client = RestClient::remote(server_url, Some("test-token".into()));

    // Merge Parity over HTTP
    let input1 = DocumentSource::from_bytes(b"%PDF-doc1".to_vec(), None);
    let input2 = DocumentSource::from_bytes(b"%PDF-doc2".to_vec(), None);
    let inputs = vec![input1, input2];

    let local_res = local_client.merge(&inputs, MergeOptions::default()).unwrap();
    let rest_res = rest_client.merge(&inputs, MergeOptions::default()).unwrap();

    assert_eq!(local_res.mime_type(), rest_res.mime_type());
    assert_eq!(local_res.byte_len(), rest_res.byte_len());
    match (local_res, rest_res) {
        (DocumentResult::Memory { data: d1, .. }, DocumentResult::Memory { data: d2, .. }) => {
            assert_eq!(d1, d2);
        }
        _ => panic!("Expected Memory results"),
    }

    // Split Parity over HTTP
    let split_input = DocumentSource::from_bytes(b"%PDF-doc-split-http".to_vec(), None);
    let local_split = local_client.split(&split_input, SplitMode::EveryNPages(1)).unwrap();
    let rest_split = rest_client.split(&split_input, SplitMode::EveryNPages(1)).unwrap();
    assert_eq!(local_split.files.len(), rest_split.files.len());
    let local_b1 = std::fs::read(&local_split.files[0]).unwrap();
    let rest_b1 = std::fs::read(&rest_split.files[0]).unwrap();
    assert_eq!(local_b1, rest_b1);
    assert_eq!(local_b1, b"%PDF-1.7 chunk 1");
}
