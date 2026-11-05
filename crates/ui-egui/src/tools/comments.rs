//! Commenting in the document view (execution plan M5.3–M5.4; Acrobat's "Add comments").
//!
//! - **Tools** ([`CommentTool`]): sticky note, text box, highlight / underline / strikethrough
//!   (driven by text selection), freehand, line, arrow, rectangle, oval. They live in the quick
//!   bar in three flyout groups, as in Acrobat's comment toolbar (audit `a14-*`).
//! - **On the page:** drawing gestures with live previews; with the Select tool, comments can be
//!   hovered, selected (blue frame with 8 round handles), moved, resized (rectangles, ovals and
//!   text boxes), edited (double-click) and deleted (⌫ / Delete); a context menu offers Edit,
//!   Reply, Set status, Colour and Delete.
//! - **Composer:** the floating "Add a comment" card used for new notes, text boxes and edits.
//!
//! Everything is turned into `pdfcraft_engine::Edit`s, queued on the view as `pending_edit` and
//! applied by the app, so each change is one undo step.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::{Edit, LineEnding, Markup, NewAnnotation, NoteIcon, ReviewState, Rgb, Shape, Style};
use pdfcraft_render::{Annotation, DocInfo, PageInfo, PageText};

use crate::canvas::{DocView, PageXform};
use crate::page_pointer::PagePointer;
use crate::theme::{self, Tokens};
use crate::{QuickTool, icons};

mod composer;
mod geometry;
mod input;
mod menu;
mod paint;
mod state;

pub(crate) use composer::{composer, submit_key};
#[cfg(test)]
use geometry::handle_pos;
#[cfg(test)]
pub(crate) use geometry::{caret_snap, snapped_caret};
pub(crate) use geometry::{corner_ratio, page_rotation, resized};
pub use geometry::{drawn_text_box_rect, text_box_rect};
#[cfg(test)]
use input::hover_tip;
pub(crate) use input::{describe, fill_grabs, hit, keys, nudge_keys, page_after_text, page_input};
pub use menu::{CanvasAction, fill_picker, recent_custom_colours, remember_custom_colour, status_badge, swatch_grid};
#[cfg(test)]
use menu::{RECENT_CUSTOM, same_colour};
pub(crate) use menu::{attachment_items, context_menu, quick_bar_controls};
pub(crate) use paint::paint_page;
#[cfg(test)]
use paint::{changing_points, drawn_shape, edited_in_view};
#[cfg(test)]
use state::after_delete;
use state::login_name;
pub use state::{CommentView, Composer, ComposerKind, Gesture, SortBy};
pub(crate) use state::{EditorFocus, PageCx};

/// Acrobat's selection blue for annotation frames and handles (11-visual-spec §5).
const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// Maximum stroke thickness offered by commenting controls, in PDF points.
pub const MAX_LINE_WIDTH: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommentTool {
    Note,
    TextBox,
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Ink,
    FreehandHighlight,
    Line,
    Arrow,
    Rectangle,
    Oval,
    Polygon,
    PolyLine,
    Cloud,
    Callout,
    Caret,
    ReplaceText,
    Attach,
    Eraser,
}

/// The quick-bar flyout groups, in Acrobat's order: Comment ▸, Highlight ▸, Draw ▸.
pub const GROUPS: [&[CommentTool]; 3] = [
    &[CommentTool::Note, CommentTool::TextBox, CommentTool::Callout, CommentTool::Attach],
    &[
        CommentTool::Highlight,
        CommentTool::FreehandHighlight,
        CommentTool::Underline,
        CommentTool::StrikeOut,
        CommentTool::Squiggly,
        CommentTool::Caret,
        CommentTool::ReplaceText,
    ],
    &[
        CommentTool::Ink,
        CommentTool::Line,
        CommentTool::Arrow,
        CommentTool::Rectangle,
        CommentTool::Oval,
        CommentTool::PolyLine,
        CommentTool::Polygon,
        CommentTool::Cloud,
        CommentTool::Eraser,
    ],
];

pub const ALL: [CommentTool; 20] = [
    CommentTool::FreehandHighlight,
    CommentTool::Eraser,
    CommentTool::ReplaceText,
    CommentTool::Attach,
    CommentTool::Polygon,
    CommentTool::PolyLine,
    CommentTool::Cloud,
    CommentTool::Callout,
    CommentTool::Caret,
    CommentTool::Note,
    CommentTool::TextBox,
    CommentTool::Highlight,
    CommentTool::Underline,
    CommentTool::StrikeOut,
    CommentTool::Squiggly,
    CommentTool::Ink,
    CommentTool::Line,
    CommentTool::Arrow,
    CommentTool::Rectangle,
    CommentTool::Oval,
];

impl CommentTool {
    /// The command registry identifier (`"comment.*"`) corresponding to this tool.
    pub fn command(self) -> &'static str {
        match self {
            Self::Note => "comment.note",
            Self::TextBox => "comment.freetext",
            Self::Highlight => "comment.highlight",
            Self::Underline => "comment.underline",
            Self::StrikeOut => "comment.strikeout",
            Self::Squiggly => "comment.squiggly",
            Self::FreehandHighlight => "comment.freehand_highlight",
            Self::Ink => "comment.ink",
            Self::Line => "comment.line",
            Self::Arrow => "comment.arrow",
            Self::Rectangle => "comment.square",
            Self::Oval => "comment.circle",
            Self::Polygon => "comment.polygon",
            Self::PolyLine => "comment.polyline",
            Self::Cloud => "comment.cloud",
            Self::Callout => "comment.callout",
            Self::Caret => "comment.caret",
            Self::ReplaceText => "comment.replace",
            Self::Attach => "comment.attach",
            Self::Eraser => "comment.eraser",
        }
    }

    /// Resolve a [`CommentTool`] from its command registry identifier.
    pub fn from_command(id: &str) -> Option<Self> {
        ALL.into_iter().find(|t| t.command() == id)
    }

    /// User-facing label for this commenting tool.
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Add a sticky note",
            Self::TextBox => "Add a text box",
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "Strikethrough",
            Self::Squiggly => "Squiggly underline",
            Self::FreehandHighlight => "Freehand-Highlight",
            Self::Ink => "Draw",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Oval => "Oval",
            Self::Polygon => "Polygon",
            Self::PolyLine => "Connected lines",
            Self::Cloud => "Cloud",
            Self::Callout => "Add a callout",
            Self::Caret => "Insert text",
            Self::ReplaceText => "Replace text",
            Self::Attach => "Attach file",
            Self::Eraser => "Eraser",
        }
    }

    /// Lucide icon name for this commenting tool.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Note => "message-square-plus",
            Self::TextBox => "type",
            Self::Highlight => "highlighter",
            Self::Underline => "underline",
            Self::StrikeOut => "strikethrough",
            Self::Squiggly => "spline",
            Self::FreehandHighlight => "highlighter",
            Self::Ink => "pencil",
            Self::Line => "minus",
            Self::Arrow => "move-right",
            Self::Rectangle => "square",
            Self::Oval => "circle",
            Self::Polygon => "pentagon",
            Self::PolyLine => "spline",
            Self::Cloud => "cloud",
            Self::Callout => "message-square-quote",
            Self::Caret => "text-cursor-input",
            Self::ReplaceText => "replace",
            Self::Attach => "paperclip",
            Self::Eraser => "eraser",
        }
    }

    pub fn group(self) -> usize {
        GROUPS.iter().position(|g| g.contains(&self)).unwrap_or(0)
    }

    pub fn markup(self) -> Option<Markup> {
        match self {
            Self::Highlight => Some(Markup::Highlight),
            Self::Underline => Some(Markup::Underline),
            Self::StrikeOut => Some(Markup::StrikeOut),
            Self::Squiggly => Some(Markup::Squiggly),
            _ => None,
        }
    }

    /// Tools that draw with a drag gesture.
    pub fn draws(self) -> bool {
        matches!(self, Self::Ink | Self::FreehandHighlight | Self::Line | Self::Arrow | Self::Rectangle | Self::Oval | Self::Eraser)
    }

    /// Tools that place points one click at a time (double-click or Enter finishes).
    pub fn clicks_points(self) -> bool {
        matches!(self, Self::Polygon | Self::PolyLine | Self::Cloud)
    }

    /// Whether the line-thickness control applies.
    pub fn has_width(self) -> bool {
        self.draws() || self.clicks_points() || matches!(self, Self::TextBox | Self::Callout)
    }

    /// Text comments may have no border while retaining visible text or fill.
    fn min_width(self) -> f64 {
        if matches!(self, Self::TextBox | Self::Callout) { 0.0 } else { 0.5 }
    }

    /// Whether the fill-colour control applies to this tool.
    pub fn has_fill(self) -> bool {
        matches!(self, Self::Rectangle | Self::Oval | Self::Polygon | Self::Cloud | Self::TextBox | Self::Callout)
    }

    /// A placeholder shape of this kind (for per-tool default styles).
    fn sample(self) -> Shape {
        match self {
            Self::Note => Shape::Note { at: [0.0; 2], icon: NoteIcon::Comment },
            Self::TextBox => Shape::TextBox { rect: [0.0; 4], font_size: 12.0 },
            Self::Highlight | Self::Underline | Self::StrikeOut | Self::Squiggly => {
                Shape::TextMarkup { kind: self.markup().unwrap_or(Markup::Highlight), quads: Vec::new() }
            }
            Self::FreehandHighlight => Shape::FreehandHighlight { strokes: Vec::new() },
            Self::Ink => Shape::Ink { strokes: Vec::new() },
            Self::Line => Shape::Line { from: [0.0; 2], to: [0.0; 2], start: LineEnding::None, end: LineEnding::None },
            Self::Arrow => Shape::Line { from: [0.0; 2], to: [0.0; 2], start: LineEnding::None, end: LineEnding::OpenArrow },
            Self::Rectangle => Shape::Rectangle { rect: [0.0; 4] },
            Self::Oval => Shape::Oval { rect: [0.0; 4] },
            Self::Polygon => Shape::Polygon { vertices: Vec::new(), cloud: false },
            Self::Cloud => Shape::Polygon { vertices: Vec::new(), cloud: true },
            Self::PolyLine => Shape::PolyLine { vertices: Vec::new(), start: LineEnding::None, end: LineEnding::None },
            Self::Callout => Shape::Callout { rect: [0.0; 4], knee: [0.0; 2], point: [0.0; 2], font_size: 10.0, ending: LineEnding::OpenArrow },
            Self::Caret => Shape::Caret { rect: [0.0; 4] },
            Self::ReplaceText => Shape::TextMarkup { kind: Markup::StrikeOut, quads: Vec::new() },
            Self::Eraser => Shape::Ink { strokes: Vec::new() },
            Self::Attach => Shape::Attachment { at: [0.0; 2], icon: pdfcraft_engine::AttachIcon::PushPin, file: String::new(), data: Vec::new() },
        }
    }
}

/// The comment swatches (our palette, close to Acrobat's quick colours).
pub const SWATCHES: [(&str, Rgb); 10] = [
    ("Yellow", [1.0, 0.94, 0.0]),
    ("Orange", [1.0, 0.54, 0.0]),
    ("Red", [0.89, 0.13, 0.13]),
    ("Pink", [1.0, 0.37, 0.64]),
    ("Purple", [0.54, 0.25, 0.82]),
    ("Blue", [0.0, 0.47, 0.84]),
    ("Light blue", [0.36, 0.75, 0.98]),
    ("Green", [0.18, 0.62, 0.36]),
    ("Gray", [0.5, 0.5, 0.5]),
    ("Black", [0.0, 0.0, 0.0]),
];

pub fn color32(c: Rgb) -> Color32 {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

/// Commenting preferences shared by every document (author, per-tool styles, pin).
#[derive(Clone, Debug)]
pub struct CommentPrefs {
    /// `/T` of new comments (Acrobat: Preferences ▸ Identity, default the login name).
    pub author: String,
    styles: Vec<(CommentTool, Style)>,
    /// Keep the tool selected after use (the pin).
    pub pinned: bool,
    /// The tool each flyout group shows (the last one used).
    pub group_tool: [CommentTool; 3],
    /// Hide hover previews on highlight annotations when they contain no comment text beyond the author.
    pub hide_empty_highlight_hover: bool,
    /// How a pen's pressure widens the previewed freehand ink (`stylus.rs`): conservative by
    /// default and off until a preference turns it on, scaling into an independently
    /// configurable band of the tool's own width.
    pub pen_pressure: PenPressure,
}

impl Default for CommentPrefs {
    fn default() -> Self {
        Self {
            author: login_name(),
            styles: ALL.iter().map(|t| (*t, Style::default_for(&t.sample()))).collect(),
            pinned: false,
            group_tool: [CommentTool::Note, CommentTool::Highlight, CommentTool::Ink],
            hide_empty_highlight_hover: false,
            pen_pressure: PenPressure::default(),
        }
    }
}

/// How a pen's pressure shapes the previewed freehand ink: pressure does not change what is
/// saved (the Ink annotation keeps its configured fixed width), only the live preview's
/// width, scaled into a band of the tool's configured width.
///
/// Conservative defaults and honest bounds: the knobs are clamped here rather than trusted
/// (a settings file is user data), and the whole feature is off until a preference turns it
/// on. An inverted band survives the clamping as it was written — light pressure wide, full
/// pressure narrow — and `[`width_scale`](Self::width_scale) reads it in that order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenPressure {
    /// Whether the pen's pressure widens (and thins) the live preview at all.
    pub on: bool,
    /// The previewed width at the lightest pressure, as a share of the tool's configured width.
    pub min: f64,
    /// The previewed width at full pressure, as a share of the tool's configured width.
    pub max: f64,
    /// How much of each new pressure report the smoothed value takes (`0.05`–`1.0`; less is
    /// smoother). 1.0 draws with each report as it comes.
    pub smoothing: f64,
}

impl Default for PenPressure {
    fn default() -> Self {
        Self { on: false, min: 0.75, max: 1.15, smoothing: 0.35 }
    }
}

impl PenPressure {
    /// Restore a persisted choice: every field clamps, and an unreadable field falls back to
    /// its default rather than being trusted.
    pub fn clamped(on: bool, min: f64, max: f64, smoothing: f64) -> Self {
        let none = Self::default();
        let between = |v: f64, lo: f64, hi: f64, base: f64| if v.is_finite() { v.clamp(lo, hi) } else { base };
        Self {
            on,
            min: between(min, 0.1, 2.0, none.min),
            max: between(max, 0.1, 2.0, none.max),
            smoothing: between(smoothing, 0.05, 1.0, none.smoothing),
        }
    }

    /// The previewed width factor for a normalized pressure `p` (0–1): the band interpolates
    /// from `min` at none to `max` at full — in whatever order the two knobs arrive, an
    /// inverted band thins with pressure instead of widening.
    pub(crate) fn width_scale(&self, p: f64) -> f64 {
        let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
        self.min + (self.max - self.min) * p
    }

    /// The configuration with one width bound edited in the Preferences dialog: the bounds
    /// stay coupled — an edited bound past its peer pulls the peer along, so the band stays
    /// well-formed while it is being edited instead of the input being dropped — and every
    /// field re-enters [`Self::clamped`]'s clamps. Restoring a settings file does not couple
    /// (it reads what was written, whatever band order it holds); only live edits do.
    pub(crate) fn with_min(mut self, min: f64) -> Self {
        self.min = min;
        self.max = self.max.max(min);
        Self::clamped(self.on, self.min, self.max, self.smoothing)
    }

    /// The same for the full-pressure bound.
    pub(crate) fn with_max(mut self, max: f64) -> Self {
        self.max = max;
        self.min = self.min.min(max);
        Self::clamped(self.on, self.min, self.max, self.smoothing)
    }

    /// The same for smoothing: no peer of its own, straight back through the clamps.
    pub(crate) fn with_smoothing(self, smoothing: f64) -> Self {
        Self::clamped(self.on, self.min, self.max, smoothing)
    }
}

impl CommentPrefs {
    /// Restore the persisted pressure choice through the clamping constructor above.
    pub fn set_pen_pressure(&mut self, pen: PenPressure) {
        self.pen_pressure = pen;
    }

    pub fn style(&self, tool: CommentTool) -> Style {
        self.styles.iter().find(|(t, _)| *t == tool).map(|(_, s)| s.clone()).unwrap_or_default()
    }

    /// Every tool with its current default style (the persisted settings read this).
    pub fn styles(&self) -> impl Iterator<Item = (CommentTool, &Style)> {
        self.styles.iter().map(|(t, s)| (*t, s))
    }

    pub fn set_color(&mut self, tool: CommentTool, c: Rgb) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.color = c;
        }
    }

    /// Clamp and store the stroke width (`tool.min_width()..=MAX_LINE_WIDTH`) for `tool`.
    pub fn set_width(&mut self, tool: CommentTool, w: f64) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.width = if w.is_finite() { w.clamp(tool.min_width(), MAX_LINE_WIDTH) } else { s.width };
        }
    }

    /// The tool's opacity, 10–100 % (an invisible comment can't be found again).
    pub fn set_opacity(&mut self, tool: CommentTool, o: f64) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.opacity = if o.is_finite() { o.clamp(0.1, 1.0) } else { 1.0 };
        }
    }

    /// The tool's fill (`None`: no fill).
    pub fn set_fill(&mut self, tool: CommentTool, fill: Option<Rgb>) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.fill = if tool.has_fill() { fill.filter(|c| c.iter().all(|x| x.is_finite())).map(|c| c.map(|x| x.clamp(0.0, 1.0))) } else { None };
        }
    }

    pub fn set_style(&mut self, tool: CommentTool, style: Style) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            *s = style;
        }
    }
}

/// Whether `tool` makes comments like `a`. Arrows are saved as plain Line annotations with an
/// ending, so the Line and Arrow tools both match a line.
pub(crate) fn made_by(a: &Annotation, tool: CommentTool) -> bool {
    match tool_for(a) {
        Some(CommentTool::Line) => matches!(tool, CommentTool::Line | CommentTool::Arrow),
        made => made == Some(tool),
    }
}

/// The tool whose defaults "Make Current Properties Default" should update.
///
/// Arrows are stored as Line annotations. An arrow ending belongs to the Arrow tool, so its
/// defaults stay separate from a plain line.
pub(crate) fn tool_for_default(a: &Annotation, endings: Option<&[LineEnding]>) -> Option<CommentTool> {
    match tool_for(a) {
        Some(CommentTool::Line) if endings.is_some_and(|ends| ends.iter().copied().any(arrow_ending)) => Some(CommentTool::Arrow),
        other => other,
    }
}

fn arrow_ending(ending: LineEnding) -> bool {
    matches!(ending, LineEnding::OpenArrow | LineEnding::ClosedArrow | LineEnding::ROpenArrow | LineEnding::RClosedArrow)
}

/// The tool that makes comments like `a` (for Make Current Properties Default).
pub fn tool_for(a: &Annotation) -> Option<CommentTool> {
    Some(match (a.subtype.as_str(), a.intent.as_deref()) {
        ("Text", _) => CommentTool::Note,
        ("FreeText", Some("FreeTextCallout")) => CommentTool::Callout,
        ("FreeText", Some("FreeTextTypeWriter")) => return None,
        ("FreeText", _) => CommentTool::TextBox,
        ("Highlight", _) => CommentTool::Highlight,
        ("Underline", _) => CommentTool::Underline,
        ("StrikeOut", Some("StrikeOutTextEdit")) => CommentTool::ReplaceText,
        ("StrikeOut", _) => CommentTool::StrikeOut,
        ("Squiggly", _) => CommentTool::Squiggly,
        ("Ink", Some("FreehandHighlight")) => CommentTool::FreehandHighlight,
        ("Ink", _) => CommentTool::Ink,
        ("Line", _) => CommentTool::Line,
        ("Square", _) => CommentTool::Rectangle,
        ("Circle", _) => CommentTool::Oval,
        ("Polygon", Some("PolygonCloud")) => CommentTool::Cloud,
        ("Polygon", _) => CommentTool::Polygon,
        ("PolyLine", _) => CommentTool::PolyLine,
        ("Caret", _) => CommentTool::Caret,
        ("FileAttachment", _) => CommentTool::Attach,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
