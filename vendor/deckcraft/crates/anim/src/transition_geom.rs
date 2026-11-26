//! Geometry for transition effects: easing, 3-D cards, opacity fields, polygons.

use std::f64::consts::PI;

use crate::easing::{ease, smooth};
use crate::transition::{FULL, Layer, P, Q, R, Source, corners};

// ---------- easing ----------

/// Smoothstep ease in-out (PowerPoint's fades).
pub(crate) fn ss(x: f64) -> f64 {
    smooth(x)
}

/// Quadratic ease in-out: velocity ramps up for the first half and down for the second
/// (PowerPoint's moves: Push, Cover, Uncover, Pan…).
pub(crate) fn q2(x: f64) -> f64 {
    ease(x, 0.5, 0.5, 0.0)
}

/// Power ease in-out of degree `n` (2: quadratic, 3: cubic, 4: quartic).
pub(crate) fn pow_inout(x: f64, n: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 { 0.5 * (2.0 * x).powf(n) } else { 1.0 - 0.5 * (2.0 - 2.0 * x).powf(n) }
}

/// Quadratic ease-in.
pub(crate) fn ein(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x
}

/// Quadratic ease-out.
pub(crate) fn eout(x: f64) -> f64 {
    let x = 1.0 - x.clamp(0.0, 1.0);
    1.0 - x * x
}

/// Progress through the window [a, b] of t, clamped to 0..1.
pub(crate) fn win(t: f64, a: f64, b: f64) -> f64 {
    if b <= a {
        return if t >= b { 1.0 } else { 0.0 };
    }
    ((t - a) / (b - a)).clamp(0.0, 1.0)
}

// ---------- randomness ----------

pub(crate) fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Deterministic pseudo-random value in [0, 1).
pub(crate) fn rnd(i: u64) -> f64 {
    (hash64(i) >> 11) as f64 / (1u64 << 53) as f64
}

// ---------- 2-D ----------

pub(crate) fn offset(r: R, dx: f64, dy: f64) -> R {
    [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]
}

pub(crate) fn center(r: R) -> P {
    [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]
}

pub(crate) fn scale_rect(r: R, s: f64) -> R {
    scale_rect_about(r, s, center(r))
}

pub(crate) fn scale_rect_about(r: R, s: f64, c: P) -> R {
    [c[0] + (r[0] - c[0]) * s, c[1] + (r[1] - c[1]) * s, c[0] + (r[2] - c[0]) * s, c[1] + (r[3] - c[1]) * s]
}

/// The whole source moved by (dx, dy).
pub(crate) fn moved(source: Source, dx: f64, dy: f64, alpha: f64) -> Layer {
    Layer::rect(source, FULL, offset(FULL, dx, dy), alpha)
}

/// The whole source scaled by `s` about (cx, cy) and moved.
pub(crate) fn scaled(source: Source, s: f64, cx: f64, cy: f64, dx: f64, dy: f64, alpha: f64) -> Layer {
    Layer::rect(source, FULL, offset(scale_rect_about(FULL, s, [cx, cy]), dx, dy), alpha)
}

/// Rotate a point about a pivot by `deg` (clockwise on screen), in a slide `a` wide per unit of
/// height, so angles and shapes stay true.
pub(crate) fn rot(p: P, c: P, deg: f64, a: f64) -> P {
    let (s, co) = deg.to_radians().sin_cos();
    let (x, y) = ((p[0] - c[0]) * a, p[1] - c[1]);
    [c[0] + (x * co - y * s) / a, c[1] + x * s + y * co]
}

pub(crate) fn rot_quad(q: Q, c: P, deg: f64, a: f64) -> Q {
    [rot(q[0], c, deg, a), rot(q[1], c, deg, a), rot(q[2], c, deg, a), rot(q[3], c, deg, a)]
}

/// Motion direction of an option: l, r, u, d and diagonals.
pub(crate) fn dir(opt: &str, default: (f64, f64)) -> (f64, f64) {
    let mut d = (0.0, 0.0);
    if opt.contains('l') {
        d.0 = -1.0;
    }
    if opt.contains('r') {
        d.0 = 1.0;
    }
    if opt.contains('u') {
        d.1 = -1.0;
    }
    if opt.contains('d') {
        d.1 = 1.0;
    }
    if d == (0.0, 0.0) { default } else { d }
}

/// A rectangle with a soft border `f` wide (slide units) fading out to nothing: the rectangle at
/// full opacity, four edge ramps and four corner ramps. `src` maps onto `dst`; the border samples
/// the texture's edge.
pub(crate) fn soft_rect(source: Source, src: R, dst: R, f: f64, alpha: f64) -> Vec<Layer> {
    let mut v = vec![Layer::rect(source, src, dst, alpha)];
    if f <= 1e-6 {
        return v;
    }
    let (l, t, r, b) = (dst[0], dst[1], dst[2], dst[3]);
    let (fx, fy) = (f, f * 16.0 / 9.0);
    let uv_at = |x: f64, y: f64| -> P {
        let u = if r > l { src[0] + (x.clamp(l, r) - l) / (r - l) * (src[2] - src[0]) } else { src[0] };
        let w = if b > t { src[1] + (y.clamp(t, b) - t) / (b - t) * (src[3] - src[1]) } else { src[1] };
        [u, w]
    };
    let mut q = |pts: Q, fade: [f64; 4]| {
        let uv = pts.map(|p| uv_at(p[0], p[1]));
        let xs = uv.map(|p| p[0]);
        let ys = uv.map(|p| p[1]);
        let mn = |a: [f64; 4]| a.iter().copied().fold(f64::INFINITY, f64::min);
        let mx = |a: [f64; 4]| a.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        v.push(Layer { source, src: [mn(xs), mn(ys), mx(xs), mx(ys)], quad: pts, uv, alpha, fade });
    };
    q([[l, t - fy], [r, t - fy], [r, t], [l, t]], [0.0, 0.0, 1.0, 1.0]);
    q([[l, b], [r, b], [r, b + fy], [l, b + fy]], [1.0, 1.0, 0.0, 0.0]);
    q([[l - fx, t], [l, t], [l, b], [l - fx, b]], [0.0, 1.0, 1.0, 0.0]);
    q([[r, t], [r + fx, t], [r + fx, b], [r, b]], [1.0, 0.0, 0.0, 1.0]);
    q([[l - fx, t - fy], [l, t - fy], [l, t], [l - fx, t]], [0.0, 0.0, 1.0, 0.0]);
    q([[r, t - fy], [r + fx, t - fy], [r + fx, t], [r, t]], [0.0, 0.0, 0.0, 1.0]);
    q([[r, b], [r + fx, b], [r + fx, b + fy], [r, b + fy]], [1.0, 0.0, 0.0, 0.0]);
    q([[l - fx, b], [l, b], [l, b + fy], [l - fx, b + fy]], [0.0, 1.0, 0.0, 0.0]);
    v
}

// ---------- opacity fields ----------

/// The source shown in place through an opacity field `f(x, y)` (0..1) sampled on an `n` × `m`
/// grid: each cell is a quad with its corners' opacities. Cells with nothing to show are left out
/// and runs of fully opaque cells merge, so most frames stay small.
pub(crate) fn field(source: Source, n: usize, m: usize, f: &dyn Fn(f64, f64) -> f64) -> Vec<Layer> {
    let (n, m) = (n.clamp(1, 256), m.clamp(1, 256));
    let mut vals = vec![0.0; (n + 1) * (m + 1)];
    for j in 0..=m {
        for i in 0..=n {
            let v = f(i as f64 / n as f64, j as f64 / m as f64);
            if let Some(x) = vals.get_mut(j * (n + 1) + i) {
                *x = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
            }
        }
    }
    let at = |i: usize, j: usize| vals.get(j * (n + 1) + i).copied().unwrap_or(0.0);
    let mut out = Vec::new();
    for j in 0..m {
        let (y0, y1) = (j as f64 / m as f64, (j + 1) as f64 / m as f64);
        let mut run: Option<usize> = None;
        for i in 0..=n {
            let full = i < n && at(i, j) >= 1.0 && at(i + 1, j) >= 1.0 && at(i, j + 1) >= 1.0 && at(i + 1, j + 1) >= 1.0;
            if full {
                run.get_or_insert(i);
                continue;
            }
            if let Some(s) = run.take() {
                out.push(Layer::inplace(source, [s as f64 / n as f64, y0, i as f64 / n as f64, y1], 1.0));
            }
            if i == n {
                break;
            }
            let c = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            if c.iter().all(|v| *v <= 0.0) {
                continue;
            }
            out.push(Layer::inplace(source, [i as f64 / n as f64, y0, (i + 1) as f64 / n as f64, y1], 1.0).faded(c));
        }
    }
    out
}

/// Opacity of a soft edge at position `p` when the edge's middle is at `c` and it ramps over `w`.
pub(crate) fn edge(c: f64, p: f64, w: f64) -> f64 {
    if w <= 1e-9 {
        return if p <= c { 1.0 } else { 0.0 };
    }
    ((c - p) / w + 0.5).clamp(0.0, 1.0)
}

/// Point on the unit square's boundary seen from its centre at angle `ang` (radians, clockwise
/// from 12 o'clock, measured on a slide `a` wide per unit of height).
pub(crate) fn boundary(ang: f64, a: f64) -> P {
    let (dx, dy) = (ang.sin() / a, -ang.cos());
    let m = (dx.abs() / 0.5).max(dy.abs() / 0.5).max(1e-9);
    [0.5 + dx / m, 0.5 + dy / m]
}

/// The source shown in place through an angular opacity field `f(angle)` (radians clockwise from
/// 12 o'clock, 0..2π): a fan of thin wedges from the centre.
pub(crate) fn fan(source: Source, a: f64, steps: usize, f: &dyn Fn(f64) -> f64) -> Vec<Layer> {
    let n = steps.clamp(8, 720);
    let mut angles: Vec<f64> = (0..=n).map(|k| 2.0 * PI * k as f64 / n as f64).collect();
    // The square's corners, so wedges reach them.
    for c in [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]] {
        let ang = ((c[0] - 0.5) * a).atan2(-(c[1] - 0.5));
        angles.push(ang.rem_euclid(2.0 * PI));
    }
    angles.sort_by(f64::total_cmp);
    angles.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
    let mut out = Vec::new();
    let c = [0.5, 0.5];
    for w in angles.windows(2) {
        let [x, y] = w else { continue };
        let (fx, fy) = (f(*x).clamp(0.0, 1.0), f(*y).clamp(0.0, 1.0));
        if fx <= 0.0 && fy <= 0.0 {
            continue;
        }
        let (bx, by) = (boundary(*x, a), boundary(*y, a));
        out.push(Layer::tri_uv(source, [c, bx, by], [c, bx, by], [(fx + fy) / 2.0, fx, fy], 1.0));
    }
    out
}

// ---------- polygons ----------

/// Keep the part of a convex polygon where `n · p <= d`.
pub(crate) fn clip(poly: &[P], n: P, d: f64) -> Vec<P> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    let side = |p: P| n[0] * p[0] + n[1] * p[1] - d;
    for (i, p) in poly.iter().enumerate() {
        let q = poly.get((i + 1) % poly.len()).copied().unwrap_or(*p);
        let (sp, sq) = (side(*p), side(q));
        if sp <= 0.0 {
            out.push(*p);
        }
        if (sp < 0.0 && sq > 0.0) || (sp > 0.0 && sq < 0.0) {
            let k = sp / (sp - sq);
            out.push([p[0] + (q[0] - p[0]) * k, p[1] + (q[1] - p[1]) * k]);
        }
    }
    out
}

/// A convex polygon as triangles: vertices drawn at `map(p)` with texture coordinate `p`.
pub(crate) fn poly_layers(source: Source, poly: &[P], alpha: f64, map: &dyn Fn(P) -> P) -> Vec<Layer> {
    let Some(first) = poly.first().copied() else { return vec![] };
    poly.windows(2)
        .skip(1)
        .filter_map(|w| match w {
            [b, c] => Some(Layer::tri_uv(source, [map(first), map(*b), map(*c)], [first, *b, *c], [1.0; 3], alpha)),
            _ => None,
        })
        .collect()
}

pub(crate) fn rect_poly(r: R) -> Vec<P> {
    corners(r).to_vec()
}

// ---------- 3-D ----------

/// A point in the 3-D scene: x right and y down in slide heights (the slide spans x ∈ [−a/2, a/2],
/// y ∈ [−½, ½] at z = 0), z away from the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub(crate) fn v3(x: f64, y: f64, z: f64) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    pub fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    /// Rotate about the vertical axis through `c` (degrees; positive turns the right side away).
    pub fn yaw(self, c: V3, deg: f64) -> V3 {
        let (s, co) = deg.to_radians().sin_cos();
        let p = self.sub(c);
        v3(p.x * co - p.z * s, p.y, p.x * s + p.z * co).add(c)
    }
    /// Rotate about the horizontal axis through `c` (degrees; positive turns the top away).
    pub fn pitch(self, c: V3, deg: f64) -> V3 {
        let (s, co) = deg.to_radians().sin_cos();
        let p = self.sub(c);
        v3(p.x, p.y * co + p.z * s, -p.y * s + p.z * co).add(c)
    }
    /// Rotate in the screen plane about `c` (degrees, clockwise).
    pub fn roll(self, c: V3, deg: f64) -> V3 {
        let (s, co) = deg.to_radians().sin_cos();
        let p = self.sub(c);
        v3(p.x * co - p.y * s, p.x * s + p.y * co, p.z).add(c)
    }
}

/// A perspective camera looking at the slide plane from `d` slide heights away.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cam {
    pub a: f64,
    pub d: f64,
}

impl Cam {
    pub fn p(&self, v: V3) -> P {
        let k = self.d / (self.d + v.z).max(self.d * 0.02);
        [v.x * k / self.a + 0.5, v.y * k + 0.5]
    }
    /// The 3-D corners (tl, tr, br, bl) of the unit-space rectangle `r` lying on the slide plane.
    pub fn rect(&self, r: R) -> [V3; 4] {
        corners(r).map(|p| v3((p[0] - 0.5) * self.a, p[1] - 0.5, 0.0))
    }
    /// The unit-space point of a 3-D point on the slide plane.
    pub fn unit(&self, x: f64, y: f64) -> V3 {
        v3((x - 0.5) * self.a, y - 0.5, 0.0)
    }
    pub fn quad(&self, c: [V3; 4]) -> Q {
        c.map(|v| self.p(v))
    }
}

/// Signed area of a quad (positive: front side facing the viewer).
pub(crate) fn area(q: &Q) -> f64 {
    let mut s = 0.0;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    s / 2.0
}

/// A card: `src` of the source mapped onto 3-D corners, if it faces the viewer; `shade` (0..1)
/// darkens it. Seen from behind it is left out unless `back` is set (then drawn mirrored).
pub(crate) fn card(cam: &Cam, source: Source, src: R, c: [V3; 4], alpha: f64, shade: f64, back: bool) -> Vec<Layer> {
    let q = cam.quad(c);
    if !q.iter().all(|p| p[0].is_finite() && p[1].is_finite()) {
        return vec![];
    }
    let front = area(&q) > 0.0;
    if !front && !back {
        return vec![];
    }
    let l = Layer::quad(source, src, q, alpha);
    let mut v = vec![l];
    if shade > 0.001 {
        v.push(l.shade(shade));
    }
    v
}

/// How much a card facing along `n` (unit normal toward the viewer is (0, 0, −1)) is darkened by
/// the light behind the viewer: 0 facing straight on.
pub(crate) fn facing_shade(c: [V3; 4], k: f64) -> f64 {
    let u = c[1].sub(c[0]);
    let w = c[3].sub(c[0]);
    let n = v3(u.y * w.z - u.z * w.y, u.z * w.x - u.x * w.z, u.x * w.y - u.y * w.x);
    let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
    if len < 1e-12 {
        return 0.0;
    }
    let cos = (n.z / len).abs();
    (k * (1.0 - cos)).clamp(0.0, 1.0)
}
