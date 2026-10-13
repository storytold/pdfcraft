//! Error types and panic containment for the UniFFI bridge.

use std::panic::UnwindSafe;

/// Flat, user-friendly error enum surfaced across the FFI boundary to Swift and Kotlin.
#[derive(Debug, thiserror::Error, uniffi::Error, PartialEq, Eq)]
pub enum FfiError {
    #[error("the file is not a readable PDF: {reason}")]
    InvalidPdf { reason: String },

    #[error("the document is protected by a password")]
    NeedsPassword,

    #[error("the password is incorrect")]
    WrongPassword,

    #[error("unsupported PDF feature: {reason}")]
    Unsupported { reason: String },

    #[error("document not found: {id}")]
    NotFound { id: String },

    #[error("the document cannot be edited: {reason}")]
    ReadOnly { reason: String },

    #[error("operation not permitted by document permissions: {operation}")]
    NotPermitted { operation: String },

    #[error("page index out of bounds: {page} (total pages: {total})")]
    PageOutOfBounds { page: u32, total: u32 },

    #[error("tool execution failed: {reason}")]
    ToolFailed { reason: String },

    #[error("unknown tool: {name}")]
    UnknownTool { name: String },

    #[error("invalid tool arguments: {reason}")]
    InvalidArgs { reason: String },

    #[error("edit error: {reason}")]
    EditFailed { reason: String },

    #[error("io error: {reason}")]
    Io { reason: String },

    #[error("internal error: {reason}")]
    Internal { reason: String },
}

impl From<pdfcraft_render::OpenError> for FfiError {
    fn from(e: pdfcraft_render::OpenError) -> Self {
        match e {
            pdfcraft_render::OpenError::Invalid(reason) => FfiError::InvalidPdf { reason },
            pdfcraft_render::OpenError::NeedsPassword => FfiError::NeedsPassword,
            pdfcraft_render::OpenError::WrongPassword => FfiError::WrongPassword,
            pdfcraft_render::OpenError::Unsupported(reason) => FfiError::Unsupported { reason },
        }
    }
}

impl From<pdfcraft_automation::ToolError> for FfiError {
    fn from(e: pdfcraft_automation::ToolError) -> Self {
        match e {
            pdfcraft_automation::ToolError::UnknownTool(name) => FfiError::UnknownTool { name },
            pdfcraft_automation::ToolError::InvalidArgs(reason) => FfiError::InvalidArgs { reason },
            pdfcraft_automation::ToolError::Failed(reason) => FfiError::ToolFailed { reason },
        }
    }
}

impl From<pdfcraft_engine::EditError> for FfiError {
    fn from(e: pdfcraft_engine::EditError) -> Self {
        match e {
            pdfcraft_engine::EditError::NoDocument => FfiError::NotFound { id: "active".into() },
            pdfcraft_engine::EditError::ReadOnly(reason) => FfiError::ReadOnly { reason },
            pdfcraft_engine::EditError::NotPermitted(op) => FfiError::NotPermitted { operation: op.to_string() },
            pdfcraft_engine::EditError::WrongPassword => FfiError::WrongPassword,
            other => FfiError::EditFailed { reason: other.to_string() },
        }
    }
}

impl From<std::io::Error> for FfiError {
    fn from(e: std::io::Error) -> Self {
        FfiError::Io { reason: e.to_string() }
    }
}

/// Executes a closure inside a `catch_unwind` guard.
///
/// Ensures that no panic escapes into foreign code (Swift/Kotlin/C ABI),
/// transforming any panic payload into [`FfiError::Internal`].
pub fn catch_panic<F, T>(f: F) -> Result<T, FfiError>
where
    F: FnOnce() -> Result<T, FfiError> + UnwindSafe,
{
    match std::panic::catch_unwind(f) {
        Ok(result) => result,
        Err(payload) => {
            let reason = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            Err(FfiError::Internal { reason })
        }
    }
}
