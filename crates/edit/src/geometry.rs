//! The graphics state the page-object finders share: the CTM, `q`/`Q`, clipping paths, the
//! fill and stroke colours, the line width and the current path, built in user space.

use kurbo::{BezPath, PathEl, Point, Shape};
use pdfcraft_content::{Matrix, Op};
use std::sync::Arc;

use crate::EditError;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClipPath {
    pub(crate) path: Arc<BezPath>,
    pub(crate) even_odd: bool,
}

impl ClipPath {
    pub(crate) fn contains(&self, point: Point) -> bool {
        let winding = self.path.winding(point);
        if self.even_odd { winding % 2 != 0 } else { winding != 0 }
    }
}

/// Filling and clipping close every open subpath (ISO 32000-2 §8.5.3.1); kurbo's winding
/// closes only explicit `ClosePath`s.
pub(crate) fn closed(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut open = false;
    for el in path.elements() {
        if open && matches!(el, PathEl::MoveTo(_)) {
            out.close_path();
        }
        open = !matches!(el, PathEl::MoveTo(_) | PathEl::ClosePath);
        out.push(*el);
    }
    if open {
        out.close_path();
    }
    out
}

pub(crate) fn intersection(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// A fill or stroke colour: how many components its device colour space has (none for other
/// spaces), when it can be read an RGB approximation, and whether it is a pattern.
#[derive(Clone, Copy)]
pub(crate) struct Colour {
    components: Option<usize>,
    pub(crate) rgb: Option<[f64; 3]>,
    pub(crate) pattern: bool,
}

impl Colour {
    const BLACK: Colour = Colour { components: Some(1), rgb: Some([0.0; 3]), pattern: false };

    /// `g`/`rg`/`k` (and the stroke forms) and `sc`/`scn` in a device space.
    fn device(components: usize, op: &Op) -> Colour {
        let values: Option<Vec<f64>> = if op.operands.len() == components {
            op.operands.iter().map(|o| o.as_f64().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0))).collect()
        } else {
            None
        };
        let rgb = match values.as_deref() {
            Some(&[g]) => Some([g; 3]),
            Some(&[r, g, b]) => Some([r, g, b]),
            Some(&[c, m, y, k]) => Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
            _ => None,
        };
        Colour { components: Some(components), rgb, pattern: false }
    }
}

#[derive(Clone)]
pub(crate) struct Placement {
    pub(crate) matrix: Matrix,
    pub(crate) clip: [f64; 4],
    pub(crate) clips: Vec<ClipPath>,
    pub(crate) fill: Colour,
    pub(crate) stroke: Colour,
    pub(crate) line_width: f64,
    /// The text rendering mode (`Tr` 4 to 7) adds the glyphs shown to the clip.
    clipping_text: bool,
    /// Glyphs clip from here on (§9.3.6); their outlines aren't modelled.
    pub(crate) text_clip: bool,
}

/// Path coordinates are transformed when constructed; q/Q save clips, not the current path.
pub(crate) struct GState {
    pub(crate) placement: Placement,
    stack: Vec<Placement>,
    path: BezPath,
    pending_clip: Option<bool>,
    /// Glyphs were shown in a clipping mode in the current text object.
    glyph_clip: bool,
}

impl GState {
    pub(crate) fn new(matrix: Matrix, clip: [f64; 4], clips: Vec<ClipPath>) -> Self {
        Self {
            placement: Placement {
                matrix,
                clip,
                clips,
                fill: Colour::BLACK,
                stroke: Colour::BLACK,
                line_width: 1.0,
                clipping_text: false,
                text_clip: false,
            },
            stack: Vec::new(),
            path: BezPath::new(),
            pending_clip: None,
            glyph_clip: false,
        }
    }

    fn point(&self, x: f64, y: f64) -> Point {
        let (x, y) = self.placement.matrix.apply(x, y);
        Point::new(x, y)
    }

    /// Apply one operator. Returns the path a painting operator paints, unless it also clips.
    pub(crate) fn apply(&mut self, op: &Op) -> Result<Option<BezPath>, EditError> {
        if matches!(op.op.as_slice(), b"cm" | b"m" | b"l" | b"c" | b"v" | b"y" | b"re")
            && op.operands.iter().any(|v| v.as_f64().is_some_and(|v| !v.is_finite() || v.abs() > 1e12))
        {
            return Err(EditError::Invalid("the image path contains invalid coordinates".into()));
        }
        if self.path.elements().len() > 16384 || self.stack.len() > 4096 || self.placement.clips.len() > 256 {
            return Err(EditError::Invalid("the image clip or graphics state exceeds its complexity budget".into()));
        }
        // A segment or close before any `m` (or `re`) has no subpath to extend: kurbo panics on
        // it by contract, and the renderer, MuPDF and pdftotext skip it, so it is skipped (#3581).
        // `v` needs the current point itself and checks below.
        if matches!(op.op.as_slice(), b"l" | b"c" | b"y" | b"h") && self.path.elements().is_empty() {
            return Ok(None);
        }
        let stroke = op.op.first().is_some_and(u8::is_ascii_uppercase);
        let colour = if stroke { &mut self.placement.stroke } else { &mut self.placement.fill };
        match op.op.as_slice() {
            b"q" => self.stack.push(self.placement.clone()),
            b"Q" => {
                if let Some(saved) = self.stack.pop() {
                    self.placement = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    self.placement.matrix = Matrix(m).then(&self.placement.matrix);
                }
            }
            b"w" => {
                if let Some([w]) = op.nums::<1>().filter(|[w]| (0.0..=1e12).contains(w)) {
                    self.placement.line_width = w;
                }
            }
            b"Tr" => {
                if let Some([mode]) = op.nums::<1>() {
                    self.placement.clipping_text = (4.0..=7.0).contains(&mode);
                }
            }
            b"BT" => self.glyph_clip = false,
            b"Tj" | b"TJ" | b"'" | b"\"" => self.glyph_clip |= self.placement.clipping_text,
            b"ET" => self.placement.text_clip |= std::mem::take(&mut self.glyph_clip),
            b"g" | b"G" => *colour = Colour::device(1, op),
            b"rg" | b"RG" => *colour = Colour::device(3, op),
            b"k" | b"K" => *colour = Colour::device(4, op),
            b"cs" | b"CS" => {
                let components = match op.name(0) {
                    Some(b"DeviceGray") => Some(1),
                    Some(b"DeviceRGB") => Some(3),
                    Some(b"DeviceCMYK") => Some(4),
                    _ => None,
                };
                // Setting a colour space selects its initial colour: black for the device spaces.
                // A pattern space named by a resource is the caller's to mark.
                *colour = Colour { components, rgb: components.map(|_| [0.0; 3]), pattern: matches!(op.name(0), Some(b"Pattern")) };
            }
            // Other spaces have no RGB to update, and a pattern stays one.
            b"sc" | b"scn" | b"SC" | b"SCN" => {
                if let Some(n) = colour.components {
                    *colour = Colour::device(n, op);
                }
            }
            b"m" => {
                if let Some([x, y]) = op.nums::<2>() {
                    self.path.move_to(self.point(x, y));
                }
            }
            b"l" => {
                if let Some([x, y]) = op.nums::<2>() {
                    self.path.line_to(self.point(x, y));
                }
            }
            b"c" => {
                if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>() {
                    self.path.curve_to(self.point(x1, y1), self.point(x2, y2), self.point(x3, y3));
                }
            }
            b"v" => {
                if let Some([x1, y1, x2, y2]) = op.nums::<4>()
                    && let Some(current) = self.path.elements().last().and_then(|e| e.end_point())
                {
                    self.path.curve_to(current, self.point(x1, y1), self.point(x2, y2));
                }
            }
            b"y" => {
                if let Some([x1, y1, x2, y2]) = op.nums::<4>() {
                    let end = self.point(x2, y2);
                    self.path.curve_to(self.point(x1, y1), end, end);
                }
            }
            b"h" => self.path.close_path(),
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    self.path.move_to(self.point(x, y));
                    self.path.line_to(self.point(x + w, y));
                    self.path.line_to(self.point(x + w, y + h));
                    self.path.line_to(self.point(x, y + h));
                    self.path.close_path();
                }
            }
            b"W" => self.pending_clip = Some(false),
            b"W*" => self.pending_clip = Some(true),
            b"n" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                let path = std::mem::take(&mut self.path);
                let Some(even_odd) = self.pending_clip.take() else {
                    return Ok((!op.is("n")).then_some(path));
                };
                // Closing edges stay inside each subpath's hull, so these bounds (cubic
                // extrema, not controls) are the closed clip's too.
                let bounds = path.bounding_box();
                self.placement.clip = intersection(self.placement.clip, [bounds.x0, bounds.y0, bounds.x1, bounds.y1]).unwrap_or([0.0; 4]);
                self.placement.clips.push(ClipPath { path: Arc::new(closed(&path)), even_odd });
            }
            _ => {}
        }
        Ok(None)
    }
}
