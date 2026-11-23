//! pdfcraft-render — page rasterization and document inspection.
//!
//! **Bootstrap status (see ADR-0004):** rasterization uses the shared `craft-pdf` facade over `hayro`
//! (hayro-interpret + vello_cpu). Inspection uses the lazy `cos` reader with a read-only lopdf
//! object adapter and retains `lopdf` loading for compatibility repairs. The renderer and that
//! adapter are replaced by `model` and the DisplayList design in M1–M2. The public API is what the engine and UI rely on, so
//! the swap stays internal to this crate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod backend;
mod credentials;
mod inspect;
mod link_area;
pub use credentials::AuthenticatedDocument;
mod pixels;
mod raster;
mod reading;
mod structure;
mod system_font;
pub mod text;
pub mod transition;

pub use inspect::{
    Annotation, Attachment, AttachmentSource, DestView, DocInfo, Field, FieldKind, FontInfo, Layer, LayerItem, LayerOp, LayerRow, Link, LinkTarget,
    MAX_GROUP_DEPTH, OutlineItem, PageInfo, PageTransition, Popup, Portfolio, PortfolioView, RemoteDestination, ReviewStates, TransitionStyle, Xfa,
    attachment_data, attachment_data_with_credentials, check_pages, check_pages_with_credentials, fill_group_text, group_primaries, inspect,
    inspect_with_credentials, link_replies, named_destination, pretty_date, thread_tops,
};
pub use link_area::{MAX_LINK_QUADS, MAX_URI_BASE_BYTES, link_quads, quad_contains, resolve_uri};
pub use pixels::Pixels;
pub use raster::{
    MAX_PIXELS, MAX_SIDE, PRINT_TAG, PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderStats, RenderWarning, RenderedPage, RequestKind,
    Retiring, Tile, device_pixels, effective_scale,
};
pub use reading::{ReadingColors, ReadingParams};
pub use text::{PageText, TextGlyph};

/// Errors surfaced to the user when a document cannot be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the file is not a readable PDF: {0}")]
    Invalid(String),
    #[error("the document is protected by a password")]
    NeedsPassword,
    #[error("the document requires a certificate private key")]
    NeedsCertificate,
    #[error("certificate security: {0}")]
    CertificateSecurity(String),
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("{0}")]
    Unsupported(String),
}

#[cfg(test)]
mod lab_bradford_tests;
