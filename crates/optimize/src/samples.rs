//! An image's samples read out and written back at the same size, for tools that change the
//! pixels of an image in place (Scan & OCR ▸ Enhance ▸ Scanned document). The image dictionary
//! stays as it was apart from the encoding: its size, colour space, masks, `/Interpolate`,
//! `/Intent`, `/Metadata` and every other entry are kept.

use pdfcraft_cos::{Document, Object, Stream};

use crate::images::{MAX_PIXELS, components, is_dct, is_identity_decode, jpeg, pixels, ycc_encoded};

/// An image's samples, one byte each, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Samples {
    pub width: u32,
    pub height: u32,
    /// 1 (grey) or 3 (RGB).
    pub components: usize,
    /// The samples. A bi-level image's are 0 (black) or 255 (white) as drawn, whatever its
    /// decode array.
    pub data: Vec<u8>,
    /// Stored as a JPEG (written back as one); otherwise written back losslessly (Flate).
    pub jpeg: bool,
    /// One bit per pixel (written back as one).
    pub bilevel: bool,
    /// A bi-level image whose 1 bits are black (decode array `[1 0]`): written back the same way,
    /// so its decode array stays true.
    pub inverted: bool,
}

/// Why an image's samples were not read. Each says what the image is, so a report can name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NotDecoded {
    #[error("it is a stencil mask, not a picture")]
    StencilMask,
    #[error("its colour space is not grey or RGB (CMYK, indexed and special colours are not supported yet)")]
    ColourSpace,
    #[error("it has {0} bits per sample (8, and 1 for black and white, are supported)")]
    BitDepth(i64),
    #[error("its decode array remaps the samples")]
    Decode,
    #[error("it is compressed in a format PdfCraft cannot read back yet (JPEG 2000, JBIG2 or CCITT fax)")]
    Filter,
    #[error("its JPEG colour transform is ambiguous, so readers may disagree about its colours")]
    Ambiguous,
    #[error("it is larger than 80 million pixels")]
    TooLarge,
    #[error("its data could not be decoded, or is shorter than its size says")]
    Corrupt,
}

/// Read the samples of image stream `s` (a dictionary with `/Subtype /Image`).
pub fn decode_samples(doc: &Document, s: &Stream) -> Result<Samples, NotDecoded> {
    let d = &s.dict;
    if matches!(d.get(b"ImageMask").map(|v| doc.resolve(v)).as_deref(), Some(Object::Bool(true))) {
        return Err(NotDecoded::StencilMask);
    }
    // The filter first: a JPEG 2000 image may leave its colour space and bit depth to the data,
    // and the report should name the format, not the missing entries.
    let jpeg = is_dct(s).ok_or(NotDecoded::Filter)?;
    let n = d.get(b"ColorSpace").and_then(|cs| components(doc, cs)).ok_or(NotDecoded::ColourSpace)?;
    let bpc = d.get(b"BitsPerComponent").map(|v| doc.resolve(v)).and_then(|v| v.as_int()).unwrap_or(0);
    let bilevel = match bpc {
        8 => false,
        1 if n == 1 => true,
        other => return Err(NotDecoded::BitDepth(other)),
    };
    let width = d.get(b"Width").map(|v| doc.resolve(v)).and_then(|v| v.as_int()).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
    let height = d.get(b"Height").map(|v| doc.resolve(v)).and_then(|v| v.as_int()).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(NotDecoded::Corrupt);
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(NotDecoded::TooLarge);
    }
    if bilevel {
        if jpeg {
            // A JPEG has 8 bits per sample: the dictionary contradicts the data.
            return Err(NotDecoded::Corrupt);
        }
        let inverted = match d.get(b"Decode") {
            None => false,
            Some(a) if is_identity_decode(doc, a, 1) => false,
            Some(a) if is_inverted_decode(doc, a) => true,
            Some(_) => return Err(NotDecoded::Decode),
        };
        let data = unpack_bits(s, width, height, inverted).ok_or(NotDecoded::Corrupt)?;
        return Ok(Samples { width, height, components: 1, data, jpeg: false, bilevel: true, inverted });
    }
    if d.get(b"Decode").is_some_and(|a| !is_identity_decode(doc, a, n)) {
        return Err(NotDecoded::Decode);
    }
    if jpeg && n == 3 && ycc_encoded(doc, s).is_none() {
        return Err(NotDecoded::Ambiguous);
    }
    let data = pixels(doc, s, n).ok_or(NotDecoded::Corrupt)?;
    Ok(Samples { width, height, components: n, data, jpeg, bilevel: false, inverted: false })
}

/// `[1 0]`: a bi-level image whose 1 bits are black.
fn is_inverted_decode(doc: &Document, decode: &Object) -> bool {
    let decode = doc.resolve(decode);
    matches!(decode.as_array().map(Vec::as_slice), Some([a, b]) if doc.resolve(a).as_f64() == Some(1.0) && doc.resolve(b).as_f64() == Some(0.0))
}

/// The bytes a `w` × `h` bi-level image takes: each row padded to whole bytes.
fn packed_len(w: u32, h: u32) -> Option<usize> {
    usize::try_from(w).ok()?.div_ceil(8).checked_mul(usize::try_from(h).ok()?)
}

/// A bi-level image's pixels, 0 (black) or 255 (white) as drawn.
fn unpack_bits(s: &Stream, w: u32, h: u32, inverted: bool) -> Option<Vec<u8>> {
    let stride = usize::try_from(w).ok()?.div_ceil(8);
    let need = packed_len(w, h)?;
    // Some writers pad the data; anything far longer than the image needs is not this image.
    let bits = s.decoded_within(need.saturating_add(need / 2).saturating_add(4096)).ok()?;
    let bits = bits.get(..need)?;
    let (w, h) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let mut out = Vec::with_capacity(w.checked_mul(h)?);
    for row in bits.chunks(stride).take(h) {
        for x in 0..w {
            let byte = row.get(x / 8).copied().unwrap_or(0);
            let one = byte & (0x80 >> (x % 8)) != 0;
            // Without a decode array 0 is black and 1 white; [1 0] swaps them.
            out.push(if one != inverted { 255 } else { 0 });
        }
    }
    Some(out)
}

/// The pixels (0 black … 255 white, threshold at half) packed back to bits.
fn pack_bits(px: &[u8], w: u32, h: u32, inverted: bool) -> Option<Vec<u8>> {
    let stride = usize::try_from(w).ok()?.div_ceil(8);
    let mut out = vec![0u8; packed_len(w, h)?];
    let (w, h) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    if px.len() != w.checked_mul(h)? {
        return None;
    }
    for (y, row) in px.chunks(w.max(1)).enumerate() {
        for (x, &v) in row.iter().enumerate() {
            let white = v >= 128;
            if white != inverted
                && let Some(b) = out.get_mut(y * stride + x / 8)
            {
                *b |= 0x80 >> (x % 8);
            }
        }
    }
    Some(out)
}

/// Image stream `old` with its samples replaced by `samples` (same size and components): a JPEG
/// stays a JPEG (at `quality`, 1–100), anything else becomes Flate, and a bi-level image stays
/// one bit per pixel with its decode array. Filter parameters go (the new data needs none); every
/// other entry stays. `None` if the samples don't fit the image.
pub fn encode_samples(old: &Stream, samples: &Samples, quality: u8) -> Option<Stream> {
    let Samples { width, height, components: n, data, jpeg: as_jpeg, bilevel, inverted } = samples;
    let (w, h, n) = (*width, *height, *n);
    if !(n == 1 || n == 3) || data.len() != usize::try_from(w).ok()?.checked_mul(usize::try_from(h).ok()?)?.checked_mul(n)? {
        return None;
    }
    let mut d = old.dict.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"Length", b"DL"] {
        d.remove(k);
    }
    if *bilevel {
        if n != 1 {
            return None;
        }
        d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        return Some(Stream::flate(d, &pack_bits(data, w, h, *inverted)?));
    }
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    if *as_jpeg {
        let bytes = jpeg(data, n, w, h, quality)?;
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Some(Stream::from_raw(d, bytes))
    } else {
        Some(Stream::flate(d, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Dict;

    fn image(dict: &[(&str, Object)], raw: Vec<u8>) -> Stream {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        for (k, v) in dict {
            d.set(k.as_bytes().to_vec(), v.clone());
        }
        Stream::from_raw(d, raw)
    }

    fn doc() -> Document {
        let pdf = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
        Document::open(std::sync::Arc::new(pdf.to_vec())).unwrap()
    }

    #[test]
    fn grey_samples_round_trip_losslessly_and_keep_the_other_entries() {
        let px: Vec<u8> = (0..12 * 5).map(|i| (i * 4) as u8).collect();
        let s = image(
            &[
                ("Width", Object::Int(12)),
                ("Height", Object::Int(5)),
                ("ColorSpace", Object::name("DeviceGray")),
                ("BitsPerComponent", Object::Int(8)),
                ("Interpolate", Object::Bool(true)),
            ],
            px.clone(),
        );
        let got = decode_samples(&doc(), &s).unwrap();
        assert_eq!((got.width, got.height, got.components, got.jpeg, got.bilevel), (12, 5, 1, false, false));
        assert_eq!(got.data, px);
        let back = encode_samples(&s, &got, 90).unwrap();
        assert_eq!(back.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
        assert_eq!(back.dict.get(b"Interpolate"), Some(&Object::Bool(true)));
        assert_eq!(decode_samples(&doc(), &back).unwrap().data, px);
    }

    #[test]
    fn bilevel_samples_keep_their_decode_array_and_bits() {
        // 10 × 2: first row black on the left half, second row all white; [1 0] makes 1 black.
        let raw = vec![0b1111_1000, 0b0000_0000, 0, 0];
        let s = image(
            &[
                ("Width", Object::Int(10)),
                ("Height", Object::Int(2)),
                ("ColorSpace", Object::name("DeviceGray")),
                ("BitsPerComponent", Object::Int(1)),
                ("Decode", Object::Array(vec![Object::Int(1), Object::Int(0)])),
            ],
            raw.clone(),
        );
        let got = decode_samples(&doc(), &s).unwrap();
        assert!(got.bilevel);
        assert_eq!(&got.data[..10], &[0, 0, 0, 0, 0, 255, 255, 255, 255, 255]);
        assert!(got.data[10..].iter().all(|&v| v == 255));
        assert!(got.inverted);
        let back = encode_samples(&s, &got, 90).unwrap();
        assert_eq!(back.dict.int(b"BitsPerComponent"), Some(1));
        assert!(back.dict.get(b"Decode").is_some());
        assert_eq!(back.decoded().unwrap(), raw);
    }

    #[test]
    fn what_is_not_supported_says_why_and_never_panics() {
        let base = |extra: &[(&str, Object)], raw: Vec<u8>| {
            let mut d: Vec<(&str, Object)> = vec![
                ("Width", Object::Int(4)),
                ("Height", Object::Int(4)),
                ("ColorSpace", Object::name("DeviceGray")),
                ("BitsPerComponent", Object::Int(8)),
            ];
            d.extend(extra.iter().cloned());
            decode_samples(&doc(), &image(&d, raw))
        };
        assert_eq!(base(&[("ColorSpace", Object::name("DeviceCMYK"))], vec![0; 64]), Err(NotDecoded::ColourSpace));
        assert_eq!(base(&[("BitsPerComponent", Object::Int(4))], vec![0; 8]), Err(NotDecoded::BitDepth(4)));
        assert_eq!(base(&[("Filter", Object::name("JBIG2Decode"))], vec![0; 8]), Err(NotDecoded::Filter));
        // JPEG 2000 may leave the colour space and bit depth to its data: the format is the reason.
        let jpx = |dict: &[(&str, Object)]| decode_samples(&doc(), &image(dict, vec![0; 8]));
        let size = [("Width", Object::Int(4)), ("Height", Object::Int(4)), ("Filter", Object::name("JPXDecode"))];
        assert_eq!(jpx(&size), Err(NotDecoded::Filter));
        assert_eq!(jpx(&[size[0].clone(), size[1].clone(), size[2].clone(), ("ColorSpace", Object::name("DeviceRGB"))]), Err(NotDecoded::Filter));
        assert_eq!(base(&[("ImageMask", Object::Bool(true))], vec![0; 8]), Err(NotDecoded::StencilMask));
        assert_eq!(base(&[("Decode", Object::Array(vec![Object::Int(1), Object::Int(0)]))], vec![0; 16]), Err(NotDecoded::Decode));
        // Too short, a huge size, a broken JPEG, no size at all.
        assert_eq!(base(&[], vec![0; 5]), Err(NotDecoded::Corrupt));
        assert_eq!(base(&[("Width", Object::Int(1 << 30)), ("Height", Object::Int(1 << 30))], vec![0; 16]), Err(NotDecoded::TooLarge));
        assert_eq!(base(&[("Filter", Object::name("DCTDecode"))], vec![0xFF, 0xD8, 1, 2, 3]), Err(NotDecoded::Corrupt));
        assert_eq!(base(&[("Width", Object::Int(-3))], vec![0; 16]), Err(NotDecoded::Corrupt));
        // Samples that don't fit the image are refused when written back.
        let s = image(&[("Width", Object::Int(4)), ("Height", Object::Int(4))], vec![0; 16]);
        let wrong = Samples { width: 4, height: 4, components: 1, data: vec![0; 15], jpeg: false, bilevel: false, inverted: false };
        assert!(encode_samples(&s, &wrong, 90).is_none());
    }
}
