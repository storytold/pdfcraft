//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510). Graphics state tracked here is deliberately small: the CTM stack (`q`/`Q`/`cm`)
//! and, from ExtGState (`gs`), the non-stroking alpha (`/ca`) and the blend mode (`/BM`): a fill
//! hides what is under it only when it is opaque and in Normal (or Compatible) blend mode, so a
//! translucent or Multiply highlight is not collected. The fill colour does not matter: an
//! opaque fill hides the text whatever its colour. Form XObjects are followed read-only with a
//! depth cap, and so are the normal appearance streams of the page's visible annotations, placed
//! by `/Rect`, `/BBox` and `/Matrix` and painted after the page content. Anything fancier (other
//! filled paths, shadings, patterns) is out of scope and documented as such.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a `gs`) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{Dict, Document, Object};

use crate::AuditError;

/// Maximum form-XObject nesting followed. Deeper nesting (including a cycle) fails the audit: a
/// cover drawn below the cut could otherwise go unseen and the page be reported clean.
const MAX_DEPTH: usize = 32;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
/// Hitting it fails the audit: a truncated walk must never report "clean".
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle, in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// Paint position: `(content-stream index, operator index in the joined stream)`.
    /// Rectangles inside a form XObject carry the `Do` operator's position; those painted by
    /// an annotation's appearance carry `(usize::MAX, annotation index)`, after every page
    /// operator.
    pub pos: (usize, usize),
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    /// The non-stroking alpha (`/ca`).
    alpha: f64,
    /// Whether the blend mode (`/BM`) is Normal or Compatible: the only modes whose opaque
    /// fills hide what is under them.
    blend_normal: bool,
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
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, alpha: 1.0, blend_normal: true }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    /// Walk one operator slice with the given graphics state. `at_do` is the `Do` position to
    /// stamp on rectangles found inside a form XObject, or `None` at page level (where each
    /// fill operator's own position is used).
    fn walk_ops(
        &mut self,
        ops: &[Op],
        gs: &mut Gs,
        stack: &mut Vec<Gs>,
        resources: &Dict,
        depth: usize,
        at_do: Option<(usize, usize)>,
    ) -> Result<(), AuditError> {
        let mut path = Path::Empty;
        for (i, op) in ops.iter().enumerate() {
            self.bump()?;
            // Paint position of this operator: its own `(piece, joined index)` at page level,
            // the enclosing `Do`'s position inside a form XObject.
            let pos = match at_do {
                Some(p) => p,
                None => (self.pieces.pieces_of(op).0, i),
            };
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        *gs = g;
                    }
                    path = Path::Empty;
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
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        apply_ext_gstate(self.doc, resources, name, gs);
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
                    // Only an opaque, normally blended fill hides what is under it.
                    if let Some(r) = std::mem::replace(&mut path, Path::Empty).rect()
                        && gs.alpha >= 1.0
                        && gs.blend_normal
                    {
                        self.out.push(FilledRect { rect: r, pos });
                    }
                }
                b"S" | b"s" | b"n" => path = Path::Empty,
                b"c" | b"v" | b"y" | b"W" | b"W*" => path = Path::Other,
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        if depth >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        self.walk_xobject(name, gs, resources, depth, pos)?;
                    }
                    path = Path::Empty;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out; every rectangle found is stamped with the `Do`
    /// position `pos`.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let Some(entry) = xobjects.get(name) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        if stream.dict.name(b"Subtype") != Some(b"Form") {
            return Ok(());
        }
        self.walk_form(stream, gs, depth, pos)
    }

    /// Walk a form stream's content with `gs` as the state outside it: its `/Matrix` maps form
    /// space into that state's user space (ISO 32000-2 §8.10.1). `depth` is the nesting of the
    /// form being entered; every rectangle found is stamped with `pos`.
    fn walk_form(&mut self, stream: &pdfcraft_cos::Stream, gs: &Gs, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        let dict = &stream.dict;
        let matrix = numbers::<6>(self.doc, dict.get(b"Matrix")).map_or(Matrix::IDENTITY, Matrix);
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = stream.decoded_strict()?;
        let ops = parse(&data).ops;
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        let mut sub_stack = Vec::new();
        self.walk_ops(&ops, &mut sub_gs, &mut sub_stack, &sub_resources, depth + 1, Some(pos))
    }

    /// Walk the normal appearance a viewer paints for the `k`-th annotation of the page, placed
    /// on its `/Rect` by [`placement`]. The appearance is painted after all page content and is
    /// clipped to its box, so every rectangle it paints is cut to `/Rect` and stamped with a
    /// position past every page operator. An annotation the audit does not walk ([`is_painted`])
    /// or with no appearance to show contributes nothing; an appearance that cannot be decoded
    /// fails the audit like page content does.
    fn walk_annotation(&mut self, annot: &Dict, k: usize) -> Result<(), AuditError> {
        if !is_painted(self.doc, annot) {
            return Ok(());
        }
        let Some(on_page) = rect(self.doc, annot.get(b"Rect")) else { return Ok(()) };
        let Some(appearance) = self.normal_appearance(annot) else { return Ok(()) };
        let Object::Stream(stream) = &*appearance else { return Ok(()) };
        let matrix = numbers::<6>(self.doc, stream.dict.get(b"Matrix")).map_or(Matrix::IDENTITY, Matrix);
        let gs = Gs { ctm: placement(matrix, rect(self.doc, stream.dict.get(b"BBox")), on_page), ..Gs::default() };
        let from = self.out.len();
        self.walk_form(stream, &gs, 0, annotation_pos(k))?;
        // The appearance shows only inside its box, which the placement maps onto `/Rect`.
        self.clip_since(from, on_page);
        Ok(())
    }

    /// Cut every rectangle collected since `from` to `clip`, dropping those left with no area.
    /// (`from` is never past the end, since walks only append; past it, nothing is cut.)
    fn clip_since(&mut self, from: usize, clip: [f64; 4]) {
        let painted = self.out.split_off(from.min(self.out.len()));
        self.out.extend(painted.into_iter().filter_map(|mut f| {
            let r = f.rect;
            f.rect = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
            (f.rect[2] > f.rect[0] && f.rect[3] > f.rect[1]).then_some(f)
        }));
    }

    /// The normal appearance stream a viewer shows for `annot`: `/AP /N`, either a stream or a
    /// dictionary of states from which `/AS` (or the only state) picks one. `None` when there is
    /// nothing a viewer would paint.
    fn normal_appearance(&self, annot: &Dict) -> Option<std::sync::Arc<Object>> {
        let ap = annot.get(b"AP").map(|o| self.doc.resolve(o))?;
        let n = ap.as_dict()?.get(b"N").map(|o| self.doc.resolve(o))?;
        match &*n {
            Object::Stream(_) => Some(n),
            Object::Dict(states) => {
                let state = match annot.name(b"AS") {
                    Some(name) => states.get(name)?,
                    None if states.len() == 1 => states.iter().next().map(|(_, o)| o)?,
                    None => return None,
                };
                let resolved = self.doc.resolve(state);
                matches!(&*resolved, Object::Stream(_)).then_some(resolved)
            }
            _ => None,
        }
    }
}

/// Paint position of the `k`-th annotation's appearance: after every page operator (no content
/// stream has index `usize::MAX`).
fn annotation_pos(k: usize) -> (usize, usize) {
    (usize::MAX, k)
}

/// Whether the audit walks `annot`'s appearance as paint over the page: not when a viewer does
/// not show it (the Hidden or NoView flag, or a popup), and not for a `/Redact` mark, which is
/// reported as an unapplied mark rather than as a cover.
fn is_painted(doc: &Document, annot: &Dict) -> bool {
    const HIDDEN: i64 = 2;
    const NO_VIEW: i64 = 32;
    let flags = annot.get(b"F").map(|o| doc.resolve(o)).and_then(|o| o.as_int()).unwrap_or(0);
    flags & (HIDDEN | NO_VIEW) == 0 && !matches!(annot.name(b"Subtype"), Some(b"Popup" | b"Redact"))
}

/// The matrix that places an annotation's appearance on the page (ISO 32000-2 §12.5.5,
/// Algorithm 8.1): the form's `bbox` goes through its `matrix`, and the box that comes out is
/// scaled and moved onto `rect`. A box with no extent in a direction is not scaled in it.
/// Without a `bbox` the form has no box to map: its content is taken as drawn in page space.
fn placement(matrix: Matrix, bbox: Option<[f64; 4]>, rect: [f64; 4]) -> Matrix {
    let Some(bbox) = bbox else { return Matrix::IDENTITY };
    let b = matrix.bbox(bbox);
    let scale = |from: f64, to: f64| if from > 0.0 && from.is_finite() { to / from } else { 1.0 };
    let (sx, sy) = (scale(b[2] - b[0], rect[2] - rect[0]), scale(b[3] - b[1], rect[3] - rect[1]));
    Matrix([sx, 0.0, 0.0, sy, rect[0] - b[0] * sx, rect[1] - b[1] * sy])
}

/// A rectangle array (`/Rect`, `/BBox`) as `[x0, y0, x1, y1]` with `x0 <= x1` and `y0 <= y1`:
/// four finite numbers, as [`numbers`] reads them, or `None`.
pub(super) fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let r = numbers::<4>(doc, o)?;
    Some([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])])
}

/// `N` finite numbers from an array object (`/Rect`, `/BBox`, `/Matrix`), or `None`.
fn numbers<const N: usize>(doc: &Document, o: Option<&Object>) -> Option<[f64; N]> {
    let resolved = doc.resolve(o?);
    let v: Vec<f64> = resolved.as_array()?.iter().filter_map(|o| doc.resolve(o).as_f64()).collect();
    let a = <[f64; N]>::try_from(v).ok()?;
    a.iter().all(|x| x.is_finite()).then_some(a)
}

/// The `x y` operands of `m`/`l`, mapped to user space.
fn user_point(gs: &Gs, op: &Op) -> Option<[f64; 2]> {
    let [x, y] = op.nums::<2>().filter(|v| v.iter().all(|n| n.is_finite()))?;
    let (x, y) = gs.ctm.apply(x, y);
    Some([x, y])
}

/// Apply the named ExtGState's non-stroking alpha (`/ca`) and blend mode (`/BM`, a name or an
/// array of names of which the first counts) to `gs`. An entry the dictionary does not have
/// leaves the parameter as it is (ISO 32000-2 §8.4.5), and so does an unknown resource.
fn apply_ext_gstate(doc: &Document, resources: &Dict, name: &[u8], gs: &mut Gs) {
    let Some(ext) = resources
        .get(b"ExtGState")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(name).map(|o| doc.resolve(o)))
        .and_then(|o| o.as_dict().cloned())
    else {
        return;
    };
    if let Some(alpha) = ext.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()).filter(|a| a.is_finite()) {
        gs.alpha = alpha.clamp(0.0, 1.0);
    }
    if let Some(bm) = ext.get(b"BM").map(|o| doc.resolve(o)) {
        let mode = match &*bm {
            Object::Name(n) => Some(n.as_ref()),
            Object::Array(a) => a.first().and_then(Object::as_name),
            _ => None,
        };
        if let Some(mode) = mode {
            gs.blend_normal = matches!(mode, b"Normal" | b"Compatible");
        }
    }
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects), then by the
/// normal appearances of its annotations `annots` (in that order, with `/Redact` marks left
/// out: they are reported as unapplied marks), with paint positions. A page's content streams
/// are walked once, joined, with one graphics state, so state set in an earlier stream is still
/// in force in the later ones. Any content or appearance stream that cannot be decoded is an
/// audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict, annots: &[Dict]) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut streams = Vec::with_capacity(contents.len());
    for c in &contents {
        if let Object::Stream(s) = &*doc.resolve(c) {
            streams.push(s.decoded_strict()?);
        }
    }
    let pieces = Pieces::join(&streams);
    let parsed = pieces.parse();
    let mut w = Walker { doc, pieces: &pieces, out: Vec::new(), ops_seen: 0 };
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    w.walk_ops(&parsed.ops, &mut gs, &mut stack, &resources, 0, None)?;
    for (k, annot) in annots.iter().enumerate() {
        w.walk_annotation(annot, k)?;
    }
    Ok(w.out)
}
