//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectAlign, ObjectKind, ObjectTarget, PagePath, ZOrder};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::page_pointer::PagePointer;

const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
/// The size and reach of a resize handle (screen pixels), as for images.
const HANDLE: f32 = 8.0;

#[derive(Default)]
pub(crate) struct ObjectSelection {
    generation: Option<u64>,
    cache: HashMap<usize, Result<Vec<EditableObject>, String>>,
    /// One page's vector paths, for hit tests: each press would parse the page again.
    paths: Option<(usize, Vec<PagePath>)>,
    page: Option<usize>,
    selected: Vec<EditableObject>,
    gesture: Option<Gesture>,
    /// Expected geometry after our own edit, for remapping indexes that grouping can change;
    /// `false` when the boxes are the engine's to work out (an align) or stay (an arrange).
    pending: Option<(Vec<EditableObject>, bool)>,
    /// The page menu was opened on the selection's page: it offers the selection's commands.
    menu: bool,
    pub notice: Option<String>,
    pub hold_editor: bool,
    blocked_page: Option<usize>,
    input_blocked: bool,
}

#[derive(Clone, Copy)]
struct Gesture {
    start: Pos2,
    now: Pos2,
    marquee: bool,
    additive: bool,
    /// The resize handle being dragged (see [`crate::edit_text_ui::corners`]).
    corner: Option<usize>,
}

impl ObjectSelection {
    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// The selection's only object, when that is a vector path: it gets resize handles.
    fn single_path(&self) -> Option<&EditableObject> {
        match self.selected.as_slice() {
            [object] if object.target.kind == ObjectKind::Path => Some(object),
            _ => None,
        }
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

    fn clear(&mut self) {
        self.page = None;
        self.selected.clear();
        self.gesture = None;
        self.pending = None;
        self.blocked_page = None;
    }

    pub fn prepare(&mut self, doc: &Document, enabled: bool, ctx: &egui::Context) {
        self.hold_editor = false;
        // Set again by the page input when the right-click is on the selection's page.
        if ctx.input(|i| i.pointer.secondary_clicked()) {
            self.menu = false;
        }
        if !enabled {
            self.clear();
            return;
        }
        let generation = doc.edit_generation();
        if self.generation != Some(generation) {
            self.cache.clear();
            self.paths = None;
            self.gesture = None;
            let expected = self.pending.take();
            let remapped = self.page.zip(expected).and_then(|(page, (expected, placed))| {
                let inventory = doc.editable_objects(page).ok()?;
                let size = |r: [f64; 4]| [r[2] - r[0], r[3] - r[1]];
                let mut out = Vec::new();
                for old in expected {
                    let matches: Vec<_> = inventory
                        .iter()
                        .filter(|o| {
                            o.target.kind == old.target.kind
                                && o.content_type == old.content_type
                                && o.text == old.text
                                && match old.target.kind {
                                    // Path edits keep the paint order, and a resized stroke's box
                                    // lands only near the one asked for (its width scales too).
                                    ObjectKind::Path => o.target == old.target,
                                    _ if placed => o.rect.iter().zip(old.rect).all(|(a, b)| (a - b).abs() < 0.01),
                                    // Aligns and arranges keep sizes, and images and added items
                                    // are listed where expected; paragraphs may renumber.
                                    kind => {
                                        size(o.rect).iter().zip(size(old.rect)).all(|(a, b)| (a - b).abs() < 0.01)
                                            && (kind == ObjectKind::Text || o.target == old.target)
                                    }
                                }
                        })
                        .collect();
                    // Twins (the same text, size or box) are told apart by an unchanged reference.
                    let object = match matches.as_slice() {
                        [only] => *only,
                        twins => *twins.iter().find(|o| o.target == old.target)?,
                    };
                    if out.iter().any(|o: &EditableObject| o.target == object.target) {
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
        // Escape closes an open menu first; the selection it acts on stays.
        if self.page.is_some() && !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.clear();
        }
    }

    /// The move of the whole selection by `offset` (user space). The selection follows it.
    fn move_by(&mut self, page: usize, offset: [f64; 2]) -> Edit {
        let mut expected = self.selected.clone();
        for object in &mut expected {
            object.rect = [object.rect[0] + offset[0], object.rect[1] + offset[1], object.rect[2] + offset[0], object.rect[3] + offset[1]];
        }
        self.pending = Some((expected, true));
        Edit::MoveObjects { page, objects: self.selected.iter().map(|o| o.target).collect(), offset }
    }

    /// Whether [`Self::delete`] acts: vector paths can't go in one step with text or added items.
    fn deletable(&self) -> bool {
        let any = |kinds: &[ObjectKind]| self.selected.iter().any(|o| kinds.contains(&o.target.kind));
        !(any(&[ObjectKind::Path]) && any(&[ObjectKind::Text, ObjectKind::Added]))
    }

    /// The deletion of the selection as one undo step. Vector paths and images go in one edit,
    /// one splice of the page's content; a selection with a paragraph or an added item (and so
    /// no path) goes as [`delete_edit`] says. The selection stays until the edit applies:
    /// `prepare` clears it when the document changes, and a refused delete keeps it.
    fn delete(&mut self, page: usize) -> Option<Edit> {
        if !self.deletable() {
            self.notice = Some(tl!("Text and added items can't be deleted with this selection.").into());
            return None;
        }
        let targets: Vec<ObjectTarget> = self.selected.iter().map(|o| o.target).collect();
        if !targets.is_empty() && targets.iter().all(|t| matches!(t.kind, ObjectKind::Path | ObjectKind::Image)) {
            return Some(Edit::DeleteObjects { page, objects: targets });
        }
        delete_edit(page, targets)
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

/// An object painting at least 90% of the crop box is the page's background.
fn covers_page(info: &DocInfo, page: usize, object: &EditableObject) -> bool {
    let Some(crop) = info.pages.get(page).map(|p| p.crop.map(f64::from)) else {
        return false;
    };
    let r = object.rect;
    let area = |x0: f64, y0: f64, x1: f64, y1: f64| (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let page_area = area(crop[0], crop[1], crop[2], crop[3]);
    page_area > 0.0 && area(r[0].max(crop[0]), r[1].max(crop[1]), r[2].min(crop[2]), r[3].min(crop[3])) >= 0.9 * page_area
}

/// The topmost listed vector path painted under `p` (not just its box, which a diagonal line
/// mostly leaves empty), within 2 screen pixels. A page background only counts once it is
/// `selected`, so a modifier click can take it out again.
fn path_hit<'a>(
    xf: &PageXform,
    page: usize,
    doc: &Document,
    objects: &'a [EditableObject],
    selected: &[EditableObject],
    paths: &mut Option<(usize, Vec<PagePath>)>,
    p: Pos2,
) -> Option<&'a EditableObject> {
    let candidate =
        |o: &EditableObject| o.target.kind == ObjectKind::Path && (!covers_page(&doc.info, page, o) || selected.iter().any(|s| s.target == o.target));
    // The precise test parses the whole page: skip it unless the press is on a path's box.
    if !objects.iter().any(|o| candidate(o) && screen(xf, &doc.info, page, o).expand(2.0).contains(p)) {
        return None;
    }
    let (a, b) = (user(xf, &doc.info, page, p)?, user(xf, &doc.info, page, p + egui::vec2(2.0, 0.0))?);
    let index = page_paths(paths, doc, page).iter().rposition(|path| path.contains(a, (b[0] - a[0]).hypot(b[1] - a[1])))?;
    objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Path, index }).filter(|o| candidate(o))
}

/// `page`'s vector paths from the selection's cache, parsed again only for another page.
fn page_paths<'a>(cache: &'a mut Option<(usize, Vec<PagePath>)>, doc: &Document, page: usize) -> &'a [PagePath] {
    if cache.as_ref().is_none_or(|(cached, _)| *cached != page) {
        *cache = Some((page, doc.page_paths(page).unwrap_or_default()));
    }
    cache.as_ref().map_or(&[], |(_, paths)| paths)
}

/// A screen point in user space.
fn user(xf: &PageXform, info: &DocInfo, page: usize, q: Pos2) -> Option<[f64; 2]> {
    let (x, y) = xf.screen_to_view(q);
    Some(info.pages.get(page)?.view_to_user(x, y).map(f64::from))
}

/// The box (screen) a drag of resize handle `corner` by `delta` gives `b`: the opposite corner
/// stays. None once the box would be empty or turned inside out.
fn resized(b: Rect, corner: usize, delta: egui::Vec2) -> Option<Rect> {
    let corners = crate::edit_text_ui::corners(b);
    let (grabbed, fixed) = (*corners.get(corner)?, *corners.get(3_usize.checked_sub(corner)?)?);
    let moved = grabbed + handle_delta(b, delta);
    let same_side = |extent: f32, m: f32, g: f32, f: f32| extent < HANDLE || if g < f { m < f } else { m > f };
    (same_side(b.width(), moved.x, grabbed.x, fixed.x) && same_side(b.height(), moved.y, grabbed.y, fixed.y))
        .then(|| Rect::from_two_pos(fixed, moved))
}

/// A handle drag by `delta`, less its motion along an axis on which `b` (screen) is thinner
/// than a handle: lengthening a hairline rule with an unsteady hand neither flips nor thickens it.
fn handle_delta(b: Rect, delta: egui::Vec2) -> egui::Vec2 {
    egui::vec2(if b.width() < HANDLE { 0.0 } else { delta.x }, if b.height() < HANDLE { 0.0 } else { delta.y })
}

/// The user-space box of `object` once handle `corner` is dragged from `start` to `now`. The
/// fixed sides come from the object and the drag goes through the linear map, so a large
/// crop-box origin can neither shift them nor round a small drag away.
fn resized_user(xf: &PageXform, doc: &Document, page: usize, object: &EditableObject, corner: usize, start: Pos2, now: Pos2) -> Option<[f64; 4]> {
    let b = screen(xf, &doc.info, page, object);
    resized(b, corner, now - start)?;
    let corners = crate::edit_text_ui::corners(b);
    let along = |from: Pos2, to: Pos2| {
        let (a, b) = (xf.screen_to_view(from), xf.screen_to_view(to));
        doc.object_move_offset(page, [f64::from(b.0 - a.0), f64::from(b.1 - a.1)]).ok()
    };
    // Which sides the grabbed corner is on, whatever the page and view rotation.
    let span = along(*corners.get(3_usize.checked_sub(corner)?)?, *corners.get(corner)?)?;
    let [dx, dy] = along(start, start + handle_delta(b, now - start))?;
    let mut rect = object.rect;
    rect[if span[0] > 0.0 { 2 } else { 0 }] += dx;
    rect[if span[1] > 0.0 { 3 } else { 1 }] += dy;
    (rect[2] > rect[0] && rect[3] > rect[1] && rect != object.rect).then_some(rect)
}

/// The resize handle of the selected path's box `b` (screen) under `p`. Inside a small box
/// they reach less far, so a press in its middle still moves it.
fn handle_at(b: Rect, p: Pos2) -> Option<usize> {
    let reach = if b.contains(p) { HANDLE.min(b.width().min(b.height()) / 3.0) } else { HANDLE };
    crate::edit_text_ui::corners(b).iter().position(|c| c.distance(p) < reach)
}

fn corner_cursor(corner: usize) -> egui::CursorIcon {
    if matches!(corner, 0 | 3) { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw }
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
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    ptr: &PagePointer,
    page: usize,
    doc: &Document,
    view: &mut DocView,
) -> bool {
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
    let result = input(&mut selection, ui, resp, xf, ptr, page, doc, view);
    view.objects = selection;
    result
}

#[allow(clippy::too_many_arguments)]
fn input(
    s: &mut ObjectSelection,
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    ptr: &PagePointer,
    page: usize,
    doc: &Document,
    view: &mut DocView,
) -> bool {
    let pointer = ptr.pos;
    let (pressed, down) = (ptr.pressed().is_some(), ptr.primary_down);
    let info = &doc.info;
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
                    // A vector page background joins only an additive marquee over most of the page.
                    let whole_page = gesture.additive && area.width() * area.height() >= 0.9 * xf.rect.width() * xf.rect.height();
                    let candidates: Vec<_> = objects
                        .into_iter()
                        .filter(|o| {
                            area.intersects(screen(xf, info, page, o))
                                && (whole_page || o.target.kind != ObjectKind::Path || !covers_page(info, page, o))
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
            } else if let Some(corner) = gesture.corner {
                if gesture.start.distance(gesture.now) > 3.0
                    && view.pending_edit.is_none()
                    && let Some(object) = s.single_path().cloned()
                    && let Some(rect) = resized_user(xf, doc, page, &object, corner, gesture.start, gesture.now)
                {
                    view.pending_edit = Some(Edit::TransformPath { page, index: object.target.index, rect });
                    s.pending = Some((vec![object], true));
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
        ui.ctx().set_cursor_icon(match gesture.corner {
            Some(corner) => corner_cursor(corner),
            None if gesture.marquee => egui::CursorIcon::Crosshair,
            None => egui::CursorIcon::Move,
        });
        return true;
    }
    // A press on a menu or popup over the page is not on the page: it must not clear the
    // selection the menu acts on.
    let on_page =
        |p: &Pos2| xf.rect.contains(*p) && ui.clip_rect().contains(*p) && resp.rect.contains(*p) && ui.ctx().layer_id_at(*p) == Some(ui.layer_id());
    let Some(p) = pointer.filter(on_page) else {
        return s.page == Some(page) && !s.selected.is_empty();
    };
    if resp.secondary_clicked() {
        s.menu = s.page == Some(page);
    }
    // A handle sits on the box's edge, half outside it: it is looked for before the box.
    let handle = s.single_path().filter(|_| s.page == Some(page)).and_then(|o| handle_at(screen(xf, info, page, o), p));
    if !pressed {
        if s.page == Some(page) && !s.selected.is_empty() {
            let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
            if let Some(corner) = handle {
                ui.ctx().set_cursor_icon(corner_cursor(corner));
            } else if union.contains(p) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
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
    // A page-sized background image must not hide editable labels, added notes or the vector
    // paths drawn over it. Within each kind, prefer the last object as the ordinary single-item
    // tools do. A press on a path's painted shape takes the path (a plain one selects it alone);
    // a page-covering path is left to the marquee. Any other image still wins over a path
    // without comparing paint order; the inventory doesn't carry image op indices yet.
    let boxed = |kind: ObjectKind| objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p));
    let image = boxed(ObjectKind::Image);
    let selected: &[EditableObject] = if s.page == Some(page) { &s.selected } else { &[] };
    let hit = boxed(ObjectKind::Added)
        .or_else(|| boxed(ObjectKind::Text))
        .or_else(|| image.filter(|o| !covers_page(info, page, o)))
        .or_else(|| path_hit(xf, page, doc, &objects, selected, &mut s.paths, p))
        .or(image);
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
            s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: true, corner: None });
        }
        leave_single(view);
        view.current = page;
        return true;
    }
    if s.page == Some(page) && !s.selected.is_empty() {
        if handle.is_some() {
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: handle });
            return true;
        }
        let union = s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o)));
        if union.contains(p) && !resp.double_clicked() {
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: None });
            return true;
        }
    }
    // The frame of the paragraph open for typing takes hold of the box, as in Acrobat: a click
    // selects it (for the arrow keys), a drag moves it. Its text keeps the caret. A vector path
    // is taken the same way by its painted shape.
    let frame = view
        .line_editor
        .as_ref()
        .filter(|e| e.page == page && !e.has_changes())
        .and_then(|e| objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: e.block }))
        .filter(|o| on_frame(screen(xf, info, page, o), p));
    // A closed paragraph's width handle reaches past its box, and a press there rewraps the
    // paragraph (as in `edit_text_ui`) instead of starting an area selection (#2276) or taking
    // a path under the handle, as text wins over paths.
    let on_edge = view.line_editor.as_ref().is_none_or(|e| e.page != page)
        && crate::edit_text_ui::grabs_edge(xf, info, page, objects.iter().filter(|o| o.target.kind == ObjectKind::Text).map(|o| o.rect), p);
    if let Some(object) = frame.or(hit.filter(|o| o.target.kind == ObjectKind::Path && !on_edge)) {
        s.clear();
        s.page = Some(page);
        s.selected.push(object.clone());
        s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: None });
        leave_single(view);
        view.current = page;
        return true;
    }
    s.clear();
    if hit.is_some() || on_edge {
        return false;
    }
    leave_single(view);
    s.page = Some(page);
    s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: false, corner: None });
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
/// as in Acrobat. They are taken before the view turns the page with them. Delete removes the
/// selected objects.
pub(crate) fn nudge_keys(ctx: &egui::Context, view: &mut DocView, doc: &Document) {
    // The page grid has keys of its own, and the selection is out of sight there.
    if ctx.egui_wants_keyboard_input() || view.pending_edit.is_some() || view.organize {
        return;
    }
    let image = view.image_selection.as_ref().filter(|s| !s.dragging()).map(|s| (s.page, s.index));
    // An undo earlier in this frame leaves the selection naming objects that may have moved in
    // the inventory; the view finds them again before it draws.
    let objects = view
        .objects
        .page
        .filter(|_| !view.objects.selected.is_empty() && view.objects.gesture.is_none() && view.objects.generation == Some(doc.edit_generation()));
    use egui::{Key, Modifiers};
    // A group selection never coexists with a single selected image or added item
    // (`leave_single`); taking the key also keeps their own Delete from firing. With the
    // pointer over a panel, Delete is the panel's (the Pages panel deletes pages with it).
    let over_document = ctx.input(|i| i.pointer.hover_pos()).is_none_or(|p| view.viewport_rect().contains(p));
    if let Some(page) = objects.filter(|_| over_document && view.line_editor.is_none() && view.content.draft.is_none())
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace))
    {
        view.pending_edit = view.objects.delete(page);
        return;
    }
    let Some(page) = objects.or(image.map(|(page, _)| page)) else { return };
    let Some(xf) = view.page_xform(page) else { return };
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

/// The page menu over a group selection: Arrange, Align, Distribute and Delete. False, with
/// nothing drawn, unless the menu was opened on the selection's page.
pub(crate) fn context_menu(ui: &mut egui::Ui, doc: &Document, view: &mut DocView, can_modify: bool) -> bool {
    let s = &mut view.objects;
    let Some(page) = s.page.filter(|_| s.menu && !s.selected.is_empty()) else { return false };
    let mut edit = None;
    let single = match s.selected.as_slice() {
        [o] if matches!(o.target.kind, ObjectKind::Path | ObjectKind::Image) => Some(o.clone()),
        _ => None,
    };
    ui.add_enabled_ui(can_modify && single.is_some(), |ui| {
        ui.menu_button(tl!("Arrange"), |ui| {
            for (order, label) in [
                (ZOrder::Front, "Bring to front"),
                (ZOrder::Forward, "Bring forward"),
                (ZOrder::Backward, "Send backward"),
                (ZOrder::Back, "Send to back"),
            ] {
                if ui.button(tl!(label)).clicked()
                    && let Some(object) = &single
                {
                    // Where it is listed afterwards comes from a dry run on a copy of the page.
                    match doc.arranged_target(page, object.target, order) {
                        Ok(target) => {
                            s.pending = Some((vec![EditableObject { target, ..object.clone() }], false));
                            edit = Some(Edit::ArrangeObjects { page, object: object.target, order });
                        }
                        Err(e) => s.notice = Some(crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", tl!(label)), ("e", e.as_str())])),
                    }
                    ui.close();
                }
            }
        });
    });
    let aligns = [
        (ObjectAlign::Left, "Left"),
        (ObjectAlign::CentreHorizontal, "Center horizontally"),
        (ObjectAlign::Right, "Right"),
        (ObjectAlign::Top, "Top"),
        (ObjectAlign::CentreVertical, "Center vertically"),
        (ObjectAlign::Bottom, "Bottom"),
    ];
    let distributes = [(ObjectAlign::DistributeHorizontal, "Horizontally"), (ObjectAlign::DistributeVertical, "Vertically")];
    for (menu, least, items) in [("Align", 2, &aligns[..]), ("Distribute", 3, &distributes[..])] {
        ui.add_enabled_ui(can_modify && s.selected.len() >= least, |ui| {
            ui.menu_button(tl!(menu), |ui| {
                for &(align, label) in items {
                    if ui.button(tl!(label)).clicked() {
                        s.pending = Some((s.selected.clone(), false));
                        edit = Some(Edit::AlignObjects { page, objects: s.selected.iter().map(|o| o.target).collect(), align });
                        ui.close();
                    }
                }
            });
        });
    }
    ui.separator();
    if ui.add_enabled(can_modify && s.deletable(), egui::Button::new(tl!("Delete"))).clicked() {
        edit = s.delete(page);
        ui.close();
    }
    if edit.is_some() {
        view.pending_edit = edit;
    }
    true
}

/// Fill colour and Stroke colour for the selected vector paths, each a swatch of the first such
/// path's colour that opens the colour grid. A pick recolours the selected paths that paint that
/// part, in one step, and the selection stays. False when the selection holds no path.
pub(crate) fn colour_section(app: &mut crate::PdfCraftApp, ui: &mut egui::Ui) -> bool {
    let Some((i, id)) = app.active_ids().filter(|_| app.quick_tool == crate::QuickTool::EditText) else { return false };
    let mut edit = None;
    {
        let (Some(doc), Some(view)) = (app.session.get(id), app.views.get_mut(i)) else { return false };
        let s = &mut view.objects;
        let Some(page) = s.page.filter(|_| s.selected.iter().any(|o| o.target.kind == ObjectKind::Path)) else { return false };
        // Until the view finds its objects again after an edit, their references may name others.
        let fresh = s.generation == Some(doc.edit_generation()) && doc.allows_modification();
        let paths = page_paths(&mut s.paths, doc, page);
        for (label, fill) in [(tl!("Fill colour"), true), (tl!("Stroke colour"), false)] {
            let (objects, colours): (Vec<ObjectTarget>, Vec<Option<[f64; 3]>>) = s
                .selected
                .iter()
                .filter(|o| o.target.kind == ObjectKind::Path)
                .filter_map(|o| paths.get(o.target.index).map(|p| (o.target, p)))
                .filter(|(_, p)| if fill { p.paint.fill.is_some() } else { p.paint.stroke })
                .map(|(target, p)| (target, if fill { p.fill_rgb } else { p.stroke_rgb }))
                .unzip();
            // A colour without an RGB look (a pattern, a separation) shows as grey.
            let current = colours.first().copied().flatten();
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 7.0, crate::comments::color32(current.unwrap_or([0.5; 3])));
                ui.painter().circle_stroke(r.center(), 7.0, Stroke::new(1.0, Color32::from_black_alpha(40)));
                let enabled = fresh && !objects.is_empty();
                let button = ui.add_enabled(enabled, egui::Button::new(label));
                // Open until a pick: the full picker commits once, on Apply.
                if enabled {
                    // Escape closes the grid only: the page, drawn after this panel, would also
                    // clear the selection with it.
                    let popup = egui::Popup::default_response_id(&button);
                    if egui::Popup::is_id_open(ui.ctx(), popup) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                        egui::Popup::close_id(ui.ctx(), popup);
                    }
                    egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        if let Some(c) = crate::comments::swatch_grid(ui, current) {
                            edit =
                                Some(Edit::RecolourObjects { page, objects: objects.clone(), fill: fill.then_some(c), stroke: (!fill).then_some(c) });
                            ui.close();
                        }
                    });
                }
            });
        }
        if edit.is_some() {
            s.pending = Some((s.selected.clone(), true));
        }
    }
    // Applied now: the panel draws before the page, whose `prepare` would drop the remap of an
    // edit still queued.
    if let Some(edit) = edit {
        app.apply_edit(edit);
    }
    ui.add_space(6.0);
    ui.separator();
    true
}

/// The edit that deletes `targets` (indexes into one inventory of `page`) as one undo step.
/// Each deletion renumbers only what follows it in its own inventory, so paragraphs and images
/// go from the last to the first, and added content, which the source inventories leave out,
/// after them.
fn delete_edit(page: usize, mut targets: Vec<ObjectTarget>) -> Option<Edit> {
    targets.sort_by_key(|t| (t.kind == ObjectKind::Added, std::cmp::Reverse(t.index)));
    targets.dedup();
    let mut edits: Vec<Edit> = targets
        .into_iter()
        .map(|t| match t.kind {
            ObjectKind::Text => Edit::EditTextBlock { page, block: t.index, text: String::new(), style: pdfcraft_engine::BlockStyle::default() },
            ObjectKind::Image => Edit::EditPageImage { page, index: t.index, change: pdfcraft_engine::ImageEdit::Delete },
            ObjectKind::Added => Edit::DeleteContent { page, index: t.index },
            // `ObjectSelection::delete` sends paths as one `DeleteObjects`; this keeps the match total.
            ObjectKind::Path => Edit::DeleteObjects { page, objects: vec![t] },
        })
        .collect();
    if edits.len() > 1 {
        // "Delete content" is an edit label already, so the history reads the same in every language.
        Some(Edit::Batch { label: "Delete content".into(), edits })
    } else {
        edits.pop()
    }
}

pub(crate) fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, selection: &ObjectSelection) {
    if selection.page != Some(page) {
        return;
    }
    let drag = selection.gesture.filter(|g| !g.marquee);
    let delta = drag.map_or(egui::Vec2::ZERO, |g| g.now - g.start);
    let mut union = Rect::NOTHING;
    for object in &selection.selected {
        let b = screen(xf, info, page, object);
        // A handle dragged inside out leaves the box as it was until it comes back.
        let rect = match drag.and_then(|g| g.corner) {
            Some(corner) => resized(b, corner, delta).unwrap_or(b),
            None => b.translate(delta),
        };
        painter.rect_filled(rect, 0.0, BLUE.gamma_multiply(0.05));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5, BLUE), egui::StrokeKind::Outside);
        union = union.union(rect);
    }
    if selection.single_path().is_some() {
        crate::edit_text_ui::paint_handles(painter, union);
    }
    if selection.selected.len() > 1 {
        painter.rect_stroke(union.expand(4.0), 0.0, Stroke::new(1.0, BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    if let Some(g) = selection.gesture.filter(|g| g.marquee) {
        let area = Rect::from_two_pos(g.start, g.now).intersect(xf.rect);
        painter.rect_filled(area, 0.0, BLUE.gamma_multiply(0.08));
        painter.rect_stroke(area, 0.0, Stroke::new(1.0, BLUE), egui::StrokeKind::Outside);
    }
}

/// Keep a move's or an align's refusal (`detail`) actionable and translated; headless callers
/// retain the detailed error. None for other edits, and for an align's own refusals ("already
/// aligned"), which say what to do as they are.
pub(crate) fn move_error(edit: &Edit, detail: &str) -> Option<String> {
    let align = match edit {
        Edit::MoveObjects { .. } => false,
        Edit::AlignObjects { .. } => true,
        _ => return None,
    };
    Some(match detail {
        "vertical text cannot be moved in a group" => tl!("Vertical text cannot be moved with this selection.").into(),
        "text used as a clipping path cannot be moved in a group" => tl!("Text used as a clipping path cannot be moved with this selection.").into(),
        _ if align => return None,
        _ => tl!("These objects could not be moved. Select them again and try a smaller move.").into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(kind: ObjectKind, index: usize) -> ObjectTarget {
        ObjectTarget { kind, index }
    }

    /// Deleting renumbers what follows in the same inventory, so the batch takes each inventory
    /// from its last item to its first, and added content (outside the source inventories) last.
    #[test]
    fn several_objects_delete_last_first_in_one_step() {
        let targets = vec![t(ObjectKind::Added, 0), t(ObjectKind::Text, 1), t(ObjectKind::Image, 0), t(ObjectKind::Text, 3), t(ObjectKind::Added, 2)];
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
                Edit::DeleteContent { index, .. } => ('a', *index),
                other => panic!("unexpected {}", other.label().wording),
            })
            .collect();
        assert_eq!(order, [('t', 3), ('t', 1), ('i', 0), ('a', 2), ('a', 0)]);
    }

    #[test]
    fn one_object_deletes_with_its_own_edit_and_none_with_nothing() {
        assert!(matches!(delete_edit(2, vec![t(ObjectKind::Image, 4)]), Some(Edit::EditPageImage { page: 2, index: 4, .. })));
        assert!(delete_edit(0, Vec::new()).is_none());
    }

    #[test]
    fn an_align_refused_as_a_group_move_says_so_as_a_move_does() {
        use pdfcraft_engine::{Edit, ObjectAlign};
        let align = Edit::AlignObjects { page: 0, objects: Vec::new(), align: ObjectAlign::Left };
        let shift = Edit::MoveObjects { page: 0, objects: Vec::new(), offset: [1.0, 0.0] };
        for edit in [&align, &shift] {
            assert_eq!(
                super::move_error(edit, "vertical text cannot be moved in a group").as_deref(),
                Some("Vertical text cannot be moved with this selection.")
            );
        }
        assert_eq!(super::move_error(&align, "the objects are already aligned"), None);
        assert_eq!(
            super::move_error(&shift, "the objects are already aligned").as_deref(),
            Some("These objects could not be moved. Select them again and try a smaller move.")
        );
        assert_eq!(super::move_error(&Edit::DeleteObjects { page: 0, objects: Vec::new() }, "vertical text cannot be moved in a group"), None);
    }
}
