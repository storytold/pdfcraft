//! Colour converted to CMYK through the output profile, for PDF/X-1a, which carries CMYK, gray
//! and spot colour only.
//!
//! What converts:
//! - colour that content sets in DeviceRGB, an ICC-based gray, RGB or CMYK space, or one of
//!   PDF's own CIE-based spaces, CalGray, CalRGB and Lab (`rg`/`RG`, and `cs`/`CS` followed by
//!   `sc`/`scn`/`SC`/`SCN`);
//! - pictures in those spaces at any bit depth, through their decode arrays (inline pictures too,
//!   and JPEG pictures, which are written back Flate-compressed), written as 8-bit CMYK: PDF/X-1a
//!   is PDF 1.3 or 1.4, which have no 16-bit pictures;
//! - the colour tables of indexed spaces over them (their indices don't change);
//! - the alternates of Separation and DeviceN spaces: the tint transform becomes a sampled
//!   function into DeviceCMYK (the tints, and so the spot colour's own plates, don't change);
//! - shadings: a function-based, axial or radial shading's colour function, and a mesh shading's
//!   function, become sampled functions into DeviceCMYK; a mesh shading without one has each
//!   corner's colour converted, its triangles and patches split where colour blended in CMYK
//!   between the corners would stray from colour blended in the original space and then
//!   converted ([`mesh`]).
//!
//! DeviceRGB is taken to be sRGB, or the `DefaultRGB` space where content's resources set one.
//! CalGray, CalRGB and Lab convert from profiles made from their parameters (`icc.rs`), with their
//! white adapted to D50; a `BlackPoint` isn't compensated, as relative colorimetry doesn't. Colour
//! is mapped with the relative colorimetric intent, the PDF default (ISO 32000-2 8.6.5.8), or
//! perceptual when the output profile has no colorimetric table; a content stream's `ri` operator
//! and a picture's `/Intent` are not followed. Gray, CMYK and spot colour stay as they are.
//!
//! A sampled function is kept only when it is within [`TOLERANCE`] of the function it replaces,
//! composed with the conversion, at every point between its samples (and a split mesh only when
//! every piece is): what can't be resampled that closely refuses the conversion.
//!
//! What doesn't convert, and so refuses the conversion: uncoloured patterns over RGB or
//! device-independent colour, pictures with a colour-key mask, JPEG pictures in CMYK, functions
//! that can't be evaluated (a cubic sampled function, a calculator program that fails), mesh data
//! whose records don't end on byte boundaries, and content, parameters or profiles that can't be
//! read.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use moxcms::{BarycentricWeightScale, ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformF32Executor, TransformOptions};
use pdfcraft_content::{Op, Pieces, write_op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::conform::Blocker;
use crate::function::{self, Function, Functions};
use crate::walk::{self, Content};

mod mesh;

/// Where colour converts from: sRGB, the ICC profile in a stream with `n` components, or one of
/// the CIE-based spaces in [`Recolour::cals`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Source {
    Srgb,
    Icc { profile: ObjRef, n: u8 },
    Cal { index: u16, n: u8 },
}

impl Source {
    fn components(self) -> usize {
        match self {
            Source::Srgb => 3,
            Source::Icc { n, .. } | Source::Cal { n, .. } => usize::from(n),
        }
    }
}

/// A CIE-based space PDF defines by its parameters (ISO 32000-2 8.6.5.2–8.6.5.4).
#[derive(Clone, Debug, PartialEq)]
enum Calibrated {
    Gray {
        gamma: f64,
    },
    Rgb {
        white: [f64; 3],
        gamma: [f64; 3],
        matrix: [f64; 9],
    },
    /// L*a*b* under `white`, a* and b* within `range`; `adapt` takes XYZ under `white` to D50.
    Lab {
        white: [f64; 3],
        range: [f64; 4],
        adapt: [[f64; 3]; 3],
    },
}

/// What a colour space does when colour converts to CMYK.
#[derive(Clone, Debug, PartialEq)]
enum Space {
    /// Stays as it is: gray, CMYK, spot colour over those, coloured patterns.
    Keep,
    /// Converts.
    Convert(Source),
    /// An indexed space over a space that converts: its colour table converts, its indices don't.
    Indexed(Source),
    /// A Separation or DeviceN space whose alternate converts: its tints stay, and its tint
    /// transform is resampled into CMYK.
    Spot,
    /// Can't convert.
    Blocked(&'static str, Option<String>),
}

type Transform = Arc<TransformF32Executor>;

/// How deep a colour space is followed (an indexed space over a named one…).
const MAX_SPACE_DEPTH: u8 = 6;
/// Pixels converted at a time.
const CHUNK: usize = 16 * 1024;
/// The largest picture converted, in samples of the converted result.
const MAX_SAMPLES: usize = 512 * 1024 * 1024;
/// The furthest a resampled function, or a mesh blended in CMYK, may stray from the colour it
/// replaces, on any ink: one step of an 8-bit picture.
pub(crate) const TOLERANCE: f64 = 1.0 / 255.0;
/// The most points a resampled function holds.
const MAX_WRITTEN: usize = 1 << 17;
/// The most points a resampled function is checked at.
const MAX_CHECKED: usize = 1 << 21;
/// The most inputs a resampled function can have (DeviceN colorants, a function-based shading's
/// two coordinates).
const MAX_SAMPLED_INPUTS: usize = 8;

/// What the conversion did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Stats {
    pub colours: usize,
    pub pictures: usize,
    pub tables: usize,
    pub spots: usize,
    pub shadings: usize,
    pub spaces_removed: usize,
    pub group_spaces: usize,
}

pub(crate) struct Recolour {
    output: Option<ColorProfile>,
    srgb: Option<ColorProfile>,
    cals: Vec<Calibrated>,
    chains: HashMap<Source, Option<Vec<Transform>>>,
    /// Spot colour spaces already rewritten, by what they say: the same space written into many
    /// resource lists gets one tint transform.
    spots: HashMap<String, Option<Object>>,
    pub stats: Stats,
    pub blockers: Vec<Blocker>,
}

/// A resource list that is an object of its own, converted once every object has been seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum List {
    Spaces,
    Shadings,
    Patterns,
}

/// Why a function couldn't be resampled.
#[derive(Debug, PartialEq)]
enum Resample {
    /// It can't be evaluated somewhere in its domain.
    Unreadable,
    /// No sampling within the limits comes within [`TOLERANCE`] of it.
    Inexact,
    /// The conversion itself failed, and a blocker already says why.
    Reported,
}

/// Convert every colour in `doc` that isn't gray, CMYK or spot colour to CMYK through the output
/// profile `icc`. Returns what it did and what it couldn't convert; with anything it couldn't,
/// `doc` is half converted and must be thrown away.
pub(crate) fn to_cmyk(doc: &mut Document, icc: &[u8]) -> Recolour {
    let mut r = Recolour {
        output: None,
        srgb: ColorProfile::new_from_slice(&crate::icc::srgb()).ok(),
        cals: Vec::new(),
        chains: HashMap::new(),
        spots: HashMap::new(),
        stats: Stats::default(),
        blockers: Vec::new(),
    };
    match ColorProfile::new_from_slice(icc) {
        Ok(output) => r.output = Some(output),
        Err(e) => {
            r.block("The output profile can't convert colour to CMYK ({})", Some(e.to_string()), None);
            return r;
        }
    }
    let mut rewritten = HashSet::new();
    for c in walk::contents(doc) {
        r.content(doc, &c, &mut rewritten);
    }
    r.pictures(doc);
    r.spaces(doc);
    r
}

impl Recolour {
    fn block(&mut self, pattern: &'static str, value: Option<String>, page: Option<usize>) {
        let b = Blocker::new(pattern, value, page);
        if !self.blockers.iter().any(|x| x.pattern == b.pattern && x.value == b.value) {
            self.blockers.push(b);
        }
    }

    /// The transforms from `src` to the output profile, made the first time they are asked for.
    fn chain(&mut self, doc: &Document, src: Source) -> Option<Vec<Transform>> {
        if let Some(c) = self.chains.get(&src) {
            return c.clone();
        }
        let made = self.make_chain(doc, src);
        if made.is_none() {
            self.block("A colour space's ICC profile can't be used to convert its colour", None, None);
        }
        self.chains.insert(src, made.clone());
        made
    }

    fn make_chain(&self, doc: &Document, src: Source) -> Option<Vec<Transform>> {
        let profile = match src {
            Source::Srgb => self.srgb.clone()?,
            Source::Icc { profile, n } => {
                let profile = match &*doc.get(profile) {
                    Object::Stream(s) => ColorProfile::new_from_slice(&s.decoded_strict().ok()?).ok()?,
                    _ => return None,
                };
                // Components are taken to run 0–1, which holds for gray, RGB and CMYK profiles
                // only (an ICC-based Lab space's range isn't followed).
                let fits = matches!((profile.color_space, n), (DataColorSpace::Gray, 1) | (DataColorSpace::Rgb, 3) | (DataColorSpace::Cmyk, 4));
                fits.then_some(profile)?
            }
            Source::Cal { index, .. } => {
                let bytes = match self.cals.get(usize::from(index))? {
                    Calibrated::Gray { gamma } => crate::icc::calibrated_gray(*gamma),
                    Calibrated::Rgb { white, gamma, matrix } => crate::icc::calibrated_rgb(*white, *gamma, *matrix)?,
                    Calibrated::Lab { .. } => crate::icc::lab(),
                };
                ColorProfile::new_from_slice(&bytes).ok()?
            }
        };
        let output = self.output.as_ref()?;
        match src.components() {
            3 => Some(vec![transform(&profile, Layout::Rgb, output)?]),
            4 => Some(vec![transform(&profile, Layout::Rgba, output)?]),
            // Gray goes through sRGB: the colour engine converts gray only to RGB and gray.
            1 => {
                let srgb = self.srgb.as_ref()?;
                let to_rgb = [RenderingIntent::RelativeColorimetric, RenderingIntent::Perceptual]
                    .into_iter()
                    .find_map(|intent| profile.create_transform_f32(Layout::Gray, srgb, Layout::Rgb, options(intent)).ok())?;
                Some(vec![to_rgb, transform(srgb, Layout::Rgb, output)?])
            }
            _ => None,
        }
    }

    /// Whole colours in `src`'s own units (L* 0–100 for Lab…), as the colour engine takes them
    /// (each 0–1), appended to `out`.
    fn normalize(&self, src: Source, values: &[f64], out: &mut Vec<f32>) {
        let lab = match src {
            Source::Cal { index, .. } => match self.cals.get(usize::from(index)) {
                Some(Calibrated::Lab { white, range, adapt }) => Some((white, range, adapt)),
                _ => None,
            },
            _ => None,
        };
        let Some((white, range, adapt)) = lab else {
            out.extend(values.iter().map(|v| v.clamp(0.0, 1.0) as f32));
            return;
        };
        let d50 = white.iter().zip(crate::icc::D50).all(|(a, b)| (a - b).abs() < 1e-4);
        for c in values.chunks(3) {
            let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
            let lab = [at(0).clamp(0.0, 100.0), at(1).clamp(range[0], range[1]), at(2).clamp(range[2], range[3])];
            let [l, a, b] = if d50 { lab } else { crate::icc::lab_to_d50(lab, *white, adapt) };
            // Version 4 Lab encoding: L* 0–100 and a*, b* −128–127 to 0–1.
            out.extend([l / 100.0, (a + 128.0) / 255.0, (b + 128.0) / 255.0].map(|v| if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 }));
        }
    }

    /// Each of `src`'s components' default range: what a picture's samples and an indexed
    /// space's table entries map to.
    fn ranges(&self, src: Source) -> Vec<(f64, f64)> {
        if let Source::Cal { index, .. } = src
            && let Some(Calibrated::Lab { range, .. }) = self.cals.get(usize::from(index))
        {
            return vec![(0.0, 100.0), (range[0], range[1]), (range[2], range[3])];
        }
        vec![(0.0, 1.0); src.components()]
    }

    /// Convert samples (each 0–1, `src.components()` per pixel) to CMYK (4 per pixel, 0–1).
    fn convert(&mut self, doc: &Document, src: Source, samples: &[f32]) -> Option<Vec<f32>> {
        let chain = self.chain(doc, src)?;
        let n = src.components();
        let pixels = samples.len() / n.max(1);
        let mut out = Vec::with_capacity(pixels.saturating_mul(4));
        for chunk in samples.chunks(CHUNK.saturating_mul(n)) {
            let mut data = chunk.to_vec();
            let mut channels = n;
            for t in &chain {
                let next_channels = if channels == 1 { 3 } else { 4 };
                let mut next = vec![0f32; data.len() / channels * next_channels];
                t.transform(&data, &mut next).ok()?;
                data = next;
                channels = next_channels;
            }
            out.extend(data.into_iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }));
        }
        Some(out)
    }

    /// Whole colours in `src`'s own units to CMYK (4 per colour, 0–1).
    fn convert_values(&mut self, doc: &Document, src: Source, values: &[f64]) -> Option<Vec<f32>> {
        let mut samples = Vec::with_capacity(values.len());
        self.normalize(src, values, &mut samples);
        self.convert(doc, src, &samples)
    }

    fn colour(&mut self, doc: &Document, src: Source, values: &[f64]) -> Option<[f64; 4]> {
        let c = self.convert_values(doc, src, values)?;
        self.stats.colours += 1;
        // Four decimals are finer than any press resolves.
        let round = |v: f32| (f64::from(v) * 10_000.0).round() / 10_000.0;
        Some([round(*c.first()?), round(*c.get(1)?), round(*c.get(2)?), round(*c.get(3)?)])
    }

    /// A picture's samples (`bpc` bits per component, `width` pixels a row, each row starting on
    /// a byte), mapped through `decode`, as 8-bit CMYK.
    fn pixels(&mut self, doc: &Document, src: Source, data: &[u8], width: usize, bpc: u32, decode: &[(f64, f64)]) -> Option<Vec<u8>> {
        let n = src.components();
        let row = row_bytes(width, n, bpc)?;
        if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
            return None;
        }
        if row == 0 || decode.len() != n {
            return Some(Vec::new());
        }
        let max = ((1u64 << bpc) - 1) as f64;
        let rows_per_chunk = (CHUNK / width.max(1)).max(1);
        let mut out = Vec::with_capacity((data.len() / row).saturating_mul(width).saturating_mul(4));
        for rows in data.chunks(row.saturating_mul(rows_per_chunk)) {
            let mut values = Vec::with_capacity(rows.len() / row * width * n);
            for line in rows.chunks_exact(row) {
                for i in 0..width.saturating_mul(n) {
                    let raw = f64::from(unpack(line, i, bpc)?);
                    let (d0, d1) = *decode.get(i % n)?;
                    values.push(d0 + raw * (d1 - d0) / max);
                }
            }
            out.extend(self.convert_values(doc, src, &values)?.into_iter().map(|v| (v * 255.0).round() as u8));
        }
        Some(out)
    }

    /// The source that DeviceRGB stands for in content drawn with `resources`.
    fn device_rgb(&mut self, doc: &Document, resources: Option<&Dict>) -> Source {
        let default = resources.and_then(|r| walk::dict_of(doc, r.get(b"ColorSpace")).get(b"DefaultRGB").cloned());
        match default.map(|d| doc.resolve(&d)).as_deref() {
            Some(Object::Array(a)) => match a.first().and_then(Object::as_name) {
                Some(b"ICCBased") => match a.get(1).and_then(Object::as_ref) {
                    Some(profile) if icc_components(doc, profile) == Some(3) => Source::Icc { profile, n: 3 },
                    _ => Source::Srgb,
                },
                Some(b"CalRGB") => self.calibrated(doc, b"CalRGB", a.get(1)).unwrap_or(Source::Srgb),
                _ => Source::Srgb,
            },
            _ => Source::Srgb,
        }
    }

    /// The source for a CalGray, CalRGB or Lab space with the parameters `params`: `None` when
    /// they can't be read (no white point, a white point with Y other than 1, a gamma that isn't
    /// positive, a range out of order).
    fn calibrated(&mut self, doc: &Document, family: &[u8], params: Option<&Object>) -> Option<Source> {
        let dict = doc.resolve(params?);
        let dict = dict.as_dict()?;
        let numbers = |key: &[u8], n: usize| -> Option<Option<Vec<f64>>> {
            let Some(o) = dict.get(key) else { return Some(None) };
            let values: Vec<f64> = match &*doc.resolve(o) {
                Object::Array(a) => a.iter().map(|v| doc.resolve(v).as_f64().filter(|v| v.is_finite())).collect::<Option<_>>()?,
                one => vec![one.as_f64().filter(|v| v.is_finite())?],
            };
            (values.len() == n).then_some(Some(values))
        };
        let white: [f64; 3] = numbers(b"WhitePoint", 3)??.try_into().ok()?;
        if white[0] <= 0.0 || white[2] <= 0.0 || (white[1] - 1.0).abs() > 1e-3 {
            return None;
        }
        let white = [white[0], 1.0, white[2]];
        let (cal, n) = match family {
            b"CalGray" => {
                let gamma = numbers(b"Gamma", 1)?.map_or(Some(1.0), |g| g.first().copied())?;
                (gamma > 0.0).then_some(())?;
                (Calibrated::Gray { gamma }, 1)
            }
            b"CalRGB" => {
                let gamma: [f64; 3] = numbers(b"Gamma", 3)?.unwrap_or_else(|| vec![1.0; 3]).try_into().ok()?;
                let matrix: [f64; 9] = numbers(b"Matrix", 9)?.unwrap_or_else(|| vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).try_into().ok()?;
                gamma.iter().all(|g| *g > 0.0).then_some(())?;
                // The profile must be one the colour engine reads.
                crate::icc::calibrated_rgb(white, gamma, matrix)?;
                (Calibrated::Rgb { white, gamma, matrix }, 3)
            }
            b"Lab" => {
                let range: [f64; 4] = numbers(b"Range", 4)?.unwrap_or_else(|| vec![-100.0, 100.0, -100.0, 100.0]).try_into().ok()?;
                (range[0] <= range[1] && range[2] <= range[3]).then_some(())?;
                let adapt = crate::icc::bradford(white, crate::icc::D50)?;
                (Calibrated::Lab { white, range, adapt }, 3)
            }
            _ => return None,
        };
        let index = match self.cals.iter().position(|c| *c == cal) {
            Some(i) => i,
            None => {
                self.cals.push(cal);
                self.cals.len() - 1
            }
        };
        Some(Source::Cal { index: u16::try_from(index).ok()?, n })
    }

    fn classify(&mut self, doc: &Document, cs: &Object, resources: Option<&Dict>, depth: u8) -> Space {
        if depth > MAX_SPACE_DEPTH {
            return Space::Blocked("A colour space is nested too deeply to convert", None);
        }
        let o = doc.resolve(cs);
        match &*o {
            Object::Name(n) => match n.as_slice() {
                b"DeviceRGB" | b"RGB" => Space::Convert(self.device_rgb(doc, resources)),
                b"DeviceGray" | b"G" | b"DeviceCMYK" | b"CMYK" | b"Pattern" => Space::Keep,
                other => match resources.and_then(|r| walk::dict_of(doc, r.get(b"ColorSpace")).get(other).cloned()) {
                    Some(named) => self.classify(doc, &named, resources, depth.saturating_add(1)),
                    None => Space::Keep,
                },
            },
            Object::Array(a) => {
                let Some(family) = a.first().and_then(Object::as_name) else { return Space::Keep };
                match family {
                    b"ICCBased" => {
                        let Some(profile) = a.get(1).and_then(Object::as_ref) else {
                            return Space::Blocked("A colour space's ICC profile can't be used to convert its colour", None);
                        };
                        match icc_components(doc, profile) {
                            Some(n @ (1 | 3 | 4)) => Space::Convert(Source::Icc { profile, n: n as u8 }),
                            _ => Space::Blocked("A colour space's ICC profile can't be used to convert its colour", None),
                        }
                    }
                    b"Indexed" | b"I" => match a.get(1).map(|b| self.classify(doc, b, resources, depth.saturating_add(1))) {
                        Some(Space::Convert(s)) => Space::Indexed(s),
                        // The table holds tints, which stay; the base gets a new tint transform.
                        Some(Space::Keep | Space::Spot) => Space::Keep,
                        Some(Space::Blocked(p, v)) => Space::Blocked(p, v),
                        _ => Space::Blocked("A colour space is nested too deeply to convert", None),
                    },
                    b"Separation" | b"DeviceN" => match a.get(2).map(|b| self.classify(doc, b, resources, depth.saturating_add(1))) {
                        Some(Space::Keep) => Space::Keep,
                        Some(Space::Convert(_)) => Space::Spot,
                        Some(Space::Blocked(p, v)) => Space::Blocked(p, v),
                        _ => Space::Blocked("The spot colour {} has an alternate that can't be converted to CMYK yet", Some(spot_names(doc, a))),
                    },
                    b"Pattern" => match a.get(1).map(|b| self.classify(doc, b, resources, depth.saturating_add(1))) {
                        None | Some(Space::Keep | Space::Spot) => Space::Keep,
                        _ => Space::Blocked("A pattern over RGB or device-independent colour can't be converted to CMYK yet", None),
                    },
                    b"CalRGB" | b"CalGray" | b"Lab" => match self.calibrated(doc, family, a.get(1)) {
                        Some(src) => Space::Convert(src),
                        None => Space::Blocked("A {} colour space's parameters can't be read", Some(String::from_utf8_lossy(family).into_owned())),
                    },
                    b"DeviceRGB" | b"DeviceGray" | b"DeviceCMYK" => {
                        self.classify(doc, &Object::Name(family.to_vec()), resources, depth.saturating_add(1))
                    }
                    _ => Space::Keep,
                }
            }
            _ => Space::Keep,
        }
    }

    /// Rewrite the colour operators and inline pictures of one content stream (or a page's).
    fn content(&mut self, doc: &mut Document, c: &Content, rewritten: &mut HashSet<ObjRef>) {
        let mut data = Vec::new();
        let mut originals = Vec::new();
        for r in &c.streams {
            let o = doc.get(*r);
            let Object::Stream(s) = &*o else { continue };
            match s.decoded_strict() {
                Ok(d) => {
                    data.push(d);
                    originals.push((*r, s.clone()));
                }
                Err(_) => {
                    self.block("A content stream can't be read, so its colour can't be converted", None, c.page);
                    return;
                }
            }
        }
        let pieces = Pieces::join(&data);
        let parsed = pieces.parse();
        let resources = Some(&c.resources);
        #[derive(Clone)]
        struct State {
            fill: Space,
            stroke: Space,
        }
        let mut state = State { fill: Space::Keep, stroke: Space::Keep };
        let mut saved: Vec<State> = Vec::new();
        let mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
        let replace = |edits: &mut Vec<(std::ops::Range<usize>, Vec<u8>)>, op: &Op, new: &[Op]| {
            let mut bytes = Vec::new();
            for n in new {
                write_op(n, &mut bytes);
            }
            edits.push((op.span.clone(), bytes));
        };
        for op in &parsed.ops {
            match op.op.as_slice() {
                b"q" => saved.push(state.clone()),
                b"Q" => {
                    if let Some(s) = saved.pop() {
                        state = s;
                    }
                }
                b"g" => state.fill = Space::Keep,
                b"G" => state.stroke = Space::Keep,
                b"k" => state.fill = Space::Keep,
                b"K" => state.stroke = Space::Keep,
                b"rg" | b"RG" => {
                    let fill = op.op == b"rg";
                    let src = self.device_rgb(doc, resources);
                    let space = Space::Convert(src);
                    if fill {
                        state.fill = space;
                    } else {
                        state.stroke = space;
                    }
                    match op.nums::<3>().and_then(|v| self.colour(doc, src, &v)) {
                        Some(cmyk) => replace(
                            &mut edits,
                            op,
                            &[Op::new(if fill { "k" } else { "K" }, cmyk.iter().map(|v| pdfcraft_content::num(*v)).collect())],
                        ),
                        None => self.block("A colour operator's operands don't match its colour space", None, c.page),
                    }
                }
                b"cs" | b"CS" => {
                    let fill = op.op == b"cs";
                    let space = match op.operands.first() {
                        // Abbreviations (G, RGB, CMYK, I) stand for families in inline pictures
                        // only: here they are resource names like any other.
                        Some(Object::Name(n)) if matches!(n.as_slice(), b"G" | b"RGB" | b"CMYK" | b"I") => {
                            match resources.and_then(|r| walk::dict_of(doc, r.get(b"ColorSpace")).get(n).cloned()) {
                                Some(named) => self.classify(doc, &named, resources, 1),
                                None => Space::Keep,
                            }
                        }
                        Some(name @ Object::Name(_)) => self.classify(doc, name, resources, 0),
                        _ => Space::Keep,
                    };
                    match &space {
                        Space::Convert(src) => {
                            // Setting a space sets its initial colour: each component at its
                            // minimum, or 0 where the range holds it (L*a*b* 0 0 0).
                            let initial = if src.components() == 4 { vec![0.0, 0.0, 0.0, 1.0] } else { vec![0.0; src.components()] };
                            match self.colour(doc, *src, &initial) {
                                Some(cmyk) => {
                                    let (set, colour) = if fill { ("cs", "sc") } else { ("CS", "SC") };
                                    replace(
                                        &mut edits,
                                        op,
                                        &[
                                            Op::new(set, vec![Object::name("DeviceCMYK")]),
                                            Op::new(colour, cmyk.iter().map(|v| pdfcraft_content::num(*v)).collect()),
                                        ],
                                    );
                                }
                                None => self.block("A colour operator's operands don't match its colour space", None, c.page),
                            }
                        }
                        Space::Blocked(p, v) => self.block(p, v.clone(), c.page),
                        Space::Keep | Space::Indexed(_) | Space::Spot => {}
                    }
                    if fill {
                        state.fill = space;
                    } else {
                        state.stroke = space;
                    }
                }
                b"sc" | b"scn" | b"SC" | b"SCN" => {
                    let fill = matches!(op.op.as_slice(), b"sc" | b"scn");
                    let space = if fill { &state.fill } else { &state.stroke };
                    if let Space::Convert(src) = space.clone() {
                        let values: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
                        let named = op.operands.iter().any(|o| matches!(o, Object::Name(_)));
                        match (values.len() == src.components() && !named).then(|| self.colour(doc, src, &values)).flatten() {
                            Some(cmyk) => replace(
                                &mut edits,
                                op,
                                &[Op::new(if fill { "sc" } else { "SC" }, cmyk.iter().map(|v| pdfcraft_content::num(*v)).collect())],
                            ),
                            None => self.block("A colour operator's operands don't match its colour space", None, c.page),
                        }
                    }
                }
                b"BI" => {
                    if let Some((dict, bytes)) = &op.inline
                        && let Some(new) = self.inline_picture(doc, dict, bytes, resources, c.page)
                    {
                        replace(&mut edits, op, &[new]);
                    }
                }
                _ => {}
            }
        }
        if edits.is_empty() {
            return;
        }
        if c.streams.iter().any(|r| rewritten.contains(r)) {
            self.block("A content stream is drawn with different resources in different places, so its colour can't be converted", None, c.page);
            return;
        }
        let pieces_out = pieces.splice(edits);
        for ((r, original), (new, old)) in originals.into_iter().zip(pieces_out.into_iter().zip(data.iter())) {
            if &new != old {
                let mut dict = original.dict.clone();
                dict.remove(b"Length");
                doc.set(r, Object::Stream(Stream::flate(dict, &new)));
            }
            rewritten.insert(r);
        }
    }

    /// An inline picture converted, or `None` when it needs nothing (or can't be converted: then a
    /// blocker says why).
    fn inline_picture(&mut self, doc: &Document, dict: &Dict, data: &[u8], resources: Option<&Dict>, page: Option<usize>) -> Option<Op> {
        let get = |short: &[u8], long: &[u8]| dict.get(short).or_else(|| dict.get(long)).cloned();
        if matches!(get(b"IM", b"ImageMask"), Some(Object::Bool(true))) {
            return None;
        }
        let cs = get(b"CS", b"ColorSpace")?;
        let space = self.classify(doc, &cs, resources, 0);
        let mut new = dict.clone();
        match space {
            Space::Keep | Space::Spot => return None,
            Space::Blocked(p, v) => {
                self.block(p, v, page);
                return None;
            }
            Space::Indexed(src) => {
                // An inline indexed space is written into the picture; a named one is a resource,
                // converted with the others.
                let Object::Array(a) = (*doc.resolve(&cs)).clone() else { return None };
                if !matches!(cs, Object::Array(_)) {
                    return None;
                }
                let converted = self.indexed(doc, &a, src)?;
                for key in [b"CS".as_slice(), b"ColorSpace"] {
                    new.remove(key);
                }
                new.set(b"CS".to_vec(), converted);
            }
            Space::Convert(src) => {
                let decode = get(b"D", b"Decode").map(|d| decode_array(doc, &d));
                let bpc = get(b"BPC", b"BitsPerComponent").and_then(|b| b.as_int());
                let mut stream_dict = Dict::new();
                if let Some(f) = get(b"F", b"Filter") {
                    stream_dict.set(b"Filter".to_vec(), f);
                }
                if let Some(p) = get(b"DP", b"DecodeParms") {
                    stream_dict.set(b"DecodeParms".to_vec(), p);
                }
                let (w, h) = (get(b"W", b"Width").and_then(|v| v.as_int()), get(b"H", b"Height").and_then(|v| v.as_int()));
                let cmyk = self.picture(doc, &Stream::from_raw(stream_dict, data.to_vec()), src, (w, h, bpc), decode, page)?;
                for key in [b"CS".as_slice(), b"ColorSpace", b"F", b"Filter", b"DP", b"DecodeParms", b"BPC", b"BitsPerComponent", b"D", b"Decode"] {
                    new.remove(key);
                }
                new.set(b"CS".to_vec(), Object::name("DeviceCMYK"));
                new.set(b"BPC".to_vec(), Object::Int(8));
                // Hex after Flate: the data can't hold an `EI` a reader would stop at.
                new.set(b"F".to_vec(), Object::Array(vec![Object::name("AHx"), Object::name("Fl")]));
                let mut hex = hex(&pdfcraft_filters::encode_flate(&cmyk));
                hex.push(b'>');
                self.stats.pictures += 1;
                return Some(Op { op: b"BI".to_vec(), operands: Vec::new(), inline: Some((new, hex)), span: 0..0 });
            }
        }
        Some(Op { op: b"BI".to_vec(), operands: Vec::new(), inline: Some((new, data.to_vec())), span: 0..0 })
    }

    /// A picture converted to 8-bit CMYK: its samples (`w` × `h` pixels of `bpc` bits) decoded,
    /// mapped through `decode` (or the space's ranges), and converted. `None`, with a blocker,
    /// when they can't be read or don't fill the picture.
    fn picture(
        &mut self,
        doc: &Document,
        s: &Stream,
        src: Source,
        (width, height, bpc): (Option<i64>, Option<i64>, Option<i64>),
        decode: Option<Option<Vec<(f64, f64)>>>,
        page: Option<usize>,
    ) -> Option<Vec<u8>> {
        let n = src.components();
        let bpc = bpc.and_then(|b| u32::try_from(b).ok()).filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16));
        let size = width
            .zip(height)
            .and_then(|(w, h)| usize::try_from(w).ok().zip(usize::try_from(h).ok()))
            .filter(|(w, h)| w.checked_mul(*h).and_then(|px| px.checked_mul(4)).is_some_and(|s| s <= MAX_SAMPLES));
        let decode = match decode {
            None => Some(self.ranges(src)),
            Some(d) => d.filter(|d| d.len() == n),
        };
        let (Some(bpc), Some((width, height)), Some(decode)) = (bpc, size, decode) else {
            self.block("A picture's data can't be read, so it can't be converted to CMYK", None, page);
            return None;
        };
        let Some(row) = row_bytes(width, n, bpc) else {
            self.block("A picture's data can't be read, so it can't be converted to CMYK", None, page);
            return None;
        };
        let expected = row.saturating_mul(height);
        let chain = s.filters();
        let decoded = if chain.last().is_some_and(|(f, _)| *f == pdfcraft_filters::Filter::Dct) {
            if n == 4 {
                self.block("A JPEG picture in CMYK can't be converted yet", None, page);
                return None;
            }
            let mut before = s.clone();
            let names: Vec<Object> = match s.dict.get(b"Filter") {
                Some(Object::Array(a)) => a.iter().take(a.len().saturating_sub(1)).cloned().collect(),
                _ => Vec::new(),
            };
            let parms: Option<Object> = match s.dict.get(b"DecodeParms") {
                Some(Object::Array(a)) => Some(Object::Array(a.iter().take(a.len().saturating_sub(1)).cloned().collect())),
                _ => None,
            };
            before.dict.set(b"Filter".to_vec(), Object::Array(names));
            match parms {
                Some(p) => before.dict.set(b"DecodeParms".to_vec(), p),
                None => {
                    before.dict.remove(b"DecodeParms");
                }
            }
            // A JPEG's samples are 8 bits.
            (bpc == 8).then(|| before.decoded_strict().ok().and_then(|jpeg| jpeg_samples(&jpeg, n))).flatten()
        } else {
            s.decoded_strict().ok()
        };
        let samples = match decoded {
            Some(d) if d.len() >= expected && expected > 0 => d.get(..expected).map(<[u8]>::to_vec),
            _ => None,
        };
        let Some(samples) = samples else {
            self.block("A picture's data can't be read, so it can't be converted to CMYK", None, page);
            return None;
        };
        self.pixels(doc, src, &samples, width, bpc, &decode)
    }

    /// An indexed space over CMYK with `a`'s colour table converted from `src`.
    fn indexed(&mut self, doc: &Document, a: &[Object], src: Source) -> Option<Object> {
        let hival = usize::try_from(a.get(2).and_then(|h| doc.resolve(h).as_int())?).ok()?.min(255);
        let table = match &*doc.resolve(a.get(3)?) {
            Object::String(s) => s.bytes.clone(),
            Object::Stream(s) => s.decoded_strict().ok()?,
            _ => return None,
        };
        let n = src.components();
        let want = hival.checked_add(1)?.checked_mul(n)?;
        let Some(entries) = table.get(..want) else {
            self.block("An indexed colour space's table is too short to convert", None, None);
            return None;
        };
        // The entries are bytes over each component's range, like an 8-bit picture's samples.
        let ranges = self.ranges(src);
        let cmyk = self.pixels(doc, src, entries, hival.checked_add(1)?, 8, &ranges)?;
        self.stats.tables += 1;
        Some(Object::Array(vec![
            Object::name("Indexed"),
            Object::name("DeviceCMYK"),
            Object::Int(i64::try_from(hival).ok()?),
            Object::String(pdfcraft_cos::PdfString { bytes: cmyk, hex: true }),
        ]))
    }

    /// Every picture XObject in a space that converts.
    fn pictures(&mut self, doc: &mut Document) {
        for n in doc.object_numbers() {
            let r = ObjRef::new(n, doc.generation(n));
            let o = doc.get(r);
            let Object::Stream(s) = &*o else { continue };
            if s.dict.name(b"Subtype") != Some(b"Image") || matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true))) {
                continue;
            }
            let Some(cs) = s.dict.get(b"ColorSpace") else { continue };
            match self.classify(doc, cs, None, 0) {
                Space::Keep | Space::Indexed(_) | Space::Spot => {}
                Space::Blocked(p, v) => self.block(p, v, None),
                Space::Convert(src) => {
                    if matches!(s.dict.get(b"Mask").map(|m| doc.resolve(m)).as_deref(), Some(Object::Array(_))) {
                        self.block("A picture with a colour-key mask can't be converted to CMYK yet", None, None);
                        continue;
                    }
                    let decode = s.dict.get(b"Decode").map(|d| decode_array(doc, d));
                    let size = (s.dict.int(b"Width"), s.dict.int(b"Height"), s.dict.int(b"BitsPerComponent"));
                    let Some(cmyk) = self.picture(doc, s, src, size, decode, None) else { continue };
                    let mut dict = s.dict.clone();
                    for key in [b"Length".as_slice(), b"DecodeParms", b"Filter", b"Decode"] {
                        dict.remove(key);
                    }
                    dict.set(b"ColorSpace".to_vec(), Object::name("DeviceCMYK"));
                    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                    doc.set(r, Object::Stream(Stream::flate(dict, &cmyk)));
                    self.stats.pictures += 1;
                }
            }
        }
    }

    /// A colour space array written out again with what converts converted: an indexed space's
    /// table, a spot colour's tint transform, and the spaces under indexed, pattern and DeviceN
    /// spaces written into them. `None` when nothing changes, or for a reference (what it refers
    /// to is rewritten where it is).
    fn respace(&mut self, doc: &mut Document, cs: &Object, depth: u8) -> Option<Object> {
        let Object::Array(a) = cs else { return None };
        if depth > MAX_SPACE_DEPTH {
            return None;
        }
        let family = a.first().and_then(Object::as_name)?;
        match family {
            b"Indexed" | b"I" => match self.classify(doc, a.get(1)?, None, depth.saturating_add(1)) {
                Space::Convert(src) => self.indexed(doc, a, src),
                Space::Keep | Space::Spot => {
                    let base = self.respace(doc, a.get(1)?, depth.saturating_add(1))?;
                    let mut new = a.clone();
                    *new.get_mut(1)? = base;
                    Some(Object::Array(new))
                }
                Space::Indexed(_) | Space::Blocked(..) => None,
            },
            b"Pattern" => {
                let base = self.respace(doc, a.get(1)?, depth.saturating_add(1))?;
                let mut new = a.clone();
                *new.get_mut(1)? = base;
                Some(Object::Array(new))
            }
            b"Separation" | b"DeviceN" => {
                let key = format!("{a:?}");
                if let Some(done) = self.spots.get(&key) {
                    return done.clone();
                }
                let done = self.spot(doc, a, depth);
                self.spots.insert(key, done.clone());
                done
            }
            _ => None,
        }
    }

    /// A Separation or DeviceN space `a` with its alternate converted: a sampled tint transform
    /// into DeviceCMYK, and the Separation spaces of a DeviceN space's colorants rewritten too.
    fn spot(&mut self, doc: &mut Document, a: &[Object], depth: u8) -> Option<Object> {
        let mut new = a.to_vec();
        let mut changed = false;
        if let Space::Convert(src) = self.classify(doc, a.get(2)?, None, depth.saturating_add(1)) {
            let names = spot_names(doc, a);
            let colorants = match &*doc.resolve(a.get(1)?) {
                Object::Array(list) => list.len(),
                _ => 1,
            };
            let tint = Functions::read(doc, a.get(3)?).filter(|f| f.inputs() == colorants && f.outputs() == Some(src.components()));
            let Some(tint) = tint else {
                self.block("The tint transform of the spot colour {} can't be read or evaluated", Some(names), None);
                return None;
            };
            match self.resample(doc, src, &tint.domain(), &|x| tint.eval(x), &tint) {
                Ok(f) => {
                    *new.get_mut(2)? = Object::name("DeviceCMYK");
                    *new.get_mut(3)? = f;
                    self.stats.spots += 1;
                    changed = true;
                }
                Err(Resample::Unreadable) => {
                    self.block("The tint transform of the spot colour {} can't be read or evaluated", Some(names), None);
                    return None;
                }
                Err(Resample::Inexact) => {
                    self.block("The tint transform of the spot colour {} can't be resampled into CMYK exactly", Some(names), None);
                    return None;
                }
                Err(Resample::Reported) => return None,
            }
        }
        // A DeviceN space's attributes: the Separation spaces its colorants name.
        if let Some(Object::Dict(attributes)) = a.get(4)
            && let Some(Object::Dict(colorants)) = attributes.get(b"Colorants")
        {
            let mut colorants = colorants.clone();
            let mut any = false;
            let entries: Vec<(Vec<u8>, Object)> = colorants.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (name, cs) in entries {
                if let Some(rewritten) = self.respace(doc, &cs, depth.saturating_add(1)) {
                    colorants.set(name, rewritten);
                    any = true;
                }
            }
            if any {
                let mut attributes = attributes.clone();
                attributes.set(b"Colorants".to_vec(), Object::Dict(colorants));
                *new.get_mut(4)? = Object::Dict(attributes);
                changed = true;
            }
        }
        changed.then_some(Object::Array(new))
    }

    /// `f`, which gives colour in `src` over `domain`, as a function into DeviceCMYK: sampled
    /// (type 0), or for one input, sampled between the points where `functions` may jump and
    /// stitched (type 3), so a gradient's hard stops stay hard. The sampled functions are added to
    /// `doc`; what's returned names them.
    fn resample(
        &mut self,
        doc: &mut Document,
        src: Source,
        domain: &[(f64, f64)],
        f: &dyn Fn(&[f64]) -> Option<Vec<f64>>,
        functions: &Functions,
    ) -> Result<Object, Resample> {
        let &[(lo, hi)] = domain else {
            let stream = self.sample(doc, src, domain, domain, false, f)?;
            return Ok(Object::Ref(doc.add(Object::Stream(stream))));
        };
        let breaks = functions.breaks(lo, hi);
        let cuts: Vec<f64> = std::iter::once(lo).chain(breaks.iter().copied()).chain(std::iter::once(hi)).collect();
        let pieces = cuts.len().saturating_sub(1);
        let mut streams = Vec::with_capacity(pieces);
        for (i, w) in cuts.windows(2).enumerate() {
            let &[a, b] = w else { continue };
            // Every piece but the last ends where the next begins: sample it up to there.
            let last = i + 1 == pieces;
            let written = if pieces == 1 { (lo, hi) } else { (0.0, 1.0) };
            streams.push(self.sample(doc, src, &[(a, b)], &[written], !last, f)?);
        }
        let mut refs: Vec<Object> = streams.into_iter().map(|s| Object::Ref(doc.add(Object::Stream(s)))).collect();
        if refs.len() == 1 {
            return refs.pop().ok_or(Resample::Unreadable);
        }
        Ok(function::stitching((lo, hi), &breaks, refs))
    }

    /// `f` composed with the conversion, sampled over `region` and written as a function over
    /// `written`: on grids of 2ᵏ + 1 points along each input, growing until the function written is
    /// within [`TOLERANCE`] of the composition at every point of the next grid. `open_end`: sample
    /// a one-input region's end from below, where the function jumps.
    fn sample(
        &mut self,
        doc: &Document,
        src: Source,
        region: &[(f64, f64)],
        written: &[(f64, f64)],
        open_end: bool,
        f: &dyn Fn(&[f64]) -> Option<Vec<f64>>,
    ) -> Result<Stream, Resample> {
        let k = region.len();
        if k == 0 || k > MAX_SAMPLED_INPUTS || written.len() != k {
            return Err(Resample::Inexact);
        }
        let mut grid: usize = if k == 1 { 9 } else { 3 };
        loop {
            let fine = grid.saturating_mul(2).saturating_sub(1);
            let points = grid.checked_pow(k as u32).filter(|p| *p <= MAX_WRITTEN).ok_or(Resample::Inexact)?;
            let checked = fine.checked_pow(k as u32).filter(|p| *p <= MAX_CHECKED).ok_or(Resample::Inexact)?;
            // The grid's colour, the first input varying fastest.
            let coarse: Vec<f64> =
                (0..points).map(|i| digits(i, grid, k)).collect::<Vec<_>>().chunks(CHUNK).try_fold(Vec::new(), |mut acc, chunk| {
                    let xs: Vec<Vec<f64>> = chunk.iter().map(|d| at(region, d, grid, open_end)).collect();
                    acc.extend(self.exact(doc, src, &xs, f)?.into_iter().map(f64::from));
                    Ok::<_, Resample>(acc)
                })?;
            let stream = function::sampled_stream(written, &vec![grid; k], &coarse);
            let candidate = Function::read(doc, &Object::Stream(stream.clone())).ok_or(Resample::Inexact)?;
            let mut worst = 0.0f64;
            let mut start = 0;
            while start < checked {
                let end = start.saturating_add(CHUNK).min(checked);
                let ds: Vec<Vec<usize>> = (start..end).map(|i| digits(i, fine, k)).collect();
                let xs: Vec<Vec<f64>> = ds.iter().map(|d| at(region, d, fine, open_end)).collect();
                let exact = self.exact(doc, src, &xs, f)?;
                for (d, want) in ds.iter().zip(exact.as_chunks::<4>().0) {
                    let p = at(written, d, fine, false);
                    let got = candidate.eval(&p).ok_or(Resample::Inexact)?;
                    for (g, w) in got.iter().zip(want) {
                        worst = worst.max((g - f64::from(*w)).abs());
                    }
                }
                start = end;
            }
            if worst <= TOLERANCE {
                return Ok(stream);
            }
            grid = fine;
        }
    }

    /// The CMYK of `f` at each of `xs` (4 values a point).
    fn exact(&mut self, doc: &Document, src: Source, xs: &[Vec<f64>], f: &dyn Fn(&[f64]) -> Option<Vec<f64>>) -> Result<Vec<f32>, Resample> {
        let n = src.components();
        let mut values = Vec::with_capacity(xs.len().saturating_mul(n));
        for x in xs {
            let v = f(x).filter(|v| v.len() == n).ok_or(Resample::Unreadable)?;
            values.extend(v);
        }
        self.convert_values(doc, src, &values).ok_or(Resample::Reported)
    }

    /// A shading converted to CMYK, or `None` when it needs nothing (or can't be converted: then a
    /// blocker says why).
    fn shading(&mut self, doc: &mut Document, shading: &Object) -> Option<Object> {
        let dict = shading.as_dict()?;
        let cs = dict.get(b"ColorSpace")?.clone();
        let src = match self.classify(doc, &cs, None, 0) {
            Space::Keep | Space::Spot | Space::Indexed(_) => {
                // Only the space changes (a table, a tint transform), if it is written in here.
                let new = self.respace(doc, &cs, 0)?;
                let mut d = dict.clone();
                d.set(b"ColorSpace".to_vec(), new);
                return Some(match shading {
                    Object::Stream(s) => Object::Stream(Stream { dict: d, ..s.clone() }),
                    _ => Object::Dict(d),
                });
            }
            Space::Blocked(p, v) => {
                self.block(p, v, None);
                return None;
            }
            Space::Convert(src) => src,
        };
        let kind = doc.resolve(dict.get(b"ShadingType")?).as_int()?;
        let mut d = dict.clone();
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceCMYK"));
        if let Some(background) = dict.get(b"Background") {
            let values: Option<Vec<f64>> = match &*doc.resolve(background) {
                Object::Array(a) => a.iter().map(|v| doc.resolve(v).as_f64()).collect(),
                _ => None,
            };
            let cmyk = values.filter(|v| v.len() == src.components()).and_then(|v| self.colour(doc, src, &v));
            let Some(cmyk) = cmyk else {
                self.block("A shading's data can't be read, so it can't be converted to CMYK", None, None);
                return None;
            };
            d.set(b"Background".to_vec(), Object::Array(cmyk.iter().map(|v| pdfcraft_content::num(*v)).collect()));
        }
        let numbers = |key: &[u8]| -> Option<Vec<f64>> {
            match &*doc.resolve(dict.get(key)?) {
                Object::Array(a) => a.iter().map(|v| doc.resolve(v).as_f64().filter(|v| v.is_finite())).collect(),
                _ => None,
            }
        };
        let mesh = matches!(kind, 4..=7);
        if let Some(function) = dict.get(b"Function").cloned() {
            // The function's input: the shading's domain, or a mesh's parametric value.
            let domain: Option<Vec<(f64, f64)>> = match kind {
                1 => Some(numbers(b"Domain").map_or(vec![(0.0, 1.0), (0.0, 1.0)], |v| v.as_chunks::<2>().0.iter().map(|&[a, b]| (a, b)).collect())),
                2 | 3 => Some(numbers(b"Domain").map_or(vec![(0.0, 1.0)], |v| v.as_chunks::<2>().0.iter().map(|&[a, b]| (a, b)).collect())),
                _ if mesh => numbers(b"Decode").and_then(|v| v.get(4..6).map(|t| vec![(t[0].min(t[1]), t[0].max(t[1]))])),
                _ => None,
            };
            let functions = Functions::read(doc, &function);
            let usable = |fs: &Functions, domain: &[(f64, f64)]| {
                fs.inputs() == domain.len() && fs.outputs() == Some(src.components()) && domain.iter().all(|(a, b)| a <= b)
            };
            let (Some(domain), Some(functions)) = (domain, functions) else {
                self.block("A shading's colour function can't be read or evaluated", None, None);
                return None;
            };
            if !usable(&functions, &domain) {
                self.block("A shading's colour function can't be read or evaluated", None, None);
                return None;
            }
            match self.resample(doc, src, &domain, &|x| functions.eval(x), &functions) {
                Ok(f) => {
                    d.set(b"Function".to_vec(), f);
                }
                Err(Resample::Unreadable) => {
                    self.block("A shading's colour function can't be read or evaluated", None, None);
                    return None;
                }
                Err(Resample::Inexact) => {
                    self.block("A shading's colour function can't be resampled into CMYK exactly", None, None);
                    return None;
                }
                Err(Resample::Reported) => return None,
            }
            self.stats.shadings += 1;
            return Some(match shading {
                Object::Stream(s) => Object::Stream(Stream { dict: d, ..s.clone() }),
                _ => Object::Dict(d),
            });
        }
        let Object::Stream(s) = shading else {
            self.block("A shading's data can't be read, so it can't be converted to CMYK", None, None);
            return None;
        };
        if !mesh {
            self.block("A shading's data can't be read, so it can't be converted to CMYK", None, None);
            return None;
        }
        let data = s.decoded_strict().ok();
        let converted = data.and_then(|data| mesh::Mesh::read(doc, kind, dict, &data, src.components())).map(|m| {
            let mut convert = |values: &[f64]| self.convert_values(doc, src, values);
            m.into_cmyk(&mut convert)
        });
        match converted {
            Some(Ok((entries, data))) => {
                for (k, v) in entries {
                    match v {
                        Some(v) => d.set(k.to_vec(), v),
                        None => {
                            d.remove(k);
                        }
                    }
                }
                d.remove(b"Length");
                self.stats.shadings += 1;
                Some(Object::Stream(Stream::flate(d, &data)))
            }
            Some(Err(mesh::Failed::Inexact)) => {
                self.block("A mesh shading's colours change too sharply between its corners to convert to CMYK exactly", None, None);
                None
            }
            Some(Err(mesh::Failed::Reported)) => None,
            Some(Err(mesh::Failed::Unreadable)) | None => {
                self.block("A shading's data can't be read, so it can't be converted to CMYK", None, None);
                None
            }
        }
    }

    /// The shadings written into a resource dictionary's `/Shading` list, converted in place.
    fn shading_list(&mut self, doc: &mut Document, list: &mut Dict) -> bool {
        let mut changed = false;
        let entries: Vec<(Vec<u8>, Object)> = list.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, s) in entries {
            if matches!(s, Object::Dict(_))
                && let Some(new) = self.shading(doc, &s)
            {
                list.set(name, new);
                changed = true;
            }
        }
        changed
    }

    /// `d`'s `/Shading` entry: a pattern's shading written into it, or a resource dictionary's
    /// list (written in, or noted in `later` when it is an object of its own).
    fn shadings_in(&mut self, doc: &mut Document, d: &mut Dict, later: &mut Vec<(List, ObjRef)>) -> bool {
        match d.get(b"Shading").cloned() {
            Some(shading @ Object::Dict(_)) if shading.as_dict().is_some_and(|s| s.get(b"ShadingType").is_some()) => {
                match self.shading(doc, &shading) {
                    Some(new) => {
                        d.set(b"Shading".to_vec(), new);
                        true
                    }
                    None => false,
                }
            }
            Some(Object::Dict(mut list)) => {
                if self.shading_list(doc, &mut list) {
                    d.set(b"Shading".to_vec(), Object::Dict(list));
                    return true;
                }
                false
            }
            Some(Object::Ref(r)) if doc.get(r).as_dict().is_some_and(|t| t.get(b"ShadingType").is_none()) => {
                later.push((List::Shadings, r));
                false
            }
            _ => false,
        }
    }

    /// The shading patterns written into a resource dictionary's `/Pattern` list, their shadings
    /// converted in place (tiling patterns are streams, objects of their own).
    fn pattern_list(&mut self, doc: &mut Document, list: &mut Dict) -> bool {
        let mut changed = false;
        let entries: Vec<(Vec<u8>, Object)> = list.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, p) in entries {
            if let Object::Dict(mut pattern) = p {
                // A shading written into the pattern converts here; one of its own, where it is.
                if self.shadings_in(doc, &mut pattern, &mut Vec::new()) {
                    list.set(name, Object::Dict(pattern));
                    changed = true;
                }
            }
        }
        changed
    }

    /// A resource dictionary `d`'s lists: colour spaces, shadings and patterns, converted in place
    /// when they are written into it, else noted in `later`.
    fn resource_lists(&mut self, doc: &mut Document, d: &mut Dict, later: &mut Vec<(List, ObjRef)>) -> bool {
        let mut changed = self.list_in(doc, d, later);
        match d.get(b"Pattern").cloned() {
            Some(Object::Dict(mut list)) => {
                if self.pattern_list(doc, &mut list) {
                    d.set(b"Pattern".to_vec(), Object::Dict(list));
                    changed = true;
                }
            }
            Some(Object::Ref(r)) if matches!(&*doc.get(r), Object::Dict(t) if t.get(b"PatternType").is_none()) => {
                later.push((List::Patterns, r));
            }
            _ => {}
        }
        changed
    }

    /// Colour spaces and shadings outside content: indexed tables and spot colours' tint
    /// transforms converted, shadings converted, the spaces content no longer names taken out of
    /// resource lists, and the RGB blending spaces of transparency groups dropped.
    fn spaces(&mut self, doc: &mut Document) {
        let mut later: Vec<(List, ObjRef)> = Vec::new();
        for n in doc.object_numbers() {
            let r = ObjRef::new(n, doc.generation(n));
            let o = doc.get(r);
            match &*o {
                Object::Array(_) => {
                    if let Some(new) = self.respace(doc, &o, 0) {
                        doc.set(r, new);
                    }
                }
                Object::Dict(_) | Object::Stream(_) => {
                    let Some(dict) = o.as_dict() else { continue };
                    // A shading of its own.
                    if dict.get(b"ShadingType").is_some() {
                        if let Some(new) = self.shading(doc, &o) {
                            doc.set(r, new);
                        }
                        continue;
                    }
                    let mut d = dict.clone();
                    let mut changed = false;
                    // A pattern's shading, or a resource dictionary's list of shadings.
                    changed |= self.shadings_in(doc, &mut d, &mut later);
                    // An image's own space (an indexed or spot space written into it).
                    if let Some(cs @ Object::Array(_)) = d.get(b"ColorSpace").cloned()
                        && let Some(new) = self.respace(doc, &cs, 0)
                    {
                        d.set(b"ColorSpace".to_vec(), new);
                        changed = true;
                    }
                    // A resource dictionary's lists, and an inline /Resources' lists.
                    changed |= self.resource_lists(doc, &mut d, &mut later);
                    if let Some(Object::Dict(mut res)) = d.get(b"Resources").cloned() {
                        let listed = self.resource_lists(doc, &mut res, &mut later);
                        if self.shadings_in(doc, &mut res, &mut later) | listed {
                            d.set(b"Resources".to_vec(), Object::Dict(res));
                            changed = true;
                        }
                    }
                    // A transparency group's blending space in RGB.
                    if let Some(Object::Dict(mut group)) = d.get(b"Group").cloned()
                        && let Some(cs) = group.get(b"CS").cloned()
                        && !matches!(self.classify(doc, &cs, None, 0), Space::Keep)
                    {
                        group.remove(b"CS");
                        d.set(b"Group".to_vec(), Object::Dict(group));
                        self.stats.group_spaces += 1;
                        changed = true;
                    }
                    if changed {
                        match &*o {
                            Object::Stream(s) => doc.set(r, Object::Stream(Stream { dict: d, ..s.clone() })),
                            _ => doc.set(r, Object::Dict(d)),
                        }
                    }
                }
                _ => {}
            }
        }
        // Resource lists that are objects of their own.
        later.sort_unstable_by_key(|(kind, r)| (*kind as u8, r.num, r.generation));
        later.dedup();
        for (kind, r) in later {
            if let Object::Dict(mut list) = (*doc.get(r)).clone() {
                let changed = match kind {
                    List::Spaces => self.list(doc, &mut list),
                    List::Shadings => self.shading_list(doc, &mut list),
                    List::Patterns => self.pattern_list(doc, &mut list),
                };
                if changed {
                    doc.set(r, Object::Dict(list));
                }
            }
        }
    }

    /// `d`'s `/ColorSpace` list, when `d` is a resource dictionary: converted in place when it is
    /// written into `d`, else noted in `later`.
    fn list_in(&mut self, doc: &mut Document, d: &mut Dict, later: &mut Vec<(List, ObjRef)>) -> bool {
        match d.get(b"ColorSpace").cloned() {
            Some(Object::Dict(mut list)) => {
                if self.list(doc, &mut list) {
                    d.set(b"ColorSpace".to_vec(), Object::Dict(list));
                    return true;
                }
                false
            }
            Some(Object::Ref(r)) if matches!(&*doc.get(r), Object::Dict(_)) => {
                later.push((List::Spaces, r));
                false
            }
            _ => false,
        }
    }

    /// Convert one colour-space list: indexed and spot spaces written into it are rewritten,
    /// spaces that convert go (content named them, and now names DeviceCMYK), and spaces that can't
    /// convert are reported. Returns whether it changed.
    fn list(&mut self, doc: &mut Document, list: &mut Dict) -> bool {
        let mut changed = false;
        let entries: Vec<(Vec<u8>, Object)> = list.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, cs) in entries {
            match self.classify(doc, &cs, None, 0) {
                Space::Blocked(p, v) => self.block(p, v, None),
                Space::Convert(_) => {
                    list.remove(&name);
                    self.stats.spaces_removed += 1;
                    changed = true;
                }
                // A reference to an array is converted where the array is.
                Space::Keep | Space::Indexed(_) | Space::Spot => {
                    if let Some(new) = self.respace(doc, &cs, 0) {
                        list.set(name, new);
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}

/// A Separation or DeviceN space's colorant names, for a blocker.
fn spot_names(doc: &Document, a: &[Object]) -> String {
    match a.get(1).map(|n| doc.resolve(n)).as_deref() {
        Some(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
        Some(Object::Array(list)) => {
            list.iter().filter_map(Object::as_name).map(|n| String::from_utf8_lossy(n).into_owned()).collect::<Vec<_>>().join(", ")
        }
        _ => String::new(),
    }
}

/// Grid point `i`'s position along each of `k` inputs, the first varying fastest.
fn digits(i: usize, grid: usize, k: usize) -> Vec<usize> {
    let mut rest = i;
    (0..k)
        .map(|_| {
            let d = rest % grid.max(1);
            rest /= grid.max(1);
            d
        })
        .collect()
}

/// The point at grid position `d` (of `grid` points along each input) over `region`. `open_end`:
/// the last point along a single input lies just below the region's end.
fn at(region: &[(f64, f64)], d: &[usize], grid: usize, open_end: bool) -> Vec<f64> {
    let last = grid.saturating_sub(1).max(1);
    region
        .iter()
        .zip(d)
        .map(|((lo, hi), i)| if open_end && *i == last && hi > lo { hi.next_down().max(*lo) } else { lo + (hi - lo) * (*i as f64) / (last as f64) })
        .collect()
}

/// The bytes a row of `width` pixels of `n` components at `bpc` bits takes.
fn row_bytes(width: usize, n: usize, bpc: u32) -> Option<usize> {
    Some(width.checked_mul(n)?.checked_mul(bpc as usize)?.div_ceil(8))
}

/// Sample `i` of a row at `bpc` bits (1, 2, 4, 8 or 16).
fn unpack(row: &[u8], i: usize, bpc: u32) -> Option<u32> {
    match bpc {
        8 => row.get(i).map(|b| u32::from(*b)),
        16 => {
            let at = i.checked_mul(2)?;
            Some(u32::from(u16::from_be_bytes([*row.get(at)?, *row.get(at.checked_add(1)?)?])))
        }
        1 | 2 | 4 => {
            let bit = i.checked_mul(bpc as usize)?;
            let byte = *row.get(bit / 8)?;
            let shift = 8 - bpc - (bit % 8) as u32;
            Some(u32::from(byte >> shift) & ((1 << bpc) - 1))
        }
        _ => None,
    }
}

/// A decode array's pairs; `None` when it isn't one.
fn decode_array(doc: &Document, d: &Object) -> Option<Vec<(f64, f64)>> {
    let Object::Array(a) = &*doc.resolve(d) else { return None };
    let values: Vec<f64> = a.iter().map(|v| doc.resolve(v).as_f64().filter(|v| v.is_finite())).collect::<Option<_>>()?;
    values.len().is_multiple_of(2).then(|| values.as_chunks::<2>().0.iter().map(|&[a, b]| (a, b)).collect())
}

/// The number of components of the ICC profile stream `r` (`/N`, else its header).
fn icc_components(doc: &Document, r: ObjRef) -> Option<i64> {
    let o = doc.get(r);
    let Object::Stream(s) = &*o else { return None };
    s.dict.int(b"N").or_else(|| match s.decoded_strict().ok()?.get(16..20)? {
        b"GRAY" => Some(1),
        b"RGB " => Some(3),
        b"CMYK" => Some(4),
        _ => None,
    })
}

/// How colour is transformed: with `intent`, and with 16-bit table weights (8-bit ones step every
/// 1/255 of the input).
fn options(intent: RenderingIntent) -> TransformOptions {
    TransformOptions { rendering_intent: intent, barycentric_weight_scale: BarycentricWeightScale::High, ..Default::default() }
}

/// A transform from `src` to the CMYK output profile, colorimetric if the profile can, else
/// perceptual.
fn transform(src: &ColorProfile, layout: Layout, output: &ColorProfile) -> Option<Transform> {
    [RenderingIntent::RelativeColorimetric, RenderingIntent::Perceptual]
        .into_iter()
        .find_map(|intent| src.create_transform_f32(layout, output, Layout::Rgba, options(intent)).ok())
}

/// A JPEG's samples: gray or RGB at 8 bits.
fn jpeg_samples(jpeg: &[u8], components: usize) -> Option<Vec<u8>> {
    let image = image::ImageReader::with_format(std::io::Cursor::new(jpeg), image::ImageFormat::Jpeg).decode().ok()?;
    match components {
        1 => Some(image.to_luma8().into_raw()),
        3 => Some(image.to_rgb8().into_raw()),
        _ => None,
    }
}

fn hex(data: &[u8]) -> Vec<u8> {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(data.len().saturating_mul(2).saturating_add(1));
    let digit = |v: u8| DIGITS.get(usize::from(v)).copied().unwrap_or(b'0');
    for b in data {
        out.push(digit(b >> 4));
        out.push(digit(b & 15));
    }
    out
}
