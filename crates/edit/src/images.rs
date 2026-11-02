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
#[derive(Clone, Debug)]
pub(crate) struct MaskIdentity {
    // Share the owning ExtGState instead of copying an arbitrarily large inline mask on q/Q.
    // Its complete SMask value retains S, TR, BC and every other mask parameter, not just G.
    definition: Arc<Object>,
    ctm: Matrix,
}

impl PartialEq for MaskIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.ctm == other.ctm && self.definition.as_dict().and_then(|d| d.get(b"SMask")) == other.definition.as_dict().and_then(|d| d.get(b"SMask"))
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
    /// The content stream its `Do` (its first token) is in, and the `Do`'s index among the
    /// operators of the page's streams joined ([`Pieces`]).
    pub stream: usize,
    op: usize,
    /// CTM before `Do`, excluding a Form's own Matrix.
    draw_matrix: [f64; 6],
    clips: Vec<ClipPath>,
}

impl PageImage {
    /// Whether an active clip was written by PdfCraft's image crop tool.
    pub fn has_editor_crop(&self) -> bool {
        self.clips.iter().any(|clip| clip.app_crop)
    }

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
    app_crop: bool,
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
    app_crop: bool,
}

impl ImageState {
    fn new(matrix: Matrix, clip: [f64; 4], clips: Vec<ClipPath>) -> Self {
        Self {
            placement: Placement { matrix, clip, clips },
            stack: Vec::new(),
            path: BezPath::new(),
            subpath_start: None,
            pending_clip: None,
            app_crop: false,
        }
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
            b"MP" if op.name(0) == Some(b"PdfCraftImageCrop") => self.app_crop = true,
            b"W" => self.pending_clip = Some(false),
            b"W*" => self.pending_clip = Some(true),
            b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if let Some(even_odd) = self.pending_clip.take() {
                    // Filling/clipping implicitly closes open subpaths. BezPath's winding
                    // follows the same rule, and its bounds use cubic extrema, not controls.
                    let bounds = self.path.bounding_box();
                    self.placement.clip = intersection(self.placement.clip, [bounds.x0, bounds.y0, bounds.x1, bounds.y1]).unwrap_or([0.0; 4]);
                    self.placement.clips.push(ClipPath { path: Arc::new(std::mem::take(&mut self.path)), even_odd, app_crop: self.app_crop });
                }
                self.path = BezPath::new();
                self.subpath_start = None;
                self.app_crop = false;
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
    let mut out = Vec::new();
    // The streams are one stream in pieces, so the graphics state and operators carry over.
    let mut state = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let pieces = joined(&content_streams(doc, &p.dict)?);
    for (i, op) in pieces.parse().ops.iter().enumerate() {
        state.apply(op)?;
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
            if depth == 1 && i < ops.len().saturating_sub(1) {
                return false;
            }
            depth = depth.saturating_sub(1);
        }
    }
    depth == 0
}

/// The end (exclusive) of the sequence opened at `i`: just past the `close` that matches it,
/// counting the nested `opens`, or `end` when it never closes.
fn sequence_end(ops: &[Op], i: usize, end: usize, opens: &[&str], close: &str) -> usize {
    let mut depth: usize = 1;
    for (j, op) in ops.iter().enumerate().take(end).skip(i.saturating_add(1)) {
        if opens.iter().any(|o| op.is(o)) {
            depth = depth.saturating_add(1);
        } else if op.is(close) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return j.saturating_add(1);
            }
        }
    }
    end
}

pub(crate) fn content_units(ops: &[Op], scan_start: usize, scan_end: usize) -> Vec<std::ops::Range<usize>> {
    let mut units = Vec::new();
    let mut i = scan_start;
    while i < scan_end {
        let Some(op) = ops.get(i) else { break };
        let j = if op.is("BDC") || op.is("BMC") {
            // A marked-content sequence is one unit up to its matching EMC (#3145), so Arrange moves
            // it whole and never inserts into another one: its content belongs to its tag (ISO 32000-2
            // § 14.6). An unbalanced sequence runs to the end of the scan.
            sequence_end(ops, i, scan_end, &["BDC", "BMC"], "EMC")
        } else if op.is("q") {
            sequence_end(ops, i, scan_end, &["q"], "Q")
        } else if op.is("BT") {
            sequence_end(ops, i, scan_end, &[], "ET")
        } else if op.is("BI") {
            sequence_end(ops, i, scan_end, &[], "EI")
        } else if op.is("Do") || op.is("sh") {
            i.saturating_add(1)
        } else {
            let mut j = i;
            let mut found_end = false;
            while j < scan_end {
                let Some(next) = ops.get(j) else { break };
                if matches!(next.op.as_slice(), b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                    j = j.saturating_add(1);
                    found_end = true;
                    break;
                } else if j > i
                    && matches!(
                        next.op.as_slice(),
                        b"q" | b"Q" | b"BT" | b"BI" | b"Do" | b"sh" | b"BDC" | b"BMC" | b"EMC" | b"Tj" | b"TJ" | b"'" | b"\""
                    )
                {
                    found_end = true;
                    break;
                }
                j = j.saturating_add(1);
            }
            if found_end { j } else { scan_end.max(i.saturating_add(1)) }
        };
        units.push(i..j);
        i = j;
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
    pub do_op: Op,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EffectiveGState {
    pub(crate) ca: Option<f64>,
    pub(crate) ca_stroke: Option<f64>,
    pub(crate) bm: Option<Vec<u8>>,
    pub(crate) smask: Option<MaskIdentity>,
    // Printing parameters cannot generally be reset by a content-stream operator. Retain their
    // exact values so Arrange refuses a destination whose state it cannot reconstruct.
    print_params: std::collections::BTreeMap<Vec<u8>, std::rc::Rc<Object>>,
    rendering_intent: Option<Vec<u8>>,
}

impl EffectiveGState {
    pub(crate) fn visually_differs(&self, other: &EffectiveGState) -> bool {
        (self.ca.unwrap_or(1.0) - other.ca.unwrap_or(1.0)).abs() > 1e-4
            || (self.ca_stroke.unwrap_or(1.0) - other.ca_stroke.unwrap_or(1.0)).abs() > 1e-4
            || self.bm.as_deref().unwrap_or(b"Normal") != other.bm.as_deref().unwrap_or(b"Normal")
            || self.smask != other.smask
            || self.print_params != other.print_params
    }

    #[cfg(test)]
    pub(crate) fn merge_dict(&mut self, dict: &Dict, doc: &Document, ctm: Matrix) {
        self.merge_shared(&Arc::new(Object::Dict(dict.clone())), doc, ctm);
    }

    fn merge_shared(&mut self, definition: &Arc<Object>, doc: &Document, ctm: Matrix) {
        let Some(dict) = definition.as_dict() else { return };
        for key in [b"TR".as_slice(), b"TR2", b"OP", b"op", b"OPM", b"BG", b"BG2", b"UCR", b"UCR2", b"HT"] {
            if let Some(value) = dict.get(key) {
                self.print_params.insert(key.to_vec(), std::rc::Rc::new(value.clone()));
            }
        }
        if let Some(intent) = dict.get(b"RI").map(|v| doc.resolve(v)).and_then(|v| v.as_name().map(<[u8]>::to_vec)) {
            self.rendering_intent = Some(intent);
        }
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
            let disabled = raw_smask.as_name() == Some(b"None") || raw_smask.as_ref().is_some_and(|r| doc.get(r).as_name() == Some(b"None"));
            self.smask = (!disabled).then(|| MaskIdentity { definition: Arc::clone(definition), ctm });
        }
    }
}

const MAX_ACTIVE_GS_OPS: usize = 128;

type ActiveGsOp = (Vec<u8>, Op, Matrix);
type ActiveGsOps = std::rc::Rc<Vec<ActiveGsOp>>;

#[derive(Clone, Default)]
struct ExtendedGStateTracker {
    stack: Vec<(EffectiveGState, ActiveGsOps)>,
    current_state: EffectiveGState,
    active_gs_ops: ActiveGsOps,
    definitions: std::collections::HashMap<Vec<u8>, Arc<Object>>,
}

impl ExtendedGStateTracker {
    fn step(&mut self, op: &Op, ext_gstates: &Dict, doc: &Document, ctm: Matrix) -> Result<(), EditError> {
        match op.op.as_slice() {
            b"ri" => self.current_state.rendering_intent = op.name(0).map(<[u8]>::to_vec),
            b"q" => {
                if self.stack.len() >= 4096 {
                    return Err(EditError::Invalid("the image graphics state is too deeply nested".into()));
                }
                self.stack.push((self.current_state.clone(), std::rc::Rc::clone(&self.active_gs_ops)));
            }
            b"Q" => {
                if let Some((prev_state, prev_ops)) = self.stack.pop() {
                    self.current_state = prev_state;
                    self.active_gs_ops = prev_ops;
                }
            }
            b"gs" => {
                let valid_gs = op.name(0).and_then(|name| {
                    let definition = self.definitions.get(name).cloned().or_else(|| {
                        let definition = doc.resolve(ext_gstates.get(name)?);
                        definition.as_dict()?;
                        self.definitions.insert(name.to_vec(), Arc::clone(&definition));
                        Some(definition)
                    })?;
                    Some((name, definition))
                });
                if let Some((name, definition)) = valid_gs {
                    self.current_state.merge_shared(&definition, doc, ctm);
                    let mut next = (*self.active_gs_ops).clone();
                    if let Some(pos) = next.iter().position(|(k, _, _)| k.as_slice() == name) {
                        next.remove(pos);
                    }
                    if next.len() < MAX_ACTIVE_GS_OPS {
                        next.push((name.to_vec(), op.clone(), ctm));
                    }
                    self.active_gs_ops = std::rc::Rc::new(next);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// The drawing-state operators besides colour that a moved unit restores, in the order it writes them.
const DRAWING_PARAMS: [&str; 14] = ["w", "J", "j", "M", "d", "ri", "i", "Tc", "Tw", "Tz", "TL", "Tr", "Ts", "Tf"];

#[derive(Clone, Default, Debug, PartialEq)]
struct DrawingState {
    non_stroking_color: Option<SharedColorOps>,
    stroking_color: Option<SharedColorOps>,
    /// The last operator of each of [`DRAWING_PARAMS`].
    params: [Option<std::rc::Rc<Op>>; DRAWING_PARAMS.len()],
}

// Save/restore shares operand storage, including when only the current colour changes under q.
type SharedColorOps = std::rc::Rc<Vec<std::rc::Rc<Op>>>;

/// Keep the explicit or implicit colour-space selector before `sc`/`scn` (or `SC`/`SCN`).
fn set_color(slot: &mut Option<SharedColorOps>, op: &Op, space: &str) {
    match slot {
        Some(ops) if ops.first().is_some_and(|o| o.is(space) || matches!(o.op.as_slice(), b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K")) => {
            let ops = std::rc::Rc::make_mut(ops);
            ops.truncate(1);
            ops.push(std::rc::Rc::new(op.clone()));
        }
        _ => *slot = Some(std::rc::Rc::new(vec![std::rc::Rc::new(op.clone())])),
    }
}

#[derive(Clone, Default, Debug)]
struct DrawingStateTracker {
    stack: Vec<DrawingState>,
    current: DrawingState,
}

impl DrawingStateTracker {
    fn step(&mut self, op: &Op) -> Result<(), EditError> {
        match op.op.as_slice() {
            b"q" => {
                if self.stack.len() >= 4096 {
                    return Err(EditError::Invalid("the image graphics state is too deeply nested".into()));
                }
                self.stack.push(self.current.clone());
            }
            b"Q" => {
                if let Some(prev) = self.stack.pop() {
                    self.current = prev;
                }
            }
            b"g" | b"rg" | b"k" | b"cs" => self.current.non_stroking_color = Some(std::rc::Rc::new(vec![std::rc::Rc::new(op.clone())])),
            b"sc" | b"scn" => set_color(&mut self.current.non_stroking_color, op, "cs"),
            b"G" | b"RG" | b"K" | b"CS" => self.current.stroking_color = Some(std::rc::Rc::new(vec![std::rc::Rc::new(op.clone())])),
            b"SC" | b"SCN" => set_color(&mut self.current.stroking_color, op, "CS"),
            _ => {
                if let Some(slot) = DRAWING_PARAMS.iter().position(|p| op.is(p)).and_then(|k| self.current.params.get_mut(k)) {
                    *slot = Some(std::rc::Rc::new(op.clone()));
                }
            }
        }
        Ok(())
    }
}

/// Initial line style, rendering intent and text-state parameters.
fn initial_param(name: &str) -> Option<Op> {
    use pdfcraft_content::num;
    let operands = match name {
        "w" => vec![num(1.0)],
        "J" | "j" => vec![num(0.0)],
        "M" => vec![num(10.0)],
        "d" => vec![Object::Array(vec![]), num(0.0)],
        "ri" => vec![Object::name("RelativeColorimetric")],
        "Tc" | "Tw" | "TL" | "Tr" | "Ts" => vec![num(0.0)],
        "Tz" => vec![num(100.0)],
        _ => return None,
    };
    Some(Op::new(name, operands))
}

/// The drawing state the image had at `source` that differs at `dest`, unless the moved unit's own
/// state operators (`extracted_state_ops`) set it.
fn compute_inherited_drawing_ops(source: &DrawingState, dest: &DrawingState, extracted_state_ops: &[Op]) -> Vec<Op> {
    let sets = |names: &[&str]| extracted_state_ops.iter().any(|o| names.iter().any(|n| o.is(n)));
    let mut ops = Vec::new();
    for (own, source, dest, black) in [
        (sets(&["g", "rg", "k", "cs"]), &source.non_stroking_color, &dest.non_stroking_color, "g"),
        (sets(&["G", "RG", "K", "CS"]), &source.stroking_color, &dest.stroking_color, "G"),
    ] {
        match (own, source, dest) {
            (true, _, _) | (false, None, None) => {}
            (false, Some(color), _) => ops.extend(color.iter().map(|op| (**op).clone())),
            (false, None, Some(_)) => ops.push(Op::new(black, vec![pdfcraft_content::num(0.0)])),
        }
    }
    let mut text_ops = Vec::new();
    for ((name, source), dest) in DRAWING_PARAMS.iter().zip(&source.params).zip(&dest.params) {
        let output = if matches!(*name, "Tc" | "Tw" | "Tz" | "TL" | "Tr" | "Ts" | "Tf") { &mut text_ops } else { &mut ops };
        match (sets(&[name]), source, dest) {
            (true, _, _) | (false, None, None) => {}
            (false, Some(op), _) => output.push((**op).clone()),
            (false, None, Some(_)) => output.extend(initial_param(name)),
        }
    }
    if !text_ops.is_empty() {
        // Forms inherit text state, even though each BT resets its text matrices.
        ops.push(Op::new("BT", vec![]));
        ops.extend(text_ops);
        ops.push(Op::new("ET", vec![]));
    }
    ops
}

/// The operators that set `clip` again, in user space. [`ImageState`] builds clips only from
/// finite points with `m`, `l`, `c` and `h`, so `None` (a quadratic segment) does not happen.
fn clip_path_to_ops(clip: &ClipPath) -> Option<Vec<Op>> {
    let mut ops = Vec::new();
    if clip.app_crop {
        ops.push(crop_marker());
    }
    for el in clip.path.elements() {
        let (name, points) = match el {
            kurbo::PathEl::MoveTo(p) => ("m", vec![*p]),
            kurbo::PathEl::LineTo(p) => ("l", vec![*p]),
            kurbo::PathEl::CurveTo(p1, p2, p3) => ("c", vec![*p1, *p2, *p3]),
            kurbo::PathEl::ClosePath => ("h", Vec::new()),
            kurbo::PathEl::QuadTo(..) => return None,
        };
        ops.push(Op::new(name, points.iter().flat_map(|p| [pdfcraft_content::num(p.x), pdfcraft_content::num(p.y)]).collect()));
    }
    if !ops.is_empty() {
        ops.push(Op::new(if clip.even_odd { "W*" } else { "W" }, vec![]));
        ops.push(Op::new("n", vec![]));
    }
    Some(ops)
}

pub(crate) fn extract_scope_state(ops: &[Op], start: usize, target: usize, initial_ctm: Matrix) -> Result<Option<ExtractedScopeState>, EditError> {
    if target >= ops.len() || start > target {
        return Ok(None);
    }
    let target_op = ops.get(target).cloned().ok_or_else(|| EditError::Invalid("the image operation is missing".into()))?;
    if start == target {
        return Ok(Some(ExtractedScopeState { clip_ops: Vec::new(), state_ops: Vec::new(), do_op: target_op }));
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

    Ok(Some(ExtractedScopeState { clip_ops, state_ops, do_op: target_op }))
}

/// The unit `order` puts the image's unit `target_idx` next to, and whether after it. An image that
/// shares its unit with other paint at the limit moves within that unit, past or behind the paint.
fn target_destination_unit<'a>(
    units: &'a [std::ops::Range<usize>],
    target_idx: usize,
    target_unit: &'a std::ops::Range<usize>,
    order: ArrangeOrder,
    has_other_paint: bool,
) -> Result<(&'a std::ops::Range<usize>, bool), EditError> {
    let forward = matches!(order, ArrangeOrder::BringToFront | ArrangeOrder::BringForward);
    let refused = || EditError::Invalid(if forward { "the image is already in front" } else { "the image is already at the back" }.into());
    let at_limit = if forward { target_idx.saturating_add(1) >= units.len() } else { target_idx == 0 };
    if at_limit {
        return if has_other_paint { Ok((target_unit, forward)) } else { Err(refused()) };
    }
    let dest = match order {
        ArrangeOrder::BringToFront => units.last(),
        ArrangeOrder::SendToBack => units.first(),
        ArrangeOrder::BringForward => units.get(target_idx.saturating_add(1)),
        ArrangeOrder::SendBackward => target_idx.checked_sub(1).and_then(|i| units.get(i)),
    };
    dest.map(|u| (u, forward)).ok_or_else(refused)
}

/// A moved unit must not borrow a graphics or marked-content boundary from its neighbours.
fn balanced_arrange_span(ops: &[Op], range: std::ops::Range<usize>) -> bool {
    let Some(slice) = ops.get(range) else { return false };
    let (mut graphics, mut marked) = (0usize, 0usize);
    for op in slice {
        let depth = match op.op.as_slice() {
            b"q" => {
                graphics = graphics.saturating_add(1);
                continue;
            }
            b"BDC" | b"BMC" => {
                marked = marked.saturating_add(1);
                continue;
            }
            b"Q" => &mut graphics,
            b"EMC" => &mut marked,
            _ => continue,
        };
        let Some(next) = depth.checked_sub(1) else { return false };
        *depth = next;
    }
    graphics == 0 && marked == 0
}

/// What Arrange takes out: the byte span, the op where the image's own scope state starts, and the
/// ops `start..end` removed. That is the image's own `q … Q` scope inside its unit, else the `Do`
/// alone when the unit paints something else too, else the whole unit.
fn determine_arrange_spans(
    ops: &[Op],
    target: usize,
    target_unit: &std::ops::Range<usize>,
    has_other_paint: bool,
    marked_unit: bool,
) -> Result<(std::ops::Range<usize>, usize, usize, usize), EditError> {
    let scope = image_scopes(ops, target, "arranged")
        .ok()
        .and_then(|s| s.isolated)
        .filter(|&(start, end, _)| start >= target_unit.start && end < target_unit.end);
    let unit_last = target_unit.end.saturating_sub(1);
    let (first, scope_start, last) = match scope {
        Some((start, end, _)) if has_other_paint || !marked_unit => (start, start, end),
        _ if has_other_paint => (target, target, target),
        // A marked-content unit holding only this image moves whole, with its BDC…EMC.
        // The whole sequence is removed; the image's state is rebuilt from the ops inside it.
        _ if marked_unit => (target_unit.start, target_unit.start.saturating_add(1).min(target), unit_last),
        _ => (target_unit.start, target_unit.start, unit_last),
    };
    if !balanced_arrange_span(ops, first..last.saturating_add(1)) {
        return Err(EditError::Invalid("cannot arrange an image across graphics or marked-content boundaries".into()));
    }
    let missing = || EditError::Invalid("the image content unit could not be located".into());
    let start = ops.get(first).ok_or_else(missing)?.span.start;
    let end = ops.get(last).ok_or_else(missing)?.span.end;
    Ok((start..end, scope_start, first, last.saturating_add(1)))
}

fn scan_leaking_ops(ops: &[Op], rem_start: usize, rem_end: usize) -> (Vec<Op>, bool) {
    let mut q_depth = 0usize;
    let mut leaking_ops = Vec::new();
    let mut pending_clip_path: Vec<Op> = Vec::new();
    let mut pending_clip = false;
    let mut has_leaking_clip = false;
    for op in ops.get(rem_start..rem_end).unwrap_or_default() {
        match op.op.as_slice() {
            b"q" => q_depth = q_depth.saturating_add(1),
            b"Q" => q_depth = q_depth.saturating_sub(1),
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" if q_depth == 0 => {
                pending_clip_path.push(op.clone());
            }
            b"W" | b"W*" if q_depth == 0 => {
                if !pending_clip_path.is_empty() {
                    pending_clip = true;
                    pending_clip_path.push(op.clone());
                }
            }
            b"n" if q_depth == 0 => {
                if pending_clip {
                    pending_clip_path.push(op.clone());
                    leaking_ops.extend(std::mem::take(&mut pending_clip_path));
                    has_leaking_clip = true;
                    pending_clip = false;
                } else {
                    pending_clip_path.clear();
                }
            }
            b"rg" | b"RG" | b"g" | b"G" | b"k" | b"K" | b"cs" | b"CS" | b"sc" | b"scn" | b"SC" | b"SCN" | b"w" | b"J" | b"j" | b"M" | b"d"
            | b"ri" | b"i" | b"gs" | b"Tf" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tr" | b"Ts"
                if q_depth == 0 =>
            {
                leaking_ops.push(op.clone());
            }
            b"cm" if q_depth == 0 => {
                leaking_ops.push(op.clone());
            }
            _ => {
                if q_depth == 0 && !op.is("BDC") && !op.is("BMC") && !op.is("EMC") && !pending_clip {
                    pending_clip_path.clear();
                }
            }
        }
    }
    (leaking_ops, has_leaking_clip)
}

fn build_relocated_unit_ops(
    marked_open: Option<&Op>,
    source_gs_ops: &[(Op, Matrix)],
    inherited_drawing_ops: &[Op],
    extracted: ExtractedScopeState,
    dest_ctm: Matrix,
    draw_matrix: [f64; 6],
    has_closing_emc: bool,
) -> Result<Vec<Op>, EditError> {
    let mut unit_ops = Vec::new();
    if let Some(open) = marked_open {
        unit_ops.push(open.clone());
    }
    unit_ops.push(Op::new("q", vec![]));
    let mut current_ctm = dest_ctm;

    for (gs_op, target_ctm) in source_gs_ops {
        if *target_ctm != current_ctm {
            let current_inv = current_ctm.invert().ok_or_else(|| EditError::Invalid("unsupported graphics state: non-invertible CTM".into()))?;
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

    unit_ops.extend(inherited_drawing_ops.iter().cloned());

    let non_gs_ops = extracted.state_ops.into_iter().filter(|o| !o.is("gs"));
    unit_ops.extend(non_gs_ops);

    if !extracted.clip_ops.is_empty() {
        if current_ctm != Matrix::IDENTITY {
            let current_inv = current_ctm.invert().ok_or_else(|| EditError::Invalid("unsupported graphics state: non-invertible CTM".into()))?;
            unit_ops.push(Op::new("cm", current_inv.0.map(pdfcraft_content::num).to_vec()));
        }
        unit_ops.extend(extracted.clip_ops);
        if Matrix(draw_matrix) != Matrix::IDENTITY {
            unit_ops.push(Op::new("cm", draw_matrix.map(pdfcraft_content::num).to_vec()));
        }
    } else if Matrix(draw_matrix) != current_ctm {
        let current_inv = current_ctm.invert().ok_or_else(|| EditError::Invalid("unsupported graphics state: non-invertible CTM".into()))?;
        let delta = Matrix(draw_matrix).then(&current_inv);
        if !delta.0.iter().all(|v| v.is_finite()) {
            return Err(EditError::Invalid("unsupported graphics state: invalid transform coordinates".into()));
        }
        if delta != Matrix::IDENTITY {
            unit_ops.push(Op::new("cm", delta.0.map(pdfcraft_content::num).to_vec()));
        }
    }
    unit_ops.push(extracted.do_op);
    unit_ops.push(Op::new("Q", vec![]));
    if has_closing_emc {
        unit_ops.push(Op::new("EMC", vec![]));
    }
    Ok(unit_ops)
}

/// Change the stacking order of `img`, drawn on page `p` by its content streams `all`.
fn arrange_image(
    doc: &mut Document,
    p: &pdfcraft_model::Page,
    all: &[(Object, Vec<u8>)],
    img: &PageImage,
    order: ArrangeOrder,
) -> Result<(), EditError> {
    let pieces = joined(all);
    let ops = pieces.parse().ops;
    let target = img.op;
    let target_op = ops.get(target).filter(|op| op.is("Do")).ok_or_else(|| EditError::Invalid("the image operation could not be located".into()))?;

    let is_wrapped = is_entire_page_wrapped(&ops);
    let (scan_start, scan_end) = if is_wrapped { (1, ops.len().saturating_sub(1)) } else { (0, ops.len()) };
    let units = content_units(&ops, scan_start, scan_end);
    let Some((target_idx, target_unit)) = units.iter().enumerate().find(|(_, u)| u.contains(&target)) else {
        return Err(EditError::Invalid("the image content unit could not be located".into()));
    };
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

    let (dest_unit, insert_after) = target_destination_unit(&units, target_idx, target_unit, order, has_other_paint)?;
    if !balanced_arrange_span(&ops, target_unit.clone()) || !balanced_arrange_span(&ops, dest_unit.clone()) {
        return Err(EditError::Invalid("cannot arrange an image across graphics or marked-content boundaries".into()));
    }

    // A marked-content sequence that holds other content cannot be split without generating
    // a new structure element and MCID, so we refuse to rearrange it to avoid leaving the image untagged.
    // If the sequence holds only this image, the whole marked unit is moved intact.
    let marked_unit = ops.get(target_unit.start).is_some_and(|o| o.is("BDC") || o.is("BMC"));
    let marked_depth = ops.iter().take(target).fold(0usize, |depth, op| {
        if op.is("BDC") || op.is("BMC") {
            depth.saturating_add(1)
        } else if op.is("EMC") {
            depth.saturating_sub(1)
        } else {
            depth
        }
    });
    if marked_depth > usize::from(marked_unit) {
        return Err(EditError::Invalid("cannot rearrange an image out of a nested marked-content sequence".into()));
    }
    if marked_unit && has_other_paint {
        return Err(EditError::Invalid("cannot rearrange an image that shares a marked-content sequence with other content".into()));
    }

    let (mut target_span, scope_start, rem_start, rem_end) = determine_arrange_spans(&ops, target, target_unit, has_other_paint, marked_unit)?;

    let missing = || EditError::Invalid("the destination content unit could not be located".into());
    let (dest_pos, scan_target) = if insert_after {
        (dest_unit.end.checked_sub(1).and_then(|i| ops.get(i)).ok_or_else(missing)?.span.end, dest_unit.end)
    } else {
        (ops.get(dest_unit.start).ok_or_else(missing)?.span.start, dest_unit.start)
    };

    // An added item owns its entire stream: editing or deleting it replaces that stream.
    // Insert ordinary artwork next to the complete owned unit in a separate stream instead.
    let destination_piece = if insert_after {
        dest_unit.end.checked_sub(1).and_then(|i| ops.get(i)).map(|op| pieces.piece_of(op.span.end.saturating_sub(1)))
    } else {
        ops.get(dest_unit.start).map(|op| pieces.piece_of(op.span.start))
    };
    let owned_destination = destination_piece
        .filter(|piece| all.get(*piece).is_some_and(|(obj, _)| doc.resolve(obj).as_dict().is_some_and(|d| d.get(b"PCAdded").is_some())));
    if let Some(piece) = owned_destination {
        let whole_unit = ops.iter().enumerate().all(|(index, op)| {
            let (first, last) = pieces.pieces_of(op);
            if dest_unit.contains(&index) { first == piece && last == piece } else { piece < first || piece > last }
        });
        if !whole_unit || img.stream == piece {
            return Err(EditError::Invalid("cannot arrange an image inside an added item's content stream".into()));
        }
    }

    let (leaking_ops, has_leaking_clip) = scan_leaking_ops(&ops, rem_start, rem_end);

    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let ext_gstates = res.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).unwrap_or_default();

    let mut state = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let mut tracker = ExtendedGStateTracker::default();
    let mut drawing_tracker = DrawingStateTracker::default();
    let mut source_effective_state = EffectiveGState::default();
    let mut source_gs_ops = Vec::new();
    let mut source_drawing_state = DrawingState::default();
    let mut dest_effective_state = EffectiveGState::default();
    let mut dest_drawing_state = DrawingState::default();
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
            source_gs_ops = tracker.active_gs_ops.iter().map(|(_, op, ctm)| (op.clone(), *ctm)).collect();
            source_drawing_state = drawing_tracker.current.clone();
        }
        if i == scan_target {
            dest_effective_state = tracker.current_state.clone();
            dest_drawing_state = drawing_tracker.current.clone();
            dest_ctm = state.placement.matrix;
            dest_clips = state.placement.clips.clone();
        }
        if i >= max_scan {
            break;
        }
        state.apply(op)?;
        tracker.step(op, &ext_gstates, doc, state.placement.matrix)?;
        drawing_tracker.step(op)?;
        if op.is("gs")
            && op.name(0).and_then(|name| tracker.definitions.get(name)).and_then(|gs| gs.as_dict()).is_some_and(|gs| gs.get(b"Font").is_some())
            && let Some(font) = DRAWING_PARAMS.iter().position(|name| *name == "Tf").and_then(|index| drawing_tracker.current.params.get_mut(index))
        {
            // The replayed gs selects its font; do not overwrite it with an older direct Tf.
            *font = None;
        }
        if op.is("gs")
            && let Some(intent) = &tracker.current_state.rendering_intent
        {
            drawing_tracker.step(&Op::new("ri", vec![Object::Name(intent.clone())]))?;
        }
        if i == rem_end.saturating_sub(1) {
            ctm_after_removal = state.placement.matrix;
        }
    }
    if scan_target >= ops.len() {
        dest_effective_state = tracker.current_state.clone();
        dest_drawing_state = drawing_tracker.current.clone();
        dest_ctm = state.placement.matrix;
        dest_clips = state.placement.clips.clone();
    }
    if dest_ctm.invert().is_none() {
        return Err(EditError::Invalid("unsupported graphics state: non-invertible CTM".into()));
    }

    let owned_crop = ops.get(scope_start..target).unwrap_or_default().iter().any(|op| op.is("MP") && op.name(0) == Some(b"PdfCraftImageCrop"));
    let extracted_scope = if owned_crop { None } else { extract_scope_state(&ops, scope_start, target, scope_start_ctm)? };
    let mut extracted = match extracted_scope {
        Some(ext) => ext,
        None if scope_start != target => {
            if !marked_unit || has_other_paint {
                target_span = target_op.span.clone();
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

    if all_clips {
        for clip in &img.clips {
            let ops = clip_path_to_ops(clip)
                .ok_or_else(|| EditError::Invalid("unsupported graphics state: the image clipping path cannot be cleanly preserved".into()))?;
            extracted.clip_ops.extend(ops);
        }
    }

    let has_inherited_clip = !img.clips.is_empty() && extracted.clip_ops.is_empty();
    if has_inherited_clip {
        return Err(EditError::Invalid("unsupported graphics state: inherited clipping path cannot be cleanly preserved".into()));
    }

    let mut reconstructed_state = dest_effective_state.clone();
    for (op, ctm) in &source_gs_ops {
        if let Some(name) = op.name(0)
            && let Some(definition) = tracker.definitions.get(name)
        {
            reconstructed_state.merge_shared(definition, doc, *ctm);
        }
    }

    if reconstructed_state.visually_differs(&source_effective_state) {
        return Err(EditError::Invalid(
            "unsupported graphics state: moving the image would alter opacity, blend mode, soft mask, or printing parameters".into(),
        ));
    }

    let marked_open = if marked_unit && !has_other_paint { ops.get(target_unit.start) } else { None };
    let has_closing_emc = marked_open.is_some() && target_unit.end.checked_sub(1).and_then(|i| ops.get(i)).is_some_and(|o| o.is("EMC"));
    let inherited_drawing_ops = compute_inherited_drawing_ops(&source_drawing_state, &dest_drawing_state, &extracted.state_ops);
    let unit_ops =
        build_relocated_unit_ops(marked_open, &source_gs_ops, &inherited_drawing_ops, extracted, dest_ctm, img.draw_matrix, has_closing_emc)?;

    let mut serialized = Vec::new();
    serialized.push(b'\n');
    serialized.extend_from_slice(&serialize_ops(&unit_ops));
    serialized.push(b'\n');

    let leaking = if let Some(inv) = ctm_before_removal.invert() { ctm_after_removal.then(&inv) } else { Matrix::IDENTITY };
    if leaking != Matrix::IDENTITY && has_leaking_clip {
        return Err(EditError::Invalid("unsupported graphics state: moving the image would alter clipping path transform".into()));
    }
    let mut leak_ops = leaking_ops;
    if leaking != Matrix::IDENTITY && leak_ops.iter().all(|o| !o.is("cm")) {
        leak_ops.insert(0, Op::new("cm", leaking.0.map(pdfcraft_content::num).to_vec()));
    }
    let removal_replacement = if !leak_ops.is_empty() {
        let mut s = Vec::new();
        s.push(b'\n');
        s.extend_from_slice(&serialize_ops(&leak_ops));
        s.push(b'\n');
        s
    } else {
        Vec::new()
    };

    if let Some(piece) = owned_destination {
        let changed = pieces.splice([(target_span, removal_replacement)]);
        let contents = crate::text::rewritten_contents(doc, all, changed);
        let mut list = match contents {
            Object::Array(list) => list,
            one => vec![one],
        };
        let insertion = piece.saturating_add(usize::from(insert_after));
        if insertion > list.len() {
            return Err(EditError::Invalid("the destination content stream is missing".into()));
        }
        let relocated = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), &serialized)));
        list.insert(insertion, Object::Ref(relocated));
        doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
        return Ok(());
    }

    let mut edits = Vec::new();
    if dest_pos >= target_span.end {
        edits.push((target_span, removal_replacement));
        edits.push((dest_pos..dest_pos, serialized));
    } else if dest_pos <= target_span.start {
        edits.push((dest_pos..dest_pos, serialized));
        edits.push((target_span, removal_replacement));
    } else {
        return Err(EditError::Invalid("cannot arrange image into overlapping content range".into()));
    }

    let changed = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, all, changed);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// The saved graphics-state scopes (`q … Q`) around the `Do` at `target` that paint nothing else,
/// each as `(q, Q, CTM before the q)`.
struct ImageScopes {
    /// The innermost one inside which every clip on the image is set.
    isolated: Option<(usize, usize, Matrix)>,
    /// The outermost one. It holds all of the image's private clips, but not a clip set outside it.
    outermost: Option<(usize, usize, Matrix)>,
    /// The CTM at the `Do`, and whether a clip applies there.
    placement: Matrix,
    clipped: bool,
}

impl ImageScopes {
    /// The saved graphics-state scope containing just this image and its crop, or the `Do` alone
    /// when nothing clips it. Applying a transform before that scope moves the actual clipping path
    /// as well as the pixels. A shared clip must never be moved along with unrelated artwork or text.
    ///
    /// Under a clip set outside every scope that holds only this image (a report or table drawing
    /// each picture as `q re W n cm Do Q` inside a section's clip), no scope contains every active
    /// clip. The outermost scope holding only the image is used then: it contains all of the
    /// image's private clips, which move with it, while the outer clip is shared and stays.
    fn transform_scope(&self, target: usize) -> Option<(usize, usize, Matrix)> {
        if !self.clipped {
            return Some((target, target, self.placement));
        }
        self.isolated.or(self.outermost)
    }
}

/// [`ImageScopes`] of the `Do` at `target`. An image inside a text object cannot be `done`.
fn image_scopes(ops: &[Op], target: usize, done: &str) -> Result<ImageScopes, EditError> {
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
    let mut outermost = None;
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
                    if scope.selected && !scope.other_paint && !scope.inherited_path {
                        let found = (scope.start, index, scope.matrix);
                        if !scope.inherited_clip {
                            isolated.get_or_insert(found);
                        }
                        // Scopes close inner first, so the last one recorded is the outermost.
                        outermost = Some(found);
                    }
                    matrix = scope.matrix;
                    clipped = scope.inherited_clip;
                    path = scope.inherited_path;
                    pending_clip = false;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            b"BT" => text = true,
            b"ET" => text = false,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" => path = true,
            b"W" | b"W*" => pending_clip = true,
            _ => {}
        }
        if index == target {
            if text {
                return Err(EditError::Invalid(format!("an image inside a text object cannot be {done}")));
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
    Ok(ImageScopes { isolated, outermost, placement, clipped })
}

/// A marked-content point identifies only the clipping path written by this editor.
/// It does not open a marked-content sequence or change the document's structure tags.
fn crop_marker() -> Op {
    Op::new("MP", vec![Object::Name(b"PdfCraftImageCrop".to_vec())])
}

fn app_crop_ranges(ops: &[Op], start: usize, target: usize) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    for (index, op) in ops.iter().enumerate().take(target).skip(start) {
        if !op.is("MP") || op.name(0) != Some(b"PdfCraftImageCrop") {
            continue;
        }
        let path_start = index.saturating_add(1);
        let mut end = path_start;
        while end < target && ops.get(end).is_some_and(|o| matches!(o.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")) {
            end = end.saturating_add(1);
        }
        if end > path_start && ops.get(end).is_some_and(|o| o.is("W") || o.is("W*")) && ops.get(end.saturating_add(1)).is_some_and(|o| o.is("n")) {
            ranges.push(index..end.saturating_add(2));
        }
    }
    ranges
}

/// Remove only crops written by PdfCraft, preserving the document author's clipping paths.
fn remove_image_crop(doc: &mut Document, p: &pdfcraft_model::Page, all: &[(Object, Vec<u8>)], img: &PageImage) -> Result<(), EditError> {
    let pieces = joined(all);
    let ops = pieces.parse().ops;
    let target = img.op;
    let scope = image_scopes(&ops, target, "cropped")?;
    let Some((start, _, _)) = scope.transform_scope(target) else {
        return Err(EditError::Invalid("the image has no PdfCraft crop to remove".into()));
    };
    let crops = app_crop_ranges(&ops, start, target);
    if crops.is_empty() {
        return Err(EditError::Invalid("the image has no PdfCraft crop to remove".into()));
    }
    let changed = crate::text::splice(&pieces, &ops, |index| (Vec::new(), !crops.iter().any(|r| r.contains(&index))));
    let contents = crate::text::rewritten_contents(doc, all, changed);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
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
            return arrange_image(doc, &p, &all, &img, *order);
        }
        ImageChange::RemoveCrop => {
            return remove_image_crop(doc, &p, &all, &img);
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
    let (start, end, placement) =
        image_scopes(&ops, target, "transformed")?.transform_scope(target).unwrap_or((target, target, Matrix(image.draw_matrix)));
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
    let (start, end, placement) =
        image_scopes(&ops, target, "cropped")?.transform_scope(target).unwrap_or((target, target, Matrix(image.draw_matrix)));
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
    let mut clip: Vec<Op> = if is_axis_aligned {
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
    clip.insert(0, crop_marker());
    // Replace only our identified crop; an author's path must remain intact.
    let old_crops = app_crop_ranges(&ops, start, target);
    let insert_at = start.saturating_add(1);
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
            (clip.clone(), !old_crops.iter().any(|r| r.contains(&index)))
        } else if old_crops.iter().any(|r| r.contains(&index)) {
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
    let mut crop_starts = std::collections::BTreeMap::new();
    let mut crop_ends = std::collections::BTreeSet::new();
    let mut folds = std::collections::BTreeMap::new();
    for image in images.iter().filter(|image| image.has_editor_crop()) {
        let Some(&(_, _, offset)) = plans.get(&image.op) else { continue };
        let Some((start, end, placement)) = image_scopes(&ops, image.op, "moved")?.transform_scope(image.op) else {
            return Err(EditError::Invalid("the image crop cannot be moved independently".into()));
        };
        let inverse = placement.invert().ok_or_else(|| EditError::Invalid("a selected image has no area".into()))?;
        let local = placement.then(&Matrix::translate(offset[0], offset[1])).then(&inverse);
        if !local.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the image move is too large".into()));
        }
        let leading_transform = || {
            let at = start.checked_add(1)?;
            if start >= end || !ops.get(start)?.is("q") || !ops.get(at)?.is("cm") {
                return None;
            }
            ops.get(at)?.nums::<6>().map(|m| (at, Matrix(m)))
        };
        // An enclosing cm is before the captured placement; an inside-scope cm is after it.
        // Their multiplication orders differ when that cm scales or shears a translation.
        let folded = enclosing_transform(&ops, start, end)
            .map(|(at, cm)| (at, local.then(&cm)))
            .or_else(|| leading_transform().map(|(at, cm)| (at, cm.then(&local))));
        if let Some((at, folded)) = folded {
            if !folded.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
                return Err(EditError::Invalid("the image move is too large".into()));
            }
            folds.insert(at, folded);
        } else {
            crop_starts.insert(start, local);
            crop_ends.insert(end);
        }
        plans.remove(&image.op);
    }
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
        if let Some(matrix) = crop_starts.remove(&index) {
            return (vec![Op::new("q", vec![]), Op::new("cm", matrix.0.map(Object::Real).to_vec())], true);
        }
        if crop_ends.remove(&index) {
            let mut out: Vec<_> = ops.get(index).cloned().into_iter().collect();
            out.push(Op::new("Q", vec![]));
            return (out, false);
        }
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

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[test]
    fn saved_drawing_states_share_large_operand_storage() {
        let mut tracker = DrawingStateTracker::default();
        let parsed = parse(b"[1 2 3 4 5 6 7 8] 0 d /Pattern cs 1 2 3 4 5 6 /P scn q q q");
        for op in &parsed.ops {
            tracker.step(op).unwrap();
        }
        for state in &tracker.stack {
            let dash = state.params[4].as_ref().unwrap();
            let current_dash = tracker.current.params[4].as_ref().unwrap();
            assert!(std::ptr::eq(dash.operands.as_ptr(), current_dash.operands.as_ptr()), "q must share the dash operands instead of cloning them");
            let color = state.non_stroking_color.as_ref().unwrap().last().unwrap();
            let current_color = tracker.current.non_stroking_color.as_ref().unwrap().last().unwrap();
            assert!(std::ptr::eq(color.operands.as_ptr(), current_color.operands.as_ptr()), "q must share scn operands instead of cloning them");
        }
        let saved = tracker.current.clone();
        for op in &parse(b"[9 10] 0 d 4 5 6 /Other scn Q").ops {
            tracker.step(op).unwrap();
        }
        assert_eq!(tracker.current, saved, "changing shared operands must preserve the saved state");
    }

    #[test]
    fn review_soft_mask_identity_includes_establishment_matrix() {
        let doc = Document::new_empty();
        let mask = Dict::from_iter([(b"S".to_vec(), Object::name("Alpha")), (b"G".to_vec(), Object::Ref(ObjRef::new(8, 0)))]);
        let definition = Dict::from_iter([(b"SMask".to_vec(), Object::Dict(mask))]);
        let states = Dict::from_iter([(b"A".to_vec(), Object::Dict(definition))]);
        let mut tracker = ExtendedGStateTracker::default();
        let op = Op::new("gs", vec![Object::name("A")]);
        tracker.step(&op, &states, &doc, Matrix::IDENTITY).unwrap();
        let before = tracker.current_state.clone();
        tracker.step(&op, &states, &doc, Matrix::translate(50.0, 0.0)).unwrap();
        assert!(before.visually_differs(&tracker.current_state), "the same mask established under a different CTM is a different effective mask");
    }
}
