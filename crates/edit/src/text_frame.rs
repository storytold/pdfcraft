//! An explicitly resized paragraph's frame, independent of its glyph bounds.
//! Private marked content keeps it with the text across saves without painting anything.

use super::interp::{from_frame, to_frame};
use super::{
    Dict, Document, EditError, HashMap, LineEdit, Matrix, Object, Op, Pieces, TextBlock, TextLine, content_streams, is_centred, page_dict,
    rewritten_contents, splice,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HorizontalFrame {
    pub op: usize,
    local: [f64; 4],
    ctm: Matrix,
    pub direction: (f64, f64),
    pub bounds: [f64; 2],
    local_vertical: Option<[f64; 4]>,
    pub vertical: Option<[f64; 2]>,
    pub align: crate::added::Align,
}

impl HorizontalFrame {
    /// An unfinished private scope cannot claim the remaining text on the page. Find paired
    /// opens in one reverse pass, without rescanning nested scopes for every candidate frame.
    pub fn closed(ops: &[Op]) -> std::collections::HashSet<usize> {
        let mut closes = 0_usize;
        let mut result = std::collections::HashSet::new();
        for (i, op) in ops.iter().enumerate().rev() {
            if op.is("EMC") {
                closes = closes.saturating_add(1);
            } else if (op.is("BMC") || op.is("BDC")) && closes > 0 {
                closes -= 1;
                if op.is("BDC") && op.name(0) == Some(b"PCTextFrame".as_slice()) {
                    result.insert(i);
                }
            }
        }
        result
    }

    /// Decode finite frame segments at their actual content-stream transform.
    pub fn read(op: &Op, index: usize, ctm: Matrix) -> Option<Self> {
        if !op.is("BDC") || op.name(0) != Some(b"PCTextFrame".as_slice()) {
            return None;
        }
        let a = op.operands.get(1)?.as_dict()?.get(b"Baseline")?.as_array()?;
        if a.len() != 4 {
            return None;
        }
        let local = [a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?, a.get(3)?.as_f64()?];
        if !local.iter().chain(&ctm.0).all(|v| v.is_finite() && v.abs() <= 1e12) {
            return None;
        }
        let (x0, y0) = ctm.apply(local[0], local[1]);
        let (x1, y1) = ctm.apply(local[2], local[3]);
        let width = (x1 - x0).hypot(y1 - y0);
        // Projection through a rotated CTM can put an exact maximum a few ulps over the
        // limit. Normalize that sub-micropoint error before exposing the decoded bounds.
        if ![x0, x1, y0, y1, width].iter().all(|v| v.is_finite() && v.abs() <= 1e9) || width <= 1e-9 || width > 14_400.0 + 1e-6 {
            return None;
        }
        let direction = ((x1 - x0) / width, (y1 - y0) / width);
        let (left, _) = to_frame(direction, (x0, y0));
        let right = left + width.min(14_400.0);
        let properties = op.operands.get(1)?.as_dict()?;
        let (local_vertical, vertical) = if let Some(segment) = properties.get(b"Vertical") {
            let a = segment.as_array()?;
            if a.len() != 4 {
                return None;
            }
            let local = [a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?, a.get(3)?.as_f64()?];
            if !local.iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
                return None;
            }
            let p0 = ctm.apply(local[0], local[1]);
            let p1 = ctm.apply(local[2], local[3]);
            let (x0, bottom) = to_frame(direction, p0);
            let (x1, top) = to_frame(direction, p1);
            if ![p0.0, p0.1, p1.0, p1.1, bottom, top].iter().all(|v| v.is_finite() && v.abs() <= 1e9)
                || (x1 - x0).abs() > 0.01
                || top - bottom > 14_400.0 + 1e-6
                || top <= bottom
            {
                return None;
            }
            (Some(local), Some([bottom, top.min(bottom + 14_400.0)]))
        } else {
            (None, None)
        };
        let align = match properties.get(b"Align").and_then(Object::as_name) {
            Some(b"Center") => crate::added::Align::Center,
            Some(b"Right") => crate::added::Align::Right,
            Some(b"Justify") => crate::added::Align::Justify,
            _ => crate::added::Align::Left,
        };
        Some(Self { op: index, local, ctm, direction, bounds: [left, right], local_vertical, vertical, align })
    }

    /// Encode a nonpainting frame, validating it before its caller mutates the document.
    pub fn start(
        left: f64,
        baseline: f64,
        width: f64,
        vertical: Option<[f64; 2]>,
        align: crate::added::Align,
        ctm: Matrix,
        direction: (f64, f64),
    ) -> Result<Op, EditError> {
        if ![left, baseline, width, direction.0, direction.1].iter().all(|v| v.is_finite())
            || width <= 1e-9
            || width > 14_400.0
            || (direction.0.hypot(direction.1) - 1.0).abs() > 1e-6
        {
            return Err(EditError::Invalid("the paragraph's frame has invalid geometry".into()));
        }
        let inverse = ctm.invert().ok_or_else(|| EditError::Invalid("the paragraph's frame has no area".into()))?;
        let point = |x, y| {
            let (x, y) = from_frame(direction, (x, y));
            inverse.apply(x, y)
        };
        let (x0, y0) = point(left, baseline);
        let (x1, y1) = point(left + width, baseline);
        let mut properties = Dict::new();
        let align = match align {
            crate::added::Align::Left => "Left",
            crate::added::Align::Center => "Center",
            crate::added::Align::Right => "Right",
            crate::added::Align::Justify => "Justify",
        };
        properties.set(b"Align".to_vec(), Object::name(align));
        properties.set(b"Baseline".to_vec(), Object::Array([x0, y0, x1, y1].map(Object::Real).to_vec()));
        if let Some([bottom, top]) = vertical {
            let (x0, y0) = point(left, bottom);
            let (x1, y1) = point(left, top);
            properties.set(b"Vertical".to_vec(), Object::Array([x0, y0, x1, y1].map(Object::Real).to_vec()));
        }
        let op = Op::new("BDC", vec![Object::name("PCTextFrame"), Object::Dict(properties)]);
        if Self::read(&op, 0, ctm).is_none() {
            return Err(EditError::Invalid("the paragraph's frame has invalid geometry".into()));
        }
        Ok(op)
    }

    /// Whole-paragraph moves use TJ/Ts, not a CTM change. Move the metadata in its own space too.
    pub fn moved(self, original: &Op, offset: [f64; 2]) -> Result<Op, EditError> {
        let inverse = self.ctm.invert().ok_or_else(|| EditError::Invalid("the paragraph's frame has no area".into()))?;
        let [a, b, c, d, _, _] = inverse.0;
        let (dx, dy) = (a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]);
        let [x0, y0, x1, y1] = self.local;
        let mut op = original.clone();
        let Some(Object::Dict(properties)) = op.operands.get_mut(1) else {
            return Err(EditError::Invalid("the paragraph's frame is missing".into()));
        };
        properties.set(b"Baseline".to_vec(), Object::Array([x0 + dx, y0 + dy, x1 + dx, y1 + dy].map(Object::Real).to_vec()));
        if let Some([x0, y0, x1, y1]) = self.local_vertical {
            properties.set(b"Vertical".to_vec(), Object::Array([x0 + dx, y0 + dy, x1 + dx, y1 + dy].map(Object::Real).to_vec()));
        }
        if Self::read(&op, self.op, self.ctm).is_none() {
            return Err(EditError::Invalid("the paragraph's frame cannot be moved that far".into()));
        }
        Ok(op)
    }

    /// Replace our old wrapper, keeping all unrelated/unknown marked content intact.
    pub fn remove(self, ops: &[Op], drop: &mut std::collections::HashSet<usize>) {
        let mut depth = 0_usize;
        for (index, op) in ops.iter().enumerate().skip(self.op) {
            if op.is("BMC") || op.is("BDC") {
                depth = depth.saturating_add(1);
            } else if op.is("EMC") {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    drop.insert(self.op);
                    drop.insert(index);
                    break;
                }
            }
        }
    }
}

/// The kinds of nested content objects a frame must not cross.
fn boundary(op: &Op) -> Option<(&'static str, bool)> {
    match op.op.as_slice() {
        b"BMC" | b"BDC" => Some(("marked", true)),
        b"EMC" => Some(("marked", false)),
        b"BT" => Some(("text", true)),
        b"ET" => Some(("text", false)),
        b"q" => Some(("graphics", true)),
        b"Q" => Some(("graphics", false)),
        b"BX" => Some(("compatibility", true)),
        b"EX" => Some(("compatibility", false)),
        _ => None,
    }
}

/// Complete objects crossing the selected interval; common enclosing objects stay outside.
fn complete_interval(ops: &[Op], mut start: usize, mut end: usize) -> Result<(usize, usize), EditError> {
    let (mut left, mut right, mut stack) = (Vec::new(), Vec::new(), Vec::new());
    let mut closes = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if i == start {
            left = stack.clone();
        }
        if i == end {
            right = stack.clone();
        }
        if let Some((kind, opening)) = boundary(op) {
            if opening {
                stack.push((i, kind));
            } else if let Some((open, expected)) = stack.pop() {
                if expected != kind {
                    return Err(EditError::Invalid("the paragraph's page has crossing content object boundaries".into()));
                }
                closes.insert(open, i);
            } else {
                return Err(EditError::Invalid("the paragraph's page has unmatched content object boundaries".into()));
            }
        }
    }
    let common = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    if let Some((open, _)) = left.get(common) {
        start = *open;
    }
    if let Some((open, _)) = right.get(common) {
        end = *closes.get(open).ok_or_else(|| EditError::Invalid("the paragraph crosses unfinished content objects".into()))?;
    }
    Ok((start, end))
}

fn shows_text(op: &Op) -> bool {
    match op.op.as_slice() {
        b"TJ" => !op.operands.last().and_then(Object::as_array).is_some_and(|parts| {
            parts.iter().all(|p| matches!(p, Object::Int(_) | Object::Real(_)) || p.as_string().is_some_and(|s| s.bytes.is_empty()))
        }),
        b"Tj" | b"'" | b"\"" => !op.operands.last().and_then(Object::as_string).is_some_and(|s| s.bytes.is_empty()),
        _ => false,
    }
}

fn validate_interval(ops: &[Op], selected: &std::collections::HashSet<usize>, start: usize, end: usize) -> Result<(), EditError> {
    let mut depth = 0_usize;
    for (i, op) in ops.iter().enumerate().take(end.saturating_add(1)).skip(start) {
        if shows_text(op) && !selected.contains(&i) {
            return Err(EditError::Invalid("the paragraph's frame would include unrelated text".into()));
        }
        if let Some((_, opening)) = boundary(op) {
            if opening {
                depth = depth.saturating_add(1);
            } else {
                depth = depth.checked_sub(1).ok_or_else(|| EditError::Invalid("the paragraph crosses unmatched content objects".into()))?;
            }
        }
    }
    if depth != 0 {
        return Err(EditError::Invalid("the paragraph crosses unfinished content objects".into()));
    }
    Ok(())
}

/// The wrapper can begin before an inner cm, so use the CTM at the insertion point.
fn insertion_ctm(ops: &[Op], start: usize) -> Matrix {
    let (mut ctm, mut graphics) = (Matrix::IDENTITY, Vec::new());
    for op in ops.iter().take(start) {
        match op.op.as_slice() {
            b"q" => graphics.push(ctm),
            b"Q" => {
                if let Some(saved) = graphics.pop() {
                    ctm = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ctm = Matrix(m).then(&ctm);
                }
            }
            _ => {}
        }
    }
    ctm
}

fn resized_marker(frame: HorizontalFrame, ops: &[Op], bounds: [f64; 2]) -> Result<Op, EditError> {
    let mut op = ops.get(frame.op).cloned().ok_or_else(|| EditError::Invalid("the paragraph's frame is missing".into()))?;
    let inverse = frame.ctm.invert().ok_or_else(|| EditError::Invalid("the paragraph's frame has no area".into()))?;
    let bottom = from_frame(frame.direction, (frame.bounds[0], bounds[0]));
    let top = from_frame(frame.direction, (frame.bounds[0], bounds[1]));
    let (x0, y0) = inverse.apply(bottom.0, bottom.1);
    let (x1, y1) = inverse.apply(top.0, top.1);
    let Some(Object::Dict(properties)) = op.operands.get_mut(1) else {
        return Err(EditError::Invalid("the paragraph's frame is missing".into()));
    };
    properties.set(b"Vertical".to_vec(), Object::Array([x0, y0, x1, y1].map(Object::Real).to_vec()));
    if HorizontalFrame::read(&op, frame.op, frame.ctm).is_none() {
        return Err(EditError::Invalid("the paragraph's frame has invalid geometry".into()));
    }
    Ok(op)
}

fn wrap_vertical(
    pieces: &Pieces,
    ops: &[Op],
    block: &TextBlock,
    lines: &[TextLine],
    bounds: [f64; 2],
    crop: [f64; 4],
) -> Result<Vec<Vec<u8>>, EditError> {
    let first = block.lines.first().and_then(|i| lines.get(*i)).ok_or_else(|| EditError::Invalid("the paragraph has no source runs".into()))?;
    let selected: std::collections::HashSet<_> = block.lines.iter().filter_map(|i| lines.get(*i)).flat_map(|l| l.ops.iter().copied()).collect();
    let start = selected.iter().copied().min().ok_or_else(|| EditError::Invalid("the paragraph has no drawing".into()))?;
    let end = selected.iter().copied().max().ok_or_else(|| EditError::Invalid("the paragraph has no drawing".into()))?;
    let (start, end) = complete_interval(ops, start, end)?;
    validate_interval(ops, &selected, start, end)?;
    let ctm = insertion_ctm(ops, start);
    let rect = block.editing_frame();
    // Once the frame is explicit, later rewrites no longer infer alignment from the page.
    // Retain that inference now without changing any of the original text operators.
    let align = block.horizontal_alignment.unwrap_or_else(|| {
        if crate::bidi::rtl_base(&block.editor_text) {
            crate::added::Align::Right
        } else if first.upright() && is_centred(block, lines, (crop[0] + crop[2]) / 2.0, first.size) {
            crate::added::Align::Center
        } else {
            crate::added::Align::Left
        }
    });
    let marker = HorizontalFrame::start(rect[0], first.frame_origin().1, rect[2] - rect[0], Some(bounds), align, ctm, first.direction())?;
    Ok(splice(pieces, ops, |i| {
        let mut insert = Vec::new();
        if i == start {
            insert.push(marker.clone());
        }
        if i == end {
            if let Some(op) = ops.get(i) {
                insert.push(op.clone());
            }
            insert.push(Op::new("EMC", vec![]));
        }
        (insert, i != end)
    }))
}

/// Change only the frame metadata: source glyph operators, kerning and row breaks stay intact.
pub(super) fn resize_vertical(
    doc: &mut Document,
    page: usize,
    block: &TextBlock,
    lines: &[TextLine],
    bounds: [f64; 2],
) -> Result<LineEdit, EditError> {
    let first = block.lines.first().and_then(|i| lines.get(*i)).ok_or_else(|| EditError::Invalid("the paragraph has no source runs".into()))?;
    let p = page_dict(doc, page)?;
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let data = if let Some(frame) = first.horizontal_frame {
        let op = resized_marker(frame, &ops, bounds)?;
        splice(&pieces, &ops, |i| if i == frame.op { (vec![op.clone()], false) } else { (Vec::new(), true) })
    } else {
        wrap_vertical(&pieces, &ops, block, lines, bounds, p.crop(doc))?
    };
    let contents = rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(LineEdit { substituted: None })
}
