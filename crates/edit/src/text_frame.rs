//! An explicitly resized paragraph's horizontal frame, independent of its glyph bounds.
//! Private marked content keeps it with the text across saves without painting anything.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HorizontalFrame {
    pub op: usize,
    local: [f64; 4],
    ctm: Matrix,
    pub bounds: [f64; 2],
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
        let (left, y0) = ctm.apply(local[0], local[1]);
        let (right, y1) = ctm.apply(local[2], local[3]);
        // Only upright text has horizontal handles. A later rotation may make this metadata
        // inapplicable; ignore it rather than assigning a misleading horizontal container.
        if ![left, right, y0, y1].iter().all(|v| v.is_finite() && v.abs() <= 1e9)
            || (y1 - y0).abs() > 0.01
            || !(0.0..=14_400.0).contains(&(right - left))
            || right <= left
        {
            return None;
        }
        let align = match op.operands.get(1)?.as_dict()?.get(b"Align").and_then(Object::as_name) {
            Some(b"Center") => crate::added::Align::Center,
            Some(b"Right") => crate::added::Align::Right,
            Some(b"Justify") => crate::added::Align::Justify,
            _ => crate::added::Align::Left,
        };
        Some(Self { op: index, local, ctm, bounds: [left, right], align })
    }

    pub fn start(left: f64, baseline: f64, width: f64, align: crate::added::Align, ctm: Matrix) -> Result<Op, EditError> {
        let inverse = ctm.invert().ok_or_else(|| EditError::Invalid("the paragraph's frame has no area".into()))?;
        let (x0, y0) = inverse.apply(left, baseline);
        let (x1, y1) = inverse.apply(left + width, baseline);
        let mut properties = Dict::new();
        let align = match align {
            crate::added::Align::Left => "Left",
            crate::added::Align::Center => "Center",
            crate::added::Align::Right => "Right",
            crate::added::Align::Justify => "Justify",
        };
        properties.set(b"Align".to_vec(), Object::name(align));
        properties.set(b"Baseline".to_vec(), Object::Array([x0, y0, x1, y1].map(Object::Real).to_vec()));
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
