//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510). A single `m`/`l` segment along an axis, stroked (`S`) wide enough to blot the text
//! out ("marker" style), counts as the rectangle the stroke paints (#2385). Graphics state tracked
//! here is deliberately small: the CTM stack (`q`/`Q`/`cm`), the colors (`g`/`rg`/`k` and
//! `G`/`RG`/`K`), the line width (`w`) and what ExtGState sets of them (`gs` reading `/ca`,
//! `/CA` and `/LW`), rectangular and glyph clipping, form bounds, and default-off
//! optional-content groups. Rotated rectangles are conservatively bounded in user space.
//! If only the source of a glyph clip is known, the audit excludes fills of that text object
//! from its own cover findings and retains potential covers on independently drawn text.
//! Form XObjects are followed read-only with a depth and decode budget. Other filled paths,
//! slanted or bent strokes, shadings and patterns remain out of scope.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use std::collections::HashSet;
use std::ops::Range;

use pdfcraft_content::{Matrix, Op, Pieces, ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

use crate::AuditError;

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
    /// Page operators of the text object whose own glyphs this paint fills.
    pub glyph_source: Option<GlyphSource>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphSource {
    pub operators: Range<usize>,
    pub painted: bool,
}

#[derive(Clone, Copy)]
enum Clip {
    Unbounded,
    Rect([f64; 4]),
    Empty,
}

impl Clip {
    fn visible_rect(self, rect: [f64; 4]) -> Option<[f64; 4]> {
        match self {
            Self::Unbounded => Some(rect),
            Self::Rect(clip) => {
                let intersection = [rect[0].max(clip[0]), rect[1].max(clip[1]), rect[2].min(clip[2]), rect[3].min(clip[3])];
                (intersection[0] < intersection[2] && intersection[1] < intersection[3]).then_some(intersection)
            }
            Self::Empty => None,
        }
    }

    fn intersect_rect(&mut self, rect: [f64; 4]) {
        *self = self.visible_rect(rect).map_or(Self::Empty, Self::Rect);
    }
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    alpha: f64,
    stroke: [f64; 3],
    stroke_alpha: f64,
    clip: Clip,
    text_render_mode: u8,
    glyph_source: Option<GlyphSource>,
    /// Line width, in the space the path is drawn in.
    width: f64,
    /// Line cap (`J`, `/LC`): 0 butt, 1 round, 2 projecting square.
    cap: i64,
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
    fn rect(&self) -> Option<[f64; 4]> {
        match self {
            Path::Rect(r) => Some(*r),
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
fn lines_rect(pts: &[[f64; 2]]) -> Option<[f64; 4]> {
    // CTM products leave rounding noise, so corners compare with a tolerance (in points).
    const EPS: f64 = 1e-6;
    let same = |a: f64, b: f64| (a - b).abs() <= EPS;
    let pts = if pts.len() == 5 && same(pts[0][0], pts[4][0]) && same(pts[0][1], pts[4][1]) { &pts[..4] } else { pts };
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
        Gs {
            ctm: Matrix::IDENTITY,
            fill: [0.0, 0.0, 0.0],
            alpha: 1.0,
            stroke: [0.0, 0.0, 0.0],
            stroke_alpha: 1.0,
            clip: Clip::Unbounded,
            text_render_mode: 0,
            glyph_source: None,
            width: 1.0,
            cap: 0,
        }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
    /// Decoded bytes and form entries left for the page, its forms included: spending either
    /// fails the audit.
    budget: DecodeBudget,
    off_layers: HashSet<ObjRef>,
    blank_clips: &'a HashSet<usize>,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    fn push_cover(&mut self, rect: Option<[f64; 4]>, gs: &Gs, stroke: bool, pos: (usize, usize)) {
        let (alpha, rgb) = if stroke { (gs.stroke_alpha, gs.stroke) } else { (gs.alpha, gs.fill) };
        if alpha >= 1.0
            && let Some(rect) = rect.and_then(|rect| gs.clip.visible_rect(rect))
        {
            self.out.push(FilledRect { rect, rgb, pos, glyph_source: gs.glyph_source.clone() });
        }
    }

    fn paint_path(&mut self, path: &Path, operator: &[u8], gs: &Gs, pos: (usize, usize)) {
        if matches!(operator, b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s") {
            self.push_cover(path.stroked_rect(gs), gs, true, pos);
        }
        if !matches!(operator, b"S" | b"s" | b"n") {
            self.push_cover(path.rect(), gs, false, pos);
        }
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
        let mut path = Path::Empty;
        let mut multiple_subpaths = false;
        let mut clip_next = false;
        let mut glyph_clip_pending = false;
        let mut glyph_only_text = false;
        let mut glyph_source_painted = false;
        let mut text_start = 0;
        let mut hidden_depth = 0usize;
        for (i, op) in ops.enumerate() {
            self.bump()?;
            let op = &op;
            // Paint position of this operator: its own `(piece, joined index)` at page level,
            // the enclosing `Do`'s position inside a form XObject.
            let pos = match at_do {
                Some(p) => p,
                None => (self.pieces.pieces_of(op).0, i),
            };
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
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
                    }
                }
                b"re" => {
                    multiple_subpaths |= !matches!(path, Path::Empty);
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
                    multiple_subpaths |= !matches!(path, Path::Empty);
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
                b"W" | b"W*" => clip_next = true,
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"n" => {
                    let done = std::mem::replace(&mut path, Path::Empty);
                    if hidden_depth == 0 {
                        self.paint_path(&done, op.op.as_slice(), gs, pos);
                    }
                    // A clipping operator takes effect after the path-ending paint.
                    if clip_next
                        && !multiple_subpaths
                        && let Some(rect) = done.rect()
                    {
                        gs.clip.intersect_rect(rect);
                    }
                    clip_next = false;
                    multiple_subpaths = false;
                }
                b"c" | b"v" | b"y" => path = Path::Other,
                b"BT" => {
                    glyph_clip_pending = false;
                    glyph_only_text = true;
                    glyph_source_painted = false;
                    text_start = i;
                }
                b"Tr" => {
                    if let Some(mode) = op.num(0).filter(|v| (0.0..=7.0).contains(v) && v.fract() == 0.0) {
                        gs.text_render_mode = mode as u8;
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" if shows_glyphs(op) => {
                    let clips = gs.text_render_mode >= 4;
                    glyph_clip_pending |= clips && (at_do.is_some() || !self.blank_clips.contains(&i));
                    glyph_only_text &= clips;
                    glyph_source_painted |= hidden_depth == 0
                        && ((matches!(gs.text_render_mode, 4 | 6) && gs.alpha > 0.0)
                            || (matches!(gs.text_render_mode, 5 | 6) && gs.stroke_alpha > 0.0));
                }
                b"ET" => {
                    if std::mem::take(&mut glyph_clip_pending) {
                        // Without glyph outlines, retain the rectangular bound as a conservative
                        // estimate. Paint through the clip can still cover independently drawn text.
                        gs.glyph_source =
                            (glyph_only_text && at_do.is_none()).then_some(GlyphSource { operators: text_start..i, painted: glyph_source_painted });
                    }
                }
                b"BDC" => {
                    if hidden_depth > 0 || (op.name(0) == Some(b"OC") && self.layer_is_off(resources, op.operands.get(1))) {
                        hidden_depth += 1;
                    }
                }
                b"BMC" if hidden_depth > 0 => hidden_depth += 1,
                b"EMC" => hidden_depth = hidden_depth.saturating_sub(1),
                b"Do" => {
                    if let Some(name) = op.name(0).filter(|_| hidden_depth == 0) {
                        if depth >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        self.walk_xobject(name, gs, resources, depth, pos)?;
                    }
                    path = Path::Empty;
                    clip_next = false;
                    multiple_subpaths = false;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn layer_is_off(&self, resources: &Dict, property: Option<&Object>) -> bool {
        let Object::Name(name) = property.unwrap_or(&Object::Null) else {
            return matches!(property, Some(Object::Ref(id)) if self.off_layers.contains(id));
        };
        let props = resources.get(b"Properties").map(|value| self.doc.resolve(value));
        let layer = props.as_deref().and_then(Object::as_dict).and_then(|dict| dict.get(name));
        matches!(layer, Some(Object::Ref(id)) if self.off_layers.contains(id))
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out; every rectangle found is stamped with the `Do`
    /// position `pos`.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        // Borrowed, not copied: a `Do` must not cost the size of the whole XObject table.
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o));
        let Some(entry) = xobjects.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        let dict = &stream.dict;
        if crate::subtype(self.doc, dict)?.as_deref() != Some(b"Form") {
            return Ok(());
        }
        if matches!(dict.get(b"OC"), Some(Object::Ref(id)) if self.off_layers.contains(id)) {
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
        let bbox = dict.get(b"BBox").map(|value| self.doc.resolve(value));
        if let Some([x0, y0, x1, y1]) =
            bbox.as_deref().and_then(Object::as_array).and_then(|operands| <&[Object; 4]>::try_from(operands.as_slice()).ok()).and_then(|values| {
                let [x0, y0, x1, y1] = values.each_ref().map(|value| self.doc.resolve(value).as_f64());
                Some([x0?, y0?, x1?, y1?]).filter(|values| values.iter().all(|v| v.is_finite()))
            })
        {
            let bounds = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
            let bbox = sub_gs.ctm.bbox(bounds);
            if bbox.iter().all(|v| v.is_finite()) {
                sub_gs.clip.intersect_rect(bbox);
            }
        }
        let mut sub_stack = Vec::new();
        self.walk_ops(ops(&data), &mut sub_gs, &mut sub_stack, sub_resources, depth + 1, Some(pos))
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

/// Empty shows and TJ positioning adjustments contribute no outline to a text clip.
fn shows_glyphs(op: &Op) -> bool {
    let nonempty = |value: &Object| matches!(value, Object::String(text) if !text.bytes.is_empty());
    match op.operands.last() {
        Some(Object::Array(items)) if op.is("TJ") => items.iter().any(nonempty),
        Some(value) if !op.is("TJ") => nonempty(value),
        _ => false,
    }
}

/// Only explicit groups in `/D /OFF` suppress covers. `/BaseState`, `/ON` and
/// optional-content membership dictionaries stay visible to the audit.
fn off_layers(doc: &Document) -> HashSet<ObjRef> {
    let Some(root) = doc.trailer().get(b"Root").map(|value| doc.resolve(value)) else { return HashSet::new() };
    let props = root.as_dict().and_then(|dict| dict.get(b"OCProperties")).map(|value| doc.resolve(value));
    let configuration = props.as_deref().and_then(Object::as_dict).and_then(|dict| dict.get(b"D")).map(|value| doc.resolve(value));
    let off = configuration.as_deref().and_then(Object::as_dict).and_then(|dict| dict.get(b"OFF")).map(|value| doc.resolve(value));
    off.as_deref()
        .and_then(Object::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| if let Object::Ref(id) = value { Some(*id) } else { None })
        .collect()
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects), with paint
/// positions. A page's content streams are walked once, joined, with one graphics state, so
/// state set in an earlier stream is still in force in the later ones. Any content stream that
/// cannot be decoded, or content and forms that decode or enter past one page's [`DecodeBudget`]
/// in all, is an audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict, blank_clips: &HashSet<usize>) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut budget = DecodeBudget::page();
    let mut streams = Vec::with_capacity(contents.len());
    for c in &contents {
        if let Object::Stream(s) = &*doc.resolve(c) {
            streams.push(budget.decode_strict(doc, s)?);
        }
    }
    let pieces = Pieces::join(&streams);
    let mut w = Walker { doc, pieces: &pieces, out: Vec::new(), ops_seen: 0, budget, off_layers: off_layers(doc), blank_clips };
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    // Read one operator at a time: the joined stream can hold millions of them within the
    // decode budget, and the walk only ever needs the current one.
    w.walk_ops(ops(pieces.data()), &mut gs, &mut stack, &resources, 0, None)?;
    Ok(w.out)
}
