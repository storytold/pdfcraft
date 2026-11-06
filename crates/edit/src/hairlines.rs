//! Print production ▸ Fix hairlines: lines too thin to print reliably are thickened.
//!
//! A line width (`w`, ISO 32000-2 §8.4.3.2) is in user space, so how thick a stroke prints
//! depends on the current transformation matrix too: the width is measured as it lands on the
//! page (scaled by the CTM's mean scale, √|det|). Widths of 0 (the thinnest line the device can
//! draw) always count as hairlines. A width under the threshold is replaced by one that lands at
//! the replacement width. Graphics state dictionaries (`gs` with `/LW`), form XObjects, patterns
//! and Type 3 glyphs are left as they are.

use pdfcraft_content::{Matrix, Op, Pieces, num};
use pdfcraft_cos::Document;

use crate::EditError;

/// Deepest `q` nesting followed; deeper saves share the last state.
const MAX_DEPTH: usize = 256;

/// How the CTM `m` scales a line width (the mean scale, √|det|).
fn scale_of(m: &Matrix) -> f64 {
    let [a, b, c, d, _, _] = m.0;
    let s = (a * d - b * c).abs().sqrt();
    if s.is_finite() && s > 0.0 { s } else { 1.0 }
}

/// The `w` operators of `ops` that draw hairlines, with the width each should get.
fn plan(ops: &[Op], threshold: f64, replace: f64) -> Vec<(usize, f64)> {
    let mut ctm = Matrix::IDENTITY;
    let mut stack: Vec<Matrix> = Vec::new();
    let mut out = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if stack.len() < MAX_DEPTH {
                    stack.push(ctm);
                }
            }
            b"Q" => {
                if let Some(m) = stack.pop() {
                    ctm = m;
                }
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    ctm = m.then(&ctm);
                }
            }
            b"w" => {
                let Some(w) = op.num(0) else { continue };
                if !w.is_finite() {
                    continue;
                }
                let s = scale_of(&ctm);
                let on_page = w.abs() * s;
                if w == 0.0 || on_page < threshold {
                    out.push((i, replace / s));
                }
            }
            _ => {}
        }
    }
    out
}

/// Thicken hairlines on `pages` (0-based): every line width that lands under `threshold` points
/// (or is 0) becomes `replace` points. Returns how many line widths changed.
pub fn fix_hairlines(doc: &mut Document, pages: &[usize], threshold: f64, replace: f64) -> Result<usize, EditError> {
    if !(threshold.is_finite() && threshold > 0.0 && threshold <= 72.0) {
        return Err(EditError::Invalid("the hairline threshold must be more than 0 and at most 72 points".into()));
    }
    if !(replace.is_finite() && replace >= threshold && replace <= 72.0) {
        return Err(EditError::Invalid("the new line width must be at least the threshold and at most 72 points".into()));
    }
    let count = pdfcraft_model::pages(doc).len();
    if let Some(p) = pages.iter().find(|p| **p >= count) {
        return Err(EditError::NoSuchPage(*p));
    }
    let mut changed = 0_usize;
    for &p in pages {
        let page = crate::text::page_dict(doc, p)?;
        let streams = crate::text::content_streams(doc, &page.dict)?;
        if streams.is_empty() {
            continue;
        }
        let pieces = Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>());
        let ops = pieces.parse().ops;
        let fixes = plan(&ops, threshold, replace);
        if fixes.is_empty() {
            continue;
        }
        changed = changed.saturating_add(fixes.len());
        let mut next = fixes.into_iter().peekable();
        let new = crate::text::splice(&pieces, &ops, |i| match next.peek() {
            Some((at, w)) if *at == i => {
                let w = *w;
                next.next();
                (vec![Op::new("w", vec![num(w)])], false)
            }
            _ => (Vec::new(), true),
        });
        let contents = crate::text::rewritten_contents(doc, &streams, new);
        doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_measured_on_the_page() {
        let ops = pdfcraft_content::parse(b"0 w 0.1 w 1 w q 0.1 0 0 0.1 0 0 cm 2 w 0.5 w Q 0.2 w").ops;
        let p = plan(&ops, 0.25, 0.3);
        let at: Vec<usize> = p.iter().map(|(i, _)| *i).collect();
        // 0 w, 0.1 w, then 2 w and 0.5 w under a 0.1 scale (0.2 and 0.05 on the page), and 0.2 w
        // after the Q; 1 w stays.
        assert_eq!(at, [0, 1, 5, 6, 8], "{ops:?}");
        assert!((p[2].1 - 3.0).abs() < 1e-9, "0.3 pt on the page is 3 units under the 0.1 scale");
        assert!((p[0].1 - 0.3).abs() < 1e-9);
    }
}
