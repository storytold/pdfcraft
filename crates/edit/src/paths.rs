//! Edit a PDF ▸ the vector paths a page paints. One path is the path object from its first
//! construction operator (`m l c v y h re`) through its painting operator. Clipping paths (`W`/
//! `W*`, even when they also paint), `n`, paths inside text objects, under a text clip or in
//! PdfCraft's own marks and added content (`/PCMark` streams, edited with their own tools), and
//! the contents of Form XObjects (one grouped item, see [`crate::images`]) are not listed.

use std::ops::RangeInclusive;

use kurbo::{BezPath, ParamCurveNearest, PathEl, Point, Shape};
use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Dict, Document};

use crate::EditError;
use crate::geometry::{ClipPath, GState, closed, intersection};

/// A page painting more paths than this is a map or a chart, not artwork to pick apart.
const MAX_PAGE_PATHS: usize = 5_000;
/// Clip path elements summed over the listed paths: what hit testing every path can walk.
/// Paths share their clips, so a few complex ones over many paths multiply.
const MAX_CLIP_WORK: usize = 1 << 22;
/// Hairlines (`0 w`) and thinner lines are drawn one device pixel wide: a point at 72 dpi.
const MIN_LINE_WIDTH: f64 = 1.0;
/// Larger user-space coordinates come from runaway `cm`s.
const MAX_COORDINATE: f64 = 1e12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

/// How a path is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paint {
    pub fill: Option<FillRule>,
    pub stroke: bool,
}

/// A vector path painted on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct PagePath {
    /// Visible bounds in user space: the path's, out by half the line width when stroked and
    /// at least a point across, within the clip.
    pub rect: [f64; 4],
    /// The path in user space (`s`, `b` and `b*` close it).
    pub path: BezPath,
    pub paint: Paint,
    /// RGB approximations of the colours it is painted with: `None` for a part it doesn't
    /// paint, or a colour that isn't DeviceGray, DeviceRGB or DeviceCMYK.
    pub fill_rgb: Option<[f64; 3]>,
    pub stroke_rgb: Option<[f64; 3]>,
    pub line_width_user: f64,
    /// Whether it fills or strokes with a pattern (a tiling or a shading), which is fixed to the
    /// page: a `cm` moves the path but not its pattern.
    pub pattern: bool,
    /// Its operators in the page's joined content (see [`Pieces`]), for the path edits.
    pub(crate) ops: RangeInclusive<usize>,
    /// The CTM it is drawn with.
    pub(crate) matrix: Matrix,
    clips: Vec<ClipPath>,
}

impl PagePath {
    /// Whether a user-space point is on the painted path (inside its fill, or within half the
    /// line width of a stroke), allowing `tolerance` either way, and inside every active clip.
    pub fn contains(&self, point: [f64; 2], tolerance: f64) -> bool {
        let [x, y] = point;
        let tolerance = if tolerance.is_finite() { tolerance.clamp(0.0, MAX_COORDINATE) } else { 0.0 };
        let r = self.rect;
        if !x.is_finite() || !y.is_finite() || x < r[0] - tolerance || x > r[2] + tolerance || y < r[1] - tolerance || y > r[3] + tolerance {
            return false;
        }
        let p = Point::new(x, y);
        let near = |path: &BezPath, reach: f64| path.segments().any(|s| s.nearest(p, 1e-6).distance_sq <= reach * reach);
        let stroked = self.paint.stroke && near(&self.path, self.line_width_user.max(MIN_LINE_WIDTH) / 2.0 + tolerance);
        let painted = stroked
            || self.paint.fill.is_some_and(|rule| {
                let outline = closed(&self.path);
                let winding = outline.winding(p);
                (if rule == FillRule::EvenOdd { winding % 2 != 0 } else { winding != 0 }) || near(&outline, tolerance)
            });
        // The clips are shared with other paths and usually cost more to test.
        painted && self.clips.iter().all(|clip| clip.contains(p))
    }
}

fn bounded(path: &BezPath) -> bool {
    let ok = |p: &Point| p.x.abs() <= MAX_COORDINATE && p.y.abs() <= MAX_COORDINATE;
    path.elements().iter().all(|el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => ok(p),
        PathEl::QuadTo(p1, p2) => ok(p1) && ok(p2),
        PathEl::CurveTo(p1, p2, p3) => ok(p1) && ok(p2) && ok(p3),
        PathEl::ClosePath => true,
    })
}

/// A rule along one axis still gets something to select: at least a point, about its centre.
fn widen(lo: f64, hi: f64) -> (f64, f64) {
    if hi - lo >= 1.0 {
        (lo, hi)
    } else {
        let centre = (lo + hi) / 2.0;
        (centre - 0.5, centre + 0.5)
    }
}

/// The line width a `gs` operator's ExtGState sets (`/LW`), if any.
fn gs_line_width(doc: &Document, ext_gstates: &Dict, op: &Op) -> Option<f64> {
    let gs = doc.resolve(ext_gstates.get(op.name(0)?)?);
    doc.resolve(gs.as_dict()?.get(b"LW")?).as_f64().filter(|w| (0.0..=MAX_COORDINATE).contains(w))
}

/// Whether a `cs`/`CS` operator names a pattern space resource (`/Pattern`, or `[/Pattern base]`
/// for uncoloured patterns).
fn named_pattern_space(doc: &Document, colour_spaces: &Dict, op: &Op) -> bool {
    let Some(space) = op.name(0).and_then(|n| colour_spaces.get(n)).map(|s| doc.resolve(s)) else { return false };
    let first = space.as_array().and_then(|a| a.first()).map(|f| doc.resolve(f));
    matches!(first.as_deref().unwrap_or(&space).as_name(), Some(b"Pattern"))
}

/// The vector paths page `page` (0-based) paints, in painting order. Refuses a page whose
/// content can't be read within the group-selection budgets or that paints too many paths.
pub fn page_paths(doc: &Document, page: usize) -> Result<Vec<PagePath>, EditError> {
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let streams = crate::objects::checked_streams(doc, page)?;
    let resources = p.dict.get(b"Resources").map(|r| doc.resolve(r));
    let category = |key: &[u8]| {
        resources
            .as_ref()
            .and_then(|r| r.as_dict().and_then(|r| r.get(key)).map(|c| doc.resolve(c)))
            .and_then(|c| c.as_dict().cloned())
            .unwrap_or_default()
    };
    let (ext_gstates, colour_spaces) = (category(b"ExtGState"), category(b"ColorSpace"));
    let marked: Vec<bool> = streams.iter().map(|(o, _)| crate::stream_tag(doc, o).is_some()).collect();
    let any_marked = marked.contains(&true);
    let in_mark = |piece: usize| marked.get(piece) == Some(&true);
    let pieces = Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let mut state = GState::new(Matrix::IDENTITY, p.crop(doc), Vec::new());
    let mut out = Vec::new();
    let mut clip_work = 0_usize;
    let mut text = 0_usize;
    // The path being built: its first operator, and whether any other operator came in between.
    let mut start = None;
    let mut interrupted = false;
    for (index, op) in ops.iter().enumerate() {
        let painted =
            state.apply(op).map_err(|_| EditError::Invalid("a vector path on this page has invalid coordinates or is too complex".into()))?;
        let name = op.op.as_slice();
        if name == b"gs"
            && let Some(w) = gs_line_width(doc, &ext_gstates, op)
        {
            state.placement.line_width = w;
        }
        if matches!(name, b"cs" | b"CS") && named_pattern_space(doc, &colour_spaces, op) {
            let colour = if name == b"CS" { &mut state.placement.stroke } else { &mut state.placement.fill };
            colour.pattern = true;
        }
        if matches!(name, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re") {
            start.get_or_insert(index);
            continue;
        }
        if name == b"BT" {
            text = text.saturating_add(1);
        } else if name == b"ET" {
            text = text.saturating_sub(1);
        }
        if !matches!(name, b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*") {
            interrupted |= start.is_some() && !matches!(name, b"W" | b"W*");
            continue;
        }
        let (Some(first), Some(mut path), false) = (start.take(), painted, std::mem::take(&mut interrupted)) else { continue };
        // Under a text clip only the glyph outlines show, and those aren't modelled.
        if text > 0
            || state.placement.text_clip
            || (any_marked && (ops.get(first).is_some_and(|o| in_mark(pieces.pieces_of(o).0)) || in_mark(pieces.pieces_of(op).1)))
            || path.segments().next().is_none()
        {
            continue;
        }
        if matches!(name, b"s" | b"b" | b"b*") {
            path.close_path();
        }
        let [a, b, c, d, _, _] = state.placement.matrix.0;
        let line_width_user = state.placement.line_width * (a * d - b * c).abs().sqrt();
        if !bounded(&path) || !line_width_user.is_finite() {
            continue;
        }
        let paint = Paint {
            fill: match name {
                b"f" | b"F" | b"B" | b"b" => Some(FillRule::NonZero),
                b"f*" | b"B*" | b"b*" => Some(FillRule::EvenOdd),
                _ => None,
            },
            stroke: matches!(name, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*"),
        };
        let half = if paint.stroke { line_width_user.max(MIN_LINE_WIDTH) / 2.0 } else { 0.0 };
        let bounds = path.bounding_box();
        let (x0, x1) = widen(bounds.x0 - half, bounds.x1 + half);
        let (y0, y1) = widen(bounds.y0 - half, bounds.y1 + half);
        // Editing refuses a page with an object beyond its coordinate limit, so leave it out.
        let Some(rect) = intersection([x0, y0, x1, y1], state.placement.clip).filter(|r| crate::objects::valid_rect(*r)) else { continue };
        clip_work = state.placement.clips.iter().fold(clip_work, |sum, clip| sum.saturating_add(clip.path.elements().len()));
        if out.len() >= MAX_PAGE_PATHS {
            return Err(EditError::Invalid("this page has too many vector paths to select".into()));
        }
        if clip_work > MAX_CLIP_WORK {
            return Err(EditError::Invalid("the clipping on this page is too complex to select its vector paths".into()));
        }
        out.push(PagePath {
            rect,
            path,
            paint,
            fill_rgb: paint.fill.and(state.placement.fill.rgb),
            stroke_rgb: if paint.stroke { state.placement.stroke.rgb } else { None },
            line_width_user,
            pattern: (paint.fill.is_some() && state.placement.fill.pattern) || (paint.stroke && state.placement.stroke.pattern),
            ops: first..=index,
            matrix: state.placement.matrix,
            clips: state.placement.clips.clone(),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "paths_tests.rs"]
pub(crate) mod tests;
