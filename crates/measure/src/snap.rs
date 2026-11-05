//! Bounded path extraction and snapping in PDF user space, including nested Form XObjects.
use crate::{Point, Result, check_points, distance, invalid};
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use serde::Serialize;
use std::collections::HashSet;

/// Open subpaths remembered for a fill, which closes each of them.
const MAX_OPEN_SUBPATHS: usize = Limits::DEFAULT.segments;
/// How much of a page extraction reads and keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Straight segments (curves are split into several).
    pub segments: usize,
    /// Endpoints and midpoints together, across the whole page.
    pub targets: usize,
    /// Decoded content bytes, across every stream read.
    pub bytes: usize,
}
impl Limits {
    /// Enough for heavy CAD sheets (several million segments), and
    /// the renderer's own content limit. At about 40 bytes a segment, the cap holds ~320 MB, plus
    /// up to [`MAX_INDEX_ENTRIES`] in the index.
    pub const DEFAULT: Self = Self { segments: 8_000_000, targets: 24_000_000, bytes: 512 << 20 };
    /// What a window can read without missing frames that matter.
    pub const WITHIN_A_FRAME: Self = Self { segments: 20_000, targets: 40_000, bytes: 16 << 20 };
    /// Within what the index can number.
    fn indexable(self) -> Self {
        let most = u32::MAX as usize / 2;
        Self { segments: self.segments.min(most), targets: self.targets.min(most), ..self }
    }
}
impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
/// The page's drawn geometry in user space, compactly (`f32`: about 0.0003 units of precision at
/// A0 sizes), with a grid index so that a snap looks only near the pointer.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    /// Segments as `[ax, ay, bx, by]`.
    segs: Vec<[f32; 4]>,
    /// Snap targets: a straight edge's or curve's two ends, then its midpoint.
    tgts: Vec<[f32; 6]>,
    /// A limit stopped extraction early (segments, targets, bytes, streams, nesting).
    pub truncated: bool,
    /// Content streams that couldn't be decoded and were skipped.
    pub unreadable: usize,
    limits: Limits,
    grid: Grid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Intersection,
    Path,
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
}
impl Default for SnapOptions {
    fn default() -> Self {
        Self { endpoints: true, midpoints: true, intersections: true, paths: true }
    }
}
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
/// Save a subpath's implicit closing edge for fill operations. An unpainted or
/// stroke-only open path must not gain that edge.
fn remember_close(closures: &mut Vec<[Point; 2]>, current: Option<Point>, start: Option<Point>) -> bool {
    if let (Some(a), Some(b)) = (current, start)
        && distance(a, b) > 1e-9
    {
        if closures.len() >= MAX_OPEN_SUBPATHS {
            return false;
        }
        closures.push([a, b]);
    }
    true
}

fn seg(s: &[f32; 4]) -> [Point; 2] {
    [[f64::from(s[0]), f64::from(s[1])], [f64::from(s[2]), f64::from(s[3])]]
}
fn pt(x: f32, y: f32) -> Point {
    [f64::from(x), f64::from(y)]
}
impl Geometry {
    fn with_limits(limits: Limits) -> Self {
        Self { limits, ..Self::default() }
    }
    /// How many segments were kept.
    pub fn segment_count(&self) -> usize {
        self.segs.len()
    }
    /// The segments, in drawing order.
    pub fn segments(&self) -> impl ExactSizeIterator<Item = [Point; 2]> + '_ {
        self.segs.iter().map(seg)
    }
    /// Snap endpoints: each edge's or curve's two ends, in drawing order.
    pub fn endpoints(&self) -> impl Iterator<Item = Point> + '_ {
        self.tgts.iter().flat_map(|t| [pt(t[0], t[1]), pt(t[2], t[3])])
    }
    /// Snap midpoints: each edge's or curve's middle, in drawing order.
    pub fn midpoints(&self) -> impl Iterator<Item = Point> + '_ {
        self.tgts.iter().map(|t| pt(t[4], t[5]))
    }
    /// Dense intersection searches are capped independently from path extraction.
    pub fn intersection_limited(&self, at: Point, tolerance: f64) -> bool {
        if tolerance.is_nan() || tolerance < 0.0 {
            return false;
        }
        let (segs, _) = self.grid.near(self, at, tolerance);
        segs.iter()
            .filter_map(|&i| self.segs.get(i as usize))
            .map(seg)
            .filter(|[a, b]| distance(at, nearest(at, *a, *b)) <= tolerance)
            .take(257)
            .count()
            > 256
    }
    pub fn snap(&self, at: Point, tolerance: f64, options: SnapOptions) -> Result<Option<Snap>> {
        check_points(&[at])?;
        if !tolerance.is_finite() || !(0.0..=10_000.0).contains(&tolerance) {
            return Err(invalid("snap tolerance must be between 0 and 10000 user units"));
        }
        let mut best: Option<Snap> = None;
        let consider = |best: &mut Option<Snap>, p: Point, kind: SnapKind| {
            let distance = distance(at, p);
            // Discrete targets take precedence over projections at the same distance.
            if distance <= tolerance && best.is_none_or(|b| distance < b.distance - 1e-9) {
                *best = Some(Snap { point: p, kind, distance });
            }
        };
        // Only what the grid finds near the pointer: everything within `tolerance` is there, in
        // drawing order, so the result is the one a scan of the whole page would give.
        let (segs, tgts) = self.grid.near(self, at, tolerance);
        let targets: Vec<&[f32; 6]> = tgts.iter().filter_map(|&i| self.tgts.get(i as usize)).collect();
        if options.endpoints {
            for t in &targets {
                consider(&mut best, pt(t[0], t[1]), SnapKind::Endpoint);
                consider(&mut best, pt(t[2], t[3]), SnapKind::Endpoint);
            }
        }
        if options.midpoints {
            for t in &targets {
                consider(&mut best, pt(t[4], t[5]), SnapKind::Midpoint);
            }
        }
        let near: Vec<[Point; 2]> = segs.iter().filter_map(|&i| self.segs.get(i as usize)).map(seg).collect();
        // Only segments in the pointer's neighbourhood participate in intersection tests.
        // Cap the local quadratic work even on a malicious densely overlapping drawing.
        let local: Vec<_> = near.iter().filter(|[a, b]| distance(at, nearest(at, *a, *b)) <= tolerance).take(256).collect();
        if options.intersections {
            for (i, s) in local.iter().enumerate() {
                for t in local.iter().skip(i + 1) {
                    if let Some(p) = intersection(s[0], s[1], t[0], t[1]) {
                        consider(&mut best, p, SnapKind::Intersection);
                    }
                }
            }
        }
        if options.paths && best.is_none() {
            for [a, b] in &near {
                consider(&mut best, nearest(at, *a, *b), SnapKind::Path);
            }
        }
        Ok(best)
    }
    /// Endpoints and midpoints together.
    fn targets(&self) -> usize {
        self.tgts.len().saturating_mul(3)
    }
    /// Whether the segment was accepted (finite, non-degenerate and within the cap).
    fn segment(&mut self, a: Point, b: Point) -> bool {
        if self.segs.len() >= self.limits.segments {
            self.truncated = true;
            return false;
        }
        if check_points(&[a, b]).is_ok() && distance(a, b) > 1e-9 {
            self.segs.push([a[0] as f32, a[1] as f32, b[0] as f32, b[1] as f32]);
            return true;
        }
        false
    }
    /// Snap targets are recorded only for accepted geometry, and are capped.
    fn target(&mut self, ends: [Point; 2], mid: Point) {
        if self.targets().saturating_add(3) > self.limits.targets {
            self.truncated = true;
            return;
        }
        let [a, b] = ends;
        self.tgts.push([a[0] as f32, a[1] as f32, b[0] as f32, b[1] as f32, mid[0] as f32, mid[1] as f32]);
    }
    fn edge(&mut self, a: Point, b: Point) {
        if self.segment(a, b) {
            self.target([a, b], [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        }
    }
    fn curve(&mut self, points: [Point; 4], depth: usize) {
        let [a, b, c, d] = points;
        if self.segs.len() >= self.limits.segments {
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
        let mut path = Geometry::with_limits(self.limits);
        // Read on the first `Do` and kept: resolving a direct dictionary copies it.
        let mut xobjects = None;
        let point = |m: Matrix, x: f64, y: f64| {
            let (x, y) = m.apply(x, y);
            [x, y]
        };
        for op in parse(data).ops {
            if self.geometry.segs.len().saturating_add(path.segs.len()) >= self.limits.segments
                || self.geometry.targets().saturating_add(path.targets()) >= self.limits.targets
            {
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
                    }
                }
                "l" => {
                    if let (Some(a), Some([x, y])) = (current, op.nums::<2>()) {
                        let b = point(matrix, x, y);
                        path.edge(a, b);
                        current = Some(b);
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
                            let before = path.segs.len();
                            path.curve([a, b, c, d], 0);
                            if path.segs.len() > before {
                                path.target([a, d], [(a[0] + 3.0 * b[0] + 3.0 * c[0] + d[0]) / 8.0, (a[1] + 3.0 * b[1] + 3.0 * c[1] + d[1]) / 8.0]);
                            }
                            current = Some(d);
                        }
                    }
                }
                "h" => {
                    if let (Some(a), Some(b)) = (current, start) {
                        path.edge(a, b);
                        current = Some(b);
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
                            if self.geometry.segs.len().saturating_add(path.segs.len()) >= self.limits.segments
                                || self.geometry.targets().saturating_add(path.targets()) >= self.limits.targets
                            {
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
                    self.geometry.segs.append(&mut path.segs);
                    self.geometry.tgts.append(&mut path.tgts);
                    self.geometry.truncated |= path.truncated;
                    closures.clear();
                    current = None;
                    start = None;
                }
                "n" => {
                    self.geometry.truncated |= path.truncated;
                    path = Geometry::with_limits(self.limits);
                    closures.clear();
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

/// The page's geometry for snapping, within [`Limits::DEFAULT`].
pub fn geometry(doc: &Document, page_index: usize) -> Result<Geometry> {
    geometry_with_limits(doc, page_index, Limits::DEFAULT)
}

/// The page's geometry for snapping, within `limits`.
pub fn geometry_with_limits(doc: &Document, page_index: usize, limits: Limits) -> Result<Geometry> {
    let limits = limits.indexable();
    let p = crate::page(doc, page_index)?;
    let resources = p.dict.get(b"Resources").map(|o| doc.resolve(o));
    let empty = Dict::new();
    let resources = resources.as_ref().and_then(|o| o.as_dict()).unwrap_or(&empty);
    let mut walker = Walker { doc, geometry: Geometry::with_limits(limits), seen: HashSet::new(), bytes: 0, streams: 0, limits };
    // Extraction is lenient: limits and unreadable streams are reported in the geometry.
    if let Some(contents) = p.dict.get(b"Contents") {
        walker.walk(contents, resources, Matrix::IDENTITY, 0)?;
    }
    let mut geometry = walker.geometry;
    geometry.segs.shrink_to_fit();
    geometry.tgts.shrink_to_fit();
    geometry.grid = Grid::build(&geometry, MAX_INDEX_ENTRIES);
    Ok(geometry)
}

/// The most cells a side of the grid has.
const GRID_SIDE: usize = 1024;
/// A segment whose box covers more cells than this is kept in one list checked at every snap
/// (a long line would otherwise fill a whole row of cells).
const WIDE_CELLS: usize = 64;
/// The most cells one snap reads; a larger search area (a huge tolerance) scans everything.
const MAX_QUERY_CELLS: usize = 4096;
/// The most entries the segment lists of the cells hold (4 bytes each). Past it, shorter segments
/// go in the list checked at every snap: snaps get slower, and find the same.
const MAX_INDEX_ENTRIES: u64 = 64_000_000;

/// A uniform grid over the geometry's box: per cell, the segments and targets touching it.
#[derive(Clone, Debug, Default)]
struct Grid {
    x0: f32,
    y0: f32,
    cell: f32,
    cols: usize,
    rows: usize,
    /// Per cell, where its segment indices start in `seg_items` (one more entry than cells).
    seg_start: Vec<u32>,
    seg_items: Vec<u32>,
    tgt_start: Vec<u32>,
    tgt_items: Vec<u32>,
    /// Segments too long for the cells.
    wide: Vec<u32>,
}

impl Grid {
    fn build(g: &Geometry, max_entries: u64) -> Self {
        let mut bounds: Option<[f32; 4]> = None;
        let mut grow = |x: f32, y: f32| {
            let b = bounds.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        };
        for s in &g.segs {
            grow(s[0], s[1]);
            grow(s[2], s[3]);
        }
        for t in &g.tgts {
            grow(t[0], t[1]);
            grow(t[2], t[3]);
            grow(t[4], t[5]);
        }
        let Some([x0, y0, x1, y1]) = bounds else { return Self::default() };
        let items = g.segs.len().max(g.tgts.len()).max(1);
        // About eight items a cell.
        let side = ((items as f64 / 8.0).sqrt().ceil() as usize).clamp(1, GRID_SIDE);
        let extent = (x1 - x0).max(y1 - y0).max(1e-3);
        let cell = extent / side as f32;
        let cols = (((x1 - x0) / cell) as usize + 1).min(GRID_SIDE);
        let rows = (((y1 - y0) / cell) as usize + 1).min(GRID_SIDE);
        let mut grid = Self { x0, y0, cell, cols, rows, ..Self::default() };
        let cells = cols * rows;
        // Segments: count, then fill (compressed rows).
        let seg_cells = |s: &[f32; 4]| grid.range(s[0].min(s[2]), s[1].min(s[3]), s[0].max(s[2]), s[1].max(s[3]));
        let covered = |s: &[f32; 4]| {
            let (c0, r0, c1, r1) = seg_cells(s);
            (c1 - c0 + 1) * (r1 - r0 + 1)
        };
        // A segment of one cell adds one entry, so the last step fits whatever the sheet.
        let most_cells = [WIDE_CELLS, 16, 4]
            .into_iter()
            .find(|&most| g.segs.iter().map(covered).filter(|&n| n <= most).map(|n| n as u64).sum::<u64>() <= max_entries)
            .unwrap_or(1);
        let mut counts = vec![0u32; cells];
        let mut wide = Vec::new();
        for (i, s) in g.segs.iter().enumerate() {
            let (c0, r0, c1, r1) = seg_cells(s);
            if (c1 - c0 + 1) * (r1 - r0 + 1) > most_cells {
                wide.push(i as u32);
                continue;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    counts[r * cols + c] += 1;
                }
            }
        }
        let (seg_start, mut fill) = Self::starts(&counts);
        let mut seg_items = vec![0u32; seg_start.last().copied().unwrap_or(0) as usize];
        for (i, s) in g.segs.iter().enumerate() {
            let (c0, r0, c1, r1) = seg_cells(s);
            if (c1 - c0 + 1) * (r1 - r0 + 1) > most_cells {
                continue;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let k = r * cols + c;
                    seg_items[fill[k] as usize] = i as u32;
                    fill[k] += 1;
                }
            }
        }
        // Targets: the cells of their three points.
        let tgt_cells = |t: &[f32; 6]| {
            let mut cs = [grid.cell_of(t[0], t[1]), grid.cell_of(t[2], t[3]), grid.cell_of(t[4], t[5])];
            cs.sort_unstable();
            cs
        };
        let mut counts = vec![0u32; cells];
        for t in &g.tgts {
            let cs = tgt_cells(t);
            for (j, &k) in cs.iter().enumerate() {
                if j == 0 || cs[j - 1] != k {
                    counts[k] += 1;
                }
            }
        }
        let (tgt_start, mut fill) = Self::starts(&counts);
        let mut tgt_items = vec![0u32; tgt_start.last().copied().unwrap_or(0) as usize];
        for (i, t) in g.tgts.iter().enumerate() {
            let cs = tgt_cells(t);
            for (j, &k) in cs.iter().enumerate() {
                if j == 0 || cs[j - 1] != k {
                    tgt_items[fill[k] as usize] = i as u32;
                    fill[k] += 1;
                }
            }
        }
        grid.seg_start = seg_start;
        grid.seg_items = seg_items;
        grid.tgt_start = tgt_start;
        grid.tgt_items = tgt_items;
        grid.wide = wide;
        grid
    }

    /// Prefix sums of `counts` (the starts), and a copy to fill from.
    fn starts(counts: &[u32]) -> (Vec<u32>, Vec<u32>) {
        let mut start = Vec::with_capacity(counts.len() + 1);
        let mut total = 0u32;
        start.push(0);
        for &c in counts {
            total = total.saturating_add(c);
            start.push(total);
        }
        let fill = start[..counts.len()].to_vec();
        (start, fill)
    }

    fn col(&self, x: f32) -> usize {
        (((x - self.x0) / self.cell).max(0.0) as usize).min(self.cols.saturating_sub(1))
    }
    fn row(&self, y: f32) -> usize {
        (((y - self.y0) / self.cell).max(0.0) as usize).min(self.rows.saturating_sub(1))
    }
    fn cell_of(&self, x: f32, y: f32) -> usize {
        self.row(y) * self.cols + self.col(x)
    }
    /// The cells (first and last column, first and last row) a box covers.
    fn range(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> (usize, usize, usize, usize) {
        (self.col(x0), self.row(y0), self.col(x1), self.row(y1))
    }

    /// The segments and targets near `at` (all within `tolerance` of it, and maybe more), each
    /// list in drawing order without repeats.
    fn near(&self, g: &Geometry, at: Point, tolerance: f64) -> (Vec<u32>, Vec<u32>) {
        let all = || ((0..g.segs.len() as u32).collect(), (0..g.tgts.len() as u32).collect());
        if self.cols == 0 || self.rows == 0 {
            return if g.segs.is_empty() && g.tgts.is_empty() { (Vec::new(), Vec::new()) } else { all() };
        }
        // Rounding to `f32` can stop the box short of a candidate's cell: this much wider can't.
        let t = (tolerance + (at[0].abs().max(at[1].abs()) + tolerance) * 2.0 * f64::from(f32::EPSILON)) as f32;
        let (x, y) = (at[0] as f32, at[1] as f32);
        if !(x.is_finite() && y.is_finite() && t.is_finite()) {
            return all();
        }
        let (c0, r0, c1, r1) = self.range(x - t, y - t, x + t, y + t);
        // Entirely outside the grid: nothing is near (except wide segments).
        let outside =
            x + t < self.x0 || y + t < self.y0 || x - t > self.x0 + self.cell * self.cols as f32 || y - t > self.y0 + self.cell * self.rows as f32;
        if (c1 - c0 + 1) * (r1 - r0 + 1) > MAX_QUERY_CELLS {
            return all();
        }
        let mut segs: Vec<u32> = self.wide.clone();
        let mut tgts: Vec<u32> = Vec::new();
        if !outside {
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let k = r * self.cols + c;
                    let range = |start: &[u32]| (start[k] as usize)..(start[k + 1] as usize);
                    segs.extend_from_slice(self.seg_items.get(range(&self.seg_start)).unwrap_or(&[]));
                    tgts.extend_from_slice(self.tgt_items.get(range(&self.tgt_start)).unwrap_or(&[]));
                }
            }
        }
        segs.sort_unstable();
        segs.dedup();
        tgts.sort_unstable();
        tgts.dedup();
        (segs, tgts)
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    fn geometry(segs: Vec<[f32; 4]>, tgts: Vec<[f32; 6]>, max_entries: u64) -> Geometry {
        let mut g = Geometry { segs, tgts, ..Geometry::default() };
        g.grid = Grid::build(&g, max_entries);
        g
    }

    /// Past the allowance the index holds fewer entries and finds the same.
    #[test]
    fn a_bounded_index_finds_what_a_full_one_does() {
        let mut segs = Vec::new();
        for i in 0..4000u16 {
            let (x, y) = (f32::from(i % 64) * 15.0, f32::from(i / 64) * 15.0);
            segs.push([x, y, x + 37.0, y + 41.0]);
        }
        let tgts = Vec::new();
        let full = geometry(segs.clone(), tgts.clone(), u64::MAX);
        let budget = segs.len() as u64;
        let bounded = geometry(segs, tgts, budget);
        assert!(full.grid.seg_items.len() as u64 > budget, "the sheet needs more than the allowance");
        assert!(bounded.grid.seg_items.len() as u64 <= budget, "{} entries", bounded.grid.seg_items.len());
        let options = SnapOptions { endpoints: false, midpoints: false, intersections: true, paths: true };
        for i in 0..400u16 {
            let at = [f64::from(i) * 2.37, f64::from(i) * 1.13];
            assert_eq!(bounded.snap(at, 6.0, options).unwrap(), full.snap(at, 6.0, options).unwrap(), "at {at:?}");
        }
    }

    #[test]
    fn limits_stay_within_what_the_index_can_number() {
        let limits = Limits { segments: usize::MAX, targets: usize::MAX, bytes: 1 }.indexable();
        assert!(limits.segments <= u32::MAX as usize && limits.targets <= u32::MAX as usize);
        assert_eq!(Limits::DEFAULT.indexable(), Limits::DEFAULT);
    }

    /// A tolerance below zero finds nothing; it must not reverse the cells searched.
    #[test]
    fn a_negative_tolerance_finds_nothing() {
        let segs = (0..4000u16)
            .map(|i| [f32::from(i % 64) * 15.0, f32::from(i / 64) * 15.0, f32::from(i % 64) * 15.0 + 5.0, f32::from(i / 64) * 15.0])
            .collect();
        let g = geometry(segs, Vec::new(), u64::MAX);
        assert!(g.grid.cols > 1 && g.grid.rows > 1);
        assert!(!g.intersection_limited([400.0, 400.0], -200.0));
    }

    /// Near 1e8 one `f32` step is 8 units, which must not hide an end just within tolerance.
    #[test]
    fn rounding_doesnt_hide_a_target_at_the_edge_of_the_grid() {
        let g = geometry(Vec::new(), vec![[100_000_008.0, 0.0, 100_000_016.0, 0.0, 100_000_012.0, 0.0]], u64::MAX);
        let options = SnapOptions { endpoints: true, midpoints: false, intersections: false, paths: false };
        let hit = g.snap([100_000_003.999_999_9, 0.0], 4.000_000_11, options).unwrap();
        assert_eq!(hit.map(|h| h.point), Some([100_000_008.0, 0.0]));
    }

    /// A target is three points, and all three must fit.
    #[test]
    fn a_target_is_kept_only_if_all_its_points_fit() {
        let mut g = Geometry::with_limits(Limits { segments: 10, targets: 2, bytes: 1 });
        g.target([[0.0, 0.0], [1.0, 0.0]], [0.5, 0.0]);
        assert_eq!(g.targets(), 0);
        assert!(g.truncated);
    }
}
