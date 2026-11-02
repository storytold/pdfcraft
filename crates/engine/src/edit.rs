//! The edits a document takes: what each is called, whether its permissions allow it, and running it.

use std::sync::Arc;

use crate::{
    AddedContent, AddedText, Background, BoxSpec, EditError, FieldProps, FieldValue, FillMark, HeaderFooter, Hidden, ImageEdit, InitialView, Label,
    LineEnding, LinkAction, LinkStyle, MarkFile, MarkKind, Markup, NewAnnotation, NewField, NoteIcon, ObjectTarget, PageBox, Protection, ReviewState,
    Rgb, Shape, Standard, Style, TabOrder, TransitionSpec, Watermark, a11y, copy_image, decode_data_file, embed_image, field_table, fit_picture,
    image_content_type, is_xfa_data_file, js, mark_source, measure, open_source, strip_xfa, unrestricted, xfa, xfa_after_data_change,
};

mod labels;
mod run;

pub use labels::{BATCH_LABELS, EDIT_LABELS};
pub(crate) use run::{EditCtx, check_permission, run_edit};

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
    /// Colour a bookmark's title (`/C`); `None` or black is the default black.
    SetBookmarkColor {
        path: Vec<usize>,
        color: Option<[f64; 3]>,
    },
    /// Make a bookmark's title bold or italic or not (`/F`); `None` keeps that one.
    SetBookmarkStyle {
        path: Vec<usize>,
        bold: Option<bool>,
        italic: Option<bool>,
    },
    /// Bookmarks for the tagged headings (H, H1–H6), nested by level under a new first
    /// "Untitled" bookmark (New Bookmarks from Structure).
    BookmarksFromStructure,
    /// Bookmarks for the lines set in `levels`' styles (the first is level 1, the next nests
    /// under it, …), under a new first "Untitled" bookmark (#1693; see [`crate::text_style_bookmarks`]).
    BookmarksFromTextStyles {
        levels: Vec<crate::text_style_bookmarks::TextStyle>,
    },
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
    /// Any other `rect` is a box: the picture is scaled to fit inside it, keeping its shape,
    /// and centred in it (#4077).
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
    /// Delete one widget of a field (`widget`, 0-based in the field's widgets): one radio button
    /// of a group, or one copy of a duplicated field. The last widget takes the field with it.
    DeleteWidget {
        name: String,
        widget: usize,
    },
    /// Page properties ▸ Tab order, for pages (0-based).
    SetTabOrder {
        pages: Vec<usize>,
        order: TabOrder,
    },
    /// Document Properties ▸ Initial View (and the language and binding).
    SetInitialView(Box<InitialView>),
    /// Edit ▸ Attach a file: a document-level attachment (`/Names /EmbeddedFiles`) under
    /// `name`, made unique; `mime` is the embedded file's `/Subtype` when known.
    AddAttachment {
        name: String,
        bytes: Vec<u8>,
        description: Option<String>,
        mime: Option<String>,
    },
    /// Remove the document-level attachment whose name-tree key is `key`.
    DeleteAttachment {
        key: String,
    },
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
    /// image (from [`ocr::OcrJob::run`](crate::ocr::OcrJob::run)).
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
    /// in their current order (see [`Document::calculation_order`](crate::Document::calculation_order)). The form recalculates.
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
    /// Edit ▸ Paste: an added item copied in this document or another, as it was: text with its
    /// style, an image in its box (display space) with its turns, flips and crop. An image's
    /// picture is `file` (from [`Document::added_image_file`](crate::Document::added_image_file)): the copied image object is reused
    /// while it is still that picture in this document, else one an added image here already draws
    /// it from, else the file is embedded.
    PasteContent {
        page: usize,
        content: AddedContent,
        file: Option<(String, Arc<Vec<u8>>)>,
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
    /// Import comments and/or form data (XFDF, FDF, XML, CSV, tab-delimited text), or the
    /// comments of another PDF onto the pages with the same numbers.
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
