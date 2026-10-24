//! Mixed-object inventory and atomic translation in Edit a PDF. Source indexes are resolved
//! together before mutation. Added content appears once, not again as a paragraph/image.

use std::collections::{BTreeMap, BTreeSet};

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Dict, Document, Object};

use crate::{Content, EditError};

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

/// Set stroke properties (color, width, dash pattern) for a vector line segment.
pub fn set_line_properties(
    doc: &mut Document,
    page: usize,
    target: ObjectTarget,
    color: Option<[f64; 3]>,
    width: Option<f64>,
    dash: Option<Vec<f64>>,
) -> Result<(), EditError> {
    // 1. Validate inputs FIRST (before any mutation):
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
    if let Some(ref d) = dash
        && (d.len() > 8 || !d.iter().all(|v| v.is_finite() && (0.0..=100.0).contains(v)))
    {
        return Err(EditError::Invalid("dash values must be finite, between 0.0 and 100.0, and have length <= 8".into()));
    }

    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = checked_streams(doc, page)?;
    let parsed_lines = parse_page_lines(&streams)?;
    let target_line = parsed_lines.get(target.index).ok_or_else(|| EditError::Invalid("line no longer exists".into()))?;

    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;

    let m_op = ops.get(target_line.m_op_idx).ok_or_else(|| EditError::Invalid("line moveto operator missing".into()))?;
    let last_l_idx = *target_line.l_op_indices.last().ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;
    let last_l_op = ops.get(last_l_idx).ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;
    let stroke_op = ops.get(target_line.stroke_op_idx).ok_or_else(|| EditError::Invalid("line stroke operator missing".into()))?;

    // Check if there are preceding path construction operators before m_op within the current path.
    let mut has_preceding_path = false;
    for idx in (0..target_line.m_op_idx).rev() {
        let op = &ops[idx];
        match op.op.as_slice() {
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => {
                has_preceding_path = true;
                break;
            }
            b"S" | b"s" | b"f" | b"f*" | b"F" | b"b" | b"b*" | b"B" | b"B*" | b"n" | b"q" | b"Q" => {
                break;
            }
            _ => {}
        }
    }

    // Check if there are subsequent path construction operators after target_line before stroke_op.
    let has_subsequent_path = ((last_l_idx + 1)..target_line.stroke_op_idx).any(|idx| {
        let op = &ops[idx];
        matches!(op.op.as_slice(), b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
    });

    let mut prefix_bytes = Vec::new();
    if has_preceding_path {
        prefix_bytes.extend_from_slice(b"\nS\n");
    }

    let mut prefix_ops: Vec<Op> = Vec::new();
    prefix_ops.push(Op::new("q", Vec::new()));

    if let Some(c) = color {
        prefix_ops.push(Op::new("RG", vec![pdfcraft_content::num(c[0]), pdfcraft_content::num(c[1]), pdfcraft_content::num(c[2])]));
    }
    if let Some(w) = width {
        let [a, b, c, d, _, _] = target_line.matrix.0;
        let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
        let local_w = if scale > 1e-6 { w / scale } else { w };
        prefix_ops.push(Op::new("w", vec![pdfcraft_content::num(local_w)]));
    }
    if let Some(ref d) = dash {
        let dash_array = Object::Array(d.iter().copied().map(pdfcraft_content::num).collect());
        prefix_ops.push(Op::new("d", vec![dash_array, pdfcraft_content::num(0.0)]));
    }

    prefix_bytes.push(b'\n');
    prefix_bytes.extend_from_slice(&pdfcraft_content::serialize_ops(&prefix_ops));
    if prefix_bytes.last() != Some(&b'\n') && prefix_bytes.last() != Some(&b' ') {
        prefix_bytes.push(b'\n');
    }

    let mut suffix_bytes = Vec::new();
    suffix_bytes.extend_from_slice(b"\nS\nQ\n");

    let mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
    edits.push((m_op.span.start..m_op.span.start, prefix_bytes));
    edits.push((last_l_op.span.end..last_l_op.span.end, suffix_bytes));

    if !has_subsequent_path {
        // If there are no subsequent segments in this path, replace original stroke_op
        edits.push((stroke_op.span.clone(), b"\n".to_vec()));
    }

    edits.sort_by_key(|(span, _)| span.start);

    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// Update endpoints for a vector line segment on a page.
pub fn update_line_endpoints(doc: &mut Document, page: usize, target: ObjectTarget, new_start: [f64; 2], new_end: [f64; 2]) -> Result<(), EditError> {
    // 1. Validate inputs FIRST:
    if target.kind != ObjectKind::VectorLine {
        return Err(EditError::Invalid("expected target kind VectorLine".into()));
    }
    if !new_start.iter().chain(new_end.iter()).all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) {
        return Err(EditError::Invalid("coordinates must be finite and within MAX_COORDINATE".into()));
    }
    if new_start == new_end {
        return Err(EditError::Invalid("new_start and new_end must not be identical (zero-length line)".into()));
    }
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;

    // 2. Re-derive the line inventory:
    let streams = checked_streams(doc, page)?;
    let parsed_lines = parse_page_lines(&streams)?;
    let target_line = parsed_lines.get(target.index).ok_or_else(|| EditError::Invalid("line no longer exists".into()))?;

    let inv = target_line.matrix.invert().ok_or_else(|| EditError::Invalid("line coordinate transform is singular".into()))?;
    let (local_start_x, local_start_y) = inv.apply(new_start[0], new_start[1]);
    let (local_end_x, local_end_y) = inv.apply(new_end[0], new_end[1]);

    if !local_start_x.is_finite() || !local_start_y.is_finite() || !local_end_x.is_finite() || !local_end_y.is_finite() {
        return Err(EditError::Invalid("transformed coordinates must be finite".into()));
    }

    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;

    let m_op = ops.get(target_line.m_op_idx).ok_or_else(|| EditError::Invalid("line moveto operator missing".into()))?;
    let last_l_idx = *target_line.l_op_indices.last().ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;
    let last_l_op = ops.get(last_l_idx).ok_or_else(|| EditError::Invalid("line lineto operator missing".into()))?;

    let mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();

    let mut m_bytes =
        pdfcraft_content::serialize_ops(&[Op::new("m", vec![pdfcraft_content::num(local_start_x), pdfcraft_content::num(local_start_y)])]);
    if m_bytes.last() == Some(&b'\n') {
        m_bytes.pop();
    }
    edits.push((m_op.span.clone(), m_bytes));

    if target_line.l_op_indices.len() > 1 {
        for &l_idx in &target_line.l_op_indices[..target_line.l_op_indices.len() - 1] {
            if let Some(l_op) = ops.get(l_idx) {
                edits.push((l_op.span.clone(), Vec::new()));
            }
        }
    }

    let mut l_bytes = pdfcraft_content::serialize_ops(&[Op::new("l", vec![pdfcraft_content::num(local_end_x), pdfcraft_content::num(local_end_y)])]);
    if l_bytes.last() == Some(&b'\n') {
        l_bytes.pop();
    }
    edits.push((last_l_op.span.clone(), l_bytes));

    edits.sort_by_key(|(span, _)| span.start);

    let data = pieces.splice(edits);
    let contents = crate::text::rewritten_contents(doc, &streams, data);
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
    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let owned: BTreeSet<_> = added.iter().map(|a| a.obj).collect();
    let mut geometry = BTreeMap::new();
    let mut first = BTreeMap::new();
    let mut matrix = Matrix::IDENTITY;
    let mut stack = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let (stream, end) = pieces.pieces_of(op);
        let object = streams.get(stream).and_then(|(r, _)| r.as_ref());
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
    let contents = crate::text::rewritten_contents(doc, &streams, data);
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

/// A missing or undecodable preceding stream can hide graphics state that affects every
/// later item. Refuse group editing rather than inventing an inventory from partial data.
fn checked_streams(doc: &Document, page: usize) -> Result<Vec<(Object, Vec<u8>)>, EditError> {
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
    let mut depth = 0usize;
    let mut operations = 0usize;
    let mut streams = Vec::with_capacity(contents.len());
    for object in contents {
        let resolved = doc.resolve(&object);
        let Object::Stream(stream) = &*resolved else {
            return Err(EditError::Invalid("a page content stream is missing".into()));
        };
        let bytes = stream
            .decoded_within(remaining.min(64 * 1024 * 1024))
            .map_err(|_| EditError::Invalid("the page content cannot be decoded within its size limit".into()))?;
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
        streams.push((object, bytes));
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
        (dx * dx + dy * dy).sqrt()
    }

    /// Squared distance from a user-space point to this segment.
    pub fn distance_sq_to(&self, p: [f64; 2]) -> f64 {
        let vx = self.end[0] - self.start[0];
        let vy = self.end[1] - self.start[1];
        let len_sq = vx * vx + vy * vy;
        let t = if len_sq <= f64::EPSILON { 0.0 } else { (((p[0] - self.start[0]) * vx + (p[1] - self.start[1]) * vy) / len_sq).clamp(0.0, 1.0) };
        let px = self.start[0] + t * vx;
        let py = self.start[1] + t * vy;
        let dx = p[0] - px;
        let dy = p[1] - py;
        dx * dx + dy * dy
    }
}

#[derive(Clone, Debug)]
struct ParsedLine {
    start: [f64; 2],
    end: [f64; 2],
    thickness: f64,
    #[allow(dead_code)]
    color: [f64; 3],
    #[allow(dead_code)]
    dash: (Vec<f64>, f64),
    matrix: Matrix,
    m_op_idx: usize,
    l_op_indices: Vec<usize>,
    stroke_op_idx: usize,
}

fn parse_page_lines(streams: &[(Object, Vec<u8>)]) -> Result<Vec<ParsedLine>, EditError> {
    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let parsed = pieces.parse();
    let ops = &parsed.ops;

    #[derive(Clone)]
    struct GState {
        matrix: Matrix,
        width: f64,
        color: [f64; 3],
        dash: (Vec<f64>, f64),
    }

    let mut state = GState {
        matrix: Matrix::IDENTITY,
        width: 1.0,
        color: [0.0, 0.0, 0.0],
        dash: (Vec::new(), 0.0),
    };
    let mut stack: Vec<GState> = Vec::new();

    struct PendingSeg {
        start_user: [f64; 2],
        end_user: [f64; 2],
        thickness: f64,
        color: [f64; 3],
        dash: (Vec<f64>, f64),
        matrix: Matrix,
        m_op_idx: usize,
        l_op_idx: usize,
    }

    let mut current_point: Option<([f64; 2], [f64; 2])> = None;
    let mut subpath_start: Option<([f64; 2], [f64; 2])> = None;
    let mut current_m_idx: usize = 0;
    let mut pending_segs: Vec<PendingSeg> = Vec::new();
    let mut raw_lines: Vec<ParsedLine> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= 1024 {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                stack.push(state.clone());
            }
            b"Q" => {
                if let Some(prev) = stack.pop() {
                    state = prev;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    state.matrix = Matrix(m).then(&state.matrix);
                }
            }
            b"w" => {
                if let Some(w) = op.num(0)
                    && w.is_finite() && w >= 0.0
                {
                    state.width = w;
                }
            }
            b"RG" => {
                if let Some(rgb) = op.nums::<3>()
                    && rgb.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                {
                    state.color = rgb;
                }
            }
            b"G" => {
                if let Some(g) = op.num(0)
                    && g.is_finite() && (0.0..=1.0).contains(&g)
                {
                    state.color = [g, g, g];
                }
            }
            b"K" => {
                if let Some([c, m, y, k]) = op.nums::<4>()
                    && [c, m, y, k].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                {
                    state.color = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                }
            }
            b"d" => {
                if let Some(arr) = op.operands.first().and_then(Object::as_array) {
                    let dash_vec: Vec<f64> = arr.iter().filter_map(Object::as_f64).collect();
                    let phase = op.num(1).unwrap_or(0.0);
                    state.dash = (dash_vec, phase);
                }
            }
            b"m" => {
                if let Some([x, y]) = op.nums::<2>() {
                    let (ux, uy) = state.matrix.apply(x, y);
                    current_point = Some(([x, y], [ux, uy]));
                    subpath_start = Some(([x, y], [ux, uy]));
                    current_m_idx = idx;
                }
            }
            b"l" => {
                if let Some([x, y]) = op.nums::<2>() {
                    let (ux, uy) = state.matrix.apply(x, y);
                    if let Some((_prev_local, prev_user)) = current_point {
                        let dx = ux - prev_user[0];
                        let dy = uy - prev_user[1];
                        if (dx * dx + dy * dy).sqrt() > 1e-4 {
                            let [a, b, c, d, _, _] = state.matrix.0;
                            let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
                            let thickness = if scale > 1e-6 { (state.width * scale).max(0.1) } else { state.width.max(0.1) };
                            pending_segs.push(PendingSeg {
                                start_user: prev_user,
                                end_user: [ux, uy],
                                thickness,
                                color: state.color,
                                dash: state.dash.clone(),
                                matrix: state.matrix,
                                m_op_idx: current_m_idx,
                                l_op_idx: idx,
                            });
                        }
                    }
                    current_point = Some(([x, y], [ux, uy]));
                }
            }
            b"h" => {
                if let (Some((_prev_local, prev_user)), Some((start_local, start_user))) = (current_point, subpath_start) {
                    let dx = start_user[0] - prev_user[0];
                    let dy = start_user[1] - prev_user[1];
                    if (dx * dx + dy * dy).sqrt() > 1e-4 {
                        let [a, b, c, d, _, _] = state.matrix.0;
                        let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
                        let thickness = if scale > 1e-6 { (state.width * scale).max(0.1) } else { state.width.max(0.1) };
                        pending_segs.push(PendingSeg {
                            start_user: prev_user,
                            end_user: start_user,
                            thickness,
                            color: state.color,
                            dash: state.dash.clone(),
                            matrix: state.matrix,
                            m_op_idx: current_m_idx,
                            l_op_idx: idx,
                        });
                    }
                    current_point = Some((start_local, start_user));
                }
            }
            b"c" => {
                if let Some(pts) = op.nums::<6>() {
                    let (ux, uy) = state.matrix.apply(pts[4], pts[5]);
                    current_point = Some(([pts[4], pts[5]], [ux, uy]));
                }
            }
            b"v" | b"y" => {
                if let Some(pts) = op.nums::<4>() {
                    let (ux, uy) = state.matrix.apply(pts[2], pts[3]);
                    current_point = Some(([pts[2], pts[3]], [ux, uy]));
                }
            }
            b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                if matches!(op.op.as_slice(), b"s" | b"b" | b"b*")
                    && let (Some((_prev_local, prev_user)), Some((_start_local, start_user))) = (current_point, subpath_start)
                {
                    let dx = start_user[0] - prev_user[0];
                    let dy = start_user[1] - prev_user[1];
                    if (dx * dx + dy * dy).sqrt() > 1e-4 {
                        let [a, b, c, d, _, _] = state.matrix.0;
                        let scale = ((a * a + b * b).sqrt() + (c * c + d * d).sqrt()) / 2.0;
                        let thickness = if scale > 1e-6 { (state.width * scale).max(0.1) } else { state.width.max(0.1) };
                        pending_segs.push(PendingSeg {
                            start_user: prev_user,
                            end_user: start_user,
                            thickness,
                            color: state.color,
                            dash: state.dash.clone(),
                            matrix: state.matrix,
                            m_op_idx: current_m_idx,
                            l_op_idx: idx,
                        });
                    }
                }
                for seg in pending_segs.drain(..) {
                    raw_lines.push(ParsedLine {
                        start: seg.start_user,
                        end: seg.end_user,
                        thickness: seg.thickness,
                        color: seg.color,
                        dash: seg.dash,
                        matrix: seg.matrix,
                        m_op_idx: seg.m_op_idx,
                        l_op_indices: vec![seg.l_op_idx],
                        stroke_op_idx: idx,
                    });
                }
                current_point = None;
                subpath_start = None;
            }
            b"f" | b"f*" | b"F" | b"n" => {
                pending_segs.clear();
                current_point = None;
                subpath_start = None;
            }
            _ => {}
        }
    }

    Ok(merge_collinear_parsed(raw_lines))
}

fn merge_collinear_parsed(raw: Vec<ParsedLine>) -> Vec<ParsedLine> {
    let mut out: Vec<ParsedLine> = Vec::with_capacity(raw.len());
    for item in raw {
        if let Some(last) = out.last_mut() {
            let ls = last.start;
            let le = last.end;
            let lw = last.thickness;
            if (lw - item.thickness).abs() < MERGE_DIST_EPS
                && endpoints_merge(le, item.start)
                && collinear(ls, le, item.end)
                && last.matrix == item.matrix
                && last.stroke_op_idx == item.stroke_op_idx
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

    if parsed_lines.len() > MAX_PAGE_LINES {
        return Err(EditError::Invalid("this page has too many line segments to edit as a group".into()));
    }

    Ok(parsed_lines
        .into_iter()
        .enumerate()
        .map(|(index, pl)| LineSegment {
            page,
            index,
            start: pl.start,
            end: pl.end,
            thickness: pl.thickness,
        })
        .collect())
}

#[cfg(test)]
fn merge_collinear(raw: &[([f64; 2], [f64; 2], f64)]) -> Vec<([f64; 2], [f64; 2], f64)> {
    let mut out: Vec<([f64; 2], [f64; 2], f64)> = Vec::with_capacity(raw.len());
    for &(s, e, w) in raw {
        if let Some(last) = out.last_mut() {
            let (ls, le, lw) = *last;
            if (lw - w).abs() < MERGE_DIST_EPS && endpoints_merge(le, s) && collinear(ls, le, e) {
                *last = (ls, e, lw);
                continue;
            }
        }
        out.push((s, e, w));
    }
    out
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

    #[test]
    fn bbox_is_normalised() {
        let l = LineSegment { page: 0, index: 0, start: [10.0, 20.0], end: [5.0, 8.0], thickness: 1.0 };
        assert_eq!(l.bbox(), [5.0, 8.0, 10.0, 20.0]);
    }

    #[test]
    fn distance_to_segment_endpoints_and_middle() {
        let l = LineSegment { page: 0, index: 0, start: [0.0, 0.0], end: [10.0, 0.0], thickness: 1.0 };
        assert!(l.distance_sq_to([5.0, 0.0]) < 1e-9);
        assert!((l.distance_sq_to([5.0, 3.0]) - 9.0).abs() < 1e-9);
        assert!((l.distance_sq_to([13.0, 0.0]) - 9.0).abs() < 1e-9);
    }

    #[test]
    fn merge_collinear_joins_two_halves() {
        let raw = vec![([0.0, 0.0], [5.0, 0.0], 1.0), ([5.0, 0.0], [10.0, 0.0], 1.0)];
        let merged = merge_collinear(&raw);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0, [0.0, 0.0]);
        assert_eq!(merged[0].1, [10.0, 0.0]);
    }

    #[test]
    fn merge_does_not_join_perpendicular() {
        let raw = vec![([0.0, 0.0], [5.0, 0.0], 1.0), ([5.0, 0.0], [5.0, 5.0], 1.0)];
        let merged = merge_collinear(&raw);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_does_not_join_different_widths() {
        let raw = vec![([0.0, 0.0], [5.0, 0.0], 1.0), ([5.0, 0.0], [10.0, 0.0], 2.0)];
        let merged = merge_collinear(&raw);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_does_not_join_reversed() {
        let raw = vec![([0.0, 0.0], [5.0, 0.0], 1.0), ([5.0, 0.0], [2.0, 0.0], 1.0)];
        let merged = merge_collinear(&raw);
        assert_eq!(merged.len(), 2);
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
        let mut doc = Document::new_empty();
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        let stream_bytes = b"10 20 m 100 20 l S\n".to_vec();
        let stream = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), stream_bytes))));
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

        let lines = editable_lines(&doc, 0).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].start, [10.0, 20.0]);
        assert_eq!(lines[0].end, [100.0, 20.0]);
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
        let mut doc = Document::new_empty();
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        // 2 subpaths sharing a single stroke operator S:
        let stream_bytes = b"10 20 m 100 20 l 10 40 m 100 40 l S\n".to_vec();
        let stream = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), stream_bytes))));
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
}
