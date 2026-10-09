//! pdfcraft-render — page rasterization and document inspection.
//!
//! **Bootstrap status (see ADR-0004):** rasterization goes straight through the `hayro` crate
//! (hayro-interpret + vello_cpu), and inspection (outline, annotations, fields, layers,
//! attachments, metadata) uses `lopdf`. Both are replaced by our own `cos`/`model` crates and the
//! DisplayList device design in M1–M2. The public API here is what the engine and UI rely on, so
//! the swap stays internal to this crate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod inspect;
mod raster;
pub mod text;

pub use inspect::{
    Annotation, Attachment, AttachmentSource, DestView, DocInfo, Field, FieldKind, FontInfo, Layer, LayerOp, Link, LinkTarget, OutlineItem, PageInfo,
    Xfa, attachment_data, inspect, pretty_date,
};
pub use raster::{
    MAX_PIXELS, MAX_SIDE, PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderedPage, RequestKind, Tile, device_pixels, effective_scale,
};
pub use text::{PageText, TextGlyph};

/// Errors surfaced to the user when a document cannot be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the file is not a readable PDF: {0}")]
    Invalid(String),
    #[error("the document is protected by a password")]
    NeedsPassword,
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("{0}")]
    Unsupported(String),
}
