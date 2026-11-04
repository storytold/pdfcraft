//! Projection, near-plane clipping and the depth-tested triangle fill behind [`crate::render`].

use crate::render::{Camera, Projection, RenderError};
use crate::vec3::{length, sub};

/// One view-space corner: x, y, z (z along the view direction), then its red, green and blue shade.
pub(crate) type ViewCorner = [f64; 6];

/// One screen-space corner: pixel x, pixel y and depth key, and its shade.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Corner {
    pub(crate) at: [f64; 3],
    pub(crate) shade: [f64; 3],
}

/// The perspective near plane, as a fraction of the eye-to-target distance.
pub(crate) const NEAR_FRACTION: f64 = 1e-4;

pub(crate) enum View {
    /// `focal` = 1 / tan(fov_y / 2); `near` = the clip distance.
    Perspective {
        focal: f64,
        aspect: f64,
        near: f64,
    },
    Orthographic {
        half_height: f64,
        aspect: f64,
    },
}

impl View {
    /// The projection for `camera` at width/height `aspect`; refuses a field of view outside
    /// (0, 180) degrees or a non-positive orthographic height.
    pub(crate) fn new(camera: &Camera, aspect: f64) -> Result<View, RenderError> {
        match camera.projection {
            Projection::Perspective { fov_y } => {
                if !(fov_y > 0.0 && fov_y < 180.0) {
                    return Err(RenderError::Camera("the field of view is not between 0 and 180 degrees"));
                }
                let focal = 1.0 / (fov_y.to_radians() / 2.0).tan();
                // A field of view too small to measure leaves no finite focal length.
                if !focal.is_finite() {
                    return Err(RenderError::Camera("the field of view is too small"));
                }
                let distance = length(sub(camera.target, camera.eye));
                Ok(View::Perspective { focal, aspect, near: (distance * NEAR_FRACTION).max(f64::MIN_POSITIVE) })
            }
            Projection::Orthographic { height } => {
                if !(height.is_finite() && height > 0.0) {
                    return Err(RenderError::Camera("the orthographic height is not positive"));
                }
                Ok(View::Orthographic { half_height: height / 2.0, aspect })
            }
        }
    }

    /// The part of `tri` in front of the near plane (Sutherland–Hodgman against one plane):
    /// 0, 3 or 4 corners, the shade interpolated along each cut edge.
    pub(crate) fn clip(&self, tri: [ViewCorner; 3]) -> Vec<ViewCorner> {
        let View::Perspective { near, .. } = *self else {
            return tri.to_vec();
        };
        let mut out = Vec::with_capacity(4);
        for (k, &p) in tri.iter().enumerate() {
            let q = tri.get((k + 1) % 3).copied().unwrap_or(p);
            let z = |v: ViewCorner| v.get(2).copied().unwrap_or(0.0);
            let (p_in, q_in) = (z(p) >= near, z(q) >= near);
            if p_in {
                out.push(p);
            }
            if p_in != q_in {
                let t = (near - z(p)) / (z(q) - z(p));
                let mut cut = p;
                for (c, d) in cut.iter_mut().zip(q) {
                    *c += t * (d - *c);
                }
                out.push(cut);
            }
        }
        out
    }

    /// The screen corner of view corner `v`: pixel x, pixel y and a depth key that is linear
    /// in screen space and larger for nearer points.
    pub(crate) fn project(&self, [x, y, z, r, g, b]: ViewCorner, width: u32, height: u32) -> Corner {
        let (nx, ny, key) = match *self {
            View::Perspective { focal, aspect, .. } => (x * focal / (z * aspect), y * focal / z, 1.0 / z),
            View::Orthographic { half_height, aspect } => (x / (half_height * aspect), y / half_height, -z),
        };
        Corner { at: [(nx + 1.0) / 2.0 * f64::from(width), (1.0 - ny) / 2.0 * f64::from(height), key], shade: [r, g, b] }
    }
}

/// The image being drawn: `rgba` holds `width * height` straight-RGBA pixels and `depth` their
/// depth keys, row by row from the top.
pub(crate) struct Target<'b> {
    /// Depth keys are 1/z (a perspective view), so the shade is interpolated perspective-correct.
    pub(crate) perspective: bool,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: &'b mut [u8],
    pub(crate) depth: &'b mut [f64],
}

impl Target<'_> {
    /// Fill the screen triangle of `corners` with straight-RGBA `colour`, depth-tested at pixel
    /// centres, the shade interpolated across it. A pixel centre on an edge two triangles share
    /// belongs to one of them only. An opaque pixel replaces the pixel and its depth; a
    /// translucent one is blended over it, leaving the depth.
    pub(crate) fn fill(&mut self, corners: [Corner; 3], colour: [u8; 4]) {
        let [a, b, c] = corners;
        let area = edge(a.at, b.at, c.at);
        let edges = [(b.at, c.at), (c.at, a.at), (a.at, b.at)];
        if !area.is_finite() || area.abs() < 1e-12 {
            return;
        }
        let Some((x0, x1, y0, y1)) = self.bounds(&corners) else {
            return;
        };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = [f64::from(x) + 0.5, f64::from(y) + 0.5, 0.0];
                let w = edges.map(|(from, to)| edge(from, to, p) / area);
                if w.iter().zip(edges).any(|(&k, (from, to))| k < 0.0 || (k == 0.0 && !owns(from, to, area))) {
                    continue;
                }
                let mix = |f: &dyn Fn(&Corner) -> f64| w.iter().zip(&corners).map(|(k, c)| k * f(c)).sum::<f64>();
                let key = mix(&|c| at(c.at, 2));
                let Some(i) = pixel_index(self.width, x, y) else {
                    continue;
                };
                if self.depth.get(i).is_none_or(|&d| key <= d) {
                    continue;
                }
                let shade = if a.shade == b.shade && b.shade == c.shade {
                    a.shade
                } else if self.perspective && key > 0.0 {
                    // Screen weights times 1/z, renormalised, are the surface's own weights.
                    [0, 1, 2].map(|j| mix(&|c| at(c.shade, j) * at(c.at, 2)) / key)
                } else {
                    [0, 1, 2].map(|j| mix(&|c| at(c.shade, j)))
                };
                self.blend(i, key, shaded(colour, shade));
            }
        }
    }

    /// The pixel rows and columns `corners` may cover, clipped to the image; `None` when off it.
    fn bounds(&self, corners: &[Corner; 3]) -> Option<(u32, u32, u32, u32)> {
        let lo = |i: usize| corners.iter().map(|c| at(c.at, i)).fold(f64::INFINITY, f64::min).floor().max(0.0);
        let hi = |i: usize, limit: u32| corners.iter().map(|c| at(c.at, i)).fold(f64::NEG_INFINITY, f64::max).ceil().min(f64::from(limit) - 1.0);
        let (x0, x1) = (lo(0), hi(0, self.width));
        let (y0, y1) = (lo(1), hi(1, self.height));
        // Each bound is finite and within 0..limit here, so the casts are exact.
        (x0 <= x1 && y0 <= y1).then_some((x0 as u32, x1 as u32, y0 as u32, y1 as u32))
    }

    /// Write straight-RGBA `rgba` at pixel `i`: replacing it and its depth when opaque, blended
    /// over it otherwise.
    fn blend(&mut self, i: usize, key: f64, rgba: [u8; 4]) {
        let opaque = rgba[3] == 255;
        if opaque && let Some(d) = self.depth.get_mut(i) {
            *d = key;
        }
        let Some(px) = i.checked_mul(4).and_then(|s| self.rgba.get_mut(s..s.checked_add(4)?)) else {
            return;
        };
        if opaque {
            px.copy_from_slice(&rgba);
        } else if let [pr, pg, pb, pa] = px {
            let [sr, sg, sb, sa] = rgba;
            let src = f64::from(sa) / 255.0;
            let dst = f64::from(*pa) / 255.0 * (1.0 - src);
            let out = src + dst;
            if out <= 0.0 {
                return;
            }
            for (p, s) in [pr, pg, pb].into_iter().zip([sr, sg, sb]) {
                *p = channel((f64::from(s) * src + f64::from(*p) * dst) / out);
            }
            *pa = channel(out * 255.0);
        }
    }
}

/// Pixel `x, y`'s index in an image `width` wide; `None` on overflow.
pub(crate) fn pixel_index(width: u32, x: u32, y: u32) -> Option<usize> {
    usize::try_from(y).ok()?.checked_mul(usize::try_from(width).ok()?)?.checked_add(usize::try_from(x).ok()?)
}

/// `v` rounded and clamped to a colour channel; NaN is 0.
pub(crate) fn channel(v: f64) -> u8 {
    // Clamped to 0..=255 first, so the cast is exact (NaN casts to 0).
    v.round().clamp(0.0, 255.0) as u8
}

/// `colour`'s RGB scaled channel by channel by `shade`, saturating at 255; alpha kept.
fn shaded([r, g, b, a]: [u8; 4], [sr, sg, sb]: [f64; 3]) -> [u8; 4] {
    let [r, g, b] = [(r, sr), (g, sg), (b, sb)].map(|(v, s)| channel(f64::from(v) * s));
    [r, g, b, a]
}

/// Twice the signed area of (a, b, p) in the xy plane.
pub(crate) fn edge(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> f64 {
    // Two triangles sharing an edge meet it in opposite orders; evaluating it in one canonical
    // order makes their values exact negatives, so a pixel centre on the edge cannot round out
    // of both and leave a crack.
    if (at(a, 0), at(a, 1)) > (at(b, 0), at(b, 1)) {
        return -edge(b, a, p);
    }
    (at(b, 0) - at(a, 0)) * (at(p, 1) - at(a, 1)) - (at(b, 1) - at(a, 1)) * (at(p, 0) - at(a, 0))
}

/// Whether a pixel centre exactly on the edge `from`-`to` of a triangle of signed doubled area
/// `area` is the triangle's: the side the triangle grows towards, +x first, then +y. Two
/// triangles sharing the edge lie on opposite sides of it, so exactly one owns it.
fn owns(from: [f64; 3], to: [f64; 3], area: f64) -> bool {
    let s = area.signum();
    let (gx, gy) = (-(at(to, 1) - at(from, 1)) * s, (at(to, 0) - at(from, 0)) * s);
    gx > 0.0 || (gx == 0.0 && gy > 0.0)
}

/// Component `i` of `v`, 0.0 out of range.
pub(crate) fn at(v: [f64; 3], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_seen_from_either_triangle_is_exactly_negated() {
        // So no pixel centre on a shared edge is rejected by both.
        let vals = [0.1, 0.7, 1.3, 2.9, 3.3, 5.5];
        for (a, b, p) in [([0.1, 0.1], [0.7, 0.7], [0.1, 2.9]), ([5.5, 0.7], [1.3, 3.3], [2.9, 0.1])]
            .into_iter()
            .chain(vals.iter().map(|&v| ([v, 0.1], [0.7, v], [v, v])))
        {
            let [a, b, p] = [a, b, p].map(|[x, y]| [x, y, 0.0]);
            assert_eq!(edge(a, b, p), -edge(b, a, p), "{a:?} {b:?} {p:?}");
        }
    }

    #[test]
    fn pixel_indices_and_channels_never_overflow() {
        assert_eq!(pixel_index(10, 3, 2), Some(23));
        assert_eq!(channel(f64::NAN), 0);
        assert_eq!(channel(1e300), 255);
        assert_eq!(channel(-4.0), 0);
        assert_eq!(channel(127.5), 128);
    }

    fn corner(x: f64, y: f64, key: f64, shade: f64) -> Corner {
        Corner { at: [x, y, key], shade: [shade; 3] }
    }

    #[test]
    fn a_pixel_on_an_edge_two_triangles_share_is_blended_once() {
        let (mut rgba, mut depth) = (vec![0; 64], vec![f64::NEG_INFINITY; 16]);
        let mut t = Target { perspective: false, width: 4, height: 4, rgba: &mut rgba, depth: &mut depth };
        let [a, b, c, d] = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)].map(|(x, y)| corner(x, y, 1.0, 1.0));
        t.fill([a, b, c], [255, 0, 0, 128]);
        t.fill([a, c, d], [255, 0, 0, 128]);
        // The diagonal runs through four pixel centres; each must look like the rest.
        for px in rgba.chunks(4) {
            assert_eq!(px, [255, 0, 0, 128]);
        }
    }

    #[test]
    fn a_perspective_shade_is_interpolated_on_the_surface_not_the_screen() {
        let (mut rgba, mut depth) = (vec![0; 64], vec![f64::NEG_INFINITY; 16]);
        let mut t = Target { perspective: true, width: 4, height: 4, rgba: &mut rgba, depth: &mut depth };
        // The x = 4 corner is three times as far away as the others, and the only lit one.
        t.fill([corner(0.0, 0.0, 1.0, 0.0), corner(4.0, 0.0, 1.0 / 3.0, 1.0), corner(0.0, 4.0, 1.0, 0.0)], [240, 240, 240, 255]);
        // At screen (1.5, 0.5) the weights are 0.5, 0.375, 0.125: on the surface the far corner
        // counts 0.125 / 0.75 = 1/6, where on the screen it would count 0.375.
        assert_eq!(rgba.get(4..8), Some(&[40, 40, 40, 255][..]));
    }
}
