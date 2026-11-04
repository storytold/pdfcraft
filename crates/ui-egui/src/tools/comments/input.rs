use super::geometry::{
    CARET_SIZE, HANDLES, clamp_to, corner_ratio, drawn_box, handle_pos, is_markup, point_handle, reaches_points, reshapable, reshaped, resizable,
    resized, snapped_caret,
};
use super::paint::{drawn_shape, edit_anchor, erase, new_comment};
use super::state::{Composer, ComposerKind, Gesture, PageCx};
use super::{Annotation, CommentPrefs, CommentTool, DocInfo, DocView, Edit, Pos2, QuickTool, Rect, Shape, vec2};
use crate::page_cursor::{Layer, Subject};
use crate::page_router::{CommentPart, Hit, Owner, PagePress};

/// Comment input on one page, before text selection runs. `press` is what this page has to do with
/// the press the pages hold: a press on a comment where the tool reaches it ([`hit`]: the Select
/// tool, Fill & Sign over its marks, a shape tool over the selected shape's handles) is that
/// comment's, and one nobody holds is a commenting tool's. When another tool took this frame
/// (`taken`: Prepare, added content, which still decide for themselves), the commenting tools leave
/// it to that tool, as before; the Select tool's click there still unselects. Returns `true` when
/// the pointer gesture belongs to commenting (text selection must ignore it).
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, cx: &PageCx<'_>, press: PagePress, taken: bool, view: &mut DocView) -> bool {
    let (pointer, origin) = (cx.ptr.pos, cx.ptr.origin);
    let page_rect = cx.xf.rect;
    let free = press == PagePress::Free;
    let pressed_here = free && origin.is_some_and(|o| page_rect.contains(o));
    let over_page = pointer.is_some_and(|p| page_rect.contains(p));
    let owned = match press {
        PagePress::Mine(Owner::Comment { index, part }) => Some((index, part)),
        _ => None,
    };
    let caret_text = if cx.tool == QuickTool::Comment(CommentTool::Caret) { view.page_text(cx.page) } else { None };
    let cv = &mut view.comments;
    if cv.gesture.is_some() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        cv.gesture = None;
        return true;
    }
    let moving = moving_on(view, cx.page);
    let gestures = Gestures { ui, resp, cx, pointer, origin, page_rect, pressed_here, over_page, free };
    match cx.tool {
        // The selected shape's handles, and Fill & Sign's marks, take their own presses.
        QuickTool::Comment(_) | QuickTool::Fill(_) if owned.is_some() || moving => select_input(ui, resp, cx, view, press, pointer, origin),
        // Neither a press nor a double-click or a key another tool took this frame.
        QuickTool::Comment(_) if taken => false,
        QuickTool::Comment(tool) if tool.draws() => gestures.draw_shape(view, tool),
        QuickTool::Comment(tool) if tool.clicks_points() => gestures.click_points(view, tool),
        QuickTool::Comment(CommentTool::Callout) => gestures.callout(view),
        QuickTool::Comment(CommentTool::Attach) => gestures.attach(view),
        QuickTool::Comment(CommentTool::Caret) => gestures.caret(view, caret_text.as_deref()),
        QuickTool::Comment(tool @ (CommentTool::Note | CommentTool::TextBox)) => gestures.note_or_box(view, tool),
        QuickTool::Select => select_input(ui, resp, cx, view, press, pointer, origin),
        _ => false,
    }
}

/// A comment (a Fill & Sign mark included) is being moved, resized or reshaped on `page`: it keeps
/// the page's frames until its gesture ends.
pub(crate) fn moving_on(view: &DocView, page: usize) -> bool {
    matches!(view.comments.gesture, Some(Gesture::Move { page: p, .. } | Gesture::Resize { page: p, .. } | Gesture::Reshape { page: p, .. }) if p == page)
}

/// What of the comments lies under `p`, for the cursor, the tip and the press: the one hit test
/// of this module. With the Select tool, the selected comment's point handles, then its resize
/// handles, then the topmost comment. With a Fill & Sign tool, the same over Fill & Sign's marks
/// only, as Acrobat picks them up: a press anywhere else places the tool's mark. With a shape
/// tool (rectangle, oval, line, polygon…), only the selected comment's handles, so the shape just
/// drawn can be resized straight away (#2338); a press anywhere else, the shape itself included,
/// draws. `None` with other tools, off the page, or with comments hidden. `on_link`: a link lies
/// under `p` ([`crate::link_ui::hit`]).
pub(crate) fn hit(cx: &PageCx<'_>, view: &DocView, p: Pos2, on_link: bool) -> Option<Hit> {
    if cx.hidden || !cx.xf.rect.contains(p) {
        return None;
    }
    let pick: fn(&Annotation) -> bool = match cx.tool {
        QuickTool::Select => |_| true,
        QuickTool::Fill(_) => |a| a.fill_sign,
        QuickTool::Comment(tool) if (tool.draws() || tool.clicks_points()) && tool != CommentTool::Eraser => {
            // A polygon being clicked out keeps its clicks, even near an old shape's handle.
            if view.comments.gesture.is_some() {
                return None;
            }
            |_| true
        }
        _ => return None,
    };
    let bodies = matches!(cx.tool, QuickTool::Select | QuickTool::Fill(_));
    // A selected group shows every member's handles on the page, and each changes only its own
    // member, as Acrobat's do; the topmost member's come first where they overlap.
    let mut selected = cx.selected_here(view.comments.selected);
    selected.sort_by_key(|a| std::cmp::Reverse(a.index));
    for a in selected.into_iter().filter(|a| pick(a) && cx.allowed) {
        if reaches_points(cx.tool)
            && reshapable(a)
            && let Some(point) = point_handle(cx, a, p)
        {
            return Some(Hit::new(Owner::Comment { index: a.index, part: CommentPart::Point(point) }, Layer::Handle, egui::CursorIcon::Crosshair));
        }
        if resizable(a)
            && let Some((x, y)) = HANDLES.into_iter().find(|h| handle_pos(cx.screen_rect(a), *h).distance(p) <= 7.0)
        {
            let icon = match (x, y) {
                (0, _) => egui::CursorIcon::ResizeVertical,
                (_, 0) => egui::CursorIcon::ResizeHorizontal,
                (-1, -1) | (1, 1) => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::ResizeNeSw,
            };
            return Some(Hit::new(Owner::Comment { index: a.index, part: CommentPart::Handle(x, y) }, Layer::Handle, icon));
        }
    }
    // A click on a group subordinate (`/RT /Group`) selects its group (ISO 32000-2 §12.5.6.2); a
    // drag moves just it, as Acrobat's does. The tool's own filter picks the candidates, so a
    // Fill & Sign tool reaches its mark under a comment or group member it ignores.
    let a = cx.click_target_where(p, |a| bodies && pick(a))?;
    // A click on a highlight over a link follows the link, so the link's hand and tip show there.
    if cx.tool == QuickTool::Select && a.subtype == "Highlight" && on_link {
        return None;
    }
    let markup = is_markup(&a.subtype);
    let icon = if cx.allowed && !markup { egui::CursorIcon::Move } else { egui::CursorIcon::PointingHand };
    // Text markup does not move: a click selects it, a drag across it selects the text.
    Some(Hit { declines_drag: markup, ..Hit::new(Owner::Comment { index: a.index, part: CommentPart::Body }, layer(a), icon) })
}

/// The layer a comment's body lies on: what a drag moves whole (a note, a shape, a signature, a
/// Fill & Sign mark) is placed on the page, over a form field; text markup lies on the text, under
/// one.
fn layer(a: &Annotation) -> Layer {
    if a.fill_sign || !is_markup(&a.subtype) { Layer::Placed } else { Layer::Comment }
}

/// The comment a right-click at `p` is on: the topmost, with every tool, as a comment's menu shows
/// whatever tool is active; a group subordinate (`/RT /Group`) counts, and [`context`] selects its
/// group (ISO 32000-2 §12.5.6.2). A highlight over a link is the highlight's here (a left click on
/// it follows the link, [`hit`]). `None` off the page or with comments hidden.
pub(crate) fn context_hit(cx: &PageCx<'_>, p: Pos2) -> Option<Hit> {
    if cx.hidden || !cx.xf.rect.contains(p) {
        return None;
    }
    let a = cx.click_target(p)?;
    Some(Hit::new(Owner::Comment { index: a.index, part: CommentPart::Body }, layer(a), egui::CursorIcon::Default))
}

/// A right-click at `p` on this page: the page's menu is anchored there (Delete page, Paste, Copy
/// Image, a note added here).
pub(crate) fn anchor(cx: &PageCx<'_>, p: Pos2, view: &mut DocView) {
    view.comments.context_at = Some((cx.page, cx.to_user(p)));
}

/// A right-click on comment `index` of this page ([`context_hit`], when it wins there)
/// selects what a click there selects: the comment, or the group a subordinate belongs to, so the
/// menu offers the comment's actions. One on anything else selects none.
pub(crate) fn context(cx: &PageCx<'_>, index: Option<usize>, view: &mut DocView) {
    match index {
        // `index` is what the right-click is on ([`context_hit`]): no second look at the point.
        Some(i) => match cx.annotation(i) {
            Some(a) => view.comments.pick(cx, a),
            None => view.comments.select(Some((cx.page, i))),
        },
        None => view.comments.select(None),
    }
}

/// The tip for the comment under the pointer, with any tool, beside that tool's own cursor: Acrobat
/// shows a comment's tooltip whatever tool is active. Not with Fill & Sign, whose placement stays
/// clear; not while drawing freehand, where it would cover the next stroke (#429); and not while a
/// comment is moved, resized or drawn.
pub(crate) fn describe(cx: &PageCx<'_>, view: &mut DocView) {
    let freehand = matches!(cx.tool, QuickTool::Comment(CommentTool::Ink | CommentTool::FreehandHighlight | CommentTool::Eraser));
    if matches!(cx.tool, QuickTool::Fill(_)) || freehand || view.comments.gesture.is_some() {
        return;
    }
    let Some(p) = cx.ptr.pos else { return };
    // Inspection sorts comments by position, but the renderer paints them in `/Annots` order: the
    // tip is that of the one drawn on top, among those with something to say (#3620).
    // A group subordinate says what its group does: its primary's author and text (ISO 32000-2
    // §12.5.6.2), wherever the primary is.
    let members = cx.group_members();
    let top = cx
        .comments()
        .map(|a| (a, a))
        .chain(members.iter().copied())
        .filter(|(a, _)| cx.screen_rects(a).any(|r| r.contains(p)))
        .filter_map(|(a, says)| hover_tip(says, cx.prefs).map(|text| (a.index, text)))
        .max_by_key(|(index, _)| *index);
    if let Some((_, text)) = top {
        view.cursor.describe(Subject::Comment, text);
    }
}

/// What hovering `a` says, as Acrobat's popups do. Replies and Fill & Sign's marks (typed text,
/// checks, signatures) say nothing: they are page content being edited, not comments. Neither
/// does a highlight with no text of its own, when the preferences say so.
pub(super) fn hover_tip(a: &Annotation, prefs: &CommentPrefs) -> Option<String> {
    let empty_highlight = a.subtype == "Highlight" && a.contents.as_deref().is_none_or(str::is_empty);
    if a.is_reply() || a.fill_sign || prefs.hide_empty_highlight_hover && empty_highlight {
        return None;
    }
    let who = a.author.clone().unwrap_or_else(|| a.subtype.clone());
    let body = a.contents.clone().unwrap_or_default();
    Some(if body.is_empty() { who } else { format!("{who}\n{body}") })
}

/// The Select tool: select, move, resize, edit, delete, on the comment the press is on and the
/// part of it (`press`, from [`hit`]). The cursor comes from [`hit`] too. A click on this page on
/// anything else (a link, a field, added text, the page) leaves comments unselected; one that
/// began off the pages is no page's.
fn select_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    cx: &PageCx<'_>,
    view: &mut DocView,
    press: PagePress,
    pointer: Option<Pos2>,
    origin: Option<Pos2>,
) -> bool {
    let owner = match press {
        PagePress::Mine(Owner::Comment { index, part }) => cx.get(index).map(|a| (a, part)),
        _ => None,
    };
    let cv = &mut view.comments;
    let mut consumed = false;
    if resp.drag_started()
        && let (Some((a, part)), Some(o)) = (owner, origin)
    {
        match part {
            CommentPart::Point(point) => {
                let points = a.points.iter().map(|q| [f64::from(q[0]), f64::from(q[1])]).collect();
                cv.gesture = Some(Gesture::Reshape { page: cx.page, index: a.index, point, points });
                consumed = true;
            }
            CommentPart::Handle(x, y) => {
                // On screen, the image is also turned by the view's rotation.
                let turned = !cx.xf.rot.is_multiple_of(180);
                let aspect_ratio = view.signature_drag.aspect_ratio(cx.page, a.index).map(|ratio| if turned { ratio.recip() } else { ratio });
                // A drawn signature keeps its shape from the corners, as image signatures do.
                let aspect_ratio = aspect_ratio.or_else(|| {
                    let r = cx.screen_rect(a);
                    (a.fill_sign && a.subtype == "Ink" && r.height() > 0.0).then(|| r.width() / r.height())
                });
                cv.gesture = Some(Gesture::Resize { page: cx.page, index: a.index, handle: (x, y), from: o, aspect_ratio });
                consumed = true;
            }
            CommentPart::Body if !is_markup(&a.subtype) => {
                cv.pick(cx, a);
                cv.reveal = true;
                if cx.allowed {
                    cv.gesture = Some(Gesture::Move { page: cx.page, index: a.index, from: o });
                }
                consumed = true;
            }
            CommentPart::Body => {}
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
        // A click elsewhere (a link beneath text highlighting included: it follows the link; a
        // form field; added text) leaves comments unselected.
        match owner {
            Some((a, _)) => {
                cv.pick(cx, a);
                cv.reveal = true;
                consumed = true;
            }
            None if press != PagePress::Elsewhere => cv.select(None),
            None => {}
        }
    }
    if resp.double_clicked()
        && on_page
        && cx.allowed
        && let Some((a, _)) = owner
    {
        let (page, index) = cx.selects(a);
        cv.pick(cx, a);
        // A group's text is its primary's, wherever in the group the double-click was.
        let a = cx.get(index).filter(|_| page == cx.page).unwrap_or(a);
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

/// What the pointer is doing on one page, for the comment tools that draw or place things.
#[derive(Clone, Copy)]
struct Gestures<'a> {
    ui: &'a egui::Ui,
    resp: &'a egui::Response,
    cx: &'a PageCx<'a>,
    pointer: Option<Pos2>,
    origin: Option<Pos2>,
    page_rect: Rect,
    /// The press began on this page, and nobody holds it: the tool's.
    pressed_here: bool,
    over_page: bool,
    /// Nobody holds the press, and it began on this page, or no button is down.
    free: bool,
}

impl Gestures<'_> {
    /// The tool's own click: one nobody holds.
    fn clicked(&self) -> bool {
        self.free && self.resp.clicked()
    }

    fn draw_shape(&self, view: &mut DocView, tool: CommentTool) -> bool {
        let Self { ui, resp, cx, pointer, origin, page_rect, pressed_here, over_page, .. } = *self;
        let cv = &mut view.comments;
        if !cx.allowed {
            return false;
        }
        if over_page {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
        }
        if resp.drag_started()
            && pressed_here
            && let Some(o) = origin
        {
            cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![cx.to_user(o)] });
            cv.select(None);
        }
        if let Some(Gesture::Draw { page, tool, points }) = cv.gesture.as_mut()
            && *page == cx.page
            && let Some(p) = pointer
        {
            let p = clamp_to(page_rect, p);
            let u = cx.to_user(p);
            if matches!(*tool, CommentTool::Ink | CommentTool::FreehandHighlight | CommentTool::Eraser) {
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
        if matches!(tool, CommentTool::Ink | CommentTool::FreehandHighlight)
            && self.clicked()
            && over_page
            && let Some(p) = pointer
            && let Some(shape) = drawn_shape(tool, &[cx.to_user(p)])
        {
            cv.select(None);
            let now = ui.input(|i| i.time);
            cv.settle(now, cx.page, tool, &[cx.to_user(p)]);
            view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
        }
        true
    }

    fn click_points(&self, view: &mut DocView, tool: CommentTool) -> bool {
        let Self { ui, resp, cx, pointer, over_page, .. } = *self;
        let cv = &mut view.comments;
        if !cx.allowed {
            return false;
        }
        if over_page {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
        }
        let finish = resp.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
        // Backspace (or Delete) takes back the last point, e.g. after a wrong click; Undo does too.
        // Only while a drawing is in progress: otherwise Delete belongs to the selected comment.
        if matches!(cv.gesture, Some(Gesture::Draw { page, .. }) if page == cx.page)
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace) || i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
        {
            cv.undo_point();
            return true;
        }
        if self.clicked()
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
                    cv.select(None);
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

    fn callout(&self, view: &mut DocView) -> bool {
        let Self { resp, cx, pointer, origin, page_rect, pressed_here, over_page, .. } = *self;
        let cv = &mut view.comments;
        // Drag from what the callout points at to where its text box goes.
        if !cx.allowed {
            return false;
        }
        if over_page {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
        }
        if resp.drag_started()
            && pressed_here
            && let Some(o) = origin
        {
            cv.gesture = Some(Gesture::Draw { page: cx.page, tool: CommentTool::Callout, points: vec![cx.to_user(o)] });
            cv.select(None);
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

    fn attach(&self, view: &mut DocView) -> bool {
        let Self { cx, pointer, over_page, .. } = *self;
        let cv = &mut view.comments;
        if !cx.allowed {
            return false;
        }
        if over_page {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
        }
        if self.clicked()
            && over_page
            && let Some(p) = pointer
        {
            cv.attach_at = Some((cx.page, cx.to_user(p)));
            cv.select(None);
        }
        true
    }

    fn caret(&self, view: &mut DocView, caret_text: Option<&pdfcraft_render::PageText>) -> bool {
        let Self { cx, pointer, over_page, .. } = *self;
        let cv = &mut view.comments;
        if !cx.allowed {
            return false;
        }
        if over_page {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Text);
        }
        if self.clicked()
            && over_page
            && let Some(p) = pointer
        {
            let (vx, vy) = cx.xf.screen_to_view(p);
            let snapped = cx.info.pages.get(cx.page).zip(caret_text).and_then(|(page, t)| snapped_caret(page, t, vx, vy));
            let (at, size) = snapped.unwrap_or_else(|| (cx.to_user(p), CARET_SIZE));
            cv.composer = Some(Composer { page: cx.page, at, kind: ComposerKind::Caret { size }, text: String::new(), focus: true });
            cv.select(None);
        }
        true
    }

    fn note_or_box(&self, view: &mut DocView, tool: CommentTool) -> bool {
        let Self { resp, cx, pointer, origin, page_rect, pressed_here, over_page, .. } = *self;
        let cv = &mut view.comments;
        if !cx.allowed {
            return false;
        }
        if over_page {
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
        }
        // A text box also takes a drag, as in Acrobat: the box is drawn at that size and its
        // text wraps inside it (#2669).
        if tool == CommentTool::TextBox {
            if resp.drag_started()
                && pressed_here
                && let Some(o) = origin
            {
                cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![cx.to_user(o)] });
                cv.select(None);
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
        if self.clicked()
            && over_page
            && let Some(p) = pointer
        {
            let kind = if tool == CommentTool::Note { ComposerKind::Note } else { ComposerKind::TextBox { size: None } };
            cv.composer = Some(Composer { page: cx.page, at: cx.to_user(p), kind, text: String::new(), focus: true });
            cv.select(None);
        }
        true
    }
}

/// Delete / Escape handling for comments (only while no text field has focus).
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView, tool: &mut QuickTool, allowed: bool) {
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
        && let Some((page, index)) = cv.target()
        && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        cv.select(None);
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
    let Some((page, index)) = cv.target() else { return };
    let movable = info
        .annotations
        .iter()
        .any(|a| a.page == page && a.index == index && (!a.is_reply() || a.is_group_member()) && !is_markup(&a.subtype) && !a.locked);
    let (Some(xf), Some(p)) = (view.page_xform(page), info.pages.get(page)) else { return };
    if !movable {
        return;
    }
    use egui::Key;
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        // Only an arrow pressed alone or with ⌘/Ctrl or Shift, judged by each press's own
        // modifiers. Alt+← / Alt+→ go back and forward through the view history in
        // `canvas::shortcuts` (#3632), and egui's key patterns would let an extra Alt through.
        // A Ctrl that isn't the command key (macOS) is left alone, as the patterns left it.
        i.events.retain(|e| {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { return true };
            if modifiers.alt || (modifiers.ctrl && !modifiers.command) {
                return true;
            }
            let direction = match key {
                Key::ArrowLeft => vec2(-1.0, 0.0),
                Key::ArrowRight => vec2(1.0, 0.0),
                Key::ArrowUp => vec2(0.0, -1.0),
                Key::ArrowDown => vec2(0.0, 1.0),
                _ => return true,
            };
            step += direction * if modifiers.command || modifiers.shift { 10.0 } else { 1.0 };
            false
        });
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
