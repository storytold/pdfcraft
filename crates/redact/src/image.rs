//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose centre maps into a region is set to zero (no ink for image masks), and the result is
//! written as a new Flate image; the original object is untouched (other pages may use it).
//! Images whose codec PdfCraft can't decode (DCT, JPX, JBIG2, CCITT) return `None`, and the
//! caller removes the whole image instead; that is fail-closed.

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
            // /N lives in the ICC profile stream.
            match &*doc.resolve(cs.as_array()?.get(1)?) {
                Object::Stream(icc) => icc.dict.int(b"N")? as usize,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// `Ok(Some(copy))` with the covered pixels cleared, `Ok(None)` when no pixel centre is covered,
/// `Err(())` when the image can't be cleared (the caller removes it).
#[allow(clippy::result_unit_err)]
pub(crate) fn clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Result<Option<Stream>, ()> {
    decode_and_clear(doc, s, ctm, rects).ok_or(())
}

fn decode_and_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Option<Option<Stream>> {
    let supported = |n: &[u8]| {
        matches!(
            n,
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" | b"RunLengthDecode" | b"RL"
        )
    };
    let ok = match s.dict.get(b"Filter") {
        None => true,
        Some(Object::Name(n)) => supported(n),
        Some(Object::Array(a)) => a.iter().all(|f| f.as_name().is_some_and(supported)),
        _ => false,
    };
    if !ok {
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
    let mut data = s.decoded().ok()?;
    let row = (w * ncomp * bpc).div_ceil(8);
    if data.len() < row * h {
        return None;
    }
    data.truncate(row * h);
    // Image masks: a sample of 0 paints unless /Decode is [1 0]; cleared samples must not paint.
    let clear_bit = if mask {
        let inverted = s.dict.get(b"Decode").and_then(Object::as_array).and_then(|d| d.first()).and_then(Object::as_f64) == Some(1.0);
        !inverted
    } else {
        false
    };
    let bits_per_pixel = ncomp * bpc;
    let mut cleared = 0usize;
    for j in 0..h {
        // Image space: (0,0) is the bottom-left of the unit square; row 0 is the top.
        let v = 1.0 - (j as f64 + 0.5) / h as f64;
        let (ax, ay) = ctm.apply(0.0, v);
        let (bx, by) = ctm.apply(1.0, v);
        let row_box = [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)];
        if !rects.iter().any(|r| overlaps(*r, [row_box[0], row_box[1] - 0.01, row_box[2], row_box[3] + 0.01], 0.0)) {
            continue;
        }
        for i in 0..w {
            let u = (i as f64 + 0.5) / w as f64;
            let (x, y) = ctm.apply(u, v);
            if !rects.iter().any(|r| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]) {
                continue;
            }
            cleared += 1;
            let start = j * row * 8 + i * bits_per_pixel;
            for bit in start..start + bits_per_pixel {
                let (byte, shift) = (bit / 8, 7 - bit % 8);
                if clear_bit {
                    data[byte] |= 1 << shift;
                } else {
                    data[byte] &= !(1 << shift);
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
    Some(Some(Stream::flate(dict, &data)))
}
