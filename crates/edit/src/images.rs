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

/// The page's `/Resources /Properties` dictionary, resolved when present.
fn properties(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"Properties").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned())))
        .unwrap_or_default()
}

/// Optional-content visibility evaluated against the document's default configuration (`/OCProperties /D`).
#[derive(Default)]
struct DefaultOc {
    ocgs: std::collections::HashSet<ObjRef>,
    hidden: std::collections::HashSet<ObjRef>,
}

impl DefaultOc {
    /// Reads the default optional-content configuration (`/OCProperties /D`) from the catalog.
    fn from_doc(doc: &Document) -> Self {
        let Some(cat) = doc.root().and_then(|r| doc.dict(&Object::Ref(r))) else { return Self::default() };
        let Some(oc) = cat.get(b"OCProperties").and_then(|o| doc.dict(o)) else { return Self::default() };
        let Some(d) = oc.get(b"D").and_then(|o| doc.dict(o)) else { return Self::default() };
        let refs = |dict: &Dict, key: &[u8]| -> std::collections::HashSet<ObjRef> {
            dict.get(key).and_then(|v| doc.resolve(v).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
        };
        let mut ocgs = refs(&oc, b"OCGs");
        let (on, off) = (refs(&d, b"ON"), refs(&d, b"OFF"));
        let base_off = d.get(b"BaseState").map(|b| doc.resolve(b)).and_then(|b| b.as_name().map(|n| n == b"OFF")).unwrap_or(false);
        let mut hidden = std::collections::HashSet::new();
        for &r in &ocgs {
            let is_off = if off.contains(&r) {
                true
            } else if on.contains(&r) {
                false
            } else {
                base_off
            };
            if is_off {
                hidden.insert(r);
            }
        }
        for r in off {
            if ocgs.contains(&r) || doc.get(r).as_dict().is_some_and(|g| g.name(b"Type") == Some(b"OCG")) {
                ocgs.insert(r);
                hidden.insert(r);
            }
        }
        Self { ocgs, hidden }
    }

    /// Reads the default optional-content configuration from `doc` and applies live layer overrides `(num, gen, visible)`.
    fn from_doc_with_layers(doc: &Document, layers: &[(i32, i32, bool)]) -> Self {
        let mut oc = Self::from_doc(doc);
        for &(num, generation, visible) in layers {
            if let (Ok(num), Ok(generation)) = (u32::try_from(num), u16::try_from(generation)) {
                let r = ObjRef::new(num, generation);
                oc.ocgs.insert(r);
                if visible {
                    oc.hidden.remove(&r);
                } else {
                    oc.hidden.insert(r);
                }
            }
        }
        oc
    }

    /// Whether `r` is an optional-content group in the document.
    fn is_ocg(&self, doc: &Document, r: ObjRef) -> bool {
        self.ocgs.contains(&r) || doc.get(r).as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OCG"))
    }

    /// Whether the optional-content group or membership dictionary `obj` is off in the default configuration.
    fn is_hidden(&self, doc: &Document, obj: &Object) -> bool {
        if self.ocgs.is_empty() && self.hidden.is_empty() {
            return false;
        }
        if let Some(r) = obj.as_ref()
            && self.hidden.contains(&r)
        {
            return true;
        }
        let resolved = doc.resolve(obj);
        let Some(d) = resolved.as_dict() else { return false };
        if d.name(b"Type") != Some(b"OCMD") {
            return false;
        }
        if let Some(ve) = d.get(b"VE")
            && let Some(visible) = self.eval_ve(doc, ve, 0, &mut std::collections::HashSet::new(), &mut 4096)
        {
            return !visible;
        }
        let groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).filter(|r| self.is_ocg(doc, *r)).collect(),
            Some(_) => d.get(b"OCGs").and_then(Object::as_ref).filter(|r| self.is_ocg(doc, *r)).into_iter().collect(),
            None => Vec::new(),
        };
        if groups.is_empty() {
            return false;
        }
        let any_on = groups.iter().any(|g| !self.hidden.contains(g));
        let all_on = groups.iter().all(|g| !self.hidden.contains(g));
        let policy = d.get(b"P").map(|p| doc.resolve(p)).and_then(|p| p.as_name().map(<[u8]>::to_vec));
        match policy.as_deref() {
            Some(b"AllOn") => !all_on,
            Some(b"AnyOff") => all_on,
            Some(b"AllOff") => any_on,
            _ => !any_on,
        }
    }

    /// Evaluates a `/VE` visibility expression (ISO 32000-2 §8.11.2.2); returns `None` if malformed.
    fn eval_ve(&self, doc: &Document, expr: &Object, depth: usize, path: &mut std::collections::HashSet<ObjRef>, budget: &mut usize) -> Option<bool> {
        if depth > 40 || *budget == 0 {
            return None;
        }
        *budget -= 1;
        let reference = expr.as_ref();
        if let Some(r) = reference
            && !path.insert(r)
        {
            return None;
        }
        let resolved = doc.resolve(expr);
        let result = match &*resolved {
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => reference.filter(|r| self.ocgs.contains(r)).map(|r| !self.hidden.contains(&r)),
            Object::Array(items) if (2..=4097).contains(&items.len()) => {
                let Some(Object::Name(op)) = items.first() else { return None };
                match op.as_slice() {
                    b"And" => {
                        let mut visible = true;
                        for item in items.iter().skip(1) {
                            visible &= self.eval_ve(doc, item, depth + 1, path, budget)?;
                        }
                        Some(visible)
                    }
                    b"Or" => {
                        let mut visible = false;
                        for item in items.iter().skip(1) {
                            visible |= self.eval_ve(doc, item, depth + 1, path, budget)?;
                        }
                        Some(visible)
                    }
                    b"Not" if items.len() == 2 => Some(!self.eval_ve(doc, items.get(1)?, depth + 1, path, budget)?),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(r) = reference {
            path.remove(&r);
        }
        result
    }

    /// Updates the marked-content visibility stack for `BMC`, `BDC` and `EMC`.
    fn step_marked(&self, doc: &Document, op: &Op, props: &Dict, stack: &mut Vec<bool>) {
        match op.op.as_slice() {
            b"BMC" => stack.push(stack.last().copied().unwrap_or(false)),
            b"BDC" => {
                let parent = stack.last().copied().unwrap_or(false);
                let tag = op.operands.len().checked_sub(2).and_then(|i| op.operands.get(i)).and_then(Object::as_name);
                let this = tag == Some(b"OC".as_slice())
                    && match op.operands.last() {
                        Some(Object::Name(n)) => props.get(n).is_some_and(|o| self.is_hidden(doc, o)),
                        Some(inline) => self.is_hidden(doc, inline),
                        None => false,
                    };
                stack.push(parent || this);
            }
            b"EMC" => {
                stack.pop();
            }
            _ => {}
        }
    }
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
            // ClosePath with no subpath panics by the same contract (#3581).
            b"h" if self.path.elements().last().is_some() => {
                self.path.close_path();
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
    page_images_with_layers(doc, page, &[])
}

/// [`page_images`] evaluated with live layer overrides `(num, gen, visible)` on top of `/OCProperties /D`.
pub fn page_images_with_layers(doc: &Document, page: usize, layers: &[(i32, i32, bool)]) -> Result<Vec<PageImage>, EditError> {
    let p = page_of(doc, page)?;
    let xo = xobjects(doc, &p.dict);
    let props = properties(doc, &p.dict);
    let oc = DefaultOc::from_doc_with_layers(doc, layers);
    let mut oc_stack: Vec<bool> = Vec::new();
    let mut out = Vec::new();
    // The streams are one stream in pieces, so the graphics state and operators carry over.
    let mut state = ImageState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let pieces = joined(&content_streams(doc, &p.dict)?);
    for (i, op) in pieces.parse().ops.iter().enumerate() {
        state.apply(op)?;
        oc.step_marked(doc, op, &props, &mut oc_stack);
        let ctm = state.placement.matrix;
        if op.is("Do") {
            if oc_stack.last().copied().unwrap_or(false) {
                continue;
            }
            let Some(name) = op.name(0) else { continue };
            let Some(resource) = xo.get(name) else { continue };
            let obj = doc.resolve(resource);
            let Object::Stream(s) = &*obj else { continue };
            if s.dict.get(b"OC").is_some_and(|entry| oc.is_hidden(doc, entry)) {
                continue;
            }
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
        oc: DefaultOc,
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
        let props = resources.get(b"Properties").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default();
        let mut oc_stack: Vec<bool> = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            st.apply(op)?;
            forms.oc.step_marked(doc, op, &props, &mut oc_stack);
            let ctm = st.placement.matrix;
            if op.is("Do") {
                if oc_stack.last().copied().unwrap_or(false) {
                    continue;
                }
                let Some(name) = op.name(0) else { continue };
                let Some(resource) = xo.get(name) else { continue };
                let obj = doc.resolve(resource);
                let Object::Stream(s) = &*obj else { continue };
                if s.dict.get(b"OC").is_some_and(|entry| forms.oc.is_hidden(doc, entry)) {
                    continue;
                }
                if let Some(form) = crate::text::form_call(doc, resources, name, ctm, &forms.path, &mut forms.budget) {
                    if let Some(r) = form.obj {
                        forms.path.push(r);
                    }
                    let bounds = s.dict.get(b"BBox").map(|b| doc.resolve(b)).and_then(|b| {
                        let a = b.as_array()?;
                        let [x0, y0, x1, y1] = a.as_slice() else { return None };
                        Some(form.ctm.bbox([x0.as_f64()?, y0.as_f64()?, x1.as_f64()?, y1.as_f64()?]))
                    });
                    let clip = bounds.map_or(st.placement.clip, |b| intersection(st.placement.clip, b).unwrap_or([0.0; 4]));
                    let mut inner = ImageState::new(form.ctm, clip, st.placement.clips.clone());
                    let piece = stream(op);
                    walk(doc, &parse(&form.data).ops, &form.resources, &mut inner, &|_| piece, forms, out)?;
                    if form.obj.is_some() {
                        forms.path.pop();
                    }
                    continue;
                }
                let Some(r) = resource.as_ref() else { continue };
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
    let mut forms = Forms { path: Vec::new(), budget: crate::text::FormBudget::new(decode), oc: DefaultOc::from_doc(doc) };
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
    change_image_with_layers(doc, page, index, change, &[])
}

/// [`change_image`] evaluated with live layer overrides `(num, gen, visible)` on top of `/OCProperties /D`.
pub fn change_image_with_layers(
    doc: &mut Document,
    page: usize,
    index: usize,
    change: &ImageChange,
    layers: &[(i32, i32, bool)],
) -> Result<(), EditError> {
    let images = page_images_with_layers(doc, page, layers)?;
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
    layers: &[(i32, i32, bool)],
) -> Result<Vec<(usize, PageImage)>, EditError> {
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    Ok(page_images_with_layers(doc, page, layers)?
        .into_iter()
        .enumerate()
        .filter(|(_, image)| !all.get(image.stream).and_then(|(o, _)| o.as_ref()).is_some_and(|r| excluded.contains(&r)))
        .collect())
}

/// Find the saved graphics-state scope containing just this image and its crop. Applying
/// the transform before that scope moves the actual clipping path as well as the pixels.
/// A shared clip must never be moved along with unrelated artwork or text.
///
/// Under a clip set outside every scope that holds only this image (a report or table drawing
/// each picture as `q re W n cm Do Q` inside a section's clip), no scope contains every active
/// clip. The outermost scope holding only the image is used then: it contains all of the
/// image's private clips, which move with it, while the outer clip is shared and stays.
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
    let mut outermost = None;
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
                    if scope.selected && !scope.other_paint && !scope.inherited_path {
                        if !scope.inherited_clip {
                            // Prefer the innermost scope that contains every active clipping path.
                            isolated.get_or_insert((scope.start, index, scope.matrix));
                        }
                        // Scopes close inner first, so the last one recorded is the outermost.
                        outermost = Some((scope.start, index, scope.matrix));
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
    Ok(isolated.or(outermost))
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
#[cfg(test)]
pub(crate) fn translate_images(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    translate_images_with_layers(doc, page, indexes, offset, &[])
}

pub(crate) fn translate_images_with_layers(
    doc: &mut Document,
    page: usize,
    indexes: &[usize],
    offset: [f64; 2],
    layers: &[(i32, i32, bool)],
) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let images = page_images_with_layers(doc, page, layers)?;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::objects::{ObjectKind, editable_objects};

    fn build_pdf(objects: &[String]) -> Document {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for off in offsets {
            bytes.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        Document::open(Arc::new(bytes)).expect("valid synthetic PDF")
    }

    fn stream_obj(dict: &str, body: &str) -> String {
        format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len())
    }

    fn layered_image_doc(d_config: &str, bdc_wrap: bool, xobject_oc: &str, extra_obj: Option<&str>) -> Document {
        let draw = if bdc_wrap { "/OC /L1 BDC q 100 0 0 80 100 500 cm /Im0 Do Q EMC" } else { "q 100 0 0 80 100 500 cm /Im0 Do Q" };
        let content = format!("BT /F1 14 Tf 100 700 Td (Visible title) Tj ET\n{draw}");
        let prop_target = if extra_obj.is_some() { "8 0 R" } else { "5 0 R" };
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << {d_config} >> >> >>"),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
                 /Resources << /Font << /F1 6 0 R >> /XObject << /Im0 7 0 R >> /Properties << /L1 {prop_target} >> >> >>"
            ),
            stream_obj("", &content),
            "<< /Type /OCG /Name (Layer 1) >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
            stream_obj(
                &format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 {xobject_oc}"),
                "\x00\x00\x00",
            ),
        ];
        if let Some(extra) = extra_obj {
            objs.push(extra.into());
        }
        build_pdf(&objs)
    }

    #[test]
    fn hidden_layer_images_are_not_listed_hit_or_exported() {
        // 1. Marked-content /OC /L1 BDC … EMC with /L1 OFF: hidden.
        let bdc_off = layered_image_doc("/OFF [5 0 R] /Order [5 0 R]", true, "", None);
        assert!(page_images(&bdc_off, 0).unwrap().is_empty(), "BDC off layer must list no image");
        assert!(reading_images(&bdc_off, 0).unwrap().is_empty(), "BDC off layer must export no image");
        let objs = editable_objects(&bdc_off, 0).unwrap();
        assert!(objs.iter().all(|o| o.target.kind != ObjectKind::Image), "object_list must only have text: {objs:?}");

        // 2. XObject /OC 5 0 R with 5 0 R OFF (and with /BaseState /OFF): hidden.
        for cfg in ["/OFF [5 0 R] /Order [5 0 R]", "/BaseState /OFF /Order [5 0 R]"] {
            let xobj_off = layered_image_doc(cfg, false, "/OC 5 0 R", None);
            assert!(page_images(&xobj_off, 0).unwrap().is_empty(), "XObject /OC off ({cfg}) must list no image");
            assert!(reading_images(&xobj_off, 0).unwrap().is_empty(), "XObject /OC off ({cfg}) must export no image");
            let objs = editable_objects(&xobj_off, 0).unwrap();
            assert!(objs.iter().all(|o| o.target.kind != ObjectKind::Image), "object_list must only have text: {objs:?}");
        }

        // 3. Marked-content /OC /L1 BDC … EMC with /L1 ON: listed and hit.
        let bdc_on = layered_image_doc("/ON [5 0 R] /Order [5 0 R]", true, "", None);
        let imgs = page_images(&bdc_on, 0).unwrap();
        assert_eq!(imgs.len(), 1, "BDC on layer must list the image");
        assert!(imgs[0].contains([150.0, 540.0]), "BDC on layer image must hit inside its box");
        assert_eq!(reading_images(&bdc_on, 0).unwrap().len(), 1);

        // 4. XObject /OC 5 0 R with 5 0 R ON: listed and hit.
        let xobj_on = layered_image_doc("/BaseState /OFF /ON [5 0 R] /Order [5 0 R]", false, "/OC 5 0 R", None);
        let imgs = page_images(&xobj_on, 0).unwrap();
        assert_eq!(imgs.len(), 1, "XObject /OC on layer must list the image");
        assert!(imgs[0].contains([150.0, 540.0]), "XObject /OC on layer image must hit inside its box");
        assert_eq!(reading_images(&xobj_on, 0).unwrap().len(), 1);

        // 5. OCMD membership policies and /VE visibility expressions.
        let ocmd_all_off = layered_image_doc("/ON [5 0 R]", false, "/OC 8 0 R", Some("<< /Type /OCMD /OCGs [5 0 R] /P /AllOff >>"));
        assert!(page_images(&ocmd_all_off, 0).unwrap().is_empty(), "OCMD /AllOff with ON group is hidden");
        let ocmd_ve_not = layered_image_doc("/ON [5 0 R]", true, "", Some("<< /Type /OCMD /OCGs [5 0 R] /VE [/Not 5 0 R] >>"));
        assert!(page_images(&ocmd_ve_not, 0).unwrap().is_empty(), "OCMD /VE [/Not 5 0 R] with ON group is hidden");

        // 6. Editing a visible image after a hidden image still targets the visible image's operator.
        let mut two_imgs = build_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /XObject << /Im0 6 0 R /Im1 7 0 R >> /Properties << /L1 5 0 R >> >> >>"
                .into(),
            stream_obj("", "/OC /L1 BDC q 50 0 0 50 10 10 cm /Im0 Do Q EMC q 60 0 0 40 200 300 cm /Im1 Do Q"),
            "<< /Type /OCG /Name (Hidden) >>".into(),
            stream_obj("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", "\x00\x00\x00"),
            stream_obj("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", "\x7f\x7f\x7f"),
        ]);
        let listed = page_images(&two_imgs, 0).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Im1");
        change_image(&mut two_imgs, 0, 0, &ImageChange::Delete).unwrap();
        assert!(page_images(&two_imgs, 0).unwrap().is_empty(), "deleting index 0 removes the visible image");
        // Revealing the hidden layer via live layer overrides exposes Im0 for listing and editing.
        let revealed = page_images_with_layers(&two_imgs, 0, &[(5, 0, true)]).unwrap();
        assert_eq!(revealed.len(), 1);
        assert_eq!(revealed[0].name, "Im0");
        change_image_with_layers(&mut two_imgs, 0, 0, &ImageChange::Delete, &[(5, 0, true)]).unwrap();
        assert!(page_images_with_layers(&two_imgs, 0, &[(5, 0, true)]).unwrap().is_empty());
    }
}
