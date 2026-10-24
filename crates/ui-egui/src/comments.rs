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
use crate::theme::{self, Tokens};
use crate::{QuickTool, icons};

/// Acrobat's selection blue for annotation frames and handles (11-visual-spec §5).
const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommentTool {
    Note,
    TextBox,
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Ink,
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
    &[CommentTool::Highlight, CommentTool::Underline, CommentTool::StrikeOut, CommentTool::Squiggly, CommentTool::Caret, CommentTool::ReplaceText],
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

pub const ALL: [CommentTool; 19] = [
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
    pub fn command(self) -> &'static str {
        match self {
            Self::Note => "comment.note",
            Self::TextBox => "comment.freetext",
            Self::Highlight => "comment.highlight",
            Self::Underline => "comment.underline",
            Self::StrikeOut => "comment.strikeout",
            Self::Squiggly => "comment.squiggly",
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

    pub fn from_command(id: &str) -> Option<Self> {
        ALL.into_iter().find(|t| t.command() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Add a sticky note",
            Self::TextBox => "Add a text box",
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "Strikethrough",
            Self::Squiggly => "Squiggly underline",
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

    pub fn icon(self) -> &'static str {
        match self {
            Self::Note => "message-square-plus",
            Self::TextBox => "type",
            Self::Highlight => "highlighter",
            Self::Underline => "underline",
            Self::StrikeOut => "strikethrough",
            Self::Squiggly => "spline",
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
        matches!(self, Self::Ink | Self::Line | Self::Arrow | Self::Rectangle | Self::Oval | Self::Eraser)
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
}

impl Default for CommentPrefs {
    fn default() -> Self {
        Self {
            author: login_name(),
            styles: ALL.iter().map(|t| (*t, Style::default_for(&t.sample()))).collect(),
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

    pub fn set_width(&mut self, tool: CommentTool, w: f64) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.width = w.clamp(tool.min_width(), 12.0);
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

/// The user's login name, which Acrobat uses as the default comment author.
fn login_name() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return v;
        }
    }
    "Guest".into()
}

/// A drag in progress on a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Gesture {
    /// Drawing with a tool: points in user space (ink: the stroke; polygons: the vertices
    /// clicked so far; others: start and end).
    Draw { page: usize, tool: CommentTool, points: Vec<[f64; 2]> },
    /// Moving a comment, from the press position on screen.
    Move { page: usize, index: usize, from: Pos2 },
    /// Resizing a comment by one of its handles: (dx, dy) ∈ {-1, 0, 1}² says which sides move.
    Resize { page: usize, index: usize, handle: (i8, i8), from: Pos2, aspect_ratio: Option<f32> },
    /// Dragging point `point` of a line, connected lines, a polygon or a cloud (its `points`
    /// before the drag, in user space) to where the pointer is.
    Reshape { page: usize, index: usize, point: usize, points: Vec<[f64; 2]> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ComposerKind {
    Note,
    /// A text box; `size` is the box dragged out (displayed width and height, in user space
    /// units), `None` after a click: then the box fits its text.
    TextBox {
        size: Option<[f64; 2]>,
    },
    /// A callout whose arrow touches `point`; the composer's anchor is the text box's top-left.
    Callout {
        point: [f64; 2],
    },
    /// Insert text at the anchor (the caret's tip); `size` is the side of the caret's square box,
    /// in user space units.
    Caret {
        size: f64,
    },
    /// Replace the selected text (`CommentView::replace` holds the quads).
    Replace,
    /// Edit the text of the comment at this index on the composer's page.
    Edit(usize),
}

/// The floating "Add a comment" card.
#[derive(Clone, Debug, PartialEq)]
pub struct Composer {
    pub page: usize,
    /// Anchor in user space (the note's top-left, the text box's top-left, the comment's corner).
    pub at: [f64; 2],
    pub kind: ComposerKind,
    pub text: String,
    pub focus: bool,
}

/// A released drawing gesture waiting for the page raster to show its annotation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settling {
    pub page: usize,
    pub tool: CommentTool,
    pub points: Vec<[f64; 2]>,
    /// Frame time (seconds) at release; a raster that never arrives must not leave a ghost.
    pub since: f64,
    /// Its annotation is in the document, so a raster of the page from now on draws it. Until
    /// then a raster is of the document without it (in full screen the edit is applied the frame
    /// after the release, and a raster already on its way can arrive in between).
    pub committed: bool,
}

/// Longest a released drawing stays painted without its page raster, and how many are kept.
const SETTLE_SECS: f64 = 20.0;
const SETTLE_MAX: usize = 16;

impl CommentView {
    /// Remember a released drag drawing so it stays on screen until its page is re-rendered.
    pub(crate) fn settle(&mut self, now: f64, page: usize, tool: CommentTool, points: &[[f64; 2]]) {
        if !matches!(tool, CommentTool::Ink | CommentTool::Line | CommentTool::Arrow | CommentTool::Rectangle | CommentTool::Oval) {
            return;
        }
        self.settling.retain(|s| now - s.since < SETTLE_SECS);
        if self.settling.len() >= SETTLE_MAX {
            self.settling.remove(0);
        }
        self.settling.push(Settling { page, tool, points: points.to_vec(), since: now, committed: false });
    }

    /// How many released drawings are still painted from their points (waiting for a raster).
    pub fn settling_len(&self) -> usize {
        self.settling.len()
    }

    /// A fresh raster of `page` is on screen: it draws the committed annotations itself. One
    /// still waiting for its edit isn't in that raster, and stays drawn from its points.
    pub(crate) fn page_rendered(&mut self, page: usize) {
        self.settling.retain(|s| s.page != page || !s.committed);
    }

    /// The edit adding the released drawings was applied: rasters from now on draw them.
    pub(crate) fn drawings_committed(&mut self) {
        for s in &mut self.settling {
            s.committed = true;
        }
    }

    /// The edit adding the released drawings failed: nothing will draw them.
    pub(crate) fn drawings_refused(&mut self) {
        self.settling.retain(|s| s.committed);
    }
}

/// Per-document commenting state.
#[derive(Clone, Debug, Default)]
pub struct CommentView {
    /// The selected comment: (page, index in `/Annots`).
    pub selected: Option<(usize, usize)>,
    pub gesture: Option<Gesture>,
    /// Drawings just released: shown from their points until the page raster that includes the
    /// committed annotation arrives, so the stroke never vanishes in between (#1858).
    pub(crate) settling: Vec<Settling>,
    /// Escape ends egui's drag too; ignore its synthetic release until the mouse is up.
    cancelled_drag: bool,
    pub composer: Option<Composer>,
    /// Reply being typed under the selected card.
    pub reply: String,
    /// Inline edit of a card's text in the Comments panel: (page, index, text).
    pub editing: Option<(usize, usize, String)>,
    /// The panel should scroll the selected card into view.
    pub reveal: bool,
    /// The "Add a comment" box at the top of the Comments panel.
    pub add_box: String,
    /// The Comments panel's search query (`None`: search closed).
    pub search: Option<String>,
    /// Comment types, authors and statuses hidden by the panel's filter.
    pub hidden_types: Vec<String>,
    pub hidden_authors: Vec<String>,
    pub hidden_statuses: Vec<String>,
    /// Colours (`RRGGBB`, empty for none) and checkmark states ("Checked", "Unchecked") hidden.
    pub hidden_colors: Vec<String>,
    pub hidden_checks: Vec<String>,
    /// How the panel orders comments.
    pub sort: SortBy,
    /// Pages whose group is folded in the panel (when sorted by page).
    pub collapsed_pages: Vec<usize>,
    /// Comment Properties was asked for from the panel: (page, index).
    pub props_request: Option<(usize, usize)>,
    /// Make Current Properties Default was asked for: (page, index).
    pub default_request: Option<(usize, usize)>,
    /// A file attachment comment's file was asked for: (page, index, open it in a new tab
    /// rather than save it).
    pub attachment_request: Option<(usize, usize, bool)>,
    /// Replace Text: the struck-out text's quads while the replacement is typed.
    pub replace: Option<Vec<[f64; 8]>>,
    /// Attach file: where the icon goes (page, top-left in user space), for the app to pick a file.
    pub attach_at: Option<(usize, [f64; 2])>,
    pub search_focus: bool,
    /// Where the canvas context menu was opened: (page, user-space point).
    pub context_at: Option<(usize, [f64; 2])>,
    /// A one-shot tool just finished; the app returns to the Select tool unless pinned.
    pub tool_done: bool,
    /// What the inline editor (`editing`) does with the keyboard the next time it is drawn.
    pub(crate) editor_focus: Option<EditorFocus>,
}

/// A one-off change to the keyboard focus of the Comments panel's inline editor. The editor
/// never asks for the keyboard on its own: when it did so on every frame, it took the keyboard
/// back after every click elsewhere, and the tool keys (V, H) were typed into it (#2669).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorFocus {
    /// The editor just opened: it takes the keyboard, once.
    Take,
    /// Another comment was added: the editor keeps its text but gives the keyboard back.
    Release,
}

impl CommentView {
    /// Takes back the last point of a polygon, cloud or connected lines being drawn; the first
    /// point's removal cancels the drawing. Returns whether there was such a drawing.
    pub fn undo_point(&mut self) -> bool {
        let Some(Gesture::Draw { tool, points, .. }) = self.gesture.as_mut() else { return false };
        if !tool.clicks_points() {
            return false;
        }
        points.pop();
        if points.is_empty() {
            self.gesture = None;
        }
        true
    }

    /// Open the inline editor on comment (page, index) with `text`. It takes the keyboard once,
    /// as it opens; after that the keyboard goes wherever the user clicks.
    pub(crate) fn open_editor(&mut self, page: usize, index: usize, text: String) {
        self.editing = Some((page, index, text));
        self.editor_focus = Some(EditorFocus::Take);
    }

    /// A comment was just added. The editor open on another one must not keep the keyboard, or
    /// the tool keys pressed next are typed into it (#2669): an empty editor closes, and one
    /// holding text stays open with it, without the keyboard.
    pub(crate) fn comment_added(&mut self) {
        if self.editing.as_ref().is_some_and(|(_, _, text)| !text.is_empty()) {
            self.editor_focus = Some(EditorFocus::Release);
        } else {
            self.editing = None;
            self.editor_focus = None;
        }
    }
}

/// Comments panel order (Acrobat: Sort by page, author, date, type, colour).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Page,
    Author,
    Date,
    Type,
    Color,
}

/// What a page needs to know to handle comment input.
pub(crate) struct PageCx<'a> {
    pub page: usize,
    pub xf: &'a PageXform,
    pub info: &'a DocInfo,
    pub tool: QuickTool,
    pub prefs: &'a CommentPrefs,
    /// A redaction mark's fill while the pointer is over it, and its opacity (Redaction Tool
    /// Properties).
    pub redact_mark: ([f64; 3], f64),
    /// The document allows commenting.
    pub allowed: bool,
    /// Comments are hidden (Hide all comments): nothing to select.
    pub hidden: bool,
}

impl PageCx<'_> {
    fn to_user(&self, p: Pos2) -> [f64; 2] {
        let (vx, vy) = self.xf.screen_to_view(p);
        let u = self.info.pages[self.page].view_to_user(vx, vy);
        [u[0] as f64, u[1] as f64]
    }

    fn to_screen(&self, p: [f64; 2]) -> Pos2 {
        let v = self.info.pages[self.page].user_to_view(p[0] as f32, p[1] as f32);
        self.xf.norm_to_screen(v[0] / self.xf.pw, v[1] / self.xf.ph)
    }

    /// The page's clockwise `/Rotate`.
    fn rotation(&self) -> u16 {
        self.info.pages.get(self.page).map_or(0, |p| p.rotation)
    }

    /// Where a comment is drawn on screen (a sticky note stays upright on a turned page).
    fn screen_rect(&self, a: &Annotation) -> Rect {
        self.xf.user_rect(self.info, self.page, a.shown_rect(self.rotation()))
    }

    /// Top-level comments on this page, in paint order.
    fn comments(&self) -> impl Iterator<Item = &Annotation> {
        self.info.annotations.iter().filter(move |a| !self.hidden && a.page == self.page && a.in_reply_to.is_none() && a.subtype != "Popup")
    }

    /// Where a comment is on screen: one rectangle, or one per marked line for text markup.
    pub(crate) fn screen_rects<'b>(&'b self, a: &'b Annotation) -> impl Iterator<Item = Rect> + 'b {
        let quads = if is_markup(&a.subtype) { a.quads.as_slice() } else { &[] };
        let fallback = quads.is_empty().then(|| self.screen_rect(a));
        quads
            .iter()
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                let (x0, x1) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
                let (y0, y1) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
                self.xf.user_rect(self.info, self.page, [x0, y0, x1, y1])
            })
            .chain(fallback)
    }

    /// A redaction mark's areas: one per marked quad, or its rectangle.
    fn redact_rects(&self, a: &Annotation) -> Vec<Rect> {
        if a.quads.is_empty() {
            return vec![self.screen_rect(a)];
        }
        a.quads
            .iter()
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                let (x0, x1) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
                let (y0, y1) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
                self.xf.user_rect(self.info, self.page, [x0, y0, x1, y1])
            })
            .collect()
    }

    /// The topmost comment under `p`.
    fn hit(&self, p: Pos2) -> Option<&Annotation> {
        self.comments().filter(|a| self.screen_rects(a).any(|r| r.expand(3.0).contains(p))).last()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Annotation> {
        self.comments().find(|a| a.index == index)
    }

    pub(crate) fn rect_to_user(&self, r: Rect) -> [f64; 4] {
        let (a, b) = (self.to_user(r.left_top()), self.to_user(r.right_bottom()));
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    }

    /// The same geometry drives the live image, its handles, and the edit on release.
    /// `shift` is whether Shift is held now: it keeps the box's proportions from a corner.
    pub(crate) fn adjusted_rect(
        &self,
        a: &Annotation,
        gesture: Option<&Gesture>,
        pointer: Option<Pos2>,
        shift: bool,
        pending: Option<&Edit>,
    ) -> Rect {
        let r = self.screen_rect(a);
        if let Some(Edit::ResizeAnnotation { page, index, rect }) = pending
            && (*page, *index) == (self.page, a.index)
        {
            return self.xf.user_rect(self.info, self.page, a.shown(*rect, self.rotation()).map(|v| v as f32));
        }
        if let Some(Edit::MoveAnnotation { page, index, dx, dy }) = pending
            && (*page, *index) == (self.page, a.index)
        {
            let r = a.shown_rect(self.rotation());
            return self.xf.user_rect(self.info, self.page, [r[0] + *dx as f32, r[1] + *dy as f32, r[2] + *dx as f32, r[3] + *dy as f32]);
        }
        match (gesture, pointer) {
            (Some(Gesture::Move { page, index, from }), Some(p)) if (*page, *index) == (self.page, a.index) => r.translate(p - *from),
            (Some(Gesture::Resize { page, index, handle, from, aspect_ratio }), Some(p)) if (*page, *index) == (self.page, a.index) => {
                resized(r, *handle, p - *from, corner_ratio(r, *aspect_ratio, shift))
            }
            _ => r,
        }
    }
}

fn is_markup(subtype: &str) -> bool {
    matches!(subtype, "Highlight" | "Underline" | "StrikeOut" | "Squiggly")
}

/// Rectangles, ovals, text boxes, stamps and drawings (drawn signatures among them). A callout's
/// `/Rect` also holds its leader line, so it only moves.
fn resizable(a: &Annotation) -> bool {
    matches!(a.subtype.as_str(), "Square" | "Circle" | "FreeText" | "Stamp" | "Ink") && a.intent.as_deref() != Some("FreeTextCallout")
}

/// Lines, connected lines, polygons and clouds: their ends and corners have handles of their own
/// (measurements keep the points they measured, and a locked comment its shape).
fn reshapable(a: &Annotation) -> bool {
    matches!(a.subtype.as_str(), "Line" | "PolyLine" | "Polygon")
        && !a.points.is_empty()
        && !a.locked
        && !a.intent.as_deref().is_some_and(|i| i.ends_with("Dimension"))
}

/// Whether `tool` picks up a line's end and corner handles: the Select and Fill & Sign tools.
/// The drawing tools draw from where they are pressed, so a new line starts at the end of the
/// one just drawn rather than dragging it.
fn grabs_points(tool: QuickTool) -> bool {
    !matches!(tool, QuickTool::Comment(t) if t.draws() || t.clicks_points())
}

/// The point of `a` (an end or a corner) whose handle is at `p` on screen.
fn point_handle(cx: &PageCx<'_>, a: &Annotation, p: Pos2) -> Option<usize> {
    a.points.iter().position(|q| cx.to_screen([f64::from(q[0]), f64::from(q[1])]).distance(p) <= 7.0)
}

/// `points` with point `i` moved to `to`; with Shift, held to the nearest 45° from the point
/// before it (a line's other end; the first corner turns about the second).
pub(crate) fn reshaped(points: &[[f64; 2]], i: usize, to: [f64; 2], shift: bool) -> Vec<[f64; 2]> {
    let mut out = points.to_vec();
    let anchor = if i == 0 { points.get(1) } else { points.get(i - 1) };
    let to = match anchor {
        Some(a) if shift => pdfcraft_engine::measure::snap::constrain(*a, to),
        _ => to,
    };
    if let Some(p) = out.get_mut(i) {
        *p = to;
    }
    out
}

const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

fn handle_pos(r: Rect, (hx, hy): (i8, i8)) -> Pos2 {
    let x = match hx {
        -1 => r.left(),
        0 => r.center().x,
        _ => r.right(),
    };
    let y = match hy {
        -1 => r.top(),
        0 => r.center().y,
        _ => r.bottom(),
    };
    pos2(x, y)
}

/// The proportions a corner drag of `r` keeps: the box's own fixed ratio (an image signature's),
/// else `r`'s current one while Shift is held, else none.
pub(crate) fn corner_ratio(r: Rect, fixed: Option<f32>, shift: bool) -> Option<f32> {
    fixed.or_else(|| (shift && r.width() > 0.0 && r.height() > 0.0).then(|| r.width() / r.height()))
}

pub(crate) fn resized(r: Rect, (hx, hy): (i8, i8), d: egui::Vec2, aspect_ratio: Option<f32>) -> Rect {
    if hx != 0
        && hy != 0
        && let Some(ratio) = aspect_ratio.filter(|v| v.is_finite() && *v > 0.0)
    {
        // As in content_ui, the larger requested dimension drives proportional corner resizing.
        // Keep the opposite corner fixed and prevent crossing it from flipping the image.
        let width = (r.width() + f32::from(hx) * d.x).max((r.height() + f32::from(hy) * d.y) * ratio).max(4.0).max(4.0 * ratio);
        let anchor = handle_pos(r, (-hx, -hy));
        return Rect::from_two_pos(anchor, anchor + vec2(f32::from(hx) * width, f32::from(hy) * width / ratio));
    }
    let mut r = r;
    match hx {
        -1 => r.min.x += d.x,
        1 => r.max.x += d.x,
        _ => {}
    }
    match hy {
        -1 => r.min.y += d.y,
        1 => r.max.y += d.y,
        _ => {}
    }
    Rect::from_two_pos(r.min, r.max)
}

/// Comment input on one page, before text selection runs. Returns `true` when the pointer
/// gesture belongs to commenting (text selection must ignore it).
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, cx: &PageCx<'_>, view: &mut DocView) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let origin = ui.input(|i| i.pointer.press_origin());
    let page_rect = cx.xf.rect;
    let pressed_here = origin.is_some_and(|o| page_rect.contains(o));
    let over_page = pointer.is_some_and(|p| page_rect.contains(p));
    let fill_grab = fill_grabs(ui, cx, view);
    let draw_grab = draw_grabs(ui, cx, view);
    let caret_text = if cx.tool == QuickTool::Comment(CommentTool::Caret) { view.page_text(cx.page) } else { None };
    let cv = &mut view.comments;
    if cv.gesture.is_some() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        cv.gesture = None;
        cv.cancelled_drag = true;
        return true;
    }
    if cv.cancelled_drag {
        return true;
    }
    if resp.secondary_clicked()
        && let Some(p) = pointer.filter(|p| page_rect.contains(*p))
    {
        cv.context_at = Some((cx.page, cx.to_user(p)));
        cv.selected = cx.hit(p).map(|a| (cx.page, a.index));
    }
    match cx.tool {
        // The shape just drawn is selected with its handles showing: they resize it without a
        // trip to the Select tool (#2338). Anywhere else the tool keeps drawing.
        QuickTool::Comment(_) if draw_grab => select_input(ui, resp, cx, view, pointer, origin, pressed_here),
        QuickTool::Comment(tool) if tool.draws() => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.drag_started()
                && pressed_here
                && let Some(o) = origin
            {
                cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![cx.to_user(o)] });
                cv.selected = None;
            }
            if let Some(Gesture::Draw { page, tool, points }) = cv.gesture.as_mut()
                && *page == cx.page
                && let Some(p) = pointer
            {
                let p = clamp_to(page_rect, p);
                let u = cx.to_user(p);
                if matches!(*tool, CommentTool::Ink | CommentTool::Eraser) {
                    let last = points.last().map(|l| cx.to_screen(*l)).unwrap_or(p);
                    if last.distance(p) >= 1.5 {
                        points.push(u);
                    }
                } else {
                    points.truncate(1);
                    points.push(u);
                }
            }
            if resp.drag_stopped()
                && let Some(Gesture::Draw { page, tool, points }) = cv.gesture.clone()
                && page == cx.page
            {
                cv.gesture = None;
                if tool == CommentTool::Eraser {
                    view.pending_edit = erase(cx, &points);
                } else if let Some(shape) = drawn_shape(tool, &points) {
                    let now = ui.input(|i| i.time);
                    cv.settle(now, page, tool, &points);
                    view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
                }
            }
            // A quick press/release does not start an egui drag, but Acrobat's Draw tool treats
            // it as a one-point ink stroke (a dot). The appearance uses round line caps, so a
            // single PDF point is visible and remains editable like any other drawing.
            if tool == CommentTool::Ink
                && resp.clicked()
                && over_page
                && let Some(p) = pointer
                && let Some(shape) = drawn_shape(tool, &[cx.to_user(p)])
            {
                cv.selected = None;
                let now = ui.input(|i| i.time);
                cv.settle(now, cx.page, tool, &[cx.to_user(p)]);
                view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
            }
            true
        }
        QuickTool::Comment(tool) if tool.clicks_points() => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            let finish = resp.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            // Backspace (or Delete) takes back the last point, e.g. after a wrong click; Undo does too.
            // Only while a drawing is in progress: otherwise Delete belongs to the selected comment.
            if matches!(cv.gesture, Some(Gesture::Draw { page, .. }) if page == cx.page)
                && !ui.ctx().egui_wants_keyboard_input()
                && ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) || i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                })
            {
                cv.undo_point();
                return true;
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                let u = cx.to_user(p);
                match cv.gesture.as_mut() {
                    Some(Gesture::Draw { page, tool: t, points }) if *page == cx.page && *t == tool => {
                        // Clicking the first point again closes a polygon.
                        let closes = points.len() >= 3 && tool != CommentTool::PolyLine && cx.to_screen(points[0]).distance(p) <= 6.0;
                        let repeat = points.last().is_some_and(|l| cx.to_screen(*l).distance(p) < 2.0);
                        if closes {
                            let pts = std::mem::take(points);
                            cv.gesture = None;
                            if let Some(shape) = drawn_shape(tool, &pts) {
                                view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
                            }
                            return true;
                        }
                        if !repeat {
                            points.push(u);
                        }
                    }
                    _ => {
                        cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![u] });
                        cv.selected = None;
                    }
                }
            }
            if finish
                && let Some(Gesture::Draw { page, tool: t, points }) = cv.gesture.clone()
                && page == cx.page
                && t == tool
            {
                cv.gesture = None;
                if let Some(shape) = drawn_shape(tool, &points) {
                    view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
                }
            }
            true
        }
        QuickTool::Comment(CommentTool::Callout) => {
            // Drag from what the callout points at to where its text box goes.
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.drag_started()
                && pressed_here
                && let Some(o) = origin
            {
                cv.gesture = Some(Gesture::Draw { page: cx.page, tool: CommentTool::Callout, points: vec![cx.to_user(o)] });
                cv.selected = None;
            }
            if let Some(Gesture::Draw { page, points, .. }) = cv.gesture.as_mut()
                && *page == cx.page
                && let Some(p) = pointer
            {
                points.truncate(1);
                points.push(cx.to_user(clamp_to(page_rect, p)));
            }
            if resp.drag_stopped()
                && let Some(Gesture::Draw { page, points, .. }) = cv.gesture.clone()
                && page == cx.page
            {
                cv.gesture = None;
                if let [point, at] = points[..]
                    && (point[0] - at[0]).hypot(point[1] - at[1]) >= 8.0
                {
                    cv.composer = Some(Composer { page, at, kind: ComposerKind::Callout { point }, text: String::new(), focus: true });
                }
            }
            true
        }
        QuickTool::Comment(CommentTool::Attach) => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                cv.attach_at = Some((cx.page, cx.to_user(p)));
                cv.selected = None;
            }
            true
        }
        QuickTool::Comment(CommentTool::Caret) => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                let (vx, vy) = cx.xf.screen_to_view(p);
                let snapped = cx.info.pages.get(cx.page).zip(caret_text.as_deref()).and_then(|(page, t)| snapped_caret(page, t, vx, vy));
                let (at, size) = snapped.unwrap_or_else(|| (cx.to_user(p), CARET_SIZE));
                cv.composer = Some(Composer { page: cx.page, at, kind: ComposerKind::Caret { size }, text: String::new(), focus: true });
                cv.selected = None;
            }
            true
        }
        QuickTool::Comment(tool @ (CommentTool::Note | CommentTool::TextBox)) => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            // A text box also takes a drag, as in Acrobat: the box is drawn at that size and its
            // text wraps inside it (#2669).
            if tool == CommentTool::TextBox {
                if resp.drag_started()
                    && pressed_here
                    && let Some(o) = origin
                {
                    cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![cx.to_user(o)] });
                    cv.selected = None;
                }
                if let Some(Gesture::Draw { page, points, .. }) = cv.gesture.as_mut()
                    && *page == cx.page
                    && let Some(p) = pointer
                {
                    points.truncate(1);
                    points.push(cx.to_user(clamp_to(page_rect, p)));
                }
                if resp.drag_stopped()
                    && let Some(Gesture::Draw { page, points, .. }) = cv.gesture.clone()
                    && page == cx.page
                {
                    cv.gesture = None;
                    if let (Some(first), Some(last)) = (points.first(), points.last()) {
                        let r = Rect::from_two_pos(cx.to_screen(*first), cx.to_screen(*last));
                        // A drag of a few pixels is a click: the box fits its text.
                        let drawn =
                            (r.width() >= 6.0 && r.height() >= 6.0).then(|| cx.info.pages.get(page)).flatten().map(|p| drawn_box(p, *first, *last));
                        let (at, size) = drawn.map_or((*first, None), |(at, size)| (at, Some(size)));
                        cv.composer = Some(Composer { page, at, kind: ComposerKind::TextBox { size }, text: String::new(), focus: true });
                    }
                }
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                let kind = if tool == CommentTool::Note { ComposerKind::Note } else { ComposerKind::TextBox { size: None } };
                cv.composer = Some(Composer { page: cx.page, at: cx.to_user(p), kind, text: String::new(), focus: true });
                cv.selected = None;
            }
            true
        }
        QuickTool::Select => select_input(ui, resp, cx, view, pointer, origin, pressed_here),
        QuickTool::Fill(_) if fill_grab => select_input(ui, resp, cx, view, pointer, origin, pressed_here),
        _ => false,
    }
}

/// With a Fill & Sign tool, the Fill & Sign marks already on the page are picked up as with the
/// Select tool, as in Acrobat: hovering one shows the move cursor, a click selects it, a drag
/// moves it and the selected one's handles resize it. Elsewhere the tool places its mark. While
/// the button is down the press decides; otherwise the pointer (egui clears the press origin on
/// release, so a click is located by the pointer).
pub(crate) fn fill_grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView) -> bool {
    matches!(cx.tool, QuickTool::Fill(_)) && grabs(ui, cx, view, |a| a.fill_sign)
}

/// With the Select tool, a comment that would move (not text markup) is picked up before a form
/// field under it, so a signature placed over a field can still be dragged off it.
pub(crate) fn select_grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView) -> bool {
    cx.tool == QuickTool::Select && grabs(ui, cx, view, |a| !is_markup(&a.subtype))
}

/// With a shape tool (rectangle, oval, line, polygon…), the selected comment's resize handles
/// are picked up as with the Select tool, so the shape just drawn can be resized straight away
/// (#2338). Only the handles: a press anywhere else, the shape itself included, still draws.
pub(crate) fn draw_grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView) -> bool {
    let QuickTool::Comment(tool) = cx.tool else { return false };
    if !(tool.draws() || tool.clicks_points()) || tool == CommentTool::Eraser || cx.hidden {
        return false;
    }
    let cv = &view.comments;
    if matches!(cv.gesture, Some(Gesture::Resize { page, .. } | Gesture::Reshape { page, .. }) if page == cx.page) {
        return true;
    }
    // A polygon being clicked out keeps its clicks, even near an old shape's handle.
    if cv.gesture.is_some() {
        return false;
    }
    press_point(ui, cx).is_some_and(|p| on_handle(cx, view, p, |_| true))
}

/// Whether the press (or, with no button down, the pointer) is on a `pick` comment or on the
/// selected one's resize handles.
fn grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView, pick: impl Fn(&Annotation) -> bool) -> bool {
    if cx.hidden {
        return false;
    }
    if matches!(view.comments.gesture, Some(Gesture::Move { page, .. } | Gesture::Resize { page, .. } | Gesture::Reshape { page, .. }) if page == cx.page)
    {
        return true;
    }
    let Some(p) = press_point(ui, cx) else { return false };
    on_handle(cx, view, p, &pick) || cx.hit(p).is_some_and(pick)
}

/// Where a gesture on this page is decided: the press while the button is down, otherwise the
/// pointer (egui clears the press origin on release). `None` off the page.
fn press_point(ui: &egui::Ui, cx: &PageCx<'_>) -> Option<Pos2> {
    let (pointer, origin, down) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.press_origin(), i.pointer.any_down()));
    (if down { origin } else { pointer }).filter(|p| cx.xf.rect.contains(*p))
}

/// Whether `p` is on a resize handle of the selected comment, when it is a resizable `pick`.
fn on_handle(cx: &PageCx<'_>, view: &DocView, p: Pos2, pick: impl Fn(&Annotation) -> bool) -> bool {
    view.comments.selected.filter(|(page, _)| *page == cx.page).and_then(|(_, i)| cx.get(i)).filter(|a| pick(a) && cx.allowed).is_some_and(|a| {
        (resizable(a) && HANDLES.into_iter().any(|h| handle_pos(cx.screen_rect(a), h).distance(p) <= 7.0))
            || (grabs_points(cx.tool) && reshapable(a) && point_handle(cx, a, p).is_some())
    })
}

fn clamp_to(r: Rect, p: Pos2) -> Pos2 {
    pos2(p.x.clamp(r.left(), r.right()), p.y.clamp(r.top(), r.bottom()))
}

/// The Select tool: hover, select, move, resize, edit, delete.
fn select_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    cx: &PageCx<'_>,
    view: &mut DocView,
    pointer: Option<Pos2>,
    origin: Option<Pos2>,
    pressed_here: bool,
) -> bool {
    let cv = &mut view.comments;
    let selected = cv.selected.filter(|(p, _)| *p == cx.page).and_then(|(_, i)| cx.get(i));
    // Handles of a selected, resizable comment.
    let handle_at = |p: Pos2| -> Option<(i8, i8)> {
        let a = selected.filter(|a| cx.allowed && resizable(a))?;
        let r = cx.screen_rect(a);
        HANDLES.into_iter().find(|h| handle_pos(r, *h).distance(p) <= 7.0)
    };
    // Ends and corners of a selected line, connected lines, polygon or cloud.
    let point_at =
        |p: Pos2| -> Option<usize> { selected.filter(|a| cx.allowed && grabs_points(cx.tool) && reshapable(a)).and_then(|a| point_handle(cx, a, p)) };
    let mut consumed = false;
    if let Some(p) = pointer {
        if point_at(p).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if let Some(h) = handle_at(p) {
            ui.ctx().set_cursor_icon(match h {
                (0, _) => egui::CursorIcon::ResizeVertical,
                (_, 0) => egui::CursorIcon::ResizeHorizontal,
                (-1, -1) | (1, 1) => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::ResizeNeSw,
            });
        } else if let Some(a) = cx.hit(p) {
            let movable = cx.allowed && !is_markup(&a.subtype);
            ui.ctx().set_cursor_icon(if movable { egui::CursorIcon::Move } else { egui::CursorIcon::PointingHand });
        }
    }
    if resp.drag_started()
        && pressed_here
        && let Some(o) = origin
    {
        if let (Some(point), Some(a)) = (point_at(o), selected) {
            let points = a.points.iter().map(|q| [f64::from(q[0]), f64::from(q[1])]).collect();
            cv.gesture = Some(Gesture::Reshape { page: cx.page, index: a.index, point, points });
            consumed = true;
        } else if let (Some(h), Some(a)) = (handle_at(o), selected) {
            // On screen, the image is also turned by the view's rotation.
            let turned = !cx.xf.rot.is_multiple_of(180);
            let aspect_ratio = view.signature_drag.aspect_ratio(cx.page, a.index).map(|ratio| if turned { ratio.recip() } else { ratio });
            // A drawn signature keeps its shape from the corners, as image signatures do.
            let aspect_ratio = aspect_ratio.or_else(|| {
                let r = cx.screen_rect(a);
                (a.fill_sign && a.subtype == "Ink" && r.height() > 0.0).then(|| r.width() / r.height())
            });
            cv.gesture = Some(Gesture::Resize { page: cx.page, index: a.index, handle: h, from: o, aspect_ratio });
            consumed = true;
        } else if let Some(a) = cx.hit(o)
            && !is_markup(&a.subtype)
        {
            cv.selected = Some((cx.page, a.index));
            cv.reveal = true;
            if cx.allowed {
                cv.gesture = Some(Gesture::Move { page: cx.page, index: a.index, from: o });
            }
            consumed = true;
        }
    }
    if matches!(cv.gesture, Some(Gesture::Move { page, .. } | Gesture::Resize { page, .. } | Gesture::Reshape { page, .. }) if page == cx.page) {
        consumed = true;
    }
    if resp.drag_stopped()
        && let Some(p) = pointer
    {
        match cv.gesture.clone() {
            Some(Gesture::Move { page, index, from }) if page == cx.page => {
                cv.gesture = None;
                let (a, b) = (cx.to_user(from), cx.to_user(p));
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                if from.distance(p) >= 2.0 {
                    view.pending_edit = Some(Edit::MoveAnnotation { page, index, dx, dy });
                }
            }
            Some(Gesture::Resize { page, index, handle, from, aspect_ratio }) if page == cx.page => {
                cv.gesture = None;
                if let Some(a) = cx.get(index) {
                    let from_rect = cx.screen_rect(a);
                    let shift = ui.input(|i| i.modifiers.shift);
                    let r = resized(from_rect, handle, p - from, corner_ratio(from_rect, aspect_ratio, shift));
                    let rect = a.rect_for_shown(cx.rect_to_user(r), cx.rotation());
                    if r.width() >= 4.0 && r.height() >= 4.0 {
                        view.pending_edit = Some(Edit::ResizeAnnotation { page, index, rect });
                    }
                }
            }
            Some(Gesture::Reshape { page, index, point, points }) if page == cx.page => {
                cv.gesture = None;
                let shift = ui.input(|i| i.modifiers.shift);
                let moved = points.get(point).is_some_and(|q| cx.to_screen(*q).distance(p) >= 2.0);
                if moved {
                    view.pending_edit = Some(Edit::ReshapeAnnotation { page, index, points: reshaped(&points, point, cx.to_user(p), shift) });
                }
            }
            _ => {}
        }
    }
    // (egui clears the press origin on release, so clicks are located by the pointer.)
    let on_page = pointer.is_some_and(|p| cx.xf.rect.contains(p));
    if resp.clicked() && on_page {
        let follow_link = cx.tool == QuickTool::Select
            && !consumed
            && cv.gesture.is_none()
            && !resp.double_clicked()
            && pointer
                .is_some_and(|p| cx.info.links.iter().any(|link| link.page == cx.page && cx.xf.user_rect(cx.info, cx.page, link.rect).contains(p)));
        match pointer.and_then(|p| cx.hit(p)) {
            Some(a) if a.subtype == "Highlight" && follow_link => {
                // A plain click follows the link beneath text highlighting. Keep other
                // annotations and active gestures in control of their own input.
                cv.selected = None;
            }
            Some(a) => {
                cv.selected = Some((cx.page, a.index));
                cv.reveal = true;
                consumed = true;
            }
            None => cv.selected = None,
        }
    }
    if resp.double_clicked()
        && on_page
        && cx.allowed
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
    {
        cv.selected = Some((cx.page, a.index));
        cv.composer = Some(Composer {
            page: cx.page,
            at: edit_anchor(cx.info, a),
            kind: ComposerKind::Edit(a.index),
            text: a.contents.clone().unwrap_or_default(),
            focus: true,
        });
        consumed = true;
    }
    consumed
}

/// After text selection ran: a markup tool turns a finished selection into a comment; Replace
/// Text asks for the replacement.
pub(crate) fn page_after_text(resp: &egui::Response, cx: &PageCx<'_>, view: &mut DocView) {
    let QuickTool::Comment(tool) = cx.tool else { return };
    if tool == CommentTool::ReplaceText && cx.allowed && (resp.drag_stopped() || resp.double_clicked()) {
        if let Some((page, quads)) = view.selection_quads(cx.info).filter(|(p, _)| *p == cx.page) {
            view.clear_selection();
            let last = quads.last().copied().unwrap_or_default();
            let at = [last[2].max(last[6]), last[1].max(last[3])];
            view.comments.replace = Some(quads);
            view.comments.composer = Some(Composer { page, at, kind: ComposerKind::Replace, text: String::new(), focus: true });
        }
        return;
    }
    if tool.markup().is_none() || !cx.allowed || !(resp.drag_stopped() || resp.double_clicked()) {
        return;
    }
    if let Some(kind) = tool.markup()
        && let Some((page, quads)) = view.selection_quads(cx.info).filter(|(p, _)| *p == cx.page)
    {
        view.clear_selection();
        view.pending_edit = Some(new_comment(cx, tool, Shape::TextMarkup { kind, quads }, String::new()));
        let _ = page;
    }
}

/// Paint comment selection, hover and gesture previews on a page.
pub(crate) fn paint_page(ui: &egui::Ui, painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView) {
    let cv = &view.comments;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    // A redaction mark shows its fill while the pointer is over it, as Acrobat's does, so the
    // marked area reads before anything is applied.
    if cv.gesture.is_none()
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
        && a.subtype == "Redact"
    {
        let ([r, g, b], opacity) = cx.redact_mark;
        let fill = Color32::from_rgba_unmultiplied((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, (opacity.clamp(0.0, 1.0) * 255.0) as u8);
        for rect in cx.redact_rects(a) {
            painter.rect_filled(rect, CornerRadius::ZERO, fill);
        }
    }
    if cv.gesture.is_none()
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
        && (cx.tool == QuickTool::Select || matches!(cx.tool, QuickTool::Fill(_)) && a.fill_sign)
        && cv.selected != Some((cx.page, a.index))
    {
        for r in cx.screen_rects(a) {
            painter.rect_stroke(r.expand(2.0), CornerRadius::same(2), Stroke::new(1.0, SELECT_BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
    if let Some((page, index)) = cv.selected
        && page == cx.page
        && let Some(a) = cx.get(index)
    {
        if is_markup(&a.subtype) && !a.quads.is_empty() {
            for r in cx.screen_rects(a) {
                painter.rect_stroke(r.expand(2.0), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Middle);
            }
            return paint_gesture(painter, cx, view);
        }
        if view.signature_drag.contains(page, index) && matches!(cv.gesture, Some(Gesture::Move { page: p, index: i, .. }) if (p, i) == (page, index))
        {
            return paint_gesture(painter, cx, view);
        }
        let shift = ui.input(|i| i.modifiers.shift);
        if cx.allowed && grabs_points(cx.tool) && reshapable(a) {
            return paint_points(painter, cx, view, a, pointer, shift);
        }
        let r = cx.adjusted_rect(a, cv.gesture.as_ref(), pointer, shift, view.pending_edit.as_ref()).expand(2.0);
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Middle);
        if cx.allowed && resizable(a) {
            for h in HANDLES {
                let c = handle_pos(r, h);
                painter.circle(c, 4.5, Color32::WHITE, Stroke::new(1.0, SELECT_BLUE));
            }
        }
    }
    paint_gesture(painter, cx, view);
}

/// A selected line, connected lines, polygon or cloud: a handle on each end or corner and, while
/// one is dragged (or its edit is on its way), the new shape over the old.
/// The points a selected line, polygon or cloud is drawn with while it changes: an end or corner
/// being dragged, the whole comment being moved by its body, or the edit just released (until the
/// page is drawn again). `None` while it isn't changing.
fn changing_points(
    gesture: Option<&Gesture>,
    pending: Option<&Edit>,
    (page, index): (usize, usize),
    points: &[[f64; 2]],
    to_user: impl Fn(Pos2) -> [f64; 2],
    pointer: Option<Pos2>,
    shift: bool,
) -> Option<Vec<[f64; 2]>> {
    let shifted = |dx: f64, dy: f64| points.iter().map(|q| [q[0] + dx, q[1] + dy]).collect();
    match (gesture, pending) {
        (Some(Gesture::Reshape { page: p, index: i, point, points: from }), _) if (*p, *i) == (page, index) => {
            pointer.map(|at| reshaped(from, *point, to_user(at), shift))
        }
        (Some(Gesture::Move { page: p, index: i, from }), _) if (*p, *i) == (page, index) => pointer.map(|at| {
            let (a, b) = (to_user(*from), to_user(at));
            shifted(b[0] - a[0], b[1] - a[1])
        }),
        (_, Some(Edit::ReshapeAnnotation { page: p, index: i, points: to })) if (*p, *i) == (page, index) => Some(to.clone()),
        (_, Some(Edit::MoveAnnotation { page: p, index: i, dx, dy })) if (*p, *i) == (page, index) => Some(shifted(*dx, *dy)),
        _ => None,
    }
}

fn paint_points(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView, a: &Annotation, pointer: Option<Pos2>, shift: bool) {
    let cv = &view.comments;
    let points: Vec<[f64; 2]> = a.points.iter().map(|q| [f64::from(q[0]), f64::from(q[1])]).collect();
    let changing = changing_points(cv.gesture.as_ref(), view.pending_edit.as_ref(), (cx.page, a.index), &points, |p| cx.to_user(p), pointer, shift);
    let changed = changing.is_some();
    let points = changing.unwrap_or(points);
    let screen: Vec<Pos2> = points.iter().map(|q| cx.to_screen(*q)).collect();
    if changed {
        let stroke = Stroke::new(1.5, SELECT_BLUE);
        painter.add(if a.subtype == "Polygon" {
            egui::Shape::closed_line(screen.clone(), stroke)
        } else {
            egui::Shape::line(screen.clone(), stroke)
        });
    }
    for c in screen {
        painter.circle(c, 4.5, Color32::WHITE, Stroke::new(1.0, SELECT_BLUE));
    }
    paint_gesture(painter, cx, view);
}

fn paint_gesture(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView) {
    let now = painter.ctx().input(|i| i.time);
    for s in view.comments.settling.iter().filter(|s| s.page == cx.page && now - s.since < SETTLE_SECS) {
        paint_draw(painter, cx, &s.tool, &s.points);
    }
    if let Some(Gesture::Draw { page, tool, points }) = &view.comments.gesture
        && *page == cx.page
    {
        paint_draw(painter, cx, tool, points);
    }
}

/// A drawing tool's shape from its points (a live drag, or a released one still settling).
fn paint_draw(painter: &egui::Painter, cx: &PageCx<'_>, tool: &CommentTool, points: &[[f64; 2]]) {
    {
        let style = cx.prefs.style(*tool);
        let zoom = cx.xf.zoom();
        let stroke = Stroke::new((style.width as f32 * zoom).max(1.0), color32(style.color));
        let pts: Vec<Pos2> = points.iter().map(|p| cx.to_screen(*p)).collect();
        match tool {
            CommentTool::Ink => {
                painter.add(egui::Shape::line(pts, stroke));
            }
            CommentTool::Eraser => {
                // As wide as what it rubs out (see `erase`).
                let width = (2.0 * eraser_radius_px(style.width)) as f32;
                painter.add(egui::Shape::line(pts, Stroke::new(width, Color32::from_gray(128).gamma_multiply(0.35))));
            }
            CommentTool::Line | CommentTool::Arrow if pts.len() == 2 => {
                painter.line_segment([pts[0], pts[1]], stroke);
                if *tool == CommentTool::Arrow {
                    let d = (pts[0] - pts[1]).normalized();
                    let s = (6.0 + 3.0 * style.width as f32) * zoom;
                    let rot = |v: egui::Vec2, a: f32| vec2(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
                    for a in [0.52f32, -0.52] {
                        painter.line_segment([pts[1], pts[1] + rot(d, a) * s], stroke);
                    }
                }
            }
            CommentTool::Rectangle if pts.len() == 2 => {
                painter.rect_stroke(Rect::from_two_pos(pts[0], pts[1]), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            CommentTool::Oval if pts.len() == 2 => {
                let r = Rect::from_two_pos(pts[0], pts[1]);
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            CommentTool::Polygon | CommentTool::PolyLine | CommentTool::Cloud => {
                // The vertices so far, and a rubber band to the pointer.
                let mut line = pts.clone();
                if let Some(p) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                    line.push(p);
                }
                painter.add(egui::Shape::line(line, stroke));
                if let Some(first) = pts.first().filter(|_| *tool != CommentTool::PolyLine && pts.len() >= 3) {
                    painter.circle_stroke(*first, 5.0, Stroke::new(1.0, SELECT_BLUE));
                }
            }
            CommentTool::TextBox if pts.len() == 2 => {
                painter.rect_stroke(Rect::from_two_pos(pts[0], pts[1]), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Inside);
            }
            CommentTool::Callout if pts.len() == 2 => {
                painter.line_segment([pts[0], pts[1]], stroke);
                painter.rect_stroke(Rect::from_min_size(pts[1], vec2(160.0, 40.0) * zoom), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            _ => {}
        }
    }
}

/// The shape a finished drawing gesture makes (`None` if it is too small to mean anything).
fn drawn_shape(tool: CommentTool, points: &[[f64; 2]]) -> Option<Shape> {
    let (first, last) = (*points.first()?, *points.last()?);
    let far = (first[0] - last[0]).hypot(first[1] - last[1]) >= 2.0;
    let rect = [first[0].min(last[0]), first[1].min(last[1]), first[0].max(last[0]), first[1].max(last[1])];
    let big = rect[2] - rect[0] >= 2.0 && rect[3] - rect[1] >= 2.0;
    match tool {
        CommentTool::Ink if !points.is_empty() => Some(Shape::Ink { strokes: vec![points.to_vec()] }),
        CommentTool::Line | CommentTool::Arrow if far => Some(Shape::Line {
            from: first,
            to: last,
            start: LineEnding::None,
            end: if tool == CommentTool::Arrow { LineEnding::OpenArrow } else { LineEnding::None },
        }),
        CommentTool::Rectangle if big => Some(Shape::Rectangle { rect }),
        CommentTool::Oval if big => Some(Shape::Oval { rect }),
        CommentTool::Polygon | CommentTool::Cloud if points.len() >= 3 => {
            Some(Shape::Polygon { vertices: points.to_vec(), cloud: tool == CommentTool::Cloud })
        }
        CommentTool::PolyLine if points.len() >= 2 => {
            Some(Shape::PolyLine { vertices: points.to_vec(), start: LineEnding::None, end: LineEnding::None })
        }
        _ => None,
    }
}

/// The eraser's radius on screen, in pixels, from its thickness setting (#3747): three pixels
/// per point, so the default 2 pt keeps the 12-pixel eraser it always had, and the thickness
/// slider's 0.5–12 pt gives 3 to 72 pixels across. A setting that isn't a number is the default.
fn eraser_radius_px(width: f64) -> f64 {
    3.0 * if width.is_finite() { width.clamp(0.5, 12.0) } else { 2.0 }
}

/// Eraser: rub out drawings along `path` (user space); one undo step for all of them. It rubs
/// as wide on screen as its preview, whatever the zoom.
fn erase(cx: &PageCx<'_>, path: &[[f64; 2]]) -> Option<Edit> {
    let zoom = f64::from(cx.xf.zoom());
    // A page with no size on screen has nothing to rub out (and no finite radius).
    if !(zoom.is_finite() && zoom > 0.0) {
        return None;
    }
    // Exactly the preview's width: no floor, which at a high zoom rubbed past it.
    let radius = eraser_radius_px(cx.prefs.style(CommentTool::Eraser).width) / zoom;
    let b = path.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]);
    let mut hits: Vec<usize> = cx
        .comments()
        .filter(|a| a.subtype == "Ink" && !a.locked)
        .filter(|a| {
            let r = a.rect;
            (r[0] as f64) <= b[2] + radius && (r[2] as f64) >= b[0] - radius && (r[1] as f64) <= b[3] + radius && (r[3] as f64) >= b[1] - radius
        })
        .map(|a| a.index)
        .collect();
    if hits.is_empty() {
        return None;
    }
    // Later indices first: deleting an emptied drawing shifts the ones after it.
    hits.sort_unstable_by(|a, b| b.cmp(a));
    let edits = hits.into_iter().map(|index| Edit::EraseInk { page: cx.page, index, path: path.to_vec(), radius }).collect();
    Some(Edit::Batch { label: "Erase".into(), edits })
}

/// Where the composer for editing `a` opens: the top-right corner of the comment as drawn on the
/// displayed page.
fn edit_anchor(info: &DocInfo, a: &Annotation) -> [f64; 2] {
    let Some(p) = info.pages.get(a.page) else { return [f64::from(a.rect[2]), f64::from(a.rect[3])] };
    let r = a.shown_rect(p.rotation);
    let (u, v) = (p.user_to_view(r[0], r[1]), p.user_to_view(r[2], r[3]));
    p.view_to_user(u[0].max(v[0]), u[1].min(v[1])).map(f64::from)
}

/// Whether comment `index` on the composer's page is at least partly inside `viewport`, as drawn.
fn edited_in_view(cx: &PageCx<'_>, index: usize, viewport: Rect) -> bool {
    cx.info.annotations.iter().any(|a| a.page == cx.page && a.index == index && viewport.intersects(cx.screen_rect(a)))
}

fn new_comment(cx: &PageCx<'_>, tool: CommentTool, shape: Shape, contents: String) -> Edit {
    Edit::AddAnnotation(NewAnnotation { page: cx.page, shape, style: cx.prefs.style(tool), contents, author: cx.prefs.author.clone() })
}

/// The floating composer card (new note, new text box, edit). Returns the edit to apply.
pub(crate) fn composer(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, prefs: &CommentPrefs) -> Option<Edit> {
    let c = view.comments.composer.as_ref()?;
    let page = c.page;
    let Some(xf) = view.page_xform(page) else {
        // Scrolled away: keep the draft until the page is back, and give it the keyboard then.
        if let Some(c) = view.comments.composer.as_mut() {
            c.focus = true;
        }
        return None;
    };
    let cx = PageCx { page, xf: &xf, info, tool: QuickTool::Select, prefs, allowed: true, hidden: false, redact_mark: ([0.89, 0.13, 0.13], 1.0) };
    let anchor = cx.to_screen(c.at);
    let viewport = view.viewport_rect();
    let t = Tokens::get(ctx);
    let mut post = false;
    let mut cancel = false;
    let c = view.comments.composer.as_mut()?;
    let title = match c.kind {
        ComposerKind::Note => "Sticky note",
        ComposerKind::TextBox { .. } => "Text box",
        ComposerKind::Callout { .. } => "Callout",
        ComposerKind::Caret { .. } => "Inserted text",
        ComposerKind::Replace => "Replacement text",
        ComposerKind::Edit(_) => "Edit comment",
    };
    // Its comment scrolled out of the document: keep the draft until it is back, rather than
    // pushing the card into the window over the mode bar and toolbar (#4021). An edited comment
    // that is only partly scrolled away keeps its card, at the nearest point inside the document.
    let mut anchor = anchor;
    if !viewport.contains(anchor) {
        let part_visible = matches!(c.kind, ComposerKind::Edit(index) if edited_in_view(&cx, index, viewport));
        if !part_visible {
            c.focus = true;
            return None;
        }
        anchor = anchor.clamp(viewport.min, viewport.max);
    }
    let pos = pos2(anchor.x + 12.0, anchor.y);
    let area = egui::Area::new(egui::Id::new(("comment-composer", view.id.0))).order(egui::Order::Foreground).constrain_to(viewport);
    area.fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(12)).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
            ui.set_width(260.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::bidi::visual_auto(&prefs.author)).font(theme::semibold(13.0)));
                ui.label(egui::RichText::new(tl!(title)).font(theme::regular(11.5)).color(t.text_faint));
            });
            ui.add_space(6.0);
            let hint = match c.kind {
                ComposerKind::TextBox { .. } | ComposerKind::Callout { .. } => tl!("Type text"),
                ComposerKind::Caret { .. } => tl!("Text to insert"),
                ComposerKind::Replace => tl!("Replacement text"),
                _ => tl!("Add a comment"),
            };
            let edit =
                ui.add(egui::TextEdit::multiline(&mut c.text).hint_text(hint).desired_rows(3).desired_width(f32::INFINITY).id_salt("composer-text"));
            if c.focus {
                edit.request_focus();
                c.focus = false;
            }
            let enter = submit_key(ui, &edit, false);
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_post = !c.text.trim().is_empty() || matches!(c.kind, ComposerKind::Edit(_));
                    if ui
                        .add_enabled(
                            can_post,
                            egui::Button::new(egui::RichText::new(tl!("Post")).color(Color32::WHITE)).fill(t.accent).corner_radius(14),
                        )
                        .clicked()
                        || (enter && can_post)
                    {
                        post = true;
                    }
                    if ui.add(egui::Button::new(tl!("Cancel")).corner_radius(14)).clicked() {
                        cancel = true;
                    }
                });
            });
        });
    });
    if cancel {
        view.comments.composer = None;
        return None;
    }
    if !post {
        return None;
    }
    let c = view.comments.composer.take()?;
    let text = c.text.trim_end().to_string();
    match c.kind {
        ComposerKind::Edit(index) => Some(Edit::SetAnnotationContents { page, index, text }),
        ComposerKind::Note => {
            view.comments.tool_done = true;
            Some(new_comment(&cx, CommentTool::Note, Shape::Note { at: c.at, icon: NoteIcon::Comment }, text))
        }
        ComposerKind::TextBox { size } => {
            view.comments.tool_done = true;
            let rotation = page_rotation(info, page);
            let border = prefs.style(CommentTool::TextBox).width;
            let rect = match (size, info.pages.get(page)) {
                (Some(size), Some(p)) => drawn_text_box_rect(p, c.at, size, &text, 12.0, border),
                _ => text_box_rect(c.at, &text, 12.0, rotation),
            };
            Some(new_comment(&cx, CommentTool::TextBox, Shape::TextBox { rect, font_size: 12.0 }, text))
        }
        ComposerKind::Callout { point } => {
            view.comments.tool_done = true;
            // A callout's leader and box stay in user-space axes (its appearance isn't turned).
            let rect = text_box_rect(c.at, &text, 10.0, 0);
            let side = if point[0] < rect[0] {
                rect[0]
            } else if point[0] > rect[2] {
                rect[2]
            } else {
                (rect[0] + rect[2]) / 2.0
            };
            let knee = [(point[0] + side) / 2.0, (rect[1] + rect[3]) / 2.0];
            Some(new_comment(&cx, CommentTool::Callout, Shape::Callout { rect, knee, point, font_size: 10.0, ending: LineEnding::OpenArrow }, text))
        }
        ComposerKind::Replace => {
            view.comments.tool_done = true;
            let quads = view.comments.replace.take()?;
            Some(Edit::ReplaceText {
                page,
                quads,
                text,
                author: prefs.author.clone(),
                strike: prefs.style(CommentTool::ReplaceText),
                caret: prefs.style(CommentTool::Caret),
            })
        }
        ComposerKind::Caret { size } => {
            view.comments.tool_done = true;
            let [x, y] = c.at;
            let half = size / 2.0;
            Some(new_comment(&cx, CommentTool::Caret, Shape::Caret { rect: [x - half, y - size, x + half, y] }, text))
        }
    }
}

/// Submit only from the focused comment editor. Match each key event's modifiers exactly,
/// so Shift+Enter stays a newline and another editor cannot submit this draft. Replies also
/// keep their existing unmodified Enter-to-post behavior.
pub(crate) fn submit_key(ui: &mut egui::Ui, field: &egui::Response, plain_enter: bool) -> bool {
    field.has_focus()
        && ui.input_mut(|input| {
            let mut submit = false;
            input.events.retain(|event| {
                let matched = matches!(event,
                    egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. }
                        if modifiers.matches_exact(egui::Modifiers::CTRL)
                            || modifiers.matches_exact(egui::Modifiers::COMMAND)
                            || plain_enter && modifiers.matches_exact(egui::Modifiers::NONE)
                );
                submit |= matched;
                !matched
            });
            submit
        })
}

/// The side of the caret's box, in user space units, away from text.
const CARET_SIZE: f64 = 8.0;

/// [`caret_snap`] in the page's user space: the caret's tip and the side of its box.
///
/// The caret is drawn upright in user space, so it only snaps on an unrotated page, where up on
/// the page is up on screen; its size goes through the same mapping as the tip, so a page with
/// `/UserUnit` gets a caret of the same size relative to its text.
pub(crate) fn snapped_caret(page: &PageInfo, text: &PageText, vx: f32, vy: f32) -> Option<([f64; 2], f64)> {
    if !page.rotation.is_multiple_of(360) {
        return None;
    }
    let ([x, y], size) = caret_snap(text, vx, vy)?;
    let tip = page.view_to_user(x, y);
    let foot = page.view_to_user(x, y + size);
    let side = f64::from(tip[1] - foot[1]);
    (side.is_finite() && side > 0.0).then(|| ([f64::from(tip[0]), f64::from(tip[1])], side))
}

/// Insert text: where a click puts the caret's tip (view space) and how tall the caret is.
///
/// On upright, horizontal text the tip snaps to the nearer edge of the character under (or
/// next to) the click, on the line's baseline, and the caret's side is half the text's em size
/// (clamped to 4–36 view-space points; [`snapped_caret`] maps it to user space): the
/// proposal marks an insertion point in the text, as a proofreader's caret does. Elsewhere
/// (away from text, or on turned, sloped or vertical text) it returns `None` and the caret stays
/// at the click. A ligature is one glyph, so the caret goes before or after it, never inside.
pub(crate) fn caret_snap(text: &PageText, vx: f32, vy: f32) -> Option<([f32; 2], f32)> {
    let g = text.glyphs.get(text.nearest(vx, vy)?)?;
    if !(g.direction[0] > 0.0 && g.direction[1].abs() <= 0.02) {
        return None;
    }
    // Close enough to belong to this glyph: within half an em of its box.
    let [x0, y0, x1, y1] = g.rect;
    let slack = (y1 - y0) / 2.0;
    if !(slack.is_finite() && slack > 0.0) || vx < x0 - slack || vx > x1 + slack || vy < y0 - slack || vy > y1 + slack {
        return None;
    }
    // The em box runs from 0.2 em below the baseline to 0.8 em above it (`pdfcraft_render`'s
    // text extraction); `quad` is its bottom edge in the writing direction, then its top edge
    // backwards. Take the side of the box the click is nearer to: its foot and head.
    let [q0, q1, q2, q3] = g.quad;
    let (foot, head) = if vx < (q0[0] + q1[0]) / 2.0 { (q0, q3) } else { (q1, q2) };
    let em = (head[0] - foot[0]).hypot(head[1] - foot[1]);
    if !(em.is_finite() && em > 0.0) {
        return None;
    }
    let baseline = [foot[0] + 0.2 * (head[0] - foot[0]), foot[1] + 0.2 * (head[1] - foot[1])];
    Some((baseline, (em / 2.0).clamp(4.0, 36.0)))
}

/// A text box sized to its text (at most 300 pt wide), hanging from its top-left corner.
/// The box for `text` at `size` with its displayed top-left corner at `at`, on a page turned by
/// `rotation` (its `/Rotate`): on a quarter-turned page it is taller than wide in user space.
pub fn text_box_rect(at: [f64; 2], text: &str, size: f64, rotation: i64) -> [f64; 4] {
    use pdfcraft_engine::annot_text::{text_width, wrap};
    let pad = 2.0;
    let longest = text.lines().map(|l| text_width(l, size)).fold(0.0, f64::max);
    let w = (longest + 2.0 * pad + 4.0).clamp(40.0, 300.0);
    let lines = wrap(text, size, w - 2.0 * pad).len().max(1);
    let h = lines as f64 * size * 1.2 + 2.0 * pad + 2.0;
    pdfcraft_engine::text_box_at(rotation, at, w, h).unwrap_or([at[0], at[1] - h, at[0] + w, at[1]])
}

/// The box dragged between user-space points `a` and `b`: its displayed top-left corner (user
/// space) and its displayed width and height. Measured in the page's view space, so the view's
/// own rotation and zoom (View > Rotate View) do not turn or scale it.
fn drawn_box(page: &PageInfo, a: [f64; 2], b: [f64; 2]) -> ([f64; 2], [f64; 2]) {
    let (a, b) = (page.user_to_view(a[0] as f32, a[1] as f32), page.user_to_view(b[0] as f32, b[1] as f32));
    let at = page.view_to_user(a[0].min(b[0]), a[1].min(b[1]));
    ([f64::from(at[0]), f64::from(at[1])], [f64::from((a[0] - b[0]).abs()), f64::from((a[1] - b[1]).abs())])
}

/// A text box dragged out `size` (displayed width and height) from its displayed top-left corner
/// `at`, with a border `border` wide: the text wraps to its width inside the border, as its
/// appearance lays it out. When the text needs more lines the box grows down, and moves up to stay
/// on the page; text that cannot fit even so keeps the drawn height and is clipped.
pub fn drawn_text_box_rect(page: &PageInfo, at: [f64; 2], size: [f64; 2], text: &str, font_size: f64, border: f64) -> [f64; 4] {
    use pdfcraft_engine::annot_text::wrap;
    let pad = 2.0 + border.max(0.0);
    let [w, drawn] = size;
    let lines = wrap(text, font_size, (w - 2.0 * pad).max(1.0)).len().max(1);
    let needed = lines as f64 * font_size * 1.2 + 2.0 * pad + 2.0;
    let [vx, vy] = page.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
    let bottom = f64::from(page.height);
    let (top, h) = if needed <= drawn || needed > bottom { (vy, drawn) } else { (vy.min(bottom - needed).max(0.0), needed) };
    let at = page.view_to_user(vx as f32, top as f32).map(f64::from);
    pdfcraft_engine::text_box_at(i64::from(page.rotation), at, w, h).unwrap_or([at[0], at[1] - h, at[0] + w, at[1]])
}

/// The `/Rotate` of `page` (0 when it is unknown).
pub(crate) fn page_rotation(info: &DocInfo, page: usize) -> i64 {
    info.pages.get(page).map_or(0, |p| i64::from(p.rotation))
}

/// Delete / Escape handling for comments (only while no text field has focus).
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView, tool: &mut QuickTool, allowed: bool) {
    if !ctx.input(|i| i.pointer.any_down()) {
        view.comments.cancelled_drag = false;
    }
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let cv = &mut view.comments;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        // One step back per press: cancel the gesture, then deselect, then drop the tool.
        let cancelled = cv.gesture.take().is_some() || cv.selected.take().is_some();
        if !cancelled && matches!(tool, QuickTool::Comment(_)) {
            *tool = QuickTool::Select;
        }
    }
    if allowed
        && let Some((page, index)) = cv.selected
        && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        cv.selected = None;
        view.pending_edit = Some(Edit::DeleteAnnotation { page, index });
    }
}

/// The arrow keys move the selected comment a screen pixel; with ⌘/Ctrl or Shift, ten. Up is up
/// on screen whatever the zoom, the page's rotation and the view's; the keys are taken before
/// the view scrolls or turns the page with them. Each press is one undo step. Text markup stays
/// with its text, and a locked comment where it is.
pub(crate) fn nudge_keys(ctx: &egui::Context, view: &mut DocView, info: &DocInfo) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() {
        return;
    }
    let cv = &view.comments;
    if cv.gesture.is_some() || cv.composer.is_some() {
        return;
    }
    let Some((page, index)) = cv.selected else { return };
    let movable =
        info.annotations.iter().any(|a| a.page == page && a.index == index && a.in_reply_to.is_none() && !is_markup(&a.subtype) && !a.locked);
    let (Some(xf), Some(p)) = (view.page_xform(page), info.pages.get(page)) else { return };
    if !movable {
        return;
    }
    use egui::{Key, Modifiers};
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        for (key, direction) in
            [(Key::ArrowLeft, vec2(-1.0, 0.0)), (Key::ArrowRight, vec2(1.0, 0.0)), (Key::ArrowUp, vec2(0.0, -1.0)), (Key::ArrowDown, vec2(0.0, 1.0))]
        {
            // The modified patterns first: a plain one also matches a Shift press.
            if i.consume_key(Modifiers::COMMAND, key) || i.consume_key(Modifiers::SHIFT, key) {
                step += direction * 10.0;
            } else if i.consume_key(Modifiers::NONE, key) {
                step += direction;
            }
        }
    });
    if step == egui::Vec2::ZERO {
        return;
    }
    // The step on screen, in user space: from the page's centre to `step` pixels away.
    let user = |s: Pos2| {
        let (vx, vy) = xf.screen_to_view(s);
        let u = p.view_to_user(vx, vy);
        [f64::from(u[0]), f64::from(u[1])]
    };
    let (a, b) = (user(xf.rect.center()), user(xf.rect.center() + step));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    if dx.is_finite() && dy.is_finite() && (dx != 0.0 || dy != 0.0) {
        view.pending_edit = Some(Edit::MoveAnnotation { page, index, dx, dy });
    }
}

/// Save attachment… and, for a PDF, Open in a new tab, first in a file attachment comment's
/// menus as in the Attachments panel: the comment is where the reader finds its file. Reading
/// the file changes nothing, so a read-only document offers them too.
pub(crate) fn attachment_items(ui: &mut egui::Ui, view: &mut DocView, info: &DocInfo, a: &Annotation) {
    let source = pdfcraft_render::AttachmentSource::Annotation { page: a.page, index: a.index };
    let Some(file) = info.attachments.iter().find(|f| f.source == source) else { return };
    if ui.button(tl!("Save attachment…")).clicked() {
        view.comments.attachment_request = Some((a.page, a.index, false));
        ui.close();
    }
    if file.name.to_lowercase().ends_with(".pdf") && ui.button(tl!("Open in a new tab")).clicked() {
        view.comments.attachment_request = Some((a.page, a.index, true));
        ui.close();
    }
    ui.separator();
}

/// Items of the canvas context menu for the selected comment (or the page).
/// `can_modify`: the document's pages may change (Flatten comments draws into them).
pub(crate) fn context_menu(
    ui: &mut egui::Ui,
    view: &mut DocView,
    info: &DocInfo,
    prefs: &CommentPrefs,
    allowed: bool,
    can_modify: bool,
) -> Option<CanvasAction> {
    let mut action = None;
    let selected = view.comments.selected.and_then(|(p, i)| info.annotations.iter().find(|a| a.page == p && a.index == i && a.in_reply_to.is_none()));
    match selected {
        Some(a) => {
            let (page, index) = (a.page, a.index);
            attachment_items(ui, view, info, a);
            if ui.add_enabled(allowed, egui::Button::new(tl!("Edit text…"))).clicked() {
                view.comments.composer = Some(Composer {
                    page,
                    at: edit_anchor(info, a),
                    kind: ComposerKind::Edit(index),
                    text: a.contents.clone().unwrap_or_default(),
                    focus: true,
                });
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Reply"))).clicked() {
                view.comments.reveal = true;
                action = Some(CanvasAction::OpenComments);
                ui.close();
            }
            ui.add_enabled_ui(allowed, |ui| {
                ui.menu_button(tl!("Set status"), |ui| {
                    for s in [ReviewState::None, ReviewState::Accepted, ReviewState::Cancelled, ReviewState::Completed, ReviewState::Rejected] {
                        if ui.button(tl!(s.name())).clicked() {
                            action =
                                Some(CanvasAction::Edit(Box::new(Edit::SetAnnotationStatus { page, index, state: s, author: prefs.author.clone() })));
                            ui.close();
                        }
                    }
                });
                let keep_open = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                let _ = egui::containers::menu::SubMenuButton::new(tl!("Colour")).config(keep_open).ui(ui, |ui| {
                    if let Some(c) = swatch_grid(ui, a.color.map(|c| c.map(f64::from))) {
                        action = Some(CanvasAction::Edit(Box::new(Edit::StyleAnnotation {
                            page,
                            index,
                            color: Some(c),
                            opacity: None,
                            width: None,
                            endings: None,
                        })));
                        ui.close();
                    }
                });
            });
            let thread: Vec<&pdfcraft_render::Annotation> = info.annotations.iter().filter(|r| a.name.is_some() && r.in_reply_to == a.name).collect();
            let marked = crate::comments_panel::is_marked(&thread);
            if ui.add_enabled(allowed, egui::Button::new(tl!(if marked { "Remove checkmark" } else { "Mark with checkmark" }))).clicked() {
                action = Some(CanvasAction::Edit(Box::new(Edit::MarkAnnotation { page, index, marked: !marked, author: prefs.author.clone() })));
                ui.close();
            }
            if ui.button(tl!("Copy text")).clicked() {
                ui.ctx().copy_text(a.contents.clone().unwrap_or_default());
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(allowed && !a.locked, egui::Button::new(tl!("Delete"))).clicked() {
                view.comments.selected = None;
                action = Some(CanvasAction::Edit(Box::new(Edit::DeleteAnnotation { page, index })));
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Properties…"))).clicked() {
                action = Some(CanvasAction::Properties(page, index));
                ui.close();
            }
            if ui.add_enabled(tool_for(a).is_some(), egui::Button::new(tl!("Make Current Properties Default"))).clicked() {
                view.comments.default_request = Some((page, index));
                ui.close();
            }
        }
        None => {
            if let Some((page, at)) = view.comments.context_at
                && ui.add_enabled(allowed, egui::Button::new(tl!("Add a sticky note here"))).clicked()
            {
                view.comments.composer = Some(Composer { page, at, kind: ComposerKind::Note, text: String::new(), focus: true });
                ui.close();
            }
            if ui.button(tl!("Comments panel")).clicked() {
                action = Some(CanvasAction::OpenComments);
                ui.close();
            }
        }
    }
    // Every comment of the document into its pages, as Comment ▸ Flatten comments does: signed
    // or reviewed pages whose comments should no longer move.
    if !info.annotations.is_empty() {
        ui.separator();
        if ui.add_enabled(can_modify, egui::Button::new(tl!("Flatten comments"))).clicked() {
            view.comments.selected = None;
            action = Some(CanvasAction::Edit(Box::new(Edit::Flatten { comments: true, fields: false })));
            ui.close();
        }
    }
    action
}

/// What the canvas asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Edit(Box<Edit>),
    OpenComments,
    /// Comment Properties for (page, index).
    Properties(usize, usize),
    /// Copy Image: image (page, index in `page_images`) to the clipboard.
    CopyImage(usize, usize),
    /// Copy Image in Original Format: the same image as a file (JPEG as stored, else PNG).
    CopyImageFile(usize, usize),
}

/// How many custom colours the "Recent colours" row keeps.
const RECENT_CUSTOM: usize = 5;

fn recent_custom_id() -> egui::Id {
    egui::Id::new("pdfcraft-recent-custom-colours")
}

/// Custom colours picked with "Other colour…" this session, newest first (#1193).
pub fn recent_custom_colours(ctx: &egui::Context) -> Vec<Rgb> {
    ctx.data(|d| d.get_temp::<Vec<Rgb>>(recent_custom_id())).unwrap_or_default()
}

/// Put `c` first in the recent custom colours (once, at most [`RECENT_CUSTOM`]). Colours that are
/// already swatches are not repeated there.
pub fn remember_custom_colour(ctx: &egui::Context, c: Rgb) {
    if !c.iter().all(|v| v.is_finite()) || SWATCHES.iter().any(|(_, s)| same_colour(s, &c)) {
        return;
    }
    let c = c.map(|v| v.clamp(0.0, 1.0));
    ctx.data_mut(|d| {
        let recent = d.get_temp_mut_or_default::<Vec<Rgb>>(recent_custom_id());
        recent.retain(|r| !same_colour(r, &c));
        recent.insert(0, c);
        recent.truncate(RECENT_CUSTOM);
    });
}

fn same_colour(a: &Rgb, b: &Rgb) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
}

fn rgb_of(c: Color32) -> Rgb {
    [c.r(), c.g(), c.b()].map(|v| f64::from(v) / 255.0)
}

/// One round swatch; `on` rings it as the current colour.
fn swatch(ui: &mut egui::Ui, c: Rgb, on: bool, tooltip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip));
    ui.painter().circle_filled(r.center(), 9.0, color32(c));
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(40)));
    if on || resp.hovered() {
        ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.5, SELECT_BLUE));
    }
    resp.on_hover_text(tooltip)
}

/// The "+" swatch that opens "Other colour…": a ring with a plus, the size of a swatch.
fn other_colour_swatch(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let ink = ui.visuals().widgets.inactive.fg_stroke.color;
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, ink.gamma_multiply(0.6)));
    let c = r.center();
    for d in [vec2(4.5, 0.0), vec2(0.0, 4.5)] {
        ui.painter().line_segment([c - d, c + d], Stroke::new(1.5, ink));
    }
    if open || resp.hovered() {
        ui.painter().circle_stroke(c, 11.0, Stroke::new(1.5, SELECT_BLUE));
    }
    let label = tl!("Other colour…");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, label));
    resp.on_hover_text(label)
}

/// A grid of colour swatches with a "+" swatch for "Other colour…" (a full picker, as Acrobat's
/// quick colours offer) and, once used, the recent custom colours; returns the colour chosen.
/// The "+" sits in the first row, so the grid is no taller than before until the picker opens.
pub fn swatch_grid(ui: &mut egui::Ui, current: Option<Rgb>) -> Option<Rgb> {
    let mut picked = None;
    let on = |c: &Rgb| current.is_some_and(|cur| same_colour(&cur, c));
    // Keyed on this grid's own place in the parent, so two grids in one dialog open their
    // pickers separately.
    let base = ui.next_auto_id();
    ui.skip_ahead_auto_ids(1);
    let open_id = base.with("other-colour");
    let value_id = open_id.with("value");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    egui::Grid::new(base.with("swatches")).spacing(vec2(6.0, 6.0)).show(ui, |ui| {
        for (i, (name, c)) in SWATCHES.iter().enumerate() {
            if swatch(ui, *c, on(c), tl!(name)).clicked() {
                picked = Some(*c);
            }
            if i == 4 && other_colour_swatch(ui, open).clicked() {
                open = !open;
                // Start from the current colour each time the picker opens.
                ui.data_mut(|d| d.insert_temp(value_id, current.map_or(Color32::BLACK, color32)));
            }
            if i % 5 == 4 {
                ui.end_row();
            }
        }
    });
    let recent = recent_custom_colours(ui.ctx());
    if !recent.is_empty() {
        ui.label(egui::RichText::new(tl!("Recent colours")).small());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for c in &recent {
                let [r, g, b, _] = color32(*c).to_array();
                if swatch(ui, *c, on(c), &format!("#{r:02X}{g:02X}{b:02X}")).clicked() {
                    picked = Some(*c);
                }
            }
        });
    }
    if open && picked.is_none() {
        let mut value = ui.data(|d| d.get_temp::<Color32>(value_id)).unwrap_or_else(|| current.map_or(Color32::BLACK, color32));
        egui::widgets::color_picker::color_picker_color32(ui, &mut value, egui::widgets::color_picker::Alpha::Opaque);
        ui.data_mut(|d| d.insert_temp(value_id, value));
        // Dragging in the picker previews nothing; Apply commits once (one undo step).
        if ui.button(tl!("Apply")).clicked() {
            let c = rgb_of(value);
            remember_custom_colour(ui.ctx(), c);
            picked = Some(c);
        }
    }
    if picked.is_some() {
        open = false;
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    picked
}

/// "No fill" and the colour swatches; returns the choice clicked (`Some(None)`: no fill).
pub fn fill_picker(ui: &mut egui::Ui, current: Option<Rgb>) -> Option<Option<Rgb>> {
    let mut picked = None;
    ui.push_id("fill-picker", |ui| {
        ui.vertical(|ui| {
            if ui.selectable_label(current.is_none(), tl!("No fill")).clicked() {
                picked = Some(None);
            }
            if let Some(c) = swatch_grid(ui, current) {
                picked = Some(Some(c));
            }
        });
    });
    picked
}

/// The comment tools' extra quick-bar controls: pin, colour, fill, opacity and thickness.
pub(crate) fn quick_bar_controls(ui: &mut egui::Ui, tool: CommentTool, prefs: &mut CommentPrefs) -> Option<Rgb> {
    let style = prefs.style(tool);
    let mut selected_color = None;
    if icons::button(ui, "pin", 32.0, prefs.pinned, if prefs.pinned { tl!("Keep tool selected: on") } else { tl!("Keep tool selected") }).clicked() {
        prefs.pinned = !prefs.pinned;
    }
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    ui.painter().circle_filled(r.center(), 9.0, color32(style.color));
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(50)));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Colour")));
    let resp = resp.on_hover_text(tl!("Colour"));
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        if let Some(c) = swatch_grid(ui, Some(style.color)) {
            prefs.set_color(tool, c);
            selected_color = Some(c);
            ui.close();
        }
    });
    if tool.has_fill() {
        let resp = fill_button(ui, style.fill);
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.label(egui::RichText::new(tl!("Fill colour")).font(theme::semibold(12.0)));
            if let Some(f) = fill_picker(ui, style.fill) {
                prefs.set_fill(tool, f);
                ui.close();
            }
        });
    }
    let resp = icons::button(ui, "blend", 32.0, false, tl!("Opacity"));
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(180.0);
        let mut percent = (style.opacity * 100.0).round();
        ui.label(egui::RichText::new(tl!("Opacity")).font(theme::semibold(12.0)));
        if ui.add(egui::Slider::new(&mut percent, 10.0..=100.0).step_by(5.0).suffix(" %")).changed() {
            prefs.set_opacity(tool, percent / 100.0);
        }
    });
    if tool.has_width() {
        let resp = icons::button(ui, "sliders-horizontal", 32.0, false, tl!("Line thickness"));
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(180.0);
            let mut w = style.width;
            ui.label(egui::RichText::new(tl!("Line thickness")).font(theme::semibold(12.0)));
            if ui.add(egui::Slider::new(&mut w, tool.min_width()..=12.0).step_by(0.5).suffix(" pt")).changed() {
                prefs.set_width(tool, w);
            }
        });
    }
    selected_color
}

/// The quick bar's fill button: a filled square in the fill colour, or a crossed-out outline
/// for no fill.
fn fill_button(ui: &mut egui::Ui, fill: Option<Rgb>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let sq = Rect::from_center_size(r.center(), vec2(16.0, 16.0));
    let edge = Stroke::new(1.0, Color32::from_black_alpha(90));
    match fill {
        Some(c) => {
            ui.painter().rect_filled(sq, 2.0, color32(c));
        }
        None => {
            ui.painter().line_segment([sq.left_bottom(), sq.right_top()], Stroke::new(1.5, Color32::from_rgb(0xD3, 0x2F, 0x2F)));
        }
    }
    ui.painter().rect_stroke(sq, 2.0, edge, egui::StrokeKind::Inside);
    if resp.hovered() {
        ui.painter().rect_stroke(r.shrink(2.0), 6.0, Stroke::new(1.0, Color32::from_black_alpha(40)), egui::StrokeKind::Inside);
    }
    let label = match fill {
        Some(_) => tl!("Fill colour"),
        None => tl!("Fill colour: none"),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp.on_hover_text(label)
}

/// Status badge icon and label for a review state name.
pub fn status_badge(state: &str) -> Option<(&'static str, &'static str, Color32)> {
    match state {
        "Accepted" => Some(("thumbs-up", "Accepted", Color32::from_rgb(0x2D, 0x9D, 0x5B))),
        "Rejected" => Some(("thumbs-down", "Rejected", Color32::from_rgb(0xD3, 0x2F, 0x2F))),
        "Cancelled" => Some(("circle-x", "Cancelled", Color32::from_rgb(0x8E, 0x8E, 0x8E))),
        "Completed" => Some(("check", "Completed", Color32::from_rgb(0x2D, 0x9D, 0x5B))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A selected line is drawn where it is going: moved by its body (both ends follow the
    /// pointer), with an end dragged (that end alone), and as released until the page is drawn.
    #[test]
    fn a_moved_or_reshaped_line_is_drawn_at_its_new_points() {
        let line = [[10.0, 10.0], [50.0, 10.0]];
        let to_user = |p: Pos2| [f64::from(p.x), f64::from(p.y)];
        let moving = Gesture::Move { page: 0, index: 3, from: pos2(20.0, 10.0) };
        let at = Some(pos2(25.0, 30.0));
        assert_eq!(changing_points(Some(&moving), None, (0, 3), &line, to_user, at, false), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
        assert_eq!(changing_points(Some(&moving), None, (0, 4), &line, to_user, at, false), None, "another comment's move");
        let released = Edit::MoveAnnotation { page: 0, index: 3, dx: 5.0, dy: 20.0 };
        assert_eq!(changing_points(None, Some(&released), (0, 3), &line, to_user, None, false), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
        let dragging = Gesture::Reshape { page: 0, index: 3, point: 1, points: line.to_vec() };
        let to = Some(pos2(60.0, 10.0));
        assert_eq!(changing_points(Some(&dragging), None, (0, 3), &line, to_user, to, false), Some(vec![[10.0, 10.0], [60.0, 10.0]]));
        assert_eq!(changing_points(None, None, (0, 3), &line, to_user, to, false), None, "not changing");
    }

    /// A sticky note on a page turned a quarter is drawn upright beside its `/Rect`: an edit
    /// composer stays open while the drawn note is partly in view, even with `/Rect` out of it.
    #[test]
    fn an_upright_note_on_a_turned_page_is_in_view_where_it_is_drawn() {
        let note = |no_rotate| Annotation {
            page: 0,
            subtype: "Text".into(),
            author: None,
            contents: None,
            modified: None,
            name: None,
            in_reply_to: None,
            rect: [40.0, 20.0, 70.0, 40.0],
            color: None,
            index: 0,
            state: None,
            quads: Vec::new(),
            locked: false,
            intent: None,
            fill_sign: false,
            no_rotate,
            points: Vec::new(),
        };
        let page = PageInfo { width: 140.0, height: 200.0, label: String::new(), crop: [0.0, 0.0, 200.0, 140.0], rotation: 90 };
        let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(140.0, 200.0)), rot: 0, pw: 140.0, ph: 200.0 };
        let prefs = CommentPrefs::default();
        // Screen x is user y here: `/Rect` spans x 20..40, the upright note x 40..70.
        let viewport = Rect::from_min_max(pos2(50.0, 0.0), pos2(140.0, 200.0));
        for (no_rotate, in_view) in [(true, true), (false, false)] {
            let info = DocInfo { pages: vec![page.clone()], annotations: vec![note(no_rotate)], ..Default::default() };
            let cx = PageCx {
                page: 0,
                xf: &xf,
                info: &info,
                tool: QuickTool::Select,
                prefs: &prefs,
                allowed: true,
                hidden: false,
                redact_mark: ([0.0; 3], 1.0),
            };
            assert_eq!(edited_in_view(&cx, 0, viewport), in_view, "no_rotate: {no_rotate}");
        }
    }

    use pdfcraft_render::TextGlyph;

    /// One glyph per character, `w` points wide, on a line of `em`-point text whose baseline is at `y`, drawn
    /// along `direction` (a unit vector in view space, y down).
    fn line(chars: &str, x: f32, y: f32, w: f32, em: f32, direction: [f32; 2]) -> PageText {
        let [dx, dy] = direction;
        // "Up" for the glyph: its writing direction turned a quarter counterclockwise (y down).
        let [ux, uy] = [dy, -dx];
        let glyphs = chars
            .chars()
            .enumerate()
            .map(|(i, c)| {
                let start = [x + dx * w * i as f32, y + dy * w * i as f32];
                let at = |along: f32, up: f32| [start[0] + dx * along + ux * up, start[1] + dy * along + uy * up];
                let quad = [at(0.0, -0.2 * em), at(w, -0.2 * em), at(w, 0.8 * em), at(0.0, 0.8 * em)];
                let xs = quad.map(|p| p[0]);
                let ys = quad.map(|p| p[1]);
                let rect = [
                    xs.into_iter().fold(f32::INFINITY, f32::min),
                    ys.into_iter().fold(f32::INFINITY, f32::min),
                    xs.into_iter().fold(f32::NEG_INFINITY, f32::max),
                    ys.into_iter().fold(f32::NEG_INFINITY, f32::max),
                ];
                TextGlyph { text: c.to_string(), rect, quad, direction }
            })
            .collect::<Vec<_>>();
        let n = glyphs.len();
        PageText { glyphs, line_of: vec![0; n], space_before: vec![false; n] }
    }

    fn near(a: Option<([f32; 2], f32)>, b: ([f32; 2], f32)) -> bool {
        a.is_some_and(|([x, y], s)| (x - b.0[0]).abs() < 1e-3 && (y - b.0[1]).abs() < 1e-3 && (s - b.1).abs() < 1e-3)
    }

    #[test]
    fn the_caret_snaps_to_the_nearer_edge_on_the_baseline() {
        let text = line("abcd", 20.0, 100.0, 10.0, 14.0, [1.0, 0.0]);
        // "c" spans 40..50: its left half snaps before it, its right half after it.
        assert!(near(caret_snap(&text, 43.0, 95.0), ([40.0, 100.0], 7.0)));
        assert!(near(caret_snap(&text, 47.0, 92.0), ([50.0, 100.0], 7.0)));
        // Just past the last character, on the line: after it.
        assert!(near(caret_snap(&text, 63.0, 99.0), ([60.0, 100.0], 7.0)));
    }

    #[test]
    fn the_snapped_caret_follows_the_page_into_user_space() {
        let text = line("abcd", 20.0, 100.0, 10.0, 14.0, [1.0, 0.0]);
        let page = |rotation, user_unit: f32| PageInfo {
            width: 300.0 * user_unit,
            height: 200.0 * user_unit,
            label: "1".into(),
            crop: [0.0, 0.0, 300.0, 200.0],
            rotation,
        };
        // An unrotated page: view y = 100 is user y = 100 (y up from the bottom of a 200 pt page).
        let (tip, side) = snapped_caret(&page(0, 1.0), &text, 43.0, 95.0).expect("snaps");
        assert!((tip[0] - 40.0).abs() < 1e-3 && (tip[1] - 100.0).abs() < 1e-3 && (side - 7.0).abs() < 1e-3);
        // /UserUnit 2: the view is twice as large, so the same text is half as many user units.
        let doubled = line("abcd", 40.0, 200.0, 20.0, 28.0, [1.0, 0.0]);
        let (tip, side) = snapped_caret(&page(0, 2.0), &doubled, 86.0, 190.0).expect("snaps");
        assert!((tip[0] - 40.0).abs() < 1e-3 && (tip[1] - 100.0).abs() < 1e-3 && (side - 7.0).abs() < 1e-3, "{tip:?} {side}");
        // A rotated page: the caret would lie on its side, so it stays at the click.
        for rotation in [90, 180, 270] {
            assert_eq!(snapped_caret(&page(rotation, 1.0), &text, 43.0, 95.0), None, "/Rotate {rotation}");
        }
    }

    #[test]
    fn away_from_upright_horizontal_text_the_caret_stays_at_the_click() {
        let text = line("abcd", 20.0, 100.0, 10.0, 14.0, [1.0, 0.0]);
        assert_eq!(caret_snap(&text, 200.0, 100.0), None, "far to the right");
        assert_eq!(caret_snap(&text, 40.0, 40.0), None, "far above the line");
        assert_eq!(caret_snap(&PageText::default(), 40.0, 100.0), None, "no text");
        for (direction, what) in [([0.0, 1.0], "vertical"), ([-1.0, 0.0], "upside down"), ([0.995, 0.0998], "sloped")] {
            let turned = line("abcd", 20.0, 100.0, 10.0, 14.0, direction);
            let [x0, y0, x1, y1] = turned.glyphs[2].rect;
            assert_eq!(caret_snap(&turned, (x0 + x1) / 2.0, (y0 + y1) / 2.0), None, "{what}");
        }
    }

    #[test]
    fn recent_custom_colours_dedupe_cap_and_skip_swatches() {
        // #1193: "Other colour…" remembers what it applied, newest first.
        let ctx = egui::Context::default();
        assert!(recent_custom_colours(&ctx).is_empty());
        for i in 0..7 {
            remember_custom_colour(&ctx, [0.1 * f64::from(i), 0.3, 0.7]);
        }
        let recent = recent_custom_colours(&ctx);
        assert_eq!(recent.len(), RECENT_CUSTOM, "capped");
        assert!(same_colour(&recent[0], &[0.6, 0.3, 0.7]), "newest first: {recent:?}");
        remember_custom_colour(&ctx, [0.2, 0.3, 0.7]);
        let again = recent_custom_colours(&ctx);
        assert_eq!(again.len(), RECENT_CUSTOM);
        assert!(
            same_colour(&again[0], &[0.2, 0.3, 0.7]) && again.iter().filter(|c| same_colour(c, &[0.2, 0.3, 0.7])).count() == 1,
            "moved, not repeated"
        );
        // Swatches and unusable values are not remembered; out-of-range ones are clamped.
        remember_custom_colour(&ctx, SWATCHES[0].1);
        remember_custom_colour(&ctx, [f64::NAN, 0.0, 0.0]);
        assert!(same_colour(&recent_custom_colours(&ctx)[0], &[0.2, 0.3, 0.7]));
        remember_custom_colour(&ctx, [1.7, -0.5, 0.25]);
        assert!(same_colour(&recent_custom_colours(&ctx)[0], &[1.0, 0.0, 0.25]));
    }

    #[test]
    fn other_colour_applies_the_picked_colour_once() {
        // #1193: the swatch grid has a full picker; Apply returns its colour and remembers it.
        use egui_kittest::kittest::Queryable;
        let current = [0.2, 0.4, 0.6];
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, picked: &mut Vec<Rgb>| {
                if let Some(c) = swatch_grid(ui, Some(current)) {
                    picked.push(c);
                }
            },
            Vec::new(),
        );
        h.run();
        assert!(h.query_by_label("Apply").is_none(), "the picker starts closed");
        assert!(h.query_by_label("Recent colours").is_none());
        h.get_by_label("Other colour…").click();
        h.run();
        h.get_by_label("Apply").click();
        h.run();
        let picked = h.state().clone();
        assert_eq!(picked.len(), 1, "one choice: {picked:?}");
        assert!(same_colour(&picked[0], &current), "the picker opens on the current colour: {picked:?}");
        assert!(h.query_by_label("Apply").is_none(), "applying closes the picker");
        assert!(h.query_by_label("Recent colours").is_some(), "the colour is offered again");
    }

    #[test]
    fn two_grids_in_one_parent_open_their_pickers_separately() {
        // Redaction properties and Prepare a form draw two grids in one parent ui.
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::new_ui(|ui| {
            let _ = swatch_grid(ui, Some([0.2, 0.4, 0.6]));
            let _ = swatch_grid(ui, Some([0.6, 0.4, 0.2]));
        });
        h.run();
        let plus = h.query_all_by_label("Other colour…").collect::<Vec<_>>();
        assert_eq!(plus.len(), 2);
        plus[0].click();
        h.run();
        assert_eq!(h.query_all_by_label("Apply").count(), 1, "only the first grid's picker opens");
    }

    #[test]
    fn other_colour_opens_the_picker_inside_the_quick_bar_popup() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, prefs: &mut CommentPrefs| {
                let _ = quick_bar_controls(ui, CommentTool::Highlight, prefs);
            },
            CommentPrefs::default(),
        );
        h.run();
        h.get_by_label("Colour").click();
        h.run();
        h.get_by_label("Other colour…").click();
        h.run();
        assert!(h.query_by_label("Apply").is_some(), "the popup stays open with the picker showing");
        h.get_by_label("Apply").click();
        h.run();
        assert!(h.query_by_label("Other colour…").is_none(), "applying a colour closes the popup");
    }

    #[test]
    fn other_colour_opens_the_picker_inside_the_quick_bar_fill_popup() {
        use egui_kittest::kittest::Queryable;
        let prefs = CommentPrefs::default();
        let fill_label = if prefs.style(CommentTool::Rectangle).fill.is_some() { "Fill colour" } else { "Fill colour: none" };
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, prefs: &mut CommentPrefs| {
                let _ = quick_bar_controls(ui, CommentTool::Rectangle, prefs);
            },
            prefs,
        );
        h.ctx.set_fonts(theme::font_definitions());
        h.run();
        h.get_by_label(fill_label).click();
        h.run();
        h.get_by_label("Other colour…").click();
        h.run();
        assert!(h.query_by_label("Apply").is_some(), "the popup stays open with the picker showing");
        h.get_by_label("Apply").click();
        h.run();
        assert!(h.query_by_label("No fill").is_none(), "applying a colour closes the popup");
        assert!(h.state().style(CommentTool::Rectangle).fill.is_some());
    }

    #[test]
    fn context_menu_colour_submenu_keeps_its_picker_open_and_a_swatch_closes_the_menu() {
        use egui_kittest::kittest::Queryable;
        let info = DocInfo {
            pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
            annotations: vec![Annotation {
                page: 0,
                subtype: "Square".into(),
                author: None,
                contents: None,
                modified: None,
                name: None,
                in_reply_to: None,
                rect: [10.0, 10.0, 50.0, 50.0],
                color: None,
                index: 0,
                state: None,
                quads: Vec::new(),
                locked: false,
                intent: None,
                fill_sign: false,
                points: Vec::new(),
                no_rotate: false,
            }],
            ..Default::default()
        };
        let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default());
        view.comments.selected = Some((0, 0));
        let prefs = CommentPrefs::default();
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, (view, actions): &mut (DocView, Vec<CanvasAction>)| {
                ui.button("Page").context_menu(|ui| {
                    if let Some(a) = context_menu(ui, view, &info, &prefs, true, true) {
                        actions.push(a);
                    }
                });
            },
            (view, Vec::new()),
        );
        h.run();
        h.get_by_label("Page").click_secondary();
        h.run();
        h.get_by_label("Colour ⏵").click();
        h.run();
        h.get_by_label("Other colour…").click();
        h.run();
        assert!(h.query_by_label("Apply").is_some(), "the submenu stays open with the picker showing");
        h.get_by_label("Red").click();
        h.run();
        assert!(h.query_by_label("Edit text…").is_none(), "picking a swatch closes the whole menu");
        let actions = &h.state().1;
        assert!(
            matches!(actions.as_slice(), [CanvasAction::Edit(e)] if matches!(**e, Edit::StyleAnnotation { page: 0, index: 0, color: Some(_), .. })),
            "{actions:?}"
        );
    }

    #[test]
    fn shift_keeps_the_current_proportions_unless_the_box_has_its_own() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 132.0));
        assert_eq!(corner_ratio(r, None, false), None);
        assert_eq!(corner_ratio(r, None, true), Some(3.0));
        assert_eq!(corner_ratio(r, Some(2.0), false), Some(2.0));
        assert_eq!(corner_ratio(r, Some(2.0), true), Some(2.0));
        // A degenerate box has no proportions to keep (no division by zero).
        let flat = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 100.0));
        assert_eq!(corner_ratio(flat, None, true), None);
        // Dragging a corner with Shift held scales the box instead of stretching it.
        let out = resized(r, (1, 1), vec2(48.0, 0.0), corner_ratio(r, None, true));
        assert_eq!(out.min, r.min);
        assert!((out.width() / out.height() - 3.0).abs() < 0.001);
    }

    #[test]
    fn proportional_corners_keep_the_opposite_anchor_and_edges_stretch_one_axis() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 132.0));
        for (hx, hy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            for (dx, dy) in [(24.0, 0.0), (0.0, 16.0), (-48.0, -20.0), (-200.0, -200.0)] {
                let out = resized(r, (hx, hy), vec2(f32::from(hx) * dx, f32::from(hy) * dy), Some(3.0));
                assert_eq!(handle_pos(out, (-hx, -hy)), handle_pos(r, (-hx, -hy)));
                assert!((out.width() / out.height() - 3.0).abs() < 0.001);
                assert!(out.width() >= 4.0 && out.height() >= 4.0);
            }
        }
        for handle in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let out = resized(r, handle, vec2(24.0, 16.0), Some(3.0));
            assert_eq!(out, resized(r, handle, vec2(24.0, 16.0), None));
            assert_eq!(if handle.0 == 0 { out.width() } else { out.height() }, if handle.0 == 0 { r.width() } else { r.height() });
        }
        // Ordinary comment corners keep their existing independent width/height behavior.
        let free = resized(r, (1, 1), vec2(24.0, 0.0), None);
        assert_eq!(free.size(), vec2(120.0, 32.0));
    }

    #[test]
    fn tools_round_trip_through_their_commands() {
        for t in ALL {
            assert_eq!(CommentTool::from_command(t.command()), Some(t));
            assert!(pdfcraft_engine::commands::command(t.command()).is_some(), "{} is registered", t.command());
            assert!(icons::exists(t.icon()), "icon {}", t.icon());
            assert!(GROUPS[t.group()].contains(&t));
        }
    }

    #[test]
    fn drawn_shapes_ignore_slips() {
        assert_eq!(drawn_shape(CommentTool::Rectangle, &[[0.0, 0.0], [1.0, 1.0]]), None);
        assert_eq!(drawn_shape(CommentTool::Rectangle, &[[10.0, 0.0], [0.0, 10.0]]), Some(Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }));
        assert_eq!(drawn_shape(CommentTool::Ink, &[[0.0, 0.0]]), Some(Shape::Ink { strokes: vec![vec![[0.0, 0.0]]] }));
        assert!(matches!(
            drawn_shape(CommentTool::Arrow, &[[0.0, 0.0], [5.0, 0.0]]),
            Some(Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. })
        ));
    }

    #[test]
    fn text_boxes_fit_their_text() {
        let r = text_box_rect([100.0, 500.0], "Hi", 12.0, 0);
        assert_eq!((r[0], r[3]), (100.0, 500.0));
        assert!(r[2] - r[0] < 60.0);
        let long = text_box_rect([0.0, 500.0], &"word ".repeat(80), 12.0, 0);
        assert_eq!(long[2], 300.0);
        assert!(long[3] - long[1] > 50.0);
    }
}
