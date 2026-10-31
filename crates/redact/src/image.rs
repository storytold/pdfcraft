//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose footprint intersects a region is set to zero (no ink for image masks), and the result is
//! written as a new image, Flate, or for a one-bit image of one component Group 4, or JBIG2 when it
//! was JBIG2, whichever is smallest; the original object is untouched (other pages may use it).
//! CCITT fax and JBIG2 images decode through `Document::decoded_image_within`, but for Group 3 on
//! bytes, which PdfCraft's renderer may draw as other rows, and a JBIG2 page of other rows than the
//! image's. Images whose codec PdfCraft can't decode (DCT, JPX), and those, return `None`, and the
//! caller removes the whole image instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Dict, Document, Object, Stream};

/// Most pixels an image may have to be cleared in place (larger ones are removed).
const MAX_PIXELS: u64 = 64 << 20;

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
pub(crate) fn clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Result<Option<Stream>, ()> {
    decode_and_clear(doc, s, ctm, rects).ok_or(())
}

fn decode_and_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Option<Option<Stream>> {
    // Separate masks can retain the original image data.
    if s.dict.contains(b"SMask") || s.dict.contains(b"Mask") {
        return None;
    }
    let supported = |n: &[u8]| {
        matches!(
            n,
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" | b"RunLengthDecode" | b"RL"
        )
    };
    // A CCITT fax or JBIG2 image (its last filter) decodes through `Document::decoded_image_within`,
    // after the filters before it, so a scan stored as Group 4 or JBIG2 is cleared like any other
    // image.
    let fax = |f: &Object| f.as_name().is_some_and(|n| matches!(n, b"CCITTFaxDecode" | b"CCF"));
    let jbig2 = |f: &Object| f.as_name() == Some(b"JBIG2Decode".as_slice());
    let readable = |f: &Object| f.as_name().is_some_and(supported);
    let (ok, is_fax, is_jbig2) = match s.dict.get(b"Filter") {
        None => (true, false, false),
        Some(f @ Object::Name(_)) => (readable(f) || fax(f) || jbig2(f), fax(f), jbig2(f)),
        Some(Object::Array(a)) => match a.split_last() {
            Some((last, before)) if fax(last) || jbig2(last) => (before.iter().all(readable), fax(last), jbig2(last)),
            _ => (a.iter().all(readable), false, false),
        },
        _ => (false, false, false),
    };
    if !ok {
        return None;
    }
    // Group 3 on bytes is removed whole for now. The decoder reads it by its end-of-line codes
    // when the data starts with one, but PdfCraft's renderer draws such rows (the fill ending
    // each code on a byte) as other rows, so the pixels a mark covers on screen needn't be the
    // ones cleared.
    if is_fax && doc.fax_params(s).is_none_or(|p| p.k >= 0 && p.encoded_byte_align) {
        return None;
    }
    let (w, h) = (s.dict.int(b"Width")?, s.dict.int(b"Height")?);
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask { (1, 1) } else { (components(doc, s)?, s.dict.int(b"BitsPerComponent")? as usize) };
    // JBIG2 codes one-bit images of one component (ISO 32000-2 §7.4.7).
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) || (is_jbig2 && (ncomp, bpc) != (1, 1)) {
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
    let allowance = if is_fax {
        w.checked_mul(h)?.checked_add(h.checked_mul(4)?)?.checked_add(4096)?
    } else {
        bytes.checked_add(h)?.checked_mul(3)?.checked_add(4096)?
    };
    // Through the document, which reads `/DecodeParms` given by reference (`Stream::decoded*`
    // alone would skip such a predictor, and the cleared bytes would then miss the covered pixels),
    // and refuses a JBIG2 page of another size than the image: the renderer draws the whole page
    // into the image, so the rows cleared wouldn't be the ones a mark covers on screen.
    let mut data = doc.decoded_image_within(s, allowance).ok()?;
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
    Some(Some(smallest(dict, &data, w, h, ncomp == 1 && bpc == 1, is_jbig2)))
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
