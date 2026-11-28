//! View ▸ Zoom ▸ Loupe tool: a floating, movable, resizable non-modal window that displays a
//! magnified view of an area on the active document page, with interactive canvas targeting,
//! zoom controls (100%–800%), and drag-to-pan inside the loupe image.

use craft::behavior::{CancelReason, Cursor, CursorIcon, CursorPriority, IconId, Input, Metrics, PressPhase, Sense, ZoomLadder};
use craft::pages::ui::PlacedPage;
use craft::theme::TypeRole;
use craft::ui::buttons::IconButton;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolPath, ToolPointer};
use craft::ui::contract::{
    Bind, Common, Component, Cx, MeasureCx, NoValue, Outcome, PaintUi, Pointer as _, Props as _, Role, Show as _, ShowContainer as _, clicked,
};
use craft::ui::dialog::Dialog;
use craft::ui::display::{Divider, Shade, Text};
use craft::ui::layout_components::Row;
use craft::ui::primitives::textures::{
    CachedTexture, Priority, Revision, TextureState, ThumbRender, ThumbRequest, ThumbSource, ThumbnailOptions, Thumbnails,
};
use craft::ui::primitives::{Draw, PaintShape, StrokeWidth};
use craft::ui::style::{ComponentStyle, DataColor, StyleColor, StyleCx};
use craft::ui::value_drag::Slider;
use egui::{Color32, CornerRadius, Rect, Response, Vec2, pos2, vec2};
use pdfcraft_engine::DocId;

use crate::canvas::page_box_drag::page_under;
use crate::canvas::pictures::DocPages;
use crate::canvas::{PT, PageXform};
use crate::{PdfCraftApp, QuickTool};

/// Minimum loupe magnification factor (100%).
pub const LOUPE_ZOOM_MIN: f32 = 1.0;

/// Maximum loupe magnification factor (800%).
pub const LOUPE_ZOOM_MAX: f32 = 8.0;

/// Default loupe magnification factor (200%).
pub const LOUPE_ZOOM_DEFAULT: f32 = 2.0;

/// Default viewport size of the magnified image pane in logical points.
const LOUPE_DEFAULT_VIEWPORT: [f32; 2] = [280.0, 180.0];

/// Smallest viewport size used for aperture calculations in logical points.
const LOUPE_MIN_VIEWPORT: [f32; 2] = [80.0, 60.0];

/// Initial floating window size in logical points.
const LOUPE_WINDOW_SIZE: Vec2 = vec2(340.0, 280.0);

/// Minimum resizable window size in logical points.
const LOUPE_WINDOW_MIN_SIZE: Vec2 = vec2(240.0, 190.0);

/// Largest raster side requested for the loupe tile in device pixels.
const LOUPE_MAX_TEXTURE_SIDE: u32 = 2048;

/// Clamps `zoom` to the supported loupe range (`1.0..=8.0`). Non-finite `NaN` falls back to
/// [`LOUPE_ZOOM_DEFAULT`]; infinities clamp to the ends.
pub fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_nan() {
        return LOUPE_ZOOM_DEFAULT;
    }
    zoom.clamp(LOUPE_ZOOM_MIN, LOUPE_ZOOM_MAX)
}

/// Steps `zoom` up (`larger == true`) or down (`larger == false`) along the viewer's
/// [`ZoomLadder`], clamped to `1.0..=8.0`.
pub fn step_zoom(zoom: f32, larger: bool) -> f32 {
    let clamped = clamp_zoom(zoom);
    let range = f64::from(LOUPE_ZOOM_MIN)..=f64::from(LOUPE_ZOOM_MAX);
    let stepped = ZoomLadder::step_in_range(f64::from(clamped), if larger { 1 } else { -1 }, range);
    clamp_zoom(stepped as f32)
}

/// Parses a loupe zoom percentage string (e.g. `"200"`, `"200%"`, `" 400 % "`) in `100..=800`
/// into a zoom factor (`1.0..=8.0`). Returns `None` for out-of-range or malformed input.
pub fn parse_zoom_percent(value: &str) -> Option<f32> {
    let percent = value.trim().trim_end_matches('%').trim_end().parse::<f32>().ok()?;
    (percent.is_finite() && (LOUPE_ZOOM_MIN * 100.0..=LOUPE_ZOOM_MAX * 100.0).contains(&percent)).then_some(percent / 100.0)
}

/// Clamps a page-space target point `[x, y]` into `[0.0..=page_w, 0.0..=page_h]`.
/// `NaN` coordinates default to the page centre; non-finite page dimensions clamp to `0.0`.
pub fn clamp_target(target: [f32; 2], page_size: [f32; 2]) -> [f32; 2] {
    let pw = if page_size[0].is_finite() { page_size[0].max(0.0) } else { 0.0 };
    let ph = if page_size[1].is_finite() { page_size[1].max(0.0) } else { 0.0 };
    let x = if target[0].is_nan() { pw * 0.5 } else { target[0].clamp(0.0, pw) };
    let y = if target[1].is_nan() { ph * 0.5 } else { target[1].clamp(0.0, ph) };
    [x, y]
}

/// Computes the aperture rectangle `[x0, y0, x1, y1]` in page view space (points, y-down)
/// centred on `target`, sized from `loupe_zoom` and `window_size` (in logical screen points),
/// and kept inside `[0..page_size[0], 0..page_size[1]]`.
pub fn aperture_rect(target: [f32; 2], page_size: [f32; 2], loupe_zoom: f32, window_size: [f32; 2]) -> [f32; 4] {
    let zoom = clamp_zoom(loupe_zoom);
    let pw = if page_size[0].is_finite() { page_size[0].max(1.0) } else { 1.0 };
    let ph = if page_size[1].is_finite() { page_size[1].max(1.0) } else { 1.0 };
    let win_w = if window_size[0].is_finite() { window_size[0].max(1.0) } else { LOUPE_DEFAULT_VIEWPORT[0] };
    let win_h = if window_size[1].is_finite() { window_size[1].max(1.0) } else { LOUPE_DEFAULT_VIEWPORT[1] };
    let scale = zoom * PT;
    let ap_w = (win_w / scale).clamp(1.0, pw);
    let ap_h = (win_h / scale).clamp(1.0, ph);
    let [cx, cy] = clamp_target(target, [pw, ph]);
    let x0 = (cx - ap_w * 0.5).clamp(0.0, (pw - ap_w).max(0.0));
    let y0 = (cy - ap_h * 0.5).clamp(0.0, (ph - ap_h).max(0.0));
    let x1 = (x0 + ap_w).min(pw);
    let y1 = (y0 + ap_h).min(ph);
    [x0, y0, x1, y1]
}

/// Aperture rectangle in unturned page view space for a view rotated clockwise by `rotation`
/// degrees (a quarter turn swaps which window axis maps to page width vs height).
pub fn aperture_rect_for_rotation(target: [f32; 2], page_size: [f32; 2], loupe_zoom: f32, window_size: [f32; 2], rotation: u16) -> [f32; 4] {
    let effective_window = if rotation % 180 == 90 { [window_size[1], window_size[0]] } else { window_size };
    aperture_rect(target, page_size, loupe_zoom, effective_window)
}

/// Converts an unturned view-space `aperture` `[x0, y0, x1, y1]` on a page of `page_size` into
/// normalised `[u0, v0, u1, v1]` coordinates in the view-rotated frame expected by `DocPages`.
pub fn turned_norm_region(aperture: [f32; 4], page_size: [f32; 2], rotation: u16) -> [f32; 4] {
    let pw = if page_size[0].is_finite() { page_size[0].max(1.0) } else { 1.0 };
    let ph = if page_size[1].is_finite() { page_size[1].max(1.0) } else { 1.0 };
    let a0 = (aperture[0] / pw).clamp(0.0, 1.0);
    let b0 = (aperture[1] / ph).clamp(0.0, 1.0);
    let a1 = (aperture[2] / pw).clamp(0.0, 1.0);
    let b1 = (aperture[3] / ph).clamp(0.0, 1.0);
    let turn = |u: f32, v: f32| match rotation % 360 {
        90 => (1.0 - v, u),
        180 => (1.0 - u, 1.0 - v),
        270 => (v, 1.0 - u),
        _ => (u, v),
    };
    let (p0, p1) = (turn(a0, b0), turn(a1, b1));
    [p0.0.min(p1.0).clamp(0.0, 1.0), p0.1.min(p1.1).clamp(0.0, 1.0), p0.0.max(p1.0).clamp(0.0, 1.0), p0.1.max(p1.1).clamp(0.0, 1.0)]
}

/// Pans `target` in unturned page view space by a screen-space `drag_delta` inside the loupe
/// image at `loupe_zoom` and view `rotation`, clamped to `page_size`.
pub fn pan_target(target: [f32; 2], drag_delta: [f32; 2], page_size: [f32; 2], loupe_zoom: f32, rotation: u16) -> [f32; 2] {
    if !drag_delta[0].is_finite() || !drag_delta[1].is_finite() {
        return clamp_target(target, page_size);
    }
    let scale = clamp_zoom(loupe_zoom) * PT;
    // Dragging the magnified content right pulls the viewport left.
    let (dx, dy) = (-drag_delta[0] / scale, -drag_delta[1] / scale);
    let (dvx, dvy) = match rotation % 360 {
        90 => (dy, -dx),
        180 => (-dx, -dy),
        270 => (-dy, dx),
        _ => (dx, dy),
    };
    clamp_target([target[0] + dvx, target[1] + dvy], page_size)
}

/// Active state for the floating Loupe magnifier window and canvas aperture.
#[derive(Clone, Debug, PartialEq)]
pub struct LoupeState {
    /// Document this loupe is attached to.
    pub doc_id: Option<DocId>,
    /// Current target: `(page_index, [x, y])` in 0-based page index and page view space (points).
    pub target: Option<(usize, [f32; 2])>,
    /// Magnification factor (`1.0` = 100%, `8.0` = 800%).
    pub zoom: f32,
    /// Last measured viewport size of the magnified image area in logical points.
    pub viewport_size: [f32; 2],
}

impl LoupeState {
    /// Creates a new Loupe state on `doc_id` with optional initial `target` and `zoom`.
    pub fn new(doc_id: Option<DocId>, target: Option<(usize, [f32; 2])>, zoom: f32) -> Self {
        Self { doc_id, target, zoom: clamp_zoom(zoom), viewport_size: LOUPE_DEFAULT_VIEWPORT }
    }

    /// Sets the magnification factor, clamped to `1.0..=8.0`.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = clamp_zoom(zoom);
    }

    /// Steps the magnification factor up or down along the viewer's zoom ladder (`1.0..=8.0`).
    pub fn zoom_step(&mut self, larger: bool) {
        self.zoom = step_zoom(self.zoom, larger);
    }

    /// Sets the target page and point in page view space, clamped to `page_size`.
    pub fn set_target(&mut self, page: usize, point: [f32; 2], page_size: [f32; 2]) {
        self.target = Some((page, clamp_target(point, page_size)));
    }

    /// Clears the target so no stale page or content is shown.
    pub fn clear_target(&mut self) {
        self.target = None;
    }
}

/// Canvas tool for `QuickTool::Loupe`: claims presses and drags on placed pages to position the
/// loupe target and shows a crosshair cursor while over a page.
pub(crate) struct LoupeTool<'a> {
    placed: Vec<PlacedPage>,
    /// Unrotated view-space page sizes for the placed pages.
    sizes: Vec<(usize, (f32, f32))>,
    rotation: u16,
    /// Page where the active drag started, kept across passes on `PageSession`.
    dragging_page: &'a mut Option<usize>,
    /// Target picked during this pass (0-based page index + page view-space point).
    target: Option<(usize, [f32; 2])>,
}

impl<'a> LoupeTool<'a> {
    /// Builds the Loupe tool for a pass over `placed` pages.
    pub(crate) fn new(
        dragging_page: &'a mut Option<usize>,
        placed: &[PlacedPage],
        rotation: u16,
        page_size_of: impl Fn(usize) -> (f32, f32),
    ) -> Self {
        let sizes = placed.iter().map(|p| (p.page, page_size_of(p.page))).collect();
        Self { placed: placed.to_vec(), sizes, rotation, dragging_page, target: None }
    }

    /// The target updated by this pass's press or drag, once.
    pub(crate) fn take_target(&mut self) -> Option<(usize, [f32; 2])> {
        self.target.take()
    }

    fn size_of(&self, page: usize) -> Option<(f32, f32)> {
        self.sizes.iter().find(|(p, _)| *p == page).map(|(_, s)| *s)
    }

    /// Maps screen position `pos` to `(page, [vx, vy])` on `placed`, clamped to the page bounds.
    fn target_on(&self, placed: &PlacedPage, pos: egui::Pos2) -> Option<(usize, [f32; 2])> {
        let size = self.size_of(placed.page)?;
        let xf = PageXform::of(placed, self.rotation, size);
        let (vx, vy) = xf.screen_to_view(pos);
        Some((placed.page, clamp_target([vx, vy], [size.0, size.1])))
    }

    /// Resolves a drag/release position: uses the page under the pointer if any, else clamps to the
    /// page where the drag began.
    fn update_drag(&mut self, pos: egui::Pos2) {
        if let Some(placed) = page_under(&self.placed, pos) {
            *self.dragging_page = Some(placed.page);
            self.target = self.target_on(placed, pos);
        } else if let Some(page) = *self.dragging_page
            && let Some(placed) = self.placed.iter().find(|p| p.page == page)
        {
            self.target = self.target_on(placed, pos);
        }
    }
}

impl CanvasTool for LoupeTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        let Some(placed) = page_under(&self.placed, pointer.pos) else {
            *self.dragging_page = None;
            return ToolAction::Ignored;
        };
        *self.dragging_page = Some(placed.page);
        self.target = self.target_on(placed, pointer.pos);
        ToolAction::Claimed
    }

    fn drag(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.update_drag(pointer.pos);
    }

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        self.update_drag(pointer.pos);
        *self.dragging_page = None;
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {
        *self.dragging_page = None;
    }

    fn paint(&self, _draw: &Draw<'_>, _cx: &ToolContext<'_>) {}

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        if cx.pointer.is_some_and(|p| page_under(&self.placed, p.pos).is_some()) {
            Cursor::System(CursorIcon::Crosshair)
        } else {
            Cursor::System(CursorIcon::Default)
        }
    }

    fn autoscrolls(&self) -> bool {
        false
    }

    fn view_only(&self) -> bool {
        true
    }

    fn takes_every_move(&self) -> bool {
        true
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Loupe tool command and control options")
    }
}

/// Thumbnail source wrapper keying loupe tiles by `(DocId, page)` so switching documents never
/// reuses a stand-in texture from another document.
struct LoupeSource<'a> {
    pages: DocPages<'a>,
}

impl ThumbSource<(DocId, usize)> for LoupeSource<'_> {
    fn revision(&self, key: &(DocId, usize), request: &ThumbRequest) -> Revision {
        Revision::of(&(key.0.0, self.pages.revision(&key.1, request)))
    }

    fn render(&self, key: &(DocId, usize), request: &ThumbRequest) -> ThumbRender {
        self.pages.render(&key.1, request)
    }
}

/// Visual tokens for the loupe magnified viewport.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub(crate) struct LoupeViewportStyle {
    #[style(metrics)]
    metrics: Metrics,
    border: StyleColor,
    corners: CornerRadius,
}

/// Output reported by [`LoupeViewport`] for one pass.
#[derive(Debug)]
struct LoupeViewportOutput {
    response: Response,
    drag_delta: Option<Vec2>,
}

impl Outcome for LoupeViewportOutput {
    type Value = NoValue;

    fn response(&self) -> &Response {
        &self.response
    }
}

/// The magnified image pane inside the Loupe window: paints the rendered tile over paper colour
/// and senses pointer drags to pan the loupe target.
#[derive(craft::ui::contract::Component)]
#[component(content_painter, unregistered)]
struct LoupeViewport {
    #[common]
    common: Common,
    paper: Color32,
    picture: Option<(CachedTexture, Vec2)>,
    can_pan: bool,
    drag_delta: Option<Vec2>,
}

impl LoupeViewport {
    fn new(paper: Color32, picture: Option<(CachedTexture, Vec2)>, can_pan: bool) -> Self {
        LoupeViewport { common: Common::new(), paper, picture, can_pan, drag_delta: None }.label(tl!("Magnified view"))
    }
}

impl Component for LoupeViewport {
    const ROLE: Role = Role::IMAGE;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    const INPUT: Input = Input::Pointer;
    type Style = LoupeViewportStyle;
    type Output = LoupeViewportOutput;

    fn style(cx: &StyleCx<'_>, _: LoupeViewportStyleKey) -> LoupeViewportStyle {
        let radius = cx.tokens.shape.radius.sm.round().clamp(0.0, 255.0) as u8;
        LoupeViewportStyle { metrics: Metrics::new(), border: cx.color(cx.tokens.color.line.frame), corners: CornerRadius::same(radius) }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::new(LOUPE_MIN_VIEWPORT[0], LOUPE_MIN_VIEWPORT[1])
    }

    fn sense(&self) -> Sense {
        if self.can_pan { Sense::HOVER | Sense::DRAG } else { Sense::HOVER }
    }

    fn update(&mut self, cx: &mut Cx<'_, Self>) {
        if !self.can_pan {
            return;
        }
        let press = *cx.press();
        match press.phase {
            PressPhase::DragStart { start, pos, .. } => {
                self.drag_delta = Some(pos - start.origin);
                cx.cursor(Cursor::System(CursorIcon::Grabbing), CursorPriority::Drag);
            }
            PressPhase::Drag { delta, .. } => {
                self.drag_delta = Some(delta);
                cx.cursor(Cursor::System(CursorIcon::Grabbing), CursorPriority::Drag);
            }
            _ if cx.hit() => {
                cx.cursor(Cursor::System(CursorIcon::Grab), CursorPriority::Hover);
            }
            _ => {}
        }
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let rect = cx.rect;
        ui.draw().data_fill(&PaintShape::Rect(rect, cx.style.corners), DataColor::new(self.paper));
        if let Some((tex, shown_size)) = self.picture {
            let img_rect = Rect::from_center_size(rect.center(), shown_size);
            if img_rect.is_finite() {
                ui.clipped(rect, |ui| {
                    ui.content_painter(cx).picture(tex.id, img_rect, tex.uv(), 0.0);
                });
            }
        }
        ui.draw().stroke(&PaintShape::Rect(rect, cx.style.corners), StrokeWidth::Thin, cx.style.border);
    }

    fn finish(self, cx: Cx<'_, Self>) -> LoupeViewportOutput {
        LoupeViewportOutput { response: cx.response.clone(), drag_delta: self.drag_delta }
    }
}

impl PdfCraftApp {
    /// Current loupe magnification factor (`1.0..=8.0`).
    pub fn loupe_zoom_factor(&self) -> f32 {
        self.loupe.as_ref().map_or(self.loupe_zoom, |l| l.zoom)
    }

    /// Toggles the Loupe magnifier tool on or off.
    pub fn toggle_loupe(&mut self) {
        if self.loupe.is_some() {
            self.close_loupe();
        } else {
            self.open_loupe();
        }
    }

    /// Activates the Loupe tool and opens its floating window on the active document.
    pub fn open_loupe(&mut self) {
        let Some((index, doc_id)) = self.active_ids() else {
            return;
        };
        let Some(view) = self.views.get(index) else {
            return;
        };
        let page = view.current();
        let target = self
            .session
            .get(doc_id)
            .and_then(|d| d.info.pages.get(page))
            .map(|p| (page, clamp_target([p.width * 0.5, p.height * 0.5], [p.width, p.height])));
        if let Some(existing) = self.loupe.as_mut() {
            if existing.doc_id != Some(doc_id) {
                existing.doc_id = Some(doc_id);
                existing.target = target;
            }
        } else {
            self.loupe = Some(LoupeState::new(Some(doc_id), target, self.loupe_zoom));
        }
        self.quick_tool = QuickTool::Loupe;
    }

    /// Ends the Loupe tool and closes its floating window.
    pub fn close_loupe(&mut self) {
        self.loupe = None;
        if self.quick_tool == QuickTool::Loupe {
            self.quick_tool = QuickTool::Select;
        }
    }

    /// Sets the loupe target to `point` (in page view space) on 0-based `page` of the active
    /// document, clamping to the page bounds. Ignores out-of-range pages.
    pub fn set_loupe_target(&mut self, page: usize, point: [f32; 2]) {
        let Some((_, doc_id)) = self.active_ids() else {
            return;
        };
        let Some(page_size) = self.session.get(doc_id).and_then(|d| d.info.pages.get(page)).map(|p| [p.width, p.height]) else {
            if let Some(loupe) = self.loupe.as_mut() {
                loupe.clear_target();
            }
            return;
        };
        if let Some(loupe) = self.loupe.as_mut() {
            loupe.doc_id = Some(doc_id);
            loupe.set_target(page, point, page_size);
        } else {
            let mut state = LoupeState::new(Some(doc_id), None, self.loupe_zoom);
            state.set_target(page, point, page_size);
            self.loupe = Some(state);
        }
    }

    /// Sets the loupe zoom factor, clamped to `1.0..=8.0`.
    pub fn set_loupe_zoom(&mut self, zoom: f32) {
        let clamped = clamp_zoom(zoom);
        self.loupe_zoom = clamped;
        if let Some(loupe) = self.loupe.as_mut() {
            loupe.set_zoom(clamped);
        }
    }

    /// Steps the loupe zoom up (`larger == true`) or down (`larger == false`) along the viewer's
    /// zoom ladder, clamped to `1.0..=8.0`.
    pub fn step_loupe_zoom(&mut self, larger: bool) {
        let next = step_zoom(self.loupe_zoom_factor(), larger);
        self.set_loupe_zoom(next);
    }

    /// Keeps the loupe consistent with the active tool and document so stale content is never
    /// shown or reported: ends the loupe when another tool is chosen or the document closes, and
    /// clears its target if the active document changed or the target page no longer exists.
    pub(crate) fn settle_loupe(&mut self) {
        if self.quick_tool != QuickTool::Loupe {
            self.loupe = None;
            return;
        }
        let Some((_, doc_id)) = self.active_ids() else {
            self.close_loupe();
            return;
        };
        let Some(doc) = self.session.get(doc_id) else {
            self.close_loupe();
            return;
        };
        let Some(loupe) = self.loupe.as_mut() else {
            return;
        };
        if loupe.doc_id != Some(doc_id) {
            loupe.doc_id = Some(doc_id);
            loupe.clear_target();
            return;
        }
        if let Some((page, point)) = loupe.target {
            if let Some(info) = doc.info.pages.get(page) {
                loupe.target = Some((page, clamp_target(point, [info.width, info.height])));
            } else {
                loupe.clear_target();
            }
        }
    }
}

/// Shows the floating Loupe window while the Loupe tool is active (hidden in full screen).
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    app.settle_loupe();
    if app.full_screen_document() {
        return;
    }
    let Some((active_index, doc_id)) = app.active_ids() else {
        return;
    };
    let Some(loupe_snapshot) = app.loupe.clone() else {
        return;
    };
    let Some(view) = app.views.get(active_index) else {
        return;
    };
    let Some(doc) = app.session.get(doc_id) else {
        return;
    };
    let rotation = view.view.rotation;
    let paper = app
        .session
        .reading_colors(doc_id)
        .and_then(|c| c.params)
        .map_or(Color32::WHITE, |p| Color32::from_rgb(p.background[0], p.background[1], p.background[2]));

    // Prepare the magnified tile through the same `DocPages` (`RenderConfig::on_screen`) path.
    let target_info = loupe_snapshot.target.and_then(|(page, point)| {
        let info = doc.info.pages.get(page)?;
        Some((page, point, [info.width, info.height]))
    });
    let picture = target_info.and_then(|(page, point, page_size)| {
        if page_size[0] <= 0.0 || page_size[1] <= 0.0 {
            return None;
        }
        let aperture = aperture_rect_for_rotation(point, page_size, loupe_snapshot.zoom, loupe_snapshot.viewport_size, rotation);
        let [u0, v0, u1, v1] = turned_norm_region(aperture, page_size, rotation);
        let region = Rect::from_min_max(pos2(u0, v0), pos2(u1, v1));
        let longest = loupe_snapshot.viewport_size[0].max(loupe_snapshot.viewport_size[1]).max(1.0);
        let request = ThumbRequest::for_cell(longest, ui.ctx().pixels_per_point()).region(region);
        let store = Thumbnails::<(DocId, usize)>::in_context(
            ui.ctx(),
            "loupe-magnifier",
            ThumbnailOptions::new().max_side(LOUPE_MAX_TEXTURE_SIDE).lanes(1).jobs(app.host.jobs_shared().clone()),
        );
        let source = LoupeSource { pages: DocPages::new(&app.session, doc_id).rotated(rotation).revisions(&view.revisions) };
        let state = store.state(ui.ctx(), &source, &(doc_id, page), &request, Priority::VISIBLE);
        // Never panic or show stale content when rendering fails: leave the area empty.
        let tex = match state {
            TextureState::Ready(t) | TextureState::Loading { stand_in: Some(t), .. } => t,
            _ => return None,
        };
        let ap_w = (aperture[2] - aperture[0]).max(1.0);
        let ap_h = (aperture[3] - aperture[1]).max(1.0);
        let (shown_w, shown_h) = if rotation % 180 == 90 { (ap_h, ap_w) } else { (ap_w, ap_h) };
        let scale = clamp_zoom(loupe_snapshot.zoom) * PT;
        Some((tex, vec2(shown_w * scale, shown_h * scale)))
    });

    let measuring = craft::ui::runtime::measure::active(ui.ctx());
    let mut open = true;
    let mut zoom = loupe_snapshot.zoom;
    let mut drag_delta: Option<Vec2> = None;
    let mut measured_viewport: Option<[f32; 2]> = None;

    Dialog::new(tl!("Loupe tool"))
        .opened(Bind::bound(&mut open))
        .modeless()
        .resizable(true)
        .overlay(|overlay| overlay.size(LOUPE_WINDOW_SIZE).min_size(LOUPE_WINDOW_MIN_SIZE).body_scroll(false))
        .show(ui, |ui, _| {
            Row::new().show(ui, |ui, _| {
                if clicked(&IconButton::new(IconId::new("zoom_out")).label(tl!("Zoom out")).enabled(zoom > LOUPE_ZOOM_MIN).show(ui)) {
                    zoom = step_zoom(zoom, false);
                }
                let mut zoom_percent = f64::from(zoom * 100.0);
                let percent_text = format!("{:.0}%", zoom_percent);
                Slider::new(Bind::bound(&mut zoom_percent), f64::from(LOUPE_ZOOM_MIN * 100.0)..=f64::from(LOUPE_ZOOM_MAX * 100.0))
                    .step(25.0)
                    .label(tl!("Zoom"))
                    .value_text(percent_text.clone())
                    .short(true)
                    .show(ui);
                zoom = clamp_zoom((zoom_percent / 100.0) as f32);
                if clicked(&IconButton::new(IconId::new("zoom_in")).label(tl!("Zoom in")).enabled(zoom < LOUPE_ZOOM_MAX).show(ui)) {
                    zoom = step_zoom(zoom, true);
                }
                Text::new(format!("{:.0}%", zoom * 100.0)).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
            });
            Divider::horizontal().show(ui);

            let avail = ui.available_rect_before_wrap();
            let vp_size = vec2(avail.width().max(LOUPE_MIN_VIEWPORT[0]), avail.height().max(LOUPE_MIN_VIEWPORT[1]));
            let vp_rect = Rect::from_min_size(avail.min, vp_size);
            if !measuring {
                measured_viewport = Some([vp_size.x, vp_size.y]);
            }
            let out = LoupeViewport::new(paper, picture, target_info.is_some()).show_at(ui, vp_rect);
            if !measuring {
                drag_delta = out.drag_delta;
            }
        });

    if !measuring && !open {
        app.close_loupe();
        return;
    }
    app.set_loupe_zoom(zoom);
    if let Some(loupe) = app.loupe.as_mut() {
        if let Some(vp) = measured_viewport {
            loupe.viewport_size = vp;
        }
        if let (Some(delta), Some((page, point, page_size))) = (drag_delta, target_info) {
            let next = pan_target(point, [delta.x, delta.y], page_size, loupe.zoom, rotation);
            loupe.set_target(page, next, page_size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loupe_zoom_clamps_to_supported_range() {
        assert_eq!(clamp_zoom(0.25), LOUPE_ZOOM_MIN);
        assert_eq!(clamp_zoom(1.0), 1.0);
        assert_eq!(clamp_zoom(3.5), 3.5);
        assert_eq!(clamp_zoom(8.0), 8.0);
        assert_eq!(clamp_zoom(16.0), LOUPE_ZOOM_MAX);
        assert_eq!(clamp_zoom(f32::NEG_INFINITY), LOUPE_ZOOM_MIN);
        assert_eq!(clamp_zoom(f32::INFINITY), LOUPE_ZOOM_MAX);
        assert_eq!(clamp_zoom(f32::NAN), LOUPE_ZOOM_DEFAULT);
    }

    #[test]
    fn loupe_zoom_steps_follow_viewer_ladder_and_clamp() {
        let mut z = LOUPE_ZOOM_MIN;
        let mut up = vec![z];
        for _ in 0..12 {
            let next = step_zoom(z, true);
            if next == z {
                break;
            }
            z = next;
            up.push(z);
        }
        assert_eq!(*up.first().unwrap(), 1.0);
        assert_eq!(*up.last().unwrap(), 8.0);
        assert!(up.windows(2).all(|w| w[1] > w[0]), "strictly increasing ladder: {up:?}");
        assert_eq!(step_zoom(8.0, true), 8.0, "clamps at 800%");

        let mut down = vec![z];
        for _ in 0..12 {
            let next = step_zoom(z, false);
            if next == z {
                break;
            }
            z = next;
            down.push(z);
        }
        assert_eq!(*down.last().unwrap(), 1.0);
        assert!(down.windows(2).all(|w| w[1] < w[0]), "strictly decreasing ladder: {down:?}");
        assert_eq!(step_zoom(1.0, false), 1.0, "clamps at 100%");
    }

    #[test]
    fn loupe_target_clamps_to_page_bounds() {
        let page = [612.0, 792.0];
        assert_eq!(clamp_target([100.0, 200.0], page), [100.0, 200.0]);
        assert_eq!(clamp_target([-50.0, 900.0], page), [0.0, 792.0]);
        assert_eq!(clamp_target([1000.0, -10.0], page), [612.0, 0.0]);
        assert_eq!(clamp_target([f32::NAN, f32::INFINITY], page), [306.0, 792.0]);
        assert_eq!(clamp_target([50.0, 50.0], [-10.0, f32::NAN]), [0.0, 0.0]);
    }

    #[test]
    fn loupe_aperture_rect_shrinks_with_zoom_and_stays_inside_page() {
        let page = [612.0, 792.0];
        let win = [280.0, 180.0];
        let at_200 = aperture_rect([306.0, 396.0], page, 2.0, win);
        let at_400 = aperture_rect([306.0, 396.0], page, 4.0, win);

        let w200 = at_200[2] - at_200[0];
        let h200 = at_200[3] - at_200[1];
        let w400 = at_400[2] - at_400[0];
        let h400 = at_400[3] - at_400[1];

        assert!(w400 < w200 && h400 < h200, "higher zoom gives a smaller aperture: {at_400:?} vs {at_200:?}");
        assert!((w200 / w400 - 2.0).abs() < 0.01, "doubling zoom halves aperture width");
        assert!((h200 / h400 - 2.0).abs() < 0.01, "doubling zoom halves aperture height");

        // Larger window gives a wider aperture.
        let wider = aperture_rect([306.0, 396.0], page, 4.0, [400.0, 180.0]);
        assert!(wider[2] - wider[0] > w400);

        // Near edges and outside the page, the aperture stays inside [0..pw, 0..ph].
        for target in [[-100.0, -100.0], [0.0, 0.0], [612.0, 792.0], [999.0, 999.0]] {
            let r = aperture_rect(target, page, 2.0, win);
            assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= page[0] && r[3] <= page[1], "out of bounds for {target:?}: {r:?}");
            assert!(r[2] > r[0] && r[3] > r[1]);
        }

        // Small page smaller than the requested window clamps to the full page.
        let small = aperture_rect([50.0, 50.0], [100.0, 80.0], 1.0, [400.0, 400.0]);
        assert_eq!(small, [0.0, 0.0, 100.0, 80.0]);
    }

    #[test]
    fn loupe_pan_moves_target_opposite_to_drag_and_clamps() {
        let page = [612.0, 792.0];
        let start = [300.0, 400.0];
        let moved = pan_target(start, [40.0, -20.0], page, 2.0, 0);
        assert!(moved[0] < start[0], "dragging right moves target left");
        assert!(moved[1] > start[1], "dragging up moves target down");

        let clamped = pan_target([10.0, 10.0], [5000.0, 5000.0], page, 1.0, 0);
        assert_eq!(clamped, [0.0, 0.0]);
    }

    #[test]
    fn loupe_parse_zoom_percent_accepts_valid_and_rejects_out_of_range() {
        assert_eq!(parse_zoom_percent("100"), Some(1.0));
        assert_eq!(parse_zoom_percent("250%"), Some(2.5));
        assert_eq!(parse_zoom_percent(" 800 % "), Some(8.0));
        assert_eq!(parse_zoom_percent("99"), None);
        assert_eq!(parse_zoom_percent("801"), None);
        assert_eq!(parse_zoom_percent("NaN"), None);
        assert_eq!(parse_zoom_percent("fast"), None);
    }

    #[test]
    fn loupe_rotation_swaps_aperture_aspect_and_normalises_inside_unit_square() {
        let page = [600.0, 800.0];
        let win = [300.0, 150.0];
        let r0 = aperture_rect_for_rotation([300.0, 400.0], page, 4.0, win, 0);
        let r90 = aperture_rect_for_rotation([300.0, 400.0], page, 4.0, win, 90);
        assert!((r0[2] - r0[0]) > (r0[3] - r0[1]), "unrotated wide window gives wider aperture");
        assert!((r90[3] - r90[1]) > (r90[2] - r90[0]), "90-degree rotated wide window gives taller unturned aperture");

        for rot in [0, 90, 180, 270] {
            let ap = aperture_rect_for_rotation([120.0, 240.0], page, 4.0, win, rot);
            let n = turned_norm_region(ap, page, rot);
            assert!(n[0] >= 0.0 && n[1] >= 0.0 && n[2] <= 1.0 && n[3] <= 1.0, "rot {rot}: {n:?}");
            assert!(n[2] > n[0] && n[3] > n[1], "non-empty region at rot {rot}: {n:?}");
        }
    }
}
