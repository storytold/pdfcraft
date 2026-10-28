use super::geometry::{corner_ratio, is_markup, resized};
use super::{Annotation, CommentPrefs, CommentTool, DocInfo, Edit, PagePointer, PageXform, Pos2, QuickTool, Rect};

/// The user's login name, which Acrobat uses as the default comment author.
pub(super) fn login_name() -> String {
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
pub(super) const SETTLE_SECS: f64 = 20.0;

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
    pub(super) cancelled_drag: bool,
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
    /// The pointer as the pages see it this frame.
    pub ptr: &'a PagePointer,
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
    pub(super) fn to_user(&self, p: Pos2) -> [f64; 2] {
        let (vx, vy) = self.xf.screen_to_view(p);
        let u = self.info.pages[self.page].view_to_user(vx, vy);
        [u[0] as f64, u[1] as f64]
    }

    pub(super) fn to_screen(&self, p: [f64; 2]) -> Pos2 {
        let v = self.info.pages[self.page].user_to_view(p[0] as f32, p[1] as f32);
        self.xf.norm_to_screen(v[0] / self.xf.pw, v[1] / self.xf.ph)
    }

    /// The page's clockwise `/Rotate`.
    pub(super) fn rotation(&self) -> u16 {
        self.info.pages.get(self.page).map_or(0, |p| p.rotation)
    }

    /// Where a comment is drawn on screen (a sticky note stays upright on a turned page).
    pub(super) fn screen_rect(&self, a: &Annotation) -> Rect {
        self.xf.user_rect(self.info, self.page, a.shown_rect(self.rotation()))
    }

    /// Top-level comments on this page, in paint order.
    pub(super) fn comments(&self) -> impl Iterator<Item = &Annotation> {
        self.info.annotations.iter().filter(move |a| !self.hidden && a.page == self.page && a.is_comment() && a.subtype != "Popup")
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
    pub(super) fn redact_rects(&self, a: &Annotation) -> Vec<Rect> {
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
    pub(super) fn hit(&self, p: Pos2) -> Option<&Annotation> {
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
