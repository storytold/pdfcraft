//! pdfcraft-cos — the PDF object layer (L1, standalone with `pdfcraft-filters`).
//!
//! Parses the COS object graph lazily and tolerantly, keeps edits in a copy-on-write overlay
//! (cheap snapshots for undo), and writes documents back either incrementally (original bytes
//! untouched) or as a full, garbage-collected rewrite. The PDF object graph *is* PdfCraft's
//! document model (plan/architecture.md §5, ADR-0010).
//!
//! Status (M1 in progress): parsing of all xref forms, object streams, repair by scanning,
//! incremental and full writing. Not yet: encryption (M1.6), object-stream / xref-stream output
//! for full saves, linearization (M11).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bytes;
mod document;
mod object;
pub mod page_labels;
mod parser;
mod security;
mod writer;

pub use bytes::Bytes;
pub use document::{Document, Revision, XrefEntry};
pub use object::{Dict, MAX_DECODED, Name, ObjRef, Object, PdfString, Stream};
pub use parser::{Lexer, parse_indirect};
pub use pdfcraft_crypt::{Algorithm, Auth, Method as CryptMethod, NewEncryption, Permissions, SecurityHandler};
pub use writer::{SaveOptions, pdf_date, serialize, write_full, write_incremental};

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CosError {
    #[error("not a PDF file (no %PDF- header)")]
    NotPdf,
    #[error("syntax error at byte {offset}: {detail}")]
    Syntax { offset: usize, detail: String },
    #[error("object {0} is missing")]
    MissingObject(u32),
    #[error("object {0} is not a dictionary")]
    NotADictionary(ObjRef),
    #[error("stream data could not be decoded: {0}")]
    Filter(String),
    #[error("the document is protected by a password")]
    NeedsPassword,
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("unsupported security: {0}")]
    Security(String),
    #[error("internal lock poisoned")]
    Poisoned,
}
