//! Image resampling and recompression.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Matrix, Pieces};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::{Compression, ImageSettings, MonoCompression, MonoSettings, OptimizeError, Report, Settings, Stage};

/// Images with more pixels than this are left alone (memory).
const MAX_PIXELS: u64 = 80_000_000;

/// The resources of a page, inherited from the page tree if the page has none.
pub(crate) fn page_resources(doc: &Document, page: ObjRef) -> Option<Dict> {
    let mut node = Some(page);
    for _ in 0..64 {
        let d = doc.get(node?).as_dict()?.clone();
        if let Some(r) = d.get(b"Resources") {
            return doc.resolve(r).as_dict().cloned();
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
    /// Forms with their own resources already walked, by the linear part of their matrix. Drawing
    /// one again at the same scale (only moved) can't change any image's resolution.
    walked: HashSet<(ObjRef, [u64; 4])>,
    /// Forms walked at least once.
    seen: HashSet<ObjRef>,
    rewalks: usize,
    /// Decoded bytes left for the page being walked.
    budget: DecodeBudget,
    /// False once a use may have been missed (depth or work limit).
    complete: bool,
    out: HashMap<ObjRef, f64>,
}

impl Walk<'_> {
    /// Walk one content stream, recording each image's displayed resolution (pixels per inch, the
    /// smaller of the two axes) and descending into form XObjects.
    fn content(&mut self, data: &[u8], resources: &Dict, ctm: Matrix) {
        let doc = self.doc;
        let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
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
                        }
                        Some(b"Form") => self.form(r, s, resources, top),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Descend into form `r` drawn with `ctm` from a stream whose resources are `resources`.
    fn form(&mut self, r: ObjRef, s: &Stream, resources: &Dict, ctm: Matrix) {
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
            if !self.walked.insert((r, [a.to_bits(), b.to_bits(), c.to_bits(), d.to_bits()])) {
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
        self.content(&data, own.as_ref().unwrap_or(resources), inner);
        self.path.pop();
    }
}

/// The effective resolution (ppi) of every image drawn by `pages`: the smallest over its uses.
///
/// If forms nest too deeply, are drawn too many times, or a page's content doesn't decode within
/// its budget, every image is given `INFINITY` (never resampled), since its largest use may be
/// among those missed.
pub fn effective_resolutions(doc: &Document, pages: &[ObjRef]) -> HashMap<ObjRef, f64> {
    let mut walk = Walk {
        doc,
        path: Vec::new(),
        walked: HashSet::new(),
        seen: HashSet::new(),
        rewalks: 0,
        budget: DecodeBudget::page(),
        complete: true,
        out: HashMap::new(),
    };
    for p in pages {
        let Some(d) = doc.get(*p).as_dict().cloned() else { continue };
        let Some(contents) = d.get(b"Contents") else { continue };
        let res = page_resources(doc, *p).unwrap_or_default();
        walk.budget = DecodeBudget::page();
        let Some(data) = content_bytes(doc, contents, &mut walk.budget) else {
            walk.complete = false;
            continue;
        };
        walk.content(&data, &res, Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    }
    if !walk.complete {
        walk.out.values_mut().for_each(|v| *v = f64::INFINITY);
    }
    walk.out
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
/// every row, and the image would be written back wrong.
pub(crate) fn pixels(doc: &Document, s: &Stream, n: usize) -> Option<Vec<u8>> {
    let (w, h) = (u32::try_from(s.dict.int(b"Width")?).ok()?, u32::try_from(s.dict.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let bytes = usize::try_from(w).ok()?.checked_mul(usize::try_from(h).ok()?)?.checked_mul(n)?;
    if is_dct(s)? {
        use image::ImageDecoder as _;
        // The JPEG's own size is what gets decoded: one other than the dictionary's is refused from
        // its header, before any pixels are.
        let decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&s.raw[..])).ok()?;
        if decoder.dimensions() != (w, h) {
            return None;
        }
        let img = image::DynamicImage::from_decoder(decoder).ok()?;
        count_decoded(u64::from(img.width()) * u64::from(img.height()));
        // Adobe CMYK/YCCK JPEGs aren't handled (the colour space check excludes them).
        let v = if n == 1 { img.into_luma8().into_raw() } else { img.into_rgb8().into_raw() };
        (v.len() == bytes).then_some(v)
    } else {
        let v = doc.with_direct_parms(s).decoded().ok()?;
        Some(v.get(..bytes)?.to_vec())
    }
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

/// Whether [`pixels`] reads `s` as it is drawn: a codec it decodes and no decode array that
/// changes anything. `None` when the colour space isn't grey or RGB.
pub(crate) fn decodable(doc: &Document, s: &Stream) -> Option<bool> {
    let n = components(doc, s.dict.get(b"ColorSpace")?)?;
    Some(is_dct(s).is_some() && s.dict.get(b"Decode").is_none_or(|d| is_identity_decode(doc, d, n)))
}

/// Whether `decode` is the default decode array for `n` components ([0 1] each), which changes
/// nothing. Scanners write it on page images (#490).
fn is_identity_decode(doc: &Document, decode: &Object, n: usize) -> bool {
    let decode = doc.resolve(decode);
    let Some(a) = decode.as_array() else { return false };
    a.len() == 2 * n && a.chunks(2).all(|p| matches!(p, [lo, hi] if doc.resolve(lo).as_f64() == Some(0.0) && doc.resolve(hi).as_f64() == Some(1.0)))
}

/// A new image stream from 8-bit samples: the original dictionary with the new size and filter.
/// A decode array is dropped: only images without one, or with the default one, get here.
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
/// DeviceGray counts: another name is no colour space on its own (`/Indexed` and `/CalGray` need
/// their parameters), so it can only name a resource, which might be RGB.
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
///   general-purpose ones, or with CCITT fax last ([`Document::decoded_image`]);
/// - with `pack`, and with no colour-key `/Mask` (its ranges are in the image's own values):
///   - eight-bit grey (one component, not indexed) whose every sample is 0 or 255. A decode array
///     maps 0 and 255 at eight bits as it maps 0 and 1 at one bit, so it stays;
///   - eight-bit `DeviceRGB` whose every pixel is black or white, which becomes `DeviceGray`. A
///     decode array the same for all three components becomes that one pair; another stops it.
///     Calibrated RGB and RGB with a profile are left out, as the profile could make them other
///     than black and white;
///   - an indexed image of 2, 4 or 8 bits that uses only palette entries 0 and 1, which stay 0 and
///     1 at one bit, whatever their colours, so the colour space stays. A decode array other than
///     the default stops it: in index units it would mean something else at one bit.
fn one_bit(doc: &Document, s: &Stream, pack: bool) -> Option<(Vec<u8>, u32, u32, Option<Dict>)> {
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
        let mut data = doc.decoded_image(s).ok()?;
        let len = stride.checked_mul(hu)?;
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
    let decode = d.get(b"Decode").map(|v| doc.resolve(v));
    let mut packed = d.clone();
    packed.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    // Components a pixel, bits a component, and a pixel's samples as 1 or 0.
    let (n, bpc, white): (usize, usize, Pixel) = match &*doc.resolve(cs) {
        Object::Array(a) if matches!(a.first().and_then(Object::as_name), Some(b"Indexed" | b"I")) => {
            let bpc = bits.filter(|b| matches!(b, 2 | 4 | 8))?;
            match decode.as_deref() {
                None => {}
                Some(Object::Array(v)) if v[..] == [Object::Int(0), Object::Int((1 << bpc) - 1)] => {
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
            match decode.as_deref() {
                None => {}
                Some(Object::Array(v)) if v.len() == 6 && v.chunks(2).all(|pair| Some(pair) == v.get(..2)) => {
                    packed.set(b"Decode".to_vec(), Object::Array(v.get(..2)?.to_vec()));
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
    let samples = doc.with_direct_parms(s).decoded().ok()?;
    let samples = samples.get(..row_len.checked_mul(hu)?)?;
    let mut out = vec![0u8; stride.checked_mul(hu)?];
    // Neither length is 0, as `w` isn't, and x·n·bpc stays within a row, whose size was checked.
    for (row, out_row) in samples.chunks(row_len).zip(out.chunks_mut(stride)) {
        for x in 0..wu {
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
    Some((out, w, h, Some(packed)))
}

/// Whether sample 1 is the dark one of the 1-bit image `d` describes, after its decode array
/// (`[0 1]` when it has none of two numbers): the painted part of an image mask and black in grey
/// decode to 0, and full ink in a separation (or a DeviceN of one colourant) to 1. An indexed image
/// says 0, as its palette, not its decode array, says which entry is darker.
fn dark_is_one(doc: &Document, d: &Dict) -> bool {
    let number = |v: Option<&Object>| v.and_then(|v| doc.resolve(v).as_f64());
    let (d0, d1) = match d.get(b"Decode").map(|v| doc.resolve(v)).as_deref() {
        Some(Object::Array(a)) => number(a.first()).zip(number(a.get(1))).unwrap_or((0.0, 1.0)),
        _ => (0.0, 1.0),
    };
    if matches!(d.get(b"ImageMask"), Some(Object::Bool(true))) {
        return d1 < d0;
    }
    match d.get(b"ColorSpace").map(|cs| doc.resolve(cs)).as_deref() {
        Some(Object::Array(a)) => match a.first().and_then(Object::as_name) {
            Some(b"Indexed" | b"I") => false,
            Some(b"Separation" | b"DeviceN") => d1 > d0,
            _ => d1 < d0,
        },
        _ => d1 < d0,
    }
}

/// `w` × `h` 1-bit rows (each starting on a new byte) averaged down to `nw` × `nh`, no larger:
/// each new pixel takes the sample that covers more of its area, and the dark one when the two tie
/// (1 if `dark_is_one`, see [`dark_is_one`]), so a dark line half a new pixel wide stays. `None`
/// when the rows are short or the new size isn't smaller in both directions or empty.
pub(crate) fn downsample_bits(data: &[u8], w: u32, h: u32, nw: u32, nh: u32, dark_is_one: bool) -> Option<Vec<u8>> {
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
    for j in 0..nh {
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
            if covered > zeros || (dark_is_one && covered == zeros) {
                let i = usize::try_from(i).ok()?;
                *out_row.get_mut(i / 8)? |= 0x80 >> (i % 8);
            }
        }
    }
    Some(out)
}

/// A black-and-white image (see [`one_bit`]) resampled and stored again per `settings`, if that is
/// smaller as stored; `ppi` is its effective resolution. Also says whether it was resampled.
///
/// - **Resampled** ([`downsample_bits`]) when its resolution is above `above_ppi`, to `target_ppi`,
///   unless it has a soft mask, whose own size would then have to follow.
/// - **Stored** as CCITT Group 4 with `CcittG4`, or as 1-bit Flate when that is smaller, which only
///   a resampled image or one packed from more bits may be: a 1-bit image that is only stored again
///   becomes Group 4 or stays. Group 4 alone is only decoded to be resampled; Group 3 is stored
///   again like any other. With `Retain`, a resampled image keeps its kind of compression
///   (CCITT, or Flate), and nothing else changes: an image of more bits stays as it is, for the
///   grey or colour settings.
///
/// The samples keep their meaning, so the decode array and `/Mask` stay as they are, and the
/// colour space too, but for black-and-white RGB, which becomes grey.
fn mono(doc: &Document, s: &Stream, ppi: f64, settings: &MonoSettings) -> Option<(Stream, bool)> {
    let d = &s.dict;
    let downsample = settings.downsample && ppi.is_finite() && ppi > settings.above_ppi && settings.target_ppi > 0.0 && !d.contains(b"SMask");
    let retain = settings.compression == MonoCompression::Retain;
    let fax = doc.fax_params(s);
    // Its kind of compression, which Retain keeps, behind any other filters.
    let ccitt = fax.is_some();
    // Group 4 alone is only worth decoding to resample it. Group 3, or Group 4 behind
    // ASCII85Decode, say, can be smaller stored again as Group 4 alone.
    let g4_alone = fax.is_some_and(|p| p.k < 0) && s.filters().len() == 1;
    if (retain || g4_alone) && !downsample {
        return None;
    }
    let (data, w, h, packed) = one_bit(doc, s, !retain)?;
    let packed_from_more = packed.is_some();
    let k = settings.target_ppi / ppi;
    // At most `w` × `h`, as k < 1: no overflow.
    let (nw, nh) =
        if downsample && k < 1.0 { (((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1)) } else { (w, h) };
    let resampled = (nw, nh) != (w, h);
    if (retain || g4_alone) && !resampled {
        return None;
    }
    // The dark sample of the image as it ends up: packed RGB is grey, with grey's decode pair.
    let dark_one = dark_is_one(doc, packed.as_ref().unwrap_or(d));
    let data = if resampled { downsample_bits(&data, w, h, nw, nh, dark_one)? } else { data };
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
    let flate = ((resampled || packed_from_more) && !(retain && ccitt)).then(|| Stream::flate(dict, &data));
    // Smaller as stored, counting the dictionary: a tiny image can lose by Group 4's parameters.
    let best = [g4, flate].into_iter().flatten().filter_map(|c| Some((crate::stored_len(&c)?, c))).min_by_key(|(len, _)| *len)?;
    (best.0 < crate::stored_len(s)?).then_some((best.1, resampled))
}

pub(crate) fn run(
    doc: &mut Document,
    pages: &[ObjRef],
    settings: &Settings,
    report: &mut Report,
    progress: &mut dyn FnMut(Stage) -> bool,
) -> Result<(), OptimizeError> {
    let ppi = effective_resolutions(doc, pages);
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
    for (done, (r, ppi)) in refs.into_iter().enumerate() {
        if !progress(Stage::Images { done, total }) {
            return Err(OptimizeError::Cancelled);
        }
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        report.images += 1;
        if let Some((new, resampled)) = mono(doc, s, ppi, &settings.mono) {
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
        let Some(n) = s.dict.get(b"ColorSpace").and_then(|cs| components(doc, cs)) else { continue };
        let st = if n == 1 { &settings.gray } else { &settings.color };
        let Some((new, smask)) = process(doc, s, ppi, st, n) else { continue };
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
        assert_eq!(downsample_bits(&[0b1100_0000, 0b1000_0000], 4, 2, 2, 1, false), Some(vec![0b1000_0000]));
        // A tie gives the dark sample: 0, black in grey, so a black line half a new pixel wide
        // stays (the second of four white columns is black); a white one that thin on black doesn't.
        assert_eq!(downsample_bits(&[0b1011_0000, 0b1011_0000], 4, 2, 2, 1, false), Some(vec![0b0100_0000]));
        assert_eq!(downsample_bits(&[0b0100_0000, 0b0100_0000], 4, 2, 2, 1, false), Some(vec![0]));
        // With 1 the dark sample (an inverting decode array, a separation), that line is a 1.
        assert_eq!(downsample_bits(&[0b0100_0000, 0b0100_0000], 4, 2, 2, 1, true), Some(vec![0b1000_0000]));
        assert_eq!(downsample_bits(&[0b1011_0000, 0b1011_0000], 4, 2, 2, 1, true), Some(vec![0b1100_0000]));
        // The same size gives the same bits; padding bits are 0.
        assert_eq!(downsample_bits(&[0b1010_1111], 3, 1, 3, 1, false), Some(vec![0b1010_0000]));
        // 3 to 2 across: the middle column counts half to each new one.
        assert_eq!(downsample_bits(&[0b1100_0000], 3, 1, 2, 1, false), Some(vec![0b1000_0000]));
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
            (vec![grey.clone()], false),
            (vec![grey, inverted.clone()], true),
            (vec![mask.clone()], false),
            (vec![mask, inverted.clone()], true),
            (vec![separation.clone()], true),
            (vec![separation, inverted.clone()], false),
            (vec![indexed.clone()], false),
            (vec![indexed, inverted], false),
            (vec![("ColorSpace", Object::name("DeviceGray")), ("Decode", Object::Array(vec![Object::name("x"), 0.into()]))], false),
        ] {
            assert_eq!(dark_is_one(&doc, &image(&entries)), dark_one, "{entries:?}");
        }
    }

    #[test]
    fn what_it_cannot_resample_is_none() {
        assert_eq!(downsample_bits(&[0; 2], 4, 2, 5, 1, false), None, "wider");
        assert_eq!(downsample_bits(&[0; 2], 4, 2, 2, 3, false), None, "taller");
        assert_eq!(downsample_bits(&[0; 2], 4, 2, 0, 1, false), None);
        assert_eq!(downsample_bits(&[0; 1], 4, 2, 2, 1, false), None, "short rows");
        assert_eq!(downsample_bits(&[], u32::MAX, u32::MAX, 1, 1, true), None);
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
            proptest::prop_assert_eq!(downsample_bits(&data, w, h, nw, nh, dark_one), Some(naive(&data, w, h, nw, nh, dark_one)));
            let white = downsample_bits(&vec![0xFF; len], w, h, nw, nh, dark_one).unwrap();
            proptest::prop_assert_eq!(white, naive(&vec![0xFF; len], w, h, nw, nh, dark_one));
            proptest::prop_assert!(naive(&vec![0xFF; len], w, h, nw, nh, dark_one).iter().any(|b| *b != 0));
        }
    }
}
