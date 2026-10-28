//! A page's ruled lines: the horizontal and vertical strokes (and thin filled bars) its content
//! draws, so exports can tell a ruled table from text that merely lines up (#5149).

use pdfcraft_content::{Matrix, Pieces};
use pdfcraft_cos::Document;

use crate::EditError;

/// How deep `q` may nest (deeper saves are not tracked).
const MAX_SAVES: usize = 64;

/// At most this many rules are returned, and a path keeps at most this many segments, so a
/// hostile page stays bounded.
pub const MAX_RULES: usize = 4096;

/// A segment whose ends differ by at most this much across its run (points) is horizontal or
/// vertical; a filled rectangle at most this thick is a bar drawn as a rule.
const THIN: f64 = 2.0;

/// The page's ruled lines, each as its box `[x0, y0, x1, y1]` in default user space (`x0 <= x1`,
/// `y0 <= y1`; a stroke has zero thickness): horizontal and vertical straight segments of stroked
/// paths (rectangle edges included) and filled rectangles at most two points thick. Curves,
/// slanted lines and paths that are only clipped are left out, and so are the contents of forms.
/// At most [`MAX_RULES`] rules, in content order.
pub fn page_rules(doc: &Document, page: usize) -> Result<Vec<[f64; 4]>, EditError> {
    let pages = pdfcraft_model::pages(doc);
    let p = pages.get(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = crate::text::content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;

    let mut out: Vec<[f64; 4]> = Vec::new();
    let mut ctm = Matrix::IDENTITY;
    let mut saved: Vec<Matrix> = Vec::new();
    let mut lost_saves = 0usize;
    // The current path in page space: its straight segments, its rectangles, the current point
    // and where the current subpath started.
    let mut segments: Vec<[(f64, f64); 2]> = Vec::new();
    let mut rects: Vec<[(f64, f64); 4]> = Vec::new();
    let mut at: Option<(f64, f64)> = None;
    let mut start: Option<(f64, f64)> = None;
    for op in &ops {
        if out.len() >= MAX_RULES {
            break;
        }
        match op.op.as_slice() {
            b"q" => {
                if saved.len() < MAX_SAVES {
                    saved.push(ctm);
                } else {
                    lost_saves = lost_saves.saturating_add(1);
                }
            }
            b"Q" => {
                if lost_saves > 0 {
                    lost_saves -= 1;
                } else if let Some(m) = saved.pop() {
                    ctm = m;
                }
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    ctm = m.then(&ctm);
                }
            }
            b"m" => {
                let p = op.nums::<2>().map(|[x, y]| ctm.apply(x, y));
                (at, start) = (p, p);
            }
            b"l" => {
                let p = op.nums::<2>().map(|[x, y]| ctm.apply(x, y));
                if let (Some(a), Some(b)) = (at, p)
                    && segments.len() < MAX_RULES
                {
                    segments.push([a, b]);
                }
                at = p;
            }
            b"c" => at = op.nums::<6>().map(|[.., x, y]| ctm.apply(x, y)),
            b"v" | b"y" => at = op.nums::<4>().map(|[.., x, y]| ctm.apply(x, y)),
            b"h" => {
                if let (Some(a), Some(b)) = (at, start)
                    && segments.len() < MAX_RULES
                {
                    segments.push([a, b]);
                }
                at = start;
            }
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    let q = [ctm.apply(x, y), ctm.apply(x + w, y), ctm.apply(x + w, y + h), ctm.apply(x, y + h)];
                    if rects.len() < MAX_RULES {
                        rects.push(q);
                    }
                    (at, start) = (Some(q[0]), Some(q[0]));
                }
            }
            b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                let closes = matches!(op.op.as_slice(), b"s" | b"b" | b"b*");
                if closes && let (Some(a), Some(b)) = (at, start) {
                    segments.push([a, b]);
                }
                let edges = rects.iter().flat_map(|q| [[q[0], q[1]], [q[1], q[2]], [q[2], q[3]], [q[3], q[0]]]);
                out.extend(segments.iter().copied().chain(edges).filter_map(axis_aligned));
                (segments, rects, at, start) = (Vec::new(), Vec::new(), None, None);
            }
            b"f" | b"F" | b"f*" => {
                out.extend(rects.iter().filter_map(thin_bar));
                (segments, rects, at, start) = (Vec::new(), Vec::new(), None, None);
            }
            b"n" => (segments, rects, at, start) = (Vec::new(), Vec::new(), None, None),
            _ => {}
        }
    }
    out.truncate(MAX_RULES);
    Ok(out)
}

/// The box of a horizontal or vertical segment, or `None` for a slanted, tiny or non-finite one.
fn axis_aligned([a, b]: [(f64, f64); 2]) -> Option<[f64; 4]> {
    let r = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    (r.iter().all(|v| v.is_finite()) && w.min(h) <= THIN && w.max(h) > THIN).then_some(r)
}

/// The box of a filled rectangle drawn as a rule: upright, at most [`THIN`] thick and longer than that.
fn thin_bar(q: &[(f64, f64); 4]) -> Option<[f64; 4]> {
    let upright = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() <= 0.01 || (a.1 - b.1).abs() <= 0.01;
    if !(upright(q[0], q[1]) && upright(q[1], q[2])) {
        return None;
    }
    let xs = q.map(|p| p.0);
    let ys = q.map(|p| p.1);
    let r = [
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ];
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    (r.iter().all(|v| v.is_finite()) && w.min(h) <= THIN && w.max(h) > THIN).then_some(r)
}
