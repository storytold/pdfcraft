//! The vector artwork a page paints: the rules, boxes and fills of a form or a table, read so
//! Export ▸ Word can draw them again (Word export kept only text and pictures, so a form's boxes
//! and comb cells were lost; `docs/gaps.md` gap 8).
//!
//! Read-only and lenient. Paths are flattened to polylines in user space (curves in a few
//! straight steps), with the stroke and fill colour as plain sRGB (gray, RGB and CMYK converted
//! without colour management; ICC-based spaces by their number of components; a one-component
//! Separation or DeviceN tint as a gray of that much ink). Paint in a colour that cannot be read
//! (a pattern, an indexed or Lab colour) is left out rather than guessed. Clipping is applied as
//! a bounding box: a path wholly outside the clip is left out, one partly inside is kept whole.
//! Shadings (`sh`), images and text are not paths and are read elsewhere.

use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{EditError, Rgb};

/// Most painted paths one page reports; past it the rest are left out and the result says so.
pub const MAX_PAGE_SHAPES: usize = 20_000;
/// Most points all of a page's paths hold together (a hostile path of millions of segments).
const MAX_PAGE_POINTS: usize = 400_000;
/// Straight steps a Bézier curve is drawn in.
const CURVE_STEPS: usize = 8;
/// Deepest `q` nesting tracked; deeper saves are counted and their `Q` ignored.
const MAX_SAVES: usize = 256;
/// Coordinates past this are hostile, not geometry.
const MAX_COORD: f64 = 1e7;

/// How a path's outline is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub color: Rgb,
    /// Line width in user space (the CTM's scale applied); 0 is the thinnest line a device draws.
    pub width: f64,
    /// A dash pattern is set (`d` with a non-empty array).
    pub dashed: bool,
}

/// One subpath: its points in user space, and whether it is closed (back to its first point).
#[derive(Clone, Debug, PartialEq)]
pub struct Subpath {
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
}

/// A painted path.
#[derive(Clone, Debug, PartialEq)]
pub struct PageShape {
    pub subpaths: Vec<Subpath>,
    pub stroke: Option<Stroke>,
    pub fill: Option<Rgb>,
    /// Filled by the even-odd rule (`f*`, `B*`, `b*`).
    pub even_odd: bool,
    /// Bounding box of its points in user space.
    pub bounds: [f64; 4],
}

/// The paths a page paints, in painting order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingShapes {
    pub shapes: Vec<PageShape>,
    /// A limit (paths, points, form nesting or visits, decode budget) left part of the page unread.
    pub truncated: bool,
}

/// The colour space a colour is given in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// A one-component Separation or DeviceN: the tint is an amount of ink, shown as gray.
    Tint,
    /// A pattern, an indexed, Lab or many-ink colour: no plain colour can be read.
    Other,
}

impl Space {
    /// The colour an `SC`/`sc`/`SCN`/`scn` (or `G`, `RG`, `K` and their fill forms) sets in this
    /// space, components clamped to 0..=1; `None` when the space or the operand count has no
    /// plain colour.
    fn colour(self, op: &Op) -> Option<Rgb> {
        let n = op.operands.iter().filter(|o| o.as_f64().is_some()).count();
        let unit = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        match (self, n) {
            (Space::Gray, 1) => op.nums::<1>().map(|[g]| [unit(g); 3]),
            (Space::Rgb, 3) => op.nums::<3>().map(|c| c.map(unit)),
            (Space::Cmyk, 4) => op.nums::<4>().map(|c| cmyk(c.map(unit))),
            (Space::Tint, 1) => op.nums::<1>().map(|[t]| [1.0 - unit(t); 3]),
            _ => None,
        }
    }

    /// The colour a space starts with when it is selected (ISO 32000-2 §8.6.8 Table 72 and 8.6.6.4).
    fn initial(self) -> Option<Rgb> {
        match self {
            Space::Gray | Space::Rgb => Some([0.0; 3]),
            Space::Cmyk => Some(cmyk([0.0, 0.0, 0.0, 1.0])),
            Space::Tint => Some([0.0; 3]),
            Space::Other => None,
        }
    }
}

/// CMYK to RGB, as a plain conversion without colour management.
fn cmyk([c, m, y, k]: [f64; 4]) -> Rgb {
    [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
}

/// The space a `CS`/`cs` operand names: a device family, or a resource in `/ColorSpace`.
fn space_named(doc: &Document, resources: &Dict, name: &[u8]) -> Space {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => return Space::Gray,
        b"DeviceRGB" | b"RGB" | b"CalRGB" => return Space::Rgb,
        b"DeviceCMYK" | b"CMYK" => return Space::Cmyk,
        _ => {}
    }
    let Some(spaces) = resources.get(b"ColorSpace").and_then(|o| doc.dict(o)) else { return Space::Other };
    let Some(cs) = spaces.get(name).map(|o| doc.resolve(o)) else { return Space::Other };
    if let Some(n) = cs.as_name() {
        // A resource that names a device family.
        return match n {
            b"DeviceGray" => Space::Gray,
            b"DeviceRGB" => Space::Rgb,
            b"DeviceCMYK" => Space::Cmyk,
            _ => Space::Other,
        };
    }
    let Some(a) = cs.as_array() else { return Space::Other };
    match a.first().and_then(Object::as_name) {
        Some(b"CalGray") => Space::Gray,
        Some(b"CalRGB") => Space::Rgb,
        Some(b"ICCBased") => {
            let n = a.get(1).map(|s| doc.resolve(s)).and_then(|s| match &*s {
                Object::Stream(s) => s.dict.get(b"N").and_then(|n| doc.resolve(n).as_f64()),
                _ => None,
            });
            match n.map(|n| n.round() as i64) {
                Some(1) => Space::Gray,
                Some(3) => Space::Rgb,
                Some(4) => Space::Cmyk,
                _ => Space::Other,
            }
        }
        Some(b"Separation") => Space::Tint,
        Some(b"DeviceN") if a.get(1).map(|n| doc.resolve(n)).and_then(|n| n.as_array().map(Vec::len)) == Some(1) => Space::Tint,
        _ => Space::Other,
    }
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    stroke_space: Space,
    fill_space: Space,
    stroke: Option<Rgb>,
    fill: Option<Rgb>,
    width: f64,
    dashed: bool,
    /// Stroke and fill opacity (`CA`, `ca`): a paint at 0 shows nothing.
    stroke_alpha: f64,
    fill_alpha: f64,
    /// Bounding box of the clip in user space.
    clip: [f64; 4],
}

impl State {
    /// The initial graphics state (§8.4.1): black, 1 pt, solid, opaque, clipped to `clip`.
    fn new(ctm: Matrix, clip: [f64; 4]) -> Self {
        State {
            ctm,
            stroke_space: Space::Gray,
            fill_space: Space::Gray,
            stroke: Some([0.0; 3]),
            fill: Some([0.0; 3]),
            width: 1.0,
            dashed: false,
            stroke_alpha: 1.0,
            fill_alpha: 1.0,
            clip,
        }
    }

    /// The line width as it lands on the page: the CTM's mean scale (√|det|) times `w`.
    fn device_width(&self) -> f64 {
        let [a, b, c, d, _, _] = self.ctm.0;
        let w = self.width.abs() * (a * d - b * c).abs().sqrt();
        if w.is_finite() { w.min(MAX_COORD) } else { 0.0 }
    }

    /// The line width, opacities and dash pattern an ExtGState sets (`LW`, `CA`, `ca`, `D`).
    fn apply_gstate(&mut self, doc: &Document, g: &Dict) {
        let num = |k: &[u8]| g.get(k).and_then(|v| doc.resolve(v).as_f64()).filter(|v| v.is_finite());
        if let Some(w) = num(b"LW") {
            self.width = w;
        }
        if let Some(a) = num(b"CA") {
            self.stroke_alpha = a;
        }
        if let Some(a) = num(b"ca") {
            self.fill_alpha = a;
        }
        if let Some(d) = g.get(b"D").map(|d| doc.resolve(d)) {
            self.dashed =
                d.as_array().and_then(|a| a.first()).map(|a| doc.resolve(a)).and_then(|a| a.as_array().map(|a| !a.is_empty())).unwrap_or(false);
        }
    }
}

/// The path being built, in user space.
#[derive(Default)]
struct Path {
    subpaths: Vec<Subpath>,
    points: usize,
}

impl Path {
    /// The current point. After `h` it is the closed subpath's first point, and a segment drawn
    /// from there starts a new subpath (ISO 32000-2 §8.5.2.1).
    fn current(&mut self) -> Option<[f64; 2]> {
        let last = self.subpaths.last()?;
        if !last.closed {
            return last.points.last().copied();
        }
        let first = *last.points.first()?;
        self.start(first);
        Some(first)
    }

    /// A new subpath at `p` (`m`, `re`).
    fn start(&mut self, p: [f64; 2]) {
        self.subpaths.push(Subpath { points: vec![p], closed: false });
        self.points = self.points.saturating_add(1);
    }

    /// A straight segment to `p` on the last subpath.
    fn to(&mut self, p: [f64; 2]) {
        if let Some(s) = self.subpaths.last_mut() {
            s.points.push(p);
            self.points = self.points.saturating_add(1);
        }
    }

    /// Close the last subpath (`h`, and the closing paint operators).
    fn close(&mut self) {
        if let Some(s) = self.subpaths.last_mut() {
            s.closed = true;
        }
    }
}

struct Walk<'a> {
    doc: &'a Document,
    out: ReadingShapes,
    points: usize,
    forms: Vec<ObjRef>,
    budget: crate::text::FormBudget,
}

/// A point mapped to user space, or `None` for a hostile coordinate.
fn point(m: &Matrix, x: f64, y: f64) -> Option<[f64; 2]> {
    let (x, y) = m.apply(x, y);
    (x.is_finite() && y.is_finite() && x.abs() <= MAX_COORD && y.abs() <= MAX_COORD).then_some([x, y])
}

/// The bounding box of the subpaths' points, or `None` when they have none.
fn bounds(subpaths: &[Subpath]) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in subpaths.iter().flat_map(|s| &s.points) {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// Whether two boxes meet (touching edges count; an empty or inverted box meets nothing).
fn touches(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

impl Walk<'_> {
    /// Walk one content stream's operators with `resources`, from the state `st` (a form starts
    /// from the state at its `Do`), recording what it paints and entering the forms it draws.
    fn ops(&mut self, ops: &[Op], resources: &Dict, st: &mut State) {
        let doc = self.doc;
        let gstates = resources.get(b"ExtGState").and_then(|g| doc.dict(g)).unwrap_or_default();
        let mut saved: Vec<State> = Vec::new();
        let mut lost_saves = 0usize;
        let mut path = Path::default();
        let mut clip_pending = false;
        for op in ops {
            if self.out.truncated && (self.out.shapes.len() >= MAX_PAGE_SHAPES || self.points >= MAX_PAGE_POINTS) {
                return;
            }
            match op.op.as_slice() {
                b"q" => {
                    if saved.len() < MAX_SAVES {
                        saved.push(st.clone());
                    } else {
                        lost_saves = lost_saves.saturating_add(1);
                    }
                }
                b"Q" => {
                    if lost_saves > 0 {
                        lost_saves -= 1;
                    } else if let Some(s) = saved.pop() {
                        *st = s;
                    }
                }
                b"cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        let next = m.then(&st.ctm);
                        if next.0.iter().all(|v| v.is_finite()) {
                            st.ctm = next;
                        }
                    }
                }
                b"w" => {
                    if let Some([w]) = op.nums::<1>().filter(|[w]| w.is_finite()) {
                        st.width = w;
                    }
                }
                b"d" => st.dashed = op.operands.first().and_then(Object::as_array).is_some_and(|a| !a.is_empty()),
                b"G" => (st.stroke_space, st.stroke) = (Space::Gray, Space::Gray.colour(op)),
                b"g" => (st.fill_space, st.fill) = (Space::Gray, Space::Gray.colour(op)),
                b"RG" => (st.stroke_space, st.stroke) = (Space::Rgb, Space::Rgb.colour(op)),
                b"rg" => (st.fill_space, st.fill) = (Space::Rgb, Space::Rgb.colour(op)),
                b"K" => (st.stroke_space, st.stroke) = (Space::Cmyk, Space::Cmyk.colour(op)),
                b"k" => (st.fill_space, st.fill) = (Space::Cmyk, Space::Cmyk.colour(op)),
                b"CS" => {
                    st.stroke_space = op.last_name().map_or(Space::Other, |n| space_named(doc, resources, n));
                    st.stroke = st.stroke_space.initial();
                }
                b"cs" => {
                    st.fill_space = op.last_name().map_or(Space::Other, |n| space_named(doc, resources, n));
                    st.fill = st.fill_space.initial();
                }
                b"SC" | b"SCN" => st.stroke = st.stroke_space.colour(op),
                b"sc" | b"scn" => st.fill = st.fill_space.colour(op),
                b"gs" => {
                    if let Some(g) = op.last_name().and_then(|n| gstates.get(n)).and_then(|g| doc.dict(g)) {
                        st.apply_gstate(doc, &g);
                    }
                }
                b"m" => {
                    if let Some(p) = op.nums::<2>().and_then(|[x, y]| point(&st.ctm, x, y)) {
                        path.start(p);
                    }
                }
                b"l" => {
                    if path.current().is_some()
                        && let Some(p) = op.nums::<2>().and_then(|[x, y]| point(&st.ctm, x, y))
                    {
                        path.to(p);
                    }
                }
                b"c" | b"v" | b"y" => {
                    let Some(a) = path.current() else { continue };
                    let m = st.ctm;
                    let pts = match op.op.as_slice() {
                        b"c" => {
                            op.nums::<6>().and_then(|[x1, y1, x2, y2, x3, y3]| Some([point(&m, x1, y1)?, point(&m, x2, y2)?, point(&m, x3, y3)?]))
                        }
                        b"v" => op.nums::<4>().and_then(|[x2, y2, x3, y3]| Some([a, point(&m, x2, y2)?, point(&m, x3, y3)?])),
                        _ => op.nums::<4>().and_then(|[x1, y1, x3, y3]| {
                            let d = point(&m, x3, y3)?;
                            Some([point(&m, x1, y1)?, d, d])
                        }),
                    };
                    if let Some([b, c, d]) = pts {
                        for i in 1..=CURVE_STEPS {
                            let t = i as f64 / CURVE_STEPS as f64;
                            let u = 1.0 - t;
                            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                            path.to([w[0] * a[0] + w[1] * b[0] + w[2] * c[0] + w[3] * d[0], w[0] * a[1] + w[1] * b[1] + w[2] * c[1] + w[3] * d[1]]);
                        }
                    }
                }
                b"h" => path.close(),
                b"re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        let m = st.ctm;
                        if let (Some(a), Some(b), Some(c), Some(d)) =
                            (point(&m, x, y), point(&m, x + w, y), point(&m, x + w, y + h), point(&m, x, y + h))
                        {
                            path.start(a);
                            path.to(b);
                            path.to(c);
                            path.to(d);
                            path.close();
                        }
                    }
                }
                b"W" | b"W*" => clip_pending = true,
                b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    let paint = op.op.as_slice();
                    if matches!(paint, b"s" | b"b" | b"b*") {
                        path.close();
                    }
                    let subpaths = std::mem::take(&mut path).subpaths;
                    let found = bounds(&subpaths);
                    if paint != b"n"
                        && let Some(b) = found
                    {
                        self.paint(paint, subpaths, b, st);
                    }
                    // A new clip takes effect after the path is painted (§8.5.4).
                    if std::mem::take(&mut clip_pending)
                        && let Some(b) = found
                    {
                        // The clip is the intersection; an empty one clips everything away.
                        let c = st.clip;
                        st.clip = [c[0].max(b[0]), c[1].max(b[1]), c[2].min(b[2]), c[3].min(b[3])];
                    }
                }
                b"Do" => {
                    let Some(name) = op.last_name() else { continue };
                    let Some(form) = crate::text::form_call(doc, resources, name, st.ctm, &self.forms, &mut self.budget) else { continue };
                    if self.budget.truncated {
                        self.out.truncated = true;
                    }
                    let bbox = resources
                        .get(b"XObject")
                        .and_then(|x| doc.dict(x))
                        .and_then(|x| x.get(name).map(|o| doc.resolve(o)))
                        .and_then(|o| match &*o {
                            Object::Stream(s) => s.dict.get(b"BBox").map(|b| doc.resolve(b)),
                            _ => None,
                        })
                        .and_then(|b| {
                            let a = b.as_array()?;
                            let [x0, y0, x1, y1] = a.as_slice() else { return None };
                            Some(form.ctm.bbox([x0.as_f64()?, y0.as_f64()?, x1.as_f64()?, y1.as_f64()?]))
                        });
                    let mut inner = st.clone();
                    inner.ctm = form.ctm;
                    if let Some(b) = bbox.filter(|b| b.iter().all(|v| v.is_finite())) {
                        let c = inner.clip;
                        inner.clip = [c[0].max(b[0]), c[1].max(b[1]), c[2].min(b[2]), c[3].min(b[3])];
                    }
                    if let Some(r) = form.obj {
                        self.forms.push(r);
                    }
                    self.ops(&parse(&form.data).ops, &form.resources, &mut inner);
                    if form.obj.is_some() {
                        self.forms.pop();
                    }
                }
                _ => {}
            }
            if path.points > MAX_PAGE_POINTS {
                // One path alone past the page's budget: drop it and say so.
                path = Path::default();
                self.out.truncated = true;
            }
        }
        if self.budget.truncated {
            self.out.truncated = true;
        }
    }

    /// Record a path painted by `paint` (`S`, `f`, `B` …) in the state `st`, unless it paints
    /// nothing, lies outside the clip, or the page's budget is spent (which marks the result
    /// truncated).
    fn paint(&mut self, paint: &[u8], subpaths: Vec<Subpath>, bounds: [f64; 4], st: &State) {
        let strokes = matches!(paint, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
        let fills = matches!(paint, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
        let stroke =
            st.stroke.filter(|_| strokes && st.stroke_alpha > 0.0).map(|color| Stroke { color, width: st.device_width(), dashed: st.dashed });
        let fill = st.fill.filter(|_| fills && st.fill_alpha > 0.0);
        if stroke.is_none() && fill.is_none() {
            return;
        }
        // A stroke's half width can reach past its points into the clip.
        let pad = stroke.map_or(0.0, |s| s.width / 2.0);
        let padded = [bounds[0] - pad, bounds[1] - pad, bounds[2] + pad, bounds[3] + pad];
        if !touches(padded, st.clip) {
            return;
        }
        let n: usize = subpaths.iter().map(|s| s.points.len()).sum();
        if self.out.shapes.len() >= MAX_PAGE_SHAPES || self.points.saturating_add(n) > MAX_PAGE_POINTS {
            self.out.truncated = true;
            return;
        }
        self.points = self.points.saturating_add(n);
        self.out.shapes.push(PageShape { subpaths, stroke, fill, even_odd: matches!(paint, b"f*" | b"B*" | b"b*"), bounds });
    }
}

/// The paths page `page` (0-based) paints, in painting order, including those inside the form
/// XObjects it draws (Export ▸ Word). Coordinates are user space, before `/Rotate`.
pub fn reading_shapes(doc: &Document, page: usize) -> Result<ReadingShapes, EditError> {
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let resources = p.dict.get(b"Resources").and_then(|r| doc.dict(r)).unwrap_or_default();
    let mut decode = pdfcraft_cos::DecodeBudget::page();
    let streams = crate::text::content_streams_within(doc, &p.dict, &mut decode)?;
    // The page's streams are one stream in pieces: an operator may span two of them.
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let mut walk = Walk { doc, out: ReadingShapes::default(), points: 0, forms: Vec::new(), budget: crate::text::FormBudget::new(decode) };
    let mut st = State::new(Matrix::IDENTITY, p.crop(doc));
    walk.ops(&pieces.parse().ops, &resources, &mut st);
    Ok(walk.out)
}

#[cfg(test)]
#[path = "shapes_tests.rs"]
mod tests;
