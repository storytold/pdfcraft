//! Image-only coverage. Rectangular path fills deliberately retain their existing semantics.
//! ISO 32000-1 §8.9: samples run from the top left; every image maps to the unit square.
use std::sync::Arc;

use crate::{AuditError, TextLine};
use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, Stream};

type Point = [f64; 2];
pub(crate) type Polygon = Vec<Point>;
// Keep geometry arithmetic finite, including cross products used while clipping.
pub(crate) const MAX_COORD: f64 = 1.0e9;
const MAX_PIXELS: usize = 1_000_000;
const MAX_VERTICES: usize = 100_000;

#[derive(Default)]
pub(crate) struct Budget {
    pixels: usize,
    vertices: usize,
}

/// Report an unsupported or invalid image as an incomplete audit, never a clean result.
fn incomplete(reason: &str) -> AuditError {
    AuditError::Image(reason.into())
}

/// Transform a rectangle into a polygon, reversing reflected vertices to keep positive area.
pub(crate) fn rectangle(m: Matrix, r: [f64; 4]) -> Polygon {
    let [x0, y0, x1, y1] = r;
    let mut p: Polygon = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .into_iter()
        .map(|(x, y)| {
            let (x, y) = m.apply(x, y);
            [x, y]
        })
        .collect();
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    p
}
/// Signed cross product of `AB` and `AC`; nonnegative means `C` is left of directed `AB`.
fn cross(a: Point, b: Point, c: Point) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
/// Oriented polygon area, positive for counterclockwise vertices and zero for an empty path.
fn signed_area(p: &[Point]) -> f64 {
    let Some(&origin) = p.first() else { return 0.0 };
    p.iter().zip(p.iter().cycle().skip(1)).take(p.len()).map(|(&a, &b)| cross(origin, a, b)).sum::<f64>() * 0.5
}
/// Whether every coordinate is finite and within the image audit's arithmetic bounds.
fn valid_polygon(p: &[Point]) -> bool {
    p.iter().flatten().all(|v| v.is_finite() && v.abs() <= MAX_COORD)
}

/// Clip `p` against a counterclockwise convex polygon; fewer than three clip vertices is empty.
fn intersection(mut p: Polygon, clip: &[Point]) -> Polygon {
    if clip.len() < 3 {
        return Vec::new();
    }
    for (&a, &b) in clip.iter().zip(clip.iter().cycle().skip(1)).take(clip.len()) {
        let input = std::mem::take(&mut p);
        for (&s, &e) in input.iter().zip(input.iter().cycle().skip(1)).take(input.len()) {
            let ds = cross(a, b, s);
            let de = cross(a, b, e);
            if ds >= 0.0 {
                p.push(s);
            }
            if (ds >= 0.0) != (de >= 0.0) {
                let t = ds / (ds - de);
                p.push([s[0] + t * (e[0] - s[0]), s[1] + t * (e[1] - s[1])]);
            }
        }
    }
    p
}
/// Axis-aligned bounds of the vertices; an empty polygon retains inverted infinite bounds.
pub(crate) fn bounds(p: &[Point]) -> [f64; 4] {
    p.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |r, p| {
        [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
    })
}

/// State used only by image detection; unsupported clips are errors if an image uses them.
#[derive(Clone, Default)]
pub(crate) struct State {
    clip: Option<Arc<Polygon>>,
    unknown: bool,
    pub alpha: Option<f64>,
    soft_mask: bool,
    blend: bool,
    overprint: bool,
    effects: bool,
    pub color_unknown: bool,
    text_mode: i64,
}
impl State {
    /// Intersect the current image clip with `p`; unsupported or oversized geometry makes
    /// clipping unknown until the enclosing graphics state is restored.
    pub fn clip(&mut self, p: Option<Polygon>) {
        if self.unknown {
            return;
        }
        match p {
            Some(p) if valid_polygon(&p) => {
                // Store the intersection, bounding memory even with repeated W operators.
                let p = if let Some(previous) = self.clip.take() { intersection((*previous).clone(), &p) } else { p };
                if p.len() > 128 || !valid_polygon(&p) {
                    self.unknown = true;
                } else {
                    self.clip = Some(Arc::new(p));
                }
            }
            _ => self.unknown = true,
        }
    }
    /// Apply named ExtGState entries relevant to image opacity and compositing, retaining
    /// unspecified values and recording unsupported effects for a later incomplete result.
    pub fn ext(&mut self, doc: &Document, res: &Dict, name: &[u8]) {
        let states = res.get(b"ExtGState").map(|o| doc.resolve(o));
        let state = states.as_ref().and_then(|o| o.as_dict()).and_then(|d| d.get(name)).map(|o| doc.resolve(o));
        let Some(d) = state.as_ref().and_then(|o| o.as_dict()) else {
            self.effects = true;
            return;
        };
        if let Some(a) = d.get(b"ca") {
            self.alpha = Some(doc.resolve(a).as_f64().filter(|a| a.is_finite() && (0.0..=1.0).contains(a)).unwrap_or(f64::NAN));
        }
        // Unspecified entries retain their previous graphics-state values.
        if let Some(s) = d.get(b"SMask") {
            self.soft_mask = doc.resolve(s).as_name() != Some(b"None");
        }
        if let Some(b) = d.get(b"BM") {
            self.blend = !matches!(doc.resolve(b).as_name(), Some(b"Normal" | b"Compatible"));
        }
        if let Some(o) = d.get(b"op").or_else(|| d.get(b"OP")) {
            self.overprint = !matches!(&*doc.resolve(o), Object::Bool(false));
        }
        for key in [b"TR".as_slice(), b"TR2", b"HT", b"BG", b"BG2", b"UCR", b"UCR2"] {
            if d.get(key).is_some() {
                self.effects = true;
            }
        }
    }
    /// Record default device colour-space overrides that prevent direct sample interpretation.
    pub fn resources(&mut self, doc: &Document, resources: &Dict) {
        if let Some(o) = resources.get(b"ColorSpace") {
            let o = doc.resolve(o);
            if let Some(d) = o.as_dict() {
                self.effects |= [b"DefaultGray".as_slice(), b"DefaultRGB", b"DefaultCMYK"].iter().any(|k| d.get(k).is_some());
            }
        }
    }
    /// Track text clipping, nonstroking colour validity and invalid transforms that affect
    /// whether a subsequent image can be audited completely.
    pub fn observe(&mut self, op: &Op) {
        match op.op.as_slice() {
            b"Tr" => self.text_mode = op.num(0).unwrap_or(0.0) as i64,
            b"Tj" | b"TJ" | b"'" | b"\"" if (4..=7).contains(&self.text_mode) => self.unknown = true,
            b"cs" | b"sc" | b"scn" => self.color_unknown = true,
            b"g" => self.color_unknown = !op.num(0).is_some_and(|v| v.is_finite()),
            b"rg" => self.color_unknown = !op.nums::<3>().is_some_and(|v| v.iter().all(|n| n.is_finite())),
            b"k" => self.color_unknown = !op.nums::<4>().is_some_and(|v| v.iter().all(|n| n.is_finite())),
            b"cm" if !Matrix::from_operands(&op.operands).is_some_and(|m| m.0.iter().all(|v| v.is_finite())) => self.unknown = true,
            _ => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tile {
    polygon: Polygon,
    rgb: [f64; 3],
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Coverage {
    tiles: Vec<Tile>,
}
impl Coverage {
    /// Comparison cost in stored polygon vertices, charging at least one unit per cover.
    pub fn cost(&self) -> usize {
        self.tiles.iter().map(|t| t.polygon.len()).sum::<usize>().max(1)
    }
    /// Whether low-contrast tiles substantially hide text painted before `pos`, using the
    /// audit's word and line area thresholds and excluding later runs of a joined line.
    pub fn covers(&self, line: &TextLine, pos: (usize, usize)) -> bool {
        let ink: Vec<[f64; 4]> = if line.painted_word_runs().next().is_some() {
            line.painted_word_runs().filter(|(p, _)| *p < pos).flat_map(|(_, words)| words.iter().copied()).collect()
        } else {
            line.ink.clone()
        };
        if ink.is_empty() && !line.ink.is_empty() {
            return false;
        }
        let line_rect = if ink.is_empty() {
            line.rect
        } else {
            ink.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, b| {
                [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
            })
        };
        let hidden = |r: [f64; 4]| {
            let clip = rectangle(Matrix::IDENTITY, r);
            self.tiles
                .iter()
                .filter(|t| crate::contrast(t.rgb, line.color) < crate::COVER_CONTRAST)
                .map(|t| signed_area(&intersection(t.polygon.clone(), &clip)).abs())
                .sum::<f64>()
        };
        if ink.iter().any(|&r| crate::area(&r) > 0.0 && hidden(r) / crate::area(&r) >= crate::WORD_FRACTION) {
            return true;
        }
        let total: f64 = ink.iter().map(crate::area).sum();
        let covered: f64 = ink.iter().map(|&r| hidden(r)).sum();
        (total > 0.0 && covered / total >= crate::COVER_FRACTION)
            || (crate::area(&line_rect) > 0.0
                && hidden(line_rect) / crate::area(&line_rect) >= crate::COVER_FRACTION
                && (ink.is_empty() || covered > 0.0))
    }
}

/// Normalize the abbreviated inline dictionary before strict filter decoding.
pub(crate) fn inline(dict: &Dict, data: &[u8]) -> Stream {
    let mut d = Dict::new();
    for (k, v) in dict.iter() {
        let key = match k.as_slice() {
            b"W" => b"Width".as_slice(),
            b"H" => b"Height",
            b"BPC" => b"BitsPerComponent",
            b"CS" => b"ColorSpace",
            b"IM" => b"ImageMask",
            b"D" => b"Decode",
            b"F" => b"Filter",
            b"DP" => b"DecodeParms",
            b"I" => b"Interpolate",
            _ => k,
        };
        /// Expand inline colour-space and lossless-filter name abbreviations; retain other values.
        fn expand_name(v: &Object) -> Object {
            match v {
                Object::Name(n) => Object::Name(match n.as_slice() {
                    b"G" => b"DeviceGray".to_vec(),
                    b"RGB" => b"DeviceRGB".to_vec(),
                    b"CMYK" => b"DeviceCMYK".to_vec(),
                    b"Fl" => b"FlateDecode".to_vec(),
                    b"AHx" => b"ASCIIHexDecode".to_vec(),
                    b"A85" => b"ASCII85Decode".to_vec(),
                    b"RL" => b"RunLengthDecode".to_vec(),
                    b"LZW" => b"LZWDecode".to_vec(),
                    _ => n.clone(),
                }),
                _ => v.clone(),
            }
        }
        let value = match v {
            Object::Array(a) => Object::Array(a.iter().map(expand_name).collect()),
            _ => expand_name(v),
        };
        d.set(key.to_vec(), value);
    }
    Stream::from_raw(d, data.to_vec())
}

struct Samples {
    width: usize,
    height: usize,
    channels: usize,
    bits: usize,
    data: Vec<u8>,
    decode: Vec<[f64; 2]>,
    stencil: bool,
}
impl Samples {
    /// Decode supported device-colour or stencil samples under shared byte and pixel budgets.
    /// Invalid dimensions, unsupported formats, malformed Decode arrays and short data error.
    fn read(doc: &Document, s: &Stream, budget: &mut DecodeBudget, pixels: &mut usize) -> Result<Self, AuditError> {
        let d = &s.dict;
        let integer = |k: &[u8]| {
            d.get(k)
                .and_then(|o| doc.resolve(o).as_int())
                .and_then(|v| usize::try_from(v).ok())
                .filter(|&v| v > 0)
                .ok_or_else(|| incomplete("invalid image dimensions or bit depth"))
        };
        let width = integer(b"Width")?;
        let height = integer(b"Height")?;
        let count = width.checked_mul(height).ok_or_else(|| incomplete("image sample bound exceeded"))?;
        *pixels = pixels.checked_add(count).filter(|&n| n <= MAX_PIXELS).ok_or_else(|| incomplete("image sample bound exceeded"))?;
        let stencil = d.get(b"ImageMask").is_some_and(|o| matches!(&*doc.resolve(o), Object::Bool(true)));
        let channels = if stencil {
            1
        } else {
            match d.get(b"ColorSpace").map(|o| doc.resolve(o)).as_deref().and_then(Object::as_name) {
                Some(b"DeviceGray") => 1,
                Some(b"DeviceRGB") => 3,
                Some(b"DeviceCMYK") => 4,
                _ => return Err(incomplete("unsupported image colour space")),
            }
        };
        let bits = if stencil && d.get(b"BitsPerComponent").is_none() { 1 } else { integer(b"BitsPerComponent")? };
        if ![1, 2, 4, 8, 16].contains(&bits) || (stencil && bits != 1) {
            return Err(incomplete("unsupported image bit depth"));
        }
        if d.get(b"Interpolate").is_some_and(|o| matches!(&*doc.resolve(o), Object::Bool(true))) {
            return Err(incomplete("interpolated image samples"));
        }
        let mut decode = vec![[0.0, 1.0]; channels];
        if let Some(v) = d.get(b"Decode") {
            let v = doc.resolve(v);
            let a = v.as_array().ok_or_else(|| incomplete("invalid image Decode"))?;
            if a.len() != channels * 2 {
                return Err(incomplete("invalid image Decode"));
            }
            for (range, pair) in decode.iter_mut().zip(a.as_chunks::<2>().0) {
                for (to, from) in range.iter_mut().zip(pair) {
                    *to = doc.resolve(from).as_f64().filter(|v| v.is_finite()).ok_or_else(|| incomplete("invalid image Decode"))?;
                }
            }
        }
        if stencil && decode[0] != [0.0, 1.0] && decode[0] != [1.0, 0.0] {
            return Err(incomplete("invalid stencil Decode"));
        }
        let data = budget.decode_strict(s)?;
        // width * height is at most MAX_PIXELS; channels <= 4 and bits <= 16,
        // so all row/sample offsets below fit even on wasm32.
        let row = width * channels * bits;
        let len = row.div_ceil(8) * height;
        if data.len() < len {
            return Err(incomplete("truncated image samples"));
        }
        Ok(Self { width, height, channels, bits, data, decode, stencil })
    }
    /// Read one packed component at a validated pixel coordinate, apply its Decode interval
    /// and clamp to the unit range. Scanline padding is excluded from component offsets.
    fn value(&self, x: usize, y: usize, c: usize) -> f64 {
        let row = (self.width * self.channels * self.bits).div_ceil(8);
        let start = y * row * 8 + (x * self.channels + c) * self.bits;
        let mut raw = 0u32;
        for bit in start..start + self.bits {
            raw = (raw << 1) | u32::from((self.data.get(bit / 8).copied().unwrap_or(0) >> (7 - bit % 8)) & 1);
        }
        let [lo, hi] = self.decode.get(c).copied().unwrap_or([0.0, 1.0]);
        (lo + (hi - lo) * f64::from(raw) / f64::from((1u32 << self.bits) - 1)).clamp(0.0, 1.0)
    }
    /// Convert a validated pixel to RGB; stencils use `fill` for painted bits and return
    /// `None` for holes, respecting the decoded stencil polarity.
    fn rgb(&self, x: usize, y: usize, fill: [f64; 3]) -> Option<[f64; 3]> {
        let v = |c| self.value(x, y, c);
        if self.stencil {
            return (v(0) == 0.0).then_some(fill);
        }
        Some(match self.channels {
            1 => [v(0); 3],
            3 => [v(0), v(1), v(2)],
            _ => [(1.0 - v(0)) * (1.0 - v(3)), (1.0 - v(1)) * (1.0 - v(3)), (1.0 - v(2)) * (1.0 - v(3))],
        })
    }
}

/// Collect opaque sample tiles transformed by `ctm` and intersected with the active clip.
/// Returns their bounds and coverage, or `None` for no painted area or translucent images.
/// Unsupported compositing, masks, decoding or exhausted work budgets fail the audit.
pub(crate) fn collect(
    doc: &Document,
    s: &Stream,
    budget: &mut DecodeBudget,
    work: &mut Budget,
    state: &State,
    ctm: Matrix,
    fill: [f64; 3],
) -> Result<Option<([f64; 4], Coverage)>, AuditError> {
    if state.alpha == Some(0.0) {
        return Ok(None);
    }
    if state.unknown || (state.soft_mask || state.blend || state.overprint || state.effects) || state.alpha.is_some_and(|a| !a.is_finite()) {
        return Err(incomplete("unsupported image clipping or graphics-state transparency"));
    }
    if !ctm.0.iter().all(|v| v.is_finite()) {
        return Err(incomplete("non-finite image transform"));
    }
    let [a, b, c, d, ..] = ctm.0;
    if !(a * d - b * c).is_finite() {
        return Err(incomplete("image transform overflow"));
    }
    if a * d - b * c == 0.0 {
        return Ok(None);
    }
    let samples = Samples::read(doc, s, budget, &mut work.pixels)?;
    if samples.stencil && state.color_unknown {
        return Err(incomplete("stencil colour space requires further analysis"));
    }
    let mut mask = None;
    if s.dict.get(b"Mask").is_some() {
        return Err(incomplete("image Mask requires further analysis"));
    }
    if let Some(o) = s.dict.get(b"SMask") {
        let o = doc.resolve(o);
        let Object::Stream(s) = &*o else {
            return Err(incomplete("invalid image soft mask"));
        };
        if s.dict.get(b"Matte").is_some() || s.dict.get(b"SMask").is_some() || s.dict.get(b"Mask").is_some() {
            return Err(incomplete("complex image soft mask"));
        }
        let m = Samples::read(doc, s, budget, &mut work.pixels)?;
        if m.channels != 1 || m.stencil || m.width != samples.width || m.height != samples.height {
            return Err(incomplete("soft mask geometry or colour space"));
        }
        mask = Some(m);
    }
    if s.dict.get(b"SMaskInData").is_some_and(|o| doc.resolve(o).as_int() != Some(0)) {
        return Err(incomplete("embedded image alpha"));
    }
    // A translucent image follows the same policy as translucent rectangle fills.
    if state.alpha.is_some_and(|a| a < 1.0) {
        return Ok(None);
    }
    let mut out = Vec::<Tile>::new();
    for y in 0..samples.height {
        let color = |x| if mask.as_ref().is_some_and(|m| m.value(x, y, 0) < 1.0) { None } else { samples.rgb(x, y, fill) };
        let mut x = 0;
        while x < samples.width {
            let rgb = color(x);
            let start = x;
            x += 1;
            while x < samples.width && color(x) == rgb {
                x += 1;
            }
            let Some(rgb) = rgb else { continue };
            let r = [
                start as f64 / samples.width as f64,
                1.0 - (y + 1) as f64 / samples.height as f64,
                x as f64 / samples.width as f64,
                1.0 - y as f64 / samples.height as f64,
            ];
            let mut polygon = rectangle(ctm, r);
            if !valid_polygon(&polygon) {
                return Err(incomplete("image transform overflow"));
            }
            if let Some(clip) = &state.clip {
                polygon = intersection(polygon, clip);
            }
            if !signed_area(&polygon).is_finite() {
                return Err(incomplete("image geometry overflow"));
            }
            if polygon.len() < 3 || signed_area(&polygon).abs() == 0.0 {
                continue;
            }
            work.vertices =
                work.vertices.checked_add(polygon.len()).filter(|&n| n <= MAX_VERTICES).ok_or_else(|| incomplete("image coverage bound exceeded"))?;
            out.push(Tile { polygon, rgb });
        }
    }
    if out.is_empty() {
        return Ok(None);
    }
    let rect = bounds(&out.iter().flat_map(|t| t.polygon.iter().copied()).collect::<Vec<_>>());
    Ok(Some((rect, Coverage { tiles: out })))
}

/// The current path belongs to the content stream, rather than the q/Q graphics-state stack.
/// Convex polygon clips are exact. Multiple subpaths and curves require further analysis.
#[derive(Default)]
pub(crate) struct ClipPath {
    active: bool,
    points: Option<Polygon>,
    pending: bool,
}
impl ClipPath {
    /// Accumulate a supported path in user space and apply pending clipping when a path-ending
    /// operator arrives. Curves, compound or nonconvex paths mark image clipping unknown.
    pub fn observe(&mut self, op: &Op, ctm: Matrix, state: &mut State) {
        match op.op.as_slice() {
            b"re" => {
                self.points = if self.active {
                    None
                } else {
                    op.nums::<4>().map(|[x, y, w, h]| rectangle(ctm, [x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]))
                };
                self.active = true;
            }
            b"m" => {
                self.points = if self.active {
                    None
                } else {
                    op.nums::<2>().map(|[x, y]| {
                        let (x, y) = ctm.apply(x, y);
                        vec![[x, y]]
                    })
                };
                self.active = true;
            }
            b"l" => {
                if let Some(p) = &mut self.points {
                    if p.len() >= 5 {
                        self.points = None;
                    } else if let Some([x, y]) = op.nums::<2>() {
                        let (x, y) = ctm.apply(x, y);
                        p.push([x, y]);
                    } else {
                        self.points = None;
                    }
                }
            }
            b"c" | b"v" | b"y" => {
                self.active = true;
                self.points = None;
            }
            b"W" | b"W*" => self.pending = true,
            b"n" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => {
                if self.pending {
                    let polygon = self.points.take().and_then(|mut p| {
                        if p.len() == 5 && p.first() == p.last() {
                            p.pop();
                        }
                        if !(3..=4).contains(&p.len()) {
                            return None;
                        }
                        let area = signed_area(&p);
                        if area < 0.0 {
                            p.reverse();
                        }
                        let convex = p
                            .iter()
                            .zip(p.iter().cycle().skip(1))
                            .zip(p.iter().cycle().skip(2))
                            .take(p.len())
                            .all(|((&a, &b), &c)| cross(a, b, c) >= 0.0);
                        if !convex || !area.is_finite() {
                            return None;
                        }
                        Some(if area == 0.0 { Vec::new() } else { p })
                    });
                    state.clip(polygon);
                }
                *self = Self::default();
            }
            _ => {}
        }
    }
}
