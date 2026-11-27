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

use egui::{Color32, Pos2, Rect};
use pdfcraft_engine::{Edit, LineEnding, Markup, NewAnnotation, NoteIcon, ReviewState, Rgb, Shape, Style};
use pdfcraft_render::{Annotation, DocInfo, PageInfo, PageText};

use craft::review::{AnnotationKind, AnnotationTool};

use crate::QuickTool;
use crate::canvas::{DocView, PageXform};
#[cfg(test)]
use crate::icons;
#[cfg(test)]
use craft::review::ui::annotate::{AnnotateCommand, AnnotateIcons};
#[cfg(test)]
use egui::{pos2, vec2};

mod bridge;
pub(crate) mod colours;
mod composer;
mod geometry;
mod menu;
mod overlay;
mod popups;
mod state;
mod tool;

#[cfg(test)]
use bridge::hover_tip;
/// The comment thread of the annotation at `(page, index)`: the key the Comments panel files it under.
pub use bridge::thread_id;
pub(crate) use bridge::{PageMap, delete_selected, id_of, key_of, kind_of, parts, pdf_date_seconds, rgb8};
pub(crate) use colours::{RecentColours, border_choice, fill_choice, palette, to_rgb};
pub(crate) use composer::composer;
pub(crate) use geometry::caret_snap;
pub(crate) use geometry::page_rotation;
#[cfg(test)]
pub(crate) use geometry::snapped_caret;
pub use geometry::{drawn_text_box_rect, snug_text_box_rect, text_box_rect};
pub use menu::CanvasAction;
pub(crate) use menu::{CommentMenuCx, MenuRights, context_rows, popover_from, quick_bar_controls};
pub(crate) use overlay::{Hover, show};
pub(crate) use popups::note_popups;
pub(crate) use state::PageCx;
#[cfg(test)]
use state::after_delete;
use state::login_name;
pub use state::{CommentView, Composer, ComposerKind};
pub(crate) use tool::{CommentsTool, PastePlacement, context_anchor, context_comment, context_target, page_after_text, part_pressed, take_edits};

/// Maximum stroke thickness offered by commenting controls, in PDF points.
pub const MAX_LINE_WIDTH: f64 = 100.0;
/// The font sizes a text box or callout default may take (Comment Properties' range).
const MIN_FONT_SIZE: f64 = 1.0;
const MAX_FONT_SIZE: f64 = 400.0;

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

    /// The framework's annotation kind this tool draws; the Eraser draws none.
    pub(crate) fn kind(self) -> Option<AnnotationKind> {
        Some(match self {
            Self::Note => AnnotationKind::Note,
            Self::TextBox => AnnotationKind::TextBox,
            Self::Highlight => AnnotationKind::Highlight,
            Self::Underline => AnnotationKind::Underline,
            Self::StrikeOut => AnnotationKind::StrikeOut,
            Self::Squiggly => AnnotationKind::Squiggly,
            Self::Ink => AnnotationKind::Ink,
            Self::FreehandHighlight => AnnotationKind::FreehandHighlight,
            Self::Line => AnnotationKind::Line,
            Self::Arrow => AnnotationKind::Arrow,
            Self::Rectangle => AnnotationKind::Rectangle,
            Self::Oval => AnnotationKind::Oval,
            Self::Polygon => AnnotationKind::Polygon,
            Self::PolyLine => AnnotationKind::PolyLine,
            Self::Cloud => AnnotationKind::Cloud,
            Self::Callout => AnnotationKind::Callout,
            Self::Caret => AnnotationKind::Caret,
            Self::ReplaceText => AnnotationKind::ReplaceText,
            Self::Attach => AnnotationKind::Attach,
            Self::Eraser => return None,
        })
    }

    /// The framework's tool for this one.
    pub fn tool(self) -> AnnotationTool {
        self.kind().map_or(AnnotationTool::Eraser, AnnotationTool::Draw)
    }

    /// The tool that draws `kind`.
    pub fn from_kind(kind: AnnotationKind) -> Option<Self> {
        ALL.into_iter().find(|t| t.kind() == Some(kind))
    }

    /// Icon name for this commenting tool: a catalogue name, or a Lucide name the app's icon set serves.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Note => "message-square-plus",
            Self::TextBox => "type",
            Self::Highlight => "highlighter",
            Self::Underline => "text_underline",
            Self::StrikeOut => "text_strikethrough",
            Self::Squiggly => "spline",
            Self::FreehandHighlight => "freehand-highlighter",
            Self::Ink => "pencil",
            Self::Line => "minus",
            Self::Arrow => "move-right",
            Self::Rectangle => "square",
            Self::Oval => "circle",
            Self::Polygon => "pentagon",
            Self::PolyLine => "spline",
            Self::Cloud => "cloud",
            Self::Callout => "message-square-quote",
            Self::Caret => "message-square-text",
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

    /// Whether the border-colour control applies to this tool: text boxes and callouts, whose
    /// border takes the text colour unless it has its own.
    pub fn has_border_color(self) -> bool {
        matches!(self, Self::TextBox | Self::Callout)
    }

    /// Whether the fill-colour control applies to this tool.
    pub fn has_fill(self) -> bool {
        matches!(self, Self::Rectangle | Self::Oval | Self::Polygon | Self::Cloud | Self::TextBox | Self::Callout)
    }

    /// The font size new comments of this kind start with (`None`: no text).
    fn default_font_size(self) -> Option<f64> {
        match self.sample() {
            Shape::TextBox { font_size, .. } | Shape::Callout { font_size, .. } => Some(font_size),
            _ => None,
        }
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

/// Each comment tool's icon as the framework's annotate commands show it, on every surface (toolbar, flyouts,
/// palette, menus): [`CommentTool::icon`] is the one map.
#[cfg(test)]
pub(crate) fn annotate_icons() -> AnnotateIcons {
    ALL.iter().fold(AnnotateIcons::new(), |set, t| set.with(AnnotateCommand::of(t.tool()), t.icon()))
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
    /// The font size of new text boxes and callouts (Make Properties Default).
    font_sizes: Vec<(CommentTool, f64)>,
    /// Keep the tool selected after use (the pin).
    pub pinned: bool,
    /// The tool each flyout group shows (the last one used).
    pub group_tool: [CommentTool; 3],
    /// Hide hover previews on highlight annotations when they contain no comment text beyond the author.
    pub hide_empty_highlight_hover: bool,
}

impl Default for CommentPrefs {
    fn default() -> Self {
        Self {
            author: login_name(),
            styles: ALL.iter().map(|t| (*t, Style::default_for(&t.sample()))).collect(),
            font_sizes: ALL.iter().filter_map(|t| t.default_font_size().map(|s| (*t, s))).collect(),
            pinned: false,
            group_tool: [CommentTool::Note, CommentTool::Highlight, CommentTool::Ink],
            hide_empty_highlight_hover: false,
        }
    }
}

impl CommentPrefs {
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

    /// The text box tool's border shape (the other tools keep a rectangle).
    pub fn set_border_shape(&mut self, tool: CommentTool, shape: pdfcraft_engine::BorderShape) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.border_shape = if tool == CommentTool::TextBox { shape } else { pdfcraft_engine::BorderShape::Rectangle };
        }
    }

    /// The tool's fill's own opacity (0–100 %) and a text box's or callout's text's (10–100 %),
    /// for new comments; `None`: as the tool's opacity.
    pub fn set_part_opacity(&mut self, tool: CommentTool, fill: Option<f64>, text: Option<f64>) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.fill_opacity = fill.filter(|o| o.is_finite() && tool.has_fill()).map(|o| o.clamp(0.0, 1.0));
            s.text_opacity = text.filter(|o| o.is_finite() && matches!(tool, CommentTool::TextBox | CommentTool::Callout)).map(|o| o.clamp(0.1, 1.0));
        }
    }

    /// The tool's fill (`None`: no fill).
    pub fn set_fill(&mut self, tool: CommentTool, fill: Option<Rgb>) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.fill = if tool.has_fill() { fill.filter(|c| c.iter().all(|x| x.is_finite())).map(|c| c.map(|x| x.clamp(0.0, 1.0))) } else { None };
        }
    }

    /// The border's own colour for new text boxes and callouts (`None`: the text colour).
    pub fn set_border_color(&mut self, tool: CommentTool, color: Option<Rgb>) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.border_color =
                if tool.has_border_color() { color.filter(|c| c.iter().all(|x| x.is_finite())).map(|c| c.map(|x| x.clamp(0.0, 1.0))) } else { None };
        }
    }

    /// The font size of new comments of `tool` (`None`: the tool has no text).
    pub fn font_size(&self, tool: CommentTool) -> Option<f64> {
        self.font_sizes.iter().find(|(t, _)| *t == tool).map(|(_, s)| *s)
    }

    /// Every text tool with its default font size (the persisted settings read this).
    pub fn font_sizes(&self) -> impl Iterator<Item = (CommentTool, f64)> + '_ {
        self.font_sizes.iter().copied()
    }

    /// Clamp and store the font size (1–400 pt, as Comment Properties allows) for a text tool;
    /// a non-finite size keeps the current one.
    pub fn set_font_size(&mut self, tool: CommentTool, size: f64) {
        if let Some((_, s)) = self.font_sizes.iter_mut().find(|(t, _)| *t == tool)
            && size.is_finite()
        {
            *s = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
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
/// defaults stay separate from a plain line. A Replace Text group is selected by its caret (the
/// group's primary, ISO 32000-2 §12.5.6.2), so a caret whose group holds a Replace Text strikeout
/// (`/IT /StrikeOutTextEdit`) belongs to the Replace Text tool; `all` is the document's listing.
pub(crate) fn tool_for_default(all: &[Annotation], a: &Annotation, endings: Option<&[LineEnding]>) -> Option<CommentTool> {
    match tool_for(a) {
        Some(CommentTool::Line) if endings.is_some_and(|ends| ends.iter().copied().any(arrow_ending)) => Some(CommentTool::Arrow),
        Some(CommentTool::Caret) if replaces_text(all, a) => Some(CommentTool::ReplaceText),
        other => other,
    }
}

/// Whether the caret `a` is the primary of a Replace Text group: a grouped strikeout with
/// `/IT /StrikeOutTextEdit` is one of its subordinates.
fn replaces_text(all: &[Annotation], a: &Annotation) -> bool {
    all.iter().any(|s| s.is_group_member() && s.subtype == "StrikeOut" && s.intent.as_deref() == Some("StrikeOutTextEdit") && s.replies_to(a))
}

fn arrow_ending(ending: LineEnding) -> bool {
    matches!(ending, LineEnding::OpenArrow | LineEnding::ClosedArrow | LineEnding::ROpenArrow | LineEnding::RClosedArrow)
}

/// Typed text (Fill & Sign's, or a typewriter's): no comment tool makes it, so Make Current
/// Properties Default sets what Fill & Sign gives new text.
pub(crate) fn typed_text(a: &Annotation) -> bool {
    a.subtype == "FreeText" && a.intent.as_deref() == Some("FreeTextTypeWriter")
}

/// Whether Make Current Properties Default has defaults to set from `a`.
pub(crate) fn defaultable(a: &Annotation) -> bool {
    tool_for(a).is_some() || typed_text(a)
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
        ("StrikeOut", Some("StrikeOutTextEdit")) | ("Caret", Some("Replace")) => CommentTool::ReplaceText,
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
