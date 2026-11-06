//! Picking: the model point under an image position, the inverse of the projection
//! [`crate::render`] draws with, so a pick lands on what the image shows there.

use crate::raster::View;
use crate::render::{corners, invisible};
use crate::vec3::{add, cross, dot, normalize, scale, sub, unit_cross};
use crate::{Camera, RenderError, WorldMesh};

/// A half-line through the scene, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Where the ray starts: the eye (perspective), or the point through the image position on
    /// the plane through the eye (orthographic).
    pub origin: [f64; 3],
    /// The unit direction.
    pub direction: [f64; 3],
    /// The smallest distance along the ray that counts as a hit: the renderer's near plane under
    /// perspective, negative infinity under orthographic projection (which draws behind the eye).
    pub min_distance: f64,
}

/// The nearest triangle a [`Ray`] meets ([`pick`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// The index of the mesh in the slice picked from.
    pub mesh: usize,
    /// The index of the triangle in that mesh's `triangles`.
    pub triangle: usize,
    /// The hit point, world coordinates.
    pub point: [f64; 3],
    /// The triangle's unit face normal from its winding, not turned towards the ray: a hit on a
    /// back face reports a normal pointing away from the viewer.
    pub normal: [f64; 3],
    /// The distance from [`Ray::origin`] to [`Self::point`].
    pub distance: f64,
}

impl Camera {
    /// The ray through image position (`x`, `y`) of a `width` × `height` rendering from this
    /// camera, y down: pixel (i, j)'s centre is (i + 0.5, j + 0.5). [`pick`] along it finds the
    /// surface [`crate::render`] draws at that pixel.
    ///
    /// # Errors
    /// [`RenderError::Size`] for a zero `width` or `height`; [`RenderError::Camera`] for a camera
    /// [`crate::render`] refuses or a position that is not finite.
    pub fn ray_through(&self, x: f64, y: f64, width: u32, height: u32) -> Result<Ray, RenderError> {
        if width == 0 || height == 0 {
            return Err(RenderError::Size { width, height });
        }
        if !(x.is_finite() && y.is_finite()) {
            return Err(RenderError::Camera("the image position is not a finite number"));
        }
        let aspect = f64::from(width) / f64::from(height);
        let view = View::new(self, aspect)?;
        let [right, up, forward] = self.basis()?;
        let nx = x / f64::from(width) * 2.0 - 1.0;
        let ny = 1.0 - y / f64::from(height) * 2.0;
        let ray = match view {
            View::Perspective { focal, near, .. } => {
                let along = add(add(scale(right, nx * aspect / focal), scale(up, ny / focal)), forward);
                let direction = normalize(along).ok_or(RenderError::Camera("the image position is out of range"))?;
                Ray { origin: self.eye, direction, min_distance: near / dot(direction, forward) }
            }
            View::Orthographic { half_height, .. } => {
                let offset = add(scale(right, nx * half_height * aspect), scale(up, ny * half_height));
                Ray { origin: add(self.eye, offset), direction: forward, min_distance: f64::NEG_INFINITY }
            }
        };
        if ray.origin.iter().chain(&ray.direction).all(|c| c.is_finite()) {
            Ok(ray)
        } else {
            Err(RenderError::Camera("the image position is out of range"))
        }
    }
}

/// The nearest triangle of `meshes` that `ray` meets at or beyond [`Ray::min_distance`], or
/// `None`. Both faces count, as the renderer draws both; a triangle the renderer skips (fully
/// transparent, a missing or non-finite corner, no area) is skipped. A tie goes to the lower
/// (mesh, triangle).
#[must_use]
pub fn pick(meshes: &[WorldMesh], ray: &Ray) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for (m, world) in meshes.iter().enumerate() {
        for (t, tri) in world.mesh.triangles.iter().enumerate() {
            if invisible(world, t) {
                continue;
            }
            let Some(hit) = corners(&world.mesh, tri).and_then(|c| intersect(&c, ray)) else { continue };
            if best.is_none_or(|b| hit.distance < b.distance) {
                best = Some(Hit { mesh: m, triangle: t, ..hit });
            }
        }
    }
    best
}

/// Möller–Trumbore ray–triangle intersection, two-sided; the hit's mesh and triangle are 0.
pub(crate) fn intersect(&[a, b, c]: &[[f64; 3]; 3], ray: &Ray) -> Option<Hit> {
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    let normal = unit_cross(e1, e2)?;
    let p = cross(ray.direction, e2);
    let det = dot(e1, p);
    // A ray within rounding of the triangle's plane meets it nowhere useful; the scale keeps the
    // test independent of the model's size.
    if det.abs() < f64::EPSILON * dot(e1, e1).max(dot(e2, e2)) {
        return None;
    }
    let s = sub(ray.origin, a);
    let u = dot(s, p) / det;
    let q = cross(s, e1);
    let v = dot(ray.direction, q) / det;
    let distance = dot(e2, q) / det;
    let inside = (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0;
    let point = add(ray.origin, scale(ray.direction, distance));
    (inside && distance.is_finite() && distance >= ray.min_distance && point.iter().all(|c| c.is_finite())).then_some(Hit {
        mesh: 0,
        triangle: 0,
        point,
        normal,
        distance,
    })
}

#[cfg(test)]
mod tests;
