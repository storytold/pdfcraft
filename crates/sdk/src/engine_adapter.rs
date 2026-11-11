//! engine_adapter.rs — Concrete OperationExecutor adapter connecting crates/sdk to pdfcraft_automation.
//! =====================================================================================================
//!
//! Purpose:
//!     Executes document transformations headlessly through `pdfcraft_automation::Automation`
//!     inside an ephemeral sandbox with:
//!     - Panic containment (`std::panic::catch_unwind`) converting engine panics to `SdkError::EnginePanic`
//!     - Strict path containment within an isolated temporary directory
//!     - High-DPI physical rendering calculation decoupled from display PPI (#739)
//!     - Automatic disk spillover for outputs exceeding memory limits
//!
//! Layer:
//!     Infrastructure / Adapters Layer
//!
//! Key Input Dependencies:
//!     - pdfcraft_automation::Automation
//!     - tempfile::TempDir (Isolated ephemeral sandbox directory)
//!     - crate::ports::OperationExecutor
//!     - crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::AutomationEngineAdapter;
//!     let adapter = AutomationEngineAdapter::default();
//!     ```
//!
//! Key Types & Functions Index:
//!     - AutomationEngineAdapter: Concrete engine adapter implementing OperationExecutor
//!     - AutomationEngineAdapter::new: Constructs adapter with custom limits
//!     - AutomationEngineAdapter::default: Constructs adapter with standard production limits
//!     - ExecutionSandbox: RAII sandbox creating private tempdir with guaranteed cleanup on Drop
//!     - ExecutionSandbox::materialize_source: Writes DocumentSource to disk if needed
//!     - ExecutionSandbox::collect_output: Collects output and spills to disk if exceeding max_memory_bytes
//!     - AutomationEngineAdapter::execute_call_tool: Generic bridge dispatching any named automation tool

use std::cell::RefCell;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

use crate::annotation::{CommentInfo, CommentRequest, HighlightRequest, MarkupTarget, StampRequest, UnderlineRequest};
use crate::document::{
    Bookmark, BookmarkAddRequest, BookmarkRemoveRequest, DeletePagesOptions, ExtractPagesOptions, FitMode, InsertPagesOptions, PageOrientation,
    PageSubset, RotatePagesOptions, ViewDestination,
};
use crate::error::SdkError;
use crate::graphics::{Color, Quad, Rect};
use crate::ports::OperationExecutor;
use crate::types::{DocumentResult, DocumentSource, ImageFormat, MergeOptions, RenderOptions, SplitMode, SplitResult};
use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// Structured JSON payload of a tool result, if it is one.
fn json_of(c: &Content) -> Option<&Value> {
    match c {
        Content::Json(v) => Some(v),
        Content::Png { .. } => None,
    }
}

/// PNG bytes of a tool result, if it is an image.
fn png_of(c: &Content) -> Option<Vec<u8>> {
    match c {
        Content::Png { data, .. } => Some(data.clone()),
        Content::Json(_) => None,
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Opens `file_name` (already materialized into the sandbox) and returns its engine document id.
fn open_doc(automation: &mut Automation, file_name: &str) -> Result<i64, SdkError> {
    let open_res =
        automation.call("doc_open", &json!({ "path": file_name })).map_err(|e| SdkError::EngineError(format!("Failed to open document: {e}")))?;
    open_res
        .first()
        .and_then(json_of)
        .and_then(|v| v.get("doc"))
        .and_then(Value::as_i64)
        .ok_or_else(|| SdkError::EngineError("Invalid doc id returned".into()))
}

fn page_subset_str(subset: PageSubset) -> &'static str {
    match subset {
        PageSubset::All => "all",
        PageSubset::Even => "even",
        PageSubset::Odd => "odd",
    }
}

fn page_orientation_str(orientation: PageOrientation) -> &'static str {
    match orientation {
        PageOrientation::All => "all",
        PageOrientation::Landscape => "landscape",
        PageOrientation::Portrait => "portrait",
    }
}

/// `#RRGGBB`, or the engine's own colour name for a `Named` colour.
fn color_to_arg(color: &Color) -> String {
    let hex = |r: f64, g: f64, b: f64| {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(r), c(g), c(b))
    };
    match color {
        Color::RGB(r, g, b) => hex(*r, *g, *b),
        Color::Gray(v) => hex(*v, *v, *v),
        Color::CMYK(c, m, y, k) => hex((1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)),
        Color::Named { name, .. } => name.clone(),
    }
}

/// `[x, y, …]` for the four corners of a quad, as the engine's `quads` argument expects.
fn quad_to_array(q: &Quad) -> [f64; 8] {
    [q.top_left.x, q.top_left.y, q.top_right.x, q.top_right.y, q.bottom_left.x, q.bottom_left.y, q.bottom_right.x, q.bottom_right.y]
}

/// Sets the `find`/`all` or `quads` fields of a `comment_add` text-markup call.
fn apply_markup_target(args: &mut Value, target: &MarkupTarget) {
    match target {
        MarkupTarget::Find { text, all } => {
            args["find"] = json!(text);
            args["all"] = json!(all);
        }
        MarkupTarget::Quads(quads) => {
            args["quads"] = json!(quads.iter().map(quad_to_array).collect::<Vec<_>>());
        }
    }
}

fn parse_fit_mode(s: &str) -> Option<FitMode> {
    match s {
        "XYZ" => Some(FitMode::XYZ),
        "Fit" => Some(FitMode::Fit),
        "FitH" => Some(FitMode::FitH),
        "FitV" => Some(FitMode::FitV),
        "FitR" => Some(FitMode::FitR),
        _ => None,
    }
}

/// A view rectangle `[x0, y0, x1, y1]` as `Rect { x, y, width, height }`.
fn parse_rect_array(v: &Value) -> Option<Rect> {
    let nums: Vec<f64> = v.as_array()?.iter().filter_map(Value::as_f64).collect();
    let [x0, y0, x1, y1]: [f64; 4] = nums.try_into().ok()?;
    Some(Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()))
}

/// Bookmarks and comment replies are plain tool JSON, not raw file structure, but both nest an
/// attacker-influenced document's outline/annotation tree, so recursion here stays bounded too.
const MAX_PARSE_DEPTH: u32 = 64;

/// Charges every node, including nested children, against the whole operation's budget.
fn charge_bookmark_nodes(rows: &[Value], remaining: &mut usize) -> Result<(), SdkError> {
    let mut stack = vec![rows.iter()];
    while let Some(iter) = stack.last_mut() {
        if let Some(row) = iter.next() {
            if stack.len() > MAX_PARSE_DEPTH as usize + 1 {
                return Err(SdkError::QuotaExceeded("Bookmark depth exceeds 64".into()));
            }
            *remaining = remaining.checked_sub(1).ok_or_else(|| SdkError::QuotaExceeded("Bookmark listing exceeds 5,000 total nodes".into()))?;
            if let Some(children) = row.get("children").and_then(Value::as_array) {
                stack.push(children.iter());
            }
        } else {
            stack.pop();
        }
    }
    Ok(())
}

/// Counts serialized output without allocating another copy; all additions fail closed.
struct OutputBudget {
    remaining: u64,
}
impl OutputBudget {
    fn charge(&mut self, bytes: u64) -> Result<(), SdkError> {
        self.remaining =
            self.remaining.checked_sub(bytes).ok_or_else(|| SdkError::QuotaExceeded("Aggregate output exceeds configured byte budget".into()))?;
        Ok(())
    }
    fn json(&mut self, value: &impl serde::Serialize) -> Result<(), SdkError> {
        serde_json::to_writer(self, value).map_err(|_| SdkError::QuotaExceeded("Serialized output exceeds configured byte budget".into()))
    }
    fn files(&mut self, files: &[PathBuf]) -> Result<(), SdkError> {
        for path in files {
            self.charge(fs::metadata(path)?.len())?;
        }
        Ok(())
    }
}
impl std::io::Write for OutputBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.charge(bytes.len() as u64).map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Accepts only a complete list of existing regular files owned by this sandbox.
fn validate_split_files(payload: Option<&Value>, sandbox: &ExecutionSandbox) -> Result<Vec<PathBuf>, SdkError> {
    let invalid = || SdkError::EngineError("Invalid split output: expected non-empty files within the sandbox".into());
    let entries = payload.and_then(|v| v.get("files")).and_then(Value::as_array).filter(|v| !v.is_empty()).ok_or_else(invalid)?;
    let root = sandbox.root().canonicalize().map_err(|_| invalid())?;
    entries
        .iter()
        .map(|entry| {
            let name = entry.as_str().or_else(|| entry.get("path").and_then(Value::as_str)).ok_or_else(invalid)?;
            let path = root.join(name).canonicalize().map_err(|_| invalid())?;
            if !path.starts_with(&root) || !path.is_file() || sandbox.is_input_path(name) {
                return Err(invalid());
            }
            Ok(path)
        })
        .collect()
}

/// Parses one `bookmark_list` row (and its nested `children`, up to [`MAX_PARSE_DEPTH`]) into a
/// [`Bookmark`].
fn parse_bookmark_row(row: &Value, depth: u32) -> Bookmark {
    let title = row.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut bookmark = Bookmark::new(title);
    bookmark.is_open = row.get("open").and_then(Value::as_bool).unwrap_or(true);
    if let Some(page) = row.get("page").and_then(Value::as_u64) {
        let dest = row.get("destination");
        let fit_mode = dest.and_then(|d| d.get("fit")).and_then(Value::as_str).and_then(parse_fit_mode).unwrap_or_default();
        let zoom = dest.and_then(|d| d.get("zoom")).and_then(Value::as_f64);
        let rect = dest.and_then(|d| d.get("rect")).and_then(parse_rect_array);
        bookmark.destination = Some(ViewDestination { page_number: page as usize, fit_mode, zoom, rect });
    }
    if depth < MAX_PARSE_DEPTH
        && let Some(children) = row.get("children").and_then(Value::as_array)
    {
        bookmark.children = children.iter().map(|c| parse_bookmark_row(c, depth + 1)).collect();
    }
    bookmark
}

/// Parses one `comment_list` entry (and its flat `replies`, up to [`MAX_PARSE_DEPTH`]) into a
/// [`CommentInfo`].
fn parse_comment_info(v: &Value, depth: u32) -> CommentInfo {
    CommentInfo {
        id: v.get("id").and_then(Value::as_str).map(str::to_string),
        page: v.get("page").and_then(Value::as_u64).unwrap_or(1) as usize,
        index: v.get("index").and_then(Value::as_u64).unwrap_or(1) as usize,
        kind: v.get("type").and_then(Value::as_str).unwrap_or_default().to_string(),
        author: v.get("author").and_then(Value::as_str).map(str::to_string),
        contents: v.get("contents").and_then(Value::as_str).map(str::to_string),
        rect: v.get("rect").and_then(parse_rect_array),
        status: v.get("status").and_then(Value::as_str).map(str::to_string),
        replies: if depth < MAX_PARSE_DEPTH {
            v.get("replies").and_then(Value::as_array).map(|arr| arr.iter().map(|r| parse_comment_info(r, depth + 1)).collect()).unwrap_or_default()
        } else {
            Vec::new()
        },
    }
}

/// Strips any directory components or traversal segments from a user-supplied filename.
fn sanitize_sandbox_filename(raw: &str, fallback: &str) -> Result<String, SdkError> {
    let path = Path::new(raw);
    if path.components().count() != 1 {
        return Err(SdkError::InvalidArgument(format!("Filename must not contain path separators or traversal segments: {raw}")));
    }
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) if !name.is_empty() && name != "." && name != ".." => Ok(name.to_string()),
        _ => Ok(fallback.to_string()),
    }
}

/// Execution sandbox managing ephemeral disk isolation and cleanup.
///
/// The directory is deleted on drop unless [`ExecutionSandbox::persist`] is called, which is
/// Execution sandbox managing ephemeral disk isolation and cleanup.
///
/// The directory is deleted on drop unless [`ExecutionSandbox::persist`] is called, which is
/// required when a result (a spilled `DocumentResult::File`, split parts) must outlive the call.
pub struct ExecutionSandbox {
    temp_dir: RefCell<Option<TempDir>>,
    root: PathBuf,
    max_request_bytes: u64,
    inputs: RefCell<Vec<PathBuf>>,
    total_input_bytes: RefCell<u64>,
}

impl ExecutionSandbox {
    pub fn new(max_request_bytes: u64) -> Result<Self, SdkError> {
        let temp_dir = tempfile::Builder::new().prefix("pdfcraft-sandbox-").tempdir().map_err(SdkError::Io)?;
        let root = temp_dir.path().to_path_buf();
        Ok(Self {
            temp_dir: RefCell::new(Some(temp_dir)),
            root,
            max_request_bytes,
            inputs: RefCell::new(Vec::new()),
            total_input_bytes: RefCell::new(0),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stops the sandbox directory from being deleted on drop; the caller then owns its contents.
    pub fn persist(&self) {
        if let Ok(mut slot) = self.temp_dir.try_borrow_mut()
            && let Some(dir) = slot.take()
        {
            let _ = dir.keep();
        }
    }

    /// Registers a file as an input or transient artifact to be purged before persistence.
    pub fn record_input(&self, path: PathBuf) {
        if let Ok(mut inputs) = self.inputs.try_borrow_mut() {
            inputs.push(path);
        }
    }

    /// Deletes transient input files and sensitive credentials from the sandbox so that
    /// persisted results only contain final generated artifacts.
    /// Fails closed if removal fails for any existing file.
    pub fn purge_inputs(&self) -> Result<(), SdkError> {
        let mut inputs = self.inputs.try_borrow_mut().map_err(|_| SdkError::EngineError("Sandbox input registry is already borrowed".into()))?;
        while let Some(path) = inputs.last() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(SdkError::Io(e)),
            }
            inputs.pop();
        }
        Ok(())
    }

    /// Checks if a proposed output path matches any registered input path to prevent namespace collisions.
    pub fn is_input_path(&self, name_or_path: &str) -> bool {
        let path = self.root.join(name_or_path);
        let resolved = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match path.parent().and_then(|parent| fs::canonicalize(parent).ok()).zip(path.file_name()) {
                    Some((parent, name)) => parent.join(name),
                    None => return true,
                }
            }
            Err(_) => return true,
        };
        let Ok(inputs) = self.inputs.try_borrow() else {
            return true;
        };
        inputs.iter().any(|input| match fs::canonicalize(input) {
            Ok(input) => input == resolved,
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        })
    }

    /// Returns true if the sandbox contains any output files (excluding registered inputs).
    pub fn has_output_files(&self) -> bool {
        self.output_files().map(|files| !files.is_empty()).unwrap_or(false)
    }

    /// Enumerates regular output files recursively, rejecting symlinks and special files.
    fn output_files(&self) -> Result<Vec<PathBuf>, SdkError> {
        let mut dirs = vec![fs::canonicalize(&self.root)?];
        let mut files = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
                    return Err(SdkError::EngineError("Invalid sandbox output file type".into()));
                }
                let path = entry.path();
                if kind.is_dir() {
                    dirs.push(path);
                } else if !self.is_input_path(&path.to_string_lossy()) {
                    files.push(path);
                }
            }
        }
        Ok(files)
    }

    /// Copies an input through a stream bounded by the remaining aggregate budget.
    ///
    /// One additional byte is inspected, but never written, to distinguish an
    /// exact-budget input from an oversized or concurrently growing input.
    fn materialize_input(&self, filename: &str, mut reader: impl std::io::Read) -> Result<String, SdkError> {
        use std::io::{Read, Write};

        let filename = sanitize_sandbox_filename(filename, "input.bin")?;
        let destination = self.root.join(&filename);

        let mut total = self
            .total_input_bytes
            .try_borrow_mut()
            .map_err(|_| SdkError::EngineError("Sandbox total_input_bytes registry already borrowed".into()))?;
        let mut inputs = self.inputs.try_borrow_mut().map_err(|_| SdkError::EngineError("Sandbox input registry is already borrowed".into()))?;

        let remaining =
            self.max_request_bytes.checked_sub(*total).ok_or_else(|| SdkError::EngineError("Sandbox input accounting exceeds its budget".into()))?;

        // Never overwrite an input or an artifact already in the namespace.
        let mut writer = fs::OpenOptions::new().write(true).create_new(true).open(&destination).map_err(SdkError::Io)?;

        let copied = (|| -> Result<u64, SdkError> {
            let copied = {
                let mut bounded = (&mut reader).take(remaining);
                std::io::copy(&mut bounded, &mut writer).map_err(SdkError::Io)?
            };

            let mut extra = [0u8; 1];
            let extra_len = loop {
                match reader.read(&mut extra) {
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    result => break result.map_err(SdkError::Io)?,
                }
            };

            if extra_len != 0 {
                return Err(SdkError::QuotaExceeded(format!(
                    "Input exceeds the remaining request budget ({remaining} bytes); configured max_request_bytes is {} bytes",
                    self.max_request_bytes
                )));
            }

            writer.flush().map_err(SdkError::Io)?;
            Ok(copied)
        })();

        // Close before removing a failed write, including on Windows.
        drop(writer);

        let copied = match copied {
            Ok(copied) => copied,
            Err(error) => {
                match fs::remove_file(&destination) {
                    Ok(()) => {}
                    Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => {}
                    Err(cleanup) => {
                        inputs.push(destination);
                        return Err(SdkError::Io(std::io::Error::new(
                            cleanup.kind(),
                            format!("{error}; failed to remove partial sandbox input: {cleanup}"),
                        )));
                    }
                }
                return Err(error);
            }
        };

        *total += copied;
        inputs.push(destination);
        Ok(filename)
    }

    /// Materializes an auxiliary input under the same aggregate request budget.
    pub fn materialize_auxiliary(&self, filename: &str, bytes: &[u8]) -> Result<String, SdkError> {
        self.materialize_input(filename, bytes)
    }

    /// Materializes a source without copying beyond the remaining input budget.
    pub fn materialize_source(&self, source: &DocumentSource, index: usize) -> Result<String, SdkError> {
        match source {
            DocumentSource::Path(path) => {
                let reader = fs::File::open(path).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        SdkError::FileNotFound { path: path.display().to_string() }
                    } else {
                        SdkError::Io(error)
                    }
                })?;

                let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or("pdf");
                let filename = format!("input_{index}.{extension}");

                self.materialize_input(&filename, reader)
            }
            DocumentSource::Bytes { data, name } => {
                let default_name = format!("input_{index}.pdf");
                let filename = match name {
                    Some(name) => format!("input_{index}_{}", sanitize_sandbox_filename(name, &default_name)?),
                    None => default_name,
                };

                self.materialize_input(&filename, data.as_slice())
            }
        }
    }
}

/// Concrete engine adapter delegating to `pdfcraft_automation::Automation`.
pub struct AutomationEngineAdapter {
    max_memory_bytes: u64,
    max_request_bytes: u64,
}

impl Default for AutomationEngineAdapter {
    fn default() -> Self {
        Self {
            max_memory_bytes: 10 * 1024 * 1024,   // 10 MB threshold to spill to disk
            max_request_bytes: 100 * 1024 * 1024, // 100 MB default max input size (consistent with ResourceLimits)
        }
    }
}

impl AutomationEngineAdapter {
    pub fn new(max_memory_bytes: u64) -> Self {
        Self { max_memory_bytes, max_request_bytes: 100 * 1024 * 1024 }
    }

    pub fn with_limits(max_memory_bytes: u64, max_request_bytes: u64) -> Self {
        Self { max_memory_bytes, max_request_bytes }
    }

    fn create_sandbox(&self) -> Result<ExecutionSandbox, SdkError> {
        ExecutionSandbox::new(self.max_request_bytes)
    }

    /// Helper executing an engine closure under an `AssertUnwindSafe` panic boundary.
    fn with_automation<F, R>(&self, sandbox: &ExecutionSandbox, f: F) -> Result<R, SdkError>
    where
        F: FnOnce(&mut Automation) -> Result<R, SdkError>,
    {
        let mut automation =
            Automation::new().with_root(sandbox.root()).map_err(|e| SdkError::EngineError(format!("Failed to configure automation root: {e}")))?;

        let result = catch_unwind(AssertUnwindSafe(|| f(&mut automation)));

        match result {
            Ok(inner_res) => inner_res,
            Err(panic_err) => {
                let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_err.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Unknown engine panic".to_string()
                };
                Err(SdkError::EnginePanic(msg))
            }
        }
    }

    /// Reads an already-written sandbox output file into a `DocumentResult`, spilling to disk
    /// (persisting the sandbox) when it exceeds `max_memory_bytes`.
    fn read_sandbox_output(&self, sandbox: &ExecutionSandbox, out_name: &str) -> Result<DocumentResult, SdkError> {
        if sandbox.is_input_path(out_name) {
            return Err(SdkError::InvalidArgument(format!("Output filename '{out_name}' collides with an input file")));
        }
        let out_path = sandbox.root().join(out_name);
        let metadata = fs::metadata(&out_path).map_err(SdkError::Io)?;
        let size = metadata.len();
        if size > self.max_memory_bytes {
            sandbox.purge_inputs()?;
            sandbox.persist();
            Ok(DocumentResult::file(out_path, "application/pdf", size))
        } else {
            let bytes = fs::read(&out_path).map_err(SdkError::Io)?;
            Ok(DocumentResult::memory(bytes, "application/pdf"))
        }
    }

    /// Saves `doc_id` to `out_name` within the sandbox and reads the result back.
    fn save_and_read(
        &self,
        automation: &mut Automation,
        sandbox: &ExecutionSandbox,
        doc_id: i64,
        out_name: &str,
    ) -> Result<DocumentResult, SdkError> {
        if sandbox.is_input_path(out_name) {
            return Err(SdkError::InvalidArgument(format!("Output filename '{out_name}' collides with an input file")));
        }
        automation.call("doc_save", &json!({ "doc": doc_id, "path": out_name })).map_err(|e| SdkError::EngineError(format!("Save failed: {e}")))?;
        self.read_sandbox_output(sandbox, out_name)
    }

    /// The sandbox-relative output filename for a new operation result: the caller's choice,
    /// sanitized, or a counter-suffixed default.
    fn out_name(custom: &Option<String>, prefix: &str) -> Result<String, SdkError> {
        let default_out = format!("{prefix}_{}.pdf", COUNTER.fetch_add(1, Ordering::Relaxed));
        match custom {
            Some(name) => sanitize_sandbox_filename(name, &default_out),
            None => Ok(default_out),
        }
    }
}

impl OperationExecutor for AutomationEngineAdapter {
    fn with_limits(&self, limits: &crate::types::ResourceLimits) -> Option<std::sync::Arc<dyn OperationExecutor>> {
        Some(std::sync::Arc::new(AutomationEngineAdapter::with_limits(limits.max_memory_bytes, limits.max_request_bytes as u64)))
    }

    fn execute_merge(&self, sources: &[DocumentSource], options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        if sources.is_empty() {
            return Err(SdkError::InvalidArgument("No input documents provided for merge".into()));
        }

        let mut total_len = 0u64;
        for src in sources {
            if let Some(len) = src.byte_len() {
                total_len = total_len.saturating_add(len as u64);
                if total_len > self.max_request_bytes {
                    return Err(SdkError::QuotaExceeded(format!(
                        "Total input size ({total_len} bytes) exceeds configured max_request_bytes ({} bytes)",
                        self.max_request_bytes
                    )));
                }
            }
        }

        let sandbox = self.create_sandbox()?;
        let mut materialized_paths = Vec::with_capacity(sources.len());
        for (i, src) in sources.iter().enumerate() {
            materialized_paths.push(sandbox.materialize_source(src, i)?);
        }

        let default_out = format!("merged_{}.pdf", COUNTER.fetch_add(1, Ordering::Relaxed));
        let out_name = match &options.output_filename {
            Some(custom) => {
                let sanitized = sanitize_sandbox_filename(custom, &default_out)?;
                if sandbox.is_input_path(&sanitized) {
                    return Err(SdkError::InvalidArgument(format!("Output filename '{sanitized}' conflicts with an input artifact")));
                }
                sanitized
            }
            None => default_out,
        };

        self.with_automation(&sandbox, |automation| {
            let args = json!({
                "paths": materialized_paths,
                "pages": options.pages,
                "passwords": options.passwords,
                "out": out_name,
            });

            automation.call("doc_combine", &args).map_err(|e| SdkError::EngineError(format!("Merge failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        })
    }

    fn execute_split(&self, source: &DocumentSource, mode: &SplitMode) -> Result<SplitResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            let open_res = automation
                .call("doc_open", &json!({ "path": file_name }))
                .map_err(|e| SdkError::EngineError(format!("Failed to open document: {e}")))?;

            let doc_id = open_res
                .first()
                .and_then(json_of)
                .and_then(|v| v.get("doc"))
                .and_then(Value::as_i64)
                .ok_or_else(|| SdkError::EngineError("Invalid doc id returned".into()))?;

            let out_dir = "parts";
            let mut split_args = json!({
                "doc": doc_id,
                "out_dir": out_dir,
            });

            match mode {
                SplitMode::EveryNPages(every) => split_args["every"] = json!(every),
                SplitMode::BeforePages(pages) => split_args["before"] = json!(pages),
                SplitMode::AtBookmarks => split_args["bookmarks"] = json!(true),
                SplitMode::MaxFileSizeMb(mb) => split_args["max_mb"] = json!(mb),
            }

            let res = automation.call("doc_split", &split_args).map_err(|e| SdkError::EngineError(format!("Split failed: {e}")))?;

            let raw_files = validate_split_files(res.first().and_then(json_of), &sandbox)?;
            sandbox.purge_inputs()?;
            sandbox.persist(); // split parts must outlive this call

            Ok(SplitResult { files: raw_files })
        })
    }

    fn execute_render(&self, source: &DocumentSource, page: usize, options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }

        // The engine's `page_render` only produces PNG; never label PNG bytes as another format.
        if options.format != ImageFormat::Png {
            return Err(SdkError::UnsupportedOperation(format!("render format {:?} (the engine renders PNG only)", options.format)));
        }

        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            // Strictly compute DPI without coupling to monitor display PPI (#739)
            let open_res = automation
                .call("doc_open", &json!({ "path": file_name }))
                .map_err(|e| SdkError::EngineError(format!("Failed to open document: {e}")))?;
            let doc_id = open_res
                .first()
                .and_then(json_of)
                .and_then(|v| v.get("doc"))
                .and_then(Value::as_i64)
                .ok_or_else(|| SdkError::EngineError("Invalid doc id returned".into()))?;

            let args = json!({
                "doc": doc_id,
                "page": page,
                "dpi": options.dpi,
            });

            let res = automation.call("page_render", &args).map_err(|e| SdkError::EngineError(format!("Render failed: {e}")))?;

            let img_bytes = res.first().and_then(png_of).ok_or_else(|| SdkError::EngineError("Render produced no image bytes".into()))?;

            if img_bytes.len() as u64 > self.max_memory_bytes {
                let out_name = format!("render_{page}_{}.png", COUNTER.fetch_add(1, Ordering::Relaxed));
                let out_path = sandbox.root().join(&out_name);
                fs::write(&out_path, &img_bytes).map_err(SdkError::Io)?;
                let len = img_bytes.len() as u64;
                sandbox.purge_inputs()?;
                sandbox.persist();
                Ok(DocumentResult::file(out_path, "image/png", len))
            } else {
                Ok(DocumentResult::memory(img_bytes, "image/png"))
            }
        })
    }

    fn execute_call_tool(&self, name: &str, args: &Value) -> Result<Vec<crate::ports::ToolContent>, SdkError> {
        if name.trim().is_empty() {
            return Err(SdkError::InvalidArgument("Tool name must not be empty".into()));
        }

        let sandbox = self.create_sandbox()?;

        self.with_automation(&sandbox, |automation| {
            let result = automation.call(name, args).map_err(|e| match e {
                ToolError::UnknownTool(t) => SdkError::UnsupportedOperation(format!("Unknown tool: {t}")),
                ToolError::InvalidArgs(m) => SdkError::InvalidArgument(m),
                ToolError::Failed(m) => SdkError::EngineError(m),
            })?;

            let result: Vec<crate::ports::ToolContent> = result.into_iter().map(Into::into).collect();
            let files = sandbox.output_files()?;
            let mut budget = OutputBudget { remaining: self.max_request_bytes };
            budget.json(&result)?;
            budget.files(&files)?;
            if !files.is_empty() {
                sandbox.purge_inputs()?;
                sandbox.persist();
            }
            Ok(result)
        })
    }

    fn execute_rotate_pages(&self, source: &DocumentSource, options: &RotatePagesOptions) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&options.output_filename, "rotated")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "degrees": options.degrees,
                "subset": page_subset_str(options.subset),
                "orientation": page_orientation_str(options.orientation),
            });
            if let Some(pages) = &options.pages {
                args["pages"] = json!(pages);
            }
            automation.call("page_rotate", &args).map_err(|e| SdkError::EngineError(format!("Rotate failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_extract_pages(&self, source: &DocumentSource, options: &ExtractPagesOptions) -> Result<DocumentResult, SdkError> {
        if options.pages.is_empty() {
            return Err(SdkError::InvalidArgument("No pages specified for extraction".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&options.output_filename, "extracted")?;
        if sandbox.is_input_path(&out_name) {
            return Err(SdkError::InvalidArgument(format!("Output filename '{out_name}' collides with an input file")));
        }

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let args = json!({ "doc": doc_id, "pages": options.pages, "out": out_name, "delete": options.delete_after });
            automation.call("page_extract", &args).map_err(|e| SdkError::EngineError(format!("Extract failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        })
    }

    fn execute_delete_pages(&self, source: &DocumentSource, options: &DeletePagesOptions) -> Result<DocumentResult, SdkError> {
        if options.pages.is_empty() {
            return Err(SdkError::InvalidArgument("No pages specified for deletion".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&options.output_filename, "deleted")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            automation
                .call("page_delete", &json!({ "doc": doc_id, "pages": options.pages }))
                .map_err(|e| SdkError::EngineError(format!("Delete failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_insert_pages(
        &self,
        source: &DocumentSource,
        insert_source: &DocumentSource,
        options: &InsertPagesOptions,
    ) -> Result<DocumentResult, SdkError> {
        if options.at == 0 {
            return Err(SdkError::InvalidArgument("Insert position must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let insert_file_name = sandbox.materialize_source(insert_source, 1)?;
        let out_name = Self::out_name(&options.output_filename, "inserted")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({ "doc": doc_id, "path": insert_file_name, "at": options.at });
            if let Some(pages) = &options.pages {
                args["pages"] = json!(pages);
            }
            automation.call("page_insert_file", &args).map_err(|e| SdkError::EngineError(format!("Insert failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_list_bookmarks(&self, source: &DocumentSource) -> Result<Vec<Bookmark>, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut all_bookmarks = Vec::new();
            let mut remaining_nodes = 5_000;
            let mut offset: usize = 0;
            const PAGE_LIMIT: usize = 500;
            loop {
                let res = automation
                    .call("bookmark_list", &json!({ "doc": doc_id, "offset": offset, "limit": PAGE_LIMIT }))
                    .map_err(|e| SdkError::EngineError(format!("List bookmarks failed: {e}")))?;
                let obj =
                    res.first().and_then(json_of).ok_or_else(|| SdkError::EngineError("Invalid bookmark_list response: empty payload".into()))?;
                if obj.get("truncated").and_then(Value::as_bool) == Some(true) {
                    return Err(SdkError::QuotaExceeded("Engine bookmark traversal exceeded its limits".into()));
                }
                let rows = obj
                    .get("bookmarks")
                    .and_then(Value::as_array)
                    .ok_or_else(|| SdkError::EngineError("Malformed engine response: missing 'bookmarks' array".into()))?;
                charge_bookmark_nodes(rows, &mut remaining_nodes)?;
                for r in rows {
                    all_bookmarks.push(parse_bookmark_row(r, 0));
                }
                match obj.get("next") {
                    None | Some(Value::Null) => break,
                    Some(value) => {
                        let next_offset = value
                            .as_u64()
                            .ok_or_else(|| SdkError::EngineError("Invalid bookmark continuation: expected an unsigned integer".into()))?;
                        let next = usize::try_from(next_offset)
                            .map_err(|_| SdkError::EngineError("Bookmark continuation exceeds the platform index range".into()))?;

                        if next <= offset {
                            return Err(SdkError::EngineError("Bookmark continuation did not advance".into()));
                        }

                        offset = next;
                    }
                }
            }
            Ok(all_bookmarks)
        })
    }

    fn execute_add_bookmark(&self, source: &DocumentSource, request: &BookmarkAddRequest) -> Result<DocumentResult, SdkError> {
        if request.page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&request.output_filename, "bookmarked")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({ "doc": doc_id, "title": request.title, "page": request.page });
            if let Some(parent) = &request.parent {
                args["parent"] = json!(parent);
            }
            if let Some(position) = request.position {
                args["position"] = json!(position);
            }
            automation.call("bookmark_add", &args).map_err(|e| SdkError::EngineError(format!("Add bookmark failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_remove_bookmark(&self, source: &DocumentSource, request: &BookmarkRemoveRequest) -> Result<DocumentResult, SdkError> {
        if request.path.is_empty() {
            return Err(SdkError::InvalidArgument("Bookmark path must not be empty".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&request.output_filename, "unbookmarked")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            automation
                .call("bookmark_delete", &json!({ "doc": doc_id, "path": request.path }))
                .map_err(|e| SdkError::EngineError(format!("Remove bookmark failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_add_highlight(&self, source: &DocumentSource, request: &HighlightRequest) -> Result<DocumentResult, SdkError> {
        if request.page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "highlighted")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({ "doc": doc_id, "page": request.page, "type": "highlight" });
            apply_markup_target(&mut args, &request.target);
            if let Some(color) = &request.color {
                args["color"] = json!(color_to_arg(color));
            }
            if let Some(author) = &request.author {
                args["author"] = json!(author);
            }
            if let Some(opacity) = request.opacity {
                args["opacity"] = json!(opacity);
            }
            automation.call("comment_add", &args).map_err(|e| SdkError::EngineError(format!("Highlight failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_add_underline(&self, source: &DocumentSource, request: &UnderlineRequest) -> Result<DocumentResult, SdkError> {
        if request.page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "underlined")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({ "doc": doc_id, "page": request.page, "type": "underline" });
            apply_markup_target(&mut args, &request.target);
            if let Some(color) = &request.color {
                args["color"] = json!(color_to_arg(color));
            }
            if let Some(author) = &request.author {
                args["author"] = json!(author);
            }
            if let Some(opacity) = request.opacity {
                args["opacity"] = json!(opacity);
            }
            automation.call("comment_add", &args).map_err(|e| SdkError::EngineError(format!("Underline failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_add_stamp(&self, source: &DocumentSource, request: &StampRequest) -> Result<DocumentResult, SdkError> {
        if request.page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "stamped")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "page": request.page,
                "type": "stamp",
                "at": [request.at.x, request.at.y],
                "stamp": request.stamp.as_str(),
                "dynamic": request.dynamic,
            });
            if let Some(author) = &request.author {
                args["author"] = json!(author);
            }
            automation.call("comment_add", &args).map_err(|e| SdkError::EngineError(format!("Stamp failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_add_comment(&self, source: &DocumentSource, request: &CommentRequest) -> Result<DocumentResult, SdkError> {
        if request.page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "commented")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "page": request.page,
                "type": "note",
                "at": [request.at.x, request.at.y],
                "contents": request.contents,
            });
            if let Some(author) = &request.author {
                args["author"] = json!(author);
            }
            if let Some(color) = &request.color {
                args["color"] = json!(color_to_arg(color));
            }
            automation.call("comment_add", &args).map_err(|e| SdkError::EngineError(format!("Add comment failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_list_comments(&self, source: &DocumentSource, page: Option<usize>) -> Result<Vec<CommentInfo>, SdkError> {
        if page == Some(0) {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({ "doc": doc_id });
            if let Some(p) = page {
                args["page"] = json!(p);
            }
            let res = automation.call("comment_list", &args).map_err(|e| SdkError::EngineError(format!("List comments failed: {e}")))?;
            let obj = res.first().and_then(json_of).ok_or_else(|| SdkError::EngineError("Invalid comment_list response: empty payload".into()))?;
            let comments = obj
                .get("comments")
                .and_then(Value::as_array)
                .ok_or_else(|| SdkError::EngineError("Malformed engine response: missing 'comments' array".into()))?;
            Ok(comments.iter().map(|c| parse_comment_info(c, 0)).collect())
        })
    }

    fn execute_get_form_fields(&self, source: &DocumentSource) -> Result<Vec<crate::forms::FormField>, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let res =
                automation.call("form_fields", &json!({ "doc": doc_id })).map_err(|e| SdkError::EngineError(format!("form_fields failed: {e}")))?;
            let obj = res.first().and_then(json_of).ok_or_else(|| SdkError::EngineError("Invalid form_fields response: empty payload".into()))?;
            let fields_json = obj
                .get("fields")
                .and_then(Value::as_array)
                .ok_or_else(|| SdkError::EngineError("Malformed engine response: missing 'fields' array".into()))?;

            let mut fields = Vec::with_capacity(fields_json.len());
            for f in fields_json {
                let name = f.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let field_type_str = f.get("type").and_then(Value::as_str).unwrap_or("text");
                let field_type = match field_type_str {
                    "button" | "checkbox" | "radio" => crate::forms::FieldType::Button(crate::forms::ButtonField::default()),
                    "choice" | "combobox" | "listbox" => crate::forms::FieldType::Choice(crate::forms::ChoiceField::default()),
                    "signature" => crate::forms::FieldType::Signature(crate::forms::SignatureField::default()),
                    _ => crate::forms::FieldType::Text(crate::forms::TextField::default()),
                };
                let value = f.get("value").and_then(Value::as_str).map(ToString::to_string);
                let page_number = f.get("page").and_then(Value::as_u64).map(|p| p as usize).unwrap_or(1);
                let rect = f.get("rect").and_then(parse_rect_array).unwrap_or_default();
                let read_only = f.get("read_only").and_then(Value::as_bool).unwrap_or(false);
                let required = f.get("required").and_then(Value::as_bool).unwrap_or(false);

                fields.push(crate::forms::Field { name: name.clone(), full_name: name, field_type, value, page_number, rect, read_only, required });
            }
            Ok(fields)
        })
    }

    fn execute_fill_form(&self, source: &DocumentSource, data: &serde_json::Value) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "form_filled")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let args = json!({
                "doc": doc_id,
                "values": data,
            });
            automation.call("form_fill", &args).map_err(|e| SdkError::EngineError(format!("form_fill failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_flatten_form(&self, source: &DocumentSource) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "form_flattened")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let args = json!({
                "doc": doc_id,
                "fields": true,
                "comments": false,
            });
            automation.call("doc_flatten", &args).map_err(|e| SdkError::EngineError(format!("doc_flatten failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_export_form_data(&self, source: &DocumentSource, format: &str) -> Result<Vec<u8>, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let export_filename = format!("export.{format}");

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let args = json!({
                "doc": doc_id,
                "path": export_filename,
                "what": "fields",
            });
            automation.call("doc_export_data", &args).map_err(|e| SdkError::EngineError(format!("doc_export_data failed: {e}")))?;
            let export_path = sandbox.root().join(&export_filename);
            fs::read(&export_path).map_err(SdkError::Io)
        })
    }

    fn execute_import_form_data(&self, source: &DocumentSource, data_bytes: &[u8], format: &str) -> Result<DocumentResult, SdkError> {
        if data_bytes.len() as u64 > self.max_request_bytes {
            return Err(SdkError::QuotaExceeded(format!(
                "Form data size ({} bytes) exceeds configured max_request_bytes ({} bytes)",
                data_bytes.len(),
                self.max_request_bytes
            )));
        }
        let valid_format = match format.to_ascii_lowercase().as_str() {
            "fdf" => "fdf",
            "xfdf" => "xfdf",
            "xml" => "xml",
            "json" => "json",
            other => return Err(SdkError::InvalidArgument(format!("Unsupported form data format: {other}"))),
        };
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let import_filename = sandbox.materialize_auxiliary(&format!("data.{valid_format}"), data_bytes)?;

        let out_name = Self::out_name(&None, "form_imported")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let args = json!({
                "doc": doc_id,
                "path": import_filename,
            });
            automation.call("doc_import_data", &args).map_err(|e| SdkError::EngineError(format!("doc_import_data failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_optimize(&self, source: &DocumentSource, params: &crate::optimization::OptimizationParams) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "optimized")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "out": out_name,
                "streams": params.compress_streams,
                "cleanup": params.remove_unreferenced_objects,
                "linearize": params.linearize,
            });
            if let Some(dpi) = params.target_image_dpi {
                args["dpi"] = json!(dpi);
            }
            automation.call("doc_optimize", &args).map_err(|e| SdkError::EngineError(format!("doc_optimize failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        })
    }

    fn execute_reduce_size(&self, source: &DocumentSource, max_mb: Option<f64>) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "reduced")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "out": out_name,
            });
            if let Some(mb) = max_mb {
                args["max_mb"] = json!(mb);
            }
            automation.call("doc_reduce", &args).map_err(|e| SdkError::EngineError(format!("doc_reduce failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        })
    }

    fn execute_encrypt(
        &self,
        source: &DocumentSource,
        user_pw: Option<&str>,
        owner_pw: Option<&str>,
        perms: Option<u32>,
    ) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "encrypted")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "out": out_name,
            });
            if let Some(u) = user_pw {
                args["user_password"] = json!(u);
            }
            if let Some(o) = owner_pw {
                args["owner_password"] = json!(o);
            }
            if let Some(p) = perms {
                args["permissions"] = json!(p);
            }
            automation.call("doc_encrypt", &args).map_err(|e| SdkError::EngineError(format!("doc_encrypt failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        })
    }

    fn execute_decrypt(&self, source: &DocumentSource, password: &str) -> Result<DocumentResult, SdkError> {
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "decrypted")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let args = json!({
                "doc": doc_id,
                "password": password,
                "out": out_name,
            });
            automation.call("doc_decrypt", &args).map_err(|e| SdkError::EngineError(format!("doc_decrypt failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        })
    }

    fn execute_redact(&self, source: &DocumentSource, page: usize, rect: &crate::graphics::Rect) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based".into()));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let out_name = Self::out_name(&None, "redacted")?;

        self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mark_args = json!({
                "doc": doc_id,
                "page": page,
                "rect": rect.to_array(),
            });
            automation.call("redact_mark", &mark_args).map_err(|e| SdkError::EngineError(format!("redact_mark failed: {e}")))?;
            automation
                .call("redact_apply", &json!({ "doc": doc_id, "pages": [page] }))
                .map_err(|e| SdkError::EngineError(format!("redact_apply failed: {e}")))?;
            self.save_and_read(automation, &sandbox, doc_id, &out_name)
        })
    }

    fn execute_sign(&self, source: &DocumentSource, sign_doc: &crate::security::SignDoc) -> Result<DocumentResult, SdkError> {
        if sign_doc.cert_p12_bytes.len() as u64 > self.max_request_bytes {
            return Err(SdkError::QuotaExceeded(format!(
                "Signing certificate size ({} bytes) exceeds configured max_request_bytes ({} bytes)",
                sign_doc.cert_p12_bytes.len(),
                self.max_request_bytes
            )));
        }
        let sandbox = self.create_sandbox()?;
        let file_name = sandbox.materialize_source(source, 0)?;
        let cert_filename =
            sandbox.materialize_auxiliary(&format!("signing_cert_{}.p12", COUNTER.fetch_add(1, Ordering::Relaxed)), &sign_doc.cert_p12_bytes)?;
        let cert_path = sandbox.root().join(&cert_filename);

        let out_name = Self::out_name(&None, "signed")?;

        let res = self.with_automation(&sandbox, |automation| {
            let doc_id = open_doc(automation, &file_name)?;
            let mut args = json!({
                "doc": doc_id,
                "cert": cert_filename,
                "out": out_name,
            });
            if let Some(pw) = &sign_doc.password {
                args["password"] = json!(pw);
            }
            if let Some(reason) = &sign_doc.reason {
                args["reason"] = json!(reason);
            }
            let sign_res = automation.call("sign_document", &args);
            // Fail-closed security: purge sensitive signing certificate from disk immediately before reading output/persisting!
            if cert_path.exists() {
                fs::remove_file(&cert_path).map_err(SdkError::Io)?;
            }
            sign_res.map_err(|e| SdkError::EngineError(format!("sign_document failed: {e}")))?;
            self.read_sandbox_output(&sandbox, &out_name)
        });

        // Ensure cert is deleted in all failure paths
        if cert_path.exists() {
            fs::remove_file(&cert_path).map_err(SdkError::Io)?;
        }
        res
    }

    fn execute_batch(&self, steps: &[crate::ports::BatchStep]) -> Result<crate::ports::BatchReport, SdkError> {
        if steps.is_empty() {
            return Err(SdkError::InvalidArgument("Batch steps list cannot be empty".into()));
        }

        let sandbox = self.create_sandbox()?;

        self.with_automation(&sandbox, |automation| {
            let mut report = Vec::with_capacity(steps.len());
            let mut failure = false;
            let mut total_output_bytes: u64 = 0;

            for (i, step) in steps.iter().enumerate() {
                let n = i + 1;
                if failure {
                    report.push(crate::ports::BatchStepResult {
                        step: n,
                        tool: step.tool.clone(),
                        status: "skipped".into(),
                        result: None,
                        error: None,
                    });
                    continue;
                }

                match automation.call(&step.tool, &step.args) {
                    Ok(content) => {
                        let mut step_bytes: u64 = 0;
                        let items: Vec<Value> = content
                            .into_iter()
                            .map(|c| match c {
                                Content::Json(v) => {
                                    step_bytes = step_bytes.saturating_add(v.to_string().len() as u64);
                                    v
                                }
                                Content::Png { data, width, height } => {
                                    step_bytes = step_bytes.saturating_add(data.len() as u64);
                                    json!({
                                        "type": "png",
                                        "width": width,
                                        "height": height,
                                        "data_base64": crate::types::encode_base64(&data),
                                    })
                                }
                            })
                            .collect();
                        total_output_bytes = total_output_bytes.saturating_add(step_bytes);
                        if total_output_bytes > self.max_request_bytes {
                            return Err(SdkError::QuotaExceeded(format!(
                                "Batch output size ({total_output_bytes} bytes) exceeds configured output budget ({} bytes)",
                                self.max_request_bytes
                            )));
                        }
                        let json_res = match items.len() {
                            0 => Value::Null,
                            1 => items.into_iter().next().unwrap_or(Value::Null),
                            _ => Value::Array(items),
                        };
                        report.push(crate::ports::BatchStepResult {
                            step: n,
                            tool: step.tool.clone(),
                            status: "ok".into(),
                            result: Some(json_res),
                            error: None,
                        });
                    }
                    Err(e) => {
                        failure = true;
                        report.push(crate::ports::BatchStepResult {
                            step: n,
                            tool: step.tool.clone(),
                            status: "error".into(),
                            result: None,
                            error: Some(e.to_string()),
                        });
                    }
                }
            }

            let output_files = if failure { Vec::new() } else { sandbox.output_files()? };
            let mut budget = OutputBudget { remaining: self.max_request_bytes };
            budget.files(&output_files)?;
            let outputs = if output_files.is_empty() { None } else { Some(output_files) };
            let report = crate::ports::BatchReport { ok: !failure, steps: report, output_files: outputs };
            budget.json(&report)?;
            if report.output_files.is_some() {
                sandbox.purge_inputs()?;
                sandbox.persist();
            }
            Ok(report)
        })
    }
}

impl From<pdfcraft_automation::Content> for crate::ports::ToolContent {
    fn from(c: pdfcraft_automation::Content) -> Self {
        match c {
            pdfcraft_automation::Content::Json(v) => Self::Json(v),
            pdfcraft_automation::Content::Png { data, width, height } => Self::Png { data, width, height },
        }
    }
}

#[cfg(test)]
mod revision12_tests {
    use super::*;

    #[test]
    fn architecture_bookmark_depth_is_fail_closed() {
        let mut row = json!({"title": "leaf"});
        for _ in 0..MAX_PARSE_DEPTH {
            row = json!({"children": [row]});
        }
        charge_bookmark_nodes(&[row.clone()], &mut 5_000).unwrap();
        row = json!({"children": [row]});
        assert!(matches!(charge_bookmark_nodes(&[row], &mut 5_000), Err(SdkError::QuotaExceeded(_))));
    }

    #[test]
    fn architecture_canonical_input_alias_is_rejected() {
        let sandbox = ExecutionSandbox::new(1024).unwrap();
        sandbox.materialize_auxiliary("input.pdf", b"pdf").unwrap();
        fs::create_dir(sandbox.root().join("sub")).unwrap();
        let alias = "sub/../input.pdf";
        assert!(validate_split_files(Some(&json!({"files": [alias]})), &sandbox).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(sandbox.root().join("input.pdf"), sandbox.root().join("alias.pdf")).unwrap();
            assert!(validate_split_files(Some(&json!({"files": ["alias.pdf"]})), &sandbox).is_err());
        }
    }

    #[test]
    fn architecture_generic_and_batch_output_limits() {
        let adapter = AutomationEngineAdapter::with_limits(1024, 1);
        assert!(matches!(adapter.execute_call_tool("doc_list", &json!({})), Err(SdkError::QuotaExceeded(_))));
        let steps = [crate::ports::BatchStep { tool: "unknown".into(), args: json!({}) }];
        assert!(matches!(adapter.execute_batch(&steps), Err(SdkError::QuotaExceeded(_))));
    }

    #[test]
    fn architecture_aggregate_output_boundary() {
        let adapter = AutomationEngineAdapter::default();
        let steps = vec![crate::ports::BatchStep { tool: "doc_list".into(), args: json!({}) }; 2];
        let report = adapter.execute_batch(&steps).unwrap();
        let size = serde_json::to_vec(&report).unwrap().len() as u64;
        assert!(AutomationEngineAdapter::with_limits(1024, size).execute_batch(&steps).is_ok());
        assert!(matches!(AutomationEngineAdapter::with_limits(1024, size - 1).execute_batch(&steps), Err(SdkError::QuotaExceeded(_))));
        let sandbox = ExecutionSandbox::new(1024).unwrap();
        fs::create_dir(sandbox.root().join("nested")).unwrap();
        fs::write(sandbox.root().join("nested/out.bin"), [0; 10]).unwrap();
        let files = sandbox.output_files().unwrap();
        let mut budget = OutputBudget { remaining: 12 };
        budget.json(&json!([])).unwrap();
        budget.files(&files).unwrap();
        assert!(matches!(budget.charge(1), Err(SdkError::QuotaExceeded(_))));
    }

    #[test]
    fn bookmark_budget_counts_children_across_pages() {
        let page = vec![json!({"children": vec![json!({"title": "child"}); 499]})];
        let mut remaining = 5_000;
        for _ in 0..10 {
            charge_bookmark_nodes(&page, &mut remaining).unwrap();
        }
        assert_eq!(remaining, 0);
        assert!(matches!(charge_bookmark_nodes(&[json!({})], &mut remaining), Err(SdkError::QuotaExceeded(_))));
        let mut remaining = 5_000;
        assert!(matches!(charge_bookmark_nodes(&vec![json!({}); 5001], &mut remaining), Err(SdkError::QuotaExceeded(_))));
    }

    #[test]
    fn split_outputs_must_all_be_owned_regular_files() {
        let sandbox = ExecutionSandbox::new(1024).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(sandbox.root().join("good.pdf"), b"pdf").unwrap();
        for payload in [
            json!({}),
            json!({"files": []}),
            json!({"files": ["good.pdf", 42]}),
            json!({"files": ["missing.pdf"]}),
            json!({"files": ["."]}),
            json!({"files": [outside.path()]}),
        ] {
            assert!(matches!(validate_split_files(Some(&payload), &sandbox), Err(SdkError::EngineError(_))));
        }
        assert!(validate_split_files(None, &sandbox).is_err());
        let valid = json!({"files": ["good.pdf", {"path": "good.pdf"}]});
        assert_eq!(validate_split_files(Some(&valid), &sandbox).unwrap().len(), 2);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), sandbox.root().join("escape.pdf")).unwrap();
            assert!(validate_split_files(Some(&json!({"files": ["escape.pdf"]})), &sandbox).is_err());
        }
    }

    #[test]
    fn local_sdk_still_materializes_path_sources() {
        let input = tempfile::NamedTempFile::new().unwrap();
        fs::write(input.path(), b"local document").unwrap();
        let sandbox = ExecutionSandbox::new(1024).unwrap();
        let name = sandbox.materialize_source(&DocumentSource::Path(input.path().into()), 0).unwrap();
        assert_eq!(fs::read(sandbox.root().join(name)).unwrap(), b"local document");
    }
}
