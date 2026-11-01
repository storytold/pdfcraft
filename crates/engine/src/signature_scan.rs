//! Signatures scanned from a photo of paper (Fill & Sign ▸ Sign ▸ Scan).
//!
//! People sign a sheet of paper and photograph it, usually with a phone. Such a photo is far from
//! a clean signature image: it shows the table around the sheet, shadows of the phone or hand,
//! uneven light and sensor noise, and it is often larger than an image signature may be. This
//! module turns the framed part of the photo into a [`SignatureImage`]: the ink in its own colour
//! on a transparent background, trimmed, so it is placed, saved and reused like any image
//! signature.
//!
//! Everything is local and bounded: the photo is decoded within the same limits as an imported
//! image signature (a 4 MiB file, 32 megapixels, 8192 pixels per side), downscaled to at most
//! 4 megapixels and [`WORKING_MAX_SIDE`] pixels per side, and only that copy is kept. So one
//! scan of the whole frame touches at most 4 megapixels.
//!
//! The ink is separated from the paper in four steps:
//! 1. **Paper estimate.** A grayscale morphological closing (a maximum filter, then a minimum
//!    filter, van Herk/Gil-Werman, O(1) per pixel) removes every dark detail narrower than the
//!    window, i.e. the pen strokes, while following lighting gradients and sharp shadow edges.
//!    Bright noise peaks are clipped first, and edge pixels are repeated outside the image, so a
//!    table cut off by the frame keeps its size and stays background.
//! 2. **Darkness** of each pixel relative to that paper, slightly smoothed so that glossy
//!    ballpoint strokes with a lighter centre come out solid. The border of large dark areas
//!    that are not paper (the table) is cleared; shadows on the paper are kept.
//! 3. **Hysteresis.** Pixels darker than the threshold are ink; fainter pixels are kept only when
//!    they belong to a blob with real ink, so faint stroke ends survive and stray marks don't.
//!    Blobs with too little ink are dropped as dust.
//! 4. **Output.** The ink is trimmed with a small margin, drawn in its average colour with the
//!    coverage as alpha, and normalized through [`SignatureImage`].

use std::io::{Cursor, Read};
use std::sync::Arc;

use image::{DynamicImage, ImageFormat, RgbImage};

use crate::signature_image::{SignatureImage, SignatureImageError};

/// Largest photo file accepted: the image signature limit (decoding shares its limits too).
pub const MAX_PHOTO_BYTES: usize = crate::signature_image::MAX_SIGNATURE_IMAGE_BYTES;
/// Longest side of the copy that is kept and scanned (it also holds at most 4 megapixels).
pub const WORKING_MAX_SIDE: u32 = 4096;
/// Longest side of the framed area that is cleaned for the final signature.
pub const SCAN_MAX_SIDE: u32 = 2400;
/// Longest side used for a quick preview, e.g. while the frame is being dragged.
pub const PREVIEW_MAX_SIDE: u32 = 900;
/// Longest side of the downscaled photo used to find the signature.
const DETECTION_MAX_SIDE: u32 = 1024;
/// Largest signature produced, in pixels (the image signature limit).
const MAX_OUTPUT_PIXELS: u64 = 4 << 20;

/// The default of the Ink sensitivity slider (0–100).
pub const DEFAULT_SENSITIVITY: u8 = 50;
/// Detection uses a higher sensitivity so that faint strokes are framed too.
const DETECTION_SENSITIVITY: u8 = 70;
/// Darkness threshold (0–255, relative to the paper) at sensitivity 0 and 100.
const MAX_THRESHOLD: f32 = 100.0;
const MIN_THRESHOLD: f32 = 12.0;
/// Fainter pixels down to this fraction of the threshold are kept when they touch ink.
const HYSTERESIS_RATIO: f32 = 0.5;
/// Paper estimate window, relative to the photo's longest side; wider than half a felt-tip stroke.
const BACKGROUND_RADIUS_RATIO: f32 = 0.012;
const MIN_BACKGROUND_RADIUS: usize = 4;
/// One pixel of smoothing per this many pixels of the longest side.
const SMOOTHING_SCALE: usize = 1000;
/// A background darker than this fraction of the nearby paper is not paper (e.g. the table).
const DARK_AREA_RATIO: f32 = 0.45;
/// Blobs with fewer ink pixels than this fraction of the area (at least the minimum) are dust.
const SPECKLE_AREA_RATIO: f64 = 0.000_02;
const MIN_SPECKLE_AREA: usize = 6;
/// Blobs with less ink than this fraction of the largest one are not part of the signature.
const DETECTION_MIN_INK_RATIO: f64 = 0.1;
/// A detected signature this much taller than wide was photographed sideways.
const UPRIGHT_RATIO: u32 = 2;
/// Transparent margin around the trimmed signature, in pixels.
const TRIM_PADDING: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("Choose a PNG or JPEG photo.")]
    Format,
    #[error("Choose a photo smaller than 4 MiB and 32 megapixels (at most 8192 pixels per side).")]
    Size,
    #[error("No signature was found in the frame. Move the frame over the signature or raise the ink sensitivity.")]
    NoInk,
    #[error("Couldn't read the photo: {0}")]
    Read(#[from] std::io::Error),
    #[error("Couldn't decode the photo: {0}")]
    Decode(#[from] image::ImageError),
    #[error(transparent)]
    Signature(#[from] SignatureImageError),
}

/// A rectangle in photo pixels, top-left origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhotoRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PhotoRect {
    /// Keep the rectangle inside a `width`×`height` photo, at least one pixel large.
    pub fn clamped(self, width: u32, height: u32) -> Self {
        let w = self.width.clamp(1, width.max(1));
        let h = self.height.clamp(1, height.max(1));
        Self { x: self.x.min(width.saturating_sub(w)), y: self.y.min(height.saturating_sub(h)), width: w, height: h }
    }

    /// The same area after the photo (`width`×`height` before turning) is turned a quarter.
    pub fn turned(self, turn: Turn, width: u32, height: u32) -> Self {
        match turn {
            Turn::Clockwise => {
                Self { x: height.saturating_sub(self.y.saturating_add(self.height)), y: self.x, width: self.height, height: self.width }
            }
            Turn::CounterClockwise => {
                Self { x: self.y, y: width.saturating_sub(self.x.saturating_add(self.width)), width: self.height, height: self.width }
            }
        }
    }
}

/// A quarter turn of the photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Clockwise,
    CounterClockwise,
}

/// A decoded, upright and downscaled photo of a signature on paper.
#[derive(Clone, Debug, PartialEq)]
pub struct SignaturePhoto {
    rgb: Arc<RgbImage>,
}

impl SignaturePhoto {
    /// Read at most the file limit plus one byte, even if the file grows while being read.
    pub fn read(reader: impl Read) -> Result<Self, ScanError> {
        let mut bytes = Vec::new();
        reader.take(MAX_PHOTO_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ScanError> {
        if bytes.len() > MAX_PHOTO_BYTES {
            return Err(ScanError::Size);
        }
        // The image signature decoder: same limits, EXIF orientation (phones store portrait
        // photos as landscape pixels plus an orientation), scaled down to 4 megapixels.
        let image = crate::signature_image::decode_source(bytes).map_err(|e| match e {
            SignatureImageError::Size => ScanError::Size,
            SignatureImageError::Format => ScanError::Format,
            e => ScanError::Signature(e),
        })?;
        Ok(Self { rgb: Arc::new(image.into_rgb8()) })
    }

    /// Width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.rgb.width(), self.rgb.height()]
    }

    /// The photo as RGB pixels, for showing it.
    pub fn rgb(&self) -> &RgbImage {
        &self.rgb
    }

    /// The photo downscaled so that its longest side is at most `max_side`, as RGBA for display.
    pub fn thumbnail(&self, max_side: u32) -> image::RgbaImage {
        let [w, h] = self.size();
        let scale = (f64::from(max_side.max(1)) / f64::from(w.max(h).max(1))).min(1.0);
        let nw = ((f64::from(w) * scale).round() as u32).max(1);
        let nh = ((f64::from(h) * scale).round() as u32).max(1);
        DynamicImage::ImageRgb8(image::imageops::resize(&*self.rgb, nw, nh, image::imageops::FilterType::Triangle)).into_rgba8()
    }

    /// Turn the photo a quarter, returning `rect` moved along with it.
    pub fn turn(&mut self, turn: Turn, rect: PhotoRect) -> PhotoRect {
        let [w, h] = self.size();
        self.rgb = Arc::new(match turn {
            Turn::Clockwise => image::imageops::rotate90(&*self.rgb),
            Turn::CounterClockwise => image::imageops::rotate270(&*self.rgb),
        });
        rect.turned(turn, w, h)
    }

    /// Find the signature, or `None` when the photo shows no clear ink.
    pub fn detect(&self) -> Option<PhotoRect> {
        let [w, h] = self.size();
        let small = scaled(&self.rgb, PhotoRect { x: 0, y: 0, width: w, height: h }, DETECTION_MAX_SIDE);
        let (sw, sh) = (small.width(), small.height());
        let region = detect_region(&luminance(&small), sw as usize, sh as usize)?;
        let ratio = f64::from(w) / f64::from(sw.max(1));
        let scale = |v: usize| ((v as f64) * ratio).round().clamp(0.0, f64::from(u32::MAX)) as u32;
        Some(PhotoRect { x: scale(region.x), y: scale(region.y), width: scale(region.width), height: scale(region.height) }.clamped(w, h))
    }

    /// Frame the signature for the user: turn a sideways signature upright, then return its
    /// frame, or a centred default frame when no signature was found.
    pub fn auto_frame(&mut self) -> PhotoRect {
        if let Some(rect) = self.detect() {
            if rect.height > rect.width.saturating_mul(UPRIGHT_RATIO) {
                // Signatures are written horizontally. The direction is a guess; the user can
                // turn the photo further.
                return self.turn(Turn::CounterClockwise, rect);
            }
            return rect;
        }
        let [w, h] = self.size();
        PhotoRect { x: w / 7, y: h * 7 / 20, width: w * 5 / 7, height: h * 3 / 10 }.clamped(w, h)
    }

    /// Clean the framed area into a transparent signature. `max_side` bounds the processed area:
    /// [`SCAN_MAX_SIDE`] for the result, [`PREVIEW_MAX_SIDE`] for a quick preview.
    pub fn scan(&self, rect: PhotoRect, sensitivity: u8, max_side: u32) -> Result<SignatureImage, ScanError> {
        let [w, h] = self.size();
        let rect = rect.clamped(w, h);
        let area = scaled(&self.rgb, rect, max_side.clamp(16, WORKING_MAX_SIDE));
        let (aw, ah) = (area.width() as usize, area.height() as usize);
        // Size the paper estimate on the whole photo, not on the frame, so that thick strokes in
        // a tight frame are not taken for paper.
        let scale = aw as f32 / rect.width.max(1) as f32;
        let radius = (w.max(h) as f32 * BACKGROUND_RADIUS_RATIO * scale).round() as usize;
        let ink = extract_ink(&luminance(&area), aw, ah, Options { sensitivity, despeckle: true, background_radius: Some(radius) });
        let bounds = ink.bounds.ok_or(ScanError::NoInk)?;
        let png = render(&area, &ink.alpha, bounds)?;
        Ok(SignatureImage::from_bytes(&png)?)
    }
}

/// The framed area of `rgb`, downscaled so that its longest side is at most `max_side`.
fn scaled(rgb: &RgbImage, rect: PhotoRect, max_side: u32) -> RgbImage {
    let rect = rect.clamped(rgb.width(), rgb.height());
    let view = image::imageops::crop_imm(rgb, rect.x, rect.y, rect.width, rect.height).to_image();
    let longest = rect.width.max(rect.height);
    if longest <= max_side {
        return view;
    }
    let scale = f64::from(max_side) / f64::from(longest);
    let nw = ((f64::from(rect.width) * scale).round() as u32).max(1);
    let nh = ((f64::from(rect.height) * scale).round() as u32).max(1);
    image::imageops::resize(&view, nw, nh, image::imageops::FilterType::Triangle)
}

/// Perceived luminance (ITU BT.601).
fn luminance(rgb: &RgbImage) -> Vec<u8> {
    rgb.pixels().map(|p| (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])).round().clamp(0.0, 255.0) as u8).collect()
}

/// A rectangle in the pixels of a processed buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, Copy, Debug)]
struct Options {
    sensitivity: u8,
    despeckle: bool,
    /// Paper estimate window; by default a fraction of the buffer's longest side.
    background_radius: Option<usize>,
}

/// Ink coverage (0 = paper, 255 = solid ink) and the bounding box of the solid ink.
struct Ink {
    alpha: Vec<u8>,
    bounds: Option<Region>,
}

/// The darkness threshold for a sensitivity of 0–100; higher sensitivity keeps fainter strokes.
fn threshold(sensitivity: u8) -> f32 {
    let s = f32::from(sensitivity.min(100)) / 100.0;
    MAX_THRESHOLD - s * (MAX_THRESHOLD - MIN_THRESHOLD)
}

/// In-place separable box blur.
fn box_blur(values: &mut [f32], width: usize, height: usize, radius: usize) {
    if radius == 0 || width == 0 || height == 0 || values.len() != width * height {
        return;
    }
    let mut line = vec![0.0_f32; width.max(height)];
    let mut blur = |get: &dyn Fn(usize) -> f32, len: usize, out: &mut dyn FnMut(usize, f32)| {
        for (i, slot) in line.iter_mut().take(len).enumerate() {
            *slot = get(i);
        }
        let (mut sum, mut count) = (0.0_f32, 0_u32);
        for v in line.iter().take(radius.min(len)) {
            sum += v;
            count += 1;
        }
        for i in 0..len {
            if let Some(v) = line.get(i + radius).filter(|_| i + radius < len) {
                sum += v;
                count += 1;
            }
            if let Some(v) = i.checked_sub(radius + 1).and_then(|j| line.get(j)) {
                sum -= v;
                count = count.saturating_sub(1);
            }
            out(i, if count == 0 { 0.0 } else { sum / count as f32 });
        }
    };
    let mut row = vec![0.0_f32; width];
    let mut tmp = vec![0.0_f32; width];
    for y in 0..height {
        let start = y * width;
        let Some(src) = values.get(start..start + width) else { return };
        row.copy_from_slice(src);
        blur(&|x| row.get(x).copied().unwrap_or(0.0), width, &mut |x, v| {
            if let Some(slot) = tmp.get_mut(x) {
                *slot = v;
            }
        });
        if let Some(dst) = values.get_mut(start..start + width) {
            dst.copy_from_slice(&tmp);
        }
    }
    let mut column = vec![0.0_f32; height];
    let mut tmp = vec![0.0_f32; height];
    for x in 0..width {
        for (y, slot) in column.iter_mut().enumerate() {
            *slot = values.get(y * width + x).copied().unwrap_or(0.0);
        }
        blur(&|y| column.get(y).copied().unwrap_or(0.0), height, &mut |y, v| {
            if let Some(slot) = tmp.get_mut(y) {
                *slot = v;
            }
        });
        for (y, v) in tmp.iter().enumerate() {
            if let Some(slot) = values.get_mut(y * width + x) {
                *slot = *v;
            }
        }
    }
}

/// Sliding-window maximum or minimum over `radius` pixels on each side (van Herk/Gil-Werman).
/// Edge values are repeated outside the line.
fn sliding_extremum(src: &[u8], dst: &mut [u8], radius: usize, max: bool) {
    let len = src.len().min(dst.len());
    if len == 0 {
        return;
    }
    let pick = |a: u8, b: u8| if max { a.max(b) } else { a.min(b) };
    let size = 2 * radius + 1;
    let total = len + 2 * radius;
    let at = |i: usize| src.get(i.saturating_sub(radius).min(len - 1)).copied().unwrap_or(0);
    let mut g = vec![0_u8; total];
    let mut h = vec![0_u8; total];
    let mut prev = 0;
    for (i, slot) in g.iter_mut().enumerate() {
        let v = at(i);
        prev = if i % size == 0 { v } else { pick(prev, v) };
        *slot = prev;
    }
    let mut next = 0;
    for (i, slot) in h.iter_mut().enumerate().rev() {
        let v = at(i);
        next = if i == total - 1 || (i + 1) % size == 0 { v } else { pick(next, v) };
        *slot = next;
    }
    for (i, out) in dst.iter_mut().take(len).enumerate() {
        if let (Some(&a), Some(&b)) = (h.get(i), g.get(i + 2 * radius)) {
            *out = pick(a, b);
        }
    }
}

/// Separable square maximum (`max`) or minimum filter.
fn square_filter(src: &[u8], width: usize, height: usize, radius: usize, max: bool) -> Vec<u8> {
    let mut rows = vec![0_u8; src.len()];
    for (s, d) in src.chunks_exact(width.max(1)).zip(rows.chunks_exact_mut(width.max(1))) {
        sliding_extremum(s, d, radius, max);
    }
    let mut out = vec![0_u8; src.len()];
    let mut column = vec![0_u8; height];
    let mut filtered = vec![0_u8; height];
    for x in 0..width {
        for (y, slot) in column.iter_mut().enumerate() {
            *slot = rows.get(y * width + x).copied().unwrap_or(0);
        }
        sliding_extremum(&column, &mut filtered, radius, max);
        for (y, v) in filtered.iter().enumerate() {
            if let Some(slot) = out.get_mut(y * width + x) {
                *slot = *v;
            }
        }
    }
    out
}

/// The brightness of the paper behind every pixel (see the module documentation).
fn estimate_background(gray: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    let r = radius.max(MIN_BACKGROUND_RADIUS);
    // Clip bright noise peaks, which the maximum filter would otherwise pick, so that the faint
    // halo around every stroke doesn't count as ink. Only clipping keeps shadow edges sharp.
    let mut blurred: Vec<f32> = gray.iter().map(|&v| f32::from(v)).collect();
    box_blur(&mut blurred, width, height, (r / 6).max(1));
    let clipped: Vec<u8> = gray.iter().zip(&blurred).map(|(&g, &b)| f32::from(g).min(b).round().clamp(0.0, 255.0) as u8).collect();
    let closed = square_filter(&square_filter(&clipped, width, height, r, true), width, height, r, false);
    let mut smooth: Vec<f32> = closed.iter().map(|&v| f32::from(v)).collect();
    box_blur(&mut smooth, width, height, (r / 4).max(1));
    // The blur must never make the paper darker than the photo itself.
    smooth.iter().zip(gray).map(|(&s, &g)| s.max(f32::from(g)).round().clamp(1.0, 255.0) as u8).collect()
}

/// Label 8-connected blobs of non-zero `mask` pixels; returns the labels (0 = none) and count.
fn label_components(mask: &[bool], width: usize, height: usize) -> (Vec<u32>, usize) {
    let mut labels = vec![0_u32; mask.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut count = 0_usize;
    for start in 0..mask.len() {
        if !mask.get(start).copied().unwrap_or(false) || labels.get(start).copied().unwrap_or(1) != 0 {
            continue;
        }
        count += 1;
        let label = u32::try_from(count).unwrap_or(u32::MAX);
        if let Some(l) = labels.get_mut(start) {
            *l = label;
        }
        stack.push(start);
        while let Some(index) = stack.pop() {
            let (x, y) = (index % width, index / width);
            for ny in y.saturating_sub(1)..=(y + 1).min(height.saturating_sub(1)) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width.saturating_sub(1)) {
                    let n = ny * width + nx;
                    if mask.get(n).copied().unwrap_or(false)
                        && let Some(l) = labels.get_mut(n)
                        && *l == 0
                    {
                        *l = label;
                        stack.push(n);
                    }
                }
            }
        }
    }
    (labels, count)
}

/// Separate the ink from the paper (see the module documentation).
fn extract_ink(gray: &[u8], width: usize, height: usize, options: Options) -> Ink {
    let n = width * height;
    if n == 0 || gray.len() != n {
        return Ink { alpha: vec![0; gray.len()], bounds: None };
    }
    let radius =
        options.background_radius.unwrap_or_else(|| (width.max(height) as f32 * BACKGROUND_RADIUS_RATIO).round() as usize).max(MIN_BACKGROUND_RADIUS);
    let background = estimate_background(gray, width, height, radius);

    let mut darkness: Vec<f32> =
        gray.iter().zip(&background).map(|(&g, &bg)| if g >= bg { 0.0 } else { 255.0 * f32::from(bg - g) / f32::from(bg.max(1)) }).collect();
    let smoothing = width.max(height) / SMOOTHING_SCALE;
    box_blur(&mut darkness, width, height, smoothing);

    // Where the sheet meets a much darker area (the table) is not ink; a shadow on the paper is
    // only moderately darker and is kept.
    let edge = smoothing + 2;
    let brightest = square_filter(&background, width, height, edge, true);
    let darkest = square_filter(&background, width, height, edge, false);
    for ((d, &lo), &hi) in darkness.iter_mut().zip(&darkest).zip(&brightest) {
        if f32::from(lo) < f32::from(hi) * DARK_AREA_RATIO {
            *d = 0.0;
        }
    }

    let strong = threshold(options.sensitivity);
    let weak = strong * HYSTERESIS_RATIO;
    let candidates: Vec<bool> = darkness.iter().map(|&d| d >= weak).collect();
    let (labels, count) = label_components(&candidates, width, height);
    let mut strong_pixels = vec![0_usize; count + 1];
    for (&label, &d) in labels.iter().zip(&darkness) {
        if label != 0
            && d >= strong
            && let Some(c) = strong_pixels.get_mut(label as usize)
        {
            *c += 1;
        }
    }
    let min_strong = if options.despeckle { MIN_SPECKLE_AREA.max((n as f64 * SPECKLE_AREA_RATIO).round() as usize) } else { 1 };

    let mut alpha = vec![0_u8; n];
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0, 0);
    for (i, ((a, &label), &d)) in alpha.iter_mut().zip(&labels).zip(&darkness).enumerate() {
        if label == 0 || strong_pixels.get(label as usize).copied().unwrap_or(0) < min_strong {
            continue;
        }
        // Smooth ramp from the weak to the strong threshold.
        let c = ((d - weak) / (strong - weak).max(f32::EPSILON)).clamp(0.0, 1.0);
        *a = (255.0 * c * c * (3.0 - 2.0 * c)).round() as u8;
        if *a >= 128 {
            let (x, y) = (i % width, i / width);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    let bounds = (min_x <= max_x && min_y <= max_y).then(|| Region { x: min_x, y: min_y, width: max_x - min_x + 1, height: max_y - min_y + 1 });
    Ink { alpha, bounds }
}

/// Where the signature is in a (downscaled) photo of the whole sheet: nearby strokes are merged,
/// blobs touching the photo border (paper edges, table, fingers) are ignored, and the blob with
/// the most ink is framed together with the other words on its line.
fn detect_region(gray: &[u8], width: usize, height: usize) -> Option<Region> {
    let ink = extract_ink(gray, width, height, Options { sensitivity: DETECTION_SENSITIVITY, despeckle: true, background_radius: None });
    ink.bounds?;
    let solid: Vec<u8> = ink.alpha.iter().map(|&a| if a >= 128 { 255 } else { 0 }).collect();
    let radius = ((width.max(height) as f64) * 0.03).round().max(2.0) as usize;
    let merged: Vec<bool> = square_filter(&solid, width, height, radius, true).iter().map(|&v| v != 0).collect();
    let (labels, count) = label_components(&merged, width, height);

    #[derive(Clone, Copy)]
    struct Blob {
        ink: usize,
        min_x: usize,
        min_y: usize,
        max_x: usize,
        max_y: usize,
        border: bool,
    }
    let mut blobs = vec![Blob { ink: 0, min_x: usize::MAX, min_y: usize::MAX, max_x: 0, max_y: 0, border: false }; count + 1];
    for (i, (&label, &s)) in labels.iter().zip(&solid).enumerate() {
        let Some(blob) = blobs.get_mut(label as usize).filter(|_| label != 0) else { continue };
        let (x, y) = (i % width, i / width);
        if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
            blob.border = true;
        }
        if s != 0 {
            blob.ink += 1;
            blob.min_x = blob.min_x.min(x);
            blob.min_y = blob.min_y.min(y);
            blob.max_x = blob.max_x.max(x);
            blob.max_y = blob.max_y.max(y);
        }
    }
    let candidates = || blobs.iter().skip(1).filter(|b| !b.border && b.ink > 0);
    let best = *candidates().max_by_key(|b| b.ink)?;
    // Signatures often have several words: add every blob with a fair amount of ink on the same line.
    let span = best.max_y - best.min_y;
    let (top, bottom) = (best.min_y.saturating_sub(span), best.max_y + span);
    let min_ink = best.ink as f64 * DETECTION_MIN_INK_RATIO;
    let mut frame = best;
    for b in candidates().filter(|b| b.ink as f64 >= min_ink && b.max_y >= top && b.min_y <= bottom) {
        frame.min_x = frame.min_x.min(b.min_x);
        frame.min_y = frame.min_y.min(b.min_y);
        frame.max_x = frame.max_x.max(b.max_x);
        frame.max_y = frame.max_y.max(b.max_y);
    }
    let pad = radius;
    let (x, y) = (frame.min_x.saturating_sub(pad), frame.min_y.saturating_sub(pad));
    Some(Region { x, y, width: (frame.max_x + 1 + pad).min(width) - x, height: (frame.max_y + 1 + pad).min(height) - y })
}

/// The trimmed ink in its average colour with the coverage as alpha, as PNG.
fn render(area: &RgbImage, alpha: &[u8], bounds: Region) -> Result<Vec<u8>, ScanError> {
    let (w, h) = (area.width() as usize, area.height() as usize);
    let x0 = bounds.x.saturating_sub(TRIM_PADDING);
    let y0 = bounds.y.saturating_sub(TRIM_PADDING);
    let x1 = (bounds.x + bounds.width + TRIM_PADDING).min(w);
    let y1 = (bounds.y + bounds.height + TRIM_PADDING).min(h);
    let (ow, oh) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if ow == 0 || oh == 0 {
        return Err(ScanError::NoInk);
    }

    // One ink colour: the average of the solid strokes, so blue ink stays blue and paper tint at
    // the stroke edges doesn't show.
    let (mut sum, mut solid) = ([0_u64; 3], 0_u64);
    for (p, &a) in area.pixels().zip(alpha) {
        if a == 255 {
            for (s, &c) in sum.iter_mut().zip(p.0.iter()) {
                *s += u64::from(c);
            }
            solid += 1;
        }
    }
    let colour = if solid == 0 { [0, 0, 0] } else { sum.map(|s| (s / solid).min(255) as u8) };

    let mut out = image::RgbaImage::new(u32::try_from(ow).unwrap_or(1), u32::try_from(oh).unwrap_or(1));
    for (y, row) in out.rows_mut().enumerate() {
        for (x, px) in row.enumerate() {
            let a = alpha.get((y0 + y) * w + x0 + x).copied().unwrap_or(0);
            *px = image::Rgba([colour[0], colour[1], colour[2], a]);
        }
    }
    let mut out = DynamicImage::ImageRgba8(out);
    let pixels = (ow as u64).saturating_mul(oh as u64);
    if pixels > MAX_OUTPUT_PIXELS || ow > WORKING_MAX_SIDE as usize || oh > WORKING_MAX_SIDE as usize {
        let scale = (MAX_OUTPUT_PIXELS as f64 / pixels as f64).sqrt().min(f64::from(WORKING_MAX_SIDE) / ow.max(oh) as f64);
        let nw = ((ow as f64 * scale).floor() as u32).max(1);
        let nh = ((oh as f64 * scale).floor() as u32).max(1);
        out = out.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
    }
    let mut png = Cursor::new(Vec::new());
    out.write_to(&mut png, ImageFormat::Png)?;
    // A PNG larger than a saved signature may be is scaled down by `SignatureImage::from_bytes`.
    Ok(png.into_inner())
}

#[cfg(test)]
mod tests;
