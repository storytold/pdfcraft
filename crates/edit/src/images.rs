//! Edit a PDF ▸ edit existing images and grouped artwork (Image/Form XObjects painted by `Do`
//! in its own content streams), and changing one: move/resize/rotate/flip (the `Do` is wrapped in
//! `q … cm … Q` with the extra transform), replace (it draws another XObject in the same place),
//! or delete. Only the stream that draws it is rewritten, as a new object.

use kurbo::{BezPath, Point, Shape};
use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use std::sync::Arc;

use crate::EditError;
use crate::text::content_streams;

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
    stream: usize,
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
    pending_clip: Option<bool>,
}

impl ImageState {
    fn new(matrix: Matrix, clip: [f64; 4], clips: Vec<ClipPath>) -> Self {
        Self { placement: Placement { matrix, clip, clips }, stack: Vec::new(), path: BezPath::new(), pending_clip: None }
    }

    fn point(&self, x: f64, y: f64) -> Point {
        let (x, y) = self.placement.matrix.apply(x, y);
        Point::new(x, y)
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
                    self.placement.matrix = Matrix(m).then(&self.placement.matrix);
                }
            }
            b"m" => {
                if let Some([x, y]) = op.nums::<2>() {
                    self.path.move_to(self.point(x, y));
                }
            }
            b"l" => {
                // A segment before any m (or re) has no subpath to extend: BezPath's line_to and
                // friends panic on an empty path by contract, so the segment is skipped, as the
                // renderer, MuPDF and pdftotext do (#3581).
                if let Some([x, y]) = op.nums::<2>()
                    && self.path.elements().last().is_some()
                {
                    self.path.line_to(self.point(x, y));
                }
            }
            b"c" => {
                if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>()
                    && self.path.elements().last().is_some()
                {
                    self.path.curve_to(self.point(x1, y1), self.point(x2, y2), self.point(x3, y3));
                }
            }
            b"v" => {
                if let Some([x1, y1, x2, y2]) = op.nums::<4>()
                    && let Some(current) = self.path.elements().last().and_then(|e| e.end_point())
                {
                    self.path.curve_to(current, self.point(x1, y1), self.point(x2, y2));
                }
            }
            b"y" => {
                if let Some([x1, y1, x2, y2]) = op.nums::<4>()
                    && self.path.elements().last().is_some()
                {
                    let end = self.point(x2, y2);
                    self.path.curve_to(self.point(x1, y1), end, end);
                }
            }
            b"h" => {
                // ClosePath with no subpath panics by the same contract (#3581).
                if self.path.elements().last().is_some() {
                    self.path.close_path();
                }
            }
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    self.path.move_to(self.point(x, y));
                    self.path.line_to(self.point(x + w, y));
                    self.path.line_to(self.point(x + w, y + h));
                    self.path.line_to(self.point(x, y + h));
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
        ImageChange::Crop(keep) => {
            return crop_image(doc, &p, &all, &img, *keep);
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
fn crop_image(
    doc: &mut Document,
    page: &pdfcraft_model::Page,
    streams: &[(Object, Vec<u8>)],
    image: &PageImage,
    keep: [f64; 4],
) -> Result<(), EditError> {
    if !keep.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
        return Err(EditError::Invalid("the crop box must be finite and no larger than 1e12".into()));
    }
    let [x0, y0, x1, y1] = [keep[0].min(keep[2]), keep[1].min(keep[3]), keep[0].max(keep[2]), keep[1].max(keep[3])];
    if x1 - x0 < 1e-6 || y1 - y0 < 1e-6 {
        return Err(EditError::Invalid("the crop box has no area".into()));
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
    let clip: Vec<Op> = corners
        .iter()
        .enumerate()
        .map(|(i, (x, y))| Op::new(if i == 0 { "m" } else { "l" }, vec![Object::Real(*x), Object::Real(*y)]))
        .chain([Op::new("h", vec![]), Op::new("W", vec![]), Op::new("n", vec![])])
        .collect();
    // The image's own earlier crop (a clipping path right after the scope's `q`) gives way to
    // the new one, so a crop can grow again as well as shrink.
    let old_clip: Option<std::ops::Range<usize>> = (start != end)
        .then(|| {
            let mut k = start + 1;
            while ops.get(k).is_some_and(|o| matches!(o.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")) {
                k += 1;
            }
            (k > start + 1 && ops.get(k).is_some_and(|o| matches!(o.op.as_slice(), b"W" | b"W*")) && ops.get(k + 1).is_some_and(|o| o.is("n")))
                .then_some(start + 1..k + 2)
        })
        .flatten();
    let changed = crate::text::splice(&pieces, &ops, |index| {
        if start == end && index == target {
            // No scope of its own yet: one around the drawing, with the crop inside.
            let mut out = vec![Op::new("q", vec![])];
            out.extend(clip.iter().cloned());
            out.extend(ops.get(index).cloned());
            out.push(Op::new("Q", vec![]));
            (out, false)
        } else if start != end && index == start + 1 {
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
