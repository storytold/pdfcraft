//! pdfcraft-create — create PDFs from nothing, images or text (L4). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

mod density;
mod extract;
mod upright_image;
pub use extract::{ExtractedImage, ImageExport, extract_images, image_file};
use pdfcraft_fonts::{literal, win_ansi, wrap};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CreateError {
    #[error("{0}: {1}")]
    Image(String, String),
    #[error("{0}")]
    Invalid(String),
}

/// US Letter in points.
pub const LETTER: (f64, f64) = (612.0, 792.0);

/// The largest page side PDF allows (ISO 32000-2 Annex C: 14 400 units).
const MAX_SIDE: f64 = 14_400.0;

fn add_page(doc: &mut Document, w: f64, h: f64, resources: Dict, content: Option<Vec<u8>>) -> Result<ObjRef, CreateError> {
    // `Document::new_empty` always has a page tree.
    let pages = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages")))
        .ok_or_else(|| CreateError::Invalid("the new document has no page tree".into()))?;
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)]));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    if let Some(c) = content {
        let s = doc.add(Object::Stream(Stream::flate(Dict::new(), &c)));
        page.set(b"Contents".to_vec(), Object::Ref(s));
    }
    let r = doc.add(page);
    let _ = doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    });
    Ok(r)
}

fn set_title(doc: &mut Document, title: &str) {
    let mut info = Dict::new();
    info.set(b"Title".to_vec(), PdfString::text(title));
    info.set(b"Producer".to_vec(), PdfString::text("PdfCraft"));
    let r = doc.add(info);
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
}

/// A document of `pages` empty pages of `width × height` points.
pub fn blank(width: f64, height: f64, pages: usize) -> Result<Document, CreateError> {
    if !(width.is_finite() && height.is_finite() && (3.0..=MAX_SIDE).contains(&width) && (3.0..=MAX_SIDE).contains(&height))
        || pages == 0
        || pages > 10_000
    {
        return Err(CreateError::Invalid("invalid page size or count".into()));
    }
    let mut doc = Document::new_empty();
    for _ in 0..pages {
        add_page(&mut doc, width, height, Dict::new(), None)?;
    }
    Ok(doc)
}

// ── images ──────────────────────────────────────────────────────────────────────────────────

/// An image ready to embed: its XObject dictionary, encoded data, optional soft mask, the ICC
/// profile the file carried (checked against the colour space by `icc_for`), and size in pixels
/// and dots per inch.
struct Embedded {
    dict: Dict,
    data: Vec<u8>,
    filtered: bool,
    smask: Option<(Dict, Vec<u8>)>,
    icc: Option<Vec<u8>>,
    px: (u32, u32),
    dpi: (f64, f64),
}

/// `profile` when it can tag an image of `n` colour components: an ICC header (`acsp`) whose
/// colour space is grey, RGB or CMYK to match. Anything else is left out, and the image keeps
/// its device colour space.
fn icc_for(profile: Vec<u8>, n: usize) -> Option<Vec<u8>> {
    let space: &[u8] = match n {
        1 => b"GRAY",
        3 => b"RGB ",
        4 => b"CMYK",
        _ => return None,
    };
    (profile.len() >= 128 && profile.get(36..40) == Some(b"acsp") && profile.get(16..20) == Some(space)).then_some(profile)
}

/// A JPEG's ICC profile from its APP2 `ICC_PROFILE` chunks: (sequence number, chunk count,
/// data). `None` unless every chunk from 1 to the count is there exactly once.
fn jpeg_icc(mut chunks: Vec<(u8, u8, &[u8])>) -> Option<Vec<u8>> {
    let count = chunks.first()?.1;
    chunks.sort_by_key(|c| c.0);
    let complete = usize::from(count) == chunks.len() && chunks.iter().zip(1..=count).all(|(c, seq)| c.0 == seq && c.1 == count);
    complete.then(|| chunks.iter().flat_map(|c| c.2.iter().copied()).collect())
}

/// Add `img` to `doc` as an image XObject. An ICC profile becomes an `/ICCBased` colour space
/// (with the device space as `/Alternate`); `profiles` shares one object between images that
/// carry the same profile.
fn add_image(doc: &mut Document, img: Embedded, profiles: &mut Vec<(Vec<u8>, ObjRef)>) -> ObjRef {
    let mut d = img.dict;
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(img.px.0 as i64));
    d.set(b"Height".to_vec(), Object::Int(img.px.1 as i64));
    let device = d.name(b"ColorSpace").map(<[u8]>::to_vec);
    let n = match device.as_deref() {
        Some(b"DeviceGray") => 1,
        Some(b"DeviceRGB") => 3,
        Some(b"DeviceCMYK") => 4,
        _ => 0,
    };
    if let (Some(icc), Some(device)) = (img.icc.and_then(|p| icc_for(p, n)), device) {
        let r = match profiles.iter().find(|(p, _)| *p == icc) {
            Some((_, r)) => *r,
            None => {
                let mut pd = Dict::new();
                pd.set(b"N".to_vec(), Object::Int(n as i64));
                pd.set(b"Alternate".to_vec(), Object::Name(device));
                let r = doc.add(Object::Stream(Stream::flate(pd, &icc)));
                profiles.push((icc, r));
                r
            }
        };
        d.set(b"ColorSpace".to_vec(), Object::Array(vec![Object::name("ICCBased"), Object::Ref(r)]));
    }
    if let Some((m, alpha)) = img.smask {
        let mr = doc.add(Object::Stream(Stream::flate(m, &alpha)));
        d.set(b"SMask".to_vec(), Object::Ref(mr));
    }
    let stream = if img.filtered { Stream::from_raw(d, img.data) } else { Stream::flate(d, &img.data) };
    doc.add(Object::Stream(stream))
}

const JP2_SIGNATURE: &[u8] = &[0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_be_bytes([x[0], x[1]]))
}

/// The boxes of a JP2 box sequence: (type, payload).
fn jp2_boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= b.len() {
        let len = be32(b, i).unwrap_or(0) as u64;
        let ty = [b[i + 4], b[i + 5], b[i + 6], b[i + 7]];
        let (head, len) = match len {
            0 => (8, (b.len() - i) as u64),
            1 => match b.get(i + 8..i + 16) {
                Some(x) => (16, u64::from_be_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]])),
                None => break,
            },
            n => (8, n),
        };
        let end = i.saturating_add(len as usize).min(b.len());
        if len < head as u64 || end <= i {
            break;
        }
        out.push((ty, &b[i + head..end]));
        i = end;
    }
    out
}

/// A JPEG 2000 image (JP2 file or raw codestream), embedded as is (`/JPXDecode`; the colour
/// space comes from the file).
fn jpx(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let bad = |m: &str| CreateError::Image(name.into(), m.into());
    let mut dpi = (72.0, 72.0);
    let (w, h) = if bytes.starts_with(JP2_SIGNATURE) {
        let header =
            jp2_boxes(bytes).into_iter().find(|(t, _)| t == b"jp2h").map(|(_, p)| p).ok_or_else(|| bad("a JPEG 2000 file without a header"))?;
        let inner = jp2_boxes(header);
        let ihdr = inner.iter().find(|(t, _)| t == b"ihdr").map(|(_, p)| *p).ok_or_else(|| bad("a JPEG 2000 file without image size"))?;
        // Capture resolution (pixels per metre), when given.
        if let Some((_, res)) = inner.iter().find(|(t, _)| t == b"res ")
            && let Some((_, r)) = jp2_boxes(res).into_iter().find(|(t, _)| t == b"resc" || t == b"resd")
            && r.len() >= 10
        {
            let part = |n: u16, d: u16, e: i8| if d == 0 { 0.0 } else { n as f64 / d as f64 * 10f64.powi(e as i32) * 0.0254 };
            let (v, hz) = (
                part(be16(r, 0).unwrap_or(0), be16(r, 2).unwrap_or(0), r[8] as i8),
                part(be16(r, 4).unwrap_or(0), be16(r, 6).unwrap_or(0), r[9] as i8),
            );
            if v > 1.0 && hz > 1.0 {
                dpi = (hz, v);
            }
        }
        (be32(ihdr, 4).ok_or_else(|| bad("bad image header"))?, be32(ihdr, 0).ok_or_else(|| bad("bad image header"))?)
    } else {
        // SIZ: Lsiz Rsiz Xsiz Ysiz XOsiz YOsiz …
        let (x, y, xo, yo) = (be32(bytes, 8), be32(bytes, 12), be32(bytes, 16), be32(bytes, 20));
        match (x, y, xo, yo) {
            (Some(x), Some(y), Some(xo), Some(yo)) if x > xo && y > yo => (x - xo, y - yo),
            _ => return Err(bad("a damaged JPEG 2000 codestream")),
        }
    };
    if w == 0 || h == 0 || w > 100_000 || h > 100_000 {
        return Err(bad("unusual JPEG 2000 image size"));
    }
    let mut d = Dict::new();
    d.set(b"Filter".to_vec(), Object::name("JPXDecode"));
    Ok(Embedded { dict: d, data: bytes.to_vec(), filtered: true, smask: None, icc: None, px: (w, h), dpi })
}

/// A JPEG as a pass-through image: size, colour space, Adobe inversion and ICC profile from its segments. The resolution is
/// left at 72 dpi; `with_density` is the one place that reads what the file carries.
fn jpeg(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let bad = |m: &str| CreateError::Image(name.into(), m.into());
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(bad("not a JPEG file"));
    }
    let (mut i, mut size, mut comps, mut adobe) = (2usize, None, 0u8, false);
    let mut icc_chunks = Vec::new();
    while let Some(&byte) = bytes.get(i) {
        if byte != 0xFF {
            i += 1;
            continue;
        }
        i += 1;
        // Markers may be preceded by any number of 0xFF fill bytes.
        while bytes.get(i) == Some(&0xFF) {
            i += 1;
        }
        let marker = *bytes.get(i).ok_or_else(|| bad("truncated JPEG marker"))?;
        i += 1;
        // Metadata ends at the first scan; entropy-coded bytes are not segments.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let len = usize::from(be16(bytes, i).ok_or_else(|| bad("truncated JPEG segment length"))?);
        if len < 2 {
            return Err(bad("invalid JPEG segment length"));
        }
        let end = i.checked_add(len).ok_or_else(|| bad("JPEG segment length overflow"))?;
        let seg = bytes.get(i + 2..end).ok_or_else(|| bad("truncated JPEG segment"))?;
        match marker {
            0xEE if seg.starts_with(b"Adobe") => adobe = true,
            // APP2 ICC profile, possibly split over several chunks.
            0xE2 => {
                if let Some(([seq, count], data)) = seg.strip_prefix(b"ICC_PROFILE\0").and_then(<[u8]>::split_first_chunk::<2>) {
                    icc_chunks.push((*seq, *count, data));
                }
            }
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC && size.is_none() => {
                if seg.len() < 6 {
                    return Err(bad("bad frame header"));
                }
                size = Some((u16::from_be_bytes([seg[3], seg[4]]) as u32, u16::from_be_bytes([seg[1], seg[2]]) as u32));
                comps = seg[5];
            }
            _ => {}
        }
        i = end;
    }
    let (w, h) = size.filter(|(w, h)| *w > 0 && *h > 0).ok_or_else(|| bad("no image size"))?;
    let mut d = Dict::new();
    let cs = match comps {
        1 => "DeviceGray",
        3 => "DeviceRGB",
        4 => {
            if adobe {
                // Adobe writes CMYK JPEGs inverted.
                d.set(b"Decode".to_vec(), Object::Array([1, 0, 1, 0, 1, 0, 1, 0].iter().map(|v| Object::Int(*v)).collect()));
            }
            "DeviceCMYK"
        }
        n => return Err(bad(&format!("{n} colour components are not supported"))),
    };
    d.set(b"ColorSpace".to_vec(), Object::name(cs));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    let icc = jpeg_icc(icc_chunks);
    Ok(Embedded { dict: d, data: bytes.to_vec(), filtered: true, smask: None, icc, px: (w, h), dpi: (72.0, 72.0) })
}

/// A PNG decoded to an image XObject (alpha as a soft mask, 16-bit reduced to 8). The resolution is left at 72 dpi;
/// `with_density` is the one place that reads what the file carries.
fn png_image(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let bad = |m: String| CreateError::Image(name.into(), m);
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| bad(e.to_string()))?;
    let icc = reader.info().icc_profile.as_ref().map(|p| p.to_vec());
    // The decoded size comes from the header alone, so a file of a few bytes can claim any
    // size: keep the ceiling, and ask for the memory fallibly (a failed allocation would abort
    // the whole app, and no `catch_unwind` stops that).
    let size = reader.output_buffer_size().filter(|n| *n <= MAX_DECODED_BYTES).ok_or_else(|| bad("image too large".into()))?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(size).map_err(|_| bad("not enough memory for this image".into()))?;
    buf.resize(size, 0);
    let frame = reader.next_frame(&mut buf).map_err(|e| bad(e.to_string()))?;
    let (w, h) = (frame.width, frame.height);
    let data = &buf[..frame.buffer_size()];
    let (channels, cs) = match frame.color_type {
        png::ColorType::Grayscale => (1, "DeviceGray"),
        png::ColorType::GrayscaleAlpha => (2, "DeviceGray"),
        png::ColorType::Rgb => (3, "DeviceRGB"),
        png::ColorType::Rgba => (4, "DeviceRGB"),
        png::ColorType::Indexed => return Err(bad("unexpected palette output".into())),
    };
    let has_alpha = channels == 2 || channels == 4;
    let colour_n = if has_alpha { channels - 1 } else { channels };
    let mut colour = Vec::new();
    colour
        .try_reserve_exact((w as usize).saturating_mul(h as usize).saturating_mul(colour_n))
        .map_err(|_| bad("not enough memory for this image".into()))?;
    let mut alpha = Vec::new();
    if has_alpha {
        alpha.try_reserve_exact((w as usize).saturating_mul(h as usize)).map_err(|_| bad("not enough memory for this image".into()))?;
    }
    for px in data.chunks_exact(channels) {
        colour.extend_from_slice(&px[..colour_n]);
        if has_alpha {
            alpha.push(px[colour_n]);
        }
    }
    let mut d = Dict::new();
    d.set(b"ColorSpace".to_vec(), Object::name(cs));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let smask = (has_alpha && alpha.iter().any(|a| *a != 255)).then(|| {
        let mut m = Dict::new();
        m.set(b"Type".to_vec(), Object::name("XObject"));
        m.set(b"Subtype".to_vec(), Object::name("Image"));
        m.set(b"Width".to_vec(), Object::Int(w as i64));
        m.set(b"Height".to_vec(), Object::Int(h as i64));
        m.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        m.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        (m, alpha)
    });
    Ok(Embedded { dict: d, data: colour, filtered: false, smask, icc, px: (w, h), dpi: (72.0, 72.0) })
}

/// 8-bit RGBA pixels → an image (gray when every pixel is, with a soft mask when any pixel is
/// not opaque).
fn rgba_image(rgba: &[u8], (w, h): (u32, u32), dpi: (f64, f64)) -> Embedded {
    let gray = rgba.as_chunks::<4>().0.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
    let opaque = rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    let colour: Vec<u8> = if gray {
        rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
    } else {
        rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect()
    };
    let mut d = Dict::new();
    d.set(b"ColorSpace".to_vec(), Object::name(if gray { "DeviceGray" } else { "DeviceRGB" }));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let smask = (!opaque).then(|| {
        let mut m = Dict::new();
        m.set(b"Type".to_vec(), Object::name("XObject"));
        m.set(b"Subtype".to_vec(), Object::name("Image"));
        m.set(b"Width".to_vec(), Object::Int(w as i64));
        m.set(b"Height".to_vec(), Object::Int(h as i64));
        m.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        m.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        (m, rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect())
    });
    Embedded { dict: d, data: colour, filtered: false, smask, icc: None, px: (w, h), dpi }
}

/// How far from 0 / 255 a BMP palette channel may be and still count as black / white.
const BMP_GRAY_TOLERANCE: u8 = 16;

/// An uncompressed 1-bit BMP whose two palette entries are black and white (either order, small
/// tolerance) stays a 1-bit DeviceGray image: 0 = black, rows top-down, packed MSB first and
/// byte-aligned, like the bilevel TIFF path. Anything else (other palettes, compression, odd or
/// truncated headers, short pixel arrays) returns `None` and takes the general decoder, which
/// also produces the usual error for a broken file.
fn bmp_one_bit(bytes: &[u8]) -> Option<Embedded> {
    let u16_at = |at: usize| bytes.get(at..at.checked_add(2)?).and_then(|b| Some(u16::from_le_bytes(b.try_into().ok()?)));
    let u32_at = |at: usize| bytes.get(at..at.checked_add(4)?).and_then(|b| Some(u32::from_le_bytes(b.try_into().ok()?)));
    if bytes.get(..2)? != b"BM" {
        return None;
    }
    let pixel_at = usize::try_from(u32_at(10)?).ok()?;
    let header = usize::try_from(u32_at(14)?).ok()?;
    // BITMAPINFOHEADER or one of its larger successors (the OS/2 12-byte header is left alone).
    if !(40..=124).contains(&header) || u16_at(26)? != 1 || u16_at(28)? != 1 || u32_at(30)? != 0 {
        return None;
    }
    let width = usize::try_from(u32_at(18)? as i32).ok().filter(|w| *w > 0)?;
    let raw_height = u32_at(22)? as i32;
    let top_down = raw_height < 0;
    let rows = usize::try_from(raw_height.unsigned_abs()).ok().filter(|h| *h > 0)?;
    let colours = u32_at(46)?;
    if colours != 0 && colours != 2 {
        return None;
    }
    let palette_at = 14usize.checked_add(header)?;
    // `Some(true)` for (near) white, `Some(false)` for (near) black, `None` for any other colour.
    let entry = |i: usize| -> Option<bool> {
        let b = bytes.get(palette_at.checked_add(i * 4)?..)?.get(..3)?;
        if b.iter().all(|c| *c <= BMP_GRAY_TOLERANCE) {
            Some(false)
        } else if b.iter().all(|c| *c >= 255 - BMP_GRAY_TOLERANCE) {
            Some(true)
        } else {
            None
        }
    };
    let (first, second) = (entry(0)?, entry(1)?);
    if first == second || pixel_at < palette_at.checked_add(8)? {
        return None;
    }
    // The pixel array must be there in full, so every buffer below is bounded by the file's size.
    let stride = width.div_ceil(32).checked_mul(4)?;
    let pixels = bytes.get(pixel_at..)?.get(..stride.checked_mul(rows)?)?;
    let packed = width.div_ceil(8);
    let mut data = Vec::new();
    data.try_reserve_exact(packed.checked_mul(rows)?).ok()?;
    // Palette [white, black]: index 1 is black, so flip the bits to get 0 = black.
    let flip = if first { 0xFF } else { 0x00 };
    for r in 0..rows {
        let src = if top_down { r } else { rows - 1 - r };
        let row = pixels.get(src * stride..)?.get(..packed)?;
        data.extend(row.iter().map(|b| b ^ flip));
    }
    let mut d = Dict::new();
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    let px = (u32::try_from(width).ok()?, u32::try_from(rows).ok()?);
    Some(Embedded { dict: d, data, filtered: false, smask: None, icc: None, px, dpi: (72.0, 72.0) })
}

/// BMP and GIF (the first frame), through the `image` decoders. A GIF has no resolution (72 dpi); a BMP's is read by
/// [`embed`] (pels per metre) like every other format's.
fn decoded(name: &str, bytes: &[u8], format: image::ImageFormat) -> Result<Embedded, CreateError> {
    // A GIF's first frame is painted on a canvas the size of its logical screen, however small
    // the frame: a few bytes can ask for hundreds of MB.
    if format == image::ImageFormat::Gif
        && let Some(&[w0, w1, h0, h1]) = bytes.get(6..10)
        && u64::from(u16::from_le_bytes([w0, w1])) * u64::from(u16::from_le_bytes([h0, h1])) > MAX_GIF_PIXELS
    {
        return Err(CreateError::Image(name.into(), "the GIF's screen is larger than 4096×4096 pixels".into()));
    }
    if format == image::ImageFormat::Bmp
        && let Some(one_bit) = bmp_one_bit(bytes)
    {
        return Ok(one_bit);
    }
    let img = image::load_from_memory_with_format(bytes, format).map_err(|e| CreateError::Image(name.into(), e.to_string()))?;
    let rgba = img.into_rgba8();
    Ok(rgba_image(rgba.as_raw(), rgba.dimensions(), (72.0, 72.0)))
}

/// Largest GIF logical screen, in pixels (4096×4096).
const MAX_GIF_PIXELS: u64 = 1 << 24;

/// Most decoded bytes one TIFF page or PNG may take: room for the 768 MiB CMYK poster in #665,
/// twice over.
const MAX_DECODED_BYTES: usize = 2 << 30;

/// Most decoded bytes all the pages of one TIFF may take together: its pages stay in memory until
/// they are embedded, and many small directories can share one compressed strip. Saturates to the
/// whole address space on 32-bit targets (wasm32), where 4 GiB doesn't fit in `usize`.
const MAX_DECODED_TOTAL: usize = MAX_DECODED_BYTES.saturating_mul(2);

/// The first `wanted` pages of a TIFF (multi-page scans become multi-page PDFs).
fn tiff_pages(name: &str, bytes: &[u8], wanted: usize) -> Result<Vec<Embedded>, CreateError> {
    tiff_pages_within(name, bytes, wanted, MAX_DECODED_TOTAL)
}

/// [`tiff_pages`], refused once the pages kept take more than `budget` bytes.
fn tiff_pages_within(name: &str, bytes: &[u8], wanted: usize, budget: usize) -> Result<Vec<Embedded>, CreateError> {
    use tiff::ColorType as C;
    use tiff::decoder::{Decoder, DecodingResult, Limits};
    use tiff::tags::Tag;
    let bad = |m: String| CreateError::Image(name.into(), m);
    // The tiff crate stops at 256 MiB of decoded pixels, below an ordinary poster scan (#665):
    // 12000x16000 CMYK is 768 MiB. Raise it, but keep a ceiling so a tiny file claiming
    // enormous dimensions can't ask for unbounded memory.
    let mut limits = Limits::default();
    limits.decoding_buffer_size = MAX_DECODED_BYTES;
    let mut dec = Decoder::new(std::io::Cursor::new(bytes)).map_err(|e| bad(e.to_string()))?.with_limits(limits);
    let mut out = Vec::new();
    let mut kept = 0usize;
    loop {
        let (w, h) = dec.dimensions().map_err(|e| bad(e.to_string()))?;
        let ct = dec.colortype().map_err(|e| bad(e.to_string()))?;
        let unit = dec.get_tag_u32(Tag::ResolutionUnit).unwrap_or(2);
        let res = |t: Tag, dec: &mut Decoder<std::io::Cursor<&[u8]>>| -> Option<f64> {
            // Resolutions are rationals.
            match dec.get_tag(t) {
                Ok(tiff::decoder::ifd::Value::Rational(n, d)) if d != 0 => Some(f64::from(n) / f64::from(d)),
                Ok(other) => other.into_f64().ok(),
                Err(_) => None,
            }
        };
        let (x, y) = (res(Tag::XResolution, &mut dec), res(Tag::YResolution, &mut dec));
        // No unit ("none"), a missing axis or an absurd value gives nothing: 72 dpi, as for any image without one.
        let dpi = density::tiff_tag_density(unit, x, y).unwrap_or((72.0, 72.0));
        let data = match dec.read_image().map_err(|e| bad(e.to_string()))? {
            DecodingResult::U8(v) => v,
            // 16-bit samples: keep the high byte.
            DecodingResult::U16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
            _ => return Err(bad("this TIFF sample format isn't supported".into())),
        };
        let page = match ct {
            C::Gray(1) => {
                // Bilevel scans stay 1 bit per pixel (0 = black after the decoder's inversion).
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
                Embedded { dict: d, data, filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            C::Gray(8 | 16) => {
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                Embedded { dict: d, data, filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            C::RGB(8 | 16) => {
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                Embedded { dict: d, data, filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            C::CMYK(8 | 16) => {
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceCMYK"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                Embedded { dict: d, data, filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            C::RGBA(8 | 16) => rgba_image(&data, (w, h), dpi),
            C::GrayA(8 | 16) => {
                let rgba: Vec<u8> = data.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect();
                rgba_image(&rgba, (w, h), dpi)
            }
            other => return Err(bad(format!("{other:?} TIFF images aren't supported yet"))),
        };
        kept = kept.saturating_add(page.data.len()).saturating_add(page.smask.as_ref().map_or(0, |(_, alpha)| alpha.len()));
        if kept > budget {
            return Err(bad(format!("the TIFF's pages take more than {} MiB together", budget >> 20)));
        }
        out.push(page);
        if out.len() >= wanted || !dec.more_images() {
            break;
        }
        dec.next_image().map_err(|e| bad(e.to_string()))?;
    }
    Ok(out)
}

/// What a file picked for Create is, by its bytes (and, for text, its name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Pdf,
    Image,
    Text,
}

/// File extensions Create converts to PDF (images and plain text).
pub const CONVERTIBLE: [&str; 12] = ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx", "txt", "text"];

/// A classic TIFF or a BigTIFF (version 43), in either byte order.
fn is_tiff(bytes: &[u8]) -> bool {
    [b"II*\0", b"MM\0*", b"II+\0", b"MM\0+"].iter().any(|m| bytes.starts_with(*m))
}

/// The GIF headers `embed` decodes, so detection and conversion agree.
fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

fn is_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || is_tiff(bytes)
        || is_gif(bytes)
        // JPEG 2000: a JP2 file or a raw codestream.
        || bytes.starts_with(JP2_SIGNATURE)
        || bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51])
        // BMP: "BM" and a known header size (so text starting with "BM" stays text).
        || (bytes.starts_with(b"BM")
            && bytes.get(14..18).and_then(|h| <[u8; 4]>::try_from(h).ok()).is_some_and(|h| matches!(u32::from_le_bytes(h), 12 | 40 | 52 | 56 | 108 | 124)))
}

/// Whether `bytes` named `name` is a PDF, an image Create can embed, or plain text (a `.txt` or
/// `.text` file). `None` for anything else.
pub fn source_kind(name: &str, bytes: &[u8]) -> Option<SourceKind> {
    let head = bytes.get(..bytes.len().min(1024)).unwrap_or_default();
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(SourceKind::Pdf);
    }
    if is_image(bytes) {
        return Some(SourceKind::Image);
    }
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".txt") || lower.ends_with(".text")).then_some(SourceKind::Text)
}

/// Detect the image format from its bytes; a TIFF may hold several pages, of which the first
/// `wanted` are read.
fn embed(name: &str, bytes: &[u8], wanted: usize) -> Result<Vec<Embedded>, CreateError> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let upright = upright_image::upright(bytes);
        Ok(vec![with_density(jpeg(name, &upright)?, &upright)])
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(vec![with_density(png_image(name, bytes)?, bytes)])
    } else if is_tiff(bytes) {
        tiff_pages(name, bytes, wanted)
    } else if bytes.starts_with(b"BM") {
        Ok(vec![with_density(decoded(name, bytes, image::ImageFormat::Bmp)?, bytes)])
    } else if is_gif(bytes) {
        Ok(vec![decoded(name, bytes, image::ImageFormat::Gif)?])
    } else if bytes.starts_with(JP2_SIGNATURE) || bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        Ok(vec![jpx(name, bytes)?])
    } else {
        Err(CreateError::Image(name.into(), "use a PNG, JPEG, JPEG 2000, TIFF, GIF or BMP image".into()))
    }
}

/// The resolution of a single-image file (JPEG, PNG, BMP) by the one rule for every format (see `density`): what the
/// file carries when both axes are a real unit inside the sane range, else 72 dpi.
fn with_density(mut img: Embedded, bytes: &[u8]) -> Embedded {
    img.dpi = density::image_density(bytes).unwrap_or((72.0, 72.0));
    img
}

/// Embed an image file (its first page, for TIFFs) as an image XObject in `doc`. Returns the
/// object and the image's natural size in points (from its resolution).
pub fn image_xobject(doc: &mut Document, name: &str, bytes: &[u8]) -> Result<(ObjRef, (f64, f64)), CreateError> {
    let img = embed(name, bytes, 1)?.into_iter().next().ok_or_else(|| CreateError::Image(name.into(), "the file has no image".into()))?;
    let size = (img.px.0 as f64 * 72.0 / img.dpi.0, img.px.1 as f64 * 72.0 / img.dpi.1);
    Ok((add_image(doc, img, &mut Vec::new()), size))
}

/// Resolution used to size PDF pages. Image pixels are never resampled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ImageResolution {
    /// Embedded resolution, falling back to 72 dpi when absent.
    #[default]
    Embedded,
    /// Override both axes with a finite resolution between 1 and 1200 dpi.
    Dpi(f64),
}

/// One page per image, each the size of its image at the image's resolution.
pub fn from_images(images: &[(String, Vec<u8>)]) -> Result<Document, CreateError> {
    from_images_with_resolution(images, ImageResolution::Embedded)
}

/// Create image pages with embedded resolution or a fixed dpi (72 gives one point per pixel).
pub fn from_images_with_resolution(images: &[(String, Vec<u8>)], resolution: ImageResolution) -> Result<Document, CreateError> {
    if let ImageResolution::Dpi(dpi) = resolution
        && (!dpi.is_finite() || !(1.0..=1200.0).contains(&dpi))
    {
        return Err(CreateError::Invalid("image DPI must be finite and between 1 and 1200".into()));
    }
    if images.is_empty() {
        return Err(CreateError::Invalid("no images".into()));
    }
    let mut doc = Document::new_empty();
    let mut profiles = Vec::new();
    for img in images.iter().map(|(name, bytes)| embed(name, bytes, usize::MAX)).collect::<Result<Vec<_>, _>>()?.into_iter().flatten() {
        let dpi = match resolution {
            ImageResolution::Embedded => img.dpi,
            ImageResolution::Dpi(dpi) => (dpi, dpi),
        };
        let (mut w, mut h) = (img.px.0 as f64 * 72.0 / dpi.0, img.px.1 as f64 * 72.0 / dpi.1);
        // Keep huge images within the largest page PDF allows.
        let k = (MAX_SIDE / w.max(h)).min(1.0);
        w *= k;
        h *= k;
        let xr = add_image(&mut doc, img, &mut profiles);
        let mut xobj = Dict::new();
        xobj.set(b"Im0".to_vec(), Object::Ref(xr));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobj));
        let content = format!("q {w:.3} 0 0 {h:.3} 0 0 cm /Im0 Do Q\n").into_bytes();
        add_page(&mut doc, w, h, res, Some(content))?;
    }
    if let Some((name, _)) = images.first() {
        set_title(&mut doc, name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s));
    }
    Ok(doc)
}

// ── text ────────────────────────────────────────────────────────────────────────────────────

/// The text of a plain-text file. A byte order mark picks UTF-8 or UTF-16 (little or big
/// endian) and is dropped; without one the bytes are UTF-8 when they are valid UTF-8, and
/// otherwise the Windows code page they most likely are: 1254 (Turkish) or 1252 (Western).
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return encoding.decode_without_bom_handling(bytes.get(bom..).unwrap_or_default()).0.into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let encoding = if turkish_code_page(bytes) { encoding_rs::WINDOWS_1254 } else { encoding_rs::WINDOWS_1252 };
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

/// Windows-1254 and 1252 differ only in six bytes: Ğ İ Ş ğ ı ş in 1254 are Ð Ý Þ ð ý þ in 1252.
/// Of the Western languages only Icelandic and Faroese use those, and both also write á í ó ú,
/// which Turkish never does.
fn turkish_code_page(bytes: &[u8]) -> bool {
    bytes.iter().any(|b| matches!(b, 0xD0 | 0xDD | 0xDE | 0xF0 | 0xFD | 0xFE))
        && !bytes.iter().any(|b| matches!(b, 0xC1 | 0xCD | 0xD3 | 0xDA | 0xE1 | 0xED | 0xF3 | 0xFA))
}

/// Plain text set in Helvetica on pages of `page` size with 1-inch margins. Text Helvetica can't
/// draw (outside Western European) is refused with [`CreateError::Invalid`] naming the line and
/// the character; text PdfCraft generates goes through [`from_text_escaped`] instead.
pub fn from_text(title: &str, text: &str, page: (f64, f64), font_size: f64) -> Result<Document, CreateError> {
    // Helvetica in WinAnsiEncoding: text it can't draw is refused with where it is, not set as `?`.
    if let Some((line, c)) = first_undrawable_line(text) {
        return Err(CreateError::Invalid(pdfcraft_fonts::undrawable_message(&format!("the text (line {line})"), c)));
    }
    set_text(title, text, page, font_size)
}

/// The line (1-based) and character of the first one Helvetica can't draw. Line breaks, tabs and
/// form feeds (new pages) are layout, not drawn.
fn first_undrawable_line(text: &str) -> Option<(usize, char)> {
    text.split('\n').enumerate().find_map(|(i, l)| pdfcraft_fonts::first_undrawable(&l.replace('\u{c}', "")).map(|c| (i + 1, c)))
}

/// What a PDF made from text PdfCraft generated says, in its first line, about characters it
/// wrote as `[U+XXXX]`.
pub const ESCAPED_NOTE: &str = "Characters Helvetica can't draw are written as [U+XXXX] code points.";

/// [`from_text`] for text PdfCraft generates from documents (a comment summary, a compare
/// report), which may quote any language. Each character Helvetica can't draw is written as
/// `[U+XXXX]` instead of being refused or set as `?`, so nothing is lost, and the first line says
/// so ([`ESCAPED_NOTE`]). Returns the document and how many characters were written that way.
pub fn from_text_escaped(title: &str, text: &str, page: (f64, f64), font_size: f64) -> Result<(Document, usize), CreateError> {
    let mut escaped = 0usize;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\n' | '\r' | '\u{c}') || pdfcraft_fonts::first_undrawable(c.encode_utf8(&mut [0; 4])).is_none() {
            out.push(c);
        } else {
            escaped = escaped.saturating_add(1);
            out.push_str(&format!("[U+{:04X}]", u32::from(c)));
        }
    }
    if escaped > 0 {
        out = format!("{ESCAPED_NOTE}\n\n{out}");
    }
    // The title is a text string in /Info, which holds any character.
    Ok((set_text(title, &out, page, font_size)?, escaped))
}

fn set_text(title: &str, text: &str, page: (f64, f64), font_size: f64) -> Result<Document, CreateError> {
    let size = if font_size.is_finite() && font_size > 0.0 { font_size.clamp(4.0, 72.0) } else { 11.0 };
    let (w, h) = page;
    let margin = 72.0;
    let line_h = size * 1.25;
    let width = (w - 2.0 * margin).max(36.0);
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\t', "    ")
        .split('\u{c}')
        .flat_map(|p| wrap(p, size, width).into_iter().chain(std::iter::once("\u{c}".to_string())))
        .collect();
    let per_page = (((h - 2.0 * margin) / line_h).floor() as usize).max(1);
    let mut doc = Document::new_empty();
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let fr = doc.add(font);
    let mut page_lines: Vec<Vec<String>> = vec![Vec::new()];
    for l in lines.iter().take(lines.len().saturating_sub(1)) {
        if l == "\u{c}" || page_lines.last().is_some_and(|p| p.len() >= per_page) {
            page_lines.push(Vec::new());
            if l == "\u{c}" {
                continue;
            }
        }
        if let Some(p) = page_lines.last_mut() {
            p.push(l.clone());
        }
    }
    for lines in page_lines {
        let mut c = format!("BT /F1 {size} Tf {line_h:.3} TL {margin} {:.3} Td\n", h - margin - size).into_bytes();
        for l in lines {
            c.extend(literal(&win_ansi(&l)));
            c.extend_from_slice(b" Tj T*\n");
        }
        c.extend_from_slice(b"ET\n");
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Ref(fr));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        add_page(&mut doc, w, h, res, Some(c))?;
    }
    set_title(&mut doc, title);
    Ok(doc)
}

#[cfg(test)]
mod tests;
