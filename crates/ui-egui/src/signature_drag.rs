//! Cached page and stamp layers: a stamp (an image signature, a Fill & Sign mark, any other stamp)
//! follows each pointer frame, moved or resized, without editing the PDF.

use egui::{ColorImage, TextureHandle, TextureOptions, pos2, vec2};
use pdfcraft_engine::{Document, Edit};
use pdfcraft_render::{PageInfo, RenderPool, RenderRequest, Tile};

use crate::QuickTool;
use crate::comments::{CommentView, Gesture, PageCx};

/// The longest side, in pixels, a stamp's own layer is rendered at.
const MAX_STAMP_SIDE: f32 = 2048.0;

#[derive(Default)]
pub(crate) struct SignatureDrag {
    key: Option<(u64, usize, usize)>,
    aspect_ratio: Option<f32>,
    layers: Option<Layers>,
}

struct Layers {
    front: Front,
    renderer: RenderPool,
    background: Option<TextureHandle>,
    tag: Option<u64>,
    received_tag: Option<u64>,
    /// Keep the final position visible until the normal page raster catches up.
    settling: bool,
}

/// What is drawn over the page without the stamp.
enum Front {
    /// An image signature's own image, sharp at every zoom.
    Image {
        texture: TextureHandle,
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
    pixels: Option<(u64, TextureHandle, [f32; 4])>,
}

impl SignatureDrag {
    pub fn prepare(&mut self, ctx: &egui::Context, doc: &Document, selected: Option<(usize, usize)>, scale: f32) {
        let key = selected.map(|(p, i)| (doc.edit_generation(), p, i));
        if self.key != key {
            self.key = key;
            self.aspect_ratio = None;
            self.layers = None;
            if let Some((_, page, index)) = key
                && let Some(a) = doc.info.annotations.iter().find(|a| a.page == page && a.index == index && a.subtype == "Stamp" && !a.locked)
            {
                if let Ok(Some(preview)) = doc.image_signature_preview(page, index) {
                    let [width, height] = preview.image.size();
                    let ratio = width as f32 / height as f32;
                    // As displayed, the image is turned by the page's rotation less the turn it is drawn back by.
                    let shown = doc.info.pages.get(page).map_or(0, |p| i64::from(p.rotation)) - preview.turn;
                    self.aspect_ratio = Some(if shown.rem_euclid(180) == 0 { ratio } else { ratio.recip() });
                    let texture = ctx.load_texture(
                        "signature-drag-image",
                        ColorImage::from_rgba_unmultiplied(preview.image.size(), preview.image.rgba()),
                        TextureOptions::LINEAR,
                    );
                    let front = Front::Image { texture, opacity: preview.opacity, turn: preview.turn };
                    self.layers = Some(Layers::new(front, preview.renderer()));
                } else if let Ok(Some(preview)) = doc.stamp_preview(page, index) {
                    let stamp = StampLayer { renderer: preview.stamp_renderer(), from: a.rect, tag: None, pixels: None };
                    self.layers = Some(Layers::new(Front::Stamp(Box::new(stamp)), preview.background_renderer()));
                }
            }
        }
        let (Some((_, page, _)), Some(layers)) = (self.key, self.layers.as_mut()) else { return };
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
            if out.error.is_none() {
                layers.received_tag = Some(tag);
                layers.background = Some(ctx.load_texture(
                    "signature-drag-background",
                    ColorImage::from_rgba_premultiplied([out.width as usize, out.height as usize], &out.rgba),
                    TextureOptions::LINEAR,
                ));
            } else {
                // Leave the ordinary page raster usable if an unsupported PDF can't preview.
                self.layers = None;
                return;
            }
        }
        let mut pending = layers.received_tag != Some(tag);
        // The stamp itself is small: it stays sharp at every zoom.
        if let Front::Stamp(stamp) = &mut layers.front {
            let Some((scale, tile)) = stamp_tile(p, stamp.from, scale) else {
                // Nothing to lay over the page: the frame alone follows the pointer, as before.
                self.layers = None;
                return;
            };
            let tag = (scale * 1000.0) as u64;
            if stamp.tag != Some(tag) {
                stamp.renderer.set_queue(vec![RenderRequest { page, scale, tag, tile: Some(tile), ..Default::default() }]);
                stamp.tag = Some(tag);
            }
            if let Some(out) = stamp.renderer.try_recv()
                && out.request.tag == tag
            {
                let (Some(t), None) = (out.request.tile, &out.error) else {
                    self.layers = None;
                    return;
                };
                let s = out.request.scale;
                let area = [t.x as f32 / s, t.y as f32 / s, t.x.saturating_add(t.w) as f32 / s, t.y.saturating_add(t.h) as f32 / s];
                let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
                stamp.pixels = Some((tag, ctx.load_texture("signature-drag-stamp", image, TextureOptions::LINEAR), area));
            }
            pending |= stamp.pixels.as_ref().is_none_or(|(received, ..)| *received != tag);
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
        if let Edit::MoveAnnotation { page, index, .. } | Edit::ResizeAnnotation { page, index, .. } = edit
            && self.contains(*page, *index)
        {
            self.key = Some((generation, *page, *index));
            if let Some(layers) = &mut self.layers {
                layers.settling = true;
            }
        } else {
            *self = Self::default();
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.key.is_some_and(|(_, p, _)| p == page)
            && let Some(layers) = &mut self.layers
        {
            layers.settling = false;
        }
    }

    pub fn paint(&self, painter: &egui::Painter, cx: &PageCx<'_>, cv: &CommentView, pending: Option<&Edit>) {
        let (Some((_, page, index)), Some(layers)) = (self.key, &self.layers) else { return };
        if page != cx.page || !matches!(cx.tool, QuickTool::Select | QuickTool::Fill(_)) || cx.hidden {
            return;
        }
        let dragging = matches!(cv.gesture, Some(Gesture::Move { page: p, index: i, .. } | Gesture::Resize { page: p, index: i, .. }) if (p, i) == (page, index));
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
        let (pointer, shift) = painter.ctx().input(|i| (i.pointer.hover_pos(), i.modifiers.shift));
        let rect = cx.adjusted_rect(a, cv.gesture.as_ref(), pointer, shift, pending);
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(cx.xf.rect));
        cx.xf.paint_image(&painter, background.id(), 0.0, 0.0, 1.0, 1.0);
        match &layers.front {
            Front::Image { texture, opacity, turn } => cx.xf.paint_user_image(
                &painter,
                texture.id(),
                p,
                cx.rect_to_user(rect),
                *turn,
                egui::Color32::from_white_alpha((opacity * 255.0).round() as u8),
            ),
            Front::Stamp(stamp) => {
                let Some((_, texture, area)) = &stamp.pixels else { return };
                // Where the stamp's pixels were rendered, stretched as its box is from `from` to `rect`.
                let from = cx.xf.user_rect(cx.info, page, stamp.from);
                if from.width() < 0.01 || from.height() < 0.01 {
                    return;
                }
                let stretch = vec2(rect.width() / from.width(), rect.height() / from.height());
                let mut mesh = egui::Mesh::with_texture(texture.id());
                for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                    let (x, y) = (area[0] + u * (area[2] - area[0]), area[1] + v * (area[3] - area[1]));
                    let at = cx.xf.norm_to_screen(x / cx.xf.pw, y / cx.xf.ph);
                    let pos = rect.min + (at - from.min) * stretch;
                    mesh.vertices.push(egui::epaint::Vertex { pos, uv: pos2(u, v), color: egui::Color32::WHITE });
                }
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                painter.add(egui::Shape::mesh(mesh));
            }
        }
    }
}

impl Layers {
    fn new(front: Front, renderer: RenderPool) -> Self {
        Self { front, renderer, background: None, tag: None, received_tag: None, settling: false }
    }
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
