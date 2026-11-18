//! The one place that maps the engine's annotations and `Edit`s to the framework's `Annotation` and `AnnotationEdit`
//! and back. The framework's page units here are the page as shown: its `/Rotate` and the view's quarter turn both
//! applied, in points, top-left origin. [`PageMap`] carries the view's turn.

use std::collections::HashMap;

use chrono::NaiveDate;
use craft::behavior::kurbo::{Point, Rect, Vec2};
use craft::behavior::{Cursor, CursorIcon};
use craft::pages::ui::PlacedPage;
use craft::review::annotate::{marker_rect, pick_at};
use craft::review::ui::AnnotateTool;
use craft::review::{Annotation as Marked, AnnotationEdit, AnnotationId, AnnotationKind, HandleRule, Placement, ThreadId};

use super::geometry::{CARET_SIZE, clamp_to, frame, is_callout, on_text, reshapable, resizable};
use super::popups::has_popup;
use super::state::{CommentView, Composer, ComposerKind, PageCx};
use super::{Annotation, CommentPrefs, CommentTool, DocInfo, DocView, Edit, LineEnding, NewAnnotation, PageInfo, QuickTool, Shape, tool_for};
// PLAN: assumed `PagePart::new(owner, page, shape: Vec<egui::Rect>, layer)` with `.passes_drag()`, `.tip(String)` and
// `.cursor(CursorIcon)`, and `Owner::Comment { index }` (pdf-canvas-w9, `canvas/page_parts.rs`).
use crate::canvas::page_parts::{Owner, PageLayer, PagePart};

/// The id of the annotation at `index` on `page`: the page in the high 32 bits, the index in the low 32.
pub(crate) fn id_of(page: usize, index: usize) -> AnnotationId {
    let half = |v: usize| u64::from(u32::try_from(v).unwrap_or(u32::MAX));
    AnnotationId((half(page) << 32) | half(index))
}

/// The `(page, index)` an [`id_of`] id names.
pub(crate) fn key_of(id: AnnotationId) -> (usize, usize) {
    let half = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
    (half(id.0 >> 32), half(id.0 & u64::from(u32::MAX)))
}

/// The panel's thread id of the comment at `(page, index)`: the comment's own annotation id. The one mapping the
/// panel, the pop-ups and the tests use.
pub(crate) fn thread_id(page: usize, index: usize) -> ThreadId {
    ThreadId(id_of(page, index).0)
}

/// The `(page, index)` of a thread's root comment.
pub(crate) fn thread_key(thread: ThreadId) -> (usize, usize) {
    key_of(AnnotationId(thread.0))
}

/// The pages on screen this pass with the document's page geometry: the maps between a page's user space, its
/// displayed space (the framework's page units) and the screen.
pub(crate) struct PageMap<'a> {
    /// The pages placed on screen.
    pub pages: &'a [PlacedPage],
    /// The document's page geometry.
    pub info: &'a DocInfo,
    /// The view's quarter turn in degrees, which the placed pages show but `PageInfo`'s own transform does not.
    pub rotation: u16,
}

impl<'a> PageMap<'a> {
    /// A map over `pages` of the document `info`, shown turned by `rotation` degrees.
    pub(crate) fn new(info: &'a DocInfo, pages: &'a [PlacedPage], rotation: u16) -> Self {
        Self { pages, info, rotation }
    }

    fn placed(&self, page: usize) -> Option<&PlacedPage> {
        self.pages.iter().find(|p| p.page == page)
    }

    fn shows(&self, page: usize) -> bool {
        self.info.pages.get(page).is_some() && self.placed(page).is_some()
    }

    /// A user-space point of `page` in the page's displayed units.
    pub(crate) fn to_doc(&self, page: usize, user: [f64; 2]) -> Point {
        match self.info.pages.get(page) {
            Some(p) => view_point(p, self.rotation, [user[0] as f32, user[1] as f32]),
            None => Point::new(user[0], user[1]),
        }
    }

    /// A displayed point of `page` in the page's user space.
    pub(crate) fn to_user(&self, page: usize, at: Point) -> [f64; 2] {
        match self.info.pages.get(page) {
            Some(p) => user_point(p, self.rotation, at),
            None => [at.x, at.y],
        }
    }

    /// The placed page under the screen point `screen`.
    // PLAN: the sheet's `page_at(Point)` takes a screen point: a displayed point alone cannot say which page.
    pub(crate) fn page_at(&self, screen: Point) -> Option<usize> {
        let at = egui::pos2(screen.x as f32, screen.y as f32);
        self.pages.iter().find(|p| p.rect.contains(at)).map(|p| p.page)
    }

    /// The displayed point of `page` under the screen point `screen`.
    pub(crate) fn to_page(&self, page: usize, screen: Point) -> Option<Point> {
        Some(self.placed(page)?.to_page(egui::pos2(screen.x as f32, screen.y as f32)))
    }

    /// The screen position of the displayed point `at` of `page`.
    pub(crate) fn to_screen(&self, page: usize, at: Point) -> Option<egui::Pos2> {
        Some(self.placed(page)?.to_screen(at))
    }

    /// A length of `screen` points as page units. Every page is shown at one zoom, so the first placed page's scale
    /// serves.
    // PLAN: the sheet's `doc_len(screen)` has no page: the pages share one scale.
    pub(crate) fn doc_len(&self, screen: f32) -> f64 {
        let scale = self.pages.first().map_or(1.0, |p| p.scale);
        if scale.is_finite() && scale > 0.0 { f64::from(screen) / scale } else { f64::from(screen) }
    }
}

/// `PageInfo` has one entry per page; this names the "is there such a page" question for [`PageMap::shows`].
trait HasPage {
    fn contains_key_page(&self, page: usize) -> bool;
}

impl HasPage for Vec<PageInfo> {
    fn contains_key_page(&self, page: usize) -> bool {
        page < self.len()
    }
}

fn view_point(page: &PageInfo, user: [f32; 2]) -> Point {
    let v = page.user_to_view(user[0], user[1]);
    Point::new(f64::from(v[0]), f64::from(v[1]))
}

fn user_point(page: &PageInfo, at: Point) -> [f64; 2] {
    let u = page.view_to_user(at.x as f32, at.y as f32);
    [f64::from(u[0]), f64::from(u[1])]
}

/// A user-space rectangle as the displayed rectangle that bounds it.
fn view_rect(page: &PageInfo, r: [f32; 4]) -> Rect {
    let (a, b) = (view_point(page, [r[0], r[1]]), view_point(page, [r[2], r[3]]));
    Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

/// A displayed rectangle as the user-space rectangle that bounds it.
fn user_rect(page: &PageInfo, r: Rect) -> [f32; 4] {
    let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)].map(|(x, y)| page.view_to_user(x as f32, y as f32));
    let fold = |f: fn(f32, f32) -> f32, i: usize, seed: f32| corners.iter().fold(seed, |m, c| f(m, c[i]));
    [fold(f32::min, 0, f32::MAX), fold(f32::min, 1, f32::MAX), fold(f32::max, 0, f32::MIN), fold(f32::max, 1, f32::MIN)]
}

/// A displayed offset as the user-space offset it makes.
fn user_delta(page: &PageInfo, d: Vec2) -> (f64, f64) {
    let (o, p) = (page.view_to_user(0.0, 0.0), page.view_to_user(d.x as f32, d.y as f32));
    (f64::from(p[0]) - f64::from(o[0]), f64::from(p[1]) - f64::from(o[1]))
}

/// A displayed length as the user-space length it makes.
fn user_len(page: &PageInfo, len: f64) -> f64 {
    let (dx, dy) = user_delta(page, Vec2::new(len, 0.0));
    dx.hypot(dy)
}

fn bounds(points: &[Point]) -> Option<Rect> {
    let first = points.first()?;
    let seed = Rect::new(first.x, first.y, first.x, first.y);
    Some(points.iter().fold(seed, |r, p| r.union_pt(*p)))
}

/// The annotation as the framework's `AnnotationKind`: the kind of tool that makes it, and for what no tool makes (a
/// stamp, typed text, a sound) the nearest that behaves alike.
// PLAN: `DocInfo` carries no line endings, so an arrow reads as a Line here.
pub(crate) fn kind_of(all: &[Annotation], a: &Annotation) -> AnnotationKind {
    tool_for_default(all, a, None).and_then(CommentTool::kind).unwrap_or(match a.subtype.as_str() {
        "FreeText" => AnnotationKind::TextBox,
        "Stamp" => AnnotationKind::Rectangle,
        _ => AnnotationKind::Note,
    })
}

fn rgb8(c: [f32; 3]) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The framework's annotation for `a`, with `members` (its group's subordinates) joined to its shape.
// PLAN: `DocInfo` carries no ink strokes, opacity or line width, and the image signatures' aspect lives in the view's
// signature drag: `strokes` is empty, opacity 1 and width 1, and `aspect` is only a drawn signature's box.
pub(super) fn marked(info: &DocInfo, a: &Annotation, members: &[&Annotation], allowed: bool) -> Option<Marked> {
    let page = info.pages.get(a.page)?;
    let kind = kind_of(&info.annotations, a);
    let rotation = page.rotation;
    let mut rect = view_rect(page, frame(a, rotation));
    for m in members {
        rect = rect.union(view_rect(page, m.shown_rect(rotation)));
    }
    let points = if kind.is_pointwise() {
        a.points.iter().map(|q| view_point(page, *q)).collect()
    } else if kind.is_markup() {
        std::iter::once(a)
            .chain(members.iter().copied())
            .flat_map(|m| m.quads.iter())
            .flat_map(|q| [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]])
            .map(|p| view_point(page, p))
            .collect()
    } else {
        Vec::new()
    };
    let drawn_signature = a.fill_sign && a.subtype == "Ink" && rect.height() > 0.0;
    let measured = matches!(a.subtype.as_str(), "Line" | "PolyLine" | "Polygon") && a.intent.as_deref().is_some_and(|i| i.ends_with("Dimension"));
    let handle_rule = if measured {
        HandleRule::FixedPoints
    } else if kind == AnnotationKind::Callout && a.text_box.is_none() {
        HandleRule::NoResize
    } else {
        HandleRule::Free
    };
    Some(Marked {
        id: id_of(a.page, a.index),
        page: a.page,
        kind,
        rect,
        points,
        strokes: Vec::new(),
        color: a.color.map_or([255, 240, 0], rgb8),
        opacity: 1.0,
        stroke_width: 1.0,
        text: a.contents.clone().unwrap_or_default(),
        locked: a.locked || !allowed,
        aspect: drawn_signature.then(|| rect.width() / rect.height()),
        handle_rule,
    })
}

/// The comments `tool` reaches: every one with Select, a Fill & Sign tool's own marks, and with a shape tool the lot
/// (a press on one draws unless it is a handle of the selected one). None with a tool that places or marks text.
fn reach(tool: QuickTool) -> Option<fn(&Annotation) -> bool> {
    match tool {
        QuickTool::Select => Some(|_| true),
        QuickTool::Fill(_) => Some(|a| a.fill_sign),
        QuickTool::Comment(t) if t.draws() || t.clicks_points() => Some(|_| true),
        _ => None,
    }
}

/// Every comment on the placed pages that `tool` reaches, except pop-ups. A group (`/RT /Group`, or a Replace Text
/// pair) is one annotation under its primary's id with its members' shapes joined to it.
pub(crate) fn annotations(info: &DocInfo, map: &PageMap<'_>, tool: QuickTool, allowed: bool) -> Vec<Marked> {
    let Some(pick) = reach(tool) else { return Vec::new() };
    let all = &info.annotations;
    let primaries = pdfcraft_render::group_primaries(all);
    let mut members: HashMap<usize, Vec<&Annotation>> = HashMap::new();
    for (m, primary) in all.iter().zip(&primaries) {
        if let (true, Some(p)) = (m.is_group_member(), primary) {
            members.entry(*p).or_default().push(m);
        }
    }
    all.iter()
        .enumerate()
        .filter(|(_, a)| a.is_comment() && a.subtype != "Popup" && map.shows(a.page) && pick(a))
        .filter_map(|(i, a)| marked(info, a, members.get(&i).map_or(&[][..], Vec::as_slice), allowed))
        .collect()
}

fn marker_boxes(info: &DocInfo, anns: &[Marked], icon_side: f64) -> Vec<(AnnotationId, Rect)> {
    anns.iter()
        .filter_map(|a| {
            let p = info.pages.get(a.page)?;
            let page = Rect::new(0.0, 0.0, f64::from(p.width), f64::from(p.height));
            marker_rect(a, page, icon_side).map(|r| (a.id, r))
        })
        .collect()
}

/// The markers of commented text markup, in displayed units, an `icon_side` square.
pub(crate) fn markers(info: &DocInfo, map: &PageMap<'_>, icon_side: f64) -> Vec<(AnnotationId, Rect)> {
    marker_boxes(info, &annotations(info, map, QuickTool::Select, true), icon_side)
}

/// Whether `tool` shows tips at all: not with Fill & Sign, whose placement stays clear, and not while drawing
/// freehand, where a tip would cover the next stroke (#429).
fn tool_tips(tool: QuickTool) -> bool {
    let freehand = matches!(tool, QuickTool::Comment(CommentTool::Ink | CommentTool::FreehandHighlight | CommentTool::Eraser));
    !(matches!(tool, QuickTool::Fill(_)) || freehand)
}

/// Whether the page shows a tip for what lies under the pointer (a comment, a signature's badge) with `tool`: not
/// with Fill & Sign, not while drawing freehand, and not while a comment is moved, resized or drawn.
pub(crate) fn tips_shown(tool: QuickTool, view: &DocView) -> bool {
    tool_tips(tool) && view.comments.tool.in_flight().is_none() && view.comments.tool.drawing().is_empty()
}

/// What hovering `a` says, as Acrobat's popups do. Replies and Fill & Sign's marks (typed text, checks, signatures)
/// say nothing: they are page content being edited, not comments. Neither does a highlight with no text of its own,
/// when the preferences say so.
pub(super) fn hover_tip(a: &Annotation, prefs: &CommentPrefs) -> Option<String> {
    let empty_highlight = a.subtype == "Highlight" && a.contents.as_deref().is_none_or(str::is_empty);
    if a.is_reply() || a.fill_sign || prefs.hide_empty_highlight_hover && empty_highlight {
        return None;
    }
    let who = a.author.clone().unwrap_or_else(|| a.subtype.clone());
    let body = a.contents.clone().unwrap_or_default();
    Some(if body.is_empty() { who } else { format!("{who}\n{body}") })
}

/// The tips of the comments with something to say; an open pop-up already shows its comment, so it has none.
pub(crate) fn tips(info: &DocInfo, prefs: &CommentPrefs, tool: QuickTool) -> Vec<(AnnotationId, String)> {
    if !tool_tips(tool) {
        return Vec::new();
    }
    info.annotations
        .iter()
        .filter(|a| a.is_comment() && a.subtype != "Popup" && !a.popup.is_some_and(|w| w.open))
        .filter_map(|a| hover_tip(a, prefs).map(|t| (id_of(a.page, a.index), t)))
        .collect()
}

/// The one part for the comment under the screen point `pointer`, as `pick_at` finds it (so overlapping comments
/// never meet the arena's nearest-centre rule). `tolerance` and `icon_side` are page units ([`PageMap::doc_len`] of
/// the tool host's slop and icon side). Registers only under Select.
// PLAN: `pointer` is a screen point (the sheet says `Option<Point>`): `page_at` finds its page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parts(
    info: &DocInfo,
    map: &PageMap<'_>,
    prefs: &CommentPrefs,
    tool: QuickTool,
    pointer: Option<Point>,
    annotate: &AnnotateTool,
    tolerance: f64,
    icon_side: f64,
) -> Vec<PagePart> {
    let Some(screen) = pointer else { return Vec::new() };
    let (Some(page), at) = (map.page_at(screen), ()) else { return Vec::new() };
    let _ = at;
    let Some(at) = map.to_page(page, screen) else { return Vec::new() };
    let anns: Vec<Marked> = annotations(info, map, tool, true).into_iter().filter(|a| a.page == page).collect();
    let markers = marker_boxes(info, &anns, icon_side);
    let Some(id) = pick_at(&anns, at, tolerance, icon_side, &markers) else { return Vec::new() };
    let Some(found) = anns.iter().find(|a| a.id == id) else { return Vec::new() };
    let mut shapes: Vec<Rect> = if found.kind.is_markup() { found.points.chunks(4).filter_map(bounds).collect() } else { vec![found.extent(icon_side)] };
    shapes.extend(markers.iter().filter(|(m, _)| *m == id).map(|(_, r)| *r));
    let screen_shapes: Vec<egui::Rect> = shapes
        .iter()
        .filter_map(|r| Some(egui::Rect::from_two_pos(map.to_screen(page, Point::new(r.x0, r.y0))?, map.to_screen(page, Point::new(r.x1, r.y1))?)))
        .collect();
    let icon = match annotate.cursor_at(at, tolerance, icon_side) {
        Cursor::System(icon) => icon,
        _ => CursorIcon::Default,
    };
    let part = PagePart::new(Owner::Comment { index: key_of(id).1 }, page, screen_shapes, PageLayer::Comment).passes_drag().cursor(icon);
    match tips(info, prefs, tool).into_iter().find(|(t, _)| *t == id) {
        Some((_, tip)) => vec![part.tip(tip)],
        None => vec![part],
    }
}

fn engine(info: &DocInfo, page: usize, index: usize) -> Option<&Annotation> {
    info.annotations.iter().find(|a| a.page == page && a.index == index)
}

/// Whether the in-flight `edit` moves, resizes or reshapes the annotation `id`.
fn targets(edit: &AnnotationEdit, id: AnnotationId) -> bool {
    match edit {
        AnnotationEdit::Move { ids, .. } => ids.contains(&id),
        AnnotationEdit::Resize { id: own, .. } | AnnotationEdit::Reshape { id: own, .. } => *own == id,
        _ => false,
    }
}

/// Where the in-flight `edit` leaves `a`, as a user-space rectangle, when it is `a`'s: what the live image, the
/// handles and the released edit all draw.
pub(super) fn edited_user_rect(info: &DocInfo, a: &Annotation, edit: &AnnotationEdit) -> Option<[f32; 4]> {
    let own = marked(info, a, &[], true)?;
    if !targets(edit, own.id) {
        return None;
    }
    Some(user_rect(info.pages.get(a.page)?, own.edited(edit)?.rect))
}

/// The engine edit for one framework edit. A gesture that changes nothing (a click with no drag) gives none.
/// `Place` opens a composer draft (or asks for an attachment's file) on `view` and gives none itself.
pub(crate) fn edit(e: AnnotationEdit, map: &PageMap<'_>, view: &mut CommentView, info: &DocInfo, prefs: &CommentPrefs) -> Option<Edit> {
    match e {
        AnnotationEdit::Create(a) => create(&a, map, prefs),
        AnnotationEdit::Move { ids, delta } => moved(&ids, delta, info),
        AnnotationEdit::Resize { id, rect } => {
            let (page, index) = key_of(id);
            engine(info, page, index).filter(|a| resizable(a))?;
            let r = user_rect(info.pages.get(page)?, rect).map(f64::from);
            Some(Edit::ResizeAnnotation { page, index, rect: r })
        }
        AnnotationEdit::Reshape { id, points } => {
            let (page, index) = key_of(id);
            engine(info, page, index).filter(|a| reshapable(a))?;
            Some(Edit::ReshapeAnnotation { page, index, points: points.iter().map(|p| map.to_user(page, *p)).collect() })
        }
        AnnotationEdit::Delete(ids) => {
            let keys: Vec<(usize, usize)> = ids.into_iter().map(key_of).collect();
            // A group member picked on the page, with its group the selection alone, goes by itself.
            let raw = match view.target() {
                Some(member) if view.selection().len() <= 1 => vec![member],
                _ => keys,
            };
            view.clear_selection();
            delete_edit(raw, info)
        }
        AnnotationEdit::EraseAlong { ids, path, radius } => erased(&ids, &path, radius, map, info),
        AnnotationEdit::Open(id) => {
            let (page, index) = key_of(id);
            let a = engine(info, page, index)?;
            // A double-click opens the comment's pop-up window, as in Acrobat; a text box, whose text is the comment
            // itself, is edited in place.
            if has_popup(a) {
                return (!a.popup.is_some_and(|p| p.open)).then_some(Edit::SetAnnotationPopup { page, index, rect: None, open: Some(true) });
            }
            view.composer = Some(Composer {
                page,
                at: edit_anchor(info, a),
                kind: ComposerKind::Edit(index),
                text: a.contents.clone().unwrap_or_default(),
                focus: true,
            });
            None
        }
        AnnotationEdit::Place(p) => {
            place(&p, map, view);
            None
        }
        // The tool holds the selection; whole-stroke erasing is off (the app's tool erases along the path).
        _ => None,
    }
}

fn moved(ids: &[AnnotationId], delta: Vec2, info: &DocInfo) -> Option<Edit> {
    if delta == Vec2::ZERO {
        return None;
    }
    let mut seen: Vec<(usize, usize)> = Vec::new();
    let mut edits: Vec<Edit> = ids
        .iter()
        .map(|id| key_of(*id))
        .filter(|k| {
            let new = !seen.contains(k);
            seen.push(*k);
            new
        })
        .filter_map(|(page, index)| {
            // Text markup and a Replace Text caret are where their text is, a locked comment stays, and a reply is
            // moved with its comment.
            let a = engine(info, page, index).filter(|a| !a.is_reply() && !on_text(a) && !a.locked)?;
            let p = info.pages.get(page)?;
            let (dx, dy) = user_delta(p, delta);
            if !(dx.is_finite() && dy.is_finite()) {
                return None;
            }
            // A callout is pressed in its text box: the box moves alone and its arrow point stays.
            Some(if is_callout(a) && resizable(a) {
                let f = frame(a, p.rotation).map(f64::from);
                Edit::ResizeAnnotation { page, index, rect: [f[0] + dx, f[1] + dy, f[2] + dx, f[3] + dy] }
            } else {
                Edit::MoveAnnotation { page, index, dx, dy }
            })
        })
        .collect();
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Edit::Batch { label: tl!("Move comments").to_owned(), edits }),
    }
}

/// The edit deleting `raw` (one undo step for several): locked and stale slots are dropped, later positions go first
/// so each deletion leaves the earlier ones where they are.
fn delete_edit(raw: Vec<(usize, usize)>, info: &DocInfo) -> Option<Edit> {
    let mut keys: Vec<(usize, usize)> = raw.into_iter().filter(|(page, index)| engine(info, *page, *index).is_some_and(|a| !a.locked)).collect();
    keys.sort_unstable_by(|a, b| b.cmp(a));
    keys.dedup();
    match keys.as_slice() {
        [] => None,
        [(page, index)] => Some(Edit::DeleteAnnotation { page: *page, index: *index }),
        _ => Some(Edit::Batch {
            label: tl!("Delete comments").to_owned(),
            edits: keys.into_iter().map(|(page, index)| Edit::DeleteAnnotation { page, index }).collect(),
        }),
    }
}

/// The edit that deletes every selected comment, and clears the selection. A group's member picked on the page, with
/// its group the selection alone, goes by itself, as Acrobat deletes the member clicked ([`CommentView::target`]).
pub(crate) fn delete_selected(cv: &mut CommentView, info: &DocInfo) -> Option<Edit> {
    let raw = if cv.selection().len() <= 1 { cv.target().into_iter().collect() } else { cv.selection() };
    cv.clear_selection();
    delete_edit(raw, info)
}

fn erased(ids: &[AnnotationId], path: &[Point], radius: f64, map: &PageMap<'_>, info: &DocInfo) -> Option<Edit> {
    let mut hits: Vec<(usize, usize)> =
        ids.iter().map(|id| key_of(*id)).filter(|(p, i)| engine(info, *p, *i).is_some_and(|a| a.subtype == "Ink" && !a.locked)).collect();
    // Later indices first: deleting an emptied drawing shifts the ones after it.
    hits.sort_unstable_by(|a, b| b.cmp(a));
    hits.dedup();
    let edits: Vec<Edit> = hits
        .into_iter()
        .filter_map(|(page, index)| {
            let p = info.pages.get(page)?;
            Some(Edit::EraseInk { page, index, path: path.iter().map(|q| map.to_user(page, *q)).collect(), radius: user_len(p, radius) })
        })
        .collect();
    (!edits.is_empty()).then(|| Edit::Batch { label: "Erase".into(), edits })
}

fn create(a: &Marked, map: &PageMap<'_>, prefs: &CommentPrefs) -> Option<Edit> {
    let tool = CommentTool::from_kind(a.kind)?;
    let page = map.info.pages.get(a.page)?;
    let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(page.width, page.height));
    let user = |p: Point| {
        let c = clamp_to(bounds, egui::pos2(p.x as f32, p.y as f32));
        map.to_user(a.page, Point::new(f64::from(c.x), f64::from(c.y)))
    };
    let pts: Vec<[f64; 2]> = a.points.iter().map(|p| user(*p)).collect();
    let strokes = || -> Vec<Vec<[f64; 2]>> { a.strokes.iter().map(|s| s.iter().map(|p| user(*p)).collect()).collect() };
    let boxed = || {
        let (p, q) = (user(Point::new(a.rect.x0, a.rect.y0)), user(Point::new(a.rect.x1, a.rect.y1)));
        [p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1])]
    };
    let shape = match a.kind {
        AnnotationKind::Ink => Shape::Ink { strokes: strokes() },
        AnnotationKind::FreehandHighlight => Shape::FreehandHighlight { strokes: strokes() },
        AnnotationKind::Line | AnnotationKind::Arrow => Shape::Line {
            from: *pts.first()?,
            to: *pts.get(1)?,
            start: LineEnding::None,
            end: if a.kind == AnnotationKind::Arrow { LineEnding::OpenArrow } else { LineEnding::None },
        },
        AnnotationKind::Rectangle => Shape::Rectangle { rect: boxed() },
        AnnotationKind::Oval => Shape::Oval { rect: boxed() },
        AnnotationKind::Polygon | AnnotationKind::Cloud => Shape::Polygon { vertices: pts, cloud: a.kind == AnnotationKind::Cloud },
        AnnotationKind::PolyLine => Shape::PolyLine { vertices: pts, start: LineEnding::None, end: LineEnding::None },
        AnnotationKind::Highlight | AnnotationKind::Underline | AnnotationKind::StrikeOut | AnnotationKind::Squiggly => Shape::TextMarkup {
            kind: tool.markup()?,
            quads: a
                .points
                .chunks_exact(4)
                .filter_map(|q| {
                    let [p0, p1, p2, p3] = q else { return None };
                    let [u0, u1, u2, u3] = [user(*p0), user(*p1), user(*p2), user(*p3)];
                    Some([u0[0], u0[1], u1[0], u1[1], u2[0], u2[1], u3[0], u3[1]])
                })
                .collect(),
        },
        _ => return None,
    };
    Some(new_comment(a.page, prefs, tool, shape, String::new()))
}

/// A placement opens its composer draft (a note, a text box, a callout, an insertion), or asks for an attachment's
/// file, and leaves nothing selected.
fn place(p: &Placement, map: &PageMap<'_>, view: &mut CommentView) {
    let at = map.to_user(p.page, p.at);
    let kind = match p.kind {
        AnnotationKind::Note => ComposerKind::Note,
        AnnotationKind::TextBox => ComposerKind::TextBox { size: p.size.map(|s| [s.x, s.y]) },
        AnnotationKind::Callout => {
            let Some(from) = p.from else { return };
            ComposerKind::Callout { point: map.to_user(p.page, from) }
        }
        AnnotationKind::Caret => ComposerKind::Caret { size: p.size.map_or(CARET_SIZE, |s| s.y) },
        AnnotationKind::Attach => {
            view.attach_at = Some((p.page, at));
            view.select(None);
            return;
        }
        _ => return,
    };
    view.composer = Some(Composer { page: p.page, at, kind, text: String::new(), focus: true });
    view.select(None);
}

/// The edit adding a comment made with `tool`, styled by the preferences.
pub(super) fn new_comment(page: usize, prefs: &CommentPrefs, tool: CommentTool, shape: Shape, contents: String) -> Edit {
    Edit::AddAnnotation(NewAnnotation { page, shape, style: prefs.style(tool), contents, author: prefs.author.clone() })
}

/// Where the composer for editing `a` opens: the top-right corner of the comment as drawn on the displayed page.
pub(crate) fn edit_anchor(info: &DocInfo, a: &Annotation) -> [f64; 2] {
    let Some(p) = info.pages.get(a.page) else { return [f64::from(a.rect[2]), f64::from(a.rect[3])] };
    let r = a.shown_rect(p.rotation);
    let (u, v) = (p.user_to_view(r[0], r[1]), p.user_to_view(r[2], r[3]));
    p.view_to_user(u[0].max(v[0]), u[1].min(v[1])).map(f64::from)
}

/// The user-space box of the screen rectangle `r`: the bounds of its corners' user points, for a pop-up's `/Rect`.
pub(crate) fn screen_to_user_rect(cx: &PageCx<'_>, r: egui::Rect) -> [f64; 4] {
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|c| cx.to_user(c));
    corners.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, c| [b[0].min(c[0]), b[1].min(c[1]), b[2].max(c[0]), b[3].max(c[1])])
}

/// A PDF date (`D:YYYYMMDDHHmmSS` with an optional `Z` or `±HH'mm'` zone) as Unix seconds. Missing parts take the PDF
/// defaults (month and day 1, time 0, zone UTC); text that is not a date gives `None`.
pub(crate) fn pdf_date_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    let body = text.strip_prefix("D:").unwrap_or(text);
    let digits_len = body.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, zone) = body.split_at_checked(digits_len)?;
    if digits_len < 4 || digits_len % 2 != 0 {
        return None;
    }
    let part = |from: usize, len: usize, default: u32| -> Option<u32> {
        if digits_len <= from { Some(default) } else { digits.get(from..from + len)?.parse().ok() }
    };
    let year: i32 = digits.get(..4)?.parse().ok()?;
    let local = NaiveDate::from_ymd_opt(year, part(4, 2, 1)?, part(6, 2, 1)?)?.and_hms_opt(part(8, 2, 0)?, part(10, 2, 0)?, part(12, 2, 0)?)?;
    local.and_utc().timestamp().checked_sub(zone_seconds(zone)?)
}

/// The seconds a PDF date's zone (`Z`, `+HH'mm'`, `-HH'mm'`, or none) lies east of UTC.
fn zone_seconds(zone: &str) -> Option<i64> {
    let mut chars = zone.chars();
    let sign = match chars.next() {
        None | Some('Z') => return Some(0),
        Some('+') => 1,
        Some('-') => -1,
        Some(_) => return None,
    };
    let digits: String = chars.filter(char::is_ascii_digit).collect();
    let part = |from: usize| -> Option<i64> { if digits.len() <= from { Some(0) } else { digits.get(from..from + 2)?.parse().ok() } };
    let (hours, minutes) = (part(0)?, part(2)?);
    (hours < 24 && minutes < 60).then_some(sign * (hours * 3600 + minutes * 60))
}
