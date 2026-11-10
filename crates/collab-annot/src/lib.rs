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
//!
//! An existing annotation is restyled by `annotation.update` ([`StyleUpdate`]):
//! the *final* colour, opacity, line width, fill and border, addressed by `/NM`,
//! checked against the comment before anything changes. A state reply is an
//! `annotation.reply` that also carries a review state (`/State`,
//! [`ReviewStateWire`]), named by the client like any other reply.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_annot::{BorderShape, FillMark, LineEnding, Markup, NewAnnotation, NoteIcon, Props, Restyle, ReviewState, Shape, StampKind, Style};
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
/// Smallest font size (pt) a text box, typewriter text or callout may have.
pub const MIN_FONT_SIZE: f64 = 1.0;
/// Largest font size (pt) the engine draws text at.
pub const MAX_FONT_SIZE: f64 = 400.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("{0} is not shared in a collaboration session yet")]
    UnsupportedShape(&'static str),
    #[error("{0} cannot be restyled in a shared session")]
    NotRestylable(&'static str),
    #[error("{0} cannot be moved in a shared session")]
    NotMovable(&'static str),
    #[error("{0} cannot be reshaped in a shared session")]
    NotReshapable(&'static str),
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

// ── Restyling an existing annotation ─────────────────────────────────────────

/// How close two style numbers must be to count as the same value: the engine
/// writes three decimals, and a replica reads back what it wrote.
const STYLE_EPSILON: f64 = 5e-4;

/// The final style one `annotation.update` sets on an existing annotation,
/// addressed by its `/NM` (ADR-0002).
///
/// Only the properties named are changed; `null` clears a property that can be
/// taken away — a fill, a border colour. It is one shared operation where the
/// interactive interface sends several edits (`StyleAnnotation`,
/// `FillAnnotation`, `BorderColorAnnotation`, `BorderShapeAnnotation`): one
/// version, one undo step, and the same final look on every replica.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleUpdate {
    pub color: Option<[f64; 3]>,
    pub opacity: Option<f64>,
    pub width: Option<f64>,
    /// `/LE`: two endings for a line or polyline, one for a callout.
    pub endings: Option<Vec<EndingWire>>,
    /// `Some(None)` takes the fill away; `Some(Some(colour))` sets it.
    pub fill: Option<Option<[f64; 3]>>,
    /// `Some(None)` puts the border back to the text's colour.
    pub border_color: Option<Option<[f64; 3]>>,
    pub border_shape: Option<BorderShapeWire>,
}

impl StyleUpdate {
    /// Whether the update names no property at all.
    pub fn is_empty(&self) -> bool {
        self.color.is_none()
            && self.opacity.is_none()
            && self.width.is_none()
            && self.endings.is_none()
            && self.fill.is_none()
            && self.border_color.is_none()
            && self.border_shape.is_none()
    }

    /// The body of an `annotation.update`: `{"action": …, "style": {…}}`.
    pub fn to_payload(&self) -> Value {
        json!({ "action": "annotation.update", "style": self.to_value() })
    }

    /// The `style` object on its own, as every place that carries it (the
    /// accepted operation's `style` and `before`) writes it.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        if let Some(color) = self.color {
            object.insert("color".into(), json!(color));
        }
        if let Some(opacity) = self.opacity {
            object.insert("opacity".into(), json!(opacity));
        }
        if let Some(width) = self.width {
            object.insert("width".into(), json!(width));
        }
        if let Some(endings) = &self.endings {
            object.insert("endings".into(), json!(endings));
        }
        if let Some(fill) = self.fill {
            object.insert("fill".into(), fill.map_or(Value::Null, |color| json!(color)));
        }
        if let Some(color) = self.border_color {
            object.insert("border_color".into(), color.map_or(Value::Null, |color| json!(color)));
        }
        if let Some(shape) = self.border_shape {
            object.insert("border_shape".into(), json!(shape.name()));
        }
        Value::Object(object)
    }

    /// Read the `style` object of an `annotation.update`.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let style = payload.get("style").filter(|value| !value.is_null()).ok_or_else(|| WireError::Malformed("the update names no style".into()))?;
        Self::from_value(style)
    }

    /// Read a style object: the properties present are the ones the update sets.
    pub fn from_value(style: &Value) -> Result<Self, WireError> {
        let object = style.as_object().ok_or_else(|| WireError::Malformed("the style must be an object".into()))?;
        let mut update = Self::default();
        for (key, field) in object {
            match key.as_str() {
                "color" => update.color = Some(color_field(field, "color")?),
                "opacity" => update.opacity = Some(unit_field(field, "opacity")?),
                "width" => update.width = Some(width_field(field)?),
                "endings" => update.endings = Some(endings_field(field)?),
                "fill" => update.fill = Some(optional_color_field(field, "fill")?),
                "border_color" => update.border_color = Some(optional_color_field(field, "border colour")?),
                "border_shape" => {
                    let name = field.as_str().ok_or_else(|| WireError::Invalid { field: "border shape", why: "expected a name".into() })?;
                    update.border_shape = Some(BorderShapeWire::parse(name)?);
                }
                other => {
                    return Err(WireError::Invalid { field: "style", why: format!("{other:?} is not a style property") });
                }
            }
        }
        if update.is_empty() {
            return Err(WireError::Malformed("the style names no property".into()));
        }
        Ok(update)
    }

    /// Refuse properties the comment does not have, before anything changes: the
    /// shared operation must fail as a whole or not at all.
    pub fn check_for(&self, props: &Props) -> Result<(), WireError> {
        if props.locked {
            return Err(WireError::NotRestylable("a locked comment"));
        }
        let redraw = self.color.is_some() || self.opacity.is_some() || self.width.is_some() || self.endings.is_some();
        if redraw && !props.restylable {
            return Err(WireError::NotRestylable("this comment's appearance"));
        }
        if self.width.is_some() && props.width.is_none() {
            return Err(WireError::Invalid { field: "width", why: "this comment has no line width".into() });
        }
        if let Some(endings) = &self.endings {
            let Some(own) = props.endings.as_ref() else {
                return Err(WireError::Invalid { field: "endings", why: "this comment has no line endings".into() });
            };
            if own.len() != endings.len() {
                return Err(WireError::Invalid { field: "endings", why: format!("this comment has {} ending(s)", own.len()) });
            }
        }
        if self.fill.is_some() && !props.fillable {
            return Err(WireError::Invalid { field: "fill", why: "this comment has no fill".into() });
        }
        // The border's own colour and shape belong to text boxes, typewriter text
        // and callouts; a callout has no shape to choose.
        if self.border_color.is_some() && props.font_size.is_none() {
            return Err(WireError::Invalid { field: "border colour", why: "only text boxes, typewriter text and callouts have one".into() });
        }
        if self.border_shape.is_some() && props.border_shape.is_none() {
            return Err(WireError::Invalid { field: "border shape", why: "only text boxes and typewriter text have one".into() });
        }
        Ok(())
    }

    /// The style the comment had for exactly the properties this update sets —
    /// what the accepted operation carries as `before`, and what an undo puts
    /// back. A comment without `/C` is black, so its colour is recorded as black.
    pub fn before(&self, props: &Props) -> Self {
        Self {
            color: self.color.map(|_| props.color.unwrap_or([0.0, 0.0, 0.0])),
            opacity: self.opacity.map(|_| props.opacity),
            width: self.width.and(props.width),
            endings: self.endings.as_ref().map(|_| props.endings.iter().flatten().copied().map(EndingWire::from).collect()),
            fill: self.fill.map(|_| props.fill),
            border_color: self.border_color.map(|_| props.border_color),
            border_shape: self.border_shape.and(props.border_shape.map(BorderShapeWire::from)),
        }
    }

    /// Whether the comment still looks the way `expect` names: an undo is a
    /// compare-and-set, so a later edit by someone else is a conflict, never an
    /// overwrite (ADR-0010 §3).
    pub fn still_matches(&self, props: &Props) -> bool {
        let color = |value: [f64; 3], current: Option<[f64; 3]>| {
            let current = current.unwrap_or([0.0, 0.0, 0.0]);
            (0..3).all(|i| (value[i] - current[i]).abs() <= STYLE_EPSILON)
        };
        let same_optional = |value: Option<[f64; 3]>, current: Option<[f64; 3]>| match value {
            None => current.is_none(),
            Some(value) => color(value, current),
        };
        if self.color.is_some_and(|value| !color(value, props.color)) {
            return false;
        }
        if self.opacity.is_some_and(|value| (value - props.opacity).abs() > STYLE_EPSILON) {
            return false;
        }
        if self.width.is_some_and(|value| props.width.is_none_or(|current| (value - current).abs() > STYLE_EPSILON)) {
            return false;
        }
        if let Some(value) = &self.endings {
            let current: Option<Vec<EndingWire>> = props.endings.as_ref().map(|ends| ends.iter().copied().map(EndingWire::from).collect());
            if current.as_ref() != Some(value) {
                return false;
            }
        }
        if self.fill.is_some_and(|value| !same_optional(value, props.fill)) {
            return false;
        }
        if self.border_color.is_some_and(|value| !same_optional(value, props.border_color)) {
            return false;
        }
        if self.border_shape.is_some_and(|value| props.border_shape.map(BorderShapeWire::from) != Some(value)) {
            return false;
        }
        true
    }

    /// The engine's one all-or-nothing restyle for this update.
    pub fn to_restyle(&self) -> Restyle {
        Restyle {
            color: self.color,
            opacity: self.opacity,
            width: self.width,
            endings: self.endings.as_ref().map(|ends| ends.iter().copied().map(Into::into).collect()),
            fill: self.fill,
            border_color: self.border_color,
            border_shape: self.border_shape.map(Into::into),
        }
    }
}

fn finite_field(value: &Value, field: &'static str) -> Result<f64, WireError> {
    value.as_f64().filter(|value| value.is_finite()).ok_or_else(|| WireError::Invalid { field, why: "expected a finite number".into() })
}

fn font_size_field(value: &Value) -> Result<f64, WireError> {
    let number = finite_field(value, "font size")?;
    if (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&number) {
        Ok(number)
    } else {
        Err(WireError::Invalid { field: "font size", why: format!("must be {MIN_FONT_SIZE}\u{2013}{MAX_FONT_SIZE} pt") })
    }
}

fn endings_field(value: &Value) -> Result<Vec<EndingWire>, WireError> {
    let entries = value.as_array().ok_or_else(|| WireError::Invalid { field: "endings", why: "expected a list of ending names".into() })?;
    if entries.len() != 1 && entries.len() != 2 {
        return Err(WireError::Invalid { field: "endings", why: "one ending (a callout) or two (a line)".into() });
    }
    entries
        .iter()
        .map(|entry| {
            let name = entry.as_str().ok_or_else(|| WireError::Invalid { field: "endings", why: "each ending is a name".into() })?;
            EndingWire::parse(name)
        })
        .collect()
}

fn nullable_text_field(value: &Value, field: &'static str, max: usize) -> Result<Option<String>, WireError> {
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or_else(|| WireError::Invalid { field, why: "expected a string or null".into() })?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(WireError::Invalid { field, why: "must not be empty (use null to take it away)".into() });
    }
    if text.len() > max {
        return Err(WireError::Invalid { field, why: format!("at most {max} bytes") });
    }
    Ok(Some(trimmed.to_owned()))
}

/// The final font size of a text box, typewriter text or callout, as
/// `annotation.font_size` sets it (RFC §3.1.3).
///
/// One number, one version, one undo step: the engine's
/// `Edit::FontSizeAnnotation` regrows the box for the wrapped text, and the
/// accepted operation carries the size it replaced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontSizeUpdate {
    pub size: f64,
}

impl FontSizeUpdate {
    /// The body of an `annotation.font_size`: `{"action": …, "size": …}`.
    pub fn to_payload(&self) -> Value {
        json!({ "action": "annotation.font_size", "size": self.size })
    }

    /// Read the body of an `annotation.font_size`.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let size = payload.get("size").ok_or_else(|| WireError::Malformed("the update names no font size".into()))?;
        Ok(Self { size: font_size_field(size)? })
    }

    /// Read the `expect` of an `annotation.font_size`: the size the undo left.
    pub fn from_expect(value: &Value) -> Result<Self, WireError> {
        Ok(Self { size: font_size_field(value)? })
    }

    /// Refuse a comment that has no font of its own, before anything changes.
    pub fn check_for(&self, props: &Props) -> Result<(), WireError> {
        if props.locked {
            return Err(WireError::NotRestylable("a locked comment"));
        }
        if props.font_size.is_none() {
            return Err(WireError::Invalid { field: "font size", why: "only text boxes, typewriter text and callouts have one".into() });
        }
        Ok(())
    }

    /// The size the comment had — what the accepted operation carries as
    /// `before` and what an undo sets again.
    pub fn before(&self, props: &Props) -> Option<f64> {
        props.font_size
    }

    /// Whether the comment's font is still the one `expect` names.
    pub fn still_matches(&self, props: &Props) -> bool {
        props.font_size.is_some_and(|current| (self.size - current).abs() <= STYLE_EPSILON)
    }
}

/// The final author, subject and note icon of an existing comment, as
/// `annotation.info` sets them (RFC §3.1.3).
///
/// `None` leaves a property as it is; naming a property with an empty value is
/// refused, so an operation never wipes an author or a subject by accident. The
/// engine's `Edit::SetAnnotationInfo` writes `/T`, `/Subj` and, for a sticky
/// note, `/Name` (redrawing the icon), all-or-nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InfoUpdate {
    /// `Some(None)` takes the author away; `Some(Some(name))` sets it.
    pub author: Option<Option<String>>,
    /// `Some(None)` takes the subject away.
    pub subject: Option<Option<String>>,
    pub icon: Option<IconWire>,
}

impl InfoUpdate {
    /// Whether the update names no property at all.
    pub fn is_empty(&self) -> bool {
        self.author.is_none() && self.subject.is_none() && self.icon.is_none()
    }

    /// The body of an `annotation.info`: `{"action": …, "info": {…}}`.
    pub fn to_payload(&self) -> Value {
        json!({ "action": "annotation.info", "info": self.to_value() })
    }

    /// The `info` object on its own, as the accepted operation's `info` and
    /// `before` write it: `null` is a property the comment did not have.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        if let Some(author) = &self.author {
            object.insert("author".into(), author.as_ref().map_or(Value::Null, |author| json!(author)));
        }
        if let Some(subject) = &self.subject {
            object.insert("subject".into(), subject.as_ref().map_or(Value::Null, |subject| json!(subject)));
        }
        if let Some(icon) = self.icon {
            object.insert("icon".into(), json!(icon.name()));
        }
        Value::Object(object)
    }

    /// Read the `info` object of an `annotation.info`.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let info = payload.get("info").filter(|value| !value.is_null()).ok_or_else(|| WireError::Malformed("the update names no info".into()))?;
        Self::from_value(info)
    }

    /// Read an info object: the properties present are the ones the update sets
    /// — a string sets it, `null` takes it away. An empty string is refused, so
    /// a request never wipes an author or a subject by accident; only an explicit
    /// `null` (the undo of a property the comment did not have) does.
    pub fn from_value(info: &Value) -> Result<Self, WireError> {
        let object = info.as_object().ok_or_else(|| WireError::Malformed("the info must be an object".into()))?;
        let mut update = Self::default();
        for (key, field) in object {
            match key.as_str() {
                "author" => update.author = Some(nullable_text_field(field, "author", MAX_AUTHOR_BYTES)?),
                "subject" => update.subject = Some(nullable_text_field(field, "subject", MAX_AUTHOR_BYTES)?),
                "icon" => {
                    let name = field.as_str().ok_or_else(|| WireError::Invalid { field: "icon", why: "expected a name".into() })?;
                    update.icon = Some(IconWire::parse(name)?);
                }
                other => return Err(WireError::Invalid { field: "info", why: format!("{other:?} is not a comment property") }),
            }
        }
        if update.is_empty() {
            return Err(WireError::Malformed("the info names no property".into()));
        }
        Ok(update)
    }

    /// Refuse properties the comment does not have, before anything changes.
    pub fn check_for(&self, props: &Props) -> Result<(), WireError> {
        if props.locked {
            return Err(WireError::NotRestylable("a locked comment"));
        }
        if self.icon.is_some() && props.icon.is_none() {
            return Err(WireError::Invalid { field: "icon", why: "only sticky notes have an icon".into() });
        }
        Ok(())
    }

    /// The properties the comment had for exactly the ones this update sets —
    /// what the accepted operation carries as `before` and what an undo sets
    /// back. A comment without `/T` records `null`, so the undo removes the name
    /// again; a note without `/Name` reads the icon it draws (`Note`).
    pub fn before(&self, props: &Props) -> Self {
        let value = |current: &str, change: bool| {
            change.then(|| {
                let current = current.trim();
                (!current.is_empty()).then(|| current.to_owned())
            })
        };
        Self {
            author: value(&props.author, self.author.is_some()),
            subject: value(&props.subject, self.subject.is_some()),
            icon: self.icon.map(|_| IconWire::from(props.icon.unwrap_or(NoteIcon::Note))),
        }
    }

    /// Whether the comment still has the properties `expect` names: an undo is
    /// a compare-and-set, so a later edit by someone else is a conflict.
    pub fn still_matches(&self, props: &Props) -> bool {
        let matches = |wanted: &Option<String>, current: &str| match wanted {
            Some(name) => name == current.trim(),
            None => current.trim().is_empty(),
        };
        if self.author.as_ref().is_some_and(|wanted| !matches(wanted, &props.author)) {
            return false;
        }
        if self.subject.as_ref().is_some_and(|wanted| !matches(wanted, &props.subject)) {
            return false;
        }
        if let Some(icon) = self.icon {
            return props.icon.is_some_and(|current| IconWire::from(current) == icon);
        }
        true
    }

    /// What the engine writes: `/T`, `/Subj` (an empty name removes the entry)
    /// and a note's `/Name`.
    pub fn apply_to(&self) -> (Option<String>, Option<String>, Option<NoteIcon>) {
        let name = |value: &Option<Option<String>>| value.clone().map(|name| name.unwrap_or_default());
        (name(&self.author), name(&self.subject), self.icon.map(Into::into))
    }
}

fn unit_field(value: &Value, field: &'static str) -> Result<f64, WireError> {
    let number = finite_field(value, field)?;
    if (0.0..=1.0).contains(&number) { Ok(number) } else { Err(WireError::Invalid { field, why: "must be between 0 and 1".into() }) }
}

// ── Part opacity, a move and the geometry of a comment ───────────────────────

/// How close two geometry numbers must be to count as the same value: the engine
/// writes three decimals, and a move adds its offset to the numbers it read.
const GEOMETRY_EPSILON: f64 = 1e-3;

/// A comment's `/Rect` corners in order, at least 1 pt wide and high: what a
/// move's compare-and-set compares and what a resize names.
fn rect_field(value: &Value, field: &'static str) -> Result<[f64; 4], WireError> {
    let entries =
        value.as_array().filter(|entries| entries.len() == 4).ok_or_else(|| WireError::Invalid { field, why: "expected [x0, y0, x1, y1]".into() })?;
    let mut numbers = [0.0; 4];
    for (slot, entry) in numbers.iter_mut().zip(entries) {
        *slot = finite_field(entry, field)?;
    }
    let rect = [numbers[0].min(numbers[2]), numbers[1].min(numbers[3]), numbers[0].max(numbers[2]), numbers[1].max(numbers[3])];
    if rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(WireError::Invalid { field, why: "at least 1 pt wide and high".into() });
    }
    Ok(rect)
}

/// A comment's points in user space: a line's two ends, a polygon's corners or a
/// callout's arrow point (and knee). Bounded by [`MAX_TOTAL_POINTS`].
fn points_field(value: &Value, field: &'static str) -> Result<Vec<[f64; 2]>, WireError> {
    let entries = value.as_array().ok_or_else(|| WireError::Invalid { field, why: "expected a list of [x, y] points".into() })?;
    if entries.is_empty() {
        return Err(WireError::Invalid { field, why: "at least one point".into() });
    }
    if entries.len() > MAX_TOTAL_POINTS {
        return Err(WireError::TooMany { what: "points", count: entries.len(), max: MAX_TOTAL_POINTS });
    }
    let mut points = Vec::with_capacity(entries.len());
    for entry in entries {
        let pair = entry.as_array().filter(|pair| pair.len() == 2).ok_or_else(|| WireError::Invalid { field, why: "each point is [x, y]".into() })?;
        points.push([finite_field(&pair[0], field)?, finite_field(&pair[1], field)?]);
    }
    Ok(points)
}

/// A comment's `/Rect` corners from a wire value, as `annotation.move`'s
/// `expect` names them: four finite numbers, at least 1 pt wide and high.
pub fn rect_from_value(value: &Value) -> Result<[f64; 4], WireError> {
    rect_field(value, "expect")
}

/// Whether a rect still is what `expect` names, within [`GEOMETRY_EPSILON`].
pub fn rect_matches(expect: [f64; 4], current: Option<[f64; 4]>) -> bool {
    current.is_some_and(|current| (0..4).all(|i| (expect[i] - current[i]).abs() <= GEOMETRY_EPSILON))
}

fn same_points(expect: &[[f64; 2]], current: &[[f64; 2]]) -> bool {
    expect.len() == current.len() && expect.iter().zip(current).all(|(a, b)| (0..2).all(|i| (a[i] - b[i]).abs() <= GEOMETRY_EPSILON))
}

/// Whether a drag moves `props`: text markup and the caret of a Replace Text
/// proposal stay where their text is (keep in step with the comment tool).
fn on_text(props: &Props) -> bool {
    matches!(props.subtype.as_str(), "Highlight" | "Underline" | "StrikeOut" | "Squiggly")
        || (props.subtype == "Caret" && props.intent.as_deref() == Some("Replace"))
}

/// The fill's and the text's own opacities of an existing comment, as
/// `annotation.parts` sets them (RFC §3.1.6).
///
/// `annotation.update` sets the whole comment's opacity; this operation sets a
/// part's, the line or border keeping the comment's. It is one addressable
/// operation over the engine's `Edit::PartOpacity`, and the same compare-and-set
/// undo: the accepted operation carries `before` (the numbers it replaced).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PartsUpdate {
    /// A shape's or text box's fill opacity (0–1); `None` leaves it as it is.
    pub fill: Option<f64>,
    /// A text box's, typewriter text's or callout's text opacity.
    pub text: Option<f64>,
}

impl PartsUpdate {
    /// Whether the update names no part at all.
    pub fn is_empty(&self) -> bool {
        self.fill.is_none() && self.text.is_none()
    }

    /// The body of an `annotation.parts`: `{"action": …, "parts": {…}}`.
    pub fn to_payload(&self) -> Value {
        json!({ "action": "annotation.parts", "parts": self.to_value() })
    }

    /// The `parts` object on its own, as the accepted operation's `parts`,
    /// `before` and `expect` write it.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        if let Some(fill) = self.fill {
            object.insert("fill".into(), json!(fill));
        }
        if let Some(text) = self.text {
            object.insert("text".into(), json!(text));
        }
        Value::Object(object)
    }

    /// Read the `parts` object of an `annotation.parts`.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let parts = payload.get("parts").filter(|value| !value.is_null()).ok_or_else(|| WireError::Malformed("the update names no part".into()))?;
        Self::from_value(parts)
    }

    /// Read a `parts` object: the numbers present are the ones the update sets.
    pub fn from_value(parts: &Value) -> Result<Self, WireError> {
        let object = parts.as_object().ok_or_else(|| WireError::Malformed("the parts must be an object".into()))?;
        let mut update = Self::default();
        for (key, field) in object {
            match key.as_str() {
                "fill" => update.fill = Some(unit_field(field, "fill opacity")?),
                "text" => update.text = Some(unit_field(field, "text opacity")?),
                other => return Err(WireError::Invalid { field: "parts", why: format!("{other:?} is not a part of a comment") }),
            }
        }
        if update.is_empty() {
            return Err(WireError::Malformed("the parts name no opacity".into()));
        }
        Ok(update)
    }

    /// Refuse a part the comment does not draw, before anything changes.
    pub fn check_for(&self, props: &Props) -> Result<(), WireError> {
        if props.locked {
            return Err(WireError::NotRestylable("a locked comment"));
        }
        if !props.restylable {
            return Err(WireError::NotRestylable("this comment's appearance"));
        }
        if self.fill.is_some() && !props.fillable {
            return Err(WireError::Invalid { field: "fill opacity", why: "this comment has no fill".into() });
        }
        if self.text.is_some() && props.text_opacity.is_none() {
            return Err(WireError::Invalid {
                field: "text opacity",
                why: "only text boxes, typewriter text and callouts have text of their own".into(),
            });
        }
        Ok(())
    }

    /// The opacities the comment had for exactly the parts this update sets —
    /// what the accepted operation carries as `before` and what an undo sets back.
    pub fn before(&self, props: &Props) -> Self {
        Self { fill: self.fill.and(props.fill_opacity), text: self.text.and(props.text_opacity) }
    }

    /// Whether the comment's parts still are the ones `expect` names.
    pub fn still_matches(&self, props: &Props) -> bool {
        if self.fill.is_some_and(|value| props.fill_opacity.is_none_or(|current| (value - current).abs() > STYLE_EPSILON)) {
            return false;
        }
        if self.text.is_some_and(|value| props.text_opacity.is_none_or(|current| (value - current).abs() > STYLE_EPSILON)) {
            return false;
        }
        true
    }

    /// What the engine's `Edit::PartOpacity` takes: `None` leaves a part as it is.
    pub fn apply_to(&self) -> (Option<f64>, Option<f64>) {
        (self.fill, self.text)
    }
}

/// The final geometry of an existing comment, as `annotation.geometry` sets it
/// (RFC §3.1.6): a resize (a rectangle) or a reshape (points).
///
/// The rectangle is the frame Comment Properties and the comment bar resize — a
/// callout's text box, else the comment's `/Rect` — because the engine's
/// `Edit::ResizeAnnotation` takes exactly that. The accepted operation carries
/// `before` and `after` (the frames the engine drew around the edit), so an undo
/// restores the old frame and refuses if someone moved the comment since.
#[derive(Debug, Clone, PartialEq)]
pub enum GeometryUpdate {
    /// Where a resize puts the comment.
    Rect([f64; 4]),
    /// A line's two ends, connected lines' or a polygon's corners, or a callout's
    /// arrow point and knee.
    Points(Vec<[f64; 2]>),
}

impl GeometryUpdate {
    /// The body of an `annotation.geometry`: `{"action": …, "geometry": {…}}`.
    pub fn to_payload(&self) -> Value {
        json!({ "action": "annotation.geometry", "geometry": self.to_value() })
    }

    /// The `geometry` object on its own, as the accepted operation's `geometry`,
    /// `before`, `after` and `expect` write it.
    pub fn to_value(&self) -> Value {
        match self {
            Self::Rect(rect) => json!({ "rect": rect }),
            Self::Points(points) => json!({ "points": points }),
        }
    }

    /// Read the `geometry` object of an `annotation.geometry`.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let geometry =
            payload.get("geometry").filter(|value| !value.is_null()).ok_or_else(|| WireError::Malformed("the update names no geometry".into()))?;
        Self::from_value(geometry)
    }

    /// Read a geometry object: exactly one of `rect` and `points`.
    pub fn from_value(geometry: &Value) -> Result<Self, WireError> {
        let object = geometry.as_object().ok_or_else(|| WireError::Malformed("the geometry must be an object".into()))?;
        let rect = object.get("rect");
        let points = object.get("points");
        if let Some(other) = object.keys().find(|key| *key != "rect" && *key != "points") {
            return Err(WireError::Invalid { field: "geometry", why: format!("{other:?} is not a geometry of a comment") });
        }
        match (rect, points) {
            (Some(rect), None) => Ok(Self::Rect(rect_field(rect, "rect")?)),
            (None, Some(points)) => Ok(Self::Points(points_field(points, "points")?)),
            (Some(_), Some(_)) => Err(WireError::Invalid { field: "geometry", why: "a rect or points, not both".into() }),
            (None, None) => Err(WireError::Malformed("the geometry names no rect or points".into())),
        }
    }

    /// The rectangle a resize names, if this is one.
    pub fn rect(&self) -> Option<[f64; 4]> {
        match self {
            Self::Rect(rect) => Some(*rect),
            Self::Points(_) => None,
        }
    }

    /// The points a reshape names, if this is one.
    pub fn points(&self) -> &[[f64; 2]] {
        match self {
            Self::Rect(_) => &[],
            Self::Points(points) => points,
        }
    }

    /// Refuse a comment this geometry cannot be set on, before anything changes.
    pub fn check_for(&self, props: &Props) -> Result<(), WireError> {
        if props.locked {
            return Err(WireError::NotRestylable("a locked comment"));
        }
        match self {
            Self::Rect(_) => {
                if !matches!(props.subtype.as_str(), "Square" | "Circle" | "FreeText" | "Stamp" | "Ink") {
                    return Err(WireError::Invalid { field: "rect", why: format!("{} comments can't be resized", props.subtype) });
                }
                if props.frame.is_none() {
                    return Err(WireError::Invalid { field: "rect", why: "the comment's rectangle can't be read".into() });
                }
            }
            Self::Points(points) => {
                if props.intent.as_deref().is_some_and(|intent| intent.ends_with("Dimension")) {
                    return Err(WireError::NotReshapable("a measurement"));
                }
                if props.points.is_empty() {
                    return Err(WireError::NotReshapable("this comment's points"));
                }
                match (props.subtype.as_str(), points.len()) {
                    ("Line", 2) | ("PolyLine" | "Polygon", 2..) | ("FreeText", 1 | 2) => {}
                    ("Line", _) => return Err(WireError::Invalid { field: "points", why: "a line has two ends".into() }),
                    ("FreeText", _) => {
                        return Err(WireError::Invalid { field: "points", why: "a callout has an arrow point and, optionally, a knee".into() });
                    }
                    ("PolyLine" | "Polygon", _) => return Err(WireError::Invalid { field: "points", why: "at least two corners".into() }),
                    (other, _) => {
                        return Err(WireError::Invalid { field: "points", why: format!("{other} comments have no points to reshape") });
                    }
                }
            }
        }
        Ok(())
    }

    /// The geometry the comment had — what the accepted operation carries as
    /// `before` and what an undo sets again.
    pub fn before(&self, props: &Props) -> Option<Self> {
        match self {
            Self::Rect(_) => props.frame.map(Self::Rect),
            Self::Points(_) => (!props.points.is_empty()).then(|| Self::Points(props.points.clone())),
        }
    }

    /// The geometry the comment has now, in this update's shape: what the
    /// accepted operation carries as `after` (an undo's compare-and-set).
    pub fn observed(&self, props: &Props) -> Option<Self> {
        self.before(props)
    }

    /// Whether the comment's geometry still is the one `expect` names.
    pub fn still_matches(&self, props: &Props) -> bool {
        match self {
            Self::Rect(rect) => rect_matches(*rect, props.frame),
            Self::Points(points) => same_points(points, &props.points),
        }
    }
}

/// A move of an existing comment by `(dx, dy)` points, as `annotation.move`
/// sets it (RFC §3.1.6).
///
/// The engine's `Edit::MoveAnnotation` shifts the comment, its pop-up and its
/// points; the accepted operation carries `before` and `after` (the comment's
/// `/Rect` around the move), so the undo is a move by the opposite offset that
/// refuses if someone moved the comment since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveUpdate {
    pub dx: f64,
    pub dy: f64,
}

impl MoveUpdate {
    /// The body of an `annotation.move`: `{"action": …, "dx": …, "dy": …}`.
    pub fn to_payload(&self) -> Value {
        json!({ "action": "annotation.move", "dx": self.dx, "dy": self.dy })
    }

    /// Read the body of an `annotation.move`.
    pub fn from_payload(payload: &Value) -> Result<Self, WireError> {
        let dx = payload.get("dx").ok_or_else(|| WireError::Malformed("the move names no dx".into()))?;
        let dy = payload.get("dy").ok_or_else(|| WireError::Malformed("the move names no dy".into()))?;
        Ok(Self { dx: finite_field(dx, "dx")?, dy: finite_field(dy, "dy")? })
    }

    /// Refuse a comment that does not move with a drag, before anything changes.
    pub fn check_for(&self, props: &Props) -> Result<(), WireError> {
        if props.locked {
            return Err(WireError::NotRestylable("a locked comment"));
        }
        if on_text(props) {
            return Err(WireError::NotMovable("text markup"));
        }
        if props.rect.is_none() {
            return Err(WireError::Invalid { field: "rect", why: "the comment's rectangle can't be read".into() });
        }
        Ok(())
    }

    /// The `/Rect` the comment had — what the accepted operation carries as
    /// `before`, and what an undo moves back to.
    pub fn before(&self, props: &Props) -> Option<[f64; 4]> {
        props.rect
    }

    /// The `/Rect` a move of `rect` leaves: a move shifts all four numbers.
    pub fn after(&self, rect: [f64; 4]) -> [f64; 4] {
        [rect[0] + self.dx, rect[1] + self.dy, rect[2] + self.dx, rect[3] + self.dy]
    }
}

fn width_field(value: &Value) -> Result<f64, WireError> {
    let number = finite_field(value, "width")?;
    if number >= 0.0 { Ok(number) } else { Err(WireError::Invalid { field: "width", why: "must be a non-negative number".into() }) }
}

fn color_field(value: &Value, field: &'static str) -> Result<[f64; 3], WireError> {
    let entries =
        value.as_array().filter(|entries| entries.len() == 3).ok_or_else(|| WireError::Invalid { field, why: "expected [r, g, b]".into() })?;
    let mut color = [0.0; 3];
    for (slot, entry) in color.iter_mut().zip(entries) {
        *slot = unit_field(entry, field)?;
    }
    Ok(color)
}

fn optional_color_field(value: &Value, field: &'static str) -> Result<Option<[f64; 3]>, WireError> {
    if value.is_null() { Ok(None) } else { color_field(value, field).map(Some) }
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

impl EndingWire {
    /// The wire name, as `#[serde(rename_all = "snake_case")]` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Square => "square",
            Self::Circle => "circle",
            Self::Diamond => "diamond",
            Self::OpenArrow => "open_arrow",
            Self::ClosedArrow => "closed_arrow",
            Self::Butt => "butt",
            Self::ROpenArrow => "r_open_arrow",
            Self::RClosedArrow => "r_closed_arrow",
            Self::Slash => "slash",
        }
    }

    /// Read the wire name; anything else is refused rather than guessed.
    pub fn parse(name: &str) -> Result<Self, WireError> {
        match name {
            "none" => Ok(Self::None),
            "square" => Ok(Self::Square),
            "circle" => Ok(Self::Circle),
            "diamond" => Ok(Self::Diamond),
            "open_arrow" => Ok(Self::OpenArrow),
            "closed_arrow" => Ok(Self::ClosedArrow),
            "butt" => Ok(Self::Butt),
            "r_open_arrow" => Ok(Self::ROpenArrow),
            "r_closed_arrow" => Ok(Self::RClosedArrow),
            "slash" => Ok(Self::Slash),
            other => Err(WireError::Invalid { field: "endings", why: format!("{other:?} is not a line ending") }),
        }
    }
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

impl BorderShapeWire {
    /// The wire name, as `#[serde(rename_all = "snake_case")]` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rectangle => "rectangle",
            Self::Oval => "oval",
            Self::Circle => "circle",
        }
    }

    /// Read the wire name; anything else is refused rather than guessed.
    pub fn parse(name: &str) -> Result<Self, WireError> {
        match name {
            "rectangle" => Ok(Self::Rectangle),
            "oval" => Ok(Self::Oval),
            "circle" => Ok(Self::Circle),
            other => Err(WireError::Invalid { field: "border shape", why: format!("{other:?} is not rectangle, oval or circle") }),
        }
    }
}

/// A review state a reply may carry (`/State`, `/StateModel /Review`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStateWire {
    None,
    Accepted,
    Rejected,
    Cancelled,
    Completed,
}

impl ReviewStateWire {
    /// The wire name, as `#[serde(rename_all = "snake_case")]` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
        }
    }

    /// Read a review state by name; anything else is refused.
    pub fn parse(name: &str) -> Result<Self, WireError> {
        match name {
            "none" => Ok(Self::None),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "cancelled" => Ok(Self::Cancelled),
            "completed" => Ok(Self::Completed),
            other => Err(WireError::Invalid { field: "state", why: format!("{other:?} is not a review state") }),
        }
    }
}

impl From<ReviewStateWire> for ReviewState {
    fn from(value: ReviewStateWire) -> Self {
        match value {
            ReviewStateWire::None => Self::None,
            ReviewStateWire::Accepted => Self::Accepted,
            ReviewStateWire::Rejected => Self::Rejected,
            ReviewStateWire::Cancelled => Self::Cancelled,
            ReviewStateWire::Completed => Self::Completed,
        }
    }
}

impl From<ReviewState> for ReviewStateWire {
    fn from(value: ReviewState) -> Self {
        match value {
            ReviewState::None => Self::None,
            ReviewState::Accepted => Self::Accepted,
            ReviewState::Rejected => Self::Rejected,
            ReviewState::Cancelled => Self::Cancelled,
            ReviewState::Completed => Self::Completed,
        }
    }
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

impl IconWire {
    /// The wire name, as `#[serde(rename_all = "snake_case")]` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Note => "note",
            Self::Help => "help",
            Self::Insert => "insert",
            Self::Key => "key",
            Self::NewParagraph => "new_paragraph",
            Self::Paragraph => "paragraph",
            Self::Check => "check",
            Self::Circle => "circle",
            Self::Cross => "cross",
            Self::CrossHairs => "cross_hairs",
            Self::Star => "star",
            Self::RightArrow => "right_arrow",
            Self::UpArrow => "up_arrow",
        }
    }

    /// Read the wire name; anything else is refused rather than guessed.
    pub fn parse(name: &str) -> Result<Self, WireError> {
        match name {
            "comment" => Ok(Self::Comment),
            "note" => Ok(Self::Note),
            "help" => Ok(Self::Help),
            "insert" => Ok(Self::Insert),
            "key" => Ok(Self::Key),
            "new_paragraph" => Ok(Self::NewParagraph),
            "paragraph" => Ok(Self::Paragraph),
            "check" => Ok(Self::Check),
            "circle" => Ok(Self::Circle),
            "cross" => Ok(Self::Cross),
            "cross_hairs" => Ok(Self::CrossHairs),
            "star" => Ok(Self::Star),
            "right_arrow" => Ok(Self::RightArrow),
            "up_arrow" => Ok(Self::UpArrow),
            other => Err(WireError::Invalid { field: "icon", why: format!("{other:?} is not a note icon") }),
        }
    }
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
