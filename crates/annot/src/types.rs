use super::create::normalize;
use super::{ObjRef, appearance};

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

/// The style properties changed by one annotation edit. `None` leaves a property unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleChange {
    pub color: Option<Rgb>,
    pub opacity: Option<f64>,
    pub width: Option<f64>,
    pub endings: Option<Vec<LineEnding>>,
}

impl StyleChange {
    pub fn new(color: Option<Rgb>, opacity: Option<f64>, width: Option<f64>, endings: Option<Vec<LineEnding>>) -> Self {
        Self { color, opacity, width, endings }
    }
}

/// Inputs for replacing selected text with a grouped strikeout and caret annotation.
pub struct TextReplacement<'a> {
    pub page: usize,
    pub quads: &'a [[f64; 8]],
    pub replacement: &'a str,
    pub author: &'a str,
    pub strike: &'a Style,
    pub caret: &'a Style,
    pub strike_meta: &'a Meta,
    pub caret_meta: &'a Meta,
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

// ── building ────────────────────────────────────────────────────────────────────────────────

/// Annotation flags (§12.5.3).
pub(crate) const FLAG_PRINT: i64 = 4;

pub(crate) const FLAG_NO_ZOOM: i64 = 8;

pub(crate) const FLAG_NO_ROTATE: i64 = 16;

pub(crate) const FLAG_HIDDEN: i64 = 2;

pub(crate) const FLAG_NO_VIEW: i64 = 32;

pub(crate) const FLAG_LOCKED: i64 = 128;

/// Size of a note icon (points, unscaled by zoom).
pub const NOTE_SIZE: f64 = 20.0;
