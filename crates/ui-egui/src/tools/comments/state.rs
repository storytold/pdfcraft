use std::collections::HashMap;

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
    /// A notice for the app to show (the page input has no app).
    pub notice: Option<&'static str>,
    /// The page's paths for snapping (see [`SnapCache`]).
    pub snap_cache: SnapCache,
    /// Where the pointer would snap to right now: (page, point in user space, what it is), for
    /// the marker on the page.
    pub snap_hover: Option<(usize, [f64; 2], measure::snap::SnapKind)>,
    /// The nearest snap target under the pointer (its first candidate) and its page, while there
    /// is one: Tab cycles from it.
    pub snap_first: Option<(usize, [f64; 2])>,
    /// Tab's place among the candidates under the pointer: (their first one, presses forward).
    /// Another set of candidates starts again from its first.
    pub snap_cycle: Option<([f64; 2], isize)>,
    /// The point of the comment being moved that goes onto a snap target, as an index into its
    /// [`PageCx::move_anchors`]: `None` for whichever corner comes near a target. Tab and
    /// Shift+Tab move on to the next or previous point, put where the one in use is.
    pub move_anchor: Option<usize>,
    /// Tab presses while a comment is moved, applied on its page (see [`Self::move_anchor`]).
    pub anchor_steps: isize,
    /// The moved comment's anchor, for its marker: (page, point in user space, name).
    pub anchor_shown: Option<(usize, [f64; 2], &'static str)>,
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
    /// Tab (`step` 1) or Shift+Tab (-1) over a snap marker: the next or previous candidate under
    /// the pointer. Returns whether there was a marker to cycle.
    pub fn cycle_snap(&mut self, step: isize) -> bool {
        let Some((_, first)) = self.snap_first else { return false };
        self.snap_cycle = Some(match self.snap_cycle {
            Some((f, n)) if measure::distance(f, first) <= 1e-6 => (f, n.saturating_add(step)),
            _ => (first, step),
        });
        true
    }

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

    /// The annotation at `gone` was just deleted; `before` and `after` are the document's
    /// annotations until then and now. The selection and the inline editor stay on what they
    /// were on, where it now is (deleting a reply keeps its comment selected), and close when it
    /// went too.
    pub(crate) fn annotation_deleted(&mut self, before: &[Annotation], after: &[Annotation], gone: (usize, usize)) {
        self.selected = self.selected.and_then(|k| after_delete(before, after, gone, k));
        self.editing = self.editing.take().and_then(|(p, i, text)| after_delete(before, after, gone, (p, i)).map(|(p, i)| (p, i, text)));
        if self.editing.is_none() {
            self.editor_focus = None;
        }
    }

    /// Undo or Redo just ran. Either may have put annotations back or taken them away, on any
    /// page, so a (page, index) held here may now name another one. The selection and the inline
    /// editor close, its unsaved text with it, rather than Save writing that text over whatever
    /// took its place.
    pub(crate) fn history_stepped(&mut self) {
        self.selected = None;
        self.editing = None;
        self.editor_focus = None;
    }
}

/// Where the annotation at `keep` is once the one at `gone` was deleted, or `None` when it went
/// with it. Deleting takes the annotation's replies (at any depth) and pop-ups out of its page's
/// `/Annots` (`pdfcraft_annot::delete_annotation`), and everything after them moves up, so `keep`
/// is found by its place among the page's annotations that stay. When the page lost other
/// annotations than those, or the one in that place reads differently, it is not guessed at.
pub(super) fn after_delete(before: &[Annotation], after: &[Annotation], gone: (usize, usize), keep: (usize, usize)) -> Option<(usize, usize)> {
    if keep == gone {
        return None;
    }
    if keep.0 != gone.0 {
        return Some(keep);
    }
    let (old, new) = (on_page(before, gone.0), on_page(after, gone.0));
    let goes = in_thread(&old, gone);
    let stays: Vec<&Annotation> = old.iter().copied().filter(|a| !goes(a.index)).collect();
    if stays.len() != new.len() {
        return None;
    }
    let rank = stays.iter().position(|a| a.index == keep.1)?;
    let (was, now) = (stays.get(rank)?, new.get(rank)?);
    let same = (&was.subtype, &was.name, &was.author, &was.contents, was.rect) == (&now.subtype, &now.name, &now.author, &now.contents, now.rect);
    same.then_some((now.page, now.index))
}

/// Whether the annotation at an index on `top`'s page is `top` or a reply to it at any depth:
/// what deleting `top` takes along. `page` is that page's listed annotations. Only replies on the
/// page go with it (`pdfcraft_annot::delete_annotation` follows `/IRT` through the page's
/// `/Annots`).
fn in_thread(page: &[&Annotation], top: (usize, usize)) -> impl Fn(usize) -> bool {
    let parents: HashMap<usize, usize> = page.iter().filter_map(|a| a.parent.filter(|p| p.0 == top.0).map(|p| (a.index, p.1))).collect();
    move |mut i: usize| {
        // `link_replies` settles every chain within 64 steps.
        for _ in 0..=64 {
            if i == top.1 {
                return true;
            }
            let Some(&up) = parents.get(&i) else { return false };
            i = up;
        }
        false
    }
}

/// The annotations listed on `page`, in `/Annots` order.
fn on_page(list: &[Annotation], page: usize) -> Vec<&Annotation> {
    let mut v: Vec<&Annotation> = list.iter().filter(|a| a.page == page).collect();
    v.sort_by_key(|a| a.index);
    v
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
    /// The page's paths, when the tool's points snap to the drawing (Snap to drawing is on and
    /// the tool sets points or corners).
    pub snap: Option<std::sync::Arc<measure::snap::Geometry>>,
    /// Tab's place among the snap candidates (see [`CommentView::snap_cycle`]).
    pub snap_cycle: Option<([f64; 2], isize)>,
}

impl PageCx<'_> {
    /// A pointer position as a point in user space, moved onto the page's drawing when snapping
    /// is on and a path, endpoint, midpoint or intersection lies within reach; with what it
    /// snapped to.
    pub(crate) fn snapped(&self, p: Pos2) -> ([f64; 2], Option<measure::snap::SnapKind>) {
        let u = self.to_user(p);
        let all = self.snap_candidates(u);
        let Some(first) = all.first() else { return (u, None) };
        // Tab moves through the candidates; another set starts again from its first.
        let step = match self.snap_cycle {
            Some((f, n)) if measure::distance(f, first.point) <= 1e-6 => n,
            _ => 0,
        };
        let i = usize::try_from(step.rem_euclid(all.len().max(1) as isize)).unwrap_or(0);
        all.get(i).map_or((u, None), |hit| (hit.point, Some(hit.kind)))
    }

    /// The snap targets the user's choice allows within the reach (6 screen points at this zoom)
    /// of user point `u`, nearest first; none without snapping.
    pub(crate) fn snap_candidates(&self, u: [f64; 2]) -> Vec<measure::snap::Snap> {
        let Some(geometry) = &self.snap else { return Vec::new() };
        // The reach in user units: the screen distance of one user unit, at this zoom.
        let pixels_per_unit = self.to_screen([u[0] + 1.0, u[1]]).distance(self.to_screen(u)).max(1e-6);
        let tolerance = (SNAP_SENSITIVITY / f64::from(pixels_per_unit)).min(10_000.0);
        geometry.candidates(u, tolerance, self.prefs.snap_targets).unwrap_or_default()
    }

    /// The pointer `p` of a drag that started at `from`, moved so the first of `anchors` (screen
    /// points that travel with the drag: a comment's corners, a handle) that comes within reach
    /// of a snap target lands on it (a Fill & Sign mark `side` points across goes by its centre,
    /// as when it is placed); with that target. Unchanged without snapping or a target in reach.
    pub(crate) fn snap_drag(&self, anchors: &[Pos2], mark: Option<f64>, from: Pos2, p: Pos2) -> (Pos2, Option<SnapHit>) {
        let (p, hit) = self.snap_drag_by(anchors, mark, from, p);
        (p, hit.map(|(hit, _)| hit))
    }

    /// [`Self::snap_drag`], with which of `anchors` landed on the target.
    fn snap_drag_by(&self, anchors: &[Pos2], mark: Option<f64>, from: Pos2, p: Pos2) -> (Pos2, Option<(SnapHit, usize)>) {
        if self.snap.is_none() {
            return (p, None);
        }
        let travel = p - from;
        // A corner on a target point (an end, a centre…) beats one merely on a path, then the
        // nearer one.
        // (rank: a point target before a path, then the nearer; the pointer's shift; the hit and
        // which anchor made it)
        type Best = ((bool, f32), egui::Vec2, (SnapHit, usize));
        let mut best: Option<Best> = None;
        for (k, anchor) in anchors.iter().enumerate() {
            let moved = *anchor + travel;
            let (u, kind) = match mark {
                Some(side) => self.snapped_mark(moved, side),
                None => self.snapped(moved),
            };
            let Some(kind) = kind else { continue };
            let shift = self.to_screen(u) - moved;
            let rank = (kind == measure::snap::SnapKind::Path, shift.length());
            if best.is_none_or(|b| rank < b.0) {
                best = Some((rank, shift, ((u, kind), k)));
            }
        }
        match best {
            Some((_, shift, hit)) => (p + shift, Some(hit)),
            None => (p, None),
        }
    }

    /// Where a Fill & Sign mark `side` points across goes for the pointer at `p` when snapping is
    /// on: the centre of the box drawn around it when that is a check box or a table cell (at
    /// most ten marks long and three across, either way round: a protocol's "OK" column), else as
    /// [`Self::snapped`] puts a point.
    pub(crate) fn snapped_mark(&self, p: Pos2, side: f64) -> ([f64; 2], Option<measure::snap::SnapKind>) {
        let u = self.to_user(p);
        let (long, short) = (10.0 * side, 3.0 * side);
        if let Some(geometry) = &self.snap
            && self.prefs.snap_targets.centres
            && let Some([x0, y0, x1, y1]) = geometry.enclosing_box(u, long)
            && let (w, h) = (x1 - x0, y1 - y0)
            && ((w <= long && h <= short) || (w <= short && h <= long))
        {
            return ([(x0 + x1) / 2.0, (y0 + y1) / 2.0], Some(measure::snap::SnapKind::Centre));
        }
        self.snapped(p)
    }

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

    /// The topmost comment under `p`: the one drawn last, in `/Annots` order (inspection lists
    /// comments by position, not in paint order).
    pub(super) fn hit(&self, p: Pos2) -> Option<&Annotation> {
        self.comments().filter(|a| self.screen_rects(a).any(|r| r.expand(3.0).contains(p))).max_by_key(|a| a.index)
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
        anchor: Option<usize>,
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
            (Some(Gesture::Move { page, index, from }), Some(p)) if (*page, *index) == (self.page, a.index) => {
                r.translate(self.snap_move(a, *from, p, anchor).0 - *from)
            }
            (Some(Gesture::Resize { page, index, handle, from, aspect_ratio }), Some(p)) if (*page, *index) == (self.page, a.index) => {
                let p = self.snap_drag(&[handle_pos(r, *handle)], None, *from, p).0;
                resized(r, *handle, p - *from, corner_ratio(r, *aspect_ratio, shift))
            }
            _ => r,
        }
    }

    /// The points of comment `a` that can go onto a snap target while it is moved, in the order
    /// Tab steps through them, as displayed: its box's corners, the middles of its sides and its
    /// centre, clockwise from the bottom left with the centre last; a line's ends and middle;
    /// connected lines' and polygons' corners, then their box's centre. A Fill & Sign mark (a
    /// tick, cross, dot or line) has its centre only, which goes where a new mark would (the
    /// second value is its size).
    pub(crate) fn move_anchors(&self, a: &Annotation) -> (Vec<Anchor>, Option<f64>) {
        let r = self.screen_rect(a);
        let anchor = |at: Pos2, name: &'static str, corner: bool| Anchor { at, name, corner };
        let [x0, y0, x1, y1] = a.rect;
        if a.fill_sign && a.subtype == "Stamp" && (x1 - x0).abs() <= 48.0 && (y1 - y0).abs() <= 48.0 {
            return (vec![anchor(r.center(), "Middle", true)], Some(12.0));
        }
        if reshapable(a) {
            let points: Vec<Pos2> = a.points.iter().map(|q| self.to_screen([f64::from(q[0]), f64::from(q[1])])).collect();
            if let [from, to] = points[..] {
                return (vec![anchor(from, "Endpoint", true), anchor(from.lerp(to, 0.5), "Midpoint", false), anchor(to, "Endpoint", true)], None);
            }
            let mut anchors: Vec<Anchor> = points.into_iter().map(|p| anchor(p, "Vertex", true)).collect();
            anchors.push(anchor(r.center(), "Middle", false));
            return (anchors, None);
        }
        let anchors = vec![
            anchor(r.left_bottom(), "Bottom left", true),
            anchor(r.left_center(), "Left", false),
            anchor(r.left_top(), "Top left", true),
            anchor(r.center_top(), "Top", false),
            anchor(r.right_top(), "Top right", true),
            anchor(r.right_center(), "Right", false),
            anchor(r.right_bottom(), "Bottom right", true),
            anchor(r.center_bottom(), "Bottom", false),
            anchor(r.center(), "Middle", false),
        ];
        (anchors, None)
    }

    /// The pointer `p` of a move of comment `a` by a drag from `from`, moved so one of its
    /// [`Self::move_anchors`] lands on a snap target in reach: `choice` (Tab's) only, else
    /// whichever corner comes within reach (see [`Self::snap_drag`]). With that target, and the
    /// anchor that is put on it (`choice`, else the corner that snapped).
    pub(crate) fn snap_move(&self, a: &Annotation, from: Pos2, p: Pos2, choice: Option<usize>) -> (Pos2, Option<SnapHit>, Option<usize>) {
        let (anchors, mark) = self.move_anchors(a);
        let choice = choice.filter(|i| *i < anchors.len());
        let candidates: Vec<usize> = match choice {
            Some(i) => vec![i],
            None => (0..anchors.len()).filter(|i| anchors[*i].corner).collect(),
        };
        let at: Vec<Pos2> = candidates.iter().map(|i| anchors[*i].at).collect();
        let (p, hit) = self.snap_drag_by(&at, mark, from, p);
        (p, hit.map(|(hit, _)| hit), hit.and_then(|(_, k)| candidates.get(k).copied()).or(choice))
    }
}

/// The corner among `anchors` nearest `p` (the first anchor when there is none).
pub(super) fn nearest_corner(anchors: &[Anchor], p: Pos2) -> usize {
    let corners = anchors.iter().enumerate().filter(|(_, c)| c.corner);
    corners.min_by(|(_, a), (_, b)| a.at.distance(p).total_cmp(&b.at.distance(p))).map_or(0, |(i, _)| i)
}

/// A point of a comment that can go onto a snap target while the comment is moved (see
/// [`PageCx::move_anchors`]): where it is on screen, and its name for the marker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub at: Pos2,
    pub name: &'static str,
    /// A corner, or a line's end: a move starts with the one nearest the pointer.
    pub corner: bool,
}

/// An anchor's name as shown, in the reader's language.
fn anchor_label(name: &str) -> &'static str {
    match name {
        "Bottom left" => tl!("Bottom left"),
        "Left" => tl!("Left"),
        "Top left" => tl!("Top left"),
        "Top" => tl!("Top"),
        "Top right" => tl!("Top right"),
        "Right" => tl!("Right"),
        "Bottom right" => tl!("Bottom right"),
        "Bottom" => tl!("Bottom"),
        "Endpoint" => tl!("Endpoint"),
        "Midpoint" => tl!("Midpoint"),
        "Vertex" => tl!("Vertex"),
        _ => tl!("Middle"),
    }
}
