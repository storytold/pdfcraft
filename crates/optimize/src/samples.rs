//! An image's samples read out and written back at the same size, for tools that change the
//! pixels of an image in place (Scan & OCR ▸ Enhance ▸ Scanned document). The image dictionary
//! stays as it was apart from the encoding: its size, colour space, masks, `/Interpolate`,
//! `/Intent`, `/Metadata` and every other entry are kept.

use pdfcraft_cos::{Document, ImageBudget, Object, Stream};

use crate::images::{MAX_PIXELS, allowance, components, is_dct, is_identity_decode, jpeg, pixels, ycc_encoded};

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
    #[error("it is compressed in a format PdfCraft cannot read back yet (JBIG2, CCITT fax, or JPEG 2000 behind another filter)")]
    Filter,
    #[error("its JPEG colour transform is ambiguous, so readers may disagree about its colours")]
    Ambiguous,
    #[error("it is larger than 80 million pixels")]
    TooLarge,
    #[error(
        "it is a JPEG 2000 image too large to read here (more than 40 million pixels, or 20 million in colour, or more tiles than a real image has)"
    )]
    JpxTooLarge,
    #[error("it has an alpha channel (transparency stored in the JPEG 2000 data), which a JPEG cannot keep")]
    Alpha,
    #[error("reading it would go past the work allowed for one run")]
    OverBudget,
    #[error("its data could not be decoded, or is shorter than its size says")]
    Corrupt,
}

/// Read the samples of image stream `s` (a dictionary with `/Subtype /Image`), decoding it within
/// `budget` (an operation over many images shares one).
pub fn decode_samples(doc: &Document, s: &Stream, budget: &mut ImageBudget) -> Result<Samples, NotDecoded> {
    let d = &s.dict;
    if matches!(d.get(b"ImageMask").map(|v| doc.resolve(v)).as_deref(), Some(Object::Bool(true))) {
        return Err(NotDecoded::StencilMask);
    }
    // The filter first: a JPEG 2000 image may leave its colour space and bit depth to the data,
    // and the report should name the format, not the missing entries.
    if is_jpx(s) {
        return jpx_samples(doc, s, budget);
    }
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
        let data = unpack_bits(doc, s, width, height, inverted, budget).ok_or(NotDecoded::Corrupt)?;
        return Ok(Samples { width, height, components: 1, data, jpeg: false, bilevel: true, inverted });
    }
    if d.get(b"Decode").is_some_and(|a| !is_identity_decode(doc, a, n)) {
        return Err(NotDecoded::Decode);
    }
    if jpeg && n == 3 && ycc_encoded(doc, s).is_none() {
        return Err(NotDecoded::Ambiguous);
    }
    let data = pixels(doc, s, n, budget).ok_or(NotDecoded::Corrupt)?;
    Ok(Samples { width, height, components: n, data, jpeg, bilevel: false, inverted: false })
}

/// Whether `s` is a JPEG 2000 image: `/JPXDecode` as its only filter.
fn is_jpx(s: &Stream) -> bool {
    match s.dict.get(b"Filter") {
        Some(Object::Name(n)) => n.as_slice() == b"JPXDecode",
        Some(Object::Array(a)) => matches!(a.as_slice(), [f] if f.as_name() == Some(b"JPXDecode")),
        _ => false,
    }
}

/// The most pixels a JPEG 2000 image is read at: half of what other images may have, because its
/// decoder holds every sample as a float (about ten bytes a sample at its peak).
const JPX_MAX_PIXELS: u64 = MAX_PIXELS / 2;
/// And the most samples (pixels times components), so a colour image is held to the same memory
/// as a grey one and a half: about 600 MB at the decoder's peak. Running out of memory is not a
/// panic that can be caught, so the limit has to come first.
const JPX_MAX_SAMPLES: u64 = 60_000_000;

/// The samples of a JPEG 2000 image: 8-bit grey or RGB without an alpha channel or a palette,
/// whose size is the dictionary's. Written back as a JPEG, since the pages' scans are JPEG 2000
/// only for size and PdfCraft has no JPEG 2000 encoder.
///
/// What the decoder cannot be trusted with is refused from the data's own header before it sees
/// any of it ([`jpx_head`]), and a panic inside it is that image's corrupt data, not the caller's
/// crash. The header checks come first because a browser build cannot catch a panic.
fn jpx_samples(doc: &Document, s: &Stream, budget: &mut ImageBudget) -> Result<Samples, NotDecoded> {
    let head = jpx_head(&s.raw).ok_or(NotDecoded::Corrupt)?;
    // A palette makes it an indexed image, and the decoder does not check a palette against the
    // colour space it goes with.
    if head.palette {
        return Err(NotDecoded::ColourSpace);
    }
    let n = match head.components {
        1 => 1,
        3 => 3,
        // Grey or RGB with one more channel.
        2 | 4 => return Err(NotDecoded::Alpha),
        _ => return Err(NotDecoded::ColourSpace),
    };
    // Every component's depth, not the first one's: a deeper one would be scaled to eight bits.
    if head.depth > 8 {
        return Err(NotDecoded::BitDepth(i64::from(head.depth)));
    }
    // The decoder makes a structure for every component of every tile before it reads one: a tile
    // index is 16 bits, so more tiles than that are no real image, only a small file asking for a
    // great deal of memory.
    if head.tiles > u64::from(u16::MAX) {
        return Err(NotDecoded::JpxTooLarge);
    }
    // The size is the dictionary's, which the data must agree with: the rewritten image needs it.
    let d = &s.dict;
    let side = |key: &[u8]| d.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_int()).and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    let (Some(width), Some(height)) = (side(b"Width"), side(b"Height")) else { return Err(NotDecoded::Corrupt) };
    let pixels = u64::from(width) * u64::from(height);
    if pixels > JPX_MAX_PIXELS || pixels.saturating_mul(n as u64) > JPX_MAX_SAMPLES {
        return Err(NotDecoded::JpxTooLarge);
    }
    let named = d.get(b"ColorSpace").map(|cs| components(doc, cs));
    if named.is_some_and(|c| c != Some(n)) {
        return Err(NotDecoded::ColourSpace);
    }
    // A colour space the dictionary names takes the place of the data's own, and the samples are
    // then read as they are. The decoder converts sYCC and CIELab data to RGB whatever the
    // dictionary says, so only data it leaves alone is read with a named space: sRGB, greyscale,
    // a profile, or a bare codestream.
    if named.is_some() && head.enumerated.is_some_and(|space| !matches!(space, 16 | 17)) {
        return Err(NotDecoded::ColourSpace);
    }
    if d.get(b"Decode").is_some_and(|a| !is_identity_decode(doc, a, n)) {
        return Err(NotDecoded::Decode);
    }
    let read = || jpx_decode(&s.raw, (width, height), n, named.is_some(), budget);
    // The budget is spent whether or not the decoder panics, and nothing else is shared with it.
    let data = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).unwrap_or(Err(NotDecoded::Corrupt))?;
    Ok(Samples { width, height, components: n, data, jpeg: true, bilevel: false, inverted: false })
}

/// The decoder's part of [`jpx_samples`]: `raw` as `size` pixels of `n` 8-bit components.
/// `named` is whether the image dictionary names the colour space.
fn jpx_decode(raw: &[u8], size: (u32, u32), n: usize, named: bool, budget: &mut ImageBudget) -> Result<Vec<u8>, NotDecoded> {
    use hayro_jpeg2000::{ColorSpace, DecodeSettings, Image};
    let image = Image::new(raw, &DecodeSettings::default()).map_err(|_| NotDecoded::Corrupt)?;
    if (image.width(), image.height()) != size {
        return Err(NotDecoded::Corrupt);
    }
    let channels = match image.color_space() {
        ColorSpace::Gray => 1,
        ColorSpace::RGB => 3,
        // A colour space the dictionary names is the image's, and the data's own profile is
        // ignored (ISO 32000-2, 8.9.5.1): the samples are read as they are. With no name in the
        // dictionary the profile is the only word on the colours, and a JPEG could not carry it.
        ColorSpace::Icc { num_channels, .. } if named => usize::from(*num_channels),
        _ => return Err(NotDecoded::ColourSpace),
    };
    if channels != n {
        return Err(NotDecoded::ColourSpace);
    }
    if image.has_alpha() {
        return Err(NotDecoded::Alpha);
    }
    if image.original_bit_depth() > 8 {
        return Err(NotDecoded::BitDepth(i64::from(image.original_bit_depth())));
    }
    let samples = u64::from(size.0).saturating_mul(u64::from(size.1)).saturating_mul(n as u64);
    // The run's shared allowance is spent, which says nothing of this image's size.
    if !budget.pay_jpeg(samples, raw) {
        return Err(NotDecoded::OverBudget);
    }
    let data = image.decode().map_err(|_| NotDecoded::Corrupt)?;
    if u64::try_from(data.len()).ok() != Some(samples) {
        return Err(NotDecoded::Corrupt);
    }
    Ok(data)
}

/// The first two markers of a JPEG 2000 codestream: SOC, then SIZ.
const JPX_CODESTREAM: [u8; 4] = [0xff, 0x4f, 0xff, 0x51];

/// What JPEG 2000 data says of itself, read without the decoder.
#[derive(Debug, PartialEq, Eq)]
struct JpxHead {
    /// The most tiles and components a codestream has, and the most bits a component has.
    tiles: u64,
    components: u16,
    depth: u8,
    /// A JP2 file with a palette (`pclr`) box.
    palette: bool,
    /// The colour space a JP2 file's first colour box names by number (15444-1, table I.10):
    /// 16 is sRGB and 17 greyscale. `None` for a bare codestream, or a box with a profile.
    enumerated: Option<u32>,
}

/// The tiles and components the codestream `cs` says it has, and the bits of its deepest
/// component, from its SIZ segment: the image's far corner, less the tile grid's origin, over the
/// tile size; `Csiz` after those; then three bytes a component, the first its depth less one.
fn codestream_head(cs: &[u8]) -> Option<(u64, u16, u8)> {
    if !cs.starts_with(&JPX_CODESTREAM) {
        return None;
    }
    let word = |i: usize| Some(u64::from(u32::from_be_bytes(cs.get(8 + 4 * i..12 + 4 * i)?.try_into().ok()?)));
    let along = |end: u64, tile: u64, origin: u64| (tile > 0 && end > origin).then(|| (end - origin).div_ceil(tile));
    let tiles = along(word(0)?, word(4)?, word(6)?)?.checked_mul(along(word(1)?, word(5)?, word(7)?)?)?;
    let components = u16::from_be_bytes(cs.get(40..42)?.try_into().ok()?);
    let depths = cs.get(42..42 + 3 * usize::from(components))?.iter().step_by(3);
    let depth = depths.map(|ssiz| (ssiz & 0x7f) + 1).max()?;
    Some((tiles, components, depth))
}

/// The boxes of JP2 data, as (type, contents), read as the decoder reads them. A length of 0 is
/// "to the end of the data" and of 1 "in the next eight bytes"; a box that does not fit ends it.
/// A `None` item is a box of 4 GiB or more, which the decoder misreads: it casts the length to
/// `usize`, so on a 32-bit build the length wraps and it reads on from inside the box.
fn jp2_boxes(data: &[u8]) -> impl Iterator<Item = Option<(&[u8], &[u8])>> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let head = data.get(at..at.checked_add(8)?)?;
        let (start, len) = match u32::from_be_bytes(head.get(..4)?.try_into().ok()?) {
            0 => (at + 8, None),
            1 => (at.checked_add(16)?, Some(u64::from_be_bytes(data.get(at + 8..at + 16)?.try_into().ok()?))),
            n => (at + 8, Some(u64::from(n))),
        };
        let end = match len.map(|n| usize::try_from(n).ok().filter(|_| n < 1 << 32)) {
            None => data.len(),
            Some(Some(n)) => at.checked_add(n)?,
            Some(None) => {
                at = data.len();
                return Some(None);
            }
        };
        let body = data.get(start..end)?;
        at = end;
        Some(Some((head.get(4..)?, body)))
    })
}

/// [`JpxHead`] of the JPEG 2000 data `raw`: `raw` itself when it is a bare codestream, else every
/// `jp2c` box of the JP2 file (the decoder reads each one), found by walking its boxes, not by
/// looking for the markers, which other boxes may hold. `None` when there is no codestream, or a
/// box is one the decoder misreads.
fn jpx_head(raw: &[u8]) -> Option<JpxHead> {
    if raw.starts_with(&JPX_CODESTREAM) {
        let (tiles, components, depth) = codestream_head(raw)?;
        return Some(JpxHead { tiles, components, depth, palette: false, enumerated: None });
    }
    let (mut most, mut palette, mut colour) = (None, false, None);
    for item in jp2_boxes(raw) {
        let (kind, body) = item?;
        match kind {
            b"jp2c" => {
                let (tiles, components, depth) = codestream_head(body)?;
                let (t, c, d) = most.unwrap_or((0, 0, 0));
                most = Some((tiles.max(t), components.max(c), depth.max(d)));
            }
            b"jp2h" => {
                // The decoder goes by the last header box and, in it, the first colour box. A
                // palette in any header is refused all the same.
                colour = None;
                for item in jp2_boxes(body) {
                    let (kind, body) = item?;
                    palette |= kind == b"pclr";
                    if kind == b"colr" && colour.is_none() {
                        colour = Some(body);
                    }
                }
            }
            _ => {}
        }
    }
    // A method of 1 names the space by number, in the four bytes after the box's three.
    let enumerated = colour.filter(|c| c.first() == Some(&1)).and_then(|c| Some(u32::from_be_bytes(c.get(3..7)?.try_into().ok()?)));
    most.map(|(tiles, components, depth)| JpxHead { tiles, components, depth, palette, enumerated })
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

/// A bi-level image's pixels, 0 (black) or 255 (white) as drawn, decoded within `budget` with
/// its `/DecodeParms` resolved, as the 8-bit images are.
fn unpack_bits(doc: &Document, s: &Stream, w: u32, h: u32, inverted: bool, budget: &mut ImageBudget) -> Option<Vec<u8>> {
    let stride = usize::try_from(w).ok()?.div_ceil(8);
    let need = packed_len(w, h)?;
    let bits = budget.decode_filters(doc, s, allowance(need, h)?).ok()?;
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
    // A JPEG 2000 image may leave its colour space to the data: the new encoding needs it said.
    // The data's sRGB or greyscale becomes the device space, which is how a reader without colour
    // management shows it, not the same space by name.
    if d.get(b"ColorSpace").is_none() {
        d.set(b"ColorSpace".to_vec(), Object::name(if n == 1 { "DeviceGray" } else { "DeviceRGB" }));
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
        let got = decode_samples(&doc(), &s, &mut ImageBudget::with_work(u64::MAX)).unwrap();
        assert_eq!((got.width, got.height, got.components, got.jpeg, got.bilevel), (12, 5, 1, false, false));
        assert_eq!(got.data, px);
        let back = encode_samples(&s, &got, 90).unwrap();
        assert_eq!(back.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
        assert_eq!(back.dict.get(b"Interpolate"), Some(&Object::Bool(true)));
        assert_eq!(decode_samples(&doc(), &back, &mut ImageBudget::with_work(u64::MAX)).unwrap().data, px);
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
        let got = decode_samples(&doc(), &s, &mut ImageBudget::with_work(u64::MAX)).unwrap();
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
    fn bilevel_decode_parms_by_reference_decode_like_direct_ones() {
        // 10 × 2 with a TIFF predictor: each 1 bit flips the pixel from the one to its left.
        let packed = vec![0b1000_0000, 0b0000_0000, 0b0100_0000, 0b0000_0000];
        let parms = || {
            let mut p = pdfcraft_cos::Dict::new();
            for (k, v) in [("Predictor", 2), ("Colors", 1), ("BitsPerComponent", 1), ("Columns", 10)] {
                p.set(k.as_bytes().to_vec(), Object::Int(v));
            }
            Object::Dict(p)
        };
        let dict = [
            ("Width", Object::Int(10)),
            ("Height", Object::Int(2)),
            ("ColorSpace", Object::name("DeviceGray")),
            ("BitsPerComponent", Object::Int(1)),
        ];
        let mut d = doc();
        let mut direct = Stream::flate(image(&dict, vec![]).dict, &packed);
        direct.dict.set(b"DecodeParms".to_vec(), parms());
        let mut indirect = direct.clone();
        let r = d.add(parms());
        indirect.dict.set(b"DecodeParms".to_vec(), Object::Ref(r));
        let a = decode_samples(&d, &direct, &mut ImageBudget::with_work(u64::MAX)).unwrap();
        let b = decode_samples(&d, &indirect, &mut ImageBudget::with_work(u64::MAX)).unwrap();
        // Row 1: white from x = 0 on; row 2: black at x = 0, white from x = 1 on.
        assert_eq!(&a.data[..10], &[255; 10]);
        assert_eq!(&a.data[10..], &[0, 255, 255, 255, 255, 255, 255, 255, 255, 255]);
        assert_eq!(b, a);
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
            decode_samples(&doc(), &image(&d, raw), &mut ImageBudget::with_work(u64::MAX))
        };
        assert_eq!(base(&[("ColorSpace", Object::name("DeviceCMYK"))], vec![0; 64]), Err(NotDecoded::ColourSpace));
        assert_eq!(base(&[("BitsPerComponent", Object::Int(4))], vec![0; 8]), Err(NotDecoded::BitDepth(4)));
        assert_eq!(base(&[("Filter", Object::name("JBIG2Decode"))], vec![0; 8]), Err(NotDecoded::Filter));
        // JPEG 2000 data that is no JPEG 2000 is corrupt, whatever its dictionary says.
        let jpx = |dict: &[(&str, Object)]| decode_samples(&doc(), &image(dict, vec![0; 8]), &mut ImageBudget::with_work(u64::MAX));
        let size = [("Width", Object::Int(4)), ("Height", Object::Int(4)), ("Filter", Object::name("JPXDecode"))];
        assert_eq!(jpx(&size), Err(NotDecoded::Corrupt));
        assert_eq!(jpx(&[size[0].clone(), size[1].clone(), size[2].clone(), ("ColorSpace", Object::name("DeviceRGB"))]), Err(NotDecoded::Corrupt));
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

    /// Contributor-original JPEG 2000: a 1 × 1 image of `components` 8-bit components, every
    /// sample 128 (an empty packet decodes to the midpoint).
    fn jpx_codestream(components: u16) -> Vec<u8> {
        let mut c = vec![0xff, 0x4f, 0xff, 0x51];
        c.extend_from_slice(&(38 + 3 * components).to_be_bytes());
        c.extend_from_slice(&0_u16.to_be_bytes());
        for v in [1_u32, 1, 0, 0, 1, 1, 0, 0] {
            c.extend_from_slice(&v.to_be_bytes());
        }
        c.extend_from_slice(&components.to_be_bytes());
        for _ in 0..components {
            c.extend_from_slice(&[7, 1, 1]);
        }
        c.extend_from_slice(&[0xff, 0x52, 0, 12, 0, 0, 0, 1, 0, 0, 4, 4, 0, 1]);
        c.extend_from_slice(&[0xff, 0x5c, 0, 4, 0x40, 0x40]);
        c.extend_from_slice(&[0xff, 0x90, 0, 10, 0, 0]);
        c.extend_from_slice(&(14 + u32::from(components)).to_be_bytes());
        c.extend_from_slice(&[0, 1, 0xff, 0x93]);
        c.extend(std::iter::repeat_n(0, usize::from(components)));
        c.extend_from_slice(&[0xff, 0xd9]);
        c
    }

    #[test]
    fn jpeg_2000_scans_are_read_and_written_back_as_jpegs_with_a_colour_space() {
        let budget = || ImageBudget::with_work(u64::MAX);
        for (n, space) in [(1_u16, "DeviceGray"), (3, "DeviceRGB")] {
            // The data names its colour space: the dictionary may leave it out.
            let s = image(&[("Width", Object::Int(1)), ("Height", Object::Int(1)), ("Filter", Object::name("JPXDecode"))], jpx_codestream(n));
            let got = decode_samples(&doc(), &s, &mut budget()).unwrap();
            assert_eq!((got.width, got.height, got.components, got.jpeg, got.bilevel), (1, 1, usize::from(n), true, false));
            assert_eq!(got.data, vec![128; usize::from(n)]);
            let back = encode_samples(&s, &got, 90).unwrap();
            assert_eq!(back.dict.name(b"Filter"), Some(&b"DCTDecode"[..]));
            assert_eq!(back.dict.name(b"ColorSpace"), Some(space.as_bytes()));
            assert_eq!(decode_samples(&doc(), &back, &mut budget()).unwrap().components, usize::from(n));
        }
        // A dictionary that contradicts the data is refused, not trusted.
        let mut d = vec![("Filter", Object::name("JPXDecode")), ("Width", Object::Int(1)), ("Height", Object::Int(1))];
        let rgb = |extra: (&str, Object)| {
            let mut d = d.clone();
            d.push(extra);
            decode_samples(&doc(), &image(&d, jpx_codestream(3)), &mut budget())
        };
        assert_eq!(rgb(("ColorSpace", Object::name("DeviceGray"))), Err(NotDecoded::ColourSpace));
        assert_eq!(rgb(("Decode", Object::Array(vec![Object::Int(1); 6]))), Err(NotDecoded::Decode));
        d[1] = ("Width", Object::Int(2));
        assert_eq!(decode_samples(&doc(), &image(&d, jpx_codestream(3)), &mut budget()), Err(NotDecoded::Corrupt));
        // 1024 × 1024 in tiles of one pixel: a million tiles, refused before any is made.
        let mut tiled = jpx_codestream(1);
        tiled[8..16].copy_from_slice(&[0, 0, 4, 0, 0, 0, 4, 0]);
        assert_eq!(jpx_head(&tiled).map(|h| h.tiles), Some(1 << 20));
        d[1] = ("Width", Object::Int(1024));
        d[2] = ("Height", Object::Int(1024));
        assert_eq!(decode_samples(&doc(), &image(&d, tiled.clone()), &mut budget()), Err(NotDecoded::JpxTooLarge));
        // In a JP2 file the codestream is its `jp2c` box, not the first place the markers show up:
        // a small one tucked into another box does not hide the tiles of the real one.
        let boxed = |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32 + 8).to_be_bytes()[..], kind, data].concat();
        let mut jp2 = boxed(b"jP  ", &[0x0d, 0x0a, 0x87, 0x0a]);
        jp2.extend(boxed(b"free", &jpx_codestream(1)));
        jp2.extend(boxed(b"jp2c", &tiled));
        assert_eq!(jpx_head(&jp2).map(|h| h.tiles), Some(1 << 20));
        assert_eq!(jpx_head(&boxed(b"free", &tiled)), None, "no codestream box");
        assert_eq!(jpx_head(&[&[0u8][..], &tiled].concat()), None, "a bare codestream starts with its markers");
        // Out of budget.
        let one = [("Filter", Object::name("JPXDecode")), ("Width", Object::Int(1)), ("Height", Object::Int(1))];
        assert_eq!(decode_samples(&doc(), &image(&one, jpx_codestream(1)), &mut ImageBudget::with_work(0)), Err(NotDecoded::OverBudget));
        // The size is the dictionary's: without it the rewritten image would have none.
        assert_eq!(decode_samples(&doc(), &image(&one[..2], jpx_codestream(1)), &mut budget()), Err(NotDecoded::Corrupt));
        // More than 40 million pixels is refused from the dictionary, before the decoder is asked.
        let huge = [one[0].clone(), ("Width", Object::Int(8944)), ("Height", Object::Int(8944))];
        assert_eq!(decode_samples(&doc(), &image(&huge, jpx_codestream(3)), &mut budget()), Err(NotDecoded::JpxTooLarge));
        // 25 million pixels are 75 million samples in colour, past the 60 million the decoder is
        // given; in grey they are within both limits (and here not the data's size).
        let large = [one[0].clone(), ("Width", Object::Int(5000)), ("Height", Object::Int(5000))];
        assert_eq!(decode_samples(&doc(), &image(&large, jpx_codestream(3)), &mut budget()), Err(NotDecoded::JpxTooLarge));
        assert_eq!(decode_samples(&doc(), &image(&large, jpx_codestream(1)), &mut budget()), Err(NotDecoded::Corrupt));
    }

    /// A JP2 file: the signature and file type boxes, a header box of `header` boxes, and the
    /// codestream.
    fn jp2(header: &[Vec<u8>], codestream: &[u8]) -> Vec<u8> {
        let boxed = |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32 + 8).to_be_bytes()[..], kind, data].concat();
        [boxed(b"jP  ", &[0x0d, 0x0a, 0x87, 0x0a]), boxed(b"ftyp", b"jp2 \0\0\0\0jp2 "), boxed(b"jp2h", &header.concat()), boxed(b"jp2c", codestream)]
            .concat()
    }

    fn jp2_box(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        [&(data.len() as u32 + 8).to_be_bytes()[..], kind, data].concat()
    }

    /// What the JPEG 2000 decoder is not to be handed is refused from the data's own header, with
    /// a reason, before it is decoded (#6811 review: these panicked in the decoder or asked it for
    /// memory without bound).
    #[test]
    fn jpeg_2000_data_the_decoder_cannot_be_trusted_with_is_refused_from_its_header() {
        let budget = || ImageBudget::with_work(u64::MAX);
        let one = [("Filter", Object::name("JPXDecode")), ("Width", Object::Int(1)), ("Height", Object::Int(1))];
        let read = |raw: Vec<u8>| decode_samples(&doc(), &image(&one, raw), &mut budget());
        // A grey image with a palette of three columns: the decoder takes the palette for the
        // channels and the colour box for their meaning, and they disagree.
        let grey = jp2_box(b"colr", &[1, 0, 0, 0, 0, 0, 17]);
        let palette = jp2_box(b"pclr", &[&[1u8, 0, 3, 7, 7, 7][..], &[9u8; 256 * 3]].concat());
        let hostile = jp2(&[grey.clone(), palette.clone()], &jpx_codestream(1));
        assert_eq!(jpx_head(&hostile), Some(JpxHead { tiles: 1, components: 1, depth: 8, palette: true, enumerated: Some(17) }));
        assert_eq!(read(hostile), Err(NotDecoded::ColourSpace));
        // The same file without the palette is an ordinary grey image.
        assert_eq!(read(jp2(std::slice::from_ref(&grey), &jpx_codestream(1))).map(|s| s.data), Ok(vec![128]));
        // A box of 4 GiB that is empty on a 32-bit build, where the decoder's length wraps to 0
        // and it reads the palette after it, is refused on every build.
        let wraps = [&1u32.to_be_bytes()[..], b"free", &((1u64 << 32) + 16).to_be_bytes()].concat();
        let hidden = jp2(&[grey, wraps, palette], &jpx_codestream(1));
        assert_eq!(jpx_head(&hidden), None);
        assert_eq!(read(hidden), Err(NotDecoded::Corrupt));
        // An alpha channel says so; a component count no grey or RGB image has is no such image,
        // however few tiles it has (the decoder allocates for every component of every tile).
        assert_eq!(read(jpx_codestream(2)), Err(NotDecoded::Alpha));
        assert_eq!(read(jpx_codestream(4)), Err(NotDecoded::Alpha));
        assert_eq!(jpx_head(&jpx_codestream(16384)).map(|h| h.components), Some(16384));
        assert_eq!(read(jpx_codestream(16384)), Err(NotDecoded::ColourSpace));
        // Every component's depth counts, not the first one's: here the second has 12 bits.
        let mut deep = jpx_codestream(3);
        deep[45] = 11;
        assert_eq!(jpx_head(&deep).map(|h| h.depth), Some(12));
        assert_eq!(read(deep), Err(NotDecoded::BitDepth(12)));
        // JPEG 2000 behind another filter is not read, and the reason names it.
        let chain = [("Filter", Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("JPXDecode")])), one[1].clone(), one[2].clone()];
        let refused = decode_samples(&doc(), &image(&chain, jpx_codestream(1)), &mut budget());
        assert_eq!(refused, Err(NotDecoded::Filter));
        assert!(NotDecoded::Filter.to_string().contains("JPEG 2000 behind another filter"));
    }

    /// A JP2 file with an ICC profile of its own is cleaned when the dictionary names the colour
    /// space, which then is the image's; without a name the profile would be lost, so it is left.
    #[test]
    fn a_jpeg_2000_scan_with_its_own_profile_is_read_when_the_dictionary_names_the_colours() {
        let mut profile = vec![0u8; 128];
        profile[16..20].copy_from_slice(b"RGB ");
        let raw = jp2(&[jp2_box(b"colr", &[&[2u8, 0, 0][..], &profile].concat())], &jpx_codestream(3));
        let mut d = vec![("Filter", Object::name("JPXDecode")), ("Width", Object::Int(1)), ("Height", Object::Int(1))];
        let read = |d: &[(&str, Object)]| decode_samples(&doc(), &image(d, raw.clone()), &mut ImageBudget::with_work(u64::MAX));
        assert_eq!(read(&d), Err(NotDecoded::ColourSpace));
        d.push(("ColorSpace", Object::name("DeviceRGB")));
        assert_eq!(read(&d).map(|s| (s.components, s.data)), Ok((3, vec![128; 3])));
        // Data the decoder converts to RGB (sYCC is 18) is not read with a named space, which
        // says to take the samples as they are; sRGB (16) needs no conversion. With no name the
        // data's own space is the image's, and the conversion is right.
        let boxed = |space: u8| jp2(&[jp2_box(b"colr", &[1, 0, 0, 0, 0, 0, space])], &jpx_codestream(3));
        let read = |d: &[(&str, Object)], space: u8| decode_samples(&doc(), &image(d, boxed(space)), &mut ImageBudget::with_work(u64::MAX));
        assert_eq!(read(&d, 18), Err(NotDecoded::ColourSpace));
        assert_eq!(read(&d, 16).map(|s| s.components), Ok(3));
        assert_eq!(read(&d[..3], 18).map(|s| s.components), Ok(3));
        // With two header boxes the decoder goes by the last: an sRGB one in front does not hide
        // the sYCC one it reads, and an sYCC one in front does not count.
        let colr = |space: u8| jp2_box(b"jp2h", &jp2_box(b"colr", &[1, 0, 0, 0, 0, 0, space]));
        let two = |first: u8, last: u8| {
            let ends = [jp2_box(b"jP  ", &[0x0d, 0x0a, 0x87, 0x0a]), jp2_box(b"ftyp", b"jp2 \0\0\0\0jp2 ")].concat();
            [ends, colr(first), colr(last), jp2_box(b"jp2c", &jpx_codestream(3))].concat()
        };
        let read = |raw: Vec<u8>| decode_samples(&doc(), &image(&d, raw), &mut ImageBudget::with_work(u64::MAX));
        assert_eq!(jpx_head(&two(16, 18)).and_then(|h| h.enumerated), Some(18));
        assert_eq!(read(two(16, 18)), Err(NotDecoded::ColourSpace));
        assert_eq!(read(two(18, 16)).map(|s| s.components), Ok(3));
    }
}
