//! Edit text & images: a picture is picked where it shows, not across its whole box. A PNG
//! signature or a cut-out photo with a transparent margin overlaps its neighbours; clicking a
//! neighbour through that margin picked the transparent picture on top.
//!
//! A picture with transparency (a soft mask, a stencil or colour-key `/Mask`, or a stencil image
//! `/ImageMask`) gets an opacity grid of at most [`MAX_GRID`] cells a side, built once from its
//! pixels and kept until the document changes. A cell counts as opaque if any pixel
//! in it is: thin strokes and the edges of a shape stay easy to pick. Pictures without
//! transparency, Form artwork, and pictures that can't be decoded keep their whole box.

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

/// Which cells of a picture show, row 0 at the top of the picture.
#[derive(Debug)]
pub(crate) struct Opacity {
    width: u32,
    height: u32,
    cells: Vec<bool>,
}

/// Opacity grids by picture object, for the document generation they were built at; `None` for
/// a picture that is opaque everywhere or couldn't be read (its whole box counts). The
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
    let flag = |key: &[u8]| s.dict.get(key).is_some_and(|v| matches!(&*cos.resolve(v), Object::Bool(true)));
    let stencil = flag(b"ImageMask");
    let soft = s.dict.get(b"SMask").is_some_and(|v| cos.resolve(v).as_name() != Some(b"None"));
    let in_data = s.dict.int(b"SMaskInData").is_some_and(|v| v != 0);
    if !(stencil || soft || in_data || s.dict.get(b"Mask").is_some()) {
        return None;
    }
    let size = |key: &[u8]| u64::try_from(s.dict.int(key).unwrap_or(0)).unwrap_or(0);
    if size(b"Width").saturating_mul(size(b"Height")) > MAX_DECODE_PIXELS {
        return None;
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
        let row = u64::from(y) * u64::from(gh) / u64::from(h);
        for x in 0..w {
            if shows(x, y) {
                let col = u64::from(x) * u64::from(gw) / u64::from(w);
                if let Some(cell) = usize::try_from(row * u64::from(gw) + col).ok().and_then(|i| cells.get_mut(i)) {
                    *cell = true;
                }
            }
        }
    }
    if cells.iter().all(|c| *c) {
        return None;
    }
    Some(Opacity { width: gw, height: gh, cells })
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
