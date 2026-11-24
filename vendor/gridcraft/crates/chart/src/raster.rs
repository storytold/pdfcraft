//! A small self-written rasteriser: every primitive becomes one or more even-odd filled paths,
//! scan-converted with 4 vertical sub-samples and exact horizontal coverage (anti-aliased).
//! Text is not drawn (tests check geometry, the UI draws real glyphs).

use crate::{Prim, Rgba};

/// Largest image we allocate (pixels).
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const SUB: usize = 4;
/// Scan-conversion work one [`rasterize`] call may do: edge tests (an active edge at one
/// sub-sample) plus covered pixels blended. Edges are only tested on the rows they cross, but a
/// caller can still pass enough geometry to keep the rasteriser busy for minutes on a small
/// image; once the allowance is spent the remaining fills are skipped. 2^28 is about a second,
/// far beyond any chart.
const MAX_SCAN_WORK: u64 = 1 << 28;
/// Most points one filled path may have: more is skipped before any edge is built. Chart
/// renderers cap and decimate their paths far below this; it only bounds direct callers.
const MAX_FILL_POINTS: usize = 1 << 22;

/// Most points one stroked polyline may have: more is skipped before any segment path is built
/// (each segment becomes its own quad). Charts hold at most 100,000 points per series and draw far
/// fewer; it only bounds direct callers.
const MAX_STROKE_POINTS: usize = 1 << 17;

type Path = Vec<Vec<[f32; 2]>>;

struct Canvas {
    w: usize,
    h: usize,
    buf: Vec<u8>,
    acc: Vec<f32>,
    /// Scan-conversion work left (see [`MAX_SCAN_WORK`]).
    work: u64,
}

impl Canvas {
    fn blend(&mut self, x: usize, y: usize, c: Rgba, cov: f32) {
        let a = (cov.clamp(0.0, 1.0) * c[3] as f32 / 255.0).clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = (y * self.w + x) * 4;
        if let Some(px) = self.buf.get_mut(i..i + 4) {
            let da = px[3] as f32 / 255.0;
            let oa = a + da * (1.0 - a);
            if oa <= 0.0 {
                return;
            }
            for k in 0..3 {
                let s = c[k] as f32;
                let d = px[k] as f32;
                px[k] = ((s * a + d * da * (1.0 - a)) / oa).round().clamp(0.0, 255.0) as u8;
            }
            px[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    /// Fills a multi-contour path with the even-odd rule.
    fn fill(&mut self, path: &Path, color: Rgba) {
        // Out of work, or more geometry than the allowance could ever scan: no edge list is built.
        if self.work == 0 || path.iter().map(Vec::len).fold(0usize, usize::saturating_add) > MAX_FILL_POINTS {
            return;
        }
        // Each edge as [top y, bottom y, x0, y0, x1, y1].
        let mut edges: Vec<[f32; 6]> = Vec::new();
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for c in path {
            if c.len() < 3 || !c.iter().all(|p| p[0].is_finite() && p[1].is_finite()) {
                continue;
            }
            for (k, p) in c.iter().enumerate() {
                let q = c.get((k + 1) % c.len()).copied().unwrap_or(*p);
                minx = minx.min(p[0]);
                maxx = maxx.max(p[0]);
                miny = miny.min(p[1]);
                maxy = maxy.max(p[1]);
                if p[1] != q[1] {
                    edges.push([p[1].min(q[1]), p[1].max(q[1]), p[0], p[1], q[0], q[1]]);
                }
            }
        }
        if edges.is_empty() {
            return;
        }
        let x0 = minx.floor().max(0.0) as usize;
        let x1 = (maxx.ceil().max(0.0) as usize).min(self.w);
        let y0 = miny.floor().max(0.0) as usize;
        let y1 = (maxy.ceil().max(0.0) as usize).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bw = x1 - x0;
        if self.acc.len() < bw + 1 {
            self.acc.resize(bw + 1, 0.0);
        }
        // Active-edge list: edges join when the scan reaches their top and leave past their
        // bottom, so each row tests only the edges crossing it instead of every edge.
        edges.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut pending = edges.iter().peekable();
        let mut active: Vec<[f32; 6]> = Vec::new();
        let mut xs: Vec<f32> = Vec::new();
        for py in y0..y1 {
            while let Some(e) = pending.next_if(|e| e[0] < py as f32 + 1.0) {
                active.push(*e);
            }
            active.retain(|e| e[1] > py as f32);
            if active.is_empty() {
                continue;
            }
            // The row's edge tests and its `bw` accumulator clears and blends, charged before any of
            // them, so rows with no coverage still count.
            let cost = (active.len() * SUB) as u64 + bw as u64;
            if cost > self.work {
                self.work = 0;
                return;
            }
            self.work -= cost;
            for v in self.acc.iter_mut().take(bw) {
                *v = 0.0;
            }
            let mut any = false;
            for s in 0..SUB {
                let sy = py as f32 + (s as f32 + 0.5) / SUB as f32;
                xs.clear();
                for e in &active {
                    if sy >= e[0] && sy < e[1] {
                        xs.push(e[2] + (sy - e[3]) * (e[4] - e[2]) / (e[5] - e[3]));
                    }
                }
                xs.sort_by(f32::total_cmp);
                for pair in xs.as_chunks::<2>().0 {
                    let (a, b) = (pair[0].max(x0 as f32), pair[1].min(x1 as f32));
                    if b <= a {
                        continue;
                    }
                    any = true;
                    let ia = a.floor() as usize;
                    let ib = (b.ceil() as usize).min(x1);
                    for px in ia..ib {
                        let l = a.max(px as f32);
                        let r = b.min(px as f32 + 1.0);
                        if r > l
                            && let Some(v) = self.acc.get_mut(px - x0)
                        {
                            *v += (r - l) / SUB as f32;
                        }
                    }
                }
            }
            if !any {
                continue;
            }
            for px in x0..x1 {
                let cov = self.acc.get(px - x0).copied().unwrap_or(0.0);
                if cov > 0.0 {
                    self.blend(px, py, color, cov);
                }
            }
        }
    }
}

fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<[f32; 2]> {
    let sweep = a1 - a0;
    let n = ((r.abs() * sweep.abs() / 2.0).ceil() as usize).clamp(4, 512);
    (0..=n)
        .map(|k| {
            let a = a0 + sweep * k as f32 / n as f32;
            [cx + r * a.sin(), cy - r * a.cos()]
        })
        .collect()
}

fn circle(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    let mut c = arc(cx, cy, r, 0.0, std::f32::consts::TAU);
    c.pop();
    c
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<[f32; 2]> {
    // A negative or NaN extent gives an upper bound of 0 (clamp panics if max < min or NaN).
    let r = r.clamp(0.0, (w.min(h) / 2.0).max(0.0));
    if r < 0.5 {
        return vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    }
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut v = Vec::new();
    v.extend(arc(x + w - r, y + r, r, 0.0, FRAC_PI_2));
    v.extend(arc(x + w - r, y + h - r, r, FRAC_PI_2, PI));
    v.extend(arc(x + r, y + h - r, r, PI, PI + FRAC_PI_2));
    v.extend(arc(x + r, y + r, r, PI + FRAC_PI_2, 2.0 * PI));
    v
}

/// Thick polyline → one quad per segment (each a separate path so overlaps don't cancel).
fn stroke_paths(pts: &[[f32; 2]], width: f32, dash: bool, closed: bool) -> Vec<Path> {
    if pts.len() > MAX_STROKE_POINTS {
        return Vec::new();
    }
    let width = width.clamp(1.0, 50.0);
    let mut segs: Vec<([f32; 2], [f32; 2])> = pts.windows(2).map(|w| (w[0], w[1])).collect();
    if closed && let (Some(a), Some(b)) = (pts.last(), pts.first()) {
        segs.push((*a, *b));
    }
    if dash {
        // The per-segment dash cap bounds each input line, but a caller-supplied vector of many
        // long dashed lines multiplies: together they would eagerly allocate megabytes of
        // geometry even when nothing of it reaches the canvas, so the total is capped too and
        // the line is drawn solid once the budget is gone.
        let (on, off) = (width * 4.0, width * 3.0);
        let mut out = Vec::new();
        let mut budget = 65_536usize;
        for (a, b) in segs {
            if budget == 0 {
                return vec![vec![pts.to_vec()]];
            }
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if !(len.is_finite() && len > 0.0) {
                continue;
            }
            let mut t = 0.0;
            while t < len && budget > 0 {
                let e = (t + on).min(len);
                out.push(([a[0] + dx * t / len, a[1] + dy * t / len], [a[0] + dx * e / len, a[1] + dy * e / len]));
                t += on + off;
                budget -= 1;
            }
        }
        segs = out;
    }
    let hw = width / 2.0;
    segs.into_iter()
        .filter_map(|(a, b)| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if !(len.is_finite() && len > 1e-6) {
                return None;
            }
            let (nx, ny) = (-dy / len * hw, dx / len * hw);
            Some(vec![vec![[a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny], [b[0] - nx, b[1] - ny], [a[0] - nx, a[1] - ny]]])
        })
        .collect()
}

/// Rasterises primitives to straight RGBA8 (`w*h*4` bytes) over `background`. Empty for a zero
/// or oversized image. Text primitives are skipped.
pub fn rasterize(prims: &[Prim], w: u32, h: u32, background: Rgba) -> Vec<u8> {
    let px = (w as u64).saturating_mul(h as u64);
    if w == 0 || h == 0 || px > MAX_PIXELS {
        return vec![];
    }
    let (w, h) = (w as usize, h as usize);
    let mut buf = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        buf.extend_from_slice(&background);
    }
    let mut cv = Canvas { w, h, buf, acc: Vec::new(), work: MAX_SCAN_WORK };
    for p in prims {
        match p {
            Prim::Rect { x, y, w, h, fill, stroke, radius } => {
                if let Some(f) = fill {
                    cv.fill(&vec![rounded_rect(*x, *y, *w, *h, *radius)], *f);
                }
                if let Some((c, sw)) = stroke {
                    let sw = sw.clamp(0.5, 50.0);
                    let outer = rounded_rect(x - sw / 2.0, y - sw / 2.0, w + sw, h + sw, radius + sw / 2.0);
                    let inner = rounded_rect(x + sw / 2.0, y + sw / 2.0, (w - sw).max(0.0), (h - sw).max(0.0), (radius - sw / 2.0).max(0.0));
                    cv.fill(&vec![outer, inner], *c);
                }
            }
            Prim::Line { pts, color, width, dash } => {
                for path in stroke_paths(pts, *width, *dash, false) {
                    cv.fill(&path, *color);
                }
            }
            Prim::Polygon { pts, fill, stroke } => {
                cv.fill(&vec![pts.clone()], *fill);
                if let Some((c, sw)) = stroke {
                    for path in stroke_paths(pts, *sw, false, true) {
                        cv.fill(&path, *c);
                    }
                }
            }
            Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, fill, stroke } => {
                let mut c = arc(*cx, *cy, *r_outer, *a0, *a1);
                if *r_inner > 0.0 {
                    let mut inner = arc(*cx, *cy, *r_inner, *a0, *a1);
                    inner.reverse();
                    c.extend(inner);
                } else {
                    c.push([*cx, *cy]);
                }
                cv.fill(&vec![c.clone()], *fill);
                if let Some((sc, sw)) = stroke {
                    for path in stroke_paths(&c, *sw, false, true) {
                        cv.fill(&path, *sc);
                    }
                }
            }
            Prim::Circle { cx, cy, r, fill, stroke } => {
                if let Some(f) = fill {
                    cv.fill(&vec![circle(*cx, *cy, *r)], *f);
                }
                if let Some((c, sw)) = stroke {
                    let sw = sw.clamp(0.5, 50.0);
                    cv.fill(&vec![circle(*cx, *cy, r + sw / 2.0), circle(*cx, *cy, (r - sw / 2.0).max(0.0))], *c);
                }
            }
            Prim::Text { .. } => {}
        }
    }
    cv.buf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security: each row tests only the edges crossing it. A polygon of 100,000 tiny edges over
    /// 4,000 rows tested every edge at every sub-sample of every row (1.6 billion tests).
    #[test]
    fn many_short_edges_scan_only_their_rows() {
        let mut pts: Vec<[f32; 2]> = (0..100_000).map(|i| [if i % 2 == 0 { 0.0 } else { 0.5 }, i as f32 * 0.04]).collect();
        pts.push([2.0, 4_000.0]);
        pts.push([2.0, 0.0]);
        let img = rasterize(&[Prim::Polygon { pts, fill: [255, 0, 0, 255], stroke: None }], 2, 4_000, [0, 0, 0, 0]);
        // The right-hand pixel of every row is inside the polygon.
        assert_eq!(img.get(4..8), Some(&[255, 0, 0, 255][..]));
        assert_eq!(img.get(4 * (2 * 3_999 + 1)..4 * (2 * 3_999 + 2)), Some(&[255, 0, 0, 255][..]));
    }

    /// Security: scan conversion stops once the call's work allowance is spent.
    #[test]
    fn fills_stop_when_the_work_allowance_is_spent() {
        let square = vec![vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]];
        let mut cv = Canvas { w: 4, h: 4, buf: vec![0; 64], acc: Vec::new(), work: 2 * SUB as u64 * 4 + 16 };
        cv.fill(&square, [255; 4]);
        assert!(cv.buf.iter().all(|&b| b == 255));
        cv.buf = vec![0; 64];
        cv.fill(&square, [255; 4]);
        assert!(cv.buf.iter().all(|&b| b == 0));
    }

    /// Security: a polyline over the point cap builds no segment paths at all, solid or dashed.
    #[test]
    fn oversized_polylines_build_no_segment_paths() {
        let line = |n: usize| -> Vec<[f32; 2]> { (0..n).map(|i| [i as f32, (i % 2) as f32]).collect() };
        assert!(stroke_paths(&line(MAX_STROKE_POINTS + 1), 1.0, false, false).is_empty());
        assert!(stroke_paths(&line(MAX_STROKE_POINTS + 1), 1.0, true, false).is_empty());
        assert_eq!(stroke_paths(&line(MAX_STROKE_POINTS), 1.0, false, false).len(), MAX_STROKE_POINTS - 1);
    }

    #[test]
    fn dashed_geometry_stays_within_the_dash_budget() {
        // Forty very long dashed lines, each allowed 10_000 dashes per segment: before the
        // shared cap they eagerly allocated ~1.6 MB of segment pairs × points and a like
        // number of quads before any of it reached a 4-pixel canvas (finding: dash expansion).
        let prims: Vec<Prim> =
            (0..40).map(|i| Prim::Line { pts: vec![[0.0, i as f32], [900.0, i as f32]], color: [0, 0, 0, 255], width: 1.0, dash: true }).collect();
        let img = rasterize(&prims, 2, 2, [255, 255, 255, 255]);
        assert_eq!(img.len(), 2 * 2 * 4);
        // And the geometry itself is bounded even for one huge dashed polyline.
        assert!(stroke_paths(&[[0.0, 0.0], [1e6, 0.0]], 1.0, true, false).len() <= 65_536);
        let prim = [Prim::Line { pts: vec![[0.0, 0.0], [10.0, 0.0]], color: [0, 0, 0, 255], width: 1.0, dash: true }];
        assert_eq!(rasterize(&prim, 1, 1, [0, 0, 0, 255]).len(), 4);
    }
}
