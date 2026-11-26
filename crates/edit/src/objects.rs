//! Mixed-object inventory and atomic translation in Edit a PDF. Source indexes are resolved
//! together before mutation. Added content appears once, not again as a paragraph/image.

use std::collections::{BTreeMap, BTreeSet};

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object};

use crate::text::ContentStream;
use crate::{Content, EditError};

mod clip;
pub use clip::{CopiedObjects, copy_objects, paste_objects};

pub const MAX_MOVE_OBJECTS: usize = 1_000;
const MAX_PAGE_OBJECTS: usize = 10_000;
const MAX_COORDINATE: f64 = 1e9;
const MAX_CONTENT_BYTES: usize = 128 * 1024 * 1024;

/// The source inventory of an editable item. Indexes are zero-based within that inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Added,
    Text,
    Image,
    VectorLine,
}

impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Text => "text",
            Self::Image => "image",
            Self::VectorLine => "vector_line",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectTarget {
    pub kind: ObjectKind,
    pub index: usize,
}

/// An item recognised by Edit a PDF. Geometry is in the page's user space; a Form is one
/// grouped artwork item. Its nested content is kept intact, not selected again separately.
#[derive(Clone, Debug, PartialEq)]
pub struct EditableObject {
    pub target: ObjectTarget,
    pub rect: [f64; 4],
    pub content_type: &'static str,
    pub text: Option<String>,
}

fn valid_rect(rect: [f64; 4]) -> bool {
    rect.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) && rect[2] > rect[0] && rect[3] > rect[1]
}

pub fn editable_objects(doc: &Document, page: usize) -> Result<Vec<EditableObject>, EditError> {
    checked_streams(doc, page)?;
    let added: Vec<_> = crate::list_added(doc).into_iter().filter(|a| a.page == page).collect();
    if added.len() > MAX_PAGE_OBJECTS {
        return Err(EditError::Invalid("this page has too many objects to select together".into()));
    }
    let geometry = added_geometry(doc, page)?.1;
    let excluded = added.iter().map(|a| a.obj).collect();
    let mut objects = Vec::new();
    for (index, a) in added.into_iter().enumerate() {
        let (content_type, text) = match &a.content {
            Content::Text(t) => ("text", Some(t.text.clone())),
            Content::Image(_) => ("image", None),
        };
        objects.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Added, index },
            rect: geometry.get(index).ok_or_else(|| EditError::Invalid("an added item has no placement".into()))?.rect,
            content_type,
            text,
        });
    }
    for (index, b) in crate::text::selectable_blocks(doc, page, &excluded)? {
        objects.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Text, index },
            rect: b.rect,
            content_type: "text",
            text: Some(b.text),
        });
    }
    for (index, image) in crate::images::selectable_images(doc, page, &excluded)? {
        let Some(rect) = image.visible_rect else { continue };
        objects.push(EditableObject {
            target: ObjectTarget { kind: ObjectKind::Image, index },
            rect,
            content_type: if image.is_form { "form" } else { "image" },
            text: None,
        });
    }
    if objects.len() > MAX_PAGE_OBJECTS {
        return Err(EditError::Invalid("this page has too many objects to select together".into()));
    }
    if objects.iter().any(|o| !valid_rect(o.rect)) {
        return Err(EditError::Invalid("an object has invalid geometry".into()));
    }
    Ok(objects)
}

/// Move selected mixed objects by a user-space vector as one atomic operation. All targets
/// refer to the original inventories. A refusal leaves the graph, including overlay, untouched.
pub fn move_objects(doc: &mut Document, page: usize, targets: &[ObjectTarget], offset: [f64; 2]) -> Result<(), EditError> {
    if targets.is_empty() || targets.len() > MAX_MOVE_OBJECTS {
        return Err(EditError::Invalid("select between 1 and 1000 objects to move".into()));
    }
    if !offset.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) || offset == [0.0, 0.0] {
        return Err(EditError::Invalid("the move must be finite and non-zero".into()));
    }
    let inventory: BTreeMap<_, _> = editable_objects(doc, page)?.into_iter().map(|o| (o.target, o)).collect();
    let mut unique = BTreeSet::new();
    for target in targets {
        if !unique.insert(*target) {
            return Err(EditError::Invalid("an object was selected more than once".into()));
        }
        let object =
            inventory.get(target).ok_or_else(|| EditError::Invalid("a selected object no longer exists; select the objects again".into()))?;
        let r = object.rect;
        if !valid_rect([r[0] + offset[0], r[1] + offset[1], r[2] + offset[0], r[3] + offset[1]]) {
            return Err(EditError::Invalid("the object move is too large".into()));
        }
    }
    let indexes = |kind| targets.iter().filter(|t| t.kind == kind).map(|t| t.index).collect::<Vec<_>>();
    let mut work = doc.clone();
    crate::text::translate_blocks(&mut work, page, &indexes(ObjectKind::Text), offset)?;
    crate::images::translate_images(&mut work, page, &indexes(ObjectKind::Image), offset)?;
    translate_added(&mut work, page, &indexes(ObjectKind::Added), offset)?;
    // Placement operators add to size/depth/work budgets, and moving paragraphs can change
    // their grouping. Keep the resulting inventory usable; refuse before swapping otherwise.
    editable_objects(&work, page)?;
    *doc = work;
    Ok(())
}

fn validate_line_properties_args(target: ObjectTarget, color: Option<[f64; 3]>, width: Option<f64>, dash: Option<&[f64]>) -> Result<(), EditError> {
    if target.kind != ObjectKind::VectorLine {
        return Err(EditError::Invalid("expected target kind VectorLine".into()));
    }
    if let Some(c) = color
        && !c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    {
        return Err(EditError::Invalid("color channel must be finite and between 0.0 and 1.0".into()));
    }
    if let Some(w) = width
        && (!w.is_finite() || !(0.5..=20.0).contains(&w))
    {
        return Err(EditError::Invalid("width must be finite and between 0.5 and 20.0".into()));
    }
    if let Some(d) = dash
        && (d.len() > 8 || !d.iter().all(|v| v.is_finite() && (0.0..=100.0).contains(v)))
    {
        return Err(EditError::Invalid("dash values must be finite, between 0.0 and 100.0, and have length <= 8".into()));
    }
    Ok(())
}

fn is_path_op(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

fn is_paint_or_state_boundary(op: &Op) -> bool {
    matches!(op.op.as_slice(), b"S" | b"s" | b"f" | b"f*" | b"F" | b"b" | b"b*" | b"B" | b"B*" | b"n" | b"q" | b"Q")
}

struct SegmentIsolation {
    has_connected_preceding: bool,
    has_subsequent_path: bool,
    subsequent_starts_with_m: bool,
}

fn analyze_segment_isolation(ops: &[Op], m_op_idx: usize, first_l_idx: usize, last_l_idx: usize, stroke_op_idx: usize) -> SegmentIsolation {
    let has_connected_preceding = ops.get((m_op_idx + 1)..first_l_idx).is_some_and(|slice| slice.iter().any(is_path_op));
    let mut has_subsequent_path = false;
    let mut subsequent_starts_with_m = false;
    if let Some(sub_slice) = ops.get((last_l_idx + 1)..stroke_op_idx) {
        for op in sub_slice {
            if op.is("m") {
                if !has_subsequent_path {
                    subsequent_starts_with_m = true;
                }
                has_subsequent_path = true;
            } else if is_path_op(op) {
                has_subsequent_path = true;
                break;
            }
        }
    }
    SegmentIsolation { has_connected_preceding, has_subsequent_path, subsequent_starts_with_m }
}

fn has_preceding_path_op(ops: &[Op], before_idx: usize) -> bool {
    for op in ops[..before_idx].iter().rev() {
        if is_path_op(op) {
            return true;
        }
        if is_paint_or_state_boundary(op) {
            return false;
        }
    }
    false
}

fn serialize_single_op(name: &str, operands: Vec<Object>) -> Vec<u8> {
    let mut bytes = pdfcraft_content::serialize_ops(&[Op::new(name, operands)]);
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

fn build_line_property_prefix(
    has_preceding_path: bool,
    matrix: Matrix,
    color: Option<[f64; 3]>,
    width: Option<f64>,
    dash: Option<&[f64]>,
) -> Vec<u8> {
    let mut prefix_bytes = Vec::new();
    if has_preceding_path {
        // Earlier subpaths are only closed by explicit `h`/`re`, never by a trailing `s`, so `S` preserves them.
        prefix_bytes.extend_from_slice(b"\nS\n");
    }
    let mut prefix_ops = vec![Op::new("q", Vec::new())];
    if let Some(c) = color {
        prefix_ops.push(Op::new("RG", vec![pdfcraft_content::num(c[0]), pdfcraft_content::num(c[1]), pdfcraft_content::num(c[2])]));
    }
    if let Some(w) = width {
        let [a, b, c, d, _, _] = matrix.0;
        let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
        let local_w = if scale > 1e-6 { w / scale } else { w };
        prefix_ops.push(Op::new("w", vec![pdfcraft_content::num(local_w)]));
    }
    if let Some(d) = dash {
        let dash_array = Object::Array(d.iter().copied().map(pdfcraft_content::num).collect());
        prefix_ops.push(Op::new("d", vec![dash_array, pdfcraft_content::num(0.0)]));
    }
    prefix_bytes.push(b'\n');
    prefix_bytes.extend_from_slice(&pdfcraft_content::serialize_ops(&prefix_ops));
    if prefix_bytes.last() != Some(&b'\n') && prefix_bytes.last() != Some(&b' ') {
        prefix_bytes.push(b'\n');
    }
    prefix_bytes
}

/// Set stroke properties (color, width, dash pattern) for a vector line segment.
pub fn set_line_properties(
    doc: &mut Document,
    page: usize,
    target: ObjectTarget,
    color: Option<[f64; 3]>,
    width: Option<f64>,
    dash: Option<Vec<f64>>,
) -> Result<(), EditError> {
    validate_line_properties_args(target, color, width, dash.as_deref())?;

    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = checked_streams(doc, page)?;
    let parsed_lines = parse_page_lines(&streams)?;
    let target_line = parsed_lines.get(target.index).ok_or_else(|| EditError::Invalid("line no longer exists".into()))?;

    if target_line.re_info.is_some() {
        return Err(EditError::Invalid("changing properties of one edge of a rectangle is not supported".into()));
    }
    let Some(m_op_idx) = target_line.m_op_idx else {
        return Err(EditError::Invalid("changing properties of a line without a standalone moveto operator is not supported".into()));
    };

    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;

    let m_op = ops.get(m_op_idx).ok_or_else(|| EditError::Invalid("line moveto operator missing".into()))?;
    let first_l_idx = *target_line.l_op_indices.first().ok_or_else(|| EditError::Invalid("line segment missing lineto operator".into()))?;
    let last_l_idx = *target_line.l_op_indices.last().ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;
    let last_l_op = ops.get(last_l_idx).ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;
    let stroke_op = ops.get(target_line.stroke_op_idx).ok_or_else(|| EditError::Invalid("line stroke operator missing".into()))?;

    if !matches!(stroke_op.op.as_slice(), b"S" | b"s") {
        return Err(EditError::Invalid("changing properties of a line within a filled or compound path is not supported".into()));
    }
    if last_l_idx >= target_line.stroke_op_idx {
        return Err(EditError::Invalid("changing properties of a connected multi-segment path is not supported".into()));
    }

    let isolation = analyze_segment_isolation(&ops, m_op_idx, first_l_idx, last_l_idx, target_line.stroke_op_idx);
    if isolation.has_connected_preceding || (isolation.has_subsequent_path && !isolation.subsequent_starts_with_m) {
        return Err(EditError::Invalid("changing properties of a connected multi-segment path is not supported".into()));
    }

    let has_preceding_path = has_preceding_path_op(&ops, m_op_idx);
    let prefix_bytes = build_line_property_prefix(has_preceding_path, target_line.matrix, color, width, dash.as_deref());
    let preserve_close_stroke = !isolation.has_subsequent_path && stroke_op.is("s");
    let suffix_bytes = if preserve_close_stroke { b"\ns\nQ\n".to_vec() } else { b"\nS\nQ\n".to_vec() };

    let mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
    edits.push((m_op.span.start..m_op.span.start, prefix_bytes));
    edits.push((last_l_op.span.end..last_l_op.span.end, suffix_bytes));
    if !isolation.has_subsequent_path {
        edits.push((stroke_op.span.clone(), b"\n".to_vec()));
    }
    edits.sort_by_key(|(span, _)| span.start);

    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data)?;
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

fn signed_re_span(a: f64, b: f64, orig_dim: f64) -> (f64, f64) {
    let span = (b - a).abs();
    if orig_dim < 0.0 { (a.max(b), -span) } else { (a.min(b), span) }
}

fn update_rect_edge_endpoints(
    ops: &[Op],
    target_line: &ParsedLine,
    re_idx: usize,
    edge_idx: usize,
    local_start: [f64; 2],
    local_end: [f64; 2],
) -> Result<(std::ops::Range<usize>, Vec<u8>), EditError> {
    let re_op = ops.get(re_idx).ok_or_else(|| EditError::Invalid("rectangle operator missing".into()))?;
    let [rx, ry, rw, rh] = re_op.nums::<4>().ok_or_else(|| EditError::Invalid("invalid rectangle operands".into()))?;

    let (new_x, new_y, new_w, new_h) = match edge_idx {
        0 => {
            let y = (local_start[1] + local_end[1]) / 2.0;
            let (x, w) = signed_re_span(local_start[0], local_end[0], rw);
            (x, y, w, (ry + rh) - y)
        }
        1 => {
            let (y, h) = signed_re_span(local_start[1], local_end[1], rh);
            (rx, y, ((local_start[0] + local_end[0]) / 2.0) - rx, h)
        }
        2 => {
            let (x, w) = signed_re_span(local_start[0], local_end[0], rw);
            (x, ry, w, ((local_start[1] + local_end[1]) / 2.0) - ry)
        }
        3 => {
            let x = (local_start[0] + local_end[0]) / 2.0;
            let (y, h) = signed_re_span(local_start[1], local_end[1], rh);
            (x, y, (rx + rw) - x, h)
        }
        _ => return Err(EditError::Invalid("invalid rectangle edge index".into())),
    };

    let crossed_w = (rw > 0.0 && new_w <= 1e-4) || (rw < 0.0 && new_w >= -1e-4);
    let crossed_h = (rh > 0.0 && new_h <= 1e-4) || (rh < 0.0 && new_h >= -1e-4);
    let re_bytes = serialize_single_op(
        "re",
        vec![pdfcraft_content::num(new_x), pdfcraft_content::num(new_y), pdfcraft_content::num(new_w), pdfcraft_content::num(new_h)],
    );
    let Some([fx, fy, fw, fh]) = pdfcraft_content::parse(&re_bytes).ops.first().and_then(|op| op.nums::<4>()) else {
        return Err(EditError::Invalid("rectangle dimensions could not be serialized".into()));
    };
    let u0 = target_line.matrix.apply(fx, fy);
    let u1 = target_line.matrix.apply(fx + fw, fy);
    let u3 = target_line.matrix.apply(fx, fy + fh);
    let user_w = (u1.0 - u0.0).hypot(u1.1 - u0.1);
    let user_h = (u3.0 - u0.0).hypot(u3.1 - u0.1);
    if crossed_w || crossed_h || new_w.abs() <= 1e-4 || new_h.abs() <= 1e-4 || user_w <= 1e-4 || user_h <= 1e-4 {
        return Err(EditError::Invalid("rectangle dimensions collapsed or crossed the opposite edge".into()));
    }
    Ok((re_op.span.clone(), re_bytes))
}

fn serialized_user_length(matrix: Matrix, m_bytes: &[u8], l_bytes: &[u8]) -> Option<f64> {
    let [mx, my] = pdfcraft_content::parse(m_bytes).ops.first()?.nums::<2>()?;
    let [lx, ly] = pdfcraft_content::parse(l_bytes).ops.first()?.nums::<2>()?;
    let (ux0, uy0) = matrix.apply(mx, my);
    let (ux1, uy1) = matrix.apply(lx, ly);
    Some((ux1 - ux0).hypot(uy1 - uy0))
}

/// Update endpoints for a vector line segment on a page.
pub fn update_line_endpoints(doc: &mut Document, page: usize, target: ObjectTarget, new_start: [f64; 2], new_end: [f64; 2]) -> Result<(), EditError> {
    if target.kind != ObjectKind::VectorLine {
        return Err(EditError::Invalid("expected target kind VectorLine".into()));
    }
    if !new_start.iter().chain(new_end.iter()).all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) {
        return Err(EditError::Invalid("coordinates must be finite and within MAX_COORDINATE".into()));
    }
    let user_len = (new_end[0] - new_start[0]).hypot(new_end[1] - new_start[1]);
    if !user_len.is_finite() || user_len <= 1e-4 {
        return Err(EditError::Invalid("new_start and new_end must be farther than 1e-4 pt apart (zero-length line)".into()));
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;

    let streams = checked_streams(doc, page)?;
    let parsed_lines = parse_page_lines(&streams)?;
    let target_line = parsed_lines.get(target.index).ok_or_else(|| EditError::Invalid("line no longer exists".into()))?;

    let inv = target_line.matrix.invert().ok_or_else(|| EditError::Invalid("line coordinate transform is singular".into()))?;
    let (local_start_x, local_start_y) = inv.apply(new_start[0], new_start[1]);
    let (local_end_x, local_end_y) = inv.apply(new_end[0], new_end[1]);
    if !local_start_x.is_finite() || !local_start_y.is_finite() || !local_end_x.is_finite() || !local_end_y.is_finite() {
        return Err(EditError::Invalid("transformed coordinates must be finite".into()));
    }

    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;

    if let Some((re_idx, edge_idx)) = target_line.re_info {
        let edit = update_rect_edge_endpoints(&ops, target_line, re_idx, edge_idx, [local_start_x, local_start_y], [local_end_x, local_end_y])?;
        let data = pieces.splice(vec![edit]);
        let contents = crate::text::rewritten_contents(doc, &streams, data)?;
        doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
        return Ok(());
    }

    let Some(m_op_idx) = target_line.m_op_idx else {
        return Err(EditError::Invalid("editing endpoints of a line without a standalone moveto operator is not supported".into()));
    };
    let m_op = ops.get(m_op_idx).ok_or_else(|| EditError::Invalid("line moveto operator missing".into()))?;
    let first_l_idx = *target_line.l_op_indices.first().ok_or_else(|| EditError::Invalid("line segment missing lineto operator".into()))?;
    let last_l_idx = *target_line.l_op_indices.last().ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;
    let last_l_op = ops.get(last_l_idx).ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;

    if last_l_idx >= target_line.stroke_op_idx {
        return Err(EditError::Invalid("editing endpoints of a connected multi-segment path is not supported".into()));
    }
    let isolation = analyze_segment_isolation(&ops, m_op_idx, first_l_idx, last_l_idx, target_line.stroke_op_idx);
    if isolation.has_connected_preceding || (isolation.has_subsequent_path && !isolation.subsequent_starts_with_m) {
        return Err(EditError::Invalid("editing endpoints of a connected multi-segment path is not supported".into()));
    }

    let m_bytes = serialize_single_op("m", vec![pdfcraft_content::num(local_start_x), pdfcraft_content::num(local_start_y)]);
    let l_bytes = serialize_single_op("l", vec![pdfcraft_content::num(local_end_x), pdfcraft_content::num(local_end_y)]);
    if serialized_user_length(target_line.matrix, &m_bytes, &l_bytes).is_none_or(|len| !len.is_finite() || len <= 1e-4) {
        return Err(EditError::Invalid("new_start and new_end collapse below 1e-4 pt after formatting (zero-length line)".into()));
    }

    let mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
    edits.push((m_op.span.clone(), m_bytes));
    if target_line.l_op_indices.len() > 1 {
        for &l_idx in &target_line.l_op_indices[..target_line.l_op_indices.len() - 1] {
            if let Some(l_op) = ops.get(l_idx) {
                edits.push((l_op.span.clone(), Vec::new()));
            }
        }
    }
    edits.push((last_l_op.span.clone(), l_bytes));
    edits.sort_by_key(|(span, _)| span.start);

    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data)?;
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

struct AddedGeometry {
    rect: [f64; 4],
    local_rect: [f64; 4],
    origin: Matrix,
    outside: Matrix,
    current: Matrix,
    stream: usize,
    op: usize,
    params: Dict,
}

/// An added item's metadata is in the coordinate space it was written in, which can differ
/// from today's page rotation/crop/UserUnit. Read its original placement, including graphics
/// state from preceding streams. MoveOrigin retains that basis after a byte-preserving move.
fn added_geometry(doc: &Document, page: usize) -> Result<(Pieces, Vec<AddedGeometry>), EditError> {
    let added: Vec<_> = crate::list_added(doc).into_iter().filter(|a| a.page == page).collect();
    if added.is_empty() {
        return Ok((Pieces::default(), Vec::new()));
    }
    let streams = checked_streams(doc, page)?;
    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;
    let owned: BTreeSet<_> = added.iter().map(|a| a.obj).collect();
    let mut geometry = BTreeMap::new();
    let mut first = BTreeMap::new();
    let mut matrix = Matrix::IDENTITY;
    let mut stack = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let (stream, end) = pieces.pieces_of(op);
        let object = streams.get(stream).and_then(|s| s.entry.as_ref());
        if object.is_some_and(|r| owned.contains(&r)) {
            if let std::collections::btree_map::Entry::Vacant(entry) = first.entry(stream) {
                if stream != end || !op.is("q") {
                    return Err(EditError::Invalid("an added item has an unsupported placement".into()));
                }
                entry.insert((index, matrix));
            } else if first.get(&stream).is_some_and(|(i, _)| i.checked_add(1) == Some(index)) {
                if stream != end || !op.is("cm") {
                    return Err(EditError::Invalid("an added item has an unsupported placement".into()));
                }
                let current = op.nums::<6>().map(Matrix).ok_or_else(|| EditError::Invalid("an added item has invalid geometry".into()))?;
                let outside = first.get(&stream).map_or(Matrix::IDENTITY, |(_, m)| *m);
                let params = object
                    .and_then(|r| doc.get(r).as_dict().and_then(|d| d.get(b"PCAdded")).map(|p| doc.resolve(p)))
                    .and_then(|p| p.as_dict().cloned())
                    .ok_or_else(|| EditError::Invalid("an added item has no parameters".into()))?;
                let origin = match params.get(b"MoveOrigin") {
                    None => current,
                    Some(o) => doc
                        .resolve(o)
                        .as_array()
                        .and_then(|a| Matrix::from_operands(a))
                        .ok_or_else(|| EditError::Invalid("an added item has invalid geometry".into()))?,
                };
                let raw = params
                    .get(b"Rect")
                    .map(|r| doc.resolve(r))
                    .and_then(|r| r.as_array().cloned())
                    .ok_or_else(|| EditError::Invalid("an added item has no rectangle".into()))?;
                let local_rect = raw
                    .iter()
                    .map(Object::as_f64)
                    .collect::<Option<Vec<_>>>()
                    .and_then(|r| <[f64; 4]>::try_from(r).ok())
                    .ok_or_else(|| EditError::Invalid("an added item has invalid geometry".into()))?;
                if !current.0.iter().chain(origin.0.iter()).chain(outside.0.iter()).all(|v| v.is_finite() && v.abs() <= 1e12)
                    || !valid_rect(local_rect)
                {
                    return Err(EditError::Invalid("an added item has invalid geometry".into()));
                }
                geometry.insert(
                    stream,
                    AddedGeometry { rect: origin.then(&outside).bbox(local_rect), local_rect, origin, outside, current, stream, op: index, params },
                );
            }
        }
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= 1024 {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                stack.push(matrix);
            }
            b"Q" => matrix = stack.pop().unwrap_or(matrix),
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    matrix = Matrix(m).then(&matrix);
                }
            }
            _ => {}
        }
    }
    let geometry: Vec<_> = geometry.into_values().collect();
    if geometry.len() != added.len() {
        return Err(EditError::Invalid("an added item has no placement".into()));
    }
    Ok((pieces, geometry))
}

fn translate_added(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = checked_streams(doc, page)?;
    let (pieces, geometry) = added_geometry(doc, page)?;
    let ops = pieces.parse().ops;
    let mut replacements = BTreeMap::new();
    let mut parameters = Vec::new();
    let linear = |inverse: Matrix| {
        let [a, b, c, d, _, _] = inverse.0;
        [a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]]
    };
    for index in indexes {
        let g = geometry.get(*index).ok_or_else(|| EditError::Invalid("a selected added item no longer exists".into()))?;
        let delta = linear(g.origin.then(&g.outside).invert().ok_or_else(|| EditError::Invalid("an added item has no area".into()))?);
        let outer_delta = linear(g.outside.invert().ok_or_else(|| EditError::Invalid("an added item has no area".into()))?);
        let current = g.current.then(&Matrix([1.0, 0.0, 0.0, 1.0, outer_delta[0], outer_delta[1]]));
        let r = g.local_rect;
        let rect = [r[0] + delta[0], r[1] + delta[1], r[2] + delta[0], r[3] + delta[1]];
        if !valid_rect(rect) || !current.0.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the added item move is too large".into()));
        }
        let mut params = g.params.clone();
        params.set(b"MoveOrigin".to_vec(), Object::Array(g.origin.0.map(Object::Real).to_vec()));
        params.set(b"Rect".to_vec(), Object::Array(rect.map(Object::Real).to_vec()));
        parameters.push((g.stream, params));
        replacements.insert(g.op, Op::new("cm", current.0.map(Object::Real).to_vec()));
    }
    // A placement replaces exactly one operator. Keep its existing following separator,
    // rather than adding a blank line every time the item is moved.
    let edits = replacements
        .into_iter()
        .map(|(i, op)| {
            let original = ops.get(i).ok_or_else(|| EditError::Invalid("an added item placement is missing".into()))?;
            let mut bytes = pdfcraft_content::serialize_ops(&[op]);
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            Ok((original.span.clone(), bytes))
        })
        .collect::<Result<Vec<_>, EditError>>()?;
    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data)?;
    let refs = match &contents {
        Object::Array(a) => a.clone(),
        one => vec![one.clone()],
    };
    for (stream, params) in parameters {
        let r = refs.get(stream).and_then(Object::as_ref).ok_or_else(|| EditError::Invalid("the added item stream is missing".into()))?;
        doc.update_dict(r, |d| d.set(b"PCAdded".to_vec(), Object::Dict(params)))?;
    }
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// A missing, undecodable or damaged preceding stream can hide graphics state that affects every
/// later item. Refuse group editing rather than inventing an inventory from partial data.
fn checked_streams(doc: &Document, page: usize) -> Result<Vec<ContentStream>, EditError> {
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let contents = match p.dict.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => {
                if a.len() > MAX_PAGE_OBJECTS {
                    return Err(EditError::Invalid("this page has too many content streams to select objects together".into()));
                }
                a.clone()
            }
            _ => vec![c.clone()],
        },
    };
    let mut remaining = MAX_CONTENT_BYTES;
    let mut decode = DecodeBudget::new(usize::MAX, 0);
    let mut depth = 0usize;
    let mut operations = 0usize;
    let mut streams = Vec::with_capacity(contents.len());
    for object in contents {
        let resolved = doc.resolve(&object);
        let Object::Stream(stream) = &*resolved else {
            return Err(EditError::Invalid("a page content stream is missing".into()));
        };
        let (bytes, damaged) = decode
            .decode_with_damage(doc, stream, remaining.min(64 * 1024 * 1024))
            .map_err(|_| EditError::Invalid("the page content cannot be decoded within its size limit".into()))?;
        if damaged {
            return Err(EditError::Invalid("a page content stream is damaged partway".into()));
        }
        remaining =
            remaining.checked_sub(bytes.len()).ok_or_else(|| EditError::Invalid("the page content is too large for group selection".into()))?;
        let parsed = pdfcraft_content::parse(&bytes);
        operations = operations.saturating_add(parsed.ops.len());
        if operations > 100_000 {
            return Err(EditError::Invalid("this page has too many content operations for group selection".into()));
        }
        for op in parsed.ops {
            if op.is("q") {
                depth = depth.saturating_add(1);
            } else if op.is("Q") {
                depth = depth.saturating_sub(1);
            }
            if depth > 1024 {
                return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
            }
        }
        streams.push(ContentStream { entry: object, data: bytes, read: crate::text::Read::Whole });
    }
    Ok(streams)
}

// ─── Vector line inventory ────────────────────────────────────────────────

/// Upper bound on line segments returned per page. Barcode sheets can produce
/// thousands of short strokes; collinear merging cuts this 40–60%, and this
/// cap catches pathological pages.
pub const MAX_PAGE_LINES: usize = 10_000;

/// Two lines are merged if their direction differs by less than this (radians)
/// and their endpoints are within MERGE_DIST_EPS of each other.
const MERGE_ANGLE_EPS: f64 = 1e-3;
const MERGE_DIST_EPS: f64 = 0.01;

/// A single stroked line segment on a page, in PDF user space.
///
/// `index` matches `ObjectTarget { kind: ObjectKind::VectorLine, index }`, so
/// Session 2 (property panel) can locate the source object via the same
/// inventory pattern `move_objects` uses.
#[derive(Clone, Debug, PartialEq)]
pub struct LineSegment {
    /// 0-based page index.
    pub page: usize,
    /// Index in this page's vector-line inventory.
    pub index: usize,
    /// Start point `[x, y]` in PDF user space.
    pub start: [f64; 2],
    /// End point `[x, y]` in PDF user space.
    pub end: [f64; 2],
    /// Stroke width in user-space points.
    pub thickness: f64,
    /// Stroke color `[r, g, b]` in 0.0..=1.0 if determined from content stream.
    pub stroke_color: Option<[f64; 3]>,
    /// Operator index of the `re` rectangle if this edge belongs to one; `None` for standalone lines.
    pub rect_op: Option<usize>,
}

impl LineSegment {
    /// Axis-aligned bounding box in user space: `[min_x, min_y, max_x, max_y]`.
    pub fn bbox(&self) -> [f64; 4] {
        [self.start[0].min(self.end[0]), self.start[1].min(self.end[1]), self.start[0].max(self.end[0]), self.start[1].max(self.end[1])]
    }

    /// Euclidean length in user-space points.
    pub fn length(&self) -> f64 {
        let dx = self.end[0] - self.start[0];
        let dy = self.end[1] - self.start[1];
        dx.hypot(dy)
    }

    /// Squared distance from a user-space point to this segment.
    pub fn distance_sq_to(&self, p: [f64; 2]) -> f64 {
        let vx = self.end[0] - self.start[0];
        let vy = self.end[1] - self.start[1];
        let len = vx.hypot(vy);
        if len <= f64::EPSILON || !len.is_finite() {
            let d = (p[0] - self.start[0]).hypot(p[1] - self.start[1]);
            return d * d;
        }
        let ux = vx / len;
        let uy = vy / len;
        let proj = (p[0] - self.start[0]) * ux + (p[1] - self.start[1]) * uy;
        let t = (proj / len).clamp(0.0, 1.0);
        let px = self.start[0] + t * vx;
        let py = self.start[1] + t * vy;
        let dist = (p[0] - px).hypot(p[1] - py);
        dist * dist
    }
}

const MAX_RAW_SEGMENTS: usize = 200_000;

#[derive(Clone, Debug)]
struct ParsedLine {
    start: [f64; 2],
    end: [f64; 2],
    thickness: f64,
    color: Option<[f64; 3]>,
    matrix: Matrix,
    m_op_idx: Option<usize>,
    l_op_indices: Vec<usize>,
    stroke_op_idx: usize,
    straight_group: usize,
    re_info: Option<(usize, usize)>,
}

#[derive(Clone)]
struct LineGState {
    matrix: Matrix,
    width: f64,
    color: Option<[f64; 3]>,
}

impl LineGState {
    fn stroke_thickness(&self) -> f64 {
        let [a, b, c, d, _, _] = self.matrix.0;
        let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
        if scale > 1e-6 { (self.width * scale).max(0.1) } else { self.width.max(0.1) }
    }
}

struct PendingSeg {
    start_user: [f64; 2],
    end_user: [f64; 2],
    thickness: f64,
    color: Option<[f64; 3]>,
    matrix: Matrix,
    m_op_idx: Option<usize>,
    l_op_idx: usize,
    straight_group: usize,
    re_info: Option<(usize, usize)>,
}

struct LineScan {
    state: LineGState,
    stack: Vec<LineGState>,
    current_point: Option<([f64; 2], [f64; 2])>,
    subpath_start: Option<([f64; 2], [f64; 2])>,
    current_m_idx: Option<usize>,
    straight_group: usize,
    pending_segs: Vec<PendingSeg>,
    raw_lines: Vec<ParsedLine>,
}

impl LineScan {
    fn new() -> Self {
        Self {
            state: LineGState { matrix: Matrix::IDENTITY, width: 1.0, color: Some([0.0, 0.0, 0.0]) },
            stack: Vec::new(),
            current_point: None,
            subpath_start: None,
            current_m_idx: None,
            straight_group: 0,
            pending_segs: Vec::new(),
            raw_lines: Vec::new(),
        }
    }

    fn bump_group(&mut self) {
        self.straight_group = self.straight_group.saturating_add(1);
    }

    fn push_segment(
        &mut self,
        start_user: [f64; 2],
        end_user: [f64; 2],
        m_op_idx: Option<usize>,
        l_op_idx: usize,
        re_info: Option<(usize, usize)>,
    ) -> Result<(), EditError> {
        let thickness = self.state.stroke_thickness();
        let all_finite = start_user.iter().chain(end_user.iter()).all(|v| v.is_finite()) && thickness.is_finite() && thickness > 0.0;
        if !all_finite || (end_user[0] - start_user[0]).hypot(end_user[1] - start_user[1]) <= 1e-4 {
            return Ok(());
        }
        if self.raw_lines.len().saturating_add(self.pending_segs.len()) >= MAX_RAW_SEGMENTS {
            return Err(EditError::Invalid("the page has too many line segments to edit as a group".into()));
        }
        self.pending_segs.push(PendingSeg {
            start_user,
            end_user,
            thickness,
            color: self.state.color,
            matrix: self.state.matrix,
            m_op_idx,
            l_op_idx,
            straight_group: self.straight_group,
            re_info,
        });
        Ok(())
    }

    fn apply_graphics_op(&mut self, op: &Op) -> Result<bool, EditError> {
        match op.op.as_slice() {
            b"q" => {
                if self.stack.len() >= 1024 {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                self.stack.push(self.state.clone());
            }
            b"Q" => {
                if let Some(prev) = self.stack.pop() {
                    self.state = prev;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    self.state.matrix = Matrix(m).then(&self.state.matrix);
                }
            }
            b"w" => {
                if let Some(w) = op.num(0)
                    && w.is_finite()
                    && w >= 0.0
                {
                    self.state.width = w;
                }
            }
            b"RG" => {
                if let Some(rgb) = op.nums::<3>()
                    && rgb.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                {
                    self.state.color = Some(rgb);
                }
            }
            b"G" => {
                if let Some(g) = op.num(0)
                    && g.is_finite()
                    && (0.0..=1.0).contains(&g)
                {
                    self.state.color = Some([g, g, g]);
                }
            }
            b"K" => {
                if let Some([c, m, y, k]) = op.nums::<4>()
                    && [c, m, y, k].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                {
                    self.state.color = Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]);
                }
            }
            b"CS" | b"SC" | b"SCN" => self.state.color = None,
            b"d" => {}
            _ => return Ok(false),
        }
        self.bump_group();
        Ok(true)
    }

    fn apply_rect_op(&mut self, idx: usize, op: &Op) -> Result<(), EditError> {
        self.bump_group();
        let Some([x, y, w, h]) = op.nums::<4>() else {
            return Ok(());
        };
        let p0 = [x, y];
        let u0 = self.state.matrix.apply(x, y);
        let u1 = self.state.matrix.apply(x + w, y);
        let u2 = self.state.matrix.apply(x + w, y + h);
        let u3 = self.state.matrix.apply(x, y + h);
        let corners = [[u0.0, u0.1], [u1.0, u1.1], [u2.0, u2.1], [u3.0, u3.1]];
        for i in 0..4 {
            self.push_segment(corners[i], corners[(i + 1) % 4], None, idx, Some((idx, i)))?;
        }
        self.current_point = Some((p0, [u0.0, u0.1]));
        self.subpath_start = Some((p0, [u0.0, u0.1]));
        // `re` is a complete closed subpath, not a standalone `m` moveto for any subsequent `l`.
        self.current_m_idx = None;
        Ok(())
    }

    fn apply_stroke_op(&mut self, idx: usize, op: &Op) -> Result<(), EditError> {
        self.bump_group();
        if matches!(op.op.as_slice(), b"s" | b"b" | b"b*")
            && let (Some((_, prev_user)), Some((_, start_user))) = (self.current_point, self.subpath_start)
        {
            self.push_segment(prev_user, start_user, self.current_m_idx, idx, None)?;
        }
        for seg in self.pending_segs.drain(..) {
            if self.raw_lines.len() >= MAX_RAW_SEGMENTS {
                return Err(EditError::Invalid("the page has too many line segments to edit as a group".into()));
            }
            self.raw_lines.push(ParsedLine {
                start: seg.start_user,
                end: seg.end_user,
                thickness: seg.thickness,
                color: seg.color,
                matrix: seg.matrix,
                m_op_idx: seg.m_op_idx,
                l_op_indices: vec![seg.l_op_idx],
                stroke_op_idx: idx,
                straight_group: seg.straight_group,
                re_info: seg.re_info,
            });
        }
        self.current_point = None;
        self.subpath_start = None;
        self.current_m_idx = None;
        Ok(())
    }

    fn apply_path_op(&mut self, idx: usize, op: &Op) -> Result<(), EditError> {
        match op.op.as_slice() {
            b"m" => {
                self.bump_group();
                self.current_m_idx = None;
                if let Some([x, y]) = op.nums::<2>() {
                    let (ux, uy) = self.state.matrix.apply(x, y);
                    let pt = ([x, y], [ux, uy]);
                    self.current_point = Some(pt);
                    self.subpath_start = Some(pt);
                    self.current_m_idx = Some(idx);
                }
            }
            b"l" => {
                if let Some([x, y]) = op.nums::<2>() {
                    let (ux, uy) = self.state.matrix.apply(x, y);
                    if let Some((_, prev_user)) = self.current_point {
                        self.push_segment(prev_user, [ux, uy], self.current_m_idx, idx, None)?;
                    }
                    self.current_point = Some(([x, y], [ux, uy]));
                }
            }
            b"h" => {
                self.bump_group();
                if let (Some((_, prev_user)), Some((start_local, start_user))) = (self.current_point, self.subpath_start) {
                    self.push_segment(prev_user, start_user, self.current_m_idx, idx, None)?;
                    self.current_point = Some((start_local, start_user));
                }
                self.current_m_idx = None;
                self.bump_group();
            }
            b"c" => {
                self.bump_group();
                if let Some(pts) = op.nums::<6>() {
                    let (ux, uy) = self.state.matrix.apply(pts[4], pts[5]);
                    self.current_point = Some(([pts[4], pts[5]], [ux, uy]));
                }
            }
            b"v" | b"y" => {
                self.bump_group();
                if let Some(pts) = op.nums::<4>() {
                    let (ux, uy) = self.state.matrix.apply(pts[2], pts[3]);
                    self.current_point = Some(([pts[2], pts[3]], [ux, uy]));
                }
            }
            b"re" => self.apply_rect_op(idx, op)?,
            b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => self.apply_stroke_op(idx, op)?,
            b"f" | b"f*" | b"F" | b"n" => {
                self.bump_group();
                self.pending_segs.clear();
                self.current_point = None;
                self.subpath_start = None;
                self.current_m_idx = None;
            }
            _ => {}
        }
        Ok(())
    }
}

fn parse_page_lines(streams: &[ContentStream]) -> Result<Vec<ParsedLine>, EditError> {
    let pieces = crate::text::joined(streams);
    let parsed = pieces.parse();
    let mut scan = LineScan::new();
    for (idx, op) in parsed.ops.iter().enumerate() {
        if !scan.apply_graphics_op(op)? {
            scan.apply_path_op(idx, op)?;
        }
    }
    let merged = merge_collinear_parsed(scan.raw_lines);
    if merged.len() > MAX_PAGE_LINES {
        return Err(EditError::Invalid("this page has too many line segments to edit as a group".into()));
    }
    Ok(merged)
}

fn merge_collinear_parsed(raw: Vec<ParsedLine>) -> Vec<ParsedLine> {
    let mut out: Vec<ParsedLine> = Vec::with_capacity(raw.len());
    for item in raw {
        if let Some(last) = out.last_mut() {
            let ls = last.start;
            let le = last.end;
            let lw = last.thickness;
            if last.re_info.is_none()
                && item.re_info.is_none()
                && last.straight_group == item.straight_group
                && (lw - item.thickness).abs() < MERGE_DIST_EPS
                && endpoints_merge(le, item.start)
                && collinear(ls, le, item.end)
                && last.matrix == item.matrix
                && last.stroke_op_idx == item.stroke_op_idx
                && last.m_op_idx == item.m_op_idx
            {
                last.end = item.end;
                last.l_op_indices.extend(item.l_op_indices);
                continue;
            }
        }
        out.push(item);
    }
    out
}

/// Enumerate stroked line segments on `page`, merging collinear adjacent ones.
pub fn editable_lines(doc: &Document, page: usize) -> Result<Vec<LineSegment>, EditError> {
    let streams = checked_streams(doc, page)?;
    let parsed_lines = parse_page_lines(&streams)?;
    Ok(parsed_lines
        .into_iter()
        .enumerate()
        .map(|(index, pl)| LineSegment {
            page,
            index,
            start: pl.start,
            end: pl.end,
            thickness: pl.thickness,
            stroke_color: pl.color,
            rect_op: pl.re_info.map(|(re_idx, _)| re_idx),
        })
        .collect())
}

#[cfg(test)]
fn lines_for_stream_bytes(stream_bytes: &[u8]) -> Result<Vec<LineSegment>, EditError> {
    let mut doc = Document::new_empty();
    let pages = doc
        .get(doc.root().ok_or_else(|| EditError::Invalid("no root".into()))?)
        .as_dict()
        .ok_or_else(|| EditError::Invalid("no root dict".into()))?
        .reference(b"Pages")
        .ok_or_else(|| EditError::Invalid("no pages".into()))?;
    let stream = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), stream_bytes.to_vec()))));
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
    page.set(b"Contents".to_vec(), Object::Array(vec![stream]));
    let page = doc.add(page);
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })?;
    editable_lines(&doc, 0)
}

fn endpoints_merge(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < MERGE_DIST_EPS && (a[1] - b[1]).abs() < MERGE_DIST_EPS
}

fn collinear(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let v1 = [b[0] - a[0], b[1] - a[1]];
    let v2 = [c[0] - b[0], c[1] - b[1]];
    let dot = v1[0] * v2[0] + v1[1] * v2[1];
    let cross = v1[0] * v2[1] - v1[1] * v2[0];
    let mag = (v1[0] * v1[0] + v1[1] * v1[1]).sqrt() * (v2[0] * v2[0] + v2[1] * v2[1]).sqrt();
    mag > f64::EPSILON && dot > 0.0 && (cross / mag).abs() < MERGE_ANGLE_EPS
}

#[cfg(test)]
mod line_tests {
    use super::*;

    fn doc_with_stream(stream_bytes: &[u8]) -> Document {
        let mut doc = Document::new_empty();
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        let stream = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), stream_bytes.to_vec()))));
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
        page.set(b"Contents".to_vec(), Object::Array(vec![stream]));
        let page = doc.add(page);
        doc.update_dict(pages, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
        doc
    }

    #[test]
    fn bbox_is_normalised() {
        let l = LineSegment { page: 0, index: 0, start: [10.0, 20.0], end: [5.0, 8.0], thickness: 1.0, stroke_color: None, rect_op: None };
        assert_eq!(l.bbox(), [5.0, 8.0, 10.0, 20.0]);
    }

    #[test]
    fn distance_to_segment_endpoints_and_middle() {
        let l = LineSegment { page: 0, index: 0, start: [0.0, 0.0], end: [10.0, 0.0], thickness: 1.0, stroke_color: None, rect_op: None };
        assert!(l.distance_sq_to([5.0, 0.0]) < 1e-9);
        assert!((l.distance_sq_to([5.0, 3.0]) - 9.0).abs() < 1e-9);
        assert!((l.distance_sq_to([13.0, 0.0]) - 9.0).abs() < 1e-9);
    }

    #[test]
    fn merge_collinear_joins_two_halves() {
        let lines = lines_for_stream_bytes(b"0 0 m 5 0 l 10 0 l S\n").unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start, [0.0, 0.0]);
        assert_eq!(lines[0].end, [10.0, 0.0]);
    }

    #[test]
    fn merge_does_not_join_perpendicular() {
        let lines = lines_for_stream_bytes(b"0 0 m 5 0 l 5 5 l S\n").unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn merge_does_not_join_different_widths() {
        let lines = lines_for_stream_bytes(b"1 w 0 0 m 5 0 l S\n2 w 5 0 m 10 0 l S\n").unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn merge_does_not_join_reversed() {
        let lines = lines_for_stream_bytes(b"0 0 m 5 0 l 2 0 l S\n").unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn merge_does_not_join_across_curve() {
        let lines = lines_for_stream_bytes(b"0 0 m 10 0 l 20 10 0 20 10 0 c 20 0 l S\n").unwrap();
        assert_eq!(lines.len(), 2);
        let lines_v = lines_for_stream_bytes(b"0 0 m 10 0 l 20 10 10 0 v 20 0 l S\n").unwrap();
        assert_eq!(lines_v.len(), 2);
        let lines_y = lines_for_stream_bytes(b"0 0 m 10 0 l 20 10 10 0 y 20 0 l S\n").unwrap();
        assert_eq!(lines_y.len(), 2);
    }

    #[test]
    fn merge_allows_over_max_raw_collinear_segments() {
        let mut s = String::from("0 0 m ");
        for i in 1..=10_005 {
            s.push_str(&format!("{i} 0 l "));
        }
        s.push_str("S\n");
        let lines = lines_for_stream_bytes(s.as_bytes()).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start, [0.0, 0.0]);
        assert_eq!(lines[0].end, [10_005.0, 0.0]);
    }

    #[test]
    fn unknown_color_operator_clears_stroke_color() {
        let lines = lines_for_stream_bytes(b"/DeviceRGB CS 1 0 0 SC 0 0 m 10 0 l S\n").unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].stroke_color, None);
    }

    #[test]
    fn test_set_line_properties_rejects_nan_color() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        let res = set_line_properties(&mut doc, 0, target, Some([f64::NAN, 0.0, 0.0]), Some(1.0), None);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("color"), "expected error containing 'color', got: {err}");
    }

    #[test]
    fn test_set_line_properties_rejects_out_of_range_width() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        let res = set_line_properties(&mut doc, 0, target, None, Some(0.1), None);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("width"), "expected error containing 'width', got: {err}");

        let res2 = set_line_properties(&mut doc, 0, target, None, Some(25.0), None);
        let err2 = res2.unwrap_err().to_string();
        assert!(err2.contains("width"), "expected error containing 'width', got: {err2}");
    }

    #[test]
    fn test_set_line_properties_rejects_oversized_dash() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        let res = set_line_properties(&mut doc, 0, target, None, Some(1.0), Some(vec![1.0; 9]));
        let err = res.unwrap_err().to_string();
        assert!(err.contains("dash"), "expected error containing 'dash', got: {err}");
    }

    #[test]
    fn test_set_line_properties_rejects_wrong_kind() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::Text, index: 0 };
        let res = set_line_properties(&mut doc, 0, target, None, Some(1.0), None);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("kind"), "expected error containing 'kind', got: {err}");
    }

    #[test]
    fn test_update_line_endpoints_rejects_nan() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        let res = update_line_endpoints(&mut doc, 0, target, [f64::NAN, 0.0], [10.0, 0.0]);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("finite") || err.contains("coordinate"), "expected error containing 'finite'/'coordinate', got: {err}");
    }

    #[test]
    fn test_update_line_endpoints_rejects_zero_length() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        let res = update_line_endpoints(&mut doc, 0, target, [5.0, 5.0], [5.0, 5.0]);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("zero-length") || err.contains("identical"), "expected error containing 'zero-length'/'identical', got: {err}");
    }

    #[test]
    fn test_update_line_endpoints_rejects_wrong_kind() {
        let mut doc = Document::new_empty();
        let target = ObjectTarget { kind: ObjectKind::Text, index: 0 };
        let res = update_line_endpoints(&mut doc, 0, target, [0.0, 0.0], [10.0, 0.0]);
        let err = res.unwrap_err().to_string();
        assert!(err.contains("kind"), "expected error containing 'kind', got: {err}");
    }

    #[test]
    fn test_vector_line_round_trip_edit_and_properties() {
        let mut doc = doc_with_stream(b"10 20 m 100 20 l S\n");

        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start, [10.0, 20.0]);
        assert_eq!(lines[0].end, [100.0, 20.0]);
        assert_eq!(lines[0].rect_op, None);
        assert!((lines[0].thickness - 1.0).abs() < 1e-6);

        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        update_line_endpoints(&mut doc, 0, target, [15.0, 25.0], [105.0, 25.0]).unwrap();

        let lines2 = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines2.len(), 1);
        assert_eq!(lines2[0].start, [15.0, 25.0]);
        assert_eq!(lines2[0].end, [105.0, 25.0]);

        set_line_properties(&mut doc, 0, target, Some([1.0, 0.0, 0.0]), Some(3.5), Some(vec![4.0, 2.0])).unwrap();

        let lines3 = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines3.len(), 1);
        assert!((lines3[0].thickness - 3.5).abs() < 1e-6);
    }

    #[test]
    fn test_vector_line_multi_subpath_isolation() {
        let mut doc = doc_with_stream(b"10 20 m 100 20 l 10 40 m 100 40 l S\n");

        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 2);

        // Edit properties of line 1 (the second subpath):
        let target1 = ObjectTarget { kind: ObjectKind::VectorLine, index: 1 };
        set_line_properties(&mut doc, 0, target1, Some([1.0, 0.0, 0.0]), Some(4.0), None).unwrap();

        let lines_after = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines_after.len(), 2, "both lines must be preserved");
        assert_eq!(lines_after[0].start, [10.0, 20.0]);
        assert_eq!(lines_after[1].start, [10.0, 40.0]);
        assert!((lines_after[1].thickness - 4.0).abs() < 1e-6);
    }

    #[test]
    fn test_connected_multisegment_and_filled_paths_rejected() {
        // Path with connected segments (0 0 m 10 0 l 10 10 l S) and a filled-and-stroked path (0 0 m 20 0 l 20 20 l B)
        let mut doc = doc_with_stream(b"0 0 m 10 0 l 10 10 l S\n0 0 m 20 0 l 20 20 l B\n");

        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 4);

        // Segment 0 of connected path
        let target0 = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        assert!(set_line_properties(&mut doc, 0, target0, Some([1.0, 0.0, 0.0]), Some(2.0), None).is_err());
        assert!(update_line_endpoints(&mut doc, 0, target0, [0.0, 0.0], [15.0, 0.0]).is_err());

        // Segment 2 of filled-and-stroked path (B operator)
        let target2 = ObjectTarget { kind: ObjectKind::VectorLine, index: 2 };
        assert!(set_line_properties(&mut doc, 0, target2, Some([1.0, 0.0, 0.0]), Some(2.0), None).is_err());

        // Ensure document contents were not corrupted
        let lines_after = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines_after.len(), 4);
    }

    #[test]
    fn test_stroked_rectangle_edges_and_endpoint_edit() {
        let mut doc = doc_with_stream(b"10 20 100 50 re S\n");

        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 4, "stroked rectangle must produce 4 editable line edges");
        assert!(lines.iter().all(|l| l.rect_op == Some(0)));

        // Edge 1 is the right vertical edge: start [110.0, 20.0], end [110.0, 70.0]
        assert_eq!(lines[1].start, [110.0, 20.0]);
        assert_eq!(lines[1].end, [110.0, 70.0]);

        // Move right edge to x = 130.0
        let target1 = ObjectTarget { kind: ObjectKind::VectorLine, index: 1 };
        update_line_endpoints(&mut doc, 0, target1, [130.0, 20.0], [130.0, 70.0]).unwrap();

        let lines_after = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines_after.len(), 4);
        assert_eq!(lines_after[1].start, [130.0, 20.0]);
        assert_eq!(lines_after[1].end, [130.0, 70.0]);
    }

    #[test]
    fn test_rectangle_resize_collapse_and_crossing_rejected() {
        let mut doc = doc_with_stream(b"10 20 100 50 re S\n");

        let target_right = ObjectTarget { kind: ObjectKind::VectorLine, index: 1 };
        let target_top = ObjectTarget { kind: ObjectKind::VectorLine, index: 2 };
        let target_left = ObjectTarget { kind: ObjectKind::VectorLine, index: 3 };

        // Width collapse (right edge dragged to x = 10) must be rejected.
        assert!(update_line_endpoints(&mut doc, 0, target_right, [10.0, 20.0], [10.0, 70.0]).is_err());
        // Height collapse (top edge dragged to y = 20) must be rejected.
        assert!(update_line_endpoints(&mut doc, 0, target_top, [110.0, 20.0], [10.0, 20.0]).is_err());
        // Vertical edge crossing opposite edge (right edge past x = 10 to x = 9, or left past 110 to 120) must be rejected.
        assert!(update_line_endpoints(&mut doc, 0, target_right, [9.0, 20.0], [9.0, 70.0]).is_err());
        assert!(update_line_endpoints(&mut doc, 0, target_left, [120.0, 20.0], [120.0, 70.0]).is_err());

        // Document must remain completely unchanged.
        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines[1].start, [110.0, 20.0]);
        assert_eq!(lines[2].end, [10.0, 70.0]);
    }

    #[test]
    fn test_re_does_not_act_as_moveto_for_subsequent_l() {
        let mut doc = doc_with_stream(b"10 20 100 50 re 150 20 l S\n");
        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[4].start, [10.0, 20.0]);
        assert_eq!(lines[4].end, [150.0, 20.0]);
        assert_eq!(lines[4].rect_op, None);

        let trailing = ObjectTarget { kind: ObjectKind::VectorLine, index: 4 };
        assert!(set_line_properties(&mut doc, 0, trailing, Some([1.0, 0.0, 0.0]), Some(3.0), None).is_err());
        assert!(update_line_endpoints(&mut doc, 0, trailing, [20.0, 30.0], [160.0, 30.0]).is_err());

        // Original rectangle and trailing segment remain completely intact.
        let after = editable_lines(&doc, 0).unwrap();
        assert_eq!(after.len(), 5);
        assert_eq!(after[0].start, [10.0, 20.0]);
        assert_eq!(after[1].start, [110.0, 20.0]);
        assert_eq!(after[4].end, [150.0, 20.0]);

        // A subsequent subpath with an explicit `m` remains editable without corrupting the preceding `re`.
        let mut doc_with_m = doc_with_stream(b"10 20 100 50 re 200 20 m 300 20 l S\n");
        set_line_properties(&mut doc_with_m, 0, trailing, Some([1.0, 0.0, 0.0]), Some(3.0), None).unwrap();
        let after_m = editable_lines(&doc_with_m, 0).unwrap();
        assert_eq!(after_m.len(), 5);
        assert!((after_m[0].thickness - 1.0).abs() < 1e-6);
        assert!((after_m[4].thickness - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_negative_re_dimensions_preserved_on_resize() {
        let mut doc = doc_with_stream(b"110 70 -100 -50 re S\n");
        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 4);

        // Edge 0: [110, 70] -> [10, 70] (move to y = 80)
        update_line_endpoints(&mut doc, 0, ObjectTarget { kind: ObjectKind::VectorLine, index: 0 }, [110.0, 80.0], [10.0, 80.0]).unwrap();
        // Edge 1: [10, 80] -> [10, 20] (move to x = 5)
        update_line_endpoints(&mut doc, 0, ObjectTarget { kind: ObjectKind::VectorLine, index: 1 }, [5.0, 80.0], [5.0, 20.0]).unwrap();
        // Edge 2: [5, 20] -> [110, 20] (move to y = 10)
        update_line_endpoints(&mut doc, 0, ObjectTarget { kind: ObjectKind::VectorLine, index: 2 }, [5.0, 10.0], [110.0, 10.0]).unwrap();
        // Edge 3: [110, 10] -> [110, 80] (move to x = 120)
        update_line_endpoints(&mut doc, 0, ObjectTarget { kind: ObjectKind::VectorLine, index: 3 }, [120.0, 10.0], [120.0, 80.0]).unwrap();

        let after = editable_lines(&doc, 0).unwrap();
        assert_eq!(after.len(), 4);
        assert_eq!(after[0].start, [120.0, 80.0]);
        assert_eq!(after[0].end, [5.0, 80.0]);
        assert_eq!(after[2].start, [5.0, 10.0]);
        assert_eq!(after[2].end, [120.0, 10.0]);

        // Crossing opposite edge on negative-dimension `re` must still be rejected.
        assert!(update_line_endpoints(&mut doc, 0, ObjectTarget { kind: ObjectKind::VectorLine, index: 1 }, [130.0, 80.0], [130.0, 10.0]).is_err());
    }

    #[test]
    fn test_set_line_properties_preserves_closed_stroke_s() {
        let mut doc = doc_with_stream(b"10 20 m 100 20 l s\n");
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        set_line_properties(&mut doc, 0, target, Some([1.0, 0.0, 0.0]), Some(2.5), None).unwrap();

        // Both the explicit segment and the implicit closing segment from `s` must survive.
        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 2);
        assert!((lines[0].thickness - 2.5).abs() < 1e-6);
        assert!((lines[1].thickness - 2.5).abs() < 1e-6);

        let streams = checked_streams(&doc, 0).unwrap();
        let ops = crate::text::joined(&streams).parse().ops;
        assert!(ops.iter().any(|op| op.is("s")), "closed stroke `s` must be preserved");

        // Preceding `re` + standalone line ended by `s` must preserve both `re` and `s`.
        let mut doc_re_s = doc_with_stream(b"10 20 100 50 re 200 20 m 300 20 l s\n");
        let target_line = ObjectTarget { kind: ObjectKind::VectorLine, index: 4 };
        set_line_properties(&mut doc_re_s, 0, target_line, Some([0.0, 0.0, 1.0]), Some(3.0), None).unwrap();
        let lines_re_s = editable_lines(&doc_re_s, 0).unwrap();
        assert_eq!(lines_re_s.len(), 6);
        assert!((lines_re_s[0].thickness - 1.0).abs() < 1e-6);
        assert!((lines_re_s[4].thickness - 3.0).abs() < 1e-6);
        assert!((lines_re_s[5].thickness - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_update_line_endpoints_rejects_degenerate_collapse() {
        let mut doc = doc_with_stream(b"10 20 m 100 20 l 10 40 m 100 40 l S\n");
        let target = ObjectTarget { kind: ObjectKind::VectorLine, index: 0 };
        assert!(update_line_endpoints(&mut doc, 0, target, [10.0, 20.0], [10.00005, 20.0]).is_err());

        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].end, [100.0, 20.0]);
        assert_eq!(lines[1].end, [100.0, 40.0]);
    }

    #[test]
    fn test_dashed_lines_within_max_page_lines_allowed() {
        let mut s = String::from("[1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16] 0 d\n");
        for i in 0..7_000 {
            let y = (i % 700) + 10;
            let x = (i / 700) * 50 + 10;
            s.push_str(&format!("{x} {y} m {} {y} l ", x + 20));
        }
        s.push_str("S\n");
        let lines = lines_for_stream_bytes(s.as_bytes()).unwrap();
        assert_eq!(lines.len(), 7_000);
    }
}
