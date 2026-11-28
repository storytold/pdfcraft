//! A positioned page's vector paths as model shapes behind its text: an axis-aligned box is a
//! rectangle, anything else a freeform of straight segments, and a compound fill (several
//! subpaths, or the even-odd rule) trapezoids that don't overlap, with its outline stroked
//! apart. Each shape counts against the page's [`ArtBudget`].

use craft_wordmodel::connector::ShapeExtra;
use craft_wordmodel::fill::Fill;
use craft_wordmodel::freeform::{FreePath, Freeform, MAX_PATHS, MAX_POINTS};
use craft_wordmodel::line_dash::LineDash;
use craft_wordmodel::para::{Anchor, Float, ShapeKind, Wrap};
use craft_wordmodel::{InlineObject, Rgb};

use super::{MAX_DOCX_PAGE_POINTS, MAX_DOCX_PAGE_SHAPES, fill, layout_number};
use crate::as_docx::rgb8;
use crate::layout::{Shape, Stroke};

pub(super) fn rgb(c: [f64; 3]) -> Rgb {
    let [r, g, b] = rgb8(c);
    Rgb(r, g, b)
}

pub(super) fn point_bounds<'a>(points: impl Iterator<Item = &'a [f64; 2]>) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in points {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// The corners of an axis-aligned rectangle a closed subpath traces (four points, or five with
/// the first repeated), as its box.
fn axis_rect(points: &[[f64; 2]]) -> Option<[f64; 4]> {
    let corners = match points {
        [a, b, c, d] => [*a, *b, *c, *d],
        [a, b, c, d, e] if (a[0] - e[0]).abs() < 0.01 && (a[1] - e[1]).abs() < 0.01 => [*a, *b, *c, *d],
        _ => return None,
    };
    let same = |u: f64, v: f64| (u - v).abs() < 0.01;
    if corners.iter().enumerate().any(|(i, p)| corners.iter().skip(i + 1).any(|q| same(p[0], q[0]) && same(p[1], q[1]))) {
        return None;
    }
    let sides = [(corners[0], corners[1]), (corners[1], corners[2]), (corners[2], corners[3]), (corners[3], corners[0])];
    if !sides.iter().all(|(p, q)| same(p[0], q[0]) || same(p[1], q[1])) {
        return None;
    }
    point_bounds(corners.iter())
}

/// What one Word page may still draw, and whether it had to leave something out.
pub(super) struct ArtBudget {
    pub shapes: usize,
    pub points: usize,
    fill_work: usize,
    pub dropped: bool,
    /// Only boundaries actually emitted within both limits can replace a widget's border.
    pub outlines: Vec<[f64; 4]>,
}

impl ArtBudget {
    pub fn new() -> Self {
        Self { shapes: MAX_DOCX_PAGE_SHAPES, points: MAX_DOCX_PAGE_POINTS, fill_work: 1_000_000, dropped: false, outlines: Vec::new() }
    }

    /// Room for one more shape of `points` points: `false`, and the page marked as having
    /// dropped something, when there is none.
    fn take(&mut self, points: usize) -> bool {
        self.take_all(1, points)
    }

    /// Room for `shapes` more shapes of `points` points together, all of them or none: `false`,
    /// and the page marked as having dropped something, when they don't all fit.
    fn take_all(&mut self, shapes: usize, points: usize) -> bool {
        if shapes > self.shapes || points > self.points {
            self.dropped = true;
            return false;
        }
        self.shapes -= shapes;
        self.points -= points;
        true
    }
}

/// A closed ellipse, covering its perimeter once. Matching bounds alone also accepts a
/// triangle, a rounded rectangle, or an arc closed by a chord, none of which replaces a circle.
fn round_outline(points: &[[f64; 2]], closed: bool) -> Option<[f64; 4]> {
    if !closed || points.len() < 8 {
        return None;
    }
    let bounds = point_bounds(points.iter())?;
    let width = bounds[2] - bounds[0];
    let height = bounds[3] - bounds[1];
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let unit = |p: &[f64; 2]| [(2.0 * p[0] - bounds[0] - bounds[2]) / width, (2.0 * p[1] - bounds[1] - bounds[3]) / height];
    let mut turn = 0.0;
    let mut distance = 0.0;
    for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
        let (a, b) = (unit(a), unit(b));
        if (a[0] * a[0] + a[1] * a[1] - 1.0).abs() > 0.12 {
            return None;
        }
        let angle = (a[0] * b[1] - a[1] * b[0]).atan2(a[0] * b[0] + a[1] * b[1]);
        if angle.abs() > std::f64::consts::FRAC_PI_2 {
            return None;
        }
        turn += angle;
        distance += angle.abs();
    }
    ((turn.abs() - std::f64::consts::TAU).abs() < 0.1 && distance < std::f64::consts::TAU + 0.1).then_some(bounds)
}

/// A subpath as drawn: an open path that ends where it started is closed (a box drawn as four
/// `l` segments back to its corner), and a filled one is closed as filling implies.
fn normalized(points: &[[f64; 2]], closed: bool, filled: bool) -> (Vec<[f64; 2]>, bool) {
    let same = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01;
    let mut points = points.to_vec();
    let mut closed = closed || (filled && points.len() > 2);
    if points.len() > 3 && points.first().zip(points.last()).is_some_and(|(a, b)| same(*a, *b)) {
        points.pop();
        closed = true;
    }
    (points, closed)
}

/// A drawing behind the text, its top left `x` and `y` points from the page's.
pub(super) fn behind_at(x: f64, y: f64) -> Float {
    Float {
        wrap: Wrap::BehindText,
        h_rel: Anchor::Page,
        v_rel: Anchor::Page,
        x: layout_number(x) as f32,
        y: layout_number(y) as f32,
        ..Float::default()
    }
}

/// A model shape of `kind` over `bounds` (y up) on a page `page_height` tall, painted as the
/// PDF paints it; a freeform's `paths` are in the frame of `bounds`, y down from its top.
pub(super) fn shape(
    kind: ShapeKind,
    bounds: [f64; 4],
    page_height: f64,
    fill: Option<[f64; 3]>,
    stroke: Option<Stroke>,
    paths: Vec<FreePath>,
) -> InlineObject {
    let (w, h) = (layout_number(bounds[2] - bounds[0]).max(0.01) as f32, layout_number(bounds[3] - bounds[1]).max(0.01) as f32);
    let mut extra = ShapeExtra::default();
    if stroke.is_some_and(|s| s.dashed) {
        extra.dash = LineDash::Dash;
    }
    // Word draws nothing for a 0 pt line; the PDF means the thinnest visible one.
    let stroke_width = stroke.map_or(0.0, |s| if s.width.is_finite() { s.width.clamp(0.25, 100.0) } else { 0.25 });
    InlineObject::Shape {
        kind,
        w,
        h,
        fill: fill.map(|c| Fill::Solid { color: rgb(c), transparency: 0.0 }),
        stroke: stroke.map(|s| rgb(s.color)),
        stroke_width: stroke_width as f32,
        float: behind_at(bounds[0], page_height - bounds[3]),
        story: None,
        freeform: (kind == ShapeKind::Freeform).then(|| std::sync::Arc::new(Freeform { w, h, paths, ..Freeform::default() })),
        effects: Default::default(),
        extra,
    }
}

/// `p` (y up) in the frame of `bounds`, y down from its top.
fn local(p: [f64; 2], bounds: [f64; 4]) -> [f32; 2] {
    [layout_number(p[0] - bounds[0]) as f32, layout_number(bounds[3] - p[1]) as f32]
}

/// One painted path as model shapes, appended to `out` in painting order. Each shape counts
/// against the page's `budget`; nothing is appended without points or paint, or when the budget
/// has no room.
pub(super) fn shape_objects(s: &Shape, page_height: f64, budget: &mut ArtBudget, out: &mut Vec<InlineObject>) {
    objects_with_fill(s, page_height, budget, s.fill, out);
}

fn objects_with_fill(s: &Shape, page_height: f64, budget: &mut ArtBudget, fill: Option<[f64; 3]>, out: &mut Vec<InlineObject>) {
    if s.stroke.is_none() && fill.is_none() {
        return;
    }
    if let Some(color) = fill
        && (s.paths.len() > 1 || s.even_odd)
    {
        compound(s, page_height, budget, color, out);
        if s.stroke.is_some() {
            objects_with_fill(s, page_height, budget, None, out);
        }
        return;
    }
    // PDF fills implicitly close an open subpath; its stroke must still stay open (B/B*).
    // Separate the two paints and charge both against the emitted-shape budget.
    if fill.is_some() && s.stroke.is_some() && s.paths.iter().any(|(_, closed)| !closed) {
        for (points, closed) in &s.paths {
            let mut path = Shape { even_odd: false, paths: vec![(points.clone(), *closed)], stroke: s.stroke, fill };
            if !closed {
                path.stroke = None;
                shape_objects(&path, page_height, budget, out);
                path.fill = None;
                path.stroke = s.stroke;
            }
            shape_objects(&path, page_height, budget, out);
        }
        return;
    }
    for (points, closed) in &s.paths {
        if points.len() < 2 || !points.iter().flatten().all(|n| n.is_finite()) {
            continue;
        }
        let (points, closed) = normalized(points, *closed, fill.is_some());
        // One path of the model holds so many points: an open outline goes on in the next
        // path, but a closed or filled one can't be cut.
        if points.len() > MAX_POINTS && closed {
            budget.dropped = true;
            continue;
        }
        if !budget.take(points.len()) {
            continue;
        }
        let Some(bounds) = point_bounds(points.iter()) else { continue };
        if s.stroke.is_some()
            && let Some(outline) = round_outline(&points, closed)
        {
            budget.outlines.push(outline);
        }
        match axis_rect(&points).filter(|_| closed) {
            Some(r) => out.push(shape(ShapeKind::Rectangle, r, page_height, fill, s.stroke, Vec::new())),
            None => {
                let mut paths = Vec::new();
                let mut start = 0;
                while start + 1 < points.len() && paths.len() < MAX_PATHS {
                    let end = (start + MAX_POINTS).min(points.len());
                    let pts = points.get(start..end).unwrap_or_default().iter().map(|p| local(*p, bounds)).collect();
                    paths.push(FreePath { pts, closed });
                    // The next piece starts at the point this one ends at.
                    start = end - 1;
                    if end == points.len() {
                        break;
                    }
                }
                out.push(shape(ShapeKind::Freeform, bounds, page_height, fill, s.stroke, paths));
            }
        }
    }
}

/// A compound fill as freeforms of trapezoids that don't overlap, so every reader fills the same
/// area whatever its own fill rule. Left out, and the page marked, past the bounded work or when
/// the freeforms it takes (one for every [`MAX_PATHS`] trapezoids) don't all fit the page.
fn compound(s: &Shape, page_height: f64, budget: &mut ArtBudget, color: [f64; 3], out: &mut Vec<InlineObject>) {
    let Some(polygons) = fill::trapezoids(s, &mut budget.fill_work, budget.points) else {
        budget.dropped = true;
        return;
    };
    let Some(bounds) = point_bounds(polygons.iter().flatten()) else { return };
    if !budget.take_all(polygons.len().div_ceil(MAX_PATHS), polygons.len().saturating_mul(4)) {
        return;
    }
    for chunk in polygons.chunks(MAX_PATHS) {
        let paths = chunk.iter().map(|polygon| FreePath { pts: polygon.iter().map(|p| local(*p, bounds)).collect(), closed: true }).collect();
        out.push(shape(ShapeKind::Freeform, bounds, page_height, Some(color), None, paths));
    }
}
