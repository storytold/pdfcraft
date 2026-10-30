//! The session: the open documents and the operations on them.

use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
use pdfcraft_render::{LayerOp, OpenError, ReadingColors, RenderConfig, RenderPool, inspect};

#[cfg(test)]
use crate::check_refresh_panic;
use crate::edit::{EditCtx, check_permission, run_edit};
use crate::history::{Keys, MAX_UNDO, Scope, keys_after, scope_of, uses_scripts};
use crate::{
    BookmarkPage, BookmarkRow, CombineSource, Combined, DataFormat, DocId, Document, Edit, EditError, Hidden, ImageResolution,
    LOW_RESOLUTION_PRINT_DPI, Label, ObjectKind, OptimizeReport, RecoverySnapshot, SignOptions, SourceKind, SplitPart, SummarySort, TrustStore,
    XFA_REMOVED_PREFIX, XFA_SCRIPTS_OFF, apply_layer_state, comment_list, comment_summary, decode_text, display_bytes, guard, info_fields, js,
    local_utc_offset, note_warnings, open_source, open_source_with, optimize, outline_rows, parse_ymd, print, print_raster, render_threads,
    scan_colours, set_default_layer_state, shown_under_redactions, signatures_of, source_kind, sync_xfa_state, undrawn_notices, use_layer_choices,
    writable, xfa, xfa_sync_datasets,
};

mod editing;
mod files;
mod save;
mod view;

/// A new file's bytes, its modification date `now`.
fn write_dated(doc: &pdfcraft_cos::Document, now: Option<i64>) -> Result<Arc<Vec<u8>>, EditError> {
    let opts = SaveOptions { mod_date: now.map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
    write_full(doc, &opts).map(Arc::new).map_err(|e| EditError::Write(e.to_string()))
}

/// A new file written and checked like a save (see [`Session::write_new`]), dated `now`.
pub(crate) fn write_checked(doc: &pdfcraft_cos::Document, now: Option<i64>) -> Result<Arc<Vec<u8>>, EditError> {
    let bytes = write_dated(doc, now)?;
    let expected = pdfcraft_organize::page_count(doc)?;
    pdfcraft_cos::Document::open(bytes.clone()).map_err(|e| EditError::Reopen(e.to_string()))?;
    pdfcraft_render::check_pages(bytes.clone(), None, expected).map_err(|e| EditError::Reopen(e.to_string()))?;
    Ok(bytes)
}

#[derive(Default)]
pub struct Session {
    pub(crate) docs: Vec<Document>,
    pub(crate) next_id: u64,
    /// Seconds since the Unix epoch, injected so saves are deterministic in tests.
    clock: Option<fn() -> i64>,
    /// Seconds to add to UTC for local time, read from whoever knows the zone (a browser, say).
    /// Separate from `clock` on purpose: `clock` alone is the test shortcut that takes the
    /// injected time AS local time and zeroes the zone, so injecting only it leaves either the
    /// local dates or the UTC ones wrong by the zone's offset.
    utc_offset: Option<fn() -> i64>,
    /// Certificates trusted for signing (Acrobat: Trusted Certificates).
    pub(crate) trust: Arc<TrustStore>,
    /// Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript, inverted (on by default).
    pub(crate) js_off: bool,
    /// Preferences ▸ Date format, when not the default (see [`dates`]).
    pub(crate) date_format: Option<String>,
    /// Preferences ▸ Date format ▸ Language, when not following the interface language.
    pub(crate) date_language: Option<String>,
    /// What the XFA scripts' time limits are multiplied by (`None`: 1); see
    /// [`Session::with_xfa_time_scale`].
    pub(crate) xfa_time_scale: Option<u32>,
    /// The paper reflowed EPUB books are set on (`books`); US Letter when unset.
    pub(crate) book_paper: Option<(f64, f64)>,
}
