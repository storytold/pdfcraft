//! A page's ruled lines: the horizontal and vertical strokes (and thin filled bars) its content
//! draws, so exports can tell a ruled table from text that merely lines up (#5149); and the box of
//! every path it paints, so an export can find the drawings a figure is made of (#7196).

use pdfcraft_content::Matrix;
use pdfcraft_cos::Document;

use crate::EditError;

/// How deep `q` may nest (deeper saves are not tracked).
const MAX_SAVES: usize = 64;

/// At most this many rules are returned, and a path keeps at most this many segments, so a
/// hostile page stays bounded.
pub const MAX_RULES: usize = 4096;

/// At most this many drawings are returned, so a hostile page stays bounded, with room for the
/// paths of a dense chart.
pub const MAX_DRAWINGS: usize = 16_384;

/// A segment whose ends differ by at most this much across its run (points) is horizontal or
/// vertical; a filled rectangle at most this thick is a bar drawn as a rule.
const THIN: f64 = 2.0;

/// A path a page paints ([`page_drawings`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drawing {
    /// The box of the path's points `[x0, y0, x1, y1]` in default user space (`x0 <= x1`,
    /// `y0 <= y1`), curve control points included, so it holds the curve. The stroke's width is
    /// not added: a straight line has no thickness.
    pub rect: [f64; 4],
    /// Whether the path is stroked (`S`, `s`, `B`, `B*`, `b`, `b*`).
    pub stroked: bool,
    /// Whether the path is filled (`f`, `F`, `f*`, `B`, `B*`, `b`, `b*`).
    pub filled: bool,
}

/// The page's ruled lines, each as its box `[x0, y0, x1, y1]` in default user space (`x0 <= x1`,
/// `y0 <= y1`; a stroke has zero thickness): horizontal and vertical straight segments of stroked
/// paths (rectangle edges included) and filled rectangles at most two points thick. Curves,
/// slanted lines and paths that are only clipped are left out, and so are the contents of forms.
/// At most [`MAX_RULES`] rules, in content order.
pub fn page_rules(doc: &Document, page: usize) -> Result<Vec<[f64; 4]>, EditError> {
    Ok(paths(doc, page, "its ruled lines")?.0)
}

/// Every path the page paints, stroked or filled, in content order: straight lines, curves and
/// rectangles alike. Paths that are only clipped (`n`) are left out, and so are the contents of
/// forms, as [`page_rules`] leaves them out. At most [`MAX_DRAWINGS`].
pub fn page_drawings(doc: &Document, page: usize) -> Result<Vec<Drawing>, EditError> {
    Ok(paths(doc, page, "its drawings")?.1)
}

/// One walk of the page's content for [`page_rules`] and [`page_drawings`]. `what` names what
/// an incompletely decoded page can't give.
fn paths(doc: &Document, page: usize, what: &str) -> Result<(Vec<[f64; 4]>, Vec<Drawing>), EditError> {
    let pages = pdfcraft_model::pages(doc);
    let p = pages.get(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = crate::text::content_streams(doc, &p.dict)?;
    // Unread content can change the graphics state used by every later stream.
    if streams.iter().any(|stream| stream.read != crate::text::Read::Whole) {
        return Err(EditError::Invalid(format!("page content could not be completely decoded; cannot extract {what} safely")));
    }
    let pieces = crate::text::joined(&streams);
    let ops = pieces.parse().ops;

    let mut out: Vec<[f64; 4]> = Vec::new();
    let mut drawings: Vec<Drawing> = Vec::new();
    let mut ctm = Matrix::IDENTITY;
    let mut saved: Vec<Matrix> = Vec::new();
    let mut lost_saves = 0usize;
    // The current path in page space: its straight segments, its rectangles, the current point
    // and where the current subpath started.
    let mut segments: Vec<[(f64, f64); 2]> = Vec::new();
    let mut rects: Vec<[(f64, f64); 4]> = Vec::new();
    let mut at: Option<(f64, f64)> = None;
    let mut start: Option<(f64, f64)> = None;
    // The box of every point of the current path, control points included.
    let mut bounds: Option<[f64; 4]> = None;
    for op in &ops {
        // Rules stop at their limit; the walk goes on while drawings have room.
        if out.len() >= MAX_RULES && drawings.len() >= MAX_DRAWINGS {
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
                grow(&mut bounds, p);
                (at, start) = (p, p);
            }
            b"l" => {
                let p = op.nums::<2>().map(|[x, y]| ctm.apply(x, y));
                grow(&mut bounds, p);
                if let (Some(a), Some(b)) = (at, p)
                    && segments.len() < MAX_RULES
                {
                    segments.push([a, b]);
                }
                at = p;
            }
            b"c" => {
                let points = op.nums::<6>().map(|[x1, y1, x2, y2, x, y]| [ctm.apply(x1, y1), ctm.apply(x2, y2), ctm.apply(x, y)]);
                for p in points.into_iter().flatten() {
                    grow(&mut bounds, Some(p));
                }
                at = points.map(|[.., p]| p);
            }
            b"v" | b"y" => {
                let points = op.nums::<4>().map(|[x1, y1, x, y]| [ctm.apply(x1, y1), ctm.apply(x, y)]);
                for p in points.into_iter().flatten() {
                    grow(&mut bounds, Some(p));
                }
                at = points.map(|[.., p]| p);
            }
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
                    for p in q {
                        grow(&mut bounds, Some(p));
                    }
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
                if out.len() < MAX_RULES {
                    let edges = rects.iter().flat_map(|q| [[q[0], q[1]], [q[1], q[2]], [q[2], q[3]], [q[3], q[0]]]);
                    out.extend(segments.iter().copied().chain(edges).filter_map(axis_aligned));
                }
                painted(&mut drawings, bounds.take(), true, matches!(op.op.as_slice(), b"B" | b"B*" | b"b" | b"b*"));
                (segments, rects, at, start) = (Vec::new(), Vec::new(), None, None);
            }
            b"f" | b"F" | b"f*" => {
                if out.len() < MAX_RULES {
                    out.extend(rects.iter().filter_map(thin_bar));
                }
                painted(&mut drawings, bounds.take(), false, true);
                (segments, rects, at, start) = (Vec::new(), Vec::new(), None, None);
            }
            b"n" => (segments, rects, at, start, bounds) = (Vec::new(), Vec::new(), None, None, None),
            _ => {}
        }
    }
    out.truncate(MAX_RULES);
    Ok((out, drawings))
}

/// Widens `bounds` to hold `p` (a point with a non-finite coordinate is left out).
fn grow(bounds: &mut Option<[f64; 4]>, p: Option<(f64, f64)>) {
    let Some((x, y)) = p.filter(|(x, y)| x.is_finite() && y.is_finite()) else { return };
    *bounds = Some(match *bounds {
        Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        None => [x, y, x, y],
    });
}

/// Records a painted path whose points span `bounds`, while there is room for it.
fn painted(drawings: &mut Vec<Drawing>, bounds: Option<[f64; 4]>, stroked: bool, filled: bool) {
    if let Some(rect) = bounds
        && drawings.len() < MAX_DRAWINGS
    {
        drawings.push(Drawing { rect, stroked, filled });
    }
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
