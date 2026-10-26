//! pdfcraft-engine — the façade every frontend talks to (architecture §4).
//!
//! Holds open documents, their edit history and the tool catalogue. Frontends never touch the
//! parsing, rendering or editing crates directly.
//!
//! **Editing model.** Each document keeps a `pdfcraft_cos::Document` (the object graph with a
//! copy-on-write overlay of edits). An edit runs on a clone, and on success the previous state is
//! pushed onto the undo stack (clones share all unchanged data, so this is cheap). After every
//! edit the *working file* is produced by an incremental write — original bytes plus one
//! appended revision — and the view is refreshed from it, so what you see is exactly what Save
//! will write. Saving rebases onto the written bytes, so the next save appends only new edits.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod actions;
pub mod autotag;
mod bookmarks;
pub mod build_info;
pub mod camera;
pub mod camera_clean;
pub mod camera_edges;
pub mod catalog;
pub mod commands;
mod comments;
pub mod compare;
mod copy_image;
pub mod dates;
mod document;
mod edit;
mod error;
pub mod export;
pub mod field_table;
mod fill_sign;
mod history;
pub mod js;
mod layers;
pub mod links;
pub mod ocr;
mod open;
pub mod optimizer;
mod page_filter;
mod pictures;
pub mod print_raster;
pub mod printcheck;
mod protection;
mod reexports;
mod scan_colours;
mod scan_fields;
mod session;
pub mod signature_image;
pub mod signature_scan;
mod sources;
mod stamp_preview;
#[cfg(test)]
mod test_hooks;
pub mod text_fonts;
mod text_preview;
pub mod word_count;
pub mod xfa;
mod xfa_form;

use std::sync::Arc;

pub use links::{
    MAX_REMOTE_DOCUMENT_BYTES, RemoteDocumentPathError, read_remote_document, remote_document_path_is_absolute, resolve_remote_document_path,
};
pub use signature_image::{ImageSignaturePreview, SignatureImage};
pub use signature_scan::{PhotoRect, ScanError, SignaturePhoto, Turn};
pub use stamp_preview::StampPreview;
pub use text_preview::TextEditPreview;

pub use reexports::*;

use bookmarks::outline_rows;
pub use bookmarks::{BookmarkPage, BookmarkRow};
use comments::comment_list;
pub use comments::{SummarySort, comment_kind, comment_summary, comment_type_name};
pub use document::{Document, TextNotice, display_bytes_for, render_password_for};
use document::{display_bytes, info_fields, recovered_render_password, same_pages, shown_under_redactions, undrawn_notices};
pub use edit::{BATCH_LABELS, EDIT_LABELS, Edit};
pub use error::{EditError, SourceProblem};
pub use fill_sign::{displayed_offset, text_box_at, typed_signature_graphic, typed_signature_shape};
pub use history::Label;
use layers::{apply_layer_state, set_default_layer_state, use_layer_choices};
pub use open::{OpenContext, PreparedDocument};
pub use page_filter::{PageOrientation, PageParity, filter_pages};
pub use pictures::MarkFile;
use pictures::{embed_image, fit_picture, mark_source};
use protection::unrestricted;
pub use protection::{Changes, Printing, Protection, SecuritySummary};
pub use session::Session;
pub use sources::{CombineSource, Combined, combine_source_check, source_page_count};
use sources::{open_source, open_source_with};
#[cfg(test)]
use test_hooks::{check_refresh_panic, panic_on_refresh, record_link_scan, record_marker_scan, reset_secondary_scan_counts, secondary_scan_counts};
use xfa_form::{
    XFA_REMOVED_PREFIX, XFA_SCRIPTS_OFF, decode_data_file, image_content_type, is_xfa_data_file, note_warnings, strip_xfa, sync_xfa_state,
    undrawn_field_warnings, xfa_after_data_change, xfa_layout, xfa_redraw, xfa_sync_datasets, xfa_values_from_datasets,
};

/// A change to an existing page image.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageEdit {
    /// Move and resize it to this box (user space).
    Move([f64; 4]),
    /// Quarter turns clockwise about its centre.
    Rotate(i32),
    /// Any angle in degrees clockwise about its centre (the unit is degrees, not quarter turns).
    RotateBy(f64),
    Flip {
        horizontal: bool,
    },
    /// Show only this box of it (user space), the pixels staying where they are.
    Crop([f64; 4]),
    /// Draw this image file (PNG, JPEG, TIFF, GIF, BMP, JPEG 2000) in its place.
    Replace {
        name: String,
        bytes: Arc<Vec<u8>>,
    },
    Delete,
}

/// Forms ▸ Merge data files into spreadsheet: the field values of each file (FDF, XFDF or a
/// filled-in PDF form), as CSV with one row per file.
pub fn merge_data_files(files: &[(String, Vec<u8>)]) -> Result<String, String> {
    let rows =
        files.iter().map(|(name, bytes)| pdfcraft_xfdf::data_values(bytes).map_err(|e| format!("{name}: {e}"))).collect::<Result<Vec<_>, _>>()?;
    Ok(pdfcraft_xfdf::merge_csv(&rows))
}

/// One file produced by a split: (1-based first page, last page, PDF bytes).
pub type SplitPart = (usize, usize, Arc<Vec<u8>>);

/// Stable identifier of an open document within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

/// The highest resolution a document that allows only low-resolution printing (Table 22, bit 12
/// clear) is printed at.
const LOW_RESOLUTION_PRINT_DPI: f64 = 150.0;

fn render_threads() -> usize {
    std::thread::available_parallelism().map(std::num::NonZeroUsize::get).unwrap_or(4).clamp(2, 8) - 1
}

/// A document's working file captured for crash recovery.
#[derive(Clone, Debug)]
pub struct RecoverySnapshot {
    pub doc: DocId,
    pub name: String,
    pub path: Option<String>,
    pub bytes: Arc<Vec<u8>>,
    /// The snapshot is encrypted (recovering it asks for the password again).
    pub encrypted: bool,
}

/// Seconds to add to UTC for local time (0 where unknown).
fn local_utc_offset() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    return i64::from(chrono::Local::now().offset().local_minus_utc());
    #[cfg(target_arch = "wasm32")]
    return 0;
}

/// (year, month, day) from a PDF date `D:YYYYMMDD…`.
fn parse_ymd(d: &str) -> Option<(i64, u32, u32)> {
    let d = d.strip_prefix("D:").unwrap_or(d);
    Some((d.get(0..4)?.parse().ok()?, d.get(4..6)?.parse().ok()?, d.get(6..8)?.parse().ok()?))
}

/// The recovered user password of an R2–R4 file as text for the renderer, which tries a password
/// as its UTF-8 bytes and then in PDFDocEncoding: UTF-8 bytes (as some writers store passwords
/// PDFDocEncoding can't hold, such as "şifre") stay UTF-8, and other bytes are read as
/// PDFDocEncoding, so either way the renderer gets back exactly these bytes.
fn renderer_password(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|b| pdfcraft_cos::pdfdoc_char(*b)).collect(),
    }
}

/// The document as it may be written to a file (#3926, #3944): a copy without the values of text
/// fields with the Password flag (or XFA `<passwordEdit/>` fields in the datasets), or `cos`
/// itself when it holds none. ISO 32000-2 §12.7.5.3, Table 231 (a note to the Password flag):
/// "it is imperative that PDF processors never store the value of the text field in the PDF file
/// if this flag is set". The open document keeps them in memory.
fn writable(cos: &pdfcraft_cos::Document) -> Result<std::borrow::Cow<'_, pdfcraft_cos::Document>, EditError> {
    let has_form_pw = pdfcraft_forms::has_password_values(cos);
    let has_xfa_pw = pdfcraft_xfa::has_password_values(cos);
    if !has_form_pw && !has_xfa_pw {
        return Ok(std::borrow::Cow::Borrowed(cos));
    }
    let mut copy = cos.clone();
    if has_form_pw {
        pdfcraft_forms::clear_password_values(&mut copy).map_err(|e| EditError::Write(e.to_string()))?;
    }
    if has_xfa_pw {
        pdfcraft_xfa::clear_password_values(&mut copy).map_err(|e| EditError::Write(e.to_string()))?;
    }
    Ok(std::borrow::Cow::Owned(copy))
}

/// The last-resort guard (AGENTS.md §4): run `f`, turning a panic that escapes it into an error
/// message, so one bad file or edit can't take the app and its other documents down. It is a
/// safety net for bugs, not a substitute for returning errors.
pub fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>().map(|s| (*s).to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
    })
}

/// Validate the signature fields of `cos` (written as `bytes`).
fn signatures_of(cos: &pdfcraft_cos::Document, bytes: &[u8], trust: &TrustStore, cache: &pdfcraft_sign::DigestCache) -> Vec<SignatureInfo> {
    pdfcraft_sign::pdf::list_cached(cos, bytes, trust, cache)
}

/// What Optimize PDF did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptimizeReport {
    pub optimize: optimize::Report,
    /// Identical objects merged.
    pub merged: usize,
    /// Remove Hidden Information categories discarded, with counts.
    pub discarded: Vec<(Hidden, usize)>,
}

#[cfg(test)]
mod tests;
