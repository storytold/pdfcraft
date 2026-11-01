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
pub mod text_style_bookmarks;
pub mod word_count;
pub mod xfa;
mod xfa_form;

use std::sync::Arc;

pub use links::{
    MAX_REMOTE_DOCUMENT_BYTES, RemoteDocumentPathError, read_remote_document, remote_document_path_is_absolute, resolve_remote_document_path,
};
pub use signature_image::{ImageSignaturePreview, SignatureImage};
pub use signature_scan::{PhotoRect, ScanError, SignaturePhoto, Turn};
pub use stamp_preview::{CommentLayer, StampPreview};
pub use text_preview::{TextEditPreview, UnmarkedPages};

pub use pdfcraft_organize::{BoxSpec, FieldConflicts, PageBox, SplitBy, Transition, TransitionSpec, mime_for_name, split_ranges};

pub use pdfcraft_organize::LabelStyle;
pub use pdfcraft_organize::Standard;
pub use pdfcraft_organize::check_custom_key;
pub use pdfcraft_organize::view::{InitialView, Layout as InitialLayout, Magnification, Navigation};

pub use pdfcraft_cos::Algorithm;
pub use pdfcraft_create::{CONVERTIBLE, CreateError, ImageResolution, SourceKind, decode_text, source_kind};
pub use pdfcraft_edit::added::{
    CHAR_SPACING_LIMIT as ADDED_CHAR_SPACING_LIMIT, paste_place as added_paste_place, paste_rect as added_paste_rect, text_rect as added_text_rect,
    text_width as added_text_width,
};
pub use pdfcraft_edit::paragraph_align;
pub use pdfcraft_edit::{
    Added, AddedImage, AddedText, Align as TextAlign, Background, Content as AddedContent, Family as FontFamily, HeaderFooter, MarkKind, Watermark,
};
pub use pdfcraft_forms::{
    BorderStyle, CalculatedField, CalculationIssue, CalculationOrder, CheckStyle, Field as FormField, FieldAction, FieldChange, FieldFont,
    FieldKind as FormFieldKind, FieldProps, FieldValue, Look as FieldLook, LookPatch as FieldLookPatch, NewField, TabOrder, Trigger as FieldTrigger,
    Widget as FormWidget, af as form_scripts, appearance as form_appearance, appearance::auto_text_size_by as field_auto_text_size_by,
    appearance::comb_auto_text_size as field_comb_auto_text_size, calculations_changed as form_calculations_changed, flags as field_flags,
};

pub use pdfcraft_a11y as a11y;
pub use pdfcraft_edit::{Backdrop, BlockKey, BlockStyle, PageImage, RangeStyle, TextBlock, TextLine, block_overlapping, first_undrawable};
pub use pdfcraft_measure as measure;
pub use pdfcraft_xfa::Report as XfaLayout;

pub use pdfcraft_edit::{EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};

pub use pdfcraft_fonts::{MAX_SIGNATURE_CHARS, ScriptOutline, script_missing, script_outline, undrawable_message};

/// Comment geometry helpers (text-box line breaking) for frontends.
pub use pdfcraft_annot::appearance as annot_text;
pub use pdfcraft_annot::links::{Highlight as LinkHighlight, LinkAction, LinkItem, LinkStyle};
pub use pdfcraft_annot::{
    AppearanceCache, AttachIcon, CALLOUT_KNEE_LINE, FillMark, LineEnding, Markup, NOTE_SIZE, NewAnnotation, NoteIcon, OverlayFont, OverlayLook,
    Props as CommentProps, ReviewState, Rgb, Shape, Side as CalloutSide, StampGroup, StampKind, Style, callout_attach, callout_knee, callout_side,
    rect_quad,
};
pub use pdfcraft_forms::detect;
pub use pdfcraft_optimize as optimize;
pub use pdfcraft_preflight as pdfa;
pub use pdfcraft_print as print;
pub use pdfcraft_redact::codes::{CODE_SETS as REDACTION_CODE_SETS, CodeSet as RedactionCodeSet};
pub use pdfcraft_redact::patterns::{
    MAX_WORDS as REDACT_MAX_WORDS, PATTERNS as REDACT_PATTERNS, Pattern as RedactPattern, find as find_pattern, word_list as redact_word_list,
};
pub use pdfcraft_redact::sanitize::{HIDDEN, Hidden};
pub use pdfcraft_sign as sign;
pub use pdfcraft_sign::{SignOptions, SignatureInfo, Status as SignatureStatus, TrustStore};
pub use pdfcraft_xfdf::Format as DataFormat;

use bookmarks::outline_rows;
pub use bookmarks::{BookmarkPage, BookmarkRow};
use comments::comment_list;
pub use comments::{SummarySort, comment_kind, comment_summary, comment_type_name};
#[cfg(test)]
use document::page_runs;
pub use document::{Document, EditNotice, TextNotice, display_bytes_for, render_password_for};
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

mod clock;
mod doc_state;

use clock::{local_utc_offset, parse_ymd};
pub use doc_state::{DocId, ImageEdit, OptimizeReport, RecoverySnapshot, SplitPart, guard, merge_data_files};
use doc_state::{LOW_RESOLUTION_PRINT_DPI, render_threads, renderer_password, signatures_of, writable};

/// The configuration of the pool that draws the viewer: the document's own, plus installed CJK
/// fonts for fonts a PDF does not embed (`pdfcraft_render::RenderConfig::system_fonts`). Exports,
/// printing, OCR and automation use the document configuration itself, so nothing drawn with an
/// installed font is saved or printed (AGENTS.md §1.2, §1.4).
fn display_config(config: &pdfcraft_render::RenderConfig) -> pdfcraft_render::RenderConfig {
    config.clone().on_screen()
}

#[cfg(test)]
mod tests;
