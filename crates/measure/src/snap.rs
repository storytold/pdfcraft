//! Bounded path extraction and snapping in PDF user space, including nested Form XObjects.
use crate::{Point, Result, check_points, distance, invalid};
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Room for a CAD plan: an A0 floor plan holds up to a million segments.
const MAX_SEGMENTS: usize = 2_000_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;
/// Grid cells per side at most (the grid is square, over the geometry's extent).
const GRID_SIDE: usize = 512;
/// A segment whose box covers more cells than this (a page frame, an axis) is kept in a list
/// looked at on every query instead of being filed under each cell.
const LONG_SPAN: usize = 16;

/// Where path extraction stops: the segments kept (endpoints and midpoints follow, at most
/// three targets per segment) and the decoded content bytes read. The defaults take a CAD
/// plan; tests use smaller ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub segments: usize,
    pub bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { segments: MAX_SEGMENTS, bytes: MAX_BYTES }
    }
}

#[derive(Clone, Debug)]
pub struct Geometry {
    pub segments: Vec<[Point; 2]>,
    pub endpoints: Vec<Point>,
    pub midpoints: Vec<Point>,
    /// The centres of closed figures: rectangles, circles (four curves round), closed polygons
    /// and every filled subpath, each its box's centre.
    pub centres: Vec<Point>,
    /// A limit stopped extraction early (segments, bytes, streams, nesting).
    pub truncated: bool,
    /// Content streams that couldn't be decoded and were skipped.
    pub unreadable: usize,
    /// The segment cap this geometry is extracted under.
    cap: usize,
    /// Cells over the geometry, built on the first query (the vectors above are complete by
    /// then and don't change afterwards).
    grid: OnceLock<Grid>,
}
impl Default for Geometry {
    fn default() -> Self {
        Self::with_limits(Limits::default())
    }
}
impl Geometry {
    fn with_limits(limits: Limits) -> Self {
        Self {
            segments: Vec::new(),
            endpoints: Vec::new(),
            midpoints: Vec::new(),
            centres: Vec::new(),
            truncated: false,
            unreadable: 0,
            cap: limits.segments,
            grid: OnceLock::new(),
        }
    }
}

/// Which targets and segments lie in each cell of a square grid over the geometry's extent,
/// so a query looks at the pointer's neighbourhood instead of the whole page (a plan's
/// million segments took 27 ms a query). Each list is cell offsets into one items vector.
#[derive(Clone, Debug)]
struct Grid {
    origin: Point,
    cell: f64,
    side: usize,
    /// Targets by index: the endpoints, then the midpoints, then the centres.
    point_offsets: Vec<u32>,
    point_items: Vec<u32>,
    segment_offsets: Vec<u32>,
    segment_items: Vec<u32>,
    /// Segments spanning more than `LONG_SPAN` cells, ascending.
    long: Vec<u32>,
}
impl Grid {
    fn build(g: &Geometry) -> Self {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in g.segments.iter().flatten().chain(&g.endpoints).chain(&g.midpoints).chain(&g.centres) {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        if lo[0] > hi[0] || lo[1] > hi[1] {
            (lo, hi) = ([0.0, 0.0], [0.0, 0.0]);
        }
        let points = g.endpoints.len().saturating_add(g.midpoints.len()).saturating_add(g.centres.len());
        let side = ((g.segments.len().max(points) as f64).sqrt().ceil() as usize).clamp(1, GRID_SIDE);
        let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        let cell = if extent > 0.0 && extent.is_finite() { extent / side as f64 } else { 1.0 };
        let mut grid = Self {
            origin: lo,
            cell,
            side,
            point_offsets: Vec::new(),
            point_items: Vec::new(),
            segment_offsets: Vec::new(),
            segment_items: Vec::new(),
            long: Vec::new(),
        };
        let cells = side * side;
        let point = |i: usize| g.target_at(i).map(|(p, _)| p);
        (grid.point_offsets, grid.point_items) = csr(cells, points, |i, put| {
            if let Some(p) = point(i) {
                put(grid.cell_of(p));
            }
        });
        let mut long = Vec::new();
        let span = |s: &[Point; 2]| {
            let lo = [s[0][0].min(s[1][0]), s[0][1].min(s[1][1])];
            let hi = [s[0][0].max(s[1][0]), s[0][1].max(s[1][1])];
            grid.cells_over(lo, hi).filter(|(x0, y0, x1, y1)| (x1 - x0 + 1) * (y1 - y0 + 1) <= LONG_SPAN)
        };
        for (i, s) in g.segments.iter().enumerate() {
            if span(s).is_none() {
                long.push(i as u32);
            }
        }
        (grid.segment_offsets, grid.segment_items) = csr(cells, g.segments.len(), |i, put| {
            if let Some((x0, y0, x1, y1)) = g.segments.get(i).and_then(span) {
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        put(y * side + x);
                    }
                }
            }
        });
        grid.long = long;
        grid
    }
    fn axis(&self, value: f64, origin: f64) -> f64 {
        ((value - origin) / self.cell).floor()
    }
    fn cell_of(&self, p: Point) -> usize {
        let last = (self.side - 1) as f64;
        let x = self.axis(p[0], self.origin[0]).clamp(0.0, last) as usize;
        let y = self.axis(p[1], self.origin[1]).clamp(0.0, last) as usize;
        y * self.side + x
    }
    /// The cells the box `lo`..`hi` overlaps, as x0, y0, x1, y1; None when it misses the grid.
    fn cells_over(&self, lo: Point, hi: Point) -> Option<(usize, usize, usize, usize)> {
        let last = (self.side - 1) as f64;
        let (x0, y0) = (self.axis(lo[0], self.origin[0]), self.axis(lo[1], self.origin[1]));
        let (x1, y1) = (self.axis(hi[0], self.origin[0]), self.axis(hi[1], self.origin[1]));
        if [x0, y0, x1, y1].iter().any(|v| v.is_nan()) || x1 < 0.0 || y1 < 0.0 || x0 > last || y0 > last {
            return None;
        }
        Some((x0.clamp(0.0, last) as usize, y0.clamp(0.0, last) as usize, x1.clamp(0.0, last) as usize, y1.clamp(0.0, last) as usize))
    }
    fn items_in<'a>(&self, offsets: &'a [u32], items: &'a [u32], cell: usize) -> &'a [u32] {
        match (offsets.get(cell), offsets.get(cell + 1)) {
            (Some(&from), Some(&to)) => items.get(from as usize..to as usize).unwrap_or(&[]),
            _ => &[],
        }
    }
    /// The targets in the cells within `tolerance` of `at`, by index, ascending.
    fn points_near(&self, at: Point, tolerance: f64) -> Vec<u32> {
        let mut near = Vec::new();
        if let Some((x0, y0, x1, y1)) = self.cells_over([at[0] - tolerance, at[1] - tolerance], [at[0] + tolerance, at[1] + tolerance]) {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    near.extend_from_slice(self.items_in(&self.point_offsets, &self.point_items, y * self.side + x));
                }
            }
        }
        near.sort_unstable();
        near
    }
    /// The segments that may lie within `tolerance` of `at`, by index, ascending: those filed
    /// under the cells the tolerance box overlaps, and the long ones.
    fn segments_near(&self, at: Point, tolerance: f64) -> Vec<u32> {
        let mut filed = Vec::new();
        if let Some((x0, y0, x1, y1)) = self.cells_over([at[0] - tolerance, at[1] - tolerance], [at[0] + tolerance, at[1] + tolerance]) {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    filed.extend_from_slice(self.items_in(&self.segment_offsets, &self.segment_items, y * self.side + x));
                }
            }
        }
        filed.sort_unstable();
        filed.dedup();
        // Merged with the long segments (ascending too), without duplicates.
        let mut near = Vec::with_capacity(filed.len() + self.long.len());
        let (mut a, mut b) = (filed.iter().peekable(), self.long.iter().peekable());
        loop {
            match (a.peek(), b.peek()) {
                (Some(&&x), Some(&&y)) if x == y => {
                    near.push(x);
                    a.next();
                    b.next();
                }
                (Some(&&x), Some(&&y)) if x < y => {
                    near.push(x);
                    a.next();
                }
                (Some(_), Some(&&y)) => {
                    near.push(y);
                    b.next();
                }
                (Some(&&x), None) => {
                    near.push(x);
                    a.next();
                }
                (None, Some(&&y)) => {
                    near.push(y);
                    b.next();
                }
                (None, None) => break,
            }
        }
        near
    }
}
/// Items filed under cells, as offsets per cell into one list; `cells_of` names an item's
/// cells and runs twice, to count and to place.
fn csr(cells: usize, count: usize, cells_of: impl Fn(usize, &mut dyn FnMut(usize))) -> (Vec<u32>, Vec<u32>) {
    let mut offsets = vec![0u32; cells + 1];
    for i in 0..count {
        cells_of(i, &mut |c| {
            if let Some(n) = offsets.get_mut(c + 1) {
                *n = n.saturating_add(1);
            }
        });
    }
    for c in 1..=cells {
        let before = offsets.get(c - 1).copied().unwrap_or(0);
        if let Some(n) = offsets.get_mut(c) {
            *n = n.saturating_add(before);
        }
    }
    let mut cursor = offsets.clone();
    let mut items = vec![0u32; offsets.last().copied().unwrap_or(0) as usize];
    for i in 0..count {
        cells_of(i, &mut |c| {
            if let Some(at) = cursor.get_mut(c)
                && let Some(slot) = items.get_mut(*at as usize)
            {
                *slot = i as u32;
                *at += 1;
            }
        });
    }
    (offsets, items)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Intersection,
    Path,
    /// The centre of a closed figure (a rectangle, a circle, a closed or filled polygon) or of the
    /// small box drawn around the point (a check box, a table cell, [`Geometry::enclosing_box`]):
    /// where a Fill & Sign mark goes.
    Centre,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Snap {
    pub point: Point,
    pub kind: SnapKind,
    pub distance: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapOptions {
    pub endpoints: bool,
    pub midpoints: bool,
    pub intersections: bool,
    pub paths: bool,
    /// The centres of closed figures and of the table cell or box around the point.
    pub centres: bool,
}
impl Default for SnapOptions {
    fn default() -> Self {
        Self { endpoints: true, midpoints: true, intersections: true, paths: true, centres: true }
    }
}
/// How far a table cell's sides may be from the point for its centre to be a snap target.
const CELL_REACH: f64 = 60.0;
/// At most this many candidates are offered at one point.
const MAX_CANDIDATES: usize = 32;
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
pub fn intersection(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let r = sub(b, a);
    let s = sub(d, c);
    let denominator = cross(r, s);
    if !denominator.is_finite() || denominator.abs() < 1e-12 {
        return None;
    }
    let t = cross(sub(c, a), s) / denominator;
    let u = cross(sub(c, a), r) / denominator;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) { Some([a[0] + t * r[0], a[1] + t * r[1]]) } else { None }
}
fn nearest(p: Point, a: Point, b: Point) -> Point {
    let v = sub(b, a);
    let length = dot(v, v);
    if length < 1e-18 {
        return a;
    }
    let t = (dot(sub(p, a), v) / length).clamp(0.0, 1.0);
    [a[0] + v[0] * t, a[1] + v[1] * t]
}
/// The order [`Geometry::candidates`] offers its targets in: nearest first and, at the same
/// distance to a billionth of a unit, the discrete kinds before the others (endpoint, centre,
/// midpoint, intersection, path). A total order, as `sort_by` requires: "nearly the same"
/// compared pairwise is not transitive, and since Rust 1.81 a sort may panic on such an order.
pub(crate) fn candidate_order(a: &Snap, b: &Snap) -> std::cmp::Ordering {
    let rank = |k: SnapKind| match k {
        SnapKind::Endpoint => 0,
        SnapKind::Centre => 1,
        SnapKind::Midpoint => 2,
        SnapKind::Intersection => 3,
        SnapKind::Path => 4,
    };
    // Candidates are within the tolerance (at most 10,000 units), so the steps fit an i64.
    let key = |s: &Snap| ((s.distance * 1e9).round() as i64, rank(s.kind));
    key(a).cmp(&key(b)).then(a.distance.total_cmp(&b.distance))
}

/// A subpath's box and corners so far, for its centre once it is known to be closed.
#[derive(Clone, Copy, Debug)]
struct Subpath {
    lo: Point,
    hi: Point,
    start: Point,
    end: Point,
    /// Points that shape it (corners, curve ends).
    points: usize,
    closed: bool,
}
impl Subpath {
    fn new(p: Point) -> Self {
        Self { lo: p, hi: p, start: p, end: p, points: 1, closed: false }
    }
    fn add(&mut self, p: Point) {
        self.lo = [self.lo[0].min(p[0]), self.lo[1].min(p[1])];
        self.hi = [self.hi[0].max(p[0]), self.hi[1].max(p[1])];
        self.end = p;
        self.points = self.points.saturating_add(1);
    }
    /// The centre of its box when it is a closed figure (closed by `h`, a rectangle, ending where
    /// it began, or `filled`) of three points or more with some extent.
    fn centre(&self, filled: bool) -> Option<Point> {
        let closed = filled || self.closed || distance(self.start, self.end) <= 1e-6;
        let extent = (self.hi[0] - self.lo[0]).max(self.hi[1] - self.lo[1]);
        let c = [(self.lo[0] + self.hi[0]) / 2.0, (self.lo[1] + self.hi[1]) / 2.0];
        // As every other target: finite and within the coordinate range (a huge `cm` can make a
        // corner infinite, and one infinite centre would spread the grid over the whole page).
        (closed && self.points >= 3 && extent > 1e-3 && check_points(&[c]).is_ok()).then_some(c)
    }
}

/// Save a subpath's implicit closing edge for fill operations. An unpainted or
/// stroke-only open path must not gain that edge.
fn remember_close(closures: &mut Vec<[Point; 2]>, current: Option<Point>, start: Option<Point>) -> bool {
    if let (Some(a), Some(b)) = (current, start)
        && distance(a, b) > 1e-9
    {
        if closures.len() >= MAX_SEGMENTS {
            return false;
        }
        closures.push([a, b]);
    }
    true
}

impl Geometry {
    fn grid(&self) -> &Grid {
        self.grid.get_or_init(|| Grid::build(self))
    }
    /// Dense intersection searches are capped independently from path extraction.
    pub fn intersection_limited(&self, at: Point, tolerance: f64) -> bool {
        let near = self.grid().segments_near(at, tolerance);
        near.iter().filter_map(|i| self.segments.get(*i as usize)).filter(|[a, b]| distance(at, nearest(at, *a, *b)) <= tolerance).take(257).count()
            > 256
    }
    /// The snap for `at`: the first of [`Self::candidates`].
    pub fn snap(&self, at: Point, tolerance: f64, options: SnapOptions) -> Result<Option<Snap>> {
        Ok(self.candidates(at, tolerance, options)?.into_iter().next())
    }
    /// Every target within `tolerance` of `at` that `options` allows, nearest first (at the same
    /// distance: endpoint, centre, midpoint, intersection), each point once, at most 32; then the
    /// nearest point on a path, which comes first only when nothing else is in reach. What a
    /// snap marker offers, and what Tab cycles through.
    pub fn candidates(&self, at: Point, tolerance: f64, options: SnapOptions) -> Result<Vec<Snap>> {
        check_points(&[at])?;
        if !tolerance.is_finite() || !(0.0..=10_000.0).contains(&tolerance) {
            return Err(invalid("snap tolerance must be between 0 and 10000 user units"));
        }
        let grid = self.grid();
        let mut found: Vec<Snap> = Vec::new();
        let mut consider = |p: Point, kind: SnapKind| {
            let distance = distance(at, p);
            if distance <= tolerance {
                found.push(Snap { point: p, kind, distance });
            }
        };
        for i in grid.points_near(at, tolerance) {
            if let Some((p, kind)) = self.target_at(i as usize)
                && match kind {
                    SnapKind::Endpoint => options.endpoints,
                    SnapKind::Midpoint => options.midpoints,
                    SnapKind::Centre => options.centres,
                    _ => false,
                }
            {
                consider(p, kind);
            }
        }
        // The middle of the table cell (or box drawn of separate lines) around the point.
        if options.centres
            && let Some([x0, y0, x1, y1]) = self.enclosing_box(at, CELL_REACH.max(tolerance))
        {
            consider([(x0 + x1) / 2.0, (y0 + y1) / 2.0], SnapKind::Centre);
        }
        // Only segments in the pointer's neighbourhood participate in intersection tests.
        // Cap the local quadratic work even on a malicious densely overlapping drawing.
        let near = grid.segments_near(at, tolerance);
        let segment = |i: &u32| self.segments.get(*i as usize);
        let local: Vec<_> = near.iter().filter_map(segment).filter(|[a, b]| distance(at, nearest(at, *a, *b)) <= tolerance).take(256).collect();
        if options.intersections {
            for (i, s) in local.iter().enumerate() {
                for t in local.iter().skip(i + 1) {
                    if let Some(p) = intersection(s[0], s[1], t[0], t[1]) {
                        consider(p, SnapKind::Intersection);
                    }
                }
            }
        }
        found.sort_by(candidate_order);
        let mut out: Vec<Snap> = Vec::new();
        for s in found {
            if out.len() >= MAX_CANDIDATES {
                break;
            }
            if !out.iter().any(|o| distance(o.point, s.point) <= 1e-6) {
                out.push(s);
            }
        }
        if options.paths {
            let path = near
                .iter()
                .filter_map(segment)
                .map(|[a, b]| nearest(at, *a, *b))
                .map(|p| (distance(at, p), p))
                .filter(|(d, _)| *d <= tolerance)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((d, p)) = path
                && !out.iter().any(|o| distance(o.point, p) <= 1e-6)
            {
                out.push(Snap { point: p, kind: SnapKind::Path, distance: d });
            }
        }
        Ok(out)
    }
    /// Target `i` of the grid's numbering (endpoints, midpoints, centres) and its kind.
    fn target_at(&self, i: usize) -> Option<(Point, SnapKind)> {
        let (e, m) = (self.endpoints.len(), self.midpoints.len());
        if i < e {
            self.endpoints.get(i).map(|p| (*p, SnapKind::Endpoint))
        } else if i < e.saturating_add(m) {
            self.midpoints.get(i - e).map(|p| (*p, SnapKind::Midpoint))
        } else {
            self.centres.get(i - e - m).map(|p| (*p, SnapKind::Centre))
        }
    }
    /// The box drawn around `at`: the nearest horizontal segment above and below it and vertical
    /// segment left and right of it that pass `at` on their axis, each within `reach`, as
    /// `[left, bottom, right, top]` in user space. A check box or a table cell; `None` when a
    /// side is missing (or the point is on a side).
    pub fn enclosing_box(&self, at: Point, reach: f64) -> Option<[f64; 4]> {
        if check_points(&[at]).is_err() || !reach.is_finite() || !(0.0..=10_000.0).contains(&reach) {
            return None;
        }
        // Nearest side in each direction, as a distance from `at`.
        let (mut left, mut right, mut below, mut above) = (f64::MAX, f64::MAX, f64::MAX, f64::MAX);
        let near = self.grid().segments_near(at, reach);
        for [a, b] in near.iter().filter_map(|i| self.segments.get(*i as usize)) {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = dx.hypot(dy);
            // Within half a degree of the axis.
            let level = dy.abs() <= length * 0.01;
            let upright = dx.abs() <= length * 0.01;
            if level && at[0] >= a[0].min(b[0]) && at[0] <= a[0].max(b[0]) {
                let d = (a[1] + b[1]) / 2.0 - at[1];
                if d > 1e-9 {
                    above = above.min(d);
                } else if d < -1e-9 {
                    below = below.min(-d);
                }
            } else if upright && at[1] >= a[1].min(b[1]) && at[1] <= a[1].max(b[1]) {
                let d = (a[0] + b[0]) / 2.0 - at[0];
                if d > 1e-9 {
                    right = right.min(d);
                } else if d < -1e-9 {
                    left = left.min(-d);
                }
            }
        }
        [left, right, below, above].iter().all(|d| *d <= reach).then(|| [at[0] - left, at[1] - below, at[0] + right, at[1] + above])
    }
    /// Whether the segment was accepted (finite, non-degenerate and within the cap).
    fn segment(&mut self, a: Point, b: Point) -> bool {
        if self.segments.len() >= self.cap {
            self.truncated = true;
            return false;
        }
        if check_points(&[a, b]).is_ok() && distance(a, b) > 1e-9 {
            self.segments.push([a, b]);
            return true;
        }
        false
    }
    /// Snap targets are recorded only for accepted geometry (so they follow its cap).
    fn target(&mut self, ends: [Point; 2], mid: Point) {
        self.endpoints.extend(ends);
        self.midpoints.push(mid);
    }
    fn edge(&mut self, a: Point, b: Point) {
        if self.segment(a, b) {
            self.target([a, b], [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        }
    }
    fn curve(&mut self, points: [Point; 4], depth: usize) {
        let [a, b, c, d] = points;
        if self.segments.len() >= self.cap {
            self.truncated = true;
            return;
        }
        if depth >= 12 || distance(b, nearest(b, a, d)).max(distance(c, nearest(c, a, d))) <= 0.2 {
            self.segment(a, d);
            return;
        }
        let avg = |a: Point, b: Point| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let (ab, bc, cd) = (avg(a, b), avg(b, c), avg(c, d));
        let (abc, bcd) = (avg(ab, bc), avg(bc, cd));
        let mid = avg(abc, bcd);
        self.curve([a, ab, abc, mid], depth + 1);
        self.curve([mid, bcd, cd, d], depth + 1);
    }
}
/// Rotate a proposed vertex onto the nearest 45-degree ray from the last vertex.
pub fn constrain(last: Point, point: Point) -> Point {
    let v = sub(point, last);
    let length = v[0].hypot(v[1]);
    let angle = (v[1].atan2(v[0]) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    [last[0] + length * angle.cos(), last[1] + length * angle.sin()]
}
struct Walker<'a> {
    doc: &'a Document,
    geometry: Geometry,
    seen: HashSet<ObjRef>,
    bytes: usize,
    streams: usize,
    limits: Limits,
}
impl Walker<'_> {
    fn walk(&mut self, object: &Object, resources: &Dict, initial: Matrix, depth: usize) -> Result<()> {
        if depth >= 32 || self.streams >= 4096 || self.geometry.truncated {
            self.geometry.truncated = true;
            return Ok(());
        }
        let o = self.doc.resolve(object);
        if let Some(arr) = o.as_array() {
            // Contents arrays form one stream; q/Q and paths may span stream boundaries.
            let mut joined = Vec::new();
            for part in arr {
                let s = self.doc.resolve(part);
                if let Object::Stream(s) = s.as_ref() {
                    let remaining = self.limits.bytes.saturating_sub(self.bytes).saturating_sub(joined.len());
                    if remaining == 0 {
                        self.geometry.truncated = true;
                        break;
                    }
                    // An undecodable part is skipped; the parts before it still snap.
                    match s.decoded_within(remaining) {
                        Ok(data) => {
                            joined.extend(data);
                            // A space, as the renderer joins pieces: a trailing comment runs on.
                            joined.push(b' ');
                        }
                        Err(_) => self.decode_failed(remaining),
                    }
                }
            }
            return self.ops(&joined, resources, initial, depth);
        }
        let Object::Stream(stream) = o.as_ref() else { return Ok(()) };
        let remaining = self.limits.bytes.saturating_sub(self.bytes);
        if remaining == 0 {
            self.geometry.truncated = true;
            return Ok(());
        }
        match stream.decoded_within(remaining) {
            Ok(data) => self.ops(&data, resources, initial, depth),
            Err(_) => {
                self.decode_failed(remaining);
                Ok(())
            }
        }
    }
    /// A tolerant decode fails on an unsupported filter or once it has produced `remaining` bytes,
    /// so a failure uses up the byte budget: repeating a failing stream cannot repeat that work.
    fn decode_failed(&mut self, remaining: usize) {
        self.geometry.unreadable = self.geometry.unreadable.saturating_add(1);
        self.bytes = self.bytes.saturating_add(remaining);
    }
    fn ops(&mut self, data: &[u8], resources: &Dict, initial: Matrix, depth: usize) -> Result<()> {
        self.bytes = self.bytes.saturating_add(data.len());
        self.streams = self.streams.saturating_add(1);
        let mut matrix = initial;
        let mut stack = Vec::new();
        let mut start = None;
        let mut current = None;
        let mut closures = Vec::new();
        // Read on the first `Do` and kept: resolving a direct dictionary copies it.
        let mut xobjects = None;

        // The subpaths of the path being built (the last one still open), for their centres.
        let mut subpaths: Vec<Subpath> = Vec::new();
        let mut path = Geometry::with_limits(self.limits);
        let point = |m: Matrix, x: f64, y: f64| {
            let (x, y) = m.apply(x, y);
            [x, y]
        };
        for op in parse(data).ops {
            if self.geometry.segments.len().saturating_add(path.segments.len()) >= self.geometry.cap {
                self.geometry.truncated = true;
                break;
            }
            match String::from_utf8_lossy(&op.op).as_ref() {
                "q" => {
                    if stack.len() < 256 {
                        stack.push(matrix);
                    } else {
                        // Later transforms can't be tracked reliably; keep what was found.
                        self.geometry.truncated = true;
                        break;
                    }
                }
                "Q" => {
                    matrix = stack.pop().unwrap_or(initial);
                }
                "cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        matrix = m.then(&matrix);
                    }
                }
                "m" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if !remember_close(&mut closures, current, start) {
                            self.geometry.truncated = true;
                            break;
                        }
                        let p = point(matrix, x, y);
                        current = Some(p);
                        start = Some(p);
                        if subpaths.len() >= MAX_SEGMENTS {
                            self.geometry.truncated = true;
                            break;
                        }
                        subpaths.push(Subpath::new(p));
                    }
                }
                "l" => {
                    if let (Some(a), Some([x, y])) = (current, op.nums::<2>()) {
                        let b = point(matrix, x, y);
                        path.edge(a, b);
                        current = Some(b);
                        if let Some(sp) = subpaths.last_mut() {
                            sp.add(b);
                        }
                    }
                }
                "c" | "v" | "y" => {
                    if let Some(a) = current {
                        let curve = match String::from_utf8_lossy(&op.op).as_ref() {
                            "c" => op
                                .nums::<6>()
                                .map(|[x1, y1, x2, y2, x3, y3]| [a, point(matrix, x1, y1), point(matrix, x2, y2), point(matrix, x3, y3)]),
                            "v" => op.nums::<4>().map(|[x2, y2, x3, y3]| [a, a, point(matrix, x2, y2), point(matrix, x3, y3)]),
                            _ => op.nums::<4>().map(|[x1, y1, x3, y3]| {
                                let d = point(matrix, x3, y3);
                                [a, point(matrix, x1, y1), d, d]
                            }),
                        };
                        if let Some([a, b, c, d]) = curve
                            && check_points(&[a, b, c, d]).is_ok()
                        {
                            let before = path.segments.len();
                            path.curve([a, b, c, d], 0);
                            if path.segments.len() > before {
                                path.target([a, d], [(a[0] + 3.0 * b[0] + 3.0 * c[0] + d[0]) / 8.0, (a[1] + 3.0 * b[1] + 3.0 * c[1] + d[1]) / 8.0]);
                            }
                            current = Some(d);
                            // The curve's middle stands for its bulge in the box (a circle's four
                            // quarters reach its full extent there).
                            if let Some(sp) = subpaths.last_mut() {
                                sp.add([(a[0] + 3.0 * b[0] + 3.0 * c[0] + d[0]) / 8.0, (a[1] + 3.0 * b[1] + 3.0 * c[1] + d[1]) / 8.0]);
                                sp.add(d);
                            }
                        }
                    }
                }
                "h" => {
                    if let (Some(a), Some(b)) = (current, start) {
                        path.edge(a, b);
                        current = Some(b);
                    }
                    if let Some(sp) = subpaths.last_mut() {
                        sp.closed = true;
                    }
                }
                "re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        if !remember_close(&mut closures, current, start) {
                            self.geometry.truncated = true;
                            break;
                        }
                        let a = point(matrix, x, y);
                        let b = point(matrix, x + w, y);
                        let c = point(matrix, x + w, y + h);
                        let d = point(matrix, x, y + h);
                        for (f, t) in [(a, b), (b, c), (c, d), (d, a)] {
                            path.edge(f, t);
                        }
                        current = Some(a);
                        start = Some(a);
                        if subpaths.len() >= MAX_SEGMENTS {
                            self.geometry.truncated = true;
                            break;
                        }
                        let mut sp = Subpath::new(a);
                        for p in [b, c, d] {
                            sp.add(p);
                        }
                        sp.closed = true;
                        subpaths.push(sp);
                    }
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                    let paint = String::from_utf8_lossy(&op.op);
                    if matches!(paint.as_ref(), "f" | "F" | "f*" | "B" | "B*" | "b" | "b*") {
                        if !remember_close(&mut closures, current, start) {
                            self.geometry.truncated = true;
                            break;
                        }
                        // Filling implicitly closes every open subpath, not just the last one.
                        // Stroke-only S leaves its open subpaths unchanged.
                        for [a, b] in closures.drain(..) {
                            if self.geometry.segments.len().saturating_add(path.segments.len()) >= self.geometry.cap {
                                path.truncated = true;
                                break;
                            }
                            path.edge(a, b);
                        }
                    } else if paint == "s"
                        && let (Some(a), Some(b)) = (current, start)
                    {
                        path.edge(a, b);
                    }
                    let filled = !matches!(paint.as_ref(), "S" | "s");
                    let closes_last = paint == "s";
                    let count = subpaths.len();
                    for (i, sp) in subpaths.drain(..).enumerate() {
                        let sp = if closes_last && i + 1 == count { Subpath { closed: true, ..sp } } else { sp };
                        if let Some(c) = sp.centre(filled) {
                            path.centres.push(c);
                        }
                    }
                    self.geometry.segments.append(&mut path.segments);
                    self.geometry.endpoints.append(&mut path.endpoints);
                    self.geometry.midpoints.append(&mut path.midpoints);
                    self.geometry.centres.append(&mut path.centres);
                    self.geometry.truncated |= path.truncated;
                    closures.clear();
                    current = None;
                    start = None;
                }
                "n" => {
                    self.geometry.truncated |= path.truncated;
                    path = Geometry::with_limits(self.limits);
                    closures.clear();
                    subpaths.clear();
                    current = None;
                    start = None;
                }
                "Do" => {
                    if let Some(name) = op.name(0) {
                        let xobjects = xobjects.get_or_insert_with(|| {
                            #[cfg(test)]
                            XOBJECT_DICT_READS.with(|n| n.set(n.get() + 1));
                            resources.get(b"XObject").map(|o| self.doc.resolve(o))
                        });
                        if let Some(o) = xobjects.as_ref().and_then(|o| o.as_dict()).and_then(|d| d.get(name)) {
                            let r = o.as_ref();
                            if r.is_some_and(|r| self.seen.contains(&r)) {
                                continue;
                            }
                            let form = self.doc.resolve(o);
                            if let Object::Stream(s) = form.as_ref()
                                && s.dict.name(b"Subtype") == Some(b"Form")
                            {
                                let m = s
                                    .dict
                                    .get(b"Matrix")
                                    .map(|m| self.doc.resolve(m))
                                    .and_then(|o| o.as_array().and_then(|a| Matrix::from_operands(a)))
                                    .unwrap_or_default()
                                    .then(&matrix);
                                let own = s.dict.get(b"Resources").map(|o| self.doc.resolve(o));
                                let res = own.as_ref().and_then(|o| o.as_dict()).unwrap_or(resources);
                                if let Some(r) = r {
                                    self.seen.insert(r);
                                }
                                self.walk(o, res, m, depth + 1)?;
                                if let Some(r) = r {
                                    self.seen.remove(&r);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
#[cfg(test)]
thread_local! {
    /// Times a content stream's `/XObject` resources were read.
    pub(crate) static XOBJECT_DICT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The page's painted vector geometry, within the default [`Limits`].
pub fn geometry(doc: &Document, page_index: usize) -> Result<Geometry> {
    geometry_within(doc, page_index, Limits::default())
}
/// [`geometry`] within the given limits.
pub fn geometry_within(doc: &Document, page_index: usize, limits: Limits) -> Result<Geometry> {
    let p = crate::page(doc, page_index)?;
    let resources = p.dict.get(b"Resources").map(|o| doc.resolve(o));
    let empty = Dict::new();
    let resources = resources.as_ref().and_then(|o| o.as_dict()).unwrap_or(&empty);
    let mut walker = Walker { doc, geometry: Geometry::with_limits(limits), seen: HashSet::new(), bytes: 0, streams: 0, limits };
    // Extraction is lenient: limits and unreadable streams are reported in the geometry.
    if let Some(contents) = p.dict.get(b"Contents") {
        walker.walk(contents, resources, Matrix::IDENTITY, 0)?;
    }
    Ok(walker.geometry)
}
