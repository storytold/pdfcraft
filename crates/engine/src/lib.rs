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
pub use stamp_preview::StampPreview;
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
pub use pdfcraft_edit::{BlockKey, BlockStyle, PageImage, RangeStyle, TextBlock, TextLine, block_overlapping, first_undrawable};
pub use pdfcraft_measure as measure;
#[cfg(feature = "xfa")]
pub use pdfcraft_xfa::Report as XfaLayout;
#[cfg(not(feature = "xfa"))]
pub use xfa::XfaLayout;

pub use pdfcraft_edit::{EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};

pub use pdfcraft_fonts::{MAX_SIGNATURE_CHARS, ScriptOutline, script_missing, script_outline, undrawable_message};

/// Comment geometry helpers (text-box line breaking) for frontends.
pub use pdfcraft_annot::appearance as annot_text;
pub use pdfcraft_annot::links::{Highlight as LinkHighlight, LinkAction, LinkItem, LinkStyle};
pub use pdfcraft_annot::{
    AppearanceCache, AttachIcon, FillMark, LineEnding, Markup, NOTE_SIZE, NewAnnotation, NoteIcon, OverlayFont, OverlayLook, Props as CommentProps,
    ReviewState, Rgb, Shape, StampGroup, StampKind, Style, rect_quad,
};
pub use pdfcraft_forms::detect;
pub use pdfcraft_optimize as optimize;
#[cfg(feature = "preflight")]
pub use pdfcraft_preflight as pdfa;
#[cfg(not(feature = "preflight"))]
pub mod pdfa {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Level {
        A1b,
        A1a,
        A2b,
        A2u,
        A2a,
        A3b,
        A3u,
        A3a,
    }
    impl Level {
        pub fn label(self) -> &'static str {
            match self {
                Level::A1b => "PDF/A-1b",
                Level::A1a => "PDF/A-1a",
                Level::A2b => "PDF/A-2b",
                Level::A2u => "PDF/A-2u",
                Level::A2a => "PDF/A-2a",
                Level::A3b => "PDF/A-3b",
                Level::A3u => "PDF/A-3u",
                Level::A3a => "PDF/A-3a",
            }
        }
    }
}
pub use pdfcraft_print as print;
pub use pdfcraft_redact::codes::{CODE_SETS as REDACTION_CODE_SETS, CodeSet as RedactionCodeSet};
pub use pdfcraft_redact::patterns::{
    MAX_WORDS as REDACT_MAX_WORDS, PATTERNS as REDACT_PATTERNS, Pattern as RedactPattern, find as find_pattern, word_list as redact_word_list,
};
pub use pdfcraft_redact::sanitize::{HIDDEN, Hidden};
#[cfg(feature = "sign")]
pub use pdfcraft_sign as sign;
#[cfg(feature = "sign")]
pub use pdfcraft_sign::{SignOptions, SignatureInfo, Status as SignatureStatus, TrustStore};
#[cfg(not(feature = "sign"))]
pub mod sign {
    use std::sync::Arc;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub enum Status {
        #[default]
        Unknown,
        Valid,
        Invalid,
        Indeterminate,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct SignatureInfo {
        pub signed: bool,
        pub field: String,
        pub signer: Option<String>,
        pub date: Option<String>,
        pub status: Status,
        pub reason: Option<String>,
        pub location: Option<String>,
        pub contact: Option<String>,
        pub covers_whole_document: bool,
        pub digest_algorithm: Option<String>,
        pub signature_algorithm: Option<String>,
        pub cert_issuer: Option<String>,
        pub cert_serial: Option<String>,
        pub cert_validity: Option<(String, String)>,
        pub self_signed: bool,
        pub trust_anchor_found: bool,
        pub time_stamp_token: bool,
        pub ocsp_response: bool,
        pub crl: bool,
        pub revisions: usize,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TrustStore;

    #[derive(Clone, Debug)]
    pub enum Graphic {
        Outlines(Vec<Vec<[f64; 2]>>),
        Image(Arc<Vec<u8>>),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Appearance {
        pub name: bool,
        pub date: bool,
        pub reason: bool,
        pub location: bool,
        pub distinguished_name: bool,
        pub labels: bool,
    }

    impl Default for Appearance {
        fn default() -> Self {
            Self { name: true, date: true, reason: false, location: false, distinguished_name: false, labels: true }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct AppearanceLabels {
        pub signed_by: String,
        pub distinguished_name: String,
        pub reason: String,
        pub location: String,
        pub date: String,
    }

    impl Default for AppearanceLabels {
        fn default() -> Self {
            Self {
                signed_by: "Digitally signed by".into(),
                distinguished_name: "DN: ".into(),
                reason: "Reason: ".into(),
                location: "Location: ".into(),
                date: "Date: ".into(),
            }
        }
    }

    impl AppearanceLabels {
        pub fn first_undrawable(&self) -> Option<(&'static str, char)> {
            None
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct SignOptions {
        pub field: Option<String>,
        pub page: usize,
        pub rect: [f64; 4],
        pub contact: Option<String>,
        pub location: Option<String>,
        pub reason: Option<String>,
        pub signer_name: String,
        pub date: String,
        pub certify: Option<u8>,
        pub graphic: Option<Graphic>,
        pub appearance: Appearance,
        pub appearance_labels: AppearanceLabels,
    }

    #[derive(Clone, Debug, Default)]
    pub struct DigitalId {
        pub certificate_der: Vec<u8>,
        pub private_key_der: Vec<u8>,
    }

    #[derive(Debug, thiserror::Error)]
    pub enum SignError {
        #[error("unsupported: {0}")]
        Unsupported(String),
        #[error("{0}")]
        Other(String),
    }

    pub mod pkcs12 {
        pub const MIN_PASSWORD_CHARS: usize = 8;
        use super::{DigitalId, SignError};
        pub fn open(_bytes: &[u8], _password: &str) -> Result<DigitalId, SignError> {
            Err(SignError::Unsupported("Digital signatures feature not enabled".into()))
        }
    }
}
pub use pdfcraft_xfdf::Format as DataFormat;

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

mod clock;
mod doc_state;

use clock::{local_utc_offset, parse_ymd};
pub use doc_state::{DocId, ImageEdit, OptimizeReport, RecoverySnapshot, SplitPart, guard, merge_data_files};
use doc_state::{LOW_RESOLUTION_PRINT_DPI, render_threads, renderer_password, signatures_of, writable};

#[cfg(test)]
mod tests;
