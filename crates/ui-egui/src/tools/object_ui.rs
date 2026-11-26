//! Same-page mixed selection in Edit a PDF. Source references belong to one document
//! generation; a drag previews geometry and commits one engine edit on release.

use std::collections::HashMap;

use craft::behavior::CursorIcon;
use craft::behavior::hit::HitShape;
use craft::behavior::item_marquee::MarqueeSelect;
use craft::behavior::kurbo::BezPath;
use craft::behavior::press::{PressPhase, PressStart};
use craft::keymap::Mods;
use craft::pages::ui::{PageBody, PlacedPage};
use craft::ui::canvas_controls::SelectionOutline;
use craft::ui::contract::Show as _;
use egui::{Pos2, Rect, Ui, Vec2};
use pdfcraft_engine::{Document, Edit, EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::page_parts::{Owner, PageLayer, PagePart, PagePress};
use crate::canvas::{DocView, PageGeometry, PageSession, PageXform};

#[derive(Default)]
pub(crate) struct ObjectSelection {
    generation: Option<u64>,
    cache: HashMap<usize, Result<Vec<EditableObject>, String>>,
    page: Option<usize>,
    selected: Vec<EditableObject>,
    /// Expected geometry after our own edit, for remapping indexes that grouping can change.
    pending: Option<Vec<EditableObject>>,
    pub notice: Option<String>,
    input_blocked: bool,
    /// A paragraph (page, block) to select once the document is at hand: Esc in the inline
    /// editor asks for it, and the next `prepare` selects it.
    select_paragraph: Option<(usize, usize)>,
    /// Modifiers held when a marquee drag started, so releasing Shift before the mouse button still extends.
    marquee_mods: Mods,
}

impl ObjectSelection {
    pub fn count(&self) -> usize {
        self.selected.len()
    }

    pub(crate) fn target_at(&self, index: usize) -> Option<ObjectTarget> {
        self.selected.get(index).map(|o| o.target)
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
            self.pending = Some(added);
        }
    }

    /// Edit text's input is blocked: a dialog, the palette, another window or another tab has it.
    pub(crate) fn input_blocked(&self) -> bool {
        self.input_blocked
    }

    /// The objects selected together are on `page`: the hover outlines of what lies under them wait.
    pub(crate) fn holds(&self, page: usize) -> bool {
        self.page == Some(page) && !self.selected.is_empty()
    }

    /// A preview belongs to the active, focused document. Returning from another tab or a
    /// modal must not turn a release that happened elsewhere into a document edit.
    pub fn set_input_blocked(&mut self, blocked: bool) {
        self.input_blocked = blocked;
    }

    /// Forget the selection.
    pub(crate) fn clear(&mut self) {
        self.page = None;
        self.selected.clear();
        self.pending = None;
        self.select_paragraph = None;
    }

    /// Select paragraph `block` of `page` as an object at the next `prepare`, as Acrobat does
    /// when Esc leaves a text box: the box is then selected, so Delete removes it, the arrow keys
    /// move it and its frame drags it, without aiming for the frame.
    pub(crate) fn select_paragraph_next(&mut self, page: usize, block: usize) {
        self.select_paragraph = Some((page, block));
    }

    pub fn prepare(&mut self, doc: &Document, enabled: bool) {
        if !enabled {
            self.clear();
            return;
        }
        let generation = doc.edit_generation();
        if self.generation != Some(generation) {
            self.cache.clear();
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

    /// The objects of `page` that can be selected together, as `doc.editable_objects` lists them, or
    /// why they can't be.
    pub(crate) fn inventory(&mut self, doc: &Document, page: usize) -> Result<Vec<EditableObject>, String> {
        if self.cache.len() >= 8 && !self.cache.contains_key(&page) {
            self.cache.clear();
        }
        self.cache.entry(page).or_insert_with(|| doc.editable_objects(page)).clone()
    }
}

fn screen(xf: &PageXform, info: &DocInfo, page: usize, object: &EditableObject) -> Rect {
    xf.user_rect(info, page, object.rect.map(|v| v as f32))
}

fn single(session: &PageSession, page: usize) -> Option<ObjectTarget> {
    if let Some((_, index)) = session.content.selected.filter(|(p, _)| *p == page) {
        Some(ObjectTarget { kind: ObjectKind::Added, index })
    } else if let Some(s) = session.image_selection.as_ref().filter(|s| s.page == page) {
        Some(ObjectTarget { kind: ObjectKind::Image, index: s.index })
    } else {
        session.line_editor.as_ref().filter(|e| e.page == page).map(|e| ObjectTarget { kind: ObjectKind::Text, index: e.block })
    }
}

fn leave_single(session: &mut PageSession) {
    session.content.selected = None;
    session.image_selection = None;
    session.line_editor = None;
}

/// ⌘A in Edit a PDF: every object on the current page selected together, as Acrobat selects
/// them (paragraphs, pictures and added content), instead of the page's text. A page whose
/// objects can't be selected together, or has too many, says so and keeps what was selected.
pub(crate) fn select_all(view: &mut DocView, doc: &Document) {
    let page = view.current();
    let session = &mut view.session;
    match session.objects.inventory(doc, page) {
        Ok(objects) if objects.len() > MAX_MOVE_OBJECTS => session.objects.notice = Some(tl!("Select at most 1000 objects at a time.").into()),
        Ok(objects) => {
            session.objects.clear();
            if !objects.is_empty() {
                leave_single(session);
                session.objects.page = Some(page);
                session.objects.selected = objects;
            }
        }
        Err(_) => session.objects.notice = Some(tl!("This page contains objects that cannot be selected together.").into()),
    }
}

/// The object under `p` among the page's `objects`: added content over text over pictures, so a
/// page-sized background picture does not hide editable labels or added notes, and within each
/// kind the last, as the single-item tools take it.
fn object_at(xf: &PageXform, info: &DocInfo, page: usize, objects: &[EditableObject], p: Pos2) -> Option<ObjectTarget> {
    [ObjectKind::Added, ObjectKind::Text, ObjectKind::Image]
        .into_iter()
        .find_map(|kind| objects.iter().rev().find(|o| o.target.kind == kind && screen(xf, info, page, o).expand(2.0).contains(p)))
        .map(|o| o.target)
}

/// A press with Shift or ⌘ at `p` on `page`, whatever lies there: the object under it among the
/// page's objects joins the objects selected together or leaves them. Returns whether the press was
/// taken: by that toggle, by the notice that text being typed must be finished first, or by the
/// notice that the page's objects can't be selected together. `false` where no object lies (the
/// press starts an area selection that adds to them) and off the page.
pub(crate) fn modified_press_at(session: &mut PageSession, doc: &Document, info: &DocInfo, page: usize, xf: &PageXform, p: Pos2) -> bool {
    let objects = match modified_target(session, doc, page, xf, p) {
        ModifiedTarget::None => return false,
        ModifiedTarget::Refused(notice) => {
            session.objects.notice = Some(notice.into());
            return true;
        }
        ModifiedTarget::Objects(objects) => objects,
    };
    let Some(object) = object_at(xf, info, page, &objects, p).and_then(|t| objects.iter().find(|o| o.target == t)) else {
        return false;
    };
    if session.objects.page != Some(page) {
        let single = single(session, page).and_then(|t| objects.iter().find(|o| o.target == t)).cloned();
        session.objects.clear();
        session.objects.page = Some(page);
        session.objects.selected.extend(single);
    }
    let s = &mut session.objects;
    if let Some(index) = s.selected.iter().position(|o| o.target == object.target) {
        s.selected.remove(index);
    } else if s.selected.len() < MAX_MOVE_OBJECTS {
        s.selected.push(object.clone());
    } else {
        s.notice = Some(tl!("Select at most 1000 objects at a time.").into());
    }
    leave_single(session);
    session.pending_page = Some((page, None));
    true
}

/// A press with Shift or ⌘ on an object of its page: the object joins the selection or leaves it.
/// Called before the families' handlers, since a modified press is the objects' whatever lies under it.
/// Returns whether the press was taken.
// PLAN: contract A-6 gives `Option<Edit>`; a modified press never makes an edit and the caller needs to know it was taken
pub(crate) fn modified_press(session: &mut PageSession, doc: &Document, info: &DocInfo, press: &PagePress, geo: PageGeometry<'_>) -> bool {
    let Some(xf) = session.page_xform(press.page, geo) else { return false };
    match press.phase {
        PressPhase::Down { start, .. } => modified_press_at(session, doc, info, press.page, &xf, start.origin),
        // The rest of a press its Down took is the toggle's too: its release is no click on what lies under it
        // (a click on added text would open it for typing).
        _ => press.phase.start().is_some_and(|start| match modified_target(session, doc, press.page, &xf, start.origin) {
            ModifiedTarget::None => false,
            ModifiedTarget::Refused(_) => true,
            ModifiedTarget::Objects(objects) => object_at(&xf, info, press.page, &objects, start.origin).is_some(),
        }),
    }
}

/// What a modified press at `p` on `page` meets: nothing it takes (off the page), a refusal with its
/// notice, or the page's objects to toggle one of.
enum ModifiedTarget {
    None,
    Refused(&'static str),
    Objects(Vec<EditableObject>),
}

fn modified_target(session: &mut PageSession, doc: &Document, page: usize, xf: &PageXform, p: Pos2) -> ModifiedTarget {
    if !xf.rect.contains(p) {
        return ModifiedTarget::None;
    }
    if session.content.draft.is_some() || session.line_editor.as_ref().is_some_and(|e| e.has_changes()) {
        return ModifiedTarget::Refused(tl!("Finish or discard the text being edited before selecting several objects."));
    }
    match session.objects.inventory(doc, page) {
        Ok(objects) => ModifiedTarget::Objects(objects),
        Err(_) => ModifiedTarget::Refused(tl!("This page contains objects that cannot be selected together.")),
    }
}

/// An area selection begins: the single selections of the families let go. The sweep itself is the
/// press's span, read from each pass's press phase.
pub(crate) fn begin_marquee(session: &mut PageSession, mods: Mods) {
    leave_single(session);
    session.objects.marquee_mods = mods;
}

/// The swept rectangle on screen while the press `phase` drags, whatever it started from.
pub(crate) fn marquee_rect(phase: &PressPhase) -> Option<Rect> {
    match phase {
        PressPhase::DragStart { start, pos, .. } | PressPhase::Drag { start, pos, .. } => Some(Rect::from_two_pos(start.origin, *pos)),
        _ => None,
    }
}

/// The area selection that began at `start` ends at `pos`: what it swept becomes the selection as its mode
/// says (Shift adds, ⌘ or Ctrl toggles, none replaces), when it is over 3 points each way. A smaller sweep
/// (a click) sweeps nothing: it clears the selection, or with Shift or ⌘ leaves it as it was. A double click
/// within 3 points clears. More than [`MAX_MOVE_OBJECTS`] keeps the selection and says so.
pub(crate) fn end_marquee(
    session: &mut PageSession,
    doc: &Document,
    info: &DocInfo,
    start: PressStart,
    mods: Mods,
    pos: Pos2,
    geo: PageGeometry<'_>,
) {
    let Some((page, xf)) = session.page_xforms(geo).into_iter().find(|(_, xf)| xf.rect.contains(start.origin)) else { return };
    let start_mods = std::mem::take(&mut session.objects.marquee_mods);
    let mods = Mods {
        shift: start_mods.shift || mods.shift,
        cmd: start_mods.cmd || mods.cmd,
        ctrl: start_mods.ctrl || mods.ctrl,
        alt: start_mods.alt || mods.alt,
    };
    let base = session.objects.selection().filter(|(on, _)| *on == page).map(|(_, targets)| targets).unwrap_or_default();
    let mut sweep = MarqueeSelect::begin(start.origin, mods, base);
    sweep.update(pos);
    let rect = sweep.rect();
    if start.clicks >= 2 && rect.min.distance(rect.max) <= 3.0 {
        session.objects.clear();
        return;
    }
    let Ok(objects) = session.objects.inventory(doc, page) else {
        session.objects.clear();
        return;
    };
    let area = rect.intersect(xf.rect);
    let sweeps = area.width() > 3.0 && area.height() > 3.0;
    let items = objects.iter().filter(|_| sweeps).map(|o| (o.target, screen(&xf, info, page, o)));
    let targets = sweep.selection(items);
    if targets.len() > MAX_MOVE_OBJECTS {
        session.objects.notice = Some(tl!("Select at most 1000 objects at a time.").into());
        return;
    }
    let selected: Vec<EditableObject> = targets.iter().filter_map(|t| objects.iter().find(|o| o.target == *t).cloned()).collect();
    session.objects.clear();
    if !selected.is_empty() {
        session.objects.page = Some(page);
        session.objects.selected = selected;
    }
}

/// The four strips round `r` (screen, a paragraph open for typing) where a press would otherwise
/// close the paragraph: just outside the text, up to the frame's reach.
fn frame_band(r: Rect) -> [Rect; 4] {
    let inner = r.expand(2.0);
    let outer = r.expand(2.0 + crate::edit_text_ui::FRAME_REACH);
    [
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, inner.min.y)),
        Rect::from_min_max(Pos2::new(outer.min.x, inner.max.y), outer.max),
        Rect::from_min_max(Pos2::new(outer.min.x, inner.min.y), Pos2::new(inner.min.x, inner.max.y)),
        Rect::from_min_max(Pos2::new(inner.max.x, inner.min.y), Pos2::new(outer.max.x, inner.max.y)),
    ]
}

/// `rect` (screen) as a closed path in document units, through the canvas's view.
fn outline_path(body: &PageBody<'_>, rect: Rect) -> BezPath {
    let [a, b, c, d] = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].map(|p| body.view.to_doc(p));
    let mut path = BezPath::new();
    path.move_to(a);
    path.line_to(b);
    path.line_to(c);
    path.line_to(d);
    path.close_path();
    path
}

/// The paragraph open for typing on `page` with no changes yet: its frame takes hold of the box.
fn open_block(session: &PageSession, page: usize) -> Option<usize> {
    session.line_editor.as_ref().filter(|e| e.page == page && !e.has_changes()).map(|e| e.block)
}

/// The controls of the objects selected together on each page shown: an outline per object (and one
/// round them all when there are several), a part per object that a drag moves, and the frame band
/// of the paragraph open for typing. A drag shows the outlines offset by this pass's `presses` of the parts;
/// its release is [`objects_press`]'s.
// PLAN: contract A-6 has no `presses` parameter; the live drag offset is read from the pass's presses of the
// parts (pdf-canvas-w9 calls this with the `PagePress`es `PageParts` reported)
pub(crate) fn object_controls(
    ui: &mut Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    info: &DocInfo,
    presses: &[PagePress],
    geo: PageGeometry<'_>,
) -> Option<Edit> {
    for placed in body.pages {
        let page = placed.page;
        let xf = geo.xform_of(placed);
        let s = &session.objects;
        if s.page != Some(page) || s.selected.is_empty() {
            continue;
        }
        let offset = presses
            .iter()
            .filter(|p| p.page == page && matches!(p.owner, Owner::Objects { .. } | Owner::Frame { .. }))
            .find_map(|p| match &p.phase {
                PressPhase::DragStart { start, pos, .. } | PressPhase::Drag { start, pos, .. } => Some(*pos - start.origin),
                _ => None,
            })
            .unwrap_or(Vec2::ZERO);
        let rects: Vec<Rect> = s.selected.iter().map(|o| screen(&xf, info, page, o)).collect();
        let shown: Vec<BezPath> = rects.iter().map(|r| outline_path(body, r.translate(offset))).collect();
        SelectionOutline::solid(tl!("Selected object"), &shown).color(accent_ink(ui, 1.0)).show(ui);
        if rects.len() > 1 {
            let union = rects.iter().fold(Rect::NOTHING, |u, r| u.union(*r)).translate(offset).expand(4.0);
            let group = [outline_path(body, union)];
            SelectionOutline::faint(tl!("Selected objects"), &group).color(accent_ink(ui, 1.0)).show(ui);
        }
    }
    None
}

/// The parts of the objects selected together on each page shown: one per object that a drag moves, and the frame
/// band of the paragraph open for typing. [`object_controls`] draws their outlines and [`objects_press`] takes their
/// presses.
pub(crate) fn page_parts(
    out: &mut Vec<PagePart>,
    session: &mut PageSession,
    doc: &Document,
    info: &DocInfo,
    pages: &[PlacedPage],
    geo: PageGeometry<'_>,
) {
    for placed in pages {
        let page = placed.page;
        let xf = geo.xform_of(placed);
        // A frame press selects the paragraph on Down. Keep the same parts available until its
        // release so the press machine can deliver the rest of the drag to its original owner.
        let selected_block = (session.objects.page == Some(page) && session.objects.selected.len() == 1)
            .then(|| session.objects.selected.first())
            .flatten()
            .filter(|object| object.target.kind == ObjectKind::Text)
            .map(|object| object.target.index);
        if let Some(block) = open_block(session, page).or(selected_block)
            && let Ok(objects) = session.objects.inventory(doc, page)
            && let Some(object) = objects.iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block })
        {
            out.extend(frame_band(screen(&xf, info, page, object)).into_iter().enumerate().filter_map(|(side, band)| {
                let band = band.intersect(placed.rect);
                band.is_positive()
                    .then(|| PagePart::new(Owner::Frame { block, side }, page, HitShape::Rect(band), PageLayer::Handle).cursor(CursorIcon::Move))
            }));
        }
        let s = &session.objects;
        if s.page != Some(page) || s.selected.is_empty() {
            continue;
        }
        out.extend(s.selected.iter().enumerate().map(|(index, o)| {
            PagePart::new(Owner::Objects { index }, page, HitShape::Rect(screen(&xf, info, page, o)), PageLayer::Placed).cursor(CursorIcon::Move)
        }));
    }
}

/// A press of a part [`object_controls`] registered: the objects selected together, or the frame of
/// the paragraph open for typing, which selects its box and moves it. The drag shows the move; the
/// release past 3 points returns it as one edit, and one within 3 changes nothing.
pub(crate) fn objects_press(session: &mut PageSession, doc: &Document, _info: &DocInfo, press: &PagePress, geo: PageGeometry<'_>) -> Option<Edit> {
    let page = press.page;
    match press.phase {
        PressPhase::Down { .. } => {
            if let Owner::Frame { block, .. } = press.owner {
                let object = session
                    .objects
                    .inventory(doc, page)
                    .ok()
                    .and_then(|objects| objects.into_iter().find(|o| o.target == ObjectTarget { kind: ObjectKind::Text, index: block }));
                let object = object?;
                session.objects.clear();
                session.objects.page = Some(page);
                session.objects.selected.push(object);
                leave_single(session);
                session.pending_page = Some((page, None));
            }
            None
        }
        PressPhase::Release { start, pos, .. } => {
            if start.origin.distance(pos) <= 3.0 || session.pending_edit.is_some() {
                return None;
            }
            let xf = session.page_xform(page, geo)?;
            let (a, b) = (xf.screen_to_view(start.origin), xf.screen_to_view(pos));
            match doc.object_move_offset(page, [f64::from(b.0 - a.0), f64::from(b.1 - a.1)]) {
                Ok(offset) => Some(session.objects.move_by(page, offset)),
                Err(_) => {
                    session.objects.notice = Some(tl!("These objects could not be moved. Select them again and try a smaller move.").into());
                    None
                }
            }
        }
        _ => None,
    }
    // `info` is the page geometry every press handler takes; the move itself needs none.
}

/// An arrow key moves the selected objects, else the selected picture, a point (`far`: ten points), as
/// in Acrobat. `direction` is a unit vector on screen. Nothing while an edit waits to be applied.
pub(crate) fn nudge(session: &mut PageSession, doc: &Document, direction: Vec2, far: bool, geo: PageGeometry<'_>) -> Option<Edit> {
    if session.pending_edit.is_some() {
        return None;
    }
    let image = session.image_selection.as_ref().map(|s| (s.page, s.index));
    let objects = session.objects.page.filter(|_| !session.objects.selected.is_empty());
    let page = objects.or(image.map(|(page, _)| page))?;
    let xf = session.page_xform(page, geo)?;
    let step = direction * if far { 10.0 } else { 1.0 };
    // A point on screen, whatever the zoom and the view's rotation: one screen pixel's step in
    // view space, at unit length.
    let c = xf.rect.center();
    let unit = |d: Vec2| {
        let (a, b) = (xf.screen_to_view(c), xf.screen_to_view(c + d));
        let v = egui::vec2(b.0 - a.0, b.1 - a.1);
        if v.length() > 0.0 { v / v.length() } else { Vec2::ZERO }
    };
    let delta = unit(egui::vec2(1.0, 0.0)) * step.x + unit(egui::vec2(0.0, 1.0)) * step.y;
    let offset = doc.object_move_offset(page, [f64::from(delta.x), f64::from(delta.y)]).ok()?;
    if objects.is_some() {
        return Some(session.objects.move_by(page, offset));
    }
    let (page, index) = image?;
    let r = doc.page_images(page).get(index).map(|im| im.rect)?;
    let rect = [r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]];
    Some(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Move(rect) })
}

/// Delete removes the selected objects, as in Acrobat: a paragraph selected by its frame, or
/// everything an area selection took, goes at once instead of one typed character at a time.
pub(crate) fn delete(session: &PageSession) -> Option<Edit> {
    if session.pending_edit.is_some() {
        return None;
    }
    let (page, targets) = session.objects.selection()?;
    // The selection stays until the edit applies: `prepare` clears it when the document changes,
    // and a refused delete (text used as a clipping path) leaves it selected.
    Edit::delete_objects(page, targets)
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

/// The accent as outline ink: production outlines the selection and the editable paragraphs in blue, not in
/// the overlay grey the outline component defaults to.
pub(crate) fn accent_ink(ui: &egui::Ui, share: f32) -> craft::ui::style::DataColor {
    let [r, g, b, a] = craft::ui::runtime::env::Env::of(ui).tokens.color.accent.solid.0;
    craft::ui::style::DataColor::new(egui::Color32::from_rgba_unmultiplied(r, g, b, a).gamma_multiply(share))
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
}
