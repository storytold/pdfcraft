//! Vector paths painted straight into a page's content (lines, polygons, rectangles, curves) as
//! objects of Edit a PDF. They are listed with the text and images ([`crate::editable_objects`]),
//! move with them, and can also be deleted, restyled (colour, line width) and have one point
//! dragged. A path is the construction operators (`m l c v y re h`) up to the operator that paints
//! it (`S f B …`); a path that only sets a clip (`W n`) is not an object. Edits rewrite those
//! operators in place (points) or wrap them in `q … Q` (style); everything else is copied byte
//! for byte. Paths inside Form XObjects belong to the form, which is edited as a whole.

use std::collections::BTreeSet;
use std::ops::Range;

use pdfcraft_content::{Matrix, Op, Pieces, num, serialize_ops};
use pdfcraft_cos::{Dict, Document, Object};

use crate::EditError;
use crate::objects::checked_streams;

/// A page with more paths than this lists none: a map or a plot made of paths would make every
/// selection slow, and the text and images stay selectable.
pub const MAX_PAGE_PATHS: usize = 5_000;
const MAX_POINTS: usize = 10_000;
const MAX_COORDINATE: f64 = 1e9;
const MAX_DEPTH: usize = 1024;
/// Half of the line width the selection frame of a hairline or a fill-only path allows for.
const MIN_HALF_WIDTH: f64 = 0.5;

/// What a path looks like, in page user space (points).
#[derive(Clone, Debug, PartialEq)]
pub struct PathOutline {
    /// Its anchor points in drawing order (a rectangle has its four corners). Curve control points
    /// are not listed: dragging an end point leaves them where they are.
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
    pub stroked: bool,
    pub filled: bool,
    /// The line width, in page user space.
    pub width: f64,
    /// The stroke and fill colours, when they are device colours the scan could read.
    pub stroke: Option<[f64; 3]>,
    pub fill: Option<[f64; 3]>,
}

/// A change to how some paths look: `color` is the stroke colour of a stroked path and the fill
/// colour of a filled one (both for a path that is both); `width` the line width in page points.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PathStyle {
    pub color: Option<[f64; 3]>,
    pub width: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
enum Slot {
    /// The point's `x y` are these operands of the operator (`m`/`l`: 0; `c`: 4; `v`, `y`: 2).
    Operands(usize),
    /// A corner (0–3, counter-clockwise from the origin) of an `re` rectangle.
    Corner(usize),
}

/// A path found on the page. `ops` are the indexes of its construction operators in the page's
/// parsed operators, then the painting operator.
#[derive(Clone, Debug)]
pub(crate) struct ScannedPath {
    ops: Vec<usize>,
    /// The matrix at the path: its operands' space → page user space.
    ctm: Matrix,
    pub outline: PathOutline,
    pub rect: [f64; 4],
    /// For each anchor point: the operator (an index into the page's operators) and where in it.
    refs: Vec<(usize, Slot)>,
}

#[derive(Clone)]
struct State {
    matrix: Matrix,
    stroke: Option<[f64; 3]>,
    fill: Option<[f64; 3]>,
    width: f64,
}

struct Building {
    ops: Vec<usize>,
    ctm: Matrix,
    /// Anchor points in the operands' space, with where each came from.
    anchors: Vec<([f64; 2], (usize, Slot))>,
    /// Every point, control points too, for the extent.
    extent: Vec<[f64; 2]>,
    closed: bool,
    clip: bool,
}

fn device_color(op: &Op) -> Option<[f64; 3]> {
    let c = match op.op.as_slice() {
        b"g" | b"G" => op.num(0).map(|v| [v; 3]),
        b"rg" | b"RG" => op.nums::<3>(),
        b"k" | b"K" => op.nums::<4>().map(|[c, m, y, k]| [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
        _ => None,
    }?;
    Some(c.map(|v| v.clamp(0.0, 1.0)))
}

/// The line width an `ExtGState` named `name` sets, if the page's resources have one.
fn extgstate_width(doc: &Document, resources: &Dict, name: &[u8]) -> Option<f64> {
    let states = doc.resolve(resources.get(b"ExtGState")?);
    let state = doc.resolve(states.as_dict()?.get(name)?);
    state.as_dict()?.get(b"LW").map(|w| doc.resolve(w)).and_then(|w| w.as_f64()).filter(|w| w.is_finite() && *w >= 0.0)
}

/// Every path painted in the page's operators, in drawing order.
pub(crate) fn scan(doc: &Document, page: usize, ops: &[Op]) -> Vec<ScannedPath> {
    let resources = pdfcraft_model::pages(doc)
        .into_iter()
        .nth(page)
        .and_then(|p| p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()))
        .unwrap_or_default();
    let mut state = State { matrix: Matrix::IDENTITY, stroke: Some([0.0; 3]), fill: Some([0.0; 3]), width: 1.0 };
    let mut stack: Vec<State> = Vec::new();
    let mut building: Option<Building> = None;
    let mut found = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let name = op.op.as_slice();
        match name {
            b"q" => {
                if stack.len() < MAX_DEPTH {
                    stack.push(state.clone());
                }
            }
            b"Q" => {
                if let Some(s) = stack.pop() {
                    state = s;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    state.matrix = Matrix(m).then(&state.matrix);
                }
            }
            b"w" => {
                if let Some(w) = op.num(0).filter(|w| w.is_finite() && *w >= 0.0) {
                    state.width = w;
                }
            }
            b"gs" => {
                if let Some(w) = op.name(0).and_then(|n| extgstate_width(doc, &resources, n)) {
                    state.width = w;
                }
            }
            b"g" | b"rg" | b"k" => state.fill = device_color(op),
            b"G" | b"RG" | b"K" => state.stroke = device_color(op),
            b"cs" | b"sc" | b"scn" => state.fill = None,
            b"CS" | b"SC" | b"SCN" => state.stroke = None,
            b"m" | b"l" => {
                let b = building.get_or_insert_with(|| Building {
                    ops: Vec::new(),
                    ctm: state.matrix,
                    anchors: Vec::new(),
                    extent: Vec::new(),
                    closed: false,
                    clip: false,
                });
                if let Some([x, y]) = op.nums::<2>() {
                    b.ops.push(i);
                    b.anchors.push(([x, y], (i, Slot::Operands(0))));
                    b.extent.push([x, y]);
                }
            }
            b"c" | b"v" | b"y" => {
                let b = building.get_or_insert_with(|| Building {
                    ops: Vec::new(),
                    ctm: state.matrix,
                    anchors: Vec::new(),
                    extent: Vec::new(),
                    closed: false,
                    clip: false,
                });
                // `c` has two control points before its end point, `v` and `y` one.
                let (all, end): (Vec<f64>, usize) = match name {
                    b"c" => (op.nums::<6>().map(|v| v.to_vec()).unwrap_or_default(), 4),
                    _ => (op.nums::<4>().map(|v| v.to_vec()).unwrap_or_default(), 2),
                };
                if let (Some(x), Some(y)) = (all.get(end), all.get(end + 1)) {
                    b.ops.push(i);
                    b.anchors.push(([*x, *y], (i, Slot::Operands(end))));
                    b.extent.extend(all.as_chunks::<2>().0.iter().copied());
                }
            }
            b"re" => {
                let b = building.get_or_insert_with(|| Building {
                    ops: Vec::new(),
                    ctm: state.matrix,
                    anchors: Vec::new(),
                    extent: Vec::new(),
                    closed: false,
                    clip: false,
                });
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    b.ops.push(i);
                    for (corner, p) in [[x, y], [x + w, y], [x + w, y + h], [x, y + h]].into_iter().enumerate() {
                        b.anchors.push((p, (i, Slot::Corner(corner))));
                        b.extent.push(p);
                    }
                    b.closed = true;
                }
            }
            b"h" => {
                // Part of the path: deleting it must take the closing operator too, or a lone `h`
                // is left behind.
                if let Some(b) = building.as_mut() {
                    b.closed = true;
                    b.ops.push(i);
                }
            }
            b"W" | b"W*" => {
                if let Some(b) = building.as_mut() {
                    b.clip = true;
                }
            }
            b"n" => building = None,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if let Some(b) = building.take()
                    && !b.clip
                    && let Some(path) = finish(b, &state, i, name)
                {
                    found.push(path);
                }
            }
            _ => {}
        }
    }
    found
}

fn finish(mut b: Building, state: &State, paint_op: usize, paint: &[u8]) -> Option<ScannedPath> {
    if b.anchors.is_empty() || b.anchors.len() > MAX_POINTS {
        return None;
    }
    let stroked = matches!(paint, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
    let filled = matches!(paint, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
    b.closed |= matches!(paint, b"s" | b"b" | b"b*");
    let [a, bb, c, d, _, _] = b.ctm.0;
    let scale = (a * d - bb * c).abs().sqrt();
    let to_page = |p: &[f64; 2]| {
        let (x, y) = b.ctm.apply(p[0], p[1]);
        [x, y]
    };
    let points: Vec<[f64; 2]> = b.anchors.iter().map(|(p, _)| to_page(p)).collect();
    let extent: Vec<[f64; 2]> = b.extent.iter().map(to_page).collect();
    if !points.iter().chain(&extent).flatten().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) || !scale.is_finite() {
        return None;
    }
    let width = state.width * scale;
    let half = if stroked { (width / 2.0).max(MIN_HALF_WIDTH) } else { MIN_HALF_WIDTH };
    let mut rect = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for [x, y] in extent {
        rect = [rect[0].min(x), rect[1].min(y), rect[2].max(x), rect[3].max(y)];
    }
    let rect = [rect[0] - half, rect[1] - half, rect[2] + half, rect[3] + half];
    let mut ops = b.ops;
    ops.push(paint_op);
    Some(ScannedPath {
        ops,
        ctm: b.ctm,
        outline: PathOutline { points, closed: b.closed, stroked, filled, width, stroke: state.stroke, fill: state.fill },
        rect,
        refs: b.anchors.into_iter().map(|(_, r)| r).collect(),
    })
}

/// `op` rewritten with new numeric operands, as bytes without the trailing separator (the source's
/// own follows, so repeated edits add no blank lines).
fn operator_bytes(name: &str, operands: &[f64]) -> Vec<u8> {
    let mut bytes = serialize_ops(&[Op::new(name, operands.iter().map(|v| num(*v)).collect())]);
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

/// Plan edits over the page's operators, then splice them in and save the new content.
fn rewrite(
    doc: &mut Document,
    page: usize,
    plan: impl FnOnce(&Pieces, &[Op], &[ScannedPath]) -> Result<Vec<(Range<usize>, Vec<u8>)>, EditError>,
) -> Result<(), EditError> {
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = checked_streams(doc, page)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let paths = scan(doc, page, &ops);
    let mut edits = plan(&pieces, &ops, &paths)?;
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    if edits.windows(2).any(|w| matches!(w, [a, b] if a.0.end > b.0.start)) {
        return Err(EditError::Invalid("the path edits overlap".into()));
    }
    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

fn chosen<'a>(paths: &'a [ScannedPath], indexes: &[usize]) -> Result<Vec<&'a ScannedPath>, EditError> {
    let unique: BTreeSet<usize> = indexes.iter().copied().collect();
    unique.into_iter().map(|i| paths.get(i).ok_or_else(|| EditError::Invalid("a selected path no longer exists; select it again".into()))).collect()
}

/// The vector `offset` (page user space) in a path's own operand space.
fn local_offset(ctm: &Matrix, offset: [f64; 2]) -> Result<[f64; 2], EditError> {
    let inverse = ctm.invert().ok_or_else(|| EditError::Invalid("a path has no area to move in".into()))?;
    let [a, b, c, d, _, _] = inverse.0;
    Ok([a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]])
}

/// Move the paths `indexes` by `offset` (page user space): their coordinates are rewritten.
pub fn translate_paths(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    rewrite(doc, page, |_, ops, paths| {
        let mut edits = Vec::new();
        for path in chosen(paths, indexes)? {
            let [dx, dy] = local_offset(&path.ctm, offset)?;
            // The painting operator, last, has no coordinates.
            for &i in path.ops.split_last().map_or(&[][..], |(_, construction)| construction) {
                let op = ops.get(i).ok_or_else(|| EditError::Invalid("a path operator is missing".into()))?;
                let moved = match op.op.as_slice() {
                    b"m" | b"l" => op.nums::<2>().map(|[x, y]| operator_bytes(&String::from_utf8_lossy(&op.op), &[x + dx, y + dy])),
                    b"c" => op.nums::<6>().map(|[a, b, c, d, e, f]| operator_bytes("c", &[a + dx, b + dy, c + dx, d + dy, e + dx, f + dy])),
                    b"v" | b"y" => {
                        op.nums::<4>().map(|[a, b, c, d]| operator_bytes(&String::from_utf8_lossy(&op.op), &[a + dx, b + dy, c + dx, d + dy]))
                    }
                    b"re" => op.nums::<4>().map(|[x, y, w, h]| operator_bytes("re", &[x + dx, y + dy, w, h])),
                    _ => None,
                };
                if let Some(bytes) = moved {
                    edits.push((op.span.clone(), bytes));
                }
            }
        }
        Ok(edits)
    })
}

/// Remove the paths `indexes` (every operator that builds or paints them).
pub fn delete_paths(doc: &mut Document, page: usize, indexes: &[usize]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Err(EditError::Invalid("select a path to delete".into()));
    }
    rewrite(doc, page, |_, ops, paths| {
        let mut edits = Vec::new();
        for path in chosen(paths, indexes)? {
            for &i in &path.ops {
                let op = ops.get(i).ok_or_else(|| EditError::Invalid("a path operator is missing".into()))?;
                edits.push((op.span.clone(), Vec::new()));
            }
        }
        Ok(edits)
    })
}

/// Change the colour and/or line width of the paths `indexes`. The path is wrapped in `q … Q`
/// with the new settings first, so nothing drawn after it is affected.
pub fn style_paths(doc: &mut Document, page: usize, indexes: &[usize], style: PathStyle) -> Result<(), EditError> {
    if indexes.is_empty() || (style.color.is_none() && style.width.is_none()) {
        return Err(EditError::Invalid("choose a colour or a line width".into()));
    }
    if let Some(w) = style.width
        && !(w.is_finite() && (0.0..=1000.0).contains(&w))
    {
        return Err(EditError::Invalid("the line width must be between 0 and 1000 points".into()));
    }
    if let Some(c) = style.color
        && !c.iter().all(|v| v.is_finite())
    {
        return Err(EditError::Invalid("the colour is not valid".into()));
    }
    rewrite(doc, page, |pieces, ops, paths| {
        let mut edits = Vec::new();
        for path in chosen(paths, indexes)? {
            let [a, b, c, d, _, _] = path.ctm.0;
            let scale = (a * d - b * c).abs().sqrt();
            let mut prefix = b"q\n".to_vec();
            if let Some(w) = style.width {
                if !(scale.is_finite() && scale > 1e-9) {
                    return Err(EditError::Invalid("a path has no scale to set a width in".into()));
                }
                prefix.extend(operator_bytes("w", &[w / scale]));
                prefix.push(b'\n');
            }
            if let Some(rgb) = style.color.map(|c| c.map(|v| v.clamp(0.0, 1.0))) {
                if path.outline.stroked {
                    prefix.extend(operator_bytes("RG", &rgb));
                    prefix.push(b'\n');
                }
                if path.outline.filled {
                    prefix.extend(operator_bytes("rg", &rgb));
                    prefix.push(b'\n');
                }
            }
            let (Some(&first), Some(&last)) = (path.ops.first(), path.ops.last()) else { continue };
            let (Some(first), Some(last)) = (ops.get(first), ops.get(last)) else {
                return Err(EditError::Invalid("a path operator is missing".into()));
            };
            let original = |op: &Op| pieces.data().get(op.span.clone()).map(<[u8]>::to_vec).unwrap_or_default();
            prefix.extend(original(first));
            edits.push((first.span.clone(), prefix));
            let mut closing = original(last);
            closing.extend_from_slice(b"\nQ");
            edits.push((last.span.clone(), closing));
        }
        Ok(edits)
    })
}

/// Drag one anchor point of a path to `to` (page user space). A rectangle becomes a four-sided
/// path when one corner moves; the control points of a curve stay where they are.
pub fn move_path_point(doc: &mut Document, page: usize, index: usize, point: usize, to: [f64; 2]) -> Result<(), EditError> {
    if !to.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) {
        return Err(EditError::Invalid("the point is outside the page's range".into()));
    }
    rewrite(doc, page, |_, ops, paths| {
        let path = paths.get(index).ok_or_else(|| EditError::Invalid("the path no longer exists; select it again".into()))?;
        let (op_index, slot) = *path.refs.get(point).ok_or_else(|| EditError::Invalid("the path has no such point".into()))?;
        let op = ops.get(op_index).ok_or_else(|| EditError::Invalid("a path operator is missing".into()))?;
        let inverse = path.ctm.invert().ok_or_else(|| EditError::Invalid("a path has no area to move in".into()))?;
        let (x, y) = inverse.apply(to[0], to[1]);
        let bytes = match (slot, op.op.as_slice()) {
            (Slot::Operands(k), name) => {
                let mut values: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
                if values.len() != op.operands.len() || values.len() < k.saturating_add(2) {
                    return Err(EditError::Invalid("the path point is not made of numbers".into()));
                }
                if let Some(v) = values.get_mut(k) {
                    *v = x;
                }
                if let Some(v) = values.get_mut(k + 1) {
                    *v = y;
                }
                operator_bytes(&String::from_utf8_lossy(name), &values)
            }
            (Slot::Corner(corner), _) => {
                let [x0, y0, w, h] = op.nums::<4>().ok_or_else(|| EditError::Invalid("the rectangle is not made of numbers".into()))?;
                let mut corners = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]];
                if let Some(c) = corners.get_mut(corner) {
                    *c = [x, y];
                }
                let [a, b, c, d] = corners;
                let mut bytes = serialize_ops(&[
                    Op::new("m", vec![num(a[0]), num(a[1])]),
                    Op::new("l", vec![num(b[0]), num(b[1])]),
                    Op::new("l", vec![num(c[0]), num(c[1])]),
                    Op::new("l", vec![num(d[0]), num(d[1])]),
                    Op::new("h", vec![]),
                ]);
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                }
                bytes
            }
        };
        Ok(vec![(op.span.clone(), bytes)])
    })
}
