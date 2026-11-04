//! The camera, the render options and [`render`]: world meshes to an RGBA image.
//!
//! One sample per pixel centre, no antialiasing. Faces are lit on both sides because a
//! producer's winding is not trusted to face outward.

use crate::WorldMesh;
use crate::raster::{Corner, Target, View, ViewCorner, at};
use crate::vec3::{cross, dot, length, normalize, scale, sub, unit_cross};
use pdfcraft_prc::TriangleMesh;

/// The largest image [`render`] draws, in pixels (width × height).
pub const MAX_RENDER_PIXELS: u64 = 16 * 1024 * 1024;

/// The most pixel centres one [`render`] tests, summed over each drawn triangle's bounding box
/// clipped to the image: 128 full layers of the largest image.
pub const MAX_RENDER_WORK: u64 = 1 << 31;

/// [`Camera::fit`]'s framed extent over the model's: 5% clear on each side.
const FIT_MARGIN: f64 = 1.1;

/// How the camera projects the scene onto the image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// A pinhole camera with this vertical field of view, in degrees (exclusive range 0–180).
    Perspective {
        /// Vertical field of view, degrees.
        fov_y: f64,
    },
    /// A parallel projection showing this much of the scene vertically, in model units.
    Orthographic {
        /// Visible height, model units.
        height: f64,
    },
}

/// Where the camera is and where it looks, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The camera position.
    pub eye: [f64; 3],
    /// The point at the image centre.
    pub target: [f64; 3],
    /// The direction that appears upward; must not be parallel to the view direction.
    pub up: [f64; 3],
    /// The projection.
    pub projection: Projection,
}

/// An axis-aligned box around every point a set of meshes draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// The smallest x, y and z.
    pub min: [f64; 3],
    /// The largest x, y and z.
    pub max: [f64; 3],
}

impl Bounds {
    /// The box around every finite vertex the meshes' triangles use, or `None` when there is none.
    #[must_use]
    pub fn of(meshes: &[WorldMesh]) -> Option<Bounds> {
        let mut out: Option<Bounds> = None;
        for p in vertices(meshes) {
            let b = out.get_or_insert(Bounds { min: p, max: p });
            for ((lo, hi), c) in b.min.iter_mut().zip(b.max.iter_mut()).zip(p) {
                *lo = lo.min(c);
                *hi = hi.max(c);
            }
        }
        out
    }

    /// The box's centre, finite for any finite box.
    #[must_use]
    pub fn centre(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| at(self.min, i) / 2.0 + at(self.max, i) / 2.0)
    }

    /// Half the box's diagonal: the radius of a sphere holding it.
    #[must_use]
    pub fn radius(&self) -> f64 {
        length(sub(self.max, self.min)) / 2.0
    }
}

/// Every corner of the triangles [`render`] draws: those whose three corners exist and are finite.
fn vertices(meshes: &[WorldMesh]) -> impl Iterator<Item = [f64; 3]> + Clone + '_ {
    meshes.iter().flat_map(|m| m.mesh.triangles.iter().filter_map(|tri| corners(&m.mesh, tri)).flatten())
}

impl Camera {
    /// A camera looking along `direction` that fits `bounds` in an image of the given aspect
    /// ratio (width / height), with a 30° perspective or an orthographic projection: the box's
    /// eight corners framed from this direction, about 5% clear on each side of the tighter axis.
    ///
    /// # Errors
    /// [`RenderError::Camera`] when `direction` is zero or parallel to `up`, or `aspect` is not positive.
    pub fn fit(bounds: &Bounds, direction: [f64; 3], up: [f64; 3], perspective: bool, aspect: f64) -> Result<Camera, RenderError> {
        let corners = (0..8).map(|k| [0, 1, 2].map(|i| if k >> i & 1 == 0 { at(bounds.min, i) } else { at(bounds.max, i) }));
        Camera::frame(bounds, corners, direction, up, (perspective, aspect))
    }

    /// As [`Camera::fit`], framing every vertex the meshes' triangles use rather than their box,
    /// so an oblique view is not padded by empty box corners.
    ///
    /// # Errors
    /// As [`Camera::fit`], and when the meshes have no finite vertex.
    pub fn fit_meshes(meshes: &[WorldMesh], direction: [f64; 3], up: [f64; 3], perspective: bool, aspect: f64) -> Result<Camera, RenderError> {
        let bounds = Bounds::of(meshes).ok_or(RenderError::Camera("the model has no vertex"))?;
        Camera::frame(&bounds, vertices(meshes), direction, up, (perspective, aspect))
    }

    /// Frames `points` (which `bounds` holds) from `direction`, aimed at the middle of their projection.
    fn frame(
        bounds: &Bounds,
        points: impl Iterator<Item = [f64; 3]> + Clone,
        direction: [f64; 3],
        up: [f64; 3],
        (perspective, aspect): (bool, f64),
    ) -> Result<Camera, RenderError> {
        if !(aspect.is_finite() && aspect > 0.0) {
            return Err(RenderError::Camera("the aspect ratio is not positive"));
        }
        let dir = normalize(direction).ok_or(RenderError::Camera("the view direction is zero"))?;
        let centre = bounds.centre();
        let radius = match bounds.radius() {
            r if r.is_finite() && r > 0.0 => r,
            _ => 1.0,
        };
        let [right, upward, forward] = axes(dir, up)?;
        let view = |p: [f64; 3]| {
            let r = sub(p, centre);
            [dot(r, right), dot(r, upward), dot(r, forward)]
        };
        let ((x0, x1), (y0, y1)) = points
            .clone()
            .map(view)
            .fold(((f64::INFINITY, f64::NEG_INFINITY), (f64::INFINITY, f64::NEG_INFINITY)), |((a0, a1), (b0, b1)), [x, y, _]| {
                ((a0.min(x), a1.max(x)), (b0.min(y), b1.max(y)))
            });
        if !(x0 <= x1 && y0 <= y1) {
            return Err(RenderError::Camera("the model has no vertex"));
        }
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let target = [0, 1, 2].map(|i| at(centre, i) + mx * at(right, i) + my * at(upward, i));
        let floor = radius * 1e-3;
        let (projection, distance) = if perspective {
            let fov_y: f64 = 30.0;
            let ty = (fov_y.to_radians() / 2.0).tan() / FIT_MARGIN;
            let tx = ty * aspect;
            // A point at lateral (x, y) from the target and depth f past it is in view when
            // |x| <= (d + f)·tx and |y| <= (d + f)·ty.
            let d = points.map(view).fold(floor, |d, [x, y, f]| {
                let need = ((x - mx).abs() / tx).max((y - my).abs() / ty);
                d.max(need - f).max(floor - f)
            });
            (Projection::Perspective { fov_y }, d)
        } else {
            let height = ((y1 - y0).max((x1 - x0) / aspect) * FIT_MARGIN).max(floor);
            (Projection::Orthographic { height }, 2.0 * radius)
        };
        let camera = Camera { eye: sub(target, scale(dir, distance)), target, up, projection };
        camera.basis()?;
        Ok(camera)
    }

    /// Right, up and forward unit vectors.
    pub(crate) fn basis(&self) -> Result<[[f64; 3]; 3], RenderError> {
        let forward = normalize(sub(self.target, self.eye)).ok_or(RenderError::Camera("the eye and the target coincide"))?;
        axes(forward, self.up)
    }
}

/// Right, up and `forward` (a unit vector) for a camera whose up direction is `up`.
fn axes(forward: [f64; 3], up: [f64; 3]) -> Result<[[f64; 3]; 3], RenderError> {
    let right = normalize(cross(forward, up)).ok_or(RenderError::Camera("the up direction is zero or parallel to the view direction"))?;
    Ok([right, cross(right, forward), forward])
}

/// How surfaces are lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Lighting {
    /// One light at the camera: a surface's colour times `0.3 + 0.7·|n·v|`, where `v` points from
    /// the surface to the eye (orthographic: against the view direction).
    #[default]
    Headlight,
    /// No shading: every surface in its own colour.
    Unlit,
}

/// What [`render`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Image width, pixels.
    pub width: u32,
    /// Image height, pixels.
    pub height: u32,
    /// Background colour, straight RGBA.
    pub background: [u8; 4],
    /// The colour of a surface with none of its own.
    pub colour: [u8; 3],
    /// How surfaces are lit.
    pub lighting: Lighting,
}

impl Default for RenderOptions {
    /// 1024 × 768, opaque white background, light grey surface, the headlight.
    fn default() -> Self {
        RenderOptions { width: 1024, height: 768, background: [255, 255, 255, 255], colour: [190, 192, 200], lighting: Lighting::Headlight }
    }
}

/// A rendered image: `rgba` holds `width * height` straight-alpha pixels, row by row from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width, pixels.
    pub width: u32,
    /// Height, pixels.
    pub height: u32,
    /// The pixels.
    pub rgba: Vec<u8>,
}

/// Why [`render`], [`Camera::fit`] or [`crate::assemble`] refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// The image is empty or larger than [`MAX_RENDER_PIXELS`].
    #[error("a {width}x{height} image is empty or larger than the render ceiling")]
    Size {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// The camera cannot form a view.
    #[error("invalid camera: {0}")]
    Camera(&'static str),
    /// The model could not be read.
    #[error("unreadable model: {0}")]
    Model(#[from] pdfcraft_prc::PrcError),
    /// The placed model has more than [`crate::MAX_ASSEMBLED_TRIANGLES`] triangles.
    #[error("the placed model has more triangles than the assembly ceiling")]
    TooManyTriangles,
    /// The placed model has more than [`crate::MAX_ASSEMBLED_VERTICES`] positions, normals and
    /// texture coordinates.
    #[error("the placed model has more vertices than the assembly ceiling")]
    TooManyVertices,
    /// Drawing would test more than [`MAX_RENDER_WORK`] pixel centres.
    #[error("the render would test more pixels than the work ceiling")]
    TooMuchWork,
}

/// Draw `meshes` as seen by `camera`.
///
/// Each triangle is filled in its own colour ([`WorldMesh::triangle_colours`]), else its mesh's
/// ([`WorldMesh::colour`]), else [`RenderOptions::colour`], and lit per [`RenderOptions::lighting`];
/// nearer surfaces hide farther ones. Opaque triangles are drawn first; translucent ones are then
/// blended over them, hidden by nearer opaque surfaces but not by each other, in mesh order; fully
/// transparent ones are not drawn. Triangles with a non-finite or out-of-range vertex are skipped.
/// A perspective camera clips at a near plane 1/10,000 of the way from the eye to the target, so
/// what lies behind the eye, or in front of it but nearer than that, is not drawn; an orthographic
/// camera does not clip.
///
/// # Errors
/// [`RenderError::Size`] for an empty or over-ceiling image; [`RenderError::Camera`] for a camera
/// that cannot form a view (eye on the target, `up` parallel to the view, a field of view outside
/// 0–180° or too small for a finite focal length, a non-positive orthographic height);
/// [`RenderError::TooMuchWork`] past [`MAX_RENDER_WORK`].
pub fn render(meshes: &[WorldMesh], camera: &Camera, options: &RenderOptions) -> Result<Image, RenderError> {
    let (width, height) = (options.width, options.height);
    let size = RenderError::Size { width, height };
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_RENDER_PIXELS {
        return Err(size);
    }
    let pixels = usize::try_from(pixels).map_err(|_| size)?;
    let view = View::new(camera, f64::from(width) / f64::from(height))?;
    let perspective = matches!(view, View::Perspective { .. });
    let scene = Scene { eye: camera.eye, basis: camera.basis()?, view, lighting: options.lighting };
    let mut rgba = options.background.repeat(pixels);
    let mut depth = vec![f64::NEG_INFINITY; pixels];
    let mut target = Target { perspective, width, height, rgba: &mut rgba, depth: &mut depth, work: MAX_RENDER_WORK };
    let [r, g, b] = options.colour;
    for opaque in [true, false] {
        for m in meshes {
            for t in 0..m.mesh.triangles.len() {
                let colour = m.colour_of(t).unwrap_or([r, g, b, 255]);
                if (colour[3] == 255) == opaque && colour[3] != 0 {
                    scene.draw(&mut target, &m.mesh, t, colour)?;
                }
            }
        }
    }
    Ok(Image { width, height, rgba })
}

/// The camera a frame is drawn from, with its basis and projection.
struct Scene {
    eye: [f64; 3],
    /// Right, up and forward.
    basis: [[f64; 3]; 3],
    view: View,
    lighting: Lighting,
}

impl Scene {
    /// The shade at world point `p` with normal `n`.
    fn shade(&self, p: [f64; 3], n: [f64; 3]) -> [f64; 3] {
        if self.lighting == Lighting::Unlit {
            return [1.0; 3];
        }
        let towards = match self.view {
            View::Perspective { .. } => normalize(sub(self.eye, p)),
            View::Orthographic { .. } => Some(scale(at3(&self.basis, 2), -1.0)),
        };
        [0.3 + 0.7 * towards.map_or(1.0, |v| dot(n, v).abs().min(1.0)); 3]
    }

    /// Draw triangle `t` of `mesh` into `target` in straight-RGBA `colour`; refused once the
    /// target's work allowance is spent.
    fn draw(&self, target: &mut Target, mesh: &TriangleMesh, t: usize, colour: [u8; 4]) -> Result<(), RenderError> {
        let Some(world) = mesh.triangles.get(t).and_then(|tri| corners(mesh, tri)) else {
            return Ok(());
        };
        let Some(face) = unit_cross(sub(at3(&world, 1), at3(&world, 0)), sub(at3(&world, 2), at3(&world, 0))) else {
            return Ok(());
        };
        let stored = stored_normals(mesh, t);
        let normal = |k: usize| stored.and_then(|n| n.get(k).copied().flatten()).unwrap_or(face);
        // Without stored normals the triangle is flat: one shade, lit along the view ray to its first corner.
        let flat = stored.is_none().then(|| self.shade(at3(&world, 0), normal(0)));
        let [right, up, forward] = self.basis;
        let view_corners = [0, 1, 2].map(|k| -> ViewCorner {
            let p = at3(&world, k);
            let [r, g, b] = flat.unwrap_or_else(|| self.shade(p, normal(k)));
            let d = sub(p, self.eye);
            [dot(d, right), dot(d, up), dot(d, forward), r, g, b]
        });
        let screen: Vec<Corner> = self.view.clip(view_corners).into_iter().map(|v| self.view.project(v, target.width, target.height)).collect();
        if let Some((&a, rest)) = screen.split_first() {
            for pair in rest.windows(2) {
                if let [b, c] = pair {
                    target.fill([a, *b, *c], colour)?;
                }
            }
        }
        Ok(())
    }
}

/// Triangle `t`'s stored corner normals, unit length; a corner whose normal is zero or not finite
/// is `None`, and so is the whole when the mesh stores none.
fn stored_normals(mesh: &TriangleMesh, t: usize) -> Option<[Option<[f64; 3]>; 3]> {
    let slots = mesh.triangle_normals.get(t)?;
    Some(slots.map(|i| mesh.normals.get(usize::try_from(i).ok()?).copied().and_then(normalize)))
}

/// The three corners of a triangle, or `None` when one is missing or not finite.
fn corners(mesh: &TriangleMesh, tri: &[u32; 3]) -> Option<[[f64; 3]; 3]> {
    let mut out = [[0.0; 3]; 3];
    for (slot, &i) in out.iter_mut().zip(tri) {
        let p = *mesh.positions.get(usize::try_from(i).ok()?)?;
        if !p.iter().all(|c| c.is_finite()) {
            return None;
        }
        *slot = p;
    }
    Some(out)
}

/// Corner `i` of a triangle, the origin when out of range.
fn at3(v: &[[f64; 3]; 3], i: usize) -> [f64; 3] {
    v.get(i).copied().unwrap_or([0.0; 3])
}

#[cfg(test)]
mod tests;
