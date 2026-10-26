//! Read a page's ruling lines, filled rectangles and filled shapes: the table borders, cell
//! backgrounds, bullet dots, rating lights and chart wedges that text extraction cannot see (Export
//! to Word and HTML). Read-only. Upright rectangles and horizontal or vertical strokes are
//! fills and rules, which is what tables, rules and shaded bands are drawn with; filled
//! polygons and any path with a curve (circles, pie slices, rounded boxes) are shapes.
//! Diagonal lines on their own, gradients and patterns are not read.

use kurbo::{PathEl, Point};
use pdfcraft_content::{Matrix, Op, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::EditError;
use crate::images::{ImageState, intersection, joined, page_of, streams};

/// The most filled rectangles, and the most rules, read from one page. A hostile page that draws
/// millions of them reads as far as this and stops.
pub const MAX_VECTORS: usize = 8192;

/// Hairlines drawn as a stroke of zero width are given this thickness (points).
const HAIRLINE: f64 = 0.25;
/// A filled rectangle at most this thick, and at least [`MIN_RULE`] long, is a line.
const THIN: f64 = 2.0;
const MIN_RULE: f64 = 4.0;

/// A filled rectangle in user space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageFill {
    /// `[x0, y0, x1, y1]`.
    pub rect: [f64; 4],
    /// sRGB components in 0..=1.
    pub color: [f64; 3],
}

/// A horizontal or vertical line in user space: `rect` has no extent across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageRule {
    /// `[x0, y0, x1, y1]`: `y0 == y1` for a horizontal line, `x0 == x1` for a vertical one.
    pub rect: [f64; 4],
    /// Thickness in points.
    pub width: f64,
    pub color: [f64; 3],
}

/// One segment of a [`PageShape`] outline, in user space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathSeg {
    Move([f64; 2]),
    Line([f64; 2]),
    /// A cubic Bezier: two control points, then the end point.
    Curve([f64; 2], [f64; 2], [f64; 2]),
    Close,
}

/// A filled and/or stroked outline that is not a rectangle or a straight line: a dot, a pie slice,
/// a triangle, a rounded box.
#[derive(Clone, Debug, PartialEq)]
pub struct PageShape {
    /// `[x0, y0, x1, y1]`: the box of every point of `path`, control points included.
    pub rect: [f64; 4],
    pub path: Vec<PathSeg>,
    /// sRGB components in 0..=1, when the shape is filled with a plain colour.
    pub fill: Option<[f64; 3]>,
    /// Line colour and thickness in points, when the outline is drawn.
    pub stroke: Option<([f64; 3], f64)>,
}

/// What a page draws with fills and strokes, in drawing order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageVectors {
    pub fills: Vec<PageFill>,
    pub rules: Vec<PageRule>,
    pub shapes: Vec<PageShape>,
}

/// Most segments one shape may have; a longer outline (a traced map) is not read.
const MAX_SHAPE_SEGMENTS: usize = 2048;

#[derive(Clone, Copy, PartialEq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Separation, Indexed, Lab, patterns…: no colour is read.
    Other,
}

#[derive(Clone)]
struct Paint {
    fill: Option<[f64; 3]>,
    stroke: Option<[f64; 3]>,
    fill_space: Space,
    stroke_space: Space,
    width: f64,
}

impl Paint {
    fn new() -> Self {
        Paint { fill: Some([0.0; 3]), stroke: Some([0.0; 3]), fill_space: Space::Gray, stroke_space: Space::Gray, width: 1.0 }
    }
}

fn gray(v: f64) -> [f64; 3] {
    [v; 3]
}

fn cmyk([c, m, y, k]: [f64; 4]) -> [f64; 3] {
    let channel = |v: f64| (1.0 - v.clamp(0.0, 1.0)) * (1.0 - k.clamp(0.0, 1.0));
    [channel(c), channel(m), channel(y)]
}

/// The colour a `sc`/`scn` operand list names in `space`; `None` for patterns, spot colours and
/// anything whose operands do not fit the space.
fn components(space: Space, op: &Op) -> Option<[f64; 3]> {
    if op.operands.last().is_some_and(|o| o.as_name().is_some()) {
        return None;
    }
    let n: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
    if n.len() != op.operands.len() || n.iter().any(|v| !v.is_finite()) {
        return None;
    }
    match (space, n.as_slice()) {
        (Space::Gray, [g]) => Some(gray(g.clamp(0.0, 1.0))),
        (Space::Rgb, [r, g, b]) => Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]),
        (Space::Cmyk, [c, m, y, k]) => Some(cmyk([*c, *m, *y, *k])),
        _ => None,
    }
}

/// The colour space a `cs`/`CS` operand names.
fn space_of(doc: &Document, resources: &Dict, name: &[u8]) -> Space {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => return Space::Gray,
        b"DeviceRGB" | b"RGB" | b"CalRGB" => return Space::Rgb,
        b"DeviceCMYK" | b"CMYK" => return Space::Cmyk,
        b"Pattern" => return Space::Other,
        _ => {}
    }
    let spaces = resources.get(b"ColorSpace").map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()).unwrap_or_default();
    let Some(entry) = spaces.get(name).map(|e| doc.resolve(e)) else { return Space::Other };
    let Some(array) = entry.as_array() else { return Space::Other };
    match array.first().map(|f| doc.resolve(f)).as_deref().and_then(Object::as_name) {
        Some(b"CalGray") => Space::Gray,
        Some(b"CalRGB") => Space::Rgb,
        Some(b"ICCBased") => {
            let profile = array.get(1).map(|p| doc.resolve(p));
            match profile.as_deref() {
                Some(Object::Stream(s)) => match s.dict.int(b"N") {
                    Some(1) => Space::Gray,
                    Some(3) => Space::Rgb,
                    Some(4) => Space::Cmyk,
                    _ => Space::Other,
                },
                _ => Space::Other,
            }
        }
        _ => Space::Other,
    }
}

/// The initial colour of a space after `cs`/`CS` (black for the device spaces).
fn initial(space: Space) -> Option<[f64; 3]> {
    (space != Space::Other).then_some([0.0; 3])
}

struct Sub {
    pts: Vec<Point>,
    segs: Vec<PathSeg>,
    closed: bool,
    curved: bool,
}

fn xy(p: Point) -> [f64; 2] {
    [p.x, p.y]
}

fn subpaths(path: &kurbo::BezPath) -> Vec<Sub> {
    let mut out = Vec::new();
    let mut cur: Option<Sub> = None;
    // The pen's position, which a quadratic needs to become a cubic.
    let mut pen = Point::ZERO;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => {
                out.extend(cur.take());
                cur = Some(Sub { pts: vec![*p], segs: vec![PathSeg::Move(xy(*p))], closed: false, curved: false });
                pen = *p;
            }
            PathEl::LineTo(p) => {
                if let Some(s) = cur.as_mut() {
                    s.pts.push(*p);
                    s.segs.push(PathSeg::Line(xy(*p)));
                }
                pen = *p;
            }
            PathEl::ClosePath => {
                if let Some(s) = cur.as_mut() {
                    s.closed = true;
                    s.segs.push(PathSeg::Close);
                    if let Some(first) = s.pts.first() {
                        pen = *first;
                    }
                }
            }
            PathEl::QuadTo(c, p) => {
                if let Some(s) = cur.as_mut() {
                    s.curved = true;
                    let third = 2.0 / 3.0;
                    let (c1, c2) = (pen + (*c - pen) * third, *p + (*c - *p) * third);
                    s.segs.push(PathSeg::Curve(xy(c1), xy(c2), xy(*p)));
                }
                pen = *p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(s) = cur.as_mut() {
                    s.curved = true;
                    s.segs.push(PathSeg::Curve(xy(*c1), xy(*c2), xy(*p)));
                }
                pen = *p;
            }
        }
    }
    out.extend(cur);
    out
}

/// The box of every point of `segs`, control points included; `None` when a coordinate is not finite.
fn seg_bounds(segs: &[PathSeg]) -> Option<[f64; 4]> {
    let mut r = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut add = |p: &[f64; 2]| {
        r = [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])];
    };
    for s in segs {
        match s {
            PathSeg::Move(p) | PathSeg::Line(p) => add(p),
            PathSeg::Curve(a, b, c) => [a, b, c].into_iter().for_each(&mut add),
            PathSeg::Close => {}
        }
    }
    r.iter().all(|v| v.is_finite()).then_some(r)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-3
}

/// The box of a subpath that is an upright rectangle.
fn upright_rect(pts: &[Point]) -> Option<[f64; 4]> {
    let mut p: Vec<Point> = pts.to_vec();
    if p.len() == 5 && p.first().zip(p.last()).is_some_and(|(a, b)| near(a.x, b.x) && near(a.y, b.y)) {
        p.pop();
    }
    let [a, b, c, d] = p.as_slice() else { return None };
    let horizontal_first = near(a.y, b.y) && near(b.x, c.x) && near(c.y, d.y) && near(d.x, a.x);
    let vertical_first = near(a.x, b.x) && near(b.y, c.y) && near(c.x, d.x) && near(d.y, a.y);
    if !(horizontal_first || vertical_first) {
        return None;
    }
    let xs = [a.x, b.x, c.x, d.x];
    let ys = [a.y, b.y, c.y, d.y];
    let r = [
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ];
    r.iter().all(|v| v.is_finite()).then_some(r)
}

fn is_white(c: [f64; 3]) -> bool {
    c.iter().all(|v| *v >= 0.98)
}

struct Walk<'a> {
    doc: &'a Document,
    forms: Vec<ObjRef>,
    budget: crate::text::FormBudget,
    out: PageVectors,
}

impl Walk<'_> {
    fn add_rule(&mut self, rect: [f64; 4], width: f64, color: [f64; 3], clip: [f64; 4]) {
        if self.out.rules.len() >= MAX_VECTORS || is_white(color) || !width.is_finite() {
            return;
        }
        // Clip the segment to the visible box; a segment wholly outside it is not seen.
        let (x0, x1) = (rect[0].max(clip[0]), rect[2].min(clip[2]));
        let (y0, y1) = (rect[1].max(clip[1]), rect[3].min(clip[3]));
        if !(x0 <= x1 && y0 <= y1) || (x1 - x0).max(y1 - y0) < 1.0 {
            return;
        }
        self.out.rules.push(PageRule { rect: [x0, y0, x1, y1], width: width.clamp(HAIRLINE, 72.0), color });
    }

    fn add_fill(&mut self, rect: [f64; 4], color: [f64; 3], clip: [f64; 4]) {
        let Some(r) = intersection(rect, clip) else { return };
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if w.min(h) <= THIN && w.max(h) >= MIN_RULE {
            // A filled sliver is how many producers draw a rule.
            let thickness = w.min(h);
            let rule = if w >= h {
                let y = (r[1] + r[3]) / 2.0;
                [r[0], y, r[2], y]
            } else {
                let x = (r[0] + r[2]) / 2.0;
                [x, r[1], x, r[3]]
            };
            self.add_rule(rule, thickness, color, clip);
        } else if w >= 1.0 && h >= 1.0 && self.out.fills.len() < MAX_VECTORS {
            self.out.fills.push(PageFill { rect: r, color });
        }
    }

    /// `sub`, when it is a filled polygon (not a rectangle or a line) or has a curve, as a shape.
    fn add_shape(&mut self, sub: &Sub, paint: &Paint, (fill, stroke): (bool, bool), width: f64, clip: [f64; 4]) {
        let fill = paint.fill.filter(|_| fill && (sub.curved || (sub.pts.len() >= 3 && upright_rect(&sub.pts).is_none())));
        let stroke = paint.stroke.filter(|_| stroke && sub.curved).map(|c| (c, width.clamp(HAIRLINE, 72.0)));
        if (fill.is_none() && stroke.is_none()) || self.out.shapes.len() >= MAX_VECTORS || sub.segs.len() > MAX_SHAPE_SEGMENTS {
            return;
        }
        let Some(rect) = seg_bounds(&sub.segs) else { return };
        if (rect[2] - rect[0]).max(rect[3] - rect[1]) < 0.5 || intersection(rect, clip).is_none() {
            return;
        }
        let mut path = sub.segs.clone();
        if fill.is_some() && !sub.closed {
            path.push(PathSeg::Close);
        }
        self.out.shapes.push(PageShape { rect, path, fill, stroke });
    }

    /// The path `st` has built, painted by `op`.
    fn paint(&mut self, st: &ImageState, paint: &Paint, op: &[u8]) {
        let fill = matches!(op, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
        let stroke = matches!(op, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
        let close = matches!(op, b"s" | b"b" | b"b*");
        let clip = st.placement.clip;
        let m = st.placement.matrix.0;
        let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        let width = if scale.is_finite() { paint.width.max(0.0) * scale } else { 0.0 };
        for sub in subpaths(&st.path) {
            self.add_shape(&sub, paint, (fill, stroke), width, clip);
            if sub.curved {
                continue;
            }
            if fill
                && let Some(color) = paint.fill
                && let Some(r) = upright_rect(&sub.pts)
            {
                self.add_fill(r, color, clip);
            }
            let Some(color) = paint.stroke.filter(|_| stroke) else { continue };
            let closing = (sub.closed || close).then(|| sub.pts.first().copied().zip(sub.pts.last().copied())).flatten();
            let segments = sub.pts.windows(2).map(|w| (w[0], w[1])).chain(closing.map(|(first, last)| (last, first)));
            for (a, b) in segments {
                let thickness = width.max(HAIRLINE);
                if near(a.y, b.y) {
                    self.add_rule([a.x.min(b.x), a.y, a.x.max(b.x), a.y], thickness, color, clip);
                } else if near(a.x, b.x) {
                    self.add_rule([a.x, a.y.min(b.y), a.x, a.y.max(b.y)], thickness, color, clip);
                }
            }
        }
    }
}

fn form_bounds(doc: &Document, resources: &Dict, name: &[u8], ctm: Matrix) -> Option<[f64; 4]> {
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())?;
    let form = doc.resolve(xobjects.get(name)?);
    let bbox = doc.resolve(form.as_dict().or_else(|| if let Object::Stream(s) = &*form { Some(&s.dict) } else { None })?.get(b"BBox")?);
    let [x0, y0, x1, y1] = bbox.as_array()?.as_slice() else { return None };
    Some(ctm.bbox([x0.as_f64()?, y0.as_f64()?, x1.as_f64()?, y1.as_f64()?]))
}

fn walk(w: &mut Walk, ops: &[Op], resources: &Dict, st: &mut ImageState, paint: &mut Paint) -> Result<(), EditError> {
    let mut saved: Vec<Paint> = Vec::new();
    for op in ops {
        match op.op.as_slice() {
            b"q" => saved.push(paint.clone()),
            b"Q" => {
                if let Some(p) = saved.pop() {
                    *paint = p;
                }
            }
            b"w" => {
                if let Some([v]) = op.nums::<1>() {
                    paint.width = if v.is_finite() { v } else { paint.width };
                }
            }
            b"g" => {
                paint.fill_space = Space::Gray;
                paint.fill = op.nums::<1>().map(|[v]| gray(v.clamp(0.0, 1.0)));
            }
            b"G" => {
                paint.stroke_space = Space::Gray;
                paint.stroke = op.nums::<1>().map(|[v]| gray(v.clamp(0.0, 1.0)));
            }
            b"rg" => {
                paint.fill_space = Space::Rgb;
                paint.fill = op.nums::<3>().map(|c| c.map(|v| v.clamp(0.0, 1.0)));
            }
            b"RG" => {
                paint.stroke_space = Space::Rgb;
                paint.stroke = op.nums::<3>().map(|c| c.map(|v| v.clamp(0.0, 1.0)));
            }
            b"k" => {
                paint.fill_space = Space::Cmyk;
                paint.fill = op.nums::<4>().map(cmyk);
            }
            b"K" => {
                paint.stroke_space = Space::Cmyk;
                paint.stroke = op.nums::<4>().map(cmyk);
            }
            b"cs" => {
                paint.fill_space = op.name(0).map_or(Space::Other, |n| space_of(w.doc, resources, n));
                paint.fill = initial(paint.fill_space);
            }
            b"CS" => {
                paint.stroke_space = op.name(0).map_or(Space::Other, |n| space_of(w.doc, resources, n));
                paint.stroke = initial(paint.stroke_space);
            }
            b"sc" | b"scn" => paint.fill = components(paint.fill_space, op),
            b"SC" | b"SCN" => paint.stroke = components(paint.stroke_space, op),
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => w.paint(st, paint, &op.op),
            _ => {}
        }
        st.apply(op)?;
        if !op.is("Do") {
            continue;
        }
        let Some(name) = op.name(0) else { continue };
        let ctm = st.placement.matrix;
        let Some(form) = crate::text::form_call(w.doc, resources, name, ctm, &w.forms, &mut w.budget) else { continue };
        if let Some(r) = form.obj {
            w.forms.push(r);
        }
        let bounds = form_bounds(w.doc, resources, name, form.ctm);
        let clip = bounds.map_or(st.placement.clip, |b| intersection(st.placement.clip, b).unwrap_or([0.0; 4]));
        let mut inner = ImageState::new(form.ctm, clip, st.placement.clips.clone());
        let mut inner_paint = paint.clone();
        walk(w, &parse(&form.data).ops, &form.resources, &mut inner, &mut inner_paint)?;
        if form.obj.is_some() {
            w.forms.pop();
        }
    }
    Ok(())
}

/// The filled rectangles and ruling lines page `page` (0-based) draws, in drawing order,
/// including those drawn inside form XObjects. Colours are sRGB. White fills are kept (a white box
/// can cover a shaded one); white lines are not.
pub fn reading_vectors(doc: &Document, page: usize) -> Result<PageVectors, EditError> {
    let p = page_of(doc, page)?;
    let resources = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut st = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let mut paint = Paint::new();
    let mut w = Walk { doc, forms: Vec::new(), budget: crate::text::FormBudget::default(), out: PageVectors::default() };
    let pieces = joined(&streams(doc, &p.dict));
    walk(&mut w, &pieces.parse().ops, &resources, &mut st, &mut paint)?;
    Ok(w.out)
}
