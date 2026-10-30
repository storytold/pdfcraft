//! client.rs — Remote REST client implementing the unified PdfCraftClient trait (Issue #872).
//! =========================================================================================
//!
//! Purpose:
//!     Connects to an on-premise PdfCraft REST daemon, providing 100% semantic parity
//!     with LocalClient through identical input/output types and error mappings.
//!
//! Layer:
//!     REST Service / Client Transport Layer
//!
//! Key Input Dependencies:
//!     - pdfcraft_sdk::PdfCraftClient
//!     - pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SdkError, SplitMode, SplitResult}
//!     - crate::routes::{HttpRequest, HttpResponse, RestService}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::client::RestClient;
//!     use pdfcraft_sdk::PdfCraftClient;
//!     let client = RestClient::remote("http://127.0.0.1:8080", None);
//!     ```
//!
//! Key Types & Functions Index:
//!     - RestClient: Client enum supporting Direct (in-process) and Remote (HTTP network) modes
//!     - RestClient::direct: In-process service invocation bypassing network stack
//!     - RestClient::remote: Remote HTTP/1.1 client connecting to base URL
//!     - RestClient::merge: Merges documents with synchronous or polling resolution
//!     - RestClient::split: Splits document
//!     - RestClient::render_page: Renders page to image format
//!     - RestClient::poll_job_result: Polls background job until completion or error

use std::sync::Arc;
use std::time::Duration;

use crate::models::{JobResponse, JobState, MergePayload, RenderPayload, SourcePayload, SplitPayload, SplitResponseDto, decode_base64};
use crate::problem::ProblemDetails;
use crate::routes::{HttpRequest, HttpResponse, RestService};
use pdfcraft_sdk::{DocumentResult, DocumentSource, MergeOptions, PdfCraftClient, RenderOptions, SdkError, SplitMode, SplitResult};

/// Unified REST client communicating with a PdfCraft REST service.
pub enum RestClient {
    /// In-process service invocation bypassing network stack (for high performance and zero-cost testing).
    Direct(Arc<RestService>),
    /// Remote network connection using HTTP/1.1 endpoint.
    Remote { base_url: String, auth_token: Option<String> },
}

impl RestClient {
    /// Creates a client connecting to an in-process `RestService`.
    pub fn direct(service: Arc<RestService>) -> Self {
        Self::Direct(service)
    }

    /// Creates a remote HTTP client targeting a base URL (e.g. `http://127.0.0.1:8080`).
    pub fn remote(base_url: impl Into<String>, auth_token: Option<String>) -> Self {
        Self::Remote { base_url: base_url.into().trim_end_matches('/').to_string(), auth_token }
    }

    fn execute_request(&self, req: HttpRequest) -> Result<HttpResponse, SdkError> {
        match self {
            Self::Direct(service) => Ok(service.dispatch(&req)),
            Self::Remote { base_url, auth_token } => send_http_request(base_url, auth_token.as_deref(), req),
        }
    }
}

impl PdfCraftClient for RestClient {
    fn merge(&self, files: &[DocumentSource], options: MergeOptions) -> Result<DocumentResult, SdkError> {
        if files.is_empty() {
            return Err(SdkError::InvalidArgument("Cannot merge zero files".into()));
        }

        let mut sources = Vec::new();
        for file in files {
            sources.push(source_to_payload(file)?);
        }

        let payload = MergePayload { files: sources, options, async_job: false };

        let body = serde_json::to_vec(&payload).map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        let req = HttpRequest::new("POST", "/v1/merged-pdf").with_header("content-type", "application/json").with_body(body);

        let resp = self.execute_request(req)?;

        if resp.status == 200 {
            let mime = resp.headers.get("content-type").cloned().unwrap_or_else(|| "application/pdf".into());
            let data = resp.body.try_to_bytes().map_err(SdkError::Io)?;
            Ok(DocumentResult::memory(data, mime))
        } else {
            let body = resp.body.try_to_bytes().map_err(SdkError::Io)?;
            if resp.status == 202 {
                let job: JobResponse = serde_json::from_slice(&body).map_err(|e| SdkError::EngineError(e.to_string()))?;
                self.poll_job_result(&job.id)
            } else {
                Err(parse_problem_error(&body, resp.status))
            }
        }
    }

    fn split(&self, file: &DocumentSource, mode: SplitMode) -> Result<SplitResult, SdkError> {
        let payload = SplitPayload { file: source_to_payload(file)?, mode, async_job: false };
        let body = serde_json::to_vec(&payload).map_err(|e| SdkError::InvalidArgument(e.to_string()))?;
        let req = HttpRequest::new("POST", "/v1/split-pdf").with_header("content-type", "application/json").with_body(body);

        let resp = self.execute_request(req)?;
        let body = resp.body.try_to_bytes().map_err(SdkError::Io)?;

        if resp.status != 200 {
            return Err(parse_problem_error(&body, resp.status));
        }

        let dto: SplitResponseDto = serde_json::from_slice(&body).map_err(|e| SdkError::EngineError(format!("Invalid split response: {e}")))?;

        materialize_split_parts(dto)
    }

    fn render_page(&self, file: &DocumentSource, page: usize, options: RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page index must be >= 1".into()));
        }

        let payload = RenderPayload { file: source_to_payload(file)?, page, options, async_job: false };

        let body = serde_json::to_vec(&payload).map_err(|e| SdkError::InvalidArgument(e.to_string()))?;

        let req = HttpRequest::new("POST", "/v1/page-preview").with_header("content-type", "application/json").with_body(body);

        let resp = self.execute_request(req)?;
        let body = resp.body.try_to_bytes().map_err(SdkError::Io)?;

        if resp.status == 200 {
            let mime = resp.headers.get("content-type").cloned().unwrap_or_else(|| "image/png".into());
            Ok(DocumentResult::memory(body, mime))
        } else {
            Err(parse_problem_error(&body, resp.status))
        }
    }
}

/// Materializes a split response into an exclusively owned directory.
///
/// The directory remains RAII-owned until every part has been validated and
/// written successfully. Any error drops the directory and its partial files.
fn materialize_split_parts(dto: SplitResponseDto) -> Result<SplitResult, SdkError> {
    use std::collections::HashSet;
    use std::io::Write;
    use std::path::{Component, Path};

    let mut names = HashSet::new();

    // Validate the entire namespace before creating any output.
    for part in &dto.parts {
        let name = &part.filename;
        let mut components = Path::new(name).components();
        let single_normal_component = matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

        if !single_normal_component || name.contains(['/', '\\', ':', '\0']) || name.ends_with(['.', ' ']) {
            return Err(SdkError::EngineError(format!("Invalid split filename: {name:?}; expected a single normal filename")));
        }

        if !names.insert(name.as_str()) {
            return Err(SdkError::EngineError(format!("Duplicate split filename: {name:?}")));
        }
    }

    let directory = tempfile::Builder::new().prefix("pdfcraft-client-split-").tempdir().map_err(SdkError::Io)?;
    let mut files = Vec::with_capacity(dto.parts.len());

    for part in dto.parts {
        let bytes =
            decode_base64(&part.data_base64).map_err(|e| SdkError::EngineError(format!("Invalid base64 for split part {:?}: {e}", part.filename)))?;

        if bytes.len() != part.size_bytes {
            return Err(SdkError::EngineError(format!(
                "Split part {:?} declares {} bytes but contains {} bytes",
                part.filename,
                part.size_bytes,
                bytes.len()
            )));
        }

        let path = directory.path().join(&part.filename);

        {
            let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(SdkError::Io)?;
            output.write_all(&bytes).map_err(SdkError::Io)?;
            output.flush().map_err(SdkError::Io)?;
        }

        files.push(path);
    }

    // Ownership transfers to the caller only after the whole response succeeds.
    let _ = directory.keep();
    Ok(SplitResult { files })
}

impl RestClient {
    fn poll_job_result(&self, job_id: &str) -> Result<DocumentResult, SdkError> {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);

        while std::time::Instant::now() < deadline {
            let req = HttpRequest::new("GET", format!("/v1/jobs/{job_id}"));
            let resp = self.execute_request(req)?;
            let body = resp.body.try_to_bytes().map_err(SdkError::Io)?;
            if resp.status != 200 {
                return Err(parse_problem_error(&body, resp.status));
            }

            let job: JobResponse = serde_json::from_slice(&body).map_err(|e| SdkError::EngineError(e.to_string()))?;

            match job.status {
                JobState::Succeeded => {
                    let res_req = HttpRequest::new("GET", format!("/v1/jobs/{job_id}/result"));
                    let res_resp = self.execute_request(res_req)?;
                    let res_body = res_resp.body.try_to_bytes().map_err(SdkError::Io)?;
                    if res_resp.status == 200 {
                        let mime = res_resp.headers.get("content-type").cloned().unwrap_or_else(|| "application/pdf".into());
                        return Ok(DocumentResult::memory(res_body, mime));
                    } else {
                        return Err(parse_problem_error(&res_body, res_resp.status));
                    }
                }
                JobState::Failed => {
                    return Err(SdkError::EngineError(job.error.unwrap_or_else(|| "Job failed".into())));
                }
                JobState::Canceled => {
                    return Err(SdkError::EngineError("Job was canceled".into()));
                }
                JobState::Queued | JobState::Running => {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }

        Err(SdkError::EngineError("Job polling timeout exceeded".into()))
    }
}

fn source_to_payload(source: &DocumentSource) -> Result<SourcePayload, SdkError> {
    Ok(match source {
        DocumentSource::Path(p) => {
            let data = std::fs::read(p).map_err(SdkError::Io)?;
            let name = p.file_name().and_then(|n| n.to_str()).map(str::to_string);
            SourcePayload::Bytes { data_base64: encode_base64(&data), name }
        }
        DocumentSource::Bytes { data, name } => SourcePayload::Bytes { data_base64: encode_base64(data), name: name.clone() },
    })
}

fn parse_problem_error(body: &[u8], status: u16) -> SdkError {
    if let Ok(problem) = serde_json::from_slice::<ProblemDetails>(body) {
        match problem.status {
            400 => SdkError::InvalidArgument(problem.detail),
            401 => SdkError::Unauthorized(problem.detail),
            404 => SdkError::FileNotFound { path: problem.detail },
            413 => SdkError::QuotaExceeded(problem.detail),
            429 => SdkError::QuotaExceeded(problem.detail),
            501 => SdkError::UnsupportedOperation(problem.detail),
            _ => SdkError::EngineError(problem.detail),
        }
    } else {
        SdkError::EngineError(format!("HTTP error {status}"))
    }
}

fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn send_http_request(base_url: &str, auth_token: Option<&str>, req: HttpRequest) -> Result<HttpResponse, SdkError> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;

    let url_without_proto = base_url.strip_prefix("http://").unwrap_or(base_url);
    let (host, port_str) = url_without_proto.split_once(':').unwrap_or((url_without_proto, "80"));
    let port: u16 = port_str.parse().unwrap_or(80);

    let mut stream = TcpStream::connect((host, port)).map_err(SdkError::Io)?;

    let mut request_bytes = Vec::new();
    let full_path = req.path;
    request_bytes.extend_from_slice(format!("{} {} HTTP/1.1\r\n", req.method, full_path).as_bytes());
    request_bytes.extend_from_slice(format!("Host: {host}:{port}\r\n").as_bytes());

    if let Some(token) = auth_token {
        request_bytes.extend_from_slice(format!("Authorization: Bearer {token}\r\n").as_bytes());
    }

    for (k, v) in &req.headers {
        request_bytes.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    // The server reads the body only when told how long it is.
    if !req.headers.keys().any(|k| k.eq_ignore_ascii_case("content-length")) {
        request_bytes.extend_from_slice(format!("Content-Length: {}\r\n", req.body.len()).as_bytes());
    }
    request_bytes.extend_from_slice(b"Connection: close\r\n\r\n");
    request_bytes.extend_from_slice(&req.body);

    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(30)));

    stream.write_all(&request_bytes).map_err(SdkError::Io)?;
    stream.flush().map_err(SdkError::Io)?;

    let Ok(stream_clone) = stream.try_clone() else {
        return Err(SdkError::EngineError("Failed to clone TCP stream".into()));
    };
    let mut reader = BufReader::new(stream_clone);

    let mut status_line = String::new();
    reader.by_ref().take(8192).read_line(&mut status_line).map_err(SdkError::Io)?;
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    let status =
        parts.get(1).and_then(|s| s.parse::<u16>().ok()).ok_or_else(|| SdkError::EngineError(format!("Malformed status line: {status_line}")))?;

    let mut headers = std::collections::HashMap::new();
    let mut content_length = 0usize;
    let mut seen_content_length = false;
    let mut headers_terminated = false;

    for _ in 0..100 {
        let mut line = String::new();
        reader.by_ref().take(8192).read_line(&mut line).map_err(SdkError::Io)?;
        if line == "\r\n" || line == "\n" {
            headers_terminated = true;
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_lowercase();
            let val = v.trim().to_string();
            if key == "transfer-encoding" {
                return Err(SdkError::UnsupportedOperation("Transfer-Encoding in response is not supported".into()));
            }
            if key == "content-length" {
                if seen_content_length {
                    return Err(SdkError::EngineError("Duplicate Content-Length header in response".into()));
                }
                seen_content_length = true;
                content_length = val.parse::<usize>().map_err(|e| SdkError::EngineError(format!("Invalid Content-Length: {e}")))?;
            }
            headers.insert(key, val);
        }
    }

    if !headers_terminated {
        return Err(SdkError::EngineError("Unterminated HTTP response headers".into()));
    }

    const MAX_CLIENT_BODY_BYTES: usize = 100 * 1024 * 1024;
    if content_length > MAX_CLIENT_BODY_BYTES {
        return Err(SdkError::EngineError(format!(
            "Response Content-Length ({content_length} bytes) exceeds safety maximum ({MAX_CLIENT_BODY_BYTES} bytes)"
        )));
    }

    let mut body = Vec::with_capacity(content_length.min(64 * 1024));
    if content_length > 0 {
        reader.take(content_length as u64).read_to_end(&mut body).map_err(SdkError::Io)?;
        if body.len() != content_length {
            return Err(SdkError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Incomplete response body")));
        }
    }

    Ok(HttpResponse { status, headers, body: crate::routes::HttpBody::Bytes(body) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SplitPartDto, SplitResponseDto};

    #[test]
    fn local_path_is_uploaded_as_bytes() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"local PDF bytes").unwrap();
        let payload = source_to_payload(&DocumentSource::Path(file.path().into())).unwrap();
        assert!(matches!(payload, SourcePayload::Bytes { .. }));
        let DocumentSource::Bytes { data, .. } = payload.into_source().unwrap() else { panic!("expected uploaded bytes") };
        assert_eq!(data, b"local PDF bytes");
    }

    #[test]
    fn test_materialize_split_parts_path_traversal_rejected() {
        let dto = SplitResponseDto {
            parts: vec![SplitPartDto { filename: "../../../etc/passwd".into(), size_bytes: 4, data_base64: encode_base64(b"test") }],
        };
        let err = materialize_split_parts(dto).unwrap_err();
        assert!(matches!(err, SdkError::EngineError(_)));
    }

    #[test]
    fn test_materialize_split_parts_duplicate_filename_rejected() {
        let dto = SplitResponseDto {
            parts: vec![
                SplitPartDto { filename: "part1.pdf".into(), size_bytes: 4, data_base64: encode_base64(b"test") },
                SplitPartDto { filename: "part1.pdf".into(), size_bytes: 4, data_base64: encode_base64(b"test") },
            ],
        };
        let err = materialize_split_parts(dto).unwrap_err();
        assert!(matches!(err, SdkError::EngineError(_)));
    }

    #[test]
    fn test_materialize_split_parts_size_mismatch_rejected() {
        let dto =
            SplitResponseDto { parts: vec![SplitPartDto { filename: "part1.pdf".into(), size_bytes: 100, data_base64: encode_base64(b"test") }] };
        let err = materialize_split_parts(dto).unwrap_err();
        assert!(matches!(err, SdkError::EngineError(_)));
    }
}
