//! local.rs — In-process engine driver executing operations through an injected OperationExecutor.
//! ==================================================================================================
//!
//! Purpose:
//!     Provides direct in-process document processing linking directly to the Rust engine
//!     through the abstract OperationExecutor port (open-source alternative to Adobe PDFL).
//!
//! Layer:
//!     Client Layer / Facade
//!
//! Key Input Dependencies:
//!     - crate::ports::OperationExecutor
//!     - crate::engine_adapter::AutomationEngineAdapter
//!     - crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::{LocalClient, PdfCraftClient, DocumentSource, MergeOptions};
//!     let client = LocalClient::default();
//!     let doc = DocumentSource::from_bytes(b"%PDF-1.4...".to_vec(), None);
//!     let result = client.merge(&[doc], MergeOptions::default());
//!     ```
//!
//! Key Types & Functions Index:
//!     - LocalClient: In-process client implementing PdfCraftClient trait
//!     - LocalClient::default: Constructs client backed by AutomationEngineAdapter
//!     - LocalClient::with_executor: Constructs client with injected custom executor
//!     - LocalClient::merge: Merges multiple PDF sources
//!     - LocalClient::split: Splits PDF into separate files
//!     - LocalClient::render_page: Renders page at physical DPI
//!     - LocalClient::call_tool: Executes any named pdfcraft_automation tool with raw JSON arguments

use crate::PdfCraftClient;
use crate::annotation::{CommentInfo, CommentRequest, HighlightRequest, StampRequest, UnderlineRequest};
use crate::document::{
    Bookmark, BookmarkAddRequest, BookmarkRemoveRequest, DeletePagesOptions, ExtractPagesOptions, InsertPagesOptions, RotatePagesOptions,
};
use crate::error::SdkError;
use crate::ports::OperationExecutor;
use crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult};
use std::sync::Arc;

/// Local in-process client for running document transformations without an HTTP server.
pub struct LocalClient {
    executor: Arc<dyn OperationExecutor>,
    limits: ResourceLimits,
}

impl LocalClient {
    /// Creates a new `LocalClient` with the provided engine executor implementation and custom limits.
    pub fn new(executor: Arc<dyn OperationExecutor>) -> Self {
        Self { executor, limits: ResourceLimits::default() }
    }

    /// Configures resource constraints on this client, synchronizing with the underlying executor
    /// if it supports dynamic limit reconfiguration while preserving custom test doubles.
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        if let Some(new_exec) = self.executor.with_limits(&limits) {
            self.executor = new_exec;
        }
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    fn check_source(&self, source: &DocumentSource) -> Result<(), SdkError> {
        if let Some(len) = source.byte_len()
            && len > self.limits.max_request_bytes
        {
            return Err(SdkError::QuotaExceeded(format!(
                "Input document size ({len} bytes) exceeds configured limit ({} bytes)",
                self.limits.max_request_bytes
            )));
        }
        Ok(())
    }

    /// Executes any named `pdfcraft_automation` tool with raw JSON arguments.
    ///
    /// This is the generic bridge to the full automation tool set (see
    /// `pdfcraft_automation::tools()`), for callers that need tools without a dedicated typed
    /// method on [`PdfCraftClient`]. It is not part of that trait, since not every transport
    /// (e.g. a future narrow remote client) need expose the full tool surface.
    pub fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<Vec<crate::ports::ToolContent>, SdkError> {
        if name.trim().is_empty() {
            return Err(SdkError::InvalidArgument("Tool name must not be empty".into()));
        }
        self.executor.execute_call_tool(name, &args)
    }

    /// Executes a sequential batch of automation tools sharing a single engine instance.
    pub fn execute_batch(&self, steps: &[crate::ports::BatchStep]) -> Result<crate::ports::BatchReport, SdkError> {
        self.executor.execute_batch(steps)
    }

    /// Rotates designated pages within a PDF.
    pub fn rotate_pages(&self, file: &DocumentSource, options: &RotatePagesOptions) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_rotate_pages(file, options)
    }

    /// Extracts designated pages into a new document.
    pub fn extract_pages(&self, file: &DocumentSource, options: &ExtractPagesOptions) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_extract_pages(file, options)
    }

    /// Deletes designated pages from the document.
    pub fn delete_pages(&self, file: &DocumentSource, options: &DeletePagesOptions) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_delete_pages(file, options)
    }

    /// Inserts pages from one PDF into another.
    pub fn insert_pages(
        &self,
        file: &DocumentSource,
        insert_source: &DocumentSource,
        options: &InsertPagesOptions,
    ) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.check_source(insert_source)?;
        self.executor.execute_insert_pages(file, insert_source, options)
    }

    /// Lists the document's bookmark outline.
    pub fn list_bookmarks(&self, file: &DocumentSource) -> Result<Vec<Bookmark>, SdkError> {
        self.check_source(file)?;
        self.executor.execute_list_bookmarks(file)
    }

    /// Adds a bookmark.
    pub fn add_bookmark(&self, file: &DocumentSource, request: &BookmarkAddRequest) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_add_bookmark(file, request)
    }

    /// Removes a bookmark and its descendants.
    pub fn remove_bookmark(&self, file: &DocumentSource, request: &BookmarkRemoveRequest) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_remove_bookmark(file, request)
    }

    /// Highlights text on a page.
    pub fn add_highlight(&self, file: &DocumentSource, request: &HighlightRequest) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_add_highlight(file, request)
    }

    /// Underlines text on a page.
    pub fn add_underline(&self, file: &DocumentSource, request: &UnderlineRequest) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_add_underline(file, request)
    }

    /// Adds a stamp to a page.
    pub fn add_stamp(&self, file: &DocumentSource, request: &StampRequest) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_add_stamp(file, request)
    }

    /// Adds a sticky note comment.
    pub fn add_comment(&self, file: &DocumentSource, request: &CommentRequest) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_add_comment(file, request)
    }

    /// Lists comments across pages.
    pub fn list_comments(&self, file: &DocumentSource, page: Option<usize>) -> Result<Vec<CommentInfo>, SdkError> {
        self.check_source(file)?;
        self.executor.execute_list_comments(file, page)
    }

    /// Lists AcroForm fields in the document.
    pub fn get_form_fields(&self, file: &DocumentSource) -> Result<Vec<crate::forms::FormField>, SdkError> {
        self.check_source(file)?;
        self.executor.execute_get_form_fields(file)
    }

    /// Sets field values by name.
    pub fn fill_form(&self, file: &DocumentSource, data: &serde_json::Value) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_fill_form(file, data)
    }

    /// Flattens interactive form fields into static page content.
    pub fn flatten_form(&self, file: &DocumentSource) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_flatten_form(file)
    }

    /// Exports form field data.
    pub fn export_form_data(&self, file: &DocumentSource, format: &str) -> Result<Vec<u8>, SdkError> {
        self.check_source(file)?;
        self.executor.execute_export_form_data(file, format)
    }

    /// Imports form field data.
    pub fn import_form_data(&self, file: &DocumentSource, data_bytes: &[u8], format: &str) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        if data_bytes.len() as u64 > self.limits.max_request_bytes as u64 {
            return Err(SdkError::QuotaExceeded(format!(
                "Form data size ({} bytes) exceeds configured limit ({} bytes)",
                data_bytes.len(),
                self.limits.max_request_bytes
            )));
        }
        self.executor.execute_import_form_data(file, data_bytes, format)
    }

    /// Optimizes and compresses a PDF document.
    pub fn optimize(&self, file: &DocumentSource, params: &crate::optimization::OptimizationParams) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_optimize(file, params)
    }

    /// Reduces document file size.
    pub fn reduce_size(&self, file: &DocumentSource, max_mb: Option<f64>) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_reduce_size(file, max_mb)
    }

    /// Encrypts document with passwords and permissions.
    pub fn encrypt(
        &self,
        file: &DocumentSource,
        user_pw: Option<&str>,
        owner_pw: Option<&str>,
        perms: Option<u32>,
    ) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_encrypt(file, user_pw, owner_pw, perms)
    }

    /// Decrypts a password-protected document.
    pub fn decrypt(&self, file: &DocumentSource, password: &str) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_decrypt(file, password)
    }

    /// Redacts a specified rectangular region.
    pub fn redact(&self, file: &DocumentSource, page: usize, rect: &crate::graphics::Rect) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        self.executor.execute_redact(file, page, rect)
    }

    /// Digitally signs a PDF document.
    pub fn sign(&self, file: &DocumentSource, sign_doc: &crate::security::SignDoc) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        if sign_doc.cert_p12_bytes.len() as u64 > self.limits.max_request_bytes as u64 {
            return Err(SdkError::QuotaExceeded(format!(
                "Signing certificate size ({} bytes) exceeds configured limit ({} bytes)",
                sign_doc.cert_p12_bytes.len(),
                self.limits.max_request_bytes
            )));
        }
        self.executor.execute_sign(file, sign_doc)
    }
}

impl Default for LocalClient {
    fn default() -> Self {
        let limits = ResourceLimits::default();
        Self {
            executor: Arc::new(crate::engine_adapter::AutomationEngineAdapter::with_limits(limits.max_memory_bytes, limits.max_request_bytes as u64)),
            limits,
        }
    }
}

impl PdfCraftClient for LocalClient {
    fn merge(&self, files: &[DocumentSource], options: MergeOptions) -> Result<DocumentResult, SdkError> {
        if files.is_empty() {
            return Err(SdkError::InvalidArgument("files list cannot be empty".into()));
        }
        let mut total_len: usize = 0;
        for file in files {
            if let Some(len) = file.byte_len() {
                total_len = total_len.saturating_add(len);
                if total_len > self.limits.max_request_bytes {
                    return Err(SdkError::QuotaExceeded(format!(
                        "Total input size ({total_len} bytes) exceeds configured max_request_bytes ({} bytes)",
                        self.limits.max_request_bytes
                    )));
                }
            }
        }
        self.executor.execute_merge(files, &options)
    }

    fn split(&self, file: &DocumentSource, mode: SplitMode) -> Result<SplitResult, SdkError> {
        if let Some(len) = file.byte_len()
            && len > self.limits.max_request_bytes
        {
            return Err(SdkError::QuotaExceeded(format!(
                "File size ({len} bytes) exceeds configured max_request_bytes ({} bytes)",
                self.limits.max_request_bytes
            )));
        }
        self.executor.execute_split(file, &mode)
    }

    fn render_page(&self, file: &DocumentSource, page: usize, options: RenderOptions) -> Result<DocumentResult, SdkError> {
        self.check_source(file)?;
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based (greater than 0)".into()));
        }
        self.executor.execute_render(file, page, &options)
    }
}
