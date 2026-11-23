//! Paints egui's output into the window without a GPU, and presents only what changed.
//!
//! The window's pixels live in a buffer that Windows keeps between frames (softbuffer's GDI
//! backend), so a frame draws the rectangles [`Damage`] found changed and hands exactly those to
//! the system. Remote Desktop then encodes and sends those rectangles, not the window.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, Vertex};
use egui::{TextureId, TexturesDelta};
use winit::window::Window;

use super::damage::{Damage, mix};
use super::raster::{MeshJob, PixelRect, Target, Texture, draw_mesh};

/// The longest side of a texture egui may ask for. Nothing here limits it; this is what Direct3D
/// offers, so pages are cut into tiles the same way as on a GPU.
pub(crate) const MAX_TEXTURE_SIDE: usize = 16384;

struct Stored {
    texture: Texture,
    /// Changes whenever the pixels do, so what is drawn with the texture is redrawn.
    version: u64,
}

pub(crate) struct Painter {
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    /// The size the surface was last given.
    size: (u32, u32),
    textures: HashMap<TextureId, Stored>,
    versions: u64,
    damage: Damage,
    /// A paint callback was skipped and said so in the log.
    warned_callback: bool,
    /// Pixels Windows says lost their picture since the last frame, to be presented again.
    exposed: Option<PixelRect>,
}

/// What one frame did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Painted {
    /// Pixels drawn and presented; 0 when the frame looked like the last one.
    pub pixels: u64,
}

/// One mesh of a frame, with what the two passes over it share.
struct Item<'a> {
    mesh: &'a egui::Mesh,
    /// The mesh's clip rectangle, in pixels.
    clip: PixelRect,
    /// The pixels the mesh may touch.
    bounds: PixelRect,
}

impl Painter {
    pub(crate) fn new(window: &Arc<Window>) -> Result<Self, String> {
        let context = softbuffer::Context::new(Arc::clone(window)).map_err(|e| format!("no drawing context for the window: {e}"))?;
        let surface = softbuffer::Surface::new(&context, Arc::clone(window)).map_err(|e| format!("no drawing surface for the window: {e}"))?;
        Ok(Self { surface, size: (0, 0), textures: HashMap::new(), versions: 0, damage: Damage::default(), warned_callback: false, exposed: None })
    }

    /// The window was resized, uncovered or restored: what was drawn is not to be relied on.
    pub(crate) fn invalidate(&mut self) {
        self.damage.invalidate();
    }

    /// Windows lost what the window showed in `rect` (it was uncovered, or the Remote Desktop
    /// session reconnected) and asked for it again. The buffer still holds those pixels, so the
    /// next frame presents them even if nothing in them changed; otherwise that part would stay
    /// blank until something there moved.
    pub(crate) fn expose(&mut self, rect: PixelRect) {
        self.exposed = Some(self.exposed.map_or(rect, |earlier| earlier.union(rect)));
    }

    /// Draws a frame and presents the part that changed. `screenshot` asks for the whole window's
    /// pixels as well.
    pub(crate) fn paint(
        &mut self,
        window: &Window,
        scale: f32,
        clear: egui::Color32,
        primitives: &[ClippedPrimitive],
        textures: &mut TexturesDelta,
        screenshot: bool,
    ) -> Result<(Painted, Option<egui::ColorImage>), String> {
        self.set_textures(textures);
        let painted = self.draw(window, scale, clear, primitives, screenshot);
        // Freed after painting: this frame's meshes may still show them.
        for id in &textures.free {
            self.textures.remove(id);
        }
        // The changes are taken in whether or not the window could be drawn: egui checks that
        // none is dropped unseen.
        textures.clear();
        painted
    }

    fn set_textures(&mut self, delta: &TexturesDelta) {
        for (id, updates) in &delta.set {
            for update in updates {
                let ImageData::Color(image) = &update.image;
                self.versions += 1;
                let version = self.versions;
                match (update.pos, self.textures.get_mut(id)) {
                    (Some(pos), Some(stored)) => {
                        stored.texture.patch(pos, image);
                        stored.texture.options = update.options;
                        stored.version = version;
                    }
                    // A patch of a texture that was never set whole has nothing to patch.
                    (Some(_), None) => log::debug!("a partial update of the unknown texture {id:?} was dropped"),
                    (None, _) => {
                        self.textures.insert(*id, Stored { texture: Texture::new(image, update.options), version });
                    }
                }
            }
        }
    }

    fn draw(
        &mut self,
        window: &Window,
        scale: f32,
        clear: egui::Color32,
        primitives: &[ClippedPrimitive],
        screenshot: bool,
    ) -> Result<(Painted, Option<egui::ColorImage>), String> {
        let size = window.inner_size();
        // A minimized window has no pixels.
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return Ok((Painted::default(), None)) };
        if self.size != (size.width, size.height) {
            self.surface.resize(width, height).map_err(|e| format!("the window's {width}×{height} pixels couldn't be allocated: {e}"))?;
            self.size = (size.width, size.height);
            self.damage.invalidate();
        }
        let mut buffer = self.surface.buffer_mut().map_err(|e| format!("the window's pixels can't be reached: {e}"))?;
        if buffer.age() == 0 {
            // A new buffer, or one the system didn't keep.
            self.damage.invalidate();
        }
        let (width, height) = (usize::try_from(size.width).unwrap_or(0), usize::try_from(size.height).unwrap_or(0));
        let window_rect = PixelRect::new(0, 0, i32::try_from(width).unwrap_or(i32::MAX), i32::try_from(height).unwrap_or(i32::MAX));
        let clear = super::raster::pack(clear) | 0xFF00_0000;

        // First pass: what does each tile of the window show?
        self.damage.begin(width, height, mix(u64::from(clear), u64::from(scale.to_bits())));
        let mut items: Vec<Item<'_>> = Vec::with_capacity(primitives.len());
        for primitive in primitives {
            let mesh = match &primitive.primitive {
                Primitive::Mesh(mesh) => mesh,
                Primitive::Callback(_) => {
                    if !core::mem::replace(&mut self.warned_callback, true) {
                        log::warn!("a paint callback was skipped: the software renderer has no GPU to run it on");
                    }
                    continue;
                }
            };
            let clip = PixelRect::from_points(primitive.clip_rect, scale).intersect(window_rect);
            let version = self.textures.get(&mesh.texture_id).map_or(0, |stored| stored.version);
            let seed = mesh_seed(mesh.texture_id, version, clip);
            let bounds = mark(&mut self.damage, mesh, clip, scale, seed);
            items.push(Item { mesh, clip, bounds });
        }
        let rects = self.damage.finish();

        // Second pass: draw the tiles that differ from the last frame.
        let mut target = Target { pixels: &mut buffer, width, height };
        let mut pixels = 0_u64;
        for rect in &rects {
            target.fill(*rect, clear);
            pixels += u64::from(rect.x1.abs_diff(rect.x0)) * u64::from(rect.y1.abs_diff(rect.y0));
            for item in &items {
                if item.bounds.intersect(*rect).is_empty() {
                    continue;
                }
                let Some(stored) = self.textures.get(&item.mesh.texture_id) else { continue };
                draw_mesh(&mut target, &MeshJob { mesh: item.mesh, texture: &stored.texture, clip: item.clip.intersect(*rect), scale });
            }
        }
        let image = screenshot.then(|| {
            egui::ColorImage::new(
                [width, height],
                buffer.iter().map(|p| egui::Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, *p as u8)).collect(),
            )
        });
        let exposed = self.exposed.take().map(|rect| rect.intersect(window_rect));
        let damaged: Vec<softbuffer::Rect> = presented(&rects, exposed).into_iter().filter_map(present_rect).collect();
        if !damaged.is_empty() {
            buffer.present_with_damage(&damaged).map_err(|e| format!("the window couldn't be updated: {e}"))?;
        }
        Ok((Painted { pixels }, image))
    }
}

/// What a frame hands to the system: the rectangles it redrew, and what Windows asked to have shown
/// again. Nothing when neither; the window then keeps its picture.
fn presented(redrawn: &[PixelRect], exposed: Option<PixelRect>) -> Vec<PixelRect> {
    redrawn.iter().copied().chain(exposed.filter(|rect| !rect.is_empty())).collect()
}

fn present_rect(rect: PixelRect) -> Option<softbuffer::Rect> {
    Some(softbuffer::Rect {
        x: u32::try_from(rect.x0).ok()?,
        y: u32::try_from(rect.y0).ok()?,
        width: NonZeroU32::new(u32::try_from(rect.x1.checked_sub(rect.x0)?).ok()?)?,
        height: NonZeroU32::new(u32::try_from(rect.y1.checked_sub(rect.y0)?).ok()?)?,
    })
}

/// What every triangle of a mesh shares: its texture, the texture's contents and the clip.
fn mesh_seed(texture: TextureId, version: u64, clip: PixelRect) -> u64 {
    let id = match texture {
        TextureId::Managed(id) => mix(1, id),
        TextureId::User(id) => mix(2, id),
    };
    [version, clip.x0 as u64, clip.y0 as u64, clip.x1 as u64, clip.y1 as u64].into_iter().fold(id, mix)
}

fn vertex_hash(hash: u64, vertex: &Vertex) -> u64 {
    let position = u64::from(vertex.pos.x.to_bits()) << 32 | u64::from(vertex.pos.y.to_bits());
    let uv = u64::from(vertex.uv.x.to_bits()) << 32 | u64::from(vertex.uv.y.to_bits());
    mix(mix(mix(hash, position), uv), u64::from(u32::from_le_bytes(vertex.color.to_array())))
}

/// Adds every triangle of `mesh` to the tiles it may touch. Returns the pixels the mesh may touch.
fn mark(damage: &mut Damage, mesh: &egui::Mesh, clip: PixelRect, scale: f32, seed: u64) -> PixelRect {
    let mut bounds = PixelRect::default();
    if clip.is_empty() {
        return bounds;
    }
    let vertex = |index: &u32| mesh.vertices.get(usize::try_from(*index).ok()?);
    for [a, b, c] in mesh.indices.as_chunks::<3>().0 {
        let [Some(a), Some(b), Some(c)] = [vertex(a), vertex(b), vertex(c)] else { continue };
        let (x0, x1) = (a.pos.x.min(b.pos.x).min(c.pos.x) * scale, a.pos.x.max(b.pos.x).max(c.pos.x) * scale);
        let (y0, y1) = (a.pos.y.min(b.pos.y).min(c.pos.y) * scale, a.pos.y.max(b.pos.y).max(c.pos.y) * scale);
        // A pixel wider than the triangle on every side: rounding must never leave a tile out.
        let area = PixelRect::new(pixel(x0.floor()) - 1, pixel(y0.floor()) - 1, pixel(x1.ceil()) + 1, pixel(y1.ceil()) + 1).intersect(clip);
        if area.is_empty() {
            continue;
        }
        damage.add(area, vertex_hash(vertex_hash(vertex_hash(seed, a), b), c));
        bounds = bounds.union(area);
    }
    bounds
}

/// A pixel coordinate from a float that may be anything.
fn pixel(v: f32) -> i32 {
    v.clamp(-1_048_576.0, 1_048_576.0) as i32
}

#[cfg(test)]
mod tests;
