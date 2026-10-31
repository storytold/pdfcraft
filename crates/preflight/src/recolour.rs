//! Colour converted to CMYK through the output profile, for PDF/X-1a, which carries CMYK, gray
//! and spot colour only.
//!
//! What converts: colour that content sets in DeviceRGB or an ICC-based gray, RGB or CMYK space
//! (`rg`/`RG`, and `cs`/`CS` followed by `sc`/`scn`/`SC`/`SCN`), 8-bit pictures in those spaces
//! (inline pictures too, and JPEG pictures, which are written back Flate-compressed), and the
//! colour tables of indexed spaces over them (their indices don't change). DeviceRGB is taken to be
//! sRGB, or the `DefaultRGB` space where content's resources set one. Colour is mapped with the
//! relative colorimetric intent, the PDF default (ISO 32000-2 8.6.5.8), or perceptual when the
//! output profile has no colorimetric table; a content stream's `ri` operator and a picture's
//! `/Intent` are not followed. Gray, CMYK and spot colour stay as they are.
//!
//! What doesn't convert, and so refuses the conversion: shadings, Lab, CalRGB and CalGray,
//! Separation and DeviceN spaces whose alternate isn't gray or CMYK, uncoloured patterns over RGB,
//! pictures of other bit depths or with a decode array or a colour-key mask, JPEG pictures in
//! CMYK, and content or profiles that can't be read.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use moxcms::{ColorProfile, Layout, RenderingIntent, TransformF32Executor, TransformOptions};
use pdfcraft_content::{Op, Pieces, write_op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::conform::Blocker;
use crate::walk::{self, Content};

/// Where colour converts from: sRGB, or the ICC profile in a stream with `n` components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Source {
    Srgb,
    Icc { profile: ObjRef, n: u8 },
}

impl Source {
    fn components(self) -> usize {
        match self {
            Source::Srgb => 3,
            Source::Icc { n, .. } => usize::from(n),
        }
    }
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

/// What the conversion did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Stats {
    pub colours: usize,
    pub pictures: usize,
    pub tables: usize,
    pub spaces_removed: usize,
    pub group_spaces: usize,
}

pub(crate) struct Recolour {
    output: Option<ColorProfile>,
    srgb: Option<ColorProfile>,
    chains: HashMap<Source, Option<Vec<Transform>>>,
    pub stats: Stats,
    pub blockers: Vec<Blocker>,
}

/// Convert every colour in `doc` that isn't gray, CMYK or spot colour to CMYK through the output
/// profile `icc`. Returns what it did and what it couldn't convert; with anything it couldn't,
/// `doc` is half converted and must be thrown away.
pub(crate) fn to_cmyk(doc: &mut Document, icc: &[u8]) -> Recolour {
    let mut r = Recolour {
        output: None,
        srgb: ColorProfile::new_from_slice(&crate::icc::srgb()).ok(),
        chains: HashMap::new(),
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
            Source::Icc { profile, .. } => match &*doc.get(profile) {
                Object::Stream(s) => ColorProfile::new_from_slice(&s.decoded_strict().ok()?).ok()?,
                _ => return None,
            },
        };
        let output = self.output.as_ref()?;
        match src.components() {
            3 => Some(vec![transform(&profile, Layout::Rgb, output)?]),
            4 => Some(vec![transform(&profile, Layout::Rgba, output)?]),
            // Gray goes through sRGB: the colour engine converts gray only to RGB and gray.
            1 => {
                let srgb = self.srgb.as_ref()?;
                let to_rgb = [RenderingIntent::RelativeColorimetric, RenderingIntent::Perceptual].into_iter().find_map(|intent| {
                    profile
                        .create_transform_f32(Layout::Gray, srgb, Layout::Rgb, TransformOptions { rendering_intent: intent, ..Default::default() })
                        .ok()
                })?;
                Some(vec![to_rgb, transform(srgb, Layout::Rgb, output)?])
            }
            _ => None,
        }
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

    fn colour(&mut self, doc: &Document, src: Source, values: &[f64]) -> Option<[f64; 4]> {
        let samples: Vec<f32> = values.iter().map(|v| v.clamp(0.0, 1.0) as f32).collect();
        let c = self.convert(doc, src, &samples)?;
        self.stats.colours += 1;
        // Four decimals are finer than any press resolves.
        let round = |v: f32| (f64::from(v) * 10_000.0).round() / 10_000.0;
        Some([round(*c.first()?), round(*c.get(1)?), round(*c.get(2)?), round(*c.get(3)?)])
    }

    /// 8-bit samples to 8-bit CMYK.
    fn pixels(&mut self, doc: &Document, src: Source, data: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for chunk in data.chunks(CHUNK.saturating_mul(src.components())) {
            let samples: Vec<f32> = chunk.iter().map(|b| f32::from(*b) / 255.0).collect();
            out.extend(self.convert(doc, src, &samples)?.into_iter().map(|v| (v * 255.0).round() as u8));
        }
        Some(out)
    }

    /// The source that DeviceRGB stands for in content drawn with `resources`.
    fn device_rgb(doc: &Document, resources: Option<&Dict>) -> Source {
        let default = resources.and_then(|r| walk::dict_of(doc, r.get(b"ColorSpace")).get(b"DefaultRGB").cloned());
        match default.map(|d| doc.resolve(&d)).as_deref() {
            Some(Object::Array(a)) if a.first().and_then(Object::as_name) == Some(b"ICCBased") => match a.get(1).and_then(Object::as_ref) {
                Some(profile) if icc_components(doc, profile) == Some(3) => Source::Icc { profile, n: 3 },
                _ => Source::Srgb,
            },
            _ => Source::Srgb,
        }
    }

    fn classify(&self, doc: &Document, cs: &Object, resources: Option<&Dict>, depth: u8) -> Space {
        if depth > MAX_SPACE_DEPTH {
            return Space::Blocked("A colour space is nested too deeply to convert", None);
        }
        let o = doc.resolve(cs);
        match &*o {
            Object::Name(n) => match n.as_slice() {
                b"DeviceRGB" | b"RGB" => Space::Convert(Self::device_rgb(doc, resources)),
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
                        Some(Space::Keep) => Space::Keep,
                        Some(Space::Blocked(p, v)) => Space::Blocked(p, v),
                        _ => Space::Blocked("A colour space is nested too deeply to convert", None),
                    },
                    b"Separation" | b"DeviceN" => match a.get(2).map(|b| self.classify(doc, b, resources, depth.saturating_add(1))) {
                        Some(Space::Keep) => Space::Keep,
                        _ => {
                            let names = match a.get(1).map(|n| doc.resolve(n)).as_deref() {
                                Some(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
                                Some(Object::Array(list)) => list
                                    .iter()
                                    .filter_map(Object::as_name)
                                    .map(|n| String::from_utf8_lossy(n).into_owned())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                _ => String::new(),
                            };
                            Space::Blocked("The spot colour {} has an alternate that can't be converted to CMYK yet", Some(names))
                        }
                    },
                    b"Pattern" => match a.get(1).map(|b| self.classify(doc, b, resources, depth.saturating_add(1))) {
                        None | Some(Space::Keep) => Space::Keep,
                        _ => Space::Blocked("A pattern over RGB or device-independent colour can't be converted to CMYK yet", None),
                    },
                    b"CalRGB" | b"CalGray" | b"Lab" => {
                        Space::Blocked("{} colour can't be converted to CMYK yet", Some(String::from_utf8_lossy(family).into_owned()))
                    }
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
                    let src = Self::device_rgb(doc, resources);
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
                        Some(name @ Object::Name(_)) => self.classify(doc, name, resources, 0),
                        _ => Space::Keep,
                    };
                    match &space {
                        Space::Convert(src) => {
                            // Setting a space sets its initial colour: each component at its minimum.
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
                        Space::Keep | Space::Indexed(_) => {}
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
                b"sh" => {
                    if let Some(name) = op.name(0)
                        && let Some(shading) = walk::dict_of(doc, c.resources.get(b"Shading")).get(name).cloned()
                    {
                        self.shading(doc, &doc.resolve(&shading), c.page);
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

    fn shading(&mut self, doc: &Document, shading: &Object, page: Option<usize>) {
        let Some(cs) = shading.as_dict().and_then(|d| d.get(b"ColorSpace")) else { return };
        match self.classify(doc, cs, None, 0) {
            Space::Keep => {}
            Space::Blocked(p, v) => self.block(p, v, page),
            Space::Convert(_) | Space::Indexed(_) => {
                self.block("A shading in RGB or device-independent colour can't be converted to CMYK yet", None, page)
            }
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
            Space::Keep => return None,
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
                if get(b"BPC", b"BitsPerComponent").and_then(|b| b.as_int()) != Some(8) {
                    self.block("A picture with other than 8 bits per component can't be converted to CMYK yet", None, page);
                    return None;
                }
                if get(b"D", b"Decode").is_some() {
                    self.block("A picture with a decode array or a colour-key mask can't be converted to CMYK yet", None, page);
                    return None;
                }
                let mut stream_dict = Dict::new();
                if let Some(f) = get(b"F", b"Filter") {
                    stream_dict.set(b"Filter".to_vec(), f);
                }
                if let Some(p) = get(b"DP", b"DecodeParms") {
                    stream_dict.set(b"DecodeParms".to_vec(), p);
                }
                let (w, h) = (get(b"W", b"Width").and_then(|v| v.as_int()), get(b"H", b"Height").and_then(|v| v.as_int()));
                let samples = self.samples(&Stream::from_raw(stream_dict, data.to_vec()), src, w, h, page)?;
                let cmyk = self.pixels(doc, src, &samples)?;
                for key in [b"CS".as_slice(), b"ColorSpace", b"F", b"Filter", b"DP", b"DecodeParms"] {
                    new.remove(key);
                }
                new.set(b"CS".to_vec(), Object::name("DeviceCMYK"));
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

    /// A picture's samples, decoded, `components` per pixel at 8 bits: `None` (with a blocker)
    /// when they can't be read or don't fill `width` × `height`.
    fn samples(&mut self, s: &Stream, src: Source, width: Option<i64>, height: Option<i64>, page: Option<usize>) -> Option<Vec<u8>> {
        let n = src.components();
        let expected = width
            .zip(height)
            .and_then(|(w, h)| usize::try_from(w).ok().zip(usize::try_from(h).ok()))
            .and_then(|(w, h)| w.checked_mul(h))
            .filter(|px| px.checked_mul(4).is_some_and(|s| s <= MAX_SAMPLES));
        let Some(pixels) = expected else {
            self.block("A picture's data can't be read, so it can't be converted to CMYK", None, page);
            return None;
        };
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
            before.decoded_strict().ok().and_then(|jpeg| jpeg_samples(&jpeg, n))
        } else {
            s.decoded_strict().ok()
        };
        match decoded {
            Some(d) if d.len() == pixels.saturating_mul(n) => Some(d),
            Some(d) if d.len() > pixels.saturating_mul(n) => Some(d.get(..pixels.saturating_mul(n)).unwrap_or_default().to_vec()),
            _ => {
                self.block("A picture's data can't be read, so it can't be converted to CMYK", None, page);
                None
            }
        }
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
        let cmyk = self.pixels(doc, src, entries)?;
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
                Space::Keep | Space::Indexed(_) => {}
                Space::Blocked(p, v) => self.block(p, v, None),
                Space::Convert(src) => {
                    if s.dict.int(b"BitsPerComponent") != Some(8) {
                        self.block("A picture with other than 8 bits per component can't be converted to CMYK yet", None, None);
                        continue;
                    }
                    if s.dict.get(b"Decode").is_some() || matches!(s.dict.get(b"Mask").map(|m| doc.resolve(m)).as_deref(), Some(Object::Array(_))) {
                        self.block("A picture with a decode array or a colour-key mask can't be converted to CMYK yet", None, None);
                        continue;
                    }
                    let Some(samples) = self.samples(s, src, s.dict.int(b"Width"), s.dict.int(b"Height"), None) else { continue };
                    let Some(cmyk) = self.pixels(doc, src, &samples) else { continue };
                    let mut dict = s.dict.clone();
                    for key in [b"Length".as_slice(), b"DecodeParms", b"Filter"] {
                        dict.remove(key);
                    }
                    dict.set(b"ColorSpace".to_vec(), Object::name("DeviceCMYK"));
                    doc.set(r, Object::Stream(Stream::flate(dict, &cmyk)));
                    self.stats.pictures += 1;
                }
            }
        }
    }

    /// Colour spaces outside content: indexed tables converted, the spaces content no longer
    /// names taken out of resource lists, shadings and spot colour checked, and the RGB blending
    /// spaces of transparency groups dropped.
    fn spaces(&mut self, doc: &mut Document) {
        let mut lists: Vec<ObjRef> = Vec::new();
        for n in doc.object_numbers() {
            let r = ObjRef::new(n, doc.generation(n));
            let o = doc.get(r);
            match &*o {
                Object::Array(a) if matches!(a.first().and_then(Object::as_name), Some(b"Indexed" | b"I")) => {
                    if let Space::Indexed(src) = self.classify(doc, &o, None, 0)
                        && let Some(new) = self.indexed(doc, a, src)
                    {
                        doc.set(r, new);
                    }
                }
                Object::Dict(_) | Object::Stream(_) => {
                    let mut d = match &*o {
                        Object::Stream(s) => s.dict.clone(),
                        Object::Dict(d) => d.clone(),
                        _ => continue,
                    };
                    let mut changed = false;
                    if d.get(b"ShadingType").is_some() {
                        self.shading(doc, &Object::Dict(d.clone()), None);
                    }
                    if let Some(Object::Dict(shading)) = d.get(b"Shading") {
                        self.shading(doc, &Object::Dict(shading.clone()), None);
                    }
                    // An image's own space (an indexed one written into it).
                    if let Some(Object::Array(a)) = d.get(b"ColorSpace").cloned()
                        && let Space::Indexed(src) = self.classify(doc, &Object::Array(a.clone()), None, 0)
                        && let Some(new) = self.indexed(doc, &a, src)
                    {
                        d.set(b"ColorSpace".to_vec(), new);
                        changed = true;
                    }
                    // A resource dictionary's list, and an inline /Resources' list.
                    changed |= self.list_in(doc, &mut d, &mut lists);
                    if let Some(Object::Dict(mut res)) = d.get(b"Resources").cloned()
                        && self.list_in(doc, &mut res, &mut lists)
                    {
                        d.set(b"Resources".to_vec(), Object::Dict(res));
                        changed = true;
                    }
                    if let Some(Object::Dict(res)) = d.get(b"Resources").cloned()
                        && let Some(Object::Dict(shadings)) = res.get(b"Shading").map(|s| doc.resolve(s)).as_deref().cloned()
                    {
                        for (_, s) in shadings.iter() {
                            if let Object::Dict(_) = s {
                                self.shading(doc, s, None);
                            }
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
        // Colour-space lists that are objects of their own.
        lists.sort_unstable_by_key(|r| (r.num, r.generation));
        lists.dedup();
        for r in lists {
            if let Object::Dict(mut list) = (*doc.get(r)).clone()
                && self.list(doc, &mut list)
            {
                doc.set(r, Object::Dict(list));
            }
        }
    }

    /// `d`'s `/ColorSpace` list, when `d` is a resource dictionary: converted in place when it is
    /// written into `d`, else noted in `lists`.
    fn list_in(&mut self, doc: &Document, d: &mut Dict, lists: &mut Vec<ObjRef>) -> bool {
        match d.get(b"ColorSpace").cloned() {
            Some(Object::Dict(mut list)) => {
                if self.list(doc, &mut list) {
                    d.set(b"ColorSpace".to_vec(), Object::Dict(list));
                    return true;
                }
                false
            }
            Some(Object::Ref(r)) if matches!(&*doc.get(r), Object::Dict(_)) => {
                lists.push(r);
                false
            }
            _ => false,
        }
    }

    /// Convert one colour-space list: indexed spaces written into it get CMYK tables, spaces that
    /// convert go (content named them, and now names DeviceCMYK), and spaces that can't convert
    /// are reported. Returns whether it changed.
    fn list(&mut self, doc: &Document, list: &mut Dict) -> bool {
        let mut changed = false;
        let entries: Vec<(Vec<u8>, Object)> = list.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, cs) in entries {
            match self.classify(doc, &cs, None, 0) {
                Space::Keep => {}
                Space::Blocked(p, v) => self.block(p, v, None),
                Space::Convert(_) => {
                    list.remove(&name);
                    self.stats.spaces_removed += 1;
                    changed = true;
                }
                Space::Indexed(src) => {
                    // A reference to an indexed array is converted where the array is.
                    if let Object::Array(a) = &cs
                        && let Some(new) = self.indexed(doc, a, src)
                    {
                        list.set(name, new);
                        changed = true;
                    }
                }
            }
        }
        changed
    }
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

/// A transform from `src` to the CMYK output profile, colorimetric if the profile can, else
/// perceptual.
fn transform(src: &ColorProfile, layout: Layout, output: &ColorProfile) -> Option<Transform> {
    [RenderingIntent::RelativeColorimetric, RenderingIntent::Perceptual].into_iter().find_map(|intent| {
        src.create_transform_f32(layout, output, Layout::Rgba, TransformOptions { rendering_intent: intent, ..Default::default() }).ok()
    })
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
