use craft::behavior::kurbo::Point;
use craft::behavior::{CancelReason, Cursor, CursorIcon, EditEvent, PointerButton, PressPhase};
use craft::keymap::Mods;
use craft::review::ui::annotate::{
    AnnotateTool, AnnotateWords, DrawOptions, EmptyPress, EraseMode, FarNudge, HandlesWhileDrawing, InkStrokes, MarkupPress, OpenOn, SelectOptions,
    ToggleMods,
};
use craft::review::{Annotation as Marked, AnnotationEdit, AnnotationKind, AnnotationTool, ShiftLock};
use craft::ui::canvas::{CanvasTool, ToolAccess, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::primitives::Draw;
use egui::Pos2;

use super::bridge::{self, PageMap};
use super::state::{Composer, ComposerKind, PageCx};
use super::{CommentPrefs, CommentTool, CommentView, DocInfo, Edit, MAX_LINE_WIDTH, QuickTool};
use crate::canvas::page_parts::{Owner, PagePress};
use crate::canvas::{PageGeometry, PageSession};

/// Copied comments hanging on the pointer, waiting for a click to place them.
pub struct PastePlacement {
    /// The copied comments as XFDF.
    pub xfdf: String,
    /// The bundle's box in user space.
    pub bounds: [f64; 4],
    /// How many comments the bundle holds.
    pub count: usize,
    /// Each copied comment's shape, for the outline that follows the pointer.
    pub outlines: Vec<pdfcraft_engine::CommentOutline>,
}

/// The caret's snap for the page under the pointer: the snapped tip and the caret's side, in page units.
type CaretSnap = Box<dyn Fn(Point) -> Option<(Point, f64)> + Send>;

/// The comment tools on the pages as a canvas tool: the annotation tool for the comment gestures, the paste
/// placement, and the tool beneath them that takes what no comment takes: the text selection under Select or a
/// markup tool, Fill & Sign's placing under a Fill & Sign tool (whose own marks are picked up first, as Acrobat does).
// PLAN: the sheet's constructor lists four arguments; the tool also needs the pass's tool, preferences and what the
// document allows (`settings`), where a paste click's edit and notice go (`outcome`), and `paste` as the whole slot so a
// click or Escape can drop it (pdf-canvas-w9's `view.paste.as_mut()` becomes `&mut view.paste`).
pub(crate) struct CommentsTool<'a> {
    annotate: &'a mut AnnotateTool,
    beneath: Option<Box<dyn CanvasTool + 'a>>,
    paste: &'a mut Option<PastePlacement>,
    placed: Option<&'a mut Option<Edit>>,
    notice: Option<&'a mut Option<&'static str>>,
    map: PageMap<'a>,
    done: &'a mut bool,
    pressed: &'a mut bool,
    quick: QuickTool,
    prefs: Option<&'a CommentPrefs>,
    allowed: bool,
    hidden: bool,
    caret: Option<CaretSnap>,
    image_aspect: Option<(craft::review::AnnotationId, f64)>,
}

impl<'a> CommentsTool<'a> {
    /// The tool over `annotate` (kept in `CommentView::tool`, so a gesture lasts across passes), the paste slot, the
    /// pages on screen, the flag Escape sets when it leaves the tool (`CommentView::tool_done`) and whether the
    /// annotation tool took the open press (`CommentView::tool_pressed`, kept across passes with the gesture).
    pub(crate) fn new(
        annotate: &'a mut AnnotateTool,
        paste: &'a mut Option<PastePlacement>,
        map: PageMap<'a>,
        done: &'a mut bool,
        pressed: &'a mut bool,
    ) -> Self {
        CommentsTool {
            annotate,
            beneath: None,
            paste,
            placed: None,
            notice: None,
            map,
            done,
            pressed,
            quick: QuickTool::Select,
            prefs: None,
            allowed: false,
            hidden: false,
            caret: None,
            image_aspect: None,
        }
    }

    /// The tool a press no comment takes goes on to: the text selection, or Fill & Sign's placing.
    pub(crate) fn over(mut self, beneath: impl CanvasTool + 'a) -> Self {
        self.beneath = Some(Box::new(beneath));
        self
    }

    /// The tool in use, the comment preferences, whether the document allows commenting and whether comments are
    /// hidden.
    pub(crate) fn settings(mut self, quick: QuickTool, prefs: &'a CommentPrefs, allowed: bool, hidden: bool) -> Self {
        (self.quick, self.prefs, self.allowed, self.hidden) = (quick, Some(prefs), allowed, hidden);
        self
    }

    /// The embedded image's proportions, in the displayed view's axes.
    pub(crate) fn image_aspect(mut self, aspect: Option<(usize, usize, f32)>) -> Self {
        self.image_aspect = aspect.map(|(page, index, ratio)| {
            let ratio = if self.map.rotation.is_multiple_of(180) { ratio } else { ratio.recip() };
            (bridge::id_of(page, index), f64::from(ratio))
        });
        self
    }

    fn annotations(&self, page: usize) -> Vec<Marked> {
        bridge::annotations(self.map.info, &self.map, self.quick, self.allowed)
            .into_iter()
            .filter(|a| a.page == page)
            .map(|mut a| {
                if let Some((id, ratio)) = self.image_aspect
                    && a.id == id
                {
                    a.aspect = Some(ratio);
                }
                a
            })
            .collect()
    }

    /// Where a paste click's edit and notice go.
    pub(crate) fn outcome(mut self, placed: &'a mut Option<Edit>, notice: &'a mut Option<&'static str>) -> Self {
        self.placed = Some(placed);
        self.notice = Some(notice);
        self
    }

    /// The caret tool's snap to the text under the pointer.
    // PLAN: assumed pdf-comments-w7 supplies the snap over `geometry::snapped_caret` and the page's text.
    pub(crate) fn caret_snap(mut self, snap: impl Fn(Point) -> Option<(Point, f64)> + Send + 'static) -> Self {
        self.caret = Some(Box::new(snap));
        self
    }

    fn page_under(&self, at: Pos2) -> Option<usize> {
        self.map.page_at(Point::new(f64::from(at.x), f64::from(at.y)))
    }

    /// Whether the annotation tool holds the gesture, as it took the press or is clicking out a shape: the drag,
    /// release and cancel are its, else the tool's beneath.
    fn annotating(&self) -> bool {
        *self.pressed || !self.annotate.drawing().is_empty()
    }

    fn place(&mut self, page: usize, at: Pos2) {
        let Some(paste) = self.paste.take() else { return };
        let Some(at) = self.map.to_page(page, Point::new(f64::from(at.x), f64::from(at.y))) else { return };
        let [x, y] = self.map.to_user(page, at);
        let offset = [x - (paste.bounds[0] + paste.bounds[2]) / 2.0, y - (paste.bounds[1] + paste.bounds[3]) / 2.0];
        // Larger than the page it lands on (an A0 bundle on an A4 page): placed as it is, and said so.
        if let Some(info) = self.map.info.pages.get(page) {
            let a = info.user_to_view((paste.bounds[0] + offset[0]) as f32, (paste.bounds[1] + offset[1]) as f32);
            let b = info.user_to_view((paste.bounds[2] + offset[0]) as f32, (paste.bounds[3] + offset[1]) as f32);
            let (w, h) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
            if (w > info.width + 1.0 || h > info.height + 1.0)
                && let Some(notice) = self.notice.as_deref_mut()
            {
                *notice = Some("The copied comments are larger than this page; they were placed as they are");
            }
        }
        if let Some(placed) = self.placed.as_deref_mut() {
            *placed = Some(Edit::PasteComments { xfdf: paste.xfdf, page, offset: Some(offset) });
        }
    }
}

/// The framework's tool for `quick`: the comment tool's own, or Select for every other tool.
fn annotation_tool(quick: QuickTool) -> AnnotationTool {
    match quick {
        QuickTool::Comment(tool) => tool.tool(),
        _ => AnnotationTool::Select,
    }
}

fn set_page(annotate: &mut AnnotateTool, page: usize, map: &PageMap<'_>, quick: QuickTool, allowed: bool, hidden: bool) {
    annotate.page = page;
    annotate.annotations =
        if hidden { Vec::new() } else { bridge::annotations(map.info, map, quick, allowed).into_iter().filter(|a| a.page == page).collect() };
}

/// The eraser's radius, in screen points, from its thickness setting (#3747): three points per point of
/// thickness, so the default 2 pt keeps the 12-point eraser. A setting that is not a number is the default.
fn eraser_radius(width: f64) -> f32 {
    (3.0 * if width.is_finite() { width.clamp(0.5, MAX_LINE_WIDTH) } else { 2.0 }) as f32
}

/// A preference colour as straight RGB bytes.
fn rgb8(color: pdfcraft_engine::Rgb) -> [u8; 3] {
    let c = super::color32(color);
    [c.r(), c.g(), c.b()]
}

impl CanvasTool for CommentsTool<'_> {
    fn each_pass(&mut self, cx: &mut ToolContext<'_>) {
        // The page the tool works on follows the pointer between gestures.
        if cx.gesture_id().is_none()
            && self.annotate.drawing().is_empty()
            && let Some(page) = cx.pointer.and_then(|p| self.page_under(p.pos))
        {
            self.annotate.page = page;
        }
        let page = self.annotate.page;
        self.annotate.tool = annotation_tool(self.quick);
        self.annotate.annotations = if self.hidden { Vec::new() } else { self.annotations(page) };
        let mut eraser_width = 2.0;
        if let (Some(prefs), QuickTool::Comment(comment)) = (self.prefs, self.quick) {
            let style = prefs.style(comment);
            self.annotate.color = rgb8(style.color);
            self.annotate.highlight_color = rgb8(prefs.style(CommentTool::Highlight).color);
            self.annotate.width = style.width;
            if comment == CommentTool::Eraser {
                eraser_width = style.width;
            }
        }
        // Both option types are `#[non_exhaustive]`: the app sets the fields it names over the framework's defaults.
        let mut select = SelectOptions::default();
        select.empty_press = EmptyPress::PassThrough;
        select.toggle = ToggleMods::ShiftOrCommand;
        select.open = OpenOn::Every;
        select.shift_lock = ShiftLock::LevelOrPlumb;
        select.far_nudge = FarNudge::ShiftOrCommand;
        select.handles_while_drawing = HandlesWhileDrawing::Edit;
        select.markup_press = MarkupPress::ClickOnly;
        let mut draw = DrawOptions::default();
        draw.ink = InkStrokes::PerStroke;
        draw.erase = EraseMode::Path;
        draw.eraser_radius = eraser_radius(eraser_width);
        // The options are builders over the owned tool: rebuilt from the tool taken out, its gesture kept.
        let mut built = std::mem::take(self.annotate).select_options(select).draw_options(draw);
        // The snap reads the page's displayed units; the tool asks in document space.
        if self.quick == QuickTool::Comment(CommentTool::Caret)
            && let Some(snap) = self.caret.take()
            && let Some(frame) = self.map.frame(page)
        {
            built = built.caret_snap(move |at| snap(frame.shown(at)).map(|(tip, side)| (frame.doc(tip), frame.doc_len(side))));
        }
        *self.annotate = built;
        self.annotate.each_pass(cx);
        if let Some(beneath) = self.beneath.as_mut() {
            beneath.each_pass(cx);
        }
    }

    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        *self.pressed = false;
        if self.paste.is_some() {
            // A click places the bundle, centred where it is clicked, on the page under it; a right-click lets it
            // go. Nothing else on the page takes the press.
            if pointer.button == PointerButton::Secondary {
                *self.paste = None;
            } else if self.allowed
                && let Some(page) = self.page_under(pointer.pos)
            {
                self.place(page, pointer.pos);
            }
            return ToolAction::Done;
        }
        // A markup tool or Replace Text marks the text its drag selects (`page_after_text`): the text takes the press
        // first. Under Select the comments come first, and a press no comment takes goes on to the text.
        if matches!(self.quick, QuickTool::Comment(_))
            && let Some(text) = self.beneath.as_mut()
        {
            let action = text.press(pointer, cx);
            if action != ToolAction::Ignored {
                return action;
            }
        }
        if self.annotate.drawing().is_empty()
            && let Some(page) = self.page_under(pointer.pos)
            && page != self.annotate.page
        {
            self.annotate.page = page;
            self.annotate.annotations = if self.hidden { Vec::new() } else { self.annotations(page) };
        }
        let action = self.annotate.press(pointer, cx);
        *self.pressed = action != ToolAction::Ignored;
        // A press on no comment passes through to the tool beneath (`EmptyPress::PassThrough`): it leaves the comments
        // selected only while Shift or Cmd adds to the selection. A press that began off the pages is no page's and
        // leaves them selected.
        if action == ToolAction::Ignored
            && self.annotate.tool == AnnotationTool::Select
            && pointer.button == PointerButton::Primary
            && !(pointer.mods.shift || pointer.mods.cmd)
            && self.page_under(pointer.pos).is_some()
        {
            self.annotate.selected.clear();
        }
        match action {
            ToolAction::Ignored => match self.beneath.as_mut() {
                Some(beneath) => beneath.press(pointer, cx),
                None => ToolAction::Ignored,
            },
            action => action,
        }
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        if self.annotating() {
            self.annotate.drag(pointer, cx);
        } else if let Some(beneath) = self.beneath.as_mut() {
            beneath.drag(pointer, cx);
        }
    }

    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        if self.annotating() {
            self.annotate.release(pointer, cx);
        } else if let Some(beneath) = self.beneath.as_mut() {
            beneath.release(pointer, cx);
        }
        *self.pressed = false;
    }

    fn hover(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.annotate.hover(pointer, cx);
        if let Some(beneath) = self.beneath.as_mut() {
            beneath.hover(pointer, cx);
        }
    }

    fn cancel(&mut self, reason: CancelReason, cx: &mut ToolContext<'_>) {
        if self.annotating() {
            self.annotate.cancel(reason, cx);
        } else if let Some(beneath) = self.beneath.as_mut() {
            beneath.cancel(reason, cx);
        }
        *self.pressed = false;
    }

    fn claims(&self) -> ToolClaims {
        self.annotate.claims()
    }

    fn key(&mut self, key: &ToolKey, cx: &mut ToolContext<'_>) -> bool {
        if matches!(key, ToolKey::Cancel) {
            // One step back per press: drop a paste, cancel the drawing, deselect, then Fill & Sign's own steps (its
            // placing closes the type box, then leaves the tool), or drop a comment tool. The text selection beneath
            // Select or a markup tool takes no part in them.
            if self.paste.take().is_some() {
                return true;
            }
            if !self.annotate.drawing().is_empty() {
                self.annotate.cancel_drawing();
                return true;
            }
            if !self.annotate.selected.is_empty() {
                self.annotate.selected.clear();
                return true;
            }
            if matches!(self.quick, QuickTool::Fill(_) | QuickTool::Select) {
                return self.beneath.as_mut().is_some_and(|beneath| beneath.key(key, cx));
            }
            if matches!(self.quick, QuickTool::Select) {
                return self.beneath.as_mut().is_some_and(|beneath| beneath.key(key, cx));
            }
            if matches!(self.quick, QuickTool::Comment(_)) {
                *self.done = true;
                return true;
            }
            return false;
        }
        self.annotate.key(key, cx)
    }

    fn takes(&self, button: PointerButton, mods: Mods) -> bool {
        button == PointerButton::Primary || self.paste.is_some() && button == PointerButton::Secondary || self.annotate.takes(button, mods)
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        if self.paste.is_some() && cx.pointer.is_some_and(|p| self.page_under(p.pos).is_some()) {
            return Cursor::System(CursorIcon::Grabbing);
        }
        let own = self.annotate.cursor(cx);
        if self.annotating() {
            return own;
        }
        // Where no comment takes the press, it goes on to the tool beneath (`press`): its cursor shows there. A markup
        // tool's crosshair is the annotation tool's idle face; the drag selects text, so the text's cursor shows over it.
        let idle = match own {
            Cursor::System(CursorIcon::Default) => true,
            Cursor::System(CursorIcon::Crosshair) => matches!(self.quick, QuickTool::Comment(_)),
            _ => false,
        };
        match self.beneath.as_ref().map(|beneath| beneath.cursor(cx)) {
            Some(beneath) if idle && !matches!(beneath, Cursor::System(CursorIcon::Default)) => beneath,
            _ => own,
        }
    }

    fn access(&self, cx: &ToolContext<'_>) -> Option<ToolAccess> {
        self.annotate.access(cx)
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.annotate.paint(draw, cx);
        if let (Some(paste), Some(pointer)) = (self.paste.as_ref(), cx.pointer)
            && let Some(page) = self.page_under(pointer.pos)
        {
            super::overlay::paste_outline(draw, cx, paste, &self.map, page, pointer.pos);
        }
        if let Some(beneath) = self.beneath.as_ref() {
            beneath.paint(draw, cx);
        }
    }

    /// A Fill & Sign tool is reached through its own commands; the comment tools through the annotation tool's.
    fn command_form(&self) -> ToolPath {
        match self.beneath.as_ref() {
            Some(beneath) if matches!(self.quick, QuickTool::Fill(_)) => beneath.command_form(),
            _ => self.annotate.command_form(),
        }
    }
}

/// The comment a right-click at `at` is on: the topmost, with every tool, as a comment's menu shows whatever tool
/// is active; on a group subordinate (`/RT /Group`), its group's primary, so the menu offers the comment's actions
/// (ISO 32000-2 §12.5.6.2). A highlight over a link is the highlight's here (a left click on it follows the link).
/// `None` off the page or with comments hidden. The page's menu is anchored at `at` (Delete page, Paste, Copy
/// Image, a note added here), and the comment is selected, or the selection cleared.
pub(crate) fn context_target(cx: &PageCx<'_>, view: &mut CommentView, at: Pos2) -> Option<usize> {
    if !context_anchor(cx, view, at) {
        return None;
    }
    let target = if cx.hidden { None } else { cx.click_target(at) };
    match target {
        Some(a) => {
            view.pick(cx, at, a);
            Some(a.index)
        }
        None => {
            view.select(None);
            None
        }
    }
}

/// Anchors the page's menu at `at` on the page of `cx`, whoever takes the right-click: Delete page, Paste, Copy Image
/// and a note added from the menu act there. `false` off the page.
pub(crate) fn context_anchor(cx: &PageCx<'_>, view: &mut CommentView, at: Pos2) -> bool {
    if !cx.xf.rect.contains(at) {
        return false;
    }
    view.context_at = Some((cx.page, cx.to_user(at)));
    true
}

/// The comment a right-click at `at` is on, as [`context_target`] finds it, without changing the selection.
pub(crate) fn context_comment(cx: &PageCx<'_>, at: Pos2) -> Option<usize> {
    if cx.hidden || !cx.xf.rect.contains(at) {
        return None;
    }
    cx.click_target(at).map(|a| a.index)
}

/// A click on a comment's body, from the page part that owns it (M-1): the annotation tool selects it with the
/// modifiers and click count of the press, a double click opening it. A click on a group subordinate selects its
/// group's primary and remembers the member under the pointer (`cx` is the comment's page).
pub(crate) fn part_pressed(view: &mut CommentView, press: &PagePress, cx: &PageCx<'_>, map: &PageMap<'_>, quick: QuickTool, words: &AnnotateWords) {
    let Owner::Comment { index } = press.owner else { return };
    let PressPhase::Release { start, click: true, .. } = press.phase else { return };
    if start.button != PointerButton::Primary {
        return;
    }
    set_page(&mut view.tool, press.page, map, quick, cx.allowed, cx.hidden);
    view.tool.click(bridge::id_of(press.page, index), press.mods, start.clicks, words);
    view.member = cx.picked(start.origin, view.primary());
}

/// The engine edit the gestures of this pass made, taken from the annotation tool after the tool host ran: each
/// committed `EditEvent` is one engine edit, so a drag is one undo step. A drawn shape that is kept on screen until
/// its page is drawn again is remembered (`CommentView::settle`). The new comment is selected by the app once the
/// edit is applied (a pen stroke stays unselected, `editing.rs`).
pub(crate) fn take_edits(view: &mut CommentView, map: &PageMap<'_>, info: &DocInfo, prefs: &CommentPrefs, now: f64) -> Option<Edit> {
    let mut edits = Vec::new();
    for event in view.tool.take_edits() {
        let (EditEvent::Commit { after, .. } | EditEvent::Set { after, .. }) = event else { continue };
        if let AnnotationEdit::Create(a) = &after
            && let Some(tool) = CommentTool::from_kind(a.kind)
        {
            view.settle(now, a.page, tool, &settled_points(a, map));
        }
        edits.extend(bridge::edit(after, map, view, info, prefs));
    }
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Edit::Batch { label: tl!("Comments").to_owned(), edits }),
    }
}

/// The user-space points a released drawing is painted from while it settles: a stroke's points, a box's corners, a
/// line's ends.
fn settled_points(a: &Marked, map: &PageMap<'_>) -> Vec<[f64; 2]> {
    let user = |p: &Point| map.to_user(a.page, *p);
    match a.kind {
        AnnotationKind::Ink | AnnotationKind::FreehandHighlight => a.strokes.last().map(|s| s.iter().map(user).collect()).unwrap_or_default(),
        AnnotationKind::Rectangle | AnnotationKind::Oval => vec![user(&Point::new(a.rect.x0, a.rect.y0)), user(&Point::new(a.rect.x1, a.rect.y1))],
        _ => a.points.iter().map(user).collect(),
    }
}

/// After text selection ran: a markup tool turns a finished selection into a comment; Replace Text asks for the
/// replacement. `finished` is true in the pass the selection's release came (a drag release or a double click); the
/// call runs on every pass a markup tool is active, since a selection on pages still being read is marked later.
/// `geo` places the pages of the pass.
pub(crate) fn page_after_text(cx: &PageCx<'_>, tool: QuickTool, finished: bool, session: &mut PageSession, geo: PageGeometry<'_>) {
    let QuickTool::Comment(tool) = tool else { return };
    if tool == CommentTool::ReplaceText && cx.allowed && finished {
        // A replacement sits on one page: a selection across pages stays selected for the bar's
        // markup or Copy.
        if let Ok([(page, quads)]) = <[_; 1]>::try_from(session.selection_quads(cx.info, geo.rotation))
            && page == cx.page
        {
            session.clear_selection();
            let last = quads.last().copied().unwrap_or_default();
            let at = [last[2].max(last[6]), last[1].max(last[3])];
            session.comments.replace = Some(quads);
            session.comments.composer = Some(Composer { page, at, kind: ComposerKind::Replace, text: String::new(), focus: true });
        }
        return;
    }
    let Some(kind) = tool.markup().filter(|_| cx.allowed) else { return };
    // Once, from whichever of the selection's pages is drawn: after an edge scroll its first page
    // may be out of view. Every page it covers gets its own markup, in one undo step, once all
    // of them are read.
    if let crate::canvas::ToolMarks::Marks(marks, head) = session.tool_marks(QuickTool::Comment(tool), finished, Some(cx.page), false, cx.info, geo) {
        session.clear_selection();
        let edits = marks.into_iter().map(|(page, quads)| {
            Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page,
                shape: super::Shape::TextMarkup { kind, quads },
                style: cx.prefs.style(tool),
                contents: String::new(),
                author: cx.prefs.author.clone(),
            })
        });
        session.pending_edit = session.selection_marks_edit(edits.collect(), head);
    }
}
