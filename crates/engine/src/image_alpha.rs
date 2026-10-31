//! Edit text & images: a picture is picked where it shows, not across its whole box. A PNG
//! signature or a cut-out photo with a transparent margin overlaps its neighbours; clicking a
//! neighbour through that margin picked the transparent picture on top.
//!
//! A picture with transparency (a soft mask, a stencil or colour-key `/Mask`, or a stencil image
//! `/ImageMask`) gets an opacity grid of at most [`MAX_GRID`] cells a side, built once from its
//! pixels and kept until the document changes. A cell counts as opaque if any pixel
//! in it is: thin strokes and the edges of a shape stay easy to pick. Pictures without
//! transparency, Form artwork, pictures that show nowhere and pictures that can't be decoded
//! keep their whole box.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pdfcraft_cos::{ObjRef, Object};

use crate::Document;

/// The opacity grid's largest side, in cells.
const MAX_GRID: u32 = 256;
/// Grids kept at once (at most 64 KiB each); past this the cache starts over.
const MAX_CACHED: usize = 256;
/// Pictures larger than this many pixels keep their whole box rather than being decoded.
const MAX_DECODE_PIXELS: u64 = 64_000_000;
/// Alpha at or above this (of 255) counts as showing.
const OPAQUE: u8 = 16;
/// Colour components assumed for a colour space that can't be read: as many as a DeviceN space
/// may have in practice, so the decoding bound stays generous.
const MAX_COMPONENTS: u64 = 32;
/// Decoded bytes allowed past what a picture's declared size needs (with a quarter more), for
/// padding writers leave after the samples.
const DECODE_SLACK: u64 = 64 << 10;

/// Which cells of a picture show, row 0 at the top of the picture.
#[derive(Debug)]
pub(crate) struct Opacity {
    width: u32,
    height: u32,
    cells: Vec<bool>,
}

/// Opacity grids by picture object, for the document generation they were built at; `None` for
/// a picture that is opaque everywhere, shows nowhere or couldn't be read (its whole box counts). The
/// generation, not the picture object alone, says when to rebuild: an edit may change the
/// picture's soft mask without touching the picture.
type Grids = (u64, HashMap<ObjRef, Option<Arc<Opacity>>>);

#[derive(Default)]
pub(crate) struct AlphaCache(Mutex<Grids>);

impl Document {
    /// Whether `image` shows at user-space `point` (assumed inside its placement): `false` only
    /// where a picture with transparency is see-through. Form artwork, pictures without
    /// transparency and pictures that can't be read show everywhere in their box.
    pub fn image_shows_at(&self, image: &pdfcraft_edit::PageImage, point: [f64; 2]) -> bool {
        if image.is_form {
            return true;
        }
        let Some(object) = image.object else { return true };
        let Some(grid) = self.opacity(object) else { return true };
        let Some((x, y)) = unit_point(image.matrix, point) else { return true };
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return true;
        }
        // Image space: row 0 is the top of the unit square (y = 1).
        let col = ((x * f64::from(grid.width)) as u32).min(grid.width.saturating_sub(1));
        let row = (((1.0 - y) * f64::from(grid.height)) as u32).min(grid.height.saturating_sub(1));
        let at = usize::try_from(u64::from(row) * u64::from(grid.width) + u64::from(col)).unwrap_or(usize::MAX);
        grid.cells.get(at).copied().unwrap_or(true)
    }

    /// The opacity grid of picture `object`, built on first use and kept until the document
    /// changes.
    fn opacity(&self, object: ObjRef) -> Option<Arc<Opacity>> {
        let editor = self.editor.as_ref()?;
        // A poisoned cache is only a cache: build without it.
        if let Ok(cache) = self.alpha.0.lock()
            && cache.0 == self.generation
            && let Some(grid) = cache.1.get(&object)
        {
            return grid.clone();
        }
        let grid = build(&editor.cos, object).map(Arc::new);
        if let Ok(mut cache) = self.alpha.0.lock() {
            if cache.0 != self.generation || cache.1.len() >= MAX_CACHED {
                *cache = (self.generation, HashMap::new());
            }
            cache.1.insert(object, grid.clone());
        }
        grid
    }
}

/// The picture's opacity grid, or `None` when it is opaque everywhere or can't be read.
fn build(cos: &pdfcraft_cos::Document, object: ObjRef) -> Option<Opacity> {
    let current = cos.get(object);
    let Object::Stream(s) = &*current else { return None };
    let stencil = flag(cos, s, b"ImageMask");
    let soft = s.dict.get(b"SMask").is_some_and(|v| cos.resolve(v).as_name() != Some(b"None"));
    let in_data = s.dict.int(b"SMaskInData").is_some_and(|v| v != 0);
    if !(stencil || soft || in_data || s.dict.get(b"Mask").is_some()) {
        return None;
    }
    // The picture and the mask images it brings are decoded in full before anything is scaled
    // down to the picture's size, so each must be small enough, in pixels and in decoded bytes,
    // before any of it is decoded.
    let components = if stencil { 1 } else { components(cos, s.dict.get(b"ColorSpace")) };
    if !within_budget(cos, s, components, stencil) {
        return None;
    }
    for key in [&b"SMask"[..], b"Mask"] {
        if let Some(Object::Stream(mask)) = s.dict.get(key).map(|v| cos.resolve(v)).as_deref()
            && !within_budget(cos, mask, 1, key == b"Mask" || flag(cos, mask, b"ImageMask"))
        {
            return None;
        }
    }
    let (_, bytes) = pdfcraft_create::image_file(cos, object).ok()?;
    let format = image::guess_format(&bytes).ok()?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes.as_slice()), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_PIXELS.saturating_mul(4));
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let (w, h) = (picture.width(), picture.height());
    if w == 0 || h == 0 {
        return None;
    }
    let shows: Box<dyn Fn(u32, u32) -> bool> = if picture.color().has_alpha() {
        let rgba = picture.into_rgba8();
        Box::new(move |x, y| rgba.get_pixel_checked(x, y).is_none_or(|p| p.0[3] >= OPAQUE))
    } else if stencil {
        // The export draws a stencil image's painted samples white, whatever its `/Decode`.
        let luma = picture.into_luma8();
        Box::new(move |x, y| luma.get_pixel_checked(x, y).is_none_or(|p| p.0[0] >= 128))
    } else {
        return None;
    };
    let (gw, gh) = (w.min(MAX_GRID), h.min(MAX_GRID));
    let mut cells = vec![false; usize::try_from(u64::from(gw) * u64::from(gh)).ok()?];
    for y in 0..h {
        let rows = cells_covered(y, h, gh);
        for x in 0..w {
            if shows(x, y) {
                // Every cell the pixel reaches into shows: a point anywhere on a showing pixel
                // picks the picture, also where the pixel crosses into the next cell.
                for row in rows.clone() {
                    for col in cells_covered(x, w, gw) {
                        if let Some(cell) = usize::try_from(row * u64::from(gw) + col).ok().and_then(|i| cells.get_mut(i)) {
                            *cell = true;
                        }
                    }
                }
            }
        }
    }
    // A picture that shows everywhere, or nowhere (a faint watermark, a transparent
    // placeholder), keeps its whole box: one that showed nowhere couldn't be picked at all.
    if cells.iter().all(|c| *c) || !cells.iter().any(|c| *c) {
        return None;
    }
    Some(Opacity { width: gw, height: gh, cells })
}

/// The grid cells, along an axis of `pixels` pixels and `cells` cells, that pixel `i` reaches
/// into: from the cell its start is in to the cell its end is in. `cells` is at most `pixels`, so
/// a pixel reaches into at most two.
fn cells_covered(i: u32, pixels: u32, cells: u32) -> std::ops::Range<u64> {
    let (i, pixels, cells) = (u64::from(i), u64::from(pixels).max(1), u64::from(cells));
    let start = i * cells / pixels;
    let end = ((i + 1) * cells).div_ceil(pixels).min(cells);
    start..end.max(start)
}

/// Whether the boolean `key` of image stream `s` is `true`.
fn flag(cos: &pdfcraft_cos::Document, s: &pdfcraft_cos::Stream, key: &[u8]) -> bool {
    s.dict.get(key).is_some_and(|v| matches!(&*cos.resolve(v), Object::Bool(true)))
}

/// Whether image stream `s`, of `components` colour components (one bit each for a `stencil`),
/// is small enough to decode for its opacity grid: at most [`MAX_DECODE_PIXELS`] pixels and, when
/// its samples are decoded through its filters, no more decoded bytes than its declared size
/// needs, with room for padding a writer left after the samples. Without that bound a 1 × 1
/// picture whose stream inflates to a gigabyte would be decoded in full on the first hover. JPEG,
/// JPEG 2000 and JBIG2 data go to their own decoders instead.
fn within_budget(cos: &pdfcraft_cos::Document, s: &pdfcraft_cos::Stream, components: u64, stencil: bool) -> bool {
    let size = |key: &[u8]| u64::try_from(s.dict.int(key).unwrap_or(0)).unwrap_or(0);
    let (w, h) = (size(b"Width"), size(b"Height"));
    if w.saturating_mul(h) > MAX_DECODE_PIXELS {
        return false;
    }
    let codec = match s.dict.get(b"Filter").map(|f| cos.resolve(f)).as_deref() {
        Some(Object::Name(n)) => Some(n.clone()),
        Some(Object::Array(a)) => a.last().and_then(Object::as_name).map(<[u8]>::to_vec),
        _ => None,
    };
    if matches!(codec.as_deref(), Some(b"DCTDecode" | b"DCT" | b"JPXDecode" | b"JBIG2Decode")) {
        return true;
    }
    let bpc = if stencil { 1 } else { u64::try_from(s.dict.int(b"BitsPerComponent").unwrap_or(8)).unwrap_or(8).clamp(1, 16) };
    let row = w.checked_mul(components).and_then(|n| n.checked_mul(bpc)).map(|bits| bits.div_ceil(8));
    let budget = row.and_then(|r| r.checked_mul(h)).and_then(|n| n.checked_add(n / 4)).and_then(|n| n.checked_add(DECODE_SLACK));
    let Some(budget) = budget.and_then(|n| usize::try_from(n).ok()) else { return false };
    cos.decoded_image_within(s, budget).is_ok()
}

/// The colour components of an image in colour space `space` (ISO 32000-2 §8.6): one for an
/// indexed or separation space, the `/N` of an ICC-based one, as many as a DeviceN space names.
/// A space that can't be read counts as [`MAX_COMPONENTS`], which keeps the decoding bound
/// generous rather than refusing the picture.
fn components(cos: &pdfcraft_cos::Document, space: Option<&Object>) -> u64 {
    let Some(space) = space.map(|s| cos.resolve(s)) else { return MAX_COMPONENTS };
    let array = space.as_array();
    let family = match (&*space, array.and_then(|a| a.first()).and_then(Object::as_name)) {
        (Object::Name(n), _) => n.as_slice(),
        (_, Some(n)) => n,
        _ => return MAX_COMPONENTS,
    };
    let operand = || array.and_then(|a| a.get(1)).map(|o| cos.resolve(o));
    match family {
        b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
        b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => match operand().as_deref() {
            Some(Object::Stream(icc)) => {
                icc.dict.int(b"N").and_then(|n| u64::try_from(n).ok()).filter(|n| (1..=MAX_COMPONENTS).contains(n)).unwrap_or(MAX_COMPONENTS)
            }
            _ => MAX_COMPONENTS,
        },
        b"DeviceN" => match operand().as_deref() {
            Some(Object::Array(names)) => u64::try_from(names.len()).unwrap_or(MAX_COMPONENTS).clamp(1, MAX_COMPONENTS),
            _ => MAX_COMPONENTS,
        },
        _ => MAX_COMPONENTS,
    }
}

/// `point` in the placement's unit square: the inverse of `matrix` applied to it.
fn unit_point(m: [f64; 6], point: [f64; 2]) -> Option<(f64, f64)> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let (dx, dy) = (point[0] - m[4], point[1] - m[5]);
    let x = (m[3] * dx - m[2] * dy) / det;
    let y = (m[0] * dy - m[1] * dx) / det;
    (x.is_finite() && y.is_finite()).then_some((x, y))
}
