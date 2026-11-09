//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A closed `m`/`l` subpath whose
//! four corners form an axis-aligned rectangle in user space is the same shape and counts too
//! (#1510). A fill paints every subpath of its path, so each rectangular subpath counts, with
//! the holes the even-odd rule or opposite windings cut out of overlapping ones; other subpaths
//! in the same path are not read and cut no holes. Operands are taken from the end of the
//! operand list, as the renderer takes them. Graphics state tracked here is deliberately small:
//! the CTM stack (`q`/`Q`/`cm`), the non-stroking color (`g`/`rg`/`k`) and the non-stroking alpha from ExtGState (`gs` reading
//! `/ca`). Form XObjects are followed read-only with a depth cap. Anything fancier (other filled paths, shadings, patterns) is
//! out of scope and documented as such.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};

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
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
    /// Paint position: `(content-stream index, operator index in the joined stream)`.
    /// Rectangles inside a form XObject carry the `Do` operator's position.
    pub pos: (usize, usize),
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    alpha: f64,
}

/// Most subpaths in one path whose painted region is worked out exactly when the fill rule can
/// leave holes (even-odd, or windings that disagree). Past it every rectangular subpath counts
/// as painted: the audit may then flag text in a hole, but never misses a cover.
const MAX_EXACT_SUBPATHS: usize = 32;
/// Bound on the grid cells times subpaths [`painted`] examines per page.
const MAX_PATH_WORK: usize = 100_000_000;

/// The path under construction, as far as the audit cares about it: every closed rectangular
/// subpath it holds, so a fill paints all of them (ISO 32000-2 §8.5.3.3), not just the last.
#[derive(Default)]
struct Path {
    /// Rectangular subpaths so far, in user space, each with its winding direction (+1
    /// counterclockwise, -1 clockwise).
    rects: Vec<([f64; 4], i8)>,
    /// The `m`/`l` subpath being built, corners already mapped to user space; `true` once closed
    /// by `h`. `None` when there is none, or when it can no longer be a rectangle.
    lines: Option<(Vec<[f64; 2]>, bool)>,
    /// A CTM change mid-path, or a clip: the path is not read at all.
    unreadable: bool,
    /// A `re` drawn under a CTM that is not axis-aligned: it is kept as its bounding box, so a
    /// hole cut from that box can reach past the real inner shape.
    skewed: bool,
}

impl Path {
    fn is_empty(&self) -> bool {
        self.rects.is_empty() && self.lines.is_none() && !self.unreadable
    }

    /// Ends the `m`/`l` subpath being built, keeping it when it is a rectangle.
    fn close_lines(&mut self) {
        if let Some((pts, _)) = self.lines.take()
            && let Some(r) = lines_rect(pts)
        {
            self.rects.push(r);
        }
    }

    /// The rectangular subpaths a fill of this path paints, in user space.
    fn finish(mut self) -> Vec<([f64; 4], i8)> {
        self.close_lines();
        if self.unreadable { Vec::new() } else { self.rects }
    }
}

/// The bounding box of a closed polygon that is an axis-aligned rectangle, with its winding
/// direction, or `None`. A repeated start point is dropped first; the remaining four corners
/// must alternate horizontal and vertical edges.
fn lines_rect(mut pts: Vec<[f64; 2]>) -> Option<([f64; 4], i8)> {
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
    // Shoelace: twice the signed area, positive when the corners run counterclockwise.
    let twice_area: f64 = [(a, b), (b, c), (c, d), (d, a)].iter().map(|(p, q)| p[0] * q[1] - q[0] * p[1]).sum();
    let winding = if twice_area < 0.0 { -1 } else { 1 };
    (r[2] - r[0] > EPS && r[3] - r[1] > EPS).then_some((r, winding))
}

/// The region a fill paints from rectangular subpaths `rects`, as rectangles. Under the nonzero
/// rule, subpaths that all wind the same way paint their union, so each is painted whole.
/// Otherwise overlaps can cancel (an even-odd frame is an outer rectangle with an inner hole), so
/// the region is worked out on the grid the rectangles' edges make, charging the cells examined
/// to `work`.
pub(crate) fn painted(rects: Vec<([f64; 4], i8)>, even_odd: bool, work: &mut usize) -> Result<Vec<[f64; 4]>, AuditError> {
    let same_winding = rects.windows(2).all(|w| w[0].1 == w[1].1);
    if rects.len() <= 1 || rects.len() > MAX_EXACT_SUBPATHS {
        return Ok(rects.into_iter().map(|(r, _)| r).collect());
    }
    if !even_odd && same_winding {
        // The union: a rectangle inside another (or the same one again) adds nothing to it, and
        // would only report the same cover twice.
        let inside = |a: &[f64; 4], b: &[f64; 4]| b[0] <= a[0] && b[1] <= a[1] && a[2] <= b[2] && a[3] <= b[3];
        let kept = rects
            .iter()
            .enumerate()
            .filter(|(i, (r, _))| !rects.iter().enumerate().any(|(j, (o, _))| j != *i && inside(r, o) && (!inside(o, r) || j < *i)));
        return Ok(kept.map(|(_, (r, _))| *r).collect());
    }
    let edges = |lo: usize, hi: usize| {
        let mut v: Vec<f64> = rects.iter().flat_map(|(r, _)| [r[lo], r[hi]]).collect();
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    };
    let (xs, ys) = (edges(0, 2), edges(1, 3));
    let cells = xs.len().saturating_sub(1).saturating_mul(ys.len().saturating_sub(1));
    *work = work.saturating_add(cells.saturating_mul(rects.len()));
    if *work > MAX_PATH_WORK {
        return Err(AuditError::TooManyOps);
    }
    let filled = |cx: f64, cy: f64| {
        let inside = rects.iter().filter(|(r, _)| r[0] < cx && cx < r[2] && r[1] < cy && cy < r[3]);
        if even_odd { inside.count() % 2 == 1 } else { inside.map(|(_, w)| i64::from(*w)).sum::<i64>() != 0 }
    };
    // Filled cells, merged into horizontal runs per grid row, and runs with the same span in
    // adjacent rows merged into one rectangle.
    let mut out: Vec<[f64; 4]> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for row in ys.windows(2) {
        let (y0, y1) = (row[0], row[1]);
        let mut runs: Vec<(f64, f64)> = Vec::new();
        for col in xs.windows(2) {
            // Halve each edge before adding: both can sit near 1e308, where the sum is infinite.
            let (cx, cy) = (col[0] / 2.0 + col[1] / 2.0, y0 / 2.0 + y1 / 2.0);
            // A cell too narrow for a representable interior point holds nothing a stroke can be
            // attributed to. Testing one of its edges instead would flip the parity of every rect
            // whose boundary it is, dropping the whole strip the cell belongs to.
            if !(col[0] < cx && cx < col[1] && y0 < cy && cy < y1) || !filled(cx, cy) {
                continue;
            }
            match runs.last_mut() {
                Some(run) if run.1 == col[0] => run.1 = col[1],
                _ => runs.push((col[0], col[1])),
            }
        }
        let mut next_open = Vec::with_capacity(runs.len());
        for (x0, x1) in runs {
            let grown = open.iter().copied().find(|&i| out.get(i).is_some_and(|r| r[0] == x0 && r[2] == x1 && r[3] == y0));
            match grown.and_then(|i| out.get_mut(i).map(|r| (i, r))) {
                Some((i, r)) => {
                    r[3] = y1;
                    next_open.push(i);
                }
                None => {
                    out.push([x0, y0, x1, y1]);
                    next_open.push(out.len().saturating_sub(1));
                }
            }
        }
        open = next_open;
    }
    Ok(out)
}

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: [0.0, 0.0, 0.0], alpha: 1.0 }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
    /// Grid cells times subpaths examined working out what compound paths paint ([`painted`]).
    path_work: usize,
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
        let mut path = Path::default();
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
                    if gs.ctm != before && !path.is_empty() {
                        path.unreadable = true;
                    }
                }
                b"g" => {
                    // The renderer takes operands from the end and ignores any surplus before
                    // them, so the gray level is the last operand, as for `rg` and `k`.
                    if let Some([v]) = op.nums::<1>().filter(|v| v[0].is_finite()) {
                        let v = v.clamp(0.0, 1.0);
                        gs.fill = [v, v, v];
                    }
                }
                b"rg" => {
                    if let Some([r, g, b]) = op.nums::<3>()
                        && [r, g, b].iter().all(|v| v.is_finite())
                    {
                        gs.fill = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                    }
                }
                b"k" => {
                    if let Some([c, m, y, k]) = op.nums::<4>()
                        && [c, m, y, k].iter().all(|v| v.is_finite())
                    {
                        // Naive CMYK -> RGB; good enough to tell black from white.
                        let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                        gs.fill = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                    }
                }
                b"gs" => {
                    if let Some(name) = op.last_name() {
                        gs.alpha = ext_alpha(self.doc, resources, name, gs.alpha);
                    }
                }
                b"re" => {
                    // `re` is a complete subpath of its own: it ends any `m`/`l` subpath and
                    // joins the rectangles already in the path, which one fill paints together.
                    path.close_lines();
                    let rect = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            let [a, b, c, d, ..] = gs.ctm.0;
                            path.skewed |= !((b == 0.0 && c == 0.0) || (a == 0.0 && d == 0.0));
                            // Counterclockwise in pattern space when `w` and `h` share a sign;
                            // a mirroring CTM turns it around.
                            let winding = if (w * h).signum() * (a * d - b * c).signum() < 0.0 { -1 } else { 1 };
                            (r[2] > r[0] && r[3] > r[1]).then_some((r, winding))
                        } else {
                            None
                        }
                    });
                    if let Some(r) = rect {
                        path.rects.push(r);
                    }
                }
                b"m" => {
                    path.close_lines();
                    path.lines = user_point(gs, op).map(|p| (vec![p], false));
                }
                b"l" => {
                    path.lines = match (path.lines.take(), user_point(gs, op)) {
                        // Past a repeated start point it can no longer be a rectangle.
                        (Some((mut pts, false)), Some(p)) if pts.len() < 5 => {
                            pts.push(p);
                            Some((pts, false))
                        }
                        _ => None,
                    };
                }
                b"h" => {
                    // `re` already closed its subpath, so a further `h` changes nothing.
                    if let Some((_, closed)) = &mut path.lines {
                        *closed = true;
                    }
                }
                // `b`/`b*` close the path before filling it, which the fill does anyway.
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    let even_odd = op.op.ends_with(b"*");
                    let filled = std::mem::take(&mut path);
                    let skewed = filled.skewed;
                    let rects = filled.finish();
                    if gs.alpha >= 1.0 {
                        // A skewed rectangle's box overstates any hole it cuts: count every
                        // rectangle as painted, as past `MAX_EXACT_SUBPATHS`.
                        let painted =
                            if skewed { rects.into_iter().map(|(r, _)| r).collect() } else { painted(rects, even_odd, &mut self.path_work)? };
                        for r in painted {
                            self.out.push(FilledRect { rect: r, rgb: gs.fill, pos });
                        }
                    }
                }
                b"S" | b"s" | b"n" => path = Path::default(),
                // A curve leaves the subpath being built no rectangle; other subpaths stand.
                b"c" | b"v" | b"y" => path.lines = None,
                // A clipping path is not read: what it lets through is out of scope.
                b"W" | b"W*" => path.unreadable = true,
                b"Do" => {
                    if let Some(name) = op.last_name() {
                        if depth >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        self.walk_xobject(name, gs, resources, depth, pos)?;
                    }
                    path = Path::default();
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
        let dict = &stream.dict;
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
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        if !self.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = self.budget.decode_strict(stream)?;
        let ops = parse(&data).ops;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        let mut sub_stack = Vec::new();
        self.walk_ops(&ops, &mut sub_gs, &mut sub_stack, &sub_resources, depth + 1, Some(pos))
    }
}

/// The `x y` operands of `m`/`l`, mapped to user space.
fn user_point(gs: &Gs, op: &Op) -> Option<[f64; 2]> {
    let [x, y] = op.nums::<2>().filter(|v| v.iter().all(|n| n.is_finite()))?;
    let (x, y) = gs.ctm.apply(x, y);
    Some([x, y])
}

/// The non-stroking alpha (`/ca`) of a named ExtGState, or `current` when it sets none (a `gs`
/// changes only the parameters its dictionary holds).
fn ext_alpha(doc: &Document, resources: &Dict, name: &[u8], current: f64) -> f64 {
    let alpha = resources
        .get(b"ExtGState")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(name).map(|o| doc.resolve(o)))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()));
    alpha.filter(|a| a.is_finite()).map_or(current, |a| a.clamp(0.0, 1.0))
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects), with paint
/// positions. A page's content streams are walked once, joined, with one graphics state, so
/// state set in an earlier stream is still in force in the later ones. Any content stream that
/// cannot be decoded, or content and forms that decode or enter past one page's [`DecodeBudget`]
/// in all, is an audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict) -> Result<Vec<FilledRect>, AuditError> {
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
            streams.push(budget.decode_strict(s)?);
        }
    }
    let pieces = Pieces::join(&streams);
    let parsed = pieces.parse();
    let mut w = Walker { doc, pieces: &pieces, out: Vec::new(), ops_seen: 0, path_work: 0, budget };
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    w.walk_ops(&parsed.ops, &mut gs, &mut stack, &resources, 0, None)?;
    Ok(w.out)
}
