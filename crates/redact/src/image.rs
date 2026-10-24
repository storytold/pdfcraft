//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose footprint intersects a region is set to zero (no ink for image masks), and the result is
//! written as a new Flate image; the original object is untouched (other pages may use it).
//! DCT (JPEG) images in supported 8-bit grayscale and RGB layouts are decoded so unmarked samples
//! remain. Unsupported codecs/layouts (JPX, JBIG2, CCITT and other DCT cases) return `None`, and
//! the caller removes the whole image instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Document, Object, Stream};

/// Most pixels an image may have to be cleared in place (larger ones are removed).
const MAX_PIXELS: u64 = 64 << 20;

/// Decode a DCT (JPEG) image's samples for clearing. `None` when the JPEG does not decode to the
/// size and component count its dictionary declares (the caller then removes the image, as for
/// any other undecodable codec).
fn jpeg_samples(raw: &[u8], ncomp: usize, w: usize, h: usize) -> Option<Vec<u8>> {
    if jpeg_source_components(raw)? != ncomp {
        return None;
    }
    let mut reader = ::image::ImageReader::with_format(std::io::Cursor::new(raw), ::image::ImageFormat::Jpeg);
    let mut limits = ::image::Limits::default();
    // Match the PDF's checked raster dimensions before decoding, so a mismatched JPEG header
    // cannot make the decoder allocate for a larger image than this redaction path permits.
    limits.max_image_width = Some(u32::try_from(w).ok()?);
    limits.max_image_height = Some(u32::try_from(h).ok()?);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    if (img.width() as usize, img.height() as usize) != (w, h) {
        return None;
    }
    Some(match (ncomp, img.color()) {
        (1, ::image::ColorType::L8) => img.into_luma8().into_raw(),
        (3, ::image::ColorType::Rgb8) => img.into_rgb8().into_raw(),
        _ => return None,
    })
}

/// Read the JPEG frame's encoded component count before the image decoder can convert its output.
fn jpeg_source_components(raw: &[u8]) -> Option<usize> {
    if raw.get(..2)? != b"\xff\xd8" {
        return None;
    }
    let mut pos = 2;
    while pos < raw.len() {
        if raw.get(pos).copied()? != 0xff {
            return None;
        }
        while raw.get(pos).copied() == Some(0xff) {
            pos += 1;
        }
        let marker = raw.get(pos).copied()?;
        pos += 1;
        match marker {
            // Standalone markers have no length field.
            0x01 | 0xd0..=0xd7 => continue,
            // The frame header must precede scan data and the end marker.
            0x00 | 0xda | 0xd9 => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*raw.get(pos)?, *raw.get(pos + 1)?]));
        if len < 2 {
            return None;
        }
        let end = pos.checked_add(len)?;
        if end > raw.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let ncomp = usize::from(*raw.get(pos + 7)?);
            if ncomp == 0 || len != 8usize.checked_add(ncomp.checked_mul(3)?)? {
                return None;
            }
            return Some(ncomp);
        }
        pos = end;
    }
    None
}

fn dct_components(doc: &Document, s: &Stream) -> Option<usize> {
    match &*doc.resolve(s.dict.get(b"ColorSpace")?) {
        Object::Name(name) if matches!(name.as_slice(), b"DeviceGray" | b"G") => Some(1),
        Object::Name(name) if matches!(name.as_slice(), b"DeviceRGB" | b"RGB") => Some(3),
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
    let filters: Vec<&[u8]> = match s.dict.get(b"Filter") {
        None => Vec::new(),
        Some(Object::Name(n)) => vec![n],
        Some(Object::Array(a)) => a.iter().map(|f| f.as_name()).collect::<Option<_>>()?,
        _ => return None,
    };
    // The JPEG decoder consumes the raw bytes, so accept DCT only as a single filter. Other filter
    // chains retain the upstream supported-filter rules and bounded PDF stream decoder below.
    let dct = matches!(filters.as_slice(), [b"DCTDecode"] | [b"DCT"]);
    if !dct && !filters.iter().all(|n| supported(n)) {
        return None;
    }
    let (w, h) = (s.dict.int(b"Width")?, s.dict.int(b"Height")?);
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let ncomp = if mask {
        if dct {
            return None;
        }
        1
    } else if dct {
        dct_components(doc, s)?
    } else {
        components(doc, s)?
    };
    let bpc = if mask { 1 } else { s.dict.int(b"BitsPerComponent")? as usize };
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
    let mut data = if dct {
        if bpc != 8 {
            return None;
        }
        jpeg_samples(&s.raw, ncomp, w, h)?
    } else {
        // Decode no further than the raster can need: a filter stage may legitimately produce up
        // to twice it (ASCIIHex text under Flate) plus a PNG predictor's byte per row. Data past
        // that can only be a decompression bomb; the image is then removed whole.
        let allowance = bytes.checked_add(h)?.checked_mul(2)?.checked_add(4096)?;
        s.decoded_within(allowance).ok()?
    };
    if data.len() < bytes {
        return None;
    }
    data.truncate(bytes);
    // Image masks: a sample of 0 paints unless /Decode is [1 0]; cleared samples must not paint.
    let clear_bit = if mask {
        let inverted = s.dict.get(b"Decode").and_then(Object::as_array).and_then(|d| d.first()).and_then(Object::as_f64) == Some(1.0);
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
