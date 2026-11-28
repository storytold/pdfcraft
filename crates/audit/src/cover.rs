//! Walk page content streams and collect opaque path and image covers.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510). A single `m`/`l` segment along an axis, stroked (`S`) wide enough to blot the text
//! out ("marker" style), counts as the rectangle the stroke paints (#2385). Graphics state tracked
//! here is deliberately small: the CTM stack (`q`/`Q`/`cm`), the colors (`g`/`rg`/`k` and
//! `G`/`RG`/`K`), the line width (`w`) and what ExtGState sets of them (`gs` reading `/ca`,
//! `/CA` and `/LW`). Form XObjects are followed read-only with a depth cap. Anything fancier
//! (other filled paths, slanted or bent strokes, shadings, patterns) is out of scope and
//! documented as such.
//! Image-specific state and sample geometry live in `image`.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use std::sync::Arc;

use pdfcraft_content::{Matrix, Op, Pieces, ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};

use crate::{AuditError, image};

/// Maximum form-XObject nesting followed. Deeper nesting (including a cycle) fails the audit: a
/// cover drawn below the cut could otherwise go unseen and the page be reported clean.
const MAX_DEPTH: usize = 32;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
/// Hitting it fails the audit: a truncated walk must never report "clean".
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle (or a stroke that paints one), in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// The paint color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
    /// Paint position: `(content-stream index, operator index in the joined stream)`.
    /// Rectangles inside a form XObject carry the `Do` operator's position.
    pub pos: (usize, usize),
    pub image: Option<image::Coverage>,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    alpha: f64,
    stroke: [f64; 3],
    stroke_alpha: f64,
    /// Line width, in the space the path is drawn in.
    width: f64,
    /// Line cap (`J`, `/LC`): 0 butt, 1 round, 2 projecting square.
    cap: i64,
    image: image::State,
}

/// The path under construction, as far as the audit cares about it.
enum Path {
    Empty,
    /// A `re` rectangle, in user space.
    Rect([f64; 4]),
    /// One `m`/`l` subpath, corners already mapped to user space; `true` once closed by `h`.
    Lines(Vec<[f64; 2]>, bool),
    /// Anything else: curves, a second subpath, a CTM change mid-path.
    Other,
}

impl Path {
    /// The rectangle a fill of this path paints, if the path is one.
    fn rect(self) -> Option<[f64; 4]> {
        match self {
            Path::Rect(r) => Some(r),
            Path::Lines(pts, _) => lines_rect(pts),
            Path::Empty | Path::Other => None,
        }
    }

    /// The rectangle a stroke of this path paints, if the path is one segment along an axis: the
    /// segment widened by half the line width each side, and lengthened by as much at each end
    /// under a round or projecting square cap (a round cap is counted by its box). Both are
    /// measured after the CTM: the pen is a disc of the line width in the space the path is
    /// drawn in, and its extent along a page direction is that direction's length under the
    /// transposed matrix, so a non-uniform scale widens only the strokes it stretches across.
    fn stroked_rect(&self, gs: &Gs) -> Option<[f64; 4]> {
        const EPS: f64 = 1e-6;
        let Path::Lines(pts, _) = self else { return None };
        let [a, b] = <[[f64; 2]; 2]>::try_from(pts.as_slice()).ok()?;
        let [m0, m1, m2, m3, ..] = gs.ctm.0;
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        let half = gs.width * (m3 * dx - m2 * dy).hypot(m0 * dy - m1 * dx) / len / 2.0;
        if half.is_nan() || half <= EPS {
            return None;
        }
        let cap = if gs.cap == 0 { 0.0 } else { gs.width * (m0 * dx + m1 * dy).hypot(m2 * dx + m3 * dy) / len / 2.0 };
        if cap.is_nan() {
            return None;
        }
        if (a[1] - b[1]).abs() <= EPS && (a[0] - b[0]).abs() > EPS {
            Some([a[0].min(b[0]) - cap, a[1] - half, a[0].max(b[0]) + cap, a[1] + half])
        } else if (a[0] - b[0]).abs() <= EPS && (a[1] - b[1]).abs() > EPS {
            Some([a[0] - half, a[1].min(b[1]) - cap, a[0] + half, a[1].max(b[1]) + cap])
        } else {
            None
        }
    }
}

/// The bounding box of a closed polygon that is an axis-aligned rectangle, or `None`. A repeated
/// start point is dropped first; the remaining four corners must alternate horizontal and
/// vertical edges.
fn lines_rect(mut pts: Vec<[f64; 2]>) -> Option<[f64; 4]> {
    // CTM products leave rounding noise, so corners compare with a tolerance (in points).
    const EPS: f64 = 1e-6;
    let same = |a: f64, b: f64| (a - b).abs() <= EPS;
    if pts.len() == 5 && same(pts[0][0], pts[4][0]) && same(pts[0][1], pts[4][1]) {
        pts.pop();
    }
    let [a, b, c, d] = <[[f64; 2]; 4]>::try_from(pts).ok()?;
    let fits = |first_horizontal: bool| {
        [(a, b), (b, c), (c, d), (d, a)]
            .iter()
            .enumerate()
            .all(|(i, (p, q))| if (i % 2 == 0) == first_horizontal { same(p[1], q[1]) } else { same(p[0], q[0]) })
    };
    if !(fits(true) || fits(false)) {
        return None;
    }
    let (xs, ys) = ([a[0], b[0], c[0], d[0]], [a[1], b[1], c[1], d[1]]);
    let r = [
        xs.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ];
    (r[2] - r[0] > EPS && r[3] - r[1] > EPS).then_some(r)
}

impl Default for Gs {
    /// Start with an identity transform, opaque black fill and stroke and no image-specific restrictions.
    fn default() -> Self {
        Gs {
            ctm: Matrix::IDENTITY,
            fill: [0.0, 0.0, 0.0],
            alpha: 1.0,
            stroke: [0.0, 0.0, 0.0],
            stroke_alpha: 1.0,
            width: 1.0,
            cap: 0,
            image: image::State::default(),
        }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
    image_budget: image::Budget,
    images_seen: bool,
    form_text: bool,
    /// Decoded bytes and form entries left for the page, its forms included: spending either
    /// fails the audit.
    budget: DecodeBudget,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    /// Walk one stream's operators, read one at a time (a stream of millions of short operators
    /// is never held whole), with the given graphics state. `at_do` is the `Do` position to
    /// stamp on rectangles found inside a form XObject, or `None` at page level (where each
    /// fill operator's own position is used).
    fn walk_ops(
        &mut self,
        ops: impl Iterator<Item = Op>,
        gs: &mut Gs,
        stack: &mut Vec<Gs>,
        resources: &Dict,
        depth: usize,
        at_do: Option<(usize, usize)>,
    ) -> Result<(), AuditError> {
        gs.image.resources(self.doc, resources);
        let mut path = Path::Empty;
        let mut image_path = image::ClipPath::default();
        let mut marked = Vec::new();
        let mut optional_depth = 0usize;
        for (i, op) in ops.enumerate() {
            self.bump()?;
            let op = &op;
            // Paint position of this operator: its own `(piece, joined index)` at page level,
            // the enclosing `Do`'s position inside a form XObject.
            let pos = match at_do {
                Some(p) => p,
                None => (self.pieces.pieces_of(op).0, i),
            };
            if depth > 0 && matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") {
                self.form_text = true;
            }
            gs.image.observe(op);
            image_path.observe(op, gs.ctm, &mut gs.image);
            match op.op.as_slice() {
                b"BDC" => {
                    let optional = op.name(0) == Some(b"OC");
                    marked.push(optional);
                    if optional {
                        optional_depth += 1;
                    }
                }
                b"BMC" => marked.push(false),
                b"EMC" => {
                    if marked.pop() == Some(true) {
                        optional_depth = optional_depth.saturating_sub(1);
                    }
                }
                b"BI" => {
                    let (dict, data) = op.inline.as_ref().ok_or_else(|| AuditError::Image("unreadable inline image".into()))?;
                    if optional_depth > 0 {
                        return Err(AuditError::Image("image optional content requires further analysis".into()));
                    }
                    self.image(&image::inline(dict, data), gs, pos)?;
                }
                _ => {}
            }
            match op.op.as_slice() {
                b"q" => {
                    if stack.len() >= 4096 {
                        return Err(AuditError::Image("graphics-state stack bound exceeded".into()));
                    }
                    stack.push(gs.clone());
                }
                b"Q" => {
                    // The current path is not graphics state (ISO 32000-2 §8.4.1): `re Q f` still
                    // fills the rectangle, whose corners are already in user space.
                    if let Some(g) = stack.pop() {
                        *gs = g;
                    }
                }
                b"cm" => {
                    let before = gs.ctm;
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        gs.ctm = m.then(&gs.ctm);
                    }
                    // A `cm` that leaves the CTM unchanged (an identity matrix) cannot move the
                    // pending path, so keep it for the fill (#1333).
                    if gs.ctm != before && !matches!(path, Path::Empty) {
                        path = Path::Other;
                    }
                }
                b"g" | b"G" => {
                    if let Some(v) = op.num(0).filter(|v| v.is_finite()) {
                        let v = v.clamp(0.0, 1.0);
                        *gs.color(op.op[0] == b'G') = [v, v, v];
                    }
                }
                b"rg" | b"RG" => {
                    if let Some([r, g, b]) = op.nums::<3>()
                        && [r, g, b].iter().all(|v| v.is_finite())
                    {
                        *gs.color(op.op[0] == b'R') = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                    }
                }
                b"k" | b"K" => {
                    if let Some([c, m, y, k]) = op.nums::<4>()
                        && [c, m, y, k].iter().all(|v| v.is_finite())
                    {
                        // Naive CMYK -> RGB; good enough to tell black from white.
                        let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                        *gs.color(op.op[0] == b'K') = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                    }
                }
                b"w" => {
                    if let Some(v) = op.num(0).filter(|v| v.is_finite() && *v >= 0.0) {
                        gs.width = v;
                    }
                }
                b"J" => {
                    if let Some(v) = op.num(0).filter(|v| [0.0, 1.0, 2.0].contains(v)) {
                        gs.cap = v as i64;
                    }
                }
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        ext_state(self.doc, resources, name, gs);
                        gs.image.ext(self.doc, resources, name);
                    }
                }
                b"re" => {
                    let rect = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            (r[2] > r[0] && r[3] > r[1]).then_some(r)
                        } else {
                            None
                        }
                    });
                    path = rect.map_or(Path::Other, Path::Rect);
                }
                b"m" => {
                    path = match (path, user_point(gs, op)) {
                        (Path::Empty, Some(p)) => Path::Lines(vec![p], false),
                        _ => Path::Other,
                    };
                }
                b"l" => {
                    path = match (path, user_point(gs, op)) {
                        // Past a repeated start point it can no longer be a rectangle.
                        (Path::Lines(mut pts, false), Some(p)) if pts.len() < 5 => {
                            pts.push(p);
                            Path::Lines(pts, false)
                        }
                        _ => Path::Other,
                    };
                }
                b"h" => {
                    path = match path {
                        Path::Lines(pts, _) => Path::Lines(pts, true),
                        // `re` already closed its subpath, so a further `h` changes nothing.
                        Path::Rect(r) => Path::Rect(r),
                        _ => Path::Other,
                    };
                }
                // `b`/`b*` close the path before filling it, which the fill does anyway.
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    let done = std::mem::replace(&mut path, Path::Empty);
                    if matches!(op.op.as_slice(), b"B" | b"B*" | b"b" | b"b*")
                        && let Some(r) = done.stroked_rect(gs)
                        && gs.stroke_alpha >= 1.0
                    {
                        self.out.push(FilledRect { rect: r, rgb: gs.stroke, pos, image: None });
                    }
                    if let Some(r) = done.rect()
                        && gs.alpha >= 1.0
                    {
                        self.out.push(FilledRect { rect: r, rgb: gs.fill, pos, image: None });
                    }
                }
                b"S" | b"s" => {
                    if let Some(r) = std::mem::replace(&mut path, Path::Empty).stroked_rect(gs)
                        && gs.stroke_alpha >= 1.0
                    {
                        self.out.push(FilledRect { rect: r, rgb: gs.stroke, pos, image: None });
                    }
                }
                b"n" => path = Path::Empty,
                b"c" | b"v" | b"y" | b"W" | b"W*" => path = Path::Other,
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        if depth >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        let mut sub = gs.clone();
                        if optional_depth > 0 {
                            sub.image.clip(None);
                        }
                        self.walk_xobject(name, &sub, resources, depth, pos)?;
                    }
                    path = Path::Empty;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Charge image analysis to the page budgets and retain its painted coverage at `pos`.
    /// Record even invisible images as encountered, so unaudited form text cannot pass cleanly.
    fn image(&mut self, stream: &pdfcraft_cos::Stream, gs: &Gs, pos: (usize, usize)) -> Result<(), AuditError> {
        self.images_seen = true;
        if let Some((rect, coverage)) = image::collect(self.doc, stream, &mut self.budget, &mut self.image_budget, &gs.image, gs.ctm, gs.fill)? {
            self.out.push(FilledRect { rect, rgb: gs.fill, pos, image: Some(coverage) });
        }
        Ok(())
    }

    /// Audit an image XObject or follow a form read-only, composing its matrix onto the CTM
    /// and intersecting image coverage with its BBox. Form state changes never leak out;
    /// every cover found is stamped with the `Do` position `pos`. Unreadable or unsupported
    /// image analysis propagates an error instead of silently skipping the object.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        // Borrowed, not copied: a `Do` must not cost the size of the whole XObject table.
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o));
        let Some(entry) = xobjects.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else {
            return Err(AuditError::Image("unresolved XObject".into()));
        };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Err(AuditError::Image("invalid XObject stream".into())) };
        let dict = &stream.dict;
        if crate::subtype(self.doc, dict)?.as_deref() == Some(b"Image") {
            if dict.get(b"OC").is_some() {
                return Err(AuditError::Image("image optional content requires further analysis".into()));
            }
            return self.image(stream, gs, pos);
        }
        if crate::subtype(self.doc, dict)?.as_deref() != Some(b"Form") {
            return Ok(());
        }
        let matrix = dict
            .get(b"Matrix")
            .map(|o| self.doc.resolve(o))
            .and_then(|o| o.as_array().map(|a| a.to_vec()))
            .and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|o| self.doc.resolve(o).as_f64()).collect();
                (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Matrix::IDENTITY);
        let (sub_resources, empty) = (dict.get(b"Resources").map(|o| self.doc.resolve(o)), Dict::new());
        let sub_resources = sub_resources.as_deref().and_then(Object::as_dict).unwrap_or(&empty);
        if !self.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = self.budget.decode_strict(self.doc, stream)?;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        if let Some(m) = dict.get(b"Matrix") {
            let m = self.doc.resolve(m);
            let valid = m.as_array().is_some_and(|a| a.len() == 6 && a.iter().all(|o| self.doc.resolve(o).as_f64().is_some_and(|n| n.is_finite())));
            if !valid {
                sub_gs.image.clip(None);
            }
        }
        let bbox = dict.get(b"BBox").map(|o| self.doc.resolve(o));
        let bbox = bbox.as_ref().and_then(|o| o.as_array()).and_then(|a| {
            let nums: Option<Vec<f64>> = a.iter().map(|o| self.doc.resolve(o).as_f64()).collect();
            let [x0, y0, x1, y1]: [f64; 4] = nums?.try_into().ok()?;
            Some(image::rectangle(sub_gs.ctm, [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]))
        });
        sub_gs.image.clip(bbox);
        if dict.get(b"Group").is_some() || dict.get(b"OC").is_some() {
            sub_gs.image.clip(None);
        }
        let mut sub_stack = Vec::new();
        let mut reader = ops(&data);
        self.walk_ops(&mut reader, &mut sub_gs, &mut sub_stack, sub_resources, depth + 1, Some(pos))?;
        if reader.skipped() > 0 || reader.ambiguous_images() > 0 {
            return Err(AuditError::Image("incomplete form content parse".into()));
        }
        Ok(())
    }
}

/// The `x y` operands of `m`/`l`, mapped to user space.
fn user_point(gs: &Gs, op: &Op) -> Option<[f64; 2]> {
    let [x, y] = op.nums::<2>().filter(|v| v.iter().all(|n| n.is_finite()))?;
    let (x, y) = gs.ctm.apply(x, y);
    Some([x, y])
}

impl Gs {
    /// The stroking or the non-stroking color.
    fn color(&mut self, stroking: bool) -> &mut [f64; 3] {
        if stroking { &mut self.stroke } else { &mut self.fill }
    }
}

/// Apply what a named ExtGState sets of the state tracked here: the non-stroking and stroking
/// alphas (`/ca`, `/CA`), the line width (`/LW`) and the line cap (`/LC`). A key it leaves out
/// keeps its value.
fn ext_state(doc: &Document, resources: &Dict, name: &[u8], gs: &mut Gs) {
    // Borrowed, not copied: a `gs` must not cost the size of the whole ExtGState table.
    let states = resources.get(b"ExtGState").map(|o| doc.resolve(o));
    let state = states.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| doc.resolve(o));
    let Some(state) = state.as_deref().and_then(Object::as_dict) else { return };
    let num = |key: &[u8]| state.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_f64()).filter(|v| v.is_finite());
    if let Some(a) = num(b"ca") {
        gs.alpha = a.clamp(0.0, 1.0);
    }
    if let Some(a) = num(b"CA") {
        gs.stroke_alpha = a.clamp(0.0, 1.0);
    }
    if let Some(w) = num(b"LW").filter(|w| *w >= 0.0) {
        gs.width = w;
    }
    if let Some(c) = num(b"LC").filter(|c| [0.0, 1.0, 2.0].contains(c)) {
        gs.cap = c as i64;
    }
}

/// Follow content references like `Document::resolve`, while propagating failed object reads.
fn resolve_content(doc: &Document, entry: &Object) -> Result<Arc<Object>, AuditError> {
    let mut object = Arc::new(entry.clone());
    // Match the COS reader's limit of one initial lookup and up to 32 further hops.
    for _ in 0..=32 {
        let Object::Ref(reference) = &*object else { return Ok(object) };
        if !doc.accepted_generations(reference.num).contains(&reference.generation) {
            return Ok(Arc::new(Object::Null));
        }
        object = doc.try_get(reference.num)?;
    }
    if matches!(&*object, Object::Ref(_)) {
        return Err(AuditError::Image("page content reference chain is too deep".into()));
    }
    Ok(object)
}

/// Every opaque path or image cover drawn by a page's content (and its form XObjects), with paint
/// positions. A page's content streams are walked once, joined, with one graphics state, so
/// state set in an earlier stream is still in force in the later ones. Any content stream that
/// cannot be decoded, or content and forms that decode or enter past one page's [`DecodeBudget`]
/// in all, is an audit failure, never an empty result. Image coverage also obeys the page box
/// and its sample/geometry bounds; images combined with unaudited form text fail closed.
pub fn filled_rects(doc: &Document, page: &Dict) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents = page.get(b"Contents").map(|entry| resolve_content(doc, entry)).transpose()?;
    let mut budget = DecodeBudget::page();
    let mut streams = Vec::new();
    match contents.as_deref() {
        None | Some(Object::Null) => {}
        Some(Object::Stream(stream)) => streams.push(budget.decode_strict(doc, stream)?),
        Some(Object::Array(entries)) => {
            streams.reserve(entries.len());
            for entry in entries {
                match &*resolve_content(doc, entry)? {
                    Object::Stream(stream) => streams.push(budget.decode_strict(doc, stream)?),
                    // Text extraction leaves an empty piece here. Keep the same piece indices.
                    Object::Null => streams.push(Vec::new()),
                    _ => return Err(AuditError::Image("unreadable page content entry".into())),
                }
            }
        }
        Some(_) => return Err(AuditError::Image("unreadable page content entry".into())),
    }
    let pieces = Pieces::join(&streams);
    let mut w = Walker {
        doc,
        pieces: &pieces,
        out: Vec::new(),
        ops_seen: 0,
        image_budget: image::Budget::default(),
        images_seen: false,
        form_text: false,
        budget,
    };
    let mut gs = Gs::default();
    if let Some(o) = page.get(b"CropBox").or_else(|| page.get(b"MediaBox")) {
        let o = doc.resolve(o);
        let bbox = o.as_array().and_then(|a| {
            let values: Option<Vec<f64>> = a.iter().map(|o| doc.resolve(o).as_f64()).collect();
            let [x0, y0, x1, y1]: [f64; 4] = values?.try_into().ok()?;
            Some(image::rectangle(Matrix::IDENTITY, [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]))
        });
        gs.image.clip(bbox);
    }
    let mut stack = Vec::new();
    let mut reader = ops(pieces.data());
    w.walk_ops(&mut reader, &mut gs, &mut stack, &resources, 0, None)?;
    if reader.skipped() > 0 || reader.ambiguous_images() > 0 {
        return Err(AuditError::Image("incomplete page content parse".into()));
    }
    if w.images_seen && w.form_text {
        return Err(AuditError::Image("text inside form XObjects is not fully audited".into()));
    }
    Ok(w.out)
}
