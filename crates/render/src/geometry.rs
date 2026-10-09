//! Bounded snapping geometry from the same interpreter and visibility settings as rendering.
//! Returned coordinates are PDF user space, independent of crop, rotation and UserUnit.
use crate::RenderConfig;
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, FillRule, GlyphDrawMode, Image, ImageData, InterpreterCache, LumaData, Paint, PathDrawMode, SoftMask,
    TransformExt, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use kurbo::{Affine, BezPath, ParamCurve, PathEl, PathSeg, Point, Rect, Shape};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTENT_STREAMS: usize = 4096;
const MAX_SEGMENTS: usize = 20_000;
const MAX_TARGETS: usize = 40_000;
const MAX_INSTRUCTIONS: usize = 200_000;
const MAX_CLIP_EDGES: usize = 20_000;
const MAX_CLIP_WORK: usize = 8_000_000;
const MAX_IMAGE_PIXELS: usize = 4_000_000;
const MAX_PAGE_IMAGE_PIXELS: usize = 16_000_000;

#[derive(Clone, Debug, Default)]
pub struct VisibleGeometry {
    pub segments: Vec<[[f64; 2]; 2]>,
    pub endpoints: Vec<[f64; 2]>,
    pub midpoints: Vec<[f64; 2]>,
    pub truncated: bool,
    pub unreadable: usize,
    /// Paint whose visibility needs a soft-mask or pattern evaluation.
    pub visibility_limited: usize,
    pub glyphs: usize,
    pub images: usize,
}
struct Clip {
    edges: Vec<[Point; 2]>,
    rule: FillRule,
    bbox: Rect,
}
impl Clip {
    fn contains(&self, p: Point) -> bool {
        if !self.bbox.contains(p) {
            return false;
        }
        let mut winding = 0i32;
        for [a, b] in &self.edges {
            let side = (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y);
            if a.y <= p.y && b.y > p.y && side > 0.0 {
                winding += 1;
            }
            if b.y <= p.y && a.y > p.y && side < 0.0 {
                winding -= 1;
            }
        }
        match self.rule {
            FillRule::EvenOdd => winding % 2 != 0,
            FillRule::NonZero => winding != 0,
        }
    }
}
fn finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 1e9 && p.y.abs() <= 1e9
}
fn pair(p: Point) -> [f64; 2] {
    [p.x, p.y]
}
fn interpolate(a: Point, b: Point, t: f64) -> Point {
    a + (b - a) * t
}
fn split_at(a: Point, b: Point, c: Point, d: Point) -> Option<f64> {
    let r = b - a;
    let s = d - c;
    let cross = |x: kurbo::Vec2, y: kurbo::Vec2| x.x * y.y - x.y * y.x;
    let denominator = cross(r, s);
    if denominator.abs() < 1e-12 {
        return None;
    }
    let t = cross(c - a, s) / denominator;
    let u = cross(c - a, r) / denominator;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) { Some(t) } else { None }
}
/// Recursive subdivision is bounded even when malformed control points are huge.
fn flatten(segment: PathSeg, depth: usize, output: &mut Vec<[Point; 2]>, limit: usize) -> bool {
    if output.len() >= limit {
        return false;
    }
    let a = segment.start();
    let b = segment.end();
    if !finite(a) || !finite(b) {
        return false;
    }
    let chord = b - a;
    let deviation = |p: Point| {
        let length = chord.hypot2();
        let t = if length > 1e-18 { ((p - a).dot(chord) / length).clamp(0.0, 1.0) } else { 0.0 };
        p.distance(interpolate(a, b, t))
    };
    let flat = match segment {
        PathSeg::Line(_) => true,
        PathSeg::Quad(q) => finite(q.p1) && deviation(q.p1) <= 0.2,
        PathSeg::Cubic(c) => finite(c.p1) && finite(c.p2) && deviation(c.p1).max(deviation(c.p2)) <= 0.2,
    };
    if flat || depth >= 12 {
        output.push([a, b]);
        return true;
    }
    let (left, right) = segment.subdivide();
    flatten(left, depth + 1, output, limit) && flatten(right, depth + 1, output, limit)
}
// Filling and clipping implicitly close every subpath, including earlier open
// subpaths before a later MoveTo (not only the last contour).
fn close_subpaths(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut open = false;
    for element in path.elements() {
        if matches!(element, PathEl::MoveTo(_)) && open {
            out.close_path();
        }
        match element {
            PathEl::MoveTo(_) => open = true,
            PathEl::ClosePath => open = false,
            _ => {}
        }
        out.extend([*element]);
    }
    if open {
        out.close_path();
    }
    out
}
struct GeometryDevice {
    out: VisibleGeometry,
    inverse: Affine,
    clips: Vec<Clip>,
    clip_edges: usize,
    clip_work: usize,
    instructions: usize,
    image_pixels: usize,
    opacity: Vec<f32>,
    masked: bool,
}
impl GeometryDevice {
    fn visible(&self) -> bool {
        self.opacity.iter().all(|a| a.is_finite() && *a > 0.0)
    }
    fn paint_visible(&mut self, paint: &Paint<'_>) -> bool {
        if !self.visible() {
            return false;
        }
        if self.masked {
            self.out.visibility_limited = self.out.visibility_limited.saturating_add(1);
        }
        match paint {
            Paint::Color(c) => c.to_rgba().to_rgba8()[3] > 0,
            Paint::Pattern(_) => {
                self.out.visibility_limited = self.out.visibility_limited.saturating_add(1);
                true
            }
        }
    }
    fn inside(&mut self, p: Point) -> bool {
        self.clip_work = self.clip_work.saturating_add(self.clip_edges);
        if self.clip_work > MAX_CLIP_WORK {
            self.out.truncated = true;
            return false;
        }
        self.clips.iter().all(|c| c.contains(p))
    }
    fn target(&mut self, p: Point, midpoint: bool) {
        if self.out.endpoints.len().saturating_add(self.out.midpoints.len()) >= MAX_TARGETS {
            self.out.truncated = true;
            return;
        }
        if finite(p) && self.inside(p) {
            if midpoint {
                self.out.midpoints.push(pair(p));
            } else {
                self.out.endpoints.push(pair(p));
            }
        }
    }
    fn edge(&mut self, a: Point, b: Point, targets: bool) {
        if !finite(a) || !finite(b) || a.distance(b) < 1e-9 || self.out.truncated {
            return;
        }
        let mut cuts = vec![0.0, 1.0];
        for clip in &self.clips {
            self.clip_work = self.clip_work.saturating_add(clip.edges.len());
            if self.clip_work > MAX_CLIP_WORK {
                self.out.truncated = true;
                return;
            }
            for [c, d] in &clip.edges {
                if let Some(t) = split_at(a, b, *c, *d) {
                    cuts.push(t);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
        for interval in cuts.windows(2) {
            let [from, to] = interval else { continue };
            if to - from < 1e-12 || !self.inside(interpolate(a, b, (from + to) / 2.0)) {
                continue;
            }
            if self.out.segments.len() >= MAX_SEGMENTS {
                self.out.truncated = true;
                break;
            }
            let c = interpolate(a, b, *from);
            let d = interpolate(a, b, *to);
            self.out.segments.push([pair(c), pair(d)]);
            if targets {
                // Boundary points are visible even if winding classifies them as outside.
                if self.out.endpoints.len().saturating_add(self.out.midpoints.len()).saturating_add(3) <= MAX_TARGETS {
                    self.out.endpoints.extend([pair(c), pair(d)]);
                    self.out.midpoints.push(pair(interpolate(c, d, 0.5)));
                } else {
                    self.out.truncated = true;
                }
            } else {
                for p in [(*from > 0.0).then_some(c), (*to < 1.0).then_some(d)].into_iter().flatten() {
                    if self.out.endpoints.len().saturating_add(self.out.midpoints.len()) >= MAX_TARGETS {
                        self.out.truncated = true;
                        break;
                    }
                    self.out.endpoints.push(pair(p));
                }
            }
        }
    }
    fn path(&mut self, path: &BezPath, transform: Affine, closed: bool) {
        let transform = self.inverse * transform;
        if !path.is_finite() || !transform.is_finite() {
            self.out.unreadable += 1;
            return;
        }
        let mut path = transform * path.clone();
        if closed {
            path = close_subpaths(&path);
        }
        for segment in path.segments() {
            if self.out.truncated {
                break;
            }
            let straight = matches!(segment, PathSeg::Line(_));
            let mut edges = Vec::new();
            if !flatten(segment, 0, &mut edges, MAX_SEGMENTS.saturating_sub(self.out.segments.len())) {
                self.out.truncated = true;
                break;
            }
            for [a, b] in edges {
                self.edge(a, b, straight);
            }
            if !straight {
                self.target(segment.start(), false);
                self.target(segment.end(), false);
                self.target(segment.eval(0.5), true);
            }
        }
    }
    fn raster(&mut self, image: &ImageData, alpha: Option<&LumaData>, transform: Affine) {
        let width = image.width() as usize;
        let height = image.height() as usize;
        let Some(pixels) = width.checked_mul(height) else {
            self.out.truncated = true;
            return;
        };
        if pixels > MAX_IMAGE_PIXELS || width == 0 || height == 0 {
            self.out.truncated = true;
            return;
        }
        if alpha.is_some_and(|a| a.width != image.width() || a.height != image.height()) {
            self.out.unreadable += 1;
            return;
        }
        let (sx, sy) = image.scale_factors();
        let transform = self.inverse * transform * Affine::scale_non_uniform(f64::from(sx), f64::from(sy));
        if !transform.is_finite() {
            self.out.unreadable += 1;
            return;
        }
        let pixel = |x: usize, y: usize| -> Option<[u8; 4]> {
            if x >= width || y >= height {
                return None;
            }
            let i = y.checked_mul(width)?.checked_add(x)?;
            let rgb = match image {
                ImageData::Rgb(d) => {
                    let j = i.checked_mul(3)?;
                    [*d.data.get(j)?, *d.data.get(j.checked_add(1)?)?, *d.data.get(j.checked_add(2)?)?]
                }
                ImageData::Luma(d) => {
                    let v = *d.data.get(i)?;
                    [v, v, v]
                }
            };
            let a = match alpha {
                Some(d) => *d.data.get(i)?,
                None => 255,
            };
            // Detect alpha contours and luminance/colour edges. Fully transparent
            // pixels don't contribute hidden colour edges.
            Some([rgb[0], rgb[1], rgb[2], a])
        };
        let differs = |a: Option<[u8; 4]>, b: Option<[u8; 4]>| -> bool {
            let (Some(a), Some(b)) = (a, b) else {
                return false;
            };
            if a[3] == 0 && b[3] == 0 {
                return false;
            }
            if a[3].abs_diff(b[3]) >= 32 {
                return true;
            }
            (0..3).any(|c| {
                let composite = |p: [u8; 4]| (u16::from(p[c]) * u16::from(p[3]) + 255 * u16::from(255 - p[3])) / 255;
                composite(a).abs_diff(composite(b)) >= 32
            })
        };
        // Merge contiguous runs so a long scan edge is one segment, not one per pixel.
        for x in 0..=width {
            if self.out.truncated {
                break;
            }
            let mut start = None;
            for y in 0..=height {
                let edge =
                    y < height && differs(x.checked_sub(1).and_then(|x| pixel(x, y)).or(Some([0, 0, 0, 0])), pixel(x, y).or(Some([0, 0, 0, 0])));
                match (start, edge) {
                    (None, true) => start = Some(y),
                    (Some(from), false) => {
                        self.edge(transform * Point::new(x as f64, from as f64), transform * Point::new(x as f64, y as f64), true);
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        for y in 0..=height {
            if self.out.truncated {
                break;
            }
            let mut start = None;
            for x in 0..=width {
                let edge =
                    x < width && differs(y.checked_sub(1).and_then(|y| pixel(x, y)).or(Some([0, 0, 0, 0])), pixel(x, y).or(Some([0, 0, 0, 0])));
                match (start, edge) {
                    (None, true) => start = Some(x),
                    (Some(from), false) => {
                        self.edge(transform * Point::new(from as f64, y as f64), transform * Point::new(x as f64, y as f64), true);
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
}
impl<'a> Device<'a> for GeometryDevice {
    fn should_continue(&mut self) -> bool {
        self.instructions = self.instructions.saturating_add(1);
        if self.instructions > MAX_INSTRUCTIONS {
            self.out.truncated = true;
        }
        !self.out.truncated
    }
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        self.masked = mask.is_some();
    }
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, path: &BezPath, transform: Affine, paint: &Paint<'a>, mode: &PathDrawMode) {
        if self.paint_visible(paint) {
            self.path(path, transform, matches!(mode, PathDrawMode::Fill(_)));
        }
    }
    fn push_clip_path(&mut self, clip: &ClipPath) {
        if self.clips.len() >= 256 {
            self.out.truncated = true;
            return;
        }
        let path = close_subpaths(&(self.inverse * clip.path.clone()));
        let mut edges = Vec::new();
        for segment in path.segments() {
            if !flatten(segment, 0, &mut edges, MAX_CLIP_EDGES.saturating_sub(self.clip_edges)) {
                self.out.truncated = true;
                break;
            }
        }
        self.clip_edges = self.clip_edges.saturating_add(edges.len());
        self.clips.push(Clip { edges, rule: clip.fill, bbox: path.bounding_box() });
    }
    fn pop_clip_path(&mut self) {
        if let Some(c) = self.clips.pop() {
            self.clip_edges = self.clip_edges.saturating_sub(c.edges.len());
        }
    }
    fn push_transparency_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, _: BlendMode) {
        if self.opacity.len() >= 256 {
            self.out.truncated = true;
            return;
        }
        if mask.is_some() {
            self.out.visibility_limited += 1;
        }
        self.opacity.push(opacity);
    }
    fn pop_transparency_group(&mut self) {
        self.opacity.pop();
    }
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, mode: &GlyphDrawMode) {
        if matches!(mode, GlyphDrawMode::Invisible) || !self.paint_visible(paint) {
            return;
        }
        self.out.glyphs += 1;
        match glyph {
            Glyph::Outline(g) => self.path(&g.outline(), transform * glyph_transform, matches!(mode, GlyphDrawMode::Fill)),
            Glyph::Type3(g) => g.interpret(self, transform, glyph_transform, paint),
        }
    }
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        if !self.visible() {
            return;
        }
        let pixels = (image.width() as usize).checked_mul(image.height() as usize);
        let Some(pixels) = pixels.filter(|n| *n > 0 && *n <= MAX_IMAGE_PIXELS) else {
            self.out.truncated = true;
            return;
        };
        self.image_pixels = self.image_pixels.saturating_add(pixels);
        if self.image_pixels > MAX_PAGE_IMAGE_PIXELS {
            self.out.truncated = true;
            return;
        }
        self.out.images += 1;
        if self.masked {
            self.out.visibility_limited += 1;
        }
        let mut decoded = false;
        match image {
            Image::Raster(image) => image.with_rgba(
                |data, alpha| {
                    decoded = true;
                    self.raster(&data, alpha.as_ref(), transform);
                },
                None,
            ),
            Image::Stencil(image) => image.with_stencil(
                |alpha, paint| {
                    decoded = true;
                    if !self.paint_visible(paint) {
                        return;
                    }
                    let color = match paint {
                        Paint::Color(c) => c.to_rgba().to_rgba8(),
                        Paint::Pattern(_) => [0, 0, 0, 255],
                    };
                    let Some(length) = (alpha.width as usize).checked_mul(alpha.height as usize).filter(|n| *n <= MAX_IMAGE_PIXELS) else {
                        self.out.truncated = true;
                        return;
                    };
                    let mut data = Vec::with_capacity(length.saturating_mul(3));
                    for _ in 0..length {
                        data.extend_from_slice(&color[..3]);
                    }
                    let data = ImageData::Rgb(hayro::hayro_interpret::RgbData {
                        data,
                        width: alpha.width,
                        height: alpha.height,
                        interpolate: alpha.interpolate,
                        scale_factors: alpha.scale_factors,
                    });
                    self.raster(&data, Some(&alpha), transform);
                },
                None,
            ),
        }
        if !decoded {
            self.out.unreadable += 1;
        }
    }
}
/// Extract painted geometry, glyph outlines and raster edges. Complex soft masks
/// and patterns are reported separately until their visibility has been evaluated.
pub fn extract(bytes: Arc<Vec<u8>>, page: usize, config: &RenderConfig) -> Result<VisibleGeometry, String> {
    catch_unwind(AssertUnwindSafe(|| {
        let objects = Arc::new(pdfcraft_cos::Document::open_with_password(bytes.clone(), config.password.as_deref()).map_err(|e| e.to_string())?);
        let pdf =
            Pdf::new_with_password(bytes, config.password.as_deref().unwrap_or("")).map_err(|_| "the document cannot be interpreted".to_string())?;
        let pages = pdf.pages();
        let p = pages.get(page).ok_or_else(|| "measurement page does not exist".to_string())?;
        let (width, height) = p.render_dimensions();
        let initial = p.initial_transform(true).to_kurbo();
        if !initial.is_finite() || initial.determinant().abs() < 1e-18 || !width.is_finite() || !height.is_finite() {
            return Err("invalid page transform".into());
        }
        let cache = InterpreterCache::new();
        let failures = Arc::new(AtomicUsize::new(0));
        let limited = Arc::new(AtomicBool::new(false));
        let sink = failures.clone();
        let warning_limit = limited.clone();
        let mut settings = config.settings();
        settings.warning_sink = Arc::new(move |warning| {
            if matches!(warning, hayro::hayro_interpret::InterpreterWarning::ContentStreamLimit) {
                warning_limit.store(true, Ordering::Relaxed);
            } else {
                sink.fetch_add(1, Ordering::Relaxed);
            }
        });
        // One budget for page content, nested forms, appearance streams, Type 3 glyphs
        // and patterns. Repeated references consume work just like distinct streams.
        let budget = Mutex::new((0usize, 0usize, 0usize)); // streams, decoded bytes, encoded bytes
        let decode_limit = limited.clone();
        let decode_failures = failures.clone();
        settings.content_decoder = Some(Arc::new(move |stream| {
            let Ok(mut used) = budget.lock() else {
                decode_failures.fetch_add(1, Ordering::Relaxed);
                return None;
            };
            let encoded = stream.stored_len();
            if used.0 >= MAX_CONTENT_STREAMS || used.1 >= MAX_CONTENT_BYTES || encoded > MAX_CONTENT_BYTES.saturating_sub(used.2) {
                decode_limit.store(true, Ordering::Relaxed);
                return None;
            }
            used.0 += 1;
            used.2 += encoded;
            let id = stream.obj_id();
            let reference = match (u32::try_from(id.obj_number), u16::try_from(id.gen_number)) {
                (Ok(num), Ok(generation)) if num > 0 => pdfcraft_cos::ObjRef::new(num, generation),
                _ => {
                    decode_failures.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            };
            let object = objects.get(reference);
            let pdfcraft_cos::Object::Stream(content) = object.as_ref() else {
                decode_failures.fetch_add(1, Ordering::Relaxed);
                return None;
            };
            let remaining = MAX_CONTENT_BYTES.saturating_sub(used.1).saturating_sub(1);
            match content.decoded_within(remaining) {
                Ok(data) => {
                    used.1 += data.len() + 1;
                    Some(data)
                }
                Err(error) => {
                    // A failed decoder may already have used the remaining budget.
                    // Do not let a chain of corrupt/bomb streams repeat that allocation.
                    used.1 = MAX_CONTENT_BYTES;
                    let text = error.to_string();
                    if text.contains("exceeds") || text.contains("limit") {
                        decode_limit.store(true, Ordering::Relaxed);
                    } else {
                        decode_failures.fetch_add(1, Ordering::Relaxed);
                    }
                    None
                }
            }
        }));
        let mut context = Context::new(initial, Rect::new(0.0, 0.0, f64::from(width), f64::from(height)), &cache, p.xref(), settings);
        let mut device = GeometryDevice {
            out: VisibleGeometry::default(),
            inverse: initial.inverse(),
            clips: Vec::new(),
            clip_edges: 0,
            clip_work: 0,
            instructions: 0,
            image_pixels: 0,
            opacity: Vec::new(),
            masked: false,
        };
        // The interpreter omits an initial clip when it covers the context bounds.
        device.push_clip_path(&ClipPath { path: Rect::new(0.0, 0.0, f64::from(width), f64::from(height)).to_path(0.2), fill: FillRule::NonZero });
        interpret_page(p, &mut context, &mut device);
        device.out.unreadable = device.out.unreadable.saturating_add(failures.load(Ordering::Relaxed));
        device.out.truncated |= limited.load(Ordering::Relaxed);
        Ok(device.out)
    }))
    .unwrap_or_else(|_| Err("measurement geometry could not be interpreted safely".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pdf(content: &str, resources: &str, extra: &str, catalog: &str) -> Arc<Vec<u8>> {
        Arc::new(format!("%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R {catalog} >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << {resources} >> /Contents 4 0 R >> endobj\n4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n{extra}\ntrailer << /Root 1 0 R >>\n%%EOF",content.len()).into_bytes())
    }
    fn has_segment(g: &VisibleGeometry, a: [f64; 2], b: [f64; 2]) -> bool {
        g.segments.iter().any(|s| {
            let close = |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-6;
            close(s[0], a) && close(s[1], b) || close(s[0], b) && close(s[1], a)
        })
    }
    #[test]
    fn clips_segments_and_keeps_only_visible_targets() {
        let g = extract(pdf("20 20 40 40 re W n 0 40 m 100 40 l S", "", "", ""), 0, &RenderConfig::default()).unwrap();
        assert!(has_segment(&g, [20.0, 40.0], [60.0, 40.0]), "{:?}", g.segments);
        assert!(!g.endpoints.contains(&[0.0, 40.0]));
        assert!(!g.endpoints.contains(&[100.0, 40.0]));
        assert!(g.endpoints.contains(&[20.0, 40.0]));
    }
    #[test]
    fn even_odd_clip_holes_and_restore_state() {
        let g =
            extract(pdf("q 10 10 80 80 re 30 30 40 40 re W* n 5 50 m 95 50 l S Q 5 5 m 95 5 l S", "", "", ""), 0, &RenderConfig::default()).unwrap();
        assert!(has_segment(&g, [10.0, 50.0], [30.0, 50.0]), "{:?}", g.segments);
        assert!(has_segment(&g, [70.0, 50.0], [90.0, 50.0]));
        assert!(has_segment(&g, [5.0, 5.0], [95.0, 5.0]));
    }
    #[test]
    fn clipping_implicitly_closes_each_open_subpath() {
        let g = extract(
            pdf("10 10 m 90 10 l 90 90 l 10 90 l 30 30 m 70 30 l 70 70 l 30 70 l W* n 5 50 m 95 50 l S", "", "", ""),
            0,
            &RenderConfig::default(),
        )
        .unwrap();
        assert!(has_segment(&g, [10.0, 50.0], [30.0, 50.0]), "{:?}", g.segments);
        assert!(has_segment(&g, [70.0, 50.0], [90.0, 50.0]));
    }
    #[test]
    fn layer_visibility_matches_the_renderer_settings() {
        let bytes = pdf(
            "/OC /L1 BDC 10 20 m 80 20 l S EMC 10 40 m 80 40 l S",
            "/Properties << /L1 5 0 R >>",
            "5 0 obj << /Type /OCG /Name (Construction) >> endobj",
            "/OCProperties << /OCGs [5 0 R] /D << /BaseState /OFF /Order [5 0 R] >> >>",
        );
        let hidden = extract(bytes.clone(), 0, &RenderConfig::default()).unwrap();
        assert!(!has_segment(&hidden, [10.0, 20.0], [80.0, 20.0]));
        assert!(has_segment(&hidden, [10.0, 40.0], [80.0, 40.0]));
        let visible = extract(bytes, 0, &RenderConfig { layers: Arc::new(vec![(5, 0, true)]), ..RenderConfig::default() }).unwrap();
        assert!(has_segment(&visible, [10.0, 20.0], [80.0, 20.0]));
    }
    #[test]
    fn invisible_glyphs_and_zero_alpha_paths_are_omitted() {
        let resources = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> /ExtGState << /Zero << /CA 0 /ca 0 >> >>";
        let hidden =
            extract(pdf("q /Zero gs 10 10 m 80 10 l S Q BT /F1 12 Tf 3 Tr 10 20 Td (A) Tj ET", resources, "", ""), 0, &RenderConfig::default())
                .unwrap();
        assert!(hidden.segments.is_empty());
        let visible = extract(pdf("BT /F1 12 Tf 10 20 Td (A) Tj ET", resources, "", ""), 0, &RenderConfig::default()).unwrap();
        assert_eq!(visible.glyphs, 1);
        assert!(visible.segments.len() > 3);
    }
    #[test]
    fn raster_edges_have_correct_image_and_page_transforms() {
        let bytes = pdf(
            "q 40 0 0 40 20 20 cm /I Do Q",
            "/XObject << /I 5 0 R >>",
            "5 0 obj << /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length 9 >> stream\n00ff00ff>\nendstream endobj",
            "",
        );
        let g = extract(bytes, 0, &RenderConfig::default()).unwrap();
        assert_eq!(g.images, 1);
        assert!(has_segment(&g, [40.0, 20.0], [40.0, 60.0]), "{:?}", g.segments);
        assert!(!g.truncated);
    }
    #[test]
    fn page_and_nested_content_share_decoding_limits() {
        // Raw content must obey the same limit as compressed content.
        let large = " ".repeat(MAX_CONTENT_BYTES + 1);
        let raw = extract(pdf(&large, "", "", ""), 0, &RenderConfig::default()).unwrap();
        assert!(raw.truncated);
        assert!(raw.segments.is_empty());

        let bomb = pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), large.as_bytes());
        let hex: String = bomb.raw.iter().map(|b| format!("{b:02x}")).collect();
        let nested = format!(
            "5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Filter [/ASCIIHexDecode /FlateDecode] /Length {} >> stream\n{hex}>\nendstream endobj",
            hex.len() + 1
        );
        let form = extract(pdf("/F Do", "/XObject << /F 5 0 R >>", &nested, ""), 0, &RenderConfig::default()).unwrap();
        assert!(form.truncated);
        assert!(form.segments.is_empty());

        // Repeated, individually small forms must not multiply the page budget.
        let body = " ".repeat(MAX_CONTENT_BYTES / 4);
        let form = format!("5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >> stream\n{body}\nendstream endobj", body.len());
        let repeated = extract(pdf("/F Do /F Do /F Do /F Do /F Do", "/XObject << /F 5 0 R >>", &form, ""), 0, &RenderConfig::default()).unwrap();
        assert!(repeated.truncated);
    }

    #[test]
    fn content_array_limit_and_unreadable_streams_are_reported() {
        let input = pdf("", "", "", "");
        let refs = "4 0 R ".repeat(MAX_CONTENT_STREAMS + 1);
        let bytes = String::from_utf8(input.as_ref().clone()).unwrap().replace("/Contents 4 0 R", &format!("/Contents [{refs}]"));
        let result = extract(Arc::new(bytes.into_bytes()), 0, &RenderConfig::default()).unwrap();
        assert!(result.truncated);
        let malformed = pdf(
            "/F Do",
            "/XObject << /F 5 0 R >>",
            "5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Filter /UnsupportedDecode /Length 1 >> stream\nx\nendstream endobj",
            "",
        );
        let result = extract(malformed, 0, &RenderConfig::default()).unwrap();
        assert!(result.unreadable > 0);
    }

    #[test]
    fn missing_content_reference_does_not_hide_later_valid_content() {
        let input = pdf("10 10 m 50 50 l S", "", "", "");
        let bytes = String::from_utf8(input.as_ref().clone()).unwrap().replace("/Contents 4 0 R", "/Contents [99999 0 R 4 0 R]");
        let result = extract(Arc::new(bytes.into_bytes()), 0, &RenderConfig::default()).unwrap();
        assert!(result.unreadable > 0);
        assert!(!result.segments.is_empty());
        let missing = "99999 0 R ".repeat(MAX_CONTENT_STREAMS + 1);
        let bytes = String::from_utf8(input.as_ref().clone()).unwrap().replace("/Contents 4 0 R", &format!("/Contents [{missing} 4 0 R]"));
        let result = extract(Arc::new(bytes.into_bytes()), 0, &RenderConfig::default()).unwrap();
        assert!(result.truncated);
        assert!(result.segments.is_empty());
    }

    #[test]
    fn huge_images_and_instruction_work_are_bounded() {
        let bytes = pdf(
            "q 40 0 0 40 20 20 cm /I Do Q",
            "/XObject << /I 5 0 R >>",
            "5 0 obj << /Type /XObject /Subtype /Image /Width 1000000 /Height 1000000 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >> stream\nx\nendstream endobj",
            "",
        );
        let g = extract(bytes, 0, &RenderConfig::default()).unwrap();
        assert!(g.truncated || g.unreadable > 0);
        let content = "0 g ".repeat(MAX_INSTRUCTIONS + 1);
        let g = extract(pdf(&content, "", "", ""), 0, &RenderConfig::default()).unwrap();
        assert!(g.truncated);
    }
}
