//! pdfcraft-create — create PDFs from nothing, images, text or (through LibreOffice) Office documents (L4). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream, serialize};

mod extract;
mod image_probe;
mod text_font;
mod upright_image;
pub use extract::{ExtractedImage, ImageExport, extract_images, image_file, image_file_within};
#[cfg(test)]
use image_probe::JP2_SIGNATURE;
use image_probe::{is_jpx, probe_jpeg, probe_jpx};
pub use upright_image::jpeg_exif_orientation;
mod office;
#[cfg(not(target_arch = "wasm32"))]
pub use office::libreoffice;
pub use office::{OFFICE_EXTENSIONS, is_office, office_to_pdf};
use pdfcraft_fonts::wrap_with;
use text_font::TextFont;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CreateError {
    #[error("{0}: {1}")]
    Image(String, String),
    #[error("{0}")]
    Invalid(String),
    /// An Office document LibreOffice couldn't convert, or LibreOffice isn't installed.
    #[error("{0}")]
    Office(String),
}

/// US Letter in points.
pub const LETTER: (f64, f64) = (612.0, 792.0);

/// The largest page side PDF allows (ISO 32000-2 Annex C: 14 400 units).
const MAX_SIDE: f64 = 14_400.0;

/// The page tree of a document being made, with the pages added to it so far: its `/Kids` are
/// written once, by [`Pages::finish`]. Adding each page to the tree in the document would copy every
/// page before it, which a text of a million form feeds makes quadratic.
struct Pages {
    tree: ObjRef,
    kids: Vec<Object>,
}

impl Pages {
    /// The page tree of `doc`, with the pages it has.
    fn of(doc: &Document) -> Result<Self, CreateError> {
        // `Document::new_empty` always has a page tree.
        let tree = doc
            .root()
            .and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages")))
            .ok_or_else(|| CreateError::Invalid("the new document has no page tree".into()))?;
        let kids = doc.get(tree).as_dict().and_then(|d| d.get(b"Kids")).and_then(|k| k.as_array().cloned()).unwrap_or_default();
        Ok(Self { tree, kids })
    }

    fn add(&mut self, doc: &mut Document, w: f64, h: f64, resources: Dict, content: Option<Vec<u8>>) {
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(self.tree));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)]));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        if let Some(c) = content {
            let s = doc.add(Object::Stream(Stream::flate(Dict::new(), &c)));
            page.set(b"Contents".to_vec(), Object::Ref(s));
        }
        self.kids.push(Object::Ref(doc.add(page)));
    }

    /// Writes the pages added into the tree.
    fn finish(self, doc: &mut Document) -> Result<(), CreateError> {
        let count = i64::try_from(self.kids.len()).map_err(|_| CreateError::Invalid("too many pages".into()))?;
        doc.update_dict(self.tree, |d| {
            d.set(b"Count".to_vec(), Object::Int(count));
            d.set(b"Kids".to_vec(), Object::Array(self.kids));
        })
        .map_err(|e| CreateError::Invalid(e.to_string()))
    }
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
    let mut tree = Pages::of(&doc)?;
    for _ in 0..pages {
        tree.add(&mut doc, width, height, Dict::new(), None);
    }
    tree.finish(&mut doc)?;
    Ok(doc)
}

/// A document whose pages hold ready-made content: one `(width, height, resources, content)`
/// tuple per page, in page order. Empty content vectors make blank pages. The OCR Verify
/// screen's editable-text output builds its pages this way.
pub fn from_contents(pages: &[(f64, f64, Dict, Vec<u8>)]) -> Result<Document, CreateError> {
    if pages.is_empty() || pages.len() > 10_000 {
        return Err(CreateError::Invalid("invalid page count".into()));
    }
    if pages.iter().any(|(w, h, _, _)| !(w.is_finite() && h.is_finite() && (3.0..=MAX_SIDE).contains(w) && (3.0..=MAX_SIDE).contains(h))) {
        return Err(CreateError::Invalid("invalid page size".into()));
    }
    let mut doc = Document::new_empty();
    let mut tree = Pages::of(&doc)?;
    for (w, h, resources, content) in pages {
        let content = if content.is_empty() { None } else { Some(content.clone()) };
        tree.add(&mut doc, *w, *h, resources.clone(), content);
    }
    tree.finish(&mut doc)?;
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

/// The samples of a black-and-white image packed one bit per pixel (1 for white, each row
/// starting on a byte): eight-bit grey (`n` 1) or RGB (`n` 3) whose every pixel is pure black or
/// pure white, or one-bit grey, already packed (a bilevel TIFF). `None` for anything else, found
/// at its first other pixel.
fn bilevel(data: &[u8], (w, h): (u32, u32), n: usize, bits: Option<i64>) -> Option<Vec<u8>> {
    let (w, h) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let stride = w.div_ceil(8);
    let size = stride.checked_mul(h)?;
    match (bits, n) {
        (Some(1), 1) => data.get(..size).map(<[u8]>::to_vec),
        (Some(8), 1 | 3) if w > 0 => {
            let samples = data.get(..w.checked_mul(h)?.checked_mul(n)?)?;
            let mut packed = vec![0u8; size];
            for (i, px) in samples.chunks_exact(n).enumerate() {
                let white = match px {
                    [0] | [0, 0, 0] => false,
                    [255] | [255, 255, 255] => true,
                    _ => return None,
                };
                if white && let Some(b) = packed.get_mut((i / w) * stride + (i % w) / 8) {
                    *b |= 0x80 >> (i % w % 8);
                }
            }
            Some(packed)
        }
        _ => None,
    }
}

/// Add `img` to `doc` as an image XObject. An ICC profile becomes an `/ICCBased` colour space
/// (with the device space as `/Alternate`); `profiles` shares one object between images that
/// carry the same profile.
///
/// A black-and-white image is stored at one bit per pixel, as CCITT Group 4 when that is smaller
/// than Flate: losslessly, and several times smaller than the eight bits a scan saved as a 1-bit
/// PNG decodes to. RGB keeps its colour space when it has a colour profile.
fn add_image(doc: &mut Document, img: Embedded, profiles: &mut Vec<(Vec<u8>, ObjRef)>) -> ObjRef {
    let mut d = img.dict;
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(img.px.0 as i64));
    d.set(b"Height".to_vec(), Object::Int(img.px.1 as i64));
    let device = d.name(b"ColorSpace").map(<[u8]>::to_vec);
    let mut n = match device.as_deref() {
        Some(b"DeviceGray") => 1,
        Some(b"DeviceRGB") => 3,
        Some(b"DeviceCMYK") => 4,
        _ => 0,
    };
    let (mut data, mut filtered) = (img.data, img.filtered);
    let profiled_rgb = n == 3 && img.icc.is_some();
    if !filtered
        && !profiled_rgb
        && let Some(packed) = bilevel(&data, img.px, n, d.int(b"BitsPerComponent"))
    {
        if n == 3 {
            d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
            n = 1;
        }
        d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        // The smaller as stored, dictionary included: Group 4's parameters outweigh its saving on
        // a tiny image.
        let stored = |d: &Dict, data: &[u8]| {
            let mut out = Vec::new();
            serialize(&Object::Dict(d.clone()), &mut out);
            out.len().saturating_add(data.len())
        };
        let mut flate_dict = d.clone();
        flate_dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        let flate = pdfcraft_filters::encode_flate(&packed);
        (d, data) = match pdfcraft_filters::encode_g4(&packed, img.px.0, img.px.1) {
            Ok(g4) => {
                let mut parms = Dict::new();
                parms.set(b"K".to_vec(), Object::Int(-1));
                parms.set(b"Columns".to_vec(), Object::Int(i64::from(img.px.0)));
                parms.set(b"Rows".to_vec(), Object::Int(i64::from(img.px.1)));
                let mut g4_dict = d.clone();
                g4_dict.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
                g4_dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
                if stored(&g4_dict, &g4) < stored(&flate_dict, &flate) { (g4_dict, g4) } else { (flate_dict, flate) }
            }
            Err(_) => (flate_dict, flate),
        };
        filtered = true;
    }
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
    let stream = if filtered { Stream::from_raw(d, data) } else { Stream::flate(d, &data) };
    doc.add(Object::Stream(stream))
}

/// A JPEG 2000 image (JP2 file or raw codestream), embedded as is (`/JPXDecode`; the colour
/// space comes from the file).
fn jpx(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let probe = probe_jpx(bytes).map_err(|e| CreateError::Image(name.into(), e.message().into()))?;
    let mut d = Dict::new();
    d.set(b"Filter".to_vec(), Object::name("JPXDecode"));
    Ok(Embedded { dict: d, data: bytes.to_vec(), filtered: true, smask: None, icc: None, px: (probe.width, probe.height), dpi: probe.dpi })
}

fn jpeg(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let probe = probe_jpeg(bytes).map_err(|e| CreateError::Image(name.into(), e.message()))?;
    let mut d = Dict::new();
    if probe.invert_cmyk {
        // Adobe writes CMYK JPEGs inverted.
        d.set(b"Decode".to_vec(), Object::Array([1, 0, 1, 0, 1, 0, 1, 0].iter().map(|v| Object::Int(*v)).collect()));
    }
    d.set(b"ColorSpace".to_vec(), Object::name(probe.color_space));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    Ok(Embedded { dict: d, data: bytes.to_vec(), filtered: true, smask: None, icc: probe.icc, px: (probe.width, probe.height), dpi: probe.dpi })
}

fn png_image(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let bad = |m: String| CreateError::Image(name.into(), m);
    let opts = craft_image_codecs::DecodeOptions {
        keep_orientation: true,
        limits: craft_image_codecs::Limits { max_alloc: MAX_DECODED_BYTES as u64, ..Default::default() },
    };
    let img = craft_image_codecs::decode_as_with(craft_image_codecs::Format::Png, bytes, &opts).map_err(|e| match e {
        craft_image_codecs::CodecError::LimitExceeded(m) => bad(format!("image too large: {m}")),
        other => bad(other.to_string()),
    })?;
    let dpi = img.meta.dpi.map(|(x, y)| (f64::from(x), f64::from(y))).filter(|&(x, y)| x > 0.0 && y > 0.0).unwrap_or((72.0, 72.0));
    let icc = img.icc.clone();
    let (w, h) = (img.width(), img.height());
    let (channels, cs) = match img.layout() {
        craft_image_codecs::ChannelLayout::Gray => (1, "DeviceGray"),
        craft_image_codecs::ChannelLayout::GrayA => (2, "DeviceGray"),
        craft_image_codecs::ChannelLayout::Rgb => (3, "DeviceRGB"),
        craft_image_codecs::ChannelLayout::Rgba => (4, "DeviceRGB"),
        _ => return Err(bad("unexpected palette output".into())),
    };
    let data: std::borrow::Cow<'_, [u8]> = if let Some(u8s) = img.as_u8() {
        std::borrow::Cow::Borrowed(u8s)
    } else if let Some(u16s) = img.to_u16_samples() {
        std::borrow::Cow::Owned(u16s.into_iter().map(|v| (v >> 8) as u8).collect())
    } else {
        return Err(bad("expected 8-bit or 16-bit PNG samples".into()));
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
    Ok(Embedded { dict: d, data: colour, filtered: false, smask, icc, px: (w, h), dpi })
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

/// BMP and GIF (the first frame), through `craft-image-codecs`. Their resolution is not read:
/// 72 dpi, as for images without one.
fn decoded(name: &str, bytes: &[u8], format: craft_image_codecs::Format) -> Result<Embedded, CreateError> {
    // A GIF's first frame is painted on a canvas the size of its logical screen, however small
    // the frame: a few bytes can ask for hundreds of MB.
    if format == craft_image_codecs::Format::Gif
        && let Some(&[w0, w1, h0, h1]) = bytes.get(6..10)
        && u64::from(u16::from_le_bytes([w0, w1])) * u64::from(u16::from_le_bytes([h0, h1])) > MAX_GIF_PIXELS
    {
        return Err(CreateError::Image(name.into(), "the GIF's screen is larger than 4096×4096 pixels".into()));
    }
    let img = craft_image_codecs::decode_as_with(format, bytes, &craft_image_codecs::DecodeOptions::default())
        .map_err(|e| CreateError::Image(name.into(), e.to_string()))?;
    let dims = (img.width(), img.height());
    let raw = img.to_rgba8();
    Ok(rgba_image(&raw, dims, (72.0, 72.0)))
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}

/// A PCX image: a 128-byte header (size at 4-11, resolution at 12 and 14, plane count at 65, bytes
/// per plane row at 66), then run-length coded scanlines, each holding one row of every plane.
fn pcx(name: &str, bytes: &[u8]) -> Result<Embedded, CreateError> {
    let bad = |m: &str| CreateError::Image(name.into(), m.into());
    let header = bytes.get(..128).ok_or_else(|| bad("a damaged PCX header"))?;
    let word = |at: usize| le16(header, at).unwrap_or(0);
    let byte = |at: usize| header.get(at).copied().unwrap_or(0);
    let (bits, planes, stride) = (byte(3), usize::from(byte(65)), usize::from(word(66)));
    let (Some(w), Some(h)) = (word(8).checked_sub(word(4)), word(10).checked_sub(word(6))) else {
        return Err(bad("a damaged PCX header"));
    };
    let (w, h) = (usize::from(w) + 1, usize::from(h) + 1);
    if planes == 0 || stride == 0 {
        return Err(bad("a damaged PCX header"));
    }
    if !matches!((bits, planes), (8, 1 | 3) | (1, 1..=4)) {
        return Err(bad("this PCX plane layout is not supported"));
    }
    if stride < (w * usize::from(bits)).div_ceil(8) {
        return Err(bad("a damaged PCX header"));
    }
    let line = planes * stride;
    if line.saturating_mul(h) > MAX_DECODED_BYTES || w.saturating_mul(h).saturating_mul(3) > MAX_DECODED_BYTES {
        return Err(bad("unusual PCX image size"));
    }
    // Preserve Create PDF's legacy palette policy, not a second pixel decoder: monochrome
    // is black/white and an indexed file without its VGA palette is interpreted as gray.
    // The shared reader owns all RLE, plane unpacking and palette application.
    let has_vga = bytes.len().checked_sub(769).is_some_and(|at| at >= 128 && bytes.get(at) == Some(&0x0C));
    // The pinned decoder predates shared version-4 recognition. Its version byte is
    // only a header acceptance check, not a switch for pixel or palette semantics.
    // Keep this app's existing version-4 acceptance without replacing the shared pin.
    let version_four = byte(1) == 4;
    let normalize = version_four || (bits == 1 && planes == 1) || (bits == 8 && planes == 1 && !has_vga);
    let mut input = std::borrow::Cow::Borrowed(bytes);
    if normalize {
        // Normalization must not add an unbounded copy of hostile compressed input.
        if bytes.len() > MAX_DECODED_BYTES {
            return Err(bad("unusual PCX input size"));
        }
        let mut copy = Vec::new();
        copy.try_reserve_exact(bytes.len()).map_err(|_| bad("not enough memory for this image"))?;
        copy.extend_from_slice(bytes);
        if version_four && let Some(version) = copy.get_mut(1) {
            *version = 5;
        }
        if bits == 1 && planes == 1 {
            if let Some(palette) = copy.get_mut(16..22) {
                palette.copy_from_slice(&[0, 0, 0, 255, 255, 255]);
            }
        } else if bits == 8
            && planes == 1
            && !has_vga
            && let Some(info) = copy.get_mut(68..70)
        {
            info.copy_from_slice(&2u16.to_le_bytes());
        }
        input = std::borrow::Cow::Owned(copy);
    }
    let opts = craft_image_codecs::DecodeOptions {
        limits: craft_image_codecs::Limits { max_alloc: MAX_DECODED_BYTES as u64, ..Default::default() },
        ..Default::default()
    };
    let image = craft_image_codecs::decode_as_with(craft_image_codecs::Format::Pcx, &input, &opts).map_err(|e| bad(&e.to_string()))?;
    // The accepted layouts above are RGB8 in the pinned shared decoder. Refuse an unexpected
    // contract rather than interpreting alpha or high-depth bytes as DeviceRGB samples.
    if image.layout() != craft_image_codecs::ChannelLayout::Rgb || image.sample_type() != craft_image_codecs::SampleType::U8 {
        return Err(bad("unexpected PCX pixel layout"));
    }
    let data = image.into_data();
    let dpi = (f64::from(word(12)), f64::from(word(14)));
    let dpi = if dpi.0 > 0.0 && dpi.1 > 0.0 { dpi } else { (72.0, 72.0) };
    let mut d = Dict::new();
    d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let px = (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0));
    Ok(Embedded { dict: d, data, filtered: false, smask: None, icc: None, px, dpi })
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

/// Returns whether the TIFF directory at `ifd_offset` is a 1-bit bilevel grayscale image.
fn tiff_page_is_bilevel(bytes: &[u8], ifd_offset: u64) -> bool {
    let order = match bytes.get(0..2) {
        Some(b"II") => craft_tiff::ByteOrder::Little,
        Some(b"MM") => craft_tiff::ByteOrder::Big,
        _ => return false,
    };
    let bigtiff = order.read_u16(bytes, 2) == Some(43);
    let Ok((ifd, _)) = craft_tiff::parse_ifd_at(bytes, ifd_offset, order, 0, bigtiff, &craft_tiff::ParseOptions::default()) else {
        return false;
    };
    let Some(bits) = ifd.u64s(craft_tiff::tags::BITS_PER_SAMPLE) else {
        return false;
    };
    bits.len() == 1 && bits[0] == 1
}

/// The first `wanted` pages of a TIFF (multi-page scans become multi-page PDFs).
fn tiff_pages(name: &str, bytes: &[u8], wanted: usize) -> Result<Vec<Embedded>, CreateError> {
    tiff_pages_within(name, bytes, wanted, MAX_DECODED_TOTAL)
}

/// [`tiff_pages`], refused once the pages kept take more than `budget` bytes.
fn tiff_pages_within(name: &str, bytes: &[u8], wanted: usize, budget: usize) -> Result<Vec<Embedded>, CreateError> {
    use craft_image_codecs::ChannelLayout as L;
    let bad = |m: String| CreateError::Image(name.into(), m);
    let info = craft_image_codecs::tiff_info(bytes).map_err(|e| bad(e.to_string()))?;
    let opts = craft_image_codecs::DecodeOptions {
        keep_orientation: true,
        limits: craft_image_codecs::Limits { max_alloc: MAX_DECODED_BYTES as u64, ..Default::default() },
    };
    let mut out = Vec::new();
    let mut kept = 0usize;
    for (idx, p) in info.pages.iter().enumerate() {
        if out.len() >= wanted {
            break;
        }
        if p.kind == craft_image_codecs::TiffPageKind::ReducedResolution {
            continue;
        }
        let img = craft_image_codecs::decode_tiff_page(bytes, idx, &opts).map_err(|e| bad(e.to_string()))?;
        let (w, h) = (img.width(), img.height());
        let dpi = img.meta.dpi.map(|(x, y)| (f64::from(x), f64::from(y))).filter(|&(x, y)| x > 0.0 && y > 0.0).unwrap_or((72.0, 72.0));
        let data_buf: std::borrow::Cow<'_, [u8]> = if let Some(u8s) = img.as_u8() {
            std::borrow::Cow::Borrowed(u8s)
        } else if let Some(u16s) = img.to_u16_samples() {
            std::borrow::Cow::Owned(u16s.into_iter().map(|v| (v >> 8) as u8).collect())
        } else {
            return Err(bad("this TIFF sample format isn't supported".into()));
        };
        let data = data_buf.as_ref();
        let page = match img.layout() {
            L::Gray if tiff_page_is_bilevel(bytes, p.ifd_offset) => {
                // Bilevel scans stay 1 bit per pixel (0 = black).
                let row_bytes = (w as usize).div_ceil(8);
                let mut packed = vec![0u8; row_bytes.saturating_mul(h as usize)];
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        if data[y * w as usize + x] >= 128 {
                            packed[y * row_bytes + (x >> 3)] |= 0x80 >> (x & 7);
                        }
                    }
                }
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
                Embedded { dict: d, data: packed, filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            L::Gray => {
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                Embedded { dict: d, data: data.to_vec(), filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            L::Rgb => {
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                Embedded { dict: d, data: data.to_vec(), filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            L::Cmyk => {
                let mut d = Dict::new();
                d.set(b"ColorSpace".to_vec(), Object::name("DeviceCMYK"));
                d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                Embedded { dict: d, data: data.to_vec(), filtered: false, smask: None, icc: None, px: (w, h), dpi }
            }
            L::Rgba => rgba_image(data, (w, h), dpi),
            L::GrayA => {
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
    }
    Ok(out)
}

/// What a file picked for Create is, by its bytes (and, for text, its name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Pdf,
    Image,
    Text,
    /// A Word, Excel, PowerPoint or OpenDocument file, converted through LibreOffice.
    Office,
}

/// File extensions of the images Create converts to PDF.
pub const IMAGE_EXTENSIONS: [&str; 11] = ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx", "pcx"];

/// File extensions of the plain-text files Create converts to PDF.
pub const TEXT_EXTENSIONS: [&str; 2] = ["txt", "text"];

/// File extensions Create converts to PDF itself: [`IMAGE_EXTENSIONS`] and [`TEXT_EXTENSIONS`]
/// (Office documents go through LibreOffice, [`OFFICE_EXTENSIONS`]).
pub const CONVERTIBLE: [&str; 13] = ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx", "pcx", "txt", "text"];

/// A PCX file: its manufacturer byte, a version (1 was never used), run-length coding and 1 or 8
/// bits per plane. `pcx` says when the rest of the header isn't one it can read.
fn is_pcx(bytes: &[u8]) -> bool {
    bytes.first() == Some(&0x0A) && matches!(bytes.get(1), Some(0 | 2..=5)) && bytes.get(2) == Some(&1) && matches!(bytes.get(3), Some(1 | 8))
}

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
        || is_pcx(bytes)
        // JPEG 2000: a JP2 file or a raw codestream.
        || is_jpx(bytes)
        // BMP: "BM" and a known header size (so text starting with "BM" stays text).
        || (bytes.starts_with(b"BM")
            && bytes.get(14..18).and_then(|h| <[u8; 4]>::try_from(h).ok()).is_some_and(|h| matches!(u32::from_le_bytes(h), 12 | 40 | 52 | 56 | 108 | 124)))
}

/// Whether `bytes` named `name` is a PDF, an image Create can embed, plain text (a `.txt` or
/// `.text` file) or an Office document ([`is_office`]). `None` for anything else.
pub fn source_kind(name: &str, bytes: &[u8]) -> Option<SourceKind> {
    let head = bytes.get(..bytes.len().min(1024)).unwrap_or_default();
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(SourceKind::Pdf);
    }
    if is_image(bytes) {
        return Some(SourceKind::Image);
    }
    if is_office(name, bytes) {
        return Some(SourceKind::Office);
    }
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".txt") || lower.ends_with(".text")).then_some(SourceKind::Text)
}

/// Detect the image format from its bytes; a TIFF may hold several pages, of which the first
/// `wanted` are read.
fn embed(name: &str, bytes: &[u8], wanted: usize) -> Result<Vec<Embedded>, CreateError> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        Ok(vec![jpeg(name, &upright_image::upright(bytes))?])
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(vec![png_image(name, bytes)?])
    } else if is_tiff(bytes) {
        tiff_pages(name, bytes, wanted)
    } else if bytes.starts_with(b"BM") {
        Ok(vec![decoded(name, &pdfcraft_xfa::image::bmp_with_pixel_offset(bytes), craft_image_codecs::Format::Bmp)?])
    } else if is_gif(bytes) {
        Ok(vec![decoded(name, bytes, craft_image_codecs::Format::Gif)?])
    } else if is_pcx(bytes) {
        Ok(vec![pcx(name, bytes)?])
    } else if is_jpx(bytes) {
        Ok(vec![jpx(name, bytes)?])
    } else {
        Err(CreateError::Image(name.into(), "use a PNG, JPEG, JPEG 2000, TIFF, GIF or BMP image".into()))
    }
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
    let mut tree = Pages::of(&doc)?;
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
        tree.add(&mut doc, w, h, res, Some(content));
    }
    tree.finish(&mut doc)?;
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
/// draw (outside Western European) is set in the bundled Inter face instead, whole, when Inter
/// draws all of it (Latin, Greek and Cyrillic letters); anything else is refused with
/// [`CreateError::Invalid`] naming the line and the first character neither face draws. Text
/// PdfCraft generates goes through [`from_text_escaped`] instead.
pub fn from_text(title: &str, text: &str, page: (f64, f64), font_size: f64) -> Result<Document, CreateError> {
    // Helvetica in WinAnsiEncoding draws it: set as it always was.
    if first_undrawable_line(text).is_none() {
        return set_text(title, text, page, font_size, false);
    }
    // Otherwise the bundled face, or a refusal with where it is, never `?`.
    if let Some((line, c)) = text_font::first_missing(text) {
        return Err(CreateError::Invalid(text_font::undrawable_message(line, c)));
    }
    set_text(title, &text.chars().map(text_font::drawn_as).collect::<String>(), page, font_size, true)
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
/// report), which may quote any language. Text the bundled Inter face draws all of is set as
/// [`from_text`] sets it, nothing escaped. Otherwise each character Helvetica can't draw is
/// written as `[U+XXXX]` instead of being refused or set as `?`, so nothing is lost, and the first
/// line says so ([`ESCAPED_NOTE`]). Returns the document and how many characters were written
/// that way.
pub fn from_text_escaped(title: &str, text: &str, page: (f64, f64), font_size: f64) -> Result<(Document, usize), CreateError> {
    if first_undrawable_line(text).is_some() && text_font::first_missing(text).is_none() {
        return Ok((set_text(title, &text.chars().map(text_font::drawn_as).collect::<String>(), page, font_size, true)?, 0));
    }
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
    Ok((set_text(title, &out, page, font_size, false)?, escaped))
}

/// `text` set on pages of `page` size, in Helvetica or, with `inter`, the bundled Inter face.
fn set_text(title: &str, text: &str, page: (f64, f64), font_size: f64, inter: bool) -> Result<Document, CreateError> {
    let size = if font_size.is_finite() && font_size > 0.0 { font_size.clamp(4.0, 72.0) } else { 11.0 };
    let (w, h) = page;
    let margin = 72.0;
    let line_h = size * 1.25;
    let width = (w - 2.0 * margin).max(36.0);
    let mut doc = Document::new_empty();
    let font = if inter { TextFont::inter(&mut doc)? } else { TextFont::helvetica(&mut doc) };
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\t', "    ")
        .split('\u{c}')
        .flat_map(|p| wrap_with(p, size, width, |s, size| font.width(s, size)).into_iter().chain(std::iter::once("\u{c}".to_string())))
        .collect();
    let per_page = (((h - 2.0 * margin) / line_h).floor() as usize).max(1);
    let mut tree = Pages::of(&doc)?;
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
            c.extend(font.show(&l)?);
            c.extend_from_slice(b" Tj T*\n");
        }
        c.extend_from_slice(b"ET\n");
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Ref(font.font()));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        tree.add(&mut doc, w, h, res, Some(c));
    }
    tree.finish(&mut doc)?;
    set_title(&mut doc, title);
    Ok(doc)
}

#[cfg(test)]
mod tests;
