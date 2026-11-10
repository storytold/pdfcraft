//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Stroke};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectAlign, ObjectKind, ObjectTarget, PagePath, ZOrder};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_keys::PageKeys;
use crate::page_router::{Hit, ObjectsPart, Owner, PagePress};

const BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
/// The size and reach of a resize handle (screen pixels), as for images.
const HANDLE: f32 = 8.0;

#[derive(Default)]
pub(crate) struct ObjectSelection {
    generation: Option<u64>,
    cache: HashMap<usize, Result<Vec<EditableObject>, String>>,
    /// Pages' vector paths, for hit tests: each frame would parse the page again.
    paths: HashMap<usize, Vec<PagePath>>,
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
    input_blocked: bool,
    /// A paragraph (page, block) to select once the document is at hand: Esc in the inline
    /// editor asks for it, and the next `prepare` selects it.
    select_paragraph: Option<(usize, usize)>,
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
    /// Clear any object-selection gesture.
    pub(crate) fn cancel_pointer_gesture(&mut self) {
        self.gesture = None;
    }

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

    /// The page and references of the objects selected together, if any: what Copy and Cut take.
    pub(crate) fn selection(&self) -> Option<(usize, Vec<ObjectTarget>)> {
        self.page.filter(|_| !self.selected.is_empty()).map(|page| (page, self.selected.iter().map(|o| o.target).collect()))
    }

    /// Select what a paste added to `page`, as Acrobat selects pasted objects: the objects of
    /// `after` (the page's inventory with the paste) that `before` (without it) didn't have. They
    /// are taken at the next `prepare`, which sees the new document generation.
    pub(crate) fn select_pasted(&mut self, page: usize, before: &[EditableObject], after: Vec<EditableObject>) {
        let same = |a: &EditableObject, b: &EditableObject| {
            a.target.kind == b.target.kind
                && a.content_type == b.content_type
                && a.text == b.text
                && a.rect.iter().zip(b.rect).all(|(x, y)| (x - y).abs() < 0.01)
        };
        let mut old: Vec<&EditableObject> = before.iter().collect();
        let added: Vec<_> = after
            .into_iter()
            .filter(|o| match old.iter().position(|b| same(b, o)) {
                Some(i) => {
                    old.swap_remove(i);
                    false
                }
                None => true,
            })
            .collect();
        self.clear();
        if !added.is_empty() && added.len() <= MAX_MOVE_OBJECTS {
            self.page = Some(page);
            self.pending = Some((added, true));
        }
    }

    /// Edit text's input is blocked: a dialog, the palette, another window or another tab has it.
    pub(crate) fn input_blocked(&self) -> bool {
        self.input_blocked
    }

    /// The objects selected together are on `page`, or a gesture of theirs runs there: the hover
    /// outlines of what lies under them wait.
    pub(crate) fn holds(&self, page: usize) -> bool {
        self.page == Some(page) && (!self.selected.is_empty() || self.gesture.is_some())
    }

    /// A preview belongs to the active, focused document. Returning from another tab or a
    /// modal must not turn a release that happened elsewhere into a document edit.
    pub fn set_input_blocked(&mut self, blocked: bool) {
        self.input_blocked = blocked;
        if blocked {
            self.gesture = None;
        }
    }

    fn clear(&mut self) {
        self.page = None;
        self.selected.clear();
        self.gesture = None;
        self.pending = None;
        self.select_paragraph = None;
    }

    /// Select paragraph `block` of `page` as an object at the next `prepare`, as Acrobat does
    /// when Esc leaves a text box: the box is then selected, so Delete removes it, the arrow keys
    /// move it and its frame drags it, without aiming for the frame.
    pub(crate) fn select_paragraph_next(&mut self, page: usize, block: usize) {
        self.select_paragraph = Some((page, block));
    }

    pub fn prepare(&mut self, doc: &Document, enabled: bool, ctx: &egui::Context, keys: PageKeys) {
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
            self.paths.clear();
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
        if self.page.is_some() && !egui::Popup::is_any_open(ctx) && keys.take(ctx, egui::Key::Escape) {
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
        self.pending = Some((expected, true));
        Edit::MoveObjects { page, objects: self.selected.iter().map(|o| o.target).collect(), offset }
    }

    /// `page`'s objects, listed once per document generation.
    fn objects(&mut self, doc: &Document, page: usize) -> &Result<Vec<EditableObject>, String> {
        if self.cache.len() >= 8 && !self.cache.contains_key(&page) {
            self.cache.clear();
        }
        self.cache.entry(page).or_insert_with(|| doc.editable_objects(page))
    }

    /// The objects of `page` that can be selected together, as `doc.editable_objects` lists them, or
    /// why they can't be.
    pub(crate) fn inventory(&mut self, doc: &Document, page: usize) -> Result<Vec<EditableObject>, String> {
        self.objects(doc, page).clone()
    }

    /// Lists what the hit tests need of `page` ([`path_hit`]): its objects and, when they
    /// include vector paths, their shapes.
    pub(crate) fn cache_page(&mut self, doc: &Document, page: usize) {
        if self.objects(doc, page).as_ref().is_ok_and(|o| o.iter().any(|o| o.target.kind == ObjectKind::Path)) {
            page_paths(&mut self.paths, doc, page);
        }
    }
}

fn screen(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject) -> Rect {
    xf.user_rect(info, page, object.rect.map(|v| v as f32))
}

/// An object painting at least 90% of the crop box (`r`, user space) is the page's background.
pub(crate) fn covers_page(info: &DocInfo, page: usize, r: [f64; 4]) -> bool {
    let Some(crop) = info.pages.get(page).map(|p| p.crop.map(f64::from)) else {
        return false;
    };
    let area = |x0: f64, y0: f64, x1: f64, y1: f64| (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let page_area = area(crop[0], crop[1], crop[2], crop[3]);
    page_area > 0.0 && area(r[0].max(crop[0]), r[1].max(crop[1]), r[2].min(crop[2]), r[3].min(crop[3])) >= 0.9 * page_area
}

/// The topmost listed vector path painted under `p` (not just its box, which a diagonal line
/// mostly leaves empty), within 2 screen pixels, by its index among the page's paths. A page
/// background only counts once it is selected, so a modifier click can take it out again. For
/// the hit tests, from what [`ObjectSelection::cache_page`] listed.
fn path_under(s: &ObjectSelection, xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> Option<usize> {
    let objects = s.cache.get(&page)?.as_ref().ok()?;
    let selected: &[EditableObject] = if s.page == Some(page) { &s.selected } else { &[] };
    let candidate =
        |o: &EditableObject| o.target.kind == ObjectKind::Path && (!covers_page(info, page, o.rect) || selected.iter().any(|s| s.target == o.target));
    // The precise test walks the page's paths: skip it unless `p` is on a path's box.
    if !objects.iter().any(|o| candidate(o) && screen(xf, info, page, o).expand(2.0).contains(p)) {
        return None;
    }
    let (a, b) = (user(xf, info, page, p)?, user(xf, info, page, p + egui::vec2(2.0, 0.0))?);
    let index = s.paths.get(&page)?.iter().rposition(|path| path.contains(a, (b[0] - a[0]).hypot(b[1] - a[1])))?;
    objects.iter().any(|o| o.target == ObjectTarget { kind: ObjectKind::Path, index } && candidate(o)).then_some(index)
}

/// A vector path's painted shape under `p`: a click selects it alone, a drag moves it. On the
/// text layer, where the page view puts it over a page-sized background picture and under the
/// other pictures and the paragraphs (text wins over paths, #2276).
pub(crate) fn path_hit(view: &DocView, xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> Option<Hit> {
    let index = path_under(&view.objects, xf, info, page, p)?;
    Some(Hit::new(Owner::Objects { part: ObjectsPart::Path(index) }, Layer::Text, egui::CursorIcon::PointingHand))
}

/// `page`'s vector paths from the selection's cache.
fn page_paths<'a>(cache: &'a mut HashMap<usize, Vec<PagePath>>, doc: &Document, page: usize) -> &'a [PagePath] {
    if cache.len() >= 8 && !cache.contains_key(&page) {
        cache.clear();
    }
    cache.entry(page).or_insert_with(|| doc.page_paths(page).unwrap_or_default())
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

/// A menu is open (the page's, or another popup): the objects selected together leave the pointer
/// alone, and a press with Shift or ⌘ is not theirs.
pub(crate) fn menu_open(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.context_menu_opened() || egui::Popup::is_any_open(ui.ctx())
}

/// Whether `modifiers` make a press add to the objects selected together, or take from them.
pub(crate) fn adds(modifiers: egui::Modifiers) -> bool {
    modifiers.shift || modifiers.ctrl || modifiers.command
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

/// ⌘A in Edit a PDF: every object on the current page selected together, as Acrobat selects
/// them (paragraphs, pictures and added content), instead of the page's text. A page whose
/// objects can't be selected together, or has too many, says so and keeps what was selected.
pub(crate) fn select_all(view: &mut DocView, doc: &Document) {
    let page = view.current;
    match view.objects.inventory(doc, page) {
        Ok(objects) if objects.len() > MAX_MOVE_OBJECTS => view.objects.notice = Some(tl!("Select at most 1000 objects at a time.").into()),
        Ok(objects) => {
            view.objects.clear();
            if !objects.is_empty() {
                leave_single(view);
                view.objects.page = Some(page);
                view.objects.selected = objects;
            }
        }
        Err(_) => view.objects.notice = Some(tl!("This page contains objects that cannot be selected together.").into()),
    }
}

/// What of the objects lies under `p` on `page`: a resize handle of the one vector path selected
/// (a drag resizes it), the objects selected together (a drag moves them), or the frame around the
/// paragraph open for typing (`open`: its box on screen), which takes hold of the box as in
/// Acrobat: a click selects it, a drag moves it, its text keeps the caret.
pub(crate) fn hit(view: &DocView, xf: &PageXform, info: &DocInfo, page: usize, open: Option<Rect>, p: Pos2) -> Option<Hit> {
    // Past the page's edge is no object's: the next page lies there.
    if !xf.rect.contains(p) {
        return None;
    }
    let s = &view.objects;
    // A handle sits on the box's edge, half outside it: it is looked for before the box.
    if let Some(corner) = s.single_path().filter(|_| s.page == Some(page)).and_then(|o| handle_at(screen(xf, info, page, o), p)) {
        return Some(Hit::new(Owner::Objects { part: ObjectsPart::Corner(corner) }, Layer::Handle, corner_cursor(corner)));
    }
    if s.page == Some(page) && !s.selected.is_empty() && s.selected.iter().fold(Rect::NOTHING, |r, o| r.union(screen(xf, info, page, o))).contains(p)
    {
        return Some(Hit::new(Owner::Objects { part: ObjectsPart::Selection }, Layer::Placed, egui::CursorIcon::Move));
    }
    open.filter(|r| on_frame(r.shrink(2.0), p)).map(|_| Hit::new(Owner::Objects { part: ObjectsPart::Frame }, Layer::Handle, egui::CursorIcon::Move))
}

/// The object under `p` among the page's `objects`: added content over text over pictures and
/// vector paths, so a page-sized background picture does not hide editable labels, added notes or
/// the paths drawn over it, and within each kind the last, as the single-item tools take it. The
/// picture is `picture`, the one a plain press finds by its visible shape, and the path the one
/// painted under `p` ([`path_under`]): where a clip hides part of a picture's box, a path there
/// is taken. Any other picture still wins over a path; the inventory doesn't carry their paint
/// order.
fn object_at(
    s: &ObjectSelection,
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    objects: &[EditableObject],
    picture: Option<usize>,
    p: Pos2,
) -> Option<ObjectTarget> {
    let boxed = |kind: ObjectKind| objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p));
    let image = picture.and_then(|index| objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Image, index }));
    let path = path_under(s, xf, info, page, p).map(|index| ObjectTarget { kind: ObjectKind::Path, index });
    boxed(ObjectKind::Added)
        .or_else(|| boxed(ObjectKind::Text))
        .or_else(|| image.filter(|o| !covers_page(info, page, o.rect)))
        .map(|o| o.target)
        .or(path)
        .or(image.map(|o| o.target))
}

/// What a press with Shift or ⌘ at `p` takes on `page` in Edit text, whatever lies there: the
/// object under it among the page's `objects` ([`ObjectSelection::inventory`]), which it adds to
/// the objects selected together or takes out of them, or none, where it starts an area selection
/// that adds to them. Held while text being typed is open, and refused on a page whose objects
/// can't be selected together. `None` off the page and outside the pages' visible area (`clip`).
pub(crate) fn modified_hit(
    view: &DocView,
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    objects: &Result<Vec<EditableObject>, String>,
    picture: Option<usize>,
    clip: Rect,
    p: Pos2,
) -> Option<Hit> {
    if !xf.rect.contains(p) || !clip.contains(p) {
        return None;
    }
    let part = if view.content.draft.is_some() || view.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        ObjectsPart::Held
    } else {
        match objects {
            Ok(objects) => ObjectsPart::Toggle(object_at(&view.objects, xf, info, page, objects, picture, p)),
            Err(_) => ObjectsPart::Refused,
        }
    };
    Some(Hit::new(Owner::Objects { part }, Layer::Editor, egui::CursorIcon::Default))
}

/// The objects selected together, on one page, told what the press is to it (`press`). Returns
/// true while they own the interaction: while input is blocked, while a press with Shift or ⌘ is
/// held ([`modified_hit`]), and while objects on this page are selected or a gesture of theirs
/// runs. Ordinary presses on a paragraph, a picture or added content still reach their own tools.
/// A text draft is finished or discarded through its editor.
pub(crate) fn page_input(cx: PageToolContext<'_>, press: PagePress, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, ptr, page, .. } = cx;
    if view.objects.input_blocked {
        return true;
    }
    if menu_open(ui, resp) {
        return false;
    }
    match press {
        // Text being typed keeps the keyboard, and its editor stays open, until the button is up.
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Held }) => {
            view.objects.hold_editor = true;
            if ptr.pressed().is_some() {
                view.objects.notice = Some(tl!("Finish or discard the text being edited before selecting several objects.").into());
            }
            return true;
        }
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Refused }) => {
            if ptr.pressed().is_some() {
                view.objects.notice = Some(tl!("This page contains objects that cannot be selected together.").into());
            }
            return true;
        }
        _ => {}
    }
    if view.content.draft.is_some() || view.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        return false;
    }
    // Delete or Backspace removes the selected objects on this page as one edit, like
    // Edit::DeletePages removes pages (`delete_keys`). The key is consumed even when the pointer
    // is off this page, because canvas keys belong to the selection while it exists.
    if view.objects.page == Some(page) && delete_keys(ui.ctx(), cx.keys, view, cx.doc) {
        return true;
    }
    let mut selection = std::mem::take(&mut view.objects);
    let result = input(&mut selection, cx, press, view);
    view.objects = selection;
    result
}

fn input(s: &mut ObjectSelection, cx: PageToolContext<'_>, press: PagePress, view: &mut DocView) -> bool {
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
                                && (whole_page || o.target.kind != ObjectKind::Path || !covers_page(info, page, o.rect))
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
        view.cursor.claim(
            Layer::Drag,
            match gesture.corner {
                Some(corner) => corner_cursor(corner),
                None if gesture.marquee => egui::CursorIcon::Crosshair,
                None => egui::CursorIcon::Move,
            },
        );
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
    // Nor a press another page holds, on the row of pixels pages share with the gaps hidden: it may
    // have just selected something there.
    if !pressed || press == PagePress::Elsewhere {
        return s.page == Some(page) && !s.selected.is_empty();
    }
    let Ok(objects) = s.inventory(doc, page) else { return false };
    // What the press was on ([`hit`], [`modified_hit`]).
    match press {
        // With Shift or ⌘: the object it was on joins the selection or leaves it, and on none an
        // area selection adds to it.
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Toggle(target) }) => {
            if s.page != Some(page) {
                s.clear();
                s.page = Some(page);
                if let Some(single) = single(view, page)
                    && let Some(object) = objects.iter().find(|o| o.target == single)
                {
                    s.selected.push(object.clone());
                }
            }
            match target.and_then(|t| objects.iter().find(|o| o.target == t)) {
                Some(object) => {
                    if let Some(index) = s.selected.iter().position(|o| o.target == object.target) {
                        s.selected.remove(index);
                    } else if s.selected.len() < MAX_MOVE_OBJECTS {
                        s.selected.push(object.clone());
                    } else {
                        s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
                    }
                }
                None => s.gesture = Some(Gesture { start: p, now: p, marquee: true, additive: true, corner: None }),
            }
            leave_single(view);
            view.current = page;
            return true;
        }
        // The selection moves.
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Selection }) => {
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: None });
            return true;
        }
        // The frame of the paragraph open for typing selects its box (for the arrow keys) and
        // moves it.
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Frame }) => {
            if let Some(block) = view.line_editor.as_ref().filter(|e| e.page == page && !e.has_changes()).map(|e| e.block)
                && let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
            {
                s.clear();
                s.page = Some(page);
                s.selected.push(object.clone());
                s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: None });
                leave_single(view);
                view.current = page;
                return true;
            }
        }
        // A handle of the one vector path selected resizes it.
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Corner(corner) }) if s.single_path().is_some() => {
            s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: Some(corner) });
            return true;
        }
        // A vector path's painted shape ([`path_hit`]) selects it alone, and a drag moves it.
        PagePress::Mine(Owner::Objects { part: ObjectsPart::Path(index) }) => {
            if let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Path, index }) {
                s.clear();
                s.page = Some(page);
                s.selected.push(object.clone());
                s.gesture = Some(Gesture { start: p, now: p, marquee: false, additive: false, corner: None });
                leave_single(view);
                view.current = page;
                return true;
            }
        }
        _ => {}
    }
    s.clear();
    // A press on something (a paragraph, its edge reaching past its box #2276, a picture, added
    // content) is its own; only one nobody holds starts an area selection.
    if press != PagePress::Free {
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
    r.expand(2.0 + crate::edit_text_ui::FRAME_REACH).contains(p) && !r.expand(2.0).contains(p)
}

/// The arrow keys move the selected image or objects a point, Shift and an arrow ten points,
/// as in Acrobat. They are taken before the view turns the page with them.
pub(crate) fn nudge_keys(ctx: &egui::Context, keys: PageKeys, view: &mut DocView, doc: &Document) {
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
    use egui::Key;
    let Some(page) = objects.or(image.map(|(page, _)| page)) else { return };
    let Some(xf) = view.page_xform(page) else { return };
    let mut step = egui::Vec2::ZERO;
    // Only an arrow pressed alone or with Shift, judged by each press's own modifiers (the
    // frame's can differ when a key is let go in the same frame). Alt+← / Alt+→ go back and
    // forward through the view history (#3632) and Ctrl/⌘+← / → turn the page, both in
    // `canvas::shortcuts`.
    keys.take_presses(ctx, |key, modifiers| {
        if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
            return false;
        }
        let direction = match key {
            Key::ArrowLeft => egui::vec2(-1.0, 0.0),
            Key::ArrowRight => egui::vec2(1.0, 0.0),
            Key::ArrowUp => egui::vec2(0.0, -1.0),
            Key::ArrowDown => egui::vec2(0.0, 1.0),
            _ => return false,
        };
        step += direction * if modifiers.shift { 10.0 } else { 1.0 };
        true
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
    if ui.add_enabled(can_modify, egui::Button::new(tl!("Delete"))).clicked() {
        // As the Delete key does (`delete_keys`).
        edit = Edit::delete_objects(page, s.selected.iter().map(|o| o.target).collect());
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

/// Delete (or Backspace) removes the selected objects, as in Acrobat: a paragraph selected by its
/// frame, or everything an area selection took, goes at once instead of one typed character at a
/// time. True when it took the key. Not in the page grid, which has keys of its own; nor with the
/// pointer over a panel, whose Delete it is (the Pages panel deletes pages with it); nor while a
/// paragraph or a new text box is open for typing; nor in the frame of an undo, which leaves the
/// selection naming objects that may have moved in the inventory until the view finds them again.
pub(crate) fn delete_keys(ctx: &egui::Context, keys: PageKeys, view: &mut DocView, doc: &Document) -> bool {
    let s = &view.objects;
    if ctx.egui_wants_keyboard_input()
        || view.pending_edit.is_some()
        || view.organize
        || s.gesture.is_some()
        || s.generation.is_some_and(|g| g != doc.edit_generation())
        || view.line_editor.is_some()
        || view.content.draft.is_some()
        || ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| beside_pages(view.viewport_rect(), p))
    {
        return false;
    }
    let Some(page) = s.page.filter(|_| !s.selected.is_empty()) else { return false };
    use egui::Key;
    if !(keys.take(ctx, Key::Delete) || keys.take(ctx, Key::Backspace)) {
        return false;
    }
    // The selection stays until the edit applies: `prepare` clears it when the document changes,
    // and a refused delete (text used as a clipping path) leaves it selected.
    view.pending_edit = Edit::delete_objects(page, view.objects.selected.iter().map(|o| o.target).collect());
    true
}

/// Whether `p` is over a side panel: level with the pages but outside them. The menu bar and the
/// toolbars above take no Delete, so a pointer left there after closing a menu keeps it the pages'.
fn beside_pages(viewport: Rect, p: egui::Pos2) -> bool {
    !viewport.contains(p) && (viewport.top()..=viewport.bottom()).contains(&p.y)
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

    /// Several objects are one step, each object once (the engine orders the deletions).
    #[test]
    fn several_objects_delete_in_one_step() {
        let targets = vec![t(ObjectKind::Added, 0), t(ObjectKind::Text, 1), t(ObjectKind::Image, 0), t(ObjectKind::Text, 1)];
        let edit = Edit::delete_objects(0, targets).unwrap();
        let once = [t(ObjectKind::Added, 0), t(ObjectKind::Text, 1), t(ObjectKind::Image, 0)];
        assert!(matches!(&edit, Edit::DeleteObjects { page: 0, objects } if *objects == once), "{edit:?}");
        assert_eq!(edit.label().wording, "Delete content");
    }

    #[test]
    fn one_object_deletes_with_its_own_edit_and_none_with_nothing() {
        assert!(matches!(Edit::delete_objects(2, vec![t(ObjectKind::Image, 4)]), Some(Edit::EditPageImage { page: 2, index: 4, .. })));
        assert!(Edit::delete_objects(0, Vec::new()).is_none());
    }

    /// Selecting content objects on a page and pressing Delete must queue one edit that removes
    /// them: `page_input` selects and moves content objects but never consumes Delete or
    /// Backspace, so pressing Delete currently does nothing (issue #3780).
    #[test]
    fn delete_key_removes_selected_object() {
        use crate::canvas::page_pointer::PagePointer;
        use crate::{PdfCraftApp, canvas::ViewDefaults};
        use pdfcraft_engine::{DocId, ImageEdit};
        use pdfcraft_render::PageInfo;

        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
        app.open_bytes("blank.pdf", None, bytes.as_ref().clone()).unwrap();
        let doc = app.session.get(app.views[0].id).unwrap();

        let info = pdfcraft_render::DocInfo {
            pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
            ..Default::default()
        };
        let mut view = DocView::new(DocId(1), &info, ViewDefaults::default());
        view.objects.page = Some(0);
        view.objects.selected.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Image, index: 0 },
            rect: [50.0, 60.0, 100.0, 110.0],
            content_type: "image",
            text: None,
        });

        let ctx = egui::Context::default();
        let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)), rot: 0, pw: 800.0, ph: 600.0 };
        let ptr = PagePointer { pos: None, ..Default::default() };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Delete,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                let resp = ui.interact(ui.available_rect_before_wrap(), egui::Id::unique("page"), egui::Sense::click());
                page_input(
                    PageToolContext { ui, resp: &resp, xf: &xf, ptr: &ptr, page: 0, doc, info: &info, keys: Default::default() },
                    PagePress::Free,
                    &mut view,
                );
            },
        );
        output.textures_delta.clear();

        assert!(
            matches!(&view.pending_edit, Some(Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Delete })),
            "pressing Delete with one object selected must queue that object's deletion as one edit"
        );
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
