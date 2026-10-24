//! pdfcraft-annot — comments (annotations), ISO 32000-2 §12.5, execution plan M5.1–M5.3.
//!
//! Builders for the comment types Acrobat's commenting tools create (sticky note, highlight,
//! underline, strikethrough, squiggly, rectangle, oval, line/arrow, freehand ink, text box),
//! appearance streams for them ([`appearance`]), and the edits a comment goes through: reply,
//! change its text, recolour, move, resize and delete.
//!
//! Addressing: a comment is `(page, index)`, its position in the page's `/Annots` array, which
//! is what `pdfcraft_render::Annotation::index` reports. Inline annotation dictionaries are
//! promoted to indirect objects when they are edited (replies need a reference to point at).
//!
//! Every edit mutates a `pdfcraft_cos::Document` (copy-on-write); callers snapshot it first
//! for undo. Keys we do not understand are left alone.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

pub mod appearance;
pub mod links;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AnnotError {
    #[error("the document has no page tree")]
    NoPageTree,
    /// Holds the 0-based index; the message names the 1-based page a user counts (#1550).
    #[error("page {} does not exist", .0.saturating_add(1))]
    NoSuchPage(usize),
    #[error("there is no comment {index} on page {page}")]
    NoSuchAnnotation { page: usize, index: usize },
    #[error("{0}")]
    Invalid(String),
    #[error("{0} comments can't be restyled yet (their appearance can't be regenerated)")]
    Unsupported(String),
    /// Text the standard fonts can't draw where `?` would be permanent or replace a good drawing:
    /// a redaction's overlay text, or an edit that would redraw a comment another program drew
    /// with a font that has those characters. The message names the character.
    #[error("{0}")]
    Undrawable(String),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

pub type Rgb = [f64; 3];

/// Text markup kinds (§12.5.6.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Markup {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl Markup {
    fn subtype(self) -> &'static str {
        match self {
            Markup::Highlight => "Highlight",
            Markup::Underline => "Underline",
            Markup::StrikeOut => "StrikeOut",
            Markup::Squiggly => "Squiggly",
        }
    }
}

/// Text-annotation icon names (`/Name`). The first seven are the standard names ISO 32000-2
/// §12.5.6.4, Table 175 asks every viewer to draw; the rest are further names in common use, which
/// the same table allows ("Additional names may be supported as well").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoteIcon {
    #[default]
    Comment,
    Note,
    Help,
    Insert,
    Key,
    NewParagraph,
    Paragraph,
    Check,
    Circle,
    Cross,
    CrossHairs,
    Star,
    RightArrow,
    UpArrow,
}

impl NoteIcon {
    pub fn name(self) -> &'static str {
        match self {
            NoteIcon::Comment => "Comment",
            NoteIcon::Note => "Note",
            NoteIcon::Help => "Help",
            NoteIcon::Insert => "Insert",
            NoteIcon::Key => "Key",
            NoteIcon::NewParagraph => "NewParagraph",
            NoteIcon::Paragraph => "Paragraph",
            NoteIcon::Check => "Check",
            NoteIcon::Circle => "Circle",
            NoteIcon::Cross => "Cross",
            NoteIcon::CrossHairs => "CrossHairs",
            NoteIcon::Star => "Star",
            NoteIcon::RightArrow => "RightArrow",
            NoteIcon::UpArrow => "UpArrow",
        }
    }

    /// Every icon, the standard names first.
    pub const ALL: [NoteIcon; 14] = [
        Self::Comment,
        Self::Note,
        Self::Help,
        Self::Insert,
        Self::Key,
        Self::NewParagraph,
        Self::Paragraph,
        Self::Check,
        Self::Circle,
        Self::Cross,
        Self::CrossHairs,
        Self::Star,
        Self::RightArrow,
        Self::UpArrow,
    ];

    pub fn from_name(n: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.name() == n)
    }
}

/// Fill & Sign marks (Acrobat's ✓, ✕, ●, ─).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMark {
    Check,
    Cross,
    Dot,
    Line,
}

impl FillMark {
    /// The `/Name` of the stamp PdfCraft draws for it.
    pub fn name(self) -> &'static str {
        match self {
            FillMark::Check => "PCCheck",
            FillMark::Cross => "PCCross",
            FillMark::Dot => "PCDot",
            FillMark::Line => "PCLine",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FillMark::Check => "Checkmark",
            FillMark::Cross => "Cross",
            FillMark::Dot => "Dot",
            FillMark::Line => "Line",
        }
    }

    /// The user-space box of the mark placed at `at` on a page turned by `rotation`: centred
    /// there and sized in displayed axes, so a line runs across the page as displayed (#2392).
    pub fn rect_at(self, at: [f64; 2], rotation: i64) -> [f64; 4] {
        let (w, h) = if self == FillMark::Line { (36.0, 4.0) } else { (12.0, 12.0) };
        let (w, h) = if matches!(rotation.rem_euclid(360), 90 | 270) { (h, w) } else { (w, h) };
        [at[0] - w / 2.0, at[1] - h / 2.0, at[0] + w / 2.0, at[1] + h / 2.0]
    }
}

/// The stamps of Acrobat's stamp palette (drawn in PdfCraft's own style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StampKind {
    // Standard business.
    Approved,
    Completed,
    Confidential,
    Draft,
    Final,
    ForComment,
    ForPublicRelease,
    InformationOnly,
    NotApproved,
    NotForPublicRelease,
    PreliminaryResults,
    Void,
    // Sign here.
    Accepted,
    InitialHere,
    Rejected,
    SignHere,
    Witness,
    // Dynamic (with a "By … at …" line).
    DynApproved,
    DynConfidential,
    DynReceived,
    DynReviewed,
    DynRevised,
}

/// The palette's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampGroup {
    Dynamic,
    SignHere,
    StandardBusiness,
}

impl StampKind {
    pub const ALL: [StampKind; 22] = [
        StampKind::DynApproved,
        StampKind::DynConfidential,
        StampKind::DynReceived,
        StampKind::DynReviewed,
        StampKind::DynRevised,
        StampKind::Accepted,
        StampKind::InitialHere,
        StampKind::Rejected,
        StampKind::SignHere,
        StampKind::Witness,
        StampKind::Approved,
        StampKind::Completed,
        StampKind::Confidential,
        StampKind::Draft,
        StampKind::Final,
        StampKind::ForComment,
        StampKind::ForPublicRelease,
        StampKind::InformationOnly,
        StampKind::NotApproved,
        StampKind::NotForPublicRelease,
        StampKind::PreliminaryResults,
        StampKind::Void,
    ];

    pub fn group(self) -> StampGroup {
        use StampKind::*;
        match self {
            DynApproved | DynConfidential | DynReceived | DynReviewed | DynRevised => StampGroup::Dynamic,
            Accepted | InitialHere | Rejected | SignHere | Witness => StampGroup::SignHere,
            _ => StampGroup::StandardBusiness,
        }
    }

    /// The text on the stamp.
    pub fn label(self) -> &'static str {
        use StampKind::*;
        match self {
            Approved | DynApproved => "APPROVED",
            Completed => "COMPLETED",
            Confidential | DynConfidential => "CONFIDENTIAL",
            Draft => "DRAFT",
            Final => "FINAL",
            ForComment => "FOR COMMENT",
            ForPublicRelease => "FOR PUBLIC RELEASE",
            InformationOnly => "INFORMATION ONLY",
            NotApproved => "NOT APPROVED",
            NotForPublicRelease => "NOT FOR PUBLIC RELEASE",
            PreliminaryResults => "PRELIMINARY RESULTS",
            Void => "VOID",
            Accepted => "ACCEPTED",
            InitialHere => "INITIAL HERE",
            Rejected => "REJECTED",
            SignHere => "SIGN HERE",
            Witness => "WITNESS",
            DynReceived => "RECEIVED",
            DynReviewed => "REVIEWED",
            DynRevised => "REVISED",
        }
    }

    /// `/Name`: the standard stamp names of ISO 32000-2 Table 184 where one exists.
    pub fn name(self) -> &'static str {
        use StampKind::*;
        match self {
            Approved => "Approved",
            Confidential => "Confidential",
            Draft => "Draft",
            Final => "Final",
            ForComment => "ForComment",
            ForPublicRelease => "ForPublicRelease",
            NotApproved => "NotApproved",
            NotForPublicRelease => "NotForPublicRelease",
            Completed => "PCCompleted",
            InformationOnly => "PCInformationOnly",
            PreliminaryResults => "PCPreliminaryResults",
            Void => "PCVoid",
            Accepted => "PCAccepted",
            InitialHere => "PCInitialHere",
            Rejected => "PCRejected",
            SignHere => "PCSignHere",
            Witness => "PCWitness",
            DynApproved => "PCDynApproved",
            DynConfidential => "PCDynConfidential",
            DynReceived => "PCDynReceived",
            DynReviewed => "PCDynReviewed",
            DynRevised => "PCDynRevised",
        }
    }

    pub fn from_name(n: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name().as_bytes() == n)
    }

    /// The stamp's colour: green for approval, red for refusal and restriction, blue otherwise.
    pub fn color(self) -> Rgb {
        use StampKind::*;
        match self {
            Approved | Completed | Final | Accepted | DynApproved | DynReceived | DynReviewed => [0.13, 0.55, 0.13],
            NotApproved | Rejected | Void | Confidential | NotForPublicRelease | DynConfidential => [0.80, 0.10, 0.10],
            SignHere | InitialHere | Witness => [0.85, 0.35, 0.05],
            _ => [0.10, 0.30, 0.70],
        }
    }

    /// The stamp's size (points) for its label, as placed with a click.
    pub fn size(self) -> (f64, f64) {
        let w = appearance::text_width(self.label(), 16.0) * 1.12 + 24.0;
        let h = if self.group() == StampGroup::Dynamic { 42.0 } else { 30.0 };
        let w = if self.group() == StampGroup::SignHere { w + 14.0 } else { w };
        (w.max(80.0), h)
    }
}

/// A line ending (`/LE`): the ten styles in ISO 32000-2 Table 217.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ROpenArrow,
    RClosedArrow,
    Slash,
}

impl LineEnding {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Square,
        Self::Circle,
        Self::Diamond,
        Self::OpenArrow,
        Self::ClosedArrow,
        Self::Butt,
        Self::ROpenArrow,
        Self::RClosedArrow,
        Self::Slash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Diamond => "Diamond",
            Self::OpenArrow => "OpenArrow",
            Self::ClosedArrow => "ClosedArrow",
            Self::Butt => "Butt",
            Self::ROpenArrow => "ROpenArrow",
            Self::RClosedArrow => "RClosedArrow",
            Self::Slash => "Slash",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ending| ending.name() == name)
    }
}

/// Geometry of a new comment, in PDF user space of its page.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A sticky note whose icon's top-left corner is at `at`.
    Note {
        at: [f64; 2],
        icon: NoteIcon,
    },
    /// Text markup over quadrilaterals: `[x1 y1 x2 y2 x3 y3 x4 y4]` = top-left, top-right,
    /// bottom-left, bottom-right (the order Acrobat writes, §12.5.6.10).
    TextMarkup {
        kind: Markup,
        quads: Vec<[f64; 8]>,
    },
    Rectangle {
        rect: [f64; 4],
    },
    Oval {
        rect: [f64; 4],
    },
    /// A line. An open arrow at `to` is `start: None`, `end: OpenArrow` (the Arrow tool).
    Line {
        from: [f64; 2],
        to: [f64; 2],
        start: LineEnding,
        end: LineEnding,
    },
    /// Freehand strokes (Draw tool).
    Ink {
        strokes: Vec<Vec<[f64; 2]>>,
    },
    /// A text box (FreeText) showing the comment's contents.
    TextBox {
        rect: [f64; 4],
        font_size: f64,
    },
    /// Fill & Sign text typed onto the page (FreeText, typewriter intent, no border).
    Typewriter {
        rect: [f64; 4],
        font_size: f64,
    },
    /// A Fill & Sign mark in `rect`.
    Mark {
        rect: [f64; 4],
        mark: FillMark,
    },
    /// A drawn signature (ink strokes).
    Signature {
        strokes: Vec<Vec<[f64; 2]>>,
    },
    /// A rubber stamp from the stamp palette; `by` is the dynamic stamps' second line.
    Stamp {
        rect: [f64; 4],
        stamp: StampKind,
        by: Option<String>,
    },
    /// A typed signature or initials: filled outlines (each contour as points normalised to
    /// `rect`, 0–1, y up), drawn with the non-zero winding rule.
    TypedSignature {
        rect: [f64; 4],
        contours: Vec<Vec<[f64; 2]>>,
    },
    /// A custom stamp: a picture already in the document (an image XObject, or a form XObject
    /// whose `/Matrix` maps it to `size` points) filling `rect`, named `name`.
    CustomStamp {
        rect: [f64; 4],
        name: String,
        picture: ObjRef,
        image: bool,
        size: (f64, f64),
    },
    /// A redaction mark (§12.5.6.23) over quadrilaterals (text) or one rectangle as a quad
    /// (areas, pages). `overlay` is the text shown on the box once applied, drawn with `look`.
    Redact {
        quads: Vec<[f64; 8]>,
        overlay: String,
        look: OverlayLook,
    },
    /// A closed polygon (Polygon tool); with `cloud`, Acrobat's Cloud tool: the same polygon
    /// with a cloudy border (`/BE /S /C`, `/IT /PolygonCloud`).
    Polygon {
        vertices: Vec<[f64; 2]>,
        cloud: bool,
    },
    /// Connected lines (Polygonal Line tool). `start` is the first vertex, `end` the last.
    PolyLine {
        vertices: Vec<[f64; 2]>,
        start: LineEnding,
        end: LineEnding,
    },
    /// A text callout: a text box at `rect` with a leader line from `point` (arrowhead) via
    /// `knee` to the box (FreeText, `/IT /FreeTextCallout`, `/CL`).
    Callout {
        rect: [f64; 4],
        knee: [f64; 2],
        point: [f64; 2],
        font_size: f64,
        ending: LineEnding,
    },
    /// Insert text: a caret in `rect` (its point at the top centre).
    Caret {
        rect: [f64; 4],
    },
    /// Attach a file as a comment: an icon whose top-left corner is at `at`, holding `data`
    /// (embedded) under the name `file`.
    Attachment {
        at: [f64; 2],
        icon: AttachIcon,
        file: String,
        data: Vec<u8>,
    },
}

/// The standard file attachment icons (§12.5.6.15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachIcon {
    #[default]
    PushPin,
    Paperclip,
    Graph,
    Tag,
}

impl AttachIcon {
    pub const ALL: [AttachIcon; 4] = [AttachIcon::PushPin, AttachIcon::Paperclip, AttachIcon::Graph, AttachIcon::Tag];
    pub fn name(self) -> &'static str {
        match self {
            AttachIcon::PushPin => "PushPin",
            AttachIcon::Paperclip => "Paperclip",
            AttachIcon::Graph => "Graph",
            AttachIcon::Tag => "Tag",
        }
    }
    pub fn from_name(n: &str) -> Option<AttachIcon> {
        Self::ALL.into_iter().find(|i| i.name().eq_ignore_ascii_case(n))
    }
}

/// A rectangle `[x0 y0 x1 y1]` as a quad in Acrobat's order (top-left, top-right, bottom-left,
/// bottom-right).
pub fn rect_quad(r: [f64; 4]) -> [f64; 8] {
    let [x0, y0, x1, y1] = normalize(r);
    [x0, y1, x1, y1, x0, y0, x1, y0]
}

impl Shape {
    pub fn subtype(&self) -> &'static str {
        match self {
            Shape::Note { .. } => "Text",
            Shape::TextMarkup { kind, .. } => kind.subtype(),
            Shape::Rectangle { .. } => "Square",
            Shape::Oval { .. } => "Circle",
            Shape::Line { .. } => "Line",
            Shape::Ink { .. } | Shape::Signature { .. } => "Ink",
            Shape::TextBox { .. } | Shape::Typewriter { .. } | Shape::Callout { .. } => "FreeText",
            Shape::Mark { .. } | Shape::Stamp { .. } | Shape::CustomStamp { .. } | Shape::TypedSignature { .. } => "Stamp",
            Shape::Redact { .. } => "Redact",
            Shape::Polygon { .. } => "Polygon",
            Shape::PolyLine { .. } => "PolyLine",
            Shape::Caret { .. } => "Caret",
            Shape::Attachment { .. } => "FileAttachment",
        }
    }
}

/// The standard fonts of redaction overlay text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayFont {
    #[default]
    Helvetica,
    Times,
    Courier,
}

impl OverlayFont {
    pub const ALL: [OverlayFont; 3] = [OverlayFont::Helvetica, OverlayFont::Times, OverlayFont::Courier];

    pub fn name(self) -> &'static str {
        match self {
            OverlayFont::Helvetica => "Helvetica",
            OverlayFont::Times => "Times Roman",
            OverlayFont::Courier => "Courier",
        }
    }

    /// The `/DA` font resource name (Acrobat's form font names).
    pub fn resource(self) -> &'static str {
        match self {
            OverlayFont::Helvetica => "Helv",
            OverlayFont::Times => "TiRo",
            OverlayFont::Courier => "Cour",
        }
    }

    pub fn from_resource(n: &str) -> OverlayFont {
        match n {
            "TiRo" | "Times-Roman" | "TimesRoman" => OverlayFont::Times,
            "Cour" | "Courier" => OverlayFont::Courier,
            _ => OverlayFont::Helvetica,
        }
    }
}

/// How redaction overlay text is drawn (Redaction Properties ▸ Appearance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayLook {
    pub font: OverlayFont,
    /// Points; 0 = auto-size to fit the area.
    pub size: f64,
    pub color: Rgb,
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    /// Repeat the text to fill the area.
    pub repeat: bool,
}

impl Default for OverlayLook {
    fn default() -> Self {
        Self { font: OverlayFont::Helvetica, size: 0.0, color: [1.0, 0.0, 0.0], align: 1, repeat: false }
    }
}

/// How a comment looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Stroke colour (text colour for text boxes, icon colour for notes).
    pub color: Rgb,
    /// 0–1 (`/CA`).
    pub opacity: f64,
    /// Border / stroke width in points (0 = none).
    pub width: f64,
    /// Interior fill for rectangles and ovals (`/IC`), background for text boxes.
    pub fill: Option<Rgb>,
}

impl Default for Style {
    fn default() -> Self {
        Self { color: [1.0, 0.82, 0.0], opacity: 1.0, width: 1.0, fill: None }
    }
}

impl Style {
    /// The default look of each commenting tool.
    pub fn default_for(shape: &Shape) -> Self {
        let (color, width) = match shape {
            Shape::Note { .. } => ([1.0, 0.82, 0.0], 1.0),
            Shape::TextMarkup { kind: Markup::Highlight, .. } => ([1.0, 0.94, 0.0], 1.0),
            Shape::TextMarkup { kind: Markup::Underline, .. } => ([0.0, 0.47, 0.84], 1.0),
            Shape::TextMarkup { kind: Markup::StrikeOut, .. } => ([0.89, 0.13, 0.13], 1.0),
            Shape::TextMarkup { kind: Markup::Squiggly, .. } => ([0.18, 0.62, 0.36], 1.0),
            Shape::Rectangle { .. } | Shape::Oval { .. } | Shape::Line { .. } | Shape::Polygon { .. } | Shape::PolyLine { .. } => {
                ([0.89, 0.13, 0.13], 2.0)
            }
            Shape::Callout { .. } => ([0.0, 0.0, 0.0], 1.0),
            Shape::Caret { .. } | Shape::Attachment { .. } => ([0.0, 0.47, 0.84], 1.0),
            Shape::Ink { .. } => ([0.0, 0.4, 0.87], 2.0),
            Shape::TextBox { .. } | Shape::Typewriter { .. } => ([0.0, 0.0, 0.0], 0.0),
            Shape::Mark { .. } => ([0.0, 0.0, 0.0], 1.5),
            Shape::Signature { .. } => ([0.0, 0.0, 0.0], 1.5),
            Shape::Stamp { stamp, .. } => (stamp.color(), 2.0),
            Shape::CustomStamp { .. } => ([0.0, 0.0, 0.0], 0.0),
            Shape::TypedSignature { .. } => ([0.0, 0.0, 0.0], 0.0),
            // Red outline while marked; a black box once applied.
            Shape::Redact { .. } => return Self { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: Some([0.0, 0.0, 0.0]) },
        };
        Self { color, opacity: 1.0, width, fill: None }
    }
}

/// A comment to add.
#[derive(Clone, Debug, PartialEq)]
pub struct NewAnnotation {
    /// 0-based page index.
    pub page: usize,
    pub shape: Shape,
    pub style: Style,
    /// The comment text (`/Contents`); the text shown by a text box.
    pub contents: String,
    /// `/T` (shown as the comment's author).
    pub author: String,
}

/// Values stamped onto what an edit creates or changes, supplied by the caller so edits stay
/// deterministic: a PDF date (`D:…`) and a unique id for `/NM`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Meta {
    pub date: Option<String>,
    pub id: String,
}

/// Review states a reply can set (§12.5.6.3, Table 172; Acrobat's "Set status").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewState {
    None,
    Accepted,
    Rejected,
    Cancelled,
    Completed,
}

impl ReviewState {
    pub fn name(self) -> &'static str {
        match self {
            ReviewState::None => "None",
            ReviewState::Accepted => "Accepted",
            ReviewState::Rejected => "Rejected",
            ReviewState::Cancelled => "Cancelled",
            ReviewState::Completed => "Completed",
        }
    }

    pub fn from_name(n: &str) -> Option<Self> {
        [Self::None, Self::Accepted, Self::Rejected, Self::Cancelled, Self::Completed].into_iter().find(|s| s.name().eq_ignore_ascii_case(n))
    }
}

// ── pages and /Annots ───────────────────────────────────────────────────────────────────────

/// The page object of each leaf page, in order.
pub fn page_refs(doc: &Document) -> Result<Vec<ObjRef>, AnnotError> {
    let root = doc.root().ok_or(AnnotError::NoPageTree)?;
    let pages = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")).ok_or(AnnotError::NoPageTree)?;
    Ok(PageWalk::new(doc, pages).collect())
}

/// Depth-first walk over the leaf pages under `top`. It keeps one cursor per open `/Kids` array,
/// never one entry per child, so a child listed many times or shared between nodes is skipped by
/// the seen check without being queued first (`pdfcraft_model::pages` walks the same way).
/// Cycles end at the seen check, a `/Kids` array object shared by several nodes is read once,
/// and at most a million nodes and four million `/Kids` entries are read.
struct PageWalk<'a> {
    doc: &'a Document,
    seen: std::collections::HashSet<ObjRef>,
    pending: Vec<(std::sync::Arc<Object>, usize)>,
    /// `/Kids` entries read so far.
    examined: usize,
}

impl<'a> PageWalk<'a> {
    fn new(doc: &'a Document, top: ObjRef) -> Self {
        let top = std::sync::Arc::new(Object::Array(vec![Object::Ref(top)]));
        Self { doc, seen: Default::default(), pending: vec![(top, 0)], examined: 0 }
    }
}

impl Iterator for PageWalk<'_> {
    type Item = ObjRef;

    fn next(&mut self) -> Option<ObjRef> {
        while let Some((kids, next)) = self.pending.last_mut() {
            let Some(kid) = kids.as_array().and_then(|a| a.get(*next)) else {
                self.pending.pop();
                continue;
            };
            let node = kid.as_ref();
            *next += 1;
            self.examined += 1;
            let Some(node) = node else { continue };
            if self.seen.len() >= 1_000_000 || self.examined > 4_000_000 {
                self.pending.clear();
                return None;
            }
            if !self.seen.insert(node) {
                continue;
            }
            let obj = self.doc.get(node);
            let Some(d) = obj.as_dict() else { continue };
            match d.get(b"Kids") {
                // A `/Kids` array object shared by several nodes is read under the first: under
                // the others it would only list nodes already seen.
                Some(Object::Ref(array)) if d.name(b"Type") != Some(b"Page") => {
                    if self.seen.insert(*array) {
                        self.pending.push((self.doc.get(*array), 0));
                    }
                }
                Some(kids) if d.name(b"Type") != Some(b"Page") => self.pending.push((self.doc.resolve(kids), 0)),
                _ => return Some(node),
            }
        }
        None
    }
}

fn page_ref(doc: &Document, page: usize) -> Result<ObjRef, AnnotError> {
    page_refs(doc)?.get(page).copied().ok_or(AnnotError::NoSuchPage(page))
}

/// The entries of a page's `/Annots` (references or inline dictionaries).
fn annots(doc: &Document, page: ObjRef) -> Vec<Object> {
    let obj = doc.get(page);
    let Some(a) = obj.as_dict().and_then(|d| d.get(b"Annots")) else { return Vec::new() };
    let list = doc.resolve(a).as_array().cloned().unwrap_or_default();
    #[cfg(test)]
    ANNOTS_ENTRIES_COPIED.with(|n| n.set(n.get() + list.len()));
    list
}

#[cfg(test)]
thread_local! {
    /// Entries copied out of pages' `/Annots` arrays.
    static ANNOTS_ENTRIES_COPIED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Replace a page's `/Annots`, writing into the shared array object when it is indirect.
fn set_annots(doc: &mut Document, page: ObjRef, list: Vec<Object>) -> Result<(), AnnotError> {
    let existing = doc.get(page).as_dict().and_then(|d| d.reference(b"Annots"));
    match existing {
        Some(r) if doc.get(r).as_array().is_some() => doc.set(r, Object::Array(list)),
        _ => doc.update_dict(page, |d| {
            if list.is_empty() {
                d.remove(b"Annots");
            } else {
                d.set(b"Annots".to_vec(), Object::Array(list));
            }
        })?,
    }
    Ok(())
}

/// The annotation at `(page, index)` as an indirect object (inline dictionaries are promoted).
fn annot_ref(doc: &mut Document, page: usize, index: usize) -> Result<(ObjRef, ObjRef), AnnotError> {
    let p = page_ref(doc, page)?;
    let mut list = annots(doc, p);
    let entry = list.get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let r = match entry {
        Object::Ref(r) if doc.get(r).as_dict().is_some() => r,
        Object::Dict(d) => {
            let r = doc.add(Object::Dict(d));
            list[index] = Object::Ref(r);
            set_annots(doc, p, list)?;
            r
        }
        _ => return Err(AnnotError::NoSuchAnnotation { page, index }),
    };
    Ok((p, r))
}

fn annot_dict(doc: &Document, r: ObjRef) -> Dict {
    doc.get(r).as_dict().cloned().unwrap_or_default()
}

/// A Fill & Sign item: typed text, a check/cross/dot/line, or a typed, drawn or image signature.
///
/// New items carry `/PCFillSign`. Older files are recognised from the dictionaries PdfCraft
/// already writes (`/IT /FreeTextTypeWriter`, stamp `/Name`, drawn ink whose subject is
/// "Signature").
pub fn is_fill_sign(doc: &Document, d: &Dict) -> bool {
    if matches!(d.get(b"PCFillSign").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true))) {
        return true;
    }
    let subtype = d.name(b"Subtype").unwrap_or_default();
    if subtype == b"FreeText" && d.name(b"IT") == Some(b"FreeTextTypeWriter") {
        return true;
    }
    if subtype == b"Stamp" {
        if matches!(d.name(b"Name"), Some(b"PCCheck" | b"PCCross" | b"PCDot" | b"PCLine" | b"PCTypedSignature")) {
            return true;
        }
        if matches!(d.name(b"Name"), Some(b"PCCustomSignature" | b"PCCustomInitials"))
            && matches!(d.get(b"PCPictureImage").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true)))
        {
            return true;
        }
    }
    subtype == b"Ink" && text_value(doc, d, b"Subj").as_deref() == Some("Signature")
}

fn annotation_flags(doc: &Document, d: &Dict) -> i64 {
    d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0)
}

/// Whether any page has a Fill & Sign annotation that flatten would bake in (not hidden).
pub fn has_visible_fill_sign(doc: &Document) -> bool {
    let Ok(pages) = page_refs(doc) else { return false };
    for page in pages {
        for entry in annots(doc, page) {
            let obj = doc.resolve(&entry);
            let Some(d) = obj.as_dict() else { continue };
            let flags = annotation_flags(doc, d);
            if flags & (FLAG_HIDDEN | FLAG_NO_VIEW) != 0 {
                continue;
            }
            if is_fill_sign(doc, d) {
                return true;
            }
        }
    }
    false
}

fn fill_sign_shape(shape: &Shape) -> bool {
    match shape {
        Shape::Typewriter { .. } | Shape::Mark { .. } | Shape::Signature { .. } | Shape::TypedSignature { .. } => true,
        Shape::CustomStamp { name, image: true, .. } => name == "Signature" || name == "Initials",
        _ => false,
    }
}

/// The embedded image of a Fill & Sign image signature or initials (0-based target).
/// Other stamps have appearances that can't be represented by this image alone.
pub fn signature_image(doc: &Document, page: usize, index: usize) -> Result<Option<ObjRef>, AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let obj = doc.resolve(entry);
    let Some(d) = obj.as_dict() else { return Ok(None) };
    Ok((d.name(b"Subtype") == Some(b"Stamp")
        && matches!(d.name(b"Name"), Some(b"PCCustomSignature" | b"PCCustomInitials"))
        && matches!(d.get(b"PCPictureImage"), Some(Object::Bool(true))))
    .then(|| d.reference(b"PCPicture"))
    .flatten())
}

/// The page `/Rotate` the picture of the image signature at `(page, index)` is drawn turned back
/// by, so it reads upright on a page shown that way: 0 for one added to an unturned page, or by
/// another app. The picture appears turned by the page's current rotation less this.
pub fn picture_rotation(doc: &Document, page: usize, index: usize) -> Result<i64, AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    Ok(doc.resolve(entry).as_dict().map_or(0, appearance::picture_turn))
}

/// Leave page `page` of (a copy of) a document showing only its annotation at `index`, without
/// the page's content or other annotations, so a render draws that annotation alone: to lay it
/// over the page while it is dragged. `false`, with `doc` unchanged, when the annotation has no
/// normal appearance (`/AP /N`) for that render to draw.
pub fn isolate_annotation(doc: &mut Document, page: usize, index: usize) -> Result<bool, AnnotError> {
    let p = page_ref(doc, page)?;
    let entry = annots(doc, p).get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let has_normal = {
        let obj = doc.resolve(&entry);
        obj.as_dict().and_then(|d| d.get(b"AP")).is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N")))
    };
    if !has_normal {
        return Ok(false);
    }
    set_annots(doc, p, vec![entry])?;
    doc.update_dict(p, |d| {
        d.remove(b"Contents");
    })?;
    Ok(true)
}

// ── building ────────────────────────────────────────────────────────────────────────────────

/// Annotation flags (§12.5.3).
const FLAG_PRINT: i64 = 4;
const FLAG_NO_ZOOM: i64 = 8;
const FLAG_NO_ROTATE: i64 = 16;
const FLAG_HIDDEN: i64 = 2;
const FLAG_NO_VIEW: i64 = 32;
const FLAG_LOCKED: i64 = 128;

/// Size of a note icon (points, unscaled by zoom).
pub const NOTE_SIZE: f64 = 20.0;

fn num_array(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

fn rgb(c: Rgb) -> Object {
    num_array(&c.map(|x| x.clamp(0.0, 1.0)))
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() < 1e7)
}

fn normalize(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

fn bounds(points: impl Iterator<Item = [f64; 2]>) -> Option<[f64; 4]> {
    points.fold(None, |acc, [x, y]| match acc {
        None => Some([x, y, x, y]),
        Some([a, b, c, d]) => Some([a.min(x), b.min(y), c.max(x), d.max(y)]),
    })
}

fn grow(r: [f64; 4], by: f64) -> [f64; 4] {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

fn ending_draws(ending: LineEnding) -> bool {
    ending != LineEnding::None
}

fn ending_pair(start: LineEnding, end: LineEnding) -> Object {
    Object::Array(vec![Object::name(start.name()), Object::name(end.name())])
}

/// Validate and compute `/Rect` for a new comment.
fn rect_for(shape: &Shape, style: &Style) -> Result<[f64; 4], AnnotError> {
    let bad = |what: &str| AnnotError::Invalid(format!("invalid {what}"));
    let half = style.width.max(0.0) / 2.0;
    let r = match shape {
        Shape::Note { at, .. } | Shape::Attachment { at, .. } => {
            if !finite(at) {
                return Err(bad("position"));
            }
            [at[0], at[1] - NOTE_SIZE, at[0] + NOTE_SIZE, at[1]]
        }
        Shape::TextMarkup { quads, .. } | Shape::Redact { quads, .. } => {
            if quads.is_empty() || !quads.iter().all(|q| finite(q)) {
                return Err(bad("text area (no quadrilaterals)"));
            }
            bounds(quads.iter().flat_map(|q| q.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect::<Vec<_>>()))
                .ok_or_else(|| bad("text area"))?
        }
        Shape::Rectangle { rect }
        | Shape::Oval { rect }
        | Shape::TextBox { rect, .. }
        | Shape::Typewriter { rect, .. }
        | Shape::Stamp { rect, .. }
        | Shape::TypedSignature { rect, .. }
        | Shape::Mark { rect, .. } => {
            let r = normalize(*rect);
            if !finite(rect) || r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
                return Err(bad("rectangle (too small)"));
            }
            r
        }
        Shape::CustomStamp { rect, .. } => {
            let r = normalize(*rect);
            // Image signatures may be very thin; a positive PDF appearance box still works.
            if !finite(rect) || r[2] <= r[0] || r[3] <= r[1] {
                return Err(bad("rectangle (empty)"));
            }
            r
        }
        Shape::Line { from, to, start, end } => {
            if !finite(from) || !finite(to) || (from[0] - to[0]).hypot(from[1] - to[1]) < 1.0 {
                return Err(bad("line (too short)"));
            }
            let pad = half + if ending_draws(*start) || ending_draws(*end) { appearance::arrow_size(style.width) } else { 0.0 };
            grow(bounds([*from, *to].into_iter()).unwrap_or_default(), pad + 1.0)
        }
        Shape::Ink { strokes } | Shape::Signature { strokes } => {
            if strokes.iter().all(Vec::is_empty) || !strokes.iter().flatten().all(|p| finite(p)) {
                return Err(bad("drawing (no points)"));
            }
            // Strokes of three or more points are drawn as curves, which stay within their
            // points and control points.
            let controls = strokes.iter().filter(|s| s.len() > 2).flat_map(|s| {
                let pts: Vec<(f64, f64)> = s.iter().map(|p| (p[0], p[1])).collect();
                appearance::smooth_segments(&pts).into_iter().flat_map(|[a, b, _]| [[a.0, a.1], [b.0, b.1]])
            });
            grow(bounds(strokes.iter().flatten().copied().chain(controls)).unwrap_or_default(), half + 1.0)
        }
        Shape::Polygon { vertices, cloud } => {
            let b = bounds(vertices.iter().copied())
                .filter(|b| vertices.len() >= 3 && vertices.iter().all(|p| finite(p)) && b[2] - b[0] >= 1.0 && b[3] - b[1] >= 1.0);
            let b = b.ok_or_else(|| bad("polygon (it needs three points)"))?;
            grow(b, half + 1.0 + if *cloud { 1.5 * appearance::cloud_radius(style.width) } else { 0.0 })
        }
        Shape::PolyLine { vertices, start, end } => {
            let b = bounds(vertices.iter().copied())
                .filter(|b| vertices.len() >= 2 && vertices.iter().all(|p| finite(p)) && (b[2] - b[0]).max(b[3] - b[1]) >= 1.0);
            let extra = if ending_draws(*start) || ending_draws(*end) { appearance::arrow_size(style.width) } else { 0.0 };
            grow(b.ok_or_else(|| bad("connected lines (they need two points)"))?, half + 1.0 + extra)
        }
        Shape::Callout { rect, knee, point, .. } => {
            let r = normalize(*rect);
            if !finite(rect) || !finite(knee) || !finite(point) || r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
                return Err(bad("callout"));
            }
            let pad = half + appearance::arrow_size(style.width) + 1.0;
            let lead = grow(bounds([*knee, *point].into_iter()).unwrap_or_default(), pad);
            [r[0].min(lead[0]), r[1].min(lead[1]), r[2].max(lead[2]), r[3].max(lead[3])]
        }
        Shape::Caret { rect } => {
            let r = normalize(*rect);
            if !finite(rect) || r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
                return Err(bad("caret"));
            }
            r
        }
    };
    Ok(r)
}

fn base_dict(subtype: &str, rect: [f64; 4], page: ObjRef, contents: &str, author: &str, meta: &Meta) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name(subtype));
    d.set(b"Rect".to_vec(), num_array(&rect));
    d.set(b"Contents".to_vec(), PdfString::text(contents));
    if !author.is_empty() {
        d.set(b"T".to_vec(), PdfString::text(author));
    }
    if let Some(date) = &meta.date {
        d.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
        d.set(b"CreationDate".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    }
    if !meta.id.is_empty() {
        d.set(b"NM".to_vec(), PdfString::text(&meta.id));
    }
    d.set(b"P".to_vec(), Object::Ref(page));
    d.set(b"F".to_vec(), Object::Int(FLAG_PRINT));
    d
}

/// Acrobat's `/Subj` for each tool (shown as the comment type in other viewers).
fn subject(shape: &Shape) -> &'static str {
    match shape {
        Shape::Note { .. } => "Sticky Note",
        Shape::TextMarkup { kind: Markup::Highlight, .. } => "Highlight",
        Shape::TextMarkup { kind: Markup::Underline, .. } => "Underline",
        Shape::TextMarkup { kind: Markup::StrikeOut, .. } => "Strikethrough",
        Shape::TextMarkup { kind: Markup::Squiggly, .. } => "Squiggly",
        Shape::Rectangle { .. } => "Rectangle",
        Shape::Oval { .. } => "Oval",
        Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. } => "Arrow",
        Shape::Line { .. } => "Line",
        Shape::Ink { .. } => "Pencil",
        Shape::TextBox { .. } => "Text Box",
        Shape::Typewriter { .. } => "Typewriter",
        Shape::Mark { mark, .. } => mark.label(),
        Shape::Signature { .. } => "Signature",
        Shape::Redact { .. } => "Redact",
        Shape::Stamp { stamp, .. } => match stamp.group() {
            StampGroup::Dynamic => "Dynamic stamp",
            StampGroup::SignHere => "Sign Here",
            StampGroup::StandardBusiness => "Stamp",
        },
        Shape::CustomStamp { .. } => "Stamp",
        Shape::TypedSignature { .. } => "Signature",
        Shape::Polygon { cloud: true, .. } => "Cloud",
        Shape::Polygon { .. } => "Polygon",
        Shape::PolyLine { .. } => "Polygonal Line",
        Shape::Callout { .. } => "Callout",
        Shape::Caret { .. } => "Inserted Text",
        Shape::Attachment { .. } => "File Attachment",
    }
}

/// Where a callout's leader line meets its text box: the middle of the side facing `knee`.
pub fn callout_attach(rect: [f64; 4], knee: [f64; 2]) -> [f64; 2] {
    let [x0, y0, x1, y1] = normalize(rect);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    if knee[0] < x0 {
        [x0, cy]
    } else if knee[0] > x1 {
        [x1, cy]
    } else if knee[1] > y1 {
        [cx, y1]
    } else {
        [cx, y0]
    }
}

/// Add a comment; returns its index in the page's `/Annots`.
pub fn add_annotation(doc: &mut Document, new: &NewAnnotation, meta: &Meta) -> Result<usize, AnnotError> {
    let page = page_ref(doc, new.page)?;
    let style = &new.style;
    if !finite(&style.color) || !style.opacity.is_finite() || !style.width.is_finite() {
        return Err(AnnotError::Invalid("invalid style".into()));
    }
    let rect = rect_for(&new.shape, style)?;
    let mut d = base_dict(new.shape.subtype(), rect, page, &new.contents, &new.author, meta);
    d.set(b"Subj".to_vec(), PdfString::text(subject(&new.shape)));
    let opacity = style.opacity.clamp(0.0, 1.0);
    if opacity < 1.0 {
        d.set(b"CA".to_vec(), Object::Real(opacity));
    }
    let border = |d: &mut Dict| {
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(style.width.max(0.0)));
        bs.set(b"S".to_vec(), Object::name("S"));
        d.set(b"BS".to_vec(), Object::Dict(bs));
    };
    let mut popup = None;
    match &new.shape {
        Shape::Note { icon, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(icon.name()));
            d.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
            d.set(b"Open".to_vec(), Object::Bool(false));
            popup = Some([rect[2] + 10.0, rect[3] - 120.0, rect[2] + 210.0, rect[3]]);
        }
        Shape::TextMarkup { quads, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"QuadPoints".to_vec(), num_array(&quads.concat()));
        }
        Shape::Redact { quads, overlay, look } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"IC".to_vec(), rgb(style.fill.unwrap_or([0.0, 0.0, 0.0])));
            d.set(b"QuadPoints".to_vec(), num_array(&quads.concat()));
            if !overlay.is_empty() {
                d.set(b"OverlayText".to_vec(), PdfString::text(overlay));
                let [r, g, b] = look.color.map(|x| x.clamp(0.0, 1.0));
                let size = if look.size > 0.0 { look.size.min(400.0) } else { 0.0 };
                d.set(
                    b"DA".to_vec(),
                    PdfString::literal(format!("{} {} {} rg /{} {} Tf", n(r), n(g), n(b), look.font.resource(), n(size)).into_bytes()),
                );
                d.set(b"Q".to_vec(), Object::Int(look.align.min(2) as i64));
                if look.repeat {
                    d.set(b"Repeat".to_vec(), Object::Bool(true));
                }
            }
        }
        Shape::Rectangle { .. } | Shape::Oval { .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            if let Some(f) = style.fill {
                d.set(b"IC".to_vec(), rgb(f));
            }
            border(&mut d);
        }
        Shape::Line { from, to, start, end } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"L".to_vec(), num_array(&[from[0], from[1], to[0], to[1]]));
            if ending_draws(*start) || ending_draws(*end) {
                d.set(b"LE".to_vec(), ending_pair(*start, *end));
            }
            border(&mut d);
        }
        Shape::Mark { mark, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(mark.name()));
            border(&mut d);
            // Drawn upright for the page as displayed, as a stamp is (#2392).
            let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
            if matches!(turn, 90 | 180 | 270) {
                d.set(b"PCStampRotate".to_vec(), Object::Int(turn));
            }
        }
        Shape::Stamp { stamp, by, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(stamp.name()));
            // Marks the stamp as drawn by PdfCraft: other stamps with standard names keep
            // their own artwork.
            d.set(b"PCStamp".to_vec(), Object::Bool(true));
            if let Some(b) = by {
                d.set(b"PCByLine".to_vec(), PdfString::text(b));
            }
            // The label is drawn for the page as displayed. A later page rotation is a new edit.
            let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
            if matches!(turn, 90 | 180 | 270) {
                d.set(b"PCStampRotate".to_vec(), Object::Int(turn));
            }
        }
        Shape::TypedSignature { contours, .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name("PCTypedSignature"));
            let list = contours.iter().filter(|c| c.len() > 2).map(|c| num_array(&c.iter().flat_map(|p| [p[0], p[1]]).collect::<Vec<_>>())).collect();
            d.set(b"PCOutline".to_vec(), Object::Array(list));
        }
        Shape::CustomStamp { name, picture, image, size, .. } => {
            let clean: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
            d.set(b"Name".to_vec(), Object::name(&format!("PCCustom{clean}")));
            d.set(b"PCPicture".to_vec(), Object::Ref(*picture));
            d.set(b"PCPictureImage".to_vec(), Object::Bool(*image));
            d.set(b"PCPictureSize".to_vec(), num_array(&[size.0, size.1]));
            // An image signature's rectangle is in user space; on a turned page its appearance is
            // turned back (see `appearance::build`), so it reads upright as displayed.
            if *image && matches!(clean.as_str(), "Signature" | "Initials") {
                let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
                if turn != 0 {
                    d.set(b"PCPictureRotate".to_vec(), Object::Int(turn));
                }
            }
        }
        Shape::Ink { strokes } | Shape::Signature { strokes } => {
            d.set(b"C".to_vec(), rgb(style.color));
            let list = strokes.iter().filter(|s| !s.is_empty()).map(|s| num_array(&s.concat())).collect();
            d.set(b"InkList".to_vec(), Object::Array(list));
            border(&mut d);
        }
        Shape::Polygon { vertices, cloud } => {
            d.set(b"C".to_vec(), rgb(style.color));
            if let Some(f) = style.fill {
                d.set(b"IC".to_vec(), rgb(f));
            }
            d.set(b"Vertices".to_vec(), num_array(&vertices.concat()));
            if *cloud {
                d.set(b"IT".to_vec(), Object::name("PolygonCloud"));
                let mut be = Dict::new();
                be.set(b"S".to_vec(), Object::name("C"));
                be.set(b"I".to_vec(), Object::Int(1));
                d.set(b"BE".to_vec(), Object::Dict(be));
            }
            border(&mut d);
        }
        Shape::PolyLine { vertices, start, end } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Vertices".to_vec(), num_array(&vertices.concat()));
            if ending_draws(*start) || ending_draws(*end) {
                d.set(b"LE".to_vec(), ending_pair(*start, *end));
            }
            border(&mut d);
        }
        Shape::Caret { .. } => {
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Sy".to_vec(), Object::name("None"));
        }
        Shape::Attachment { icon, file, data, .. } => {
            if file.trim().is_empty() {
                return Err(AnnotError::Invalid("the attached file needs a name".into()));
            }
            d.set(b"C".to_vec(), rgb(style.color));
            d.set(b"Name".to_vec(), Object::name(icon.name()));
            d.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
            // The file: an embedded file stream inside a file specification (§7.11.4).
            let mut params = Dict::new();
            params.set(b"Size".to_vec(), Object::Int(data.len() as i64));
            if let Some(date) = &meta.date {
                params.set(b"ModDate".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
            }
            let mut ef = Dict::new();
            ef.set(b"Type".to_vec(), Object::name("EmbeddedFile"));
            ef.set(b"Params".to_vec(), Object::Dict(params));
            let ef = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(ef, data)));
            let mut efd = Dict::new();
            efd.set(b"F".to_vec(), Object::Ref(ef));
            efd.set(b"UF".to_vec(), Object::Ref(ef));
            let mut fs = Dict::new();
            fs.set(b"Type".to_vec(), Object::name("Filespec"));
            fs.set(b"F".to_vec(), PdfString::text(file.trim()));
            fs.set(b"UF".to_vec(), PdfString::text(file.trim()));
            fs.set(b"EF".to_vec(), Object::Dict(efd));
            d.set(b"FS".to_vec(), Object::Ref(doc.add(Object::Dict(fs))));
            if new.contents.is_empty() {
                d.set(b"Contents".to_vec(), PdfString::text(file.trim()));
            }
        }
        Shape::Callout { rect: tb, knee, point, ending, .. } => {
            let tb = normalize(*tb);
            let attach = callout_attach(tb, *knee);
            d.set(b"IT".to_vec(), Object::name("FreeTextCallout"));
            d.set(b"CL".to_vec(), num_array(&[point[0], point[1], knee[0], knee[1], attach[0], attach[1]]));
            d.set(b"LE".to_vec(), Object::name(ending.name()));
            // The text box inside /Rect (§12.5.6.6 /RD).
            d.set(b"RD".to_vec(), num_array(&[tb[0] - rect[0], tb[1] - rect[1], rect[2] - tb[2], rect[3] - tb[3]]));
        }
        _ => {}
    }
    match &new.shape {
        Shape::TextBox { font_size, .. } | Shape::Typewriter { font_size, .. } | Shape::Callout { font_size, .. } => {
            if matches!(new.shape, Shape::Typewriter { .. }) {
                d.set(b"IT".to_vec(), Object::name("FreeTextTypeWriter"));
            }
            let size = if font_size.is_finite() && *font_size > 0.0 { font_size.clamp(1.0, 400.0) } else { 12.0 };
            let [r, g, b] = style.color.map(|x| x.clamp(0.0, 1.0));
            d.set(b"DA".to_vec(), PdfString::literal(format!("{} {} {} rg /Helv {} Tf", n(r), n(g), n(b), n(size)).into_bytes()));
            d.set(b"Q".to_vec(), Object::Int(0));
            if let Some(f) = style.fill {
                d.set(b"C".to_vec(), rgb(f));
            }
            border(&mut d);
            rich_text(&mut d);
            // The text is laid out for the page as displayed, like a stamp's label.
            if !matches!(new.shape, Shape::Callout { .. }) {
                let turn = pdfcraft_model::pages(doc).get(new.page).map_or(0, |p| p.rotation(doc));
                if matches!(turn, 90 | 180 | 270) {
                    d.set(b"PCTextRotate".to_vec(), Object::Int(turn));
                }
            }
        }
        _ => {}
    }
    if fill_sign_shape(&new.shape) {
        // Survives a later subject edit, so flatten-on-save still finds the mark.
        d.set(b"PCFillSign".to_vec(), Object::Bool(true));
    }
    // A redaction's overlay text becomes page content when it is applied, where `?` would be there
    // for good: refused before anything is added. A comment's own text is stored as typed and
    // drawn with `?` for what the standard fonts can't draw (marked, see `draw_appearance`).
    if d.name(b"Subtype") == Some(b"Redact")
        && let Some((what, c)) = appearance::undrawable_text(&d)
    {
        return Err(AnnotError::Undrawable(pdfcraft_fonts::undrawable_message(what, c)));
    }
    let r = doc.add(Object::Dict(d.clone()));
    draw_appearance(doc, r)?;
    let mut list = annots(doc, page);
    let index = list.len();
    list.push(Object::Ref(r));
    if let Some(pr) = popup {
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Annot"));
        p.set(b"Subtype".to_vec(), Object::name("Popup"));
        p.set(b"Rect".to_vec(), num_array(&pr));
        p.set(b"Parent".to_vec(), Object::Ref(r));
        p.set(b"Open".to_vec(), Object::Bool(false));
        p.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
        let pref = doc.add(Object::Dict(p));
        doc.update_dict(r, |d| d.set(b"Popup".to_vec(), Object::Ref(pref)))?;
        list.push(Object::Ref(pref));
    }
    set_annots(doc, page, list)?;
    Ok(index)
}

/// Counterrotate a newly placed picture stamp's appearance into displayed-page axes.
/// The picture may be an image or a PDF page. The normal appearance's transformed bounding
/// box is fitted to `/Rect` by PDF viewers.
pub fn orient_image_stamp(doc: &mut Document, page: usize, index: usize, rotation: i64) -> Result<(), AnnotError> {
    let matrix = match rotation {
        90 => [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
        180 => [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0],
        270 => [0.0, -1.0, 1.0, 0.0, 0.0, 0.0],
        _ => return Ok(()),
    };
    let (_, r) = annot_ref(doc, page, index)?;
    let d = annot_dict(doc, r);
    if d.name(b"Subtype") != Some(b"Stamp") || d.get(b"PCPicture").is_none() {
        return Err(AnnotError::Invalid("only a picture stamp can be oriented".into()));
    }
    let normal = d
        .get(b"AP")
        .map(|ap| doc.resolve(ap))
        .and_then(|ap| ap.as_dict().and_then(|ap| ap.reference(b"N")))
        .ok_or_else(|| AnnotError::Invalid("the stamp has no normal appearance".into()))?;
    let Object::Stream(mut stream) = doc.get(normal).as_ref().clone() else {
        return Err(AnnotError::Invalid("the stamp's normal appearance is not a stream".into()));
    };
    stream.dict.set(b"Matrix".to_vec(), num_array(&matrix));
    doc.set(normal, Object::Stream(stream));
    Ok(())
}

/// A comment another program drew, whose text the standard fonts can't draw. Redrawing it would
/// replace those characters with `?`, so the edit is refused before the dictionary changes.
fn refuse_foreign_text(doc: &Document, d: &Dict) -> Result<(), AnnotError> {
    let drawn_elsewhere =
        d.get(b"AP").is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N"))) && pdfcraft_fonts::undrawn_mark(d).is_none();
    if drawn_elsewhere && let Some((what, c)) = appearance::undrawable_text(d) {
        return Err(AnnotError::Undrawable(format!(
            "PdfCraft can't redraw this comment: the standard fonts can't draw \"{}\" (U+{:04X}) in {what}, which would then show as ? (the comment was left as it is)",
            c.escape_debug(),
            u32::from(c)
        )));
    }
    Ok(())
}

/// (Re)generate `/AP /N` for the annotation `r` from its dictionary, for an edit that keeps its
/// text (a restyle, a new size or colour).
///
/// A comment whose text the standard fonts can't draw, with an appearance another program drew
/// (with a font that has those characters), keeps that appearance: redrawing it would put `?` in
/// their place, so the edit is refused (`AnnotError::Undrawable`). One PdfCraft drew with `?`
/// (marked `pdfcraft_fonts::UNDRAWN_MARK`) is redrawn.
pub fn set_appearance(doc: &mut Document, r: ObjRef) -> Result<(), AnnotError> {
    refuse_foreign_text(doc, &annot_dict(doc, r))?;
    draw_appearance(doc, r)
}

/// Put `before` back when redrawing fails, so a refused edit does not keep the new rectangle or style.
fn redraw_or_restore(doc: &mut Document, r: ObjRef, before: Dict) -> Result<(), AnnotError> {
    if let Err(e) = set_appearance(doc, r) {
        doc.set(r, Object::Dict(before));
        return Err(e);
    }
    Ok(())
}

/// Draw `r`'s normal appearance from its dictionary, whatever its text: characters the standard
/// fonts can't draw come out as `?`, and the annotation is marked (`pdfcraft_fonts::UNDRAWN_MARK`,
/// cleared when all of it can be drawn) so the edit is reported and flattening won't make that
/// `?` permanent. For new text (a new comment, changed contents), as Acrobat keeps text its font
/// can't show, and for display-only copies.
fn draw_appearance(doc: &mut Document, r: ObjRef) -> Result<(), AnnotError> {
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    let Some(stream) = build_appearance(doc, &d) else { return Err(AnnotError::Unsupported(subtype)) };
    apply_appearance(doc, r, &d, stream)
}

/// Draw the appearance for an annotation dictionary: the expensive half of [`set_appearance`].
/// Pure from the dictionary and the objects it points at, so [`AppearanceCache`] can reuse it.
fn build_appearance(doc: &Document, d: &Dict) -> Option<Stream> {
    let mut stream = appearance::build(d)?;
    // Picture-stamp restyling must retain the placement's page-axis correction.
    if d.get(b"PCPicture").is_some() {
        let matrix = d.get(b"AP").and_then(|ap| {
            let ap = doc.resolve(ap);
            let normal = doc.resolve(ap.as_dict()?.get(b"N")?);
            let Object::Stream(normal) = normal.as_ref() else { return None };
            normal.dict.get(b"Matrix").cloned()
        });
        if let Some(matrix) = matrix {
            stream.dict.set(b"Matrix".to_vec(), matrix);
        }
    }
    Some(stream)
}

/// Store a built appearance on the annotation: the cheap half of [`set_appearance`].
fn apply_appearance(doc: &mut Document, r: ObjRef, d: &Dict, stream: Stream) -> Result<(), AnnotError> {
    // Characters the standard fonts can't draw came out as `?`: mark the annotation with the first.
    let missing = appearance::undrawable_text(d).map(|(_, c)| c);
    let ap = doc.add(Object::Stream(stream));
    // A copy (a shared /AP is left alone) that keeps unknown entries; the old down and rollover
    // appearances would show the previous look on press or hover, so they go with the old /N.
    let mut apd = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
    apd.remove(b"D");
    apd.remove(b"R");
    apd.set(b"N".to_vec(), Object::Ref(ap));
    // A note icon has a fixed size (`NOTE_SIZE`); viewers draw it at the upper-left corner of `/Rect`.
    // The appearance's box is mapped onto `/Rect`, so any other `/Rect` (a large one, or one with
    // no width, as LaTeX `pdfcomment` writes) would stretch the icon or shrink it to nothing.
    let icon_rect = matches!(d.name(b"Subtype"), Some(b"Text" | b"FileAttachment"))
        .then(|| d.get(b"Rect")?.as_array()?.iter().map(|o| o.as_f64()).collect::<Option<Vec<f64>>>())
        .flatten()
        .filter(|q| q.len() == 4 && finite(q))
        .map(|q| {
            let (x, top) = (q[0].min(q[2]), q[1].max(q[3]));
            [x, top - NOTE_SIZE, x + NOTE_SIZE, top]
        });
    doc.update_dict(r, |d| {
        d.set(b"AP".to_vec(), Object::Dict(apd));
        if let Some(rect) = icon_rect {
            d.set(b"Rect".to_vec(), num_array(&rect));
        }
        d.remove(b"AS");
        pdfcraft_fonts::set_undrawn_mark(d, missing);
    })?;
    Ok(())
}

/// Display appearances built for annotations without any, reused while the annotation
/// dictionaries they were drawn from are unchanged.
///
/// A content edit (moving a paragraph, retyping text, …) leaves every annotation dictionary
/// alone, but the display copy is rebuilt from the edited document on every refresh: without
/// this cache each refresh redrew every appearance (27k on one CAD file, ~0.7 s per edit
/// commit). Entries are keyed by annotation reference; object numbers are never reused within
/// a document lineage, so an entry stays valid until an annotation edit runs. Callers clear
/// the cache on any refresh that may change annotations (comments, structural and full
/// refreshes) and keep it for content-only ones.
#[derive(Clone, Debug, Default)]
pub struct AppearanceCache {
    entries: std::collections::HashMap<ObjRef, Stream>,
    bytes: usize,
    hits: u64,
    builds: u64,
}

impl AppearanceCache {
    /// Cached appearance data restarts past this instead of growing without bound.
    const MAX_BYTES: usize = 64 << 20;

    /// Forget every entry (an annotation edit may have changed the dictionaries).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn get(&mut self, r: ObjRef) -> Option<Stream> {
        let stream = self.entries.get(&r)?.clone();
        self.hits += 1;
        Some(stream)
    }

    fn insert(&mut self, r: ObjRef, stream: &Stream) {
        self.builds += 1;
        if self.entries.len() >= 1_000_000 {
            self.clear();
        }
        if self.bytes.saturating_add(stream.raw.len()) <= Self::MAX_BYTES {
            self.bytes += stream.raw.len();
            self.entries.insert(r, stream.clone());
        }
        // Else over budget: keep serving the entries already cached instead of growing.
    }

    /// Cache hits so far (reused appearances); for tests.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Appearances built so far; for tests.
    pub fn builds(&self) -> u64 {
        self.builds
    }
}

/// A copy of `doc` in which every annotation without a normal appearance (`/AP /N`) has the one
/// [`appearance::build`] draws from its dictionary, for displaying the document (`None` when no
/// annotation needs one). Many files carry comments without appearances (FreeText, Ink, notes,
/// stamps, …) that viewers draw from the dictionary. `doc` is not changed: saving writes the file
/// as it was. Links, form fields and pop-ups keep their own handling.
pub fn with_missing_appearances(doc: &Document) -> Option<Document> {
    with_missing_appearances_cached(doc, &mut AppearanceCache::default())
}

/// A copy of `doc` with [`AppearanceCache`] reuse across refreshes: annotations whose
/// dictionary is unchanged since the cached build keep their stored appearance instead of
/// redrawing it. See [`AppearanceCache`] for when entries stay valid.
pub fn with_missing_appearances_cached(doc: &Document, cache: &mut AppearanceCache) -> Option<Document> {
    let mut copy: Option<Document> = None;
    // An annotation listed more than once, on one page or several, is drawn once. `has_normal`
    // reads `doc`, which never gets the appearance, so every repeat used to add another stream to
    // the copy, and the display bytes grew with the number of listings.
    let mut seen = std::collections::HashSet::new();
    for page in page_refs(doc).ok()? {
        for r in annots(doc, page).iter().filter_map(Object::as_ref) {
            if !seen.insert(r) {
                continue;
            }
            let d = annot_dict(doc, r);
            if matches!(d.name(b"Subtype"), None | Some(b"Link" | b"Widget" | b"Popup")) {
                continue;
            }
            let has_normal = d.get(b"AP").is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N")));
            if has_normal {
                continue;
            }
            if let Some(stream) = cache.get(r) {
                // Fails only when `r` isn't a dictionary, which the cached build ruled out.
                apply_appearance(copy.get_or_insert_with(|| doc.clone()), r, &d, stream).ok();
                continue;
            }
            let Some(stream) = build_appearance(doc, &d) else { continue };
            cache.insert(r, &stream);
            // Fails only when `r` isn't a dictionary, which the build just ruled out.
            apply_appearance(copy.get_or_insert_with(|| doc.clone()), r, &d, stream).ok();
        }
    }
    copy
}

/// Format a number for content streams and DA strings.
pub(crate) fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

// ── edits ───────────────────────────────────────────────────────────────────────────────────

/// A text box's rich text (§12.7.4.3): `/DS` (default style) and `/RC` (XHTML) written from its
/// plain `/Contents`, `/DA` and `/Q`, so viewers that edit rich text show the same thing.
fn rich_text(d: &mut Dict) {
    let text = d.get(b"Contents").and_then(|c| c.as_string()).map(PdfString::to_text).unwrap_or_default();
    let (col, size) = appearance::parse_da(d);
    let hex = format!(
        "#{:02X}{:02X}{:02X}",
        (col[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (col[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (col[2].clamp(0.0, 1.0) * 255.0).round() as u8
    );
    let align = match d.get(b"Q").and_then(Object::as_int) {
        Some(1) => "center",
        Some(2) => "right",
        _ => "left",
    };
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let body: String = text.split('\n').map(|line| format!("<p dir=\"ltr\">{}</p>", esc(line.trim_end_matches('\r')))).collect();
    let style = format!(
        "font-size:{}pt;text-align:{align};color:{hex};font-weight:normal;font-style:normal;font-family:Helvetica;font-stretch:normal",
        n(size)
    );
    let rc = format!(
        "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:spec=\"2.0.2\" style=\"{style}\">{body}</body>"
    );
    d.set(b"DS".to_vec(), PdfString::text(&format!("font: Helvetica {}pt; text-align:{align}; color:{hex}", n(size))));
    d.set(b"RC".to_vec(), PdfString::text(&rc));
}

fn touch(d: &mut Dict, meta: &Meta) {
    if let Some(date) = &meta.date {
        d.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    }
}

/// Delete a comment together with its pop-up and its replies (and theirs), as Acrobat does.
pub fn delete_annotation(doc: &mut Document, page: usize, index: usize) -> Result<(), AnnotError> {
    let p = page_ref(doc, page)?;
    let list = annots(doc, p);
    let target = list.get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    if let Some(r) = target.as_ref() {
        unlocked(doc, r)?;
    }
    let mut doomed: Vec<ObjRef> = target.as_ref().into_iter().collect();
    // Replies (`/IRT`) and pop-ups (`/Parent`) of anything doomed, to a fixpoint.
    loop {
        let before = doomed.len();
        for e in &list {
            let Some(r) = e.as_ref() else { continue };
            if doomed.contains(&r) {
                continue;
            }
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            let points_at = |k: &[u8]| d.reference(k).is_some_and(|t| doomed.contains(&t));
            if points_at(b"IRT") || points_at(b"Parent") {
                doomed.push(r);
            }
        }
        for r in doomed.clone() {
            if let Some(pop) = doc.get(r).as_dict().and_then(|d| d.reference(b"Popup"))
                && !doomed.contains(&pop)
            {
                doomed.push(pop);
            }
        }
        if doomed.len() == before {
            break;
        }
    }
    let kept: Vec<Object> =
        list.into_iter().enumerate().filter(|(i, e)| *i != index && !e.as_ref().is_some_and(|r| doomed.contains(&r))).map(|(_, e)| e).collect();
    set_annots(doc, p, kept)
}

/// Change a comment's text. Text boxes are redrawn to show it, and their rectangle follows the
/// new text: the wrap width and top edge stay, the height fits the wrapped lines.
pub fn set_contents(doc: &mut Document, page: usize, index: usize, text: &str, meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    let free_text = annot_dict(doc, r).name(b"Subtype") == Some(b"FreeText");
    doc.update_dict(r, |d| {
        d.set(b"Contents".to_vec(), PdfString::text(text));
        // The rich-text version follows the new plain text (text boxes), or would contradict it.
        if free_text {
            rich_text(d);
        } else {
            d.remove(b"RC");
        }
        touch(d, meta);
    })?;
    if free_text {
        // The box grows and shrinks with the text instead of clipping it. A locked box keeps its
        // text editable (as in Acrobat), so the re-fit ignores the lock. New text is drawn even
        // where the standard fonts can't draw some of it (marked and reported, see
        // `draw_appearance`).
        match fitted_box(doc, r, text) {
            Some(rect) => apply_text_box(doc, r, rect, meta, true)?,
            None => draw_appearance(doc, r)?,
        }
    }
    Ok(())
}

/// The rectangle a FreeText annotation's text box needs for `text`: the current wrap width and
/// top edge stay and the height fits the wrapped lines, as at creation. `None` keeps the old
/// rectangle (the box can't be measured, so only the appearance is redrawn).
fn fitted_box(doc: &Document, r: ObjRef, text: &str) -> Option<[f64; 4]> {
    let d = annot_dict(doc, r);
    let nums = |key: &[u8]| -> Option<Vec<f64>> { d.get(key)?.as_array()?.iter().map(|o| o.as_f64()).collect() };
    let (_, size) = appearance::parse_da(&d);
    let pad = 2.0 + border_width_of(&d);
    let rect = nums(b"Rect").filter(|v| v.len() == 4 && v.iter().all(|x| x.is_finite()))?;
    let rect = [rect[0], rect[1], rect[2], rect[3]];
    // A callout re-fits its text box (`/Rect` inset by `/RD`); the leader line keeps its place.
    let tb = if d.contains(b"CL") {
        let rd = nums(b"RD").filter(|v| v.len() == 4 && v.iter().all(|x| *x >= 0.0))?;
        [rect[0] + rd[0], rect[1] + rd[1], rect[2] - rd[2], rect[3] - rd[3]]
    } else {
        rect
    };
    let (x0, top) = (tb[0].min(tb[2]), tb[1].max(tb[3]));
    let w = tb[2] - tb[0];
    let w = if w.is_finite() && w >= 1.0 {
        w
    } else {
        // No usable old width: fall back to the creation-time width (at most 300 pt).
        let longest = text.lines().map(|l| appearance::text_width(l, size)).fold(0.0, f64::max);
        (longest + 2.0 * pad + 4.0).clamp(40.0, 300.0)
    };
    let lines = appearance::wrap(text, size, (w - 2.0 * pad).max(1.0)).len().max(1) as f64;
    Some([x0, top - lines * size * 1.2 - 2.0 * pad - 2.0, x0 + w, top])
}

/// Reply to a comment; returns the reply's index in the page's `/Annots`.
///
/// A reply is a text annotation with `/IRT` pointing at its parent (§12.5.6.2). It gets the
/// parent's rectangle and an empty appearance, so it shows in comment lists but never paints a
/// second icon on the page.
pub fn add_reply(doc: &mut Document, page: usize, index: usize, text: &str, author: &str, meta: &Meta) -> Result<usize, AnnotError> {
    reply(doc, page, index, text, author, meta, None)
}

/// Set a comment's review status (Acrobat: "Set status ▸ Accepted"…). Like Acrobat this adds a
/// state reply (`/State`, `/StateModel /Review`) by `author`; the latest one wins.
pub fn set_review_state(doc: &mut Document, page: usize, index: usize, state: ReviewState, author: &str, meta: &Meta) -> Result<usize, AnnotError> {
    let text = format!("{} set by {}", state.name(), if author.is_empty() { "unknown" } else { author });
    reply(doc, page, index, &text, author, meta, Some(state))
}

fn reply(
    doc: &mut Document,
    page: usize,
    index: usize,
    text: &str,
    author: &str,
    meta: &Meta,
    state: Option<ReviewState>,
) -> Result<usize, AnnotError> {
    let (p, parent) = annot_ref(doc, page, index)?;
    let pd = annot_dict(doc, parent);
    if pd.name(b"Subtype") == Some(b"Popup") {
        return Err(AnnotError::Invalid("pop-ups can't be replied to".into()));
    }
    let rect = pd.get(b"Rect").cloned().unwrap_or_else(|| num_array(&[0.0, 0.0, 0.0, 0.0]));
    let mut d = base_dict("Text", [0.0; 4], p, text, author, meta);
    d.set(b"Rect".to_vec(), rect);
    d.set(b"IRT".to_vec(), Object::Ref(parent));
    d.set(b"F".to_vec(), Object::Int(FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
    d.set(b"Name".to_vec(), Object::name("Comment"));
    if let Some(c) = pd.get(b"C") {
        d.set(b"C".to_vec(), c.clone());
    }
    if let Some(s) = state {
        d.set(b"State".to_vec(), PdfString::text(s.name()));
        d.set(b"StateModel".to_vec(), PdfString::text("Review"));
        d.set(b"Subj".to_vec(), PdfString::text("Status"));
    }
    // An empty form: nothing is drawn for the reply itself.
    let mut fd = Dict::new();
    fd.set(b"Type".to_vec(), Object::name("XObject"));
    fd.set(b"Subtype".to_vec(), Object::name("Form"));
    fd.set(b"BBox".to_vec(), num_array(&[0.0, 0.0, 0.0, 0.0]));
    let ap = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(fd, Vec::new())));
    let mut apd = Dict::new();
    apd.set(b"N".to_vec(), Object::Ref(ap));
    d.set(b"AP".to_vec(), Object::Dict(apd));
    let r = doc.add(Object::Dict(d));
    let mut list = annots(doc, p);
    list.push(Object::Ref(r));
    let i = list.len() - 1;
    set_annots(doc, p, list)?;
    Ok(i)
}

fn flags(doc: &Document, r: ObjRef) -> i64 {
    doc.get(r).as_dict().map(|d| annotation_flags(doc, d)).unwrap_or(0)
}

/// Refuse to change a locked comment (Acrobat: Properties ▸ Locked).
fn unlocked(doc: &Document, r: ObjRef) -> Result<(), AnnotError> {
    if flags(doc, r) & FLAG_LOCKED != 0 { Err(AnnotError::Invalid("the comment is locked".into())) } else { Ok(()) }
}

/// Lock or unlock a comment (the Locked flag). A locked comment can't be moved, resized,
/// restyled or deleted; its text and replies stay editable, as in Acrobat.
pub fn set_locked(doc: &mut Document, page: usize, index: usize, locked: bool) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    let f = if locked { flags(doc, r) | FLAG_LOCKED } else { flags(doc, r) & !FLAG_LOCKED };
    doc.update_dict(r, |d| d.set(b"F".to_vec(), Object::Int(f)))?;
    Ok(())
}

/// Mark or unmark a comment with a checkmark. Like Acrobat this adds a hidden state reply with
/// `/StateModel /Marked` by `author`; the latest one wins. It's private bookkeeping, not a status.
pub fn set_marked(doc: &mut Document, page: usize, index: usize, marked: bool, author: &str, meta: &Meta) -> Result<usize, AnnotError> {
    let (p, parent) = annot_ref(doc, page, index)?;
    let pd = annot_dict(doc, parent);
    if pd.name(b"Subtype") == Some(b"Popup") {
        return Err(AnnotError::Invalid("pop-ups can't be marked".into()));
    }
    let state = if marked { "Marked" } else { "Unmarked" };
    let text = format!("{state} set by {}", if author.is_empty() { "unknown" } else { author });
    let mut d = base_dict("Text", [0.0; 4], p, &text, author, meta);
    d.set(b"Rect".to_vec(), pd.get(b"Rect").cloned().unwrap_or_else(|| num_array(&[0.0; 4])));
    d.set(b"IRT".to_vec(), Object::Ref(parent));
    d.set(b"F".to_vec(), Object::Int(FLAG_HIDDEN | FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE));
    d.set(b"State".to_vec(), PdfString::text(state));
    d.set(b"StateModel".to_vec(), PdfString::text("Marked"));
    let r = doc.add(Object::Dict(d));
    let mut list = annots(doc, p);
    list.push(Object::Ref(r));
    let i = list.len() - 1;
    set_annots(doc, p, list)?;
    Ok(i)
}

/// Move a comment (and its pop-up) by `(dx, dy)` points. The appearance moves with `/Rect`.
pub fn move_annotation(doc: &mut Document, page: usize, index: usize, dx: f64, dy: f64, meta: &Meta) -> Result<(), AnnotError> {
    if !finite(&[dx, dy]) {
        return Err(AnnotError::Invalid("invalid offset".into()));
    }
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let shift = |o: &Object, every: bool| -> Option<Object> {
        let a = o.as_array()?;
        Some(Object::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| match v.as_f64() {
                    Some(x) if every || i < 4 => Object::Real(x + if i % 2 == 0 { dx } else { dy }),
                    _ => v.clone(),
                })
                .collect(),
        ))
    };
    let popup = annot_dict(doc, r).reference(b"Popup");
    doc.update_dict(r, |d| {
        for (k, every) in [(&b"Rect"[..], false), (b"QuadPoints", true), (b"L", false), (b"CL", true), (b"Vertices", true)] {
            if let Some(v) = d.get(k).and_then(|o| shift(o, every)) {
                d.set(k.to_vec(), v);
            }
        }
        if let Some(Object::Array(list)) = d.get(b"InkList").cloned() {
            let moved = list.iter().map(|s| shift(s, true).unwrap_or_else(|| s.clone())).collect();
            d.set(b"InkList".to_vec(), Object::Array(moved));
        }
        touch(d, meta);
    })?;
    if let Some(p) = popup
        && doc.get(p).as_dict().is_some()
    {
        doc.update_dict(p, |d| {
            if let Some(v) = d.get(b"Rect").and_then(|o| shift(o, false)) {
                d.set(b"Rect".to_vec(), v);
            }
        })?;
    }
    Ok(())
}

/// Resize a rectangle, oval, text box, stamp or drawing to `rect`. Stamps keep their appearance,
/// which PDF viewers scale from its bounding box into the new rectangle; drawings scale their
/// strokes and get a new one.
pub fn set_rect(doc: &mut Document, page: usize, index: usize, rect: [f64; 4], meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    if subtype == "Stamp" {
        let rect = normalize(rect);
        if !finite(&rect) || rect[2] <= rect[0] || rect[3] <= rect[1] {
            return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
        }
        doc.update_dict(r, |d| {
            d.set(b"Rect".to_vec(), num_array(&rect));
            touch(d, meta);
        })?;
        return Ok(());
    }
    if subtype == "Ink" {
        return resize_ink(doc, r, normalize(rect), meta);
    }
    if !matches!(subtype.as_str(), "Square" | "Circle" | "FreeText") {
        return Err(AnnotError::Invalid(format!("{subtype} comments can't be resized")));
    }
    apply_text_box(doc, r, rect, meta, false)
}

/// At most this many points reshape a comment (a drawn polygon has a handful).
const MAX_POINTS: usize = 100_000;

/// Reshape a line, connected lines, a polygon or a cloud: its points in user space (a line's two
/// ends, the others' corners), as dragging the comment's handles sets them. `/Rect` follows the
/// points with room for the line's width and endings, as when the comment was drawn, and the
/// appearance is redrawn. A measurement keeps the points it measured.
pub fn set_points(doc: &mut Document, page: usize, index: usize, points: &[[f64; 2]], meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if d.contains(b"Measure") {
        return Err(AnnotError::Invalid("a measurement's points are where it measured".into()));
    }
    if points.len() > MAX_POINTS {
        return Err(AnnotError::Invalid(format!("at most {MAX_POINTS} points")));
    }
    let style = Style { width: border_width_of(&d), ..Style::default() };
    let ends = line_endings_of(&d).unwrap_or_default();
    let (start, end) = (ends.first().copied().unwrap_or(LineEnding::None), ends.get(1).copied().unwrap_or(LineEnding::None));
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    let (shape, key): (Shape, &[u8]) = match (subtype.as_str(), points) {
        ("Line", [from, to]) => (Shape::Line { from: *from, to: *to, start, end }, b"L"),
        ("Line", _) => return Err(AnnotError::Invalid("a line has two ends".into())),
        ("PolyLine", _) => (Shape::PolyLine { vertices: points.to_vec(), start, end }, b"Vertices"),
        ("Polygon", _) => {
            let cloud = d.get(b"BE").and_then(|b| b.as_dict()).is_some_and(|b| b.name(b"S") == Some(b"C"));
            (Shape::Polygon { vertices: points.to_vec(), cloud }, b"Vertices")
        }
        (other, _) => return Err(AnnotError::Invalid(format!("{other} comments have no points to move"))),
    };
    // Checks the points too: finite, and far enough apart to draw.
    let rect = rect_for(&shape, &style)?;
    let before = d;
    doc.update_dict(r, |d| {
        d.set(key.to_vec(), num_array(&points.concat()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Scale a drawing's strokes into `rect`. The margin between the strokes and the old `/Rect` (half
/// the line width and a little more) stays the same on each side, so the line isn't clipped.
fn resize_ink(doc: &mut Document, r: ObjRef, rect: [f64; 4], meta: &Meta) -> Result<(), AnnotError> {
    let d = annot_dict(doc, r);
    let strokes: Vec<Vec<[f64; 2]>> = d
        .get(b"InkList")
        .map(|l| doc.resolve(l))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let v: Vec<f64> = doc.resolve(s).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()).unwrap_or_default();
            v.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect()
        })
        .collect();
    let Some(inner) = bounds(strokes.iter().flatten().copied()) else {
        return Err(AnnotError::Invalid("the drawing has no strokes".into()));
    };
    let old = d.get(b"Rect").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    let old: Vec<f64> = old.iter().filter_map(|v| doc.resolve(v).as_f64()).collect();
    let old = match old[..] {
        [a, b, c, e] => normalize([a, b, c, e]),
        _ => inner,
    };
    let margin = [inner[0] - old[0], inner[1] - old[1], old[2] - inner[2], old[3] - inner[3]].map(|m| m.max(0.0));
    let target = [rect[0] + margin[0], rect[1] + margin[1], rect[2] - margin[2], rect[3] - margin[3]];
    if !finite(&rect) || target[2] < target[0] || target[3] < target[1] || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
    }
    // An axis with no extent (a straight stroke) is centred rather than scaled.
    let axis = |v: f64, a0: f64, a1: f64, b0: f64, b1: f64| if a1 > a0 { b0 + (v - a0) / (a1 - a0) * (b1 - b0) } else { (b0 + b1) / 2.0 };
    let scaled: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .map(|s| {
            s.iter().map(|p| [axis(p[0], inner[0], inner[2], target[0], target[2]), axis(p[1], inner[1], inner[3], target[1], target[3])]).collect()
        })
        .collect();
    let before = annot_dict(doc, r);
    doc.update_dict(r, |d| {
        d.set(b"InkList".to_vec(), Object::Array(scaled.iter().map(|s| num_array(&s.concat())).collect()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Store `rect` as a FreeText annotation's text box (or a square's/oval's rectangle): a callout's
/// leader line re-attaches and `/Rect` grows to hold it, and the appearance is redrawn
/// (`new_text`: for changed contents, see [`set_appearance`] and `draw_appearance`).
fn apply_text_box(doc: &mut Document, r: ObjRef, rect: [f64; 4], meta: &Meta, new_text: bool) -> Result<(), AnnotError> {
    let rect = normalize(rect);
    if !finite(&rect) || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
    }
    let d = annot_dict(doc, r);
    if !new_text {
        refuse_foreign_text(doc, &d)?;
    }
    // A callout's rectangle is its text box: the leader line re-attaches and `/Rect` grows to hold it.
    let callout = d.get(b"CL").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect::<Vec<f64>>()).filter(|l| l.len() == 6);
    let (outer, cl) = match callout {
        Some(mut l) => {
            let attach = callout_attach(rect, [l[2], l[3]]);
            (l[4], l[5]) = (attach[0], attach[1]);
            let pad = border_width_of(&d) / 2.0 + appearance::arrow_size(border_width_of(&d)) + 1.0;
            let lead = grow(bounds([[l[0], l[1]], [l[2], l[3]]].into_iter()).unwrap_or_default(), pad);
            ([rect[0].min(lead[0]), rect[1].min(lead[1]), rect[2].max(lead[2]), rect[3].max(lead[3])], Some(l))
        }
        None => (rect, None),
    };
    doc.update_dict(r, |d| {
        d.set(b"Rect".to_vec(), num_array(&outer));
        match &cl {
            Some(l) => {
                d.set(b"CL".to_vec(), num_array(l));
                d.set(b"RD".to_vec(), num_array(&[rect[0] - outer[0], rect[1] - outer[1], outer[2] - rect[2], outer[3] - rect[3]]));
            }
            None => {
                d.remove(b"RD");
            }
        }
        touch(d, meta);
    })?;
    let before = d;
    if new_text {
        if let Err(e) = draw_appearance(doc, r) {
            doc.set(r, Object::Dict(before));
            return Err(e);
        }
        Ok(())
    } else {
        redraw_or_restore(doc, r, before)
    }
}

fn border_width_of(d: &Dict) -> f64 {
    d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()).unwrap_or(1.0).max(0.0)
}

/// Change a comment's colour, opacity, line width and/or line endings, and redraw it.
#[allow(clippy::too_many_arguments)]
pub fn set_style(
    doc: &mut Document,
    page: usize,
    index: usize,
    color: Option<Rgb>,
    opacity: Option<f64>,
    width: Option<f64>,
    endings: Option<&[LineEnding]>,
    meta: &Meta,
) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    // Check before changing anything: a stale appearance would contradict the new style.
    if appearance::build(&d).is_none() {
        return Err(AnnotError::Unsupported(subtype.clone()));
    }
    if color.is_some_and(|c| !finite(&c)) || opacity.is_some_and(|o| !o.is_finite()) || width.is_some_and(|w| !w.is_finite()) {
        return Err(AnnotError::Invalid("invalid style".into()));
    }
    if let Some(ends) = endings {
        let callout = subtype == "FreeText" && d.contains(b"CL");
        let pair = matches!(subtype.as_str(), "Line" | "PolyLine");
        if (callout && ends.len() != 1) || (pair && ends.len() != 2) || (!callout && !pair) {
            return Err(AnnotError::Invalid("this comment has no line endings to change".into()));
        }
    }
    refuse_foreign_text(doc, &d)?;
    let free_text = subtype == "FreeText";
    let before = d;
    doc.update_dict(r, |d| {
        if let Some(c) = color {
            if free_text {
                let (_, size) = appearance::parse_da(d);
                let [r, g, b] = c.map(|x| x.clamp(0.0, 1.0));
                d.set(b"DA".to_vec(), PdfString::literal(format!("{} {} {} rg /Helv {} Tf", n(r), n(g), n(b), n(size)).into_bytes()));
                rich_text(d);
            } else {
                d.set(b"C".to_vec(), rgb(c));
            }
        }
        if let Some(o) = opacity {
            let o = o.clamp(0.0, 1.0);
            if o < 1.0 {
                d.set(b"CA".to_vec(), Object::Real(o));
            } else {
                d.remove(b"CA");
            }
        }
        if let Some(w) = width {
            let mut bs = d.get(b"BS").and_then(|b| b.as_dict()).cloned().unwrap_or_default();
            bs.set(b"W".to_vec(), Object::Real(w.max(0.0)));
            d.set(b"BS".to_vec(), Object::Dict(bs));
            d.remove(b"Border");
        }
        if let Some(ends) = endings {
            if ends.len() == 1 {
                if let Some(ending) = ends.first() {
                    d.set(b"LE".to_vec(), Object::name(ending.name()));
                }
            } else if let (Some(start), Some(end)) = (ends.first(), ends.get(1)) {
                d.set(b"LE".to_vec(), ending_pair(*start, *end));
            }
        }
        ensure_ending_room(d);
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Change the font size of a text box, typewriter text or callout (`/DA` keeps its colour; the
/// rich-text strings follow) and redraw it. The box grows to hold the wrapped text at the new
/// size and never shrinks, so a box the user drew large stays large. Sizes are 1 to 400 pt.
pub fn set_font_size(doc: &mut Document, page: usize, index: usize, size: f64, meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if d.name(b"Subtype") != Some(b"FreeText") {
        return Err(AnnotError::Invalid("only text boxes, typewriter text and callouts have a font size".into()));
    }
    if !size.is_finite() || !(1.0..=400.0).contains(&size) {
        return Err(AnnotError::Invalid("font size must be 1\u{2013}400 pt".into()));
    }
    refuse_foreign_text(doc, &d)?;
    let before = d;
    doc.update_dict(r, |d| {
        let (col, _) = appearance::parse_da(d);
        d.set(b"DA".to_vec(), PdfString::literal(format!("{} {} {} rg /Helv {} Tf", n(col[0]), n(col[1]), n(col[2]), n(size)).into_bytes()));
        rich_text(d);
        touch(d, meta);
    })?;
    let text = text_value(doc, &annot_dict(doc, r), b"Contents").unwrap_or_default();
    let result = match fitted_box(doc, r, &text) {
        Some(fit) => {
            let cur = annot_dict(doc, r);
            let nums: Vec<f64> =
                cur.get(b"Rect").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
            // Keep the top and sides; only the bottom edge moves, and only downwards.
            match four(&nums).map(normalize) {
                Some(old) if !cur.contains(b"CL") => apply_text_box(doc, r, [fit[0], fit[1].min(old[1]), fit[2], fit[3]], meta, false),
                _ => apply_text_box(doc, r, fit, meta, false),
            }
        }
        None => redraw_or_restore(doc, r, before.clone()),
    };
    if result.is_err() {
        doc.set(r, Object::Dict(before));
    }
    result
}

/// Whether a comment has a fill: `/IC` for shapes, `/C` for FreeText (§12.5.6).
pub fn fillable(subtype: &str) -> bool {
    matches!(subtype, "Square" | "Circle" | "Polygon" | "FreeText")
}

/// Fill a shape or FreeText comment with `fill`, or remove its fill (`None`), and
/// redraw its appearance (Acrobat: Properties ▸ Fill Color).
pub fn set_fill(doc: &mut Document, page: usize, index: usize, fill: Option<Rgb>, meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
    if !fillable(&subtype) {
        return Err(AnnotError::Invalid("only rectangles, ovals, polygons and text boxes can be filled".into()));
    }
    // Check before changing anything: a stale appearance would contradict the new fill.
    if appearance::build(&d).is_none() {
        return Err(AnnotError::Unsupported(subtype));
    }
    if fill.is_some_and(|c| !finite(&c)) {
        return Err(AnnotError::Invalid("invalid fill colour".into()));
    }
    refuse_foreign_text(doc, &d)?;
    let before = d;
    doc.update_dict(r, |d| {
        let key: &[u8] = if subtype == "FreeText" { b"C" } else { b"IC" };
        match fill {
            Some(c) => d.set(key.to_vec(), rgb(c)),
            None => {
                d.remove(key);
            }
        }
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}

/// Grow a line or polyline's rectangle so a non-`None` ending is not clipped. Idempotent.
fn ensure_ending_room(d: &mut Dict) {
    let subtype = d.name(b"Subtype").unwrap_or_default();
    if !matches!(subtype, b"Line" | b"PolyLine") {
        return;
    }
    if !line_endings_of(d).is_some_and(|ends| ends.iter().any(|e| ending_draws(*e))) {
        return;
    }
    let key: &[u8] = if subtype == b"Line" { b"L" } else { b"Vertices" };
    let Some(pts) = pair_points(d, key) else { return };
    let Some(bounds) = bounds(pts.iter().copied()) else { return };
    let w = border_width_of(d);
    let need = grow(bounds, w / 2.0 + appearance::arrow_size(w) + 1.0);
    let vals: Vec<f64> = d.get(b"Rect").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
    let Some(rect) = four(&vals) else { return };
    let union = [rect[0].min(need[0]), rect[1].min(need[1]), rect[2].max(need[2]), rect[3].max(need[3])];
    if union != rect {
        d.set(b"Rect".to_vec(), num_array(&union));
    }
}

fn four(v: &[f64]) -> Option<[f64; 4]> {
    Some([*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?])
}

fn pair_points(d: &Dict, key: &[u8]) -> Option<Vec<[f64; 2]>> {
    let v: Vec<f64> = d.get(key)?.as_array()?.iter().filter_map(|o| o.as_f64()).collect();
    if v.len() < 4 || !v.len().is_multiple_of(2) || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(v.as_chunks::<2>().0.to_vec())
}

// ── reading ─────────────────────────────────────────────────────────────────────────────────

/// One comment as the viewer lists it (the same fields and rules as
/// `pdfcraft_render::Annotation`), read straight from the object graph. The engine uses it to
/// refresh the comment list after a comment edit without re-inspecting the whole document.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub page: usize,
    pub index: usize,
    pub subtype: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    /// `/M` as written (a PDF date string).
    pub modified: Option<String>,
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    pub rect: [f32; 4],
    pub color: Option<[f32; 3]>,
    pub state: Option<String>,
    pub quads: Vec<[f32; 8]>,
    pub locked: bool,
    pub intent: Option<String>,
    /// A Fill & Sign mark ([`is_fill_sign`]).
    pub fill_sign: bool,
    /// A line's two ends (`/L`), or the corners of connected lines, a polygon or a cloud
    /// (`/Vertices`), in user space; empty for other comments.
    pub points: Vec<[f32; 2]>,
}

fn text_value(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    let o = doc.resolve(d.get(key)?);
    let s = match &*o {
        Object::String(s) => s.to_text(),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => return None,
    };
    let s = s.trim_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Whether an annotation subtype is a comment. Links, form widgets, pop-ups and the non-markup
/// multimedia and print-production annotations (ISO 32000-2 §12.5.6: Screen, Movie, RichMedia,
/// 3D, PrinterMark, TrapNet, Watermark) are not.
pub fn is_comment_subtype(subtype: &str) -> bool {
    !matches!(subtype, "Link" | "Widget" | "Popup" | "Screen" | "Movie" | "RichMedia" | "3D" | "PrinterMark" | "TrapNet" | "Watermark")
}

/// Every comment (see [`is_comment_subtype`]), ordered by page and then top edge.
pub fn summaries(doc: &Document) -> Vec<Summary> {
    let mut out = Vec::new();
    let Ok(pages) = page_refs(doc) else { return out };
    for (page, p) in pages.iter().enumerate() {
        for (index, entry) in annots(doc, *p).iter().enumerate() {
            let obj = doc.resolve(entry);
            let Some(d) = obj.as_dict() else { continue };
            let Some(subtype) = d.name(b"Subtype").map(|s| String::from_utf8_lossy(s).into_owned()) else { continue };
            if !is_comment_subtype(&subtype) {
                continue;
            }
            let nums = |k: &[u8]| -> Vec<f32> {
                d.get(k)
                    .map(|o| doc.resolve(o))
                    .and_then(|o| o.as_array().map(|a| a.iter().map(|x| doc.resolve(x).as_f64().unwrap_or(0.0) as f32).collect()))
                    .unwrap_or_default()
            };
            let r = nums(b"Rect");
            let rect = if r.len() == 4 { [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])] } else { [0.0; 4] };
            let c = nums(b"C");
            let mut color = (c.len() == 3).then(|| [c[0], c[1], c[2]]);
            if subtype == "FreeText"
                && let Some(da) = text_value(doc, d, b"DA")
            {
                let t: Vec<&str> = da.split_whitespace().collect();
                if let Some(i) = t.iter().position(|x| *x == "rg")
                    && i >= 3
                {
                    let f = |k: usize| t[k].parse::<f32>().unwrap_or(0.0);
                    color = Some([f(i - 3), f(i - 2), f(i - 1)]);
                }
            }
            let quads = nums(b"QuadPoints").as_chunks::<8>().0.to_vec();
            // Keep in step with `pdfcraft_render::inspect`.
            let points = match subtype.as_str() {
                "Line" => Some(nums(b"L")).filter(|l| l.len() == 4).map(|l| l.as_chunks::<2>().0.to_vec()).unwrap_or_default(),
                "PolyLine" | "Polygon" => nums(b"Vertices").as_chunks::<2>().0.to_vec(),
                _ => Vec::new(),
            };
            let in_reply_to = d.get(b"IRT").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).and_then(|p| text_value(doc, &p, b"NM"));
            out.push(Summary {
                page,
                index,
                subtype,
                author: text_value(doc, d, b"T"),
                // Acrobat ends lines with CR, which the interface would draw as nothing (keep in
                // step with `pdfcraft_render::inspect`).
                contents: text_value(doc, d, b"Contents").map(|s| s.replace("\r\n", "\n").replace('\r', "\n")),
                modified: text_value(doc, d, b"M"),
                name: text_value(doc, d, b"NM"),
                in_reply_to,
                rect,
                color,
                state: text_value(doc, d, b"State"),
                quads,
                locked: annotation_flags(doc, d) & FLAG_LOCKED != 0,
                intent: d.get(b"IT").and_then(|o| doc.resolve(o).as_name().map(|n| String::from_utf8_lossy(n).into_owned())),
                fill_sign: is_fill_sign(doc, d),
                points,
            });
        }
    }
    out.sort_by(|a, b| a.page.cmp(&b.page).then(b.rect[3].total_cmp(&a.rect[3])));
    out
}

/// Change a comment's author (`/T`), subject (`/Subj`) and, for notes, icon (`/Name`, redrawn).
/// `None` leaves a value as it is.
pub fn set_info(
    doc: &mut Document,
    page: usize,
    index: usize,
    author: Option<&str>,
    subject: Option<&str>,
    icon: Option<NoteIcon>,
    meta: &Meta,
) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let is_note = annot_dict(doc, r).name(b"Subtype") == Some(b"Text");
    if icon.is_some() && !is_note {
        return Err(AnnotError::Invalid("only sticky notes have an icon".into()));
    }
    doc.update_dict(r, |d| {
        if let Some(a) = author {
            d.set(b"T".to_vec(), PdfString::text(a));
        }
        if let Some(s) = subject {
            d.set(b"Subj".to_vec(), PdfString::text(s));
        }
        if let Some(i) = icon {
            d.set(b"Name".to_vec(), Object::name(i.name()));
        }
        touch(d, meta);
    })?;
    if icon.is_some() {
        set_appearance(doc, r)?;
    }
    Ok(())
}

/// What Comment properties shows for one comment.
#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub subtype: String,
    pub author: String,
    pub subject: String,
    pub color: Option<Rgb>,
    pub opacity: f64,
    /// Border / line width, for shapes, lines, drawings and text boxes.
    pub width: Option<f64>,
    /// Notes only.
    pub icon: Option<NoteIcon>,
    /// `/M`, as written.
    pub modified: Option<String>,
    /// The appearance can be redrawn (so colour, opacity and width can change).
    pub restylable: bool,
    /// The Locked flag.
    pub locked: bool,
    /// `/LE`: two names for a line or polyline (`None` when unset), one for a callout.
    pub endings: Option<Vec<LineEnding>>,
    /// Whether the comment has a fill control (see [`fillable`]).
    pub fillable: bool,
    /// The interior colour (`None`: no fill).
    pub fill: Option<Rgb>,
    /// Text boxes, typewriter text and callouts: the `/DA` font size in points.
    pub font_size: Option<f64>,
}

/// The current properties of the comment at `(page, index)`.
pub fn props(doc: &Document, page: usize, index: usize) -> Option<Props> {
    let p = page_ref(doc, page).ok()?;
    let entry = annots(doc, p).get(index).cloned()?;
    let obj = doc.resolve(&entry);
    let d = obj.as_dict()?;
    let subtype = String::from_utf8_lossy(d.name(b"Subtype")?).into_owned();
    let c: Vec<f64> = d.get(b"C").and_then(|o| o.as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect())).unwrap_or_default();
    let color = if subtype == "FreeText" { Some(appearance::parse_da(d).0) } else { (c.len() == 3).then(|| [c[0], c[1], c[2]]) };
    let width = matches!(subtype.as_str(), "Square" | "Circle" | "Line" | "Ink" | "Polygon" | "PolyLine" | "FreeText").then(|| {
        if subtype == "FreeText" {
            if !d.contains(b"BS") && !d.contains(b"Border") { 0.0 } else { appearance::border_width(d) }
        } else {
            d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()).unwrap_or(1.0)
        }
    });
    Some(Props {
        author: text_value(doc, d, b"T").unwrap_or_default(),
        subject: text_value(doc, d, b"Subj").unwrap_or_default(),
        color,
        opacity: d.get(b"CA").and_then(|o| o.as_f64()).unwrap_or(1.0),
        width,
        icon: (subtype == "Text").then(|| d.name(b"Name").and_then(|n| NoteIcon::from_name(&String::from_utf8_lossy(n))).unwrap_or(NoteIcon::Note)),
        modified: text_value(doc, d, b"M"),
        restylable: appearance::build(d).is_some(),
        locked: annotation_flags(doc, d) & FLAG_LOCKED != 0,
        fillable: fillable(&subtype),
        fill: fill_of(d),
        font_size: (subtype == "FreeText").then(|| appearance::parse_da(d).1),
        subtype,
        endings: line_endings_of(d),
    })
}

/// A shape’s `/IC` or FreeText `/C` as RGB. Grey and CMYK fills are converted.
fn fill_of(d: &Dict) -> Option<Rgb> {
    let key: &[u8] = if d.name(b"Subtype") == Some(b"FreeText") { b"C" } else { b"IC" };
    let c: Vec<f64> = d.get(key)?.as_array()?.iter().filter_map(|x| x.as_f64()).collect();
    let rgb = match c.as_slice() {
        [g] => [*g; 3],
        [r, g, b] => [*r, *g, *b],
        [c, m, y, k] => [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)],
        _ => return None,
    };
    finite(&rgb).then(|| rgb.map(|x| x.clamp(0.0, 1.0)))
}

/// `/LE` for a line, polyline or callout. Unknown names yield `None` so the control stays hidden.
fn line_endings_of(d: &Dict) -> Option<Vec<LineEnding>> {
    let parse = |name: &[u8]| std::str::from_utf8(name).ok().and_then(LineEnding::parse);
    match d.name(b"Subtype")? {
        b"Line" | b"PolyLine" => match d.get(b"LE") {
            None => Some(vec![LineEnding::None, LineEnding::None]),
            Some(o) => o.as_array().filter(|a| a.len() == 2)?.iter().map(|e| parse(e.as_name()?)).collect(),
        },
        b"FreeText" if d.contains(b"CL") => Some(vec![parse(d.name(b"LE").unwrap_or(b"None"))?]),
        _ => None,
    }
}

/// Replace Text (Acrobat's proposal): strike out `quads` and add a caret at the end of the
/// struck text holding `replacement`, grouped with the strikeout (`/IRT` + `/RT /Group`) so they
/// move, list and delete as one. Returns the strikeout's index.
///
/// The two annotations need their own `/NM`: XFDF names a reply by the `/NM` of what it replies
/// to, so one id for both writes `inreplyto` equal to the caret's own `name`, and a reader that
/// resolves names (Acrobat, and our own importer) loses the pair. `caret_meta` is a second id
/// from the caller, so the edit stays deterministic.
#[allow(clippy::too_many_arguments)]
pub fn add_text_replacement(
    doc: &mut Document,
    page: usize,
    quads: &[[f64; 8]],
    replacement: &str,
    author: &str,
    strike: &Style,
    caret: &Style,
    meta: &Meta,
    caret_meta: &Meta,
) -> Result<usize, AnnotError> {
    let last = *quads.last().ok_or_else(|| AnnotError::Invalid("select the text to replace first".into()))?;
    let s = add_annotation(
        doc,
        &NewAnnotation {
            page,
            shape: Shape::TextMarkup { kind: Markup::StrikeOut, quads: quads.to_vec() },
            style: strike.clone(),
            contents: String::new(),
            author: author.into(),
        },
        meta,
    )?;
    let (_, sr) = annot_ref(doc, page, s)?;
    doc.update_dict(sr, |d| d.set(b"IT".to_vec(), Object::name("StrikeOutTextEdit")))?;
    // The caret sits on the baseline at the end of the last line, its point at the text.
    let xs = [last[0], last[2], last[4], last[6]];
    let ys = [last[1], last[3], last[5], last[7]];
    let x = xs.iter().copied().fold(f64::MIN, f64::max);
    let (y0, y1) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
    let h = ((y1 - y0) * 0.6).clamp(4.0, 24.0);
    let rect = [x - h / 2.0, y0 - h * 0.6, x + h / 2.0, y0 + h * 0.4];
    let c = add_annotation(
        doc,
        &NewAnnotation { page, shape: Shape::Caret { rect }, style: caret.clone(), contents: replacement.into(), author: author.into() },
        caret_meta,
    )?;
    let (_, cr) = annot_ref(doc, page, c)?;
    doc.update_dict(cr, |d| {
        d.set(b"IRT".to_vec(), Object::Ref(sr));
        d.set(b"RT".to_vec(), Object::name("Group"));
        d.set(b"Subj".to_vec(), PdfString::text("Replace Text"));
    })?;
    Ok(s)
}

/// The Eraser (Draw tools): remove the parts of drawing `(page, index)` within `radius` points
/// of `path`, splitting strokes where they are cut. A drawing with nothing left is deleted.
/// Returns whether anything was erased.
pub fn erase_ink(doc: &mut Document, page: usize, index: usize, path: &[[f64; 2]], radius: f64, meta: &Meta) -> Result<bool, AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if d.name(b"Subtype") != Some(b"Ink") {
        return Err(AnnotError::Invalid("only drawings can be erased".into()));
    }
    if path.is_empty() || !path.iter().all(|p| finite(p)) || !radius.is_finite() || radius <= 0.0 {
        return Err(AnnotError::Invalid("invalid eraser path".into()));
    }
    let strokes: Vec<Vec<[f64; 2]>> = d
        .get(b"InkList")
        .map(|l| doc.resolve(l))
        .and_then(|l| l.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|s| {
            let v: Vec<f64> = doc.resolve(s).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()).unwrap_or_default();
            v.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect()
        })
        .collect();
    // Distance from a point to the eraser's path (segments, or a single point).
    let near = |p: [f64; 2]| -> bool {
        if path.len() == 1 {
            return (p[0] - path[0][0]).hypot(p[1] - path[0][1]) <= radius;
        }
        path.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy)) <= radius
        })
    };
    let mut kept: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut erased = false;
    for s in &strokes {
        // Densify so long segments are cut where the eraser crosses them.
        let mut pts: Vec<[f64; 2]> = Vec::new();
        for w in s.windows(2) {
            let n = ((w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]) / (radius / 2.0)).ceil().clamp(1.0, 2000.0) as usize;
            for k in 0..n {
                let t = k as f64 / n as f64;
                pts.push([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
            }
        }
        if let Some(last) = s.last() {
            pts.push(*last);
        }
        let mut run: Vec<[f64; 2]> = Vec::new();
        for p in pts {
            if near(p) {
                erased = true;
                if run.len() >= 2 {
                    kept.push(std::mem::take(&mut run));
                }
                run.clear();
            } else {
                run.push(p);
            }
        }
        if run.len() >= 2 {
            kept.push(run);
        }
    }
    if !erased {
        return Ok(false);
    }
    if kept.is_empty() {
        delete_annotation(doc, page, index)?;
        return Ok(true);
    }
    let width = d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()).unwrap_or(1.0);
    let rect = grow(bounds(kept.iter().flatten().copied()).unwrap_or_default(), width / 2.0 + 1.0);
    doc.update_dict(r, |d| {
        d.set(b"InkList".to_vec(), Object::Array(kept.iter().map(|s| num_array(&s.concat())).collect()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    set_appearance(doc, r)?;
    Ok(true)
}
