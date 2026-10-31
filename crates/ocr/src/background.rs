//! Background de-rendering for a ClearScan page (stage 6).
//!
//! The vector glyphs replace the scanned lettering. The scan has to lose that lettering first,
//! or the fuzzy ink shows around the outlines as a second, softer copy. [`derender`] paints each
//! text blob with the paper colour sampled around it. A one-pixel halo covers the grey fringe a
//! binary mask misses. Photographs, rules and other drawings passed as `protect` stay, including
//! where that halo would have covered them. Text ink that a protect region also covers is still
//! erased, so a dark ghost cannot survive an overlap.
//!
//! [`embed_background`] stores the cleaned raster as an image XObject (ISO 32000-2 §8.9). A
//! one-channel scan, and colour whose channels are equal, is DeviceGray with FlateDecode. A tinted
//! page without a photograph's colour variation is DeviceRGB with FlateDecode. A page whose chroma
//! varies is JPEG DCTDecode. There is no JBIG2 encoder here: the decoders in the tree only read
//! JBIG2, and the usual encoder is AGPL, so a monochrome scan uses Flate.
//!
//! [`content_stream`] is the page content: `q`, the image `Do`, `Q`, then a text object that fills
//! the Type 3 glyphs. Each glyph is placed with its own `Tm` at the baseline, so a scanned word
//! does not depend on the font's advance.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::OcrError;
use crate::binarize::Bitmap;
use crate::components::{Component, ComponentKind};
use crate::grey::{self, MAX_PIXELS};

/// Halo past this is clamped. A wider ring starts to fill the counters of small letters.
pub const MAX_HALO: u32 = 8;
/// Paper is sampled at most this far from the glyph box.
pub const MAX_SAMPLE_RADIUS: u32 = 32;
/// JPEG quality [`encoding_for`] picks for a colour photograph.
pub const DEFAULT_JPEG_QUALITY: u8 = 75;
/// A working map larger than this is painted in tiles, so one page cannot allocate a second
/// full-size buffer on top of the raster copy.
const LOCAL_MAP_PIXELS: u64 = 1 << 20;
/// Chroma standard deviation, in 8-bit levels, above which a page is treated as a photograph.
const PHOTO_CHROMA: f64 = 8.0;
/// Channel spread at or below this is grey, even when the raster has three channels.
const GREY_SPREAD: u8 = 2;
/// How many pixels the paper-colour and chroma decisions look at.
const SAMPLE_CAP: usize = 16_384;

/// The knobs of [`derender`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DerenderParams {
    /// Pixels added around each ink pixel, Chebyshev distance. One covers a grey fringe.
    pub halo: u32,
    /// How far past the halo the paper colour is sampled.
    pub sample_radius: u32,
}

impl Default for DerenderParams {
    fn default() -> Self {
        DerenderParams { halo: 1, sample_radius: 4 }
    }
}

/// A page raster: `width` × `height` pixels, row-major, top-left origin, 1 or 3 bytes each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRaster {
    width: u32,
    height: u32,
    channels: u8,
    data: Vec<u8>,
}

impl PageRaster {
    /// `channels` is 1 (grey), 3 (RGB) or 4 (RGBA; the alpha byte is dropped).
    pub fn from_pixels(width: u32, height: u32, channels: u32, pixels: &[u8]) -> Result<PageRaster, OcrError> {
        if !matches!(channels, 1 | 3 | 4) {
            return Err(background_err(format!("expected 1, 3 or 4 bytes per pixel, got {channels}")));
        }
        if width == 0 || height == 0 {
            return Err(background_err("the image is empty"));
        }
        let pixel_count = u64::from(width).checked_mul(u64::from(height)).ok_or_else(|| background_err("the image is too large"))?;
        if pixel_count > MAX_PIXELS {
            return Err(background_err(format!("the image has {pixel_count} pixels; at most {MAX_PIXELS} are cleaned")));
        }
        let stored = if channels == 4 { 3 } else { channels };
        let expected = pixel_count.saturating_mul(u64::from(channels));
        if pixels.len() as u64 != expected {
            return Err(background_err(format!(
                "expected {expected} bytes for {width}×{height} at {channels} bytes per pixel, got {}",
                pixels.len()
            )));
        }
        let data = if channels == 4 { drop_alpha(pixels) } else { pixels.to_vec() };
        let channels = u8::try_from(stored).unwrap_or(3);
        Ok(PageRaster { width, height, channels, data })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The bytes of pixel `(x, y)`, or `None` outside the raster.
    pub fn pixel(&self, x: u32, y: u32) -> Option<&[u8]> {
        let start = self.offset(x, y)?;
        let end = start.saturating_add(usize::from(self.channels));
        self.data.get(start..end)
    }

    fn offset(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let row = (y as usize).checked_mul(self.width as usize)?;
        let pixel = row.checked_add(x as usize)?;
        pixel.checked_mul(usize::from(self.channels))
    }
}

fn drop_alpha(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() / 4 * 3);
    for px in pixels.as_chunks::<4>().0 {
        out.extend_from_slice(&px[..3]);
    }
    out
}

fn background_err(detail: impl Into<String>) -> OcrError {
    OcrError::Background(detail.into())
}

/// Ink to erase, or ink to keep. `bounds` is half-open `[left, top, right, bottom]` in raster
/// pixels, the same box a [`Component`] carries. `mask`, when it matches that box, is the ink;
/// its `(0, 0)` is `(left, top)`. A missing mask, or one whose size is not the box, erases or
/// protects the whole box.
#[derive(Clone, Copy, Debug)]
pub struct InkRegion<'a> {
    pub bounds: [u32; 4],
    pub mask: Option<&'a Bitmap>,
}

/// Glyph blobs: the ink [`derender`] paints over.
pub fn text_regions(components: &[Component]) -> Vec<InkRegion<'_>> {
    regions_of(components, |kind| kind == ComponentKind::Glyph)
}

/// Rules, drawings and photographs: the ink [`derender`] leaves in place.
pub fn protected_regions(components: &[Component]) -> Vec<InkRegion<'_>> {
    regions_of(components, |kind| matches!(kind, ComponentKind::Graphic | ComponentKind::Photo))
}

fn regions_of(components: &[Component], keep: impl Fn(ComponentKind) -> bool) -> Vec<InkRegion<'_>> {
    components
        .iter()
        .filter(|component| keep(component.kind))
        .map(|component| InkRegion { bounds: component.bounds, mask: component.mask.as_ref() })
        .collect()
}

/// The raster after text removal, and how many pixels changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanedPage {
    raster: PageRaster,
    erased: u64,
}

impl CleanedPage {
    pub fn raster(&self) -> &PageRaster {
        &self.raster
    }

    /// Pixels whose samples changed. A pixel covered by two glyphs is counted once.
    pub fn erased(&self) -> u64 {
        self.erased
    }
}

/// Paint `text` out of `raster`. `protect` is kept, except where it is also text ink.
/// The input raster is not changed.
pub fn derender(raster: &PageRaster, text: &[InkRegion<'_>], protect: &[InkRegion<'_>], params: &DerenderParams) -> Result<CleanedPage, OcrError> {
    derender_limited(raster, text, protect, params, LOCAL_MAP_PIXELS)
}

fn derender_limited(
    raster: &PageRaster,
    text: &[InkRegion<'_>],
    protect: &[InkRegion<'_>],
    params: &DerenderParams,
    local_limit: u64,
) -> Result<CleanedPage, OcrError> {
    let mut cleaned = raster.clone();
    let halo = params.halo.min(MAX_HALO);
    let radius = params.sample_radius.min(MAX_SAMPLE_RADIUS);
    let mut erased = 0_u64;
    for region in text {
        erased = erased.saturating_add(erase_region(&mut cleaned, region, protect, halo, radius, local_limit));
    }
    Ok(CleanedPage { raster: cleaned, erased })
}

fn erase_region(raster: &mut PageRaster, region: &InkRegion<'_>, protect: &[InkRegion<'_>], halo: u32, radius: u32, local_limit: u64) -> u64 {
    let Some(bounds) = Rect::from_bounds(region.bounds) else {
        return 0;
    };
    let paint = bounds.expand(halo, raster.width, raster.height);
    let Some(paint) = paint else {
        return 0;
    };
    let side = tile_side(local_limit, halo);
    // One paper colour for the whole glyph. A tile in the middle of a letter has no paper of
    // its own, and sampling there would fall back to white and leave a pale block.
    let paper = sample_around(raster, bounds, protect, halo, radius);
    let mut erased = 0_u64;
    let mut top = paint.top;
    while top < paint.bottom {
        let bottom = top.saturating_add(side).min(paint.bottom);
        let mut left = paint.left;
        while left < paint.right {
            let right = left.saturating_add(side).min(paint.right);
            let tile = Rect { left, top, right, bottom };
            erased = erased.saturating_add(erase_tile(raster, region, protect, tile, halo, &paper));
            left = right;
        }
        top = bottom;
    }
    erased
}

/// Largest tile whose map, grown by `pad` on every side, still fits in `limit`.
fn tile_side(limit: u64, pad: u32) -> u32 {
    let pad2 = u64::from(pad).saturating_mul(2);
    let mut side = 512_u64.min(limit.max(1));
    while side > 1 {
        let span = side.saturating_add(pad2);
        if span.saturating_mul(span) <= limit.max(1) {
            break;
        }
        side /= 2;
    }
    u32::try_from(side).unwrap_or(1).max(1)
}

fn erase_tile(raster: &mut PageRaster, region: &InkRegion<'_>, protect: &[InkRegion<'_>], tile: Rect, halo: u32, paper: &[u8]) -> u64 {
    // The map has to see ink just outside the tile, or a halo that crosses the cut is missed.
    // Paper was already sampled for the whole glyph; the ring here is only so that stamp reaches.
    let Some(map_rect) = tile.expand(halo, raster.width, raster.height) else {
        return 0;
    };
    let Some(len) = map_rect.len() else {
        return 0;
    };
    let mut map = vec![0_u8; len];
    stamp_region(&mut map, map_rect, region, raster.width(), raster.height(), halo, true);
    for kept in protect {
        stamp_region(&mut map, map_rect, kept, raster.width(), raster.height(), 0, false);
    }
    let mut erased = 0_u64;
    for y in tile.top..tile.bottom {
        for x in tile.left..tile.right {
            let Some(index) = map_rect.index(x, y) else {
                continue;
            };
            let kind = map.get(index).copied().unwrap_or(0);
            // 1 is text ink, 2 is the halo. 3 is a protected pixel and stays.
            if (kind == 1 || kind == 2) && paint(raster, x, y, paper) {
                erased = erased.saturating_add(1);
            }
        }
    }
    erased
}

/// `text` stamps ink as 1 and, when `halo` is non-zero, the ring as 2. A protect region stamps
/// 3 over paper and over the ring, and leaves text ink alone.
fn stamp_region(map: &mut [u8], map_rect: Rect, region: &InkRegion<'_>, page_w: u32, page_h: u32, halo: u32, text: bool) {
    let Some(bounds) = Rect::from_bounds(region.bounds) else {
        return;
    };
    let matched = region.mask.filter(|mask| mask.width() == bounds.width() && mask.height() == bounds.height());
    match matched {
        Some(mask) => {
            let x0 = map_rect.left.saturating_sub(bounds.left).min(mask.width());
            let y0 = map_rect.top.saturating_sub(bounds.top).min(mask.height());
            let x1 = map_rect.right.saturating_sub(bounds.left).min(mask.width());
            let y1 = map_rect.bottom.saturating_sub(bounds.top).min(mask.height());
            for ly in y0..y1 {
                for lx in x0..x1 {
                    if !mask.is_ink(i64::from(lx), i64::from(ly)) {
                        continue;
                    }
                    let Some(x) = bounds.left.checked_add(lx) else {
                        continue;
                    };
                    let Some(y) = bounds.top.checked_add(ly) else {
                        continue;
                    };
                    if x >= page_w || y >= page_h {
                        continue;
                    }
                    stamp_pixel(map, map_rect, x, y, halo, text);
                }
            }
        }
        None => {
            let left = map_rect.left.max(bounds.left);
            let top = map_rect.top.max(bounds.top);
            let right = map_rect.right.min(bounds.right).min(page_w);
            let bottom = map_rect.bottom.min(bounds.bottom).min(page_h);
            for y in top..bottom {
                for x in left..right {
                    stamp_pixel(map, map_rect, x, y, halo, text);
                }
            }
        }
    }
}

fn stamp_pixel(map: &mut [u8], map_rect: Rect, x: u32, y: u32, halo: u32, text: bool) {
    let radius = i64::from(halo);
    let (cx, cy) = (i64::from(x), i64::from(y));
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let Some(px) = u32::try_from(cx + dx).ok() else {
                continue;
            };
            let Some(py) = u32::try_from(cy + dy).ok() else {
                continue;
            };
            let Some(index) = map_rect.index(px, py) else {
                continue;
            };
            let Some(cell) = map.get_mut(index) else {
                continue;
            };
            if text {
                if dx == 0 && dy == 0 {
                    *cell = 1;
                } else if *cell == 0 {
                    *cell = 2;
                }
            } else if *cell == 0 || *cell == 2 {
                *cell = 3;
            }
        }
    }
}

/// The paper colour around `bounds`: the ring between the halo and the sample radius.
/// Protected ink is not paper. No ring (the glyph covers the page, or the radius is zero and
/// the halo is the whole window) falls back to white.
fn sample_around(raster: &PageRaster, bounds: Rect, protect: &[InkRegion<'_>], halo: u32, radius: u32) -> Vec<u8> {
    let channels = usize::from(raster.channels());
    let pad = halo.saturating_add(radius);
    let Some(outer) = bounds.expand(pad, raster.width, raster.height) else {
        return vec![255; channels];
    };
    let inner = bounds.expand(halo, raster.width, raster.height);
    let mut samples = [Vec::new(), Vec::new(), Vec::new()];
    let stride = sample_stride(outer);
    let mut seen = 0_usize;
    for y in outer.top..outer.bottom {
        for x in outer.left..outer.right {
            if inner.is_some_and(|inner| inner.contains(x, y)) || protect.iter().any(|region| covers(region, x, y)) {
                continue;
            }
            if !seen.is_multiple_of(stride) {
                seen = seen.saturating_add(1);
                continue;
            }
            seen = seen.saturating_add(1);
            push_sample(raster, x, y, channels, &mut samples);
            if samples.first().is_some_and(|bucket| bucket.len() >= SAMPLE_CAP) {
                return medians(&mut samples, channels);
            }
        }
    }
    medians(&mut samples, channels)
}

fn push_sample(raster: &PageRaster, x: u32, y: u32, channels: usize, samples: &mut [Vec<u8>; 3]) {
    let Some(pixel) = raster.pixel(x, y) else {
        return;
    };
    for (channel, value) in pixel.iter().enumerate().take(channels) {
        if let Some(bucket) = samples.get_mut(channel) {
            bucket.push(*value);
        }
    }
}

fn covers(region: &InkRegion<'_>, x: u32, y: u32) -> bool {
    let Some(bounds) = Rect::from_bounds(region.bounds) else {
        return false;
    };
    if x < bounds.left || y < bounds.top || x >= bounds.right || y >= bounds.bottom {
        return false;
    }
    match region.mask {
        Some(mask) if mask.width() == bounds.width() && mask.height() == bounds.height() => {
            mask.is_ink(i64::from(x - bounds.left), i64::from(y - bounds.top))
        }
        _ => true,
    }
}

fn sample_stride(rect: Rect) -> usize {
    let pixels = rect.len().unwrap_or(SAMPLE_CAP);
    pixels.div_ceil(SAMPLE_CAP).max(1)
}

fn medians(samples: &mut [Vec<u8>; 3], channels: usize) -> Vec<u8> {
    let mut paper = vec![255_u8; channels];
    for (channel, bucket) in samples.iter_mut().enumerate().take(channels) {
        if let Some(slot) = paper.get_mut(channel) {
            *slot = median(bucket);
        }
    }
    paper
}

fn median(values: &mut [u8]) -> u8 {
    if values.is_empty() {
        return 255;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values.get(mid).copied().unwrap_or(255)
    } else {
        let left = values.get(mid.saturating_sub(1)).copied().unwrap_or(255);
        let right = values.get(mid).copied().unwrap_or(255);
        let mixed = u16::from(left) + u16::from(right);
        u8::try_from(mixed / 2).unwrap_or(255)
    }
}

fn paint(raster: &mut PageRaster, x: u32, y: u32, paper: &[u8]) -> bool {
    let channels = usize::from(raster.channels());
    let Some(start) = raster.offset(x, y) else {
        return false;
    };
    let Some(pixel) = raster.data.get_mut(start..start.saturating_add(channels)) else {
        return false;
    };
    let Some(color) = paper.get(..channels) else {
        return false;
    };
    if pixel == color {
        return false;
    }
    pixel.copy_from_slice(color);
    true
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Rect {
    fn from_bounds(bounds: [u32; 4]) -> Option<Rect> {
        if bounds[2] > bounds[0] && bounds[3] > bounds[1] {
            Some(Rect { left: bounds[0], top: bounds[1], right: bounds[2], bottom: bounds[3] })
        } else {
            None
        }
    }

    fn width(self) -> u32 {
        self.right.saturating_sub(self.left)
    }

    fn height(self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }

    fn len(self) -> Option<usize> {
        let pixels = u64::from(self.width()).checked_mul(u64::from(self.height()))?;
        usize::try_from(pixels).ok()
    }

    fn contains(self, x: u32, y: u32) -> bool {
        x >= self.left && y >= self.top && x < self.right && y < self.bottom
    }

    fn expand(self, pad: u32, page_w: u32, page_h: u32) -> Option<Rect> {
        let left = self.left.saturating_sub(pad);
        let top = self.top.saturating_sub(pad);
        let right = self.right.saturating_add(pad).min(page_w);
        let bottom = self.bottom.saturating_add(pad).min(page_h);
        Rect::from_bounds([left, top, right, bottom])
    }

    fn index(self, x: u32, y: u32) -> Option<usize> {
        if x < self.left || y < self.top || x >= self.right || y >= self.bottom {
            return None;
        }
        let row = ((y - self.top) as usize).checked_mul(self.width() as usize)?;
        row.checked_add((x - self.left) as usize)
    }
}

/// How [`embed_background`] compresses a cleaned page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundEncoding {
    /// 8-bit DeviceGray, FlateDecode. A one-channel raster is stored as it is. A colour raster
    /// is converted with the same Rec. 601 luma as [`crate::grey::to_grey`].
    FlateGray,
    /// 8-bit DeviceRGB, FlateDecode.
    FlateRgb,
    /// JPEG DCTDecode, quality 1..=100 (values outside that are clamped).
    Jpeg { quality: u8 },
}

/// Flate for grey and for a flat tint, JPEG when the chroma varies like a photograph.
pub fn encoding_for(raster: &PageRaster) -> BackgroundEncoding {
    if raster.channels() == 1 || grey_rgb(raster) {
        BackgroundEncoding::FlateGray
    } else if photographic(raster) {
        BackgroundEncoding::Jpeg { quality: DEFAULT_JPEG_QUALITY }
    } else {
        BackgroundEncoding::FlateRgb
    }
}

fn grey_rgb(raster: &PageRaster) -> bool {
    if raster.channels() != 3 {
        return false;
    }
    sampled_pixels(raster).all(|pixel| {
        let max = pixel.iter().copied().max().unwrap_or(0);
        let min = pixel.iter().copied().min().unwrap_or(0);
        max.saturating_sub(min) <= GREY_SPREAD
    })
}

fn photographic(raster: &PageRaster) -> bool {
    if raster.channels() != 3 {
        return false;
    }
    let mut n = 0.0_f64;
    let mut mean_rg = 0.0_f64;
    let mut mean_rb = 0.0_f64;
    let mut m2_rg = 0.0_f64;
    let mut m2_rb = 0.0_f64;
    for pixel in sampled_pixels(raster) {
        let r = f64::from(pixel.first().copied().unwrap_or(0));
        let g = f64::from(pixel.get(1).copied().unwrap_or(0));
        let b = f64::from(pixel.get(2).copied().unwrap_or(0));
        n += 1.0;
        let rg = r - g;
        let rb = r - b;
        let d_rg = rg - mean_rg;
        mean_rg += d_rg / n;
        m2_rg += d_rg * (rg - mean_rg);
        let d_rb = rb - mean_rb;
        mean_rb += d_rb / n;
        m2_rb += d_rb * (rb - mean_rb);
    }
    if n < 2.0 {
        return false;
    }
    let denom = n - 1.0;
    (m2_rg / denom).sqrt() > PHOTO_CHROMA || (m2_rb / denom).sqrt() > PHOTO_CHROMA
}

fn sampled_pixels(raster: &PageRaster) -> impl Iterator<Item = &[u8]> {
    let pixels = raster.data().len() / usize::from(raster.channels()).max(1);
    let stride = pixels.div_ceil(SAMPLE_CAP).max(1);
    let channels = usize::from(raster.channels());
    raster
        .data()
        .chunks(channels)
        .enumerate()
        .filter(move |(index, _)| index % stride == 0)
        .filter_map(move |(_, pixel)| (pixel.len() == channels).then_some(pixel))
}

/// The image XObject written into the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundImage {
    pub id: ObjRef,
    pub width: u32,
    pub height: u32,
    /// `FlateDecode` or `DCTDecode`.
    pub filter: &'static str,
    /// `DeviceGray` or `DeviceRGB`.
    pub color_space: &'static str,
}

/// Write `page`'s raster as an image XObject. The document is left unchanged when this errors.
pub fn embed_background(doc: &mut Document, page: &CleanedPage, encoding: BackgroundEncoding) -> Result<BackgroundImage, OcrError> {
    let raster = page.raster();
    let (samples, channels, filter, color_space) = encoded_samples(raster, encoding)?;
    if filter == "FlateDecode" {
        let expected = u64::from(raster.width())
            .checked_mul(u64::from(raster.height()))
            .and_then(|pixels| pixels.checked_mul(u64::from(channels)))
            .ok_or_else(|| background_err("the image is too large"))?;
        if samples.len() as u64 != expected {
            return Err(background_err(format!("{filter} produced {} bytes, expected {expected}", samples.len())));
        }
    }
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Image"));
    dict.set(b"Width".to_vec(), Object::Int(i64::from(raster.width())));
    dict.set(b"Height".to_vec(), Object::Int(i64::from(raster.height())));
    dict.set(b"ColorSpace".to_vec(), Object::name(color_space));
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let stream = match filter {
        "FlateDecode" => Stream::flate(dict, &samples),
        _ => {
            dict.set(b"Filter".to_vec(), Object::name(filter));
            Stream::from_raw(dict, samples)
        }
    };
    let id = doc.add(Object::Stream(stream));
    Ok(BackgroundImage { id, width: raster.width(), height: raster.height(), filter, color_space })
}

fn encoded_samples(raster: &PageRaster, encoding: BackgroundEncoding) -> Result<(Vec<u8>, u8, &'static str, &'static str), OcrError> {
    match encoding {
        BackgroundEncoding::FlateGray => {
            let samples = if raster.channels() == 1 {
                raster.data().to_vec()
            } else {
                grey::to_grey(raster.data(), raster.width(), raster.height(), u32::from(raster.channels())).map_err(background_err)?.data().to_vec()
            };
            Ok((samples, 1, "FlateDecode", "DeviceGray"))
        }
        BackgroundEncoding::FlateRgb => {
            if raster.channels() != 3 {
                return Err(background_err("FlateRgb needs a 3-channel raster (a grey scan is FlateGray)"));
            }
            Ok((raster.data().to_vec(), 3, "FlateDecode", "DeviceRGB"))
        }
        BackgroundEncoding::Jpeg { quality } => {
            let bytes = encode_jpeg(raster, quality)?;
            let space = if raster.channels() == 1 { "DeviceGray" } else { "DeviceRGB" };
            Ok((bytes, raster.channels(), "DCTDecode", space))
        }
    }
}

fn encode_jpeg(raster: &PageRaster, quality: u8) -> Result<Vec<u8>, OcrError> {
    let quality = quality.clamp(1, 100);
    let color = if raster.channels() == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode(raster.data(), raster.width(), raster.height(), color).map_err(|error| background_err(format!("JPEG encode: {error}")))?;
    if out.is_empty() || !out.starts_with(&[0xFF, 0xD8]) {
        return Err(background_err("JPEG encode produced no image"));
    }
    Ok(out)
}

/// One Type 3 glyph on the cleaned page. `origin` is the baseline's left end in PDF user space
/// (y up). `size` is the font size: with the Type 3 matrix it is the em, so a size equal to the
/// em in pixels draws one prototype pixel as one user unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphShow {
    pub code: u8,
    pub origin: [f64; 2],
    pub size: f64,
}

/// What [`content_stream`] places. Names are ASCII letters and digits, the image box is finite
/// and positive, and each glyph has a finite origin and a positive size.
#[derive(Clone, Copy, Debug)]
pub struct ClearScanLayer<'a> {
    pub image_name: &'a str,
    pub image_width: f64,
    pub image_height: f64,
    pub image_origin: [f64; 2],
    pub font_name: &'a str,
    pub glyphs: &'a [GlyphShow],
}

/// The content stream: the image, then the filled Type 3 text.
pub fn content_stream(layer: &ClearScanLayer<'_>) -> Result<Vec<u8>, OcrError> {
    check_layer(layer)?;
    let mut out = String::new();
    push_image(&mut out, layer);
    if !layer.glyphs.is_empty() {
        push_text(&mut out, layer);
    }
    Ok(out.into_bytes())
}

fn check_layer(layer: &ClearScanLayer<'_>) -> Result<(), OcrError> {
    pdf_name("image", layer.image_name)?;
    finite_positive("image width", layer.image_width)?;
    finite_positive("image height", layer.image_height)?;
    finite_point("image origin", layer.image_origin)?;
    if layer.glyphs.is_empty() {
        return Ok(());
    }
    pdf_name("font", layer.font_name)?;
    for (index, glyph) in layer.glyphs.iter().enumerate() {
        let number = index.saturating_add(1);
        finite_point(&format!("glyph {number} origin"), glyph.origin)?;
        finite_positive(&format!("glyph {number} size"), glyph.size)?;
    }
    Ok(())
}

fn pdf_name(what: &str, name: &str) -> Result<(), OcrError> {
    let ok = !name.is_empty() && name.len() <= 127 && name.bytes().all(|byte| byte.is_ascii_alphanumeric());
    if ok { Ok(()) } else { Err(background_err(format!("{what} name must be 1..=127 ASCII letters or digits, got {name:?}"))) }
}

fn finite_positive(what: &str, value: f64) -> Result<(), OcrError> {
    if value.is_finite() && value > 0.0 && value <= 1.0e8 {
        Ok(())
    } else {
        Err(background_err(format!("{what} must be a finite number in (0, 1e8], got {value}")))
    }
}

fn finite_point(what: &str, point: [f64; 2]) -> Result<(), OcrError> {
    if point.iter().all(|value| value.is_finite() && value.abs() <= 1.0e8) {
        Ok(())
    } else {
        Err(background_err(format!("{what} must be finite coordinates, got {point:?}")))
    }
}

fn push_image(out: &mut String, layer: &ClearScanLayer<'_>) {
    out.push_str("q\n");
    push_numbers(out, &[layer.image_width, 0.0, 0.0, layer.image_height, layer.image_origin[0], layer.image_origin[1]]);
    out.push_str(" cm\n/");
    out.push_str(layer.image_name);
    out.push_str(" Do\nQ\n");
}

fn push_text(out: &mut String, layer: &ClearScanLayer<'_>) {
    out.push_str("q\n0 g\nBT\n0 Tr\n");
    let mut current: Option<f64> = None;
    for glyph in layer.glyphs {
        if current.is_none_or(|size| (size - glyph.size).abs() > 1.0e-4) {
            out.push('/');
            out.push_str(layer.font_name);
            out.push(' ');
            out.push_str(&crate::pdf_num(glyph.size));
            out.push_str(" Tf\n");
            current = Some(glyph.size);
        }
        push_numbers(out, &[1.0, 0.0, 0.0, 1.0, glyph.origin[0], glyph.origin[1]]);
        out.push_str(" Tm\n<");
        out.push_str(&format!("{:02X}", glyph.code));
        out.push_str("> Tj\n");
    }
    out.push_str("ET\nQ\n");
}

fn push_numbers(out: &mut String, numbers: &[f64]) {
    for (index, number) in numbers.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&crate::pdf_num(*number));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binarize::Bitmap;
    use crate::components::{Component, ComponentKind};
    use crate::type3::{Type3Glyph, Type3Metrics, synthesize_type3};
    use crate::vectorize::{VectorizeParams, vectorize};
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
    use std::sync::Arc;

    fn raster(width: u32, height: u32, channels: u32, fill: &[u8]) -> PageRaster {
        let pixels = (width * height) as usize;
        let mut data = Vec::with_capacity(pixels * fill.len());
        for _ in 0..pixels {
            data.extend_from_slice(fill);
        }
        PageRaster::from_pixels(width, height, channels, &data).expect("raster")
    }

    fn paint_rect(raster: &mut PageRaster, bounds: [u32; 4], color: &[u8]) {
        for y in bounds[1]..bounds[3] {
            for x in bounds[0]..bounds[2] {
                let start = raster.offset(x, y).expect("inside");
                let n = color.len();
                raster.data[start..start + n].copy_from_slice(color);
            }
        }
    }

    fn mask(width: u32, height: u32, ink_at: impl Fn(u32, u32) -> bool) -> Bitmap {
        let mut ink = vec![0_u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                if ink_at(x, y)
                    && let Some(flag) = ink.get_mut((y * width + x) as usize)
                {
                    *flag = 1;
                }
            }
        }
        Bitmap::from_ink(width, height, ink).expect("mask")
    }

    fn solid_mask(width: u32, height: u32) -> Bitmap {
        mask(width, height, |_, _| true)
    }

    fn region<'a>(bounds: [u32; 4], mask: Option<&'a Bitmap>) -> InkRegion<'a> {
        InkRegion { bounds, mask }
    }

    fn assert_rect(raster: &PageRaster, bounds: [u32; 4], color: &[u8]) {
        for y in bounds[1]..bounds[3] {
            for x in bounds[0]..bounds[2] {
                assert_eq!(raster.pixel(x, y), Some(color), "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn glyph_ink_and_its_halo_take_the_local_paper_colour() {
        let mut page = raster(24, 16, 1, &[200]);
        // A black letter and a one-pixel grey fringe the mask does not include.
        paint_rect(&mut page, [4, 4, 8, 10], &[0]);
        for (x, y) in [(3, 4), (8, 6), (5, 3), (5, 10)] {
            paint_rect(&mut page, [x, y, x + 1, y + 1], &[120]);
        }
        let letter = solid_mask(4, 6);
        let before = page.clone();
        let cleaned = derender(&page, &[region([4, 4, 8, 10], Some(&letter))], &[], &DerenderParams::default()).expect("clean");
        assert_eq!(before, page, "the scan is left as it was");
        assert_rect(cleaned.raster(), [3, 3, 9, 11], &[200]);
        assert!(cleaned.erased() >= 4 * 6, "every ink pixel changed, erased {}", cleaned.erased());
        // Paper well outside the halo is untouched, including its not-quite-white value.
        assert_eq!(cleaned.raster().pixel(0, 0), Some(&[200][..]));
        assert_eq!(cleaned.raster().pixel(20, 12), Some(&[200][..]));
    }

    #[test]
    fn a_zero_halo_leaves_the_grey_fringe() {
        let mut page = raster(16, 12, 1, &[200]);
        paint_rect(&mut page, [4, 4, 8, 8], &[0]);
        paint_rect(&mut page, [3, 5, 4, 6], &[120]);
        let letter = solid_mask(4, 4);
        let params = DerenderParams { halo: 0, sample_radius: 3 };
        let cleaned = derender(&page, &[region([4, 4, 8, 8], Some(&letter))], &[], &params).expect("clean");
        assert_rect(cleaned.raster(), [4, 4, 8, 8], &[200]);
        assert_eq!(cleaned.raster().pixel(3, 5), Some(&[120][..]));
    }

    #[test]
    fn photographs_and_rules_stay_and_a_halo_does_not_eat_them() {
        let mut page = raster(24, 16, 1, &[210]);
        paint_rect(&mut page, [4, 4, 8, 9], &[0]);
        // The rule sits on the row the halo would paint. The photo is the dark block.
        paint_rect(&mut page, [0, 9, 24, 10], &[0]);
        paint_rect(&mut page, [16, 2, 22, 8], &[30]);
        let letter = solid_mask(4, 5);
        let photo = solid_mask(6, 6);
        let rule = solid_mask(24, 1);
        let text = [region([4, 4, 8, 9], Some(&letter))];
        let protect = [region([16, 2, 22, 8], Some(&photo)), region([0, 9, 24, 10], Some(&rule))];
        let cleaned = derender(&page, &text, &protect, &DerenderParams { halo: 1, sample_radius: 3 }).expect("clean");
        assert_rect(cleaned.raster(), [4, 4, 8, 9], &[210]);
        assert_eq!(cleaned.raster().pixel(5, 9), Some(&[0][..]), "the rule under the halo stays");
        assert_eq!(cleaned.raster().pixel(18, 4), Some(&[30][..]));
        assert_rect(cleaned.raster(), [16, 2, 22, 8], &[30]);
        // The fringe on the side that is not the rule is paper.
        assert_eq!(cleaned.raster().pixel(3, 6), Some(&[210][..]));
    }

    #[test]
    fn component_kinds_split_text_from_what_the_page_keeps() {
        let letter = solid_mask(2, 2);
        let photo = solid_mask(4, 4);
        let components = vec![
            Component { bounds: [1, 1, 3, 3], area: 4, centroid: [2.0, 2.0], mask: Some(letter), stroke_width: 1.0, kind: ComponentKind::Glyph },
            Component { bounds: [8, 1, 12, 5], area: 16, centroid: [10.0, 3.0], mask: Some(photo), stroke_width: 4.0, kind: ComponentKind::Photo },
            Component { bounds: [0, 7, 12, 8], area: 12, centroid: [6.0, 7.0], mask: None, stroke_width: 1.0, kind: ComponentKind::Graphic },
            Component { bounds: [0, 0, 1, 1], area: 1, centroid: [0.0, 0.0], mask: None, stroke_width: 1.0, kind: ComponentKind::Noise },
        ];
        let text = text_regions(&components);
        let protect = protected_regions(&components);
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].bounds, [1, 1, 3, 3]);
        assert!(text[0].mask.is_some());
        assert_eq!(protect.len(), 2);
        assert_eq!(protect[0].bounds, [8, 1, 12, 5]);
        assert!(protect[1].mask.is_none(), "a rule without a stored mask still protects its box");
    }

    #[test]
    fn colour_paper_is_sampled_on_each_channel_and_alpha_is_dropped() {
        let rgba = [250_u8, 240, 220, 128, 0, 0, 0, 255];
        let mut page = PageRaster::from_pixels(2, 1, 4, &rgba).expect("rgba");
        assert_eq!(page.channels(), 3);
        assert_eq!(page.pixel(0, 0), Some(&[250, 240, 220][..]));
        // Grow it into a page: cream paper, one black pixel, a blue photograph.
        let mut data = vec![0_u8; 8 * 8 * 3];
        for pixel in data.as_chunks_mut::<3>().0 {
            pixel.copy_from_slice(&[250, 240, 220]);
        }
        page = PageRaster::from_pixels(8, 8, 3, &data).expect("rgb");
        paint_rect(&mut page, [2, 2, 4, 5], &[0, 0, 0]);
        paint_rect(&mut page, [5, 1, 8, 4], &[20, 30, 200]);
        let letter = solid_mask(2, 3);
        let photo = solid_mask(3, 3);
        let cleaned = derender(
            &page,
            &[region([2, 2, 4, 5], Some(&letter))],
            &[region([5, 1, 8, 4], Some(&photo))],
            &DerenderParams { halo: 0, sample_radius: 2 },
        )
        .expect("clean");
        assert_eq!(cleaned.raster().pixel(2, 3), Some(&[250, 240, 220][..]));
        assert_eq!(cleaned.raster().pixel(6, 2), Some(&[20, 30, 200][..]));
    }

    #[test]
    fn a_missing_or_mismatched_mask_erases_the_box_and_no_paper_samples_fall_back_to_white() {
        let mut page = raster(10, 8, 1, &[180]);
        paint_rect(&mut page, [2, 2, 6, 6], &[0]);
        let cleaned = derender(&page, &[region([2, 2, 6, 6], None)], &[], &DerenderParams { halo: 0, sample_radius: 2 }).expect("box");
        assert_rect(cleaned.raster(), [2, 2, 6, 6], &[180]);
        assert_eq!(cleaned.raster().pixel(0, 0), Some(&[180][..]));

        let wrong = solid_mask(1, 1);
        let cleaned = derender(&page, &[region([2, 2, 6, 6], Some(&wrong))], &[], &DerenderParams::default()).expect("mismatch");
        assert_rect(cleaned.raster(), [2, 2, 6, 6], &[180]);

        // The box is the whole page, so the sample window holds no paper.
        let full = raster(4, 4, 1, &[0]);
        let cleaned = derender(&full, &[region([0, 0, 4, 4], None)], &[], &DerenderParams { halo: 0, sample_radius: 0 }).expect("full");
        assert_rect(cleaned.raster(), [0, 0, 4, 4], &[255]);
    }

    #[test]
    fn an_inverted_region_and_a_tight_limit_still_erase_every_ink_pixel() {
        let page = raster(8, 8, 1, &[190]);
        let cleaned = derender(&page, &[region([4, 4, 4, 6], None)], &[], &DerenderParams::default()).expect("empty box");
        assert_eq!(cleaned.erased(), 0);
        assert_eq!(cleaned.raster(), &page);

        let mut page = raster(20, 12, 1, &[190]);
        paint_rect(&mut page, [2, 2, 14, 8], &[0]);
        let letter = solid_mask(12, 6);
        // The map cap is one pixel, so the glyph is painted as many tiles and the halo has to
        // cross a tile edge.
        let cleaned =
            derender_limited(&page, &[region([2, 2, 14, 8], Some(&letter))], &[], &DerenderParams { halo: 1, sample_radius: 2 }, 1).expect("tiled");
        assert_rect(cleaned.raster(), [1, 1, 15, 9], &[190]);
        assert_eq!(cleaned.raster().pixel(18, 10), Some(&[190][..]));
    }

    #[test]
    fn hostile_rasters_name_the_limit() {
        let err = PageRaster::from_pixels(0, 0, 1, &[]).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
        let err = PageRaster::from_pixels(2, 2, 2, &[0; 8]).unwrap_err();
        assert!(err.to_string().contains("got 2"), "{err}");
        let err = PageRaster::from_pixels(2, 2, 1, &[1, 2, 3]).unwrap_err();
        assert!(err.to_string().contains("expected 4") && err.to_string().contains("got 3"), "{err}");
        let err = PageRaster::from_pixels(100_000, 100_000, 1, &[]).unwrap_err();
        assert!(err.to_string().contains("at most"), "{err}");
        // (2^32 − 1)² fits in a u64, so the refusal is the pixel cap, not a wrapped length.
        let err = PageRaster::from_pixels(u32::MAX, u32::MAX, 1, &[1]).unwrap_err();
        assert!(err.to_string().contains("at most") && !err.to_string().contains("expected"), "{err}");
    }

    #[test]
    fn a_grey_scan_is_flate_and_a_colour_photograph_is_jpeg() {
        let grey = raster(4, 4, 1, &[240]);
        assert_eq!(encoding_for(&grey), BackgroundEncoding::FlateGray);
        let flat = raster(4, 4, 3, &[250, 240, 220]);
        assert_eq!(encoding_for(&flat), BackgroundEncoding::FlateRgb, "a tint is not a photograph");
        let equal = raster(4, 4, 3, &[40, 40, 40]);
        assert_eq!(encoding_for(&equal), BackgroundEncoding::FlateGray);

        let mut photo = raster(16, 16, 3, &[250, 240, 220]);
        paint_rect(&mut photo, [2, 2, 10, 10], &[220, 20, 30]);
        assert_eq!(encoding_for(&photo), BackgroundEncoding::Jpeg { quality: DEFAULT_JPEG_QUALITY });

        let mut doc = Document::new_empty();
        let cleaned = derender(&grey, &[], &[], &DerenderParams::default()).expect("clean");
        let image = embed_background(&mut doc, &cleaned, BackgroundEncoding::FlateGray).expect("flate");
        assert_eq!((image.filter, image.color_space, image.width), ("FlateDecode", "DeviceGray", 4));
        let stored = decoded_image(&doc, image.id);
        assert_eq!(stored, grey.data());

        let rgb = derender(&flat, &[], &[], &DerenderParams::default()).expect("tint");
        let err = embed_background(&mut doc, &rgb, BackgroundEncoding::FlateGray);
        let grey_of_tint = err.expect("luma");
        assert_eq!(grey_of_tint.color_space, "DeviceGray");
        let err = embed_background(&mut doc, &cleaned, BackgroundEncoding::FlateRgb).unwrap_err();
        assert!(err.to_string().contains("FlateRgb"), "{err}");

        let colour = derender(&photo, &[], &[], &DerenderParams::default()).expect("photo");
        let jpeg = embed_background(&mut doc, &colour, encoding_for(colour.raster())).expect("jpeg");
        assert_eq!((jpeg.filter, jpeg.color_space), ("DCTDecode", "DeviceRGB"));
        let raw = image_raw(&doc, jpeg.id);
        assert!(raw.starts_with(&[0xFF, 0xD8]), "JPEG SOI");
        let decoded = image::load_from_memory(&raw).expect("decode jpeg").to_rgb8();
        let red = decoded.get_pixel(4, 4);
        let paper = decoded.get_pixel(14, 14);
        assert!(red[0] > red[1] + 40 && red[0] > red[2] + 40, "the photograph stays red: {red:?}");
        assert!(paper[0] > 180 && paper[1] > 180 && paper[2] > 180, "the paper stays light: {paper:?}");
    }

    #[test]
    fn jpeg_quality_is_clamped_and_a_one_channel_jpeg_is_grey() {
        let grey = raster(8, 8, 1, &[20]);
        let cleaned = derender(&grey, &[], &[], &DerenderParams::default()).expect("clean");
        let mut doc = Document::new_empty();
        let image = embed_background(&mut doc, &cleaned, BackgroundEncoding::Jpeg { quality: 0 }).expect("jpeg");
        assert_eq!((image.filter, image.color_space), ("DCTDecode", "DeviceGray"));
        assert!(image_raw(&doc, image.id).starts_with(&[0xFF, 0xD8]));
    }

    fn decoded_image(doc: &Document, id: ObjRef) -> Vec<u8> {
        let Object::Stream(stream) = &*doc.get(id) else { panic!("image is not a stream") };
        assert_eq!(stream.dict.name(b"Filter"), Some(b"FlateDecode".as_slice()));
        stream.decoded().expect("flate")
    }

    fn image_raw(doc: &Document, id: ObjRef) -> Vec<u8> {
        let Object::Stream(stream) = &*doc.get(id) else { panic!("image is not a stream") };
        stream.raw.to_vec()
    }

    #[test]
    fn the_content_stream_paints_the_image_then_the_type3_text() {
        let glyphs = [
            GlyphShow { code: 1, origin: [10.0, 20.0], size: 12.0 },
            GlyphShow { code: 2, origin: [22.5, 20.0], size: 12.0 },
            GlyphShow { code: 3, origin: [4.0, 6.0], size: 8.0 },
        ];
        let layer =
            ClearScanLayer { image_name: "Im0", image_width: 32.0, image_height: 40.0, image_origin: [1.0, 2.0], font_name: "F1", glyphs: &glyphs };
        let bytes = content_stream(&layer).expect("stream");
        let text = String::from_utf8(bytes).expect("ascii");
        assert!(text.starts_with("q\n32 0 0 40 1 2 cm\n/Im0 Do\nQ\n"), "{text}");
        assert!(text.contains("q\n0 g\nBT\n0 Tr\n/F1 12 Tf\n1 0 0 1 10 20 Tm\n<01> Tj\n"), "{text}");
        assert!(text.contains("1 0 0 1 22.5 20 Tm\n<02> Tj\n"), "{text}");
        assert!(text.contains("/F1 8 Tf\n1 0 0 1 4 6 Tm\n<03> Tj\n"), "{text}");
        assert!(text.ends_with("ET\nQ\n"), "{text}");
        assert_eq!(text.matches(" Tf\n").count(), 2, "one Tf per size: {text}");
        assert_balanced(&text);

        let image_only = ClearScanLayer { glyphs: &[], font_name: "", ..layer };
        let text = String::from_utf8(content_stream(&image_only).expect("image")).expect("ascii");
        assert_eq!(text, "q\n32 0 0 40 1 2 cm\n/Im0 Do\nQ\n");
    }

    #[test]
    fn hostile_placements_are_refused() {
        let glyph = GlyphShow { code: 1, origin: [0.0, 0.0], size: 12.0 };
        let mut layer =
            ClearScanLayer { image_name: "Im0", image_width: 10.0, image_height: 10.0, image_origin: [0.0, 0.0], font_name: "F1", glyphs: &[glyph] };
        layer.image_name = "Im 0";
        assert!(content_stream(&layer).unwrap_err().to_string().contains("image name"), "space");
        layer.image_name = "";
        assert!(content_stream(&layer).is_err());
        layer.image_name = "Im0";
        layer.image_width = f64::NAN;
        assert!(content_stream(&layer).unwrap_err().to_string().contains("image width"));
        layer.image_width = 0.0;
        assert!(content_stream(&layer).is_err());
        layer.image_width = 10.0;
        layer.image_origin = [f64::INFINITY, 0.0];
        assert!(content_stream(&layer).unwrap_err().to_string().contains("origin"));
        layer.image_origin = [0.0, 0.0];
        let zero = [GlyphShow { size: 0.0, ..glyph }];
        layer.glyphs = &zero;
        assert!(content_stream(&layer).unwrap_err().to_string().contains("size"));
        let missing = [GlyphShow { origin: [f64::NAN, 1.0], ..glyph }];
        layer.glyphs = &missing;
        assert!(content_stream(&layer).unwrap_err().to_string().contains("glyph 1"));
        layer.font_name = "/F1";
        let one = [glyph];
        layer.glyphs = &one;
        assert!(content_stream(&layer).unwrap_err().to_string().contains("font name"));
    }

    fn assert_balanced(text: &str) {
        let mut depth = 0_i32;
        for token in text.split_ascii_whitespace() {
            match token {
                "q" => depth += 1,
                "Q" => depth -= 1,
                "cm" | "Do" | "g" | "Tr" | "Tf" | "Tm" | "Tj" => assert!(depth > 0, "`{token}` outside q … Q"),
                _ => {}
            }
            assert!(depth >= 0, "unbalanced Q");
        }
        assert_eq!(depth, 0, "a q stays open");
    }

    #[test]
    fn the_rendered_page_has_no_grey_ghost_around_the_vector_glyph() {
        // 32×32 scan. Paper 250. A 4×6 black letter at (8, 10) with a grey fringe. A photograph
        // at the top right. The vector replaces the letter; the fringe must come back as paper.
        let mut page = raster(32, 32, 1, &[250]);
        paint_rect(&mut page, [8, 10, 12, 16], &[0]);
        for y in 9..17 {
            for x in 7..13 {
                let inside = (8..12).contains(&x) && (10..16).contains(&y);
                if !inside && let Some(start) = page.offset(x, y) {
                    page.data[start] = 110;
                }
            }
        }
        paint_rect(&mut page, [22, 2, 30, 10], &[25]);
        let letter = solid_mask(4, 6);
        let photo = solid_mask(8, 8);
        let cleaned = derender(
            &page,
            &[region([8, 10, 12, 16], Some(&letter))],
            &[region([22, 2, 30, 10], Some(&photo))],
            &DerenderParams { halo: 1, sample_radius: 3 },
        )
        .expect("clean");
        assert_eq!(cleaned.raster().pixel(7, 12), Some(&[250][..]), "the fringe is paper before the vectors are drawn");
        assert_eq!(cleaned.raster().pixel(9, 12), Some(&[250][..]));
        assert_eq!(cleaned.raster().pixel(24, 4), Some(&[25][..]));

        let bitmap = solid_mask(4, 6);
        let glyph = Type3Glyph { text: "H".into(), outline: vectorize(&bitmap, &VectorizeParams::default()), baseline: 6.0, advance: 4.0 };
        let metrics = Type3Metrics { ascent: 6.0, cap_height: 6.0, x_height: 3.0, descent: 0.0 };
        let mut doc = Document::new_empty();
        let font = synthesize_type3(&mut doc, &[glyph], &metrics).expect("font");
        let image = embed_background(&mut doc, &cleaned, BackgroundEncoding::FlateGray).expect("image");
        // Raster row 16 is the bottom of the letter. User y is up, and the image's first row is
        // the top of the page, so the baseline sits at 32 - 16.
        let show = [GlyphShow { code: font.codes[0], origin: [8.0, 16.0], size: 6.0 }];
        let layer =
            ClearScanLayer { image_name: "Im0", image_width: 32.0, image_height: 32.0, image_origin: [0.0, 0.0], font_name: "F1", glyphs: &show };
        let content = content_stream(&layer).expect("content");
        attach(&mut doc, image.id, font.id, &content, 32.0, 32.0);
        let bytes = write_full(&doc, &SaveOptions::default()).expect("save");
        let rendered = render(&bytes);
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        // PDF y from the bottom: raster row r is device row 31 - r.
        assert!(dark(&rendered, 10, 31 - 12), "the vector letter is ink, {:?}", sample(&rendered, 10, 31 - 12));
        assert!(light(&rendered, 7, 31 - 12), "the old fringe is paper, not a grey ghost, {:?}", sample(&rendered, 7, 31 - 12));
        assert!(dark(&rendered, 24, 31 - 4), "the photograph stays, {:?}", sample(&rendered, 24, 31 - 4));
        assert!(light(&rendered, 1, 1), "untouched paper, {:?}", sample(&rendered, 1, 1));
    }

    fn attach(doc: &mut Document, image: ObjRef, font: ObjRef, content: &[u8], width: f64, height: f64) {
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), content)));
        let pages = {
            let root = doc.trailer().reference(b"Root").expect("catalog");
            let catalog = doc.dict(&Object::Ref(root)).expect("catalog");
            catalog.reference(b"Pages").expect("pages")
        };
        let mut xobjects = Dict::new();
        xobjects.set(b"Im0".to_vec(), Object::Ref(image));
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Ref(font));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(width), Object::Real(height)]));
        page.set(b"Contents".to_vec(), Object::Ref(contents));
        page.set(b"Resources".to_vec(), Object::Dict(resources));
        let page = doc.add(Object::Dict(page));
        doc.update_dict(pages, |dict| {
            dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            dict.set(b"Count".to_vec(), Object::Int(1));
        })
        .expect("pages");
    }

    fn render(bytes: &[u8]) -> pdfcraft_render::RenderedPage {
        let mut renderer = PageRenderer::new(Arc::new(bytes.to_vec()), RenderConfig::default());
        renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() })
    }

    fn sample(page: &pdfcraft_render::RenderedPage, x: u32, y_pdf: u32) -> [u8; 4] {
        let row = page.height.saturating_sub(1).saturating_sub(y_pdf);
        let index = (row.saturating_mul(page.width).saturating_add(x)) as usize * 4;
        match page.rgba.get(index..index + 4) {
            Some([r, g, b, a]) => [*r, *g, *b, *a],
            _ => [255, 255, 255, 255],
        }
    }

    fn dark(page: &pdfcraft_render::RenderedPage, x: u32, y_pdf: u32) -> bool {
        sample(page, x, y_pdf)[0] < 80
    }

    fn light(page: &pdfcraft_render::RenderedPage, x: u32, y_pdf: u32) -> bool {
        sample(page, x, y_pdf)[0] > 200
    }
}
