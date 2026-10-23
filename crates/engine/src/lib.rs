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
pub mod build_info;
pub mod camera;
pub mod camera_clean;
pub mod camera_edges;
pub mod catalog;
pub mod commands;
pub mod compare;
mod copy_image;
pub mod dates;
pub mod export;
pub mod field_table;
pub mod js;
pub mod links;
pub mod ocr;
pub mod optimizer;
pub mod print_raster;
pub mod printcheck;
mod scan_colours;
mod scan_fields;
pub mod signature_image;
pub mod signature_scan;
mod stamp_preview;
pub mod text_fonts;
mod text_preview;
pub mod word_count;
pub mod xfa;

pub use signature_image::{ImageSignaturePreview, SignatureImage};
pub use signature_scan::{PhotoRect, ScanError, SignaturePhoto, Turn};
pub use stamp_preview::StampPreview;
pub use text_preview::TextEditPreview;

pub use pdfcraft_organize::{BoxSpec, FieldConflicts, PageBox, SplitBy, Transition, TransitionSpec, split_ranges};

/// One file produced by a split: (1-based first page, last page, PDF bytes).
pub use pdfcraft_organize::LabelStyle;
pub use pdfcraft_organize::Standard;
pub use pdfcraft_organize::view::{InitialView, Layout as InitialLayout, Magnification, Navigation};

pub use pdfcraft_cos::Algorithm;
pub use pdfcraft_create::{CONVERTIBLE, CreateError, ImageResolution, SourceKind, decode_text, source_kind};
pub use pdfcraft_edit::added::{text_rect as added_text_rect, text_width as added_text_width};
pub use pdfcraft_edit::{
    Added, AddedImage, AddedText, Align as TextAlign, Background, Content as AddedContent, Family as FontFamily, HeaderFooter, MarkKind, Watermark,
};
pub use pdfcraft_forms::{
    BorderStyle, CalculatedField, CalculationIssue, CalculationOrder, CheckStyle, Field as FormField, FieldAction, FieldChange, FieldFont,
    FieldKind as FormFieldKind, FieldProps, FieldValue, Look as FieldLook, LookPatch as FieldLookPatch, NewField, TabOrder, Trigger as FieldTrigger,
    Widget as FormWidget, af as form_scripts, appearance as form_appearance, appearance::auto_text_size_by as field_auto_text_size_by,
    calculations_changed as form_calculations_changed, flags as field_flags,
};

pub use pdfcraft_a11y as a11y;
pub use pdfcraft_edit::{BlockKey, BlockStyle, PageImage, RangeStyle, TextBlock, TextLine, block_overlapping, first_undrawable};
pub use pdfcraft_measure as measure;
pub use pdfcraft_xfa::Report as XfaLayout;

pub use pdfcraft_edit::{EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};

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
pub use pdfcraft_fonts::{MAX_SIGNATURE_CHARS, ScriptOutline, script_missing, script_outline, undrawable_message};

/// Fill & Sign: `text` in the script font as a typed signature, its left edge at `at` (user
/// space, vertically centred) and `height` points tall. `None` for text with no outlines.
///
/// Left, centred and upright are as displayed on a page turned by `rotation` (its `/Rotate`):
/// the box runs along the displayed axes, like an image signature's
/// ([`SignatureImage::rect`]), and the outlines are turned back so the name reads across.
pub fn typed_signature_shape(at: [f64; 2], text: &str, height: f64, rotation: i64) -> Option<Shape> {
    let o = script_outline(text);
    let [left, bottom, right, top] = o.bounds();
    let span = (top - bottom).max(0.1);
    let width = right - left;
    if o.contours.is_empty() || o.width <= 0.0 || !(width > 0.0 && height > 0.0) {
        return None;
    }
    let k = height / span;
    let rect = signature_image::upright_box(rotation, at, width * k, height)?;
    // Displayed right and up as user-space unit vectors: a point at (nx, ny) of the displayed box
    // is at this fraction of the user-space box. Exact for an unturned page.
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    let to_user = |nx: f64, ny: f64| [a * nx + c * ny + (-a).max(0.0) + (-c).max(0.0), b * nx + d * ny + (-b).max(0.0) + (-d).max(0.0)];
    let contours = o.contours.iter().map(|c| c.iter().map(|p| to_user((p[0] - left) / width, (p[1] - bottom) / span)).collect()).collect();
    Some(Shape::TypedSignature { rect, contours })
}
/// `at` moved `right` and `up` points as displayed on a page turned by `rotation` (its `/Rotate`).
pub fn displayed_offset(rotation: i64, at: [f64; 2], right: f64, up: f64) -> [f64; 2] {
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    [at[0] + a * right + c * up, at[1] + b * right + d * up]
}

/// The user-space box `width` x `height` points as displayed on a page turned by `rotation`,
/// with its displayed top-left corner at `at`: where typed text and text boxes go. `None` for a
/// non-finite point.
pub fn text_box_at(rotation: i64, at: [f64; 2], width: f64, height: f64) -> Option<[f64; 4]> {
    signature_image::upright_box(rotation, displayed_offset(rotation, at, 0.0, -height / 2.0), width, height)
}

/// `text` in the script font as the graphic of a visible digital signature, as Fill & Sign types
/// it. `None` for text with no outlines.
pub fn typed_signature_graphic(text: &str) -> Option<pdfcraft_sign::Graphic> {
    let o = script_outline(text);
    o.contours.iter().any(|c| c.len() > 2).then_some(pdfcraft_sign::Graphic::Outlines(o.contours))
}

pub use links::{
    MAX_REMOTE_DOCUMENT_BYTES, RemoteDocumentPathError, read_remote_document, remote_document_path_is_absolute, resolve_remote_document_path,
};
/// Comment geometry helpers (text-box line breaking) for frontends.
pub use pdfcraft_annot::appearance as annot_text;
pub use pdfcraft_annot::links::{Highlight as LinkHighlight, LinkAction, LinkItem, LinkStyle};
pub use pdfcraft_annot::{
    AppearanceCache, AttachIcon, FillMark, LineEnding, Markup, NOTE_SIZE, NewAnnotation, NoteIcon, OverlayFont, OverlayLook, Props as CommentProps,
    ReviewState, Rgb, Shape, StampGroup, StampKind, Style, rect_quad,
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

/// Forms ▸ Merge data files into spreadsheet: the field values of each file (FDF, XFDF or a
/// filled-in PDF form), as CSV with one row per file.
pub fn merge_data_files(files: &[(String, Vec<u8>)]) -> Result<String, String> {
    let rows =
        files.iter().map(|(name, bytes)| pdfcraft_xfdf::data_values(bytes).map_err(|e| format!("{name}: {e}"))).collect::<Result<Vec<_>, _>>()?;
    Ok(pdfcraft_xfdf::merge_csv(&rows))
}

pub type SplitPart = (usize, usize, Arc<Vec<u8>>);

use std::sync::{Arc, OnceLock};

#[cfg(test)]
thread_local! {
    static SECONDARY_SCAN_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static REFRESH_PANICS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn panic_on_refresh(count: usize) {
    REFRESH_PANICS.with(|n| n.set(count));
}

#[cfg(test)]
fn check_refresh_panic() {
    REFRESH_PANICS.with(|n| {
        let rem = n.get();
        if rem > 0 {
            n.set(rem - 1);
            panic!("injected refresh panic");
        }
    });
}

#[cfg(test)]
fn reset_secondary_scan_counts() {
    SECONDARY_SCAN_COUNTS.with(|counts| counts.set((0, 0)));
}

#[cfg(test)]
fn secondary_scan_counts() -> (usize, usize) {
    SECONDARY_SCAN_COUNTS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_marker_scan() {
    SECONDARY_SCAN_COUNTS.with(|counts| {
        let (markers, links) = counts.get();
        counts.set((markers + 1, links));
    });
}

#[cfg(test)]
fn record_link_scan() {
    SECONDARY_SCAN_COUNTS.with(|counts| {
        let (markers, links) = counts.get();
        counts.set((markers, links + 1));
    });
}

use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
use pdfcraft_render::{DocInfo, Layer, LayerOp, OpenError, ReadingColors, RenderConfig, RenderPool, inspect};

/// Stable identifier of an open document within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

/// Undo/redo depth. Snapshots share unchanged data, so this is memory-cheap.
const MAX_UNDO: usize = 100;

/// The highest resolution a document that allows only low-resolution printing (Table 22, bit 12
/// clear) is printed at.
const LOW_RESOLUTION_PRINT_DPI: f64 = 150.0;

/// The passwords that go with one state of a document (protection can change them).
#[derive(Clone, Debug, Default, PartialEq)]
struct Keys {
    /// What the renderer and inspector open the working file with (the user password).
    render: Option<String>,
    /// What the editor re-opens a saved file with: the strongest password known (owner if any).
    reopen: Option<String>,
}

/// What an edit can change, and so what the view data must be rebuilt from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// Anything: re-inspect the whole document.
    Full,
    /// Document-information entries; page content and appearances are unchanged.
    Metadata,
    /// Only comments: re-read the comment list from the object graph.
    Comments,
    /// Only form field values (and their widget appearances).
    Form,
    /// Link annotations; the full inspection is refreshed, but page-content indexes stay valid.
    Links,
    /// Only page content streams, resources and added content/marks: the page tree,
    /// annotations, form fields, links, outline and fonts are unchanged, so the full
    /// re-inspection is skipped (seconds on very large files).
    Content,
}

/// Edits during which field scripts run.
fn uses_scripts(edit: &Edit) -> bool {
    match edit {
        Edit::SetFieldValue { .. } | Edit::ApplyScriptChanges { .. } | Edit::SetFieldScript { .. } | Edit::SetCalculationOrder { .. } => true,
        Edit::Batch { edits, .. } => edits.iter().any(uses_scripts),
        _ => false,
    }
}

fn scope_of(edit: &Edit) -> Scope {
    match edit {
        Edit::SetInfo { .. } => Scope::Metadata,
        // A file attachment also changes the Attachments list.
        Edit::AddAnnotation(a) if matches!(a.shape, Shape::Attachment { .. }) => Scope::Full,
        Edit::AddMeasurement(_)
        | Edit::AddAnnotation(_)
        | Edit::AddCustomStamp { .. }
        | Edit::DeleteAnnotation { .. }
        | Edit::SetAnnotationContents { .. }
        | Edit::ReplyToAnnotation { .. }
        | Edit::SetAnnotationStatus { .. }
        | Edit::MarkAnnotation { .. }
        | Edit::LockAnnotation { .. }
        | Edit::ReplaceText { .. }
        | Edit::EraseInk { .. }
        | Edit::MoveAnnotation { .. }
        | Edit::ReshapeAnnotation { .. }
        | Edit::ResizeAnnotation { .. }
        | Edit::StyleAnnotation { .. }
        | Edit::FillAnnotation { .. }
        | Edit::FontSizeAnnotation { .. }
        | Edit::SetAnnotationInfo { .. } => Scope::Comments,
        Edit::AddLink { .. } | Edit::SetLink { .. } | Edit::DeleteLink { .. } | Edit::RemoveLinks { .. } | Edit::AddLinks { .. } => Scope::Links,
        Edit::SetFieldValue { .. } | Edit::ResetForm { .. } | Edit::SetFieldImage { .. } | Edit::ApplyScriptChanges { .. } => Scope::Form,
        // Page content, resources and added content/marks only: the page tree, annotations,
        // fields, links and document metadata are untouched (see `refresh_content`).
        Edit::MoveObjects { .. }
        | Edit::EditTextLine { .. }
        | Edit::EditTextBlock { .. }
        | Edit::EditPageImage { .. }
        | Edit::AddText { .. }
        | Edit::AddImage { .. }
        | Edit::UpdateContent { .. }
        | Edit::DeleteContent { .. }
        | Edit::ReplaceImage { .. }
        | Edit::AddHeaderFooter { .. }
        | Edit::AddWatermark { .. }
        | Edit::AddBackground { .. }
        | Edit::RemoveMarks { .. } => Scope::Content,
        Edit::Batch { edits, .. } => {
            let mut scopes = edits.iter().map(scope_of);
            let first = scopes.next().unwrap_or(Scope::Full);
            if scopes.all(|s| s == first) { first } else { Scope::Full }
        }
        _ => Scope::Full,
    }
}

/// One undo/redo step: its label, the document state, its passwords, and what it changed.
type Snapshot = (Label, pdfcraft_cos::Document, Keys, Scope);

/// Editing state of a document (absent when the document cannot be edited yet, e.g. encrypted).
#[derive(Clone)]
struct Editor {
    cos: pdfcraft_cos::Document,
    /// The file as last opened or saved. Incremental saves append to it, and Revert opens it.
    /// Undo may restore a snapshot whose own base is an earlier revision; this does not follow it.
    saved: Arc<Vec<u8>>,
    /// Passwords that open [`Self::saved`].
    saved_keys: Keys,
    /// [`Self::saved`] was never written where the document belongs: a new document (combine,
    /// extract, create) or one restored from recovery. It stays unsaved until a save, whatever
    /// undo brings back, or closing it would lose the whole document without asking.
    unsaved_file: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    keys: Keys,
    /// Display appearances built for annotations without any, reused while no annotation edit
    /// runs (content edits leave every annotation dictionary alone). Cleared on any refresh
    /// that may change annotations; see [`AppearanceCache`].
    appearances: AppearanceCache,
    /// Display appearances drawn for form fields the file leaves to the viewer (#3470), reused
    /// for each widget while the objects it was drawn from are unchanged; see
    /// [`pdfcraft_forms::FieldAppearanceCache`].
    field_appearances: pdfcraft_forms::FieldAppearanceCache,
    /// The text blocks last computed for one page, reused while the document is unchanged.
    text_blocks: TextBlocksCache,
}

/// One page's text blocks ([`pdfcraft_edit::text_blocks`]) and the document state they were read
/// from. Opening a paragraph and applying its edit both need them, and grouping a long page is not
/// cheap. The state ([`pdfcraft_cos::ReadState`]) changes with every edit and save and returns
/// with undo, so a stale entry never matches, also after the document has been edited.
#[derive(Default)]
struct TextBlocksCache(std::sync::Mutex<Option<CachedTextBlocks>>);

/// The document state, the page (0-based) and its text blocks.
type CachedTextBlocks = (pdfcraft_cos::ReadState, usize, Arc<Vec<pdfcraft_edit::TextBlock>>);

impl Clone for TextBlocksCache {
    fn clone(&self) -> Self {
        // A poisoned cache only loses the cached blocks.
        Self(std::sync::Mutex::new(self.0.lock().ok().and_then(|c| c.clone())))
    }
}

impl Editor {
    /// The text blocks on `page` (0-based), from the cache when the document hasn't changed.
    fn text_blocks(&self, page: usize) -> Result<Arc<Vec<pdfcraft_edit::TextBlock>>, EditError> {
        if let Ok(cache) = self.text_blocks.0.lock()
            && let Some((state, p, blocks)) = cache.as_ref()
            && *p == page
            && state.matches(&self.cos)
        {
            return Ok(blocks.clone());
        }
        let blocks = Arc::new(pdfcraft_edit::text_blocks(&self.cos, page)?);
        if let Ok(mut cache) = self.text_blocks.0.lock() {
            *cache = Some((self.cos.read_state(), page, blocks.clone()));
        }
        Ok(blocks)
    }
}

pub struct Document {
    pub id: DocId,
    pub name: String,
    pub path: Option<String>,
    /// The working file: what Save writes.
    pub bytes: Arc<Vec<u8>>,
    /// What is displayed (the page view, `page_render`, page images): `bytes`, plus appearances
    /// drawn for display only for comments that have none ([`display_bytes`]). Never saved.
    pub display: Arc<Vec<u8>>,
    pub info: DocInfo,
    pub renderer: RenderPool,
    /// The password the document was opened with (needed to read attachments, etc.).
    pub password: Option<String>,
    /// `true` when there are edits that have not been saved.
    pub dirty: bool,
    /// Bumped on every change to the working file (edit, undo, redo, save); autosave compares it.
    generation: u64,
    /// The generation last handed out by `autosave_snapshots`.
    snapshot_generation: u64,
    /// Why the document cannot be edited (e.g. encryption), if so.
    pub read_only_reason: Option<String>,
    /// Interactive form fields of the current state (empty without a form).
    pub form: Arc<Vec<pdfcraft_forms::Field>>,
    marker_scan: OnceLock<pdfcraft_edit::MarkerScan>,
    links: OnceLock<Vec<pdfcraft_annot::links::LinkItem>>,
    /// Signature fields, validated against the session's trust store (Signatures panel).
    pub signatures: Arc<Vec<SignatureInfo>>,
    trust: Arc<TrustStore>,
    sig_cache: Arc<pdfcraft_sign::DigestCache>,
    editor: Option<Editor>,
    config: RenderConfig,
    /// What field scripts printed or asked for (see [`Session::take_js_output`]).
    js_output: js::JsOutput,
    /// Dynamic XFA forms: what laying the template out produced (pages and fields are
    /// PdfCraft's; Adobe's viewers draw the form from the XFA packets themselves).
    pub xfa: Option<XfaLayout>,
    /// The parsed template of a laid-out XFA form, for its scripts.
    xfa_template: Option<Arc<pdfcraft_xfa::model::Template>>,
    /// A script of the XFA form ran away, so its scripts are off (see [`Document::xfa_scripts_off`]).
    xfa_scripts_off: bool,
    /// A content-only refresh keeps `info` from before the edit; an edit may have added or
    /// embedded fonts, so the font list is read again on demand ([`Session::refresh_fonts`]).
    fonts_stale: bool,
    /// XFA forms: what was approximated, rewritten or could not be written to the XFA data
    /// (also in `info.warnings`, kept there when the document is re-read).
    pub xfa_warnings: Vec<String>,
    /// What the last successful [`Session::apply`] wrote that its fonts can't draw: kept as typed,
    /// drawn with `?` in its place (see [`TextNotice`]). The interface shows it; tools return it.
    pub edit_notices: Vec<TextNotice>,
}

/// Text an edit stored as typed but drew with `?` in place of `ch`, because the font it is drawn
/// with (a form field's, or the standard fonts for comments) can't draw that character. Acrobat
/// keeps such text too, drawing it with fonts PdfCraft can't embed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextNotice {
    /// The form field's full name, or `None` for a comment.
    pub field: Option<String>,
    /// The page (0-based) of the comment or of the field's widget.
    pub page: usize,
    pub ch: char,
    /// Other PDF viewers redraw it from the stored text (a form field, with `/NeedAppearances`
    /// set); otherwise the `?` shows in every viewer.
    pub redrawn_elsewhere: bool,
}

impl std::fmt::Display for TextNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ch = format!("\"{}\" (U+{:04X})", self.ch.escape_debug(), u32::from(self.ch));
        let page = self.page + 1;
        match (&self.field, self.redrawn_elsewhere) {
            (Some(name), true) => write!(
                f,
                "form field {name:?} (page {page}) holds {ch}, which its font can't draw: the value is saved, PdfCraft shows ? in its place, and other PDF viewers redraw the field from the value"
            ),
            (Some(name), false) => write!(
                f,
                "form field {name:?} (page {page}) holds {ch}, which its font can't draw: the value is saved, but this PDF/A document can't ask other viewers to redraw it, so ? shows in its place everywhere"
            ),
            (None, _) => write!(
                f,
                "a comment on page {page} has {ch}, which the standard fonts can't draw: its text is saved, but its appearance shows ? in its place in every viewer"
            ),
        }
    }
}

/// What `next` drew with `?` that `prev` didn't (a new or redrawn appearance carrying
/// `pdfcraft_fonts::UNDRAWN_MARK`), one notice per field or comment, in page order.
fn undrawn_notices(prev: &pdfcraft_cos::Document, next: &pdfcraft_cos::Document) -> Vec<TextNotice> {
    let Ok(pages) = pdfcraft_annot::page_refs(next) else { return Vec::new() };
    let redrawn_elsewhere = pdfcraft_forms::need_appearances(next);
    let mut names: Option<std::collections::HashMap<pdfcraft_cos::ObjRef, String>> = None;
    let mut out: Vec<TextNotice> = Vec::new();
    for (page, p) in pages.iter().enumerate() {
        let annots = next.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()).map(|a| next.resolve(&a));
        for r in annots.as_deref().and_then(pdfcraft_cos::Object::as_array).into_iter().flatten().filter_map(pdfcraft_cos::Object::as_ref) {
            let now = next.get(r);
            let Some(d) = now.as_dict() else { continue };
            let Some(ch) = pdfcraft_fonts::undrawn_mark(d) else { continue };
            // Only what this edit drew: a new annotation, or a new appearance.
            let was = prev.get(r);
            if was.as_dict().is_some_and(|w| w.get(b"AP") == d.get(b"AP") && pdfcraft_fonts::undrawn_mark(w) == Some(ch)) {
                continue;
            }
            let field = (d.name(b"Subtype") == Some(b"Widget")).then(|| {
                let names = names.get_or_insert_with(|| {
                    pdfcraft_forms::fields(next)
                        .into_iter()
                        .flat_map(|f| f.widgets.iter().map(|w| (w.obj, f.name.clone())).collect::<Vec<_>>())
                        .collect()
                });
                names.get(&r).cloned().unwrap_or_default()
            });
            if field.is_some() && out.iter().any(|n| n.field == field) {
                continue;
            }
            out.push(TextNotice { redrawn_elsewhere: redrawn_elsewhere && field.is_some(), field, page, ch });
        }
    }
    out
}

impl Document {
    /// How the document opens (Document Properties ▸ Initial View).
    /// Accessibility ▸ Check for accessibility: the full check (`None` if the document can't be
    /// read for editing).
    pub fn accessibility_check(&self, options: &a11y::Options) -> Option<a11y::Report> {
        self.editor.as_ref().map(|e| a11y::check(&e.cos, options))
    }

    /// A script of this XFA form ran past its time limit and was abandoned, so the form's
    /// scripts no longer run in this document (reopening it runs them again). The script's
    /// error is in the JavaScript output; this is the state, for a viewer to show.
    pub fn xfa_scripts_off(&self) -> bool {
        self.xfa_scripts_off
    }

    /// PDF Optimizer ▸ Audit space usage.
    pub fn audit_space(&self) -> Vec<optimize::SpaceUse> {
        self.editor.as_ref().map(|e| optimize::audit_space(&e.cos, self.bytes.len() as u64)).unwrap_or_default()
    }

    /// Edit a PDF: the images `page` draws (0-based).
    pub fn page_images(&self, page: usize) -> Vec<pdfcraft_edit::PageImage> {
        self.editor.as_ref().and_then(|e| pdfcraft_edit::page_images(&e.cos, page).ok()).unwrap_or_default()
    }

    /// Mixed editable objects on one page. Source inventories are zero-based and added items
    /// appear once. Rectangles are user-space points, independent of the viewer's zoom.
    pub fn editable_objects(&self, page: usize) -> Result<Vec<EditableObject>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_edit::editable_objects(&editor.cos, page).map_err(|e| e.to_string())
    }

    /// Displayed top-left-origin motion to a user-space vector. Use the linear map only,
    /// so even a large crop-box origin cannot erase a small drag by cancellation.
    pub fn object_move_offset(&self, page: usize, delta: [f64; 2]) -> Result<[f64; 2], String> {
        let p = self.info.pages.get(page).ok_or("the page no longer exists")?;
        let [x0, y0, x1, y1] = p.crop.map(f64::from);
        let (a, b) = (delta[0] / f64::from(p.width.max(1e-3)), delta[1] / f64::from(p.height.max(1e-3)));
        let (u, v) = match p.rotation {
            90 => (b, -a),
            180 => (-a, -b),
            270 => (-b, a),
            _ => (a, b),
        };
        let offset = [u * (x1 - x0), -v * (y1 - y0)];
        if !offset.iter().all(|v| v.is_finite()) {
            return Err("the move must be finite and non-zero".into());
        }
        Ok(offset)
    }

    /// Save image as: image `index` on `page` as a file (extension, bytes).
    pub fn page_image_file(&self, page: usize, index: usize) -> Result<(&'static str, Vec<u8>), String> {
        self.check_copying().map_err(|e| e.to_string())?;
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        let img = self.page_images(page).into_iter().nth(index).ok_or_else(|| format!("page {} has no image {}", page + 1, index + 1))?;
        if img.is_form {
            return Err("grouped Form artwork is not a bitmap; Save Image As is only available for raster images".into());
        }
        pdfcraft_create::image_file(&editor.cos, img.object.ok_or("the image has no object")?)
    }

    /// Edit a PDF ▸ Edit text: the paragraphs on `page` (0-based).
    pub fn text_blocks(&self, page: usize) -> Vec<pdfcraft_edit::TextBlock> {
        self.editor.as_ref().and_then(|e| e.text_blocks(page).ok()).map(|b| b.as_ref().clone()).unwrap_or_default()
    }

    /// Saved measurement annotations, calculated from their geometry and PDF scales, plus
    /// the ones that couldn't be read (unsupported formats are skipped, not fatal).
    pub fn measurements(&self) -> Result<measure::Listing, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        Ok(measure::list(&e.cos))
    }
    pub fn measurement_scale(&self, page: usize, at: measure::Point) -> Result<measure::Scale, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::scale_at(&e.cos, page, at).map_err(|e| e.to_string())
    }
    /// [`Self::measurement_scale`], measuring true size in `unit` (`per_point` of them in a
    /// point) where the page sets no scale.
    pub fn measurement_scale_or(&self, page: usize, at: measure::Point, per_point: f64, unit: &str) -> Result<measure::Scale, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::scale_at_or(&e.cos, page, at, per_point, unit).map_err(|e| e.to_string())
    }
    pub fn measurement_to_user(&self, page: usize, point: measure::Point) -> Result<measure::Point, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::view_to_user(&e.cos, page, point).map_err(|e| e.to_string())
    }
    pub fn measurement_to_view(&self, page: usize, point: measure::Point) -> Result<measure::Point, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::user_to_view(&e.cos, page, point).map_err(|e| e.to_string())
    }
    pub fn measurement_paths(&self, page: usize) -> Result<measure::snap::Geometry, String> {
        let e = self.editor.as_ref().ok_or("the document can't be read")?;
        measure::snap::geometry(&e.cos, page).map_err(|e| e.to_string())
    }

    /// A counter that changes with every edit (for caches of derived data).
    pub fn edit_generation(&self) -> u64 {
        self.generation
    }

    /// Edit a PDF ▸ Edit text: the lines of existing text on `page` (0-based).
    pub fn text_lines(&self, page: usize) -> Vec<pdfcraft_edit::TextLine> {
        self.editor.as_ref().and_then(|e| pdfcraft_edit::text_lines(&e.cos, page).ok()).unwrap_or_default()
    }

    /// The form's calculation order: its calculated fields in the order they calculate, what each
    /// formula reads, what is wrong with the order, and the order Fix order writes.
    pub fn calculation_order(&self) -> CalculationOrder {
        self.editor.as_ref().map(|e| pdfcraft_forms::calculation_order(&e.cos, &self.form)).unwrap_or_default()
    }

    /// Add alternate text: the figures, in document order.
    pub fn figures(&self) -> Vec<a11y::Figure> {
        self.editor.as_ref().map(|e| a11y::figures(&e.cos)).unwrap_or_default()
    }

    /// Reachable indirect structure tags, in document order.
    pub fn tags(&self) -> Vec<a11y::TagInfo> {
        self.editor.as_ref().map(|e| a11y::tags(&e.cos)).unwrap_or_default()
    }

    /// Marked-content items directly owned by one reachable indirect structure tag.
    pub fn tag_content(&self, object: pdfcraft_cos::ObjRef) -> Result<Vec<a11y::TagContentItem>, a11y::TagError> {
        self.editor.as_ref().map(|e| a11y::tag_content(&e.cos, object)).unwrap_or_else(|| Ok(Vec::new()))
    }

    /// The edit that fixes `rule`, for the rules with an automatic fix: the document language
    /// (`value` is the language), the title (`value` replaces it; else the current title or the
    /// file name) and the tab order.
    pub fn accessibility_fix(&self, rule: a11y::Rule, value: Option<&str>) -> Result<Edit, String> {
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        match rule {
            a11y::Rule::PrimaryLanguage => {
                let mut v = self.initial_view();
                v.language = Some(value.ok_or("give the document language (for example en-US)")?.to_owned());
                Ok(Edit::SetInitialView(Box::new(v)))
            }
            a11y::Rule::Title => {
                let title = value
                    .map(str::to_owned)
                    .or_else(|| self.info_value("Title").filter(|t| !t.trim().is_empty()))
                    .unwrap_or_else(|| self.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_owned());
                let mut v = self.initial_view();
                v.display_title = true;
                Ok(Edit::Batch {
                    label: "Set document title".into(),
                    edits: vec![Edit::SetInfo { key: "Title".into(), value: title }, Edit::SetInitialView(Box::new(v))],
                })
            }
            a11y::Rule::TabOrder => Ok(Edit::SetTabOrder { pages: (0..self.info.pages.len()).collect(), order: TabOrder::Structure }),
            other => Err(format!("\"{}\" has no automatic fix", other.name())),
        }
    }

    pub fn initial_view(&self) -> InitialView {
        self.editor.as_ref().map(|e| pdfcraft_organize::initial_view(&e.cos)).unwrap_or_default()
    }

    /// Signed: at least one signature field holds a signature.
    pub fn is_signed(&self) -> bool {
        self.signatures.iter().any(|s| s.signed)
    }

    /// Whether comments are hidden (Comments ▸ Hide all comments).
    pub fn comments_hidden(&self) -> bool {
        self.config.hide_comments
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo_label().map(Label::as_str)
    }

    pub fn can_redo(&self) -> Option<&str> {
        self.redo_label().map(Label::as_str)
    }

    /// What Undo would undo, in parts, for an interface that says it in another language.
    pub fn undo_label(&self) -> Option<&Label> {
        self.editor.as_ref().and_then(|e| e.undo.last()).map(|(l, ..)| l)
    }

    /// What Redo would redo, in parts.
    pub fn redo_label(&self) -> Option<&Label> {
        self.editor.as_ref().and_then(|e| e.redo.last()).map(|(l, ..)| l)
    }

    pub fn editable(&self) -> bool {
        self.editor.is_some()
    }

    /// What the opening password allows; `None` when the document is not encrypted.
    pub fn permissions(&self) -> Option<pdfcraft_cos::Permissions> {
        self.editor.as_ref().and_then(|e| e.cos.permissions())
    }

    /// Printing is allowed (Table 22, bit 3).
    pub fn allows_printing(&self) -> bool {
        self.permissions().is_none_or(|p| p.print())
    }

    /// Copying or otherwise extracting text and images is allowed (Table 22, bit 5): copying
    /// selected text, snapshots, Save Image As, exporting. Assistive technology may read the
    /// content regardless (§7.6.4.2), so reading aloud and accessibility tools don't ask.
    pub fn allows_copying(&self) -> bool {
        self.permissions().is_none_or(|p| p.copy())
    }

    /// [`Self::allows_copying`], as the error to report when it isn't.
    pub fn check_copying(&self) -> Result<(), EditError> {
        if self.allows_copying() { Ok(()) } else { Err(EditError::NotPermitted("copying text and images")) }
    }

    /// Page changes (insert, delete, rotate, move, extract) are allowed.
    pub fn allows_assembly(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.assemble())
    }

    /// Redaction marks waiting to be applied.
    pub fn redaction_marks(&self) -> usize {
        self.info.annotations.iter().filter(|a| a.subtype == "Redact").count()
    }

    /// Page marks present (headers and footers, watermarks, backgrounds), for Update/Remove.
    pub fn marks(&self) -> &[MarkKind] {
        &self
            .marker_scan
            .get_or_init(|| {
                self.editor
                    .as_ref()
                    .map(|e| {
                        #[cfg(test)]
                        record_marker_scan();
                        pdfcraft_edit::scan_markers(&e.cos)
                    })
                    .unwrap_or_default()
            })
            .marks
    }

    /// Text and images added with Edit a PDF ▸ Add content (still editable).
    pub fn added(&self) -> &[pdfcraft_edit::Added] {
        &self
            .marker_scan
            .get_or_init(|| {
                self.editor
                    .as_ref()
                    .map(|e| {
                        #[cfg(test)]
                        record_marker_scan();
                        pdfcraft_edit::scan_markers(&e.cos)
                    })
                    .unwrap_or_default()
            })
            .added
    }

    /// Link annotations, for Edit a PDF ▸ Link.
    pub fn links(&self) -> &[pdfcraft_annot::links::LinkItem] {
        self.links.get_or_init(|| {
            self.editor
                .as_ref()
                .map(|e| {
                    #[cfg(test)]
                    record_link_scan();
                    pdfcraft_annot::links::list(&e.cos)
                })
                .unwrap_or_default()
        })
    }

    /// Changes to content and document information are allowed.
    pub fn allows_modification(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.modify())
    }

    /// Filling in form fields is allowed (Table 22, bits 6 and 9).
    pub fn allows_form_filling(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.fill_forms())
    }

    /// Adding and changing comments is allowed (Table 22, bit 6).
    pub fn allows_annotation(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| p.annotate())
    }

    /// The document's security may be changed (Protect, Remove security): it is editable and,
    /// if encrypted, was opened with the owner password.
    pub fn allows_security_change(&self) -> bool {
        self.editable() && self.permissions().is_none_or(|p| unrestricted(&p))
    }

    /// A password restricts the document and it wasn't opened with the owner password: entering
    /// that password ([`Session::unlock`]) lifts the restrictions.
    pub fn can_unlock(&self) -> bool {
        self.editable() && self.permissions().is_some_and(|p| !unrestricted(&p))
    }

    /// A summary of the document's security for Document Properties ▸ Security, including
    /// protection applied in this session (written by the next save).
    pub fn security_summary(&self) -> Option<SecuritySummary> {
        let editor = self.editor.as_ref()?;
        let h = editor.cos.output_handler()?;
        let d = h.dict();
        let stream = d.crypt_filters.iter().find(|(name, _)| *name == d.stm_f).map(|(_, m)| *m);
        let method = match (d.v, stream) {
            (1..=3, _) if d.length_bits <= 40 => "RC4, 40-bit",
            (1..=3, _) | (_, Some(pdfcraft_cos::CryptMethod::Rc4)) => "RC4, 128-bit",
            (_, Some(pdfcraft_cos::CryptMethod::Aes128)) => "AES, 128-bit",
            (_, Some(pdfcraft_cos::CryptMethod::Aes256)) => "AES, 256-bit",
            _ => "Attachments only",
        };
        let pending = editor.cos.encryption_changed();
        let permissions = if pending { pdfcraft_cos::Permissions { bits: h.permissions().bits, owner: false } } else { h.permissions() };
        Some(SecuritySummary { method: method.into(), owner: h.auth() == pdfcraft_cos::Auth::Owner && !pending, permissions, pending })
    }

    /// Comment properties of the comment at `(page, index)` (Comment Properties dialog).
    pub fn comment_props(&self, page: usize, index: usize) -> Option<pdfcraft_annot::Props> {
        self.editor.as_ref().and_then(|e| pdfcraft_annot::props(&e.cos, page, index))
    }

    /// A form field's Appearance-tab look (borders, colours, font).
    pub fn field_look(&self, name: &str) -> Option<pdfcraft_forms::Look> {
        let e = self.editor.as_ref()?;
        let f = self.form.iter().find(|f| f.name == name)?;
        Some(pdfcraft_forms::look(&e.cos, f))
    }

    /// A check box's or radio button's mark (Field Properties ▸ Options), `None` for other fields.
    pub fn field_check_style(&self, name: &str) -> Option<CheckStyle> {
        let e = self.editor.as_ref()?;
        let f = self.form.iter().find(|f| f.name == name)?;
        matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio).then(|| pdfcraft_forms::check_style(&e.cos, f))
    }

    /// Remove Hidden Information: what each category would remove.
    pub fn hidden_info(&self) -> Vec<(Hidden, usize)> {
        self.editor.as_ref().map(|e| pdfcraft_redact::sanitize::scan(&e.cos)).unwrap_or_default()
    }

    /// Verification: faux redactions (opaque covers over extractable text) and redaction
    /// marks that were never applied. Fails closed: any read error is returned, never an
    /// empty list.
    pub fn audit_redactions(&self) -> Result<Vec<pdfcraft_audit::AuditFinding>, String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        pdfcraft_audit::audit_redactions(&editor.cos).map_err(|e| e.to_string())
    }

    /// Every page's media, crop, bleed, trim and art boxes (user space), for Set Page Boxes.
    pub fn page_boxes(&self) -> Vec<[[f64; 4]; 5]> {
        self.editor.as_ref().and_then(|e| pdfcraft_organize::page_boxes(&e.cos).ok()).unwrap_or_default()
    }

    /// Current value of a document-information entry (Title, Author, …).
    pub fn info_value(&self, key: &str) -> Option<String> {
        self.editor.as_ref().and_then(|e| pdfcraft_organize::info(&e.cos, key))
    }

    /// The custom document properties as they are now (name, text), Document Properties ▸ Custom.
    pub fn custom_info(&self) -> Vec<(String, String)> {
        self.editor.as_ref().map(|e| pdfcraft_organize::custom_info(&e.cos)).unwrap_or_default()
    }

    /// The name shown on the tab and window: the document title when the document asks for it
    /// (Initial View ▸ Show: Document Title) and has a real one, else the file name.
    pub fn display_name(&self) -> String {
        self.editor
            .as_ref()
            .filter(|e| pdfcraft_organize::displays_doc_title(&e.cos))
            .and_then(|e| pdfcraft_organize::info(&e.cos, "Title"))
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty() && !is_placeholder_title(t))
            .unwrap_or_else(|| self.name.clone())
    }

    /// Where each saved revision of the file ends (oldest first; `bytes[..end]` is that
    /// revision). Unsaved edits aren't a revision yet.
    pub fn revision_ends(&self) -> Vec<usize> {
        self.editor.as_ref().map(|e| e.cos.revision_ends()).unwrap_or_default()
    }

    /// The file as last opened or saved, without unsaved edits: what an incremental save appends
    /// to. This stays that file when undo has restored an older snapshot.
    pub fn saved_bytes(&self) -> Arc<Vec<u8>> {
        self.editor.as_ref().map_or_else(|| self.bytes.clone(), |e| e.saved.clone())
    }

    /// Notes about damage repaired while opening (Document Properties ▸ Advanced, notices).
    pub fn repair_log(&self) -> Vec<String> {
        self.editor.as_ref().map(|e| e.cos.repair_log().to_vec()).unwrap_or_default()
    }
}

/// A title that names no document: what a web browser writes when it saves a blank page or a
/// pop-up as PDF (`about:blank`), another browser-internal address, or a bare "Untitled". Such a
/// title says less than the file name, so the file name is shown instead. An address is one
/// word: "About: our company" is a real title.
fn is_placeholder_title(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    let address = !t.contains(char::is_whitespace) && ["about:", "blob:", "data:"].iter().any(|p| t.starts_with(p));
    address || t == "untitled"
}

/// The working file plus appearances for comments that have none, and for form fields the file
/// leaves to the viewer (#3470). Signature artwork is preserved; validation badges belong to the
/// viewer and never enter these bytes, which image/text exports and Print as Image also consume.
fn display_bytes(editor: Option<&mut Editor>, bytes: &Arc<Vec<u8>>) -> Arc<Vec<u8>> {
    let Some(e) = editor else { return bytes.clone() };
    let comments = pdfcraft_annot::with_missing_appearances_cached(&e.cos, &mut e.appearances);
    let fields = pdfcraft_forms::with_field_appearances_cached(comments.as_ref().unwrap_or(&e.cos), &mut e.field_appearances);
    let Some(display) = fields.or(comments) else {
        return bytes.clone();
    };
    match write_incremental(&display, &SaveOptions::default()) {
        Ok(written) => Arc::new(written),
        Err(err) => {
            e.cos.note_repair(format!("missing comment or field appearances could not be drawn ({err})"));
            bytes.clone()
        }
    }
}

/// `comments` (the display copy with comment appearances, if it needed any; else the document
/// itself) with its form fields drawn from their values where the file leaves that to the viewer.
fn with_field_appearances(comments: Option<pdfcraft_cos::Document>, doc: &pdfcraft_cos::Document) -> Option<pdfcraft_cos::Document> {
    pdfcraft_forms::with_field_appearances(comments.as_ref().unwrap_or(doc)).or(comments)
}

/// The reader shows the same pages in `a` and `b`: count, displayed size and rotation.
fn same_pages(a: &[pdfcraft_render::PageInfo], b: &[pdfcraft_render::PageInfo]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| (x.width - y.width).abs() < 0.01 && (x.height - y.height).abs() < 0.01 && x.rotation == y.rotation)
}

/// The copy PDFCraft displays when opening `bytes`: with appearances drawn, in memory only, for
/// comments that have none and form fields the file leaves to the viewer. Render paths outside the app, such as the CLI, use this so their page
/// images agree with the app and independent viewers. It never changes the source document, and an
/// unparseable file, a write failure or a parser panic falls back to the original bytes.
pub fn display_bytes_for(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Arc<Vec<u8>> {
    guard(|| {
        pdfcraft_cos::Document::open_with_password(bytes.clone(), password)
            .ok()
            .and_then(|cos| with_field_appearances(pdfcraft_annot::with_missing_appearances(&cos), &cos))
            .and_then(|display| write_incremental(&display, &SaveOptions::default()).ok())
            .map(Arc::new)
    })
    .ok()
    .flatten()
    .unwrap_or(bytes)
}

/// The password to give the renderer (`inspect`, `PageRenderer`) for `bytes` opened with
/// `password`. The renderer authenticates only the user password of R2–R4 files, so for their
/// owner password this is the user password that owner authentication recovers; otherwise it is
/// `password` itself. Render paths outside a [`Session`], such as the CLI, use this so the owner
/// password works there as it does in `doc_open`. A file that doesn't open with `password` keeps
/// it, so the renderer reports the same error it always did.
pub fn render_password_for(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Option<String> {
    let password = password?;
    let recovered =
        guard(|| pdfcraft_cos::Document::open_with_password(bytes, Some(password)).ok().and_then(|d| recovered_render_password(&d))).ok().flatten();
    Some(recovered.unwrap_or_else(|| password.to_owned()))
}

/// The user password owner authentication recovered from an R2–R4 `cos`, as the renderer takes it.
fn recovered_render_password(cos: &pdfcraft_cos::Document) -> Option<String> {
    cos.security().and_then(|s| s.recovered_user_password()).map(|user| renderer_password(&user))
}

/// Fails when the renderer still draws text under an applied redaction mark.
///
/// Redaction decides what lies under a mark from the positions its own interpreter computes, and
/// its verify pass reuses them. Where they differ from the renderer's (a standard font without
/// /Widths, whose advances the interpreter approximates; a CMap
/// that it reads differently), a mark drawn over the text as displayed lies over glyphs the
/// interpreter believes are elsewhere: they stay in the file and nothing notices. The renderer is
/// what the user saw when placing the marks, so it is asked. `after` is written as a save would
/// write it, without the marks' own `overlays` (the box and its label are drawn on purpose), its
/// text is extracted, and a glyph still mostly inside a mark fails the edit: the caller keeps
/// the document as it was.
fn shown_under_redactions(
    after: &pdfcraft_cos::Document,
    marks: &[pdfcraft_redact::Mark],
    overlays: &[pdfcraft_cos::ObjRef],
    pages: &[pdfcraft_render::PageInfo],
    password: Option<&str>,
) -> Result<(), EditError> {
    use pdfcraft_cos::Object;
    use pdfcraft_redact::RedactError;
    let mut marked: Vec<usize> = marks.iter().map(|m| m.page).collect();
    marked.sort_unstable();
    marked.dedup();
    if marked.is_empty() {
        return Ok(());
    }
    let mut shown = after.clone();
    let model = pdfcraft_model::pages(&shown);
    for &pi in &marked {
        let Some(page) = model.get(pi) else { continue };
        let Some(Object::Array(list)) = page.dict.get(b"Contents").map(|c| (*shown.resolve(c)).clone()) else { continue };
        let kept: Vec<Object> = list.into_iter().filter(|o| !o.as_ref().is_some_and(|r| overlays.contains(&r))).collect();
        shown.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(kept))).map_err(|e| EditError::Write(e.to_string()))?;
    }
    let bytes = write_incremental(&shown, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?;
    let config = RenderConfig { password: password.map(Arc::from), ..Default::default() };
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), config);
    let mut residue = 0;
    for &pi in &marked {
        let request = pdfcraft_render::RenderRequest { page: pi, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() };
        // A page that cannot be read back cannot be confirmed clean.
        let (Some(info), Some(text)) = (pages.get(pi), renderer.render(request).text) else { return Err(RedactError::Unreadable(pi + 1).into()) };
        let rects: Vec<[f64; 4]> = marks
            .iter()
            .filter(|m| m.page == pi)
            .flat_map(|m| m.rects.iter())
            .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])])
            .collect();
        for glyph in text.glyphs.iter().filter(|g| !g.text.trim().is_empty()) {
            let [x0, y0, x1, y1] = info.view_rect_to_user(glyph.rect).map(f64::from);
            let area = (x1 - x0) * (y1 - y0);
            if area.is_nan() || area <= 0.0 {
                continue;
            }
            // Redaction itself takes any glyph a fifth under a mark; one that is half under it
            // and still drawn was not where redaction thought it was.
            let under = |r: &[f64; 4]| (x1.min(r[2]) - x0.max(r[0])).max(0.0) * (y1.min(r[3]) - y0.max(r[1])).max(0.0) >= 0.5 * area;
            if rects.iter().any(under) {
                residue += 1;
            }
        }
    }
    if residue > 0 {
        return Err(RedactError::Residue(residue).into());
    }
    Ok(())
}

fn render_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8) - 1
}

/// Write `layers`' visibility into the default optional-content configuration of `cos` (a copy
/// made for printing): `/BaseState /ON`, `/ON` the visible layers, `/OFF` the hidden ones. Its
/// other entries (`/Order`, `/RBGroups`, `/AS`…) are kept. A document without `/OCProperties`
/// is left alone.
fn set_default_layer_state(cos: &mut pdfcraft_cos::Document, layers: &[Layer]) {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    let Some(root_ref) = cos.trailer().get(b"Root").and_then(Object::as_ref) else { return };
    let Some(mut root) = cos.get(root_ref).as_dict().cloned() else { return };
    let Some(properties) = root.get(b"OCProperties").cloned() else { return };
    let Some(mut properties) = cos.resolve(&properties).as_dict().cloned() else { return };
    let mut config = properties.get(b"D").and_then(|d| cos.resolve(d).as_dict().cloned()).unwrap_or_else(Dict::new);
    let refs =
        |visible: bool| Object::Array(layers.iter().filter(|l| l.visible == visible).map(|l| Object::Ref(ObjRef::new(l.id.0, l.id.1))).collect());
    config.set(b"BaseState".to_vec(), Object::name("ON"));
    config.set(b"ON".to_vec(), refs(true));
    config.set(b"OFF".to_vec(), refs(false));
    properties.set(b"D".to_vec(), Object::Dict(config));
    root.set(b"OCProperties".to_vec(), Object::Dict(properties));
    cos.set(root_ref, Object::Dict(root));
}

/// Render `doc` with the layer visibility in its `info.layers`.
fn use_layer_choices(doc: &mut Document) {
    let overrides: Vec<(i32, i32, bool)> = doc.info.layers.iter().map(|l| (l.id.0 as i32, l.id.1 as i32, l.visible)).collect();
    doc.config.layers = Arc::new(overrides);
    doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
}

/// Apply a set-layer-visibility action to `layers`, one change at a time, so a toggle flips the
/// state the changes before it left. Returns whether any layer ended up changed.
fn apply_layer_state(layers: &mut [Layer], groups: &[Vec<(u32, u16)>], changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
    let before: Vec<bool> = layers.iter().map(|l| l.visible).collect();
    let index: std::collections::HashMap<(u32, u16), usize> = layers.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
    for &(op, ocg) in changes {
        let Some(layer) = index.get(&ocg).and_then(|&i| layers.get_mut(i)) else { continue };
        let on = match op {
            LayerOp::On => true,
            LayerOp::Off => false,
            LayerOp::Toggle => !layer.visible,
        };
        layer.visible = on;
        // Turning a layer off leaves the rest of its groups alone.
        if on && preserve_rb {
            for other in groups.iter().filter(|g| g.contains(&ocg)).flatten().filter(|&&o| o != ocg) {
                if let Some(l) = index.get(other).and_then(|&i| layers.get_mut(i)) {
                    l.visible = false;
                }
            }
        }
    }
    layers.iter().zip(before).any(|(l, was)| l.visible != was)
}

/// A file to combine: its name (the bookmark title), bytes, and page range (`None`: all).
pub type CombineSource = (String, Arc<Vec<u8>>, Option<String>);

/// What a combine produced: the new PDF bytes, and the fields renamed to keep colliding names
/// separate (old fully-qualified name → new one), in order across the sources.
#[derive(Debug)]
pub struct Combined {
    pub bytes: Arc<Vec<u8>>,
    pub renamed_fields: Vec<(String, String)>,
}

/// A bookmark as the bookmark list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkRow {
    /// Child indices from the top level, 0-based, as the edit functions take them.
    pub path: Vec<usize>,
    pub title: String,
    /// Shown expanded (a positive `/Count`).
    pub open: bool,
    /// The 0-based page it goes to.
    pub page: Option<usize>,
    /// Where on that page it goes ([`pdfcraft_render::DestView::Top`] when it doesn't say).
    pub view: pdfcraft_render::DestView,
}

/// One page of the bookmark list, in outline order.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkPage {
    pub rows: Vec<BookmarkRow>,
    /// Where the next page starts, or `None` at the end.
    pub next: Option<usize>,
    /// Set only when the document's structure could not be read for editing: its listing then
    /// comes from the viewer's copy of the outline, which is shorter than the file's.
    pub truncated: bool,
}

/// The rows of a viewer outline, in outline order, with their paths.
fn outline_rows(items: &[pdfcraft_render::OutlineItem]) -> Vec<BookmarkRow> {
    let mut rows = Vec::new();
    // One entry per open level: its items, and how many of them have been listed.
    let mut levels: Vec<(&[pdfcraft_render::OutlineItem], usize)> = vec![(items, 0)];
    while let Some(&(list, next)) = levels.last() {
        let Some(item) = list.get(next) else {
            levels.pop();
            continue;
        };
        if let Some(top) = levels.last_mut() {
            top.1 += 1;
        }
        let path: Vec<usize> = levels.iter().map(|(_, listed)| listed.saturating_sub(1)).collect();
        rows.push(BookmarkRow { path, title: item.title.clone(), open: item.open, page: item.page, view: item.view });
        if !item.children.is_empty() {
            levels.push((&item.children, 0));
        }
    }
    rows
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

/// Document Properties ▸ Security.
#[derive(Clone, Debug, PartialEq)]
pub struct SecuritySummary {
    /// "AES, 256-bit" etc.
    pub method: String,
    /// Opened with the owner password (no restrictions apply).
    pub owner: bool,
    /// For pending protection: the restrictions as they will apply to others.
    pub permissions: pdfcraft_cos::Permissions,
    /// Set in this session; written by the next save.
    pub pending: bool,
}

/// What printing a protected document allows (Acrobat: Printing allowed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Printing {
    None,
    /// "Low Resolution (150 dpi)".
    Low,
    High,
}

/// What changes a protected document allows (Acrobat: Changes allowed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changes {
    None,
    /// "Inserting, deleting, and rotating pages".
    Pages,
    /// "Filling in form fields and signing existing signature fields".
    FillSign,
    /// "Commenting, filling in form fields, and signing existing signature fields".
    CommentFillSign,
    /// "Any except extracting pages".
    AnyExceptExtract,
}

/// Password protection to apply (Protect Using Password and its Advanced options).
#[derive(Clone, PartialEq)]
pub struct Protection {
    /// Required to open the document (the user password).
    pub open_password: Option<String>,
    /// Required to change security and lift the restrictions below (the owner password).
    pub permissions_password: Option<String>,
    pub printing: Printing,
    pub changes: Changes,
    /// "Enable copying of text, images, and other content".
    pub copy: bool,
    /// "Enable text access for screen reader devices".
    pub accessibility: bool,
    /// Compatibility level; AES-256 (Acrobat X and later) by default.
    pub algorithm: pdfcraft_cos::Algorithm,
    /// `false`: "Encrypt all document contents except metadata".
    pub encrypt_metadata: bool,
}

impl std::fmt::Debug for Protection {
    // Never print passwords (edits end up in logs and error messages).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Protection")
            .field("open_password", &self.open_password.as_ref().map(|_| "…"))
            .field("permissions_password", &self.permissions_password.as_ref().map(|_| "…"))
            .field("printing", &self.printing)
            .field("changes", &self.changes)
            .field("copy", &self.copy)
            .field("accessibility", &self.accessibility)
            .field("algorithm", &self.algorithm)
            .field("encrypt_metadata", &self.encrypt_metadata)
            .finish()
    }
}

impl Default for Protection {
    /// Acrobat's defaults: AES-256, printing and accessibility allowed, no changes, no copying.
    fn default() -> Self {
        Self {
            open_password: None,
            permissions_password: None,
            printing: Printing::High,
            changes: Changes::None,
            copy: false,
            accessibility: true,
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            encrypt_metadata: true,
        }
    }
}

impl Protection {
    /// The `/P` permission bits (ISO 32000-2 Table 22), as Acrobat maps its choices.
    pub fn permission_bits(&self) -> i32 {
        let bit = |n: u32| 1i32 << (n - 1);
        if self.permissions_password.is_none() {
            return -1; // nothing restricted without a permissions password
        }
        let mut p = 0;
        match self.printing {
            Printing::None => {}
            Printing::Low => p |= bit(3),
            Printing::High => p |= bit(3) | bit(12),
        }
        p |= match self.changes {
            Changes::None => 0,
            Changes::Pages => bit(11),
            Changes::FillSign => bit(9),
            Changes::CommentFillSign => bit(6) | bit(9),
            Changes::AnyExceptExtract => bit(4) | bit(6) | bit(9),
        };
        if self.copy {
            p |= bit(5) | bit(10);
        }
        if self.accessibility {
            p |= bit(10);
        }
        p
    }

    fn validate(&self) -> Result<(), EditError> {
        let bad = |m: &str| Err(EditError::Protection(m.to_string()));
        match (&self.open_password, &self.permissions_password) {
            (None, None) => return bad("enter a password to open the document, a permissions password, or both"),
            (Some(a), Some(b)) if a == b => return bad("the open password and the permissions password must be different"),
            _ => {}
        }
        if [&self.open_password, &self.permissions_password].into_iter().flatten().any(|p| p.is_empty()) {
            return bad("passwords can't be empty");
        }
        if self.algorithm != pdfcraft_cos::Algorithm::Aes256
            && [&self.open_password, &self.permissions_password].into_iter().flatten().any(|p| p.chars().any(|c| !(' '..='~').contains(&c)))
        {
            return bad("this compatibility level supports only plain ASCII passwords; use AES-256 (Acrobat X and later)");
        }
        // Encryption keys on a password as `effective_password` gives it (AES-256: SASLprep,
        // which drops soft hyphens and folds compatibility forms; older levels: the first 32
        // bytes, padded). One that comes out as the empty password opens the file without asking,
        // and an open password that comes out as the permissions password lifts the restrictions.
        let effective = |p: &str| pdfcraft_cos::effective_password(self.algorithm, p);
        let (open, permissions) = (self.open_password.as_deref().map(effective), self.permissions_password.as_deref().map(effective));
        let empty = effective("");
        if open.iter().chain(&permissions).any(|p| *p == empty) {
            return bad("passwords can't be empty or made only of characters that passwords ignore, such as soft hyphens");
        }
        if open.is_some() && open == permissions {
            return bad("the open password and the permissions password must be different (PDF readers treat these two as the same password)");
        }
        Ok(())
    }
}

/// Edits that can be applied to a document. Page indices are 0-based.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    RotatePages {
        pages: Vec<usize>,
        degrees: i64,
    },
    DeletePages {
        pages: Vec<usize>,
    },
    MovePages {
        pages: Vec<usize>,
        to: usize,
    },
    /// `pages` reversed among the positions they occupy.
    ReversePages {
        pages: Vec<usize>,
    },
    InsertBlankPage {
        at: usize,
        width: f64,
        height: f64,
    },
    /// Copies of `pages` inserted at `at` (an index in the page order as it is now), or after
    /// the last of them when `at` is `None`.
    DuplicatePages {
        pages: Vec<usize>,
        at: Option<usize>,
    },
    /// Replace Pages: the content of `pages` is replaced by `src_pages` of another PDF; the
    /// pages' links, comments and bookmarks stay.
    ReplacePages {
        pages: Vec<usize>,
        name: String,
        bytes: Arc<Vec<u8>>,
        src_pages: Vec<usize>,
    },
    /// Set Page Boxes / Crop: set (or reset) one box on `pages`.
    SetPageBox {
        pages: Vec<usize>,
        which: PageBox,
        spec: BoxSpec,
    },
    /// Set or remove the page transition and display time of pages (ISO 32000-2 §12.4.4),
    /// Organize ▸ Page Transitions.
    SetTransition {
        pages: Vec<usize>,
        spec: TransitionSpec,
    },
    SetInfo {
        key: String,
        value: String,
    },
    /// Insert pages from another PDF (all pages when `pages` is `None`) at position `at`.
    InsertPagesFrom {
        name: String,
        bytes: Arc<Vec<u8>>,
        pages: Option<Vec<usize>>,
        at: usize,
        /// How a form field whose name the document already uses is kept separate.
        field_conflicts: pdfcraft_organize::FieldConflicts,
    },
    /// Add a bookmark to `page` as child `index` of the bookmark at `parent` (`[]` = top level).
    /// It goes to the user-space point (`left`, `top`) of the page at the reader's zoom, and
    /// either may be `None` to keep the reader's position that way (both `None` is the page top).
    /// A coordinate that is not finite fails the edit.
    AddBookmark {
        parent: Vec<usize>,
        index: usize,
        title: String,
        page: usize,
        left: Option<f64>,
        top: Option<f64>,
    },
    RenameBookmark {
        path: Vec<usize>,
        title: String,
    },
    /// Delete a bookmark and the bookmarks under it.
    DeleteBookmark {
        path: Vec<usize>,
    },
    /// Move a bookmark to child `index` of `to_parent` (indices after removing it).
    MoveBookmark {
        from: Vec<usize>,
        to_parent: Vec<usize>,
        index: usize,
    },
    /// Point a bookmark at another page.
    SetBookmarkPage {
        path: Vec<usize>,
        page: usize,
    },
    /// Bookmarks for the tagged headings (H, H1–H6), nested by level under a new first
    /// "Untitled" bookmark (New Bookmarks from Structure).
    BookmarksFromStructure,
    /// Label pages `from..=to` (0-based) as Acrobat's "Number pages" does; later pages keep their labels.
    NumberPages {
        from: usize,
        to: usize,
        style: pdfcraft_organize::LabelStyle,
        prefix: String,
        first: u32,
    },
    /// Add a calibrated distance, perimeter or area annotation.
    AddMeasurement(measure::NewMeasurement),
    /// Store a drawing scale for a rectangular viewport (PDF user space).
    SetMeasurementScale {
        page: usize,
        bbox: [f64; 4],
        name: String,
        scale: measure::Scale,
    },
    /// Add a comment (sticky note, highlight, shape, drawing, text box…).
    AddAnnotation(NewAnnotation),
    /// A custom stamp from a picture file (a PDF page or an image) on `page`. A zero-size
    /// `rect` is a point: the stamp is centred there at its natural size (at most 200 pt).
    AddCustomStamp {
        page: usize,
        rect: [f64; 4],
        name: String,
        file: MarkFile,
        author: String,
    },
    /// Delete the comment at `index` in the page's `/Annots`, with its pop-up and replies.
    DeleteAnnotation {
        page: usize,
        index: usize,
    },
    SetAnnotationContents {
        page: usize,
        index: usize,
        text: String,
    },
    ReplyToAnnotation {
        page: usize,
        index: usize,
        text: String,
        author: String,
    },
    /// Acrobat's "Set status" (a state reply by `author`).
    SetAnnotationStatus {
        page: usize,
        index: usize,
        state: ReviewState,
        author: String,
    },
    /// The Eraser: rub out the parts of drawing `(page, index)` near `path` (user space).
    EraseInk {
        page: usize,
        index: usize,
        path: Vec<[f64; 2]>,
        radius: f64,
    },
    /// Replace Text: strike out `quads` and add a grouped caret holding `text`.
    ReplaceText {
        page: usize,
        quads: Vec<[f64; 8]>,
        text: String,
        author: String,
        strike: Style,
        caret: Style,
    },
    /// Acrobat's "Mark with checkmark" (a hidden `/StateModel /Marked` reply by `author`).
    MarkAnnotation {
        page: usize,
        index: usize,
        marked: bool,
        author: String,
    },
    /// Properties ▸ Locked.
    LockAnnotation {
        page: usize,
        index: usize,
        locked: bool,
    },
    MoveAnnotation {
        page: usize,
        index: usize,
        dx: f64,
        dy: f64,
    },
    /// Reshape a line, connected lines, a polygon or a cloud: its points in user space (a line's
    /// two ends, the others' corners).
    ReshapeAnnotation {
        page: usize,
        index: usize,
        points: Vec<[f64; 2]>,
    },
    /// Resize a rectangle, oval or text box.
    ResizeAnnotation {
        page: usize,
        index: usize,
        rect: [f64; 4],
    },
    StyleAnnotation {
        page: usize,
        index: usize,
        color: Option<Rgb>,
        opacity: Option<f64>,
        width: Option<f64>,
        /// Line or polyline: two endings. Callout: one. `None` leaves `/LE` unchanged.
        endings: Option<Vec<pdfcraft_annot::LineEnding>>,
    },
    /// Fill a rectangle, oval or polygon comment, or remove its fill (`None`).
    FillAnnotation {
        page: usize,
        index: usize,
        fill: Option<Rgb>,
    },
    /// Change the font size (pt) of a text box, typewriter text or callout.
    FontSizeAnnotation {
        page: usize,
        index: usize,
        size: f64,
    },
    /// Comment properties ▸ General / note icon.
    SetAnnotationInfo {
        page: usize,
        index: usize,
        author: Option<String>,
        subject: Option<String>,
        icon: Option<NoteIcon>,
    },
    /// Fill in a form field.
    SetFieldValue {
        name: String,
        value: FieldValue,
    },
    /// Clear form: the named fields (all when `None`) back to their defaults.
    ResetForm {
        names: Option<Vec<String>>,
    },
    /// Prepare a form: add a field on a page (`rect` in PDF user space); `name` defaults to
    /// Acrobat's next free name ("Text1", "Check Box2", …).
    AddField {
        page: usize,
        rect: [f64; 4],
        kind: NewField,
        name: Option<String>,
    },
    /// Field Properties ▸ General / Options.
    SetFieldProps {
        name: String,
        props: Box<FieldProps>,
    },
    /// An image field (or any button): show this image file (PNG, JPEG, TIFF, GIF, BMP).
    SetFieldImage {
        name: String,
        image: Arc<Vec<u8>>,
    },
    /// Duplicate a field onto other pages (same field, shared value).
    DuplicateField {
        name: String,
        pages: Vec<usize>,
    },
    DeleteField {
        name: String,
    },
    /// Page properties ▸ Tab order, for pages (0-based).
    SetTabOrder {
        pages: Vec<usize>,
        order: TabOrder,
    },
    /// Document Properties ▸ Initial View (and the language and binding).
    SetInitialView(Box<InitialView>),
    /// Build a tagged structure tree for an untagged, text-only document.
    AutoTagText,
    /// Add alternate text: set (or clear, with `None`) a figure's alternate text. `figure` is
    /// the figure element's object number (from `Document::figures`).
    SetAltText {
        figure: u32,
        alt: Option<String>,
    },
    /// Add `/Form` tags and object references for all widgets in an untagged AcroForm.
    AutoTagFormFields,
    /// Change one property on a reachable indirect structure tag.
    SetTagProperty {
        object: pdfcraft_cos::ObjRef,
        property: a11y::TagProperty,
        value: Option<String>,
    },
    /// Move a structure tag before or after a direct sibling in reading order.
    MoveTag {
        object: pdfcraft_cos::ObjRef,
        anchor: pdfcraft_cos::ObjRef,
        placement: a11y::TagPlacement,
    },
    /// Add alternate text ▸ Decorative figure: the figure's content becomes an artifact.
    MarkDecorative {
        figure: u32,
    },
    /// Tags panel ▸ Mark as artifact: retire a leaf tag and mark its content as an artifact.
    MarkTagArtifact {
        object: pdfcraft_cos::ObjRef,
    },
    /// Document JavaScripts: add, replace (`script`) or remove (`None`) the document-level
    /// script `name`.
    SetDocumentScript {
        name: String,
        script: Option<String>,
    },
    /// Field Properties ▸ a custom script: set or remove field `name`'s JavaScript for `event`
    /// (keystroke, format, validate, calculate or mouse_up).
    SetFieldScript {
        name: String,
        event: String,
        script: Option<String>,
    },
    /// Standards ▸ Save as PDF/A: fix what can be fixed for `level` (metadata, output intent,
    /// forbidden actions, annotation flags, …).
    ConvertPdfA {
        level: pdfcraft_preflight::Level,
    },
    /// Remove what identifies the document as meeting `standard` (PDF/A or PDF/X).
    RemoveStandardId {
        standard: Standard,
    },
    /// What a script (button or console) changed in form fields: values, read-only, required
    /// and visibility.
    ApplyScriptChanges {
        changes: Vec<pdfcraft_forms::FieldChange>,
    },
    /// An XFA form's scripted event (a button's `click`, say) on the object at `som`: what the
    /// script changes (values, rows, visibility) is applied and the form laid out again.
    XfaEvent {
        som: String,
        activity: String,
    },
    /// Scan & OCR ▸ Recognize text: put recognised words on `page` as invisible text over the
    /// image (from [`ocr::OcrJob::run`]).
    AddOcrText {
        page: usize,
        words: Vec<pdfcraft_ocr::PlacedWord>,
    },
    /// Edit a PDF ▸ Edit text: replace the text of line `line` (from `Document::text_lines`) on
    /// `page`, in its own font when it can show it, else in Helvetica.
    EditTextLine {
        page: usize,
        line: usize,
        text: String,
    },
    /// Edit a PDF ▸ an existing image on `page` (index from `Document::page_images`).
    EditPageImage {
        page: usize,
        index: usize,
        change: ImageEdit,
    },
    /// Move mixed existing and added objects together. References resolve before any change;
    /// offset is a user-space vector. No retyping, reflow or font substitution is involved.
    MoveObjects {
        page: usize,
        objects: Vec<ObjectTarget>,
        offset: [f64; 2],
    },
    /// Edit text in a paragraph box: replace paragraph `block` (from `Document::text_blocks`),
    /// rewrapped to the box's width.
    EditTextBlock {
        page: usize,
        block: usize,
        text: String,
        /// Formatting changes (font, size, colour, alignment); default keeps the paragraph's.
        style: pdfcraft_edit::BlockStyle,
    },
    /// An explicit open-licensed full font program, completing subsets or selected by a user.
    RegisterTextFont {
        page: usize,
        bytes: Arc<Vec<u8>>,
    },
    /// Order tabs manually: move a field one place earlier or later on its page.
    MoveInTabOrder {
        name: String,
        earlier: bool,
    },
    /// Set the calculation order: these calculated fields first, in this order, then the others
    /// in their current order (see [`Document::calculation_order`]). The form recalculates.
    SetCalculationOrder {
        names: Vec<String>,
    },
    /// Add a header and footer (with `replace`, existing ones on those pages go first).
    AddHeaderFooter {
        pages: Vec<usize>,
        settings: HeaderFooter,
        replace: bool,
    },
    /// `file`: a picture from a file (an image, or a page of a PDF) instead of text.
    AddWatermark {
        pages: Vec<usize>,
        settings: Watermark,
        replace: bool,
        file: Option<MarkFile>,
    },
    /// `file`: a picture from a file instead of a colour.
    AddBackground {
        pages: Vec<usize>,
        settings: Background,
        replace: bool,
        file: Option<MarkFile>,
    },
    /// Remove every mark of a kind from every page.
    RemoveMarks {
        kind: MarkKind,
    },
    /// Edit a PDF ▸ Add content ▸ Text (`text.rect` in display space).
    AddText {
        page: usize,
        text: AddedText,
    },
    /// Edit a PDF ▸ Add content ▸ Image: an image file placed at `rect` (display space), or at
    /// its natural size centred on the page (shrunk to fit).
    AddImage {
        page: usize,
        rect: Option<[f64; 4]>,
        name: String,
        bytes: Arc<Vec<u8>>,
    },
    /// Move, resize, retype or reformat an added item (`index` among the page's added items).
    UpdateContent {
        page: usize,
        index: usize,
        content: AddedContent,
    },
    DeleteContent {
        page: usize,
        index: usize,
    },
    /// Replace an added image with another file (keeping its box and transforms).
    ReplaceImage {
        page: usize,
        index: usize,
        name: String,
        bytes: Arc<Vec<u8>>,
    },
    /// Apply redaction marks (all, or those on `pages`): remove what they cover for good.
    ApplyRedactions {
        pages: Option<Vec<usize>>,
    },
    /// Remove redaction marks without applying them.
    ClearRedactions,
    /// Edit a PDF ▸ Link: a new link over `rect` (user space).
    AddLink {
        page: usize,
        rect: [f64; 4],
        action: LinkAction,
        style: LinkStyle,
    },
    /// Link Properties (`index` in the page's `/Annots`).
    SetLink {
        page: usize,
        index: usize,
        rect: Option<[f64; 4]>,
        action: Option<LinkAction>,
        style: Option<LinkStyle>,
    },
    DeleteLink {
        page: usize,
        index: usize,
    },
    /// Remove all links (on `pages`, or everywhere).
    RemoveLinks {
        pages: Option<Vec<usize>>,
    },
    /// Create links from URLs in the text: (page, line rects in user space, URI).
    AddLinks {
        links: Vec<(usize, Vec<[f64; 4]>, String)>,
        style: LinkStyle,
    },
    /// Import comments and/or form data (XFDF, FDF, XML, CSV, tab-delimited text).
    ImportData {
        name: String,
        bytes: Arc<Vec<u8>>,
    },
    /// Remove Hidden Information: the chosen categories.
    RemoveHidden {
        which: Vec<Hidden>,
    },
    /// Sanitize Document: every category, then a full rewrite on save.
    Sanitize,
    /// An XFA form made an ordinary (static) PDF form: the XFA packets, `/NeedsRendering` and
    /// PdfCraft's private keys go, the pages as laid out and the fields with their values
    /// stay, so every viewer shows the same form and none lays it out again. Its scripts no
    /// longer run.
    XfaToStatic,
    /// Flatten comments and/or form fields on every page into page content.
    Flatten {
        comments: bool,
        fields: bool,
    },
    /// Bake Fill & Sign text, marks and signatures into the page. Other comments and fields stay.
    /// A no-op (no undo step) when the document has none.
    FlattenFillSign,
    /// Protect with passwords and permissions (written by the next save, which is a full rewrite).
    Protect(Protection),
    /// Remove password security (needs the owner password).
    RemoveProtection,
    /// Several edits applied as one undoable step (all or nothing).
    Batch {
        label: String,
        edits: Vec<Edit>,
    },
}

/// What an edit is called in the Edit menu and in the history ("Undo Rotate pages"), in two parts:
/// a whole English wording, and the one value the document itself supplies for the wording's hole
/// (a file name, a field name). An interface in another language looks the wording up and says the
/// label with [`said`](Self::said); [`as_str`](Self::as_str) is the English, said the same way, so
/// the two cannot drift apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// The wording, with one named hole (`{name}`) where `value` goes. Listed in [`EDIT_LABELS`].
    pub wording: &'static str,
    /// What the document supplies, when the wording has a hole for it.
    pub value: Option<String>,
    text: String,
}

impl Label {
    /// A wording with nothing to fill in.
    pub fn of(wording: &'static str) -> Self {
        Self { wording, value: None, text: wording.to_string() }
    }

    /// A wording whose hole the document fills in.
    pub fn with(wording: &'static str, value: impl Into<String>) -> Self {
        let mut label = Self { wording, value: Some(value.into()), text: String::new() };
        label.text = label.said(wording);
        label
    }

    /// The label read with this wording: the interface's translation of [`wording`](Self::wording),
    /// or the English wording itself. What the document supplies goes in as it stands and is not
    /// read again, so a name with braces in it stays whole.
    pub fn said(&self, wording: &str) -> String {
        let Some(value) = &self.value else { return wording.to_owned() };
        // `{` and `}` are ASCII, so these are char boundaries.
        match (wording.find('{'), wording.find('}')) {
            (Some(open), Some(close)) if open < close => {
                let mut said = String::with_capacity(wording.len() + value.len());
                said.push_str(&wording[..open]);
                said.push_str(value);
                said.push_str(&wording[close + 1..]);
                said
            }
            // A translation that lost its hole is said as it stands rather than losing the value.
            _ => wording.to_owned(),
        }
    }

    /// The label as English reads it.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Every wording [`Edit::label`] can produce, for an interface to translate. `Edit::Batch` is not
/// here: its wording is only the hole, because a batch carries its name as text (see
/// [`BATCH_LABELS`]).
pub const EDIT_LABELS: &[&str] = &[
    "Rotate page",
    "Change tag properties",
    "Rotate pages",
    "Delete page",
    "Delete pages",
    "Move page",
    "Move pages",
    "Reverse pages",
    "Insert blank page",
    "Duplicate page",
    "Duplicate pages",
    "Replace page",
    "Replace pages",
    "Crop page",
    "Crop pages",
    "Set page boxes",
    "Set page transitions",
    "Change Title",
    "Change Author",
    "Change Subject",
    "Change Keywords",
    "Change {key}",
    "Insert pages from {name}",
    "Insert pages from {name} — 1 field renamed",
    "Insert pages from {name} — fields renamed",
    "Add bookmark",
    "Rename bookmark",
    "Delete bookmark",
    "Move bookmark",
    "Set bookmark destination",
    "New bookmarks from structure",
    "Number pages",
    "Measure distance",
    "Measure perimeter",
    "Measure area",
    "Set measurement scale",
    "Add sticky note",
    "Add highlight",
    "Add underline",
    "Add strikethrough",
    "Add squiggly underline",
    "Add rectangle",
    "Add oval",
    "Add arrow",
    "Add line",
    "Add drawing",
    "Add text box",
    "Add checkmark",
    "Add cross",
    "Add dot",
    "Add signature",
    "Add redaction mark",
    "Add cloud",
    "Add polygon",
    "Add connected lines",
    "Add callout",
    "Add inserted text",
    "Add file attachment",
    "Add stamp",
    "Delete comment",
    "Edit comment",
    "Reply",
    "Set status None",
    "Set status Accepted",
    "Set status Rejected",
    "Set status Cancelled",
    "Set status Completed",
    "Replace text",
    "Erase",
    "Mark with checkmark",
    "Remove checkmark",
    "Lock comment",
    "Unlock comment",
    "Move comment",
    "Reshape comment",
    "Resize comment",
    "Change comment properties",
    "Fill in {name}",
    "Set the image of {name}",
    "Clear form",
    "Add field",
    "Change field properties",
    "Delete field",
    "Duplicate field",
    "Set tab order",
    "Set calculation order",
    "Change initial view",
    "Set alternate text",
    "Mark figure as decorative",
    "Recognize text",
    "Run JavaScript",
    "Run form script",
    "Save as {level}",
    "Remove {standard} identification",
    "Edit script of {name}",
    "Delete document JavaScript",
    "Edit document JavaScript",
    "Edit text",
    "Load document font",
    "Move object",
    "Move objects",
    "Move image",
    "Rotate image",
    "Flip image",
    "Replace image",
    "Delete image",
    "Add header & footer",
    "Update header & footer",
    "Add watermark",
    "Update watermark",
    "Add background",
    "Update background",
    "Remove header & footer",
    "Remove watermark",
    "Remove background",
    "Add text",
    "Add image",
    "Edit content",
    "Delete content",
    "Apply redactions",
    "Remove redaction marks",
    "Remove hidden information",
    "Import {name}",
    "Add link",
    "Change link properties",
    "Delete link",
    "Remove all links",
    "Create link",
    "Create links",
    "Sanitize document",
    "Flatten comments",
    "Flatten form fields",
    "Flatten",
    "Flatten Fill & Sign",
    "Protect with password",
    "Remove security",
];

/// The names of the batches built below L7, which an interface shows in the same places as
/// [`EDIT_LABELS`] and so translates the same way. A batch built in the interface itself arrives
/// already worded in the interface's language.
pub const BATCH_LABELS: &[&str] =
    &["Set document title", "Mark differences", "Detect form fields", "Run JavaScript", "Recognize text", field_table::LABEL];

impl Edit {
    /// Label for the Edit menu and history ("Undo Rotate pages").
    pub fn label(&self) -> Label {
        match self {
            Edit::RotatePages { pages, .. } => plural("Rotate page", "Rotate pages", pages.len()),
            Edit::DeletePages { pages } => plural("Delete page", "Delete pages", pages.len()),
            Edit::MovePages { pages, .. } => plural("Move page", "Move pages", pages.len()),
            Edit::ReversePages { .. } => Label::of("Reverse pages"),
            Edit::InsertBlankPage { .. } => Label::of("Insert blank page"),
            Edit::DuplicatePages { pages, .. } => plural("Duplicate page", "Duplicate pages", pages.len()),
            Edit::ReplacePages { pages, .. } => plural("Replace page", "Replace pages", pages.len()),
            Edit::SetPageBox { pages, which: PageBox::Crop, .. } => plural("Crop page", "Crop pages", pages.len()),
            Edit::SetPageBox { .. } => Label::of("Set page boxes"),
            Edit::SetTransition { .. } => Label::of("Set page transitions"),
            // The four `/Info` keys PdfCraft itself offers get a wording of their own, so each can
            // be said in another language; any other key is the document's and goes in as it stands.
            Edit::SetInfo { key, .. } => match key.as_str() {
                "Title" => Label::of("Change Title"),
                "Author" => Label::of("Change Author"),
                "Subject" => Label::of("Change Subject"),
                "Keywords" => Label::of("Change Keywords"),
                _ => Label::with("Change {key}", key.as_str()),
            },
            Edit::InsertPagesFrom { name, .. } => Label::with("Insert pages from {name}", name.as_str()),
            Edit::AddBookmark { .. } => Label::of("Add bookmark"),
            Edit::RenameBookmark { .. } => Label::of("Rename bookmark"),
            Edit::DeleteBookmark { .. } => Label::of("Delete bookmark"),
            Edit::MoveBookmark { .. } => Label::of("Move bookmark"),
            Edit::SetBookmarkPage { .. } => Label::of("Set bookmark destination"),
            Edit::BookmarksFromStructure => Label::of("New bookmarks from structure"),
            Edit::NumberPages { .. } => Label::of("Number pages"),
            // What was measured is part of the wording too, for the same reason.
            Edit::AddMeasurement(m) => Label::of(match m.kind {
                pdfcraft_measure::Kind::Distance => "Measure distance",
                pdfcraft_measure::Kind::Perimeter => "Measure perimeter",
                pdfcraft_measure::Kind::Area => "Measure area",
            }),
            Edit::SetMeasurementScale { .. } => Label::of("Set measurement scale"),
            Edit::AddAnnotation(a) => Label::of(add_annotation(&a.shape)),
            Edit::AddCustomStamp { .. } => Label::of("Add stamp"),
            Edit::DeleteAnnotation { .. } => Label::of("Delete comment"),
            Edit::SetAnnotationContents { .. } => Label::of("Edit comment"),
            Edit::ReplyToAnnotation { .. } => Label::of("Reply"),
            // The state's name is part of the wording: a language may need to agree it with the rest.
            Edit::SetAnnotationStatus { state, .. } => Label::of(match state {
                ReviewState::None => "Set status None",
                ReviewState::Accepted => "Set status Accepted",
                ReviewState::Rejected => "Set status Rejected",
                ReviewState::Cancelled => "Set status Cancelled",
                ReviewState::Completed => "Set status Completed",
            }),
            Edit::ReplaceText { .. } => Label::of("Replace text"),
            Edit::EraseInk { .. } => Label::of("Erase"),
            Edit::MarkAnnotation { marked: true, .. } => Label::of("Mark with checkmark"),
            Edit::MarkAnnotation { .. } => Label::of("Remove checkmark"),
            Edit::LockAnnotation { locked: true, .. } => Label::of("Lock comment"),
            Edit::LockAnnotation { .. } => Label::of("Unlock comment"),
            Edit::MoveAnnotation { .. } => Label::of("Move comment"),
            Edit::ReshapeAnnotation { .. } => Label::of("Reshape comment"),
            Edit::ResizeAnnotation { .. } => Label::of("Resize comment"),
            Edit::StyleAnnotation { .. } | Edit::FillAnnotation { .. } | Edit::FontSizeAnnotation { .. } | Edit::SetAnnotationInfo { .. } => {
                Label::of("Change comment properties")
            }
            Edit::SetFieldValue { name, .. } => Label::with("Fill in {name}", name.as_str()),
            Edit::SetFieldImage { name, .. } => Label::with("Set the image of {name}", name.as_str()),
            Edit::ResetForm { .. } => Label::of("Clear form"),
            Edit::AddField { .. } => Label::of("Add field"),
            Edit::SetFieldProps { .. } => Label::of("Change field properties"),
            Edit::DeleteField { .. } => Label::of("Delete field"),
            Edit::DuplicateField { .. } => Label::of("Duplicate field"),
            Edit::SetTabOrder { .. } | Edit::MoveInTabOrder { .. } => Label::of("Set tab order"),
            Edit::SetCalculationOrder { .. } => Label::of("Set calculation order"),
            Edit::SetInitialView(_) => Label::of("Change initial view"),
            Edit::AutoTagText => Label::of("Automatically tag text"),
            Edit::SetAltText { .. } => Label::of("Set alternate text"),
            Edit::AutoTagFormFields => Label::of("Tag form fields"),
            Edit::SetTagProperty { .. } => Label::of("Change tag properties"),
            Edit::MoveTag { .. } => Label::of("Move object"),
            Edit::MarkDecorative { .. } => Label::of("Mark figure as decorative"),
            Edit::MarkTagArtifact { .. } => Label::of("Mark content as artifact"),
            Edit::AddOcrText { .. } => Label::of("Recognize text"),
            Edit::ApplyScriptChanges { .. } => Label::of("Run JavaScript"),
            Edit::XfaEvent { .. } => Label::of("Run form script"),
            // The level is the standard's own name ("PDF/A-2b"), so it goes in as it stands.
            Edit::ConvertPdfA { level } => Label::with("Save as {level}", level.label()),
            Edit::RemoveStandardId { standard } => Label::with("Remove {standard} identification", standard.label()),
            Edit::SetFieldScript { name, .. } => Label::with("Edit script of {name}", name.as_str()),
            Edit::SetDocumentScript { script: None, .. } => Label::of("Delete document JavaScript"),
            Edit::SetDocumentScript { .. } => Label::of("Edit document JavaScript"),
            Edit::EditTextLine { .. } | Edit::EditTextBlock { .. } => Label::of("Edit text"),
            Edit::RegisterTextFont { .. } => Label::of("Load document font"),
            Edit::MoveObjects { objects, .. } => plural("Move object", "Move objects", objects.len()),
            Edit::EditPageImage { change, .. } => Label::of(match change {
                ImageEdit::Move(_) => "Move image",
                ImageEdit::Rotate(_) | ImageEdit::RotateBy(_) => "Rotate image",
                ImageEdit::Crop(_) => "Crop image",
                ImageEdit::Flip { .. } => "Flip image",
                ImageEdit::Replace { .. } => "Replace image",
                ImageEdit::Delete => "Delete image",
            }),
            Edit::AddHeaderFooter { replace: false, .. } => Label::of("Add header & footer"),
            Edit::AddHeaderFooter { .. } => Label::of("Update header & footer"),
            Edit::AddWatermark { replace: false, .. } => Label::of("Add watermark"),
            Edit::AddWatermark { .. } => Label::of("Update watermark"),
            Edit::AddBackground { replace: false, .. } => Label::of("Add background"),
            Edit::AddBackground { .. } => Label::of("Update background"),
            Edit::RemoveMarks { kind: MarkKind::HeaderFooter } => Label::of("Remove header & footer"),
            Edit::RemoveMarks { kind: MarkKind::Watermark } => Label::of("Remove watermark"),
            Edit::RemoveMarks { kind: MarkKind::Background } => Label::of("Remove background"),
            Edit::AddText { .. } => Label::of("Add text"),
            Edit::AddImage { .. } => Label::of("Add image"),
            Edit::UpdateContent { .. } => Label::of("Edit content"),
            Edit::DeleteContent { .. } => Label::of("Delete content"),
            Edit::ReplaceImage { .. } => Label::of("Replace image"),
            Edit::ApplyRedactions { .. } => Label::of("Apply redactions"),
            Edit::ClearRedactions => Label::of("Remove redaction marks"),
            Edit::RemoveHidden { .. } => Label::of("Remove hidden information"),
            Edit::ImportData { name, .. } => Label::with("Import {name}", name.as_str()),
            Edit::AddLink { .. } => Label::of("Add link"),
            Edit::SetLink { .. } => Label::of("Change link properties"),
            Edit::DeleteLink { .. } => Label::of("Delete link"),
            Edit::RemoveLinks { .. } => Label::of("Remove all links"),
            Edit::AddLinks { links, .. } => plural("Create link", "Create links", links.len()),
            Edit::Sanitize => Label::of("Sanitize document"),
            Edit::XfaToStatic => Label::of("Convert to a static form"),
            Edit::Flatten { comments: true, fields: false } => Label::of("Flatten comments"),
            Edit::Flatten { comments: false, fields: true } => Label::of("Flatten form fields"),
            Edit::Flatten { .. } => Label::of("Flatten"),
            Edit::FlattenFillSign => Label::of("Flatten Fill & Sign"),
            Edit::Protect(_) => Label::of("Protect with password"),
            Edit::RemoveProtection => Label::of("Remove security"),
            // A batch carries its own name, already worded by whoever built it.
            // A batch carries its name as text, so the wording is only the hole (see `BATCH_LABELS`).
            Edit::Batch { label, .. } => Label::with("{batch}", label.as_str()),
        }
    }
}

/// What the Edit menu calls a new comment ("Undo Add highlight"). A whole wording for each kind,
/// so a language can give the noun the article and the gender its own grammar asks for.
fn add_annotation(s: &Shape) -> &'static str {
    match s {
        Shape::Note { .. } => "Add sticky note",
        Shape::TextMarkup { kind: Markup::Highlight, .. } => "Add highlight",
        Shape::TextMarkup { kind: Markup::Underline, .. } => "Add underline",
        Shape::TextMarkup { kind: Markup::StrikeOut, .. } => "Add strikethrough",
        Shape::TextMarkup { kind: Markup::Squiggly, .. } => "Add squiggly underline",
        Shape::Rectangle { .. } => "Add rectangle",
        Shape::Oval { .. } => "Add oval",
        Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. } => "Add arrow",
        Shape::Line { .. } => "Add line",
        Shape::Ink { .. } => "Add drawing",
        Shape::TextBox { .. } => "Add text box",
        Shape::Typewriter { .. } => "Add text",
        Shape::Mark { mark: FillMark::Check, .. } => "Add checkmark",
        Shape::Mark { mark: FillMark::Cross, .. } => "Add cross",
        Shape::Mark { mark: FillMark::Dot, .. } => "Add dot",
        Shape::Mark { mark: FillMark::Line, .. } => "Add line",
        Shape::Signature { .. } | Shape::TypedSignature { .. } => "Add signature",
        Shape::Redact { .. } => "Add redaction mark",
        Shape::Stamp { .. } | Shape::CustomStamp { .. } => "Add stamp",
        Shape::Polygon { cloud: true, .. } => "Add cloud",
        Shape::Polygon { .. } => "Add polygon",
        Shape::PolyLine { .. } => "Add connected lines",
        Shape::Callout { .. } => "Add callout",
        Shape::Caret { .. } => "Add inserted text",
        Shape::Attachment { .. } => "Add file attachment",
    }
}

/// Opened as owner, or nothing is restricted (no permissions password was set): security may
/// be changed, as in Acrobat.
fn unrestricted(p: &pdfcraft_cos::Permissions) -> bool {
    const ALL: i32 = 0b1111_0011_1100; // bits 3–6 and 9–12
    p.owner || p.bits & ALL == ALL
}

/// Whether the opening password allows an edit (§7.6.4.2, Table 22).
fn check_permission(edit: &Edit, p: &pdfcraft_cos::Permissions) -> Result<(), EditError> {
    match edit {
        Edit::RotatePages { .. }
        | Edit::DeletePages { .. }
        | Edit::MovePages { .. }
        | Edit::ReversePages { .. }
        | Edit::InsertBlankPage { .. }
        | Edit::DuplicatePages { .. }
        | Edit::ReplacePages { .. }
        | Edit::SetPageBox { .. }
        | Edit::SetTransition { .. }
        | Edit::InsertPagesFrom { .. }
        // "Assemble the document: insert, rotate or delete pages and create bookmarks" (Table 22).
        | Edit::AddBookmark { .. }
        | Edit::RenameBookmark { .. }
        | Edit::DeleteBookmark { .. }
        | Edit::MoveBookmark { .. }
        | Edit::SetBookmarkPage { .. }
        | Edit::BookmarksFromStructure
        | Edit::NumberPages { .. } => {
            if p.assemble() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("page changes"))
            }
        }
        Edit::AddMeasurement(_)
        | Edit::AddAnnotation(_)
        | Edit::AddCustomStamp { .. }
        | Edit::DeleteAnnotation { .. }
        | Edit::SetAnnotationContents { .. }
        | Edit::ReplyToAnnotation { .. }
        | Edit::SetAnnotationStatus { .. }
        | Edit::MarkAnnotation { .. }
        | Edit::ReplaceText { .. }
        | Edit::EraseInk { .. }
        | Edit::LockAnnotation { .. }
        | Edit::MoveAnnotation { .. }
        | Edit::ReshapeAnnotation { .. }
        | Edit::ResizeAnnotation { .. }
        | Edit::StyleAnnotation { .. }
        | Edit::FillAnnotation { .. }
        | Edit::FontSizeAnnotation { .. }
        | Edit::SetAnnotationInfo { .. }
        | Edit::SetMeasurementScale { .. } => {
            if p.annotate() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("comments"))
            }
        }
        Edit::SetFieldValue { .. } | Edit::ResetForm { .. } | Edit::SetFieldImage { .. } | Edit::ApplyScriptChanges { .. } | Edit::XfaEvent { .. } => {
            if p.fill_forms() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("filling in form fields"))
            }
        }
        Edit::Protect(_) | Edit::RemoveProtection => {
            if unrestricted(p) {
                Ok(())
            } else {
                Err(EditError::NotPermitted("changing security"))
            }
        }
        Edit::SetInfo { .. }
        | Edit::AddHeaderFooter { .. }
        | Edit::AddWatermark { .. }
        | Edit::AddBackground { .. }
        | Edit::RemoveMarks { .. }
        | Edit::AddField { .. }
        | Edit::ApplyRedactions { .. }
        | Edit::AddText { .. }
        | Edit::AddImage { .. }
        | Edit::UpdateContent { .. }
        | Edit::MoveObjects { .. }
        | Edit::DeleteContent { .. }
        | Edit::ReplaceImage { .. }
        | Edit::ClearRedactions
        | Edit::RemoveHidden { .. }
        | Edit::ImportData { .. }
        | Edit::AddLink { .. }
        | Edit::SetLink { .. }
        | Edit::DeleteLink { .. }
        | Edit::RemoveLinks { .. }
        | Edit::AddLinks { .. }
        | Edit::Sanitize
        | Edit::XfaToStatic
        | Edit::SetFieldProps { .. }
        | Edit::DeleteField { .. }
        | Edit::DuplicateField { .. }
        | Edit::SetTabOrder { .. }
        | Edit::MoveInTabOrder { .. }
        | Edit::SetCalculationOrder { .. }
        | Edit::SetInitialView(_)
        | Edit::AutoTagText
        | Edit::SetAltText { .. }
        | Edit::AutoTagFormFields
        | Edit::SetTagProperty { .. }
        | Edit::MoveTag { .. }
        | Edit::MarkDecorative { .. }
        | Edit::MarkTagArtifact { .. }
        | Edit::AddOcrText { .. }
        | Edit::SetDocumentScript { .. }
        | Edit::SetFieldScript { .. }
        | Edit::ConvertPdfA { .. }
        | Edit::RemoveStandardId { .. }
        | Edit::EditTextLine { .. }
        | Edit::EditTextBlock { .. }
        | Edit::RegisterTextFont { .. }
        | Edit::EditPageImage { .. }
        | Edit::Flatten { .. }
        | Edit::FlattenFillSign => {
            if p.modify() {
                Ok(())
            } else {
                Err(EditError::NotPermitted("changes to the document"))
            }
        }
        Edit::Batch { edits, .. } => edits.iter().try_for_each(|e| check_permission(e, p)),
    }
}

/// Dates and unique ids stamped onto what an edit creates.
struct EditCtx {
    /// Field scripts run (`None`: JavaScript is off) and what they produced.
    js: Option<js::JsRunner>,
    /// A laid-out XFA form's template (`None`: not an XFA form, or JavaScript is off) and what
    /// its scripts produced.
    xfa: Option<Arc<pdfcraft_xfa::model::Template>>,
    xfa_out: js::JsOutput,
    /// The datasets stream this edit wrote (later writes in the edit replace it in place).
    xfa_datasets: Option<pdfcraft_cos::ObjRef>,
    /// A form script ran too long and was abandoned: the document's scripts go off.
    xfa_ran_away: bool,
    /// The edit took the XFA out of the form: what went with it, for the document's warnings.
    xfa_removed: Vec<String>,
    /// Pages in the document, for `xfa.layout.pageCount()`.
    pages: usize,
    date: Option<String>,
    /// Today in local time, for date tokens.
    today: (i64, u32, u32),
    seed: u64,
    count: u64,
    /// Fields the edit renamed to keep imported names separate (old → new), for the repair
    /// log and the undo label.
    renamed_fields: Vec<(String, String)>,
    /// The overlay streams the edit's applied redactions added, which alone the renderer's check
    /// leaves out.
    redaction_overlays: Vec<pdfcraft_cos::ObjRef>,
}

impl EditCtx {
    fn new(now: Option<i64>, salt: u64) -> Self {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ salt;
        if let Some(t) = now {
            seed ^= (t as u64).rotate_left(17);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if now.is_some() {
            // Real clock: mix in sub-second time so ids from two sessions don't collide.
            seed ^= std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0) << 32;
        }
        Self {
            js: None,
            xfa: None,
            xfa_out: Default::default(),
            xfa_datasets: None,
            xfa_ran_away: false,
            xfa_removed: Vec::new(),
            pages: 0,
            date: now.map(pdfcraft_cos::pdf_date),
            today: (1970, 1, 1),
            seed,
            count: 0,
            renamed_fields: Vec::new(),
            redaction_overlays: Vec::new(),
        }
    }

    /// 32 bytes of entropy for new encryption keys and salts. `RandomState` is seeded by the
    /// operating system, so no extra dependency is needed.
    fn entropy(&mut self) -> [u8; 32] {
        use std::hash::{BuildHasher, Hasher};
        let mut out = [0u8; 32];
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(self.seed ^ i as u64);
            h.write_u64(self.count);
            self.count += 1;
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        out
    }

    /// A fresh `/NM`: a random-looking UUID (version 4 layout) from a splitmix64 stream.
    fn meta(&mut self) -> pdfcraft_annot::Meta {
        let mut next = || {
            self.count += 1;
            let mut z = self.seed.wrapping_add(self.count.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let (a, b) = (next(), next());
        let id = format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            a >> 32,
            (a >> 16) & 0xffff,
            a & 0xfff,
            0x8000 | (b >> 48) & 0x3fff,
            b & 0xffff_ffff_ffff
        );
        pdfcraft_annot::Meta { date: self.date.clone(), id }
    }
}

/// Perform an edit on a working copy (the caller discards it on error).
fn run_edit(doc: &mut pdfcraft_cos::Document, edit: &Edit, cx: &mut EditCtx) -> Result<(), EditError> {
    match edit {
        Edit::RotatePages { pages, degrees } => pdfcraft_organize::rotate_pages(doc, pages, *degrees)?,
        Edit::DeletePages { pages } => pdfcraft_organize::delete_pages(doc, pages)?,
        Edit::MovePages { pages, to } => pdfcraft_organize::move_pages(doc, pages, *to)?,
        Edit::ReversePages { pages } => pdfcraft_organize::reverse_pages(doc, pages)?,
        Edit::InsertBlankPage { at, width, height } => {
            pdfcraft_organize::insert_blank_page(doc, *at, *width, *height)?;
        }
        Edit::SetInfo { key, value } => pdfcraft_organize::set_info(doc, key, value)?,
        Edit::DuplicatePages { pages, at } => pdfcraft_organize::duplicate_pages(doc, pages, *at)?,
        Edit::ReplacePages { pages, name, bytes, src_pages } => {
            let src = open_source(name, bytes)?;
            pdfcraft_organize::replace_pages(doc, pages, &src, src_pages)?;
        }
        Edit::SetPageBox { pages, which, spec } => pdfcraft_organize::set_page_box(doc, pages, *which, *spec)?,
        Edit::SetTransition { pages, spec } => pdfcraft_organize::set_transition(doc, pages, spec)?,
        Edit::InsertPagesFrom { name, bytes, pages, at, field_conflicts } => {
            let src = open_source(name, bytes)?;
            let pages = match pages {
                Some(p) => p.clone(),
                None => (0..pdfcraft_organize::page_count(&src)?).collect(),
            };
            // The file's bookmarks come along under one named after it, as in Combine Files.
            let title = std::path::Path::new(name).file_stem().map_or_else(|| name.clone(), |s| s.to_string_lossy().into_owned());
            let outcome = pdfcraft_organize::insert_file_pages(doc, &src, &title, &pages, *at, *field_conflicts)?;
            cx.renamed_fields = outcome.renamed_fields;
        }
        Edit::AddBookmark { parent, index, title, page, left, top } => {
            pdfcraft_organize::add_bookmark_at(doc, parent, *index, title, *page, *left, *top)?;
        }
        Edit::RenameBookmark { path, title } => pdfcraft_organize::rename_bookmark(doc, path, title)?,
        Edit::DeleteBookmark { path } => pdfcraft_organize::delete_bookmark(doc, path)?,
        Edit::MoveBookmark { from, to_parent, index } => {
            pdfcraft_organize::move_bookmark(doc, from, to_parent, *index)?;
        }
        Edit::SetBookmarkPage { path, page } => pdfcraft_organize::set_bookmark_page(doc, path, *page)?,
        Edit::BookmarksFromStructure => {
            let entries: Vec<_> = pdfcraft_a11y::headings(doc)
                .into_iter()
                .map(|h| pdfcraft_organize::OutlineEntry { level: h.level, title: h.title, page: h.page, element: h.obj })
                .collect();
            pdfcraft_organize::add_bookmark_tree(doc, "Untitled", &entries)?;
        }
        Edit::NumberPages { from, to, style, prefix, first } => pdfcraft_organize::number_pages(doc, *from, *to, *style, prefix, *first)?,
        Edit::AddMeasurement(m) => {
            measure::add(doc, m, &cx.meta())?;
        }
        Edit::SetMeasurementScale { page, bbox, name, scale } => measure::set_scale(doc, *page, *bbox, name, scale)?,
        Edit::AddAnnotation(a) => {
            pdfcraft_annot::add_annotation(doc, a, &cx.meta())?;
        }
        Edit::AddCustomStamp { page, rect, name, file, author } => {
            let src = mark_source(doc, file)?;
            let (sw, sh) = (src.size.0.max(1.0), src.size.1.max(1.0));
            let natural = rect[2] == rect[0] && rect[3] == rect[1];
            let rotation = pdfcraft_organize::page_rotation(doc, *page)?;
            let rect = if natural {
                let k = (200.0 / sw.max(sh)).min(1.0);
                let (w, h) = if matches!(rotation, 90 | 270) { (sh * k, sw * k) } else { (sw * k, sh * k) };
                [rect[0] - w / 2.0, rect[1] - h / 2.0, rect[0] + w / 2.0, rect[1] + h / 2.0]
            } else {
                *rect
            };
            let shape = Shape::CustomStamp { rect, name: name.clone(), picture: src.xobject, image: src.image, size: (sw, sh) };
            let style = pdfcraft_annot::Style::default_for(&shape);
            let new = NewAnnotation { page: *page, shape, style, contents: name.clone(), author: author.clone() };
            let index = pdfcraft_annot::add_annotation(doc, &new, &cx.meta())?;
            pdfcraft_annot::orient_image_stamp(doc, *page, index, rotation)?;
        }
        Edit::DeleteAnnotation { page, index } => pdfcraft_annot::delete_annotation(doc, *page, *index)?,
        Edit::SetAnnotationContents { page, index, text } => pdfcraft_annot::set_contents(doc, *page, *index, text, &cx.meta())?,
        Edit::ReplyToAnnotation { page, index, text, author } => {
            pdfcraft_annot::add_reply(doc, *page, *index, text, author, &cx.meta())?;
        }
        Edit::SetAnnotationStatus { page, index, state, author } => {
            pdfcraft_annot::set_review_state(doc, *page, *index, *state, author, &cx.meta())?;
        }
        Edit::EraseInk { page, index, path, radius } => {
            pdfcraft_annot::erase_ink(doc, *page, *index, path, *radius, &cx.meta())?;
        }
        Edit::ReplaceText { page, quads, text, author, strike, caret } => {
            // One `/NM` each: the caret replies to the strikeout, and XFDF names that reply by
            // the strikeout's `/NM` (#2666).
            let (strike_meta, caret_meta) = (cx.meta(), cx.meta());
            pdfcraft_annot::add_text_replacement(doc, *page, quads, text, author, strike, caret, &strike_meta, &caret_meta)?;
        }
        Edit::MarkAnnotation { page, index, marked, author } => {
            pdfcraft_annot::set_marked(doc, *page, *index, *marked, author, &cx.meta())?;
        }
        Edit::LockAnnotation { page, index, locked } => pdfcraft_annot::set_locked(doc, *page, *index, *locked)?,
        Edit::MoveAnnotation { page, index, dx, dy } => pdfcraft_annot::move_annotation(doc, *page, *index, *dx, *dy, &cx.meta())?,
        Edit::ReshapeAnnotation { page, index, points } => pdfcraft_annot::set_points(doc, *page, *index, points, &cx.meta())?,
        Edit::ResizeAnnotation { page, index, rect } => pdfcraft_annot::set_rect(doc, *page, *index, *rect, &cx.meta())?,
        Edit::StyleAnnotation { page, index, color, opacity, width, endings } => {
            pdfcraft_annot::set_style(doc, *page, *index, *color, *opacity, *width, endings.as_deref(), &cx.meta())?;
        }
        Edit::FillAnnotation { page, index, fill } => pdfcraft_annot::set_fill(doc, *page, *index, *fill, &cx.meta())?,
        Edit::FontSizeAnnotation { page, index, size } => pdfcraft_annot::set_font_size(doc, *page, *index, *size, &cx.meta())?,
        Edit::SetAnnotationInfo { page, index, author, subject, icon } => {
            pdfcraft_annot::set_info(doc, *page, *index, author.as_deref(), subject.as_deref(), *icon, &cx.meta())?;
        }
        Edit::SetFieldValue { name, value } => match cx.js.as_mut() {
            Some(js) => pdfcraft_forms::set_value_with(doc, name, value, js)?,
            None => pdfcraft_forms::set_value(doc, name, value)?,
        },
        Edit::ApplyScriptChanges { changes } => match cx.js.as_mut() {
            Some(js) => pdfcraft_forms::apply_script_changes(doc, changes, js)?,
            None => pdfcraft_forms::apply_script_changes(doc, changes, &mut pdfcraft_forms::NoScripts)?,
        },
        Edit::XfaEvent { som, activity } => {
            let tpl = cx.xfa.clone().ok_or_else(|| EditError::Invalid("this is not a laid-out XFA form, or JavaScript is off".into()))?;
            let mut run = xfa::XfaRun { page_count: cx.pages, out: &mut cx.xfa_out, datasets: cx.xfa_datasets, ran_away: false };
            let ran = xfa::on_event(doc, &tpl, som, activity, &mut run);
            cx.xfa_datasets = run.datasets;
            cx.xfa_ran_away |= run.ran_away;
            ran.map_err(EditError::Invalid)?;
        }
        Edit::SetFieldImage { name, image } => {
            let (img, _) = embed_image(doc, name, image)?;
            let px = match &*doc.get(img) {
                pdfcraft_cos::Object::Stream(s) => (s.dict.int(b"Width").unwrap_or(1) as u32, s.dict.int(b"Height").unwrap_or(1) as u32),
                _ => (1, 1),
            };
            // An XFA image field keeps the picture in the data too (as Designer saves it), and a
            // laid-out form shows it from there: so only pictures the layout can show, and only
            // for image fields (a laid-out one carries its SOM path; a static form's field name
            // is its path).
            let is_xfa = pdfcraft_xfa::read_packets(doc).ok().flatten().is_some();
            let image_field = is_xfa
                && pdfcraft_forms::fields(doc).iter().any(|f| f.name == *name && f.button == Some(pdfcraft_forms::af::ButtonAction::ImportIcon));
            if image_field {
                let Some(content_type) = image_content_type(image) else {
                    return Err(EditError::Invalid(format!("{name}: an XFA form keeps a PNG, JPEG, GIF, BMP or TIFF picture; choose one of those")));
                };
                pdfcraft_forms::set_button_icon(doc, name, img, px)?;
                let som = pdfcraft_xfa::fields_by_som(doc).into_iter().find(|(_, n)| n == name).map(|(s, _)| s).unwrap_or_else(|| name.clone());
                let path = match cx.xfa.as_deref() {
                    Some(tpl) => pdfcraft_xfa::data_path(tpl, &som),
                    None => pdfcraft_xfa::som_to_path(&som),
                };
                let r =
                    pdfcraft_xfa::write_data_image(doc, &path, content_type, image, cx.xfa_datasets).map_err(|e| EditError::Write(e.to_string()))?;
                if !r.written && r.warnings.is_empty() {
                    return Err(EditError::Write(format!("{name}: the picture could not be written into the form's data")));
                }
                cx.xfa_datasets = r.stream.or(cx.xfa_datasets);
                cx.xfa_out.errors.extend(r.warnings);
                xfa_after_data_change(doc, cx, false)?;
            } else {
                pdfcraft_forms::set_button_icon(doc, name, img, px)?;
            }
        }
        Edit::ResetForm { names } => {
            pdfcraft_forms::reset(doc, names.as_deref())?;
        }
        Edit::AddField { page, rect, kind, name } => {
            pdfcraft_forms::add_field(doc, *page, *rect, kind, name.as_deref())?;
        }
        Edit::SetFieldProps { name, props } => {
            pdfcraft_forms::set_props(doc, name, props)?;
        }
        Edit::DuplicateField { name, pages } => {
            if pdfcraft_forms::duplicate_field(doc, name, pages)? == 0 {
                return Err(EditError::Form(pdfcraft_forms::FormError::Invalid(format!("{name} is already on those pages"))));
            }
        }
        Edit::DeleteField { name } => pdfcraft_forms::delete_field(doc, name)?,
        Edit::SetTabOrder { pages, order } => pdfcraft_forms::set_tab_order(doc, pages, *order)?,
        Edit::MoveInTabOrder { name, earlier } => pdfcraft_forms::move_in_tab_order(doc, name, *earlier)?,
        // As after a script edit: the values the new order gives show at once.
        Edit::SetCalculationOrder { names } => {
            pdfcraft_forms::set_calculation_order(doc, names)?;
            match cx.js.as_mut() {
                Some(js) => pdfcraft_forms::recalculate_with(doc, js)?,
                None => pdfcraft_forms::recalculate(doc)?,
            };
        }
        Edit::SetInitialView(v) => pdfcraft_organize::set_initial_view(doc, v)?,
        Edit::AutoTagText => {
            a11y::autotag_text(doc).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::SetAltText { figure, alt } => {
            let r = pdfcraft_cos::ObjRef::new(*figure, doc.generation(*figure));
            a11y::set_alt(doc, r, alt.as_deref()).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::AutoTagFormFields => {
            a11y::tag_form_fields(doc).map_err(EditError::Accessibility)?;
        }
        Edit::SetTagProperty { object, property, value } => {
            a11y::set_tag_property(doc, *object, *property, value.as_deref()).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::MoveTag { object, anchor, placement } => {
            a11y::move_tag(doc, *object, *anchor, *placement).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::EditTextLine { page, line, text } => {
            pdfcraft_edit::replace_line(doc, *page, *line, text)?;
        }
        Edit::SetFieldScript { name, event, script } => {
            pdfcraft_forms::set_field_script(doc, name, event, script.as_deref())?;
            match cx.js.as_mut() {
                Some(js) => pdfcraft_forms::recalculate_with(doc, js)?,
                None => pdfcraft_forms::recalculate(doc)?,
            };
        }
        Edit::ConvertPdfA { level } => {
            pdfcraft_preflight::convert(doc, *level).map_err(|e| EditError::Invalid(e.to_string()))?;
        }
        Edit::RemoveStandardId { standard } => pdfcraft_organize::remove_standard_id(doc, *standard)?,
        Edit::SetDocumentScript { name, script } => pdfcraft_forms::set_document_script(doc, name, script.as_deref())?,
        Edit::AddOcrText { page, words } => {
            ocr::add_text(doc, *page, words)?;
        }
        Edit::EditPageImage { page, index, change } => {
            let img = pdfcraft_edit::page_images(doc, *page)?
                .into_iter()
                .nth(*index)
                .ok_or_else(|| EditError::Edit(pdfcraft_edit::EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1))))?;
            let c = match change {
                ImageEdit::Move(to) => pdfcraft_edit::ImageChange::Transform(pdfcraft_edit::rect_to_rect(img.rect, *to)),
                ImageEdit::Rotate(q) => {
                    pdfcraft_edit::ImageChange::Transform(pdfcraft_edit::turn_about_centre(img.visible_rect.unwrap_or(img.rect), *q, false, false))
                }
                ImageEdit::RotateBy(degrees) => {
                    let t = pdfcraft_edit::turn_by_degrees(img.visible_rect.unwrap_or(img.rect), *degrees)
                        .ok_or_else(|| EditError::Edit(pdfcraft_edit::EditError::Invalid("the rotation angle must be a finite number".into())))?;
                    pdfcraft_edit::ImageChange::Transform(t)
                }
                ImageEdit::Flip { horizontal } => pdfcraft_edit::ImageChange::Transform(pdfcraft_edit::turn_about_centre(
                    img.visible_rect.unwrap_or(img.rect),
                    0,
                    *horizontal,
                    !*horizontal,
                )),
                ImageEdit::Replace { name, bytes } => {
                    if img.is_form {
                        return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid(
                            "grouped Form artwork cannot be replaced with a bitmap".into(),
                        )));
                    }
                    pdfcraft_edit::ImageChange::Replace(embed_image(doc, name, bytes)?.0)
                }
                ImageEdit::Crop(keep) => pdfcraft_edit::ImageChange::Crop(*keep),
                ImageEdit::Delete => pdfcraft_edit::ImageChange::Delete,
            };
            pdfcraft_edit::change_image(doc, *page, *index, &c)?;
        }
        Edit::MoveObjects { page, objects, offset } => pdfcraft_edit::move_objects(doc, *page, objects, *offset)?,
        Edit::EditTextBlock { page, block, text, style } => {
            pdfcraft_edit::rewrite_block(doc, *page, *block, Some(text), style)?;
        }
        Edit::RegisterTextFont { page, bytes } => {
            pdfcraft_edit::text::register_text_font(doc, *page, bytes)?;
        }
        Edit::MarkDecorative { figure } => {
            let r = pdfcraft_cos::ObjRef::new(*figure, doc.generation(*figure));
            a11y::mark_decorative(doc, r).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::MarkTagArtifact { object } => {
            a11y::mark_tag_artifact(doc, *object).map_err(|e| EditError::Accessibility(e.to_string()))?;
        }
        Edit::AddHeaderFooter { pages, settings, replace } => {
            let date = cx.today;
            pdfcraft_edit::add_header_footer(doc, pages, settings, *replace, &pdfcraft_edit::Context { date })?;
        }
        Edit::AddWatermark { pages, settings, replace, file } => {
            let mut s = settings.clone();
            if let Some(f) = file {
                s.source = Some(mark_source(doc, f)?);
            }
            pdfcraft_edit::add_watermark(doc, pages, &s, *replace)?
        }
        Edit::AddBackground { pages, settings, replace, file } => {
            let mut s = settings.clone();
            if let Some(f) = file {
                s.source = Some(mark_source(doc, f)?);
            }
            pdfcraft_edit::add_background(doc, pages, &s, *replace)?
        }
        Edit::RemoveMarks { kind } => {
            let n = pdfcraft_model::pages(doc).len();
            if pdfcraft_edit::remove_marks(doc, &(0..n).collect::<Vec<_>>(), *kind)? == 0 {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("there is nothing to remove".into())));
            }
        }
        Edit::AddText { page, text } => {
            pdfcraft_edit::add_content(doc, *page, &AddedContent::Text(text.clone()))?;
        }
        Edit::AddImage { page, rect, name, bytes } => {
            let (image, natural) = embed_image(doc, name, bytes)?;
            let rect = match rect {
                Some(r) => *r,
                None => {
                    let p = pdfcraft_model::pages(doc).swap_remove(*page);
                    let (pw, ph) = p.display_size(doc);
                    let k = ((pw * 0.8) / natural.0).min((ph * 0.8) / natural.1).min(1.0);
                    let (w, h) = (natural.0 * k, natural.1 * k);
                    [(pw - w) / 2.0, (ph - h) / 2.0, (pw + w) / 2.0, (ph + h) / 2.0]
                }
            };
            pdfcraft_edit::add_content(doc, *page, &AddedContent::Image(pdfcraft_edit::AddedImage::new(rect, image)))?;
        }
        Edit::UpdateContent { page, index, content } => pdfcraft_edit::update_content(doc, *page, *index, content)?,
        Edit::DeleteContent { page, index } => pdfcraft_edit::delete_content(doc, *page, *index)?,
        Edit::ReplaceImage { page, index, name, bytes } => {
            let item = pdfcraft_edit::list_added(doc).into_iter().filter(|a| a.page == *page).nth(*index);
            let Some(AddedContent::Image(old)) = item.map(|a| a.content) else {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("that item is not an image".into())));
            };
            let (image, _) = embed_image(doc, name, bytes)?;
            pdfcraft_edit::update_content(doc, *page, *index, &AddedContent::Image(pdfcraft_edit::AddedImage { image, ..old }))?;
        }
        Edit::ApplyRedactions { pages } => {
            cx.redaction_overlays = pdfcraft_redact::apply(doc, pages.as_deref())?.overlays;
        }
        Edit::ClearRedactions => {
            pdfcraft_redact::clear_marks(doc, None)?;
        }
        Edit::AddLink { page, rect, action, style } => {
            pdfcraft_annot::links::add(doc, *page, *rect, action, style)?;
        }
        Edit::SetLink { page, index, rect, action, style } => pdfcraft_annot::links::set(doc, *page, *index, *rect, action.as_ref(), style.as_ref())?,
        Edit::DeleteLink { page, index } => pdfcraft_annot::links::delete(doc, *page, *index)?,
        Edit::RemoveLinks { pages } => {
            if pdfcraft_annot::links::remove_all(doc, pages.as_deref())? == 0 {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("there are no links to remove".into())));
            }
        }
        Edit::AddLinks { links, style } => {
            if pdfcraft_annot::links::add_many(doc, links, style)? == 0 {
                return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("no web addresses were found".into())));
            }
        }
        Edit::ImportData { bytes, .. } => {
            // An XFA form takes XFA data files (XDP, datasets, or a data document) as its data;
            // XFDF, FDF, Acrobat's XML and tables go the usual way.
            let text = (pdfcraft_xfa::read_packets(doc).ok().flatten().is_some()).then(|| decode_data_file(bytes)).flatten();
            match text {
                Some(Ok(text)) if is_xfa_data_file(&text) => {
                    let expect_root = cx.xfa.as_ref().and_then(|t| t.root.common.name.clone());
                    let r = pdfcraft_xfa::import_data(doc, &text, expect_root.as_deref(), cx.xfa_datasets)
                        .map_err(|e| EditError::Write(e.to_string()))?;
                    cx.xfa_datasets = r.stream.or(cx.xfa_datasets);
                    cx.xfa_out.errors.extend(r.warnings);
                    cx.xfa_out.console.push(format!("Imported XFA data: {} node(s)", r.nodes));
                    xfa_after_data_change(doc, cx, true)?;
                }
                Some(Err(why)) => return Err(EditError::Invalid(why)),
                _ => {
                    pdfcraft_xfdf::import(doc, bytes)?;
                }
            }
        }
        Edit::RemoveHidden { which } => {
            pdfcraft_redact::sanitize::remove_hidden(doc, which)?;
        }
        Edit::Sanitize => {
            pdfcraft_redact::sanitize::sanitize(doc)?;
        }
        Edit::Flatten { comments, fields } => {
            // An XFA form's fields baked into the pages: the XFA goes too, or Adobe's viewers
            // would lay the form out again from its template over the baked pages. A dynamic
            // form that was never laid out keeps its XFA: without it only placeholders remain.
            let shape = (*fields).then(|| pdfcraft_xfa::xfa_shape(doc)).flatten();
            let n = pdfcraft_model::pages(doc).len();
            pdfcraft_edit::flatten(doc, &(0..n).collect::<Vec<_>>(), *comments, *fields)?;
            match shape {
                Some(shape) if shape.strippable() => strip_xfa(doc, cx)?,
                Some(_) => cx.xfa_removed.push("The XFA was kept: this dynamic form was not laid out, so its fields were not baked in".into()),
                None => {}
            }
        }
        Edit::XfaToStatic => {
            let Some(shape) = pdfcraft_xfa::xfa_shape(doc) else {
                return Err(EditError::Invalid("this document is not an XFA form".into()));
            };
            if !shape.strippable() {
                return Err(EditError::Invalid("this dynamic XFA form was not laid out, so there is no static form to keep".into()));
            }
            strip_xfa(doc, cx)?;
        }
        Edit::FlattenFillSign => {
            let n = pdfcraft_model::pages(doc).len();
            pdfcraft_edit::flatten_fill_sign(doc, &(0..n).collect::<Vec<_>>())?;
        }
        Edit::Protect(p) => {
            p.validate()?;
            let seed = cx.entropy();
            // Without a permissions password nothing is restricted, so the owner password is a
            // random one nobody needs.
            let random_owner: String = cx.entropy().iter().map(|b| format!("{b:02x}")).collect();
            let params = pdfcraft_cos::NewEncryption {
                algorithm: p.algorithm,
                user_password: p.open_password.as_deref().unwrap_or(""),
                owner_password: p.permissions_password.as_deref().unwrap_or(&random_owner),
                permissions: p.permission_bits(),
                encrypt_metadata: p.encrypt_metadata,
                seed,
            };
            doc.set_encryption(&params).map_err(|e| EditError::Protection(e.to_string()))?;
        }
        Edit::RemoveProtection => {
            if doc.output_handler().is_none() {
                return Err(EditError::Protection("the document isn't protected".into()));
            }
            doc.remove_encryption();
        }
        Edit::Batch { edits, .. } => {
            for e in edits {
                run_edit(doc, e, cx)?;
            }
        }
    }
    Ok(())
}

/// The comment list, read from the object graph (as `inspect` lists comments).
fn comment_list(doc: &pdfcraft_cos::Document) -> Vec<pdfcraft_render::Annotation> {
    pdfcraft_annot::summaries(doc)
        .into_iter()
        .map(|s| pdfcraft_render::Annotation {
            page: s.page,
            subtype: s.subtype,
            author: s.author,
            contents: s.contents,
            modified: s.modified.map(|m| pdfcraft_render::pretty_date(&m)),
            name: s.name,
            in_reply_to: s.in_reply_to,
            rect: s.rect,
            color: s.color,
            index: s.index,
            state: s.state,
            quads: s.quads,
            locked: s.locked,
            intent: s.intent,
            fill_sign: s.fill_sign,
            points: s.points,
        })
        .collect()
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

/// The passwords after `edit`, if it changes them.
fn keys_after(edit: &Edit) -> Option<Keys> {
    match edit {
        Edit::Protect(p) => {
            Some(Keys { render: p.open_password.clone(), reopen: p.permissions_password.clone().or_else(|| p.open_password.clone()) })
        }
        Edit::RemoveProtection => Some(Keys::default()),
        Edit::Batch { edits, .. } => edits.iter().rev().find_map(keys_after),
        _ => None,
    }
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

/// The last-resort guard (AGENTS.md §4): run `f`, turning a panic that escapes it into an error
/// message, so one bad file or edit can't take the app and its other documents down. It is a
/// safety net for bugs, not a substitute for returning errors.
pub fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>().map(|s| (*s).to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
    })
}

/// Parse another PDF to copy pages from.
fn open_source(name: &str, bytes: &Arc<Vec<u8>>) -> Result<pdfcraft_cos::Document, EditError> {
    open_source_with(name, bytes, None)
}

/// [`open_source`], authenticating with `password` (user or owner) for an encrypted file.
fn open_source_with(name: &str, bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<pdfcraft_cos::Document, EditError> {
    source_document(bytes, password).map_err(|p| EditError::Source(format!("{name}: {p}")))
}

/// Why a file can't be a source for Combine Files (or Insert / Replace pages).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SourceProblem {
    #[error("it is password-protected")]
    Password,
    #[error("the password is wrong")]
    WrongPassword,
    #[error("its security settings don't allow copying pages")]
    NotPermitted,
    #[error("{0}")]
    Unreadable(String),
}

fn source_document(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<pdfcraft_cos::Document, SourceProblem> {
    match std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password)) {
        Ok(Ok(d)) if d.permissions().is_some_and(|p| !p.assemble()) => Err(SourceProblem::NotPermitted),
        Ok(Ok(d)) => Ok(d),
        Ok(Err(pdfcraft_cos::CosError::NeedsPassword)) => Err(SourceProblem::Password),
        Ok(Err(pdfcraft_cos::CosError::WrongPassword)) => Err(SourceProblem::WrongPassword),
        Ok(Err(e)) => Err(SourceProblem::Unreadable(e.to_string())),
        Err(_) => Err(SourceProblem::Unreadable("the file could not be read".into())),
    }
}

/// Whether `bytes` can be combined, opened with `password` (user or owner) if given, and if not
/// why: what Combine would refuse it for, so the Combine files list can say so before Combine
/// is pressed. The owner (permissions) password lifts the restriction on copying pages.
pub fn combine_source_check(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<(), SourceProblem> {
    source_document(bytes, password).map(|_| ())
}

/// The number of pages of `bytes`, opened with `password` (user or owner) if given, whatever
/// its permissions allow; `None` when it can't be read.
pub fn source_page_count(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Option<usize> {
    let doc = std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password)).ok()?.ok()?;
    pdfcraft_organize::page_count(&doc).ok()
}

/// The singular or the plural wording, by count. Both are written out: a language forms its plural
/// its own way, and `one` and `many` may differ in more than an "s".
fn plural(one: &'static str, many: &'static str, n: usize) -> Label {
    Label::of(if n == 1 { one } else { many })
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EditError {
    #[error("no such document")]
    NoDocument,
    #[error("this document can't be edited: {0}")]
    ReadOnly(String),
    #[error("the document's security settings don't allow {0}; open it with the owner password to make this change")]
    NotPermitted(&'static str),
    #[error("{0}")]
    Organize(#[from] pdfcraft_organize::OrganizeError),
    #[error("{0}")]
    Bookmark(#[from] pdfcraft_organize::OutlineError),
    #[error("{0}")]
    Comment(#[from] pdfcraft_annot::AnnotError),
    #[error(transparent)]
    Measure(#[from] measure::MeasureError),
    #[error("{0}")]
    Protection(String),
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("{0}")]
    Form(#[from] pdfcraft_forms::FormError),
    #[error(transparent)]
    Redact(#[from] pdfcraft_redact::RedactError),
    #[error("{0}")]
    Print(String),
    #[error("{0}")]
    Accessibility(String),
    #[error(transparent)]
    Data(#[from] pdfcraft_xfdf::DataError),
    #[error("{0}")]
    Edit(#[from] pdfcraft_edit::EditError),
    #[error("{0}")]
    Create(#[from] pdfcraft_create::CreateError),
    #[error("the edited document could not be written: {0}")]
    Write(String),
    #[error("the edited document could not be reopened: {0}")]
    Reopen(String),
    #[error("couldn't use {0}")]
    Source(String),
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
    #[error("{0}")]
    Sign(String),
    #[error("{0}")]
    Optimize(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0} isn't possible in a signed document: it would rewrite the file and invalidate the signatures")]
    SignedRewrite(String),
    #[error("this document is signed: rewriting it would invalidate its signatures (save it incrementally instead)")]
    Signed,
    #[error("{0}")]
    Invalid(String),
}

impl From<pdfcraft_sign::SignError> for EditError {
    fn from(e: pdfcraft_sign::SignError) -> Self {
        EditError::Sign(e.to_string())
    }
}

#[derive(Default)]
pub struct Session {
    docs: Vec<Document>,
    next_id: u64,
    /// Seconds since the Unix epoch, injected so saves are deterministic in tests.
    clock: Option<fn() -> i64>,
    /// Seconds to add to UTC for local time, read from whoever knows the zone (a browser, say).
    /// Separate from `clock` on purpose: `clock` alone is the test shortcut that takes the
    /// injected time AS local time and zeroes the zone, so injecting only it leaves either the
    /// local dates or the UTC ones wrong by the zone's offset.
    utc_offset: Option<fn() -> i64>,
    /// Certificates trusted for signing (Acrobat: Trusted Certificates).
    trust: Arc<TrustStore>,
    /// Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript, inverted (on by default).
    js_off: bool,
    /// Preferences ▸ Date format, when not the default (see [`dates`]).
    date_format: Option<String>,
    /// Preferences ▸ Date format ▸ Language, when not following the interface language.
    date_language: Option<String>,
}

/// Immutable session state needed while a document is prepared away from its owning thread.
#[derive(Clone)]
pub struct OpenContext {
    now: Option<i64>,
    js_off: bool,
    trust: Arc<TrustStore>,
}

/// A fully prepared document that has not yet been registered in a session.
pub struct PreparedDocument {
    document: Document,
}

impl OpenContext {
    /// Prepare a document without mutating the session that supplied this context.
    pub fn prepare_open(
        &self,
        name: impl Into<String>,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let name = name.into();
        guard(|| Session::prepare_open_unguarded(self, name, path, bytes, password))
            .unwrap_or_else(|m| Err(OpenError::Invalid(format!("reading it failed unexpectedly ({m})"))))
    }
}

/// Lay a dynamic XFA form out (pages and fields) and give its widgets appearances.
fn xfa_layout(doc: &mut pdfcraft_cos::Document) -> Result<XfaLayout, String> {
    let mut report = pdfcraft_xfa::render_into(doc).map_err(|e| e.to_string())?;
    xfa_redraw(doc)?;
    // The form's own values: one its field's font can't draw is drawn with `?` and reported.
    report.warnings.extend(undrawn_field_warnings(doc));
    Ok(report)
}

/// Give the fields of a laid-out XFA form their appearances. A signed signature box keeps the
/// picture its signer's application drew.
pub(crate) fn xfa_redraw(doc: &mut pdfcraft_cos::Document) -> Result<(), String> {
    for f in pdfcraft_forms::fields(doc) {
        if f.kind == pdfcraft_forms::FieldKind::Signature && doc.get(f.obj).as_dict().is_some_and(|d| d.contains(b"V")) {
            continue;
        }
        pdfcraft_forms::redraw_field(doc, &f.name).map_err(|e| format!("{}: {e}", f.name))?;
    }
    Ok(())
}

/// A warning for each field whose appearance PdfCraft drew with `?` for a character its font
/// can't draw (laying out an XFA form's own values).
fn undrawn_field_warnings(doc: &pdfcraft_cos::Document) -> Vec<String> {
    pdfcraft_forms::undrawn_fields(doc)
        .into_iter()
        .map(|(name, c)| {
            format!("form field {name:?} holds \"{}\" (U+{:04X}), which its font can't draw: it shows ? in its place", c.escape_debug(), u32::from(c))
        })
        .collect()
}

/// The form's fields as the XFA data layer wants them.
fn xfa_field_data(doc: &pdfcraft_cos::Document) -> Vec<pdfcraft_xfa::FieldDatum> {
    use pdfcraft_forms::FieldKind as K;
    pdfcraft_forms::fields(doc)
        .into_iter()
        .map(|f| {
            let data = match f.kind {
                K::Text | K::Combo | K::List => pdfcraft_xfa::FieldData::Text(f.value.join("\n")),
                K::CheckBox => pdfcraft_xfa::FieldData::Check(!f.value.is_empty()),
                K::Radio => pdfcraft_xfa::FieldData::Radio(f.value.first().cloned()),
                K::PushButton | K::Signature => pdfcraft_xfa::FieldData::None,
            };
            pdfcraft_xfa::FieldDatum { obj: f.obj, name: f.name, data }
        })
        .collect()
}

/// Keep the XFA datasets packet in step with the fields after an edit. Returns what could not
/// be written.
/// `datasets`: the stream this edit already wrote, replaced in place rather than added again
/// (and set to the one written).
/// `tpl`: the form's parsed template when the document has one cached (else it is parsed).
fn xfa_sync_datasets(
    doc: &mut pdfcraft_cos::Document,
    tpl: Option<&pdfcraft_xfa::model::Template>,
    datasets: &mut Option<pdfcraft_cos::ObjRef>,
) -> Result<Vec<String>, String> {
    let data = xfa_field_data(doc);
    let r = pdfcraft_xfa::write_datasets_with(doc, tpl, &data, *datasets).map_err(|e| e.to_string())?;
    if r.stream.is_some() {
        *datasets = r.stream;
    }
    Ok(r.warnings)
}

/// What a document whose form script ran away is told.
const XFA_SCRIPTS_OFF: &str =
    "A script of this form ran too long and was abandoned; the form's scripts are off for this document (reopen it to run them again)";

/// Most XFA warnings kept per document.
const MAX_XFA_WARNINGS: usize = 50;

/// Add `new` to `list` (no repeats, at most [`MAX_XFA_WARNINGS`]).
fn note_warnings(list: &mut Vec<String>, new: &[String]) {
    for w in new {
        if !list.contains(w) && list.len() < MAX_XFA_WARNINGS {
            list.push(w.clone());
        }
    }
}

/// A data file's text: UTF-8 or UTF-16 (by a byte order mark, or how `<` is written), or
/// Latin-1 when its XML declaration says so. `None`: not text at all (FDF); `Err`: an
/// encoding this can't read.
fn decode_data_file(bytes: &[u8]) -> Option<Result<String, String>> {
    if bytes.starts_with(b"%FDF") {
        return None;
    }
    let declared = |text: &str| -> Option<String> {
        let head = text.get(..text.len().min(256))?;
        let decl = head.strip_prefix('\u{feff}').unwrap_or(head).trim_start().strip_prefix("<?xml")?;
        let decl = decl.split_once("?>")?.0;
        let enc = decl.split_once("encoding=")?.1;
        let quote = enc.chars().next()?;
        Some(enc.get(1..)?.split(quote).next()?.trim().to_ascii_lowercase())
    };
    if let Some((text, _)) = pdfcraft_xfa::packets::decode(bytes) {
        match declared(&text).as_deref() {
            None | Some("utf-8" | "utf8" | "utf-16" | "utf-16le" | "utf-16be" | "us-ascii" | "ascii") => return Some(Ok(text)),
            Some("iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252") if !bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => {
                // Each byte is the character it names (the 0x80–0x9F block of Windows-1252
                // differs, and is rare in form data).
                return Some(Ok(bytes.iter().map(|b| char::from(*b)).collect()));
            }
            Some(other) => return Some(Err(format!("the data file's encoding {other:?} is not supported (UTF-8, UTF-16 and ISO-8859-1 are)"))),
        }
    }
    // Not valid UTF-8/UTF-16: Latin-1 by declaration, else refused.
    let latin1: String = bytes.iter().map(|b| char::from(*b)).collect();
    match declared(&latin1).as_deref() {
        Some("iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252") => Some(Ok(latin1)),
        _ => Some(Err("the data file is not UTF-8, UTF-16 or ISO-8859-1 text".into())),
    }
}

/// Take the XFA out of `doc` and note what went with it: the packets, data no field showed,
/// buttons that did nothing but run scripts, and fields whose value differs from the form's
/// data (data PdfCraft could not read into them, now lost).
fn strip_xfa(doc: &mut pdfcraft_cos::Document, cx: &mut EditCtx) -> Result<(), EditError> {
    let fields = pdfcraft_forms::fields(doc);
    let data = xfa_field_data(doc);
    let differ = pdfcraft_xfa::read_values(doc, &data)
        .into_iter()
        .filter(|(name, value)| {
            let Some(f) = fields.iter().find(|f| f.name == *name) else { return false };
            match value {
                pdfcraft_xfa::FieldData::Text(t) => f.value.join("\n") != *t && !t.trim().is_empty(),
                pdfcraft_xfa::FieldData::Check(on) => f.value.is_empty() == *on,
                pdfcraft_xfa::FieldData::Radio(sel) => f.value.first() != sel.as_ref(),
                pdfcraft_xfa::FieldData::None => false,
            }
        })
        .count();
    let r = pdfcraft_xfa::strip(doc).map_err(|e| EditError::Write(e.to_string()))?;
    cx.xfa_removed.extend(r.notes());
    if differ > 0 {
        cx.xfa_removed.push(format!(
            "{differ} field(s) held a value other than the form's XFA data (data PdfCraft could not read into them); the data's version is gone"
        ));
    }
    Ok(())
}

/// Keep the document's XFA state in step with its file after an edit: a script may have laid
/// the form out again (rows added, subforms shown), an edit may have taken the XFA out
/// (flattened, or made static), or an undo put it back.
fn sync_xfa_state(doc: &mut Document) {
    let Some(cos) = doc.editor.as_ref().map(|e| &e.cos) else { return };
    match pdfcraft_xfa::existing_layout(cos) {
        Some(report) => {
            if doc.xfa.is_none() {
                // Back after an undo: the XFA's own warnings apply again, the strip's don't.
                doc.xfa_warnings.retain(|w| !w.starts_with(XFA_REMOVED_PREFIX));
                if doc.xfa_template.is_none() && !doc.xfa_scripts_off {
                    doc.xfa_template = xfa::template(cos);
                }
            }
            doc.xfa = Some(report);
        }
        None => {
            doc.xfa = None;
            doc.xfa_template = None;
            // Only what the strip said stays (the XFA's own warnings no longer apply).
            doc.xfa_warnings.retain(|w| w.starts_with(XFA_REMOVED_PREFIX));
        }
    }
}

/// How the warnings of a strip start, so they can be told from the XFA's own.
const XFA_REMOVED_PREFIX: &str = "XFA removed: ";

/// Is this an XFA data file (an XDP, a datasets document, a data element or a data document),
/// rather than XFDF, Acrobat's `<fields>` XML or a table? By its root element.
fn is_xfa_data_file(text: &str) -> bool {
    let text = text.get(..text.len().min(65_536)).unwrap_or(text);
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    // Past the XML declaration, comments and processing instructions.
    let mut guard = 0;
    while guard < 100 {
        guard += 1;
        if let Some(r) = rest.strip_prefix("<?") {
            rest = r.split_once("?>").map_or("", |(_, r)| r).trim_start();
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.split_once("-->").map_or("", |(_, r)| r).trim_start();
        } else if let Some(r) = rest.strip_prefix("<!") {
            rest = r.split_once('>').map_or("", |(_, r)| r).trim_start();
        } else {
            break;
        }
    }
    let Some(tag) = rest.strip_prefix('<') else { return false };
    let name = tag.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or("");
    let local = name.rsplit(':').next().unwrap_or(name);
    !local.is_empty() && !matches!(local, "xfdf" | "fields" | "html" | "body")
}

/// The content type a picture is saved under, from its bytes: the kinds a laid-out form shows.
fn image_content_type(bytes: &[u8]) -> Option<&'static str> {
    match pdfcraft_xfa::image::kind(bytes)? {
        pdfcraft_xfa::image::Kind::Jpeg => Some("image/jpeg"),
        pdfcraft_xfa::image::Kind::Png => Some("image/png"),
        pdfcraft_xfa::image::Kind::Gif => Some("image/gif"),
        pdfcraft_xfa::image::Kind::Bmp => Some("image/bmp"),
        pdfcraft_xfa::image::Kind::Tiff => Some("image/tiff"),
    }
}

/// After the XFA data changed outside the fields (an import, a picture): a laid-out form is
/// laid out again from the data (then, when `calculate`, its calculations run), and the
/// fields take the data's values.
fn xfa_after_data_change(doc: &mut pdfcraft_cos::Document, cx: &mut EditCtx, calculate: bool) -> Result<(), EditError> {
    if let Some(tpl) = cx.xfa.clone() {
        let mut run = xfa::XfaRun { page_count: cx.pages, out: &mut cx.xfa_out, datasets: cx.xfa_datasets, ran_away: false };
        let r = guard(|| xfa::on_data_replaced(doc, &tpl, calculate, &mut run))
            .unwrap_or_else(|m| Err(format!("laying the form out again failed unexpectedly ({m})")));
        cx.xfa_datasets = run.datasets;
        cx.xfa_ran_away |= run.ran_away;
        if let Err(e) = r {
            cx.xfa_out.errors.push(e);
        }
    }
    let taken = xfa_values_from_datasets(doc).map_err(EditError::Write)?;
    if !taken.is_empty() {
        cx.xfa_out.console.push(format!("{} field value(s) taken from the imported data", taken.len()));
    }
    // Not laid out again (a static form, or scripts off): fields the data no longer holds are
    // emptied, so the data replaced is the data shown.
    if cx.xfa.is_none() && calculate {
        let data = xfa_field_data(doc);
        let present: std::collections::HashSet<String> = pdfcraft_xfa::read_values(doc, &data).into_iter().map(|(n, _)| n).collect();
        for f in pdfcraft_forms::fields(doc) {
            if present.contains(&f.name) || f.value.is_empty() {
                continue;
            }
            use pdfcraft_forms::FieldKind as K;
            let empty = match f.kind {
                K::Text => FieldValue::Text(String::new()),
                K::CheckBox => FieldValue::Check(false),
                K::Radio => FieldValue::Radio(None),
                K::Combo | K::List => FieldValue::Choice(Vec::new()),
                K::PushButton | K::Signature => continue,
            };
            if let Err(e) = pdfcraft_forms::set_value(doc, &f.name, &empty) {
                cx.xfa_out.errors.push(format!("{}: not emptied by the import: {e}", f.name));
            }
        }
    }
    Ok(())
}

/// Give the fields the values the XFA datasets hold (a form filled in another viewer). Returns
/// the names of the fields that changed.
fn xfa_values_from_datasets(doc: &mut pdfcraft_cos::Document) -> Result<Vec<String>, String> {
    let data = xfa_field_data(doc);
    let fields = pdfcraft_forms::fields(doc);
    let mut changed = Vec::new();
    for (name, value) in pdfcraft_xfa::read_values(doc, &data) {
        let Some(f) = fields.iter().find(|f| f.name == name) else { continue };
        let new = match value {
            pdfcraft_xfa::FieldData::Text(t) if matches!(f.kind, pdfcraft_forms::FieldKind::Combo | pdfcraft_forms::FieldKind::List) => {
                // Saved values, one per line for a multi-select list; shown text stands for
                // its saved value (an older viewer may have written it).
                let saved = |v: &str| f.options.iter().find(|(e, s)| e == v || s == v).map_or(v.to_string(), |(e, _)| e.clone());
                let picked: Vec<String> = if f.has(pdfcraft_forms::flags::MULTI_SELECT) {
                    t.lines().map(str::trim).filter(|l| !l.is_empty()).map(saved).collect()
                } else {
                    vec![saved(t.trim())]
                };
                (f.value != picked).then_some(FieldValue::Choice(picked))
            }
            pdfcraft_xfa::FieldData::Text(t) => (f.value.join("\n") != t).then_some(FieldValue::Text(t)),
            pdfcraft_xfa::FieldData::Check(on) => (f.value.is_empty() == on).then_some(FieldValue::Check(on)),
            pdfcraft_xfa::FieldData::Radio(sel) => (f.value.first() != sel.as_ref()).then_some(FieldValue::Radio(sel)),
            pdfcraft_xfa::FieldData::None => None,
        };
        if let Some(v) = new
            && pdfcraft_forms::set_value(doc, &name, &v).is_ok()
        {
            changed.push(name);
        }
    }
    Ok(changed)
}

/// Validate the signature fields of `cos` (written as `bytes`).
fn signatures_of(cos: &pdfcraft_cos::Document, bytes: &[u8], trust: &TrustStore, cache: &pdfcraft_sign::DigestCache) -> Vec<SignatureInfo> {
    pdfcraft_sign::pdf::list_cached(cos, bytes, trust, cache)
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed clock (tests) instead of the system time.
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = Some(clock);
        self
    }

    /// A real clock for a host that has neither a clock nor a time zone of its own (a
    /// `wasm32-unknown-unknown` build: `now()` would be empty and `local_utc_offset()` is 0, so
    /// Fill & Sign stamps 1 Jan 1970 and `/ModDate` is never written). `clock` is the UTC time in
    /// seconds since the epoch; `utc_offset` is the local zone's seconds east of UTC, asked on every
    /// use because the zone changes with the date (daylight saving). Unlike
    /// [`Session::with_clock`], which is a test shortcut, nothing here is taken as local time.
    pub fn with_wall_clock(mut self, clock: fn() -> i64, utc_offset: fn() -> i64) -> Self {
        self.clock = Some(clock);
        self.utc_offset = Some(utc_offset);
        self
    }

    /// Seconds to add to UTC for the local time of this session: the injected zone, else 0 under
    /// the test clock (which is already local), else the system's.
    fn local_offset(&self) -> i64 {
        match (self.utc_offset, self.clock) {
            (Some(zone), _) => zone(),
            (None, Some(_)) => 0,
            (None, None) => local_utc_offset(),
        }
    }

    /// Seconds since the Unix epoch from the session clock (0 when unknown).
    pub fn now_secs(&self) -> i64 {
        self.now().unwrap_or(0)
    }

    fn now(&self) -> Option<i64> {
        if let Some(c) = self.clock {
            return Some(c());
        }
        #[cfg(not(target_arch = "wasm32"))]
        return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64);
        #[cfg(target_arch = "wasm32")]
        return None;
    }

    /// Today's date in local time: (year, month, day). With an injected clock (tests) the clock
    /// is taken as local time.
    /// A dynamic stamp's second line: "By Ada at 2:14 pm, Oct 02, 2026" (local time).
    pub fn stamp_by_line(&self, author: &str) -> String {
        let offset = self.local_offset();
        let d = self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).unwrap_or_default();
        let num = |a: usize, b: usize| d.get(a..b).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let (y, mo, day, hh, mm) = (num(2, 6), num(6, 8), num(8, 10), num(10, 12), num(12, 14));
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let h12 = if hh % 12 == 0 { 12 } else { hh % 12 };
        let who = if author.trim().is_empty() { String::new() } else { format!("By {} ", author.trim()) };
        format!("{who}at {h12}:{mm:02} {}, {} {day:02}, {y}", if hh < 12 { "am" } else { "pm" }, MONTHS[(mo.clamp(1, 12) - 1) as usize])
    }

    pub fn today(&self) -> (i64, u32, u32) {
        let offset = self.local_offset();
        self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).as_deref().and_then(parse_ymd).unwrap_or((1970, 1, 1))
    }

    /// Open a document from bytes. Rendering starts lazily when pages are requested.
    ///
    /// `password` is tried as either the user or owner password when the file is encrypted.
    /// Open saved revision `n` (1 = the oldest) of a document as a new, unsaved document named
    /// "<name> (revision n)".
    pub fn open_revision(&mut self, id: DocId, n: usize) -> Result<DocId, String> {
        let doc = self.get(id).ok_or("no such document")?;
        let ends = doc.revision_ends();
        let end = *n.checked_sub(1).and_then(|i| ends.get(i)).ok_or_else(|| format!("the document has {} revision(s)", ends.len()))?;
        let name = format!("{} (revision {n}).pdf", doc.name.trim_end_matches(".pdf"));
        let (bytes, password) = (Arc::new(doc.bytes[..end.min(doc.bytes.len())].to_vec()), doc.password.clone());
        self.open(name, None, bytes, password.as_deref()).map_err(|e| e.to_string())
    }

    /// Snapshot the session state that affects open-time scripts, dates and signature trust.
    pub fn open_context(&self) -> OpenContext {
        OpenContext { now: self.now(), js_off: self.js_off, trust: self.trust.clone() }
    }

    /// Prepare a document without adding it to this session.
    pub fn prepare_open(
        &self,
        name: impl Into<String>,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        self.open_context().prepare_open(name, path, bytes, password)
    }

    /// Register a prepared document. This is the only open step that mutates the session.
    pub fn commit_open(&mut self, prepared: PreparedDocument) -> DocId {
        self.next_id += 1;
        let id = DocId(self.next_id);
        let mut document = prepared.document;
        document.id = id;
        self.docs.push(document);
        id
    }

    /// Synchronous convenience API used by the CLI and automation.
    pub fn open(&mut self, name: impl Into<String>, path: Option<String>, bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocId, OpenError> {
        let prepared = self.prepare_open(name, path, bytes, password)?;
        Ok(self.commit_open(prepared))
    }

    fn prepare_open_unguarded(
        context: &OpenContext,
        name: String,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        password: Option<&str>,
    ) -> Result<PreparedDocument, OpenError> {
        let cos = std::panic::catch_unwind(|| pdfcraft_cos::Document::open_with_password(bytes.clone(), password));
        // The renderer authenticates on its own. It cannot use the owner password of R2–R4
        // files, so give it the user password that owner authentication recovers.
        let (info, render_password) = match inspect(bytes.clone(), password) {
            Ok(info) => (info, password.map(str::to_owned)),
            Err(OpenError::WrongPassword) => {
                let user = match &cos {
                    Ok(Ok(d)) => recovered_render_password(d),
                    _ => None,
                };
                let user = user.ok_or(OpenError::WrongPassword)?;
                (inspect(bytes.clone(), Some(&user))?, Some(user))
            }
            Err(e) => return Err(e),
        };
        // A dynamic XFA form is a shell around an XML template; lay the template out into real
        // pages and fields so the rest of the engine works on it. The original bytes stay: the
        // laid-out form is one appended revision.
        let opts = SaveOptions { mod_date: context.now.map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        // Write `work` as one more revision and reopen it: what the document then is.
        let rebase = |work: &pdfcraft_cos::Document| -> Result<(Arc<Vec<u8>>, DocInfo, pdfcraft_cos::Document), String> {
            let new_bytes = write_incremental(work, &opts).map(Arc::new).map_err(|e| e.to_string())?;
            let new_info = inspect(new_bytes.clone(), render_password.as_deref()).map_err(|e| e.to_string())?;
            let new_cos = pdfcraft_cos::Document::open_with_password(new_bytes.clone(), password).map_err(|e| e.to_string())?;
            Ok((new_bytes, new_info, new_cos))
        };
        let (bytes, info, cos, xfa) = match (info.xfa, cos) {
            (Some(pdfcraft_render::Xfa::Dynamic), Ok(Ok(cos))) if pdfcraft_xfa::existing_layout(&cos).is_none() => {
                let mut work = cos.clone();
                let laid_out = guard(|| xfa_layout(&mut work))
                    .unwrap_or_else(|m| Err(format!("laying it out failed unexpectedly ({m})")))
                    .and_then(|report| rebase(&work).map(|(b, i, c)| (b, i, c, report)));
                match laid_out {
                    Ok((b, i, c, report)) => (b, i, Ok(Ok(c)), Some(report)),
                    Err(e) => {
                        let mut info = info;
                        info.warnings.push(format!("This dynamic XFA form could not be laid out: {e}"));
                        (bytes, info, Ok(Ok(cos)), None)
                    }
                }
            }
            // A static XFA form, or a dynamic one laid out earlier: its datasets may hold values
            // filled in by another viewer since; give the fields those values.
            (Some(_), Ok(Ok(cos))) => {
                let report = pdfcraft_xfa::existing_layout(&cos);
                let mut work = cos.clone();
                let synced =
                    guard(|| xfa_values_from_datasets(&mut work)).unwrap_or_else(|m| Err(format!("reading its data failed unexpectedly ({m})")));
                match synced {
                    Ok(names) if names.is_empty() => (bytes, info, Ok(Ok(cos)), report),
                    Ok(names) => match rebase(&work) {
                        Ok((b, mut i, c)) => {
                            // The fields were rewritten from the XFA data: say so, and which.
                            let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
                            let more = if names.len() > shown.len() { format!(" and {} more", names.len() - shown.len()) } else { String::new() };
                            i.warnings.push(format!(
                                "{} form field value{} were taken from this form's XFA data (filled in by another viewer): {}{more}",
                                names.len(),
                                if names.len() == 1 { "" } else { "s" },
                                shown.join(", ")
                            ));
                            (b, i, Ok(Ok(c)), report)
                        }
                        Err(e) => {
                            let mut info = info;
                            info.warnings.push(format!("The values in this form's XFA data could not be applied: {e}"));
                            (bytes, info, Ok(Ok(cos)), report)
                        }
                    },
                    Err(e) => {
                        let mut info = info;
                        info.warnings.push(format!("The values in this form's XFA data could not be applied: {e}"));
                        (bytes, info, Ok(Ok(cos)), report)
                    }
                }
            }
            (_, cos) => (bytes, info, cos, None),
        };
        // A laid-out XFA form: its initialize and calculate scripts run now, as on opening in
        // Acrobat; what they change is one more revision, still nothing to save.
        let mut script_output = js::JsOutput::default();
        let mut ran_away = false;
        let xfa_template = match (&xfa, &cos) {
            (Some(_), Ok(Ok(c))) => xfa::template(c),
            _ => None,
        };
        let (bytes, info, cos) = match (&xfa, cos) {
            (Some(_), Ok(Ok(cos))) if !context.js_off => match xfa_template.clone() {
                Some(tpl) => {
                    let mut work = cos.clone();
                    let pages = info.pages.len();
                    let ran = guard(|| {
                        let mut run = xfa::XfaRun { page_count: pages, out: &mut script_output, datasets: None, ran_away: false };
                        let changes = xfa::on_open(&mut work, &tpl, &mut run);
                        if run.ran_away {
                            ran_away = true;
                        }
                        changes
                    })
                    .unwrap_or_else(|m| Err(format!("its scripts failed unexpectedly ({m})")));
                    match ran {
                        Ok(changes) if work.is_modified() => match rebase(&work) {
                            Ok((b, mut i, c)) => {
                                // The file as saved differs from what opening shows: say so.
                                if !changes.is_empty() {
                                    i.warnings.push(format!(
                                        "This form's scripts changed it on opening ({}); saving keeps those changes",
                                        changes.describe()
                                    ));
                                }
                                (b, i, Ok(Ok(c)))
                            }
                            Err(e) => {
                                script_output.errors.push(format!("The form's scripts could not be applied: {e}"));
                                (bytes, info, Ok(Ok(cos)))
                            }
                        },
                        Ok(_) => (bytes, info, Ok(Ok(cos))),
                        Err(e) => {
                            script_output.errors.push(format!("The form's scripts could not run: {e}"));
                            (bytes, info, Ok(Ok(cos)))
                        }
                    }
                }
                None => (bytes, info, Ok(Ok(cos))),
            },
            (_, cos) => (bytes, info, cos),
        };
        // The catalog leads to no page, but the reader found pages by scanning: gather the page
        // objects into a page tree as one more revision, or page edits address none of them
        // (#1656). Kept only when the reader then shows the same pages, and never on a signed
        // document, where writing the repair could rewrite the file under its signatures.
        let (bytes, info, cos) = match cos {
            Ok(Ok(cos)) if !info.pages.is_empty() && pdfcraft_organize::page_count(&cos).unwrap_or(0) == 0 => {
                let mut work = cos.clone();
                let signed = || pdfcraft_sign::pdf::list(&cos, &bytes, &context.trust).iter().any(|s| s.signed);
                let rebuilt = match pdfcraft_organize::recover_page_tree(&mut work) {
                    Ok(n) if n > 0 && !signed() => rebase(&work).ok().filter(|(_, i, _)| same_pages(&i.pages, &info.pages)),
                    _ => None,
                };
                match rebuilt {
                    Some((b, i, mut c)) => {
                        for line in cos.repair_log() {
                            c.note_repair(line.clone());
                        }
                        c.note_repair(format!("the page tree is missing; its {} pages were found by scanning the file", i.pages.len()));
                        (b, i, Ok(Ok(c)))
                    }
                    None => (bytes, info, Ok(Ok(cos))),
                }
            }
            cos => (bytes, info, cos),
        };
        // XFA notes survive the document being re-read after edits.
        let xfa_warnings: Vec<String> = if info.xfa.is_some() { info.warnings.clone() } else { Vec::new() };
        let mut document = Self::build_document(&context.trust, name, path, bytes, info, cos, render_password, password, xfa);
        // A script that ran away on open turns the form's scripts off for this document.
        document.xfa_template = if ran_away { None } else { xfa_template };
        document.js_output.append(script_output);
        if ran_away {
            document.xfa_scripts_off = true;
            document.js_output.errors.push(XFA_SCRIPTS_OFF.into());
        }
        note_warnings(&mut document.xfa_warnings, &xfa_warnings);
        Ok(PreparedDocument { document })
    }

    /// The last step of opening: build the document record and register it.
    #[allow(clippy::too_many_arguments)]
    fn build_document(
        trust: &Arc<TrustStore>,
        name: String,
        path: Option<String>,
        bytes: Arc<Vec<u8>>,
        info: DocInfo,
        cos: Result<Result<pdfcraft_cos::Document, pdfcraft_cos::CosError>, Box<dyn std::any::Any + Send>>,
        render_password: Option<String>,
        password: Option<&str>,
        xfa: Option<XfaLayout>,
    ) -> Document {
        let config = RenderConfig { password: render_password.as_deref().map(Arc::from), ..Default::default() };
        let (mut editor, read_only_reason) = match cos {
            Ok(Ok(cos)) => {
                let keys = Keys { render: render_password.clone(), reopen: password.map(str::to_owned) };
                let saved = cos.bytes().clone();
                (
                    Some(Editor {
                        cos,
                        saved,
                        saved_keys: keys.clone(),
                        unsaved_file: false,
                        undo: Vec::new(),
                        redo: Vec::new(),
                        keys,
                        appearances: AppearanceCache::default(),
                        field_appearances: pdfcraft_forms::FieldAppearanceCache::default(),
                        text_blocks: TextBlocksCache::default(),
                    }),
                    None,
                )
            }
            Ok(Err(e)) => (None, Some(e.to_string())),
            Err(_) => (None, Some("the document structure could not be read for editing".into())),
        };
        let sig_cache = Arc::new(pdfcraft_sign::DigestCache::default());
        let signatures = editor.as_ref().map(|e| signatures_of(&e.cos, &bytes, trust, &sig_cache)).unwrap_or_default();
        let display = display_bytes(editor.as_mut(), &bytes);
        let renderer = RenderPool::new(display.clone(), render_threads(), config.clone());
        let (form, adopted) = editor
            .as_ref()
            .map(|e| {
                let mut fields = pdfcraft_forms::fields_with_adopted(&e.cos);
                xfa::mark_script_buttons(&e.cos, &mut fields.items);
                (fields.items, fields.adopted_page_fields)
            })
            .unwrap_or_default();
        // Fields the file lists only as page widgets are adopted leniently; the file says so.
        if adopted > 0
            && let Some(e) = &mut editor
        {
            let line = if adopted == 1 {
                "the form's /Fields list omits 1 field; it was read from the page annotations".to_string()
            } else {
                format!("the form's /Fields list omits {adopted} fields; they were read from the page annotations")
            };
            e.cos.note_repair(line);
        }
        Document {
            id: DocId(0),
            name,
            path,
            bytes,
            display,
            info,
            renderer,
            password: render_password,
            dirty: false,
            generation: 0,
            snapshot_generation: 0,
            read_only_reason,
            form: Arc::new(form),
            marker_scan: OnceLock::new(),
            links: OnceLock::new(),
            signatures: Arc::new(signatures),
            trust: trust.clone(),
            sig_cache,
            editor,
            config,
            js_output: Default::default(),
            xfa,
            xfa_template: None,
            xfa_scripts_off: false,
            fonts_stale: false,
            xfa_warnings: Vec::new(),
            edit_notices: Vec::new(),
        }
    }

    fn doc_mut(&mut self, id: DocId) -> Result<&mut Document, EditError> {
        self.docs.iter_mut().find(|d| d.id == id).ok_or(EditError::NoDocument)
    }

    /// Apply an edit. On success the previous state is undoable and the view data is refreshed.
    pub fn apply(&mut self, id: DocId, edit: Edit) -> Result<(), EditError> {
        let now = self.now();
        let today = self.today();
        let js_off = self.js_off;
        let doc = self.doc_mut(id)?;
        doc.edit_notices.clear();
        let name = doc.name.clone();
        let is_xfa = doc.info.xfa.is_some();
        let mut scope = scope_of(&edit);
        let mut cx = EditCtx::new(now, doc.generation ^ (id.0 << 48));
        if !js_off {
            cx.xfa = doc.xfa_template.clone();
        }
        cx.pages = doc.info.pages.len();
        cx.today = today;
        let reason = doc.read_only_reason.clone().unwrap_or_default();
        let signed = doc.is_signed();
        let editor = doc.editor.as_mut().ok_or(EditError::ReadOnly(reason))?;
        if let Some(p) = editor.cos.permissions() {
            check_permission(&edit, &p)?;
        }
        // Rewriting an OCR word (invisible text over a scan) also hides the scanned word: cover it
        // in the paper's colour and draw the new text in the ink's.
        let edit = match edit {
            Edit::EditTextBlock { page, block, text, mut style } => {
                // Only invisible text is sampled; the blocks are usually cached from opening it.
                if style.cover.is_none()
                    && let Some(info) = doc.info.pages.get(page)
                    && let Some(b) = editor.text_blocks(page).ok().and_then(|b| b.get(block).filter(|b| b.invisible).cloned())
                    && let Some((paper, ink)) = guard(|| scan_colours::sample(&editor.cos, info, page, &b)).ok().flatten()
                {
                    style.cover = Some(paper);
                    style.color = style.color.or(Some(ink));
                }
                Edit::EditTextBlock { page, block, text, style }
            }
            other => other,
        };
        // Nothing to bake: leave undo history and the dirty flag alone.
        if matches!(&edit, Edit::FlattenFillSign) && !pdfcraft_annot::has_visible_fill_sign(&editor.cos) {
            return Ok(());
        }
        let mut next = editor.cos.clone();
        if !js_off && uses_scripts(&edit) {
            cx.js = Some(js::JsRunner::new(&next, &name));
        }
        cx.renamed_fields.clear();
        cx.redaction_overlays.clear();
        // `next` is a copy: if the edit fails or crashes, the document is unchanged.
        guard(|| run_edit(&mut next, &edit, &mut cx))
            .unwrap_or_else(|m| Err(EditError::Invalid(format!("{} failed unexpectedly ({m}); the document was not changed", edit.label()))))?;
        // Redaction removed what its own interpreter places under the marks. The marks were drawn
        // over the page as the renderer shows it, so the renderer has the last word.
        if let Edit::ApplyRedactions { pages } = &edit {
            let marks: Vec<pdfcraft_redact::Mark> =
                pdfcraft_redact::marks(&editor.cos).into_iter().filter(|m| pages.as_ref().is_none_or(|p| p.contains(&m.page))).collect();
            guard(|| shown_under_redactions(&next, &marks, &cx.redaction_overlays, &doc.info.pages, doc.password.as_deref()))
                .unwrap_or_else(|m| Err(EditError::Invalid(format!("{} failed unexpectedly ({m}); the document was not changed", edit.label()))))?;
        }
        // Renamed fields are said, not slipped in: a repair-log note (with the caveat that
        // scripts were left alone) and a count in the undo step's label.
        let renamed_note = |renamed: &[(String, String)]| {
            (!renamed.is_empty()).then(|| {
                let shown: Vec<String> = renamed.iter().take(5).map(|(old, new)| format!("{old} → {new}")).collect();
                let more = renamed.len().saturating_sub(shown.len());
                let more = if more > 0 { format!(", and {more} more") } else { String::new() };
                format!(
                    "inserted pages renamed {} form field{} to keep their names separate ({}{more}); scripts that refer to the old names were not rewritten",
                    renamed.len(),
                    if renamed.len() == 1 { "" } else { "s" },
                    shown.join(", ")
                )
            })
        };
        if let Some(note) = renamed_note(&cx.renamed_fields) {
            next.note_repair(note);
        }
        // Only Insert from File renames fields; its label says so in a wording the interface
        // translates (the count lives in the repair note).
        let label = match (&edit, cx.renamed_fields.len()) {
            (_, 0) => edit.label(),
            (Edit::InsertPagesFrom { name, .. }, n) => Label::with(
                if n == 1 { "Insert pages from {name} — 1 field renamed" } else { "Insert pages from {name} — fields renamed" },
                name.as_str(),
            ),
            _ => edit.label(),
        };
        // An XFA form's scripts answer the change: exit and validate scripts of the field, then
        // every calculation (and a reset recalculates). They read the data, so it is brought up
        // to date first.
        if let Some(tpl) = cx.xfa.clone() {
            if is_xfa && matches!(scope_of(&edit), Scope::Form | Scope::Full | Scope::Content) {
                let datasets = &mut cx.xfa_datasets;
                let notes = guard(|| xfa_sync_datasets(&mut next, Some(&tpl), datasets))
                    .unwrap_or_else(|m| Err(format!("writing the XFA data failed unexpectedly ({m})")))
                    .map_err(EditError::Write)?;
                cx.xfa_out.errors.extend(notes);
            }
            fn collect(e: &Edit, changed: &mut Vec<String>, reset: &mut bool) {
                match e {
                    Edit::SetFieldValue { name, .. } => changed.push(name.clone()),
                    Edit::ResetForm { .. } => *reset = true,
                    Edit::Batch { edits, .. } => edits.iter().for_each(|e| collect(e, changed, reset)),
                    _ => {}
                }
            }
            let mut changed: Vec<String> = Vec::new();
            let mut reset = false;
            collect(&edit, &mut changed, &mut reset);
            if !changed.is_empty() || reset {
                let mut run = xfa::XfaRun { page_count: cx.pages, out: &mut cx.xfa_out, datasets: cx.xfa_datasets, ran_away: false };
                // Every changed field's scripts, then the calculations once (after a reset,
                // only those).
                let ran = guard(|| xfa::on_changes(&mut next, &tpl, &changed, &mut run))
                    .unwrap_or_else(|m| Err(format!("the form's scripts failed unexpectedly ({m})")));
                cx.xfa_datasets = run.datasets;
                cx.xfa_ran_away |= run.ran_away;
                if let Err(e) = ran {
                    cx.xfa_out.errors.push(e);
                }
            }
        }
        // XFA forms keep their values in the datasets packet too, for Adobe's viewers.
        let mut xfa_notes = Vec::new();
        if is_xfa && matches!(scope_of(&edit), Scope::Form | Scope::Full | Scope::Content) {
            let (tpl, datasets) = (cx.xfa.as_deref(), &mut cx.xfa_datasets);
            xfa_notes = guard(|| xfa_sync_datasets(&mut next, tpl, datasets))
                .unwrap_or_else(|m| Err(format!("writing the XFA data failed unexpectedly ({m})")))
                .map_err(EditError::Write)?;
            // Rebuilding from stale data would discard accepted widget values.
            if xfa_notes.is_empty() {
                let reflow = guard(|| xfa::relayout_changed_text(&mut next, tpl, &doc.form))
                    .unwrap_or_else(|m| Err(format!("laying out the XFA text failed unexpectedly ({m})")))
                    .map_err(EditError::Write)?;
                if let Some(warnings) = reflow {
                    xfa_notes.extend(warnings);
                    // Layout replaces pages and widgets, even if the page count is unchanged.
                    // Undo/redo must rebuild these too instead of refreshing only field values.
                    scope = Scope::Full;
                }
            }
        }
        // Signed documents are only ever saved incrementally: an edit that needs a full rewrite
        // (applying redactions, changing security, sanitizing) would invalidate the signatures.
        // So does any edit to a file rebuilt from object headers: it has no chain to append to.
        let rewrites = |c: &pdfcraft_cos::Document| c.full_save_required() || c.encryption_changed();
        if signed && ((rewrites(&next) && !rewrites(&editor.cos)) || (next.is_modified() && next.revisions().is_empty())) {
            return Err(EditError::SignedRewrite(edit.label().to_string()));
        }
        // What this edit drew with `?` (a value or comment text its font can't draw, stored as
        // typed): reported, never silent (AGENTS.md §4).
        let notices = if matches!(scope_of(&edit), Scope::Form | Scope::Comments | Scope::Full) {
            guard(|| undrawn_notices(&editor.cos, &next)).unwrap_or_default()
        } else {
            Vec::new()
        };
        let previous = std::mem::replace(&mut editor.cos, next);
        let keys = keys_after(&edit).unwrap_or_else(|| editor.keys.clone());
        let previous_keys = std::mem::replace(&mut editor.keys, keys);
        editor.undo.push((label, previous, previous_keys, scope));
        if editor.undo.len() > MAX_UNDO {
            editor.undo.remove(0);
        }
        editor.redo.clear();
        Self::adopt_keys(doc);
        let refreshed = guard(|| if scope == Scope::Content { Self::refresh_content(doc, Some(&edit)) } else { Self::refresh_scoped(doc, scope) })
            .unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if let Err(e) = refreshed {
            // Roll back: the edit produced something we cannot display.
            if let Some(ed) = doc.editor.as_mut()
                && let Some((_, prev, keys, _)) = ed.undo.pop()
            {
                ed.cos = prev;
                ed.keys = keys;
            }
            Self::adopt_keys(doc);
            if !matches!(guard(|| Self::refresh(doc)), Ok(Ok(()))) {
                Self::show_released(doc);
            }
            return Err(e);
        }
        doc.dirty = true;
        doc.generation += 1;
        doc.edit_notices = notices;
        if !cx.xfa_removed.is_empty() {
            let removed: Vec<String> = cx.xfa_removed.iter().map(|n| format!("{XFA_REMOVED_PREFIX}{n}")).collect();
            doc.xfa_warnings.retain(|w| w.starts_with(XFA_REMOVED_PREFIX));
            note_warnings(&mut doc.xfa_warnings, &removed);
            note_warnings(&mut doc.info.warnings, &removed);
        }
        note_warnings(&mut doc.xfa_warnings, &xfa_notes);
        let notes = doc.xfa_warnings.clone();
        note_warnings(&mut doc.info.warnings, &notes);
        if let Some(js) = cx.js {
            doc.js_output.append(js.output);
        }
        doc.js_output.append(cx.xfa_out);
        if cx.xfa_ran_away {
            doc.xfa_template = None;
            doc.xfa_scripts_off = true;
            doc.js_output.errors.push(XFA_SCRIPTS_OFF.into());
        }
        Ok(())
    }

    pub fn undo(&mut self, id: DocId) -> Result<Label, EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or(EditError::NothingToUndo)?;
        let (label, prev, keys, scope) = editor.undo.pop().ok_or(EditError::NothingToUndo)?;
        let current = std::mem::replace(&mut editor.cos, prev);
        let current_keys = std::mem::replace(&mut editor.keys, keys);
        editor.redo.push((label.clone(), current, current_keys, scope));
        Self::adopt_keys(doc);
        let refreshed = guard(|| Self::refresh_scoped(doc, scope)).unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if let Err(e) = refreshed {
            if let Some(ed) = doc.editor.as_mut()
                && let Some((label, current, current_keys, scope)) = ed.redo.pop()
            {
                let prev = std::mem::replace(&mut ed.cos, current);
                let keys = std::mem::replace(&mut ed.keys, current_keys);
                ed.undo.push((label, prev, keys, scope));
            }
            Self::adopt_keys(doc);
            if !matches!(guard(|| Self::refresh(doc)), Ok(Ok(()))) {
                Self::show_released(doc);
            }
            return Err(e);
        }
        doc.edit_notices.clear();
        Self::sync_dirty(doc);
        doc.generation += 1;
        Ok(label)
    }

    pub fn redo(&mut self, id: DocId) -> Result<Label, EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or(EditError::NothingToRedo)?;
        let (label, next, keys, scope) = editor.redo.pop().ok_or(EditError::NothingToRedo)?;
        let current = std::mem::replace(&mut editor.cos, next);
        let current_keys = std::mem::replace(&mut editor.keys, keys);
        editor.undo.push((label.clone(), current, current_keys, scope));
        Self::adopt_keys(doc);
        let refreshed = guard(|| Self::refresh_scoped(doc, scope)).unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if let Err(e) = refreshed {
            if let Some(ed) = doc.editor.as_mut()
                && let Some((label, current, current_keys, scope)) = ed.undo.pop()
            {
                let next = std::mem::replace(&mut ed.cos, current);
                let keys = std::mem::replace(&mut ed.keys, current_keys);
                ed.redo.push((label, next, keys, scope));
            }
            Self::adopt_keys(doc);
            if !matches!(guard(|| Self::refresh(doc)), Ok(Ok(()))) {
                Self::show_released(doc);
            }
            return Err(e);
        }
        doc.edit_notices.clear();
        Self::sync_dirty(doc);
        doc.generation += 1;
        Ok(label)
    }

    /// Unsaved when the working graph is not the last opened or saved file. Undo back to that
    /// file is clean; undo past a later save is not, because the restored snapshot's own base
    /// is an earlier revision.
    fn sync_dirty(doc: &mut Document) {
        doc.dirty = doc.editor.as_ref().is_some_and(|e| e.unsaved_file || e.cos.is_modified() || !Arc::ptr_eq(e.cos.bytes(), &e.saved));
    }

    /// Use the current state's password for viewing (protection edits change it).
    fn adopt_keys(doc: &mut Document) {
        if let Some(e) = doc.editor.as_ref() {
            doc.password = e.keys.render.clone();
            doc.config.password = e.keys.render.as_deref().map(Arc::from);
        }
    }

    /// Rebuild the working bytes and the view data that `scope` may have changed. Metadata edits
    /// keep the renderer; other scopes also replace it.
    /// Comment and form edits skip the full re-inspection (seconds on very large files): the
    /// comment list and field values are re-read from the object graph instead.
    fn refresh_scoped(doc: &mut Document, scope: Scope) -> Result<(), EditError> {
        if scope == Scope::Full || doc.info.encrypted != doc.editor.as_ref().is_some_and(|e| e.cos.output_handler().is_some()) {
            return Self::refresh(doc);
        }
        if scope == Scope::Content {
            return Self::refresh_content(doc, None);
        }
        if scope == Scope::Links {
            return Self::refresh_links(doc);
        }
        // Metadata edits keep the renderer, which holds the working file anyway.
        if scope != Scope::Metadata {
            Self::release_working(doc);
        }
        let Some(editor) = doc.editor.as_mut() else { return Ok(()) };
        // Comment edits, and form edits (a script may change annotation properties), may change
        // annotation dictionaries, invalidating cached appearances.
        if matches!(scope, Scope::Comments | Scope::Form) {
            editor.appearances.clear();
        }
        let bytes = if editor.cos.is_modified() {
            Arc::new(write_incremental(&editor.cos, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?)
        } else {
            editor.cos.bytes().clone()
        };
        if scope == Scope::Metadata {
            // Read the way the full inspection reads them (NULs and spaces trimmed, empty is
            // none), so editing one entry doesn't change how the others display.
            let info = |key| pdfcraft_organize::info(&editor.cos, key).map(|v| v.trim_matches('\0').trim().to_string()).filter(|v| !v.is_empty());
            doc.info.title = info("Title");
            doc.info.author = info("Author");
            doc.info.subject = info("Subject");
            doc.info.keywords = info("Keywords");
            doc.info.creator = info("Creator");
            doc.info.producer = info("Producer");
            doc.info.custom = pdfcraft_organize::custom_info(&editor.cos);
            doc.info.file_size = bytes.len();
            #[cfg(test)]
            check_refresh_panic();
            if !doc.signatures.is_empty() {
                let signatures = signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache);
                doc.signatures = Arc::new(signatures);
            }
            doc.bytes = bytes;
            return Ok(());
        }
        let mut form = pdfcraft_forms::fields(&editor.cos);
        xfa::mark_script_buttons(&editor.cos, &mut form);
        let form = Arc::new(form);
        match scope {
            Scope::Metadata => return Ok(()),
            Scope::Comments => {
                doc.info.annotations = comment_list(&editor.cos);
                // Link indexes are positions in /Annots, so comment insertion/deletion can move them.
                doc.links = OnceLock::new();
            }
            Scope::Form => {
                // PERF: keystroke path, index once. A linear find per field is O(F^2);
                // forms reach 10k fields.
                let mut by_name = std::collections::HashMap::with_capacity(form.len());
                for x in form.iter() {
                    // First match wins (do not simplify to collect: it keeps the last).
                    by_name.entry(x.name.as_str()).or_insert(x);
                }
                for f in &mut doc.info.fields {
                    if let Some(ff) = by_name.get(f.name.as_str()) {
                        f.value = match ff.kind {
                            pdfcraft_forms::FieldKind::CheckBox | pdfcraft_forms::FieldKind::Radio => {
                                Some(ff.value.first().cloned().unwrap_or_else(|| "Off".into()))
                            }
                            _ if ff.value.is_empty() => None,
                            _ => Some(ff.value.join(", ")),
                        };
                    }
                }
            }
            Scope::Links => return Self::refresh_links(doc),
            // Handled above; kept as a full refresh so the match stays total.
            Scope::Full => return Self::refresh(doc),
            // Handled above (content-only refresh); the arm keeps the match total.
            Scope::Content => return Self::refresh_content(doc, None),
        }
        doc.info.file_size = bytes.len();
        doc.form = form;
        let signatures = if doc.signatures.is_empty() { Vec::new() } else { signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache) };
        doc.display = display_bytes(Some(editor), &bytes);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.bytes = bytes;
        Ok(())
    }

    /// Which derived lists a content-only edit can change: added content (`None`, from
    /// undo/redo, re-reads both: always correct, rarely paid).
    fn content_refresh_lists(edit: Option<&Edit>) -> (bool, bool) {
        let touches_added = edit.is_none_or(|e| {
            matches!(
                e,
                Edit::AddText { .. } | Edit::AddImage { .. } | Edit::UpdateContent { .. } | Edit::DeleteContent { .. } | Edit::ReplaceImage { .. }
            ) || matches!(e, Edit::MoveObjects { objects, .. } if objects.iter().any(|o| o.kind == ObjectKind::Added))
        });
        let touches_marks = edit.is_none_or(|e| {
            matches!(e, Edit::AddHeaderFooter { .. } | Edit::AddWatermark { .. } | Edit::AddBackground { .. } | Edit::RemoveMarks { .. })
        });
        (touches_added, touches_marks)
    }

    /// Rebuild working bytes, display bytes and renderer after a content-only edit: page
    /// content streams, resources and added content/marks. The page tree, annotations, form
    /// fields, links, outline and fonts are unchanged, so the full re-inspection (seconds on
    /// very large files) is skipped. `edit` selects which derived lists are re-read (`None`,
    /// from undo/redo, re-reads both: always correct, rarely paid). The cached display
    /// appearances are kept: content edits leave every annotation dictionary alone.
    fn refresh_content(doc: &mut Document, edit: Option<&Edit>) -> Result<(), EditError> {
        Self::release_working(doc);
        sync_xfa_state(doc);
        let Some(editor) = doc.editor.as_mut() else { return Ok(()) };
        let bytes = if editor.cos.is_modified() {
            Arc::new(write_incremental(&editor.cos, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?)
        } else {
            editor.cos.bytes().clone()
        };
        // The edit must still leave a file the viewer can open; anything else rolls back above.
        pdfcraft_render::check_pages(bytes.clone(), doc.password.as_deref(), doc.info.pages.len()).map_err(|e| EditError::Reopen(e.to_string()))?;
        let (touches_added, touches_marks) = Self::content_refresh_lists(edit);
        if touches_added || touches_marks {
            doc.marker_scan = OnceLock::new();
        }
        doc.info.file_size = bytes.len();
        let signatures = if doc.signatures.is_empty() { Vec::new() } else { signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache) };
        doc.display = display_bytes(Some(editor), &bytes);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.bytes = bytes;
        doc.fonts_stale = true;
        Ok(())
    }

    /// Let go of the working file before the next one is written (gap 13). The working file is
    /// the original plus one appended revision, so on a 1 GiB file keeping the old one (in the
    /// working bytes, the display bytes and the renderer) while writing the new one held three
    /// copies at once; this keeps two. Until the refresh finishes, the working and display bytes
    /// are the original's (already held by the editor, so free) and the renderer draws nothing.
    /// A failed edit rolls back and refreshes again, which replaces all three; where a refresh
    /// still fails, [`Self::show_released`] draws the original rather than nothing. Save writes
    /// from the editor, never from these.
    fn release_working(doc: &mut Document) {
        let Some(original) = doc.editor.as_ref().map(|e| e.cos.bytes().clone()) else { return };
        // Dropping the pool stops its workers; each lets go of the file when it exits.
        doc.renderer = RenderPool::new_inline(Arc::new(Vec::new()), doc.config.clone());
        doc.display = original.clone();
        doc.bytes = original;
        #[cfg(test)]
        check_refresh_panic();
    }

    /// After a refresh failed: draw the display bytes it left (the original, once released),
    /// so the page still shows a document.
    fn show_released(doc: &mut Document) {
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
    }

    /// Rebuild working bytes, inspection and renderer from the current edit state.
    fn refresh(doc: &mut Document) -> Result<(), EditError> {
        Self::refresh_with_indexes(doc, true, true)
    }

    /// Reinspect a link edit while preserving the page-content marker scan cache.
    fn refresh_links(doc: &mut Document) -> Result<(), EditError> {
        Self::refresh_with_indexes(doc, false, true)
    }

    fn refresh_with_indexes(doc: &mut Document, invalidate_markers: bool, invalidate_links: bool) -> Result<(), EditError> {
        // A full refresh may follow structural or annotation changes: cached appearances
        // may be stale, so they are rebuilt below.
        if let Some(editor) = doc.editor.as_mut() {
            editor.appearances.clear();
        }
        Self::release_working(doc);
        sync_xfa_state(doc);
        let Some(editor) = doc.editor.as_mut() else { return Ok(()) };
        let bytes = if editor.cos.is_modified() {
            Arc::new(write_incremental(&editor.cos, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?)
        } else {
            editor.cos.bytes().clone()
        };
        let info = inspect(bytes.clone(), doc.password.as_deref()).map_err(|e| EditError::Reopen(e.to_string()))?;
        // Keep the user's layer choices where the layers still exist.
        let mut info = info;
        for l in &mut info.layers {
            if let Some(old) = doc.info.layers.iter().find(|o| o.id == l.id) {
                l.visible = old.visible;
            }
        }
        note_warnings(&mut info.warnings, &doc.xfa_warnings);
        doc.info = info;
        doc.fonts_stale = false;
        let mut form = pdfcraft_forms::fields(&editor.cos);
        xfa::mark_script_buttons(&editor.cos, &mut form);
        doc.form = Arc::new(form);
        if invalidate_markers {
            doc.marker_scan = OnceLock::new();
        }
        if invalidate_links {
            doc.links = OnceLock::new();
        }
        let signatures = signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache);
        doc.display = display_bytes(Some(editor), &bytes);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.bytes = bytes;
        Ok(())
    }

    /// The bytes to write for Save: an incremental update of the file as opened/last saved,
    /// with `/ModDate` stamped. Call `mark_saved` after writing them successfully.
    ///
    /// A file whose cross-reference data had to be rebuilt is written in full even when nothing
    /// was edited: its bytes as opened are the damaged file, and this save is how the repair the
    /// user was told about reaches the disk (#1937). Not a signed one: a full rewrite would
    /// break its signatures, so with no edits it keeps its bytes, damage and all.
    pub fn save_bytes(&self, id: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let Some(editor) = doc.editor.as_ref() else { return Ok(doc.bytes.clone()) };
        let on_saved = Arc::ptr_eq(editor.cos.bytes(), &editor.saved);
        if !editor.cos.is_modified() && on_saved && (!editor.cos.reconstructed() || doc.is_signed()) {
            return Ok(editor.saved.clone());
        }
        let opts = SaveOptions { mod_date: self.now().map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        // A restored snapshot can be based on an earlier revision. Append to the last saved
        // file so that revision stays in the chain. A full rewrite (redaction, new encryption,
        // a reconstructed file) still writes the current graph on its own.
        let rewrite = editor.cos.full_save_required() || editor.cos.encryption_changed() || editor.cos.revisions().is_empty();
        guard(|| {
            if on_saved || rewrite {
                write_incremental(&editor.cos, &opts)
            } else {
                let mut base = pdfcraft_cos::Document::open_with_password(editor.saved.clone(), editor.saved_keys.reopen.as_deref())?;
                base.overlay_changes_from(&editor.cos)?;
                write_incremental(&base, &opts)
            }
        })
        .map_err(EditError::Write)?
        .map(Arc::new)
        .map_err(|e| EditError::Write(e.to_string()))
    }

    /// A compact, garbage-collected rewrite (Save As ▸ "Optimized" / Reduce File Size groundwork).
    pub fn save_full_bytes(&self, id: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if doc.is_signed() {
            return Err(EditError::Signed);
        }
        let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let opts = SaveOptions { mod_date: self.now().map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        guard(|| write_full(&editor.cos, &opts)).map_err(EditError::Write)?.map(Arc::new).map_err(|e| EditError::Write(e.to_string()))
    }

    /// Check that `bytes`, from [`Self::save_bytes`] or [`Self::save_full_bytes`], reopen as this
    /// document before they are written (#1698): [`Self::mark_saved`] only finds out after the
    /// file has been replaced. Same error as `mark_saved`.
    pub fn check_save(&self, id: DocId, bytes: &Arc<Vec<u8>>) -> Result<(), EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let Some(editor) = doc.editor.as_ref() else { return Ok(()) };
        pdfcraft_cos::Document::open_with_password(bytes.clone(), editor.keys.reopen.as_deref()).map_err(|e| EditError::Reopen(e.to_string()))?;
        pdfcraft_render::check_pages(bytes.clone(), doc.password.as_deref(), doc.info.pages.len()).map_err(|e| EditError::Reopen(e.to_string()))
    }

    /// Record a successful save of `bytes` (to `path`, if any): rebase editing on the saved file
    /// so the next save appends only newer edits. Undo history is kept.
    pub fn mark_saved(&mut self, id: DocId, bytes: Arc<Vec<u8>>, path: Option<String>) -> Result<(), EditError> {
        let doc = self.doc_mut(id)?;
        if let Some(editor) = doc.editor.as_mut() {
            // An encrypted file needs the password it is protected with now (the owner
            // password when known, so saving never downgrades this session's rights).
            let keys = editor.keys.clone();
            editor.cos =
                pdfcraft_cos::Document::open_with_password(bytes.clone(), keys.reopen.as_deref()).map_err(|e| EditError::Reopen(e.to_string()))?;
            editor.saved = bytes;
            editor.saved_keys = keys;
            editor.unsaved_file = false;
        }
        if let Some(p) = path {
            doc.name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
            doc.path = Some(p);
        }
        doc.dirty = false;
        doc.generation += 1;
        Self::refresh(doc).inspect_err(|_| Self::show_released(doc))
    }

    /// Enter the permissions (owner) password of a document opened with its user password,
    /// lifting its restrictions for this session (Acrobat asks for it before changing security).
    /// Not an edit: nothing is marked unsaved and there is no undo step. Undo and redo states
    /// are unlocked as well, and saving keeps the rights. Unencrypted documents and documents
    /// opened by the owner need nothing.
    pub fn unlock(&mut self, id: DocId, password: &str) -> Result<(), EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        if editor.cos.permissions().is_none_or(|p| p.owner) {
            return Ok(());
        }
        editor.cos.unlock(password).map_err(|e| match e {
            pdfcraft_cos::CosError::WrongPassword => EditError::WrongPassword,
            other => EditError::Protection(other.to_string()),
        })?;
        // Saving a state that keeps the security it was opened with reopens it with the same
        // passwords, so it reopens as owner.
        let reopen_as_owner = |cos: &pdfcraft_cos::Document, keys: &mut Keys| {
            if !cos.encryption_changed() {
                keys.reopen = Some(password.to_owned());
            }
        };
        reopen_as_owner(&editor.cos, &mut editor.keys);
        // Other states with the same restricted access are unlocked too; states opened with
        // other passwords (protection changed and saved since) stay as they are.
        for (_, cos, keys, _) in editor.undo.iter_mut().chain(editor.redo.iter_mut()) {
            if cos.permissions().is_some_and(|p| !p.owner) && cos.unlock(password).is_ok() {
                reopen_as_owner(cos, keys);
            }
        }
        Ok(())
    }

    /// File ▸ Revert: back to the last saved version (or the file as opened). Undo history is
    /// cleared, as in Acrobat.
    pub fn revert(&mut self, id: DocId) -> Result<(), EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let base = editor.saved.clone();
        // The saved file opens with the passwords it was saved with. The current snapshot may
        // be an older revision (undo past that save), so its own base is not the saved file.
        let mut candidates = vec![editor.saved_keys.clone(), editor.keys.clone()];
        candidates.extend(editor.undo.iter().rev().map(|s| s.2.clone()));
        candidates.push(Keys::default());
        let (cos, keys) = candidates
            .into_iter()
            .find_map(|k| pdfcraft_cos::Document::open_with_password(base.clone(), k.reopen.as_deref()).ok().map(|c| (c, k)))
            .ok_or_else(|| EditError::Reopen("the saved file can't be opened".into()))?;
        editor.cos = cos;
        editor.keys = keys;
        editor.undo.clear();
        editor.redo.clear();
        Self::adopt_keys(doc);
        Self::sync_dirty(doc);
        doc.generation += 1;
        Self::refresh(doc).inspect_err(|_| Self::show_released(doc))
    }

    /// The document's file changed on disk (a LaTeX or Typst build rewrote it, #431): open
    /// `bytes`, the file as it is now, in its place. Everything opening does runs again (XFA
    /// layout and scripts, signatures, repairs), but the document keeps its id, name and path,
    /// so its tab and view stay with it. Undo history goes, as with Revert. The password it was
    /// opened with is tried; on any failure the document is left as it was.
    pub fn reload(&mut self, id: DocId, bytes: Arc<Vec<u8>>) -> Result<(), OpenError> {
        let old = self.docs.iter().position(|d| d.id == id).ok_or_else(|| OpenError::Invalid("the document is not open".into()))?;
        let doc = &self.docs[old];
        let (name, path, generation) = (doc.name.clone(), doc.path.clone(), doc.generation);
        let password = doc.editor.as_ref().and_then(|e| e.keys.reopen.clone()).or_else(|| doc.password.clone());
        let new_id = self.open(name, path, bytes, password.as_deref())?;
        let at = self.docs.iter().position(|d| d.id == new_id).ok_or_else(|| OpenError::Invalid("the new version was not kept".into()))?;
        let mut doc = self.docs.remove(at);
        doc.id = id;
        // How the document is shown is the reader's, not the file's: reading colours and hidden
        // comments stay as they were (#3443). `open` built the renderer with the defaults.
        if let Some(old) = self.docs.get(old)
            && (doc.config.reading_colors != old.config.reading_colors || doc.config.hide_comments != old.config.hide_comments)
        {
            doc.config.reading_colors = old.config.reading_colors;
            doc.config.hide_comments = old.config.hide_comments;
            doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        }
        // Caches keyed by the edit generation (edit lines, images, measurements) must see a new
        // one; there is nothing new to autosave.
        doc.generation = generation + 1;
        doc.snapshot_generation = doc.generation;
        // `open` pushed the new version last, so the old one is still at `old`.
        *self.docs.get_mut(old).ok_or_else(|| OpenError::Invalid("the document is not open".into()))? = doc;
        Ok(())
    }

    /// The page count of another PDF (Replace Pages, Insert Pages dialogs).
    pub fn page_count_of(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<usize, EditError> {
        Ok(pdfcraft_organize::page_count(&open_source(name, bytes)?)?)
    }

    /// A new blank document (Create ▸ Blank page).
    pub fn create_blank(&self, width: f64, height: f64, pages: usize) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::blank(width, height, pages)?)
    }

    /// A new document with one page per image (PNG, JPEG).
    pub fn create_from_images(&self, images: &[(String, Vec<u8>)]) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::from_images(images)?)
    }

    /// Create image pages at embedded resolution or a fixed dpi, without resampling.
    pub fn create_from_images_with_resolution(&self, images: &[(String, Vec<u8>)], resolution: ImageResolution) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::from_images_with_resolution(images, resolution)?)
    }

    /// A new document from plain text (US Letter, 11 pt Helvetica). Text Helvetica can't draw is
    /// refused, naming the line and the character (never set as `?`).
    pub fn create_from_text(&self, title: &str, text: &str) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::from_text(title, text, pdfcraft_create::LETTER, 11.0)?)
    }

    /// [`Self::create_from_text`] for a report PdfCraft writes from documents (comment summaries,
    /// compare reports), which can quote any language: characters Helvetica can't draw are written
    /// as `[U+XXXX]`, and the report's first line says so, rather than the report being refused.
    fn create_report(&self, title: &str, text: &str) -> Result<Arc<Vec<u8>>, EditError> {
        let (doc, _escaped) = pdfcraft_create::from_text_escaped(title, text, pdfcraft_create::LETTER, 11.0)?;
        self.write_new(&doc)
    }

    /// Convert a file Create understands (an image or plain text) to PDF bytes; a PDF is checked
    /// (it must open and allow copying pages) and returned as it is.
    pub fn convert_to_pdf(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<(SourceKind, Arc<Vec<u8>>), EditError> {
        let Some(kind) = source_kind(name, bytes) else {
            return Err(EditError::Source(format!("{name}: this file type can't be converted; use a PDF, an image or a .txt file")));
        };
        let title = name.rsplit_once('.').map_or(name, |(s, _)| s);
        // Image decoders read untrusted bytes: a panic in one must not take the app down.
        let created = guard(|| match kind {
            SourceKind::Pdf => open_source(name, bytes).map(|_| bytes.clone()),
            SourceKind::Image => self.create_from_images(&[(name.to_string(), bytes.to_vec())]),
            SourceKind::Text => self.create_from_text(title, &decode_text(bytes)),
        })
        .map_err(|_| EditError::Source(format!("{name}: the file could not be read")))?;
        Ok((kind, created?))
    }

    /// A file for Combine as PDF bytes: an image or text file is converted; a PDF is returned as
    /// it is, unchecked (Combine reads it itself, with its password), as is a file named .pdf
    /// without a `%PDF-` header, which the reader may still open.
    pub fn as_pdf(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, EditError> {
        let pdf = match source_kind(name, bytes) {
            Some(SourceKind::Pdf) => true,
            Some(SourceKind::Image | SourceKind::Text) => false,
            None => name.to_ascii_lowercase().ends_with(".pdf"),
        };
        if pdf { Ok(bytes.clone()) } else { self.convert_to_pdf(name, bytes).map(|(_, pdf)| pdf) }
    }

    /// Reduce File Size: Acrobat's defaults (images above 225 ppi to 150 ppi, JPEG medium
    /// quality; thumbnails dropped), identical resources merged, unused objects dropped, objects
    /// packed into compressed object streams. Returns the bytes and how many objects were
    /// merged. The open document is not changed (Acrobat saves the reduced copy as a new file).
    pub fn reduced_bytes(&self, id: DocId) -> Result<(Arc<Vec<u8>>, usize), EditError> {
        let (bytes, report) = self.optimized_bytes(id, &optimize::Settings::default(), &[])?;
        Ok((bytes, report.merged))
    }

    /// Optimize PDF ▸ Advanced optimization: `settings` for images and objects, plus Remove
    /// Hidden Information's `discard` categories (user data). A full rewrite: signed documents
    /// are refused. The open document is not changed. This is [`Self::optimize_job`] run in
    /// place, without progress.
    pub fn optimized_bytes(&self, id: DocId, settings: &optimize::Settings, discard: &[Hidden]) -> Result<(Arc<Vec<u8>>, OptimizeReport), EditError> {
        self.optimize_job(id, settings, discard)?.run(|_| true)
    }

    /// Export comments and/or form data: XFDF and FDF carry either or both; XML, CSV and text
    /// are form data only.
    pub fn export_data(&self, id: DocId, format: DataFormat, comments: bool, fields: bool) -> Result<Vec<u8>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => {
                pdfcraft_cos::Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()).map_err(|e| EditError::Write(e.to_string()))?
            }
        };
        // An XFA form's data is its datasets, as Acrobat exports it: XML or an XDP.
        let xfa = doc.info.xfa.is_some();
        if format == DataFormat::Xdp && !xfa {
            return Err(EditError::Invalid("XDP holds XFA form data; this document is not an XFA form (use .xml, .csv or .txt)".into()));
        }
        guard(|| match format {
            DataFormat::Xfdf => Ok(pdfcraft_xfdf::export_xfdf(&cos, comments, fields, &doc.name).into_bytes()),
            DataFormat::Fdf => pdfcraft_xfdf::export_fdf(&cos, comments, fields, &doc.name).map_err(|e| e.to_string()),
            DataFormat::Xml | DataFormat::Xdp if xfa => {
                pdfcraft_xfa::export_data(&cos, format == DataFormat::Xdp, &doc.name).map(String::into_bytes).map_err(|e| e.to_string())
            }
            other => Ok(pdfcraft_xfdf::export_data(&cos, other).into_bytes()),
        })
        .map_err(EditError::Write)?
        .map_err(EditError::Write)
    }

    /// The print-ready PDF for `settings` (sheets laid out for the paper; see `pdfcraft-print`).
    pub fn print_pdf(&self, id: DocId, settings: &print::Settings) -> Result<Vec<u8>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if !doc.allows_printing() {
            return Err(EditError::NotPermitted("printing"));
        }
        // Table 22, bit 12 clear: only a low-level representation may be printed, so the pages
        // always go as pictures, at no more than the low resolution.
        let low_res = doc.permissions().is_some_and(|p| !p.print_high_quality()).then(|| print::Raster {
            dpi: settings.raster.map_or(LOW_RESOLUTION_PRINT_DPI, |r| r.dpi.min(LOW_RESOLUTION_PRINT_DPI)),
            grayscale: settings.raster.is_some_and(|r| r.grayscale),
        });
        // Print as image: the pages used become pictures first, then the picture document is
        // imposed in their place (its page k is the k-th distinct source page used).
        if let Some(raster) = low_res.as_ref().or(settings.raster.as_ref()) {
            let mut distinct: Vec<usize> = Vec::new();
            for &p in &settings.pages {
                if !distinct.contains(&p) {
                    distinct.push(p);
                }
            }
            let bytes = print_raster::rasterize(doc, &distinct, raster, settings.content).map_err(EditError::Print)?;
            let pictures = pdfcraft_cos::Document::open(Arc::new(bytes)).map_err(|e| EditError::Print(e.to_string()))?;
            let remapped = print::Settings {
                pages: settings.pages.iter().map(|p| distinct.iter().position(|d| d == p).unwrap_or(0)).collect(),
                content: print::Content::Document,
                raster: None,
                ..settings.clone()
            };
            return pdfcraft_print::impose(&pictures, &remapped).map_err(|e| EditError::Print(e.to_string()));
        }
        let mut cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => {
                pdfcraft_cos::Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()).map_err(|e| EditError::Write(e.to_string()))?
            }
        };
        // #3046: the layers print as they are shown. Once the Layers panel (or a set-layer
        // action) has changed one, the print copy's default configuration says which are on, so a
        // hidden layer stays off on paper too; untouched, the document's own configuration prints.
        if !doc.config.layers.is_empty() {
            set_default_layer_state(&mut cos, &doc.info.layers);
        }
        pdfcraft_print::impose(&cos, settings).map_err(|e| EditError::Print(e.to_string()))
    }

    /// For each page, whether its text mostly runs bottom-to-top ([`pdfcraft_print::text_reads_up`]);
    /// only `pages` are read, the others are `false`.
    pub fn print_text_up(&self, id: DocId, pages: &[usize]) -> Vec<bool> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => match pdfcraft_cos::Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()) {
                Ok(c) => c,
                Err(_) => return Vec::new(),
            },
        };
        (0..doc.info.pages.len()).map(|i| pages.contains(&i) && pdfcraft_print::text_reads_up(&cos, i)).collect()
    }

    /// Combine whole files, in order, into new PDF bytes (one bookmark per file).
    pub fn combine(&self, sources: &[(String, Arc<Vec<u8>>)]) -> Result<Arc<Vec<u8>>, EditError> {
        let docs = sources.iter().map(|(n, b)| open_source(n, b)).collect::<Result<Vec<_>, _>>()?;
        let named: Vec<(&str, &pdfcraft_cos::Document)> = sources.iter().map(|(n, _)| n.as_str()).zip(docs.iter()).collect();
        let out = pdfcraft_organize::combine(&named)?;
        self.write_new(&out)
    }

    /// Combine Files with a page range per file ("1-3, 6"; `None` or empty for all pages).
    pub fn combine_ranges(&self, sources: &[CombineSource]) -> Result<Arc<Vec<u8>>, EditError> {
        self.combine_unlocked(sources, &[])
    }

    /// Combine sources one at a time, in order (Create ▸ Multiple files), to the same result as
    /// [`Self::combine_ranges`]. Each item is a source's bookmark title, its PDF bytes (see
    /// [`Self::convert_to_pdf`]) and its page range (`None` for all pages). Items are pulled one at a
    /// time: a source is opened, its pages are copied into the result, and its bytes are released
    /// before the next item is read. So the number of sources is not limited, and one source's
    /// document is held at a time. An item's error stops the combine and is returned as it is.
    /// `Ok(None)` when there are no sources.
    pub fn combine_each<I, E>(&self, sources: I) -> Result<Option<Arc<Vec<u8>>>, E>
    where
        I: IntoIterator<Item = Result<CombineSource, E>>,
        E: From<EditError>,
    {
        let mut combiner = pdfcraft_organize::Combiner::new();
        let mut added = false;
        for source in sources {
            let (title, bytes, range) = source?;
            let doc = open_source(&title, &bytes)?;
            let range = range.as_deref().map(str::trim).filter(|r| !r.is_empty());
            let pages = match range {
                Some(r) => {
                    let n = pdfcraft_organize::page_count(&doc).map_err(EditError::from)?;
                    let p = pdfcraft_print::select_pages(n, Some(r), &[], pdfcraft_print::Subset::All, false)
                        .map_err(|e| EditError::Print(format!("{title}: {e}")))?;
                    Some(p)
                }
                None => None,
            };
            combiner.add(&title, &doc, pages.as_deref()).map_err(EditError::from)?;
            added = true;
        }
        if !added {
            return Ok(None);
        }
        let out = combiner.finish().map_err(EditError::from)?;
        Ok(Some(self.write_new(&out)?))
    }

    /// [`Self::combine_ranges`] with the password each encrypted source is opened with, by
    /// position (missing or `None`: no password). The result is not encrypted.
    pub fn combine_unlocked(&self, sources: &[CombineSource], passwords: &[Option<&str>]) -> Result<Arc<Vec<u8>>, EditError> {
        Ok(self.combine_unlocked_reporting(sources, passwords, pdfcraft_organize::FieldConflicts::Rename)?.bytes)
    }

    /// The pre-flight over byte sources: what combining them, in order, would rename in each one
    /// (whole files; page ranges are the combine's to apply). A source that cannot be opened
    /// previews as nothing — its row already says why it cannot be combined.
    pub fn combine_field_conflicts(&self, sources: &[CombineSource], passwords: &[Option<&str>]) -> Vec<Vec<(String, String)>> {
        let docs: Vec<Option<pdfcraft_cos::Document>> =
            sources.iter().enumerate().map(|(i, (n, b, _))| open_source_with(n, b, passwords.get(i).copied().flatten()).ok()).collect();
        let refs: Vec<&pdfcraft_cos::Document> = docs.iter().filter_map(|d| d.as_ref()).collect();
        let mut opened = pdfcraft_organize::combine_field_conflicts(&refs).into_iter();
        // One preview per source, in step with it: a file that cannot be opened has none.
        docs.iter().map(|d| if d.is_some() { opened.next().unwrap_or_default() } else { Default::default() }).collect()
    }

    /// [`Self::combine_unlocked`] reporting what the conflict resolution renamed.
    pub fn combine_unlocked_reporting(
        &self,
        sources: &[CombineSource],
        passwords: &[Option<&str>],
        field_conflicts: pdfcraft_organize::FieldConflicts,
    ) -> Result<Combined, EditError> {
        let docs = sources
            .iter()
            .enumerate()
            .map(|(i, (n, b, _))| open_source_with(n, b, passwords.get(i).copied().flatten()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pages = Vec::with_capacity(docs.len());
        for ((name, _, range), d) in sources.iter().zip(&docs) {
            let range = range.as_deref().map(str::trim).filter(|r| !r.is_empty());
            pages.push(match range {
                Some(r) => {
                    let n = pdfcraft_organize::page_count(d)?;
                    let p = pdfcraft_print::select_pages(n, Some(r), &[], pdfcraft_print::Subset::All, false)
                        .map_err(|e| EditError::Print(format!("{name}: {e}")))?;
                    Some(p)
                }
                None => None,
            });
        }
        let named: Vec<(&str, &pdfcraft_cos::Document, Option<&[usize]>)> =
            sources.iter().zip(docs.iter()).zip(&pages).map(|(((n, _, _), d), p)| (n.as_str(), d, p.as_deref())).collect();
        let out = pdfcraft_organize::combine_selected_with(&named, field_conflicts)?;
        let bytes = self.write_new(&out.doc)?;
        Ok(Combined { bytes, renamed_fields: out.renamed_fields })
    }

    /// [`Self::combine_unlocked`] where sources with the same `groups` key (by position) are one
    /// file split into several runs placed apart: each file is opened and copied once, so it keeps
    /// one bookmark, its links between its own pages, its fields and its attachments whole (see
    /// `pdfcraft_organize::combine_grouped`). A group's later sources use its first source's bytes
    /// and password; a source without a key is a file of its own.
    pub fn combine_grouped(&self, sources: &[CombineSource], groups: &[u64], passwords: &[Option<&str>]) -> Result<Arc<Vec<u8>>, EditError> {
        Ok(self.combine_grouped_reporting(sources, groups, passwords, pdfcraft_organize::FieldConflicts::Rename)?.bytes)
    }

    /// [`Self::combine_grouped`] reporting what the conflict resolution renamed.
    pub fn combine_grouped_reporting(
        &self,
        sources: &[CombineSource],
        groups: &[u64],
        passwords: &[Option<&str>],
        field_conflicts: pdfcraft_organize::FieldConflicts,
    ) -> Result<Combined, EditError> {
        let (mut keys, mut docs, mut file): (Vec<Option<u64>>, Vec<pdfcraft_cos::Document>, Vec<usize>) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (name, bytes, _)) in sources.iter().enumerate() {
            let key = groups.get(i).copied();
            let known = key.and_then(|k| keys.iter().position(|g| *g == Some(k)));
            let at = match known {
                Some(at) => at,
                None => {
                    docs.push(open_source_with(name, bytes, passwords.get(i).copied().flatten())?);
                    keys.push(key);
                    docs.len() - 1
                }
            };
            file.push(at);
        }
        let mut pages = Vec::with_capacity(sources.len());
        for ((name, _, range), at) in sources.iter().zip(&file) {
            let range = range.as_deref().map(str::trim).filter(|r| !r.is_empty());
            let Some(d) = docs.get(*at) else { return Err(EditError::Write(format!("{name}: not opened"))) };
            pages.push(match range {
                Some(r) => {
                    let n = pdfcraft_organize::page_count(d)?;
                    let p = pdfcraft_print::select_pages(n, Some(r), &[], pdfcraft_print::Subset::All, false)
                        .map_err(|e| EditError::Print(format!("{name}: {e}")))?;
                    Some(p)
                }
                None => None,
            });
        }
        let mut runs: Vec<pdfcraft_organize::Run<'_>> = Vec::with_capacity(sources.len());
        for (((name, _, _), at), p) in sources.iter().zip(&file).zip(&pages) {
            let Some(d) = docs.get(*at) else { return Err(EditError::Write(format!("{name}: not opened"))) };
            runs.push((*at, name.as_str(), d, p.as_deref()));
        }
        let out = pdfcraft_organize::combine_grouped_with(&runs, field_conflicts)?;
        let bytes = self.write_new(&out.doc)?;
        Ok(Combined { bytes, renamed_fields: out.renamed_fields })
    }

    /// New PDF bytes containing copies of `pages` of the document (Extract Pages).
    pub fn extract(&self, id: DocId, pages: &[usize]) -> Result<Arc<Vec<u8>>, EditError> {
        let src = self.cos(id)?;
        if src.permissions().is_some_and(|p| !p.assemble()) {
            return Err(EditError::NotPermitted("extracting pages"));
        }
        let out = pdfcraft_organize::extract_pages(src, pages)?;
        self.write_new(&out)
    }

    /// Split the document into several new PDFs.
    pub fn split(&self, id: DocId, by: &pdfcraft_organize::SplitBy) -> Result<Vec<SplitPart>, EditError> {
        let src = self.cos(id)?;
        if src.permissions().is_some_and(|p| !p.assemble()) {
            return Err(EditError::NotPermitted("splitting the document"));
        }
        let n = pdfcraft_organize::page_count(src)?;
        pdfcraft_organize::split_ranges(n, by)
            .into_iter()
            .map(|r| {
                let doc = pdfcraft_organize::extract_pages(src, &r.clone().collect::<Vec<_>>())?;
                Ok((r.start + 1, r.end, self.write_new(&doc)?))
            })
            .collect()
    }

    /// Split into parts of at most `max_bytes` each (Acrobat's "File size"): pages are taken in
    /// order while their size (as single-page files, an over-estimate because shared fonts and
    /// images count once per part) fits; a page larger than the limit is a part of its own.
    pub fn split_by_size(&self, id: DocId, max_bytes: usize) -> Result<Vec<SplitPart>, EditError> {
        let src = self.cos(id)?;
        if src.permissions().is_some_and(|p| !p.assemble()) {
            return Err(EditError::NotPermitted("splitting the document"));
        }
        let n = pdfcraft_organize::page_count(src)?;
        let mut cuts = Vec::new();
        let mut used = 0usize;
        for p in 0..n {
            let size = self.write_unchecked(&pdfcraft_organize::extract_pages(src, &[p])?)?.len();
            if used > 0 && used + size > max_bytes {
                cuts.push(p);
                used = 0;
            }
            used += size;
        }
        self.split(id, &pdfcraft_organize::SplitBy::Before(cuts))
    }

    /// Web addresses in the text of every page (Create links from URLs): (page, one rectangle
    /// per line in user space, URI). Text that already has a link over it is skipped.
    pub fn find_urls(&self, id: DocId) -> Vec<(usize, Vec<[f64; 4]>, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut out = Vec::new();
        for page in 0..doc.info.pages.len() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = res.text else { continue };
            let found: std::cell::RefCell<Vec<(std::ops::Range<usize>, String)>> = Default::default();
            let hits = text.find_with(|chars| {
                let urls = pdfcraft_annot::links::find_urls(chars);
                let ranges = urls.iter().map(|u| u.0.clone()).collect();
                *found.borrow_mut() = urls;
                ranges
            });
            for (glyphs, (_, uri)) in hits.into_iter().zip(found.into_inner()) {
                let rects: Vec<[f64; 4]> = text
                    .line_rects(glyphs)
                    .into_iter()
                    .map(|v| {
                        let q = doc.info.pages[page].view_rect_to_quad(v);
                        let xs = [q[0], q[2], q[4], q[6]];
                        let ys = [q[1], q[3], q[5], q[7]];
                        [
                            xs.iter().copied().fold(f64::MAX, f64::min),
                            ys.iter().copied().fold(f64::MAX, f64::min),
                            xs.iter().copied().fold(f64::MIN, f64::max),
                            ys.iter().copied().fold(f64::MIN, f64::max),
                        ]
                    })
                    .filter(|r| {
                        !doc.links().iter().any(|l| l.page == page && l.rect[0] < r[2] && r[0] < l.rect[2] && l.rect[1] < r[3] && r[1] < l.rect[3])
                    })
                    .collect();
                if !rects.is_empty() {
                    out.push((page, rects, uri));
                }
            }
        }
        out
    }

    /// A page of the bookmark list in outline order: up to `limit` bookmarks from position `offset`.
    /// The document's own structure is read for it, so every bookmark can be reached by paging.
    pub fn bookmark_page(&self, id: DocId, offset: usize, limit: usize) -> Option<BookmarkPage> {
        let doc = self.get(id)?;
        if let Some(editor) = doc.editor.as_ref() {
            let page = pdfcraft_organize::bookmark_page(&editor.cos, offset, limit);
            let rows = page
                .bookmarks
                .into_iter()
                .map(|b| {
                    let view = b.view.map_or_else(Default::default, |(kind, numbers)| {
                        let numbers: Vec<Option<f32>> = numbers.iter().map(|n| n.map(|n| n as f32)).collect();
                        pdfcraft_render::DestView::from_operands(Some(&kind), &numbers)
                    });
                    BookmarkRow { path: b.path, title: b.title, open: b.open, page: b.page, view }
                })
                .collect();
            return Some(BookmarkPage { rows, next: page.next, truncated: false });
        }
        let all = outline_rows(&doc.info.outline);
        let end = offset.saturating_add(limit);
        let rows = all.iter().skip(offset).take(limit).cloned().collect();
        Some(BookmarkPage { rows, next: (end < all.len()).then_some(end), truncated: doc.info.outline_more })
    }

    /// Top-level bookmarks as split points: (first page of each part, its bookmark's title).
    pub fn bookmark_splits(&self, id: DocId) -> Vec<(usize, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let top: Vec<(Option<usize>, String)> = match doc.editor.as_ref() {
            Some(editor) => pdfcraft_organize::top_level_bookmarks(&editor.cos).into_iter().map(|b| (b.page, b.title)).collect(),
            None => doc.info.outline.iter().map(|o| (o.page, o.title.clone())).collect(),
        };
        let mut out: Vec<(usize, String)> = top.into_iter().filter_map(|(page, title)| Some((page?, title))).collect();
        out.sort_by_key(|x| x.0);
        out.dedup_by_key(|x| x.0);
        out
    }

    /// Open freshly created bytes (combine / extract) as a new, unsaved document.
    pub fn open_new(&mut self, name: impl Into<String>, bytes: Arc<Vec<u8>>) -> Result<DocId, OpenError> {
        let id = self.open(name, None, bytes, None)?;
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.dirty = true;
            if let Some(e) = d.editor.as_mut() {
                e.unsaved_file = true;
            }
            // Unsaved work with no edit yet still needs its first recovery snapshot.
            d.generation += 1;
        }
        Ok(id)
    }

    fn cos(&self, id: DocId) -> Result<&pdfcraft_cos::Document, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        doc.editor.as_ref().map(|e| &e.cos).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))
    }

    /// A new file (Combine, Extract, Split, Create) as bytes, checked like a save before anyone
    /// writes it (#1698, #2166): it reopens, and shows the pages it was built from.
    fn write_new(&self, doc: &pdfcraft_cos::Document) -> Result<Arc<Vec<u8>>, EditError> {
        let bytes = self.write_unchecked(doc)?;
        let expected = pdfcraft_organize::page_count(doc)?;
        pdfcraft_cos::Document::open(bytes.clone()).map_err(|e| EditError::Reopen(e.to_string()))?;
        pdfcraft_render::check_pages(bytes.clone(), None, expected).map_err(|e| EditError::Reopen(e.to_string()))?;
        Ok(bytes)
    }

    /// [`Self::write_new`] without the check, for bytes nobody writes: measuring a part's size.
    fn write_unchecked(&self, doc: &pdfcraft_cos::Document) -> Result<Arc<Vec<u8>>, EditError> {
        let opts = SaveOptions { mod_date: self.now().map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        write_full(doc, &opts).map(Arc::new).map_err(|e| EditError::Write(e.to_string()))
    }

    /// Whether a changed document still needs a recovery snapshot. This does not consume it.
    pub fn has_pending_autosave(&self) -> bool {
        self.docs.iter().any(|d| d.dirty && d.generation != d.snapshot_generation)
    }

    /// Documents with unsaved changes made since the last call: their current working file,
    /// for crash recovery. Encrypted documents stay encrypted in the snapshot.
    pub fn autosave_snapshots(&mut self) -> Vec<RecoverySnapshot> {
        let mut out = Vec::new();
        for d in &mut self.docs {
            if d.dirty && d.generation != d.snapshot_generation {
                d.snapshot_generation = d.generation;
                out.push(RecoverySnapshot {
                    doc: d.id,
                    name: d.name.clone(),
                    path: d.path.clone(),
                    bytes: d.bytes.clone(),
                    encrypted: d.info.encrypted || d.editor.as_ref().is_some_and(|e| e.cos.security().is_some()),
                });
            }
        }
        out
    }

    /// Mark a document opened from a recovery file: it has unsaved changes and belongs at
    /// `path` (where Save writes), as when the session ended.
    pub fn mark_recovered(&mut self, id: DocId, path: Option<String>) {
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.path = path;
            d.dirty = true;
            if let Some(e) = d.editor.as_mut() {
                e.unsaved_file = true;
            }
            d.generation += 1;
            d.snapshot_generation = d.generation; // its bytes are already in the recovery store
        }
    }

    /// Show or hide a layer (optional content group) for viewing. Returns `true` if it changed;
    /// callers must drop cached rasters and text for the document.
    pub fn set_layer_visible(&mut self, id: DocId, layer: usize, visible: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        let Some(l) = doc.info.layers.get_mut(layer) else { return false };
        if l.visible == visible {
            return false;
        }
        l.visible = visible;
        use_layer_choices(doc);
        true
    }

    /// Run a set-layer-visibility action (`SetOCGState`): apply `changes` in order, naming each
    /// layer by its optional content group; groups that aren't layers are skipped. With
    /// `preserve_rb`, a layer turned on turns off the other layers of its radio-button groups.
    /// Returns `true` if any layer changed; callers must drop cached rasters and text for the
    /// document, as for [`Self::set_layer_visible`].
    pub fn set_layer_state(&mut self, id: DocId, changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if !apply_layer_state(&mut doc.info.layers, &doc.info.layer_groups, changes, preserve_rb) {
            return false;
        }
        use_layer_choices(doc);
        true
    }

    /// Show each layer as the file opens (Acrobat's Layers ▸ Reset to Initial Visibility). Returns
    /// `true` if any layer changed; callers must drop cached rasters and text for the document,
    /// as for [`Self::set_layer_visible`].
    pub fn reset_layers(&mut self, id: DocId) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        let mut changed = false;
        for l in &mut doc.info.layers {
            changed |= l.visible != l.initial;
            l.visible = l.initial;
        }
        if changed {
            use_layer_choices(doc);
        }
        changed
    }

    /// Hide or show every comment on the page (Acrobat's Comments ▸ Hide all comments). Form
    /// fields and links still draw. Returns whether anything changed.
    pub fn set_hide_comments(&mut self, id: DocId, hide: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if doc.config.hide_comments == hide {
            return false;
        }
        doc.config.hide_comments = hide;
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.generation += 1;
        true
    }

    /// Set the document's reading colours (Acrobat's Replace Document Colors). Returns whether
    /// anything changed; callers must drop cached rasters and text for the document.
    pub fn set_reading_colors(&mut self, id: DocId, colors: ReadingColors) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if doc.config.reading_colors == colors {
            return false;
        }
        doc.config.reading_colors = colors;
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.generation += 1;
        true
    }

    /// The document's reading-colours setting; `None` when no such document is open.
    pub fn reading_colors(&self, id: DocId) -> Option<ReadingColors> {
        Some(self.get(id)?.config.reading_colors)
    }

    /// How the document's pages are drawn for viewing: its password and view settings (layer
    /// visibility, hidden comments, reading colours). `None` when no such document is open.
    pub fn render_config(&self, id: DocId) -> Option<RenderConfig> {
        Some(self.get(id)?.config.clone())
    }

    /// Acrobat's Summarize Comments ("Comments only" layout): a new PDF listing every comment
    /// with its number, author, type, date, text and replies, grouped by page.
    pub fn summarize_comments(&self, id: DocId, sort: SummarySort) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let text = comment_summary(&doc.name, &doc.info.annotations, sort);
        self.create_report(&format!("Summary of Comments on {}", doc.name), &text)
    }

    /// The certificates trusted for signing.
    pub fn trusted_certificates(&self) -> &[pdfcraft_sign::Certificate] {
        &self.trust.certs
    }

    /// Replace the trusted certificates, revalidate every open document's signatures,
    /// and rebuild its display copy and renderer to match (working file untouched,
    /// generation/dirty unchanged: trust alone is not an edit).
    pub fn set_trusted_certificates(&mut self, certs: Vec<pdfcraft_sign::Certificate>) {
        self.trust = Arc::new(TrustStore { certs });
        for doc in &mut self.docs {
            doc.trust = self.trust.clone();
            let Some(editor) = doc.editor.as_mut() else { continue };
            // Keep verdict refresh and renderer refresh together. Viewer caches are
            // invalidated by the UI trust path; trust alone changes no document artwork.
            let signatures = if doc.signatures.is_empty() { Vec::new() } else { signatures_of(&editor.cos, &doc.bytes, &doc.trust, &doc.sig_cache) };
            doc.display = display_bytes(Some(editor), &doc.bytes);
            doc.signatures = Arc::new(signatures);
            doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        }
    }

    /// The signing time as a PDF date in local time with its offset (`D:…+02'00'`).
    pub fn signing_date(&self) -> String {
        let offset = self.local_offset();
        let local = pdfcraft_cos::pdf_date(self.now().unwrap_or(0) + offset);
        let stamp = local.trim_end_matches('Z');
        if offset == 0 {
            return format!("{stamp}Z");
        }
        let (sign, m) = if offset < 0 { ('-', -offset / 60) } else { ('+', offset / 60) };
        format!("{stamp}{sign}{:02}'{:02}'", m / 60, m % 60)
    }

    /// Sign the document's current state with `id` (Use a certificate ▸ Digitally sign). Returns
    /// the signed file; the caller saves it and then calls [`Session::mark_signed`]. An empty
    /// `opts.date` takes the session clock.
    pub fn sign(&self, doc: DocId, id: &pdfcraft_sign::DigitalId, mut opts: SignOptions) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        // Encrypted documents are signed under their permissions (the signer checks them).
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        if opts.date.is_empty() {
            opts.date = self.signing_date();
        }
        Ok(Arc::new(pdfcraft_sign::sign(&editor.cos, id, &opts)?))
    }

    /// [`Session::sign`], embedding an RFC 3161 signature timestamp (PAdES B-T) produced by
    /// `tsa`. The transport lives with the caller; the engine never opens a socket.
    pub fn sign_with_timestamp(
        &self,
        doc: DocId,
        id: &pdfcraft_sign::DigitalId,
        mut opts: SignOptions,
        tsa: &dyn pdfcraft_sign::TimestampAuthority,
    ) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        if opts.date.is_empty() {
            opts.date = self.signing_date();
        }
        let signed = Arc::new(pdfcraft_sign::sign_with_timestamp(&editor.cos, id, &opts, tsa)?);
        // Checked like a save before anyone writes it (#2166).
        self.check_save(doc, &signed)?;
        Ok(signed)
    }

    /// Append a standalone document timestamp (RFC 3161, `/ETSI.RFC3161`) covering the file's
    /// current state. An empty `date` takes the session clock; the transport is the caller's.
    pub fn timestamp_document(&self, doc: DocId, tsa: &dyn pdfcraft_sign::TimestampAuthority, date: String) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        let date = if date.is_empty() { self.signing_date() } else { date };
        let stamped = Arc::new(pdfcraft_sign::timestamp_document(&editor.cos, tsa, &date)?);
        self.check_save(doc, &stamped)?;
        Ok(stamped)
    }

    /// Embed revocation evidence into the catalog's `/DSS` with `/VRI` entries per signature
    /// (PAdES B-LT): an incremental update that never rewrites signed bytes.
    pub fn embed_ltv(&self, doc: DocId, evidence: &pdfcraft_sign::dss::Evidence) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        let embedded = Arc::new(pdfcraft_sign::dss::embed(&editor.cos, evidence)?);
        self.check_save(doc, &embedded)?;
        Ok(embedded)
    }

    /// Record that the signed file `bytes` was saved (to `path`): like [`Session::mark_saved`],
    /// and the edit history before signing is dropped (signing can't be undone).
    pub fn mark_signed(&mut self, id: DocId, bytes: Arc<Vec<u8>>, path: Option<String>) -> Result<(), EditError> {
        self.mark_saved(id, bytes, path)?;
        let doc = self.doc_mut(id)?;
        if let Some(e) = doc.editor.as_mut() {
            e.undo.clear();
            e.redo.clear();
        }
        Ok(())
    }

    pub fn close(&mut self, id: DocId) {
        self.docs.retain(|d| d.id != id);
    }

    pub fn get(&self, id: DocId) -> Option<&Document> {
        self.docs.iter().find(|d| d.id == id)
    }

    /// Read document `id`'s font list again if content edits since the last full inspection may
    /// have changed it (Document Properties ▸ Fonts, `doc_info`). A no-op otherwise.
    pub fn refresh_fonts(&mut self, id: DocId) {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && d.fonts_stale) else { return };
        if let Ok(info) = pdfcraft_render::inspect(doc.bytes.clone(), doc.password.as_deref()) {
            doc.info.fonts = info.fonts;
        }
        doc.fonts_stale = false;
    }

    pub fn docs(&self) -> &[Document] {
        &self.docs
    }
}

/// A picture for a watermark or background (Acrobat: Source ▸ File): an image, or `page`
/// (0-based) of a PDF.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkFile {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub page: usize,
}

/// Embed an image that is placed on a page, turned the way its EXIF Orientation says.
fn embed_image(
    doc: &mut pdfcraft_cos::Document,
    name: &str,
    bytes: &[u8],
) -> Result<(pdfcraft_cos::ObjRef, (f64, f64)), pdfcraft_create::CreateError> {
    pdfcraft_create::image_xobject(doc, name, bytes)
}

/// Bring a mark's picture into `doc`: a PDF page as a form XObject, or an image.
fn mark_source(doc: &mut pdfcraft_cos::Document, f: &MarkFile) -> Result<pdfcraft_edit::MarkSource, EditError> {
    let head = &f.bytes[..f.bytes.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        let src = pdfcraft_cos::Document::open(f.bytes.clone()).map_err(|e| EditError::Source(format!("{}: {e}", f.name)))?;
        let (xobject, size) = pdfcraft_organize::page_as_form(doc, &src, f.page)?;
        Ok(pdfcraft_edit::MarkSource { xobject, size, image: false })
    } else {
        let (xobject, size) = embed_image(doc, &f.name, &f.bytes)?;
        Ok(pdfcraft_edit::MarkSource { xobject, size, image: true })
    }
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

/// Order of Summarize Comments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummarySort {
    #[default]
    Page,
    Author,
    Date,
    Type,
}

impl SummarySort {
    pub const ALL: [SummarySort; 4] = [SummarySort::Page, SummarySort::Author, SummarySort::Date, SummarySort::Type];
    pub fn name(self) -> &'static str {
        match self {
            SummarySort::Page => "Page",
            SummarySort::Author => "Author",
            SummarySort::Date => "Date",
            SummarySort::Type => "Type",
        }
    }
    pub fn parse(s: &str) -> Option<SummarySort> {
        Self::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
    }
}

/// Acrobat's name for a comment type, as comment lists show it.
pub fn comment_type_name(subtype: &str) -> &str {
    match subtype {
        "Text" => "Sticky Note",
        "FreeText" => "Text Box",
        "Highlight" => "Highlight",
        "Underline" => "Underline",
        "StrikeOut" => "Strikethrough",
        "Squiggly" => "Squiggly",
        "Square" => "Rectangle",
        "Circle" => "Oval",
        "Line" => "Line",
        "Ink" => "Pencil",
        "Polygon" => "Polygon",
        "PolyLine" => "Polygonal Line",
        "Stamp" => "Stamp",
        "Caret" => "Inserted Text",
        "FileAttachment" => "File Attachment",
        "Sound" => "Sound",
        "Redact" => "Redaction",
        other => other,
    }
}

/// [`comment_type_name`], telling callouts, clouds and typewriter text apart by `/IT`.
pub fn comment_kind(a: &pdfcraft_render::Annotation) -> &str {
    match a.intent.as_deref() {
        Some("FreeTextCallout") => "Callout",
        Some("PolygonCloud") => "Cloud",
        Some("FreeTextTypeWriter") => "Typewriter",
        _ => comment_type_name(&a.subtype),
    }
}

/// The text of a comment summary: one block per comment (replies indented under it), with a
/// "Page N" heading when sorted by page. Checkmarks and status replies are left out, as are
/// pop-ups; the number is the comment's position on its page.
pub fn comment_summary(name: &str, all: &[pdfcraft_render::Annotation], sort: SummarySort) -> String {
    use std::fmt::Write;
    let top: Vec<&pdfcraft_render::Annotation> = all.iter().filter(|a| a.in_reply_to.is_none() && a.state.is_none()).collect();
    // Write a comment's replies, indenting each level. A reply to a reply keeps its `/IRT` on the
    // direct parent, so following one level would leave the deeper replies out of the summary.
    fn write_replies(out: &mut String, all: &[pdfcraft_render::Annotation], parent: &str, depth: usize) {
        use std::fmt::Write;
        if depth > 64 {
            return;
        }
        for r in all.iter().filter(|r| r.state.is_none() && r.in_reply_to.as_deref() == Some(parent)) {
            let indent = "    ".repeat(depth + 1);
            let _ =
                writeln!(out, "{indent}Author: {}  Subject: Reply  Date: {}", r.author.as_deref().unwrap_or(""), r.modified.as_deref().unwrap_or(""));
            for line in r.contents.as_deref().unwrap_or("").lines() {
                let _ = writeln!(out, "{indent}{line}");
            }
            if let Some(nm) = r.name.as_deref() {
                write_replies(out, all, nm, depth + 1);
            }
        }
    }
    let mut numbered: Vec<(usize, &pdfcraft_render::Annotation)> = Vec::new();
    let mut last = usize::MAX;
    let mut n = 0;
    for a in &top {
        if a.page != last {
            last = a.page;
            n = 0;
        }
        n += 1;
        numbered.push((n, a));
    }
    match sort {
        SummarySort::Page => {}
        SummarySort::Author => numbered.sort_by(|x, y| x.1.author.cmp(&y.1.author)),
        SummarySort::Date => numbered.sort_by(|x, y| x.1.modified.cmp(&y.1.modified)),
        SummarySort::Type => numbered.sort_by(|x, y| comment_kind(x.1).cmp(comment_kind(y.1))),
    }
    let mut out = format!("Summary of Comments on {name}\n\n");
    if numbered.is_empty() {
        out.push_str("This document has no comments.\n");
        return out;
    }
    let mut heading = usize::MAX;
    for (n, a) in numbered {
        if sort == SummarySort::Page && a.page != heading {
            heading = a.page;
            let _ = writeln!(out, "Page: {}", a.page + 1);
        }
        let _ = write!(out, "Number: {n}  Author: {}  Subject: {}", a.author.as_deref().unwrap_or(""), comment_kind(a));
        if sort != SummarySort::Page {
            let _ = write!(out, "  Page: {}", a.page + 1);
        }
        let _ = writeln!(out, "  Date: {}", a.modified.as_deref().unwrap_or(""));
        if let Some(c) = a.contents.as_deref().filter(|c| !c.is_empty()) {
            let _ = writeln!(out, "{c}");
        }
        if let Some(nm) = a.name.as_deref() {
            write_replies(&mut out, all, nm, 0);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;

/// Page filters of Acrobat's Rotate Pages and page selection: even/odd page numbers and
/// orientation (as displayed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageParity {
    #[default]
    Both,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageOrientation {
    #[default]
    Both,
    Landscape,
    Portrait,
}

/// The pages of `pages` (0-based) that pass the filters.
pub fn filter_pages(info: &pdfcraft_render::DocInfo, pages: &[usize], parity: PageParity, orientation: PageOrientation) -> Vec<usize> {
    pages
        .iter()
        .copied()
        .filter(|p| match parity {
            PageParity::Both => true,
            // Page numbers are 1-based: index 1 is page 2, an even page.
            PageParity::Even => p % 2 == 1,
            PageParity::Odd => p % 2 == 0,
        })
        .filter(|p| {
            let Some(pg) = info.pages.get(*p) else { return false };
            match orientation {
                PageOrientation::Both => true,
                PageOrientation::Landscape => pg.width > pg.height,
                PageOrientation::Portrait => pg.width <= pg.height,
            }
        })
        .collect()
}
