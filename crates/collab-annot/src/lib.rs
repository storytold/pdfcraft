//! Typed wire representation of the annotations a collaboration session shares.
//!
//! The engine's [`pdfcraft_annot::Shape`] and [`pdfcraft_annot::Style`] are not
//! `serde` types: they carry `ObjRef`s, embedded blobs and other things a shared
//! operation must not move between replicas. This crate defines the v1 subset as
//! plain JSON with a `kind` tag and converts both ways:
//!
//! * the server adapter parses `annotation.add` into [`pdfcraft_annot::NewAnnotation`];
//! * the client projection turns an `Edit::AddAnnotation` back into a payload;
//! * the same JSON crosses the wire, so both sides agree on what "a highlight"
//!   means without sharing Rust structs.
//!
//! Supported: `TextMarkup` (kind, quads), `Rectangle`, `Oval`, `Line`, `Ink`,
//! `FreehandHighlight`, `TextBox`, `Typewriter`, `Stamp` (palette), `Mark`, and
//! the sticky `Note`. Blobs, drawn and typed signatures, custom stamps,
//! redaction marks and attachments are refused on purpose: they reference data
//! outside the operation (an image XObject, a file body) that v1 has no way to
//! carry, and redaction is a structural, irreversible change.
//!
//! The style travels as a complete object: `color`, `opacity`, `width`, `fill`,
//! `border_color`, `border_shape`, `fill_opacity`, `text_opacity`. Missing style
//! fields fall back to [`pdfcraft_annot::Style::default_for`] of the shape, so a
//! minimal payload `{"shape": …, "contents": …}` still produces the same
//! annotation every replica would draw from the same tool.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_annot::{BorderShape, FillMark, LineEnding, Markup, NewAnnotation, NoteIcon, Shape, StampKind, Style};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Most quadrilaterals one text markup may carry.
pub const MAX_QUADS: usize = 4096;
/// Most strokes one ink or freehand highlight may carry.
pub const MAX_STROKES: usize = 1024;
/// Most points in one stroke.
pub const MAX_STROKE_POINTS: usize = 4096;
/// Most points across all strokes of one drawing. Kept well under the
/// `MAX_MESSAGE_BYTES` (256 KiB) frame budget: a stroke point is ~20 bytes of
/// JSON, so 4096 points plus the fixed fields fit with room to spare.
pub const MAX_TOTAL_POINTS: usize = 4096;
/// Longest comment text (bytes) one operation carries.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// Longest author line (bytes) one operation carries.
pub const MAX_AUTHOR_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("{0} is not shared in a collaboration session yet")]
    UnsupportedShape(&'static str),
    #[error("the annotation payload is malformed: {0}")]
    Malformed(String),
    #[error("{field} is invalid: {why}")]
    Invalid { field: &'static str, why: String },
    #[error("the annotation carries {count} {what}; the limit is {max}")]
    TooMany { what: &'static str, count: usize, max: usize },
}

/// A new annotation as an operation sees it: the shape, its style and the text.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationInput {
    pub shape: Shape,
    pub style: Style,
    pub contents: String,
    pub author: String,
}

impl AnnotationInput {
    /// A shape with the style the engine would give it and no text.
    pub fn new(shape: Shape) -> Self {
        let style = Style::default_for(&shape);
        Self { shape, style, contents: String::new(), author: String::new() }
    }

    /// The engine's description of what to add, for the given 0-based page.
    pub fn to_engine(&self, page: usize) -> NewAnnotation {
        NewAnnotation { page, shape: self.shape.clone(), style: self.style.clone(), contents: self.contents.clone(), author: self.author.clone() }
    }

    /// The body of an `annotation.add` operation addressed to `nm` (ADR-0002).
    pub fn to_payload(&self, nm: &str) -> Result<Value, WireError> {
        let shape = ShapeWire::try_from(&self.shape)?;
        let mut payload = Map::new();
        payload.insert("action".into(), json!("annotation.add"));
        payload.insert("nm".into(), json!(nm));
        payload.insert("shape".into(), serde_json::to_value(&shape).map_err(malformed)?);
        payload.insert("style".into(), serde_json::to_value(StyleWire::from(&self.style)).map_err(malformed)?);
        payload.insert("contents".into(), json!(self.contents));
        payload.insert("author".into(), json!(self.author));
        Ok(Value::Object(payload))
    }

    /// Parse and validate the body of an `annotation.add`. A payload without a
    /// `shape` is read as the v1 sticky note (`at`), so operations queued by an
    /// older client still land.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let shape = match payload.get("shape") {
            Some(value) => {
                let wire: ShapeWire = serde_json::from_value(value.clone()).map_err(|e| WireError::Malformed(e.to_string()))?;
                wire.into_shape()?
            }
            None => {
                let at = payload.get("at").ok_or_else(|| WireError::Malformed("the operation has neither a shape nor a note position".into()))?;
                let at: [f64; 2] = serde_json::from_value(at.clone()).map_err(|e| WireError::Malformed(e.to_string()))?;
                check_point(at, "note position")?;
                Shape::Note { at, icon: NoteIcon::Comment }
            }
        };
        let style = match payload.get("style").filter(|value| !value.is_null()) {
            Some(value) => {
                let fields: StyleFields = serde_json::from_value(value.clone()).map_err(|e| WireError::Malformed(e.to_string()))?;
                fields.into_style(&shape)?
            }
            None => Style::default_for(&shape),
        };
        let contents = text_field(payload, "contents", MAX_TEXT_BYTES)?;
        let author = text_field(payload, "author", MAX_AUTHOR_BYTES)?;
        let input = Self { shape, style, contents, author };
        input.validate()?;
        Ok(input)
    }

    /// Reject geometry and style that would produce a different annotation on
    /// another replica (NaN, out-of-range opacity, runaway stroke counts).
    pub fn validate(&self) -> Result<(), WireError> {
        check_style(&self.style)?;
        check_shape(&self.shape)
    }
}

impl TryFrom<&NewAnnotation> for AnnotationInput {
    type Error = WireError;

    fn try_from(new: &NewAnnotation) -> Result<Self, WireError> {
        let input = Self { shape: new.shape.clone(), style: new.style.clone(), contents: new.contents.clone(), author: new.author.clone() };
        input.validate()?;
        Ok(input)
    }
}

fn malformed(error: serde_json::Error) -> WireError {
    WireError::Malformed(error.to_string())
}

fn text_field(payload: &Value, field: &'static str, max: usize) -> Result<String, WireError> {
    match payload.get(field) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => {
            if text.len() > max {
                return Err(WireError::TooMany { what: "text bytes", count: text.len(), max });
            }
            Ok(text.clone())
        }
        Some(_) => Err(WireError::Invalid { field, why: "expected a string".into() }),
    }
}

fn finite(value: f64, field: &'static str) -> Result<f64, WireError> {
    if value.is_finite() { Ok(value) } else { Err(WireError::Invalid { field, why: "the number must be finite".into() }) }
}

fn check_rect(rect: [f64; 4], field: &'static str) -> Result<(), WireError> {
    for value in rect {
        finite(value, field)?;
    }
    Ok(())
}

fn check_point(point: [f64; 2], field: &'static str) -> Result<(), WireError> {
    for value in point {
        finite(value, field)?;
    }
    Ok(())
}

fn check_quad(quad: [f64; 8]) -> Result<(), WireError> {
    for value in quad {
        finite(value, "quad")?;
    }
    Ok(())
}

fn check_strokes(strokes: &[Vec<[f64; 2]>]) -> Result<(), WireError> {
    if strokes.len() > MAX_STROKES {
        return Err(WireError::TooMany { what: "strokes", count: strokes.len(), max: MAX_STROKES });
    }
    let mut total = 0usize;
    for stroke in strokes {
        if stroke.len() > MAX_STROKE_POINTS {
            return Err(WireError::TooMany { what: "points in a stroke", count: stroke.len(), max: MAX_STROKE_POINTS });
        }
        total = total.saturating_add(stroke.len());
        for point in stroke {
            check_point(*point, "stroke point")?;
        }
    }
    if total > MAX_TOTAL_POINTS {
        return Err(WireError::TooMany { what: "stroke points", count: total, max: MAX_TOTAL_POINTS });
    }
    Ok(())
}

fn check_shape(shape: &Shape) -> Result<(), WireError> {
    match shape {
        Shape::Note { at, .. } => check_point(*at, "note position"),
        Shape::TextMarkup { quads, .. } => {
            if quads.len() > MAX_QUADS {
                return Err(WireError::TooMany { what: "quadrilaterals", count: quads.len(), max: MAX_QUADS });
            }
            quads.iter().try_for_each(|quad| check_quad(*quad))
        }
        Shape::Rectangle { rect } | Shape::Oval { rect } | Shape::TextBox { rect, .. } | Shape::Typewriter { rect, .. } => check_rect(*rect, "rect"),
        Shape::Stamp { rect, by, .. } => {
            check_rect(*rect, "rect")?;
            if let Some(by) = by
                && by.len() > MAX_AUTHOR_BYTES
            {
                return Err(WireError::TooMany { what: "text bytes", count: by.len(), max: MAX_AUTHOR_BYTES });
            }
            Ok(())
        }
        Shape::Mark { rect, .. } => check_rect(*rect, "rect"),
        Shape::Line { from, to, .. } => {
            check_point(*from, "line start")?;
            check_point(*to, "line end")
        }
        Shape::Ink { strokes } | Shape::FreehandHighlight { strokes } => check_strokes(strokes),
        other => Err(WireError::UnsupportedShape(shape_label(other))),
    }
}

/// A name a user (and a log) can act on; `/Subtype` alone maps several shapes
/// onto `Ink` or `Stamp`.
fn shape_label(shape: &Shape) -> &'static str {
    match shape {
        Shape::Signature { .. } => "a drawn signature",
        Shape::TypedSignature { .. } => "a typed signature",
        Shape::CustomStamp { .. } => "a custom stamp",
        Shape::Redact { .. } => "a redaction mark",
        Shape::Polygon { .. } => "a polygon",
        Shape::PolyLine { .. } => "a polyline",
        Shape::Callout { .. } => "a callout",
        Shape::Caret { .. } => "an insert-text caret",
        Shape::Attachment { .. } => "a file attachment",
        Shape::Note { .. } => "a sticky note",
        Shape::TextMarkup { .. } => "a text markup",
        Shape::Rectangle { .. } => "a rectangle",
        Shape::Oval { .. } => "an oval",
        Shape::Line { .. } => "a line",
        Shape::Ink { .. } => "an ink drawing",
        Shape::FreehandHighlight { .. } => "a freehand highlight",
        Shape::TextBox { .. } => "a text box",
        Shape::Typewriter { .. } => "typewriter text",
        Shape::Mark { .. } => "a fill-and-sign mark",
        Shape::Stamp { .. } => "a stamp",
    }
}

fn check_style(style: &Style) -> Result<(), WireError> {
    for value in style.color {
        finite(value, "color")?;
    }
    if !(0.0..=1.0).contains(&style.opacity) || !style.opacity.is_finite() {
        return Err(WireError::Invalid { field: "opacity", why: "must be between 0 and 1".into() });
    }
    if !style.width.is_finite() || style.width < 0.0 {
        return Err(WireError::Invalid { field: "width", why: "must be a non-negative number".into() });
    }
    if let Some(fill) = style.fill {
        for value in fill {
            finite(value, "fill")?;
        }
    }
    if let Some(color) = style.border_color {
        for value in color {
            finite(value, "border colour")?;
        }
    }
    for (name, value) in [("fill opacity", style.fill_opacity), ("text opacity", style.text_opacity)] {
        if let Some(value) = value
            && (!(0.0..=1.0).contains(&value) || !value.is_finite())
        {
            return Err(WireError::Invalid { field: name, why: "must be between 0 and 1".into() });
        }
    }
    Ok(())
}

// ── The wire types ───────────────────────────────────────────────────────────

/// A shareable annotation shape. The `kind` tag names the variant; the rest of
/// the object is its geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeWire {
    Note {
        at: [f64; 2],
        #[serde(default)]
        icon: IconWire,
    },
    TextMarkup {
        markup: MarkupWire,
        quads: Vec<[f64; 8]>,
    },
    Rectangle {
        rect: [f64; 4],
    },
    Oval {
        rect: [f64; 4],
    },
    Line {
        from: [f64; 2],
        to: [f64; 2],
        #[serde(default)]
        start: EndingWire,
        #[serde(default)]
        end: EndingWire,
    },
    Ink {
        strokes: Vec<Vec<[f64; 2]>>,
    },
    FreehandHighlight {
        strokes: Vec<Vec<[f64; 2]>>,
    },
    TextBox {
        rect: [f64; 4],
        #[serde(default = "default_font_size")]
        font_size: f64,
    },
    Typewriter {
        rect: [f64; 4],
        #[serde(default = "default_font_size")]
        font_size: f64,
    },
    Stamp {
        rect: [f64; 4],
        stamp: StampWire,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    Mark {
        rect: [f64; 4],
        mark: MarkWire,
    },
    /// Any other `kind`: decoded so the refusal names the payload rather than
    /// failing as malformed JSON.
    #[serde(other)]
    Unsupported,
}

fn default_font_size() -> f64 {
    12.0
}

impl ShapeWire {
    pub fn into_shape(self) -> Result<Shape, WireError> {
        let shape = match self {
            ShapeWire::Note { at, icon } => Shape::Note { at, icon: icon.into() },
            ShapeWire::TextMarkup { markup, quads } => Shape::TextMarkup { kind: markup.into(), quads },
            ShapeWire::Rectangle { rect } => Shape::Rectangle { rect },
            ShapeWire::Oval { rect } => Shape::Oval { rect },
            ShapeWire::Line { from, to, start, end } => Shape::Line { from, to, start: start.into(), end: end.into() },
            ShapeWire::Ink { strokes } => Shape::Ink { strokes },
            ShapeWire::FreehandHighlight { strokes } => Shape::FreehandHighlight { strokes },
            ShapeWire::TextBox { rect, font_size } => Shape::TextBox { rect, font_size },
            ShapeWire::Typewriter { rect, font_size } => Shape::Typewriter { rect, font_size },
            ShapeWire::Stamp { rect, stamp, by } => Shape::Stamp { rect, stamp: stamp.into(), by },
            ShapeWire::Mark { rect, mark } => Shape::Mark { rect, mark: mark.into() },
            ShapeWire::Unsupported => return Err(WireError::UnsupportedShape("this annotation shape")),
        };
        check_shape(&shape)?;
        Ok(shape)
    }
}

impl TryFrom<&Shape> for ShapeWire {
    type Error = WireError;

    fn try_from(shape: &Shape) -> Result<Self, WireError> {
        let wire = match shape {
            Shape::Note { at, icon } => ShapeWire::Note { at: *at, icon: (*icon).into() },
            Shape::TextMarkup { kind, quads } => ShapeWire::TextMarkup { markup: (*kind).into(), quads: quads.clone() },
            Shape::Rectangle { rect } => ShapeWire::Rectangle { rect: *rect },
            Shape::Oval { rect } => ShapeWire::Oval { rect: *rect },
            Shape::Line { from, to, start, end } => ShapeWire::Line { from: *from, to: *to, start: (*start).into(), end: (*end).into() },
            Shape::Ink { strokes } => ShapeWire::Ink { strokes: strokes.clone() },
            Shape::FreehandHighlight { strokes } => ShapeWire::FreehandHighlight { strokes: strokes.clone() },
            Shape::TextBox { rect, font_size } => ShapeWire::TextBox { rect: *rect, font_size: *font_size },
            Shape::Typewriter { rect, font_size } => ShapeWire::Typewriter { rect: *rect, font_size: *font_size },
            Shape::Stamp { rect, stamp, by } => ShapeWire::Stamp { rect: *rect, stamp: (*stamp).into(), by: by.clone() },
            Shape::Mark { rect, mark } => ShapeWire::Mark { rect: *rect, mark: (*mark).into() },
            other => return Err(WireError::UnsupportedShape(shape_label(other))),
        };
        check_shape(shape)?;
        Ok(wire)
    }
}

/// The style of a shared annotation. Every geometric style the engine's tools
/// set travels, so a replica draws the same colour, width and opacity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StyleWire {
    pub color: [f64; 3],
    pub opacity: f64,
    pub width: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<[f64; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color: Option<[f64; 3]>,
    pub border_shape: BorderShapeWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_opacity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_opacity: Option<f64>,
}

/// What may be absent in a style object; the rest comes from the shape default.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct StyleFields {
    color: Option<[f64; 3]>,
    opacity: Option<f64>,
    width: Option<f64>,
    fill: Option<[f64; 3]>,
    border_color: Option<[f64; 3]>,
    border_shape: Option<BorderShapeWire>,
    fill_opacity: Option<f64>,
    text_opacity: Option<f64>,
}

impl StyleFields {
    fn into_style(self, shape: &Shape) -> Result<Style, WireError> {
        let defaults = Style::default_for(shape);
        let style = Style {
            color: self.color.unwrap_or(defaults.color),
            opacity: self.opacity.unwrap_or(defaults.opacity),
            width: self.width.unwrap_or(defaults.width),
            fill: self.fill.or(defaults.fill),
            border_color: self.border_color.or(defaults.border_color),
            border_shape: self.border_shape.map_or(defaults.border_shape, Into::into),
            fill_opacity: self.fill_opacity.or(defaults.fill_opacity),
            text_opacity: self.text_opacity.or(defaults.text_opacity),
        };
        check_style(&style)?;
        Ok(style)
    }
}

impl From<&Style> for StyleWire {
    fn from(style: &Style) -> Self {
        Self {
            color: style.color,
            opacity: style.opacity,
            width: style.width,
            fill: style.fill,
            border_color: style.border_color,
            border_shape: style.border_shape.into(),
            fill_opacity: style.fill_opacity,
            text_opacity: style.text_opacity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkupWire {
    #[default]
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndingWire {
    #[default]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkWire {
    #[default]
    Check,
    Cross,
    Dot,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BorderShapeWire {
    #[default]
    Rectangle,
    Oval,
    Circle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconWire {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StampWire {
    #[default]
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
    Accepted,
    InitialHere,
    Rejected,
    SignHere,
    Witness,
    DynApproved,
    DynConfidential,
    DynReceived,
    DynReviewed,
    DynRevised,
}

impl From<Markup> for MarkupWire {
    fn from(value: Markup) -> Self {
        match value {
            Markup::Highlight => Self::Highlight,
            Markup::Underline => Self::Underline,
            Markup::StrikeOut => Self::StrikeOut,
            Markup::Squiggly => Self::Squiggly,
        }
    }
}

impl From<MarkupWire> for Markup {
    fn from(value: MarkupWire) -> Self {
        match value {
            MarkupWire::Highlight => Self::Highlight,
            MarkupWire::Underline => Self::Underline,
            MarkupWire::StrikeOut => Self::StrikeOut,
            MarkupWire::Squiggly => Self::Squiggly,
        }
    }
}

impl From<LineEnding> for EndingWire {
    fn from(value: LineEnding) -> Self {
        match value {
            LineEnding::None => Self::None,
            LineEnding::Square => Self::Square,
            LineEnding::Circle => Self::Circle,
            LineEnding::Diamond => Self::Diamond,
            LineEnding::OpenArrow => Self::OpenArrow,
            LineEnding::ClosedArrow => Self::ClosedArrow,
            LineEnding::Butt => Self::Butt,
            LineEnding::ROpenArrow => Self::ROpenArrow,
            LineEnding::RClosedArrow => Self::RClosedArrow,
            LineEnding::Slash => Self::Slash,
        }
    }
}

impl From<EndingWire> for LineEnding {
    fn from(value: EndingWire) -> Self {
        match value {
            EndingWire::None => Self::None,
            EndingWire::Square => Self::Square,
            EndingWire::Circle => Self::Circle,
            EndingWire::Diamond => Self::Diamond,
            EndingWire::OpenArrow => Self::OpenArrow,
            EndingWire::ClosedArrow => Self::ClosedArrow,
            EndingWire::Butt => Self::Butt,
            EndingWire::ROpenArrow => Self::ROpenArrow,
            EndingWire::RClosedArrow => Self::RClosedArrow,
            EndingWire::Slash => Self::Slash,
        }
    }
}

impl From<FillMark> for MarkWire {
    fn from(value: FillMark) -> Self {
        match value {
            FillMark::Check => Self::Check,
            FillMark::Cross => Self::Cross,
            FillMark::Dot => Self::Dot,
            FillMark::Line => Self::Line,
        }
    }
}

impl From<MarkWire> for FillMark {
    fn from(value: MarkWire) -> Self {
        match value {
            MarkWire::Check => Self::Check,
            MarkWire::Cross => Self::Cross,
            MarkWire::Dot => Self::Dot,
            MarkWire::Line => Self::Line,
        }
    }
}

impl From<BorderShape> for BorderShapeWire {
    fn from(value: BorderShape) -> Self {
        match value {
            BorderShape::Rectangle => Self::Rectangle,
            BorderShape::Oval => Self::Oval,
            BorderShape::Circle => Self::Circle,
        }
    }
}

impl From<BorderShapeWire> for BorderShape {
    fn from(value: BorderShapeWire) -> Self {
        match value {
            BorderShapeWire::Rectangle => Self::Rectangle,
            BorderShapeWire::Oval => Self::Oval,
            BorderShapeWire::Circle => Self::Circle,
        }
    }
}

impl From<NoteIcon> for IconWire {
    fn from(value: NoteIcon) -> Self {
        match value {
            NoteIcon::Comment => Self::Comment,
            NoteIcon::Note => Self::Note,
            NoteIcon::Help => Self::Help,
            NoteIcon::Insert => Self::Insert,
            NoteIcon::Key => Self::Key,
            NoteIcon::NewParagraph => Self::NewParagraph,
            NoteIcon::Paragraph => Self::Paragraph,
            NoteIcon::Check => Self::Check,
            NoteIcon::Circle => Self::Circle,
            NoteIcon::Cross => Self::Cross,
            NoteIcon::CrossHairs => Self::CrossHairs,
            NoteIcon::Star => Self::Star,
            NoteIcon::RightArrow => Self::RightArrow,
            NoteIcon::UpArrow => Self::UpArrow,
        }
    }
}

impl From<IconWire> for NoteIcon {
    fn from(value: IconWire) -> Self {
        match value {
            IconWire::Comment => Self::Comment,
            IconWire::Note => Self::Note,
            IconWire::Help => Self::Help,
            IconWire::Insert => Self::Insert,
            IconWire::Key => Self::Key,
            IconWire::NewParagraph => Self::NewParagraph,
            IconWire::Paragraph => Self::Paragraph,
            IconWire::Check => Self::Check,
            IconWire::Circle => Self::Circle,
            IconWire::Cross => Self::Cross,
            IconWire::CrossHairs => Self::CrossHairs,
            IconWire::Star => Self::Star,
            IconWire::RightArrow => Self::RightArrow,
            IconWire::UpArrow => Self::UpArrow,
        }
    }
}

impl From<StampKind> for StampWire {
    fn from(value: StampKind) -> Self {
        match value {
            StampKind::Approved => Self::Approved,
            StampKind::Completed => Self::Completed,
            StampKind::Confidential => Self::Confidential,
            StampKind::Draft => Self::Draft,
            StampKind::Final => Self::Final,
            StampKind::ForComment => Self::ForComment,
            StampKind::ForPublicRelease => Self::ForPublicRelease,
            StampKind::InformationOnly => Self::InformationOnly,
            StampKind::NotApproved => Self::NotApproved,
            StampKind::NotForPublicRelease => Self::NotForPublicRelease,
            StampKind::PreliminaryResults => Self::PreliminaryResults,
            StampKind::Void => Self::Void,
            StampKind::Accepted => Self::Accepted,
            StampKind::InitialHere => Self::InitialHere,
            StampKind::Rejected => Self::Rejected,
            StampKind::SignHere => Self::SignHere,
            StampKind::Witness => Self::Witness,
            StampKind::DynApproved => Self::DynApproved,
            StampKind::DynConfidential => Self::DynConfidential,
            StampKind::DynReceived => Self::DynReceived,
            StampKind::DynReviewed => Self::DynReviewed,
            StampKind::DynRevised => Self::DynRevised,
        }
    }
}

impl From<StampWire> for StampKind {
    fn from(value: StampWire) -> Self {
        match value {
            StampWire::Approved => Self::Approved,
            StampWire::Completed => Self::Completed,
            StampWire::Confidential => Self::Confidential,
            StampWire::Draft => Self::Draft,
            StampWire::Final => Self::Final,
            StampWire::ForComment => Self::ForComment,
            StampWire::ForPublicRelease => Self::ForPublicRelease,
            StampWire::InformationOnly => Self::InformationOnly,
            StampWire::NotApproved => Self::NotApproved,
            StampWire::NotForPublicRelease => Self::NotForPublicRelease,
            StampWire::PreliminaryResults => Self::PreliminaryResults,
            StampWire::Void => Self::Void,
            StampWire::Accepted => Self::Accepted,
            StampWire::InitialHere => Self::InitialHere,
            StampWire::Rejected => Self::Rejected,
            StampWire::SignHere => Self::SignHere,
            StampWire::Witness => Self::Witness,
            StampWire::DynApproved => Self::DynApproved,
            StampWire::DynConfidential => Self::DynConfidential,
            StampWire::DynReceived => Self::DynReceived,
            StampWire::DynReviewed => Self::DynReviewed,
            StampWire::DynRevised => Self::DynRevised,
        }
    }
}

#[cfg(test)]
mod tests;
