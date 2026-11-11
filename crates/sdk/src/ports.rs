//! ports.rs — Application ports decoupling client facades from concrete engine execution.
//! =========================================================================================
//!
//! Purpose:
//!     Enforces Hexagonal / Ports-and-Adapters boundary so `LocalClient` depends
//!     on an abstract `OperationExecutor` rather than concrete engine structs.
//!
//! Layer:
//!     Application Layer / Ports
//!
//! Key Input Dependencies:
//!     - crate::error::SdkError
//!     - crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode, SplitResult}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::ports::OperationExecutor;
//!     // Implemented by AutomationEngineAdapter or testing mocks
//!     ```
//!
//! Key Types & Functions Index:
//!     - OperationCommand: Strongly typed document command for execution
//!     - OperationExecutor: Abstract engine executor trait (execute_merge, execute_split, execute_render, execute_call_tool)

use crate::annotation::{CommentInfo, CommentRequest, HighlightRequest, StampRequest, UnderlineRequest};
use crate::document::{
    Bookmark, BookmarkAddRequest, BookmarkRemoveRequest, DeletePagesOptions, ExtractPagesOptions, InsertPagesOptions, RotatePagesOptions,
};
use crate::error::SdkError;
use crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode, SplitResult};

/// Strongly typed document command for execution by an engine adapter.
#[derive(Debug)]
pub enum OperationCommand {
    Merge { sources: Vec<DocumentSource>, options: MergeOptions },
    Split { source: DocumentSource, mode: SplitMode },
    RenderPage { source: DocumentSource, page: usize, options: RenderOptions },
}

/// Default error returned by an executor that does not implement an optional operation.
fn unsupported(op: &str) -> SdkError {
    SdkError::UnsupportedOperation(format!("{op} is not implemented by this executor"))
}

/// Abstract engine executor port.
///
/// The document-manipulation and annotation methods carry default implementations so adapters
/// and test mocks written before Phase 2 keep compiling; [`AutomationEngineAdapter`](crate::engine_adapter::AutomationEngineAdapter)
/// overrides every one of them with a real implementation.
pub trait OperationExecutor: Send + Sync {
    /// Returns a new executor instance configured with updated resource limits,
    /// or `None` if this executor does not support dynamic limit reconfiguration.
    fn with_limits(&self, _limits: &crate::types::ResourceLimits) -> Option<std::sync::Arc<dyn OperationExecutor>> {
        None
    }

    /// Executes a document command and returns a result or a structured SdkError.
    fn execute_merge(&self, sources: &[DocumentSource], options: &MergeOptions) -> Result<DocumentResult, SdkError>;
    fn execute_split(&self, source: &DocumentSource, mode: &SplitMode) -> Result<SplitResult, SdkError>;
    fn execute_render(&self, source: &DocumentSource, page: usize, options: &RenderOptions) -> Result<DocumentResult, SdkError>;

    /// Executes any named tool from `pdfcraft_automation::tools()` with raw JSON arguments.
    ///
    /// This is the universal bridge used by generic dispatch surfaces (the REST
    /// `/v1/operations/{name}` route, scripting, MCP-style callers) that must reach all of the
    /// automation tool set without a dedicated typed method per tool. Each call runs in its own
    /// ephemeral sandbox, so tool sequences that depend on state from a prior call (e.g. a
    /// `doc_open` result reused by a later call) are out of scope here.
    fn execute_call_tool(&self, name: &str, args: &serde_json::Value) -> Result<Vec<ToolContent>, SdkError>;

    /// Rotates pages (Acrobat: Organize Pages ▸ Rotate) and returns the saved document.
    fn execute_rotate_pages(&self, source: &DocumentSource, options: &RotatePagesOptions) -> Result<DocumentResult, SdkError> {
        let _ = (source, options);
        Err(unsupported("execute_rotate_pages"))
    }

    /// Copies pages into a new document (Acrobat: Organize Pages ▸ Extract), optionally deleting
    /// them from the source afterward.
    fn execute_extract_pages(&self, source: &DocumentSource, options: &ExtractPagesOptions) -> Result<DocumentResult, SdkError> {
        let _ = (source, options);
        Err(unsupported("execute_extract_pages"))
    }

    /// Deletes pages (Acrobat: Organize Pages ▸ Delete) and returns the saved document.
    fn execute_delete_pages(&self, source: &DocumentSource, options: &DeletePagesOptions) -> Result<DocumentResult, SdkError> {
        let _ = (source, options);
        Err(unsupported("execute_delete_pages"))
    }

    /// Inserts pages of `insert_source` into `source` (Acrobat: Organize Pages ▸ Insert) and
    /// returns the saved document.
    fn execute_insert_pages(
        &self,
        source: &DocumentSource,
        insert_source: &DocumentSource,
        options: &InsertPagesOptions,
    ) -> Result<DocumentResult, SdkError> {
        let _ = (source, insert_source, options);
        Err(unsupported("execute_insert_pages"))
    }

    /// Lists the document's bookmark outline.
    fn execute_list_bookmarks(&self, source: &DocumentSource) -> Result<Vec<Bookmark>, SdkError> {
        let _ = source;
        Err(unsupported("execute_list_bookmarks"))
    }

    /// Adds a bookmark and returns the saved document.
    fn execute_add_bookmark(&self, source: &DocumentSource, request: &BookmarkAddRequest) -> Result<DocumentResult, SdkError> {
        let _ = (source, request);
        Err(unsupported("execute_add_bookmark"))
    }

    /// Removes a bookmark (and the bookmarks under it) and returns the saved document.
    fn execute_remove_bookmark(&self, source: &DocumentSource, request: &BookmarkRemoveRequest) -> Result<DocumentResult, SdkError> {
        let _ = (source, request);
        Err(unsupported("execute_remove_bookmark"))
    }

    /// Adds a highlight text-markup annotation and returns the saved document.
    fn execute_add_highlight(&self, source: &DocumentSource, request: &HighlightRequest) -> Result<DocumentResult, SdkError> {
        let _ = (source, request);
        Err(unsupported("execute_add_highlight"))
    }

    /// Adds an underline text-markup annotation and returns the saved document.
    fn execute_add_underline(&self, source: &DocumentSource, request: &UnderlineRequest) -> Result<DocumentResult, SdkError> {
        let _ = (source, request);
        Err(unsupported("execute_add_underline"))
    }

    /// Adds a named stamp annotation and returns the saved document.
    fn execute_add_stamp(&self, source: &DocumentSource, request: &StampRequest) -> Result<DocumentResult, SdkError> {
        let _ = (source, request);
        Err(unsupported("execute_add_stamp"))
    }

    /// Adds a sticky note comment and returns the saved document.
    fn execute_add_comment(&self, source: &DocumentSource, request: &CommentRequest) -> Result<DocumentResult, SdkError> {
        let _ = (source, request);
        Err(unsupported("execute_add_comment"))
    }

    /// Lists comments (annotations other than links, form widgets and pop-ups), optionally
    /// filtered to one 1-based page.
    fn execute_list_comments(&self, source: &DocumentSource, page: Option<usize>) -> Result<Vec<CommentInfo>, SdkError> {
        let _ = (source, page);
        Err(unsupported("execute_list_comments"))
    }

    /// Lists every AcroForm field's current state.
    fn execute_get_form_fields(&self, source: &DocumentSource) -> Result<Vec<crate::forms::FormField>, SdkError> {
        let _ = source;
        Err(unsupported("execute_get_form_fields"))
    }

    /// Sets field values by name (`{"field name": value}`) and returns the saved document.
    fn execute_fill_form(&self, source: &DocumentSource, data: &serde_json::Value) -> Result<DocumentResult, SdkError> {
        let _ = (source, data);
        Err(unsupported("execute_fill_form"))
    }

    /// Bakes every form field's current appearance into page content, removing interactivity.
    fn execute_flatten_form(&self, source: &DocumentSource) -> Result<DocumentResult, SdkError> {
        let _ = source;
        Err(unsupported("execute_flatten_form"))
    }

    /// Exports form field data as FDF, XFDF, XML, or JSON bytes.
    fn execute_export_form_data(&self, source: &DocumentSource, format: &str) -> Result<Vec<u8>, SdkError> {
        let _ = (source, format);
        Err(unsupported("execute_export_form_data"))
    }

    /// Imports field data of the given format into the document.
    fn execute_import_form_data(&self, source: &DocumentSource, data_bytes: &[u8], format: &str) -> Result<DocumentResult, SdkError> {
        let _ = (source, data_bytes, format);
        Err(unsupported("execute_import_form_data"))
    }

    /// Optimizes and compresses a PDF document.
    fn execute_optimize(&self, source: &DocumentSource, params: &crate::optimization::OptimizationParams) -> Result<DocumentResult, SdkError> {
        let _ = (source, params);
        Err(unsupported("execute_optimize"))
    }

    /// Reduces document file size.
    fn execute_reduce_size(&self, source: &DocumentSource, max_mb: Option<f64>) -> Result<DocumentResult, SdkError> {
        let _ = (source, max_mb);
        Err(unsupported("execute_reduce_size"))
    }

    /// Encrypts document with user/owner password and permissions.
    fn execute_encrypt(
        &self,
        source: &DocumentSource,
        user_pw: Option<&str>,
        owner_pw: Option<&str>,
        perms: Option<u32>,
    ) -> Result<DocumentResult, SdkError> {
        let _ = (source, user_pw, owner_pw, perms);
        Err(unsupported("execute_encrypt"))
    }

    /// Decrypts a password-protected document.
    fn execute_decrypt(&self, source: &DocumentSource, password: &str) -> Result<DocumentResult, SdkError> {
        let _ = (source, password);
        Err(unsupported("execute_decrypt"))
    }

    /// Applies redaction to a specified rectangular region.
    fn execute_redact(&self, source: &DocumentSource, page: usize, rect: &crate::graphics::Rect) -> Result<DocumentResult, SdkError> {
        let _ = (source, page, rect);
        Err(unsupported("execute_redact"))
    }

    /// Digitally signs a PDF document.
    fn execute_sign(&self, source: &DocumentSource, sign_doc: &crate::security::SignDoc) -> Result<DocumentResult, SdkError> {
        let _ = (source, sign_doc);
        Err(unsupported("execute_sign"))
    }

    /// Executes a sequential batch of automation tools sharing a single engine instance.
    ///
    /// Enables stateful multi-step pipelines (e.g. `doc_open` -> `comment_add` -> `doc_save`)
    /// without long-lived server sessions or memory leaks. Execution halts on first failure.
    fn execute_batch(&self, steps: &[BatchStep]) -> Result<BatchReport, SdkError> {
        let _ = steps;
        Err(unsupported("execute_batch"))
    }
}

/// Individual step in a multi-tool batch execution pipeline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchStep {
    pub tool: String,
    pub args: serde_json::Value,
}

/// Result of one step in a multi-tool batch execution pipeline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchStepResult {
    pub step: usize,
    pub tool: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Consolidated report returned from executing a batch pipeline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchReport {
    pub ok: bool,
    pub steps: Vec<BatchStepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_files: Option<Vec<std::path::PathBuf>>,
}

/// Generic tool execution result element decoupled from underlying automation types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolContent {
    Json(serde_json::Value),
    Png { data: Vec<u8>, width: u32, height: u32 },
}
