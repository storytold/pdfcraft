//! Export a PDF ▸ Image ▸ Export all images, and Edit ▸ Save image as: the images that pages
//! use, as files. JPEG images are written unchanged; other images are decoded and written as
//! PNG (with their soft mask as alpha). Images `PrintCraft` can't decode yet (JPEG 2000, JBIG2,
//! CCITT, separations) are reported, never silently left out.

use std::collections::HashSet;

use printcraft_cos::{Dict, Document, ObjRef, Object, Stream};

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
#[must_use]
pub fn extract_images(doc: &Document, pages: &[usize], min_side: u32) -> ImageExport {
    let all = printcraft_model::pages(doc);
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
            if w <= 0 || h <= 0 || (w.min(h) as u64) < u64::from(min_side) {
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

/// One image `XObject` as a file: JPEG as is, else PNG ("Save image as").
///
/// # Errors
///
/// When `image` is not an image `XObject`, or when the image cannot be exported: an unsupported
/// filter, colour space or bit depth, an image over 100 000 000 pixels, undecodable data, or a
/// failed PNG encode. The message says which.
pub fn image_file(doc: &Document, image: ObjRef) -> Result<(&'static str, Vec<u8>), String> {
    match &*doc.get(image) {
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => self::image(doc, s),
        _ => Err("not an image".into()),
    }
}

/// The image `XObjects` reachable from `res`, through form `XObjects`, in resource order.
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
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(|f| f.as_name().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

/// The file for one image: its JPEG data as is, or a PNG.
fn image(doc: &Document, stream: &Stream) -> Result<(&'static str, Vec<u8>), String> {
    let filter_list = filters(stream);
    match filter_list.last().map(Vec::as_slice) {
        Some(b"DCTDecode" | b"DCT") if filter_list.len() == 1 => return Ok(("jpg", stream.raw.to_vec())),
        Some(b"JPXDecode") => return Err("JPEG 2000 images can't be exported yet".into()),
        Some(b"JBIG2Decode") => return Err("JBIG2 images can't be exported yet".into()),
        Some(b"CCITTFaxDecode" | b"CCF") => return Err("CCITT fax images can't be exported yet".into()),
        Some(b"DCTDecode" | b"DCT") => return Err("JPEG images inside other filters can't be exported yet".into()),
        _ => {}
    }
    let (width, height) = (stream.dict.int(b"Width").unwrap_or(0) as usize, stream.dict.int(b"Height").unwrap_or(0) as usize);
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(format!("{width} × {height} is too large to export"));
    }
    let data = stream.decoded().map_err(|e| e.to_string())?;
    let mask = stream.dict.get(b"ImageMask").is_some_and(|m| matches!(&*doc.resolve(m), Object::Bool(true)));
    let bpc = if mask { 1 } else { stream.dict.int(b"BitsPerComponent").unwrap_or(8) as usize };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return Err(format!("{bpc} bits per component isn't supported"));
    }
    let space = if mask { Space::Gray } else { space(doc, stream.dict.get(b"ColorSpace"))? };
    let comps = space.components();
    let samples = unpack(&data, width, height, comps, bpc).ok_or("the image data is shorter than its size says")?;
    // Decode arrays: only inversion ([1 0] per component) is honoured, the common case.
    let invert = stream
        .dict
        .get(b"Decode")
        .map(|d| doc.resolve(d))
        .and_then(|d| d.as_array().map(|a| a.first().and_then(Object::as_f64) > a.get(1).and_then(Object::as_f64)));
    let invert = invert.unwrap_or(false) ^ mask; // a stencil mask paints its 0 samples
    let max = (1u32 << bpc.min(8)) - 1;
    let mut rgb = Vec::with_capacity(width * height * 3);
    for px in samples.chunks_exact(comps) {
        let sample = |i: usize| {
            let x = (u32::from(px[i]) * 255 / max) as u8;
            if invert { 255 - x } else { x }
        };
        match &space {
            Space::Gray => rgb.extend_from_slice(&[sample(0); 3]),
            Space::Rgb => rgb.extend_from_slice(&[sample(0), sample(1), sample(2)]),
            Space::Cmyk => rgb.extend_from_slice(&cmyk(sample(0), sample(1), sample(2), sample(3))),
            Space::Indexed(base, table) => {
                let i = px[0] as usize;
                let k = base.components();
                let e = table.get(i * k..i * k + k).unwrap_or(&[0, 0, 0, 0][..k]);
                rgb.extend_from_slice(&match **base {
                    Space::Gray => [e[0]; 3],
                    Space::Cmyk => cmyk(e[0], e[1], e[2], e[3]),
                    _ => [e[0], e[1], e[2]],
                });
            }
        }
    }
    let alpha = soft_mask(doc, stream, width, height);
    png(width as u32, height as u32, &rgb, alpha.as_deref()).map(|p| ("png", p))
}

#[derive(Clone, Debug)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Base space and lookup table (one byte per base component).
    Indexed(Box<Space>, Vec<u8>),
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Space::Gray | Space::Indexed(..) => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }
}

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
                Some(Object::Stream(icc)) => match icc.dict.int(b"N") {
                    Some(1) => Ok(Space::Gray),
                    Some(3) => Ok(Space::Rgb),
                    Some(4) => Ok(Space::Cmyk),
                    _ => Err("an ICC colour space with an unusual number of components".into()),
                },
                _ => Err("a damaged ICC colour space".into()),
            },
            Some(b"CalGray") => Ok(Space::Gray),
            Some(b"CalRGB") => Ok(Space::Rgb),
            Some(b"Indexed" | b"I") => {
                let base = space(doc, a.get(1))?;
                if matches!(base, Space::Indexed(..)) {
                    return Err("nested indexed colour spaces aren't valid".into());
                }
                let table = match a.get(3).map(|t| doc.resolve(t)).as_deref() {
                    Some(Object::String(s)) => s.bytes.clone(),
                    Some(Object::Stream(t)) => t.decoded().map_err(|e| e.to_string())?,
                    _ => return Err("an indexed colour space without a lookup table".into()),
                };
                Ok(Space::Indexed(Box::new(base), table))
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
        let line = &data[y * row..(y + 1) * row];
        match bpc {
            8 => out.extend_from_slice(&line[..w * n]),
            16 => out.extend(line.as_chunks::<2>().0.iter().take(w * n).map(|c| c[0])),
            _ => {
                let per = 8 / bpc;
                let mask = (1u8 << bpc) - 1;
                for i in 0..w * n {
                    let shift = 8 - bpc * (i % per + 1);
                    out.push((line[i / per] >> shift) & mask);
                }
            }
        }
    }
    Some(out)
}

fn cmyk(c: u8, m: u8, y: u8, key: u8) -> [u8; 3] {
    let blend = |x: u8| ((255 - u32::from(x)) * (255 - u32::from(key)) / 255) as u8;
    [blend(c), blend(m), blend(y)]
}

/// The soft mask as 8-bit alpha, when it is an 8-bit gray image of the same size.
fn soft_mask(doc: &Document, stream: &Stream, w: usize, h: usize) -> Option<Vec<u8>> {
    let smask = stream.dict.get(b"SMask")?.as_ref()?;
    let obj = doc.get(smask);
    let Object::Stream(mask) = &*obj else { return None };
    if mask.dict.int(b"Width")? as usize != w || mask.dict.int(b"Height")? as usize != h {
        return None;
    }
    let bpc = mask.dict.int(b"BitsPerComponent").unwrap_or(8) as usize;
    if !filters(mask).iter().all(|f| !matches!(f.as_slice(), b"DCTDecode" | b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode")) {
        return None;
    }
    let px = unpack(&mask.decoded().ok()?, w, h, 1, bpc)?;
    let max = (1u32 << bpc.min(8)) - 1;
    Some(px.into_iter().map(|v| (u32::from(v) * 255 / max) as u8).collect())
}

fn png(w: u32, h: u32, rgb: &[u8], alpha: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let data: Vec<u8> = if let Some(a) = alpha {
            enc.set_color(png::ColorType::Rgba);
            rgb.as_chunks::<3>().0.iter().zip(a).flat_map(|(c, a)| [c[0], c[1], c[2], *a]).collect()
        } else {
            enc.set_color(png::ColorType::Rgb);
            rgb.to_vec()
        };
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
