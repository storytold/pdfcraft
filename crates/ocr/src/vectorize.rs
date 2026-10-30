//! Clean-room vectorization of a master glyph bitmap (ClearScan stage 4).
//!
//! [`vectorize`] turns the pristine binary prototype from [`crate::cluster`] into a
//! hole-aware outline of cubic Béziers. Nothing here comes from `potrace`, `autotrace`,
//! or any other GPL tracer. The three algorithms are independent implementations of
//! published methods, and the curve arithmetic is [`kurbo`] (Apache-2.0 OR MIT):
//!
//! - **Contours.** Satoshi Suzuki and Keiichi Abe, *Topological Structural Analysis of
//!   Digitized Binary Images by Border Following*, Computer Vision, Graphics, and Image
//!   Processing 30 (1985). A raster scan starts an outer border where unmarked ink has
//!   paper on its left, and a hole border where ink has paper on its right. Following
//!   the 8-neighborhood (ink 8-connected, the same rule as [`crate::components`]) numbers
//!   every border and records which border surrounds it. The frame of the picture is
//!   border 1, a hole, so a component that nothing else surrounds has no parent.
//! - **Geometry.** The border walk names the pixels. The outline that a fill reproduces
//!   is the crack around those pixels: the unit edges of the pixel squares, traced with
//!   ink kept on the right. Corners sit on the integer grid, so a nonzero fill of an
//!   unsimplified rectangle covers exactly the ink that produced it. Outer cracks wind
//!   clockwise in this y-down pixel space (positive shoelace) and holes wind the other
//!   way; a crack that comes back with the wrong sign is reversed, so the winding is a
//!   property of the border's kind and not of which way the walk stepped.
//! - **Simplification.** David Douglas and Thomas Peucker, *Algorithms for the Reduction
//!   of the Number of Points Required to Represent a Digitized Line or its Caricature*,
//!   Cartographica 10 (1973). Collinear crack steps collapse to the vertices a later
//!   stage would have kept anyway.
//! - **Fitting.** Philip J. Schneider, *An Algorithm for Automatically Fitting Digitized
//!   Curves*, Graphics Gems I (1990). The Graphics Gems listings were placed in the
//!   public domain; this is not a transcription of them. A span is chord-length
//!   parameterized (the parameter knots), least-squares fitted by two scalars along the
//!   endpoint tangents, then reparameterized by Newton on the cubic and its derivative.
//!   When Newton leaves the data point, the nearest parameter is recovered by subdividing
//!   the cubic. A span whose farthest point is still too far from the curve splits there,
//!   and the halves are fitted again. A vertex is a tangent break when its turn exceeds
//!   [`VectorizeParams::corner_angle`] and it is not a one-pixel staircase step. The spans
//!   on either side of a break do not share a tangent.
//!
//! Coordinates are prototype pixels, x right, y down, the same frame as [`crate::binarize::Bitmap`].
//! The Type 3 stage flips y when it writes PDF space.

use std::collections::{HashSet, VecDeque};

use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveDeriv, Point, Vec2};

use crate::binarize::Bitmap;

/// Eight neighborhood, clockwise from east, y down: E, SE, S, SW, W, NW, N, NE.
const NEIGHBORS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// Crack steps, y down: east, south, west, north.
const CRACK_STEP: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
/// Offset from a crack's start corner to the pixel on its right, one entry per direction.
const RIGHT_PIXEL: [(i32, i32); 4] = [(0, 0), (-1, 0), (-1, -1), (0, -1)];
/// Offset from a crack's start corner to the pixel on its left.
const LEFT_PIXEL: [(i32, i32); 4] = [(0, -1), (0, 0), (-1, 0), (-1, -1)];

/// Turns tried at a crack corner, added to the arrival direction: right, straight, left, back.
const CRACK_TURNS: [u8; 4] = [1, 0, 3, 2];

/// Newton reparameterizations attempted before a span is split.
const FIT_ITERATIONS: usize = 4;
/// Subdivisions of a cubic when Newton does not improve the parameter.
const FIT_SUBDIVISIONS: u32 = 4;
/// Schneider recursion never goes deeper than this, whatever the caller asked.
const FIT_DEPTH_CAP: u32 = 32;
/// A control point this close to its chord (pixels) is a straight segment.
const LINE_SLOP: f64 = 0.05;

/// The knobs of [`vectorize`]. Defaults keep a glyph's corners and stay inside half a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorizeParams {
    /// Douglas–Peucker tolerance in pixels. A vertex closer than this to the chord between
    /// its neighbors is dropped. `0` keeps every non-collinear crack corner.
    pub simplify_epsilon: f64,
    /// Schneider accepts a span when every sample lies within this many pixels of the cubic.
    pub max_error: f64,
    /// Smallest turn, in radians, that can be a corner. A staircase step turns this far too,
    /// but it alternates left and right on unit edges, so it stays inside the smooth span.
    /// A corner turns the same way as a neighbor, or it meets an edge longer than a pixel
    /// and a half. A right angle is π/2; the default (π/3) breaks rectangle corners and
    /// leaves a smooth bowl alone.
    pub corner_angle: f64,
    /// How many times a span may be split before the remainder is drawn as one cubic.
    pub max_fit_depth: u32,
    /// Borders beyond this are not followed. The outline says so in [`GlyphOutline::truncated`].
    pub max_contours: u32,
    /// Crack steps one border may take before it is dropped. Also capped at four times the
    /// pixel count, so a walk that never returns cannot allocate without bound.
    pub max_points: u32,
}

impl Default for VectorizeParams {
    fn default() -> Self {
        VectorizeParams {
            simplify_epsilon: 0.5,
            max_error: 0.5,
            corner_angle: std::f64::consts::FRAC_PI_3,
            max_fit_depth: 16,
            max_contours: 4_096,
            max_points: u32::MAX,
        }
    }
}

/// Whether a contour goes around ink or around a hole in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContourKind {
    /// The outer border of a connected component of ink.
    Outer,
    /// A border around paper that this component encloses.
    Hole,
}

/// One closed outline: its kind, the contour that surrounds it, and the fitted path.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorContour {
    kind: ContourKind,
    /// Index into [`GlyphOutline::contours`]. `None` when the frame of the bitmap is the
    /// surrounder (a top-level component). A hole's parent is the outer contour of its
    /// component; an island's parent is the hole it sits in.
    parent: Option<u32>,
    /// Closed path in pixel-corner coordinates, y down. Outers wind clockwise (positive
    /// shoelace) and holes wind counterclockwise, so a nonzero fill punches holes out.
    path: BezPath,
}

impl VectorContour {
    pub fn kind(&self) -> ContourKind {
        self.kind
    }

    pub fn parent(&self) -> Option<u32> {
        self.parent
    }

    pub fn path(&self) -> &BezPath {
        &self.path
    }
}

/// Every contour of one bitmap, in the order the raster scan met them.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    contours: Vec<VectorContour>,
    truncated: bool,
}

impl GlyphOutline {
    pub fn contours(&self) -> &[VectorContour] {
        &self.contours
    }

    /// A border was dropped because it exceeded [`VectorizeParams::max_contours`] or
    /// [`VectorizeParams::max_points`], or the walk did not close.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

/// Trace `bitmap` into a Bézier outline. An empty bitmap, or one with no ink, has no
/// contours. The walk never panics on a hostile bitmap: a border that will not close, or
/// that blows its budget, is omitted and [`GlyphOutline::truncated`] is set.
pub fn vectorize(bitmap: &Bitmap, params: &VectorizeParams) -> GlyphOutline {
    let params = sanitize(params);
    let Some(mut grid) = working_grid(bitmap) else {
        return GlyphOutline { contours: Vec::new(), truncated: false };
    };
    if grid.width == 0 || grid.height == 0 {
        return GlyphOutline { contours: Vec::new(), truncated: false };
    }
    let point_cap = point_budget(bitmap, params.max_points);
    let (borders, mut truncated) = suzuki_abe(&mut grid, params.max_contours);
    let mut built: Vec<Built> = Vec::new();
    for border in &borders {
        let (rings, dropped) = crack_rings(bitmap, border, point_cap);
        // A piece whose crack does not close would leave its pixels out of the fill.
        // Drop the whole contour instead of emitting the pieces that did close.
        if dropped || rings.is_empty() {
            truncated = true;
            continue;
        }
        let mut path = BezPath::new();
        for ring in rings {
            let oriented = orient(ring, border.kind);
            if signed_area(&oriented).abs() < 1e-9 {
                continue;
            }
            let simplified = douglas_peucker_closed(&oriented, params.simplify_epsilon);
            path.extend(fit_ring(&simplified, &params).elements().iter().copied());
        }
        if path.elements().is_empty() {
            truncated = true;
            continue;
        }
        built.push(Built { nbd: border.nbd, parent_nbd: border.parent_nbd, kind: border.kind, path });
    }
    GlyphOutline { contours: resolve_parents(&built), truncated }
}

/// A border the scan found, before its crack is fitted.
struct Border {
    nbd: i32,
    parent_nbd: i32,
    kind: ContourKind,
    start: (i32, i32),
}

struct Built {
    nbd: i32,
    parent_nbd: i32,
    kind: ContourKind,
    path: BezPath,
}

fn sanitize(params: &VectorizeParams) -> VectorizeParams {
    let mut params = *params;
    if !(params.simplify_epsilon.is_finite() && params.simplify_epsilon >= 0.0) {
        params.simplify_epsilon = 0.0;
    }
    if !(params.max_error.is_finite() && params.max_error >= 0.0) {
        params.max_error = 0.5;
    }
    if !params.corner_angle.is_finite() {
        params.corner_angle = std::f64::consts::FRAC_PI_3;
    } else if params.corner_angle < 0.0 {
        params.corner_angle = 0.0;
    }
    params.max_fit_depth = params.max_fit_depth.min(FIT_DEPTH_CAP);
    params
}

fn point_budget(bitmap: &Bitmap, asked: u32) -> usize {
    // Four steps per pixel covers the crack of any planar border with room to return home.
    // The caller's cap can only lower it.
    let from_pixels = bitmap.ink().len().saturating_mul(4).max(8);
    usize::try_from(asked).unwrap_or(from_pixels).min(from_pixels).max(1)
}

struct Grid {
    cells: Vec<i32>,
    width: i32,
    height: i32,
}

fn working_grid(bitmap: &Bitmap) -> Option<Grid> {
    let width = i32::try_from(bitmap.width()).ok()?;
    let height = i32::try_from(bitmap.height()).ok()?;
    let pixels = bitmap.width().checked_mul(bitmap.height())?;
    let len = usize::try_from(pixels).ok()?;
    if bitmap.ink().len() != len {
        return None;
    }
    let mut cells = vec![0_i32; len];
    for (index, flag) in bitmap.ink().iter().enumerate() {
        if *flag != 0
            && let Some(cell) = cells.get_mut(index)
        {
            *cell = 1;
        }
    }
    Some(Grid { cells, width, height })
}

// --- Suzuki–Abe ---------------------------------------------------------------

struct Labels {
    /// `kind_of[nbd]` and `parent_of[nbd]`. Index 1 is the frame, a hole with no parent.
    kind_of: Vec<ContourKind>,
    parent_of: Vec<i32>,
}

struct Scan<'a> {
    grid: &'a mut Grid,
    labels: &'a mut Labels,
    borders: &'a mut Vec<Border>,
    nbd: &'a mut i32,
    lnbd: &'a mut i32,
    cap: usize,
}

fn suzuki_abe(grid: &mut Grid, max_contours: u32) -> (Vec<Border>, bool) {
    let mut labels = Labels { kind_of: vec![ContourKind::Hole, ContourKind::Hole], parent_of: vec![0, 0] };
    let mut borders = Vec::new();
    let mut nbd = 1_i32;
    let mut truncated = false;
    let cap = usize::try_from(max_contours).unwrap_or(usize::MAX);
    let height = grid.height;
    let width = grid.width;
    for y in 0..height {
        // The frame is the border in force at the left of every row.
        let mut lnbd = 1_i32;
        let mut x = 0_i32;
        while x < width {
            let value = grid.get(x, y);
            if value != 0
                && !visit_pixel(&mut Scan { grid, labels: &mut labels, borders: &mut borders, nbd: &mut nbd, lnbd: &mut lnbd, cap }, x, y, value)
            {
                truncated = true;
                break;
            }
            x += 1;
        }
        if truncated {
            break;
        }
    }
    (borders, truncated)
}

/// One raster-scan pixel. Returns false when a new border was required and the budget refused it.
fn visit_pixel(scan: &mut Scan<'_>, x: i32, y: i32, value: i32) -> bool {
    let west = scan.grid.get(x - 1, y);
    let east = scan.grid.get(x + 1, y);
    if value == 1 && west == 0 {
        if !start_border(scan, (x, y), ContourKind::Outer, (-1, 0)) {
            return false;
        }
    } else if value >= 1 && east == 0 {
        // A hole start that already carries a border number adopts that border
        // before the parent is chosen (Suzuki–Abe step 1b).
        if value > 1 {
            *scan.lnbd = value;
        }
        if !start_border(scan, (x, y), ContourKind::Hole, (1, 0)) {
            return false;
        }
    }
    let marked = scan.grid.get(x, y);
    if marked != 1 {
        *scan.lnbd = nbd_abs(marked).max(1);
    }
    true
}

/// Begin the next border at `at`, looking first toward `at + look`. Returns false when the
/// contour budget or the border-number range is spent.
fn start_border(scan: &mut Scan<'_>, at: (i32, i32), kind: ContourKind, look: (i32, i32)) -> bool {
    if scan.borders.len() >= scan.cap {
        return false;
    }
    let Some(next) = scan.nbd.checked_add(1) else {
        return false;
    };
    *scan.nbd = next;
    let parent = parent_nbd(kind, *scan.lnbd, scan.labels);
    remember(scan.labels, *scan.nbd, kind, parent);
    follow(scan.grid, at, (at.0 + look.0, at.1 + look.1), *scan.nbd);
    scan.borders.push(Border { nbd: *scan.nbd, parent_nbd: parent, kind, start: at });
    true
}

fn parent_nbd(kind: ContourKind, lnbd: i32, labels: &Labels) -> i32 {
    let idx = usize::try_from(lnbd).unwrap_or(1);
    let previous = labels.kind_of.get(idx).copied().unwrap_or(ContourKind::Hole);
    if kind == previous { labels.parent_of.get(idx).copied().unwrap_or(0) } else { lnbd.max(1) }
}

fn remember(labels: &mut Labels, nbd: i32, kind: ContourKind, parent: i32) {
    let Some(idx) = usize::try_from(nbd).ok() else {
        return;
    };
    if labels.kind_of.len() == idx {
        labels.kind_of.push(kind);
        labels.parent_of.push(parent);
    }
}

/// Follow the border through `(sx, sy)`, having arrived from the direction of `(px, py)`.
/// Pixels of the border are renumbered `nbd` (or `-nbd` on a right edge) so the scan does
/// not start the same border again and so a later pixel knows which border it belongs to.
fn follow(grid: &mut Grid, start: (i32, i32), previous: (i32, i32), nbd: i32) {
    let (sx, sy) = start;
    let (px, py) = previous;
    let Some(start_dir) = dir_between(sx, sy, px, py) else {
        return;
    };
    let Some((first_x, first_y)) = find_neighbor(grid, sx, sy, start_dir, true) else {
        mark(grid, sx, sy, nbd);
        return;
    };
    let mut prev = (first_x, first_y);
    let mut current = start;
    // A border visits each pixel a handful of times. Twice the canvas is a backstop, not a budget.
    let limit = (grid.width as usize).saturating_mul(grid.height as usize).saturating_mul(2).max(8);
    for _ in 0..limit {
        mark(grid, current.0, current.1, nbd);
        let Some(back) = dir_between(current.0, current.1, prev.0, prev.1) else {
            return;
        };
        let Some(next) = find_neighbor(grid, current.0, current.1, back, false) else {
            return;
        };
        // Back at the start, and the step that got us here is the first step we took.
        if next == start && current == (first_x, first_y) {
            return;
        }
        prev = current;
        current = next;
    }
}

fn mark(grid: &mut Grid, x: i32, y: i32, nbd: i32) {
    if grid.get(x + 1, y) == 0 {
        grid.set(x, y, nbd.saturating_neg());
    } else if grid.get(x, y) == 1 {
        grid.set(x, y, nbd);
    }
}

/// The first nonzero neighbor of `(x, y)`, walking clockwise from `dir` (inclusive) or
/// counterclockwise from the neighbor after `dir`.
fn find_neighbor(grid: &Grid, x: i32, y: i32, dir: u8, clockwise: bool) -> Option<(i32, i32)> {
    for step in 0..8 {
        let offset = if clockwise { dir.wrapping_add(step) } else { dir.wrapping_add(8).wrapping_sub(step).wrapping_sub(1) };
        let (dx, dy) = NEIGHBORS.get(usize::from(offset & 7)).copied().unwrap_or((1, 0));
        let (nx, ny) = (x + dx, y + dy);
        if grid.get(nx, ny) != 0 {
            return Some((nx, ny));
        }
    }
    None
}

fn dir_between(x0: i32, y0: i32, x1: i32, y1: i32) -> Option<u8> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    NEIGHBORS.iter().position(|&step| step == (dx, dy)).and_then(|dir| u8::try_from(dir).ok())
}

impl Grid {
    fn get(&self, x: i32, y: i32) -> i32 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        let Some(index) = row_index(self.width, x, y) else {
            return 0;
        };
        self.cells.get(index).copied().unwrap_or(0)
    }

    fn set(&mut self, x: i32, y: i32, value: i32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let Some(index) = row_index(self.width, x, y) else {
            return;
        };
        if let Some(slot) = self.cells.get_mut(index) {
            *slot = value;
        }
    }
}

fn row_index(width: i32, x: i32, y: i32) -> Option<usize> {
    let width = usize::try_from(width).ok()?;
    let x = usize::try_from(x).ok()?;
    let y = usize::try_from(y).ok()?;
    y.checked_mul(width)?.checked_add(x)
}

fn nbd_abs(value: i32) -> i32 {
    i32::try_from(value.unsigned_abs()).unwrap_or(1)
}

// --- Crack walk ---------------------------------------------------------------

/// Crack rings of one Suzuki border, and whether a piece failed to close. A hole is one ring.
/// An outer border is one ring per 4-connected piece of its 8-connected component: strokes that
/// only touch on a diagonal are still one border (the scan said so) but the pixel squares do
/// not share an edge, so each piece keeps its own subpath and a nonzero fill covers every pixel.
/// The flag is set when the budget runs out before a piece gets back to its start; the caller
/// drops the whole contour rather than fill only the pieces that closed.
fn crack_rings(bitmap: &Bitmap, border: &Border, point_cap: usize) -> (Vec<Vec<(f64, f64)>>, bool) {
    let Some(primary) = crack_walk(bitmap, border, point_cap) else {
        return (Vec::new(), true);
    };
    let mut rings = vec![primary];
    if border.kind != ContourKind::Outer {
        return (rings, false);
    }
    let component = flood(bitmap, border.start, true);
    let mut covered = flood(bitmap, border.start, false);
    let mut rest: Vec<(i32, i32)> = component.difference(&covered).copied().collect();
    rest.sort_unstable();
    for seed in rest {
        if !covered.insert(seed) {
            continue;
        }
        let blob = flood(bitmap, seed, false);
        let Some(start) = blob.iter().min_by_key(|(x, y)| (*y, *x)).copied() else {
            return (rings, true);
        };
        covered.extend(blob);
        let piece = Border { nbd: border.nbd, parent_nbd: border.parent_nbd, kind: ContourKind::Outer, start };
        let Some(ring) = crack_walk(bitmap, &piece, point_cap) else {
            return (rings, true);
        };
        rings.push(ring);
    }
    (rings, false)
}

/// The pixel-corner ring of `border`. Outer borders start at the north-west corner of their
/// start pixel heading east (that pixel is the top-left of its component, so the edge above
/// it is paper). Hole borders start at the north-east corner heading south, into the paper
/// on the start pixel's right. Ink stays on the walker's right, which makes outers clockwise
/// and holes counterclockwise; [`orient`] corrects a ring that came back with the other sign.
fn crack_walk(bitmap: &Bitmap, border: &Border, point_cap: usize) -> Option<Vec<(f64, f64)>> {
    let (mut cx, mut cy, mut dir) = match border.kind {
        ContourKind::Outer => (border.start.0, border.start.1, 0_u8),
        ContourKind::Hole => (border.start.0.checked_add(1)?, border.start.1, 1_u8),
    };
    let start = (cx, cy);
    let mut points = Vec::new();
    for _ in 0..point_cap {
        points.push((f64::from(cx), f64::from(cy)));
        let (nx, ny, nd) = crack_step(bitmap, cx, cy, dir)?;
        cx = nx;
        cy = ny;
        dir = nd;
        if (cx, cy) == start {
            return (points.len() >= 3).then_some(points);
        }
    }
    None
}

/// Ink pixels reachable from `start`. `eight` joins diagonals; otherwise only edges do.
fn flood(bitmap: &Bitmap, start: (i32, i32), eight: bool) -> HashSet<(i32, i32)> {
    let mut seen = HashSet::new();
    if !bitmap.is_ink(i64::from(start.0), i64::from(start.1)) {
        return seen;
    }
    let mut queue = VecDeque::from([start]);
    seen.insert(start);
    let limit = bitmap.ink().len().saturating_add(1);
    while let Some((x, y)) = queue.pop_front() {
        if seen.len() > limit {
            break;
        }
        let steps: &[(i32, i32)] = if eight { &NEIGHBORS } else { &[(1, 0), (-1, 0), (0, 1), (0, -1)] };
        for (dx, dy) in steps {
            let next = (x.saturating_add(*dx), y.saturating_add(*dy));
            if seen.contains(&next) || !bitmap.is_ink(i64::from(next.0), i64::from(next.1)) {
                continue;
            }
            seen.insert(next);
            queue.push_back(next);
        }
    }
    seen
}

fn crack_step(bitmap: &Bitmap, cx: i32, cy: i32, dir: u8) -> Option<(i32, i32, u8)> {
    for turn in CRACK_TURNS {
        let nd = dir.wrapping_add(turn) & 3;
        if crack_edge_is_boundary(bitmap, cx, cy, nd) {
            let (dx, dy) = CRACK_STEP.get(usize::from(nd)).copied().unwrap_or((0, 0));
            return Some((cx.saturating_add(dx), cy.saturating_add(dy), nd));
        }
    }
    None
}

fn crack_edge_is_boundary(bitmap: &Bitmap, cx: i32, cy: i32, dir: u8) -> bool {
    let dir = usize::from(dir & 3);
    let (rx, ry) = RIGHT_PIXEL.get(dir).copied().unwrap_or((0, 0));
    let (lx, ly) = LEFT_PIXEL.get(dir).copied().unwrap_or((0, 0));
    let right = bitmap.is_ink(i64::from(cx.saturating_add(rx)), i64::from(cy.saturating_add(ry)));
    let left = bitmap.is_ink(i64::from(cx.saturating_add(lx)), i64::from(cy.saturating_add(ly)));
    right && !left
}

fn orient(mut points: Vec<(f64, f64)>, kind: ContourKind) -> Vec<(f64, f64)> {
    let area = signed_area(&points);
    let outer_sign = area > 0.0;
    let want_outer = kind == ContourKind::Outer;
    if area != 0.0 && outer_sign != want_outer {
        points.reverse();
    }
    points
}

fn signed_area(points: &[(f64, f64)]) -> f64 {
    let mut sum = 0.0;
    let n = points.len();
    for i in 0..n {
        let Some((x0, y0)) = points.get(i).copied() else {
            continue;
        };
        let Some((x1, y1)) = points.get((i + 1) % n).copied() else {
            continue;
        };
        sum += x0 * y1 - x1 * y0;
    }
    sum * 0.5
}

fn resolve_parents(built: &[Built]) -> Vec<VectorContour> {
    let mut index_of: Vec<Option<u32>> = Vec::new();
    for (index, border) in built.iter().enumerate() {
        let Some(slot) = usize::try_from(border.nbd).ok() else {
            continue;
        };
        if index_of.len() <= slot {
            index_of.resize(slot + 1, None);
        }
        if let Some(place) = index_of.get_mut(slot) {
            *place = u32::try_from(index).ok();
        }
    }
    built
        .iter()
        .map(|border| VectorContour { kind: border.kind, parent: parent_index(border.parent_nbd, &index_of), path: border.path.clone() })
        .collect()
}

fn parent_index(parent_nbd: i32, index_of: &[Option<u32>]) -> Option<u32> {
    let slot = usize::try_from(parent_nbd).ok()?;
    index_of.get(slot).copied().flatten()
}

// --- Douglas–Peucker ----------------------------------------------------------

/// Closed-ring Douglas–Peucker. The ring is split at its leftmost-topmost vertex and at the
/// vertex farthest from that, and each chain is reduced. Both anchors survive, so a closed
/// ring does not collapse to a chord across a concavity.
fn douglas_peucker_closed(points: &[(f64, f64)], epsilon: f64) -> Vec<(f64, f64)> {
    let ring = dedup_ring(points);
    if ring.len() < 3 {
        return ring;
    }
    let anchor = leftmost(&ring);
    let far = farthest(&ring, anchor);
    if anchor == far {
        return ring;
    }
    let mut kept = douglas_peucker_open(&chain(&ring, anchor, far), epsilon);
    let second = douglas_peucker_open(&chain(&ring, far, anchor), epsilon);
    // `far` is the last of the first chain and the first of the second.
    kept.pop();
    kept.extend(second);
    kept.pop();
    if kept.len() >= 3 { kept } else { ring }
}

fn dedup_ring(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut ring = Vec::new();
    for point in points {
        if ring.last().is_none_or(|prev: &(f64, f64)| (prev.0 - point.0).abs() > 1e-9 || (prev.1 - point.1).abs() > 1e-9) {
            ring.push(*point);
        }
    }
    if let (Some(first), Some(last)) = (ring.first().copied(), ring.last().copied())
        && ring.len() >= 2
        && (first.0 - last.0).abs() <= 1e-9
        && (first.1 - last.1).abs() <= 1e-9
    {
        ring.pop();
    }
    ring
}

fn leftmost(ring: &[(f64, f64)]) -> usize {
    let mut best = 0_usize;
    for (index, point) in ring.iter().enumerate().skip(1) {
        let Some(champion) = ring.get(best) else {
            continue;
        };
        if point.0 < champion.0 || (point.0 == champion.0 && point.1 < champion.1) {
            best = index;
        }
    }
    best
}

fn farthest(ring: &[(f64, f64)], from: usize) -> usize {
    let Some(origin) = ring.get(from).copied() else {
        return from;
    };
    let mut best = from;
    let mut best_d = -1.0;
    for (index, point) in ring.iter().enumerate() {
        let d = (point.0 - origin.0).powi(2) + (point.1 - origin.1).powi(2);
        if d > best_d {
            best_d = d;
            best = index;
        }
    }
    best
}

/// The inclusive chain from `start` to `end`, wrapping around the ring.
fn chain(ring: &[(f64, f64)], start: usize, end: usize) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    if ring.is_empty() {
        return out;
    }
    let mut index = start;
    for _ in 0..=ring.len() {
        if let Some(point) = ring.get(index) {
            out.push(*point);
        }
        if index == end {
            break;
        }
        index = (index + 1) % ring.len();
    }
    out
}

fn douglas_peucker_open(points: &[(f64, f64)], epsilon: f64) -> Vec<(f64, f64)> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; n];
    if let Some(slot) = keep.get_mut(0) {
        *slot = true;
    }
    if let Some(slot) = keep.get_mut(n - 1) {
        *slot = true;
    }
    let mut stack = vec![(0_usize, n - 1)];
    while let Some((from, to)) = stack.pop() {
        if to <= from.saturating_add(1) {
            continue;
        }
        let mut max_d = 0.0;
        let mut at = from;
        for index in from + 1..to {
            let Some(point) = points.get(index) else {
                continue;
            };
            let d = match (points.get(from), points.get(to)) {
                (Some(a), Some(b)) => perpendicular_distance(*point, *a, *b),
                _ => 0.0,
            };
            if d > max_d {
                max_d = d;
                at = index;
            }
        }
        if max_d > epsilon
            && let Some(slot) = keep.get_mut(at)
        {
            *slot = true;
            stack.push((from, at));
            stack.push((at, to));
        }
    }
    points.iter().enumerate().filter(|(index, _)| keep.get(*index).copied().unwrap_or(false)).map(|(_, point)| *point).collect()
}

fn perpendicular_distance(point: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= 0.0 || !len2.is_finite() {
        return ((point.0 - a.0).powi(2) + (point.1 - a.1).powi(2)).sqrt();
    }
    let cross = (point.0 - a.0) * dy - (point.1 - a.1) * dx;
    let distance = cross.abs() / len2.sqrt();
    if distance.is_finite() { distance } else { 0.0 }
}

// --- Corners and Schneider ----------------------------------------------------

fn fit_ring(points: &[(f64, f64)], params: &VectorizeParams) -> BezPath {
    let corners = corner_indices(points, params.corner_angle);
    let mut path = BezPath::new();
    let mut started = false;
    if corners.len() >= 2 {
        for window in corners.iter().zip(corners.iter().cycle().skip(1)).take(corners.len()) {
            let (start, end) = window;
            push_span(&mut path, &span_points(points, *start, *end), params, &mut started);
        }
    } else if corners.len() == 1 {
        let Some(corner) = corners.first().copied() else {
            return path;
        };
        push_span(&mut path, &span_points(points, corner, corner), params, &mut started);
    } else {
        push_closed_smooth(&mut path, points, params, &mut started);
    }
    if started {
        path.close_path();
    }
    path
}

fn corner_indices(points: &[(f64, f64)], min_turn: f64) -> Vec<usize> {
    let n = points.len();
    let mut corners = Vec::new();
    if n < 3 {
        return corners;
    }
    // Signed turns, positive when the walk turns right (y down). A pixel staircase alternates
    // left and right at every vertex; a real corner keeps the polygon's turn, or it meets a
    // long edge. Gating on that keeps a diagonal stroke one smooth span and a square four corners.
    let mut turns = vec![0.0_f64; n];
    for index in 0..n {
        let prev = (index + n - 1) % n;
        let next = (index + 1) % n;
        if let Some([from, at, to]) = point3(points, prev, index, next)
            && let Some(slot) = turns.get_mut(index)
        {
            *slot = signed_turn(from, at, to);
        }
    }
    for index in 0..n {
        let turn = turns.get(index).copied().unwrap_or(0.0);
        if turn.abs() < min_turn {
            continue;
        }
        let prev_turn = turns.get((index + n - 1) % n).copied().unwrap_or(0.0);
        let next_turn = turns.get((index + 1) % n).copied().unwrap_or(0.0);
        let same_way = prev_turn.signum() == turn.signum() || next_turn.signum() == turn.signum();
        if same_way || adjacent_edge_is_long(points, index) {
            corners.push(index);
        }
    }
    corners
}

fn adjacent_edge_is_long(points: &[(f64, f64)], index: usize) -> bool {
    let n = points.len();
    let prev = (index + n - 1) % n;
    let next = (index + 1) % n;
    edge_longer_than(points, prev, index, 1.5) || edge_longer_than(points, index, next, 1.5)
}

fn edge_longer_than(points: &[(f64, f64)], a: usize, b: usize, min: f64) -> bool {
    let Some((x0, y0)) = points.get(a).copied() else {
        return false;
    };
    let Some((x1, y1)) = points.get(b).copied() else {
        return false;
    };
    let (dx, dy) = (x1 - x0, y1 - y0);
    dx * dx + dy * dy >= min * min
}

fn signed_turn(prev: (f64, f64), at: (f64, f64), next: (f64, f64)) -> f64 {
    let v1 = (at.0 - prev.0, at.1 - prev.1);
    let v2 = (next.0 - at.0, next.1 - at.1);
    let cross = v1.0 * v2.1 - v1.1 * v2.0;
    let dot = v1.0 * v2.0 + v1.1 * v2.1;
    if cross.is_finite() && dot.is_finite() { cross.atan2(dot) } else { 0.0 }
}

fn point3(points: &[(f64, f64)], a: usize, b: usize, c: usize) -> Option<[(f64, f64); 3]> {
    Some([*points.get(a)?, *points.get(b)?, *points.get(c)?])
}

/// Points from `start` to `end` inclusive, wrapping. A span that begins and ends on the same
/// corner walks the whole ring and repeats the corner, so the fitter sees a closed loop.
fn span_points(points: &[(f64, f64)], start: usize, end: usize) -> Vec<Point> {
    let mut out = Vec::new();
    if points.is_empty() {
        return out;
    }
    let mut index = start % points.len();
    for _ in 0..=points.len() {
        if let Some((x, y)) = points.get(index).copied() {
            out.push(Point::new(x, y));
        }
        if index == end % points.len() && out.len() > 1 {
            break;
        }
        let next = (index + 1) % points.len();
        if next == index {
            break;
        }
        index = next;
    }
    out
}

fn push_span(path: &mut BezPath, points: &[Point], params: &VectorizeParams, started: &mut bool) {
    if points.len() < 2 {
        return;
    }
    let t_hat1 = unit_tangent(points.get(1).copied().unwrap_or(Point::ZERO) - points.first().copied().unwrap_or(Point::ZERO));
    let last = points.len() - 1;
    let t_hat2 = unit_tangent(points.get(last - 1).copied().unwrap_or(Point::ZERO) - points.get(last).copied().unwrap_or(Point::ZERO));
    let mut cubics = Vec::new();
    fit_cubic(points, t_hat1, t_hat2, params.max_error, params.max_fit_depth, &mut cubics);
    append_cubics(path, &cubics, started);
}

fn push_closed_smooth(path: &mut BezPath, points: &[(f64, f64)], params: &VectorizeParams, started: &mut bool) {
    if points.len() < 3 {
        return;
    }
    // One tangent at the join, from the central difference through the repeated start point,
    // so the two cubics that meet there are G1.
    let n = points.len();
    let Some([first, second, last]) = point3(points, 0, 1, n - 1) else {
        return;
    };
    let t_hat = unit_tangent(Vec2::new(second.0 - last.0, second.1 - last.1));
    let mut samples = Vec::with_capacity(n + 1);
    for (x, y) in points {
        samples.push(Point::new(*x, *y));
    }
    samples.push(Point::new(first.0, first.1));
    let mut cubics = Vec::new();
    fit_cubic(&samples, t_hat, -t_hat, params.max_error, params.max_fit_depth, &mut cubics);
    append_cubics(path, &cubics, started);
}

fn append_cubics(path: &mut BezPath, cubics: &[CubicBez], started: &mut bool) {
    for cubic in cubics {
        if !cubic.p0.x.is_finite() || !cubic.p3.x.is_finite() {
            continue;
        }
        if !*started {
            path.move_to(cubic.p0);
            *started = true;
        }
        // Only a cubic that already lies on its chord becomes a line. Gating on the fit
        // tolerance would shave the corner off a shallow bow that was accepted as a curve.
        if controls_on_chord(cubic) {
            path.line_to(cubic.p3);
        } else {
            path.curve_to(cubic.p1, cubic.p2, cubic.p3);
        }
    }
}

fn controls_on_chord(cubic: &CubicBez) -> bool {
    let a = (cubic.p0.x, cubic.p0.y);
    let b = (cubic.p3.x, cubic.p3.y);
    perpendicular_distance((cubic.p1.x, cubic.p1.y), a, b) <= LINE_SLOP && perpendicular_distance((cubic.p2.x, cubic.p2.y), a, b) <= LINE_SLOP
}

fn unit_tangent(v: Vec2) -> Vec2 {
    let len = v.hypot();
    if len > 1e-12 && len.is_finite() { v / len } else { Vec2::new(1.0, 0.0) }
}

/// Schneider's fit of one open span. `t_hat1` points into the span at its start, `t_hat2`
/// points back into the span at its end. Recursion is bounded by `depth`.
fn fit_cubic(points: &[Point], t_hat1: Vec2, t_hat2: Vec2, max_error: f64, depth: u32, out: &mut Vec<CubicBez>) {
    if points.len() < 2 {
        return;
    }
    let Some(first) = points.first().copied() else {
        return;
    };
    let Some(last) = points.last().copied() else {
        return;
    };
    if points.len() == 2 || depth == 0 {
        out.push(heuristic_cubic(first, last, t_hat1, t_hat2));
        return;
    }
    let mut knots = parameter_knots(points);
    let mut curve = generate_bezier(points, &knots, t_hat1, t_hat2);
    let (mut best_error, mut split_at) = max_sample_error(points, curve, &knots);
    let mut best = curve;
    if best_error > max_error {
        for _ in 0..FIT_ITERATIONS {
            knots = reparameterize(points, &knots, best);
            curve = generate_bezier(points, &knots, t_hat1, t_hat2);
            let (error, split) = max_sample_error(points, curve, &knots);
            if error < best_error {
                best_error = error;
                split_at = split;
                best = curve;
            }
            if best_error <= max_error {
                break;
            }
        }
    }
    if best_error.is_finite() && best_error <= max_error {
        out.push(best);
        return;
    }
    let Some(split) = split_index(split_at, points.len()) else {
        out.push(heuristic_cubic(first, last, t_hat1, t_hat2));
        return;
    };
    let (Some(left), Some(right)) = (points.get(..=split), points.get(split..)) else {
        out.push(heuristic_cubic(first, last, t_hat1, t_hat2));
        return;
    };
    let center = center_tangent(points, split);
    let next_depth = depth.saturating_sub(1);
    fit_cubic(left, t_hat1, -center, max_error, next_depth, out);
    fit_cubic(right, center, t_hat2, max_error, next_depth, out);
}

fn split_index(hint: usize, len: usize) -> Option<usize> {
    if len < 3 {
        return None;
    }
    if (1..len - 1).contains(&hint) { Some(hint) } else { Some(len / 2).filter(|index| (1..len - 1).contains(index)) }
}

/// Chord-length parameter knots in `0..=1`. The first is 0 and the last is 1.
fn parameter_knots(points: &[Point]) -> Vec<f64> {
    let mut knots = vec![0.0; points.len()];
    for index in 1..points.len() {
        let Some(prev) = points.get(index - 1).copied() else {
            continue;
        };
        let Some(point) = points.get(index).copied() else {
            continue;
        };
        let step = (point - prev).hypot();
        let step = if step.is_finite() { step } else { 0.0 };
        let previous = knots.get(index - 1).copied().unwrap_or(0.0);
        if let Some(slot) = knots.get_mut(index) {
            *slot = previous + step;
        }
    }
    let total = knots.last().copied().unwrap_or(0.0);
    if total > 1e-12 {
        for knot in &mut knots {
            *knot /= total;
        }
        if let Some(last) = knots.last_mut() {
            *last = 1.0;
        }
        if let Some(first) = knots.first_mut() {
            *first = 0.0;
        }
    }
    knots
}

/// Least squares for the two tangent scale factors. Endpoints stay put; the inner controls
/// slide along `t_hat1` and `t_hat2`. A non-positive or runaway factor falls back to the
/// chord-length heuristic, which keeps the controls between the endpoints.
fn generate_bezier(points: &[Point], knots: &[f64], t_hat1: Vec2, t_hat2: Vec2) -> CubicBez {
    let p0 = points.first().copied().unwrap_or(Point::ZERO);
    let p3 = points.last().copied().unwrap_or(p0);
    let mut c11 = 0.0;
    let mut c12 = 0.0;
    let mut c22 = 0.0;
    let mut x1 = 0.0;
    let mut x2 = 0.0;
    for (index, point) in points.iter().enumerate() {
        let u = knots.get(index).copied().unwrap_or(0.0);
        let b = bernstein(u);
        let a1 = t_hat1 * b[1];
        let a2 = t_hat2 * b[2];
        c11 += a1.dot(a1);
        c12 += a1.dot(a2);
        c22 += a2.dot(a2);
        let fixed = p0.to_vec2() * (b[0] + b[1]) + p3.to_vec2() * (b[2] + b[3]);
        let residual = point.to_vec2() - fixed;
        x1 += a1.dot(residual);
        x2 += a2.dot(residual);
    }
    let chord = (p3 - p0).hypot();
    let fallback = if chord.is_finite() { chord / 3.0 } else { 0.0 };
    let alpha = solve_alphas(c11, c12, c22, x1, x2, fallback, chord);
    CubicBez::new(p0, p0 + t_hat1 * alpha.0, p3 + t_hat2 * alpha.1, p3)
}

fn solve_alphas(c11: f64, c12: f64, c22: f64, x1: f64, x2: f64, fallback: f64, chord: f64) -> (f64, f64) {
    let det = c11 * c22 - c12 * c12;
    let scale = c11.abs() + c22.abs() + 1.0;
    if !det.is_finite() || det.abs() < 1e-12 * scale || c11.abs() < 1e-14 || c22.abs() < 1e-14 {
        return (fallback, fallback);
    }
    let a1 = (c22 * x1 - c12 * x2) / det;
    let a2 = (c11 * x2 - c12 * x1) / det;
    // A control point many chords away is a singular fit, not a sharp corner.
    let limit = chord * 4.0 + 1.0;
    if a1 > 0.0 && a2 > 0.0 && a1.is_finite() && a2.is_finite() && a1 <= limit && a2 <= limit { (a1, a2) } else { (fallback, fallback) }
}

fn bernstein(u: f64) -> [f64; 4] {
    let t = if u.is_finite() { u.clamp(0.0, 1.0) } else { 0.0 };
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

fn max_sample_error(points: &[Point], curve: CubicBez, knots: &[f64]) -> (f64, usize) {
    let mut max_error = 0.0;
    let mut split = points.len() / 2;
    for index in 1..points.len().saturating_sub(1) {
        let Some(point) = points.get(index).copied() else {
            continue;
        };
        let u = knots.get(index).copied().unwrap_or(0.0);
        let delta = (curve.eval(u) - point).hypot();
        if delta.is_finite() && delta > max_error {
            max_error = delta;
            split = index;
        }
    }
    (max_error, split)
}

fn reparameterize(points: &[Point], knots: &[f64], curve: CubicBez) -> Vec<f64> {
    let mut next = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        if index == 0 {
            next.push(0.0);
            continue;
        }
        if index + 1 == points.len() {
            next.push(1.0);
            continue;
        }
        let hint = knots.get(index).copied().unwrap_or(0.0);
        next.push(nearest_t(curve, *point, hint));
    }
    // Newton can step backwards. Chord-length knots have to stay ordered or the next fit
    // solves the wrong assignment; clamping forward keeps the iteration well posed.
    let mut previous = 0.0;
    for knot in &mut next {
        if !knot.is_finite() || *knot < previous {
            *knot = previous;
        }
        if *knot > 1.0 {
            *knot = 1.0;
        }
        previous = *knot;
    }
    if let Some(last) = next.last_mut() {
        *last = 1.0;
    }
    next
}

/// One Newton step on the cubic, kept only when it lands closer than the hint. Otherwise the
/// parameter comes from subdividing the cubic and taking the closer half, [`FIT_SUBDIVISIONS`] deep.
fn nearest_t(curve: CubicBez, point: Point, hint: f64) -> f64 {
    let hint = if hint.is_finite() { hint.clamp(0.0, 1.0) } else { 0.5 };
    let newton = newton_t(curve, point, hint);
    let subdivided = refine_t(curve, point, 0.0, 1.0, FIT_SUBDIVISIONS);
    let newton_d = (curve.eval(newton) - point).hypot2();
    let sub_d = (curve.eval(subdivided) - point).hypot2();
    if newton_d.is_finite() && newton_d <= sub_d { newton } else { subdivided }
}

fn newton_t(curve: CubicBez, point: Point, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let first = curve.deriv();
    let d1p = first.eval(t);
    let d2p = first.deriv().eval(t);
    let d1 = Vec2::new(d1p.x, d1p.y);
    let d2 = Vec2::new(d2p.x, d2p.y);
    let diff = curve.eval(t) - point;
    let denominator = d1.dot(d1) + diff.dot(d2);
    if !denominator.is_finite() || denominator.abs() < 1e-12 {
        return t;
    }
    let next = t - diff.dot(d1) / denominator;
    if next.is_finite() { next.clamp(0.0, 1.0) } else { t }
}

fn refine_t(curve: CubicBez, point: Point, t0: f64, t1: f64, depth: u32) -> f64 {
    if depth == 0 {
        let mid = 0.5 * (t0 + t1);
        let d0 = (curve.eval(0.0) - point).hypot2();
        let dm = (curve.eval(0.5) - point).hypot2();
        let d1 = (curve.eval(1.0) - point).hypot2();
        return if dm <= d0 && dm <= d1 {
            mid
        } else if d0 <= d1 {
            t0
        } else {
            t1
        };
    }
    let (left, right) = curve.subdivide();
    let mid = 0.5 * (t0 + t1);
    let left_d = (left.eval(0.5) - point).hypot2();
    let right_d = (right.eval(0.5) - point).hypot2();
    if left_d <= right_d { refine_t(left, point, t0, mid, depth - 1) } else { refine_t(right, point, mid, t1, depth - 1) }
}

fn center_tangent(points: &[Point], index: usize) -> Vec2 {
    let prev = points.get(index.saturating_sub(1)).copied().unwrap_or(Point::ZERO);
    let next = points.get(index.saturating_add(1)).copied().unwrap_or(prev);
    let at = points.get(index).copied().unwrap_or(prev);
    let across = next - prev;
    if across.hypot() > 1e-12 { unit_tangent(across) } else { unit_tangent(next - at) }
}

fn heuristic_cubic(p0: Point, p3: Point, t_hat1: Vec2, t_hat2: Vec2) -> CubicBez {
    let chord = (p3 - p0).hypot();
    let alpha = if chord.is_finite() { chord / 3.0 } else { 0.0 };
    CubicBez::new(p0, p0 + t_hat1 * alpha, p3 + t_hat2 * alpha, p3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;
    use std::collections::HashSet;

    fn bitmap_from(width: u32, height: u32, ink_at: impl Fn(u32, u32) -> bool) -> Bitmap {
        let mut ink = vec![0_u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                if ink_at(x, y) {
                    let index = (y * width + x) as usize;
                    if let Some(flag) = ink.get_mut(index) {
                        *flag = 1;
                    }
                }
            }
        }
        Bitmap::from_ink(width, height, ink).unwrap()
    }

    fn rect(width: u32, height: u32) -> Bitmap {
        bitmap_from(width, height, |_, _| true)
    }

    /// Nonzero fill of `path` into a bitmap of the same size. Edges are half-open in y so a
    /// vertex shared by two segments counts once. This is the oracle the fidelity tests share,
    /// and it is checked on a hand-built square before any traced path is trusted.
    fn rasterize(width: u32, height: u32, path: &BezPath) -> Bitmap {
        let mut flat = Vec::new();
        kurbo::flatten(path.elements().iter().copied(), 0.15, |element| flat.push(element));
        let mut edges: Vec<(f64, f64, f64, f64)> = Vec::new();
        let mut start: Option<Point> = None;
        let mut cursor: Option<Point> = None;
        for element in flat {
            match element {
                PathEl::MoveTo(p) => {
                    start = Some(p);
                    cursor = Some(p);
                }
                PathEl::LineTo(p) => {
                    if let Some(from) = cursor {
                        edges.push((from.x, from.y, p.x, p.y));
                    }
                    cursor = Some(p);
                }
                PathEl::ClosePath => {
                    if let (Some(from), Some(to)) = (cursor, start) {
                        let gap = (from.x - to.x).abs() > 1e-12 || (from.y - to.y).abs() > 1e-12;
                        if gap {
                            edges.push((from.x, from.y, to.x, to.y));
                        }
                    }
                    cursor = start;
                }
                PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                    cursor = Some(p);
                }
            }
        }
        let mut ink = vec![0_u8; (width * height) as usize];
        for y in 0..height {
            let yc = f64::from(y) + 0.5;
            let mut hits: Vec<(f64, i32)> = Vec::new();
            for (x0, y0, x1, y1) in &edges {
                let crosses = (*y0 <= yc && yc < *y1) || (*y1 <= yc && yc < *y0);
                let dy = y1 - y0;
                if !crosses || dy.abs() < 1e-15 {
                    continue;
                }
                let x = x0 + (yc - y0) / dy * (x1 - x0);
                if x.is_finite() {
                    hits.push((x, if *y1 > *y0 { 1 } else { -1 }));
                }
            }
            hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0_i32;
            let mut hit = 0_usize;
            for x in 0..width {
                let xc = f64::from(x) + 0.5;
                while hit < hits.len() && hits.get(hit).is_some_and(|(hx, _)| *hx <= xc) {
                    winding += hits.get(hit).map(|(_, w)| *w).unwrap_or(0);
                    hit += 1;
                }
                if winding != 0 {
                    let index = (y * width + x) as usize;
                    if let Some(flag) = ink.get_mut(index) {
                        *flag = 1;
                    }
                }
            }
        }
        Bitmap::from_ink(width, height, ink).unwrap()
    }

    fn paint(outline: &GlyphOutline, width: u32, height: u32) -> Bitmap {
        let mut path = BezPath::new();
        for contour in outline.contours() {
            path.extend(contour.path().elements().iter().copied());
        }
        rasterize(width, height, &path)
    }

    fn ink_set(bitmap: &Bitmap) -> HashSet<(u32, u32)> {
        let mut ink = HashSet::new();
        for y in 0..bitmap.height() {
            for x in 0..bitmap.width() {
                if bitmap.is_ink(i64::from(x), i64::from(y)) {
                    ink.insert((x, y));
                }
            }
        }
        ink
    }

    fn assert_exact(source: &Bitmap, outline: &GlyphOutline) {
        let got = ink_set(&paint(outline, source.width(), source.height()));
        let want = ink_set(source);
        let missing: Vec<_> = want.difference(&got).collect();
        let extra: Vec<_> = got.difference(&want).collect();
        assert!(missing.is_empty() && extra.is_empty(), "missing {missing:?}, extra {extra:?}");
    }

    fn curve_count(path: &BezPath) -> usize {
        path.elements().iter().filter(|el| matches!(el, PathEl::CurveTo(_, _, _))).count()
    }

    fn line_count(path: &BezPath) -> usize {
        path.elements().iter().filter(|el| matches!(el, PathEl::LineTo(_))).count()
    }

    fn shoelace(path: &BezPath) -> f64 {
        let mut points = Vec::new();
        kurbo::flatten(path.elements().iter().copied(), 0.2, |el| match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push((p.x, p.y)),
            _ => {}
        });
        signed_area(&points)
    }

    fn paper_four(bitmap: &Bitmap, x: u32, y: u32) -> bool {
        let (x, y) = (i64::from(x), i64::from(y));
        [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().any(|(dx, dy)| !bitmap.is_ink(x + dx, y + dy))
    }

    #[test]
    fn the_rasterizer_fills_a_hand_built_square_and_punches_a_hole() {
        let mut square = BezPath::new();
        square.move_to((1.0, 1.0));
        square.line_to((4.0, 1.0));
        square.line_to((4.0, 3.0));
        square.line_to((1.0, 3.0));
        square.close_path();
        let filled = rasterize(6, 5, &square);
        for y in 0..5 {
            for x in 0..6 {
                let want = (1..4).contains(&x) && (1..3).contains(&y);
                assert_eq!(filled.is_ink(i64::from(x), i64::from(y)), want, "pixel ({x},{y})");
            }
        }
        let mut ring = square.clone();
        ring.move_to((2.0, 1.0));
        ring.line_to((2.0, 2.0));
        ring.line_to((3.0, 2.0));
        ring.line_to((3.0, 1.0));
        ring.close_path();
        let punched = rasterize(6, 5, &ring);
        assert!(!punched.is_ink(2, 1), "the hole pixel stays paper");
        assert!(punched.is_ink(1, 1) && punched.is_ink(3, 1), "the ink around the hole stays");
    }

    #[test]
    fn blank_and_empty_bitmaps_have_no_contours() {
        let blank = bitmap_from(8, 5, |_, _| false);
        let outline = vectorize(&blank, &VectorizeParams::default());
        assert!(outline.contours().is_empty());
        assert!(!outline.truncated());
        let empty = Bitmap::from_ink(0, 0, Vec::new()).unwrap();
        assert!(vectorize(&empty, &VectorizeParams::default()).contours().is_empty());
    }

    #[test]
    fn one_pixel_is_a_unit_square() {
        let bitmap = bitmap_from(3, 3, |x, y| x == 1 && y == 1);
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(outline.contours().len(), 1);
        let only = &outline.contours()[0];
        assert_eq!(only.kind(), ContourKind::Outer);
        assert_eq!(only.parent(), None);
        assert!(shoelace(only.path()) > 0.0);
        assert_eq!(curve_count(only.path()), 0);
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn a_solid_rectangle_is_one_clockwise_outer_of_four_lines() {
        let bitmap = rect(9, 4);
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert!(!outline.truncated());
        assert_eq!(outline.contours().len(), 1);
        let only = &outline.contours()[0];
        assert_eq!(only.kind(), ContourKind::Outer);
        assert_eq!(only.parent(), None);
        assert!(shoelace(only.path()) > 0.0, "outers are clockwise in y-down, shoelace {}", shoelace(only.path()));
        assert_eq!(curve_count(only.path()), 0);
        assert_eq!(line_count(only.path()), 4);
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn a_frame_has_one_hole_of_opposite_winding() {
        // 10×8 ink with a 4×2 hole inset by 3.
        let bitmap = bitmap_from(10, 8, |x, y| !((3..7).contains(&x) && (3..5).contains(&y)));
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(outline.contours().len(), 2, "kinds {:?}", outline.contours().iter().map(|c| c.kind()).collect::<Vec<_>>());
        assert_eq!(outline.contours()[0].kind(), ContourKind::Outer);
        assert_eq!(outline.contours()[0].parent(), None);
        assert_eq!(outline.contours()[1].kind(), ContourKind::Hole);
        assert_eq!(outline.contours()[1].parent(), Some(0));
        assert!(shoelace(outline.contours()[0].path()) > 0.0);
        assert!(shoelace(outline.contours()[1].path()) < 0.0, "holes wind against the outer");
        assert_eq!(curve_count(outline.contours()[0].path()) + curve_count(outline.contours()[1].path()), 0);
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn two_blobs_are_two_top_level_outers() {
        let bitmap = bitmap_from(14, 6, |x, y| ((1..5).contains(&x) && (1..5).contains(&y)) || ((9..13).contains(&x) && (1..4).contains(&y)));
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(outline.contours().len(), 2);
        assert!(outline.contours().iter().all(|c| c.kind() == ContourKind::Outer && c.parent().is_none()));
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn an_island_in_a_hole_is_parented_to_the_hole() {
        // 11×9 frame, hole inset by 2, 3×3 island in the hole.
        let bitmap = bitmap_from(11, 9, |x, y| {
            let frame = x < 2 || y < 2 || x >= 9 || y >= 7;
            let island = (4..7).contains(&x) && (3..6).contains(&y);
            frame || island
        });
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        let kinds: Vec<_> = outline.contours().iter().map(|c| c.kind()).collect();
        assert_eq!(kinds, vec![ContourKind::Outer, ContourKind::Hole, ContourKind::Outer], "{kinds:?}");
        assert_eq!(outline.contours()[0].parent(), None);
        assert_eq!(outline.contours()[1].parent(), Some(0));
        assert_eq!(outline.contours()[2].parent(), Some(1));
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn a_concave_c_is_one_outer_and_rasterizes_exactly() {
        // A block C: six columns, the bite taken out of the right of the middle rows.
        let bitmap = bitmap_from(8, 7, |x, y| x < 6 && y < 7 && !(x >= 3 && (2..5).contains(&y)));
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(outline.contours().len(), 1);
        assert_eq!(outline.contours()[0].kind(), ContourKind::Outer);
        assert_eq!(curve_count(outline.contours()[0].path()), 0);
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn a_diagonal_bar_and_a_chevron_rasterize_exactly() {
        // Two pixels wide, so the bar is one 4-connected piece and its outline is a staircase
        // rather than a stack of separate squares.
        let bar = bitmap_from(8, 8, |x, y| {
            let delta = i64::from(y) - i64::from(x);
            (0..6).contains(&x) && (0..6).contains(&y) && (delta == 0 || delta == 1)
        });
        let bar_outline = vectorize(&bar, &VectorizeParams::default());
        assert_eq!(bar_outline.contours().len(), 1);
        assert_exact(&bar, &bar_outline);
        // A two-pixel chevron. The apex is a real corner between two staircases.
        let chevron = bitmap_from(11, 7, |x, y| {
            if y > 5 {
                return false;
            }
            let t = 5 - y;
            if t > 4 {
                return false;
            }
            let left = 5 - t;
            let right = 5 + t;
            x == left || x + 1 == left || x == right || x == right + 1
        });
        let chevron_outline = vectorize(&chevron, &VectorizeParams::default());
        assert_eq!(chevron_outline.contours().len(), 1);
        assert_exact(&chevron, &chevron_outline);
    }

    #[test]
    fn diagonal_touches_stay_one_component() {
        let bitmap = bitmap_from(4, 4, |x, y| (x, y) == (1, 1) || (x, y) == (2, 2));
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(outline.contours().len(), 1, "8-connected diagonals are one border");
        assert_eq!(outline.contours()[0].kind(), ContourKind::Outer);
        assert_exact(&bitmap, &outline);
    }

    #[test]
    fn traced_borders_are_exactly_the_pixels_with_a_paper_side() {
        for (name, bitmap) in [
            ("rect", rect(6, 5)),
            ("frame", bitmap_from(9, 7, |x, y| !((2..7).contains(&x) && (2..5).contains(&y)))),
            ("diagonal", bitmap_from(4, 4, |x, y| (x, y) == (1, 1) || (x, y) == (2, 2))),
        ] {
            let mut want = HashSet::new();
            for y in 0..bitmap.height() {
                for x in 0..bitmap.width() {
                    if bitmap.is_ink(i64::from(x), i64::from(y)) && paper_four(&bitmap, x, y) {
                        want.insert((x, y));
                    }
                }
            }
            // Every pixel the scan renumbered away from 1 is a border pixel; interior ink stays 1.
            let mut grid = working_grid(&bitmap).unwrap();
            let (borders, truncated) = suzuki_abe(&mut grid, 4_096);
            assert!(!truncated, "{name}");
            let mut got = HashSet::new();
            for y in 0..grid.height {
                for x in 0..grid.width {
                    let value = grid.get(x, y);
                    if value != 0 && value != 1 {
                        got.insert((u32::try_from(x).unwrap_or(0), u32::try_from(y).unwrap_or(0)));
                    }
                }
            }
            assert_eq!(got, want, "{name}: border pixels differ");
            assert!(!borders.is_empty(), "{name}");
        }
    }

    #[test]
    fn a_disk_stays_within_a_pixel_of_its_raster() {
        let (width, height) = (36_u32, 36_u32);
        let bitmap = bitmap_from(width, height, |x, y| {
            let dx = f64::from(x) + 0.5 - 18.0;
            let dy = f64::from(y) + 0.5 - 18.0;
            dx * dx + dy * dy <= 14.0 * 14.0
        });
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert!(!outline.truncated());
        assert!(outline.contours().iter().any(|c| curve_count(c.path()) > 0), "a disk is not a rectangle");
        let painted = paint(&outline, width, height);
        let (want, got) = (ink_set(&bitmap), ink_set(&painted));
        let inter = want.intersection(&got).count() as f64;
        let union = want.union(&got).count() as f64;
        let iou = inter / union;
        assert!(iou > 0.95, "disk IoU {iou}");
        // Two pixels in from the boundary the classification is exact: the fit's half-pixel
        // budget is spent on the rim, not the interior.
        for y in 0..height {
            for x in 0..width {
                let deep = (-2..=2).all(|dy| {
                    (-2..=2).all(|dx| {
                        let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
                        bitmap.is_ink(nx, ny) == bitmap.is_ink(i64::from(x), i64::from(y))
                    })
                });
                if deep {
                    assert_eq!(painted.is_ink(i64::from(x), i64::from(y)), bitmap.is_ink(i64::from(x), i64::from(y)), "deep pixel ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn schneider_reproduces_a_cubic_it_was_handed() {
        // The fitter is constrained to the tangents it is given. The cubic's own derivatives
        // are those tangents; the chord to the next sample is not, and a fit locked to that
        // chord cannot land on the samples.
        let original = CubicBez::new(Point::new(0.0, 0.0), Point::new(0.0, 30.0), Point::new(40.0, 30.0), Point::new(40.0, 0.0));
        let samples: Vec<Point> = (0..=16).map(|i| original.eval(f64::from(i) / 16.0)).collect();
        let start = original.deriv().eval(0.0);
        let end = original.deriv().eval(1.0);
        let t_hat1 = unit_tangent(Vec2::new(start.x, start.y));
        // Schneider's end tangent points back into the span, opposite the derivative.
        let t_hat2 = unit_tangent(Vec2::new(-end.x, -end.y));
        let mut fitted = Vec::new();
        fit_cubic(&samples, t_hat1, t_hat2, 0.05, 8, &mut fitted);
        assert_eq!(fitted.len(), 1, "a cubic's own samples are one span, got {}", fitted.len());
        let mut worst = 0.0_f64;
        for sample in &samples {
            let t = nearest_t(fitted[0], *sample, 0.5);
            worst = worst.max((fitted[0].eval(t) - *sample).hypot());
        }
        assert!(worst <= 0.05, "worst deviation {worst}");
    }

    #[test]
    fn a_second_piece_the_walk_cannot_close_is_truncated() {
        // One pixel touching a 2×2 block only at a corner. Four crack steps close the pixel
        // and do not get around the block, so the block's pixels must not vanish quietly.
        let bitmap = bitmap_from(5, 5, |x, y| (x, y) == (1, 1) || ((2..4).contains(&x) && (2..4).contains(&y)));
        let params = VectorizeParams { max_points: 4, ..VectorizeParams::default() };
        let outline = vectorize(&bitmap, &params);
        assert!(outline.truncated(), "the block's crack did not close");
    }

    #[test]
    fn douglas_peucker_drops_a_shallow_wiggle_and_keeps_a_corner() {
        let line = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.1), (3.0, 0.0), (4.0, 0.0)];
        let loose = douglas_peucker_open(&line, 0.5);
        assert_eq!(loose, vec![(0.0, 0.0), (4.0, 0.0)]);
        let tight = douglas_peucker_open(&line, 0.05);
        assert!(tight.contains(&(2.0, 0.1)));
        let corner = [(0.0, 0.0), (5.0, 0.0), (5.0, 5.0)];
        assert_eq!(douglas_peucker_open(&corner, 0.5), corner);
    }

    #[test]
    fn corners_break_a_right_angle_and_not_a_shallow_bowl() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)];
        assert_eq!(corner_indices(&square, std::f64::consts::FRAC_PI_3), vec![0, 1, 2, 3]);
        let mut bowl = Vec::new();
        for i in 0..16 {
            let t = f64::from(i) / 16.0 * std::f64::consts::TAU;
            bowl.push((10.0 * t.cos(), 10.0 * t.sin()));
        }
        assert!(corner_indices(&bowl, std::f64::consts::FRAC_PI_3).is_empty());
    }

    #[test]
    fn vectorize_is_deterministic() {
        let bitmap = bitmap_from(12, 10, |x, y| {
            let frame = x < 2 || y < 2 || x >= 10 || y >= 8;
            let island = (5..8).contains(&x) && (4..7).contains(&y);
            frame || island
        });
        let once = vectorize(&bitmap, &VectorizeParams::default());
        let twice = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(once, twice);
    }

    #[test]
    fn the_contour_and_point_budgets_truncate_instead_of_running_away() {
        let bitmap = bitmap_from(16, 8, |x, y| ((1..6).contains(&x) && (1..6).contains(&y)) || ((10..15).contains(&x) && (1..6).contains(&y)));
        let capped = vectorize(&bitmap, &VectorizeParams { max_contours: 1, ..VectorizeParams::default() });
        assert!(capped.truncated());
        assert!(capped.contours().len() <= 1);
        let short = vectorize(&rect(20, 10), &VectorizeParams { max_points: 4, ..VectorizeParams::default() });
        assert!(short.truncated());
        assert!(short.contours().is_empty());
    }

    #[test]
    fn non_finite_parameters_fall_back_and_a_checkerboard_finishes() {
        let bitmap = rect(6, 3);
        let params = VectorizeParams {
            simplify_epsilon: f64::NAN,
            max_error: f64::INFINITY,
            corner_angle: f64::NEG_INFINITY,
            max_fit_depth: 10_000,
            ..VectorizeParams::default()
        };
        let outline = vectorize(&bitmap, &params);
        assert_eq!(outline.contours().len(), 1);
        assert_exact(&bitmap, &outline);
        let board = bitmap_from(6, 6, |x, y| (x + y) % 2 == 0);
        let traced = vectorize(&board, &VectorizeParams::default());
        assert!(traced.contours().len() <= usize::try_from(VectorizeParams::default().max_contours).unwrap_or(usize::MAX));
    }

    #[test]
    fn a_full_bleed_bitmap_is_one_outer_contour() {
        let bitmap = rect(5, 5);
        let outline = vectorize(&bitmap, &VectorizeParams::default());
        assert_eq!(outline.contours().len(), 1);
        assert_exact(&bitmap, &outline);
    }
}
