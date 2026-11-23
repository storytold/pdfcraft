//! Cached page and stamp layers: a stamp (an image signature, a Fill & Sign mark, any other stamp)
//! follows each pointer frame, moved or resized, without editing the PDF. Any other comment
//! follows the pointer while it is moved: its own layer is drawn where it goes.

use craft::behavior::{Input, Sense};
use craft::review::AnnotationEdit;
use craft::ui::contract::{Access, Common, Component, Cx, MeasureCx, PaintUi, Role};
use craft::ui::primitives::Draw;
use craft::ui::primitives::textures::{CachedTexture, Revision, TextureStore, TextureStoreOptions};
use craft::ui::style::{ComponentStyle, Metrics, StyleCx};
use egui::{ColorImage, Pos2, Rect, Response, TextureId, Vec2, pos2, vec2};
use pdfcraft_engine::{Document, Edit};
use pdfcraft_render::{PageInfo, RenderPool, RenderRequest, Tile};

use super::PageXform;
use crate::QuickTool;
use crate::comments::{CommentView, PageCx, PageMap, id_of, key_of};

/// The longest side, in pixels, a stamp's own layer is rendered at.
const MAX_STAMP_SIDE: f32 = 2048.0;

/// Which of a drag's pictures a texture is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Layer {
    /// An image signature's own image.
    Image,
    /// The page without the stamp.
    Background,
    /// A stamp rendered alone.
    Stamp,
}

/// What names one of the drag's textures in the store: the document's edit generation, the page, the
/// comment's index and the picture.
type DragKey = (u64, usize, usize, Layer);

/// The store of the drag's textures. Its pictures are named by the app and change as the view zooms, so they
/// are not an icon or a thumbnail.
fn drag_store(ctx: &egui::Context) -> TextureStore<DragKey> {
    TextureStore::in_context(ctx, "signature-drag", TextureStoreOptions::frames())
}

#[derive(Default)]
pub(crate) struct SignatureDrag {
    key: Option<(u64, usize, usize)>,
    aspect_ratio: Option<f32>,
    layers: Option<Layers>,
    /// The comment being moved that has no stamp layers: (edit generation, page, index), and
    /// the comment alone, drawn over the page where it goes.
    moving: Option<((u64, usize, usize), Moving)>,
    /// The comment being moved whose layer couldn't be made (nothing to draw, or a document that
    /// can't be read): only its frame follows the pointer, and the layer isn't made again on
    /// every frame of the drag.
    no_layer: Option<(u64, usize, usize)>,
    /// Textures of layers that were dropped where no context was at hand, freed by the next call that has one.
    retired: Vec<DragKey>,
}

struct Moving {
    /// The comment's layer: `from` is its box when the move started.
    layer: StampLayer,
    /// Released and applied: keep it where it went until the page shows it there.
    settling: bool,
}

struct Layers {
    front: Front,
    renderer: RenderPool,
    /// The page without the stamp, up to one whole-page texture.
    background: Option<CachedTexture>,
    tag: Option<u64>,
    received_tag: Option<u64>,
    /// Keep the final position visible until the normal page raster catches up.
    settling: bool,
    /// Every key this drag put in the store, to free them with the layers.
    keys: Vec<DragKey>,
}

/// What is drawn over the page without the stamp.
enum Front {
    /// An image signature's own image, sharp at every zoom.
    Image {
        texture: CachedTexture,
        opacity: f32,
        /// The page /Rotate the PDF draws the image turned back by.
        turn: i64,
    },
    /// Any other stamp, rendered alone. Moving or resizing a stamp only changes its `/Rect`,
    /// which its appearance is stretched to fit, so its pixels are stretched with the frame.
    Stamp(Box<StampLayer>),
}

struct StampLayer {
    renderer: RenderPool,
    /// The stamp's rectangle (user space) in the document it is rendered from.
    from: [f32; 4],
    tag: Option<u64>,
    /// The rendered stamp: its tag, and the view-space box its pixels cover.
    pixels: Option<(u64, CachedTexture, [f32; 4])>,
    /// The tag whose render failed: not waited for, and asked for again only at another scale.
    failed: Option<u64>,
    /// The key the pixels were put in the store under.
    stored: Option<DragKey>,
}

impl SignatureDrag {
    pub fn prepare(&mut self, ctx: &egui::Context, doc: &Document, selected: Option<(usize, usize)>, scale: f32) {
        self.forget_retired(ctx);
        let key = selected.map(|(p, i)| (doc.edit_generation(), p, i));
        if self.key != key {
            self.key = key;
            self.aspect_ratio = None;
            self.retire_layers();
            if let Some((generation, page, index)) = key
                && let Some(a) = doc.info.annotations.iter().find(|a| a.page == page && a.index == index && a.subtype == "Stamp" && !a.locked)
            {
                if let Ok(Some(preview)) = doc.image_signature_preview(page, index) {
                    let [width, height] = preview.image.size();
                    let ratio = width as f32 / height as f32;
                    // As displayed, the image is turned by the page's rotation less the turn it is drawn back by.
                    let shown = doc.info.pages.get(page).map_or(0, |p| i64::from(p.rotation)) - preview.turn;
                    self.aspect_ratio = Some(if shown.rem_euclid(180) == 0 { ratio } else { ratio.recip() });
                    let image_key = (generation, page, index, Layer::Image);
                    let image = ColorImage::from_rgba_unmultiplied(preview.image.size(), preview.image.rgba());
                    let texture = drag_store(ctx).put(ctx, image_key, Revision::new(generation), image);
                    let front = Front::Image { texture, opacity: preview.opacity, turn: preview.turn };
                    self.layers = Some(Layers::new(front, preview.renderer(), vec![image_key]));
                } else if let Ok(Some(preview)) = doc.stamp_preview(page, index) {
                    let stamp = StampLayer { renderer: preview.stamp_renderer(), from: a.rect, tag: None, pixels: None, failed: None, stored: None };
                    self.layers = Some(Layers::new(Front::Stamp(Box::new(stamp)), preview.background_renderer(), Vec::new()));
                }
            }
        }
        let (Some((generation, page, index)), Some(layers)) = (self.key, self.layers.as_mut()) else { return };
        // The background matches the page view up to one whole-page texture; past that the view
        // tiles and this preview stays at that cap.
        let Some(p) = doc.info.pages.get(page) else { return };
        let background_scale = scale.min(crate::canvas::BASE_SIDE / p.width.max(p.height).max(1.0));
        let tag = (background_scale * 1000.0) as u64;
        if layers.tag != Some(tag) {
            layers.renderer.set_queue(vec![RenderRequest { page, scale: background_scale, tag, preview: true, ..Default::default() }]);
            layers.tag = Some(tag);
        }
        if let Some(out) = layers.renderer.try_recv()
            && out.request.tag == tag
        {
            let background_key = (generation, page, index, Layer::Background);
            let uploaded = if out.error.is_none() {
                let image = ColorImage::from_rgba_premultiplied([out.width as usize, out.height as usize], &out.rgba);
                Some(drag_store(ctx).put(ctx, background_key, Revision::new(tag), image))
            } else {
                None
            };
            // Leave the ordinary page raster usable if an unsupported PDF can't preview.
            match uploaded {
                Some(texture) => {
                    layers.received_tag = Some(tag);
                    layers.background = Some(texture);
                    if !layers.keys.contains(&background_key) {
                        layers.keys.push(background_key);
                    }
                }
                None => {
                    self.retire_layers();
                    self.forget_retired(ctx);
                    return;
                }
            }
        }
        let mut pending = layers.received_tag != Some(tag);
        // The stamp itself is small: it stays sharp at every zoom.
        if let Front::Stamp(stamp) = &mut layers.front {
            match stamp.update(ctx, (generation, page, index, Layer::Stamp), page, p, scale) {
                Some(waiting) => pending |= waiting,
                // Nothing to lay over the page: the frame alone follows the pointer, as before.
                None => {
                    self.retire_layers();
                    self.forget_retired(ctx);
                    return;
                }
            }
        }
        if pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub fn contains(&self, page: usize, index: usize) -> bool {
        self.key.is_some_and(|(_, p, i)| (p, i) == (page, index)) && self.layers.is_some()
    }

    /// Use the embedded image's proportions, even after an edge handle stretched its rectangle: its
    /// width over its height as the page is displayed (before the view's own rotation).
    pub fn aspect_ratio(&self, page: usize, index: usize) -> Option<f32> {
        self.key.filter(|(_, p, i)| (*p, *i) == (page, index)).and(self.aspect_ratio)
    }

    /// Movement/resizing only change this stamp's geometry, so its layers are reusable.
    pub fn committed(&mut self, edit: &Edit, generation: u64) {
        if let Some((key, m)) = self.moving.as_mut()
            && matches!(edit, Edit::MoveAnnotation { page, index, .. } if (*page, *index) == (key.1, key.2))
        {
            key.0 = generation;
            m.settling = true;
        } else {
            self.retire_moving();
        }
        if let Edit::MoveAnnotation { page, index, .. } | Edit::ResizeAnnotation { page, index, .. } = edit
            && self.contains(*page, *index)
        {
            self.key = Some((generation, *page, *index));
            if let Some(layers) = &mut self.layers {
                layers.settling = true;
            }
        } else {
            self.retire_layers();
            self.key = None;
            self.aspect_ratio = None;
            self.no_layer = None;
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.key.is_some_and(|(_, p, _)| p == page)
            && let Some(layers) = &mut self.layers
        {
            layers.settling = false;
        }
    }

    /// Drops the selected stamp's layers, keeping their textures' keys to free.
    fn retire_layers(&mut self) {
        if let Some(layers) = self.layers.take() {
            self.retired.extend(layers.keys);
            self.retired.extend(layers.front.stored());
        }
    }

    /// Drops the moved comment's layer, keeping its texture's key to free.
    fn retire_moving(&mut self) {
        if let Some((_, moving)) = self.moving.take() {
            self.retired.extend(moving.layer.stored);
        }
    }

    /// Frees the textures of dropped layers.
    fn forget_retired(&mut self, ctx: &egui::Context) {
        if self.retired.is_empty() {
            return;
        }
        let store = drag_store(ctx);
        for key in self.retired.drain(..) {
            store.forget(ctx, &key);
        }
    }

    /// Whether the comments tool is moving or resizing the comment `index` of `page` now.
    fn dragging(in_flight: Option<&AnnotationEdit>, page: usize, index: usize) -> bool {
        let id = id_of(page, index);
        match in_flight {
            Some(AnnotationEdit::Move { ids, .. }) => ids.contains(&id),
            Some(AnnotationEdit::Resize { id: resized, .. }) => *resized == id,
            _ => false,
        }
    }

    /// The comment being dragged, drawn from its layers under `tool`, the quick tool the page sees: only Select and the
    /// Fill & Sign tools move comments.
    pub fn paint(&self, draw: &Draw<'_>, cx: &PageCx<'_>, map: &PageMap<'_>, tool: QuickTool, cv: &CommentView, pending: Option<&Edit>) {
        let (Some((_, page, index)), Some(layers)) = (self.key, &self.layers) else { return };
        if page != cx.page || !matches!(tool, QuickTool::Select | QuickTool::Fill(_)) || cx.hidden {
            return;
        }
        let in_flight = cv.tool.in_flight();
        let dragging = Self::dragging(in_flight.as_ref(), page, index);
        let released = matches!(pending, Some(Edit::MoveAnnotation { page: p, index: i, .. } | Edit::ResizeAnnotation { page: p, index: i, .. }) if (*p, *i) == (page, index));
        if !(dragging || released || layers.settling) {
            return;
        }
        let (Some(background), Some(a), Some(p)) = (&layers.background, cx.get(index), cx.info.pages.get(page)) else { return };
        if let Front::Stamp(stamp) = &layers.front
            && stamp.pixels.is_none()
        {
            return;
        }
        let rect = cx.adjusted_rect(map, a, in_flight, pending);
        draw.clipped(cx.xf.rect, |draw| {
            paint_quad(draw, page_corners(cx.xf), background.id);
            match &layers.front {
                Front::Image { texture, opacity, turn } => {
                    draw.faded(*opacity, |draw| paint_quad(draw, user_image_corners(cx.xf, p, cx.rect_to_user(rect), *turn), texture.id));
                }
                Front::Stamp(stamp) => stamp.paint(draw, cx, page, rect),
            }
        });
    }

    /// The page of the comment being moved, whose raster [`Self::prepare_move`] asks about.
    pub fn moving_page(&self) -> Option<usize> {
        self.moving.as_ref().map(|((_, page, _), _)| *page)
    }

    /// While a comment without stamp layers is moved, its own layer: rendered when the move
    /// starts, kept after release until the page's raster shows it where it went (`stale`: the
    /// page is still drawn as before its last change).
    pub fn prepare_move(&mut self, ctx: &egui::Context, doc: &Document, cv: &CommentView, pending: Option<&Edit>, stale: bool, scale: f32) {
        self.forget_retired(ctx);
        let generation = doc.edit_generation();
        let dragged = match cv.tool.in_flight() {
            Some(AnnotationEdit::Move { ids, .. }) => {
                ids.first().map(|id| key_of(*id)).filter(|(page, index)| !self.contains(*page, *index)).map(|(page, index)| (generation, page, index))
            }
            _ => None,
        };
        let keep = |key: (u64, usize, usize), m: &Moving| {
            let (_, page, index) = key;
            dragged == Some(key)
                || matches!(pending, Some(Edit::MoveAnnotation { page: p, index: i, .. }) if (*p, *i) == (page, index))
                || m.settling && stale
        };
        if self.moving.as_ref().is_some_and(|(key, m)| !keep(*key, m)) {
            self.retire_moving();
        }
        if self.no_layer != dragged {
            self.no_layer = None;
        }
        if self.moving.is_none()
            && self.no_layer.is_none()
            && let Some(key @ (_, page, index)) = dragged
            && let Some(a) = doc.info.annotations.iter().find(|a| a.page == page && a.index == index && !a.locked)
        {
            // Made once per drag: it copies and writes out the whole document.
            match doc.comment_layer(page, &[index]) {
                Ok(Some(alone)) => {
                    // The box it is drawn in: a comment kept upright on a turned page (NoRotate)
                    // isn't drawn in its /Rect, and the frame it follows is that box too.
                    let rotation = doc.info.pages.get(page).map_or(0, |p| p.rotation);
                    let layer =
                        StampLayer { renderer: alone.renderer(), from: a.shown_rect(rotation), tag: None, pixels: None, failed: None, stored: None };
                    self.moving = Some((key, Moving { layer, settling: false }));
                }
                _ => self.no_layer = Some(key),
            }
        }
        let Some(((generation, page, index), m)) = self.moving.as_mut() else { return };
        let Some(p) = doc.info.pages.get(*page) else { return };
        match m.layer.update(ctx, (*generation, *page, *index, Layer::Stamp), *page, p, scale) {
            Some(pending) => {
                if pending {
                    ctx.request_repaint_after(std::time::Duration::from_millis(16));
                }
            }
            // Nothing to lay over the page at this scale (no box, or the render failed): the
            // frame alone follows the pointer, as before. The layer stays, so another scale
            // renders it again without making it anew.
            None => m.layer.pixels = None,
        }
    }

    /// The comment being moved, drawn where it goes (as far as its frame has gone).
    pub fn paint_move(&self, draw: &Draw<'_>, cx: &PageCx<'_>, map: &PageMap<'_>, cv: &CommentView, pending: Option<&Edit>) {
        let Some(((_, page, index), m)) = &self.moving else { return };
        if *page != cx.page || cx.hidden {
            return;
        }
        let Some(a) = cx.get(*index) else { return };
        // As far as its frame has gone from where the move started (once applied, its box is
        // already where it went).
        let rect = cx.adjusted_rect(map, a, cv.tool.in_flight(), pending);
        let from = cx.xf.user_rect(cx.info, *page, m.layer.from);
        draw.clipped(cx.xf.rect, |draw| m.layer.paint(draw, cx, *page, from.translate(rect.min - from.min)));
    }
}

/// The style of the drag's layers: they paint the document's own pixels and nothing of the theme.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub struct DragLayerStyle {
    #[style(metrics)]
    metrics: Metrics,
}

/// The comment being dragged over one page, drawn from the drag's layers where it goes: the selected stamp's
/// ([`SignatureDrag::paint`]), then any other comment being moved ([`SignatureDrag::paint_move`]). Paint only.
// `unregistered`: it needs a live document and a drag under way, so the suite has no sample of it.
#[derive(craft::ui::contract::Component)]
#[component(canvas_control, unregistered)]
pub(crate) struct DragLayer<'a> {
    #[common]
    common: Common,
    drag: &'a SignatureDrag,
    cx: &'a PageCx<'a>,
    map: &'a PageMap<'a>,
    tool: QuickTool,
    cv: &'a CommentView,
    pending: Option<&'a Edit>,
}

impl<'a> DragLayer<'a> {
    pub(crate) fn new(
        drag: &'a SignatureDrag,
        cx: &'a PageCx<'a>,
        map: &'a PageMap<'a>,
        tool: QuickTool,
        cv: &'a CommentView,
        pending: Option<&'a Edit>,
    ) -> Self {
        DragLayer { common: Common::new(), drag, cx, map, tool, cv, pending }
    }
}

impl Component for DragLayer<'_> {
    const ROLE: Role = Role::GROUP;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    const SHOWS_LABEL: bool = false;
    type Style = DragLayerStyle;
    type Output = Response;

    fn style(_: &StyleCx<'_>, _: DragLayerStyleKey) -> DragLayerStyle {
        DragLayerStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    fn sense(&self) -> Sense {
        Sense::empty()
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, _: &Cx<'_, Self>) {
        let draw = ui.draw();
        self.drag.paint(&draw, self.cx, self.map, self.tool, self.cv, self.pending);
        self.drag.paint_move(&draw, self.cx, self.map, self.cv, self.pending);
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

impl Front {
    /// The key a stamp front's pixels were put in the store under.
    fn stored(&self) -> Option<DragKey> {
        match self {
            Front::Image { .. } => None,
            Front::Stamp(stamp) => stamp.stored,
        }
    }
}

impl StampLayer {
    /// Render the layer's tile at `scale` (or less, within [`MAX_STAMP_SIDE`]) and take it when
    /// it arrives, stored under `key`. `Some(true)` while the pixels at this scale are still on
    /// their way, `None` when there is nothing to draw.
    fn update(&mut self, ctx: &egui::Context, key: DragKey, page: usize, p: &PageInfo, scale: f32) -> Option<bool> {
        let (scale, tile) = stamp_tile(p, self.from, scale)?;
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            self.renderer.set_queue(vec![RenderRequest { page, scale, tag, tile: Some(tile), ..Default::default() }]);
            self.tag = Some(tag);
        }
        if self.failed == Some(tag) {
            return None;
        }
        if let Some(out) = self.renderer.try_recv()
            && out.request.tag == tag
        {
            let (Some(t), None) = (out.request.tile, &out.error) else {
                self.failed = Some(tag);
                return None;
            };
            let s = out.request.scale;
            let area = [t.x as f32 / s, t.y as f32 / s, t.x.saturating_add(t.w) as f32 / s, t.y.saturating_add(t.h) as f32 / s];
            let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
            let store = drag_store(ctx);
            let texture = store.put(ctx, key, Revision::new(tag), image);
            // The edit generation moved on while the layer was kept: its pixels are under the new key now.
            if let Some(old) = self.stored.filter(|old| *old != key) {
                store.forget(ctx, &old);
            }
            self.stored = Some(key);
            self.pixels = Some((tag, texture, area));
        }
        Some(self.pixels.as_ref().is_none_or(|(received, ..)| *received != tag))
    }

    /// The layer's pixels where its box goes on screen, `rect`: moved, and stretched as its box
    /// is from `from` to `rect`.
    fn paint(&self, draw: &Draw<'_>, cx: &PageCx<'_>, page: usize, rect: Rect) {
        let Some((_, texture, area)) = &self.pixels else { return };
        let from = cx.xf.user_rect(cx.info, page, self.from);
        if from.width() < 0.01 || from.height() < 0.01 {
            return;
        }
        let stretch = vec2(rect.width() / from.width(), rect.height() / from.height());
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
            let (x, y) = (area[0] + u * (area[2] - area[0]), area[1] + v * (area[3] - area[1]));
            let at = cx.xf.norm_to_screen(x / cx.xf.pw, y / cx.xf.ph);
            rect.min + (at - from.min) * stretch
        });
        paint_quad(draw, corners, texture.id);
    }
}

impl Layers {
    fn new(front: Front, renderer: RenderPool, keys: Vec<DragKey>) -> Self {
        Self { front, renderer, background: None, tag: None, received_tag: None, settling: false, keys }
    }
}

/// The screen corners of the whole page: top-left, top-right, bottom-right, bottom-left of its raster.
fn page_corners(xf: &PageXform) -> [Pos2; 4] {
    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| xf.norm_to_screen(u, v))
}

/// The screen corners an image placed in the user-space `rect` covers, turned back by `turn` (the page
/// /Rotate it was placed for) as a custom stamp's appearance draws it, so with the page's own rotation it
/// reads upright as displayed: the image's top-left, top-right, bottom-right and bottom-left.
fn user_image_corners(xf: &PageXform, p: &PageInfo, rect: [f64; 4], turn: i64) -> [Pos2; 4] {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let (shown_w, shown_h) = if turn % 180 == 0 { (w, h) } else { (h, w) };
    let [a, b, c, d, e, f] = pdfcraft_model::view_matrix_for(turn, rect);
    [(0.0_f64, 0.0_f64), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
        // The texture's (u, v), from its top-left, in the picture's upright frame, then in user space.
        let (dx, dy) = (u * shown_w, (1.0 - v) * shown_h);
        let (x, y) = (a * dx + c * dy + e, b * dx + d * dy + f);
        let q = p.user_to_view(x as f32, y as f32);
        xf.norm_to_screen(q[0] / xf.pw, q[1] / xf.ph)
    })
}

/// Paints `texture` over its four screen corners.
fn paint_quad(draw: &Draw<'_>, corners: [Pos2; 4], texture: TextureId) {
    if !corners.iter().all(|c| c.is_finite()) {
        return;
    }
    draw.texture_quad(corners, texture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
}

/// The scale and tile of the page raster that hold a stamp at `rect` (user space), a pixel to
/// spare around it for its anti-aliased edges: at `scale`, or less to keep it within
/// [`MAX_STAMP_SIDE`]. `None` for a page or stamp that has no finite box.
fn stamp_tile(p: &PageInfo, rect: [f32; 4], scale: f32) -> Option<(f32, Tile)> {
    let [a, b] = [p.user_to_view(rect[0], rect[1]), p.user_to_view(rect[2], rect[3])];
    let (x0, y0, x1, y1) = (a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1]));
    let scale = scale.min(MAX_STAMP_SIDE / (x1 - x0).max(y1 - y0).max(1.0));
    let (width, height) = ((p.width * scale).ceil(), (p.height * scale).ceil());
    if ![scale, x0, y0, x1, y1, width, height].iter().all(|v| v.is_finite()) || scale <= 0.0 {
        return None;
    }
    // Float to integer casts saturate, and each edge stays on the page.
    let (left, top) = (((x0 * scale).floor() - 1.0).max(0.0).min(width), ((y0 * scale).floor() - 1.0).max(0.0).min(height));
    let (right, bottom) = (((x1 * scale).ceil() + 1.0).min(width).max(left + 1.0), ((y1 * scale).ceil() + 1.0).min(height).max(top + 1.0));
    Some((scale, Tile { x: left as u32, y: top as u32, w: (right - left) as u32, h: (bottom - top) as u32 }))
}

#[cfg(test)]
mod tests {

    use egui::Pos2;

    use super::*;

    /// One page with one comment that has nothing to draw: a sound annotation without an
    /// appearance, for which none is built.
    fn silent() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] >>",
            "<< /Type /Annot /Subtype /Sound /Rect [50 100 70 120] /Contents (hush) >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A moved comment whose own layer can't be made is asked for once per drag, not on every
    /// pointer frame (making it copies and writes out the whole document); only its frame follows
    /// the pointer. Once the drag ends, the next one asks again. The drag is the comments tool's
    /// own, started by a real press and move on the comment in the shell.
    #[test]
    fn a_comment_with_nothing_to_draw_is_prepared_once_per_drag() {
        let bytes = silent();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = crate::PdfCraftApp::for_unit_tests();
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "silent.pdf", None, bytes.clone()).unwrap();
                app.set_option(&cc.egui_ctx, "left", "closed").unwrap();
                app.set_option(&cc.egui_ctx, "panel", "none").unwrap();
                app
            }
        });
        h.run_steps(6);
        let (centre, key): (Pos2, _) = {
            let app = h.state();
            let view = &app.views[0];
            let doc = app.session.get(view.id).expect("open");
            let a = doc.info.annotations.iter().find(|a| a.subtype == "Sound").expect("listed");
            let xf = view.page_xform(0).expect("page 0 is on screen");
            let rect = xf.user_rect(&doc.info, 0, a.rect);
            (rect.center(), (doc.edit_generation(), 0, a.index))
        };
        assert!(h.state().views[0].viewport_rect().contains(centre + vec2(40.0, 30.0)), "the drag is inside the page viewport");
        h.drag_at(centre);
        h.run_steps(2);
        h.hover_at(centre + vec2(30.0, 20.0));
        h.run_steps(2);
        assert_eq!(h.state().views[0].session.signature_drag.no_layer, Some(key), "remembered for this drag");
        assert!(h.state().views[0].session.signature_drag.moving.is_none(), "nothing to draw");
        h.hover_at(centre + vec2(40.0, 30.0));
        h.run_steps(2);
        assert_eq!(h.state().views[0].session.signature_drag.no_layer, Some(key), "and not made again");
        h.drop_at(centre + vec2(40.0, 30.0));
        h.run_steps(3);
        assert_eq!(h.state().views[0].session.signature_drag.no_layer, None, "forgotten when the drag ends");
    }
}
