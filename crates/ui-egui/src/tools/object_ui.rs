//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_router::{Hit, ObjectsPart, Owner};

const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
/// How many faint inline-graphic outlines are drawn per page per frame, like the paragraph and
/// image outlines. Past this, only the selected and hovered graphics are boxed.
const MAX_GRAPHIC_OUTLINES: usize = 250;

#[derive(Default)]
pub(crate) struct ObjectSelection {
    generation: Option<u64>,
    cache: HashMap<usize, Result<Vec<EditableObject>, String>>,
    page: Option<usize>,
    selected: Vec<EditableObject>,
    gesture: Option<Gesture>,
    /// A handle-resize drag of the one selected graphic (it has no single-object tool).
    resize: Option<ResizeDrag>,
    /// Expected geometry after our own edit, for remapping indexes that grouping can change.
    pending: Option<Vec<EditableObject>>,
    pub notice: Option<String>,
    pub hold_editor: bool,
    blocked_page: Option<usize>,
    input_blocked: bool,
    /// A paragraph (page, block) to select once the document is at hand: Esc in the inline
    /// editor asks for it, and the next `prepare` selects it.
    select_paragraph: Option<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct Gesture {
    start: Pos2,
    now: Pos2,
    // For a marquee selection, the start in view space, so the rectangle's corner stays
    // anchored to the page content while the page scrolls under the pointer.
    start_view: Option<(f32, f32)>,
    marquee: bool,
    additive: bool,
}

/// A graphic's handle-resize drag: the grip (0–3 corners from the left-top clockwise, 4–7 the
/// top, right, bottom and left side midpoints), the press and last pointer (screen), its box at
/// the press (user space) and the box the release would give it (screen), updated every frame
/// for the preview.
#[derive(Clone, Copy)]
struct ResizeDrag {
    grip: usize,
    start: Pos2,
    now: Pos2,
    rect: [f64; 4],
    preview: Rect,
}

impl ObjectSelection {
    /// Clear any object-selection gesture and the page it was blocking.
    pub(crate) fn cancel_pointer_gesture(&mut self) {
        self.gesture = None;
        self.resize = None;
        self.blocked_page = None;
    }

    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// The page and every selected object reference, unless a drag is in flight. A group is
    /// restacked together, keeping its own painting order.
    pub(crate) fn targets(&self) -> Option<(usize, Vec<ObjectTarget>)> {
        let page = self.page?;
        (self.gesture.is_none() && self.resize.is_none() && !self.selected.is_empty())
            .then(|| (page, self.selected.iter().map(|o| o.target).collect()))
    }

    /// Keep the selection across a restack: the edit only moves the drawings in the page's paint
    /// order, so the same objects are remapped by their unchanged geometry and text.
    pub(crate) fn keep_after_restack(&mut self) {
        if !self.selected.is_empty() {
            self.pending = Some(self.selected.clone());
        }
    }

    /// A preview belongs to the active, focused document. Returning from another tab or a
    /// modal must not turn a release that happened elsewhere into a document edit.
    pub fn set_input_blocked(&mut self, blocked: bool) {
        self.input_blocked = blocked;
        if blocked {
            self.gesture = None;
            self.resize = None;
            self.blocked_page = None;
        }
    }

    fn clear(&mut self) {
        self.page = None;
        self.selected.clear();
        self.gesture = None;
        self.resize = None;
        self.pending = None;
        self.blocked_page = None;
        self.select_paragraph = None;
    }

    /// Select paragraph `block` of `page` as an object at the next `prepare`, as Acrobat does
    /// when Esc leaves a text box: the box is then selected, so Delete removes it, the arrow keys
    /// move it and its frame drags it, without aiming for the frame.
    pub(crate) fn select_paragraph_next(&mut self, page: usize, block: usize) {
        self.select_paragraph = Some((page, block));
    }

    pub fn prepare(&mut self, doc: &Document, enabled: bool, ctx: &egui::Context) {
        self.hold_editor = false;
        if !enabled {
            self.clear();
            return;
        }
        let generation = doc.edit_generation();
        if self.generation != Some(generation) {
            self.cache.clear();
            self.gesture = None;
            self.resize = None;
            let expected = self.pending.take();
            let remapped = self.page.zip(expected).and_then(|(page, expected)| {
                let inventory = doc.editable_objects(page).ok()?;
                let mut out = Vec::new();
                for old in expected {
                    let mut matches = inventory.iter().filter(|o| {
                        o.target.kind == old.target.kind
                            && o.content_type == old.content_type
                            && o.text == old.text
                            && o.rect.iter().zip(old.rect).all(|(a, b)| (a - b).abs() < 0.01)
                    });
                    let object = matches.next()?;
                    if matches.next().is_some() || out.iter().any(|o: &EditableObject| o.target == object.target) {
                        return None;
                    }
                    out.push(object.clone());
                }
                Some(out)
            });
            if let Some(objects) = remapped {
                self.selected = objects;
            } else {
                self.clear();
            }
            self.generation = Some(generation);
        } else {
            // A refused edit keeps the generation and the original selection.
            self.pending = None;
        }
        if self.page.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.clear();
        }
        // After the Escape check: the Esc that closed the editor must not clear what it selected.
        if let Some((page, block)) = self.select_paragraph.take()
            && let Ok(objects) = self.inventory(doc, page)
            && let Some(object) = objects.into_iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
        {
            self.clear();
            self.page = Some(page);
            self.selected.push(object);
        }
    }

    /// The move of the whole selection by `offset` (user space). The selection follows it.
    fn move_by(&mut self, page: usize, offset: [f64; 2]) -> Edit {
        let mut expected = self.selected.clone();
        for object in &mut expected {
            object.rect = [object.rect[0] + offset[0], object.rect[1] + offset[1], object.rect[2] + offset[0], object.rect[3] + offset[1]];
        }
        self.pending = Some(expected);
        Edit::MoveObjects { page, objects: self.selected.iter().map(|o| o.target).collect(), offset }
    }

    fn inventory(&mut self, doc: &Document, page: usize) -> Result<Vec<EditableObject>, String> {
        if self.cache.len() >= 8 && !self.cache.contains_key(&page) {
            self.cache.clear();
        }
        self.cache.entry(page).or_insert_with(|| doc.editable_objects(page)).clone()
    }
}

fn screen_rect(xf: &PageXform, info: &DocInfo, page: usize, rect: [f64; 4]) -> Rect {
    xf.user_rect(info, page, rect.map(|v| v as f32))
}

fn screen(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject) -> Rect {
    screen_rect(xf, info, page, object.rect)
}

/// The resize handles of a single selected graphic: the four corners, then the midpoints of the
/// top, right, bottom and left sides.
fn resized_grips(b: Rect) -> [Pos2; 8] {
    [
        b.left_top(),
        b.right_top(),
        b.left_bottom(),
        b.right_bottom(),
        Pos2::new(b.center().x, b.top()),
        Pos2::new(b.right(), b.center().y),
        Pos2::new(b.center().x, b.bottom()),
        Pos2::new(b.left(), b.center().y),
    ]
}

/// The smallest side a resize may leave (screen pixels), so a handle cannot turn the box inside
/// out.
const MIN_RESIZE: f32 = 2.0;

/// The box a corner drag gives `b` (screen), keeping its aspect ratio, as `image_input` does:
/// the axis with the larger relative pointer change wins and a handle grabbed off-centre cannot
/// make the box jump. A side grip moves only its own edge, changing one dimension freely; the
/// box may leave the page.
fn resized_box(b: Rect, grip: usize, start: Pos2, p: Pos2) -> Rect {
    let d = p - start;
    match grip {
        0..=3 => {
            let fixed = resized_grips(b)[3 - grip];
            let (w0, h0) = (b.width().max(1.0), b.height().max(1.0));
            let horizontal = d.x * (start.x - fixed.x).signum() / w0;
            let vertical = d.y * (start.y - fixed.y).signum() / h0;
            let delta = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
            let k = (1.0 + delta).max(0.05);
            let (w, h) = (w0 * k, h0 * k);
            let x = if start.x < fixed.x { fixed.x - w } else { fixed.x };
            let y = if start.y < fixed.y { fixed.y - h } else { fixed.y };
            Rect::from_min_size(Pos2::new(x, y), egui::vec2(w, h))
        }
        4 => Rect::from_min_max(Pos2::new(b.left(), (b.top() + d.y).min(b.bottom() - MIN_RESIZE)), b.max),
        6 => Rect::from_min_max(b.min, Pos2::new(b.right(), (b.bottom() + d.y).max(b.top() + MIN_RESIZE))),
        7 => Rect::from_min_max(Pos2::new((b.left() + d.x).min(b.right() - MIN_RESIZE), b.top()), b.max),
        _ => Rect::from_min_max(b.min, Pos2::new((b.right() + d.x).max(b.left() + MIN_RESIZE), b.bottom())),
    }
}

fn resize_cursor(grip: usize) -> egui::CursorIcon {
    match grip {
        0 | 3 => egui::CursorIcon::ResizeNwSe,
        1 | 2 => egui::CursorIcon::ResizeNeSw,
        4 | 6 => egui::CursorIcon::ResizeVertical,
        _ => egui::CursorIcon::ResizeHorizontal,
    }
}

/// White handles on a selected graphic's box: a picture's corners plus one at each side's middle.
fn handles(painter: &egui::Painter, b: Rect) {
    for c in resized_grips(b) {
        painter.rect(Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 0.0, Color32::WHITE, Stroke::new(1.0, BLUE), egui::StrokeKind::Middle);
    }
}

/// A screen rectangle as a user-space box (the page's own axes; crop, rotation and UserUnit
/// handled by the page info, as `edit_text_ui::user_box` does for pictures).
fn user_box(xf: &PageXform, info: &DocInfo, page: usize, r: Rect) -> [f64; 4] {
    let p = &info.pages[page];
    let (a, b) = (xf.screen_to_view(r.min), xf.screen_to_view(r.max));
    let (u, v) = (p.view_to_user(a.0, a.1), p.view_to_user(b.0, b.1));
    [f64::from(u[0].min(v[0])), f64::from(u[1].min(v[1])), f64::from(u[0].max(v[0])), f64::from(u[1].max(v[1]))]
}

fn modified(ui: &egui::Ui) -> bool {
    ui.input(|i| {
        let modifiers = i
            .events
            .iter()
            .find_map(|e| match e {
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: true, modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(i.modifiers);
        modifiers.shift || modifiers.ctrl || modifiers.command
    })
}

fn single(view: &DocView, page: usize) -> Option<ObjectTarget> {
    if let Some((_, index)) = view.content.selected.filter(|(p, _)| *p == page) {
        Some(ObjectTarget { kind: ObjectKind::Added, index })
    } else if let Some(s) = view.image_selection.as_ref().filter(|s| s.page == page) {
        Some(ObjectTarget { kind: ObjectKind::Image, index: s.index })
    } else {
        view.line_editor.as_ref().filter(|e| e.page == page).map(|e| ObjectTarget { kind: ObjectKind::Text, index: e.block })
    }
}

fn leave_single(view: &mut DocView) {
    view.content.selected = None;
    view.image_selection = None;
    view.line_editor = None;
    view.block_drag = None;
}

/// What of the objects lies under `p` on `page`: the objects selected together (a drag moves
/// them), or the frame around the paragraph open for typing (`open`: its box on screen), which
/// takes hold of the box as in Acrobat: a click selects it, a drag moves it, its text keeps the
/// caret.
pub(crate) fn hit(view: &DocView, xf: &PageXform, info: &DocInfo, page: usize, open: Option<Rect>, p: Pos2) -> Option<Hit> {
    let s = &view.objects;
    if s.page == Some(page) && !s.selected.is_empty() && s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o))).contains(p)
    {
        return Some(Hit::new(Owner::Objects { part: ObjectsPart::Selection }, Layer::Placed, egui::CursorIcon::Move));
    }
    open.filter(|r| on_frame(r.shrink(2.0), p)).map(|_| Hit::new(Owner::Objects { part: ObjectsPart::Frame }, Layer::Handle, egui::CursorIcon::Move))
}

/// A graphic covering nearly the whole page is background artwork: a click should reach the
/// smaller icon or text on top of it, never select and delete the page's background.
fn background_rect(info: &DocInfo, page: usize, rect: [f64; 4]) -> bool {
    let p = &info.pages[page];
    let [x0, y0, x1, y1] = p.crop;
    let page_area = f64::from((x1 - x0).max(0.0)) * f64::from((y1 - y0).max(0.0));
    let width = (rect[2].min(f64::from(x1)) - rect[0].max(f64::from(x0))).max(0.0);
    let height = (rect[3].min(f64::from(y1)) - rect[1].max(f64::from(y0))).max(0.0);
    page_area > 0.0 && width * height >= page_area * 0.9
}

fn background_graphic(info: &DocInfo, page: usize, graphic: &EditableObject) -> bool {
    background_rect(info, page, graphic.rect)
}

fn area(o: &EditableObject) -> f64 {
    (o.rect[2] - o.rect[0]).max(0.0) * (o.rect[3] - o.rect[1]).max(0.0)
}

fn area_rect(rect: Rect) -> f32 {
    rect.width().max(0.0) * rect.height().max(0.0)
}

/// Returns true while group selection owns the interaction. Ordinary clicks still reach the
/// single-object text/resize tools. A text draft is finished or discarded through its editor.
pub(crate) fn page_input(cx: PageToolContext<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, xf, ptr, page, .. } = cx;
    if view.objects.input_blocked {
        return true;
    }
    if view.content.draft.is_some() || view.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        let blocked = view.objects.blocked_page == Some(page);
        let modifier_press = ptr.pressed().is_some() && modified(ui) && ptr.pos.is_some_and(|p| xf.rect.contains(p) && ui.clip_rect().contains(p));
        if blocked || modifier_press {
            view.objects.hold_editor = true;
            view.objects.blocked_page = ptr.primary_down.then_some(page);
            if modifier_press {
                view.objects.notice = Some(tl!("Finish or discard the text being edited before selecting several objects.").into());
            }
            return true;
        }
        return false;
    }
    let mut selection = std::mem::take(&mut view.objects);
    let result = input(&mut selection, cx, view);
    view.objects = selection;
    result
}

/// Adds every object the marquee area touches to the selection, or replaces it when `keep` is
/// false, capped so a stray drag over a dense page cannot select an unbounded set.
fn select_within(s: &mut ObjectSelection, objects: Vec<EditableObject>, area: Rect, keep: bool, xf: &PageXform, info: &DocInfo, page: usize) {
    let mut selected = if keep { s.selected.clone() } else { Vec::new() };
    for object in objects.into_iter().filter(|o| area.intersects(screen(xf, info, page, o))) {
        if !selected.iter().any(|o| o.target == object.target) {
            selected.push(object);
        }
    }
    if selected.len() <= MAX_MOVE_OBJECTS {
        s.selected = selected;
    } else {
        s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
    }
}

/// Continue the handle-resize drag of the one selected graphic: preview the box under the
/// pointer every frame and commit one resize on release. Returns true while it owns the pointer.
fn resize_input(s: &mut ObjectSelection, pointer: Option<Pos2>, down: bool, xf: &PageXform, info: &DocInfo, page: usize, view: &mut DocView) -> bool {
    let Some(mut drag) = s.resize.filter(|_| s.page == Some(page)) else { return false };
    if let Some(p) = pointer {
        drag.now = p;
    }
    drag.preview = resized_box(screen_rect(xf, info, page, drag.rect), drag.grip, drag.start, drag.now);
    if down {
        s.resize = Some(drag);
    } else {
        s.resize = None;
        let target = user_box(xf, info, page, drag.preview);
        if drag.preview.is_positive()
            && let [object] = s.selected.as_slice()
            && target != object.rect
        {
            let index = object.target.index;
            let mut expected = s.selected.clone();
            expected[0].rect = target;
            s.pending = Some(expected);
            view.pending_edit = Some(Edit::ResizePageGraphic { page, index, rect: target });
        }
    }
    view.cursor.claim(Layer::Drag, resize_cursor(drag.grip));
    true
}

fn input(s: &mut ObjectSelection, cx: PageToolContext<'_>, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, doc, .. } = cx;
    let pointer = ptr.pos;
    let (pressed, down) = (ptr.pressed().is_some(), ptr.primary_down);
    let info = &doc.info;
    if s.page == Some(page)
        && let Some(mut gesture) = s.gesture
    {
        if let Some(p) = pointer {
            gesture.now = p;
        }
        // Anchor a marquee's start to the page content, so scrolling moves the rectangle with
        // the page instead of leaving it pinned to the screen.
        let start_screen = gesture.start_view.map(|v| xf.norm_to_screen(v.0 / xf.pw, v.1 / xf.ph)).unwrap_or(gesture.start);
        // While a marquee drag moves the rectangle, the intersecting objects are highlighted
        // live, so the box that follows the pointer already shows what is being selected.
        if down
            && gesture.marquee
            && let Some(objects) = s.inventory(doc, page).ok()
        {
            let area = Rect::from_two_pos(start_screen, gesture.now).intersect(xf.rect);
            if area.width() > 3.0 && area.height() > 3.0 {
                select_within(s, objects, area, gesture.additive, xf, info, page);
            }
        }
        if !down {
            if !gesture.marquee && start_screen.distance(gesture.now) <= 3.0 && resp.double_clicked() {
                s.clear();
                return false;
            }
            s.gesture = None;
            if gesture.marquee {
                let area = Rect::from_two_pos(start_screen, gesture.now).intersect(xf.rect);
                if !gesture.additive {
                    s.selected.clear();
                }
                if area.width() > 3.0
                    && area.height() > 3.0
                    && let Ok(objects) = s.inventory(doc, page)
                {
                    select_within(s, objects, area, true, xf, info, page);
                }
                if s.selected.is_empty() {
                    s.clear();
                }
            } else if gesture.start.distance(gesture.now) > 3.0 && view.pending_edit.is_none() {
                let (a, b) = (xf.screen_to_view(start_screen), xf.screen_to_view(gesture.now));
                match doc.object_move_offset(page, [f64::from(b.0 - a.0), f64::from(b.1 - a.1)]) {
                    Ok(offset) => view.pending_edit = Some(s.move_by(page, offset)),
                    Err(_) => s.notice = Some(tl!("These objects could not be moved. Select them again and try a smaller move.").into()),
                }
            }
        } else {
            s.gesture = Some(gesture);
        }
        view.cursor.claim(Layer::Drag, if gesture.marquee { egui::CursorIcon::Crosshair } else { egui::CursorIcon::Move });
        return true;
    }
    if resize_input(s, pointer, down, xf, info, page, view) {
        return true;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p) && ui.clip_rect().contains(*p) && resp.rect.contains(*p)) else {
        return s.page == Some(page) && !s.selected.is_empty();
    };
    if !pressed {
        if s.page == Some(page) && !s.selected.is_empty() {
            let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
            if union.contains(p) {
                let grip = match s.selected.as_slice() {
                    [object] if object.target.kind == ObjectKind::Graphic => {
                        resized_grips(screen(xf, info, page, object)).iter().position(|c| c.distance(p) < 8.0)
                    }
                    _ => None,
                };
                view.cursor.claim(Layer::Placed, grip.map_or(egui::CursorIcon::Move, resize_cursor));
            }
            return true;
        }
        return false;
    }
    let objects = match s.inventory(doc, page) {
        Ok(objects) => objects,
        Err(_) => {
            if modified(ui) {
                s.notice = Some(tl!("This page contains objects that cannot be selected together.").into());
                return true;
            }
            return false;
        }
    };
    let additive = modified(ui);
    // A page-sized background image must not hide editable labels or added notes.
    // Within each kind, prefer the last object as the ordinary single-item tools do. Vector
    // graphics come last (text and pictures printed over artwork stay reachable), and among
    // them the smallest wins, so a small icon is picked before a larger group around it.
    let hit = [ObjectKind::Added, ObjectKind::Text, ObjectKind::Image]
        .into_iter()
        .find_map(|kind| objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p)))
        .or_else(|| {
            objects
                .iter()
                .filter(|o| o.target.kind == ObjectKind::Graphic && !background_graphic(info, page, o))
                .filter(|o| screen(xf, info, page, o).expand(2.0).contains(p))
                .min_by(|a, b| area(a).total_cmp(&area(b)))
        });
    if additive {
        if s.page != Some(page) {
            s.clear();
            s.page = Some(page);
            if let Some(target) = single(view, page)
                && let Some(object) = objects.iter().find(|o| o.target == target)
            {
                s.selected.push(object.clone());
            }
        }
        if let Some(object) = hit {
            if let Some(index) = s.selected.iter().position(|o| o.target == object.target) {
                s.selected.remove(index);
            } else if s.selected.len() < MAX_MOVE_OBJECTS {
                s.selected.push(object.clone());
            } else {
                s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
            }
        } else {
            s.gesture = Some(Gesture { start: p, now: p, start_view: Some(xf.screen_to_view(p)), marquee: true, additive: true });
        }
        leave_single(view);
        view.current = page;
        return true;
    }
    // What the press was on ([`hit`]): the selection moves; the frame of the paragraph open for
    // typing selects its box (for the arrow keys) and moves it.
    let owner = view.router.owner(page);
    // A press on a corner or side handle of the one selected graphic resizes it instead of
    // moving it.
    if s.page == Some(page)
        && let [object] = s.selected.as_slice()
        && object.target.kind == ObjectKind::Graphic
        && let Some(grip) = resized_grips(screen(xf, info, page, object)).iter().position(|c| c.distance(p) < 8.0)
    {
        s.resize = Some(ResizeDrag { grip, start: p, now: p, rect: object.rect, preview: screen(xf, info, page, object) });
        view.cursor.claim(Layer::Handle, resize_cursor(grip));
        return true;
    }
    // Vector graphics have no single-object tool of their own: an ordinary click selects one, a
    // drag moves it (the same press starts the move, as on a picture), and Delete or Backspace
    // removes it. The handles above resize the one selected graphic: corners keep its aspect
    // ratio, side midpoints stretch one axis.
    if let Some(object) = hit.filter(|o| o.target.kind == ObjectKind::Graphic)
        && !(s.selected.len() > 1 && s.selected.iter().any(|o| o.target == object.target))
    {
        s.clear();
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, start_view: None, marquee: false, additive: false });
        leave_single(view);
        view.current = page;
        return true;
    }
    if owner == Some(Owner::Objects { part: ObjectsPart::Selection }) && !resp.double_clicked() {
        s.gesture = Some(Gesture { start: p, now: p, start_view: None, marquee: false, additive: false });
        return true;
    }
    if s.page == Some(page) && !s.selected.is_empty() {
        let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
        if union.contains(p) && !resp.double_clicked() {
            s.gesture = Some(Gesture { start: p, now: p, start_view: None, marquee: false, additive: false });
            return true;
        }
    }
    if owner == Some(Owner::Objects { part: ObjectsPart::Frame })
        && let Some(block) = view.line_editor.as_ref().filter(|e| e.page == page && !e.has_changes()).map(|e| e.block)
        && let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
        && on_frame(screen(xf, info, page, object), p)
        && !(s.selected.len() > 1 && s.selected.iter().any(|o| o.target == object.target))
    {
        s.clear();
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, start_view: None, marquee: false, additive: false });
        leave_single(view);
        view.current = page;
        return true;
    }
    s.clear();
    // A press on something (a paragraph, its edge reaching past its box #2276, a picture, added
    // content) is its own; only one on the empty page starts an area selection.
    if hit.is_some() || view.router.owner(page).is_some() {
        return false;
    }
    leave_single(view);
    s.page = Some(page);
    s.gesture = Some(Gesture { start: p, now: p, start_view: Some(xf.screen_to_view(p)), marquee: true, additive: false });
    view.current = page;
    true
}

/// Whether `p` is on the frame drawn around a paragraph open for typing (`r`, screen): just
/// outside the text, where a press would otherwise close the paragraph.
fn on_frame(r: Rect, p: Pos2) -> bool {
    r.expand(2.0 + crate::edit_text_ui::FRAME_REACH).contains(p) && !r.expand(2.0).contains(p)
}

/// The arrow keys move the selected image or objects a point, Shift and an arrow ten points,
/// as in Acrobat. They are taken before the view turns the page with them.
pub(crate) fn nudge_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() {
        return;
    }
    let image = view.image_selection.as_ref().filter(|s| !s.dragging()).map(|s| (s.page, s.index));
    let objects = view.objects.page.filter(|_| !view.objects.selected.is_empty() && view.objects.gesture.is_none() && view.objects.resize.is_none());
    let Some(page) = objects.or(image.map(|(page, _)| page)) else { return };
    let Some(xf) = view.page_xform(page) else { return };
    use egui::Key;
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        // Only an arrow pressed alone or with Shift, judged by each press's own modifiers (the
        // frame's can differ when a key is let go in the same frame). Alt+← / Alt+→ go back and
        // forward through the view history (#3632) and Ctrl/⌘+← / → turn the page, both in
        // `canvas::shortcuts`.
        i.events.retain(|e| {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { return true };
            if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
                return true;
            }
            let direction = match key {
                Key::ArrowLeft => egui::vec2(-1.0, 0.0),
                Key::ArrowRight => egui::vec2(1.0, 0.0),
                Key::ArrowUp => egui::vec2(0.0, -1.0),
                Key::ArrowDown => egui::vec2(0.0, 1.0),
                _ => return true,
            };
            step += direction * if modifiers.shift { 10.0 } else { 1.0 };
            false
        });
    });
    if step == egui::Vec2::ZERO {
        return;
    }
    // A point on screen, whatever the zoom and the view's rotation: one screen pixel's step in
    // view space, at unit length.
    let c = xf.rect.center();
    let unit = |d: egui::Vec2| {
        let (a, b) = (xf.screen_to_view(c), xf.screen_to_view(c + d));
        let v = egui::vec2(b.0 - a.0, b.1 - a.1);
        if v.length() > 0.0 { v / v.length() } else { egui::Vec2::ZERO }
    };
    let delta = unit(egui::vec2(1.0, 0.0)) * step.x + unit(egui::vec2(0.0, 1.0)) * step.y;
    let Ok(offset) = doc.object_move_offset(page, [f64::from(delta.x), f64::from(delta.y)]) else { return };
    if objects.is_some() {
        view.pending_edit = Some(view.objects.move_by(page, offset));
    } else if let Some((page, index)) = image
        && let Some(r) = doc.page_images(page).get(index).map(|im| im.rect)
    {
        let rect = [r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]];
        view.pending_edit = Some(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Move(rect) });
    }
}

/// Delete (or Backspace) removes the selected objects, as in Acrobat: a paragraph selected by its
/// frame, or everything an area selection took, goes at once instead of one typed character at a
/// time.
pub(crate) fn delete_keys(ctx: &egui::Context, view: &mut DocView) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() || view.objects.gesture.is_some() || view.objects.resize.is_some() {
        return;
    }
    let Some(page) = view.objects.page.filter(|_| !view.objects.selected.is_empty()) else { return };
    use egui::{Key, Modifiers};
    if !ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)) {
        return;
    }
    // The selection stays until the edit applies: `prepare` clears it when the document changes,
    // and a refused delete (text used as a clipping path) leaves it selected.
    view.pending_edit = delete_edit(page, view.objects.selected.iter().map(|o| o.target).collect());
}

/// The edit that deletes `targets` (indexes into one inventory of `page`) as one undo step.
/// Each deletion renumbers only what follows it in its own inventory, so paragraphs and images
/// go from the last to the first, and added content, which the source inventories leave out,
/// after them. Graphics go before text and images: removing the text from a shared scope can
/// regroup its paths, which would move the graphic index the selection still names.
fn delete_edit(page: usize, mut targets: Vec<ObjectTarget>) -> Option<Edit> {
    targets.sort_by_key(|t| (t.kind != ObjectKind::Graphic, t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    let mut edits: Vec<Edit> = targets
        .into_iter()
        .map(|t| match t.kind {
            ObjectKind::Text => Edit::EditTextBlock { page, block: t.index, text: String::new(), style: pdfcraft_engine::BlockStyle::default() },
            ObjectKind::Image => Edit::EditPageImage { page, index: t.index, change: pdfcraft_engine::ImageEdit::Delete },
            ObjectKind::Graphic => Edit::DeletePageGraphic { page, index: t.index },
            ObjectKind::Added => Edit::DeleteContent { page, index: t.index },
        })
        .collect();
    if edits.len() > 1 {
        // "Delete content" is an edit label already, so the history reads the same in every language.
        Some(Edit::Batch { label: "Delete content".into(), edits })
    } else {
        edits.pop()
    }
}

pub(crate) fn paint(
    painter: &egui::Painter,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    selection: &ObjectSelection,
    graphics: &[pdfcraft_engine::PageGraphic],
    pointer: Option<Pos2>,
) {
    let selected = |index: usize| {
        selection.page == Some(page) && selection.selected.iter().any(|o| o.target.kind == ObjectKind::Graphic && o.target.index == index)
    };
    // Inline graphics have no single-object tool of their own, so they get the same faint boxes
    // paragraphs and images show: the boxes say what the tool can select before it is clicked.
    let boxes: Vec<(usize, Rect)> = graphics
        .iter()
        .enumerate()
        .filter(|(index, _)| !background_rect(info, page, graphics[*index].rect))
        .map(|(index, graphic)| (index, screen_rect(xf, info, page, graphic.rect)))
        .filter(|(_, rect)| rect.is_positive())
        .collect();
    let hovered = pointer.filter(|p| xf.rect.contains(*p)).and_then(|p| {
        boxes
            .iter()
            .filter(|(index, _)| !selected(*index))
            .filter(|(_, rect)| rect.expand(2.0).contains(p))
            .min_by(|(_, a), (_, b)| area_rect(*a).total_cmp(&area_rect(*b)))
            .map(|(index, _)| *index)
    });
    let faint_all = boxes.len() <= MAX_GRAPHIC_OUTLINES;
    for (index, rect) in &boxes {
        if !selected(*index) && (faint_all || hovered == Some(*index)) {
            painter.rect_stroke(*rect, 0.0, Stroke::new(0.75, BLUE.gamma_multiply(0.35)), egui::StrokeKind::Outside);
        }
    }
    if let Some(index) = hovered
        && let Some(graphic) = graphics.get(index)
    {
        let rect = screen_rect(xf, info, page, graphic.rect);
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5, BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    if selection.page != Some(page) {
        return;
    }
    let delta = selection.gesture.filter(|g| !g.marquee).map_or(egui::Vec2::ZERO, |g| g.now - g.start);
    let mut union = Rect::NOTHING;
    for object in &selection.selected {
        // While a resize drag previews the box below, the old box is not drawn as well.
        if selection.resize.is_some() && object.target.kind == ObjectKind::Graphic {
            continue;
        }
        let rect = screen(xf, info, page, object).translate(delta);
        painter.rect_filled(rect, 0.0, BLUE.gamma_multiply(0.05));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Outside);
        union = union.union(rect);
    }
    if selection.selected.len() > 1 {
        painter.rect_stroke(union.expand(4.0), 0.0, Stroke::new(1.0, BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    // The one selected graphic resizes by its corner handles, like a selected picture.
    if let [object] = selection.selected.as_slice()
        && object.target.kind == ObjectKind::Graphic
    {
        if let Some(drag) = selection.resize {
            let preview = drag.preview.intersect(xf.rect);
            if preview.is_positive() {
                painter.rect_filled(preview, 0.0, BLUE.gamma_multiply(0.04));
                painter.rect_stroke(preview, 0.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Outside);
                handles(painter, preview);
            }
        } else {
            handles(painter, screen(xf, info, page, object).translate(delta));
        }
    }
    if let Some(g) = selection.gesture.filter(|g| g.marquee) {
        let start = g.start_view.map(|v| xf.norm_to_screen(v.0 / xf.pw, v.1 / xf.ph)).unwrap_or(g.start);
        let area = Rect::from_two_pos(start, g.now).intersect(xf.rect);
        painter.rect_filled(area, 0.0, BLUE.gamma_multiply(0.08));
        painter.rect_stroke(area, 0.0, Stroke::new(1.0, BLUE), egui::StrokeKind::Outside);
    }
}

/// Keep the UI refusal actionable and translated; headless callers retain the detailed error.
pub(crate) fn move_error(error: &pdfcraft_engine::EditError) -> String {
    let detail = error.to_string();
    match detail.as_str() {
        "vertical text cannot be moved in a group" => tl!("Vertical text cannot be moved with this selection.").into(),
        "text used as a clipping path cannot be moved in a group" => tl!("Text used as a clipping path cannot be moved with this selection.").into(),
        _ => tl!("These objects could not be moved. Select them again and try a smaller move.").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(kind: ObjectKind, index: usize) -> ObjectTarget {
        ObjectTarget { kind, index }
    }

    /// Deleting renumbers what follows in the same inventory, so the batch takes each inventory
    /// from its last item to its first, and added content (outside the source inventories) last.
    /// Graphics go first, before a text edit can regroup the paths around them.
    #[test]
    fn several_objects_delete_last_first_in_one_step() {
        let targets = vec![
            t(ObjectKind::Added, 0),
            t(ObjectKind::Text, 1),
            t(ObjectKind::Image, 0),
            t(ObjectKind::Graphic, 2),
            t(ObjectKind::Text, 3),
            t(ObjectKind::Graphic, 4),
            t(ObjectKind::Added, 2),
        ];
        let Some(Edit::Batch { label, edits }) = delete_edit(0, targets) else { panic!("several objects make one batch") };
        assert_eq!(label, "Delete content");
        let order: Vec<(char, usize)> = edits
            .iter()
            .map(|e| match e {
                Edit::EditTextBlock { block, text, .. } => {
                    assert!(text.is_empty());
                    ('t', *block)
                }
                Edit::EditPageImage { index, change: pdfcraft_engine::ImageEdit::Delete, .. } => ('i', *index),
                Edit::DeletePageGraphic { index, .. } => ('g', *index),
                Edit::DeleteContent { index, .. } => ('a', *index),
                other => panic!("unexpected {}", other.label().wording),
            })
            .collect();
        assert_eq!(order, [('g', 4), ('g', 2), ('t', 3), ('t', 1), ('i', 0), ('a', 2), ('a', 0)]);
    }

    #[test]
    fn one_object_deletes_with_its_own_edit_and_none_with_nothing() {
        assert!(matches!(delete_edit(2, vec![t(ObjectKind::Image, 4)]), Some(Edit::EditPageImage { page: 2, index: 4, .. })));
        assert!(delete_edit(0, Vec::new()).is_none());
    }
}
