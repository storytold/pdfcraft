//! Draws egui's meshes into a window-sized pixel buffer, on the processor.
//!
//! egui hands a renderer triangles: positions in points, a texture coordinate and a premultiplied
//! colour per vertex. A pixel is the texture's texel times the interpolated colour, blended over
//! what is there in gamma space, as egui's GPU renderers do it.
//!
//! Two paths:
//!
//! - **Rectangles** ([`draw_quad`]): two triangles that make an axis-aligned rectangle with one
//!   colour, which is every glyph, every image and every page tile. A texture drawn at its own size
//!   is copied row by row, which is what makes scrolling a page cheap; any other is sampled.
//! - **Triangles** ([`draw_triangle`]): everything else (rounded corners, strokes, the
//!   anti-aliasing fringe egui puts around shapes), a row of pixels at a time.
//!
//! Pixels are `0xAARRGGBB`, premultiplied; the window's are opaque. Every access to the buffers is
//! bounds-checked: a mesh is input, and its numbers are not trusted to stay inside the window.

use egui::epaint::{Mesh, Vertex};
use egui::{Color32, TextureFilter, TextureOptions, TextureWrapMode};

/// Whole pixels `x0..x1` by `y0..y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl PixelRect {
    pub(crate) const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub(crate) fn intersect(self, other: Self) -> Self {
        Self { x0: self.x0.max(other.x0), y0: self.y0.max(other.y0), x1: self.x1.min(other.x1), y1: self.y1.min(other.y1) }
    }

    /// The smallest rectangle holding both; an empty one adds nothing.
    pub(crate) fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self { x0: self.x0.min(other.x0), y0: self.y0.min(other.y0), x1: self.x1.max(other.x1), y1: self.y1.max(other.y1) }
    }

    /// `rect` (in points) at `scale` pixels per point, rounded to whole pixels as a GPU scissor is.
    pub(crate) fn from_points(rect: egui::Rect, scale: f32) -> Self {
        Self {
            x0: to_pixel((rect.min.x * scale).round()),
            y0: to_pixel((rect.min.y * scale).round()),
            x1: to_pixel((rect.max.x * scale).round()),
            y1: to_pixel((rect.max.y * scale).round()),
        }
    }
}

/// Farthest coordinate, in pixels, that anything is drawn at. Keeps the fixed-point edge functions
/// (8 fractional bits, multiplied in pairs) inside an `i64` whatever a mesh holds.
const MAX_COORD: f32 = 1_048_576.0;

/// A pixel coordinate from a float that may be anything (`NaN` gives 0).
fn to_pixel(v: f32) -> i32 {
    v.clamp(-MAX_COORD, MAX_COORD) as i32
}

/// The first pixel whose centre is at or after `v`.
fn first_center(v: f32) -> i32 {
    to_pixel((v - 0.5).ceil())
}

const WHITE: u32 = 0xFFFF_FFFF;

/// `color` (premultiplied) as a pixel.
pub(crate) fn pack(color: Color32) -> u32 {
    let [r, g, b, a] = color.to_array();
    u32::from(a) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// One channel of `texel × tint`, both 0..=255.
#[inline]
fn mul_channel(a: u32, b: u32) -> u32 {
    ((a * b + 128) * 257) >> 16
}

/// The texel times the vertex colour, channel by channel.
#[inline]
fn modulate(texel: u32, tint: u32) -> u32 {
    if tint == WHITE {
        return texel;
    }
    if texel == WHITE {
        return tint;
    }
    mul_channel(texel >> 24, tint >> 24) << 24
        | mul_channel((texel >> 16) & 0xFF, (tint >> 16) & 0xFF) << 16
        | mul_channel((texel >> 8) & 0xFF, (tint >> 8) & 0xFF) << 8
        | mul_channel(texel & 0xFF, tint & 0xFF)
}

/// Premultiplied `src` over the opaque `dst`. Colours with more light than alpha (egui's additive
/// ones) saturate.
#[inline]
fn over(dst: u32, src: u32) -> u32 {
    let alpha = src >> 24;
    if alpha == 255 {
        return src;
    }
    if src == 0 {
        return dst;
    }
    // 0..=256, so that full alpha keeps nothing of `dst`.
    let keep = 256 - (alpha + (alpha >> 7));
    let rb = (((dst & 0x00FF_00FF) * keep) >> 8) & 0x00FF_00FF;
    let g = (((dst & 0x0000_FF00) * keep) >> 8) & 0x0000_FF00;
    let r = (((src >> 16) & 0xFF) + (rb >> 16)).min(255);
    let g = (((src >> 8) & 0xFF) + (g >> 8)).min(255);
    let b = ((src & 0xFF) + (rb & 0xFF)).min(255);
    0xFF00_0000 | r << 16 | g << 8 | b
}

/// `a` to `b` by `t` of 256, channel by channel.
#[inline]
fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let s = 256 - t;
    let rb = (((a & 0x00FF_00FF) * s + (b & 0x00FF_00FF) * t) >> 8) & 0x00FF_00FF;
    let ag = (((a >> 8) & 0x00FF_00FF) * s + ((b >> 8) & 0x00FF_00FF) * t) & 0xFF00_FF00;
    rb | ag
}

/// An image egui gave the renderer: the font atlas, an icon, a page tile.
pub(crate) struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    /// Every pixel has full alpha, so it can be copied without blending.
    opaque: bool,
    pub options: TextureOptions,
}

impl Texture {
    pub(crate) fn new(image: &egui::ColorImage, options: TextureOptions) -> Self {
        let [width, height] = image.size;
        let mut pixels: Vec<u32> = image.pixels.iter().map(|c| pack(*c)).collect();
        // An image that says it is taller than its pixels is as tall as they go.
        let height = pixels.len().checked_div(width).map_or(0, |rows| height.min(rows));
        pixels.truncate(width.saturating_mul(height));
        let opaque = pixels.iter().all(|p| p >> 24 == 255);
        Self { width, height, pixels, opaque, options }
    }

    /// Replaces the part of the image at `pos` with `patch`; what falls outside is left out.
    pub(crate) fn patch(&mut self, pos: [usize; 2], patch: &egui::ColorImage) {
        let [x, y] = pos;
        let [patch_width, _] = patch.size;
        if patch_width == 0 || x >= self.width {
            return;
        }
        let copied = patch_width.min(self.width - x);
        for (row, source) in patch.pixels.chunks_exact(patch_width).enumerate() {
            let Some(start) = y.checked_add(row).and_then(|row| row.checked_mul(self.width)).and_then(|at| at.checked_add(x)) else { return };
            let Some(target) = start.checked_add(copied).and_then(|end| self.pixels.get_mut(start..end)) else { return };
            for (to, from) in target.iter_mut().zip(source) {
                *to = pack(*from);
                self.opaque &= from.a() == 255;
            }
        }
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn height(&self) -> usize {
        self.height
    }

    /// Texels `x0..x1` of row `y`, if all of them exist.
    fn row(&self, y: i64, x0: i64, x1: i64) -> Option<&[u32]> {
        let (y, x0, x1) = (usize::try_from(y).ok()?, usize::try_from(x0).ok()?, usize::try_from(x1).ok()?);
        if y >= self.height || x1 > self.width || x0 > x1 {
            return None;
        }
        let start = y.checked_mul(self.width)?.checked_add(x0)?;
        self.pixels.get(start..start.checked_add(x1 - x0)?)
    }

    /// An index along a side of `len` texels, by the texture's wrap mode.
    fn wrap(&self, at: i64, len: usize) -> usize {
        let len = len.max(1) as i64;
        let at = match self.options.wrap_mode {
            TextureWrapMode::ClampToEdge => at.clamp(0, len - 1),
            // Mirroring is drawn as repeating: nothing PdfCraft shows mirrors a texture.
            TextureWrapMode::Repeat | TextureWrapMode::MirroredRepeat => at.rem_euclid(len),
        };
        at as usize
    }

    fn texel(&self, x: i64, y: i64) -> u32 {
        let at = self.wrap(y, self.height).saturating_mul(self.width).saturating_add(self.wrap(x, self.width));
        self.pixels.get(at).copied().unwrap_or(0)
    }

    /// The texel that holds the point (`u`, `v`), in texels.
    fn nearest(&self, u: f32, v: f32) -> u32 {
        self.texel(u.floor() as i64, v.floor() as i64)
    }

    /// The four texels around (`u`, `v`), in texels, weighted by how close each is.
    fn bilinear(&self, u: f32, v: f32) -> u32 {
        let (u, v) = (u - 0.5, v - 0.5);
        let (x, y) = (u.floor(), v.floor());
        let (tx, ty) = (((u - x) * 256.0) as u32, ((v - y) * 256.0) as u32);
        let (x, y) = (x as i64, y as i64);
        let top = lerp(self.texel(x, y), self.texel(x.saturating_add(1), y), tx.min(256));
        let bottom = lerp(self.texel(x, y.saturating_add(1)), self.texel(x.saturating_add(1), y.saturating_add(1)), tx.min(256));
        lerp(top, bottom, ty.min(256))
    }

    fn sample(&self, u: f32, v: f32, linear: bool) -> u32 {
        if linear { self.bilinear(u, v) } else { self.nearest(u, v) }
    }

    /// The filter for drawing `texels_per_pixel` texels on each pixel.
    fn linear_at(&self, texels_per_pixel: f32) -> bool {
        let filter = if texels_per_pixel.abs() < 1.0 { self.options.magnification } else { self.options.minification };
        filter == TextureFilter::Linear
    }
}

/// The pixels being drawn to.
pub(crate) struct Target<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Target<'_> {
    pub(crate) fn bounds(&self) -> PixelRect {
        PixelRect::new(0, 0, i32::try_from(self.width).unwrap_or(i32::MAX), i32::try_from(self.height).unwrap_or(i32::MAX))
    }

    /// Pixels `x0..x1` of row `y`, if all of them exist.
    fn row(&mut self, y: i32, x0: i32, x1: i32) -> Option<&mut [u32]> {
        let (y, x0, x1) = (usize::try_from(y).ok()?, usize::try_from(x0).ok()?, usize::try_from(x1).ok()?);
        if y >= self.height || x1 > self.width || x0 >= x1 {
            return None;
        }
        let start = y.checked_mul(self.width)?.checked_add(x0)?;
        self.pixels.get_mut(start..start.checked_add(x1 - x0)?)
    }

    /// Sets every pixel of `rect` to the opaque `color`.
    pub(crate) fn fill(&mut self, rect: PixelRect, color: u32) {
        let rect = rect.intersect(self.bounds());
        for y in rect.y0..rect.y1 {
            if let Some(row) = self.row(y, rect.x0, rect.x1) {
                row.fill(color | 0xFF00_0000);
            }
        }
    }
}

/// One mesh to draw.
pub(crate) struct MeshJob<'a> {
    pub mesh: &'a Mesh,
    /// The mesh's texture; a mesh whose texture is gone draws nothing.
    pub texture: &'a Texture,
    /// Nothing outside is touched: the mesh's clip rectangle, within what is being redrawn.
    pub clip: PixelRect,
    /// Pixels per point.
    pub scale: f32,
}

/// Draws `job.mesh` inside `job.clip`.
pub(crate) fn draw_mesh(target: &mut Target<'_>, job: &MeshJob<'_>) {
    let clip = job.clip.intersect(target.bounds());
    if clip.is_empty() || !job.scale.is_finite() || job.scale <= 0.0 {
        return;
    }
    let job = MeshJob { clip, ..*job };
    for piece in pieces(job.mesh) {
        match piece {
            Piece::Quad(quad, _) => draw_quad(target, &job, &quad),
            Piece::Triangle(triangle) => {
                if let [Some(a), Some(b), Some(c)] = [vertex(job.mesh, triangle, 0), vertex(job.mesh, triangle, 1), vertex(job.mesh, triangle, 2)] {
                    draw_triangle(target, &job, [a, b, c]);
                }
            }
        }
    }
}

/// A part of a mesh as `draw_mesh` draws it.
pub(crate) enum Piece<'a> {
    /// Two triangles (their six indices) drawn as one rectangle.
    Quad(Quad, &'a [u32]),
    /// Three indices drawn as a triangle.
    Triangle(&'a [u32]),
}

/// How a mesh is drawn, in order. What finds the changed parts of a frame reads the same pieces,
/// so the same triangles drawn another way count as a change.
pub(crate) fn pieces(mesh: &Mesh) -> impl Iterator<Item = Piece<'_>> {
    let indices = &mesh.indices;
    let mut at = 0;
    std::iter::from_fn(move || {
        let triangle = indices.get(at..at + 3)?;
        if let Some((six, quad)) = indices.get(at..at + 6).and_then(|six| Some((six, quad_of(mesh, six)?))) {
            at += 6;
            return Some(Piece::Quad(quad, six));
        }
        at += 3;
        Some(Piece::Triangle(triangle))
    })
}

fn vertex<'a>(mesh: &'a Mesh, indices: &[u32], at: usize) -> Option<&'a Vertex> {
    mesh.vertices.get(usize::try_from(*indices.get(at)?).ok()?)
}

/// An axis-aligned rectangle of one colour whose texture coordinates follow its sides.
pub(crate) struct Quad {
    /// In points.
    min: egui::Pos2,
    max: egui::Pos2,
    /// The texture coordinates at `min` and `max`.
    uv_min: egui::Pos2,
    uv_max: egui::Pos2,
    color: Color32,
}

/// The rectangle two triangles make, when they make one: four corners, both triangles on the same
/// diagonal, one colour, and texture coordinates that go with the sides. egui draws every glyph and
/// image this way (`Mesh::add_rect_with_uv`, and the inside of a filled rectangle).
fn quad_of(mesh: &Mesh, six: &[u32]) -> Option<Quad> {
    let (first, second) = (six.get(..3)?, six.get(3..6)?);
    // The corner each triangle has alone; the other two are the diagonal they share.
    let alone = |own: &[u32], other: &[u32]| {
        let mut only = own.iter().filter(|i| !other.contains(i));
        match (only.next(), only.next()) {
            (Some(i), None) => Some(*i),
            _ => None,
        }
    };
    let (p, q) = (alone(first, second)?, alone(second, first)?);
    let mut shared = first.iter().filter(|i| **i != p);
    let (r, s) = (*shared.next()?, *shared.next()?);
    if r == s || !second.contains(&r) || !second.contains(&s) {
        return None;
    }
    let at = |i: u32| mesh.vertices.get(usize::try_from(i).ok()?);
    let (p, q, r, s) = (at(p)?, at(q)?, at(r)?, at(s)?);
    if p.color != q.color || p.color != r.color || p.color != s.color {
        return None;
    }
    // `p` and `q` are opposite corners, and so are `r` and `s`: `r` and `s` each share one side
    // with `p` and the other with `q`.
    let corners = (r.pos.x == p.pos.x && r.pos.y == q.pos.y && s.pos.x == q.pos.x && s.pos.y == p.pos.y)
        || (r.pos.x == q.pos.x && r.pos.y == p.pos.y && s.pos.x == p.pos.x && s.pos.y == q.pos.y);
    let uv_follows = |v: &Vertex| {
        (if v.pos.x == p.pos.x { v.uv.x == p.uv.x } else { v.uv.x == q.uv.x })
            && (if v.pos.y == p.pos.y { v.uv.y == p.uv.y } else { v.uv.y == q.uv.y })
    };
    if !corners || p.pos.x == q.pos.x || p.pos.y == q.pos.y || !uv_follows(r) || !uv_follows(s) {
        return None;
    }
    let (left, right) = if p.pos.x < q.pos.x { (p, q) } else { (q, p) };
    let (top, bottom) = if p.pos.y < q.pos.y { (p, q) } else { (q, p) };
    Some(Quad {
        min: egui::pos2(left.pos.x, top.pos.y),
        max: egui::pos2(right.pos.x, bottom.pos.y),
        uv_min: egui::pos2(left.uv.x, top.uv.y),
        uv_max: egui::pos2(right.uv.x, bottom.uv.y),
        color: p.color,
    })
}

/// How far, in texels over the whole side, a texture may be from its own size and still be copied.
const COPY_TOLERANCE: f32 = 0.05;

fn draw_quad(target: &mut Target<'_>, job: &MeshJob<'_>, quad: &Quad) {
    let tint = pack(quad.color);
    if tint == 0 {
        return;
    }
    let (x0, y0, x1, y1) = (quad.min.x * job.scale, quad.min.y * job.scale, quad.max.x * job.scale, quad.max.y * job.scale);
    let cover = PixelRect::new(first_center(x0), first_center(y0), first_center(x1), first_center(y1)).intersect(job.clip);
    if cover.is_empty() || !(x1 > x0 && y1 > y0) {
        return;
    }
    let texture = job.texture;
    let (width, height) = (texture.width() as f32, texture.height() as f32);
    let (u0, v0, u1, v1) = (quad.uv_min.x * width, quad.uv_min.y * height, quad.uv_max.x * width, quad.uv_max.y * height);
    if u0 == u1 && v0 == v1 {
        // One texel for the whole rectangle: a plain fill (egui's white texel, mostly).
        let color = modulate(texture.nearest(u0, v0), tint);
        for y in cover.y0..cover.y1 {
            if let Some(row) = target.row(y, cover.x0, cover.x1) {
                fill_row(row, color);
            }
        }
        return;
    }
    let (step_x, step_y) = ((u1 - u0) / (x1 - x0), (v1 - v0) / (y1 - y0));
    let (shift_x, shift_y) = (u0 - x0, v0 - y0);
    let own_size = ((u1 - u0) - (x1 - x0)).abs() < COPY_TOLERANCE
        && ((v1 - v0) - (y1 - y0)).abs() < COPY_TOLERANCE
        && (shift_x - shift_x.round()).abs() < COPY_TOLERANCE
        && (shift_y - shift_y.round()).abs() < COPY_TOLERANCE;
    let (shift_x, shift_y) = (shift_x.round() as i64, shift_y.round() as i64);
    let linear = texture.linear_at(step_x.abs().max(step_y.abs()));
    for y in cover.y0..cover.y1 {
        let Some(row) = target.row(y, cover.x0, cover.x1) else { continue };
        // The shifts come from the mesh's texture coordinates, which may be anything: a shift that
        // doesn't add up is no copy, and the texels are sampled instead.
        let texels = |row: i32, from: i32, to: i32| {
            texture.row(i64::from(row).checked_add(shift_y)?, i64::from(from).checked_add(shift_x)?, i64::from(to).checked_add(shift_x)?)
        };
        let copied = if own_size { texels(y, cover.x0, cover.x1) } else { None };
        if let Some(texels) = copied {
            copy_row(row, texels, tint, texture.opaque);
            continue;
        }
        let v = v0 + (y as f32 + 0.5 - y0) * step_y;
        let mut u = u0 + (cover.x0 as f32 + 0.5 - x0) * step_x;
        for pixel in row {
            *pixel = over(*pixel, modulate(texture.sample(u, v, linear), tint));
            u += step_x;
        }
    }
}

/// One colour over a row.
fn fill_row(row: &mut [u32], color: u32) {
    if color >> 24 == 255 {
        row.fill(color);
    } else if color != 0 {
        for pixel in row {
            *pixel = over(*pixel, color);
        }
    }
}

/// A row of texels over a row of pixels of the same length.
fn copy_row(row: &mut [u32], texels: &[u32], tint: u32, opaque: bool) {
    if opaque && tint == WHITE {
        // A page tile at its own size: the whole of scrolling a page.
        if row.len() == texels.len() {
            row.copy_from_slice(texels);
        }
        return;
    }
    for (pixel, texel) in row.iter_mut().zip(texels) {
        // Most of a glyph's rectangle is empty.
        if *texel != 0 {
            *pixel = over(*pixel, modulate(*texel, tint));
        }
    }
}

/// Fractional bits of the positions the edge functions are computed with.
const SUBPIXEL: i64 = 256;

/// One edge of a triangle, as `cx·X + cy·Y + c0`: zero on the edge, positive inside.
#[derive(Clone, Copy)]
struct Edge {
    cx: i64,
    cy: i64,
    c0: i64,
    /// 0 when a pixel centre exactly on the edge is inside, -1 when it belongs to the neighbour.
    bias: i64,
}

impl Edge {
    fn new(from: (i64, i64), to: (i64, i64)) -> Self {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        // Two triangles sharing an edge walk it in opposite directions, so exactly one of them
        // gets the pixels whose centre is on it: nothing is drawn twice, nothing left out.
        // Which one is a GPU's rule, and `draw_quad`'s: a centre on a left or a top edge is inside
        // (turned as `draw_triangle` turns them, a left edge goes up and a top edge goes right).
        // A stroke is a rectangle with a fringe of triangles on each side; under any other rule
        // the pixels where they meet were drawn by neither, and thin lines came out as gaps.
        let owns = dy < 0 || (dy == 0 && dx > 0);
        Self { cx: -dy, cy: dx, c0: dy * from.0 - dx * from.1, bias: if owns { 0 } else { -1 } }
    }

    fn at(&self, x: i64, y: i64) -> i64 {
        self.cx * x + self.cy * y + self.c0
    }
}

/// Per vertex: red, green, blue, alpha (0..=255), then the texture coordinate in texels.
type Attributes = [f32; 6];

/// What a pixel of a triangle is, decided once for the triangle.
enum Shade {
    /// One colour.
    Flat(u32),
    /// One texel, colours interpolated.
    Gradient(u32),
    /// Texels interpolated; `Some` colour when the three vertices share it.
    Textured { linear: bool, tint: Option<u32> },
}

fn fixed(v: f32) -> i64 {
    (v.clamp(-MAX_COORD, MAX_COORD) * SUBPIXEL as f32).round() as i64
}

/// The first pixel whose centre is at or after `at` (in subpixels).
fn first_center_at(at: i64) -> i32 {
    let pixel = (at - SUBPIXEL / 2 + SUBPIXEL - 1).div_euclid(SUBPIXEL);
    i32::try_from(pixel).unwrap_or(if pixel < 0 { i32::MIN } else { i32::MAX })
}

/// One past the last pixel whose centre is at or before `at` (in subpixels).
fn past_center_at(at: i64) -> i32 {
    let pixel = (at - SUBPIXEL / 2).div_euclid(SUBPIXEL) + 1;
    i32::try_from(pixel).unwrap_or(if pixel < 0 { i32::MIN } else { i32::MAX })
}

fn draw_triangle(target: &mut Target<'_>, job: &MeshJob<'_>, vertices: [&Vertex; 3]) {
    let [a, b, c] = vertices;
    let position = |v: &Vertex| (v.pos.x * job.scale, v.pos.y * job.scale);
    let fixed_point = |p: (f32, f32)| (fixed(p.0), fixed(p.1));
    let (mut fa, mut fb, fc) = (fixed_point(position(a)), fixed_point(position(b)), fixed_point(position(c)));
    // From the same snapped corners the edges are tested with: a corner snapped across a pixel
    // centre would otherwise leave that centre out of the bounds of the triangle that owns it.
    // The bounds hold every centre on the triangle's border; `span` decides which are inside.
    let bounds = PixelRect::new(
        first_center_at(fa.0.min(fb.0).min(fc.0)),
        first_center_at(fa.1.min(fb.1).min(fc.1)),
        past_center_at(fa.0.max(fb.0).max(fc.0)),
        past_center_at(fa.1.max(fb.1).max(fc.1)),
    )
    .intersect(job.clip);
    if bounds.is_empty() {
        return;
    }
    let (mut a, mut b) = (a, b);
    // Either winding is drawn: egui doesn't cull. Turn the triangle so that inside is positive.
    if Edge::new(fa, fb).at(fc.0, fc.1) < 0 {
        core::mem::swap(&mut fa, &mut fb);
        core::mem::swap(&mut a, &mut b);
    }
    // Each edge is opposite the vertex it gives the weight of.
    let edges = [Edge::new(fb, fc), Edge::new(fc, fa), Edge::new(fa, fb)];
    let area = edges[2].at(fc.0, fc.1);
    if area <= 0 {
        return;
    }
    let texture = job.texture;
    let (width, height) = (texture.width() as f32, texture.height() as f32);
    let attributes = |v: &Vertex| -> Attributes {
        let [r, g, bl, al] = v.color.to_array();
        [f32::from(r), f32::from(g), f32::from(bl), f32::from(al), v.uv.x * width, v.uv.y * height]
    };
    let corners = [attributes(a), attributes(b), attributes(c)];
    let one_color = a.color == b.color && a.color == c.color;
    // By their numbers: comparing the points themselves asserts that none is NaN, and a mesh may
    // hold anything.
    let same_uv = |v: &Vertex| v.uv.x == a.uv.x && v.uv.y == a.uv.y;
    let one_texel = same_uv(b) && same_uv(c);
    if one_color && a.color == Color32::TRANSPARENT {
        return;
    }
    let area = area as f64;
    // How much each attribute changes from one pixel to the next along a row.
    let mut per_pixel: Attributes = [0.0; 6];
    for (slot, step) in per_pixel.iter_mut().enumerate() {
        *step = edges.iter().zip(&corners).map(|(edge, corner)| (edge.cx * SUBPIXEL) as f64 / area * f64::from(corner[slot])).sum::<f64>() as f32;
    }
    // And from one row to the next: with both, the filter sees how many texels a pixel covers
    // across and down. A texture enlarged across but shrunk down is shrunk.
    let per_row = |slot: usize| -> f32 {
        edges.iter().zip(&corners).map(|(edge, corner)| (edge.cy * SUBPIXEL) as f64 / area * f64::from(corner[slot])).sum::<f64>() as f32
    };
    let texels_per_pixel = per_pixel[4].abs().max(per_pixel[5].abs()).max(per_row(4).abs()).max(per_row(5).abs());
    let shade = if one_texel {
        let texel = texture.nearest(corners[0][4], corners[0][5]);
        if one_color { Shade::Flat(modulate(texel, pack(a.color))) } else { Shade::Gradient(texel) }
    } else {
        Shade::Textured { linear: texture.linear_at(texels_per_pixel), tint: one_color.then(|| pack(a.color)) }
    };
    for y in bounds.y0..bounds.y1 {
        let center_y = i64::from(y) * SUBPIXEL + SUBPIXEL / 2;
        let Some((x0, x1)) = span(&edges, center_y, bounds) else { continue };
        let Some(row) = target.row(y, x0, x1) else { continue };
        if let Shade::Flat(color) = shade {
            fill_row(row, color);
            continue;
        }
        // The attributes at the centre of the row's first pixel.
        let center_x = i64::from(x0) * SUBPIXEL + SUBPIXEL / 2;
        let mut start: Attributes = [0.0; 6];
        for (slot, value) in start.iter_mut().enumerate() {
            *value = edges.iter().zip(&corners).map(|(edge, corner)| edge.at(center_x, center_y) as f64 / area * f64::from(corner[slot])).sum::<f64>()
                as f32;
        }
        shade_row(row, texture, &shade, start, &per_pixel);
    }
}

/// The pixels of the row at `center_y` whose centres are inside all three edges, within `bounds`.
fn span(edges: &[Edge; 3], center_y: i64, bounds: PixelRect) -> Option<(i32, i32)> {
    let (mut first, mut end) = (i64::from(bounds.x0), i64::from(bounds.x1));
    for edge in edges {
        // Inside is `cx·(256·x + 128) + rest >= 0`, for the pixel `x`.
        let rest = edge.cy * center_y + edge.c0 + edge.bias + edge.cx * (SUBPIXEL / 2);
        let per_pixel = edge.cx * SUBPIXEL;
        match per_pixel.cmp(&0) {
            core::cmp::Ordering::Greater => first = first.max(-(rest.div_euclid(per_pixel))),
            core::cmp::Ordering::Less => end = end.min(rest.div_euclid(-per_pixel) + 1),
            core::cmp::Ordering::Equal if rest < 0 => return None,
            core::cmp::Ordering::Equal => {}
        }
    }
    (first < end).then_some((first as i32, end as i32))
}

fn shade_row(row: &mut [u32], texture: &Texture, shade: &Shade, mut at: Attributes, per_pixel: &Attributes) {
    let channel = |v: f32| v.clamp(0.0, 255.0) as u32;
    let color = |at: &Attributes| channel(at[3]) << 24 | channel(at[0]) << 16 | channel(at[1]) << 8 | channel(at[2]);
    for pixel in row {
        let source = match shade {
            Shade::Flat(color) => *color,
            Shade::Gradient(texel) => modulate(*texel, color(&at)),
            Shade::Textured { linear, tint } => modulate(texture.sample(at[4], at[5], *linear), tint.unwrap_or_else(|| color(&at))),
        };
        *pixel = over(*pixel, source);
        for (value, step) in at.iter_mut().zip(per_pixel) {
            *value += step;
        }
    }
}

#[cfg(test)]
mod tests;
