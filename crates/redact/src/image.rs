//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose footprint intersects a region is set to zero (no ink for image masks), and the result is
//! written as a new Flate image; the original object is untouched (other pages may use it).
//! CCITT fax images decode through `Document::decoded_image_within`, but for Group 3 on bytes,
//! which PdfCraft's renderer may draw as other rows. Images whose codec PdfCraft can't decode
//! (DCT, JPX, JBIG2), and Group 3 on bytes, return `None`, and the caller removes the whole image
//! instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Document, Object, Stream};

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
    // A CCITT fax image (its last filter) decodes through `Document::decoded_image_within`, after the
    // filters before it, so a scan stored as Group 4 is cleared like any other image.
    let fax = |f: &Object| f.as_name().is_some_and(|n| matches!(n, b"CCITTFaxDecode" | b"CCF"));
    let readable = |f: &Object| f.as_name().is_some_and(supported);
    let (ok, is_fax) = match s.dict.get(b"Filter") {
        None => (true, false),
        Some(f @ Object::Name(_)) => (readable(f) || fax(f), fax(f)),
        Some(Object::Array(a)) => match a.split_last() {
            Some((last, before)) if fax(last) => (before.iter().all(readable), true),
            _ => (a.iter().all(readable), false),
        },
        _ => (false, false),
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
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
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
    // alone would skip such a predictor, and the cleared bytes would then miss the covered pixels).
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
    Some(Some(Stream::flate(dict, &data)))
}
