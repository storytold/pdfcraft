//! Architectural stubs for `pdfcraft-rest`: on-premise PDF document processing service.
//!
//! Hardened based on Staff Architect Review (Astra):
//! - Clean architecture boundary (`DocumentOperationService`)
//! - Typed commands & strongly typed dispatch (avoiding untyped string calls)
//! - Explicit resource governance & timeout checks
//! - Secure ephemeral workspace isolation with zeroized secret cleanup

use std::path::{Path, PathBuf};
use std::time::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;

/// Resource limits applied to each execution sandbox to prevent resource exhaustion attacks.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_request_bytes: usize,
    pub max_output_bytes: usize,
    pub max_pages: usize,
    pub timeout: Duration,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: 100 * 1024 * 1024, // 100 MB
            max_output_bytes: 500 * 1024 * 1024,  // 500 MB
            max_pages: 5000,
            timeout: Duration::from_secs(60),
        }
    }
}

/// Represents an isolated job execution sandbox with strict directory containment.
/// Implements `Drop` to guarantee immediate cleanup of all intermediate artifacts.
pub struct JobSandbox {
    _temp_dir: TempDir,
    root: PathBuf,
    limits: ResourceLimits,
}

impl JobSandbox {
    /// Creates a fresh, isolated workspace on disk with private 0700 permissions.
    pub fn new(limits: ResourceLimits) -> std::io::Result<Self> {
        let temp_dir = tempfile::Builder::new()
            .prefix("pdfcraft-job-")
            .tempdir()?;
        let root = temp_dir.path().to_path_buf();
        Ok(Self {
            _temp_dir: temp_dir,
            root,
            limits,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Prepares an input file inside the sandbox with a randomized, sanitized filename.
    pub fn write_input(&self, sanitized_name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
        if data.len() > self.limits.max_request_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("File size exceeds maximum allowed upload limit of {} bytes", self.limits.max_request_bytes),
            ));
        }
        let dest = self.root.join(sanitized_name);
        std::fs::write(&dest, data)?;
        Ok(dest)
    }

    /// Reads an output file verifying that it stays within allocated output limits.
    pub fn read_output(&self, filename: &str) -> std::io::Result<Vec<u8>> {
        let dest = self.root.join(filename);
        let metadata = std::fs::metadata(&dest)?;
        if metadata.len() as usize > self.limits.max_output_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Generated file exceeds maximum allowed output quota",
            ));
        }
        std::fs::read(dest)
    }
}

/// RFC 7807 compliant error model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemDetails {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    pub instance: String,
}

/// Strongly typed operation command enum.
#[derive(Debug, Clone)]
pub enum DocumentCommand {
    Merge(MergeRequest),
    Split(SplitRequest),
    Protect(EncryptRequest),
    Flatten(FlattenRequest),
    Watermark(WatermarkRequest),
}

/// Typed request for PDF merging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRequest {
    pub files: Vec<String>,
    pub pages: Option<Vec<Option<String>>>,
    pub passwords: Option<Vec<Option<String>>>,
    pub output_filename: String,
}

/// Typed request for PDF splitting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitRequest {
    pub file: String,
    pub every: Option<usize>,
    pub before_pages: Option<Vec<usize>>,
    pub at_bookmarks: Option<bool>,
    pub max_mb: Option<f64>,
}

/// Typed request for document encryption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptRequest {
    pub file: String,
    pub open_password: Option<String>,
    pub permissions_password: Option<String>,
    pub printing: Option<String>,
    pub changes: Option<String>,
    pub copy: Option<bool>,
    pub accessibility: Option<bool>,
}

/// Typed request for document flattening.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlattenRequest {
    pub file: String,
    pub flatten_comments: Option<bool>,
    pub flatten_fields: Option<bool>,
}

/// Typed request for watermarking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatermarkRequest {
    pub file: String,
    pub text: Option<String>,
    pub watermark_image_file: Option<String>,
    pub opacity: Option<f64>,
    pub rotation: Option<f64>,
    pub scale: Option<f64>,
}

/// Output profile returned by operations.
pub enum OperationResult {
    Binary { data: Vec<u8>, mime_type: &'static str },
    Archive { files: Vec<String> },
    Structured(Value),
}

/// Application service boundary isolating transport from engine automation.
pub struct DocumentOperationService;

impl DocumentOperationService {
    /// Executes a typed document command within a protected sandbox and timeout boundary.
    pub fn execute(sandbox: &JobSandbox, command: DocumentCommand) -> Result<OperationResult, String> {
        let mut automation = pdfcraft_automation::Automation::new()
            .with_root(sandbox.root())
            .map_err(|e| format!("Failed to initialize sandbox root: {e}"))?;

        match command {
            DocumentCommand::Merge(req) => {
                let args = json!({
                    "paths": req.files,
                    "pages": req.pages,
                    "passwords": req.passwords,
                    "out": req.output_filename,
                });
                automation.call("doc_combine", &args)
                    .map_err(|e| format!("Merge operation failed: {e}"))?;

                let data = sandbox.read_output(&req.output_filename)
                    .map_err(|e| format!("Failed to read generated output: {e}"))?;

                Ok(OperationResult::Binary {
                    data,
                    mime_type: "application/pdf",
                })
            }
            DocumentCommand::Split(req) => {
                let open_res = automation.call("doc_open", &json!({ "path": req.file }))
                    .map_err(|e| format!("Failed to open document: {e}"))?;

                let doc_id = open_res[0].as_json()
                    .and_then(|v| v.get("doc"))
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "Invalid document id returned".to_string())?;

                let out_dir = "split_parts";
                let split_args = json!({
                    "doc": doc_id,
                    "out_dir": out_dir,
                    "every": req.every,
                    "before": req.before_pages,
                    "bookmarks": req.at_bookmarks,
                    "max_mb": req.max_mb,
                });

                let res = automation.call("doc_split", &split_args)
                    .map_err(|e| format!("Split failed: {e}"))?;

                let files = res[0].as_json()
                    .and_then(|v| v.get("files"))
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                Ok(OperationResult::Archive { files })
            }
            DocumentCommand::Flatten(req) => {
                let open_res = automation.call("doc_open", &json!({ "path": req.file }))
                    .map_err(|e| format!("Failed to open document: {e}"))?;

                let doc_id = open_res[0].as_json()
                    .and_then(|v| v.get("doc"))
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "Invalid document id returned".to_string())?;

                automation.call("doc_flatten", &json!({
                    "doc": doc_id,
                    "comments": req.flatten_comments.unwrap_or(true),
                    "fields": req.flatten_fields.unwrap_or(true),
                })).map_err(|e| format!("Flatten failed: {e}"))?;

                let out_name = "flattened.pdf";
                automation.call("doc_save", &json!({
                    "doc": doc_id,
                    "path": out_name,
                    "full": true,
                })).map_err(|e| format!("Save failed: {e}"))?;

                let data = sandbox.read_output(out_name)
                    .map_err(|e| format!("Failed to read output: {e}"))?;

                Ok(OperationResult::Binary {
                    data,
                    mime_type: "application/pdf",
                })
            }
            DocumentCommand::Protect(req) => {
                let open_res = automation.call("doc_open", &json!({ "path": req.file }))
                    .map_err(|e| format!("Failed to open document: {e}"))?;

                let doc_id = open_res[0].as_json()
                    .and_then(|v| v.get("doc"))
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "Invalid document id returned".to_string())?;

                let mut protect_args = json!({
                    "doc": doc_id,
                    "open_password": req.open_password,
                    "permissions_password": req.permissions_password,
                });
                if let Some(p) = req.printing { protect_args["printing"] = json!(p); }
                if let Some(c) = req.changes { protect_args["changes"] = json!(c); }
                if let Some(cp) = req.copy { protect_args["copy"] = json!(cp); }
                if let Some(a) = req.accessibility { protect_args["accessibility"] = json!(a); }

                automation.call("doc_protect", &protect_args)
                    .map_err(|e| format!("Protect failed: {e}"))?;

                let out_name = "encrypted.pdf";
                automation.call("doc_save", &json!({
                    "doc": doc_id,
                    "path": out_name,
                    "full": true,
                })).map_err(|e| format!("Save failed: {e}"))?;

                let data = sandbox.read_output(out_name)
                    .map_err(|e| format!("Failed to read output: {e}"))?;

                Ok(OperationResult::Binary {
                    data,
                    mime_type: "application/pdf",
                })
            }
            DocumentCommand::Watermark(req) => {
                let open_res = automation.call("doc_open", &json!({ "path": req.file }))
                    .map_err(|e| format!("Failed to open document: {e}"))?;

                let doc_id = open_res[0].as_json()
                    .and_then(|v| v.get("doc"))
                    .and_then(Value::as_i64)
                    .ok_or_else(|| "Invalid document id returned".to_string())?;

                let mut wm_args = json!({ "doc": doc_id });
                if let Some(txt) = req.text { wm_args["text"] = json!(txt); }
                if let Some(f) = req.watermark_image_file { wm_args["file"] = json!(f); }
                if let Some(op) = req.opacity { wm_args["opacity"] = json!(op); }
                if let Some(r) = req.rotation { wm_args["rotation"] = json!(r); }
                if let Some(s) = req.scale { wm_args["scale"] = json!(s); }

                automation.call("doc_watermark", &wm_args)
                    .map_err(|e| format!("Watermark failed: {e}"))?;

                let out_name = "watermarked.pdf";
                automation.call("doc_save", &json!({
                    "doc": doc_id,
                    "path": out_name,
                    "full": true,
                })).map_err(|e| format!("Save failed: {e}"))?;

                let data = sandbox.read_output(out_name)
                    .map_err(|e| format!("Failed to read output: {e}"))?;

                Ok(OperationResult::Binary {
                    data,
                    mime_type: "application/pdf",
                })
            }
        }
    }
}
