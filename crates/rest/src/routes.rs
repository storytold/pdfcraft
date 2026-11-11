//! routes.rs — HTTP route handlers and request dispatching for document processing operations.
//! ==============================================================================================
//!
//! Purpose:
//!     Parses HTTP requests, enforces bearer authentication, routes to sync/async
//!     workflows, and returns RFC 7807 problem details or typed results.
//!
//! Layer:
//!     REST Service / API Routing & Presentation Layer
//!
//! Key Input Dependencies:
//!     - crate::executor::BlockingExecutor
//!     - crate::storage::JobRepository
//!     - crate::models::{JobId, JobRecord, JobResponse, JobState, MergePayload, RenderPayload, SplitPayload}
//!     - crate::problem::ProblemDetails
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_rest::routes::{HttpRequest, RestService};
//!     // service.dispatch(&req) -> HttpResponse
//!     ```
//!
//! Key Types & Functions Index:
//!     - HttpRequest: In-memory HTTP request DTO (with_header, with_body, header)
//!     - HttpResponse: In-memory HTTP response DTO (ok_json, accepted_json, ok_bytes, problem)
//!     - RestService: Dispatcher mapping HTTP routes to engine execution and job management
//!     - RestService::dispatch: Main request router enforcing auth and dispatching paths
//!     - RestService::handle_merge: Handles POST /v1/merged-pdf (sync or async 202)
//!     - RestService::handle_split: Handles POST /v1/split-pdf
//!     - RestService::handle_page_preview: Handles POST /v1/page-preview
//!     - RestService::handle_call_tool: Handles POST /v1/operations/{name}
//!     - RestService::handle_get_job: Handles GET /v1/jobs/{id}
//!     - RestService::handle_get_job_result: Handles GET /v1/jobs/{id}/result
//!     - RestService::handle_cancel_job: Handles DELETE /v1/jobs/{id}

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::executor::BlockingExecutor;
use crate::models::{
    ContentDto, JobId, JobRecord, JobResponse, JobState, MergePayload, RenderPayload, SplitPartDto, SplitPayload, SplitResponseDto, encode_base64,
};
use crate::problem::ProblemDetails;
use crate::storage::JobRepository;
use pdfcraft_sdk::{DocumentResult, ToolContent};

/// In-memory representation of an HTTP request.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self { method: method.into(), path: path.into(), headers: HashMap::new(), body: Vec::new() }
    }

    pub fn with_header(mut self, name: impl Into<String>, val: impl Into<String>) -> Self {
        self.headers.insert(name.into().to_lowercase(), val.into());
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(|s| s.as_str())
    }
}

/// RAII guard ensuring temporary output files and their empty parent directories are deleted
/// when a file-backed response is dropped or fails before/during streaming.
#[derive(Debug)]
pub struct EphemeralFileGuard {
    pub path: PathBuf,
    pub deleted: std::sync::atomic::AtomicBool,
}

impl EphemeralFileGuard {
    /// Removes the owned file. A failed removal leaves the guard retryable.
    ///
    /// Parent directory removal is best effort: another output may still own
    /// files in either directory.
    pub fn cleanup(&self) -> std::io::Result<()> {
        use std::sync::atomic::Ordering;

        if self.deleted.load(Ordering::Acquire) {
            return Ok(());
        }

        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        // Mark deletion only after the file is absent.
        self.deleted.store(true, Ordering::Release);

        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
            if let Some(root) = parent.parent() {
                let _ = std::fs::remove_dir(root);
            }
        }

        Ok(())
    }
}

impl Drop for EphemeralFileGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn cleanup_cancelled_result(res: &DocumentResult) {
    if let DocumentResult::File { path, .. } = res {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
            if let Some(root) = parent.parent() {
                let _ = std::fs::remove_dir(root);
            }
        }
    }
}

/// Body of an HTTP response, either in-memory bytes or a streamed file path.
#[derive(Debug, Clone)]
pub enum HttpBody {
    Bytes(Vec<u8>),
    File { file: Arc<std::sync::Mutex<std::fs::File>>, path: PathBuf, size: u64, guard: Option<Arc<EphemeralFileGuard>> },
}

impl HttpBody {
    pub fn len(&self) -> u64 {
        match self {
            Self::Bytes(b) => b.len() as u64,
            Self::File { size, .. } => *size,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn try_to_bytes(&self) -> Result<Vec<u8>, std::io::Error> {
        match self {
            Self::Bytes(bytes) => Ok(bytes.clone()),
            Self::File { file, guard, .. } => {
                use std::io::{Read, Seek};

                let mut data = Vec::new();
                {
                    let mut reader = file.lock().map_err(|_| std::io::Error::other("File lock poisoned"))?;
                    reader.rewind()?;
                    reader.read_to_end(&mut data)?;
                }

                if let Some(guard) = guard {
                    guard.cleanup()?;
                }

                Ok(data)
            }
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, std::io::Error> {
        self.try_to_bytes()
    }
}

impl PartialEq<[u8]> for HttpBody {
    fn eq(&self, other: &[u8]) -> bool {
        self.eq(&other)
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for HttpBody {
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.eq(other.as_slice())
    }
}

impl PartialEq<&[u8]> for HttpBody {
    fn eq(&self, other: &&[u8]) -> bool {
        match self {
            Self::Bytes(b) => b.as_slice() == *other,
            Self::File { file, .. } => {
                let mut data = Vec::new();
                if let Ok(mut reader) = file.lock() {
                    use std::io::{Read, Seek, SeekFrom};
                    let initial_pos = reader.stream_position().ok();
                    if let Some(pos) = initial_pos {
                        let read_ok = reader.rewind().is_ok() && reader.read_to_end(&mut data).is_ok();
                        let restored = reader.seek(SeekFrom::Start(pos)).is_ok();
                        return read_ok && restored && data.as_slice() == *other;
                    }
                }
                false
            }
        }
    }
}

/// Representation of an HTTP response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: HttpBody,
}

impl HttpResponse {
    pub fn ok_json<T: serde::Serialize>(data: &T) -> Self {
        let body = match serde_json::to_vec(data) {
            Ok(body) => body,
            Err(error) => return Self::problem(ProblemDetails::internal_error(format!("Failed to serialize response: {error}"))),
        };
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: 200, headers, body: HttpBody::Bytes(body) }
    }

    pub fn accepted_json<T: serde::Serialize>(location: &str, data: &T) -> Self {
        let body = match serde_json::to_vec(data) {
            Ok(body) => body,
            Err(error) => return Self::problem(ProblemDetails::internal_error(format!("Failed to serialize response: {error}"))),
        };
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        headers.insert("location".into(), location.into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: 202, headers, body: HttpBody::Bytes(body) }
    }

    pub fn ok_bytes(mime_type: &str, body: Vec<u8>) -> Self {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), mime_type.into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: 200, headers, body: HttpBody::Bytes(body) }
    }

    pub fn ok_file(mime_type: &str, path: PathBuf) -> Self {
        Self::ok_file_with_cleanup(mime_type, path, false)
    }

    pub fn ok_ephemeral_file(mime_type: &str, path: PathBuf) -> Self {
        Self::ok_file_with_cleanup(mime_type, path, true)
    }

    pub fn ok_file_with_cleanup(mime_type: &str, path: PathBuf, delete_on_stream: bool) -> Self {
        match std::fs::File::open(&path) {
            Ok(file) => match file.metadata() {
                Ok(m) => {
                    let len = m.len();
                    let mut headers = HashMap::new();
                    headers.insert("content-type".into(), mime_type.into());
                    headers.insert("content-length".into(), len.to_string());
                    let guard = if delete_on_stream {
                        Some(Arc::new(EphemeralFileGuard { path: path.clone(), deleted: std::sync::atomic::AtomicBool::new(false) }))
                    } else {
                        None
                    };
                    Self { status: 200, headers, body: HttpBody::File { file: Arc::new(std::sync::Mutex::new(file)), path, size: len, guard } }
                }
                Err(e) => Self::problem(ProblemDetails::internal_error(format!("Failed to inspect output file metadata: {e}"))),
            },
            Err(e) => Self::problem(ProblemDetails::internal_error(format!("Failed to open output file: {e}"))),
        }
    }

    pub fn problem(problem: ProblemDetails) -> Self {
        let body = serde_json::to_vec(&problem).unwrap_or_default();
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/problem+json".into());
        headers.insert("content-length".into(), body.len().to_string());
        Self { status: problem.status, headers, body: HttpBody::Bytes(body) }
    }
}

/// Reads every split artifact or fails without returning a partial success.
fn read_split_response(split: &pdfcraft_sdk::SplitResult) -> Result<SplitResponseDto, std::io::Error> {
    read_split_response_with_limit(split, 100 * 1024 * 1024)
}

fn read_split_response_with_limit(split: &pdfcraft_sdk::SplitResult, limit: u64) -> Result<SplitResponseDto, std::io::Error> {
    use std::io::Read;
    let mut parts = Vec::new();
    let exceeded = || std::io::Error::new(std::io::ErrorKind::InvalidData, "Aggregate split output exceeds response limit");
    // {"parts":[]} plus commas between parts. Count JSON-escaped names and decimal sizes exactly.
    let mut remaining = limit.checked_sub(12).ok_or_else(exceeded)?;

    for path in &split.files {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Split output has no valid UTF-8 filename"))?
            .to_string();

        let file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        let size_bytes = usize::try_from(size).map_err(|_| exceeded())?;
        let shell = SplitPartDto { filename, size_bytes, data_base64: String::new() };
        let overhead = (serde_json::to_vec(&shell)?.len() as u64).checked_add(u64::from(!parts.is_empty())).ok_or_else(exceeded)?;
        let encoded = size.checked_add(2).and_then(|n| (n / 3).checked_mul(4)).ok_or_else(exceeded)?;
        remaining = remaining.checked_sub(overhead).and_then(|n| n.checked_sub(encoded)).ok_or_else(exceeded)?;
        let mut data = Vec::new();
        file.take(size.saturating_add(1)).read_to_end(&mut data)?;
        if data.len() != size_bytes {
            return Err(exceeded());
        }
        parts.push(SplitPartDto { data_base64: encode_base64(&data), ..shell });
    }

    Ok(SplitResponseDto { parts })
}

/// Owns synchronous split artifacts until they have been packaged or rejected.
struct EphemeralSplitGuard<'a>(&'a pdfcraft_sdk::SplitResult);

impl Drop for EphemeralSplitGuard<'_> {
    fn drop(&mut self) {
        for path in &self.0.files {
            drop(EphemeralFileGuard { path: path.clone(), deleted: std::sync::atomic::AtomicBool::new(false) });
        }
    }
}

/// Central document service dispatcher.
pub struct RestService {
    executor: Arc<BlockingExecutor>,
    repository: Arc<dyn JobRepository>,
    auth_token: Option<String>,
}

impl RestService {
    pub fn new(executor: Arc<BlockingExecutor>, repository: Arc<dyn JobRepository>, auth_token: Option<String>) -> Self {
        Self { executor, repository, auth_token }
    }

    /// Dispatches an incoming HTTP request to the matching endpoint.
    pub fn dispatch(&self, req: &HttpRequest) -> HttpResponse {
        // 1. Bearer Token Authentication check
        if let Some(expected_token) = &self.auth_token {
            let auth_header = req.header("authorization").unwrap_or("");
            let token = auth_header.strip_prefix("Bearer ").map(str::trim);
            if token != Some(expected_token.as_str()) {
                return HttpResponse::problem(ProblemDetails::unauthorized("Missing or invalid bearer token"));
            }
        }

        let path = req.path.trim_end_matches('/');
        let method = req.method.to_uppercase();

        match (method.as_str(), path) {
            ("POST", "/v1/merged-pdf") => self.handle_merge(req),
            ("POST", "/v1/split-pdf") => self.handle_split(req),
            ("POST", "/v1/page-preview") => self.handle_page_preview(req),
            ("POST", "/v1/batch") => self.handle_batch(req),
            ("POST", p) if p.starts_with("/v1/operations/") => {
                let name = &p["/v1/operations/".len()..];
                if name.is_empty() || name.contains('/') {
                    HttpResponse::problem(ProblemDetails::not_found(format!("Route not found: {} {}", req.method, req.path)))
                } else {
                    self.handle_call_tool(name, req)
                }
            }
            ("GET", p) if p.starts_with("/v1/jobs/") => {
                let suffix = &p["/v1/jobs/".len()..];
                if let Some((id_str, "result")) = suffix.split_once('/') {
                    self.handle_get_job_result(&JobId::new(id_str))
                } else {
                    self.handle_get_job(&JobId::new(suffix))
                }
            }
            ("DELETE", p) if p.starts_with("/v1/jobs/") => {
                let id_str = &p["/v1/jobs/".len()..];
                self.handle_cancel_job(&JobId::new(id_str))
            }
            _ => HttpResponse::problem(ProblemDetails::not_found(format!("Route not found: {} {}", req.method, req.path))),
        }
    }

    fn handle_merge(&self, req: &HttpRequest) -> HttpResponse {
        let payload: MergePayload = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
        };

        if payload.files.is_empty() {
            return HttpResponse::problem(ProblemDetails::bad_request("Files array cannot be empty"));
        }

        let mut sources = Vec::new();
        for file in payload.files {
            match file.into_source() {
                Ok(src) => sources.push(src),
                Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(e)),
            }
        }

        if payload.async_job {
            let job_id = JobId::generate();
            let record = JobRecord::new_queued(job_id.clone(), "merge", None);
            if let Err(e) = self.repository.create(record) {
                return HttpResponse::problem(ProblemDetails::internal_error(e.to_string()));
            }

            let executor = Arc::clone(&self.executor);
            let repository = Arc::clone(&self.repository);
            let jid = job_id.clone();
            let opt = payload.options;

            let spawn_res = std::thread::Builder::new().name(format!("job-merge-{}", job_id.0)).spawn(move || {
                if repository.transition(&jid, JobState::Queued, JobState::Running, None, None, None).is_err() {
                    return;
                }
                match executor.execute_merge(&sources, &opt) {
                    Ok(res) => {
                        if repository.transition(&jid, JobState::Running, JobState::Succeeded, None, Some(res.clone()), None).is_err() {
                            cleanup_cancelled_result(&res);
                        }
                    }
                    Err(e) => {
                        let _ = repository.transition(&jid, JobState::Running, JobState::Failed, Some(e.to_string()), None, None);
                    }
                }
            });

            if let Err(e) = spawn_res {
                let _ = self.repository.transition(
                    &job_id,
                    JobState::Queued,
                    JobState::Failed,
                    Some(format!("Failed to spawn worker thread: {e}")),
                    None,
                    None,
                );
                return HttpResponse::problem(ProblemDetails::service_unavailable(format!("Failed to spawn worker thread: {e}")));
            }

            let response_body = JobResponse {
                id: job_id.0.clone(),
                operation: "merge".into(),
                status: JobState::Queued,
                progress: 0,
                created_at: 0,
                updated_at: 0,
                error: None,
                result_url: None,
            };
            HttpResponse::accepted_json(&format!("/v1/jobs/{}", job_id.0), &response_body)
        } else {
            match self.executor.execute_merge(&sources, &payload.options) {
                Ok(DocumentResult::Memory { data, mime_type }) => HttpResponse::ok_bytes(&mime_type, data),
                Ok(DocumentResult::File { path, mime_type, .. }) => HttpResponse::ok_ephemeral_file(&mime_type, path),
                Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Merge Failed", e.to_string(), e.error_code())),
            }
        }
    }

    fn handle_split(&self, req: &HttpRequest) -> HttpResponse {
        let payload: SplitPayload = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
        };

        let source = match payload.file.into_source() {
            Ok(src) => src,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(e)),
        };

        if payload.async_job {
            let job_id = JobId::generate();
            let record = JobRecord::new_queued(job_id.clone(), "split", None);
            if let Err(e) = self.repository.create(record) {
                return HttpResponse::problem(ProblemDetails::internal_error(e.to_string()));
            }

            let executor = Arc::clone(&self.executor);
            let repository = Arc::clone(&self.repository);
            let jid = job_id.clone();
            let mode = payload.mode;

            let spawn_res = std::thread::Builder::new().name(format!("job-split-{}", job_id.0)).spawn(move || {
                if repository.transition(&jid, JobState::Queued, JobState::Running, None, None, None).is_err() {
                    return;
                }
                match executor.execute_split(&source, &mode) {
                    Ok(res) => {
                        if repository.transition(&jid, JobState::Running, JobState::Succeeded, None, None, Some(res.clone())).is_err() {
                            for f in &res.files {
                                let _ = std::fs::remove_file(f);
                                if let Some(parent) = f.parent() {
                                    let _ = std::fs::remove_dir(parent);
                                    if let Some(root) = parent.parent() {
                                        let _ = std::fs::remove_dir(root);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = repository.transition(&jid, JobState::Running, JobState::Failed, Some(e.to_string()), None, None);
                    }
                }
            });

            if let Err(e) = spawn_res {
                let _ = self.repository.transition(
                    &job_id,
                    JobState::Queued,
                    JobState::Failed,
                    Some(format!("Failed to spawn worker thread: {e}")),
                    None,
                    None,
                );
                return HttpResponse::problem(ProblemDetails::service_unavailable(format!("Failed to spawn worker thread: {e}")));
            }

            let response_body = JobResponse {
                id: job_id.0.clone(),
                operation: "split".into(),
                status: JobState::Queued,
                progress: 0,
                created_at: 0,
                updated_at: 0,
                error: None,
                result_url: None,
            };
            HttpResponse::accepted_json(&format!("/v1/jobs/{}", job_id.0), &response_body)
        } else {
            match self.executor.execute_split(&source, &payload.mode) {
                Ok(split_res) => {
                    let _cleanup = EphemeralSplitGuard(&split_res);

                    match read_split_response(&split_res) {
                        Ok(dto) => HttpResponse::ok_json(&dto),
                        Err(error) => HttpResponse::problem(ProblemDetails::internal_error(format!("Failed to read split output: {error}"))),
                    }
                }
                Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Split Failed", e.to_string(), e.error_code())),
            }
        }
    }

    fn handle_page_preview(&self, req: &HttpRequest) -> HttpResponse {
        let payload: RenderPayload = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
        };

        if payload.page == 0 {
            return HttpResponse::problem(ProblemDetails::bad_request("Page index must be >= 1 (1-based index)"));
        }

        let source = match payload.file.into_source() {
            Ok(src) => src,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(e)),
        };

        if payload.async_job {
            let job_id = JobId::generate();
            let record = JobRecord::new_queued(job_id.clone(), "render", None);
            if let Err(e) = self.repository.create(record) {
                return HttpResponse::problem(ProblemDetails::internal_error(e.to_string()));
            }

            let executor = Arc::clone(&self.executor);
            let repository = Arc::clone(&self.repository);
            let jid = job_id.clone();
            let page = payload.page;
            let options = payload.options;

            let spawn_res = std::thread::Builder::new().name(format!("job-render-{}", job_id.0)).spawn(move || {
                if repository.transition(&jid, JobState::Queued, JobState::Running, None, None, None).is_err() {
                    return;
                }
                match executor.execute_render(&source, page, &options) {
                    Ok(res) => {
                        if repository.transition(&jid, JobState::Running, JobState::Succeeded, None, Some(res.clone()), None).is_err() {
                            cleanup_cancelled_result(&res);
                        }
                    }
                    Err(e) => {
                        let _ = repository.transition(&jid, JobState::Running, JobState::Failed, Some(e.to_string()), None, None);
                    }
                }
            });

            if let Err(e) = spawn_res {
                let _ = self.repository.transition(
                    &job_id,
                    JobState::Queued,
                    JobState::Failed,
                    Some(format!("Failed to spawn worker thread: {e}")),
                    None,
                    None,
                );
                return HttpResponse::problem(ProblemDetails::service_unavailable(format!("Failed to spawn worker thread: {e}")));
            }

            let response_body = JobResponse {
                id: job_id.0.clone(),
                operation: "render".into(),
                status: JobState::Queued,
                progress: 0,
                created_at: 0,
                updated_at: 0,
                error: None,
                result_url: None,
            };
            HttpResponse::accepted_json(&format!("/v1/jobs/{}", job_id.0), &response_body)
        } else {
            match self.executor.execute_render(&source, payload.page, &payload.options) {
                Ok(DocumentResult::Memory { data, mime_type }) => HttpResponse::ok_bytes(&mime_type, data),
                Ok(DocumentResult::File { path, mime_type, .. }) => HttpResponse::ok_ephemeral_file(&mime_type, path),
                Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Render Failed", e.to_string(), e.error_code())),
            }
        }
    }

    /// Executes any named automation tool with the request body as its JSON arguments.
    ///
    /// Each call runs in its own ephemeral sandbox (see `AutomationEngineAdapter`), so tool
    /// sequences that depend on state from a prior call (e.g. reusing a `doc_open` document id)
    /// are not supported through this route; this is a generic one-shot bridge, not a session.
    fn handle_call_tool(&self, name: &str, req: &HttpRequest) -> HttpResponse {
        let args: serde_json::Value = if req.body.is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON payload: {e}"))),
            }
        };

        match self.executor.execute_call_tool(name, &args) {
            Ok(content) => match content.as_slice() {
                [ToolContent::Png { data, .. }] => HttpResponse::ok_bytes("image/png", data.clone()),
                _ => {
                    let dtos: Vec<ContentDto> = content.iter().map(ContentDto::from).collect();
                    HttpResponse::ok_json(&dtos)
                }
            },
            Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Operation Failed", e.to_string(), e.error_code())),
        }
    }

    /// Executes a sequential batch of automation tools in a single engine instance.
    ///
    /// Enables stateful multi-step pipelines (e.g. `doc_open` -> `comment_add` -> `doc_save`)
    /// without long-lived server sessions or memory leaks. Execution halts on first failure.
    fn handle_batch(&self, req: &HttpRequest) -> HttpResponse {
        let steps: Vec<pdfcraft_sdk::ports::BatchStep> = match serde_json::from_slice(&req.body) {
            Ok(s) => s,
            Err(e) => return HttpResponse::problem(ProblemDetails::bad_request(format!("Invalid JSON batch payload: {e}"))),
        };

        if steps.is_empty() {
            return HttpResponse::problem(ProblemDetails::bad_request("Batch steps array cannot be empty"));
        }

        match self.executor.execute_batch(&steps) {
            Ok(report) => HttpResponse::ok_json(&report),
            Err(e) => HttpResponse::problem(ProblemDetails::new(e.http_status(), "Batch Execution Failed", e.to_string(), e.error_code())),
        }
    }

    fn handle_get_job(&self, id: &JobId) -> HttpResponse {
        match self.repository.get(id) {
            Ok(Some(record)) => HttpResponse::ok_json(&JobResponse::from(&record)),
            Ok(None) => HttpResponse::problem(ProblemDetails::not_found(format!("Job not found: {id}"))),
            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(e.to_string())),
        }
    }

    fn handle_get_job_result(&self, id: &JobId) -> HttpResponse {
        match self.repository.get(id) {
            Ok(Some(record)) => {
                if record.state != JobState::Succeeded {
                    return HttpResponse::problem(ProblemDetails::conflict(format!(
                        "Job {} has not succeeded yet (current state: {:?})",
                        id, record.state
                    )));
                }

                if let Some(res) = record.result {
                    match res {
                        DocumentResult::Memory { data, mime_type } => HttpResponse::ok_bytes(&mime_type, data),
                        DocumentResult::File { path, mime_type, .. } => HttpResponse::ok_file(&mime_type, path),
                    }
                } else if let Some(split) = record.split_result {
                    match read_split_response(&split) {
                        Ok(dto) => HttpResponse::ok_json(&dto),
                        Err(error) => HttpResponse::problem(ProblemDetails::internal_error(format!("Failed to read split job output: {error}"))),
                    }
                } else {
                    HttpResponse::problem(ProblemDetails::internal_error("Job succeeded but has no output result"))
                }
            }
            Ok(None) => HttpResponse::problem(ProblemDetails::not_found(format!("Job not found: {id}"))),
            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(e.to_string())),
        }
    }

    fn handle_cancel_job(&self, id: &JobId) -> HttpResponse {
        match self.repository.get(id) {
            Ok(Some(record)) => {
                if record.state.is_terminal() {
                    return HttpResponse::problem(ProblemDetails::conflict(format!("Cannot cancel terminal job {} in state {:?}", id, record.state)));
                }

                // If the job already had a spilled file result, clean it up
                if let Some(DocumentResult::File { path, .. }) = &record.result {
                    let _ = std::fs::remove_file(path);
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
                if let Some(split) = &record.split_result {
                    for f in &split.files {
                        let _ = std::fs::remove_file(f);
                        if let Some(parent) = f.parent() {
                            let _ = std::fs::remove_dir(parent);
                            if let Some(root) = parent.parent() {
                                let _ = std::fs::remove_dir(root);
                            }
                        }
                    }
                }

                match self.repository.transition(id, record.state, JobState::Canceled, Some("Canceled by user".into()), None, None) {
                    Ok(()) => {
                        let updated = self.repository.get(id).ok().flatten();
                        match updated {
                            Some(r) => HttpResponse::ok_json(&JobResponse::from(&r)),
                            None => HttpResponse::ok_json(&JobResponse {
                                id: id.0.clone(),
                                operation: record.operation,
                                status: JobState::Canceled,
                                progress: record.progress_pct,
                                created_at: record.created_at,
                                updated_at: record.updated_at,
                                error: Some("Canceled by user".into()),
                                result_url: None,
                            }),
                        }
                    }
                    Err(e) => HttpResponse::problem(ProblemDetails::conflict(e.to_string())),
                }
            }
            Ok(None) => HttpResponse::problem(ProblemDetails::not_found(format!("Job not found: {id}"))),
            Err(e) => HttpResponse::problem(ProblemDetails::internal_error(e.to_string())),
        }
    }
}

#[cfg(test)]
mod revision12_tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    #[test]
    fn split_response_budget_is_aggregate_and_inclusive() {
        let dir = tempfile::tempdir().unwrap();
        let second_name = if cfg!(windows) { "b.pdf" } else { "b\"\n.pdf" };
        let files: Vec<_> = ["a.pdf", second_name].iter().map(|name| dir.path().join(name)).collect();
        for path in &files {
            std::fs::write(path, b"12345").unwrap();
        }
        let split = pdfcraft_sdk::SplitResult { files };
        let dto = read_split_response_with_limit(&split, 1024).unwrap();
        let wire_len = serde_json::to_vec(&dto).unwrap().len() as u64;
        assert_eq!(read_split_response_with_limit(&split, wire_len).unwrap().parts.len(), 2);
        assert_eq!(read_split_response_with_limit(&split, wire_len - 1).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert!(read_split_response_with_limit(&split, 10).is_err());
    }

    #[test]
    fn architecture_json_serialization_failure_is_500() {
        struct Fails;
        impl serde::Serialize for Fails {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("intentional failure"))
            }
        }
        let response = HttpResponse::ok_json(&Fails);
        assert_eq!(response.status, 500);
        assert_eq!(response.headers.get("content-type").unwrap(), "application/problem+json");
        assert_eq!(HttpResponse::accepted_json("/job", &Fails).status, 500);
    }

    #[test]
    fn file_body_comparison_preserves_cursor_on_match_and_mismatch() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"abcdef").unwrap();
        let response = HttpResponse::ok_file("application/pdf", file.path().into());
        let HttpBody::File { file, .. } = &response.body else { panic!("expected file body") };
        for position in [0, 3, 6] {
            file.lock().unwrap().seek(SeekFrom::Start(position)).unwrap();
            assert_eq!(response.body, b"abcdef");
            assert_eq!(file.lock().unwrap().stream_position().unwrap(), position);
            assert_ne!(response.body, b"different");
            assert_eq!(file.lock().unwrap().stream_position().unwrap(), position);
        }
    }
}
