//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose footprint intersects a region is set to zero (no ink for image masks), and the result is
//! written as a new image, Flate, or for a one-bit image of one component Group 4, or JBIG2 when it
//! was JBIG2, whichever is smallest; the original object is untouched (other pages may use it).
//! CCITT fax and JBIG2 images decode through `ImageBudget::decode`, but for Group 3 on
//! bytes, which PdfCraft's renderer may draw as other rows, and a JBIG2 page of other rows than the
//! image's. JPEG and JPEG 2000 images in a device space (grey, RGB, CMYK) decode through
//! `pdfcraft_filters::decode_dct` and `decode_jpx`, paid for first, so a scanned page keeps
//! everything a mark doesn't cover. Images this path can't decode, and those, return `None`, and
//! the caller removes the whole image instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Dict, Document, ImageBudget, Object, Stream};

/// Most pixels an image may have to be cleared in place (larger ones are removed), or to be stored
/// again without shared `/JBIG2Globals` (see [`without_globals`]).
const MAX_PIXELS: u64 = pdfcraft_cos::MAX_REDACTED_IMAGE_PIXELS;

/// The component count of a JPEG or JPEG 2000 image whose `/ColorSpace` names a device space,
/// which is what their decoders give samples in. Another space (Indexed, ICCBased) is removed
/// whole, like any layout this path doesn't decode (#2246).
fn device_components(doc: &Document, s: &Stream) -> Option<usize> {
    match &*doc.resolve(s.dict.get(b"ColorSpace")?) {
        Object::Name(name) if matches!(name.as_slice(), b"DeviceGray" | b"G") => Some(1),
        Object::Name(name) if matches!(name.as_slice(), b"DeviceRGB" | b"RGB") => Some(3),
        Object::Name(name) if matches!(name.as_slice(), b"DeviceCMYK" | b"CMYK") => Some(4),
        _ => None,
    }
}

fn components(doc: &Document, s: &Stream) -> Option<usize> {
    let cs = doc.resolve(s.dict.get(b"ColorSpace")?);
    let cs = &*cs;
    let name = match cs {
        Object::Name(n) => n.as_slice(),
        Object::Array(a) => a.first()?.as_name()?,
        _ => return None,
    };
    Some(match name {
        b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
        b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => {
            // /N lives in the ICC profile stream, and shall be 1, 3 or 4 (ISO 32000-2 §8.6.5.5).
            match &*doc.resolve(cs.as_array()?.get(1)?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N")?).ok().filter(|n| matches!(n, 1 | 3 | 4))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// `Ok(Some(copy))` with the covered pixels cleared, `Ok(None)` when no pixel intersects,
/// `Err(())` when the image can't be cleared (the caller removes it).
#[allow(clippy::result_unit_err)]
pub(crate) fn clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]], budget: &mut ImageBudget) -> Result<Option<Stream>, ()> {
    decode_and_clear(doc, s, ctm, rects, budget).ok_or(())
}

/// How an image's samples are reached.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    /// Through general filters alone (or none).
    Filters,
    /// CCITT fax or JBIG2, after the filters before it, through `ImageBudget::decode`.
    Fax,
    Jbig2,
    /// JPEG or JPEG 2000, whose decoders read the coded bytes themselves and give 8-bit samples.
    Dct,
    Jpx,
}

/// What the stream's filter chain says about the codecs this path can clear. `None` when the
/// image cannot be decoded at all, which the caller answers by removing it whole. A fax or JBIG2
/// codec must be the last filter; a JPEG or JPEG 2000 codec must be the only one, since its
/// decoder reads the encoded bytes itself.
fn filters(s: &Stream) -> Option<Codec> {
    let supported = |n: &[u8]| {
        matches!(
            n,
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" | b"RunLengthDecode" | b"RL"
        )
    };
    let codec = |f: &Object| match f.as_name()? {
        b"CCITTFaxDecode" | b"CCF" => Some(Codec::Fax),
        b"JBIG2Decode" => Some(Codec::Jbig2),
        b"DCTDecode" | b"DCT" => Some(Codec::Dct),
        b"JPXDecode" => Some(Codec::Jpx),
        _ => None,
    };
    let readable = |f: &Object| f.as_name().is_some_and(supported);
    // `/Crypt` was undone when the object was loaded (ISO 32000-2 §7.4.10), and decoding leaves it
    // out wherever it stands.
    let crypt = |f: &Object| f.as_name() == Some(b"Crypt".as_slice());
    match s.dict.get(b"Filter") {
        None => Some(Codec::Filters),
        Some(f @ Object::Name(_)) if readable(f) || crypt(f) => Some(Codec::Filters),
        Some(f @ Object::Name(_)) => codec(f),
        Some(Object::Array(a)) => {
            let chain: Vec<&Object> = a.iter().filter(|f| !crypt(f)).collect();
            match chain.split_last() {
                Some((last, before)) => match codec(last) {
                    Some(c @ (Codec::Fax | Codec::Jbig2)) => before.iter().all(|f| readable(f)).then_some(c),
                    Some(c @ (Codec::Dct | Codec::Jpx)) => before.is_empty().then_some(c),
                    _ => chain.iter().all(|f| readable(f)).then_some(Codec::Filters),
                },
                None => Some(Codec::Filters),
            }
        }
        _ => None,
    }
}

/// The 8-bit samples of a JPEG or JPEG 2000 image `s` (`codec`), paid for first, with their
/// component count and, for a JPEG 2000 image whose dictionary names no colour space, the device
/// space its file states (its cleared copy has to name one). `None` for what isn't cleared in
/// place: an image mask, another colour space than a device one (Indexed, ICCBased), a JPEG of
/// another depth than 8 bits or with a `/ColorTransform` of its own, a JPEG 2000 image with alpha
/// or of another component count than its colour space, data that doesn't decode to its
/// dictionary's size, and a decode past what is left of `budget` (which refuses the run).
fn coded_samples(
    doc: &Document,
    s: &Stream,
    codec: Codec,
    w: usize,
    h: usize,
    budget: &mut ImageBudget,
) -> Option<(Vec<u8>, usize, Option<&'static str>)> {
    if matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true))) {
        return None;
    }
    let pixels = (w as u64).saturating_mul(h as u64);
    if codec == Codec::Dct {
        // shortcut: a `/ColorTransform` of the image's own is removed whole, since the decoder
        // follows the JPEG's markers; read it into `decode_dct` if such scans turn up.
        let parms = s.dict.get(b"DecodeParms").map(|p| doc.resolve(p));
        if parms.as_deref().and_then(Object::as_dict).is_some_and(|p| p.contains(b"ColorTransform")) || s.dict.int(b"BitsPerComponent") != Some(8) {
            return None;
        }
        let n = device_components(doc, s)?;
        if !budget.pay_jpeg(pixels.saturating_mul(n as u64), &s.raw) {
            return None;
        }
        return Some((pdfcraft_filters::decode_dct(&s.raw, w, h, n)?, n, None));
    }
    // shortcut: JPEG 2000 pays as a JPEG of four components whatever it holds (its count is known
    // only once decoded); give it a rate of its own if a run of JPEG 2000 scans proves slower.
    if !budget.pay_jpeg(pixels.saturating_mul(4), &s.raw) {
        return None;
    }
    let jpx = pdfcraft_filters::decode_jpx(&s.raw, w, h)?;
    // The dictionary's colour space, when it gives one, is the one the page draws the samples in
    // (ISO 32000-2 §8.9.5.1, `/ColorSpace`); otherwise the file's own.
    if s.dict.contains(b"ColorSpace") {
        (device_components(doc, s)? == jpx.components).then_some((jpx.data, jpx.components, None))
    } else {
        let space = jpx.device?;
        Some((jpx.data, jpx.components, Some(space)))
    }
}

fn decode_and_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]], budget: &mut ImageBudget) -> Option<Option<Stream>> {
    // Separate masks can retain the original image data.
    if s.dict.contains(b"SMask") || s.dict.contains(b"Mask") {
        return None;
    }
    let codec = filters(s)?;
    // Group 3 on bytes is removed whole for now. The decoder reads it by its end-of-line codes
    // when the data starts with one, but PdfCraft's renderer draws such rows (the fill ending
    // each code on a byte) as other rows, so the pixels a mark covers on screen needn't be the
    // ones cleared.
    if codec == Codec::Fax && doc.fax_params(s).is_none_or(|p| p.k >= 0 && p.encoded_byte_align) {
        return None;
    }
    let (w, h) = (s.dict.int(b"Width")?, s.dict.int(b"Height")?);
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let coded = if matches!(codec, Codec::Dct | Codec::Jpx) { Some(coded_samples(doc, s, codec, w, h, budget)?) } else { None };
    let (ncomp, bpc) = match &coded {
        Some((_, n, _)) => (*n, 8),
        None if mask => (1, 1),
        None => (components(doc, s)?, s.dict.int(b"BitsPerComponent")? as usize),
    };
    // JBIG2 codes one-bit images of one component (ISO 32000-2 §7.4.7).
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) || (codec == Codec::Jbig2 && (ncomp, bpc) != (1, 1)) {
        return None;
    }
    // Every size comes from the file: multiply checked, so an overflowing product refuses the image
    // instead of sizing the writes below.
    let bits_per_pixel = ncomp.checked_mul(bpc)?;
    let row = w.checked_mul(bits_per_pixel)?.div_ceil(8);
    let bytes = row.checked_mul(h)?;
    let row_bits = row.checked_mul(8)?;
    bytes.checked_mul(8)?;
    // Decode no further than the raster can need: a filter stage may legitimately produce up to
    // three bytes for each of it and a PNG predictor's byte per row (ASCIIHex text under Flate,
    // two digits a byte broken into lines or spaced), as the optimizer allows. A fax image's
    // code, which the filters before its decoder produce, takes at most about a byte per pixel
    // plus each row's end. Data past that can only be a decompression bomb; the image is then
    // removed whole.
    let allowance = if codec == Codec::Fax {
        w.checked_mul(h)?.checked_add(h.checked_mul(4)?)?.checked_add(4096)?
    } else {
        bytes.checked_add(h)?.checked_mul(3)?.checked_add(4096)?
    };
    // Through the document, which reads `/DecodeParms` given by reference (`Stream::decoded*`
    // alone would skip such a predictor, and the cleared bytes would then miss the covered pixels),
    // and refuses a JBIG2 page of another size than the image: the renderer draws the whole page
    // into the image, so the rows cleared wouldn't be the ones a mark covers on screen.
    let (mut data, jpx_space) = match coded {
        Some((data, _, space)) => (data, space),
        None => (budget.decode(doc, s, allowance).ok()?, None),
    };
    if data.len() < bytes {
        return None;
    }
    data.truncate(bytes);
    // Image masks: a sample of 0 paints unless /Decode is [1 0]; cleared samples must not paint.
    let clear_bit = if mask {
        // The array, and its first number, given by reference too.
        let decode = s.dict.get(b"Decode").map(|d| doc.resolve(d));
        let first = decode.as_deref().and_then(Object::as_array).and_then(|d| d.first()).map(|v| doc.resolve(v));
        let inverted = first.as_deref().and_then(Object::as_f64) == Some(1.0);
        !inverted
    } else {
        false
    };
    let mut cleared = 0usize;
    for j in 0..h {
        // Image space has its origin at the bottom left; row 0 is at the top.
        let v0 = 1.0 - (j + 1) as f64 / h as f64;
        let v1 = 1.0 - j as f64 / h as f64;
        let row_box = ctm.bbox([0.0, v0, 1.0, v1]);
        if !rects.iter().any(|r| overlaps(*r, row_box, 0.0)) {
            continue;
        }
        for i in 0..w {
            let u0 = i as f64 / w as f64;
            let u1 = (i + 1) as f64 / w as f64;
            // The bounding box may cover extra pixels for a rotated or skewed image; that
            // conservative over-clear prevents a touched sample surviving in the file.
            let pixel_box = ctm.bbox([u0, v0, u1, v1]);
            if !rects.iter().any(|r| overlaps(*r, pixel_box, 0.0)) {
                continue;
            }
            cleared += 1;
            // `bytes * 8` fits, and every bit index is below it, so this arithmetic can't overflow.
            let start = j * row_bits + i * bits_per_pixel;
            for bit in start..start + bits_per_pixel {
                let (byte, shift) = (bit / 8, 7 - bit % 8);
                let b = data.get_mut(byte)?;
                if clear_bit {
                    *b |= 1 << shift;
                } else {
                    *b &= !(1 << shift);
                }
            }
        }
    }
    if cleared == 0 {
        return Some(None);
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    dict.remove(b"DecodeParms");
    dict.remove(b"Alternates");
    if codec == Codec::Jpx {
        as_decoded_jpx(&mut dict, jpx_space);
    }
    Some(Some(smallest(dict, &data, w, h, ncomp == 1 && bpc == 1, codec == Codec::Jbig2)))
}

/// What JPEG 2000 leaves to its file a Flate copy's `dict` has to say: 8-bit samples, and their
/// `space` when the dictionary names none. `/Decode` is ignored for JPEG 2000 (ISO 32000-2
/// §8.9.5.1) and would apply to the copy, and `/SMaskInData` means nothing to it.
fn as_decoded_jpx(dict: &mut Dict, space: Option<&'static str>) {
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    if let Some(space) = space {
        dict.set(b"ColorSpace".to_vec(), Object::name(space));
    }
    dict.remove(b"Decode");
    dict.remove(b"SMaskInData");
}

/// The cleared samples `data` of a `w` × `h` image as a stream with `dict`: Flate, or, for one bit
/// of one component (`bilevel`, a scan as a rule, which Flate stores several times larger), CCITT
/// Group 4, or JBIG2 when the image was JBIG2 (`jbig2`; JBIG2 is PDF 1.4, which such a file already
/// is), whichever is smallest as stored, dictionary included (a tiny image can lose by Group 4's
/// parameters). Both decode to `data`, cleared before it is coded, so nothing of the original's
/// code is carried over; Group 4 or JBIG2 that can't code the rows (too wide) is left out.
fn smallest(dict: Dict, data: &[u8], w: usize, h: usize, bilevel: bool, jbig2: bool) -> Stream {
    let flate = Stream::flate(dict.clone(), data);
    let Some((columns, rows)) = u32::try_from(w).ok().zip(u32::try_from(h).ok()).filter(|_| bilevel) else { return flate };
    let g4 = pdfcraft_filters::encode_g4(data, columns, rows).ok().map(|coded| {
        let mut dict = dict.clone();
        let mut parms = Dict::new();
        parms.set(b"K".to_vec(), Object::Int(-1));
        parms.set(b"Columns".to_vec(), Object::Int(i64::from(columns)));
        parms.set(b"Rows".to_vec(), Object::Int(i64::from(rows)));
        dict.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
        dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
        Stream::from_raw(dict, coded)
    });
    let jbig = jbig2.then(|| pdfcraft_filters::encode_jbig2(data, columns, rows).ok()).flatten().map(|coded| {
        let mut dict = dict.clone();
        dict.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
        Stream::from_raw(dict, coded)
    });
    [g4, jbig].into_iter().flatten().fold(flate, |best, s| if stored_len(&s) < stored_len(&best) { s } else { best })
}

/// The bytes stream `s` takes in a file: its data and its dictionary as written.
fn stored_len(s: &Stream) -> usize {
    let mut dict = Vec::new();
    pdfcraft_cos::serialize(&Object::Dict(s.dict.clone()), &mut dict);
    dict.len().saturating_add(s.raw.len())
}

/// JBIG2 image `s` stored again without its `/JBIG2Globals`: decoded through them, as it is drawn,
/// and written as the same samples, Flate, Group 4 or JBIG2 coded alone, whichever is smallest
/// (`smallest`), its other entries kept. `None` when it can't be decoded for certain (too large,
/// past what is left of `budget`, data that doesn't decode, a JBIG2 page of another size than the
/// image, or not a one-bit image of one component). Redaction does this to every image that
/// shares a redacted image's globals, since their symbol dictionary holds the shapes of marks drawn
/// under the area too.
pub(crate) fn without_globals(doc: &Document, s: &Stream, budget: &mut ImageBudget) -> Option<Stream> {
    let (w, h) = (s.dict.int(b"Width")?, s.dict.int(b"Height")?);
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    // JBIG2 codes one-bit images of one component (ISO 32000-2 §7.4.7).
    if !mask && (components(doc, s)?, s.dict.int(b"BitsPerComponent")?) != (1, 1) {
        return None;
    }
    let bytes = w.div_ceil(8).checked_mul(h)?;
    // As for `clear`: the filters before the JBIG2 decoder may give up to three bytes for each one
    // of the raster and a byte a row.
    let allowance = bytes.checked_add(h)?.checked_mul(3)?.checked_add(4096)?;
    let mut data = budget.decode(doc, s, allowance).ok()?;
    if data.len() < bytes {
        return None;
    }
    data.truncate(bytes);
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    // The globals are named in the parameters; the copy is coded alone, as `clear`'s are.
    dict.remove(b"DecodeParms");
    dict.remove(b"Alternates");
    Some(smallest(dict, &data, w, h, true, true))
}
