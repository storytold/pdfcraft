//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget, PathOutline, PathStyle};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};

const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

#[derive(Default)]
pub(crate) struct ObjectSelection {
    generation: Option<u64>,
    cache: HashMap<usize, Result<Vec<EditableObject>, String>>,
    page: Option<usize>,
    selected: Vec<EditableObject>,
    gesture: Option<Gesture>,
    /// Expected geometry after our own edit, for remapping indexes that grouping can change.
    pending: Option<Vec<EditableObject>>,
    pub notice: Option<String>,
    pub hold_editor: bool,
    blocked_page: Option<usize>,
    input_blocked: bool,
    /// One point of the selected path being dragged.
    vertex: Option<VertexDrag>,
}

/// A press on a point of the selected path, until the button is released.
#[derive(Clone, Copy)]
struct VertexDrag {
    point: usize,
    start: Pos2,
    now: Pos2,
}

#[derive(Clone, Copy)]
struct Gesture {
    start: Pos2,
    now: Pos2,
    marquee: bool,
    additive: bool,
}

impl ObjectSelection {
    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// A preview belongs to the active, focused document. Returning from another tab or a
    /// modal must not turn a release that happened elsewhere into a document edit.
    pub fn set_input_blocked(&mut self, blocked: bool) {
        self.input_blocked = blocked;
        if blocked {
            self.gesture = None;
            self.blocked_page = None;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.page = None;
        self.selected.clear();
        self.gesture = None;
        self.pending = None;
        self.blocked_page = None;
        self.vertex = None;
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
            let expected = self.pending.take();
            let remapped = self.page.zip(expected).and_then(|(page, expected)| {
                let inventory = doc.editable_objects(page).ok()?;
                let mut out = Vec::new();
                for old in expected {
                    let mut matches = inventory.iter().filter(|o| {
                        o.target.kind == old.target.kind
                            && o.content_type == old.content_type
                            && o.text == old.text
                            && if old.target.kind == ObjectKind::Path {
                                // A path is told apart by its points: its frame follows its line width.
                                same_points(o, &old)
                            } else {
                                o.rect.iter().zip(old.rect).all(|(a, b)| (a - b).abs() < 0.01)
                            }
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
    }

    /// The move of the whole selection by `offset` (user space). The selection follows it.
    fn move_by(&mut self, page: usize, offset: [f64; 2]) -> Edit {
        let mut expected = self.selected.clone();
        for object in &mut expected {
            object.rect = [object.rect[0] + offset[0], object.rect[1] + offset[1], object.rect[2] + offset[0], object.rect[3] + offset[1]];
            for point in object.outline.iter_mut().flat_map(|o| o.points.iter_mut()) {
                *point = [point[0] + offset[0], point[1] + offset[1]];
            }
        }
        self.pending = Some(expected);
        Edit::MoveObjects { page, objects: self.selected.iter().map(|o| o.target).collect(), offset }
    }

    /// The selected vector paths and their page, when the selection is made of paths only.
    pub(crate) fn paths(&self) -> Option<(usize, &[EditableObject])> {
        let page = self.page?;
        (!self.selected.is_empty() && self.selected.iter().all(|o| o.target.kind == ObjectKind::Path)).then_some((page, self.selected.as_slice()))
    }

    /// Format the selected paths; the selection follows (a path is known by its points).
    pub(crate) fn style_edit(&mut self, style: PathStyle) -> Option<Edit> {
        let (page, objects) = self.paths()?;
        let paths = objects.iter().map(|o| o.target.index).collect();
        self.pending = Some(self.selected.clone());
        Some(Edit::StylePaths { page, paths, style })
    }

    fn inventory(&mut self, doc: &Document, page: usize) -> Result<Vec<EditableObject>, String> {
        if self.cache.len() >= 8 && !self.cache.contains_key(&page) {
            self.cache.clear();
        }
        self.cache.entry(page).or_insert_with(|| doc.editable_objects(page)).clone()
    }
}

fn screen(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject) -> Rect {
    xf.user_rect(info, page, object.rect.map(|v| v as f32))
}

/// Whether two path objects have the same points (to a hundredth of a point).
fn same_points(a: &EditableObject, b: &EditableObject) -> bool {
    match (&a.outline, &b.outline) {
        (Some(a), Some(b)) => {
            a.points.len() == b.points.len() && a.points.iter().flatten().zip(b.points.iter().flatten()).all(|(x, y)| (x - y).abs() < 0.01)
        }
        _ => false,
    }
}

/// A page user-space point on screen.
fn point_screen(xf: &PageXform, info: &DocInfo, page: usize, p: [f64; 2]) -> Pos2 {
    xf.user_rect(info, page, [p[0] as f32, p[1] as f32, p[0] as f32, p[1] as f32]).min
}

/// The point of the only selected path that `p` is on, if any.
fn near_point(s: &ObjectSelection, xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> Option<usize> {
    if s.page != Some(page) {
        return None;
    }
    let [object] = s.selected.as_slice() else { return None };
    let outline = object.outline.as_ref().filter(|_| object.target.kind == ObjectKind::Path)?;
    outline.points.iter().position(|q| point_screen(xf, info, page, *q).distance(p) <= POINT_REACH)
}

/// Whether a press at `p` takes hold of the selection to move it: on one of its lines when it is
/// made of paths (a frame around a diagonal would cover other lines), else inside its frame.
fn on_selection(s: &ObjectSelection, xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> bool {
    if s.paths().is_some() {
        near_point(s, xf, info, page, p).is_some() || s.selected.iter().any(|o| path_hit(xf, info, page, o, p))
    } else {
        s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o))).contains(p)
    }
}

/// How near a click must be to a line to take it (screen pixels), beyond half its width.
const LINE_REACH: f32 = 4.0;
/// How near a press must be to a point of the selected path to take it (screen pixels).
const POINT_REACH: f32 = 7.0;

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// Whether `p` is inside the polygon `points` (even-odd).
fn polygon_contains(points: &[Pos2], p: Pos2) -> bool {
    let mut inside = false;
    let mut j = points.len().wrapping_sub(1);
    for (i, a) in points.iter().enumerate() {
        let Some(b) = points.get(j) else { break };
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether a click at `p` is on the vector path `object`: on its line, or inside it when it is
/// filled (a fill covering a quarter of the page is a background, which a click goes through).
fn path_hit(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject, p: Pos2) -> bool {
    let Some(outline) = object.outline.as_ref().filter(|_| object.target.kind == ObjectKind::Path) else { return false };
    let frame = screen(xf, info, page, object);
    if !frame.expand(LINE_REACH + 2.0).contains(p) {
        return false;
    }
    let points: Vec<Pos2> = outline.points.iter().map(|q| point_screen(xf, info, page, *q)).collect();
    let per_point = xf.rect.width() / xf.pw.max(1.0);
    let reach = if outline.stroked { outline.width as f32 * per_point / 2.0 } else { 0.0 } + LINE_REACH;
    let edges = points.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?)));
    let closing = outline.closed.then(|| points.first().zip(points.last()).map(|(a, b)| (*b, *a))).flatten();
    if (outline.stroked || points.len() < 3) && edges.chain(closing).any(|(a, b)| segment_distance(p, a, b) <= reach) {
        return true;
    }
    outline.filled && points.len() >= 3 && frame.area() < xf.rect.area() * 0.25 && polygon_contains(&points, p)
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

/// Returns true while group selection owns the interaction. Ordinary clicks still reach the
/// single-object text/resize tools. A text draft is finished or discarded through its editor.
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, page: usize, doc: &Document, view: &mut DocView) -> bool {
    if view.objects.input_blocked {
        return true;
    }
    if view.content.draft.is_some() || view.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        let blocked = view.objects.blocked_page == Some(page);
        let modifier_press = ui.input(|i| i.pointer.primary_pressed())
            && modified(ui)
            && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| xf.rect.contains(p) && ui.clip_rect().contains(p));
        if blocked || modifier_press {
            view.objects.hold_editor = true;
            view.objects.blocked_page = ui.input(|i| i.pointer.primary_down()).then_some(page);
            if modifier_press {
                view.objects.notice = Some(tl!("Finish or discard the text being edited before selecting several objects.").into());
            }
            return true;
        }
        return false;
    }
    let mut selection = std::mem::take(&mut view.objects);
    let result = input(&mut selection, ui, resp, xf, page, doc, view);
    view.objects = selection;
    result
}

#[allow(clippy::too_many_arguments)]
fn input(s: &mut ObjectSelection, ui: &egui::Ui, resp: &egui::Response, xf: &PageXform, page: usize, doc: &Document, view: &mut DocView) -> bool {
    let (pointer, pressed, down) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_pressed(), i.pointer.primary_down()));
    let info = &doc.info;
    // A point of the selected path being dragged: previewed, then one undoable edit on release.
    if s.page == Some(page)
        && let Some(mut drag) = s.vertex
    {
        if let Some(p) = pointer {
            drag.now = p;
        }
        if down {
            s.vertex = Some(drag);
        } else {
            s.vertex = None;
            if drag.start.distance(drag.now) > 2.0
                && view.pending_edit.is_none()
                && let [object] = s.selected.as_slice()
                && let Some(from) = object.outline.as_ref().and_then(|o| o.points.get(drag.point)).copied()
            {
                let (a, b) = (xf.screen_to_view(drag.start), xf.screen_to_view(drag.now));
                match doc.object_move_offset(page, [f64::from(b.0 - a.0), f64::from(b.1 - a.1)]) {
                    Ok(offset) => {
                        let to = [from[0] + offset[0], from[1] + offset[1]];
                        let mut expected = object.clone();
                        if let Some(point) = expected.outline.as_mut().and_then(|o| o.points.get_mut(drag.point)) {
                            *point = to;
                        }
                        let path = object.target.index;
                        s.pending = Some(vec![expected]);
                        view.pending_edit = Some(Edit::MovePathPoint { page, path, point: drag.point, to });
                    }
                    Err(_) => s.notice = Some(tl!("This point could not be moved. Try a smaller move.").into()),
                }
            }
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        return true;
    }
    if s.page == Some(page)
        && let Some(mut gesture) = s.gesture
    {
        if let Some(p) = pointer {
            gesture.now = p;
        }
        if !down {
            if !gesture.marquee && gesture.start.distance(gesture.now) <= 3.0 && resp.double_clicked() {
                s.clear();
                return false;
            }
            s.gesture = None;
            if gesture.marquee {
                let area = Rect::from_two_pos(gesture.start, gesture.now).intersect(xf.rect);
                if !gesture.additive {
                    s.selected.clear();
                }
                if area.width() > 3.0
                    && area.height() > 3.0
                    && let Ok(objects) = s.inventory(doc, page)
                {
                    // Pictures and text are taken when the box touches them; a line only when the box
                    // holds it whole, so a box over a drawing doesn't pick up its background shapes.
                    let candidates: Vec<_> = objects
                        .into_iter()
                        .filter(|o| {
                            let frame = screen(xf, info, page, o);
                            if o.target.kind == ObjectKind::Path { area.contains_rect(frame) } else { area.intersects(frame) }
                        })
                        .collect();
                    let mut selected = s.selected.clone();
                    for object in candidates {
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
                if s.selected.is_empty() {
                    s.clear();
                }
            } else if gesture.start.distance(gesture.now) > 3.0 && view.pending_edit.is_none() {
                let (a, b) = (xf.screen_to_view(gesture.start), xf.screen_to_view(gesture.now));
                match doc.object_move_offset(page, [f64::from(b.0 - a.0), f64::from(b.1 - a.1)]) {
                    Ok(offset) => view.pending_edit = Some(s.move_by(page, offset)),
                    Err(_) => s.notice = Some(tl!("These objects could not be moved. Select them again and try a smaller move.").into()),
                }
            }
        } else {
            s.gesture = Some(gesture);
        }
        ui.ctx().set_cursor_icon(if gesture.marquee { egui::CursorIcon::Crosshair } else { egui::CursorIcon::Move });
        return true;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p) && ui.clip_rect().contains(*p) && resp.rect.contains(*p)) else {
        return s.page == Some(page) && !s.selected.is_empty();
    };
    if !pressed {
        if s.page == Some(page) && !s.selected.is_empty() {
            if near_point(s, xf, info, page, p).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            } else if on_selection(s, xf, info, page, p) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
            return true;
        }
        return false;
    }
    // A press on a point of the only selected path takes that point.
    if !modified(ui)
        && let Some(point) = near_point(s, xf, info, page, p)
    {
        s.vertex = Some(VertexDrag { point, start: p, now: p });
        return true;
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
    // Within each kind, prefer the last object as the ordinary single-item tools do.
    let hit = [ObjectKind::Added, ObjectKind::Text]
        .into_iter()
        .find_map(|kind| objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p)))
        // A line or shape is taken by its drawing, not its frame; it comes before pictures, which
        // may be page-sized backgrounds.
        .or_else(|| objects.iter().rev().find(|o| path_hit(xf, info, page, o, p)))
        .or_else(|| objects.iter().rev().find(|o| o.target.kind == ObjectKind::Image && screen(xf, info, page, o).expand(2.0).contains(p)));
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
            s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: true });
        }
        leave_single(view);
        view.current = page;
        return true;
    }
    if s.page == Some(page) && !s.selected.is_empty() && on_selection(s, xf, info, page, p) && !resp.double_clicked() {
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false });
        return true;
    }
    // The frame of the paragraph open for typing takes hold of the box, as in Acrobat: a click
    // selects it (for the arrow keys), a drag moves it. Its text keeps the caret.
    if let Some(block) = view.line_editor.as_ref().filter(|e| e.page == page && !e.has_changes()).map(|e| e.block)
        && let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
        && on_frame(screen(xf, info, page, object), p)
    {
        s.clear();
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false });
        leave_single(view);
        view.current = page;
        return true;
    }
    s.clear();
    if let Some(object) = hit.filter(|o| o.target.kind == ObjectKind::Path) {
        leave_single(view);
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false });
        view.current = page;
        return true;
    }
    // A closed paragraph's width handle reaches past its box, and a press there rewraps the
    // paragraph (as in `edit_text_ui`) instead of starting an area selection (#2276).
    let on_edge = view.line_editor.as_ref().is_none_or(|e| e.page != page)
        && crate::edit_text_ui::grabs_edge(xf, info, page, objects.iter().filter(|o| o.target.kind == ObjectKind::Text).map(|o| o.rect), p);
    if hit.is_some() || on_edge {
        return false;
    }
    leave_single(view);
    s.page = Some(page);
    s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: false });
    view.current = page;
    true
}

/// Whether `p` is on the frame drawn around a paragraph open for typing (`r`, screen): just
/// outside the text, where a press would otherwise close the paragraph.
fn on_frame(r: Rect, p: Pos2) -> bool {
    r.expand(2.0 + FRAME_REACH).contains(p) && !r.expand(2.0).contains(p)
}

/// How far outside a paragraph's frame a press still grabs it (screen pixels).
const FRAME_REACH: f32 = 6.0;

/// The arrow keys move the selected image or objects a point, Shift and an arrow ten points,
/// as in Acrobat. They are taken before the view turns the page with them.
pub(crate) fn nudge_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() {
        return;
    }
    let image = view.image_selection.as_ref().filter(|s| !s.dragging()).map(|s| (s.page, s.index));
    let objects = view.objects.page.filter(|_| !view.objects.selected.is_empty() && view.objects.gesture.is_none());
    let Some(page) = objects.or(image.map(|(page, _)| page)) else { return };
    let Some(xf) = view.page_xform(page) else { return };
    use egui::{Key, Modifiers};
    let mut step = egui::Vec2::ZERO;
    ctx.input_mut(|i| {
        for (key, direction) in [
            (Key::ArrowLeft, egui::vec2(-1.0, 0.0)),
            (Key::ArrowRight, egui::vec2(1.0, 0.0)),
            (Key::ArrowUp, egui::vec2(0.0, -1.0)),
            (Key::ArrowDown, egui::vec2(0.0, 1.0)),
        ] {
            // Shift first: a plain key pattern also matches a Shift press.
            if i.consume_key(Modifiers::SHIFT, key) {
                step += direction * 10.0;
            } else if i.consume_key(Modifiers::NONE, key) {
                step += direction;
            }
        }
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
pub(crate) fn delete_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    if ctx.egui_wants_keyboard_input()
        || view.pending_edit.is_some()
        || view.objects.input_blocked
        || view.objects.gesture.is_some()
        || view.objects.vertex.is_some()
    {
        return;
    }
    let selection = view
        .objects
        .page
        .filter(|_| !view.objects.selected.is_empty())
        .map(|page| (page, view.objects.selected.iter().map(|o| o.target).collect()))
        .or_else(|| {
            view.image_selection
                .as_ref()
                .filter(|s| s.crop.is_none() && !s.dragging())
                .map(|s| (s.page, vec![ObjectTarget { kind: ObjectKind::Image, index: s.index }]))
        });
    let Some((page, targets)) = selection else { return };
    use egui::{Key, Modifiers};
    if !ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)) {
        return;
    }
    if view.objects.page.is_some() && view.objects.generation != Some(doc.edit_generation()) {
        view.objects.clear();
        return;
    }
    // apply_edit clears the selection only after success; a refusal leaves it selected.
    view.pending_edit = Some(Edit::DeleteObjects { page, objects: targets });
}

pub(crate) fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, selection: &ObjectSelection) {
    if selection.page != Some(page) {
        return;
    }
    let delta = selection.gesture.filter(|g| !g.marquee).map_or(egui::Vec2::ZERO, |g| g.now - g.start);
    let mut union = Rect::NOTHING;
    let single = selection.selected.len() == 1;
    for object in &selection.selected {
        if let Some(outline) = object.outline.as_ref().filter(|_| object.target.kind == ObjectKind::Path) {
            // A line or shape is outlined along its drawing, with a handle on each point when it
            // is the only one selected (a point being dragged follows the pointer).
            let mut points: Vec<Pos2> = outline.points.iter().map(|q| point_screen(xf, info, page, *q) + delta).collect();
            if let Some(drag) = selection.vertex
                && let Some(point) = points.get_mut(drag.point)
            {
                *point += drag.now - drag.start;
            }
            if outline.closed
                && let Some(first) = points.first().copied()
            {
                points.push(first);
            }
            painter.add(egui::Shape::line(points.clone(), Stroke::new(3.0, BLUE.gamma_multiply(0.55))));
            if single {
                for (i, point) in points.iter().take(outline.points.len()).enumerate() {
                    let handle = Rect::from_center_size(*point, egui::vec2(8.0, 8.0));
                    let held = selection.vertex.is_some_and(|d| d.point == i);
                    painter.rect_filled(handle, 1.0, if held { BLUE } else { Color32::WHITE });
                    painter.rect_stroke(handle, 1.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Middle);
                }
            }
            continue;
        }
        let rect = screen(xf, info, page, object).translate(delta);
        painter.rect_filled(rect, 0.0, BLUE.gamma_multiply(0.05));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Outside);
        union = union.union(rect);
    }
    if selection.selected.len() > 1 && union.is_positive() {
        painter.rect_stroke(union.expand(4.0), 0.0, Stroke::new(1.0, BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    if let Some(g) = selection.gesture.filter(|g| g.marquee) {
        let area = Rect::from_two_pos(g.start, g.now).intersect(xf.rect);
        painter.rect_filled(area, 0.0, BLUE.gamma_multiply(0.08));
        painter.rect_stroke(area, 0.0, Stroke::new(1.0, BLUE), egui::StrokeKind::Outside);
    }
}

/// The Edit panel for selected vector paths: line width and colour. Returns what was changed.
pub(crate) fn line_panel(ui: &mut egui::Ui, count: usize, look: &PathOutline) -> Option<PathStyle> {
    let mut style = PathStyle::default();
    crate::widgets::section_title(ui, if count == 1 { tl!("Line") } else { tl!("Lines") });
    ui.horizontal(|ui| {
        ui.label(tl!("Width"));
        let shown = format!("{} pt", (look.width * 100.0).round() / 100.0);
        egui::ComboBox::from_id_salt("path-width").selected_text(shown).width(80.0).show_ui(ui, |ui| {
            for width in [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0] {
                if ui.selectable_label((look.width - width).abs() < 0.01, format!("{width} pt")).clicked() {
                    style.width = Some(width);
                }
            }
        });
        // The colour a stroked line is drawn in, or a shape's fill.
        let current = look.stroke.filter(|_| look.stroked).or(look.fill).unwrap_or([0.0; 3]);
        let mut rgb = current.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        let pending = egui::Id::new("path-color-pending");
        if ui.color_edit_button_srgb(&mut rgb).on_hover_text(tl!("Line colour")).changed() {
            ui.data_mut(|d| d.insert_temp(pending, rgb));
        }
        // One undoable change when the picker is let go, not one for every colour it passes through.
        if !ui.input(|i| i.pointer.any_down())
            && let Some(rgb) = ui.data_mut(|d| d.remove_temp::<[u8; 3]>(pending))
        {
            style.color = Some(rgb.map(|v| f64::from(v) / 255.0));
        }
    });
    ui.label(egui::RichText::new(tl!("Drag a point to reshape the line. Delete removes it.")).small().weak());
    (style != PathStyle::default()).then_some(style)
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
    #[test]
    fn selection_labels_and_refusals_are_translated_in_every_catalog() {
        for language in crate::i18n::LANGUAGES.iter().filter(|l| !l.source.is_empty()) {
            let lang = crate::i18n::Lang::from_code(language.code).unwrap();
            for label in [
                "Drag the selection to move it. Esc clears the selection.",
                "Shift-click to select several objects. Drag empty space to select an area.",
                "Select at most 1000 objects at a time.",
                "Finish or discard the text being edited before selecting several objects.",
                "Move object",
                "Move objects",
                "Vertical text cannot be moved with this selection.",
                "Text used as a clipping path cannot be moved with this selection.",
                "These objects could not be moved. Select them again and try a smaller move.",
                "This page contains objects that cannot be selected together.",
            ] {
                assert_ne!(crate::i18n::tr(lang, label), label, "{}: {label}", language.code);
            }
            assert_ne!(crate::i18n::trn(lang, 2, "{n} object selected", "{n} objects selected"), "2 objects selected", "{}", language.code);
        }
    }
}
