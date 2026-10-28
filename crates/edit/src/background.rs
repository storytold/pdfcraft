//! A page's background colour: the colour the page is painted with before anything else, as
//! Edit a PDF ▸ Background and many producers do it, for exports that keep a page colour (#2992).

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Dict, Document};

use crate::{EditError, Rgb};

/// How deep `q` may nest while looking for the background (deeper saves are not tracked).
const MAX_SAVES: usize = 64;

/// How many clipping rectangles are kept; past that the clip counts as unknown.
const MAX_CLIPS: usize = 64;

/// A rectangle as it lands on the page: its four corners in page space, in order. Under a
/// rotating or shearing CTM it is a parallelogram, so its bounding box says too much (#2994).
type Quad = [(f64, f64); 4];

/// The colour space a fill colour is given in.
#[derive(Clone, Copy, PartialEq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Anything else (Separation, ICC, patterns…): no plain colour can be read from it.
    Other,
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    space: Space,
    fill: Option<Rgb>,
    /// Fill opacity (`ca`), and whether a soft mask or a blend mode makes the fill other than a
    /// plain paint.
    alpha: f64,
    plain: bool,
    /// The clip, as the rectangles it is the intersection of, in page space; `None` when a clip
    /// that isn't one rectangle was set (its area can't be told).
    clip: Option<Vec<Quad>>,
}

/// The page's background colour: the colour of the first thing its content paints, when that is
/// an opaque fill, in a gray, RGB or CMYK colour, of a path with a rectangle that covers the
/// whole visible page (and no clip keeps it from doing so). `None` for a page that paints anything else
/// first, or nothing. Forms and later content are not looked at.
pub fn page_background(doc: &Document, page: usize) -> Result<Option<Rgb>, EditError> {
    let pages = pdfcraft_model::pages(doc);
    let p = pages.get(page).ok_or(EditError::NoSuchPage(page))?;
    let crop = p.crop(doc);
    let resources = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let gstates = resources.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).unwrap_or_default();
    let streams = crate::text::content_streams(doc, &p.dict);
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;

    let mut st = State { ctm: Matrix::IDENTITY, space: Space::Gray, fill: Some([0.0; 3]), alpha: 1.0, plain: true, clip: Some(Vec::new()) };
    let mut saved: Vec<State> = Vec::new();
    let mut lost_saves = 0usize;
    // The path being built: its rectangles in page space, and whether it has anything else.
    // Each rectangle of the current path: its box on the page, and whether it winds
    // counterclockwise there (its width and height and the CTM's determinant agree in sign).
    let mut rects: Vec<(Quad, bool)> = Vec::new();
    let mut curved = false;
    let mut clip_pending = false;
    for op in &ops {
        match op.op.as_slice() {
            b"q" => {
                if saved.len() < MAX_SAVES {
                    saved.push(st.clone());
                } else {
                    lost_saves = lost_saves.saturating_add(1);
                }
            }
            b"Q" => {
                if lost_saves > 0 {
                    lost_saves -= 1;
                } else if let Some(s) = saved.pop() {
                    st = s;
                }
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    st.ctm = m.then(&st.ctm);
                }
            }
            b"g" => (st.space, st.fill) = (Space::Gray, op.nums::<1>().map(|[g]| [g; 3])),
            b"rg" => (st.space, st.fill) = (Space::Rgb, op.nums::<3>()),
            b"k" => (st.space, st.fill) = (Space::Cmyk, op.nums::<4>().map(cmyk)),
            b"cs" => {
                st.space = match op.name(0) {
                    Some(b"DeviceGray" | b"G") => Space::Gray,
                    Some(b"DeviceRGB" | b"RGB") => Space::Rgb,
                    Some(b"DeviceCMYK" | b"CMYK") => Space::Cmyk,
                    _ => Space::Other,
                };
                st.fill = None;
            }
            b"sc" | b"scn" => st.fill = colour(st.space, op),
            b"gs" => {
                if let Some(g) = op.name(0).and_then(|n| gstates.get(n)).map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()) {
                    apply_gstate(doc, &g, &mut st);
                }
            }
            b"re" => match op.nums::<4>() {
                Some([x, y, w, h]) => {
                    let [a, b, c, d, _, _] = st.ctm.0;
                    rects.push((quad(&st.ctm, [x, y, x + w, y + h]), (w * h) * (a * d - b * c) > 0.0));
                }
                None => curved = true,
            },
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" => curved = true,
            b"W" | b"W*" => clip_pending = true,
            b"n" => {
                if clip_pending {
                    st.clip = match (st.clip.take(), rects.as_slice(), curved) {
                        (Some(mut c), [(r, _)], false) if c.len() < MAX_CLIPS => {
                            c.push(*r);
                            Some(c)
                        }
                        _ => None,
                    };
                }
                (rects, curved, clip_pending) = (Vec::new(), false, false);
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" => {
                let opaque = st.plain && st.alpha >= 0.999;
                // A second rectangle can cut a hole in the one that covers the page: under the
                // even-odd rule any second one, under nonzero one wound the other way (a
                // frame-style page border).
                let even_odd = matches!(op.op.as_slice(), b"f*" | b"B*");
                let solid = match rects.as_slice() {
                    [_] => true,
                    [(_, first), rest @ ..] => !even_odd && rest.iter().all(|(_, ccw)| ccw == first),
                    [] => false,
                };
                let covers = !curved
                    && solid
                    && st.clip.as_ref().is_some_and(|c| c.iter().all(|q| covers(q, &crop)))
                    && rects.iter().any(|(r, _)| covers(r, &crop));
                return Ok(st.fill.filter(|_| opaque && covers && !clip_pending).map(|c| c.map(|v| v.clamp(0.0, 1.0))));
            }
            // Anything else painted first: no page background.
            b"S" | b"s" | b"b" | b"b*" | b"Tj" | b"TJ" | b"'" | b"\"" | b"Do" | b"sh" | b"BI" => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}

/// CMYK to RGB, as a plain conversion without colour management.
fn cmyk([c, m, y, k]: [f64; 4]) -> Rgb {
    [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
}

/// The colour `sc`/`scn` sets in `space`, if it is a plain one.
fn colour(space: Space, op: &Op) -> Option<Rgb> {
    match (space, op.operands.len()) {
        (Space::Gray, 1) => op.nums::<1>().map(|[g]| [g; 3]),
        (Space::Rgb, 3) => op.nums::<3>(),
        (Space::Cmyk, 4) => op.nums::<4>().map(cmyk),
        _ => None,
    }
}

/// The fill opacity, soft mask and blend mode an ExtGState sets.
fn apply_gstate(doc: &Document, g: &Dict, st: &mut State) {
    if let Some(ca) = g.get(b"ca").and_then(|v| doc.resolve(v).as_f64()) {
        st.alpha = if ca.is_finite() { ca } else { 0.0 };
    }
    if let Some(mask) = g.get(b"SMask") {
        st.plain &= doc.resolve(mask).as_name() == Some(b"None");
    }
    if let Some(bm) = g.get(b"BM") {
        let normal = |n: Option<&[u8]>| matches!(n, Some(b"Normal" | b"Compatible"));
        st.plain &= match &*doc.resolve(bm) {
            pdfcraft_cos::Object::Array(a) => a.first().is_some_and(|n| normal(doc.resolve(n).as_name())),
            other => normal(other.as_name()),
        };
    }
}

/// The page-space corners of a user-space rectangle under `ctm`, in order around it.
fn quad(ctm: &Matrix, r: [f64; 4]) -> Quad {
    [ctm.apply(r[0], r[1]), ctm.apply(r[2], r[1]), ctm.apply(r[2], r[3]), ctm.apply(r[0], r[3])]
}

/// Whether `outer` covers `page`, give or take half a percent of its size: each corner of the
/// page, pulled in by that much, lies inside the parallelogram `outer`.
fn covers(outer: &Quad, page: &[f64; 4]) -> bool {
    let (w, h) = ((page[2] - page[0]).abs(), (page[3] - page[1]).abs());
    let (tx, ty) = (w * 0.005, h * 0.005);
    let (x0, y0, x1, y1) = (page[0].min(page[2]) + tx, page[1].min(page[3]) + ty, page[0].max(page[2]) - tx, page[1].max(page[3]) - ty);
    let cross = |a: (f64, f64), b: (f64, f64), p: (f64, f64)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
    // Twice the signed area: its sign says which way the corners run.
    let area: f64 = (0..4).map(|i| cross((0.0, 0.0), outer[i], outer[(i + 1) % 4])).sum();
    if !area.is_finite() || area.abs() < f64::EPSILON || outer.iter().any(|p| !(p.0.is_finite() && p.1.is_finite())) {
        return false;
    }
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].iter().all(|&p| (0..4).all(|i| cross(outer[i], outer[(i + 1) % 4], p) * area.signum() >= 0.0))
}
