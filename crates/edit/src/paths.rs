//! Edit a PDF ▸ vector paths: lines, rectangles and curves the page's own content streams paint
//! (ISO 32000-2 §8.5). A path object is its construction operators (`m l c v y h re`) and the
//! painting operator that ends it. Moving one shifts the coordinates of its construction
//! operators in place, so repeated moves add no operators and every other byte is kept.

use std::collections::{BTreeMap, BTreeSet};

use kurbo::{BezPath, ParamCurveNearest, Point, Shape};
use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Document, ObjRef, Object};

use crate::EditError;
use crate::text::content_streams;

/// Paths one page's inventory lists at most. A page that paints more (a detailed drawing) is
/// still shown, printed and edited otherwise; only its paths are not offered one by one.
pub(crate) const MAX_PAGE_PATHS: usize = 5_000;
const MAX_STATE_DEPTH: usize = 4_096;
const MAX_PATH_ELEMENTS: usize = 16_384;
const MAX_COORDINATE: f64 = 1e9;
/// Half the width of the thinnest stroke a click can still find (user-space points).
const MIN_STROKE_HALF: f64 = 0.5;

/// A path a page paints.
#[derive(Clone, Debug, PartialEq)]
pub struct PagePath {
    /// Its painted box in user space, half the stroke width included.
    pub rect: [f64; 4],
    pub stroked: bool,
    pub filled: bool,
    /// The content stream its first operator is in, and the joined-stream indexes ([`Pieces`])
    /// of its first construction operator and of its painting operator.
    stream: usize,
    first: usize,
    paint: usize,
    /// The CTM it is drawn in.
    matrix: Matrix,
    /// Its outline in user space, for hit testing.
    outline: BezPath,
    even_odd: bool,
    stroke_half: f64,
}

impl PagePath {
    /// Whether user-space `point` is on its stroke or inside its fill, within `tolerance`.
    pub fn contains(&self, point: [f64; 2], tolerance: f64) -> bool {
        let tolerance = if tolerance.is_finite() { tolerance.clamp(0.0, 1e6) } else { 0.0 };
        if !point.iter().all(|v| v.is_finite()) {
            return false;
        }
        let reach = tolerance + if self.stroked { self.stroke_half } else { 0.0 };
        let [x0, y0, x1, y1] = self.rect;
        if point[0] < x0 - reach || point[0] > x1 + reach || point[1] < y0 - reach || point[1] > y1 + reach {
            return false;
        }
        let p = Point::new(point[0], point[1]);
        if self.filled {
            let winding = self.outline.winding(p);
            if if self.even_odd { winding % 2 != 0 } else { winding != 0 } {
                return true;
            }
        }
        // A fill's edge counts too, so a thin filled bar can be picked up near it.
        let reach_sq = reach * reach;
        self.outline.segments().any(|seg| seg.nearest(p, 1e-3).distance_sq <= reach_sq)
    }
}

/// The page's streams joined into the one stream they are: an operator may span two of them.
fn joined(streams: &[(Object, Vec<u8>)]) -> Pieces {
    Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>())
}

fn page_of(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn is_construction(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

fn is_paint(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*")
}

/// The path being built: from its first construction operator up to its painting operator.
struct Building {
    stream: usize,
    first: usize,
    matrix: Matrix,
    outline: BezPath,
    /// False once something makes moving its coordinates alone unsafe: a transform or graphics
    /// state change inside it, an out-of-range coordinate, or being inside a text object.
    movable: bool,
}

impl Building {
    fn point(&self, x: f64, y: f64) -> Option<Point> {
        let (x, y) = self.matrix.apply(x, y);
        (x.is_finite() && y.is_finite()).then(|| Point::new(x, y))
    }

    fn add(&mut self, op: &Op) {
        let in_range = op.operands.iter().all(|v| v.as_f64().is_none_or(|v| v.is_finite() && v.abs() <= MAX_COORDINATE));
        if !in_range || self.outline.elements().len() > MAX_PATH_ELEMENTS {
            self.movable = false;
            return;
        }
        // BezPath's line_to and friends need a subpath to extend; a segment before any `m`
        // draws nothing (#3581).
        let open = self.outline.elements().last().is_some();
        match op.op.as_slice() {
            b"m" => {
                if let Some([x, y]) = op.nums::<2>()
                    && let Some(p) = self.point(x, y)
                {
                    self.outline.move_to(p);
                }
            }
            b"l" => {
                if let Some([x, y]) = op.nums::<2>()
                    && let Some(p) = self.point(x, y)
                    && open
                {
                    self.outline.line_to(p);
                }
            }
            b"c" => {
                if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>()
                    && let (Some(a), Some(b), Some(c)) = (self.point(x1, y1), self.point(x2, y2), self.point(x3, y3))
                    && open
                {
                    self.outline.curve_to(a, b, c);
                }
            }
            b"v" => {
                if let Some([x2, y2, x3, y3]) = op.nums::<4>()
                    && let Some(current) = self.outline.elements().last().and_then(|e| e.end_point())
                    && let (Some(b), Some(c)) = (self.point(x2, y2), self.point(x3, y3))
                {
                    self.outline.curve_to(current, b, c);
                }
            }
            b"y" => {
                if let Some([x1, y1, x3, y3]) = op.nums::<4>()
                    && let (Some(a), Some(c)) = (self.point(x1, y1), self.point(x3, y3))
                    && open
                {
                    self.outline.curve_to(a, c, c);
                }
            }
            b"h" => {
                if open {
                    self.outline.close_path();
                }
            }
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>()
                    && let (Some(a), Some(b), Some(c), Some(d)) =
                        (self.point(x, y), self.point(x + w, y), self.point(x + w, y + h), self.point(x, y + h))
                {
                    self.outline.move_to(a);
                    self.outline.line_to(b);
                    self.outline.line_to(c);
                    self.outline.line_to(d);
                    self.outline.close_path();
                }
            }
            _ => {}
        }
    }
}

/// The paths page `page` (0-based) paints in its own content streams, in drawing order. Paths
/// inside Form XObjects belong to that grouped artwork (see [`crate::images`]); clipping paths
/// and paths that paint nothing (`n`) are not listed.
pub fn page_paths(doc: &Document, page: usize) -> Result<Vec<PagePath>, EditError> {
    let p = page_of(doc, page)?;
    let pieces = joined(&content_streams(doc, &p.dict)?);
    let mut out = Vec::new();
    let mut matrix = Matrix::IDENTITY;
    let mut line_width = 1.0_f64;
    let mut stack: Vec<(Matrix, f64)> = Vec::new();
    let mut in_text = false;
    let mut clip = false;
    let mut building: Option<Building> = None;
    for (i, op) in pieces.parse().ops.iter().enumerate() {
        if is_construction(op) {
            let b = building.get_or_insert_with(|| Building {
                stream: pieces.piece_of(op.span.start),
                first: i,
                matrix,
                outline: BezPath::new(),
                movable: !in_text,
            });
            b.add(op);
            continue;
        }
        if is_paint(op) {
            if let Some(b) = building.take()
                && b.movable
                && !clip
                && !op.is("n")
                && let Some(path) = finished(b, i, op, line_width)
            {
                if out.len() >= MAX_PAGE_PATHS {
                    // Listing only some of a drawing's paths would leave the rest unreachable
                    // for no visible reason; offer none.
                    return Ok(Vec::new());
                }
                out.push(path);
            }
            clip = false;
            continue;
        }
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= MAX_STATE_DEPTH {
                    return Err(EditError::Invalid("the page's graphics state is too deeply nested".into()));
                }
                stack.push((matrix, line_width));
            }
            b"Q" => {
                if let Some((m, w)) = stack.pop() {
                    (matrix, line_width) = (m, w);
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            b"w" => {
                if let Some(w) = op.num(0).filter(|w| w.is_finite() && *w >= 0.0) {
                    line_width = w;
                }
            }
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            b"W" | b"W*" => clip = true,
            _ => {}
        }
        // A transform or saved state inside a path object changes how its later points are
        // read; its coordinates can't be shifted by one offset.
        if matches!(op.op.as_slice(), b"q" | b"Q" | b"cm")
            && let Some(b) = building.as_mut()
        {
            b.movable = false;
        }
    }
    Ok(out)
}

/// The listed path for `b`, painted by `op` (at `paint`); `None` when it shows nothing.
fn finished(b: Building, paint: usize, op: &Op, line_width: f64) -> Option<PagePath> {
    let stroked = matches!(op.op.as_slice(), b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
    let filled = matches!(op.op.as_slice(), b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
    let even_odd = matches!(op.op.as_slice(), b"f*" | b"B*" | b"b*");
    if b.outline.elements().is_empty() {
        return None;
    }
    let bounds = b.outline.bounding_box();
    let [a, bb, c, d, _, _] = b.matrix.0;
    let scale = (a * d - bb * c).abs().sqrt();
    let stroke_half = if stroked { (line_width * scale / 2.0).max(MIN_STROKE_HALF) } else { 0.0 };
    if !stroked && (bounds.width() <= 0.0 || bounds.height() <= 0.0) {
        // A fill with no area paints nothing.
        return None;
    }
    let rect = [bounds.x0 - stroke_half, bounds.y0 - stroke_half, bounds.x1 + stroke_half, bounds.y1 + stroke_half];
    if !rect.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) || rect[2] <= rect[0] || rect[3] <= rect[1] {
        return None;
    }
    Some(PagePath { rect, stroked, filled, stream: b.stream, first: b.first, paint, matrix: b.matrix, outline: b.outline, even_odd, stroke_half })
}

/// Inventory indexes exclude the streams that belong to added content.
pub(crate) fn selectable_paths(doc: &Document, page: usize, excluded: &BTreeSet<ObjRef>) -> Result<Vec<(usize, PagePath)>, EditError> {
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    Ok(page_paths(doc, page)?
        .into_iter()
        .enumerate()
        .filter(|(_, path)| !all.get(path.stream).and_then(|(o, _)| o.as_ref()).is_some_and(|r| excluded.contains(&r)))
        .collect())
}

/// The topmost listed path (its [`page_paths`] index) at user-space `point`, within
/// `tolerance` points of its stroke or inside its fill.
pub fn path_at(doc: &Document, page: usize, point: [f64; 2], tolerance: f64) -> Result<Option<usize>, EditError> {
    let excluded = crate::list_added(doc).into_iter().filter(|a| a.page == page).map(|a| a.obj).collect();
    Ok(selectable_paths(doc, page, &excluded)?.into_iter().rev().find(|(_, path)| path.contains(point, tolerance)).map(|(i, _)| i))
}

/// `op` with its points shifted by `d` (in the path's own coordinates).
fn shifted(op: &Op, d: [f64; 2]) -> Option<Op> {
    let point_count = match op.op.as_slice() {
        b"m" | b"l" => 1,
        b"v" | b"y" => 2,
        b"c" => 3,
        b"re" => {
            let [x, y, w, h] = op.nums::<4>()?;
            return Some(Op::new("re", [x + d[0], y + d[1], w, h].map(Object::Real).to_vec()));
        }
        _ => return None,
    };
    let start = op.operands.len().checked_sub(point_count * 2)?;
    let mut operands = Vec::with_capacity(point_count * 2);
    for (k, v) in op.operands.get(start..)?.iter().enumerate() {
        operands.push(Object::Real(v.as_f64()? + d[k % 2]));
    }
    Some(Op { op: op.op.clone(), operands, inline: None, span: 0..0 })
}

/// Move paths `indexes` (from [`page_paths`]) by user-space `offset`. Plans every change before
/// writing; only the selected paths' construction operators are rewritten.
pub(crate) fn translate_paths(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let paths = page_paths(doc, page)?;
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    let pieces = joined(&all);
    let ops = pieces.parse().ops;
    let mut plans: BTreeMap<usize, Op> = BTreeMap::new();
    for index in indexes {
        let path = paths.get(*index).ok_or_else(|| EditError::Invalid("a selected path no longer exists".into()))?;
        if !ops.get(path.paint).is_some_and(is_paint) {
            return Err(EditError::Invalid("a selected path operation is missing".into()));
        }
        let inverse = path.matrix.invert().ok_or_else(|| EditError::Invalid("a selected path has no area".into()))?;
        let [a, b, c, d, _, _] = inverse.0;
        let local = [a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]];
        if !local.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) {
            return Err(EditError::Invalid("the path move is too large".into()));
        }
        for i in path.first..path.paint {
            let Some(op) = ops.get(i) else { continue };
            // `h` has no points; an operator whose operands don't read was not drawn either.
            if let Some(moved) = shifted(op, local) {
                plans.insert(i, moved);
            }
        }
    }
    let data = crate::text::splice(&pieces, &ops, |index| match plans.remove(&index) {
        Some(op) => (vec![op], false),
        None => (Vec::new(), true),
    });
    let contents = crate::text::rewritten_contents(doc, &all, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}
