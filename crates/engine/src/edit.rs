//! The edits a document takes: what each is called, whether its permissions allow it, and running it.

use std::sync::Arc;

use crate::{
    AddedContent, AddedText, Background, BoxSpec, EditError, FieldProps, FieldValue, FillMark, HeaderFooter, Hidden, ImageEdit, InitialView, Label,
    LineEnding, LinkAction, LinkStyle, MarkFile, MarkKind, Markup, NewAnnotation, NewField, NoteIcon, ObjectTarget, PageBox, Protection, ReviewState,
    Rgb, Shape, Standard, Style, TabOrder, TransitionSpec, Watermark, a11y, copy_image, decode_data_file, embed_image, field_table, fit_picture,
    image_content_type, is_xfa_data_file, js, mark_source, measure, open_source, strip_xfa, unrestricted, xfa, xfa_after_data_change,
};

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
    "Attach {name}",
    "Delete attachment {key}",
    "Add bookmark",
    "Rename bookmark",
    "Delete bookmark",
    "Move bookmark",
    "Set bookmark destination",
    "Colour",
    "Style",
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
    "Paste",
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
    &["Set document title", "Mark differences", "Detect form fields", "Run JavaScript", "Recognize text", field_table::LABEL, "Style"];

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
            // The bookmark Style and Colour menus' own names, already said in every language.
            Edit::SetBookmarkColor { .. } => Label::of("Colour"),
            Edit::SetBookmarkStyle { .. } => Label::of("Style"),
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
            Edit::AddAttachment { name, .. } => Label::with("Attach {name}", name.as_str()),
            Edit::DeleteAttachment { key } => Label::with("Delete attachment {key}", key.as_str()),
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
            Edit::PasteContent { .. } => Label::of("Paste"),
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

/// Whether the opening password allows an edit (§7.6.4.2, Table 22).
pub(crate) fn check_permission(edit: &Edit, p: &pdfcraft_cos::Permissions) -> Result<(), EditError> {
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
        | Edit::SetBookmarkColor { .. }
        | Edit::SetBookmarkStyle { .. }
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
        // Converting to PDF/A takes the encryption off (PDF/A forbids it), so on an encrypted
        // document it is a change of security (#3921).
        Edit::ConvertPdfA { .. } => {
            if unrestricted(p) {
                Ok(())
            } else {
                Err(EditError::NotPermitted("removing the encryption, which PDF/A forbids"))
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
        | Edit::PasteContent { .. }
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
        | Edit::AddAttachment { .. }
        | Edit::DeleteAttachment { .. }
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
pub(crate) struct EditCtx {
    /// Field scripts run (`None`: JavaScript is off) and what they produced.
    pub(crate) js: Option<js::JsRunner>,
    /// A laid-out XFA form's template (`None`: not an XFA form, or JavaScript is off) and what
    /// its scripts produced.
    pub(crate) xfa: Option<Arc<pdfcraft_xfa::model::Template>>,
    pub(crate) xfa_out: js::JsOutput,
    /// The datasets stream this edit wrote (later writes in the edit replace it in place).
    pub(crate) xfa_datasets: Option<pdfcraft_cos::ObjRef>,
    /// A form script ran too long and was abandoned: the document's scripts go off.
    pub(crate) xfa_ran_away: bool,
    /// What the XFA scripts' time limits are multiplied by ([`Session::with_xfa_time_scale`]).
    pub(crate) xfa_time_scale: u32,
    /// The edit took the XFA out of the form: what went with it, for the document's warnings.
    pub(crate) xfa_removed: Vec<String>,
    /// Pages in the document, for `xfa.layout.pageCount()`.
    pub(crate) pages: usize,
    pub(crate) date: Option<String>,
    /// Today in local time, for date tokens.
    pub(crate) today: (i64, u32, u32),
    pub(crate) seed: u64,
    pub(crate) count: u64,
    /// Fields the edit renamed to keep imported names separate (old → new), for the repair
    /// log and the undo label.
    pub(crate) renamed_fields: Vec<(String, String)>,
    /// The overlay streams the edit's applied redactions added, which alone the renderer's check
    /// leaves out.
    pub(crate) redaction_overlays: Vec<pdfcraft_cos::ObjRef>,
}

impl EditCtx {
    pub(crate) fn new(now: Option<i64>, salt: u64) -> Self {
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
            xfa_time_scale: 1,
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
pub(crate) fn run_edit(doc: &mut pdfcraft_cos::Document, edit: &Edit, cx: &mut EditCtx) -> Result<(), EditError> {
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
        Edit::SetBookmarkColor { path, color } => pdfcraft_organize::set_bookmark_color(doc, path, *color)?,
        Edit::SetBookmarkStyle { path, bold, italic } => pdfcraft_organize::set_bookmark_style(doc, path, *bold, *italic)?,
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
            // The picture as displayed: a quarter turn swaps its sides in user space.
            let (dw, dh) = if matches!(rotation, 90 | 270) { (sh, sw) } else { (sw, sh) };
            let rect = if natural {
                let k = (200.0 / sw.max(sh)).min(1.0);
                let (w, h) = (dw * k, dh * k);
                [rect[0] - w / 2.0, rect[1] - h / 2.0, rect[0] + w / 2.0, rect[1] + h / 2.0]
            } else {
                fit_picture(*rect, dw, dh)
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
        Edit::AddAttachment { name, bytes, description, mime } => {
            pdfcraft_organize::add_attachment(doc, name, bytes, description.as_deref(), mime.as_deref(), cx.date.as_deref())?;
        }
        Edit::DeleteAttachment { key } => pdfcraft_organize::delete_attachment(doc, key)?,
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
        Edit::SetFieldValue { name, value } => {
            let mut set = |doc: &mut pdfcraft_cos::Document, value: &FieldValue| match cx.js.as_mut() {
                Some(js) => pdfcraft_forms::set_value_with(doc, name, value, js),
                None => pdfcraft_forms::set_value(doc, name, value),
            };
            match set(doc, value) {
                Ok(()) => {}
                // A date the field's format doesn't read may be typed by another of an XFA date
                // field's patterns: it is kept in the first. Only a format refusal is tried
                // again: it comes before any of the field's scripts run, so they run once.
                Err(refused @ pdfcraft_forms::FormError::Format(_)) => match xfa::typed_date(doc, cx.xfa.as_deref(), name, value) {
                    Some(typed) => set(doc, &typed)?,
                    None => return Err(refused.into()),
                },
                Err(refused) => return Err(refused.into()),
            }
        }
        Edit::ApplyScriptChanges { changes } => match cx.js.as_mut() {
            Some(js) => pdfcraft_forms::apply_script_changes(doc, changes, js)?,
            None => pdfcraft_forms::apply_script_changes(doc, changes, &mut pdfcraft_forms::NoScripts)?,
        },
        Edit::XfaEvent { som, activity } => {
            let tpl = cx.xfa.clone().ok_or_else(|| EditError::Invalid("this is not a laid-out XFA form, or JavaScript is off".into()))?;
            let mut run =
                xfa::XfaRun { page_count: cx.pages, out: &mut cx.xfa_out, datasets: cx.xfa_datasets, ran_away: false, time_scale: cx.xfa_time_scale };
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
            pdfcraft_edit::stamp(doc, *page, "OCR", pdfcraft_ocr::text_layer(words))?;
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
        Edit::PasteContent { page, content, file } => {
            let content = match content {
                AddedContent::Text(_) => content.clone(),
                AddedContent::Image(img) => {
                    let Some((name, bytes)) = file else {
                        return Err(EditError::Edit(pdfcraft_edit::EditError::Invalid("a pasted image needs its picture".into())));
                    };
                    // The same picture in this document (copied here, pasted here before, or an
                    // identical file): drawn again from the one image object rather than stored twice.
                    let image = if copy_image::is_picture(doc, img.image, bytes) {
                        img.image
                    } else {
                        match copy_image::added_picture(doc, bytes) {
                            Some(image) => image,
                            None => embed_image(doc, name, bytes)?.0,
                        }
                    };
                    AddedContent::Image(pdfcraft_edit::AddedImage { image, ..img.clone() })
                }
            };
            pdfcraft_edit::add_content(doc, *page, &content)?;
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

/// The singular or the plural wording, by count. Both are written out: a language forms its plural
/// its own way, and `one` and `many` may differ in more than an "s".
fn plural(one: &'static str, many: &'static str, n: usize) -> Label {
    Label::of(if n == 1 { one } else { many })
}
