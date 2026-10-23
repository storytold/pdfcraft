//! Export a PDF ▸ Image ▸ Export all images, and Edit ▸ Save image as: the images that pages
//! use, as files. JPEG images are written unchanged when the file shows the colours the page
//! does; other images, and JPEG images with transparency, CMYK samples or an ICC profile the
//! file doesn't carry, are decoded and written as PNG with their soft mask, stencil mask or
//! colour key as alpha. Decoded colours are converted to sRGB the way the page renderer draws
//! them (`pdfcraft_render::cmyk_to_srgb`, `IccProfile`). Images PdfCraft can't decode yet
//! (JPEG 2000, JBIG2, CCITT, separations) are reported, never silently left out.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_render::IccProfile;

/// One image, ready to write.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedImage {
    /// 0-based page the image was first found on.
    pub page: usize,
    pub object: ObjRef,
    pub width: u32,
    pub height: u32,
    /// "jpg" or "png".
    pub extension: &'static str,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageExport {
    pub images: Vec<ExtractedImage>,
    /// Images left out: (page, object, why).
    pub skipped: Vec<(usize, ObjRef, String)>,
}

/// Images larger than this many pixels are skipped (memory).
const MAX_PIXELS: u64 = 100_000_000;

/// The images of `pages` (0-based; each image once, on the first page using it), skipping
/// those with fewer than `min_side` pixels on their shorter side.
pub fn extract_images(doc: &Document, pages: &[usize], min_side: u32) -> ImageExport {
    let all = pdfcraft_model::pages(doc);
    let mut out = ImageExport::default();
    let mut seen = HashSet::new();
    for &p in pages {
        let Some(page) = all.get(p) else { continue };
        let mut refs = Vec::new();
        if let Some(res) = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()) {
            collect(doc, &res, 0, &mut HashSet::new(), &mut refs);
        }
        for r in refs {
            if !seen.insert(r) {
                continue;
            }
            let obj = doc.get(r);
            let Object::Stream(s) = &*obj else { continue };
            let (w, h) = (s.dict.int(b"Width").unwrap_or(0), s.dict.int(b"Height").unwrap_or(0));
            if w <= 0 || h <= 0 || (w.min(h) as u64) < min_side as u64 {
                continue;
            }
            match image(doc, s) {
                Ok((extension, data)) => out.images.push(ExtractedImage { page: p, object: r, width: w as u32, height: h as u32, extension, data }),
                Err(why) => out.skipped.push((p, r, why)),
            }
        }
    }
    out
}

/// One image XObject as a file: JPEG as is, else PNG ("Save image as").
pub fn image_file(doc: &Document, image: ObjRef) -> Result<(&'static str, Vec<u8>), String> {
    match &*doc.get(image) {
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => self::image(doc, s),
        _ => Err("not an image".into()),
    }
}

/// The image XObjects reachable from `res`, through form XObjects, in resource order.
fn collect(doc: &Document, res: &Dict, depth: usize, forms: &mut HashSet<ObjRef>, out: &mut Vec<ObjRef>) {
    if depth > 12 {
        return;
    }
    let Some(xobjects) = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { return };
    for (_, v) in xobjects.iter() {
        let Some(r) = v.as_ref() else { continue };
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        match s.dict.name(b"Subtype") {
            Some(b"Image") => out.push(r),
            Some(b"Form") if forms.insert(r) => {
                if let Some(inner) = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) {
                    collect(doc, &inner, depth + 1, forms, out);
                }
            }
            _ => {}
        }
    }
}

fn filters(s: &Stream) -> Vec<Vec<u8>> {
    match s.dict.get(b"Filter") {
        Some(Object::Name(n)) => vec![n.to_vec()],
        Some(Object::Array(a)) => a.iter().filter_map(|f| f.as_name().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

/// The file for one image: its JPEG data as is, or a PNG. An image with transparency (a soft
/// mask, a stencil `/Mask` or a colour-key `/Mask`) is always a PNG with that transparency as
/// alpha, JPEG images included, so cut-out images keep their transparent background.
fn image(doc: &Document, s: &Stream) -> Result<(&'static str, Vec<u8>), String> {
    let f = filters(s);
    let jpeg = matches!(f.last().map(Vec::as_slice), Some(b"DCTDecode" | b"DCT")) && f.len() == 1;
    match f.last().map(Vec::as_slice) {
        Some(b"JPXDecode") => return Err("JPEG 2000 images can't be exported yet".into()),
        Some(b"JBIG2Decode") => return Err("JBIG2 images can't be exported yet".into()),
        Some(b"CCITTFaxDecode" | b"CCF") => return Err("CCITT fax images can't be exported yet".into()),
        Some(b"DCTDecode" | b"DCT") if !jpeg => return Err("JPEG images inside other filters can't be exported yet".into()),
        _ => {}
    }
    // The JPEG file as it is in the PDF. The page draws a grey or RGB JPEG's samples as they are
    // and ignores an ICC profile inside the file, which a viewer would apply: it is left out.
    let as_is = || {
        let device = matches!(space(doc, s.dict.get(b"ColorSpace")), Ok(Space::Gray | Space::Rgb));
        Ok(("jpg", if device { without_icc_profile(&s.raw).unwrap_or_else(|| s.raw.to_vec()) } else { s.raw.to_vec() }))
    };
    let (w, h) = (s.dict.int(b"Width").unwrap_or(0), s.dict.int(b"Height").unwrap_or(0));
    let Some((w, h)) = size(w, h) else {
        return match () {
            _ if jpeg => as_is(),
            _ if w <= 0 || h <= 0 => Err("the image has no size".into()),
            _ => Err(format!("{w} × {h} is too large to export")),
        };
    };
    let mask = s.dict.get(b"ImageMask").is_some_and(|m| matches!(&*doc.resolve(m), Object::Bool(true)));
    // A stencil mask (`ImageMask true`) has no /SMask or /Mask of its own (8.9.6.2).
    let mut alpha = if mask { None } else { soft_mask(doc, s, w, h).or_else(|| stencil_mask(doc, s, w, h)) };
    let key = if mask { None } else { colour_key(doc, s) };
    // Keep the existing JPEG and stencil-mask Decode handling separate from lossless colour images.
    let inverted = (jpeg || mask)
        && s.dict
            .get(b"Decode")
            .map(|d| doc.resolve(d))
            .and_then(|d| d.as_array().map(|a| a.first().and_then(Object::as_f64) > a.get(1).and_then(Object::as_f64)))
            .unwrap_or(false);
    let space = if mask { Ok(Space::Gray) } else { space(doc, s.dict.get(b"ColorSpace")) };
    // A JPEG in a colour space PdfCraft can't convert yet is written as is, if it can be.
    let unchanged = jpeg && space.as_ref().map_or(true, |space| shown_as_drawn(space, inverted, &s.raw));
    if unchanged && alpha.is_none() && key.is_none() {
        return as_is();
    }
    let space = space?;
    let (samples, bpc) = if jpeg {
        // Grey, RGB or CMYK samples (YCCK is converted); a JPEG that doesn't decode, or an
        // indexed one (not valid), is written as is, without its transparency.
        let Some(samples) = jpeg_samples(&s.raw, &space, w, h) else { return as_is() };
        (samples, 8)
    } else {
        let data = s.decoded().map_err(|e| e.to_string())?;
        let bpc = if mask { 1 } else { s.dict.int(b"BitsPerComponent").unwrap_or(8) };
        let bpc = match bpc {
            1 => 1,
            2 => 2,
            4 => 4,
            8 => 8,
            16 => 16,
            other => return Err(format!("{other} bits per component isn't supported")),
        };
        (unpack(&data, w, h, space.components(), bpc).ok_or("the image data is shorter than its size says")?, bpc)
    };
    let n = space.components();
    let decode = if jpeg || mask { None } else { decode(doc, s, n) };
    if alpha.is_none() {
        alpha = key.and_then(|k| keyed_alpha(&samples, n, bpc, &k));
    }
    if unchanged && alpha.is_none() {
        return as_is();
    }
    let invert = inverted ^ mask; // a stencil mask paints its 0 samples
    // `unpack` keeps only the high byte at 16 bits, so Decode uses an effective maximum of 255 too.
    let max = (1u32 << bpc.min(8)) - 1;
    // 8-bit samples in the image's colour space, converted to sRGB below; an indexed image's
    // palette is in sRGB already.
    let mut values = Vec::with_capacity(w * h * n.max(3));
    for px in samples.chunks_exact(n) {
        let decoded = |i: usize| {
            decode.as_ref().and_then(|pairs| pairs.get(i)).map(|&[lo, hi]| {
                let t = f64::from(px.get(i).copied().unwrap_or(0)) / f64::from(max);
                // Avoid overflowing hi - lo for finite endpoints with opposite signs.
                (1.0 - t) * lo + t * hi
            })
        };
        let v = |i: usize| {
            if let Some(value) = decoded(i) {
                return (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            let x = (px.get(i).copied().unwrap_or(0) as u32 * 255 / max) as u8;
            if invert { 255 - x } else { x }
        };
        match &space {
            Space::Indexed(hival, palette) => {
                // Explicit Decode maps into index units, rounded then clipped (8.6.6.3).
                // Without Decode, retain raw indices rather than normalizing to hival.
                let i = match decoded(0) {
                    Some(value) => value.round().clamp(0.0, f64::from(*hival)) as usize,
                    None => px.first().copied().unwrap_or(0).min(*hival) as usize,
                };
                values.extend_from_slice(palette.get(i * 3..i * 3 + 3).unwrap_or(&[0; 3]));
            }
            _ => values.extend((0..n).map(v)),
        }
    }
    // Freed before converting: images up to `MAX_PIXELS` hold several buffers of this size.
    drop(samples);
    let mut rgb = match space {
        Space::Indexed(..) => values,
        _ => to_srgb(&space, values).ok_or("the image's colours can't be converted")?,
    };
    if let (Some(a), Some(matte)) = (alpha.as_deref(), matte(doc, s, &space)) {
        unpremultiply(&mut rgb, a, matte);
    }
    png(w as u32, h as u32, &rgb, alpha.as_deref()).map(|p| ("png", p))
}

/// The explicit Decode intervals for a lossless colour image (8.9.5.2, Table 90).
///
/// A malformed array (not an array, the wrong length, a non-number or non-finite entry) gives
/// the default mapping, so the image still exports.
fn decode(doc: &Document, s: &Stream, n: usize) -> Option<Vec<[f64; 2]>> {
    let value = doc.resolve(s.dict.get(b"Decode")?);
    let values = value.as_array()?;
    if Some(values.len()) != n.checked_mul(2) {
        return None;
    }
    let endpoint = |value: &Object| doc.resolve(value).as_f64().filter(|v| v.is_finite());
    let mut pairs = Vec::with_capacity(n);
    for [lo, hi] in values.as_chunks::<2>().0 {
        pairs.push([endpoint(lo)?, endpoint(hi)?]);
    }
    Some(pairs)
}

/// Width and height as sizes, when both are positive and the image is at most `MAX_PIXELS`.
fn size(w: i64, h: i64) -> Option<(usize, usize)> {
    let (w, h) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let pixels = (w as u64).checked_mul(h as u64)?;
    (w > 0 && h > 0 && pixels <= MAX_PIXELS).then_some((w, h))
}

/// Whether a JPEG file written as is shows the colours the page draws it in: grey or RGB
/// samples the page shows as they are (written without any ICC profile in the file, which the
/// page ignores), or samples in an ICC profile that the file carries itself (inverted, for
/// CMYK, as JPEG files store CMYK). Image viewers convert CMYK in their own way, never the
/// page's, so a DeviceCMYK JPEG is always decoded.
fn shown_as_drawn(space: &Space, inverted: bool, data: &[u8]) -> bool {
    match space {
        Space::Gray | Space::Rgb => true,
        Space::Icc(device, profile, _) => {
            (!matches!(**device, Space::Cmyk) || inverted) && crate::jpeg("", data).ok().and_then(|j| j.icc).as_deref() == Some(&profile[..])
        }
        Space::Cmyk | Space::Indexed(..) => false,
    }
}

/// A JPEG file without its ICC profile (the APP2 `ICC_PROFILE` segments, however many chunks it
/// is split into), every other byte copied as it is, the scans included. `None` when the
/// segments before the first scan can't be read.
fn without_icc_profile(data: &[u8]) -> Option<Vec<u8>> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(data.get(..2)?);
    let mut i = 2;
    loop {
        // A marker: 0xFF, any number of 0xFF fill bytes, then its code.
        let start = i;
        if *data.get(i)? != 0xFF {
            return None;
        }
        while data.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let marker = *data.get(i + 1)?;
        i += 2;
        match marker {
            // The first scan (or the end): the rest is entropy-coded data and later segments.
            0xDA | 0xD9 => {
                out.extend_from_slice(data.get(start..)?);
                return Some(out);
            }
            // Markers without a length.
            0x01 | 0xD0..=0xD8 => out.extend_from_slice(data.get(start..i)?),
            _ => {
                let len = usize::from(u16::from_be_bytes([*data.get(i)?, *data.get(i + 1)?]));
                let end = i.checked_add(len)?;
                let segment = data.get(i.checked_add(2)?..end)?;
                let icc = marker == 0xE2 && segment.starts_with(b"ICC_PROFILE\0");
                if !icc {
                    out.extend_from_slice(data.get(start..end)?);
                }
                i = end;
            }
        }
    }
}

/// A JPEG's samples (8 bits) in the image's colour space: grey, RGB, or CMYK as stored.
fn jpeg_samples(data: &[u8], space: &Space, w: usize, h: usize) -> Option<Vec<u8>> {
    let space = match space {
        Space::Icc(device, ..) => device,
        other => other,
    };
    match space {
        Space::Indexed(..) => return None,
        Space::Cmyk => return cmyk_jpeg_samples(data, w, h),
        _ => {}
    }
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).ok()?;
    if img.width() as usize != w || img.height() as usize != h {
        return None;
    }
    Some(match space {
        Space::Gray => img.into_luma8().into_raw(),
        _ => img.into_rgb8().into_raw(),
    })
}

/// A four-component JPEG's CMYK samples as the page reads them: as stored, not inverted the way
/// a decoder converting to RGB inverts them, with YCCK converted to CMYK.
fn cmyk_jpeg_samples(data: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().set_strict_mode(false).set_max_width(usize::from(u16::MAX)).set_max_height(usize::from(u16::MAX));
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    dec.decode_headers().ok()?;
    let input = dec.input_colorspace()?;
    if !matches!(input, ColorSpace::CMYK | ColorSpace::YCCK) || dec.dimensions()? != (w, h) {
        return None;
    }
    dec.set_options(options.jpeg_set_out_colorspace(input));
    let mut px = dec.decode().ok()?;
    if px.len() != w.checked_mul(h)?.checked_mul(4)? {
        return None;
    }
    if input == ColorSpace::YCCK {
        for p in px.as_chunks_mut::<4>().0 {
            let (y, cb, cr) = (f32::from(p[0]), f32::from(p[1]) - 128.0, f32::from(p[2]) - 128.0);
            let ink = |rgb: f32| (255.0 - rgb).round().clamp(0.0, 255.0) as u8;
            p[0] = ink(y + 1.402 * cr);
            p[1] = ink(y - 0.344_136 * cb - 0.714_136 * cr);
            p[2] = ink(y + 1.772 * cb);
        }
    }
    Some(px)
}

/// The colour-key `/Mask` (8.9.6.4): a [min max] pair per colour component, in sample values.
fn colour_key(doc: &Document, s: &Stream) -> Option<Vec<(u32, u32)>> {
    let m = doc.resolve(s.dict.get(b"Mask")?);
    let a = m.as_array()?;
    let v: Vec<u32> = a.iter().map(|o| doc.resolve(o).as_f64().map(|f| f.clamp(0.0, 65_535.0) as u32)).collect::<Option<_>>()?;
    (!v.is_empty() && v.len().is_multiple_of(2)).then(|| v.as_chunks::<2>().0.iter().map(|&[lo, hi]| (lo, hi)).collect())
}

/// Alpha from a colour key: pixels whose every component falls in its range are transparent.
fn keyed_alpha(samples: &[u8], n: usize, bpc: usize, key: &[(u32, u32)]) -> Option<Vec<u8>> {
    if key.len() != n {
        return None;
    }
    // 16-bit samples keep their high byte (see `unpack`), so compare the ranges' high bytes.
    let shift = if bpc == 16 { 8 } else { 0 };
    Some(
        samples
            .chunks_exact(n)
            .map(|px| {
                let hidden = px.iter().zip(key).all(|(&c, &(lo, hi))| (lo >> shift..=hi >> shift).contains(&(c as u32)));
                if hidden { 0 } else { 255 }
            })
            .collect(),
    )
}

/// The `/Matte` colour of the image's soft mask (11.6.5.3), in sRGB: the colour the image
/// samples were pre-blended with. Like the page renderer, the blend is undone in sRGB.
fn matte(doc: &Document, s: &Stream, space: &Space) -> Option<Vec<u8>> {
    let r = s.dict.get(b"SMask")?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(m) = &*obj else { return None };
    let a = doc.resolve(m.dict.get(b"Matte")?);
    let v: Vec<u8> =
        a.as_array()?.iter().map(|o| doc.resolve(o).as_f64().map(|f| (f.clamp(0.0, 1.0) * 255.0).round() as u8)).collect::<Option<_>>()?;
    (v.len() == space.components() && !matches!(space, Space::Indexed(..))).then(|| to_srgb(space, v))?
}

/// Undo a pre-blend with `matte`: c = m + (c′ − m) / α.
fn unpremultiply(rgb: &mut [u8], alpha: &[u8], matte: Vec<u8>) {
    for (px, &a) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(alpha) {
        if a == 0 {
            continue;
        }
        for (c, &m) in px.iter_mut().zip(&matte) {
            let v = m as i32 + (*c as i32 - m as i32) * 255 / a as i32;
            *c = v.clamp(0, 255) as u8;
        }
    }
}

#[derive(Clone, Debug)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// An ICC profile the page converts samples with: the device space of as many components,
    /// the profile's bytes, and its conversion to sRGB.
    Icc(Box<Space>, Vec<u8>, IccProfile),
    /// Highest valid palette index, and the palette in sRGB (three bytes an entry, all of them).
    Indexed(u8, Vec<u8>),
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Space::Gray | Space::Indexed(..) => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
            Space::Icc(device, ..) => device.components(),
        }
    }
}

/// 8-bit `samples` in `space` (its components a pixel) as sRGB, converted the way the page
/// renderer draws them. `None` for an indexed space (its palette is sRGB already) and when
/// `samples` aren't whole pixels.
fn to_srgb(space: &Space, samples: Vec<u8>) -> Option<Vec<u8>> {
    match space {
        Space::Gray => Some(samples.iter().flat_map(|&g| [g; 3]).collect()),
        Space::Rgb => samples.len().is_multiple_of(3).then_some(samples),
        Space::Cmyk => pdfcraft_render::cmyk_to_srgb(&samples),
        Space::Icc(_, _, profile) => profile.to_srgb(&samples),
        Space::Indexed(..) => None,
    }
}

/// A lookup table holds at most 256 entries of 4 bytes; decoding stops well past that.
const MAX_LOOKUP: usize = 64 << 10;

fn space(doc: &Document, cs: Option<&Object>) -> Result<Space, String> {
    let Some(cs) = cs else { return Err("the image has no colour space".into()) };
    let cs = doc.resolve(cs);
    let unsupported = |n: &[u8]| Err(format!("{} images can't be exported yet", String::from_utf8_lossy(n)));
    match &*cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Ok(Space::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Ok(Space::Rgb),
            b"DeviceCMYK" | b"CMYK" => Ok(Space::Cmyk),
            other => unsupported(other),
        },
        Object::Array(a) => match a.first().and_then(Object::as_name) {
            Some(b"ICCBased") => match a.get(1).map(|s| doc.resolve(s)).as_deref() {
                Some(Object::Stream(icc)) => {
                    let (device, n) = match icc.dict.int(b"N") {
                        Some(1) => (Space::Gray, 1),
                        Some(3) => (Space::Rgb, 3),
                        Some(4) => (Space::Cmyk, 4),
                        _ => return Err("an ICC colour space with an unusual number of components".into()),
                    };
                    // A profile the renderer can't read leaves the device space; an sRGB one draws
                    // the samples as they are.
                    let profile = icc.decoded().ok();
                    let conversion = profile.as_deref().and_then(|p| IccProfile::new(p, n)).filter(|c| !c.is_srgb());
                    Ok(match (profile, conversion) {
                        (Some(profile), Some(conversion)) => Space::Icc(Box::new(device), profile, conversion),
                        _ => device,
                    })
                }
                _ => Err("a damaged ICC colour space".into()),
            },
            Some(b"CalGray") => Ok(Space::Gray),
            Some(b"CalRGB") => Ok(Space::Rgb),
            Some(b"Indexed" | b"I") => {
                // Checked before recursing: Indexed is the only space that recurses, so refusing
                // an Indexed base here also ends a cyclic base reference.
                let nested = match a.get(1).map(|b| doc.resolve(b)).as_deref() {
                    Some(Object::Array(b)) => matches!(b.first().and_then(Object::as_name), Some(b"Indexed" | b"I")),
                    Some(Object::Name(n)) => matches!(n.as_slice(), b"Indexed" | b"I"),
                    _ => false,
                };
                if nested {
                    return Err("nested indexed colour spaces aren't valid".into());
                }
                let base = space(doc, a.get(1))?;
                let hival = match a.get(2).map(|value| doc.resolve(value)).as_deref() {
                    Some(Object::Int(value)) if (0..=255).contains(value) => *value as u8,
                    _ => return Err("an indexed colour space with an invalid hival".into()),
                };
                let table = match a.get(3).map(|t| doc.resolve(t)).as_deref() {
                    Some(Object::String(s)) => s.bytes.clone(),
                    Some(Object::Stream(t)) => {
                        t.decoded_within(MAX_LOOKUP).map_err(|_| "an indexed colour space with an oversized or damaged lookup table".to_string())?
                    }
                    _ => return Err("an indexed colour space without a lookup table".into()),
                };
                // Every entry up to hival, converted once; one the table is too short for is zero
                // in the base space.
                let k = base.components();
                let entries = (0..=usize::from(hival)).flat_map(|i| table.get(i * k..i * k + k).unwrap_or(&[0; 4][..k]).iter().copied()).collect();
                let palette = to_srgb(&base, entries).ok_or("the indexed colour space's palette can't be converted")?;
                Ok(Space::Indexed(hival, palette))
            }
            Some(other) => unsupported(other),
            None => Err("a damaged colour space".into()),
        },
        _ => Err("a damaged colour space".into()),
    }
}

/// Samples as one byte each (16-bit samples keep their high byte).
fn unpack(data: &[u8], w: usize, h: usize, n: usize, bpc: usize) -> Option<Vec<u8>> {
    let row_bits = w * n * bpc;
    let row = row_bits.div_ceil(8);
    if data.len() < row * h {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * n);
    for y in 0..h {
        let r = &data[y * row..(y + 1) * row];
        match bpc {
            8 => out.extend_from_slice(&r[..w * n]),
            16 => out.extend(r.as_chunks::<2>().0.iter().take(w * n).map(|c| c[0])),
            _ => {
                let per = 8 / bpc;
                let m = (1u8 << bpc) - 1;
                for i in 0..w * n {
                    let shift = 8 - bpc * (i % per + 1);
                    out.push((r[i / per] >> shift) & m);
                }
            }
        }
    }
    Some(out)
}

/// The soft mask (`/SMask`) as 8-bit alpha, scaled to the image's size when it has another.
fn soft_mask(doc: &Document, s: &Stream, w: usize, h: usize) -> Option<Vec<u8>> {
    let r = s.dict.get(b"SMask")?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(m) = &*obj else { return None };
    gray_plane(doc, m, w, h)
}

/// A stencil `/Mask` (an image mask stream, 8.9.6.3) as 8-bit alpha: its 1 samples (0 with
/// `/Decode [1 0]`) hide the image.
fn stencil_mask(doc: &Document, s: &Stream, w: usize, h: usize) -> Option<Vec<u8>> {
    let r = s.dict.get(b"Mask")?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(m) = &*obj else { return None };
    Some(gray_plane(doc, m, w, h)?.into_iter().map(|v| 255 - v).collect())
}

/// A one-component mask image as 8-bit values (its `/Decode` inversion applied), scaled to
/// `w` × `h`. `None` when it can't be decoded (JPEG 2000, JBIG2 and CCITT masks).
fn gray_plane(doc: &Document, m: &Stream, w: usize, h: usize) -> Option<Vec<u8>> {
    let (mw, mh) = size(m.dict.int(b"Width")?, m.dict.int(b"Height")?)?;
    let stencil = m.dict.get(b"ImageMask").is_some_and(|v| matches!(&*doc.resolve(v), Object::Bool(true)));
    let f = filters(m);
    let mut px = match f.last().map(Vec::as_slice) {
        Some(b"DCTDecode" | b"DCT") if f.len() == 1 => {
            let img = image::load_from_memory_with_format(&m.raw, image::ImageFormat::Jpeg).ok()?.into_luma8();
            (img.width() as usize == mw && img.height() as usize == mh).then(|| img.into_raw())?
        }
        Some(b"DCTDecode" | b"DCT" | b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode" | b"CCF") => return None,
        _ => {
            let bpc = if stencil { 1 } else { m.dict.int(b"BitsPerComponent").unwrap_or(8) };
            let bpc = match bpc {
                1 => 1,
                2 => 2,
                4 => 4,
                8 => 8,
                16 => 16,
                _ => return None,
            };
            let max = (1u32 << bpc.min(8)) - 1;
            unpack(&m.decoded().ok()?, mw, mh, 1, bpc)?.into_iter().map(|v| (v as u32 * 255 / max) as u8).collect()
        }
    };
    let invert = m
        .dict
        .get(b"Decode")
        .map(|d| doc.resolve(d))
        .and_then(|d| d.as_array().map(|a| a.first().and_then(Object::as_f64) > a.get(1).and_then(Object::as_f64)))
        .unwrap_or(false);
    if invert {
        px.iter_mut().for_each(|v| *v = 255 - *v);
    }
    if (mw, mh) == (w, h) {
        return Some(px);
    }
    let img = image::GrayImage::from_raw(mw as u32, mh as u32, px)?;
    Some(image::imageops::resize(&img, w as u32, h as u32, image::imageops::FilterType::Triangle).into_raw())
}

fn png(w: u32, h: u32, rgb: &[u8], alpha: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let data: Vec<u8> = match alpha {
            Some(a) => {
                enc.set_color(png::ColorType::Rgba);
                rgb.as_chunks::<3>().0.iter().zip(a).flat_map(|(c, a)| [c[0], c[1], c[2], *a]).collect()
            }
            None => {
                enc.set_color(png::ColorType::Rgb);
                rgb.to_vec()
            }
        };
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
