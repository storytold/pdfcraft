use super::{DocInfo, Edit, MAX_OUTLINES, PageGeometry, PageSession, PageXform, Pos2, Rect, user_box};
use crate::canvas::page_parts::{Owner, PageLayer, PagePart, PagePress};
use craft::behavior::geometry::{BoxAllow, BoxPart, BoxRules, BoxState, CropBox, CropGuide, ScaleRule, TransformMode};
use craft::behavior::kurbo::{BezPath, Point, Rect as DocRect, Vec2};
use craft::behavior::{CanvasView, CursorIcon, EditEvent, HitShape, IconId, PointerButton};
use craft::draw::ui::transform::{RotateZone, TransformBox};
use craft::imaging::ui::transform_preview::{PreviewMap, PreviewPixels, PreviewTextures, TransformPreview};
use craft::pages::ui::{PageBody, PageRevision, PlacedPage};
use craft::ui::buttons::Button;
use craft::ui::canvas::{CropApply, CropOutside, CropOverlay};
use craft::ui::canvas_controls::{SelectionOutline, TaskBar};
use craft::ui::contract::{Access, Bind, Common, Component, Cx, MeasureCx, PaintUi, Props as _, Role, Show as _};
use craft::ui::display::Divider;
use craft::ui::menu::{Menu, MenuIcon, MenuIconRow, MenuItem};
use craft::ui::style::{ComponentStyle, Metrics, StyleCx};

pub(crate) use pdfcraft_content::Matrix;

/// A change smaller than this, in document units, is no change.
const EPS: f64 = 1e-6;
/// A turn smaller than this, in degrees, is no turn.
const MIN_TURN: f64 = 0.05;

/// A selected page image, and the pixels shown while it is being changed.
pub struct ImageSelection {
    pub page: usize,
    pub index: usize,
    /// Crop mode: the box kept so far.
    pub crop: Option<CropMode>,
    /// A move or resize applied on release: the frame it had and the one it got, in the page's
    /// 0..1 coordinates, while the page raster still has the picture where it was.
    pub settled: Option<(Rect, Rect)>,
    /// Revision containing the applied edit, once the edit has been accepted.
    pub(crate) settled_revision: Option<PageRevision>,
    /// The picture's pixels, cut from the page render when a gesture begins, drawn through the
    /// box's map until the page render shows the picture where it went. It is the framework's
    /// session object and is not cloned or compared.
    pub(crate) preview: Option<PreviewTextures>,
    /// The frame the moving picture left on its page, in page units, rewritten every pass while the
    /// preview shows: the page's content painter fills it with paper so the picture's old pixels do
    /// not show beside the preview. Not cloned or compared.
    // PLAN: the page's content painter (pdfcraft-canvas-w8) reads this and fills the quad with paper.
    pub(crate) paper: Option<[Point; 4]>,
}

impl Clone for ImageSelection {
    fn clone(&self) -> Self {
        Self {
            page: self.page,
            index: self.index,
            crop: self.crop.clone(),
            settled: self.settled,
            settled_revision: self.settled_revision,
            preview: None,
            paper: None,
        }
    }
}

impl PartialEq for ImageSelection {
    fn eq(&self, other: &Self) -> bool {
        (self.page, self.index, &self.crop, self.settled) == (other.page, other.index, &other.crop, other.settled)
    }
}

impl std::fmt::Debug for ImageSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageSelection")
            .field("page", &self.page)
            .field("index", &self.index)
            .field("crop", &self.crop)
            .field("settled", &self.settled)
            .field("previewing", &self.preview.is_some())
            .finish()
    }
}

impl ImageSelection {
    /// Select a page image without an active gesture.
    pub fn at(page: usize, index: usize) -> Self {
        Self { page, index, crop: None, settled: None, settled_revision: None, preview: None, paper: None }
    }

    /// Whether a gesture is running: the pixels are cut and the page has not yet been edited.
    fn gesturing(&self) -> bool {
        self.preview.is_some() && self.settled.is_none()
    }
}

/// Cropping a page image: the handles trim instead of resizing, the part that will go is
/// dimmed, Enter or Done applies and Esc or Cancel leaves it.
#[derive(Clone, Debug, PartialEq)]
pub struct CropMode {
    /// The box to keep (user space or normalized UV).
    pub keep: [f64; 4],
}

impl CropMode {
    pub fn new(keep: [f64; 4]) -> Self {
        Self { keep }
    }
}

pub(crate) fn user_to_screen(xf: &PageXform, info: &DocInfo, page: usize, x: f64, y: f64) -> Pos2 {
    let Some(p) = info.pages.get(page) else { return Pos2::ZERO };
    let v = p.user_to_view(x as f32, y as f32);
    xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
}

fn screen_to_user(xf: &PageXform, info: &DocInfo, page: usize, pt: Pos2) -> (f64, f64) {
    let (vx, vy) = xf.screen_to_view(pt);
    let Some(p) = info.pages.get(page) else { return (0.0, 0.0) };
    let u = p.view_to_user(vx, vy);
    (f64::from(u[0]), f64::from(u[1]))
}

fn same_rect(a: [f64; 4], b: [f64; 4]) -> bool {
    a.into_iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

/// The four corners of the picture's frame on screen, top left, top right, bottom right and bottom
/// left of the picture as the box takes them: the selectable bounds when the picture is upright,
/// else the matrix's unit square.
fn picture_quad(xf: &PageXform, info: &DocInfo, page: usize, image: &pdfcraft_engine::PageImage) -> [Pos2; 4] {
    let [a, b, c, d, ..] = image.matrix;
    let turned = b.abs() > 1e-4 || c.abs() > 1e-4 || a < 0.0 || d < 0.0;
    let bounds = crate::canvas::image_selectable_bounds(image);
    let frame = if !turned && bounds[2] > bounds[0] && bounds[3] > bounds[1] {
        Matrix([bounds[2] - bounds[0], 0.0, 0.0, bounds[3] - bounds[1], bounds[0], bounds[1]])
    } else {
        Matrix(image.matrix)
    };
    matrix_corners(xf, info, page, frame.0)
}

/// The corners of the unit square under `matrix` on screen, top left clockwise as the picture's
/// pixels run.
fn matrix_corners(xf: &PageXform, info: &DocInfo, page: usize, matrix: [f64; 6]) -> [Pos2; 4] {
    let frame = Matrix(matrix);
    [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)].map(|(u, v)| {
        let p = frame.apply(u, v);
        user_to_screen(xf, info, page, p.0, p.1)
    })
}

/// What the controls of the selected picture read of its page.
#[derive(Clone, Copy)]
struct PagePicture<'a> {
    placed: &'a PlacedPage,
    xf: &'a PageXform,
    info: &'a DocInfo,
    index: usize,
    image: &'a pdfcraft_engine::PageImage,
}

/// The least side a resize or crop leaves, in document units: one device pixel, so a side never
/// collapses to nothing or flips.
fn least_side(ui: &egui::Ui, placed: &PlacedPage) -> f64 {
    1.0 / (placed.scale.max(1e-6) * f64::from(ui.ctx().pixels_per_point()).max(1e-6))
}

fn bounds_of(q: &[Point; 4]) -> DocRect {
    let xs = q.iter().map(|p| p.x);
    let ys = q.iter().map(|p| p.y);
    DocRect::new(
        xs.clone().fold(f64::INFINITY, f64::min),
        ys.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
        ys.fold(f64::NEG_INFINITY, f64::max),
    )
}

fn corners_of(r: DocRect) -> [Point; 4] {
    [Point::new(r.x0, r.y0), Point::new(r.x1, r.y0), Point::new(r.x1, r.y1), Point::new(r.x0, r.y1)]
}

/// The map that takes the parallelogram `from` onto `to`, corner for corner.
fn affine_between(from: [Point; 4], to: [Point; 4]) -> impl Fn(Point) -> Point {
    let (e1, e2) = (from[1] - from[0], from[3] - from[0]);
    let det = e1.x * e2.y - e1.y * e2.x;
    let (f1, f2) = (to[1] - to[0], to[3] - to[0]);
    move |p| {
        let d = p - from[0];
        if det.abs() < 1e-12 {
            return to[0] + d;
        }
        let a = (d.x * e2.y - d.y * e2.x) / det;
        let b = (e1.x * d.y - e1.y * d.x) / det;
        to[0] + f1 * a + f2 * b
    }
}

/// The picture's own decoded pixels, as the preview's texture: they cover the picture's whole
/// frame, so the preview follows the box whatever else the page shows there.
fn cut_preview(doc: &pdfcraft_engine::Document, placed: &PlacedPage, index: usize) -> Option<PreviewTextures> {
    let (w, h, rgba) = doc.page_image_rgba(placed.page, index).ok()?;
    let size = [usize::try_from(w).ok()?, usize::try_from(h).ok()?];
    if size[0] == 0 || size[1] == 0 || rgba.len() != size[0].checked_mul(size[1])?.checked_mul(4)? {
        return None;
    }
    let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
    Some(PreviewTextures::new(format!("picture-{}-{index}", placed.page), PreviewPixels::new(image, [1.0, 1.0])))
}

/// `r`, in the canvas's document space, as the page's 0..1 coordinates.
fn norm_rect(view: &CanvasView, xf: &PageXform, r: DocRect) -> Rect {
    let n = |x: f64, y: f64| {
        let (u, v) = xf.screen_to_norm(view.to_screen(Point::new(x, y)));
        Pos2::new(u, v)
    };
    Rect::from_two_pos(n(r.x0, r.y0), n(r.x1, r.y1))
}

/// `r`, in the page's 0..1 coordinates, in the canvas's document space.
fn doc_rect(view: &CanvasView, xf: &PageXform, r: Rect) -> DocRect {
    let d = |p: Pos2| view.to_doc(xf.norm_to_screen(p.x, p.y));
    DocRect::from_points(d(r.min), d(r.max))
}

fn path_of(q: &[Point; 4]) -> BezPath {
    let mut path = BezPath::new();
    path.move_to(q[0]);
    for p in &q[1..] {
        path.line_to(*p);
    }
    path.close_path();
    path
}

/// The controls of the selected picture on every page shown, and the faint outlines of the
/// pictures beside it. The edit a gesture commits comes back for the caller to queue.
pub(crate) fn picture_controls(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    info: &DocInfo,
    geo: PageGeometry<'_>,
) -> Option<Edit> {
    let mut edit = None;
    for placed in body.pages.iter().filter(|placed| placed.rect.intersects(geo.viewport)) {
        let xf = geo.xform_of(placed);
        let images = doc.page_images(placed.page);
        let selection = session.image_selection.as_ref().filter(|s| s.page == placed.page && s.index < images.len());
        let selected = selection.map(|s| s.index);
        let moving = selection.is_some_and(ImageSelection::gesturing);
        let all = images.len() <= MAX_OUTLINES;
        // Past MAX_OUTLINES only the selected picture is outlined: the hovered one is not in `PageBody`, and no hover state is kept between passes.
        let paths: Vec<BezPath> = images
            .iter()
            .enumerate()
            .filter(|(i, _)| !(moving && Some(*i) == selected) && (all || Some(*i) == selected))
            .map(|(_, image)| path_of(&picture_quad(&xf, info, placed.page, image).map(|p| body.view.to_doc(p))))
            .collect();
        if !paths.is_empty() {
            let _ = SelectionOutline::faint(tl!("Pictures"), &paths).show(ui);
        }
        let Some(index) = selected else { continue };
        let Some(image) = images.get(index) else { continue };
        let cropping = session.image_selection.as_ref().is_some_and(|s| s.crop.is_some());
        let pic = PagePicture { placed, xf: &xf, info, index, image };
        let made = if cropping { crop_controls(ui, body, session, &pic, &images) } else { box_controls(ui, body, session, doc, &pic) };
        edit = edit.or(made);
    }
    edit
}

fn box_controls(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    pic: &PagePicture<'_>,
) -> Option<Edit> {
    let PagePicture { placed, xf, info, index, image } = *pic;
    let page = placed.page;
    // The page's picture is the current revision's, not a stand-in for the one before the edit.
    if let Some(s) = session.image_selection.as_mut()
        && s.settled.is_some()
        && s.settled_revision.is_some()
        && s.settled_revision == placed.picture_revision
    {
        s.settled = None;
        s.settled_revision = None;
        s.preview = None;
    }
    let view = &body.view;
    let quad = picture_quad(xf, info, page, image).map(|p| view.to_doc(p));
    let bounds = bounds_of(&quad);
    let visible = image.visible_rect.unwrap_or(image.rect);
    let pivot = view.to_doc(user_to_screen(xf, info, page, (visible[0] + visible[2]) * 0.5, (visible[1] + visible[3]) * 0.5));
    let rules = BoxRules::in_mode(TransformMode::Free)
        .with_scale(ScaleRule::LargerAxis)
        .with_allow(BoxAllow::new(true, false, false))
        .with_min_side(least_side(ui, placed));
    let shown = TransformBox::new(tl!("Picture"), Bind::given(BoxState::new(quad, pivot)), bounds, 0.0)
        .rules(rules)
        .reference_point(false)
        .rotate_zone(RotateZone::Knob)
        .round_handles(true)
        // The arrow keys nudge through the tool (`EditTextTool::key`).
        .session_arrows(false)
        .show_at(ui, body.rect);
    let to_user = |p: Point| screen_to_user(xf, info, page, view.to_screen(p));
    let mut edit = None;
    for event in shown.edit.events.iter() {
        match event {
            EditEvent::Begin { .. } => {
                let cut = cut_preview(doc, placed, index);
                if let Some(s) = session.image_selection.as_mut() {
                    if s.preview.is_none() {
                        s.preview = cut;
                    }
                }
            }
            EditEvent::Commit { after, .. } => {
                edit = picture_edit(page, index, image.matrix, image.rect, quad, after.quad, &to_user);
                if let Some(s) = session.image_selection.as_mut() {
                    let from = s.settled.map_or_else(|| norm_rect(view, xf, bounds), |(from, _)| from);
                    if edit.is_some() {
                        s.settled = Some((from, norm_rect(view, xf, bounds_of(&after.quad))));
                    }
                    if s.settled.is_none() {
                        s.preview = None;
                    }
                }
            }
            EditEvent::Cancel { .. } => {
                if let Some(s) = session.image_selection.as_mut() {
                    if s.settled.is_none() {
                        s.preview = None;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some((_, point)) = shown.clicked {
        let at = view.to_screen(point);
        let images = doc.page_images(page);
        let hit = images.iter().enumerate().rposition(|(i, image)| {
            let quad = picture_quad(xf, info, page, image);
            let visible = image.visible_rect.map(|r| xf.user_rect(info, page, r.map(|v| v as f32)).intersect(placed.rect));
            let shape =
                if i == index { HitShape::Polygon(quad.to_vec()) } else { visible.map_or_else(|| HitShape::Polygon(quad.to_vec()), HitShape::Rect) };
            shape.hit(at)
        });
        if let Some(hit) = hit.filter(|hit| *hit != index) {
            session.image_selection = Some(ImageSelection::at(page, hit));
            return edit;
        }
    }
    // `shown.discard` (Escape with no drag) is ignored: a selected picture has no Escape outside crop mode.
    let Some(s) = session.image_selection.as_mut() else { return edit };
    s.paper = None;
    let Some(textures) = s.preview.as_ref() else { return edit };
    let from = s.settled.map_or(quad, |(from, _)| corners_of(doc_rect(view, xf, from)));
    let to = if shown.held.is_some() {
        shown.edit.shown.as_ref().and_then(|m| m.same()).map_or(quad, |state| state.quad)
    } else {
        s.settled.map_or(quad, |(_, to)| corners_of(doc_rect(view, xf, to)))
    };
    s.paper = Some(from.map(|p| placed.to_page(view.to_screen(p))));
    let paper_quad = from.map(|p| view.to_screen(p));
    PicturePaper::new(paper_quad, placed.rect).show_at(ui, body.rect);
    let full = matrix_corners(xf, info, page, image.matrix).map(|p| view.to_doc(p));
    let map = affine_between(quad, to);
    let unit = [Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(1.0, 1.0), Point::new(0.0, 1.0)];
    let source = affine_between(full, unit);
    let page_quad = [placed.rect.left_top(), placed.rect.right_top(), placed.rect.right_bottom(), placed.rect.left_bottom()];
    let source_clip = page_quad.map(|p| source(view.to_doc(p)));
    let _ = TransformPreview::new(tl!("Picture"), textures)
        .map(PreviewMap::Quad(full.map(map)))
        .source_clip(source_clip)
        .clip_quad(to)
        .show_at(ui, body.rect);
    edit
}

#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
struct PicturePaperStyle {
    #[style(metrics)]
    metrics: Metrics,
}

/// Fills the quad a moving or resized picture left behind on its page with white paper, so the old pixels
/// on the page raster do not show beside or under `TransformPreview`.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, content_painter, unregistered)]
struct PicturePaper {
    #[common]
    common: Common,
    quad: [Pos2; 4],
    clip: Rect,
}

impl PicturePaper {
    fn new(quad: [Pos2; 4], clip: Rect) -> Self {
        Self { common: Common::new(), quad, clip }
    }
}

impl Component for PicturePaper {
    const ROLE: Role = Role::GROUP;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: craft::behavior::Input = craft::behavior::Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = PicturePaperStyle;
    type Output = egui::Response;

    fn style(_: &StyleCx<'_>, _: PicturePaperStyleKey) -> PicturePaperStyle {
        PicturePaperStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> egui::Vec2 {
        egui::Vec2::ZERO
    }

    fn sense(&self) -> craft::behavior::Sense {
        craft::behavior::Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        if !self.quad.iter().all(|p| p.is_finite()) {
            return;
        }
        let content = ui.content_painter(cx);
        let painter = content.painter.with_clip_rect(self.clip.intersect(content.painter.clip_rect()));
        painter.add(egui::Shape::convex_polygon(self.quad.to_vec(), egui::Color32::WHITE, egui::Stroke::NONE));
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> egui::Response {
        cx.response.clone()
    }
}

fn crop_controls(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    pic: &PagePicture<'_>,
    images: &[pdfcraft_engine::PageImage],
) -> Option<Edit> {
    let PagePicture { placed, xf, info, image, .. } = *pic;
    let page = placed.page;
    let keep = session.image_selection.as_ref().and_then(|s| s.crop.as_ref()).map(|c| c.keep)?;
    let view = &body.view;
    let frame = picture_quad(xf, info, page, image).map(|p| view.to_doc(p));
    let kept_screen = xf.user_rect(info, page, keep.map(|v| v as f32));
    let kept = DocRect::from_points(view.to_doc(kept_screen.min), view.to_doc(kept_screen.max));
    let before = CropBox::upright(kept);
    let shown = CropOverlay::new(tl!("Crop"), Bind::given(before))
        .outside(CropOutside::Apply)
        .area(frame)
        .bounds(bounds_of(&frame))
        .min_side(least_side(ui, placed))
        .guide((CropGuide::Thirds, 0))
        // Its outside hit area must not take input from the other pages in the view.
        .show_at(ui, placed.rect);
    // The overlay takes every right-click. One on the picture being cropped, its box or its grips is the crop's: crop
    // mode stays and the page's menu stays shut. Anywhere else it goes to what lies there, as without crop mode
    // (#5835).
    if let Some((at, _)) = shown.context {
        let hits = |image: &pdfcraft_engine::PageImage| HitShape::Polygon(picture_quad(xf, info, page, image).to_vec()).hit(at);
        if hits(image) || !matches!(shown.hovered, None | Some(BoxPart::Outside)) {
            session.context_held = true;
        } else {
            let under = placed.rect.contains(at).then(|| images.iter().rposition(|other| hits(other))).flatten();
            picture_context(page, under, session);
            session.menu_for = under.map(|index| (page, Owner::Picture { index }));
            return None;
        }
    }
    let committed = shown.edit.events.iter().find_map(|event| match event {
        EditEvent::Commit { after, .. } | EditEvent::Set { after, .. } => Some(*after),
        _ => None,
    });
    if let Some(crop) = committed.filter(|crop| crop.is_finite() && crop.rect != before.rect) {
        let to_screen = |p: Point| view.to_screen(p);
        let moved = Rect::from_two_pos(to_screen(Point::new(crop.rect.x0, crop.rect.y0)), to_screen(Point::new(crop.rect.x1, crop.rect.y1)));
        let keep = user_box(xf, info, page, moved);
        if let Some(mode) = session.image_selection.as_mut().and_then(|s| s.crop.as_mut()) {
            mode.keep = keep;
        }
    }
    let keep = session.image_selection.as_ref().and_then(|s| s.crop.as_ref()).map_or(keep, |c| c.keep);
    let initial_keep = image.visible_rect.unwrap_or(image.rect);
    let has_crop = image.has_editor_crop() || !same_rect(keep, initial_keep);
    let (mut reset, mut cancel, mut done) = (false, false, false);
    let anchor = corners_of(match &shown.edit.shown {
        Some(craft::behavior::Mixed::Same(shown)) => shown.rect,
        _ => before.rect,
    });
    let _ = TaskBar::fixed(tl!("Crop"), &anchor).show(ui, body.rect, |ui, _| {
        if has_crop {
            reset |= Button::new(tl!("Remove Crop")).show(ui).clicks > 0;
        }
        cancel |= Button::new(tl!("Cancel")).show(ui).clicks > 0;
        done |= Button::new(tl!("Done")).show(ui).clicks > 0;
    });
    if reset {
        if let Some(mode) = session.image_selection.as_mut().and_then(|s| s.crop.as_mut()) {
            mode.keep = if image.has_editor_crop() { image.rect } else { initial_keep };
        }
        return None;
    }
    if done || shown.apply.is_some() {
        apply_crop_mode(session, image);
        // A click away lets the selection go; Enter and a double click keep it.
        if shown.apply == Some(CropApply::Away) {
            session.image_selection = None;
        }
    } else if (cancel || shown.discard)
        && let Some(s) = session.image_selection.as_mut()
    {
        s.crop = None;
    }
    None
}

/// Done in crop mode: it ends, and the picture `image` of the selection keeps the box kept (or
/// all of itself again).
fn apply_crop_mode(session: &mut PageSession, image: &pdfcraft_engine::PageImage) {
    let Some(s) = session.image_selection.as_mut() else { return };
    let Some(keep) = s.crop.take().map(|mode| mode.keep) else { return };
    let (page, index) = (s.page, s.index);
    if same_rect(keep, image.rect) {
        if image.has_editor_crop() {
            session.pending_edit = Some(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::RemoveCrop });
        }
    } else if image.visible_rect != Some(keep) {
        session.pending_edit = Some(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Crop(keep) });
    }
}

/// The edit a box change makes to a picture of `matrix` and `rect`, from its corners `start` to
/// `end` (document units, top left clockwise); `to_user` takes a document point to user space. A
/// change of nothing is no edit, a translation or an upright resize is a move, a pure turn is a
/// turn, and anything else is the transform that takes the old corners onto the new.
fn picture_edit(
    page: usize,
    index: usize,
    matrix: [f64; 6],
    rect: [f64; 4],
    start: [Point; 4],
    end: [Point; 4],
    to_user: &dyn Fn(Point) -> (f64, f64),
) -> Option<Edit> {
    let change = picture_change(matrix, rect, start, end, to_user)?;
    Some(Edit::EditPageImage { page, index, change })
}

fn picture_change(
    matrix: [f64; 6],
    rect: [f64; 4],
    start: [Point; 4],
    end: [Point; 4],
    to_user: &dyn Fn(Point) -> (f64, f64),
) -> Option<pdfcraft_engine::ImageEdit> {
    use pdfcraft_engine::ImageEdit as E;
    let (s, e) = (start.map(to_user), end.map(to_user));
    if s.iter().chain(&e).any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return None;
    }
    // A translation is told where the box made it, in document units: user space is reached through screen
    // points, whose rounding would make an exact move look like a slight resize.
    let moved = end[0] - start[0];
    if (0..4).all(|i| ((end[i] - start[i]) - moved).hypot() < EPS) {
        if moved.hypot() < EPS {
            return None;
        }
        let (dx, dy) = (e[0].0 - s[0].0, e[0].1 - s[0].1);
        return Some(E::Move([rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy]));
    }
    let (sw, sh, ew, eh) = (start[1] - start[0], start[3] - start[0], end[1] - end[0], end[3] - end[0]);
    let turn = |a: Vec2, b: Vec2| {
        let d = b.atan2() - a.atan2();
        (d + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
    };
    let (tw, th) = (turn(sw, ew), turn(sh, eh));
    if (ew.hypot() - sw.hypot()).abs() < EPS && (eh.hypot() - sh.hypot()).abs() < EPS && (tw - th).abs() < EPS {
        let degrees = tw.to_degrees();
        return (degrees.abs() >= MIN_TURN).then_some(E::RotateBy(degrees));
    }
    let [a, b, c, d, ..] = matrix;
    let upright_start = b.abs() < 1e-4 && c.abs() < 1e-4 && a > 0.0 && d > 0.0;
    let (u, v) = ((e[2].0 - e[3].0, e[2].1 - e[3].1), (e[0].0 - e[3].0, e[0].1 - e[3].1));
    let upright_end = u.1.abs() < EPS && v.0.abs() < EPS && u.0 > 0.0 && v.1 > 0.0;
    if upright_start && upright_end {
        let (fw, fh) = (s[2].0 - s[3].0, s[0].1 - s[3].1);
        if fw.abs() > 1e-9 && fh.abs() > 1e-9 {
            let (kx, ky) = (u.0 / fw, v.1 / fh);
            let at = |x: f64, y: f64| (e[3].0 + (x - s[3].0) * kx, e[3].1 + (y - s[3].1) * ky);
            let (p0, p1) = (at(rect[0], rect[1]), at(rect[2], rect[3]));
            return Some(E::Move([p0.0, p0.1, p1.0, p1.1]));
        }
    }
    let frame = |c: [(f64, f64); 4]| Matrix([c[2].0 - c[3].0, c[2].1 - c[3].1, c[0].0 - c[3].0, c[0].1 - c[3].1, c[3].0, c[3].1]);
    let t = frame(s).invert()?.then(&frame(e));
    t.0.iter().all(|v| v.is_finite()).then_some(E::Transform(t.0))
}

/// The parts of the pictures of `page`, in paint order so the topmost wins a press: a picture's
/// visible shape, and for the selected one its whole frame, a hole in it included. None while
/// the picture is cropped, whose crop overlay holds the page.
pub(crate) fn picture_parts(
    session: &PageSession,
    doc: &pdfcraft_engine::Document,
    page: usize,
    placed: &PlacedPage,
    geo: PageGeometry<'_>,
) -> Vec<PagePart> {
    let Some(xf) = session.page_xform(page, geo) else { return Vec::new() };
    let selection = session.image_selection.as_ref().filter(|s| s.page == page);
    if selection.is_some_and(|s| s.crop.is_some()) {
        return Vec::new();
    }
    let selected = selection.map(|s| s.index);
    doc.page_images(page)
        .iter()
        .enumerate()
        .map(|(index, image)| {
            let quad = picture_quad(&xf, &doc.info, page, image);
            let visible = image.visible_rect.map(|r| xf.user_rect(&doc.info, page, r.map(|v| v as f32)).intersect(placed.rect));
            let (shape, icon) = if selected == Some(index) {
                (HitShape::Polygon(quad.to_vec()), CursorIcon::Move)
            } else {
                (visible.map_or_else(|| HitShape::Polygon(quad.to_vec()), HitShape::Rect), CursorIcon::PointingHand)
            };
            PagePart::new(Owner::Picture { index }, page, shape, PageLayer::Picture).cursor(icon)
        })
        .collect()
}

/// A press that went to a picture: a click selects it, or on grouped artwork with text prepares
/// the page so the paragraph clicked opens (a drag still moves the group selected).
pub(crate) fn picture_press(session: &mut PageSession, doc: &pdfcraft_engine::Document, press: &PagePress, geo: PageGeometry<'_>) -> Option<Edit> {
    let Owner::Picture { index } = press.owner else { return None };
    let start = press.phase.start().filter(|s| s.button == PointerButton::Primary)?;
    if !press.phase.is_click() {
        return None;
    }
    let page = press.page;
    let grouped = doc.page_images(page).get(index).is_some_and(|im| im.is_form)
        && session.pending_edit.is_none()
        && session.line_editor.is_none()
        && doc.grouped_text_at(page, index);
    if grouped && let Some(xf) = session.page_xform(page, geo) {
        let (x, y) = screen_to_user(&xf, &doc.info, page, start.origin);
        session.image_selection = None;
        session.grouped_text_click = Some((page, [x, y]));
        return Some(Edit::PrepareText { page });
    }
    session.image_selection = Some(ImageSelection::at(page, index));
    None
}

/// Delete or Backspace on the selected picture, while its page is shown and it is not being cropped
/// or changed.
pub(crate) fn delete_picture(session: &mut PageSession, placed: &[PlacedPage], viewport: Rect) -> Option<Edit> {
    let s = session.image_selection.as_ref().filter(|s| s.crop.is_none() && !s.gesturing())?;
    if !placed.iter().any(|p| p.page == s.page && p.rect.intersects(viewport)) {
        return None;
    }
    let (page, index) = (s.page, s.index);
    session.image_selection = None;
    // Added content's selection follows the picture's, and goes with it.
    session.content.selected = None;
    Some(Edit::EditPageImage { page, index, change: pdfcraft_engine::ImageEdit::Delete })
}

/// What a right-click on a selected image asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageAction {
    Replace(usize, usize),
    Save(usize, usize),
    Copy(usize, usize),
    CopyFile(usize, usize),
    /// Copy the picture, then remove it from the page.
    Cut(usize, usize),
    /// Mark the picture for redaction (applied later from Redact a PDF).
    Redact(usize, usize),
    /// Create a link over the picture.
    Link(usize, usize),
    /// Open Recognize Text for the picture's page.
    Recognize(usize),
    Paste(usize, Option<[f64; 2]>),
    DrawFreehand,
    AddComment(usize, usize),
}

/// A right-click on `page` that went to its picture `index` (on it or a grip of it) selects it for
/// its menu, keeping it as it is if it was selected (crop mode included); one that went to anything
/// else (`None`) unselects the picture selected.
pub(crate) fn picture_context(page: usize, index: Option<usize>, session: &mut PageSession) {
    match index {
        Some(index) if session.image_selection.as_ref().is_none_or(|s| (s.page, s.index) != (page, index)) => {
            session.image_selection = Some(ImageSelection::at(page, index));
        }
        Some(_) => {}
        None => session.image_selection = None,
    }
}

/// One row of the menu: whether it ran.
fn item(ui: &mut egui::Ui, enabled: bool, label: &str) -> bool {
    MenuItem::new(label.to_string()).enabled(enabled).show(ui).ran
}

/// What the picture menu needs to know of its target: the pictures of `page`, the selected one
/// `hit`, whether a picture can be pasted, where a paste would go and the page's rotation.
#[derive(Clone, Copy)]
pub(crate) struct PictureMenu<'a> {
    pub(crate) images: &'a [pdfcraft_engine::PageImage],
    pub(crate) page: usize,
    pub(crate) hit: usize,
    pub(crate) can_paste: bool,
    pub(crate) at: Option<[f64; 2]>,
    pub(crate) rotation: u16,
}

/// The right-click menu of the selected picture `menu.hit` on `menu.page`; the action it asks the
/// caller to run, if any (edits it makes itself are queued on `session`).
pub(crate) fn picture_menu(ui: &mut egui::Ui, session: &mut PageSession, menu: &PictureMenu<'_>) -> Option<ImageAction> {
    let PictureMenu { images, page, hit, can_paste, at, rotation } = *menu;
    let img = images.get(hit)?;
    use pdfcraft_engine::ImageEdit as E;
    let raster = !img.is_form;
    let mut action = None;
    let edit = |change| Some(Edit::EditPageImage { page, index: hit, change });

    #[derive(Clone, Copy)]
    enum Pick {
        Note,
        Crop,
        Ccw,
        Cw,
    }
    let mut picks = vec![(Pick::Note, "message-square", tl!("Add a sticky note here"))];
    if raster {
        picks.push((Pick::Crop, "crop", tl!("Crop")));
    }
    picks.push((Pick::Ccw, "rotate-ccw", tl!("Rotate Counterclockwise")));
    picks.push((Pick::Cw, "rotate-cw", tl!("Rotate Clockwise")));
    let icons: Vec<MenuIcon<'_>> = picks.iter().map(|(_, name, label)| MenuIcon::new(IconId::new(*name), *label)).collect();
    if let Some(picked) = MenuIconRow::new(tl!("Picture"), &icons).show(ui).picked.and_then(|i| picks.get(i)) {
        match picked.0 {
            Pick::Note => action = Some(ImageAction::AddComment(page, hit)),
            Pick::Crop => {
                if let Some(s) = session.image_selection.as_mut() {
                    s.crop = Some(CropMode::new(img.visible_rect.unwrap_or(img.rect)));
                }
            }
            Pick::Ccw => session.pending_edit = edit(E::Rotate(3)),
            Pick::Cw => session.pending_edit = edit(E::Rotate(1)),
        }
    }

    let _ = Divider::horizontal().show(ui);
    if item(ui, true, tl!("Cut Image")) {
        action = Some(ImageAction::Cut(page, hit));
    }
    if item(ui, true, tl!("Copy Image")) {
        action = Some(ImageAction::Copy(page, hit));
    }
    if item(ui, can_paste, tl!("Paste")) {
        action = Some(ImageAction::Paste(page, at));
    }
    let _ = Divider::horizontal().show(ui);
    if raster && img.has_editor_crop() && item(ui, true, tl!("Remove crop")) {
        session.pending_edit = edit(E::RemoveCrop);
    }
    if item(ui, true, tl!("Flip horizontal")) {
        session.pending_edit = edit(E::Flip { horizontal: flip_user_axis(true, rotation) });
    }
    if item(ui, true, tl!("Flip vertical")) {
        session.pending_edit = edit(E::Flip { horizontal: flip_user_axis(false, rotation) });
    }
    let _ = Menu::submenu_row(ui, tl!("Arrange image"), |ui, _| {
        use pdfcraft_engine::ArrangeOrder as A;
        for (order, label) in [
            (A::BringToFront, tl!("Bring to front")),
            (A::SendToBack, tl!("Send to back")),
            (A::BringForward, tl!("Bring forward")),
            (A::SendBackward, tl!("Send backward")),
        ] {
            if item(ui, true, label) {
                session.pending_edit = edit(E::Arrange(order));
            }
        }
    });
    // Grouped Form artwork: draw its content in place, so its text and pictures edit one by
    // one (#2964, #3005).
    if img.is_form && item(ui, true, tl!("Ungroup")) {
        session.image_selection = None;
        session.pending_edit = edit(E::Ungroup);
    }
    if item(ui, raster, tl!("Replace image")) {
        action = Some(ImageAction::Replace(page, hit));
    }
    if item(ui, raster, tl!("Save Image As…")) {
        action = Some(ImageAction::Save(page, hit));
    }
    let _ = Divider::horizontal().show(ui);
    if item(ui, true, tl!("Draw freehand")) {
        action = Some(ImageAction::DrawFreehand);
    }
    if item(ui, true, tl!("Redact Image")) {
        action = Some(ImageAction::Redact(page, hit));
    }
    let _ = Divider::horizontal().show(ui);
    if item(ui, true, tl!("Create Link…")) {
        action = Some(ImageAction::Link(page, hit));
    }
    if item(ui, true, tl!("Recognize Text…")) {
        action = Some(ImageAction::Recognize(page));
    }
    let _ = Divider::horizontal().show(ui);
    if item(ui, true, tl!("Delete")) {
        session.image_selection = None;
        session.pending_edit = edit(E::Delete);
    }
    action
}

fn flip_user_axis(horizontal: bool, rotation: u16) -> bool {
    horizontal ^ (rotation % 180 == 90)
}

#[cfg(test)]
mod screen_orientation_tests {
    use super::*;
    use pdfcraft_engine::ImageEdit as E;

    #[test]
    fn flip_labels_follow_the_screen_axis() {
        for rotation in [0, 90, 180, 270] {
            assert_eq!(flip_user_axis(true, rotation), rotation % 180 == 0);
            assert_eq!(flip_user_axis(false, rotation), rotation % 180 != 0);
        }
    }

    /// The box corners of a `w` by `h` frame whose top left is `at`, turned by `degrees` about its centre.
    fn quad(at: (f64, f64), w: f64, h: f64, degrees: f64) -> [Point; 4] {
        let c = (at.0 + w / 2.0, at.1 + h / 2.0);
        let (sin, cos) = degrees.to_radians().sin_cos();
        [(at.0, at.1), (at.0 + w, at.1), (at.0 + w, at.1 + h), (at.0, at.1 + h)].map(|(x, y)| {
            let (dx, dy) = (x - c.0, y - c.1);
            Point::new(c.0 + dx * cos - dy * sin, c.1 + dx * sin + dy * cos)
        })
    }

    fn user(p: Point) -> (f64, f64) {
        (p.x, 100.0 - p.y)
    }

    fn frame(c: [Point; 4]) -> [f64; 6] {
        let u = c.map(user);
        [u[2].0 - u[3].0, u[2].1 - u[3].1, u[0].0 - u[3].0, u[0].1 - u[3].1, u[3].0, u[3].1]
    }

    #[test]
    fn a_committed_frame_becomes_one_picture_edit() {
        let start = quad((10.0, 10.0), 20.0, 10.0, 0.0);
        let (matrix, rect) = (frame(start), [10.0, 80.0, 30.0, 90.0]);
        let change = |end| picture_change(matrix, rect, start, end, &user);

        let moved = start.map(|p| Point::new(p.x + 5.0, p.y + 3.0));
        assert!(matches!(change(moved), Some(E::Move(r)) if r == [15.0, 77.0, 35.0, 87.0]), "a translation is a move");
        assert!(change(start).is_none(), "no movement is no edit");
        assert!(change(quad((10.0, 10.0), 20.0, 10.0, 0.01)).is_none(), "a turn under 0.05 degrees is none");
        assert!(
            matches!(change(quad((10.0, 10.0), 20.0, 10.0, 30.0)), Some(E::RotateBy(d)) if (d - 30.0).abs() < 1e-6),
            "a clockwise turn is positive"
        );
        assert!(
            matches!(change(quad((10.0, 10.0), 40.0, 20.0, 0.0)), Some(E::Move(r)) if r == [10.0, 70.0, 50.0, 90.0]),
            "an upright resize is a move"
        );

        let turned = quad((10.0, 10.0), 20.0, 10.0, 30.0);
        let tm = frame(turned);
        let end = turned.map(|p| Point::new(turned[3].x + (p.x - turned[3].x) * 1.5, turned[3].y + (p.y - turned[3].y) * 1.5));
        let Some(E::Transform(t)) = picture_change(tm, rect, turned, end, &user) else { panic!("a resize of a turned picture is a transform") };
        for (s, e) in turned.map(user).into_iter().zip(end.map(user)) {
            let (x, y) = Matrix(t).apply(s.0, s.1);
            assert!((x - e.0).abs() < 1e-6 && (y - e.1).abs() < 1e-6, "old corner {s:?} lands on the new {e:?}");
        }
    }
}
