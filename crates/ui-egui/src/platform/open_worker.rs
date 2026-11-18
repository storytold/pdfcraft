//! Native document reads and preparation run on the host's job queue, away from the frame thread.

use std::path::PathBuf;
use std::sync::Arc;

use craft::host::queue::{Cancel, JobQueue, QueueOptions, QueueOrder};
use craft::host::{Host, Wake};
use pdfcraft_engine::{OpenContext, PreparedDocument, SourceKind};
use pdfcraft_render::OpenError;

/// Where an open reads its bytes from.
pub(crate) enum Input {
    Path(PathBuf),
    RemotePath(PathBuf),
    /// The recovery store's copy, at the path its entry names.
    Recovery(PathBuf),
    Bytes(Arc<Vec<u8>>),
}

pub(crate) enum ResultKind {
    Conversion(Arc<Vec<u8>>),
    /// An Office document LibreOffice converted here, off the frame thread, or why it couldn't.
    Converted(Result<Arc<Vec<u8>>, String>),
    Prepared {
        bytes: Arc<Vec<u8>>,
        result: Box<Result<PreparedDocument, OpenError>>,
    },
    ReadFailed(String),
    RemoteFailed(String),
    RecoveryFailed(String),
    Failed(String),
}

/// One open to prepare; its key on the queue is the open's token.
pub(crate) struct Job {
    pub context: OpenContext,
    pub name: String,
    pub path: Option<String>,
    pub password: Option<zeroize::Zeroizing<String>>,
    pub input: Input,
}

/// The queue that prepares opens: one lane, oldest first, so files open in the order they were
/// asked for.
pub(crate) fn queue(host: &Host, wake: Wake) -> JobQueue<u64, ResultKind> {
    JobQueue::new(host.jobs_shared().clone(), wake, QueueOptions { lanes: 1, name: "pdfcraft-open", order: QueueOrder::OldestFirst })
}

/// Prepares `job`; a panic inside the engine becomes a failed open, since the queue delivers
/// nothing for a panicked job.
pub(crate) fn run(job: Job, cancel: &Cancel) -> ResultKind {
    pdfcraft_engine::guard(|| prepare(job, cancel)).unwrap_or_else(|_| ResultKind::Failed("opening failed unexpectedly in the background".into()))
}

fn prepare(job: Job, cancel: &Cancel) -> ResultKind {
    let bytes = match &job.input {
        Input::Path(path) => match std::fs::read(path) {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => return ResultKind::ReadFailed(error.to_string()),
        },
        Input::RemotePath(path) => match pdfcraft_engine::read_remote_document(path) {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => return ResultKind::RemoteFailed(error.to_string()),
        },
        Input::Recovery(path) => match std::fs::read(path) {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => return ResultKind::RecoveryFailed(error.to_string()),
        },
        Input::Bytes(bytes) => bytes.clone(),
    };
    if cancel.is_cancelled() {
        return ResultKind::Failed("the open was cancelled".into());
    }
    match pdfcraft_engine::source_kind(&job.name, &bytes) {
        Some(SourceKind::Image | SourceKind::Text) => return ResultKind::Conversion(bytes),
        // LibreOffice takes seconds: it runs here, and the frame thread only opens the result.
        Some(SourceKind::Office) => return ResultKind::Converted(pdfcraft_engine::office_to_pdf(&job.name, &bytes).map_err(|e| e.to_string())),
        Some(SourceKind::Pdf) | None => {}
    }
    let result = job.context.prepare_open(job.name, job.path, bytes.clone(), job.password.as_deref().map(String::as_str));
    ResultKind::Prepared { bytes, result: Box::new(result) }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
