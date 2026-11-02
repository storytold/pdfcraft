//! Find a photographed document's corners (Enhance ▸ Camera image): the edges of the sheet are
//! the four strongest straight lines that enclose a large convex area, each backed by an edge
//! along most of its length. Pure Rust and deterministic; `None` when nothing convinces, so the
//! caller falls back to corners the user places.
//!
//! Steps: shrink the picture to [`WORK`] pixels on its long side, blur it, take Sobel gradients
//! and keep thin, strong edges; vote for lines (a Hough transform that only votes along each
//! edge's own gradient direction, give or take a few degrees); pair two near-horizontal with two near-vertical lines; keep
//! the quadrilaterals whose four sides lie on edges, and choose the largest.

use pdfcraft_geom::perspective::{Point, order_quad};

/// The working size: the long side of the shrunken picture, in pixels.
const WORK: usize = 400;
/// The most lines tried (strongest first).
const MAX_LINES: usize = 24;
/// A side counts when at least this share of it lies on a matching edge.
const MIN_SUPPORT: f64 = 0.55;
/// A document fills at least this share of the picture.
const MIN_AREA: f64 = 0.12;
/// How far (share of the picture's size) a corner may lie outside the picture.
const MARGIN: f64 = 0.02;

/// A grey picture, row-major, read with bounds checks.
struct Grey {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Grey {
    fn at(&self, x: isize, y: isize) -> f32 {
        // Outside the picture repeats its border.
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.v.get(y * self.w + x).copied().unwrap_or(0.0)
    }
}

/// An edge pixel: its gradient's strength and direction (radians, the normal of the edge).
#[derive(Clone, Copy)]
struct Edge {
    mag: f32,
    dir: f32,
}

/// The document's corners in `rgba` (premultiplied RGBA8, `width` × `height`), in pixels:
/// top-left, top-right, bottom-right, bottom-left. `None` when no quadrilateral convinces.
pub fn detect_document(rgba: &[u8], width: u32, height: u32) -> Option<[Point; 4]> {
    let (w, h) = (width as usize, height as usize);
    if w < 16 || h < 16 || w.checked_mul(h)?.checked_mul(4)? != rgba.len() {
        return None;
    }
    let grey = shrink(rgba, w, h);
    let k = w.max(h) as f64 / grey.w.max(grey.h) as f64;
    let edges = edges(&blur(&blur(&grey)));
    let lines = hough(&edges, grey.w, grey.h);
    let quad = best_quad(&lines, &edges, grey.w, grey.h)?;
    let quad = refine(quad, &edges, grey.w, grey.h).unwrap_or(quad);
    // Back to the picture's pixels (pixel centres line up at k × (x + 0.5)).
    Some(quad.map(|[x, y]| [(x + 0.5) * k, (y + 0.5) * k]))
}

/// Box-filter `rgba` (premultiplied, over white) down to at most [`WORK`] pixels on its long
/// side, as luminance 0–255.
fn shrink(rgba: &[u8], w: usize, h: usize) -> Grey {
    let step = w.max(h).div_ceil(WORK).max(1);
    let (sw, sh) = (w.div_ceil(step), h.div_ceil(step));
    let mut v = Vec::with_capacity(sw * sh);
    for sy in 0..sh {
        for sx in 0..sw {
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for y in sy * step..((sy + 1) * step).min(h) {
                for x in sx * step..((sx + 1) * step).min(w) {
                    if let Some(p) = rgba.get((y * w + x) * 4..(y * w + x) * 4 + 4) {
                        let white = 255.0 - p[3] as f32;
                        let (r, g, b) = (p[0] as f32 + white, p[1] as f32 + white, p[2] as f32 + white);
                        sum += 0.299 * r + 0.587 * g + 0.114 * b;
                        n += 1.0;
                    }
                }
            }
            v.push(if n > 0.0 { sum / n } else { 255.0 });
        }
    }
    Grey { w: sw, h: sh, v }
}

/// A 3×3 binomial blur.
fn blur(g: &Grey) -> Grey {
    let mut v = Vec::with_capacity(g.v.len());
    for y in 0..g.h as isize {
        for x in 0..g.w as isize {
            let mut s = 0.0;
            for (dy, wy) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                for (dx, wx) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                    s += wy * wx * g.at(x + dx, y + dy);
                }
            }
            v.push(s / 16.0);
        }
    }
    Grey { w: g.w, h: g.h, v }
}

/// Thin, strong edges: Sobel gradients, kept where they are a local maximum across the edge
/// and above a threshold relative to the picture's strongest edges.
fn edges(g: &Grey) -> Vec<Option<Edge>> {
    let mut grad = Vec::with_capacity(g.v.len());
    for y in 0..g.h as isize {
        for x in 0..g.w as isize {
            let p = |dx, dy| g.at(x + dx, y + dy);
            let gx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1)) - (p(-1, -1) + 2.0 * p(-1, 0) + p(-1, 1));
            let gy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)) - (p(-1, -1) + 2.0 * p(0, -1) + p(1, -1));
            grad.push(Edge { mag: gx.hypot(gy), dir: gy.atan2(gx) });
        }
    }
    // The threshold: a share of the 98th percentile, never below a faint-edge floor.
    let mut mags: Vec<f32> = grad.iter().map(|e| e.mag).filter(|m| m.is_finite()).collect();
    mags.sort_by(f32::total_cmp);
    let p98 = mags.get(mags.len() * 98 / 100).copied().unwrap_or(0.0);
    let threshold = (0.25 * p98).max(24.0);
    let mag = |x: isize, y: isize| -> f32 {
        if x < 0 || y < 0 || x >= g.w as isize || y >= g.h as isize {
            return 0.0;
        }
        grad.get(y as usize * g.w + x as usize).map_or(0.0, |e| e.mag)
    };
    let mut out = Vec::with_capacity(grad.len());
    for (i, e) in grad.iter().enumerate() {
        let (x, y) = ((i % g.w) as isize, (i / g.w) as isize);
        // Edges touching the picture's border are where blurring repeats it: no information.
        let inside = x > 1 && y > 1 && x < g.w as isize - 2 && y < g.h as isize - 2;
        let keep = inside && e.mag >= threshold && {
            let (dx, dy) = (e.dir.cos().round() as isize, e.dir.sin().round() as isize);
            e.mag >= mag(x + dx, y + dy) && e.mag >= mag(x - dx, y - dy)
        };
        out.push(keep.then_some(*e));
    }
    out
}

/// A line `x cos θ + y sin θ = ρ` and its votes.
#[derive(Clone, Copy, Debug)]
struct Line {
    theta: f64,
    rho: f64,
}

/// The strongest lines through the edges: each edge votes for the lines within 6° of its own
/// normal, weighted by its strength; peaks are taken strongest first, clearing their
/// neighbourhood.
fn hough(edges: &[Option<Edge>], w: usize, h: usize) -> Vec<Line> {
    const STEPS: usize = 180;
    let diag = ((w * w + h * h) as f64).sqrt().ceil() as usize;
    let rows = 2 * diag + 1;
    let mut acc = vec![0.0f32; STEPS * rows];
    let trig: Vec<(f64, f64)> = (0..STEPS).map(|t| (t as f64).to_radians()).map(|a| (a.cos(), a.sin())).collect();
    for (i, e) in edges.iter().enumerate() {
        let Some(e) = e else { continue };
        let (x, y) = ((i % w) as f64, (i / w) as f64);
        let normal = (e.dir as f64).to_degrees().rem_euclid(180.0).round() as isize;
        // ±6°: a jagged (aliased) edge's local gradient leans towards the axes.
        for t in normal - 6..=normal + 6 {
            let t = t.rem_euclid(STEPS as isize) as usize;
            let Some(&(c, s)) = trig.get(t) else { continue };
            let r = (x * c + y * s).round() as isize + diag as isize;
            if let Some(cell) = usize::try_from(r).ok().and_then(|r| acc.get_mut(t * rows + r)) {
                *cell += e.mag;
            }
        }
    }
    let mut lines = Vec::new();
    let first = acc.iter().copied().fold(0.0f32, f32::max);
    while lines.len() < MAX_LINES {
        let Some((best, &v)) = acc.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else { break };
        if v <= 0.0 || v < 0.08 * first {
            break;
        }
        let (t, r) = (best / rows, best % rows);
        lines.push(Line { theta: (t as f64).to_radians(), rho: r as f64 - diag as f64 });
        // Clear ±6° and ±6 px around the peak (θ wraps, flipping ρ's sign).
        for dt in -6isize..=6 {
            let tt = t as isize + dt;
            let (tt, flip) = if tt < 0 {
                (tt + STEPS as isize, true)
            } else if tt >= STEPS as isize {
                (tt - STEPS as isize, true)
            } else {
                (tt, false)
            };
            let rr = if flip { 2 * diag as isize - r as isize } else { r as isize };
            for dr in -6isize..=6 {
                if let Some(cell) = usize::try_from(rr + dr).ok().filter(|&q| q < rows).and_then(|q| acc.get_mut(tt as usize * rows + q)) {
                    *cell = 0.0;
                }
            }
        }
    }
    lines
}

fn intersect(a: Line, b: Line) -> Option<Point> {
    let (c1, s1, c2, s2) = (a.theta.cos(), a.theta.sin(), b.theta.cos(), b.theta.sin());
    let det = c1 * s2 - s1 * c2;
    if det.abs() < 1e-6 {
        return None;
    }
    let p = [(a.rho * s2 - b.rho * s1) / det, (c1 * b.rho - c2 * a.rho) / det];
    p.iter().all(|v| v.is_finite()).then_some(p)
}

/// The share of the side from `a` to `b` that lies on an edge facing across it.
fn support(a: Point, b: Point, edges: &[Option<Edge>], w: usize, h: usize) -> f64 {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    if len < 1.0 {
        return 0.0;
    }
    let (ux, uy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
    // The side's normal, (-uy, ux), as an angle.
    let normal = ux.atan2(-uy) as f32;
    let n = (len / 2.0).clamp(8.0, 200.0) as usize;
    let mut hits = 0;
    for i in 0..n {
        let t = 0.05 + 0.9 * (i as f64 + 0.5) / n as f64;
        let (px, py) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
        let hit = (-2i32..=2).any(|o| {
            let (x, y) = ((px - uy * o as f64).round(), (py + ux * o as f64).round());
            if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
                return false;
            }
            edges.get(y as usize * w + x as usize).copied().flatten().is_some_and(|e| {
                // Either way across: dark-to-light or light-to-dark.
                let d = (e.dir - normal).rem_euclid(std::f32::consts::PI);
                // A NaN direction is no hit, as the two comparisons it replaces said.
                !d.is_nan() && !(0.35..=std::f32::consts::PI - 0.35).contains(&d)
            })
        });
        hits += hit as usize;
    }
    hits as f64 / n as f64
}

/// Sharpen the quadrilateral found on the coarse line grid: fit each side's line through the
/// edge points along it (total least squares), then intersect neighbouring sides again.
/// `None` when a side has too few points or the result is no longer convex.
fn refine(q: [Point; 4], edges: &[Option<Edge>], w: usize, h: usize) -> Option<[Point; 4]> {
    // Each side as (a point on it, its unit direction).
    let mut sides = Vec::with_capacity(4);
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 4.0 {
            return None;
        }
        let (ux, uy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
        let normal = ux.atan2(-uy) as f32;
        let mut pts = Vec::new();
        // Pixels near the side, between 5% and 95% of its length (away from the corners).
        let (x0, x1) = (a[0].min(b[0]).floor() as isize - 3, a[0].max(b[0]).ceil() as isize + 3);
        let (y0, y1) = (a[1].min(b[1]).floor() as isize - 3, a[1].max(b[1]).ceil() as isize + 3);
        for y in y0.max(0)..=y1.min(h as isize - 1) {
            for x in x0.max(0)..=x1.min(w as isize - 1) {
                let Some(e) = edges.get(y as usize * w + x as usize).copied().flatten() else { continue };
                let d = (e.dir - normal).rem_euclid(std::f32::consts::PI);
                if d > 0.35 && d < std::f32::consts::PI - 0.35 {
                    continue;
                }
                let (dx, dy) = (x as f64 - a[0], y as f64 - a[1]);
                let t = (dx * ux + dy * uy) / len;
                let off = -dx * uy + dy * ux;
                if (0.05..=0.95).contains(&t) && off.abs() <= 3.0 {
                    pts.push([x as f64, y as f64]);
                }
            }
        }
        if pts.len() < 10 {
            return None;
        }
        let n = pts.len() as f64;
        let (cx, cy) = (pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n);
        let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
        for p in &pts {
            let (dx, dy) = (p[0] - cx, p[1] - cy);
            sxx += dx * dx;
            sxy += dx * dy;
            syy += dy * dy;
        }
        // The principal direction of the points.
        let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        sides.push(([cx, cy], [angle.cos(), angle.sin()]));
    }
    let meet = |(p, u): (Point, Point), (r, v): (Point, Point)| -> Option<Point> {
        let det = u[0] * v[1] - u[1] * v[0];
        if det.abs() < 1e-6 {
            return None;
        }
        let t = ((r[0] - p[0]) * v[1] - (r[1] - p[1]) * v[0]) / det;
        let out = [p[0] + t * u[0], p[1] + t * u[1]];
        out.iter().all(|c| c.is_finite()).then_some(out)
    };
    let side = |k: usize| sides.get(k).copied();
    // Corner k is where side k-1 (ending there) meets side k (starting there).
    let corner = |k: usize| meet(side((k + 3) % 4)?, side(k)?);
    let refined = [corner(0)?, corner(1)?, corner(2)?, corner(3)?];
    // A fit that moved a corner far is not a refinement.
    let moved = refined.iter().zip(q.iter()).any(|(r, o)| (r[0] - o[0]).hypot(r[1] - o[1]) > 6.0);
    if moved {
        return None;
    }
    order_quad(refined)
}

/// A candidate quad's score (its area times the square of its sides' mean support), or `None` when it
/// strays off the image, is too small, or has a side without enough edge support.
fn quad_score(q: &[Point; 4], edges: &[Option<Edge>], w: usize, h: usize) -> Option<f64> {
    let (fw, fh) = (w as f64, h as f64);
    let (mx, my) = (MARGIN * fw, MARGIN * fh);
    if q.iter().any(|p| p[0] < -mx || p[1] < -my || p[0] > fw + mx || p[1] > fh + my) {
        return None;
    }
    let area = (0..4)
        .map(|k| {
            let (p, n) = (q[k], q[(k + 1) % 4]);
            p[0] * n[1] - n[0] * p[1]
        })
        .sum::<f64>()
        .abs()
        / 2.0;
    if area < MIN_AREA * fw * fh {
        return None;
    }
    let sides: Vec<f64> = (0..4).map(|k| support(q[k], q[(k + 1) % 4], edges, w, h)).collect();
    if sides.iter().any(|&s| s < MIN_SUPPORT) {
        return None;
    }
    // Large, and with every side on its edge: a slightly larger quad with a side
    // off the sheet loses to the sheet itself.
    let mean = sides.iter().sum::<f64>() / 4.0;
    Some(area * mean * mean)
}

/// The convex quadrilateral of two near-horizontal and two near-vertical lines whose four sides
/// all lie on edges, scored by its area times the square of its sides' mean support.
fn best_quad(lines: &[Line], edges: &[Option<Edge>], w: usize, h: usize) -> Option<[Point; 4]> {
    // A normal within 45° of vertical makes a near-horizontal line.
    let flat = |l: &&Line| (l.theta.to_degrees() - 90.0).abs() <= 45.0;
    let horizontal: Vec<&Line> = lines.iter().filter(flat).collect();
    let vertical: Vec<&Line> = lines.iter().filter(|l| !flat(l)).collect();
    let mut best: Option<(f64, [Point; 4])> = None;
    for (i, &h1) in horizontal.iter().enumerate() {
        for &h2 in horizontal.iter().skip(i + 1) {
            for (j, &v1) in vertical.iter().enumerate() {
                for &v2 in vertical.iter().skip(j + 1) {
                    let corners = [intersect(*h1, *v1), intersect(*h1, *v2), intersect(*h2, *v2), intersect(*h2, *v1)];
                    let [Some(a), Some(b), Some(c), Some(d)] = corners else { continue };
                    let Some(q) = order_quad([a, b, c, d]) else { continue };
                    let Some(score) = quad_score(&q, edges, w, h) else { continue };
                    if best.is_none_or(|(b, _)| score > b) {
                        best = Some((score, q));
                    }
                }
            }
        }
    }
    best.map(|(_, q)| q)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic pseudo-random generator for noise.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f32) / (u32::MAX >> 1) as f32
        }
    }

    fn inside(q: &[Point; 4], x: f64, y: f64) -> bool {
        (0..4).all(|i| {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]) >= 0.0
        })
    }

    /// A synthetic photo: `sheet` (corners in reading order) in `paper` over `table(x, y)`, with
    /// 2×2 supersampled edges, optional text lines on the sheet, and noise of ±`noise`.
    fn photo(w: u32, h: u32, sheet: [Point; 4], paper: f32, table: impl Fn(f64, f64) -> f32, text: bool, noise: f32) -> Vec<u8> {
        let mut rng = Lcg(7);
        let at = |u: f64, v: f64| -> Point {
            let top = [sheet[0][0] + (sheet[1][0] - sheet[0][0]) * u, sheet[0][1] + (sheet[1][1] - sheet[0][1]) * u];
            let bot = [sheet[3][0] + (sheet[2][0] - sheet[3][0]) * u, sheet[3][1] + (sheet[2][1] - sheet[3][1]) * u];
            [top[0] + (bot[0] - top[0]) * v, top[1] + (bot[1] - top[1]) * v]
        };
        let bars: Vec<[Point; 4]> = if text {
            (0..12)
                .map(|k| {
                    let v = 0.12 + k as f64 * 0.065;
                    let u1 = if k % 4 == 3 { 0.5 } else { 0.88 };
                    [at(0.1, v), at(u1, v), at(u1, v + 0.022), at(0.1, v + 0.022)]
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let (fx, fy) = (x as f64 + ox, y as f64 + oy);
                    s += if bars.iter().any(|b| inside(b, fx, fy)) {
                        40.0
                    } else if inside(&sheet, fx, fy) {
                        paper
                    } else {
                        table(fx, fy)
                    };
                }
                let v = (s / 4.0 + (rng.next() - 0.5) * 2.0 * noise).clamp(0.0, 255.0) as u8;
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        out
    }

    fn assert_near(found: Option<[Point; 4]>, want: [Point; 4], tol: f64) {
        let found = found.expect("the sheet should be found");
        for (f, w) in found.iter().zip(want.iter()) {
            assert!((f[0] - w[0]).hypot(f[1] - w[1]) <= tol, "found {found:?}, want {want:?}");
        }
    }

    const SHEET: [Point; 4] = [[150.0, 170.0], [760.0, 110.0], [820.0, 1050.0], [100.0, 1090.0]];

    #[test]
    fn finds_a_tilted_sheet_on_a_plain_table() {
        let img = photo(900, 1200, SHEET, 245.0, |_, _| 95.0, false, 0.0);
        assert_near(detect_document(&img, 900, 1200), SHEET, 6.0);
    }

    #[test]
    fn text_on_the_sheet_does_not_shrink_the_quad() {
        let img = photo(900, 1200, SHEET, 245.0, |_, _| 95.0, true, 6.0);
        assert_near(detect_document(&img, 900, 1200), SHEET, 6.0);
    }

    #[test]
    fn wood_grain_and_a_shadow_do_not_fool_it() {
        // Horizontal grain stripes, darker towards the bottom right.
        let table = |x: f64, y: f64| 110.0 + 18.0 * ((y / 23.0).sin() as f32) - (0.03 * (x + y)) as f32;
        let img = photo(900, 1200, SHEET, 235.0, table, true, 8.0);
        assert_near(detect_document(&img, 900, 1200), SHEET, 6.0);
    }

    #[test]
    fn a_card_photographed_from_an_angle() {
        // Strong perspective: the far edge much shorter than the near one.
        let card = [[260.0, 300.0], [660.0, 290.0], [800.0, 700.0], [120.0, 720.0]];
        let img = photo(900, 1000, card, 225.0, |_, _| 60.0, false, 5.0);
        assert_near(detect_document(&img, 900, 1000), card, 6.0);
    }

    #[test]
    fn a_sheet_turned_by_thirty_degrees() {
        // A 500 × 650 sheet turned 30° about (450, 500), corners in reading order.
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let turn = |x: f64, y: f64| [450.0 + x * c - y * s, 500.0 + x * s + y * c];
        let sheet = [turn(-250.0, -325.0), turn(250.0, -325.0), turn(250.0, 325.0), turn(-250.0, 325.0)];
        let img = photo(900, 1000, sheet, 240.0, |_, _| 80.0, true, 5.0);
        // Turned 30° clockwise on screen, the top-left corner is the one turned from (-250, -325).
        assert_near(detect_document(&img, 900, 1000), sheet, 6.0);
    }

    #[test]
    fn a_sheet_running_off_the_picture_is_not_invented() {
        // The bottom edge is outside the picture: either nothing, or corners on the visible
        // edges — never a quadrilateral that cuts the sheet in half.
        let sheet = [[150.0, 170.0], [760.0, 110.0], [830.0, 1400.0], [90.0, 1450.0]];
        let img = photo(900, 1200, sheet, 245.0, |_, _| 95.0, true, 4.0);
        if let Some(q) = detect_document(&img, 900, 1200) {
            let top = (q[0][1] + q[1][1]) / 2.0;
            let bottom = (q[2][1] + q[3][1]) / 2.0;
            assert!(top < 200.0 && bottom > 1150.0, "{q:?}");
        }
    }

    #[test]
    fn gives_up_without_a_document() {
        let table = |x: f64, y: f64| 110.0 + 18.0 * ((y / 23.0).sin() as f32) - (0.03 * (x + y)) as f32;
        let img = photo(900, 1200, [[0.0; 2]; 4], 0.0, table, false, 8.0);
        assert_eq!(detect_document(&img, 900, 1200), None);
        // White paper on a white desk: no edge to find.
        let img = photo(900, 1200, SHEET, 246.0, |_, _| 244.0, false, 2.0);
        assert_eq!(detect_document(&img, 900, 1200), None);
    }

    #[test]
    fn bad_input_is_none_not_a_panic() {
        assert_eq!(detect_document(&[], 0, 0), None);
        assert_eq!(detect_document(&[0; 40], 10, 1), None);
        assert_eq!(detect_document(&[0; 64 * 64 * 4], 64, 63), None);
        assert_eq!(detect_document(&vec![255; 64 * 64 * 4], 64, 64), None);
    }
}
