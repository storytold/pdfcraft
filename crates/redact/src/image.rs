//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose footprint intersects a region is set to zero (no ink for image masks), and the result is
//! written as a new image, Flate, or for a one-bit image of one component Group 4, or JBIG2 when it
//! was JBIG2, whichever is smallest; the original object is untouched (other pages may use it).
//! CCITT fax and JBIG2 images decode through `ImageBudget::decode`, but for Group 3 on
//! bytes, which PdfCraft's renderer may draw as other rows, and a JBIG2 page of other rows than the
//! image's. A plain `/DCTDecode` image — the one image a scanned page is made of — is decoded and
//! stored again as a new JPEG with the covered pixels zeroed, so a mark on a scan clears what it
//! covers instead of blanking the page (#2246); every JPEG is lossy, so the rest of the scan is
//! recompressed. Images whose codec this can't clear in place (JPX, a JPEG in another colour
//! space, CMYK and YCCK), and those, return `None`, and the caller removes the whole image
//! instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Dict, Document, ImageBudget, Object, Stream};

/// Most pixels an image may have to be cleared in place (larger ones are removed), or to be stored
/// again without shared `/JBIG2Globals` (see [`crate::globals::without_dictionary`]).
pub(crate) const MAX_PIXELS: u64 = pdfcraft_cos::MAX_REDACTED_IMAGE_PIXELS;

/// The components of image `s`'s colour space, through any references (`None` when unknown).
pub(crate) fn components(doc: &Document, s: &Stream) -> Option<usize> {
    let cs = crate::globals::followed(doc, s.dict.get(b"ColorSpace")?);
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
            match &*crate::globals::followed(doc, cs.as_array()?.get(1)?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N")?).ok().filter(|n| matches!(n, 1 | 3 | 4))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// Run `clear` for every sample whose conservative box a region reaches, row by row, and say how
/// many there were. Image space has its origin at the bottom left and row 0 is at the top. The box
/// may be larger than the pixel for a rotated or skewed image, and that conservative over-clear is
/// what keeps a touched sample out of the file: raw samples and JPEG samples are cleared by the same
/// walk so the two cannot drift apart.
fn covered_pixels(ctm: &Matrix, rects: &[[f64; 4]], w: usize, h: usize, mut clear: impl FnMut(usize, usize) -> Option<()>) -> Option<usize> {
    let mut cleared = 0usize;
    for j in 0..h {
        let v0 = 1.0 - (j + 1) as f64 / h as f64;
        let v1 = 1.0 - j as f64 / h as f64;
        let row_box = ctm.bbox([0.0, v0, 1.0, v1]);
        if !rects.iter().any(|r| overlaps(*r, row_box, 0.0)) {
            continue;
        }
        for i in 0..w {
            let u0 = i as f64 / w as f64;
            let u1 = (i + 1) as f64 / w as f64;
            let pixel_box = ctm.bbox([u0, v0, u1, v1]);
            if !rects.iter().any(|r| overlaps(*r, pixel_box, 0.0)) {
                continue;
            }
            cleared += 1;
            clear(i, j)?;
        }
    }
    Some(cleared)
}

/// Whether `s`'s whole filter is a plain `/DCTDecode` — the JPEG a scanned page is made of. A DCT
/// image behind another filter is left to the fail-closed path.
fn plain_dct(s: &Stream) -> bool {
    let dct = |f: &Object| f.as_name().is_some_and(|n| matches!(n, b"DCTDecode" | b"DCT"));
    match s.dict.get(b"Filter") {
        Some(f @ Object::Name(_)) => dct(f),
        Some(Object::Array(a)) => a.len() == 1 && a.first().is_some_and(dct),
        _ => false,
    }
}

/// `Ok(Some(copy))` with the covered pixels cleared, `Ok(None)` when no pixel intersects,
/// `Err(())` when the image can't be cleared (the caller removes it).
#[allow(clippy::result_unit_err)]
pub(crate) fn clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]], budget: &mut ImageBudget) -> Result<Option<Stream>, ()> {
    decode_and_clear(doc, s, ctm, rects, budget).ok_or(())
}

fn decode_and_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]], budget: &mut ImageBudget) -> Option<Option<Stream>> {
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
    // A CCITT fax or JBIG2 image (its last filter) decodes through `ImageBudget::decode`,
    // after the filters before it, so a scan stored as Group 4 or JBIG2 is cleared like any other
    // image.
    let fax = |f: &Object| f.as_name().is_some_and(|n| matches!(n, b"CCITTFaxDecode" | b"CCF"));
    let jbig2 = |f: &Object| f.as_name() == Some(b"JBIG2Decode".as_slice());
    let readable = |f: &Object| f.as_name().is_some_and(supported);
    // `/Crypt` was undone when the object was loaded (ISO 32000-2 §7.4.10), and decoding leaves it
    // out wherever it stands.
    let crypt = |f: &Object| f.as_name() == Some(b"Crypt".as_slice());
    let is_dct = plain_dct(s);
    let (ok, is_fax, is_jbig2) = match s.dict.get(b"Filter") {
        None => (true, false, false),
        Some(f @ Object::Name(_)) => (readable(f) || crypt(f) || fax(f) || jbig2(f), fax(f), jbig2(f)),
        Some(Object::Array(a)) => {
            let chain: Vec<&Object> = a.iter().filter(|f| !crypt(f)).collect();
            match chain.split_last() {
                Some((last, before)) if fax(last) || jbig2(last) => (before.iter().all(|f| readable(f)), fax(last), jbig2(last)),
                _ => (chain.iter().all(|f| readable(f)), false, false),
            }
        }
        _ => (false, false, false),
    };
    if !ok && !is_dct {
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
    if w <= 0 || h <= 0 || (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    if is_dct {
        return dct_clear(doc, s, ctm, rects, w, h, budget);
    }
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
    let mut data = budget.decode(doc, s, allowance).ok()?;
    if data.len() < bytes {
        return None;
    }
    data.truncate(bytes);
    // Image masks: a sample of 0 paints unless /Decode is [1 0]; cleared samples must not paint.
    let clear_bit = if mask {
        // The array, and its first number, given by reference too, through any references.
        let decode = s.dict.get(b"Decode").map(|d| crate::globals::followed(doc, d));
        let first = decode.as_deref().and_then(Object::as_array).and_then(|d| d.first()).map(|v| crate::globals::followed(doc, v));
        let inverted = first.as_deref().and_then(Object::as_f64) == Some(1.0);
        !inverted
    } else {
        false
    };
    let cleared = covered_pixels(ctm, rects, w, h, |i, j| {
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
        Some(())
    })?;
    if cleared == 0 {
        return Some(None);
    }
    let mut dict = s.dict.clone();
    for key in [b"Length".as_slice(), b"DecodeParms", b"DP", b"Alternates"] {
        dict.remove(key);
    }
    if !data_elsewhere(doc, s) {
        // Parameters for data in another file mean nothing without `/F`, but may name a dictionary.
        dict.remove(b"FFilter");
        dict.remove(b"FDecodeParms");
    }
    Some(Some(smallest(dict, &data, w, h, ncomp == 1 && bpc == 1, is_jbig2)))
}

/// Quality of the copy a cleared `/DCTDecode` image is stored as: a JPEG can never be copied
/// losslessly, so the image is recompressed once, at one quality for every cleared scan so the
/// result does not depend on what the source happened to use.
const JPEG_QUALITY: u8 = 90;

/// `Ok(Some(copy))` of `s`, a plain `/DCTDecode` image `w` × `h`, with the samples under every
/// region zeroed (black) and the rest of the scan recompressed; `Ok(None)` when no region reaches
/// a pixel; `None` when it can't be cleared in place (the caller removes the image whole).
///
/// A scanned page is one full-page JPEG (issue #2246), so removing the image under a mark blanks
/// the page while the mark and the OCR layer stay. The copy keeps the image's dictionary — its
/// dimensions, colour space and masks — and only its samples change.
fn dct_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]], w: usize, h: usize, budget: &mut ImageBudget) -> Option<Option<Stream>> {
    // An image mask's samples are bits its `/Decode` may invert, not one to three components.
    if matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true))) {
        return None;
    }
    // With /F, readers draw another file instead of these inline bytes. Clearing the inline
    // JPEG would leave the external picture unredacted, so remove this placement whole.
    if data_elsewhere(doc, s) {
        return None;
    }
    if s.dict.get(b"BitsPerComponent").is_some_and(|b| b.as_int() != Some(8)) {
        return None;
    }
    // One or three components only: a CMYK or YCCK JPEG's samples would be written back in a space
    // its `/ColorSpace` doesn't name, so the cleared and kept pixels could paint somewhere else.
    let ncomp = match &*doc.resolve(s.dict.get(b"ColorSpace")?) {
        Object::Name(name) if matches!(name.as_slice(), b"DeviceGray" | b"G") => 1,
        Object::Name(name) if matches!(name.as_slice(), b"DeviceRGB" | b"RGB") => 3,
        // The decoder produces device samples, not palette indices or calibrated-space values.
        _ => return None,
    };
    if !identity_decode(doc, s, ncomp) {
        return None;
    }
    // An explicit transform can disagree with the JPEG's markers, which control this decoder.
    // Unsupported parameters must remove the image whole rather than shift the unmarked colours.
    if let Some(params) = s.dict.get(b"DecodeParms").or_else(|| s.dict.get(b"DP")).map(|o| doc.resolve(o)) {
        let entries = params.as_array().map(Vec::as_slice).unwrap_or_else(|| std::slice::from_ref(params.as_ref()));
        if entries.iter().any(|o| doc.resolve(o).as_dict().is_some_and(|d| d.contains(b"ColorTransform"))) {
            return None;
        }
    }
    let (w32, h32) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    // The document's allowance pays for the JPEG's samples and the bytes that hold them, before its
    // tables are parsed — the same charge `create` and `optimize` make for the same decode
    // (`ImageBudget::pay_jpeg`). Without it a document of many scans would cost unbounded work here.
    let samples = u64::try_from(w).ok()?.checked_mul(u64::try_from(h).ok()?)?.checked_mul(u64::try_from(ncomp).ok()?)?;
    if !budget.pay_jpeg(samples, s.raw.as_ref()) {
        return None;
    }

    // The header is read before the samples are allocated, so a dictionary that declares a small
    // image can't make the decoder allocate a large one. A header that disagrees with the
    // dictionary is not an image this can clear: a mark's pixels would be at other samples.
    let read = |s: &Stream| image::ImageReader::with_format(std::io::Cursor::new(s.raw.clone()), image::ImageFormat::Jpeg);
    let header = craft_image_codecs::jpeg_probe::probe_jpeg(&s.raw).ok()?;
    if (header.width, header.height, usize::from(header.components)) != (w32, h32, ncomp) {
        return None;
    }
    let decoded = read(s).decode().ok()?;
    if (decoded.width(), decoded.height()) != (w32, h32) {
        return None;
    }
    let (mut data, space) = if ncomp == 1 {
        (decoded.into_luma8().into_raw(), image::ExtendedColorType::L8)
    } else {
        (decoded.into_rgb8().into_raw(), image::ExtendedColorType::Rgb8)
    };
    if data.len() != w.checked_mul(h)?.checked_mul(ncomp)? {
        return None;
    }
    let cleared = covered_pixels(ctm, rects, w, h, |i, j| {
        let at = (j.checked_mul(w)?).checked_add(i)?.checked_mul(ncomp)?;
        let sample = data.get_mut(at..at.checked_add(ncomp)?)?;
        sample.fill(0);
        Some(())
    })?;
    if cleared == 0 {
        return Some(None);
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode(&data, w32, h32, space).ok()?;
    let mut dict = s.dict.clone();
    for key in [b"Length".as_slice(), b"DecodeParms", b"DP", b"Alternates"] {
        dict.remove(key);
    }
    // The copy's samples are 8 bits whatever the image's dictionary said (it may have said
    // nothing): a JPEG is 8-bit samples unless it is a 12-bit one, which this decoder refuses.
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    if !data_elsewhere(doc, s) {
        dict.remove(b"FFilter");
        dict.remove(b"FDecodeParms");
    }
    Some(Some(Stream::from_raw(dict, out)))
}

/// Whether `s`'s `/Decode` (through any references) is the identity, so that a zeroed sample is
/// no ink: an inverted or scaled one would paint the cleared pixels, so such an image is removed
/// whole instead.
fn identity_decode(doc: &Document, s: &Stream, ncomp: usize) -> bool {
    let Some(d) = s.dict.get(b"Decode") else { return true };
    let Some(values) = crate::globals::followed(doc, d).as_array().cloned() else {
        return false;
    };
    values.len() == ncomp.saturating_mul(2)
        && values.as_chunks::<2>().0.iter().all(|pair| {
            crate::globals::followed(doc, &pair[0]).as_f64() == Some(0.0) && crate::globals::followed(doc, &pair[1]).as_f64() == Some(1.0)
        })
}

/// Whether `s`'s data is in another file, which a reader may draw instead of what is here: a `/F`
/// that isn't null, through any references (a null entry is as if absent, and a reference to no
/// object is null, ISO 32000-2 §7.3.7 and §7.3.10). A ring of references counts as a file.
pub(crate) fn data_elsewhere(doc: &Document, s: &Stream) -> bool {
    s.dict.get(b"F").is_some_and(|f| !matches!(&*crate::globals::followed(doc, f), Object::Null))
}

/// The cleared samples `data` of a `w` × `h` image as a stream with `dict`: Flate, or, for one bit
/// of one component (`bilevel`, a scan as a rule, which Flate stores several times larger), CCITT
/// Group 4, or JBIG2 when the image was JBIG2 (`jbig2`; JBIG2 is PDF 1.4, which such a file already
/// is), whichever is smallest as stored, dictionary included (a tiny image can lose by Group 4's
/// parameters). Both decode to `data`, cleared before it is coded, so nothing of the original's
/// code is carried over; Group 4 or JBIG2 that can't code the rows (too wide) is left out.
pub(crate) fn smallest(dict: Dict, data: &[u8], w: usize, h: usize, bilevel: bool, jbig2: bool) -> Stream {
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
