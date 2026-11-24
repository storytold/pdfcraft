//! Triangle meshes for transition layers.
//!
//! A [`Layer`] is a textured quad. Drawn as two triangles, a trapezoid (a slide seen in
//! perspective) bends its texture along the diagonal, because GPUs and simple rasterisers
//! interpolate texture coordinates linearly in screen space. [`layer_mesh`] splits such quads into a
//! grid whose vertices follow the projective map from the source rectangle to the quad, so the
//! picture looks like a flat card turned in 3-D. Rectangles and parallelograms stay two triangles,
//! wedges one: the per-frame cost stays a handful of vertices per layer.

use crate::transition::Layer;

/// One mesh vertex: position in slide-unit space (0..1, y down), texture coordinate (0..1) and
/// opacity (the layer's alpha times its corner fade, interpolated).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshVertex {
    pub pos: [f64; 2],
    pub uv: [f64; 2],
    pub a: f64,
}

/// Grid cells per side for quads that need a projective subdivision.
const GRID: usize = 8;

/// The triangles of a layer: vertices and index triples into them.
pub fn layer_mesh(l: &Layer) -> (Vec<MeshVertex>, Vec<[u32; 3]>) {
    let q = l.quad;
    let uv = l.uv;
    let fa = l.fade.map(|f| f * l.alpha);
    // A wedge (triangle): the 4th corner repeats the 3rd.
    if q[3] == q[2] && uv[3] == uv[2] {
        let v = (0..3).map(|k| MeshVertex { pos: q[k], uv: uv[k], a: fa[k] }).collect();
        return (v, vec![[0, 1, 2]]);
    }
    let affine = {
        // Parallelogram: opposite edges equal.
        let e1 = [q[1][0] - q[0][0], q[1][1] - q[0][1]];
        let e2 = [q[2][0] - q[3][0], q[2][1] - q[3][1]];
        (e1[0] - e2[0]).abs() < 1e-9 && (e1[1] - e2[1]).abs() < 1e-9
    };
    let h = if affine { None } else { square_to_quad(q) };
    let Some(h) = h else {
        let v = (0..4).map(|k| MeshVertex { pos: q[k], uv: uv[k], a: fa[k] }).collect();
        return (v, vec![[0, 1, 2], [0, 2, 3]]);
    };
    // Small quads (tiles, shards) bend their texture too little to matter: fewer cells.
    let span = q.iter().flat_map(|p| q.iter().map(move |r| (p[0] - r[0]).abs().max((p[1] - r[1]).abs()))).fold(0.0, f64::max);
    let n = ((span * 2.0 * GRID as f64).ceil() as usize).clamp(1, GRID);
    let mut v = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        let b = j as f64 / n as f64;
        for i in 0..=n {
            let a = i as f64 / n as f64;
            // Texture: bilinear in (a, b) over the four uv corners (exact for rectangles).
            let top = lerp2(uv[0], uv[1], a);
            let bot = lerp2(uv[3], uv[2], a);
            let t = lerp2(top, bot, b);
            let at = fa[0] + (fa[1] - fa[0]) * a;
            let ab = fa[3] + (fa[2] - fa[3]) * a;
            v.push(MeshVertex { pos: apply(&h, a, b), uv: t, a: at + (ab - at) * b });
        }
    }
    let mut tris = Vec::with_capacity(n * n * 2);
    let w = (n + 1) as u32;
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let p = j * w + i;
            tris.push([p, p + 1, p + w + 1]);
            tris.push([p, p + w + 1, p + w]);
        }
    }
    (v, tris)
}

fn lerp2(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Projective map taking the unit square's corners (0,0), (1,0), (1,1), (0,1) to `q`'s tl, tr, br,
/// bl (Heckbert's square-to-quad). `None` when the quad is degenerate.
fn square_to_quad(q: [[f64; 2]; 4]) -> Option<[f64; 8]> {
    let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q;
    let sx = x0 - x1 + x2 - x3;
    let sy = y0 - y1 + y2 - y3;
    let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
    let den = dx1 * dy2 - dx2 * dy1;
    if !den.is_finite() || den.abs() < 1e-12 {
        return None;
    }
    let g = (sx * dy2 - dx2 * sy) / den;
    let h = (dx1 * sy - sx * dy1) / den;
    let m = [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h];
    m.iter().all(|v| v.is_finite()).then_some(m)
}

fn apply(m: &[f64; 8], u: f64, v: f64) -> [f64; 2] {
    let w = m[6] * u + m[7] * v + 1.0;
    let w = if w.abs() < 1e-9 { 1e-9_f64.copysign(w) } else { w };
    [(m[0] * u + m[1] * v + m[2]) / w, (m[3] * u + m[4] * v + m[5]) / w]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition::{Layer, Source};

    #[test]
    fn rect_is_two_triangles() {
        let (v, t) = layer_mesh(&Layer::full(Source::New, 1.0));
        assert_eq!((v.len(), t.len()), (4, 2));
    }

    #[test]
    fn trapezoid_is_subdivided_projectively() {
        // A card turned about its left edge: the right edge is shorter.
        let q = [[0.0, 0.0], [0.6, 0.2], [0.6, 0.8], [0.0, 1.0]];
        let l = Layer::quad(Source::Old, [0.0, 0.0, 1.0, 1.0], q, 1.0);
        let (v, t) = layer_mesh(&l);
        assert_eq!(v.len(), (GRID + 1) * (GRID + 1));
        assert_eq!(t.len(), GRID * GRID * 2);
        // A small tile in perspective needs no more than a couple of triangles.
        let small = Layer::quad(Source::Old, [0.0, 0.0, 1.0, 1.0], [[0.5, 0.5], [0.52, 0.505], [0.52, 0.515], [0.5, 0.52]], 1.0);
        assert!(layer_mesh(&small).1.len() <= 2);
        // Corners land on the quad's corners.
        assert_eq!(v[0].pos, q[0]);
        let last = v[v.len() - 1].pos;
        assert!((last[0] - 0.6).abs() < 1e-9 && (last[1] - 0.8).abs() < 1e-9);
        // The texture's middle column sits nearer the far (short) edge than halfway: foreshortening.
        let mid = v[GRID / 2].pos[0];
        assert!(mid > 0.3, "{mid}");
    }

    #[test]
    fn degenerate_quads_do_not_blow_up() {
        let q = [[0.5, 0.5]; 4];
        let l = Layer::quad(Source::Old, [0.0, 0.0, 1.0, 1.0], q, 1.0);
        let (v, _) = layer_mesh(&l);
        assert!(v.iter().all(|p| p.pos[0].is_finite() && p.pos[1].is_finite()));
    }
}
