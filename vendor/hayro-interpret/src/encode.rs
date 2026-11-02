//! Encoding shading patterns for easy sampling.

use crate::color::{AlphaColor, ColorComponents, ColorSpace};
use crate::interpret::state::ActiveTransferFunction;
use crate::pattern::ShadingPattern;
use crate::shading::{ShadingFunction, ShadingType, Triangle, TriangleVertex};
use kurbo::{Affine, Point};
use rustc_hash::FxHashMap;
use smallvec::{ToSmallVec, smallvec};

/// A shading pattern that was encoded so it can be sampled.
#[derive(Debug)]
pub struct EncodedShadingPattern {
    /// The base transform of the shading pattern.
    pub base_transform: Affine,
    pub(crate) color_space: ColorSpace,
    pub(crate) background_color: AlphaColor,
    pub(crate) shading_type: EncodedShadingType,
    pub(crate) opacity: f32,
    pub(crate) transfer_function: Option<ActiveTransferFunction>,
    /// PdfCraft patch (35): where the shading can paint at all, in device space: for a mesh
    /// (types 4–7), the pixels its triangles touched when it was encoded (empty when none
    /// lies in the area); `None` for the shading types that paint everywhere (function-based,
    /// axial, radial). The renderer samples a texture only there, and skips a mesh that
    /// touches nothing: painted over the whole canvas, 310 one-triangle meshes on a plan cost
    /// 310 full tiles per tile, and the watchdog gave the page up.
    pub extent: Option<kurbo::Rect>,
}

impl EncodedShadingPattern {
    /// Sample the shading at the given position.
    #[inline]
    pub fn sample(&self, pos: Point) -> [f32; 4] {
        self.shading_type
            .eval(pos, self.background_color, &self.color_space)
            .map(|v| {
                let mut components = v.components();
                components[3] *= self.opacity;

                if let Some(tf) = &self.transfer_function {
                    return tf.apply(&AlphaColor::new(components)).components();
                }

                components
            })
            .unwrap_or([0.0, 0.0, 0.0, 0.0])
    }
}

impl ShadingPattern {
    /// Encode the shading pattern.
    pub fn encode(&self) -> EncodedShadingPattern {
        self.encode_within(None)
    }

    /// PdfCraft patch: encode the shading pattern for drawing inside `area` (device space) only.
    /// Mesh shadings are sampled pixel by pixel, and only there (see `TriangleSampler`).
    pub fn encode_within(&self, area: Option<kurbo::Rect>) -> EncodedShadingPattern {
        let base_transform;
        // PdfCraft patch (35): see `EncodedShadingPattern::extent`.
        let mut extent = None;

        let shading_type = match self.shading.shading_type.as_ref() {
            ShadingType::FunctionBased {
                domain,
                matrix,
                function,
            } => {
                base_transform = (self.matrix * *matrix).inverse();
                encode_function_shading(domain, function)
            }
            ShadingType::RadialAxial {
                coords,
                domain,
                function,
                extend,
                axial,
            } => {
                let (encoded, initial_transform) =
                    encode_axial_shading(*coords, *domain, function, *extend, *axial);

                base_transform = initial_transform * self.matrix.inverse();

                encoded
            }
            ShadingType::TriangleMesh {
                triangles,
                function,
            } => {
                let full_transform = self.matrix;
                let mut sampler = TriangleSampler::new(area);
                sampler.sample(triangles, full_transform);
                extent = Some(sampler.extent());
                let samples = sampler.map;

                base_transform = Affine::IDENTITY;

                EncodedShadingType::Sampled {
                    samples,
                    function: function.clone(),
                }
            }
            ShadingType::CoonsPatchMesh { patches, function } => {
                // PdfCraft patch: one patch's triangles at a time (each is 722); collecting all
                // of them first held 150 KB per patch before any limit applied.
                let full_transform = self.matrix;
                let mut sampler = TriangleSampler::new(area);
                let mut triangles = vec![];
                for patch in patches {
                    if !mesh_budget_left() {
                        break;
                    }
                    triangles.clear();
                    patch.to_triangles(&mut triangles);
                    if !sampler.sample_owned(&mut triangles, full_transform) {
                        break;
                    }
                }
                extent = Some(sampler.extent());
                let samples = sampler.map;

                base_transform = Affine::IDENTITY;

                EncodedShadingType::Sampled {
                    samples,
                    function: function.clone(),
                }
            }
            ShadingType::TensorProductPatchMesh { patches, function } => {
                // PdfCraft patch: one patch's triangles at a time, as for Coons patches.
                let full_transform = self.matrix;
                let mut sampler = TriangleSampler::new(area);
                let mut triangles = vec![];
                for patch in patches {
                    if !mesh_budget_left() {
                        break;
                    }
                    triangles.clear();
                    patch.to_triangles(&mut triangles);
                    if !sampler.sample_owned(&mut triangles, full_transform) {
                        break;
                    }
                }
                extent = Some(sampler.extent());
                let samples = sampler.map;

                base_transform = Affine::IDENTITY;

                EncodedShadingType::Sampled {
                    samples,
                    function: function.clone(),
                }
            }
            ShadingType::Dummy => {
                base_transform = Affine::IDENTITY;

                EncodedShadingType::Dummy
            }
        };

        let color_space = self.shading.color_space.clone();

        let background_color = self
            .shading
            .background
            .as_ref()
            .map(|b| color_space.to_rgba(b, 1.0, false))
            .unwrap_or(AlphaColor::TRANSPARENT);

        EncodedShadingPattern {
            color_space,
            background_color,
            shading_type,
            base_transform,
            opacity: self.opacity,
            transfer_function: self.transfer_function.clone(),
            // A `/Background` is painted wherever the triangles aren't (pattern fills), so such
            // a mesh has no extent to confine it to.
            extent: if self.shading.background.is_some() { None } else { extent },
        }
    }
}

fn encode_axial_shading(
    coords: [f32; 6],
    domain: [f32; 2],
    function: &ShadingFunction,
    extend: [bool; 2],
    is_axial: bool,
) -> (EncodedShadingType, Affine) {
    let initial_transform;

    let params = if is_axial {
        let [x_0, y_0, x_1, y_1, _, _] = coords;

        initial_transform = ts_from_line_to_line(
            Point::new(x_0 as f64, y_0 as f64),
            Point::new(x_1 as f64, y_1 as f64),
            Point::ZERO,
            Point::new(1.0, 0.0),
        );

        RadialAxialParams::Axial
    } else {
        let [x_0, y_0, r0, x_1, y_1, r_1] = coords;

        initial_transform = Affine::translate((-x_0 as f64, -y_0 as f64));
        let new_x1 = x_1 - x_0;
        let new_y1 = y_1 - y_0;

        let p1 = Point::new(new_x1 as f64, new_y1 as f64);
        let r = Point::new(r0 as f64, r_1 as f64);

        RadialAxialParams::Radial { p1, r }
    };

    (
        EncodedShadingType::RadialAxial {
            function: function.clone(),
            params,
            domain,
            extend,
        },
        initial_transform,
    )
}

/// PdfCraft patch: pixel visits one mesh shading may make, per pixel of the area it is drawn in
/// and per triangle. Sampling walks each triangle's bounding box (rounded out to whole pixels),
/// so triangles far off the page (a fuzzed patch decoded onto ±40000) asked for 65536 × 65536
/// samples, and many triangles covering the same pixels repeat the work. A triangle's box is
/// about twice its area, and a sub-pixel triangle's box is at most 2 × 2 pixels.
const MESH_VISITS_PER_PIXEL: u64 = 8;
const MESH_VISITS_PER_TRIANGLE: u64 = 4;

/// PdfCraft patch: the visits allowed when no area is given (the public `encode`).
const MESH_VISITS_WITHOUT_AREA: u64 = 1 << 24;

/// PdfCraft patch: mesh triangles one page may sample in all, across every shading painted on
/// it. A shading is triangulated again each time it is painted (2^16 patches are 47 million
/// triangles, several seconds), so painting one a thousand times took hours. A 200 × 200
/// gradient mesh is 29 million triangles.
const MAX_PAGE_MESH_TRIANGLES: u64 = 1 << 25;

std::thread_local! {
    static MESH_TRIANGLES_LEFT: core::cell::Cell<u64> =
        const { core::cell::Cell::new(MAX_PAGE_MESH_TRIANGLES) };
}

/// PdfCraft patch: a page starts with a fresh mesh triangle budget (called from
/// `Context::new`).
pub(crate) fn reset_mesh_budget() {
    MESH_TRIANGLES_LEFT.with(|n| n.set(MAX_PAGE_MESH_TRIANGLES));
}

/// PdfCraft patch: `false` once this page's mesh triangle budget is spent.
fn mesh_budget_left() -> bool {
    MESH_TRIANGLES_LEFT.with(|n| n.get() > 0)
}

/// PdfCraft patch: samples mesh triangles into a map with one entry per pixel, only inside
/// `area` (device space), within the visit budget above.
struct TriangleSampler {
    map: FxHashMap<(u16, u16), ColorComponents>,
    area: kurbo::Rect,
    visits_left: u64,
    /// PdfCraft patch (35): the pixels sampled so far, as `(x0, y0, x1, y1)` inclusive.
    touched: Option<(u16, u16, u16, u16)>,
}

impl TriangleSampler {
    fn new(area: Option<kurbo::Rect>) -> Self {
        let visits_left = match area {
            Some(a) => (a.width().max(0.0).ceil() as u64)
                .saturating_mul(a.height().max(0.0).ceil() as u64)
                .saturating_mul(MESH_VISITS_PER_PIXEL),
            None => MESH_VISITS_WITHOUT_AREA,
        };
        Self {
            map: FxHashMap::default(),
            area: area.unwrap_or(kurbo::Rect::new(0.0, 0.0, 65536.0, 65536.0)),
            visits_left,
            touched: None,
        }
    }

    /// PdfCraft patch (35): the device-space rectangle covering every pixel sampled so far
    /// (whole pixels), or an empty rectangle when no triangle touched the area.
    fn extent(&self) -> kurbo::Rect {
        match self.touched {
            Some((x0, y0, x1, y1)) => kurbo::Rect::new(
                f64::from(x0),
                f64::from(y0),
                f64::from(x1) + 1.0,
                f64::from(y1) + 1.0,
            ),
            None => kurbo::Rect::ZERO,
        }
    }

    /// Sample `triangles`; `false` once a budget is spent (the rest is left unsampled).
    fn sample(&mut self, triangles: &[Triangle], transform: Affine) -> bool {
        if !self.take_page_budget(triangles.len()) {
            return false;
        }
        triangles.iter().all(|t| {
            self.sample_one(transformed(
                t.p0.clone(),
                t.p1.clone(),
                t.p2.clone(),
                transform,
            ))
        })
    }

    /// PdfCraft patch (33): [`Self::sample`] for triangles made for this sampling alone (a
    /// patch's), moving their vertices instead of copying each one's colours (`triangles` is
    /// left empty).
    fn sample_owned(&mut self, triangles: &mut Vec<Triangle>, transform: Affine) -> bool {
        if !self.take_page_budget(triangles.len()) {
            return false;
        }
        triangles
            .drain(..)
            .all(|t| self.sample_one(transformed(t.p0, t.p1, t.p2, transform)))
    }

    /// Take `count` triangles from the page's budget; `false` (and none left) when it is short.
    fn take_page_budget(&mut self, count: usize) -> bool {
        let page_left = MESH_TRIANGLES_LEFT.with(core::cell::Cell::get);
        if page_left < count as u64 {
            MESH_TRIANGLES_LEFT.with(|n| n.set(0));
            warn!("page mesh triangle budget exceeded; the rest of the shading is not drawn");
            return false;
        }
        MESH_TRIANGLES_LEFT.with(|n| n.set(page_left - count as u64));
        true
    }

    /// Sample one triangle in device space; `false` once the visit budget is spent.
    fn sample_one(&mut self, t: Triangle) -> bool {
        // Only the part of the triangle inside `area`; only such triangles add visits.
        let bbox = t.bounding_box().intersect(self.area);
        if bbox.width() <= 0.0 || bbox.height() <= 0.0 {
            return true;
        }
        self.visits_left = self.visits_left.saturating_add(MESH_VISITS_PER_TRIANGLE);

        for y in (bbox.y0.floor() as u16)..(bbox.y1.ceil() as u16) {
            for x in (bbox.x0.floor() as u16)..(bbox.x1.ceil() as u16) {
                if self.visits_left == 0 {
                    warn!("mesh shading visit budget exceeded; the rest is not drawn");
                    return false;
                }
                self.visits_left -= 1;
                let corner = Point::new(x as f64, y as f64);
                let sampled = if t.contains_point(corner) {
                    self.map.insert((x, y), t.interpolate(corner));
                    true
                } else if t.covers(Point::new(x as f64 + 0.5, y as f64 + 0.5)) {
                    // PdfCraft patch (36): a pixel whose corner no triangle holds strictly (it
                    // lies on an edge two triangles share, or on the mesh's side) still belongs
                    // to the triangle that covers its centre, edges included. Testing only the
                    // corner left a white seam along every shared edge. Such a pixel takes the
                    // colour at its corner, as every other pixel does (where MuPDF's colours
                    // agree), unless a triangle that holds the corner gives it one.
                    self.map.entry((x, y)).or_insert_with(|| t.interpolate(corner));
                    true
                } else {
                    false
                };
                if sampled {
                    self.touched = Some(match self.touched {
                        Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                        None => (x, y, x, y),
                    });
                }
            }
        }
        true
    }
}

/// The triangle `v0`, `v1`, `v2` with its points mapped through `transform` (its colours as
/// they are).
fn transformed(
    mut v0: TriangleVertex,
    mut v1: TriangleVertex,
    mut v2: TriangleVertex,
    transform: Affine,
) -> Triangle {
    v0.point = transform * v0.point;
    v1.point = transform * v1.point;
    v2.point = transform * v2.point;
    Triangle::new(v0, v1, v2)
}

fn encode_function_shading(domain: &[f32; 4], function: &ShadingFunction) -> EncodedShadingType {
    let domain = kurbo::Rect::new(
        domain[0] as f64,
        domain[2] as f64,
        domain[1] as f64,
        domain[3] as f64,
    );

    EncodedShadingType::FunctionBased {
        domain,
        function: function.clone(),
    }
}

#[derive(Debug)]
pub(crate) enum RadialAxialParams {
    Axial,
    Radial { p1: Point, r: Point },
}

#[derive(Debug)]
pub(crate) enum EncodedShadingType {
    FunctionBased {
        domain: kurbo::Rect,
        function: ShadingFunction,
    },
    RadialAxial {
        function: ShadingFunction,
        params: RadialAxialParams,
        domain: [f32; 2],
        extend: [bool; 2],
    },
    Sampled {
        samples: FxHashMap<(u16, u16), ColorComponents>,
        function: Option<ShadingFunction>,
    },
    Dummy,
}

impl EncodedShadingType {
    pub(crate) fn eval(
        &self,
        pos: Point,
        bg_color: AlphaColor,
        color_space: &ColorSpace,
    ) -> Option<AlphaColor> {
        match self {
            Self::FunctionBased { domain, function } => {
                if !domain.contains(pos) {
                    Some(bg_color)
                } else {
                    let out = function.eval(&smallvec![pos.x as f32, pos.y as f32])?;
                    // TODO: Clamp out-of-range values.
                    Some(color_space.to_rgba(&out, 1.0, false))
                }
            }
            Self::RadialAxial {
                function,
                params,
                domain,
                extend,
            } => {
                let (t0, t1) = (domain[0], domain[1]);

                let mut t = match params {
                    RadialAxialParams::Axial => pos.x as f32,
                    RadialAxialParams::Radial { p1, r } => {
                        radial_pos(&pos, p1, *r, extend[0], extend[1]).unwrap_or(f32::MIN)
                    }
                };

                if t == f32::MIN {
                    return Some(bg_color);
                }

                if t < 0.0 {
                    if extend[0] {
                        t = 0.0;
                    } else {
                        return Some(bg_color);
                    }
                } else if t > 1.0 {
                    if extend[1] {
                        t = 1.0;
                    } else {
                        return Some(bg_color);
                    }
                }

                let t = t0 + (t1 - t0) * t;

                let val = function.eval(&smallvec![t])?;

                Some(color_space.to_rgba(&val, 1.0, false))
            }
            Self::Sampled { samples, function } => {
                let sample_point = (pos.x as u16, pos.y as u16);

                if let Some(color) = samples.get(&sample_point) {
                    if let Some(function) = function {
                        let val = function.eval(&color.to_smallvec())?;
                        Some(color_space.to_rgba(&val, 1.0, false))
                    } else {
                        Some(color_space.to_rgba(color, 1.0, false))
                    }
                } else {
                    Some(bg_color)
                }
            }
            Self::Dummy => Some(AlphaColor::TRANSPARENT),
        }
    }
}

fn ts_from_line_to_line(src1: Point, src2: Point, dst1: Point, dst2: Point) -> Affine {
    let unit_to_line1 = unit_to_line(src1, src2);
    let line1_to_unit = unit_to_line1.inverse();
    let unit_to_line2 = unit_to_line(dst1, dst2);

    unit_to_line2 * line1_to_unit
}

fn unit_to_line(p0: Point, p1: Point) -> Affine {
    Affine::new([
        p1.y - p0.y,
        p0.x - p1.x,
        p1.x - p0.x,
        p1.y - p0.y,
        p0.x,
        p0.y,
    ])
}

fn radial_pos(
    pos: &Point,
    p1: &Point,
    r: Point,
    min_extend: bool,
    max_extend: bool,
) -> Option<f32> {
    let r0 = r.x as f32;
    let dx = p1.x as f32;
    let dy = p1.y as f32;
    let dr = r.y as f32 - r0;

    let px = pos.x as f32;
    let py = pos.y as f32;

    let a = dx * dx + dy * dy - dr * dr;
    let b = -2.0 * (px * dx + py * dy + r0 * dr);
    let c = px * px + py * py - r0 * r0;

    let discriminant = b * b - 4.0 * a * c;

    // No solution available.
    if discriminant < 0.0 {
        return None;
    }

    if a.abs() < 1e-6 {
        if b.abs() < 1e-6 {
            return None;
        }

        let t = -c / b;

        if (!min_extend && t < 0.0) || (!max_extend && t > 1.0) {
            return None;
        }

        let r_t = r0 + dr * t;
        if r_t < 0.0 {
            return None;
        }

        return Some(t);
    }

    let sqrt_d = discriminant.sqrt();
    let t1 = (-b - sqrt_d) / (2.0 * a);
    let t2 = (-b + sqrt_d) / (2.0 * a);

    let max = t1.max(t2);
    let mut take_max = Some(max);
    let min = t1.min(t2);
    let mut take_min = Some(min);

    if (!min_extend && min < 0.0) || r0 + dr * min < 0.0 {
        take_min = None;
    }

    if (!max_extend && max > 1.0) || r0 + dr * max < 0.0 {
        take_max = None;
    }

    match (take_min, take_max) {
        (Some(_), Some(max)) => Some(max),
        (Some(min), None) => Some(min),
        (None, Some(max)) => Some(max),
        (None, None) => None,
    }
}
