//! Image resampling and recompression.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Matrix, Pieces};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::{Compression, ImageSettings, MonoCompression, MonoSettings, OptimizeError, Report, Settings, Stage};

/// Images with more pixels than this are left alone (memory).
const MAX_PIXELS: u64 = 80_000_000;

/// The resources of a page, inherited from the page tree if the page has none.
pub(crate) fn page_resources(doc: &Document, page: ObjRef) -> Option<Dict> {
    held_page_resources(doc, page).map(|(resources, _)| resources)
}

/// [`page_resources`], and the object holding them: the page or the page-tree node they are
/// written in (inherited from), or the resources dictionary when it is an object of its own.
fn held_page_resources(doc: &Document, page: ObjRef) -> Option<(Dict, ObjRef)> {
    let mut node = Some(page);
    for _ in 0..64 {
        let n = node?;
        let d = doc.get(n).as_dict()?.clone();
        if let Some(r) = d.get(b"Resources") {
            return Some((doc.resolve(r).as_dict().cloned()?, r.as_ref().unwrap_or(n)));
        }
        node = d.reference(b"Parent");
    }
    None
}

/// The page content `contents` names, decoded against `budget` and joined. `None` when a stream
/// doesn't decode or the budget runs out: the content is not all known.
pub(crate) fn content_bytes(doc: &Document, contents: &Object, budget: &mut DecodeBudget) -> Option<Vec<u8>> {
    let pieces = budget.decode_contents(doc, contents).into_iter().collect::<Result<Vec<_>, _>>().ok()?;
    Some(Pieces::join(&pieces).data().to_vec())
}

/// Forms nested deeper than this aren't followed.
const MAX_FORM_DEPTH: usize = 12;
/// Walks of a form already walked once (other scale or caller) before the measurement gives up.
/// Each form's first walk is free: their number is bounded by the file's object count.
const MAX_REWALKS: usize = 100_000;

/// State of one [`effective_resolutions`] pass.
struct Walk<'a> {
    doc: &'a Document,
    /// The forms being walked right now, outermost first: a form drawing one of these is a cycle.
    path: Vec<ObjRef>,
    /// Forms with their own resources already walked, by the linear part of their matrix and
    /// whether default colour spaces were in force. Drawing one again at the same scale (only
    /// moved) can't change any image's resolution.
    walked: HashSet<(ObjRef, [u64; 4], bool)>,
    /// Forms walked at least once.
    seen: HashSet<ObjRef>,
    rewalks: usize,
    /// Decoded bytes left for the page being walked.
    budget: DecodeBudget,
    /// False once a use may have been missed (depth or work limit).
    complete: bool,
    out: HashMap<ObjRef, f64>,
    /// Images drawn or listed at least once where `/DefaultGray` or `/DefaultRGB` is in force, by
    /// object number: a reference with another generation reaches the same image.
    defaults: HashSet<u32>,
    /// The references the walk followed to reach its images, and the pages and page-tree nodes
    /// whose resources it read.
    read: Read,
}

/// Where a walk read a reference: under a name in an `/XObject` dictionary (an image or a form),
/// as resources' `/XObject` dictionary, or as the `/Resources` of a page, node or form.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Entry {
    Name(Vec<u8>),
    XObject,
    Resources,
}

/// The references a walk followed, by object number: (the object referred to, the object holding
/// the reference, where in it), and the pages and page-tree nodes whose resources it read.
#[derive(Default)]
struct Read {
    entries: HashSet<(u32, u32, Entry)>,
    roots: HashSet<u32>,
}

impl Walk<'_> {
    /// Walk one content stream, recording each image's displayed resolution (pixels per inch, the
    /// smaller of the two axes) and descending into form XObjects. `holder` is the object holding
    /// `resources`. `defaults`: an enclosing stream's resources remap a device colour space.
    fn content(&mut self, data: &[u8], resources: &Dict, holder: ObjRef, ctm: Matrix, defaults: bool) {
        let doc = self.doc;
        let defaults = defaults || remaps_device_spaces(doc, resources);
        let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
        let xholder = match resources.get(b"XObject").and_then(Object::as_ref) {
            Some(x) => {
                self.read.entries.insert((x.num, holder.num, Entry::XObject));
                x
            }
            None => holder,
        };
        // Every entry counts as read, drawn here or not: content using these resources (a Type3
        // glyph without its own, say) draws it under the same defaults.
        for (name, v) in xobjects.iter() {
            if let Some(r) = v.as_ref() {
                self.read.entries.insert((r.num, xholder.num, Entry::Name(name.to_vec())));
                if defaults {
                    self.defaults.insert(r.num);
                }
            }
        }
        let mut stack = vec![ctm];
        for op in pdfcraft_content::parse(data).ops {
            let top = stack.last().copied().unwrap_or(ctm);
            match op.op.as_slice() {
                b"q" => stack.push(top),
                b"Q" if stack.len() > 1 => {
                    stack.pop();
                }
                b"cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && let Some(t) = stack.last_mut()
                    {
                        *t = m.then(&top);
                    }
                }
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some(r) = xobjects.get(name).and_then(Object::as_ref) else { continue };
                    let obj = doc.get(r);
                    let Object::Stream(s) = &*obj else { continue };
                    match s.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            let (w, h) = (s.dict.int(b"Width").unwrap_or(0) as f64, s.dict.int(b"Height").unwrap_or(0) as f64);
                            let [a, b, c, d, ..] = top.0;
                            let (sx, sy) = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
                            if w > 0.0 && h > 0.0 && sx > 1e-6 && sy > 1e-6 {
                                let ppi = (w / (sx / 72.0)).min(h / (sy / 72.0));
                                let e = self.out.entry(r).or_insert(f64::INFINITY);
                                *e = e.min(ppi);
                            }
                            if defaults {
                                self.defaults.insert(r.num);
                            }
                        }
                        Some(b"Form") => self.form(r, s, (resources, holder), top, defaults),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Read the resources of page `p` and of each page-tree node above it, each of them a root: a
    /// node's resources are its inheriting pages' (every page is walked), or no page's.
    fn resources_above(&mut self, p: ObjRef) {
        let doc = self.doc;
        let mut node = Some(p);
        for _ in 0..64 {
            let Some(n) = node else { return };
            // A node met before had its own nodes above it read then.
            if !self.read.roots.insert(n.num) {
                return;
            }
            let Some(d) = doc.get(n).as_dict().cloned() else { return };
            if let Some(r) = d.get(b"Resources")
                && let Some(resources) = doc.resolve(r).as_dict().cloned()
            {
                let holder = match r.as_ref() {
                    Some(res) => {
                        self.read.entries.insert((res.num, n.num, Entry::Resources));
                        res
                    }
                    None => n,
                };
                self.content(&[], &resources, holder, Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), false);
            }
            node = d.reference(b"Parent");
        }
    }

    /// Descend into form `r` drawn with `ctm` from a stream whose resources are `resources`, with
    /// the object holding them.
    fn form(&mut self, r: ObjRef, s: &Stream, (resources, holder): (&Dict, ObjRef), ctm: Matrix, defaults: bool) {
        if self.path.contains(&r) {
            return;
        }
        if self.path.len() >= MAX_FORM_DEPTH {
            self.complete = false;
            return;
        }
        let doc = self.doc;
        let m = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().map(|a| Matrix::from_operands(a))).flatten();
        let inner = m.unwrap_or(Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])).then(&ctm);
        let own = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        // A form without resources uses its caller's, so the same form can draw other images.
        if own.is_some() {
            let [a, b, c, d, ..] = inner.0;
            if !self.walked.insert((r, [a.to_bits(), b.to_bits(), c.to_bits(), d.to_bits()], defaults)) {
                return;
            }
        }
        if !self.seen.insert(r) {
            if self.rewalks >= MAX_REWALKS {
                self.complete = false;
                return;
            }
            self.rewalks += 1;
        }
        if !self.budget.enter_form() {
            self.complete = false;
            return;
        }
        let Ok(data) = self.budget.decode(s) else {
            self.complete = false;
            return;
        };
        self.path.push(r);
        match &own {
            Some(own) => {
                let own_holder = match s.dict.get(b"Resources").and_then(Object::as_ref) {
                    Some(res) => {
                        self.read.entries.insert((res.num, r.num, Entry::Resources));
                        res
                    }
                    None => r,
                };
                self.content(&data, own, own_holder, inner, defaults);
            }
            None => self.content(&data, resources, holder, inner, defaults),
        }
        self.path.pop();
    }
}

/// Whether `resources` remap a device colour space an image may be moved between, so that the
/// same samples would be drawn otherwise: `/DefaultGray` or `/DefaultRGB` (ISO 32000-2 §8.6.5.6).
fn remaps_device_spaces(doc: &Document, resources: &Dict) -> bool {
    let spaces = resources.get(b"ColorSpace").map(|c| doc.resolve(c));
    spaces.as_deref().and_then(Object::as_dict).is_some_and(|c| c.contains(b"DefaultGray") || c.contains(b"DefaultRGB"))
}

/// The effective resolution (ppi) of every image drawn by `pages`: the smallest over its uses.
///
/// If forms nest too deeply, are drawn too many times, or a page's content doesn't decode within
/// its budget, every image is given `INFINITY` (never resampled), since its largest use may be
/// among those missed.
pub fn effective_resolutions(doc: &Document, pages: &[ObjRef]) -> HashMap<ObjRef, f64> {
    walk_pages(doc, pages).0
}

/// [`effective_resolutions`], the images drawn or listed where `/DefaultGray` or `/DefaultRGB` is
/// in force, by object number (all of them when a use may have been missed), and the references
/// followed to reach them.
fn walk_pages(doc: &Document, pages: &[ObjRef]) -> (HashMap<ObjRef, f64>, HashSet<u32>, Read) {
    let mut walk = Walk {
        doc,
        path: Vec::new(),
        walked: HashSet::new(),
        seen: HashSet::new(),
        rewalks: 0,
        budget: DecodeBudget::page(),
        complete: true,
        out: HashMap::new(),
        defaults: HashSet::new(),
        read: Read::default(),
    };
    for p in pages {
        let Some(dict) = doc.get(*p).as_dict().cloned() else { continue };
        let page = pdfcraft_model::Page { obj: *p, dict };
        walk.resources_above(*p);
        let Some(contents) = page.dict.get(b"Contents") else { continue };
        let (res, holder) = held_page_resources(doc, *p).unwrap_or_else(|| (Dict::default(), *p));
        walk.budget = DecodeBudget::page();
        let Some(data) = content_bytes(doc, contents, &mut walk.budget) else {
            walk.complete = false;
            continue;
        };
        // Content coordinates are in page units; image resolution is in physical inches.
        let unit = page.user_unit(doc);
        walk.content(&data, &res, holder, Matrix([unit, 0.0, 0.0, unit, 0.0, 0.0]), false);
    }
    if !walk.complete {
        walk.out.values_mut().for_each(|v| *v = f64::INFINITY);
        walk.defaults.extend(walk.out.keys().map(|r| r.num));
    }
    (walk.out, walk.defaults, walk.read)
}

/// Whether image `r` is one [`one_bit`] may make grey: eight-bit `DeviceRGB`, not in an image codec.
fn device_rgb(doc: &Document, r: ObjRef) -> bool {
    let Object::Stream(s) = &*doc.get(r) else { return false };
    let cs = s.dict.get(b"ColorSpace").map(|cs| doc.resolve(cs));
    let codec =
        s.filters().iter().any(|(f, _)| matches!(f, pdfcraft_filters::Filter::Dct | pdfcraft_filters::Filter::Jpx | pdfcraft_filters::Filter::Jbig2));
    s.dict.int(b"BitsPerComponent") == Some(8)
        && !codec
        && cs.as_deref().and_then(Object::as_name).is_some_and(|n| matches!(n, b"DeviceRGB" | b"RGB"))
}

/// Call `f` with every reference in `o`, a dictionary's or array's included, a stream's dictionary
/// too, but not those in objects referred to.
fn each_ref(o: &Object, mut f: impl FnMut(ObjRef)) {
    // Direct objects nest finitely (the parser builds trees), so this ends.
    let mut stack = vec![o];
    while let Some(o) = stack.pop() {
        match o {
            Object::Ref(r) => f(*r),
            Object::Dict(d) => stack.extend(d.iter().map(|(_, v)| v)),
            Object::Array(a) => stack.extend(a),
            Object::Stream(s) => stack.extend(s.dict.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

/// The images among `images` (by object number) that may be drawn beyond what the walk read:
/// some object the document uses refers to the image, or to an object on the way to it from a
/// page (an `/XObject` dictionary, resources, a form), other than as the page and form resources
/// the walk read do. So an annotation's appearance, a pattern, a Type3 glyph or a soft mask's
/// group drawing it, directly or through resources or a form it shares with a page, counts, as
/// other default colour spaces may be in force there, and so does any other reference (another
/// image's `/Alternates`, say). Objects nothing reaches from the trailer don't count.
fn used_elsewhere(doc: &Document, images: &HashSet<u32>, read: &Read) -> HashSet<u32> {
    if images.is_empty() {
        return HashSet::new();
    }
    let targets: HashSet<u32> = read.entries.iter().map(|(target, ..)| *target).collect();
    // References to each object the walk followed, by the object holding them.
    let mut held: HashMap<u32, HashMap<u32, usize>> = HashMap::new();
    let mut queue: Vec<u32> = Vec::new();
    let mut visited = HashSet::new();
    for (_, v) in doc.trailer().iter() {
        each_ref(v, |r| queue.push(r.num));
    }
    while let Some(num) = queue.pop() {
        if !visited.insert(num) {
            continue;
        }
        let obj = doc.get(ObjRef::new(num, doc.generation(num)));
        each_ref(&obj, |r| {
            if targets.contains(&r.num) {
                *held.entry(r.num).or_default().entry(num).or_default() += 1;
            }
            if !visited.contains(&r.num) {
                queue.push(r.num);
            }
        });
    }
    let mut seen: HashMap<(u32, u32), usize> = HashMap::new();
    for (target, holder, _) in &read.entries {
        *seen.entry((*target, *holder)).or_default() += 1;
    }
    // Covered: every reference to it is one the walk followed, held by a page or node, or by an
    // object covered in turn. Everything the walk reached starts covered, and whatever has a
    // reference it didn't follow, or is held by an object taken out, is taken out, until nothing
    // more is. Content beyond what the walk read has to refer to an object on its way to the
    // image, which takes out everything after it; a cycle the walk read whole stays covered.
    let mut holds: HashMap<u32, Vec<u32>> = HashMap::new();
    for (target, holders) in &held {
        for h in holders.keys() {
            holds.entry(*h).or_default().push(*target);
        }
    }
    let mut covered: HashSet<u32> = held.keys().copied().collect();
    let passes = |t: u32, covered: &HashSet<u32>| {
        held.get(&t)
            .is_some_and(|holders| holders.iter().all(|(h, n)| seen.get(&(t, *h)) == Some(n) && (read.roots.contains(h) || covered.contains(h))))
    };
    let mut check: Vec<u32> = covered.iter().copied().collect();
    while let Some(t) = check.pop() {
        if covered.contains(&t) && !passes(t, &covered) {
            covered.remove(&t);
            check.extend(holds.get(&t).into_iter().flatten().copied());
        }
    }
    images.iter().copied().filter(|i| !covered.contains(i)).collect()
}

/// Colour components of a colour space PdfCraft resamples: gray (1) or RGB (3).
pub(crate) fn components(doc: &Document, cs: &Object) -> Option<usize> {
    let cs = doc.resolve(cs);
    match &*cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" => Some(1),
            b"DeviceRGB" | b"RGB" => Some(3),
            _ => None,
        },
        Object::Array(a) => match a.first()?.as_name()? {
            b"ICCBased" => match &*doc.resolve(a.get(1)?) {
                Object::Stream(icc) => match icc.dict.int(b"N")? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                },
                _ => None,
            },
            b"CalGray" => Some(1),
            b"CalRGB" => Some(3),
            _ => None,
        },
        _ => None,
    }
}

fn is_dct(s: &Stream) -> Option<bool> {
    match s.dict.get(b"Filter") {
        None => Some(false),
        Some(Object::Name(n)) => match n.as_slice() {
            b"DCTDecode" | b"DCT" => Some(true),
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"RunLengthDecode" | b"RL" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" => {
                Some(false)
            }
            _ => None,
        },
        Some(Object::Array(a)) => {
            let names: Vec<&[u8]> = a.iter().filter_map(Object::as_name).collect();
            if names.len() != a.len() {
                return None;
            }
            match names.as_slice() {
                [b"DCTDecode" | b"DCT"] => Some(true),
                n if n.iter().all(|f| {
                    matches!(
                        *f,
                        b"FlateDecode"
                            | b"Fl"
                            | b"LZWDecode"
                            | b"LZW"
                            | b"RunLengthDecode"
                            | b"RL"
                            | b"ASCII85Decode"
                            | b"A85"
                            | b"ASCIIHexDecode"
                            | b"AHx"
                    )
                }) =>
                {
                    Some(false)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
thread_local! {
    /// Pixels decoded from JPEG data, for the test that keeps a mismatched JPEG from decoding.
    static DECODED_PIXELS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts `n` decoded pixels (tests only).
#[inline]
fn count_decoded(_n: u64) {
    #[cfg(test)]
    DECODED_PIXELS.with(|c| c.set(c.get().saturating_add(_n)));
}

/// 8-bit samples of an image (`n` components), decoded, with `/DecodeParms` given by reference
/// too ([`Document::with_direct_parms`]): read without them, a predictor's tag bytes would shift
/// every row, and the image would be written back wrong. `None` for a JPEG whose colour transform
/// readers may disagree about ([`ycc_encoded`]), and for data that decodes past what the samples
/// can need ([`allowance`]).
pub(crate) fn pixels(doc: &Document, s: &Stream, n: usize) -> Option<Vec<u8>> {
    let (w, h) = (u32::try_from(s.dict.int(b"Width")?).ok()?, u32::try_from(s.dict.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let bytes = usize::try_from(w).ok()?.checked_mul(usize::try_from(h).ok()?)?.checked_mul(n)?;
    if is_dct(s)? {
        // The colour transform matters to three components only (§7.4.8). CMYK and YCCK JPEGs
        // never get here: the colour space check excludes them. The JPEG's own size must be the
        // dictionary's: `jpeg_samples` refuses another from its header, before any pixels.
        let ycc = n == 3 && ycc_encoded(doc, s)?;
        let v = jpeg_samples(&s.raw, usize::try_from(w).ok()?, usize::try_from(h).ok()?, n, ycc)?;
        count_decoded(u64::from(w) * u64::from(h));
        (v.len() == bytes).then_some(v)
    } else {
        let v = doc.with_direct_parms(s).decoded_within(allowance(bytes, h)?).ok()?;
        Some(v.get(..bytes)?.to_vec())
    }
}

/// How far an image of `bytes` samples in `rows` rows is decoded: a filter stage may produce up
/// to three bytes for each of them and of a PNG predictor's bytes a row (ASCIIHex text under
/// Flate, its two digits a byte broken into lines or spaced). More can only be a decompression
/// bomb, and the image is left alone.
fn allowance(bytes: usize, rows: u32) -> Option<usize> {
    bytes.checked_add(usize::try_from(rows).ok()?)?.checked_mul(3)?.checked_add(4096)
}

/// Whether the samples of a three-component JPEG are YCbCr, to be converted to RGB (ISO 32000-2
/// §7.4.8). An Adobe APP14 marker's transform flag says so, overriding the filter's
/// /ColorTransform; without a marker /ColorTransform does, 1 (YCbCr) by default.
///
/// `None`, so the image is left as it is, when readers may disagree: the marker and
/// /ColorTransform contradict each other (PdfCraft's renderer lets /ColorTransform 0 win where
/// the standard lets the marker win), either holds a value other than 0 or 1, or the parameters
/// or the JPEG's segments can't be read.
fn ycc_encoded(doc: &Document, s: &Stream) -> Option<bool> {
    let entry = color_transform(doc, s)?;
    match adobe_transform(&s.raw)? {
        Some(t @ (0 | 1)) => entry.is_none_or(|c| c == i64::from(t)).then_some(t == 1),
        Some(_) => None,
        None => match entry {
            None | Some(1) => Some(true),
            Some(0) => Some(false),
            Some(_) => None,
        },
    }
}

/// The /ColorTransform of a JPEG stream's filter parameters: `Some(None)` without one. `None`
/// if it isn't an integer, or the parameters aren't a dictionary paired with the filter as
/// §7.4 pairs them (a dictionary for a filter name, a one-element array for a one-filter array).
fn color_transform(doc: &Document, s: &Stream) -> Option<Option<i64>> {
    let Some(parms) = s.dict.get(b"DecodeParms") else { return Some(None) };
    let parms = doc.resolve(parms);
    let parms = match (s.dict.get(b"Filter"), &*parms) {
        (Some(Object::Array(_)), Object::Array(a)) if a.len() == 1 => doc.resolve(a.first()?),
        (Some(Object::Name(_)), _) | (_, Object::Null) => parms.clone(),
        _ => return None,
    };
    match &*parms {
        Object::Null => Some(None),
        Object::Dict(d) => match d.get(b"ColorTransform").map(|c| doc.resolve(c)).as_deref() {
            None => Some(None),
            Some(Object::Int(c)) => Some(Some(*c)),
            Some(_) => None,
        },
        _ => None,
    }
}

/// Markers and fill bytes [`adobe_transform`] reads before the first scan, at most. Encoders
/// write a few dozen segments.
const MAX_HEADER_STEPS: usize = 4096;

/// The transform flag of a JPEG's Adobe APP14 segment (0: samples stored as they are,
/// 1: YCbCr, 2: YCCK), the last one if there are several, as decoders read them; `Some(None)`
/// without one. `None` if the segments before the first scan can't be read, which includes a
/// stray byte between segments and a header of more than [`MAX_HEADER_STEPS`] steps.
///
/// Each step reads one marker or fill byte and jumps over its segment, so the work is bounded
/// by the step count, not by the size of the stream. That matters: Optimize asks again for every
/// group of fragments it tries, and the same image can be drawn many times.
fn adobe_transform(data: &[u8]) -> Option<Option<u8>> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut flag = None;
    let mut i = 2usize;
    // Reading past the end returns.
    for _ in 0..MAX_HEADER_STEPS {
        // Decoders skip stray bytes between segments, but a stream can hold megabytes of them
        // and each would be a step: the header counts as unreadable and the image is left as
        // it is.
        if *data.get(i)? != 0xFF {
            return None;
        }
        let marker = *data.get(i.checked_add(1)?)?;
        match marker {
            // A fill byte before the marker.
            0xFF => i = i.checked_add(1)?,
            // The first scan (or the end): no header segments after it.
            0xDA | 0xD9 => return Some(flag),
            // Markers without a segment: TEM, RSTn, SOI.
            0x01 | 0xD0..=0xD8 => i = i.checked_add(2)?,
            _ => {
                let len = usize::from(u16::from_be_bytes([*data.get(i.checked_add(2)?)?, *data.get(i.checked_add(3)?)?]));
                // The length counts its own two bytes: anything shorter is malformed.
                let end = i.checked_add(2)?.checked_add(len)?;
                let segment = data.get(i.checked_add(4)?..end)?;
                if marker == 0xEE
                    && let Some(rest) = segment.strip_prefix(b"Adobe")
                {
                    // Version, flags0 and flags1 (two bytes each), then the transform flag.
                    flag = Some(*rest.get(6)?);
                }
                i = end;
            }
        }
    }
    None
}

/// 8-bit samples of a JPEG `w` × `h` with `n` components (grey or RGB): YCbCr converted to RGB
/// if `ycc`, otherwise the samples as stored. `None` if the JPEG doesn't match (size or number
/// of components), is too large, or its decoder reads the samples otherwise (for example
/// components named R, G and B, which decoders take as stored, while `ycc` says YCbCr).
fn jpeg_samples(data: &[u8], w: usize, h: usize, n: usize, ycc: bool) -> Option<Vec<u8>> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    if (w.checked_mul(h)? as u64) > MAX_PIXELS {
        return None;
    }
    let options = DecoderOptions::default().set_strict_mode(false).set_max_width(usize::from(u16::MAX)).set_max_height(usize::from(u16::MAX));
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    dec.decode_headers().ok()?;
    if dec.dimensions()? != (w, h) || usize::from(dec.info()?.components) != n {
        return None;
    }
    let out = match (n, ycc, dec.input_colorspace()?) {
        (1, _, ColorSpace::Luma) => ColorSpace::Luma,
        (3, true, ColorSpace::YCbCr) => ColorSpace::RGB,
        // As stored: no conversion either way. The decoder reports an Adobe marker's 0 as CMYK
        // until it decodes, and then reads three such components as RGB.
        (3, false, ColorSpace::YCbCr) => ColorSpace::YCbCr,
        (3, false, ColorSpace::RGB | ColorSpace::CMYK) => ColorSpace::RGB,
        _ => return None,
    };
    dec.set_options(options.jpeg_set_out_colorspace(out));
    let px = dec.decode().ok()?;
    (px.len() == w.checked_mul(h)?.checked_mul(n)?).then_some(px)
}

pub(crate) fn resample(px: &[u8], n: usize, w: u32, h: u32, nw: u32, nh: u32) -> Option<Vec<u8>> {
    use image::imageops::{FilterType, resize};
    Some(match n {
        1 => resize(&image::GrayImage::from_raw(w, h, px.to_vec())?, nw, nh, FilterType::CatmullRom).into_raw(),
        _ => resize(&image::RgbImage::from_raw(w, h, px.to_vec())?, nw, nh, FilterType::CatmullRom).into_raw(),
    })
}

fn jpeg(px: &[u8], n: usize, w: u32, h: u32, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
    enc.encode(px, w, h, ty).ok()?;
    Some(out)
}

/// Whether [`pixels`] reads `s` as it is drawn: a codec it decodes, no decode array that
/// changes anything, and for an RGB JPEG a colour transform readers agree on. `None` when the
/// colour space isn't grey or RGB.
pub(crate) fn decodable(doc: &Document, s: &Stream) -> Option<bool> {
    let n = components(doc, s.dict.get(b"ColorSpace")?)?;
    let dct = is_dct(s);
    Some(
        dct.is_some()
            && s.dict.get(b"Decode").is_none_or(|d| is_identity_decode(doc, d, n))
            && (n != 3 || dct == Some(false) || ycc_encoded(doc, s).is_some()),
    )
}

/// Whether `decode` is the default decode array for `n` components ([0 1] each), which changes
/// nothing. Scanners write it on page images (#490).
fn is_identity_decode(doc: &Document, decode: &Object, n: usize) -> bool {
    let decode = doc.resolve(decode);
    let Some(a) = decode.as_array() else { return false };
    a.len() == 2 * n && a.chunks(2).all(|p| matches!(p, [lo, hi] if doc.resolve(lo).as_f64() == Some(0.0) && doc.resolve(hi).as_f64() == Some(1.0)))
}

/// A new image stream from 8-bit samples: the original dictionary with the new size and filter.
/// A decode array is dropped: only images without one, or with the default one, get here. So are
/// the filter parameters: the samples are decoded, and a JPEG written here is plain YCbCr with no
/// Adobe marker, which is what a missing /ColorTransform means.
fn image_stream(old: &Dict, px: &[u8], n: usize, w: u32, h: u32, compression: Compression, was_dct: bool) -> Option<Stream> {
    let mut d = old.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"Decode", b"Length", b"DL"] {
        d.remove(k);
    }
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let use_jpeg = match compression {
        Compression::Jpeg(_) => true,
        Compression::Flate => false,
        Compression::Retain => was_dct,
    };
    if use_jpeg {
        let q = match compression {
            Compression::Jpeg(q) => q,
            _ => 80,
        };
        let data = jpeg(px, n, w, h, q)?;
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Some(Stream::from_raw(d, data))
    } else {
        Some(Stream::flate(d, px))
    }
}

/// One image: resample and/or recompress per `settings`; `ppi` is its effective resolution.
/// Returns the new image and its new soft mask, if the result is smaller.
fn process(doc: &Document, s: &Stream, ppi: f64, settings: &ImageSettings, n: usize) -> Option<(Stream, Option<(ObjRef, Stream)>)> {
    let d = &s.dict;
    if d.get(b"Decode").is_some_and(|a| !is_identity_decode(doc, a, n))
        || d.contains(b"Mask")
        || d.name(b"ImageMask").is_some()
        || matches!(d.get(b"ImageMask"), Some(Object::Bool(true)))
    {
        return None;
    }
    if d.int(b"BitsPerComponent") != Some(8) {
        return None;
    }
    let was_dct = is_dct(s)?;
    let (w, h) = (u32::try_from(d.int(b"Width")?).ok()?, u32::try_from(d.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let resize = settings.downsample && ppi.is_finite() && ppi > settings.above_ppi && settings.target_ppi > 0.0;
    let recompress = matches!(settings.compression, Compression::Jpeg(_)) || (matches!(settings.compression, Compression::Flate) && was_dct);
    if !resize && !recompress {
        return None;
    }
    // Tiny images (icons, rules) don't benefit and JPEG would blur them.
    if !resize && (w as u64) * (h as u64) < 64 * 64 {
        return None;
    }
    let (nw, nh) = if resize {
        let k = settings.target_ppi / ppi;
        (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1))
    } else {
        (w, h)
    };
    let px = pixels(doc, s, n)?;
    let px = if (nw, nh) != (w, h) { resample(&px, n, w, h, nw, nh)? } else { px };
    let new = image_stream(d, &px, n, nw, nh, settings.compression, was_dct)?;
    // A soft mask must keep the image's dimensions: resample it with the image (lossless).
    let mut smask = None;
    if let Some(mr) = d.reference(b"SMask") {
        let mobj = doc.get(mr);
        let Object::Stream(m) = &*mobj else { return None };
        if (nw, nh) != (w, h) {
            if m.dict.int(b"BitsPerComponent") != Some(8) || m.dict.contains(b"Matte") || m.dict.contains(b"Decode") {
                return None;
            }
            let (mw, mh) = (u32::try_from(m.dict.int(b"Width")?).ok()?, u32::try_from(m.dict.int(b"Height")?).ok()?);
            let mpx = pixels(doc, m, 1)?;
            let mpx = resample(&mpx, 1, mw, mh, nw, nh)?;
            smask = Some((mr, image_stream(&m.dict, &mpx, 1, nw, nh, Compression::Flate, false)?));
        }
    }
    let before = s.raw.len()
        + smask.as_ref().map_or(0, |(r, _)| match &*doc.get(*r) {
            Object::Stream(m) => m.raw.len(),
            _ => 0,
        });
    let after = new.raw.len() + smask.as_ref().map_or(0, |(_, m)| m.raw.len());
    (after < before).then_some((new, smask))
}

/// Whether colour space `cs` has one colour component, so a 1-bit image in it has one bit per
/// pixel: grey, a separation, an indexed space or a DeviceN of one colourant. Of names, only
/// DeviceGray counts, with `/G`, its abbreviation, which readers take as grey too: another name
/// is no colour space on its own (`/Indexed` and `/CalGray` need their parameters), so it can only
/// name a resource, which might be RGB.
fn one_component(doc: &Document, cs: &Object) -> bool {
    let cs = doc.resolve(cs);
    match &*cs {
        Object::Name(n) => matches!(n.as_slice(), b"DeviceGray" | b"G"),
        Object::Array(a) => match a.first().and_then(Object::as_name) {
            Some(b"CalGray" | b"Separation" | b"Indexed" | b"I") => true,
            Some(b"ICCBased") => a.get(1).is_some_and(|icc| matches!(&*doc.resolve(icc), Object::Stream(s) if s.dict.int(b"N") == Some(1))),
            Some(b"DeviceN") => a.get(1).is_some_and(|names| doc.resolve(names).as_array().is_some_and(|n| n.len() == 1)),
            _ => false,
        },
        _ => false,
    }
}

/// A pixel's samples as a 1 or a 0, or `None` for a colour that is neither.
type Pixel = fn(&[u8]) -> Option<bool>;

/// The samples of a black-and-white image, as rows of 1-bit samples each starting on a new byte,
/// with its width and height, and, when they were packed from more bits, its dictionary as a 1-bit
/// image. These count:
/// - one bit per pixel in one colour component, or an image mask, stored with no filter, with
///   general-purpose ones, or with CCITT fax last ([`Document::decoded_image_within`], no further
///   than its rows can need);
/// - with `pack`, and with no colour-key `/Mask` (its ranges are in the image's own values):
///   - eight-bit grey (one component, not indexed) whose every sample is 0 or 255. A decode array
///     maps 0 and 255 at eight bits as it maps 0 and 1 at one bit, so it stays;
///   - eight-bit `DeviceRGB` whose every pixel is black or white, which becomes `DeviceGray`. A
///     decode array the same for all three components becomes that one pair; another stops it.
///     Calibrated RGB and RGB with a profile are left out, as the profile could make them other
///     than black and white; so is one drawn where `/DefaultGray` or `/DefaultRGB` is or may be in
///     force (`defaults`), which would draw it otherwise as grey, and one whose soft mask has a
///     `/Matte` of three components;
///   - an indexed image of 2, 4 or 8 bits that uses only palette entries 0 and 1, which stay 0 and
///     1 at one bit, whatever their colours, so the colour space stays. A decode array other than
///     the default stops it: in index units it would mean something else at one bit.
fn one_bit(doc: &Document, s: &Stream, pack: bool, defaults: bool) -> Option<(Vec<u8>, u32, u32, Option<Dict>)> {
    let d = &s.dict;
    // Through u32 first, as everywhere here, so the product can't overflow.
    let (w, h) = (u32::try_from(d.int(b"Width")?).ok()?, u32::try_from(d.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let (wu, hu) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let stride = wu.div_ceil(8);
    let bits = d.int(b"BitsPerComponent");
    let mask = matches!(d.get(b"ImageMask"), Some(Object::Bool(true)));
    if (mask && bits.is_none_or(|b| b == 1)) || (bits == Some(1) && d.get(b"ColorSpace").is_some_and(|cs| one_component(doc, cs))) {
        // No further than its rows can need: a fax image's code (the filters before its decoder
        // produce it) takes at most about a byte a pixel and each row's end, as redaction allows.
        let len = stride.checked_mul(hu)?;
        let max = match s.filters().last() {
            Some((pdfcraft_filters::Filter::CcittFax, _)) => wu.checked_mul(hu)?.checked_add(hu.checked_mul(4)?)?.checked_add(4096)?,
            _ => allowance(len, h)?,
        };
        let mut data = doc.decoded_image_within(s, max).ok()?;
        if data.len() < len {
            return None;
        }
        data.truncate(len);
        return Some((data, w, h, None));
    }
    let colour_key = d.get(b"Mask").is_some_and(|m| doc.resolve(m).as_array().is_some());
    if !pack || mask || colour_key {
        return None;
    }
    let cs = d.get(b"ColorSpace")?;
    // The decode array's numbers, each given directly or by reference, as integers or reals; `None`
    // when it has something else.
    let decode = match d.get(b"Decode").map(|v| doc.resolve(v)).as_deref() {
        None => None,
        Some(Object::Array(v)) => Some(v.iter().map(|e| doc.resolve(e).as_f64()).collect::<Option<Vec<f64>>>()?),
        Some(_) => return None,
    };
    let mut packed = d.clone();
    packed.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    // Components a pixel, bits a component, and a pixel's samples as 1 or 0.
    let (n, bpc, white): (usize, usize, Pixel) = match &*doc.resolve(cs) {
        Object::Array(a) if matches!(a.first().and_then(Object::as_name), Some(b"Indexed" | b"I")) => {
            let bpc = bits.filter(|b| matches!(b, 2 | 4 | 8))?;
            match decode.as_deref() {
                None => {}
                Some([d0, d1]) if *d0 == 0.0 && *d1 == f64::from((1u32 << bpc) - 1) => {
                    packed.remove(b"Decode");
                }
                Some(_) => return None,
            }
            (1, usize::try_from(bpc).ok()?, |v| match v {
                [0] => Some(false),
                [1] => Some(true),
                _ => None,
            })
        }
        Object::Name(name) if matches!(name.as_slice(), b"DeviceRGB" | b"RGB") && bits == Some(8) => {
            let matte = |m: &Object| matches!(&*doc.resolve(m), Object::Stream(m) if m.dict.contains(b"Matte"));
            if defaults || d.get(b"SMask").is_some_and(matte) {
                return None;
            }
            match decode.as_deref() {
                None => {}
                Some(v @ [d0, d1, ..]) if v.len() == 6 && v.chunks(2).all(|pair| pair == [*d0, *d1]) => {
                    let number = |n: f64| if n.fract() == 0.0 { Object::Int(n as i64) } else { Object::Real(n) };
                    packed.set(b"Decode".to_vec(), Object::Array(vec![number(*d0), number(*d1)]));
                }
                Some(_) => return None,
            }
            packed.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
            (3, 8, |v| match v {
                [0, 0, 0] => Some(false),
                [255, 255, 255] => Some(true),
                _ => None,
            })
        }
        _ if bits == Some(8) && one_component(doc, cs) => (1, 8, |v| match v {
            [0] => Some(false),
            [255] => Some(true),
            _ => None,
        }),
        _ => return None,
    };
    let row_len = wu.checked_mul(n)?.checked_mul(bpc)?.div_ceil(8);
    // Through its parameters given by reference too: without a PNG predictor's tag bytes taken
    // off, rows of 0 and 255 (or palette entries 0 and 1) would still pass below, each shifted.
    let samples = doc.with_direct_parms(s).decoded_within(allowance(row_len.checked_mul(hu)?, h)?).ok()?;
    Some((packed_rows(&samples, row_len, wu, hu, (n, bpc, white))?, w, h, Some(packed)))
}

/// `h` rows of `w` pixels, each row `row_len` bytes of `n` samples a pixel at `bpc` bits (8, or
/// a divisor of 8 with `n` 1), as rows of 1-bit samples each starting on a new byte: 1 where
/// `white` says the pixel is white. `None` when a pixel is neither, `samples` is short, or the
/// rows have another shape: no pixels in a row, other sample sizes, or too few bytes for their
/// pixels.
fn packed_rows(samples: &[u8], row_len: usize, w: usize, h: usize, (n, bpc, white): (usize, usize, Pixel)) -> Option<Vec<u8>> {
    let stride = w.div_ceil(8);
    let shape = (n > 0 && bpc == 8) || (n == 1 && matches!(bpc, 1 | 2 | 4));
    if stride == 0 || !shape || row_len < w.checked_mul(n)?.checked_mul(bpc)?.div_ceil(8) {
        return None;
    }
    // From here no row is empty and x·n·bpc stays within one, and the output is no larger than
    // the samples it is read from.
    let samples = samples.get(..row_len.checked_mul(h)?)?;
    let mut out = vec![0u8; stride.checked_mul(h)?];
    for (row, out_row) in samples.chunks(row_len).zip(out.chunks_mut(stride)) {
        for x in 0..w {
            let on = if bpc == 8 {
                white(row.get(x * n..x * n + n)?)?
            } else {
                // Samples narrower than a byte, the first in its high bits; bpc divides 8.
                let at = x * bpc;
                white(&[(row.get(at / 8)? >> (8 - bpc - at % 8)) & ((1u8 << bpc) - 1)])?
            };
            if on {
                *out_row.get_mut(x / 8)? |= 0x80 >> (x % 8);
            }
        }
    }
    Some(out)
}

/// Resolve polarity only for explicitly understood samples. Indexed palettes and custom
/// colour spaces stay at their original resolution rather than guessing which entry is dark.
fn dark_is_one(doc: &Document, d: &Dict) -> Option<bool> {
    let number = |v: Option<&Object>| v.and_then(|v| doc.resolve(v).as_f64()).filter(|v| v.is_finite());
    let (d0, d1) = match d.get(b"Decode").map(|v| doc.resolve(v)).as_deref() {
        Some(Object::Array(a)) if a.len() == 2 => number(a.first()).zip(number(a.get(1)))?,
        None => (0.0, 1.0),
        _ => return None,
    };
    if d0 == d1 {
        return None;
    }
    if matches!(d.get(b"ImageMask"), Some(Object::Bool(true))) {
        return Some(d1 < d0);
    }
    match d.get(b"ColorSpace").map(|cs| doc.resolve(cs)).as_deref() {
        Some(Object::Name(n)) if matches!(n.as_slice(), b"DeviceGray" | b"G") => Some(d1 < d0),
        _ => None,
    }
}

/// `w` × `h` 1-bit rows (each starting on a new byte) averaged down to `nw` × `nh`, no larger:
/// each new pixel takes the sample that covers more of its area, and the dark one when the two tie
/// (1 if `dark_is_one`, see [`dark_is_one`]), so a dark line half a new pixel wide stays. `None`
/// when the rows are short or the new size isn't smaller in both directions or empty. Also says
/// how many new pixels cover both samples.
#[cfg(test)]
pub(crate) fn downsample_bits(data: &[u8], w: u32, h: u32, nw: u32, nh: u32, dark_is_one: bool) -> Option<(Vec<u8>, u64)> {
    downsample_bits_checked(data, w, h, nw, nh, dark_is_one).map(|(out, mixed, _)| (out, mixed))
}

fn downsample_bits_checked(data: &[u8], w: u32, h: u32, nw: u32, nh: u32, dark_is_one: bool) -> Option<(Vec<u8>, u64, bool)> {
    if nw == 0 || nh == 0 || nw > w || nh > h {
        return None;
    }
    let (stride, nstride) = (usize::try_from(w).ok()?.div_ceil(8), usize::try_from(nw).ok()?.div_ceil(8));
    let data = data.get(..stride.checked_mul(usize::try_from(h).ok()?)?)?;
    let mut out = vec![0u8; nstride.checked_mul(usize::try_from(nh).ok()?)?];
    // In units of 1/nw of a source column, source column x spans [x·nw, (x+1)·nw) and new column i
    // spans [i·w, (i+1)·w); rows likewise with h and nh. A new pixel's area is then w·h.
    let (w, h, nw, nh) = (u64::from(w), u64::from(h), u64::from(nw), u64::from(nh));
    let overlap = |a0: u64, a1: u64, b0: u64, b1: u64| a1.min(b1).saturating_sub(a0.max(b0));
    // Per source column, the area its 1 samples cover in the new row.
    let mut ones = vec![0u64; usize::try_from(w).ok()?];
    let mut mixed = 0u64;
    let mut previous_mixed = vec![false; usize::try_from(nw).ok()?];
    let mut local_tone = false;
    for j in 0..nh {
        let mut left_mixed = false;
        let mut above_left_mixed = false;
        ones.fill(0);
        let (y0, y1) = (j * h, (j + 1) * h);
        for y in y0 / nh..y1.div_ceil(nh).min(h) {
            let weight = overlap(y0, y1, y * nh, (y + 1) * nh);
            let row = data.get(usize::try_from(y).ok()? * stride..)?;
            for (x, sum) in ones.iter_mut().enumerate() {
                if row.get(x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0) {
                    *sum += weight;
                }
            }
        }
        let out_row = out.get_mut(usize::try_from(j).ok()? * nstride..)?;
        for i in 0..nw {
            let (x0, x1) = (i * w, (i + 1) * w);
            let covered: u64 = (x0 / nw..x1.div_ceil(nw).min(w))
                .map(|x| overlap(x0, x1, x * nw, (x + 1) * nw) * usize::try_from(x).ok().and_then(|x| ones.get(x)).copied().unwrap_or(0))
                .sum();
            // Against what the 0 samples cover: covered is at most w·h, which can't overflow
            // (w·h < 2^64 for u32 sizes).
            let zeros = w * h - covered;
            let is_mixed = covered > 0 && zeros > 0;
            mixed += u64::from(is_mixed);
            let column = usize::try_from(i).ok()?;
            let above = *previous_mixed.get(column)?;
            local_tone |= is_mixed && left_mixed && above && above_left_mixed;
            *previous_mixed.get_mut(column)? = is_mixed;
            left_mixed = is_mixed;
            above_left_mixed = above;
            if covered > zeros || (dark_is_one && covered == zeros) {
                let i = usize::try_from(i).ok()?;
                *out_row.get_mut(i / 8)? |= 0x80 >> (i % 8);
            }
        }
    }
    Some((out, mixed, local_tone))
}

/// A black-and-white image (see [`one_bit`]) resampled and stored again per `settings`, if that is
/// smaller as stored; `ppi` is its effective resolution, and `defaults` says it is drawn where
/// `/DefaultGray` or `/DefaultRGB` is or may be in force. Also says whether it was resampled.
///
/// - **Resampled** ([`downsample_bits_checked`]) when its resolution is above `above_ppi`, to `target_ppi`,
///   unless it has a soft mask, whose own size would then have to follow, or is a halftone or a
///   dither: most of its new pixels would cover both black and white, and taking the sample that
///   covers more would turn its tones solid (a 50% screen black, a 25% one white).
/// - **Stored** as CCITT Group 4 with `CcittG4`, or as 1-bit Flate when that is smaller, which only
///   a resampled image or one packed from more bits may be: a 1-bit image that is only stored again
///   becomes Group 4 or stays. Group 4 alone is only decoded to be resampled; Group 3 is stored
///   again like any other. With `Retain`, a resampled image keeps its kind of compression
///   (CCITT, or Flate), and nothing else changes: an image of more bits stays as it is, for the
///   grey or colour settings.
///
/// The samples keep their meaning, so the colour space, decode array and `/Mask` stay, but for
/// black-and-white RGB, which becomes grey with one decode pair, and an indexed image's default
/// decode array, which goes as the default at one bit.
fn mono(doc: &Document, s: &Stream, ppi: f64, defaults: bool, settings: &MonoSettings) -> Option<(Stream, bool)> {
    let d = &s.dict;
    let downsample = settings.downsample && ppi.is_finite() && ppi > settings.above_ppi && settings.target_ppi > 0.0 && !d.contains(b"SMask");
    let retain = settings.compression == MonoCompression::Retain;
    let fax = doc.fax_params(s);
    // Its kind of compression, which Retain keeps, behind any other filters.
    let ccitt = fax.is_some();
    // Group 4 alone is only worth decoding to resample it. Group 3, or Group 4 behind
    // ASCII85Decode, say, can be smaller stored again as Group 4 alone.
    let g4_alone = fax.is_some_and(|p| p.k < 0) && s.filters().len() == 1;
    if (retain || (g4_alone && settings.compression == MonoCompression::CcittG4)) && !downsample {
        return None;
    }
    let (data, w, h, packed) = one_bit(doc, s, !retain, defaults)?;
    let packed_from_more = packed.is_some();
    let k = settings.target_ppi / ppi;
    // `downsample_bits` keeps a counter for each column: no more columns than a fax row may have,
    // so a hostile strip of 80 million can't make it 640 MB.
    let narrow = w <= pdfcraft_filters::MAX_CCITT_COLUMNS;
    // At most `w` × `h`, as k < 1: no overflow.
    let (nw, nh) = if downsample && narrow && k < 1.0 {
        (((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1))
    } else {
        (w, h)
    };
    let polarity = if defaults { None } else { dark_is_one(doc, packed.as_ref().unwrap_or(d)) };
    let averaged = if (nw, nh) != (w, h) && polarity.is_some() {
        // The dark sample of the image as it ends up: packed RGB is grey, with grey's decode pair.
        let (out, mixed, local_tone) = downsample_bits_checked(&data, w, h, nw, nh, polarity?)?;
        // A halftone or dither: most new pixels cover both samples. Text covers both only along
        // its edges.
        // A dense 2x2 patch of mixed output samples can encode a local photo even when
        // white margins dominate the page. Conservatively retain the original image.
        (!local_tone && mixed.saturating_mul(2) <= u64::from(nw) * u64::from(nh)).then_some(out)
    } else {
        None
    };
    let resampled = averaged.is_some();
    let (data, nw, nh) = match averaged {
        Some(out) => (out, nw, nh),
        None => (data, w, h),
    };
    if (retain || (g4_alone && settings.compression == MonoCompression::CcittG4)) && !resampled {
        return None;
    }
    let mut dict = packed.unwrap_or_else(|| d.clone());
    for key in [&b"Filter"[..], b"DecodeParms", b"Length", b"DL"] {
        dict.remove(key);
    }
    dict.set(b"Width".to_vec(), Object::Int(i64::from(nw)));
    dict.set(b"Height".to_vec(), Object::Int(i64::from(nh)));
    let g4 = if settings.compression == MonoCompression::CcittG4 || (retain && ccitt) {
        pdfcraft_filters::encode_g4(&data, nw, nh).ok().map(|coded| {
            let mut dict = dict.clone();
            let mut parms = Dict::new();
            parms.set(b"K".to_vec(), Object::Int(-1));
            parms.set(b"Columns".to_vec(), Object::Int(i64::from(nw)));
            parms.set(b"Rows".to_vec(), Object::Int(i64::from(nh)));
            dict.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
            dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
            Stream::from_raw(dict, coded)
        })
    } else {
        None
    };
    let jbig = if settings.compression == MonoCompression::Jbig2 {
        pdfcraft_filters::encode_jbig2(&data, nw, nh).ok().map(|coded| {
            let mut dict = dict.clone();
            dict.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
            Stream::from_raw(dict, coded)
        })
    } else {
        None
    };
    let flate = ((resampled || packed_from_more) && !(retain && ccitt)).then(|| Stream::flate(dict, &data));
    // Smaller as stored, counting the dictionary: a tiny image can lose by Group 4's parameters.
    let best = [g4, jbig, flate].into_iter().flatten().filter_map(|c| Some((crate::stored_len(&c)?, c))).min_by_key(|(len, _)| *len)?;
    (best.0 < crate::stored_len(s)?).then_some((best.1, resampled))
}

pub(crate) fn run(
    doc: &mut Document,
    pages: &[ObjRef],
    settings: &Settings,
    report: &mut Report,
    progress: &mut dyn FnMut(Stage) -> bool,
) -> Result<(), OptimizeError> {
    let (ppi, mut defaults, read) = walk_pages(doc, pages);
    // Black-and-white RGB becomes grey, which default colour spaces would draw otherwise: one used
    // where the walk didn't look may be drawn so.
    let packs = settings.mono.compression != MonoCompression::Retain;
    let rgb: HashSet<u32> = ppi.keys().filter(|r| packs && device_rgb(doc, **r)).map(|r| r.num).collect();
    defaults.extend(used_elsewhere(doc, &rgb, &read));
    // Soft masks are processed with their image, not on their own.
    let masks: HashSet<ObjRef> = ppi
        .keys()
        .filter_map(|r| match &*doc.get(*r) {
            Object::Stream(s) => s.dict.reference(b"SMask"),
            _ => None,
        })
        .collect();
    let mut refs: Vec<(ObjRef, f64)> = ppi.into_iter().filter(|(r, _)| !masks.contains(r)).collect();
    refs.sort_by_key(|(r, _)| r.num);
    let total = refs.len();
    let mut jbig2 = false;
    for (done, (r, ppi)) in refs.into_iter().enumerate() {
        if !progress(Stage::Images { done, total }) {
            return Err(OptimizeError::Cancelled);
        }
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        report.images += 1;
        // Packed from more bits a sample, it is lossless at one bit, but the grey or colour
        // settings can make it smaller still (below).
        let packed_from_more = s.dict.int(b"BitsPerComponent").is_some_and(|b| b > 1);
        let (packed, mono) = match mono(doc, s, ppi, defaults.contains(&r.num), &settings.mono) {
            Some(m) if packed_from_more => (Some(m), None),
            m => (None, m),
        };
        if let Some((new, resampled)) = mono {
            report.image_bytes_before += s.raw.len();
            report.image_bytes_after += new.raw.len();
            if resampled {
                report.images_resampled += 1;
            } else {
                report.images_recompressed += 1;
            }
            jbig2 |= new.dict.name(b"Filter") == Some(b"JBIG2Decode");
            doc.set(r, Object::Stream(new));
            continue;
        }
        let n = s.dict.get(b"ColorSpace").and_then(|cs| components(doc, cs));
        let processed = n.and_then(|n| process(doc, s, ppi, if n == 1 { &settings.gray } else { &settings.color }, n));
        // A dithered scan, say, downsampled and JPEG-compressed: the smaller wins, counting a soft
        // mask resampled with it.
        let smask_len = |smask: Option<&(ObjRef, Stream)>| match smask {
            Some((_, m)) => Some(m.raw.len()),
            None => s.dict.reference(b"SMask").map(|m| match &*doc.get(m) {
                Object::Stream(m) => m.raw.len(),
                _ => 0,
            }),
        };
        let total = |new: &Stream, smask: Option<&(ObjRef, Stream)>| Some(crate::stored_len(new)?.saturating_add(smask_len(smask).unwrap_or(0)));
        let packed_wins = packed.as_ref().is_some_and(|(p, _)| {
            processed.as_ref().is_none_or(|(new, smask)| total(p, None).zip(total(new, smask.as_ref())).is_some_and(|(a, b)| a <= b))
        });
        if let Some((new, resampled)) = packed.filter(|_| packed_wins) {
            jbig2 |= new.dict.name(b"Filter") == Some(b"JBIG2Decode");
            report.image_bytes_before += s.raw.len();
            report.image_bytes_after += new.raw.len();
            if resampled {
                report.images_resampled += 1;
            } else {
                report.images_recompressed += 1;
            }
            doc.set(r, Object::Stream(new));
            continue;
        }
        let Some((new, smask)) = processed else { continue };
        let resized = new.dict.int(b"Width") != s.dict.int(b"Width") || new.dict.int(b"Height") != s.dict.int(b"Height");
        report.image_bytes_before += s.raw.len();
        report.image_bytes_after += new.raw.len();
        if resized {
            report.images_resampled += 1;
        } else {
            report.images_recompressed += 1;
        }
        doc.set(r, Object::Stream(new));
        if let Some((mr, m)) = smask {
            doc.set(mr, Object::Stream(m));
        }
    }
    if !progress(Stage::Images { done: total, total }) {
        return Err(OptimizeError::Cancelled);
    }
    if jbig2 {
        at_least_pdf_1_4(doc)?;
    }
    Ok(())
}

/// JBIG2Decode is PDF 1.4: a document whose header says less, and whose catalog doesn't say 1.4
/// or more already (directly or by reference), gets `/Version /1.4` in its catalog (ISO 32000-2
/// §7.7.2). A full save with object streams writes a 1.5 header anyway.
fn at_least_pdf_1_4(doc: &mut Document) -> Result<(), OptimizeError> {
    let below = |v: &[u8]| v.len() < 3 || v < &b"1.4"[..];
    let Some(root) = doc.root() else { return Ok(()) };
    let version = doc.get(root).as_dict().and_then(|c| c.get(b"Version").cloned());
    let catalog = version.and_then(|v| doc.resolve(&v).as_name().map(<[u8]>::to_vec));
    if below(doc.version().as_bytes()) && catalog.as_deref().is_none_or(below) {
        doc.update_dict(root, |c| c.set(b"Version".to_vec(), Object::name("1.4")))?;
    }
    Ok(())
}

#[cfg(test)]
mod dimension_tests {
    use super::*;

    #[test]
    fn pixel_reader_rejects_hostile_dimensions_before_decoding() {
        for (width, height, channels) in [(-1, 1, 1), (0, 1, 1), (i64::from(u32::MAX) + 101, 1, 1), (80_000_001, 1, 1), (2, 2, usize::MAX)] {
            let mut dict = Dict::new();
            dict.set(b"Width".to_vec(), Object::Int(width));
            dict.set(b"Height".to_vec(), Object::Int(height));
            let stream = Stream::from_raw(dict, vec![0; 4]);
            assert!(pixels(&Document::new_empty(), &stream, channels).is_none(), "{width} x {height} x {channels}");
        }
        let mut dict = Dict::new();
        dict.set(b"Width".to_vec(), Object::Int(2));
        dict.set(b"Height".to_vec(), Object::Int(2));
        let stream = Stream::from_raw(dict, vec![1, 2, 3, 4, 5]);
        assert_eq!(pixels(&Document::new_empty(), &stream, 1), Some(vec![1, 2, 3, 4]));
    }

    /// A JPEG larger than its dictionary says is refused from its header, before its pixels are decoded.
    #[test]
    fn a_jpeg_of_another_size_than_its_dictionary_is_not_decoded() {
        let data = jpeg(&vec![128; 1000 * 1000], 1, 1000, 1000, 50).unwrap_or_default();
        let stream = |w: i64, h: i64| {
            let mut dict = Dict::new();
            dict.set(b"Width".to_vec(), Object::Int(w));
            dict.set(b"Height".to_vec(), Object::Int(h));
            dict.set(b"Filter".to_vec(), Object::name("DCTDecode"));
            Stream::from_raw(dict, data.clone())
        };
        DECODED_PIXELS.with(|c| c.set(0));
        assert!(pixels(&Document::new_empty(), &stream(10, 10), 1).is_none());
        assert_eq!(DECODED_PIXELS.with(|c| c.get()), 0, "pixels decoded");
        assert_eq!(pixels(&Document::new_empty(), &stream(1000, 1000), 1).map(|p| p.len()), Some(1000 * 1000));
    }
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{DecodeBudget, Dict, Document, MAX_PAGE_DECODED, Object, Stream};

    #[test]
    fn repeated_content_references_share_one_decode_budget() {
        let mut doc = Document::new_empty();
        let stream = doc.add(Stream::flate(Dict::new(), &vec![b' '; 1 << 20]));
        let contents = Object::Array(vec![Object::Ref(stream); 300]);
        let data = super::content_bytes(&doc, &contents, &mut DecodeBudget::page());
        assert!(data.is_none_or(|d| d.len() <= MAX_PAGE_DECODED + 300));
    }
}

#[cfg(test)]
mod downsample_tests {
    use super::{dark_is_one, downsample_bits};
    use pdfcraft_cos::{Dict, Document, Object};

    /// [`downsample_bits`] the slow way: each new pixel's area against every source pixel's.
    fn naive(data: &[u8], w: u32, h: u32, nw: u32, nh: u32, dark_one: bool) -> Vec<u8> {
        let (stride, nstride) = ((w as usize).div_ceil(8), (nw as usize).div_ceil(8));
        let (w, h, nw, nh) = (u64::from(w), u64::from(h), u64::from(nw), u64::from(nh));
        let mut out = vec![0u8; nstride * nh as usize];
        for j in 0..nh {
            for i in 0..nw {
                let mut ones = 0;
                for y in 0..h {
                    for x in 0..w {
                        let across = ((i + 1) * w).min((x + 1) * nw).saturating_sub((i * w).max(x * nw));
                        let down = ((j + 1) * h).min((y + 1) * nh).saturating_sub((j * h).max(y * nh));
                        if data[y as usize * stride + x as usize / 8] & (0x80 >> (x % 8)) != 0 {
                            ones += across * down;
                        }
                    }
                }
                if 2 * ones > w * h || (dark_one && 2 * ones == w * h) {
                    out[j as usize * nstride + i as usize / 8] |= 0x80 >> (i % 8);
                }
            }
        }
        out
    }

    #[test]
    fn each_new_pixel_takes_the_sample_covering_more_of_it() {
        // 4 × 2 to 2 × 1: three 1s of four on the left, none on the right.
        assert_eq!(downsample_bits(&[0b1100_0000, 0b1000_0000], 4, 2, 2, 1, false).map(|(out, _)| out), Some(vec![0b1000_0000]));
        // A tie gives the dark sample: 0, black in grey, so a black line half a new pixel wide
        // stays (the second of four white columns is black); a white one that thin on black doesn't.
        assert_eq!(downsample_bits(&[0b1011_0000, 0b1011_0000], 4, 2, 2, 1, false).map(|(out, _)| out), Some(vec![0b0100_0000]));
        assert_eq!(downsample_bits(&[0b0100_0000, 0b0100_0000], 4, 2, 2, 1, false).map(|(out, _)| out), Some(vec![0]));
        // With 1 the dark sample (an inverting decode array, a separation), that line is a 1.
        assert_eq!(downsample_bits(&[0b0100_0000, 0b0100_0000], 4, 2, 2, 1, true).map(|(out, _)| out), Some(vec![0b1000_0000]));
        assert_eq!(downsample_bits(&[0b1011_0000, 0b1011_0000], 4, 2, 2, 1, true).map(|(out, _)| out), Some(vec![0b1100_0000]));
        // The same size gives the same bits; padding bits are 0.
        assert_eq!(downsample_bits(&[0b1010_1111], 3, 1, 3, 1, false).map(|(out, _)| out), Some(vec![0b1010_0000]));
        // 3 to 2 across: the middle column counts half to each new one.
        assert_eq!(downsample_bits(&[0b1100_0000], 3, 1, 2, 1, false).map(|(out, _)| out), Some(vec![0b1000_0000]));
    }

    /// The dark sample: 0 in grey and of an image mask, 1 in a separation, each the other way round
    /// with an inverting decode array; 0 for an indexed image, whatever its decode array.
    #[test]
    fn the_dark_sample_follows_the_colour_space_and_decode_array() {
        let doc = Document::new_empty();
        let image = |entries: &[(&str, Object)]| {
            let mut d = Dict::new();
            for (k, v) in entries {
                d.set(k.as_bytes().to_vec(), v.clone());
            }
            d
        };
        let inverted = ("Decode", Object::Array(vec![1.into(), 0.into()]));
        let grey = ("ColorSpace", Object::name("DeviceGray"));
        let mask = ("ImageMask", Object::Bool(true));
        let separation =
            ("ColorSpace", Object::Array(vec![Object::name("Separation"), Object::name("Spot"), Object::name("DeviceCMYK"), Object::Null]));
        let indexed = ("ColorSpace", Object::Array(vec![Object::name("Indexed"), Object::name("DeviceGray"), 1.into(), Object::Null]));
        for (entries, dark_one) in [
            (vec![grey.clone()], Some(false)),
            (vec![grey, inverted.clone()], Some(true)),
            (vec![mask.clone()], Some(false)),
            (vec![mask, inverted.clone()], Some(true)),
            (vec![separation.clone()], None),
            (vec![separation, inverted.clone()], None),
            (vec![indexed.clone()], None),
            (vec![indexed, inverted], None),
            (vec![("ColorSpace", Object::name("DeviceGray")), ("Decode", Object::Array(vec![Object::name("x"), 0.into()]))], None),
        ] {
            assert_eq!(dark_is_one(&doc, &image(&entries)), dark_one, "{entries:?}");
        }
    }

    #[test]
    fn what_it_cannot_resample_is_none() {
        assert_eq!(downsample_bits(&[0; 2], 4, 2, 5, 1, false).map(|(out, _)| out), None, "wider");
        assert_eq!(downsample_bits(&[0; 2], 4, 2, 2, 3, false).map(|(out, _)| out), None, "taller");
        assert_eq!(downsample_bits(&[0; 2], 4, 2, 0, 1, false).map(|(out, _)| out), None);
        assert_eq!(downsample_bits(&[0; 1], 4, 2, 2, 1, false).map(|(out, _)| out), None, "short rows");
        assert_eq!(downsample_bits(&[], u32::MAX, u32::MAX, 1, 1, true).map(|(out, _)| out), None);
    }

    proptest::proptest! {
        /// Any bitmap, to any size no larger, with either sample the dark one, gives what the slow
        /// way gives; all 1s stay all 1s.
        #[test]
        fn it_matches_the_slow_way(
            w in 1u32..40,
            h in 1u32..16,
            nw_part in 0.0f64..1.0,
            nh_part in 0.0f64..1.0,
            seed in proptest::collection::vec(0u8..=255, 1..64),
            dark_one: bool,
        ) {
            let (nw, nh) = (1 + (f64::from(w - 1) * nw_part) as u32, 1 + (f64::from(h - 1) * nh_part) as u32);
            let len = (w as usize).div_ceil(8) * h as usize;
            let data: Vec<u8> = seed.iter().cycle().take(len).copied().collect();
            proptest::prop_assert_eq!(downsample_bits(&data, w, h, nw, nh, dark_one).map(|(out, _)| out), Some(naive(&data, w, h, nw, nh, dark_one)));
            let white = downsample_bits(&vec![0xFF; len], w, h, nw, nh, dark_one).map(|(out, _)| out).unwrap();
            proptest::prop_assert_eq!(white, naive(&vec![0xFF; len], w, h, nw, nh, dark_one));
            proptest::prop_assert!(naive(&vec![0xFF; len], w, h, nw, nh, dark_one).iter().any(|b| *b != 0));
        }
    }
}

#[cfg(test)]
mod pack_tests {
    use super::{Pixel, packed_rows};

    /// Rows of one pixel of one bit, 1 white: each packed as it is.
    const WHITE_IS_1: Pixel = |v| Some(v == [1]);

    /// [`packed_rows`] packs rows it can read whole, and refuses any other shape, without reading
    /// past a row or allocating more than its samples: no pixels in a row, no components, samples
    /// of 3 bits, several components under 8 bits, rows shorter than their pixels, and a width far
    /// past the samples.
    #[test]
    fn packed_rows_refuses_rows_it_cannot_read_whole() {
        let samples = [0x80, 0x00, 0x80, 0x00];
        assert_eq!(packed_rows(&samples, 1, 1, 4, (1, 1, WHITE_IS_1)), Some(vec![0x80, 0x00, 0x80, 0x00]));
        for (row_len, w, n, bpc) in [(1, 0, 1, 1), (1, 1, 0, 8), (1, 1, 1, 3), (1, 1, 2, 4), (1, 9, 1, 1), (1, usize::MAX / 8, 1, 1)] {
            assert_eq!(packed_rows(&samples, row_len, w, 4, (n, bpc, WHITE_IS_1)), None, "row of {row_len} for {w} × {n} × {bpc}");
        }
    }
}
