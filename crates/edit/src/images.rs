//! Edit a PDF ▸ edit existing images and grouped artwork (Image/Form XObjects painted by `Do`
//! in its own content streams), and changing one: move/resize/rotate/flip (the `Do` is wrapped in
//! `q … cm … Q` with the extra transform), replace (it draws another XObject in the same place),
//! or delete. Only the stream that draws it is rewritten, as a new object.

use kurbo::{BezPath, Point, Shape};
use pdfcraft_content::{Matrix, Op, Pieces, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use std::sync::Arc;

use crate::EditError;
use crate::text::content_streams;

/// The identity of a soft mask (`/SMask`) in the PDF graphics state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaskIdentity {
    /// Indirect reference to a mask dictionary or stream.
    Ref(u32, u16),
    /// Indirect reference to the transparency group (`/G`) in a mask dictionary.
    Group(u32, u16),
    /// Explicit named mask or resource name.
    Named(Vec<u8>),
}

impl std::fmt::Display for MaskIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaskIdentity::Ref(num, generation) => write!(f, "{num} {generation} R"),
            MaskIdentity::Group(num, generation) => write!(f, "group {num} {generation} R"),
            MaskIdentity::Named(name) => write!(f, "{}", String::from_utf8_lossy(name)),
        }
    }
}

/// An image drawn on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    /// Its box in user space.
    pub rect: [f64; 4],
    /// Visible frame after page and analytic path bounds. Hit testing also respects curve
    /// interiors, winding rules and holes, rather than intercepting their empty areas.
    pub visible_rect: Option<[f64; 4]>,
    /// Current page/path clip in user space. A private crop transforms with its image.
    pub clip_rect: Option<[f64; 4]>,
    /// The placement: image unit square or Form coordinates → user space.
    pub matrix: [f64; 6],
    /// The XObject resource name and object.
    pub name: String,
    pub object: Option<ObjRef>,
    /// Whether this is grouped Form artwork, edited as a whole (including its nested content).
    pub is_form: bool,
    /// Pixel size; zero for vector/grouped Form artwork.
    pub width: u32,
    pub height: u32,
    /// Active soft mask identity at the image placement, if any.
    pub smask: Option<MaskIdentity>,
    /// The content stream its `Do` (its first token) is in, and the `Do`'s index among the
    /// operators of the page's streams joined ([`Pieces`]).
    pub stream: usize,
    op: usize,
    /// CTM before `Do`, excluding a Form's own Matrix.
    draw_matrix: [f64; 6],
    clips: Vec<ClipPath>,
}

impl PageImage {
    /// Whether a user-space point belongs to the placed artwork and every active clip.
    pub fn contains(&self, point: [f64; 2]) -> bool {
        if !point.iter().all(|v| v.is_finite()) {
            return false;
        }
        let Some(bounds) = self.visible_rect else {
            return false;
        };
        if point[0] < bounds[0] || point[0] > bounds[2] || point[1] < bounds[1] || point[1] > bounds[3] {
            return false;
        }
        if !self.is_form {
            let Some(inverse) = Matrix(self.matrix).invert() else {
                return false;
            };
            let (x, y) = inverse.apply(point[0], point[1]);
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return false;
            }
        }
        self.clips.iter().all(|clip| {
            let winding = clip.path.winding(Point::new(point[0], point[1]));
            if clip.even_odd { winding % 2 != 0 } else { winding != 0 }
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ClipPath {
    path: Arc<BezPath>,
    even_odd: bool,
}

/// Image draws one page's inventory lists at most: a form drawn many times from other forms
/// would otherwise multiply a few hundred bytes into millions of records.
pub(crate) const MAX_PAGE_IMAGES: usize = 100_000;

fn too_many_images() -> EditError {
    EditError::Invalid(format!("the page draws more than {MAX_PAGE_IMAGES} images"))
}

/// The page's streams joined into the one stream they are: an operator may span two of them.
fn joined(streams: &[(Object, Vec<u8>)]) -> Pieces {
    Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>())
}

fn page_of(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn xobjects(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())))
        .unwrap_or_default()
}

fn intersection(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
}

#[derive(Clone)]
struct Placement {
    matrix: Matrix,
    clip: [f64; 4],
    clips: Vec<ClipPath>,
}

/// Path coordinates are transformed when constructed; q/Q save clips, not the current path.
struct ImageState {
    placement: Placement,
    stack: Vec<Placement>,
    path: BezPath,
    subpath_start: Option<Point>,
    pending_clip: Option<bool>,
}

impl ImageState {
    fn new(matrix: Matrix, clip: [f64; 4], clips: Vec<ClipPath>) -> Self {
        Self { placement: Placement { matrix, clip, clips }, stack: Vec::new(), path: BezPath::new(), subpath_start: None, pending_clip: None }
    }

    fn current_point(&self) -> Option<Point> {
        match self.path.elements().last() {
            Some(kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p)) => Some(*p),
            Some(kurbo::PathEl::QuadTo(_, p2) | kurbo::PathEl::CurveTo(_, _, p2)) => Some(*p2),
            Some(kurbo::PathEl::ClosePath) => self.subpath_start,
            None => None,
        }
    }

    fn has_current_point(&self) -> bool {
        self.current_point().is_some()
    }

    fn point(&self, x: f64, y: f64) -> Option<Point> {
        let (x, y) = self.placement.matrix.apply(x, y);
        if x.is_finite() && y.is_finite() { Some(Point::new(x, y)) } else { None }
    }

    fn apply(&mut self, op: &Op) -> Result<(), EditError> {
        if matches!(op.op.as_slice(), b"cm" | b"m" | b"l" | b"c" | b"v" | b"y" | b"re")
            && op.operands.iter().any(|v| v.as_f64().is_some_and(|v| !v.is_finite() || v.abs() > 1e12))
        {
            return Err(EditError::Invalid("the image path contains invalid coordinates".into()));
        }
        if self.path.elements().len() > 16384 || self.stack.len() > 4096 || self.placement.clips.len() > 256 {
            return Err(EditError::Invalid("the image clip or graphics state exceeds its complexity budget".into()));
        }
        match op.op.as_slice() {
            b"q" => self.stack.push(self.placement.clone()),
            b"Q" => {
                if let Some(saved) = self.stack.pop() {
                    self.placement = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    let next_mat = Matrix(m).then(&self.placement.matrix);
                    if next_mat.0.iter().all(|v| v.is_finite()) {
                        self.placement.matrix = next_mat;
                    }
                }
            }
            b"m" => {
                if let Some([x, y]) = op.nums::<2>()
                    && let Some(p) = self.point(x, y)
                {
                    self.subpath_start = Some(p);
                    self.path.move_to(p);
                }
            }
            b"l" => {
                // A segment before any m (or re) has no subpath to extend: BezPath's line_to and
                // friends panic on an empty path by contract, so the segment is skipped, as the
                // renderer, MuPDF and pdftotext do (#3581).
                if let Some([x, y]) = op.nums::<2>()
                    && self.has_current_point()
                    && let Some(p) = self.point(x, y)
                {
                    self.path.line_to(p);
                }
            }
            b"c" => {
                if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>()
                    && self.has_current_point()
                    && let (Some(p1), Some(p2), Some(p3)) = (self.point(x1, y1), self.point(x2, y2), self.point(x3, y3))
                {
                    self.path.curve_to(p1, p2, p3);
                }
            }
            b"v" => {
                if let Some([x1, y1, x2, y2]) = op.nums::<4>()
                    && self.has_current_point()
                    && let (Some(p1), Some(p2)) = (self.point(x1, y1), self.point(x2, y2))
                    && let Some(current) = self.current_point()
                {
                    self.path.curve_to(current, p1, p2);
                }
            }
            b"y" => {
                if let Some([x1, y1, x2, y2]) = op.nums::<4>()
                    && self.has_current_point()
                    && let (Some(p1), Some(end)) = (self.point(x1, y1), self.point(x2, y2))
                {
                    self.path.curve_to(p1, end, end);
                }
            }
            b"h" => {
                // ClosePath with no subpath panics by the same contract (#3581).
                if self.has_current_point() {
                    self.path.close_path();
                }
            }
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>()
                    && let (Some(p0), Some(p1), Some(p2), Some(p3)) =
                        (self.point(x, y), self.point(x + w, y), self.point(x + w, y + h), self.point(x, y + h))
                {
                    self.subpath_start = Some(p0);
                    self.path.move_to(p0);
                    self.path.line_to(p1);
                    self.path.line_to(p2);
                    self.path.line_to(p3);
                    self.path.close_path();
                }
            }
            b"W" => self.pending_clip = Some(false),
            b"W*" => self.pending_clip = Some(true),
            b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if let Some(even_odd) = self.pending_clip.take() {
                    // Filling/clipping implicitly closes open subpaths. BezPath's winding
                    // follows the same rule, and its bounds use cubic extrema, not controls.
                    let bounds = self.path.bounding_box();
                    self.placement.clip = intersection(self.placement.clip, [bounds.x0, bounds.y0, bounds.x1, bounds.y1]).unwrap_or([0.0; 4]);
                    self.placement.clips.push(ClipPath { path: Arc::new(std::mem::take(&mut self.path)), even_odd });
                }
                self.path = BezPath::new();
                self.subpath_start = None;
            }
            _ => {}
        }
        Ok(())
    }
}

/// The images and Form artwork page `page` (0-based) draws, in drawing order.
/// Forms stay intact: no traversal or rewriting of their internal objects is needed.
pub fn page_images(doc: &Document, page: usize) -> Result<Vec<PageImage>, EditError> {
    let p = page_of(doc, page)?;
    let xo = xobjects(doc, &p.dict);
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let ext_gstates = res.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    // The streams are one stream in pieces, so the graphics state and operators carry over.
    let mut state = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let mut tracker = ExtendedGStateTracker::default();
    let pieces = joined(&content_streams(doc, &p.dict)?);
    for (i, op) in pieces.parse().ops.iter().enumerate() {
        state.apply(op)?;
        tracker.step(op, &ext_gstates, doc, state.placement.matrix)?;
        let ctm = state.placement.matrix;
        if op.is("Do") {
            let Some(name) = op.name(0) else { continue };
            let Some(resource) = xo.get(name) else { continue };
            let obj = doc.resolve(resource);
            let Object::Stream(s) = &*obj else { continue };
            let is_form = s.dict.name(b"Subtype") == Some(b"Form");
            let (matrix, bounds) = if is_form {
                let Some(bounds) = s.dict.get(b"BBox").map(|v| doc.resolve(v)).and_then(|v| {
                    v.as_array().and_then(|a| {
                        if a.len() != 4 {
                            return None;
                        }
                        let mut b = [0.0; 4];
                        for (n, v) in b.iter_mut().zip(a) {
                            *n = v.as_f64()?;
                        }
                        Some(b)
                    })
                }) else {
                    continue;
                };
                let matrix = match s.dict.get(b"Matrix") {
                    Some(v) => {
                        let v = doc.resolve(v);
                        let Some(m) = v.as_array().and_then(|a| Matrix::from_operands(a)) else { continue };
                        m
                    }
                    None => Matrix::IDENTITY,
                };
                (matrix.then(&ctm), bounds)
            } else if s.dict.name(b"Subtype") == Some(b"Image") {
                (ctm, [0.0, 0.0, 1.0, 1.0])
            } else {
                continue;
            };

            let rect = matrix.bbox(bounds);
            if !bounds.iter().chain(matrix.0.iter()).chain(rect.iter()).all(|v| v.is_finite())
                || bounds[2] <= bounds[0]
                || bounds[3] <= bounds[1]
                || rect[2] <= rect[0]
                || rect[3] <= rect[1]
            {
                continue;
            }
            if out.len() >= MAX_PAGE_IMAGES {
                return Err(too_many_images());
            }
            out.push(PageImage {
                rect,
                visible_rect: intersection(rect, state.placement.clip),
                clip_rect: Some(state.placement.clip),
                matrix: matrix.0,
                name: String::from_utf8_lossy(name).into_owned(),
                object: resource.as_ref(),
                is_form,
                width: if is_form { 0 } else { s.dict.int(b"Width").unwrap_or(0).clamp(0, u32::MAX as i64) as u32 },
                height: if is_form { 0 } else { s.dict.int(b"Height").unwrap_or(0).clamp(0, u32::MAX as i64) as u32 },
                smask: tracker.current_state.smask.clone(),
                stream: pieces.piece_of(op.span.start),
                op: i,
                draw_matrix: ctm.0,
                clips: state.placement.clips.clone(),
            });
        }
    }
    Ok(out)
}

/// The images page `page` (0-based) shows, in drawing order, including those drawn inside form
/// XObjects (Export to Word, HTML and RTF). Read-only: indexes into this list are not
/// [`page_images`] indexes, so they can't be passed to [`change_image`].
pub fn reading_images(doc: &Document, page: usize) -> Result<Vec<PageImage>, EditError> {
    /// The CTM and `q` stack carry from one page stream to the next (§7.8.2); a form starts
    /// its own.
    /// The forms being read (cycle guard) and how many were entered (the page's budget).
    struct Forms {
        path: Vec<ObjRef>,
        budget: crate::text::FormBudget,
    }
    fn walk(
        doc: &Document,
        ops: &[Op],
        resources: &Dict,
        st: &mut ImageState,
        stream: &dyn Fn(&Op) -> usize,
        forms: &mut Forms,
        out: &mut Vec<PageImage>,
    ) -> Result<(), EditError> {
        let xo = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        for (i, op) in ops.iter().enumerate() {
            st.apply(op)?;
            let ctm = st.placement.matrix;
            if op.is("Do") {
                let Some(name) = op.name(0) else { continue };
                if let Some(form) = crate::text::form_call(doc, resources, name, ctm, &forms.path, &mut forms.budget) {
                    if let Some(r) = form.obj {
                        forms.path.push(r);
                    }
                    let bounds =
                        xo.get(name).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().and_then(|d| d.get(b"BBox")).map(|b| doc.resolve(b))).and_then(
                            |b| {
                                let a = b.as_array()?;
                                let [x0, y0, x1, y1] = a.as_slice() else { return None };
                                Some(form.ctm.bbox([x0.as_f64()?, y0.as_f64()?, x1.as_f64()?, y1.as_f64()?]))
                            },
                        );
                    let clip = bounds.map_or(st.placement.clip, |b| intersection(st.placement.clip, b).unwrap_or([0.0; 4]));
                    let mut inner = ImageState::new(form.ctm, clip, st.placement.clips.clone());
                    let piece = stream(op);
                    walk(doc, &parse(&form.data).ops, &form.resources, &mut inner, &|_| piece, forms, out)?;
                    if form.obj.is_some() {
                        forms.path.pop();
                    }
                    continue;
                }
                let Some(r) = xo.get(name).and_then(Object::as_ref) else { continue };
                let obj = doc.get(r);
                let Object::Stream(s) = &*obj else { continue };
                if s.dict.name(b"Subtype") != Some(b"Image") {
                    continue;
                }
                if out.len() >= MAX_PAGE_IMAGES {
                    return Err(too_many_images());
                }
                out.push(PageImage {
                    rect: ctm.bbox([0.0, 0.0, 1.0, 1.0]),
                    visible_rect: intersection(ctm.bbox([0.0, 0.0, 1.0, 1.0]), st.placement.clip),
                    clip_rect: Some(st.placement.clip),
                    matrix: ctm.0,
                    name: String::from_utf8_lossy(name).into_owned(),
                    object: Some(r),
                    is_form: false,
                    width: s.dict.int(b"Width").unwrap_or(0).max(0) as u32,
                    height: s.dict.int(b"Height").unwrap_or(0).max(0) as u32,
                    smask: None,
                    stream: stream(op),
                    op: i,
                    draw_matrix: ctm.0,
                    clips: st.placement.clips.clone(),
                });
            }
        }
        Ok(())
    }
    let p = page_of(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    let mut st = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let mut decode = pdfcraft_cos::DecodeBudget::page();
    let pieces = joined(&crate::text::content_streams_within(doc, &p.dict, &mut decode)?);
    let mut forms = Forms { path: Vec::new(), budget: crate::text::FormBudget::new(decode) };
    walk(doc, &pieces.parse().ops, &res, &mut st, &|op| pieces.piece_of(op.span.start), &mut forms, &mut out)?;
    Ok(out)
}

/// The four stacking order arrangements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangeOrder {
    BringToFront,
    SendToBack,
    BringForward,
    SendBackward,
}

/// What to do with an image.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageChange {
    /// Apply a user-space transform after its placement (move, resize, rotate, flip).
    Transform([f64; 6]),
    /// Show only this part of it (a box in user space): a clipping path around the drawing,
    /// inside its own scope so a later move or resize carries it along. The pixels stay where
    /// they are; nothing is scaled.
    Crop([f64; 4]),
    /// Draw this image XObject instead, in the same place.
    Replace(ObjRef),
    Delete,
    /// Change the stacking order.
    Arrange(ArrangeOrder),
    /// Remove any private crop on the image.
    RemoveCrop,
}

/// The user-space transform that maps box `from` onto box `to` (move and resize).
pub fn rect_to_rect(from: [f64; 4], to: [f64; 4]) -> [f64; 6] {
    let (fw, fh) = ((from[2] - from[0]).max(1e-6), (from[3] - from[1]).max(1e-6));
    let (sx, sy) = ((to[2] - to[0]) / fw, (to[3] - to[1]) / fh);
    [sx, 0.0, 0.0, sy, to[0] - from[0] * sx, to[1] - from[1] * sy]
}

/// Turn the image a quarter turn clockwise (`quarters` = 1, 2, 3) about its centre, or flip it.
pub fn turn_about_centre(rect: [f64; 4], quarters: i32, flip_h: bool, flip_v: bool) -> [f64; 6] {
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let (a, b, c, d) = match quarters.rem_euclid(4) {
        1 => (0.0, -1.0, 1.0, 0.0),
        2 => (-1.0, 0.0, 0.0, -1.0),
        3 => (0.0, 1.0, -1.0, 0.0),
        _ => (1.0, 0.0, 0.0, 1.0),
    };
    let (fx, fy) = (if flip_h { -1.0 } else { 1.0 }, if flip_v { -1.0 } else { 1.0 });
    let (a, b, c, d) = (a * fx, b * fy, c * fx, d * fy);
    // Translate the centre to the origin, turn, translate back.
    [a, b, c, d, cx - a * cx - c * cy, cy - b * cx - d * cy]
}

/// Turn the image `degrees` clockwise (as drawn, whatever the page's `/Rotate`) about the centre
/// of `rect`. `None` for a non-finite angle; whole turns of 360 are the identity.
pub fn turn_by_degrees(rect: [f64; 4], degrees: f64) -> Option<[f64; 6]> {
    if !degrees.is_finite() {
        return None;
    }
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let t = (degrees % 360.0).to_radians();
    let (s, c) = t.sin_cos();
    // Clockwise in a y-up space: (x, y) -> (x cos t + y sin t, -x sin t + y cos t).
    let (a, b, cc, d) = (c, -s, s, c);
    Some([a, b, cc, d, cx - a * cx - cc * cy, cy - b * cx - d * cy])
}

/// For a `Do` at `op` that sits alone in `q re W n cm Do Q`: the index of the `re` and the `cm` matrix.
/// That clip belongs to this image only, so it may move with it. A `cm` this move put before the
/// `re` earlier (`q cm re W n cm Do Q`) keeps the pattern, so the image can be moved again.
fn exclusive_clip(ops: &[Op], op: usize) -> Option<(usize, Matrix)> {
    let at = op.checked_sub(4)?;
    let mut q = at.checked_sub(1)?;
    while ops.get(q)?.is("cm") {
        q = q.checked_sub(1)?;
    }
    let ok = ops.get(q)?.is("q")
        && ops.get(at)?.is("re")
        && (ops.get(at + 1)?.is("W") || ops.get(at + 1)?.is("W*"))
        && ops.get(at + 2)?.is("n")
        && ops.get(at + 3)?.is("cm")
        && ops.get(op + 1)?.is("Q");
    if !ok {
        return None;
    }
    Some((at, Matrix(ops.get(at + 3)?.nums::<6>()?)))
}

/// The `q cm … Q` whose only content after its `cm` is `start..=end` (a `Do`, or a balanced
/// `q … Q` scope), with the index and value of that `cm`. Another transform of the same
/// placement is folded into it, so repeated moves and resizes don't nest one more `q` each.
fn enclosing_transform(ops: &[Op], start: usize, end: usize) -> Option<(usize, Matrix)> {
    let at = start.checked_sub(1)?;
    let cm = ops.get(at)?;
    if !ops.get(at.checked_sub(1)?)?.is("q") || !ops.get(end.checked_add(1)?)?.is("Q") || !cm.is("cm") || cm.operands.len() != 6 {
        return None;
    }
    let m = Matrix(cm.nums::<6>()?);
    m.0.iter().all(|v| v.is_finite()).then_some((at, m))
}

/// The `cm` to put before the clip so that user-space transform `t` also moves the clip:
/// the clip is drawn in the CTM before the image's own `cm` (`cm` · that = `draw`).
fn clip_matrix(draw: Matrix, cm: Matrix, t: Matrix) -> Option<Matrix> {
    let outer = cm.invert()?.then(&draw);
    Some(outer.then(&t).then(&outer.invert()?))
}

pub(crate) fn is_entire_page_wrapped(ops: &[Op]) -> bool {
    if ops.len() < 2 || !ops.first().is_some_and(|o| o.is("q")) || !ops.last().is_some_and(|o| o.is("Q")) {
        return false;
    }
    let mut depth: usize = 0;
    for (i, op) in ops.iter().enumerate() {
        if op.is("q") {
            depth = depth.saturating_add(1);
        } else if op.is("Q") {
            depth = depth.saturating_sub(1);
            if depth == 0 && i < ops.len().saturating_sub(1) {
                return false;
            }
        }
    }
    depth == 0
}

pub(crate) fn content_units(ops: &[Op], scan_start: usize, scan_end: usize) -> Vec<std::ops::Range<usize>> {
    let mut units = Vec::new();
    let mut i = scan_start;
    while i < scan_end {
        let op = match ops.get(i) {
            Some(o) => o,
            None => break,
        };
        if op.is("BDC") || op.is("BMC") {
            // A marked-content sequence is one unit up to its matching EMC (#3145), so Arrange moves
            // it whole and never inserts into another one: its content belongs to its tag (ISO 32000-2
            // § 14.6). An unbalanced sequence runs to the end of the scan.
            let mut depth: usize = 1;
            let mut j = i.saturating_add(1);
            while j < scan_end {
                if let Some(next) = ops.get(j) {
                    if next.is("BDC") || next.is("BMC") {
                        depth = depth.saturating_add(1);
                    } else if next.is("EMC") {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            j = j.saturating_add(1);
                            break;
                        }
                    }
                }
                j = j.saturating_add(1);
            }
            units.push(i..j);
            i = j;
        } else if op.is("q") {
            let mut depth: usize = 1;
            let mut j = i.saturating_add(1);
            while j < scan_end {
                if let Some(next) = ops.get(j) {
                    if next.is("q") {
                        depth = depth.saturating_add(1);
                    } else if next.is("Q") {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            j = j.saturating_add(1);
                            break;
                        }
                    }
                }
                j = j.saturating_add(1);
            }
            units.push(i..j);
            i = j;
        } else if op.is("BT") {
            let mut j = i.saturating_add(1);
            while j < scan_end {
                if ops.get(j).is_some_and(|o| o.is("ET")) {
                    j = j.saturating_add(1);
                    break;
                }
                j = j.saturating_add(1);
            }
            units.push(i..j);
            i = j;
        } else if op.is("BI") {
            let mut j = i.saturating_add(1);
            while j < scan_end {
                if ops.get(j).is_some_and(|o| o.is("EI")) {
                    j = j.saturating_add(1);
                    break;
                }
                j = j.saturating_add(1);
            }
            units.push(i..j);
            i = j;
        } else if op.is("Do") || op.is("sh") {
            units.push(i..i.saturating_add(1));
            i = i.saturating_add(1);
        } else {
            let mut j = i;
            let mut found_end = false;
            while j < scan_end {
                let next = match ops.get(j) {
                    Some(o) => o,
                    None => break,
                };
                if matches!(next.op.as_slice(), b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                    j = j.saturating_add(1);
                    found_end = true;
                    break;
                } else if j > i && matches!(next.op.as_slice(), b"q" | b"BT" | b"BI" | b"Do" | b"sh" | b"BDC" | b"BMC") {
                    found_end = true;
                    break;
                }
                j = j.saturating_add(1);
            }
            let u_end = if found_end { j } else { scan_end };
            units.push(i..u_end);
            i = u_end;
        }
    }
    units.retain(|u| {
        ops.get(u.clone()).is_some_and(|slice| {
            slice.iter().any(|op| {
                matches!(
                    op.op.as_slice(),
                    b"Do" | b"sh" | b"BI" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"Tj" | b"TJ" | b"'" | b"\""
                )
            })
        })
    });
    units
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExtractedScopeState {
    pub clip_ops: Vec<Op>,
    pub state_ops: Vec<Op>,
    pub effective_cm: Matrix,
    pub do_op: Op,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct EffectiveGState {
    ca: Option<f64>,
    ca_stroke: Option<f64>,
    bm: Option<Vec<u8>>,
    smask: Option<MaskIdentity>,
}

impl EffectiveGState {
    fn effective_ca(&self) -> f64 {
        self.ca.unwrap_or(1.0)
    }

    fn effective_ca_stroke(&self) -> f64 {
        self.ca_stroke.unwrap_or(1.0)
    }

    fn effective_bm(&self) -> &[u8] {
        self.bm.as_deref().unwrap_or(b"Normal")
    }

    fn effective_smask(&self) -> Option<&MaskIdentity> {
        self.smask.as_ref()
    }

    fn visually_differs(&self, other: &EffectiveGState) -> bool {
        (self.effective_ca() - other.effective_ca()).abs() > 1e-4
            || (self.effective_ca_stroke() - other.effective_ca_stroke()).abs() > 1e-4
            || self.effective_bm() != other.effective_bm()
            || self.effective_smask() != other.effective_smask()
    }

    fn merge_dict(&mut self, dict: &Dict, doc: &Document, gs_name: Option<&[u8]>) {
        if let Some(val) = dict.get(b"ca").map(|v| doc.resolve(v)).and_then(|v| v.as_f64()) {
            self.ca = Some(val);
        }
        if let Some(val) = dict.get(b"CA").map(|v| doc.resolve(v)).and_then(|v| v.as_f64()) {
            self.ca_stroke = Some(val);
        }
        if let Some(bm_obj) = dict.get(b"BM").map(|v| doc.resolve(v)) {
            if let Some(name) = bm_obj.as_name() {
                self.bm = Some(name.to_vec());
            } else if let Some(arr) = bm_obj.as_array()
                && let Some(first_name) = arr.first().map(|v| doc.resolve(v)).and_then(|v| v.as_name().map(|n| n.to_vec()))
            {
                self.bm = Some(first_name);
            }
        }
        if let Some(raw_smask) = dict.get(b"SMask") {
            let resolved = doc.resolve(raw_smask);
            if let Some(name) = resolved.as_name() {
                if name == b"None" {
                    self.smask = None;
                } else {
                    self.smask = Some(MaskIdentity::Named(name.to_vec()));
                }
            } else if let Some(mask_dict) = resolved.as_dict() {
                if let Some(gr) = mask_dict.get(b"G").and_then(|g| g.as_ref()) {
                    self.smask = Some(MaskIdentity::Group(gr.num, gr.generation));
                } else if let Some(r) = raw_smask.as_ref() {
                    self.smask = Some(MaskIdentity::Ref(r.num, r.generation));
                } else if let Some(name) = mask_dict.get(b"Name").map(|n| doc.resolve(n)).and_then(|n| n.as_name().map(|s| s.to_vec())) {
                    self.smask = Some(MaskIdentity::Named(name));
                } else if let Some(name) = gs_name {
                    self.smask = Some(MaskIdentity::Named(name.to_vec()));
                } else {
                    self.smask = Some(MaskIdentity::Named(b"CustomMask".to_vec()));
                }
            } else if let Some(r) = raw_smask.as_ref() {
                self.smask = Some(MaskIdentity::Ref(r.num, r.generation));
            }
        }
    }
}

#[derive(Clone, Default)]
struct ExtendedGStateTracker {
    stack: Vec<(EffectiveGState, Vec<(Op, Matrix)>)>,
    current_state: EffectiveGState,
    active_gs_ops: Vec<(Op, Matrix)>,
}

impl ExtendedGStateTracker {
    fn step(&mut self, op: &Op, ext_gstates: &Dict, doc: &Document, ctm: Matrix) -> Result<(), EditError> {
        match op.op.as_slice() {
            b"q" => {
                if self.stack.len() >= 4096 {
                    return Err(EditError::Invalid("the image graphics state is too deeply nested".into()));
                }
                self.stack.push((self.current_state.clone(), self.active_gs_ops.clone()));
            }
            b"Q" => {
                if let Some((prev_state, prev_ops)) = self.stack.pop() {
                    self.current_state = prev_state;
                    self.active_gs_ops = prev_ops;
                }
            }
            b"gs" => {
                if let Some(name) = op.name(0) {
                    if let Some(dict) = ext_gstates.get(name).map(|v| doc.resolve(v)).and_then(|v| v.as_dict().cloned()) {
                        self.current_state.merge_dict(&dict, doc, Some(name));
                    }
                    self.active_gs_ops.retain(|(o, _)| o.name(0) != Some(name));
                    self.active_gs_ops.push((op.clone(), ctm));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn clip_path_to_ops(clip: &ClipPath) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut current_pt = Point::ZERO;
    for el in clip.path.elements() {
        match el {
            kurbo::PathEl::MoveTo(p) => {
                if !p.x.is_finite() || !p.y.is_finite() {
                    return Vec::new();
                }
                current_pt = *p;
                ops.push(Op::new("m", vec![pdfcraft_content::num(p.x), pdfcraft_content::num(p.y)]));
            }
            kurbo::PathEl::LineTo(p) => {
                if !p.x.is_finite() || !p.y.is_finite() {
                    return Vec::new();
                }
                current_pt = *p;
                ops.push(Op::new("l", vec![pdfcraft_content::num(p.x), pdfcraft_content::num(p.y)]));
            }
            kurbo::PathEl::QuadTo(p1, p2) => {
                if !p1.x.is_finite() || !p1.y.is_finite() || !p2.x.is_finite() || !p2.y.is_finite() {
                    return Vec::new();
                }
                let c1_x = current_pt.x + (p1.x - current_pt.x) * (2.0 / 3.0);
                let c1_y = current_pt.y + (p1.y - current_pt.y) * (2.0 / 3.0);
                let c2_x = p2.x + (p1.x - p2.x) * (2.0 / 3.0);
                let c2_y = p2.y + (p1.y - p2.y) * (2.0 / 3.0);
                if !c1_x.is_finite() || !c1_y.is_finite() || !c2_x.is_finite() || !c2_y.is_finite() {
                    return Vec::new();
                }
                current_pt = *p2;
                ops.push(Op::new(
                    "c",
                    vec![
                        pdfcraft_content::num(c1_x),
                        pdfcraft_content::num(c1_y),
                        pdfcraft_content::num(c2_x),
                        pdfcraft_content::num(c2_y),
                        pdfcraft_content::num(p2.x),
                        pdfcraft_content::num(p2.y),
                    ],
                ));
            }
            kurbo::PathEl::CurveTo(p1, p2, p3) => {
                if !p1.x.is_finite() || !p1.y.is_finite() || !p2.x.is_finite() || !p2.y.is_finite() || !p3.x.is_finite() || !p3.y.is_finite() {
                    return Vec::new();
                }
                current_pt = *p3;
                ops.push(Op::new(
                    "c",
                    vec![
                        pdfcraft_content::num(p1.x),
                        pdfcraft_content::num(p1.y),
                        pdfcraft_content::num(p2.x),
                        pdfcraft_content::num(p2.y),
                        pdfcraft_content::num(p3.x),
                        pdfcraft_content::num(p3.y),
                    ],
                ));
            }
            kurbo::PathEl::ClosePath => {
                ops.push(Op::new("h", vec![]));
            }
        }
    }
    if !ops.is_empty() {
        ops.push(Op::new(if clip.even_odd { "W*" } else { "W" }, vec![]));
        ops.push(Op::new("n", vec![]));
    }
    ops
}

pub(crate) fn extract_scope_state(ops: &[Op], start: usize, target: usize, initial_ctm: Matrix) -> Result<Option<ExtractedScopeState>, EditError> {
    if target >= ops.len() || start > target {
        return Ok(None);
    }
    let target_op = ops.get(target).cloned().ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
    if start == target {
        return Ok(Some(ExtractedScopeState { clip_ops: Vec::new(), state_ops: Vec::new(), effective_cm: initial_ctm, do_op: target_op }));
    }
    let mut clip_ops = Vec::new();
    let mut state_ops = Vec::new();
    let mut pending_path: Vec<Op> = Vec::new();
    let mut pending_clip = false;
    let mut active_path = false;
    let mut depth: usize = 0;
    let mut matrix = initial_ctm;
    let mut matrix_at_clip = None;

    let scan_start = if ops.get(start).is_some_and(|o| o.is("q")) { start.saturating_add(1) } else { start };

    for i in scan_start..target {
        let Some(op) = ops.get(i) else {
            return Ok(None);
        };
        match op.op.as_slice() {
            b"q" => {
                if depth >= 4096 {
                    return Err(EditError::Invalid("the image graphics state is too deeply nested".into()));
                }
                depth = depth.saturating_add(1);
            }
            b"Q" => {
                if depth == 0 {
                    return Ok(None);
                }
                depth = depth.saturating_sub(1);
            }
            _ if depth > 0 => {}
            b"cm" => {
                if active_path {
                    return Ok(None);
                }
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" => {
                active_path = true;
                pending_path.push(op.clone());
            }
            b"W" | b"W*" => {
                if !active_path {
                    return Ok(None);
                }
                pending_clip = true;
                pending_path.push(op.clone());
            }
            b"n" => {
                if pending_clip {
                    if matrix_at_clip.is_none() {
                        matrix_at_clip = Some(matrix);
                    } else if matrix_at_clip != Some(matrix) {
                        return Ok(None);
                    }
                    clip_ops.extend(std::mem::take(&mut pending_path));
                    clip_ops.push(op.clone());
                    pending_clip = false;
                    active_path = false;
                } else {
                    pending_path.clear();
                    active_path = false;
                }
            }
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if pending_clip {
                    return Ok(None);
                }
                pending_path.clear();
                active_path = false;
            }
            b"gs" | b"CS" | b"cs" | b"SC" | b"sc" | b"SCN" | b"scn" | b"G" | b"g" | b"RG" | b"rg" | b"K" | b"k" | b"ri" | b"i" => {
                state_ops.push(op.clone());
            }
            b"BT" => {
                return Ok(None);
            }
            _ => {}
        }
    }

    if depth != 0 || active_path || pending_clip {
        return Ok(None);
    }

    if !clip_ops.is_empty() && matrix_at_clip != Some(Matrix::IDENTITY) {
        return Ok(None);
    }

    Ok(Some(ExtractedScopeState { clip_ops, state_ops, effective_cm: matrix, do_op: target_op }))
}

/// Change the stacking order of image `index` on `page`.
pub fn arrange_image(doc: &mut Document, page: usize, index: usize, order: ArrangeOrder) -> Result<(), EditError> {
    let images = page_images(doc, page)?;
    let img = images.get(index).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    let pieces = pdfcraft_content::Pieces::join(&all.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let target = img.op;
    if !ops.get(target).is_some_and(|op| op.is("Do")) {
        return Err(EditError::Invalid("the image operation could not be located".into()));
    }

    let is_wrapped = is_entire_page_wrapped(&ops);
    let (scan_start, scan_end) = if is_wrapped { (1, ops.len().saturating_sub(1)) } else { (0, ops.len()) };
    let units = content_units(&ops, scan_start, scan_end);
    let Some(target_idx) = units.iter().position(|u| u.start <= target && target < u.end) else {
        return Err(EditError::Invalid("the image content unit could not be located".into()));
    };
    let Some(target_unit) = units.get(target_idx) else { return Ok(()) };
    let has_other_paint = ops.iter().enumerate().any(|(i, op)| {
        i >= target_unit.start
            && i < target_unit.end
            && i != target
            && matches!(
                op.op.as_slice(),
                b"Do" | b"Tj" | b"TJ" | b"'" | b"\"" | b"sh" | b"BI" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*"
            )
    });
    if units.len() <= 1 && !has_other_paint {
        return Err(EditError::Invalid("the page has only one content unit to arrange".into()));
    }

    let (dest_unit, insert_after) = match order {
        ArrangeOrder::BringToFront => {
            if target_idx == units.len().saturating_sub(1) {
                if has_other_paint {
                    (target_unit, true)
                } else {
                    return Err(EditError::Invalid("the image is already in front".into()));
                }
            } else {
                let Some(u) = units.last() else {
                    return Err(EditError::Invalid("the image is already in front".into()));
                };
                (u, true)
            }
        }
        ArrangeOrder::SendToBack => {
            if target_idx == 0 {
                if has_other_paint {
                    (target_unit, false)
                } else {
                    return Err(EditError::Invalid("the image is already at the back".into()));
                }
            } else {
                let Some(u) = units.first() else {
                    return Err(EditError::Invalid("the image is already at the back".into()));
                };
                (u, false)
            }
        }
        ArrangeOrder::BringForward => {
            if target_idx == units.len().saturating_sub(1) {
                if has_other_paint {
                    (target_unit, true)
                } else {
                    return Err(EditError::Invalid("the image is already in front".into()));
                }
            } else {
                let Some(u) = units.get(target_idx.saturating_add(1)) else {
                    return Err(EditError::Invalid("the image is already in front".into()));
                };
                (u, true)
            }
        }
        ArrangeOrder::SendBackward => {
            if target_idx == 0 {
                if has_other_paint {
                    (target_unit, false)
                } else {
                    return Err(EditError::Invalid("the image is already at the back".into()));
                }
            } else {
                let Some(u) = target_idx.checked_sub(1).and_then(|i| units.get(i)) else {
                    return Err(EditError::Invalid("the image is already at the back".into()));
                };
                (u, false)
            }
        }
    };

    // A marked-content sequence that holds other content too moves only the image out of the
    // sequence, wrapping it in its own tag at the destination so it is not grouped with the
    // other content and leaves the remaining tagged content intact.
    let marked_unit = ops.get(target_unit.start).is_some_and(|o| o.is("BDC") || o.is("BMC"));

    let target_op = ops.get(target).ok_or_else(|| EditError::Invalid("the image operation could not be located".into()))?;
    let (mut target_span, scope_start, rem_start, rem_end) = if has_other_paint {
        if let Ok(Some((sub_start, sub_end, _))) = image_exclusive_scope(&ops, target)
            && sub_start >= target_unit.start
            && sub_end < target_unit.end
        {
            let start_op = ops.get(sub_start).unwrap_or(target_op);
            let end_op = ops.get(sub_end).unwrap_or(target_op);
            (start_op.span.start..end_op.span.end, sub_start, sub_start, sub_end.saturating_add(1))
        } else {
            (target_op.span.start..target_op.span.end, target, target, target.saturating_add(1))
        }
    } else if let Ok(Some((sub_start, sub_end, _))) = image_exclusive_scope(&ops, target)
        && sub_start >= target_unit.start
        && sub_end < target_unit.end
        && !marked_unit
    {
        let start_op = ops.get(sub_start).unwrap_or(target_op);
        let end_op = ops.get(sub_end).unwrap_or(target_op);
        (start_op.span.start..end_op.span.end, sub_start, sub_start, sub_end.saturating_add(1))
    } else if marked_unit {
        // A marked-content unit holding only this image moves whole, with its BDC…EMC.
        // The whole sequence is removed; the image's state is rebuilt from the ops inside it.
        let Some(first_op) = ops.get(target_unit.start) else { return Ok(()) };
        let Some(last_op) = target_unit.end.checked_sub(1).and_then(|i| ops.get(i)) else { return Ok(()) };
        (first_op.span.start..last_op.span.end, target_unit.start.saturating_add(1).min(target), target_unit.start, target_unit.end)
    } else {
        let Some(first_op) = ops.get(target_unit.start) else { return Ok(()) };
        let Some(last_op) = target_unit.end.checked_sub(1).and_then(|i| ops.get(i)) else { return Ok(()) };
        (first_op.span.start..last_op.span.end, target_unit.start, target_unit.start, target_unit.end)
    };

    let (dest_pos, scan_target) = if insert_after {
        let Some(op) = dest_unit.end.checked_sub(1).and_then(|i| ops.get(i)) else { return Ok(()) };
        (op.span.end, dest_unit.end)
    } else {
        let Some(op) = ops.get(dest_unit.start) else { return Ok(()) };
        (op.span.start, dest_unit.start)
    };

    let mut q_depth = 0usize;
    let mut leaking_non_ctm_ops = Vec::new();
    for op in ops.get(rem_start..rem_end).unwrap_or_default() {
        match op.op.as_slice() {
            b"q" => q_depth = q_depth.saturating_add(1),
            b"Q" => q_depth = q_depth.saturating_sub(1),
            b"rg" | b"RG" | b"g" | b"G" | b"k" | b"K" | b"w" | b"J" | b"j" | b"M" | b"d" | b"ri" | b"i" | b"gs" | b"cs" | b"CS" if q_depth == 0 => {
                leaking_non_ctm_ops.push(op.clone());
            }
            _ => {}
        }
    }

    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let ext_gstates = res.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).unwrap_or_default();

    let mut state = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let mut tracker = ExtendedGStateTracker::default();
    let mut source_effective_state = EffectiveGState::default();
    let mut source_gs_ops = Vec::new();
    let mut dest_effective_state = EffectiveGState::default();
    let mut dest_ctm = Matrix::IDENTITY;
    let mut scope_start_ctm = Matrix::IDENTITY;
    let mut outer_clips = Vec::new();
    let mut dest_clips = Vec::new();
    let mut ctm_before_removal = Matrix::IDENTITY;
    let mut ctm_after_removal = Matrix::IDENTITY;

    let max_scan = scan_target.max(target.saturating_add(1)).max(scope_start.saturating_add(1)).max(rem_end);
    for (i, op) in ops.iter().enumerate() {
        if i == rem_start {
            ctm_before_removal = state.placement.matrix;
        }
        if i == scope_start {
            scope_start_ctm = state.placement.matrix;
            outer_clips = state.placement.clips.clone();
        }
        if i == target {
            source_effective_state = tracker.current_state.clone();
            source_gs_ops = tracker.active_gs_ops.clone();
        }
        if i == scan_target {
            dest_effective_state = tracker.current_state.clone();
            dest_ctm = state.placement.matrix;
            dest_clips = state.placement.clips.clone();
        }
        if i >= max_scan {
            break;
        }
        state.apply(op)?;
        tracker.step(op, &ext_gstates, doc, state.placement.matrix)?;
        if i == rem_end.saturating_sub(1) {
            ctm_after_removal = state.placement.matrix;
        }
    }
    if scan_target >= ops.len() {
        dest_effective_state = tracker.current_state.clone();
        dest_ctm = state.placement.matrix;
        dest_clips = state.placement.clips.clone();
    }
    if dest_ctm.invert().is_none() {
        return Err(EditError::Invalid("unsupported graphics state: non-invertible CTM".into()));
    }

    let mut extracted = match extract_scope_state(&ops, scope_start, target, scope_start_ctm)? {
        Some(ext) => ext,
        None if scope_start != target => {
            let target_op = ops.get(target).cloned().ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
            if !marked_unit || has_other_paint {
                target_span = target_op.span.start..target_op.span.end;
            }
            extract_scope_state(&ops, target, target, Matrix(img.draw_matrix))?
                .ok_or_else(|| EditError::Invalid("unsupported graphics state: unable to cleanly extract image scope state".into()))?
        }
        None => {
            return Err(EditError::Invalid("unsupported graphics state: unable to cleanly extract image scope state".into()));
        }
    };

    // The clips the moved unit draws itself: all of the image's when they are rebuilt from
    // `img.clips`, else those its own scope set after the clips around it.
    let all_clips = extracted.clip_ops.is_empty();
    let reemitted = if all_clips {
        img.clips.as_slice()
    } else if img.clips.starts_with(&outer_clips) {
        img.clips.get(outer_clips.len()..).unwrap_or_default()
    } else {
        return Err(EditError::Invalid("unsupported graphics state: the image clipping path cannot be cleanly preserved".into()));
    };
    // At the destination it is clipped by the clips there and the ones it draws. Moving past a
    // clip the image isn't under (or out from under one it is) would change what part shows.
    if !dest_clips.iter().all(|c| img.clips.contains(c)) || !img.clips.iter().all(|c| dest_clips.contains(c) || reemitted.contains(c)) {
        return Err(EditError::Invalid(
            "unsupported graphics state: moving the image past a clipping path would change what part of it shows".into(),
        ));
    }

    if all_clips && !img.clips.is_empty() {
        for clip in &img.clips {
            let ops = clip_path_to_ops(clip);
            if !ops.is_empty() {
                extracted.clip_ops.extend(ops);
            }
        }
    }

    let has_inherited_clip = !img.clips.is_empty() && extracted.clip_ops.is_empty();
    if has_inherited_clip {
        return Err(EditError::Invalid("unsupported graphics state: inherited clipping path cannot be cleanly preserved".into()));
    }

    let non_gs_ops: Vec<Op> = extracted.state_ops.into_iter().filter(|o| !o.is("gs")).collect();

    let mut reconstructed_state = dest_effective_state.clone();
    for (op, _) in &source_gs_ops {
        if let Some(name) = op.name(0)
            && let Some(dict) = ext_gstates.get(name).map(|v| doc.resolve(v)).and_then(|v| v.as_dict().cloned())
        {
            reconstructed_state.merge_dict(&dict, doc, Some(name));
        }
    }

    if reconstructed_state.visually_differs(&source_effective_state) {
        return Err(EditError::Invalid(
            "unsupported graphics state: moving the image would alter effective opacity, blend mode, or soft mask".into(),
        ));
    }

    let mut unit_ops = Vec::new();
    let marked_open = if marked_unit && !has_other_paint { ops.get(target_unit.start).cloned() } else { None };
    if let Some(open) = &marked_open {
        unit_ops.push(open.clone());
    }
    unit_ops.push(Op::new("q", vec![]));
    let mut current_ctm = dest_ctm;

    for (gs_op, target_ctm) in &source_gs_ops {
        if *target_ctm != current_ctm {
            let Some(current_inv) = current_ctm.invert() else {
                return Err(EditError::Invalid("unsupported graphics state: non-invertible CTM".into()));
            };
            let delta = target_ctm.then(&current_inv);
            if !delta.0.iter().all(|v| v.is_finite()) {
                return Err(EditError::Invalid("unsupported graphics state: invalid transform coordinates".into()));
            }
            if delta != Matrix::IDENTITY {
                unit_ops.push(Op::new("cm", delta.0.map(pdfcraft_content::num).to_vec()));
                current_ctm = *target_ctm;
            }
        }
        unit_ops.push(gs_op.clone());
    }

    unit_ops.extend(non_gs_ops);

    if !extracted.clip_ops.is_empty() {
        if current_ctm != Matrix::IDENTITY {
            let Some(current_inv) = current_ctm.invert() else {
                return Err(EditError::Invalid("unsupported graphics state: non-invertible CTM".into()));
            };
            unit_ops.push(Op::new("cm", current_inv.0.map(pdfcraft_content::num).to_vec()));
        }
        unit_ops.extend(extracted.clip_ops);
        if Matrix(img.draw_matrix) != Matrix::IDENTITY {
            unit_ops.push(Op::new("cm", img.draw_matrix.map(pdfcraft_content::num).to_vec()));
        }
    } else {
        if Matrix(img.draw_matrix) != current_ctm {
            let Some(current_inv) = current_ctm.invert() else {
                return Err(EditError::Invalid("unsupported graphics state: non-invertible CTM".into()));
            };
            let delta = Matrix(img.draw_matrix).then(&current_inv);
            if !delta.0.iter().all(|v| v.is_finite()) {
                return Err(EditError::Invalid("unsupported graphics state: invalid transform coordinates".into()));
            }
            if delta != Matrix::IDENTITY {
                unit_ops.push(Op::new("cm", delta.0.map(pdfcraft_content::num).to_vec()));
            }
        }
    }
    unit_ops.push(extracted.do_op);
    unit_ops.push(Op::new("Q", vec![]));
    // An unbalanced sequence had no EMC; it doesn't gain one.
    if marked_open.is_some() && (has_other_paint || target_unit.end.checked_sub(1).and_then(|i| ops.get(i)).is_some_and(|o| o.is("EMC"))) {
        unit_ops.push(Op::new("EMC", vec![]));
    }

    let mut serialized = Vec::new();
    serialized.push(b'\n');
    serialized.extend_from_slice(&serialize_ops(&unit_ops));
    serialized.push(b'\n');

    let leaking = if let Some(inv) = ctm_before_removal.invert() { ctm_after_removal.then(&inv) } else { Matrix::IDENTITY };
    let mut leak_ops = Vec::new();
    if leaking != Matrix::IDENTITY {
        leak_ops.push(Op::new("cm", leaking.0.map(pdfcraft_content::num).to_vec()));
    }
    leak_ops.extend(leaking_non_ctm_ops);
    let removal_replacement = if !leak_ops.is_empty() {
        let mut s = Vec::new();
        s.push(b'\n');
        s.extend_from_slice(&serialize_ops(&leak_ops));
        s.push(b'\n');
        s
    } else {
        Vec::new()
    };

    let mut edits = Vec::new();
    if dest_pos >= target_span.end {
        edits.push((target_span, removal_replacement));
        edits.push((dest_pos..dest_pos, serialized));
    } else if dest_pos <= target_span.start {
        edits.push((dest_pos..dest_pos, serialized));
        edits.push((target_span, removal_replacement));
    } else {
        return Err(EditError::Invalid("no change in image stacking order".into()));
    }

    let changed = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &all, changed);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// Locate the innermost graphics state scope enclosing only `target` (with no other painting operations).
/// Returns `Some((start, end, scope_matrix))` where `start` is the index of `q`, `end` is the index of `Q`,
/// and `scope_matrix` is the CTM in effect immediately before entering that `q`.
fn image_exclusive_scope(ops: &[Op], target: usize) -> Result<Option<(usize, usize, Matrix)>, EditError> {
    struct Scope {
        start: usize,
        matrix: Matrix,
        inherited_clip: bool,
        inherited_path: bool,
        selected: bool,
        other_paint: bool,
    }
    let mut stack: Vec<Scope> = Vec::new();
    let mut matrix = Matrix::IDENTITY;
    let mut clipped = false;
    let mut path = false;
    let mut pending_clip = false;
    let mut text = false;
    let mut at_image = None;
    let mut isolated = None;
    for (index, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= 256 {
                    return Err(EditError::Invalid("the image graphics state is too deeply nested".into()));
                }
                stack.push(Scope { start: index, matrix, inherited_clip: clipped, inherited_path: path, selected: false, other_paint: false });
            }
            b"Q" => {
                if let Some(scope) = stack.pop() {
                    if scope.selected && !scope.other_paint && !scope.inherited_clip && !scope.inherited_path {
                        isolated.get_or_insert((scope.start, index, scope.matrix));
                    }
                    matrix = scope.matrix;
                    clipped = scope.inherited_clip;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            b"BT" => text = true,
            b"ET" => text = false,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" => path = true,
            b"W" | b"W*" => pending_clip = true,
            _ => {}
        }
        if index == target {
            if text {
                return Err(EditError::Invalid("an image inside a text object cannot be cropped".into()));
            }
            at_image = Some(matrix);
            for scope in &mut stack {
                scope.selected = true;
            }
        } else if matches!(
            op.op.as_slice(),
            b"Do" | b"Tj" | b"TJ" | b"'" | b"\"" | b"sh" | b"BI" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*"
        ) {
            for scope in &mut stack {
                scope.other_paint = true;
            }
        }
        if matches!(op.op.as_slice(), b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*") {
            clipped |= pending_clip;
            pending_clip = false;
            path = false;
        }
    }
    if at_image.is_none() {
        return Err(EditError::Invalid("the image operation is missing".into()));
    }
    Ok(isolated)
}

/// The scope `start..=end` without its clipping paths, for drawing in the CTM `scope_matrix`
/// in effect before it. Its `cm`s go too, and one `cm` before the image `Do` at `target` puts
/// it back at `draw`, so clips any earlier move put around it (`cm inverse`, path, `W n`,
/// absolute `cm`) can't leave the image displaced.
fn extract_image_drawing_ops(ops: &[Op], start: usize, end: usize, target: usize, scope_matrix: Matrix, draw: Matrix) -> Result<Vec<Op>, EditError> {
    let inverse = scope_matrix.invert().ok_or_else(|| EditError::Invalid("unsupported graphics state: non-invertible CTM".into()))?;
    let placement = draw.then(&inverse);
    if !placement.0.iter().all(|v| v.is_finite()) {
        return Err(EditError::Invalid("unsupported graphics state: invalid transform coordinates".into()));
    }
    let is_path = |o: &Op| matches!(o.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re");
    let mut drawing = Vec::new();
    let mut i = start.saturating_add(1);
    while i < end {
        let Some(op) = ops.get(i) else { break };
        if op.is("cm") {
            i = i.saturating_add(1);
            continue;
        }
        if is_path(op) {
            let mut k = i;
            while ops.get(k).is_some_and(is_path) {
                k = k.saturating_add(1);
            }
            if ops.get(k).is_some_and(|o| o.is("W") || o.is("W*")) && ops.get(k.saturating_add(1)).is_some_and(|o| o.is("n")) {
                i = k.saturating_add(2);
                continue;
            }
        }
        if i == target && placement != Matrix::IDENTITY {
            drawing.push(Op::new("cm", placement.0.map(pdfcraft_content::num).to_vec()));
        }
        drawing.push(op.clone());
        i = i.saturating_add(1);
    }
    Ok(drawing)
}

/// Remove any private clipping path / crop on image `index` on `page`.
pub fn remove_image_crop(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let images = page_images(doc, page)?;
    let img = images.get(index).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    let pieces = pdfcraft_content::Pieces::join(&all.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let target = img.op;
    if !ops.get(target).is_some_and(|op| op.is("Do")) {
        return Err(EditError::Invalid("the image operation could not be located".into()));
    }
    if let Some((start, end, scope_matrix)) = image_exclusive_scope(&ops, target)? {
        let drawing_ops = extract_image_drawing_ops(&ops, start, end, target, scope_matrix, Matrix(img.draw_matrix))?;
        let start_op = ops.get(start).ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
        let end_op = ops.get(end).ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
        let range = start_op.span.start..end_op.span.end;
        let mut unit_ops = Vec::new();
        unit_ops.push(Op::new("q", vec![]));
        unit_ops.extend(drawing_ops);
        unit_ops.push(Op::new("Q", vec![]));
        let serialized = serialize_ops(&unit_ops);
        let changed = pieces.splice([(range, serialized)]);
        let contents = crate::text::rewritten_contents(doc, &all, changed);
        doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    }
    Ok(())
}

/// Crop image `index` on `page` to `crop_rect` in user space `[x0, y0, x1, y1]`.
pub fn crop_image(doc: &mut Document, page: usize, index: usize, crop_rect: [f64; 4]) -> Result<(), EditError> {
    change_image(doc, page, index, &ImageChange::Crop(crop_rect))
}

/// Change image `index` (from [`page_images`]) on `page`.
pub fn change_image(doc: &mut Document, page: usize, index: usize, change: &ImageChange) -> Result<(), EditError> {
    let images = page_images(doc, page)?;
    let img = images.get(index).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    let pieces = joined(&all);
    let ops = pieces.parse().ops;
    if !ops.get(img.op).is_some_and(|op| op.is("Do")) {
        return Err(EditError::Invalid("the image operation is missing".into()));
    }
    let mut resources = None;
    let replacement: Vec<Op> = match change {
        ImageChange::Delete => Vec::new(),
        ImageChange::Transform(t) => {
            return transform_image(doc, &p, &all, &img, *t);
        }
        ImageChange::Arrange(order) => {
            return arrange_image(doc, page, index, *order);
        }
        ImageChange::RemoveCrop => {
            return remove_image_crop(doc, page, index);
        }
        ImageChange::Crop(keep) => {
            return crop_image_impl(doc, &p, &all, &img, *keep);
        }
        ImageChange::Replace(r) => {
            if img.is_form {
                return Err(EditError::Invalid(
                    "grouped Form artwork can be moved, resized, rotated, flipped or deleted; replacing it with a bitmap is not supported".into(),
                ));
            }
            let name = format!("PCImg{}", r.num);
            let mut res = p.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            let mut xo = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            xo.set(name.clone().into_bytes(), Object::Ref(*r));
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            resources = Some(res);
            vec![Op::new("Do", vec![Object::name(&name)])]
        }
    };
    let changed = crate::text::splice(&pieces, &ops, |index| if index == img.op { (replacement.clone(), false) } else { (Vec::new(), true) });
    let contents = crate::text::rewritten_contents(doc, &all, changed);
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
        if let Some(res) = resources {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(())
}

/// Inventory indexes exclude the streams that belong to added content.
pub(crate) fn selectable_images(
    doc: &Document,
    page: usize,
    excluded: &std::collections::BTreeSet<ObjRef>,
) -> Result<Vec<(usize, PageImage)>, EditError> {
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    Ok(page_images(doc, page)?
        .into_iter()
        .enumerate()
        .filter(|(_, image)| !all.get(image.stream).and_then(|(o, _)| o.as_ref()).is_some_and(|r| excluded.contains(&r)))
        .collect())
}

/// Find the saved graphics-state scope containing just this image and its crop. Applying
/// the transform before that scope moves the actual clipping path as well as the pixels.
/// A shared clip must never be moved along with unrelated artwork or text.
fn image_transform_scope(ops: &[Op], target: usize) -> Result<Option<(usize, usize, Matrix)>, EditError> {
    struct Scope {
        start: usize,
        matrix: Matrix,
        inherited_clip: bool,
        inherited_path: bool,
        selected: bool,
        other_paint: bool,
    }
    let mut stack: Vec<Scope> = Vec::new();
    let mut matrix = Matrix::IDENTITY;
    let mut clipped = false;
    let mut path = false;
    let mut pending_clip = false;
    let mut text = false;
    let mut at_image = None;
    let mut isolated = None;
    for (index, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= 256 {
                    return Err(EditError::Invalid("the image graphics state is too deeply nested to transform".into()));
                }
                stack.push(Scope { start: index, matrix, inherited_clip: clipped, inherited_path: path, selected: false, other_paint: false });
            }
            b"Q" => {
                if let Some(scope) = stack.pop() {
                    if scope.selected && !scope.other_paint && !scope.inherited_clip && !scope.inherited_path {
                        // Prefer the innermost scope that contains every active clipping path.
                        isolated.get_or_insert((scope.start, index, scope.matrix));
                    }
                    matrix = scope.matrix;
                    clipped = scope.inherited_clip;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            b"BT" => text = true,
            b"ET" => text = false,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" => path = true,
            b"W" | b"W*" => pending_clip = true,
            _ => {}
        }
        if index == target {
            if text {
                return Err(EditError::Invalid("an image inside a text object cannot be transformed".into()));
            }
            at_image = Some((matrix, clipped));
            for scope in &mut stack {
                scope.selected = true;
            }
        } else if matches!(
            op.op.as_slice(),
            b"Do" | b"Tj" | b"TJ" | b"'" | b"\"" | b"sh" | b"BI" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*"
        ) {
            for scope in &mut stack {
                scope.other_paint = true;
            }
        }
        if matches!(op.op.as_slice(), b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*") {
            clipped |= pending_clip;
            pending_clip = false;
            path = false;
        }
    }
    let (placement, clipped) = at_image.ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
    if !clipped {
        return Ok(Some((target, target, placement)));
    }
    Ok(isolated)
}

fn transform_image(
    doc: &mut Document,
    page: &pdfcraft_model::Page,
    streams: &[(Object, Vec<u8>)],
    image: &PageImage,
    transform: [f64; 6],
) -> Result<(), EditError> {
    if !transform.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
        return Err(EditError::Invalid("the image transform must be finite and no larger than 1e12".into()));
    }
    let pieces = joined(streams);
    let ops = pieces.parse().ops;
    let target = image.op;
    if !ops.get(target).is_some_and(|op| op.is("Do")) {
        return Err(EditError::Invalid("the image operation could not be located".into()));
    }
    // A private path crop moves with its image regardless of intervening state operators.
    // Shared crops retain the existing fixed-clip behavior, preserving neighbouring paint.
    let (start, end, placement) = image_transform_scope(&ops, target)?.unwrap_or((target, target, Matrix(image.draw_matrix)));
    let inverse = placement.invert().ok_or_else(|| EditError::Invalid("the image has no area".into()))?;
    let local = placement.then(&Matrix(transform)).then(&inverse);
    if !local.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
        return Err(EditError::Invalid("the image transform is too large".into()));
    }

    // Already alone in a `q cm … Q` (its own placement, or the wrapper an earlier edit added):
    // change that `cm` in place. The CTM inside is `local · cm · outer` either way.
    if let Some((at, cm)) = enclosing_transform(&ops, start, end) {
        let folded = local.then(&cm);
        if !folded.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the image transform is too large".into()));
        }
        let changed = crate::text::splice(&pieces, &ops, |index| {
            if index == at { (vec![Op::new("cm", folded.0.map(Object::Real).to_vec())], false) } else { (Vec::new(), true) }
        });
        let contents = crate::text::rewritten_contents(doc, streams, changed);
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
        return Ok(());
    }
    let changed = crate::text::splice(&pieces, &ops, |index| {
        if index == start {
            let mut out = vec![Op::new("q", vec![]), Op::new("cm", local.0.map(Object::Real).to_vec())];
            if start == end {
                out.extend(ops.get(index).cloned());
                out.push(Op::new("Q", vec![]));
                return (out, false);
            }
            (out, true)
        } else if index == end {
            let mut out: Vec<_> = ops.get(index).cloned().into_iter().collect();
            out.push(Op::new("Q", vec![]));
            (out, false)
        } else {
            (Vec::new(), true)
        }
    });
    let contents = crate::text::rewritten_contents(doc, streams, changed);
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// Crop: clip the image's drawing to `keep` (user space). The clip goes just inside the scope
/// that holds the image and its private crop (or a new `q … Q` around the `Do`), written in
/// that scope's coordinates as the box's four corners, so a turned image crops to an upright
/// box too and a later transform of the scope moves the crop with the pixels.
fn crop_image_impl(
    doc: &mut Document,
    page: &pdfcraft_model::Page,
    streams: &[(Object, Vec<u8>)],
    image: &PageImage,
    keep: [f64; 4],
) -> Result<(), EditError> {
    if image.is_form {
        return Err(EditError::Invalid("cropping grouped Form artwork is not supported".into()));
    }
    if !keep.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
        return Err(EditError::Invalid("the crop box must be finite and no larger than 1e12".into()));
    }
    let [x0, y0, x1, y1] = [keep[0].min(keep[2]), keep[1].min(keep[3]), keep[0].max(keep[2]), keep[1].max(keep[3])];
    if x1 - x0 < 1e-6 || y1 - y0 < 1e-6 {
        return Err(EditError::Invalid("the crop box has no area".into()));
    }
    if intersection(image.rect, [x0, y0, x1, y1]).is_none() {
        return Err(EditError::Invalid("the crop box does not intersect the image".into()));
    }

    let pieces = joined(streams);
    let ops = pieces.parse().ops;
    let target = image.op;
    if !ops.get(target).is_some_and(|op| op.is("Do")) {
        return Err(EditError::Invalid("the image operation could not be located".into()));
    }
    let (start, end, placement) = image_transform_scope(&ops, target)?.unwrap_or((target, target, Matrix(image.draw_matrix)));
    let inverse = placement.invert().ok_or_else(|| EditError::Invalid("the image has no area".into()))?;
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| inverse.apply(x, y));
    if !corners.iter().all(|(x, y)| x.is_finite() && y.is_finite() && x.abs() <= 1e12 && y.abs() <= 1e12) {
        return Err(EditError::Invalid("the crop box is too large for the image's placement".into()));
    }
    let is_axis_aligned = ((corners[0].1 - corners[1].1).abs() < 1e-4
        && (corners[1].0 - corners[2].0).abs() < 1e-4
        && (corners[2].1 - corners[3].1).abs() < 1e-4
        && (corners[3].0 - corners[0].0).abs() < 1e-4)
        || ((corners[0].0 - corners[1].0).abs() < 1e-4
            && (corners[1].1 - corners[2].1).abs() < 1e-4
            && (corners[2].0 - corners[3].0).abs() < 1e-4
            && (corners[3].1 - corners[0].1).abs() < 1e-4);
    let clip: Vec<Op> = if is_axis_aligned {
        let min_x = corners.iter().map(|(x, _)| *x).fold(f64::INFINITY, f64::min);
        let max_x = corners.iter().map(|(x, _)| *x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|(_, y)| *y).fold(f64::INFINITY, f64::min);
        let max_y = corners.iter().map(|(_, y)| *y).fold(f64::NEG_INFINITY, f64::max);
        vec![
            Op::new("re", vec![Object::Real(min_x), Object::Real(min_y), Object::Real(max_x - min_x), Object::Real(max_y - min_y)]),
            Op::new("W", vec![]),
            Op::new("n", vec![]),
        ]
    } else {
        corners
            .iter()
            .enumerate()
            .map(|(i, (x, y))| Op::new(if i == 0 { "m" } else { "l" }, vec![Object::Real(*x), Object::Real(*y)]))
            .chain([Op::new("h", vec![]), Op::new("W", vec![]), Op::new("n", vec![])])
            .collect()
    };
    // The image's own earlier crop (a clipping path right after the scope's `q`, skipping any
    // initial `cm` view transform) gives way to the new one, so a crop can grow again as well as shrink.
    let mut clip_start = start + 1;
    while ops.get(clip_start).is_some_and(|o| o.is("cm")) {
        clip_start += 1;
    }
    let old_clip: Option<std::ops::Range<usize>> = (start != end)
        .then(|| {
            let mut k = clip_start;
            while ops.get(k).is_some_and(|o| matches!(o.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")) {
                k += 1;
            }
            (k > clip_start && ops.get(k).is_some_and(|o| matches!(o.op.as_slice(), b"W" | b"W*")) && ops.get(k + 1).is_some_and(|o| o.is("n")))
                .then_some(clip_start..k + 2)
        })
        .flatten();
    let insert_at = old_clip.as_ref().map_or(clip_start, |r| r.start);
    let changed = crate::text::splice(&pieces, &ops, |index| {
        if start == end && index == target {
            // No scope of its own yet: one around the drawing, with the crop inside.
            let mut out = vec![Op::new("q", vec![])];
            out.extend(clip.iter().cloned());
            out.extend(ops.get(index).cloned());
            out.push(Op::new("Q", vec![]));
            (out, false)
        } else if start != end && index == insert_at {
            // Inside the scope's `q`, before whatever it draws (in place of its old crop).
            (clip.clone(), old_clip.is_none())
        } else if old_clip.as_ref().is_some_and(|r| r.contains(&index)) {
            (Vec::new(), false)
        } else {
            (Vec::new(), true)
        }
    });
    let contents = crate::text::rewritten_contents(doc, streams, changed);
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// Plan all transforms before changing a stream; indexes refer to the original inventory.
/// Splice only the chosen Do operators and preserve all other bytes and stream dictionaries.
pub(crate) fn translate_images(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let images = page_images(doc, page)?;
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    let mut plans = std::collections::BTreeMap::new();
    for index in indexes {
        let image = images.get(*index).ok_or_else(|| EditError::Invalid("a selected image no longer exists".into()))?;
        let placement = Matrix(image.draw_matrix);
        let inverse = placement.invert().ok_or_else(|| EditError::Invalid("a selected image has no area".into()))?;
        let [a, b, c, d, _, _] = inverse.0;
        let movement = Matrix([1.0, 0.0, 0.0, 1.0, a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]]);
        if !movement.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the image move is too large".into()));
        }
        plans.insert(image.op, (movement, placement, offset));
    }
    let pieces = joined(&all);
    let ops = pieces.parse().ops;
    let mut inside_text = false;
    for (index, op) in ops.iter().enumerate() {
        if op.is("BT") {
            inside_text = true;
        } else if op.is("ET") {
            inside_text = false;
        } else if inside_text && plans.contains_key(&index) {
            return Err(EditError::Invalid("an image inside a text object cannot be moved in a group".into()));
        }
    }
    // Images alone in their own `q re W n cm Do Q` take the clip with them.
    let mut clips = std::collections::BTreeMap::new();
    for index in 0..ops.len() {
        let Some(&(_, placement, offset)) = plans.get(&index) else { continue };
        let Some((at, cm)) = exclusive_clip(&ops, index) else { continue };
        let translation = Matrix([1.0, 0.0, 0.0, 1.0, offset[0], offset[1]]);
        let Some(c) = clip_matrix(placement, cm, translation).filter(|c| c.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12)) else {
            return Err(EditError::Invalid("the image move is too large".into()));
        };
        plans.remove(&index);
        // A `cm` an earlier move put before the clip takes this move too: `c · prev` is the
        // same CTM as `prev` followed by another `cm c`, without one more operator per move.
        let folded = at
            .checked_sub(1)
            .and_then(|i| Some((i, c.then(&Matrix(ops.get(i).filter(|op| op.is("cm"))?.nums::<6>()?)))))
            .filter(|(_, m)| m.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12));
        match folded {
            Some((i, m)) => clips.insert(i, (m, false)),
            None => clips.insert(at, (c, true)),
        };
    }
    // An image already alone in `q cm Do Q` moves by changing that `cm`, not by nesting another.
    let mut folds = std::collections::BTreeMap::new();
    for index in 0..ops.len() {
        let Some(&(movement, _, _)) = plans.get(&index) else { continue };
        let Some((at, cm)) = enclosing_transform(&ops, index, index) else { continue };
        let folded = movement.then(&cm);
        if !folded.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the image move is too large".into()));
        }
        plans.remove(&index);
        folds.insert(at, folded);
    }
    if plans.keys().any(|index| !ops.get(*index).is_some_and(|op| op.is("Do"))) {
        return Err(EditError::Invalid("a selected image operation is missing".into()));
    }
    let data = crate::text::splice(&pieces, &ops, |index| {
        if let Some((c, keep)) = clips.remove(&index) {
            return (vec![Op::new("cm", c.0.map(Object::Real).to_vec())], keep);
        }
        if let Some(m) = folds.remove(&index) {
            return (vec![Op::new("cm", m.0.map(Object::Real).to_vec())], false);
        }
        let Some((matrix, _, _)) = plans.remove(&index) else { return (Vec::new(), true) };
        let original = ops.get(index).cloned().into_iter();
        // Local translations can be tiny under a large CTM. Preserve their precision
        // rather than rounding them to zero or amplifying an error on the page.
        let mut out = vec![Op::new("q", vec![]), Op::new("cm", matrix.0.map(Object::Real).to_vec())];
        out.extend(original);
        out.push(Op::new("Q", vec![]));
        (out, false)
    });
    let contents = crate::text::rewritten_contents(doc, &all, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}
