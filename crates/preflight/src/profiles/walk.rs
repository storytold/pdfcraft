//! One pass over each page's content for the checks that look at what pages draw: images and
//! where they are placed, strokes and their width, the colour spaces content selects, the fonts it
//! shows text in, and the transparency it uses.
//!
//! The walk follows the current transformation matrix through `q`/`Q`/`cm` and into form
//! XObjects (with their `/Matrix` and `/Resources`), so an image's resolution and a line's width
//! are measured where they are drawn. It is bounded: a form nested deeper than [`MAX_DEPTH`] is
//! skipped and a form that draws itself is drawn once, each page reads at most [`MAX_OPS`]
//! operators, and the records are capped per page and per document. A page that hits a bound, or
//! whose content can't be decoded, is listed in [`Walk::unread`]: what it holds wasn't all checked.
//!
//! Not walked: annotation appearances, tiling patterns' cells, Type 3 glyph procedures, and text
//! drawn with a stroking render mode.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep form XObjects may nest.
const MAX_DEPTH: usize = 12;
/// Operators read per page, form contents included each time they are drawn.
const MAX_OPS: usize = 2_000_000;
/// Deepest `q` nesting kept; deeper saves are counted and restored without a state of their own.
const MAX_STACK: usize = 256;
/// Records of each kind kept per page.
const MAX_RECORDS: usize = 5_000;
/// Image and stroke records kept for the whole document.
const MAX_TOTAL_RECORDS: usize = 100_000;
/// The largest content stream decoded.
const MAX_STREAM: usize = 64 << 20;
/// How deep a colour space is followed to the spaces under it.
const MAX_SPACE_DEPTH: u8 = 4;

/// A colour space, as far as printing cares.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Colour {
    DeviceGray,
    DeviceRgb,
    DeviceCmyk,
    CalGray,
    CalRgb,
    Lab,
    /// ICC-based, with its number of components.
    Icc(u8),
    /// A Separation colorant (not `All` or `None`).
    Separation(String),
    /// DeviceN colorants, `None` left out.
    DeviceN(Vec<String>),
}

/// The process inks: a Separation or DeviceN colorant of these names prints on a CMYK plate.
const PROCESS: [&str; 4] = ["Cyan", "Magenta", "Yellow", "Black"];

impl Colour {
    /// How a hit names it: the PDF name of the space, which reads the same in every language.
    pub(super) fn label(&self) -> String {
        match self {
            Colour::DeviceGray => "DeviceGray".into(),
            Colour::DeviceRgb => "DeviceRGB".into(),
            Colour::DeviceCmyk => "DeviceCMYK".into(),
            Colour::CalGray => "CalGray".into(),
            Colour::CalRgb => "CalRGB".into(),
            Colour::Lab => "Lab".into(),
            Colour::Icc(1) => "ICCBased gray".into(),
            Colour::Icc(3) => "ICCBased RGB".into(),
            Colour::Icc(4) => "ICCBased CMYK".into(),
            Colour::Icc(n) => format!("ICCBased ({n})"),
            Colour::Separation(name) => format!("Separation {name}"),
            Colour::DeviceN(names) => format!("DeviceN {}", names.join(", ")),
        }
    }

    /// Whether a check that refuses `space` refuses this.
    pub(super) fn is(&self, space: super::Space) -> bool {
        use super::Space;
        match space {
            Space::Gray => matches!(self, Colour::DeviceGray | Colour::CalGray | Colour::Icc(1)),
            Space::Rgb => matches!(self, Colour::DeviceRgb | Colour::CalRgb | Colour::Icc(3)),
            Space::Cmyk => matches!(self, Colour::DeviceCmyk | Colour::Icc(4)),
            Space::Lab => matches!(self, Colour::Lab),
            Space::Icc => matches!(self, Colour::Icc(_) | Colour::CalGray | Colour::CalRgb),
            Space::Spot => !self.spots().is_empty(),
        }
    }

    /// The spot colorants it names: not the process inks, `All` or `None`.
    pub(super) fn spots(&self) -> Vec<&str> {
        let names: Vec<&str> = match self {
            Colour::Separation(n) => vec![n.as_str()],
            Colour::DeviceN(n) => n.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        };
        names.into_iter().filter(|n| !PROCESS.contains(n)).collect()
    }

    /// Colour components, for telling a colour image from a gray one.
    fn components(&self) -> u8 {
        match self {
            Colour::DeviceGray | Colour::CalGray | Colour::Separation(_) => 1,
            Colour::DeviceRgb | Colour::CalRgb | Colour::Lab => 3,
            Colour::DeviceCmyk => 4,
            Colour::Icc(n) => *n,
            Colour::DeviceN(n) => u8::try_from(n.len().max(1)).unwrap_or(u8::MAX),
        }
    }
}

/// What kind of image, for the resolution it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ImageKind {
    Colour,
    Gray,
    /// One bit per pixel, or an image mask.
    Bitmap,
}

/// An image where it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Placed {
    pub page: usize,
    /// The image XObject; `None` for an inline image.
    pub object: Option<u32>,
    pub kind: ImageKind,
    pub width: u32,
    pub height: u32,
    /// Effective pixels per inch on the page; NaN when the placement can't be measured.
    pub ppi: f64,
    /// Its box on the page, in user space.
    pub bbox: [f64; 4],
    /// Its size on the page, in points.
    pub size: [f64; 2],
}

/// A stroke thinner than the walk was asked to record.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stroke {
    pub page: usize,
    /// Its width where it is drawn, in points (0: the thinnest line the device can draw).
    pub width: f64,
    pub bbox: Option<[f64; 4]>,
    /// The form XObject it is drawn in, if any.
    pub object: Option<u32>,
}

/// Where content uses a colour space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Where {
    Content,
    Image,
    InlineImage,
    Shading,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ColourUse {
    pub page: usize,
    pub colour: Colour,
    pub at: Where,
    /// The image, shading or form it is in.
    pub object: Option<u32>,
    pub bbox: Option<[f64; 4]>,
}

/// A kind of transparency.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Transparency {
    SoftMask,
    Opacity,
    BlendMode(String),
    ImageSoftMask,
    PageGroup,
    FormGroup,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TransparencyUse {
    pub page: usize,
    pub kind: Transparency,
    /// The graphics state, image or form; the page itself for a page group.
    pub object: Option<u32>,
    pub bbox: Option<[f64; 4]>,
}

/// What the pages draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Walk {
    pub images: Vec<Placed>,
    pub strokes: Vec<Stroke>,
    pub colours: Vec<ColourUse>,
    /// Font objects text is shown in, and the first page that does.
    pub fonts: HashMap<u32, usize>,
    pub transparency: Vec<TransparencyUse>,
    /// 0-based pages not read to the end.
    pub unread: Vec<usize>,
}

/// The graphics state the walk follows.
#[derive(Clone, Copy)]
struct State {
    ctm: Matrix,
    line_width: f64,
}

/// One page's walk.
struct PageWalk<'a> {
    doc: &'a Document,
    page: usize,
    /// Strokes thinner than this are recorded.
    hairline_below: f64,
    /// Default user space units per point (`/UserUnit`).
    unit: f64,
    ops: usize,
    /// The operator budget ran out: the rest of the page isn't read.
    stopped: bool,
    /// Something on the page wasn't checked (a bound was hit, or content couldn't be decoded).
    incomplete: bool,
    /// Forms being drawn, innermost last.
    drawing: Vec<ObjRef>,
    parsed: HashMap<ObjRef, Arc<Vec<Op>>>,
    seen_colour: HashSet<(Colour, Where, Option<u32>)>,
    seen_transparency: HashSet<(Transparency, Option<u32>)>,
    out: &'a mut Walk,
    page_records: [usize; 4],
}

impl Walk {
    /// Walk every page. Strokes thinner than `hairline_below` points are recorded.
    pub(super) fn run(doc: &Document, pages: &[pdfcraft_model::Page], hairline_below: f64) -> Walk {
        let mut out = Walk::default();
        for (i, page) in pages.iter().enumerate() {
            let unit = page.dict.get(b"UserUnit").and_then(|u| doc.resolve(u).as_f64()).filter(|u| u.is_finite() && *u > 0.0).unwrap_or(1.0);
            let mut w = PageWalk {
                doc,
                page: i,
                hairline_below,
                unit,
                ops: 0,
                stopped: false,
                incomplete: false,
                drawing: Vec::new(),
                parsed: HashMap::new(),
                seen_colour: HashSet::new(),
                seen_transparency: HashSet::new(),
                out: &mut out,
                page_records: [0; 4],
            };
            if let Some(group) = page.dict.get(b"Group").map(|g| doc.resolve(g))
                && group.as_dict().is_some_and(|g| g.name(b"S") == Some(b"Transparency"))
            {
                w.transparency(Transparency::PageGroup, Some(page.obj.num), None);
            }
            let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
            let (ops, decoded) = page_ops(doc, &page.dict);
            w.incomplete |= !decoded;
            w.content(&ops, &resources, State { ctm: Matrix::IDENTITY, line_width: 1.0 });
            if w.incomplete {
                out.unread.push(i);
            }
        }
        out
    }
}

/// The page's content streams, parsed as one (a content array is one stream split up, so an
/// operator may span two of its pieces), and whether every piece could be decoded.
fn page_ops(doc: &Document, page: &Dict) -> (Vec<Op>, bool) {
    let parts: Vec<Object> = match page.get(b"Contents").map(|c| doc.resolve(c)) {
        Some(c) => match &*c {
            Object::Array(a) => a.clone(),
            Object::Stream(_) => page.get(b"Contents").cloned().into_iter().collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let mut data = Vec::new();
    let mut decoded = true;
    for part in parts {
        // A missing piece (null, or not a stream) draws nothing; one that won't decode is unread.
        if let Object::Stream(s) = &*doc.resolve(&part) {
            let (bytes, whole) = content_data(s, MAX_STREAM.saturating_sub(data.len()));
            decoded &= whole;
            data.extend_from_slice(&bytes);
            data.push(b'\n');
        }
    }
    (pdfcraft_content::parse(&data).ops, decoded)
}

/// A content stream's data, and whether all of it decoded. A damaged stream decodes as far as it
/// goes, as a viewer draws it, and one with a filter that can't be decoded gives nothing: either
/// way the rest isn't checked.
fn content_data(s: &pdfcraft_cos::Stream, max: usize) -> (Vec<u8>, bool) {
    let chain = s.filters();
    if chain.is_empty() {
        return match s.decoded_within(max) {
            Ok(data) => (data, true),
            Err(_) => (Vec::new(), false),
        };
    }
    match pdfcraft_filters::decode_tolerant(&chain, &s.raw, max.min(pdfcraft_cos::MAX_DECODED)) {
        Ok((data, partial)) => (data, !partial),
        Err(_) => (Vec::new(), false),
    }
}

/// The smallest scale a matrix applies in any direction (its smaller singular value).
fn min_scale(m: &Matrix) -> f64 {
    let [a, b, c, d, ..] = m.0;
    let s = a * a + b * b + c * c + d * d;
    let det = a * d - b * c;
    let root = (s * s - 4.0 * det * det).max(0.0).sqrt();
    ((s - root) / 2.0).max(0.0).sqrt()
}

/// Pixels per inch of a `width` × `height` image drawn through `m`, or NaN when the placement is
/// degenerate or not finite.
fn ppi(m: &Matrix, width: u32, height: u32, unit: f64) -> f64 {
    let [a, b, c, d, ..] = m.0;
    let (sx, sy) = (a.hypot(b) * unit, c.hypot(d) * unit);
    if width == 0 || height == 0 || !sx.is_finite() || !sy.is_finite() || sx <= 1e-6 || sy <= 1e-6 {
        return f64::NAN;
    }
    let ppi = (f64::from(width) / (sx / 72.0)).min(f64::from(height) / (sy / 72.0));
    if ppi.is_finite() { ppi } else { f64::NAN }
}

fn finite(b: [f64; 4]) -> Option<[f64; 4]> {
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// A non-negative integer from an image dictionary, under either of its names.
fn dim(d: &Dict, long: &[u8], short: &[u8]) -> u32 {
    d.get(long).or_else(|| d.get(short)).and_then(Object::as_int).and_then(|v| u32::try_from(v).ok()).unwrap_or(0)
}

/// The colour spaces `cs` stands for. A name that isn't a device space is looked up in
/// `resources`; inline images may use the abbreviations.
pub(super) fn classify(doc: &Document, cs: &Object, resources: Option<&Dict>, inline: bool, depth: u8, out: &mut Vec<Colour>) {
    if depth > MAX_SPACE_DEPTH {
        return;
    }
    let cs = doc.resolve(cs);
    match &*cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" => out.push(Colour::DeviceGray),
            b"DeviceRGB" => out.push(Colour::DeviceRgb),
            b"DeviceCMYK" => out.push(Colour::DeviceCmyk),
            b"G" if inline => out.push(Colour::DeviceGray),
            b"RGB" if inline => out.push(Colour::DeviceRgb),
            b"CMYK" if inline => out.push(Colour::DeviceCmyk),
            b"Pattern" => {}
            name => {
                if let Some(spaces) = resources.and_then(|r| r.get(b"ColorSpace")).map(|c| doc.resolve(c))
                    && let Some(found) = spaces.as_dict().and_then(|s| s.get(name))
                {
                    classify(doc, found, None, false, depth.saturating_add(1), out);
                }
            }
        },
        Object::Array(a) => {
            let Some(first) = a.first().and_then(Object::as_name) else { return };
            let under = |i: usize, out: &mut Vec<Colour>| {
                if let Some(base) = a.get(i) {
                    classify(doc, base, resources, inline, depth.saturating_add(1), out);
                }
            };
            match first {
                b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" => {
                    classify(doc, &Object::Name(first.to_vec()), None, false, depth.saturating_add(1), out)
                }
                b"CalGray" => out.push(Colour::CalGray),
                b"CalRGB" => out.push(Colour::CalRgb),
                b"Lab" => out.push(Colour::Lab),
                b"ICCBased" => {
                    let n = a.get(1).map(|s| doc.resolve(s)).and_then(|s| match &*s {
                        Object::Stream(s) => s.dict.int(b"N"),
                        _ => None,
                    });
                    out.push(Colour::Icc(n.and_then(|n| u8::try_from(n).ok()).unwrap_or(0)));
                }
                b"Indexed" | b"I" => under(1, out),
                b"Pattern" => under(1, out),
                b"Separation" => {
                    if let Some(name) = a.get(1).map(|n| doc.resolve(n)).and_then(|n| n.as_name().map(|n| String::from_utf8_lossy(n).into_owned()))
                        && name != "All"
                        && name != "None"
                    {
                        out.push(Colour::Separation(name));
                    }
                }
                b"DeviceN" => {
                    let names: Vec<String> = a
                        .get(1)
                        .map(|n| doc.resolve(n))
                        .and_then(|n| n.as_array().cloned())
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|n| n.as_name().map(|n| String::from_utf8_lossy(n).into_owned()))
                        .filter(|n| n != "None")
                        .collect();
                    if !names.is_empty() {
                        out.push(Colour::DeviceN(names));
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

impl PageWalk<'_> {
    fn colour(&mut self, colour: Colour, at: Where, object: Option<u32>, bbox: Option<[f64; 4]>) {
        if !self.seen_colour.insert((colour.clone(), at, object)) {
            return;
        }
        if self.page_records[0] >= MAX_RECORDS {
            self.incomplete = true;
            return;
        }
        self.page_records[0] += 1;
        self.out.colours.push(ColourUse { page: self.page, colour, at, object, bbox });
    }

    fn transparency(&mut self, kind: Transparency, object: Option<u32>, bbox: Option<[f64; 4]>) {
        if !self.seen_transparency.insert((kind.clone(), object)) {
            return;
        }
        if self.page_records[1] >= MAX_RECORDS {
            self.incomplete = true;
            return;
        }
        self.page_records[1] += 1;
        self.out.transparency.push(TransparencyUse { page: self.page, kind, object, bbox });
    }

    /// The form being drawn, if any: what a record made inside it names.
    fn current_form(&self) -> Option<u32> {
        self.drawing.last().map(|r| r.num)
    }

    fn spaces(&mut self, cs: &Object, resources: &Dict, inline: bool, at: Where, object: Option<u32>, bbox: Option<[f64; 4]>) -> Vec<Colour> {
        let mut found = Vec::new();
        classify(self.doc, cs, Some(resources), inline, 0, &mut found);
        for c in &found {
            self.colour(c.clone(), at, object, bbox);
        }
        found
    }

    fn resource(&self, resources: &Dict, kind: &[u8], name: &[u8]) -> Option<(Option<ObjRef>, Arc<Object>)> {
        let list = self.doc.resolve(resources.get(kind)?);
        let entry = list.as_dict()?.get(name)?;
        Some((entry.as_ref(), self.doc.resolve(entry)))
    }

    /// Read `ops` in `resources`, starting from `state`.
    fn content(&mut self, ops: &[Op], resources: &Dict, state: State) {
        let mut st = state;
        let mut stack: Vec<State> = Vec::new();
        let mut extra_saves = 0usize;
        let mut path: Option<[f64; 4]> = None;
        for op in ops {
            self.ops = self.ops.saturating_add(1);
            if self.ops > MAX_OPS {
                self.stopped = true;
                self.incomplete = true;
                return;
            }
            match op.op.as_slice() {
                b"q" => {
                    if stack.len() < MAX_STACK {
                        stack.push(st);
                    } else {
                        extra_saves = extra_saves.saturating_add(1);
                    }
                }
                b"Q" => {
                    if extra_saves > 0 {
                        extra_saves -= 1;
                    } else if let Some(s) = stack.pop() {
                        st = s;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>().map(Matrix)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        st.ctm = m.then(&st.ctm);
                    }
                }
                b"w" => {
                    if let Some(w) = op.num(0).filter(|w| w.is_finite() && *w >= 0.0) {
                        st.line_width = w;
                    }
                }
                b"gs" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some((at, gs)) = self.resource(resources, b"ExtGState", name) else { continue };
                    let Some(g) = gs.as_dict() else { continue };
                    if let Some(w) = g.get(b"LW").and_then(|w| self.doc.resolve(w).as_f64()).filter(|w| w.is_finite() && *w >= 0.0) {
                        st.line_width = w;
                    }
                    let object = at.map(|r| r.num).or(self.current_form());
                    if g.get(b"SMask").is_some_and(|m| self.doc.resolve(m).as_name() != Some(b"None")) {
                        self.transparency(Transparency::SoftMask, object, None);
                    }
                    if [b"CA".as_slice(), b"ca".as_slice()]
                        .iter()
                        .any(|k| g.get(k).and_then(|a| self.doc.resolve(a).as_f64()).is_some_and(|a| a < 1.0 - 1e-6))
                    {
                        self.transparency(Transparency::Opacity, object, None);
                    }
                    let modes: Vec<Vec<u8>> = match g.get(b"BM").map(|m| self.doc.resolve(m)).as_deref() {
                        Some(Object::Name(n)) => vec![n.clone()],
                        Some(Object::Array(a)) => a.iter().filter_map(|m| m.as_name().map(<[u8]>::to_vec)).collect(),
                        _ => Vec::new(),
                    };
                    if let Some(mode) = modes.into_iter().find(|m| !matches!(m.as_slice(), b"Normal" | b"Compatible")) {
                        self.transparency(Transparency::BlendMode(String::from_utf8_lossy(&mode).into_owned()), object, None);
                    }
                }
                b"m" | b"l" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        grow(&mut path, &st.ctm, x, y);
                    }
                }
                b"c" => {
                    if let Some([x1, y1, x2, y2, x3, y3]) = op.nums::<6>() {
                        for (x, y) in [(x1, y1), (x2, y2), (x3, y3)] {
                            grow(&mut path, &st.ctm, x, y);
                        }
                    }
                }
                b"v" | b"y" => {
                    if let Some([x1, y1, x2, y2]) = op.nums::<4>() {
                        grow(&mut path, &st.ctm, x1, y1);
                        grow(&mut path, &st.ctm, x2, y2);
                    }
                }
                b"re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        for (px, py) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                            grow(&mut path, &st.ctm, px, py);
                        }
                    }
                }
                b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                    let width = st.line_width * min_scale(&st.ctm) * self.unit;
                    if width.is_finite() && width < self.hairline_below {
                        if self.page_records[2] < MAX_RECORDS && self.out.strokes.len() < MAX_TOTAL_RECORDS {
                            self.page_records[2] += 1;
                            self.out.strokes.push(Stroke { page: self.page, width, bbox: path, object: self.current_form() });
                        } else {
                            self.incomplete = true;
                        }
                    }
                    path = None;
                }
                b"f" | b"F" | b"f*" | b"n" => path = None,
                b"g" | b"G" => self.colour(Colour::DeviceGray, Where::Content, self.current_form(), None),
                b"rg" | b"RG" => self.colour(Colour::DeviceRgb, Where::Content, self.current_form(), None),
                b"k" | b"K" => self.colour(Colour::DeviceCmyk, Where::Content, self.current_form(), None),
                b"cs" | b"CS" => {
                    if let Some(name) = op.name(0) {
                        let form = self.current_form();
                        self.spaces(&Object::Name(name.to_vec()), resources, false, Where::Content, form, None);
                    }
                }
                b"scn" | b"SCN" => {
                    // A shading pattern's colour space; a tiling pattern's cell isn't walked.
                    let Some(name) = op.operands.last().and_then(Object::as_name) else { continue };
                    let Some((at, pattern)) = self.resource(resources, b"Pattern", name) else { continue };
                    let shading = match &*pattern {
                        Object::Dict(d) => d.get(b"Shading").cloned(),
                        Object::Stream(s) => s.dict.get(b"Shading").cloned(),
                        _ => None,
                    };
                    if let Some(sh) = shading.map(|s| self.doc.resolve(&s))
                        && let Some(cs) = sh.as_dict().and_then(|d| d.get(b"ColorSpace")).cloned()
                    {
                        let object = at.map(|r| r.num).or(self.current_form());
                        self.spaces(&cs, resources, false, Where::Shading, object, None);
                    }
                }
                b"sh" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some((at, sh)) = self.resource(resources, b"Shading", name) else { continue };
                    let d = match &*sh {
                        Object::Dict(d) => Some(d.clone()),
                        Object::Stream(s) => Some(s.dict.clone()),
                        _ => None,
                    };
                    if let Some(cs) = d.and_then(|d| d.get(b"ColorSpace").cloned()) {
                        let object = at.map(|r| r.num).or(self.current_form());
                        self.spaces(&cs, resources, false, Where::Shading, object, None);
                    }
                }
                b"Tf" => {
                    if let Some(name) = op.name(0)
                        && let Some((Some(at), _)) = self.resource(resources, b"Font", name)
                    {
                        self.out.fonts.entry(at.num).or_insert(self.page);
                    }
                }
                b"BI" => {
                    let Some((image, _)) = &op.inline else { continue };
                    self.image(image, None, resources, &st, true);
                }
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some((at, x)) = self.resource(resources, b"XObject", name) else { continue };
                    let Object::Stream(s) = &*x else { continue };
                    match s.dict.name(b"Subtype") {
                        Some(b"Image") => self.image(&s.dict, at, resources, &st, false),
                        Some(b"Form") => {
                            let Some(at) = at else { continue };
                            self.form(at, s, resources, &st);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if self.stopped {
                return;
            }
        }
    }

    fn image(&mut self, d: &Dict, at: Option<ObjRef>, resources: &Dict, st: &State, inline: bool) {
        let width = dim(d, b"Width", b"W");
        let height = dim(d, b"Height", b"H");
        let bpc = d.get(b"BitsPerComponent").or_else(|| d.get(b"BPC")).and_then(Object::as_int).unwrap_or(8);
        let mask = matches!(d.get(b"ImageMask").or_else(|| d.get(b"IM")), Some(Object::Bool(true)));
        let bbox = finite(st.ctm.bbox([0.0, 0.0, 1.0, 1.0]));
        let object = at.map(|r| r.num);
        let where_ = if inline { Where::InlineImage } else { Where::Image };
        let spaces = match d.get(b"ColorSpace").or_else(|| if inline { d.get(b"CS") } else { None }) {
            Some(cs) if !mask => self.spaces(cs, resources, inline, where_, object, bbox),
            _ => Vec::new(),
        };
        let components = spaces.last().map_or(3, Colour::components);
        let kind = if mask || (bpc == 1 && components == 1) {
            ImageKind::Bitmap
        } else if components == 1 {
            ImageKind::Gray
        } else {
            ImageKind::Colour
        };
        if !inline
            && (d.get(b"SMask").is_some_and(|m| !matches!(&*self.doc.resolve(m), Object::Null)) || d.int(b"SMaskInData").is_some_and(|n| n > 0))
        {
            self.transparency(Transparency::ImageSoftMask, object, bbox);
        }
        if self.page_records[3] >= MAX_RECORDS || self.out.images.len() >= MAX_TOTAL_RECORDS {
            self.incomplete = true;
            return;
        }
        self.page_records[3] += 1;
        let [a, b, c, dd, ..] = st.ctm.0;
        let size = [a.hypot(b) * self.unit, c.hypot(dd) * self.unit];
        self.out.images.push(Placed {
            page: self.page,
            object,
            kind,
            width,
            height,
            ppi: ppi(&st.ctm, width, height, self.unit),
            bbox: bbox.unwrap_or([0.0; 4]),
            size,
        });
    }

    fn form(&mut self, at: ObjRef, s: &pdfcraft_cos::Stream, resources: &Dict, st: &State) {
        // Nested deeper than any real file nests: this form isn't read, the rest of the page is.
        if self.drawing.len() >= MAX_DEPTH {
            self.incomplete = true;
            return;
        }
        // A form that draws itself, directly or through others, is drawn once.
        if self.drawing.contains(&at) {
            return;
        }
        let matrix = s
            .dict
            .get(b"Matrix")
            .map(|m| self.doc.resolve(m))
            .and_then(|m| m.as_array().and_then(|a| Matrix::from_operands(a)))
            .filter(|m| m.0.iter().all(|v| v.is_finite()))
            .unwrap_or(Matrix::IDENTITY);
        let inner = State { ctm: matrix.then(&st.ctm), line_width: st.line_width };
        if s.dict.get(b"Group").map(|g| self.doc.resolve(g)).is_some_and(|g| g.as_dict().is_some_and(|g| g.name(b"S") == Some(b"Transparency"))) {
            let bbox = form_bbox(self.doc, s, &inner.ctm);
            self.transparency(Transparency::FormGroup, Some(at.num), bbox);
        }
        let own = s.dict.get(b"Resources").map(|r| self.doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let ops = match self.parsed.get(&at) {
            Some(ops) => ops.clone(),
            None => {
                let (data, whole) = content_data(s, MAX_STREAM);
                self.incomplete |= !whole;
                let ops = Arc::new(pdfcraft_content::parse(&data).ops);
                self.parsed.insert(at, ops.clone());
                ops
            }
        };
        self.drawing.push(at);
        self.content(&ops, own.as_ref().unwrap_or(resources), inner);
        self.drawing.pop();
    }
}

/// A form's box on the page, drawn through `ctm` (its own matrix included).
fn form_bbox(doc: &Document, s: &pdfcraft_cos::Stream, ctm: &Matrix) -> Option<[f64; 4]> {
    let b: Vec<f64> = s.dict.get(b"BBox").map(|b| doc.resolve(b))?.as_array()?.iter().filter_map(Object::as_f64).collect();
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(b).ok()?;
    finite(ctm.bbox([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]))
}

/// Grow a path's box by a point, drawn through `ctm`.
fn grow(path: &mut Option<[f64; 4]>, ctm: &Matrix, x: f64, y: f64) {
    let (x, y) = ctm.apply(x, y);
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    *path = Some(match *path {
        Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
        None => [x, y, x, y],
    });
}
