//! Decoded pictures shared by every render on this process, keyed by the media bytes' address and
//! the requested size level and adjustments, so each picture is decoded once per size.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use deckcraft_model::style::PictureAdjust;
use vello_cpu::Pixmap;

const BUDGET: usize = 512 << 20;

/// Largest straight-RGBA picture a decode may produce (bytes, 4 per pixel). Checked from the
/// header before decoding and again before the conversion to RGBA, which the decoder's own
/// allocation limit doesn't cover (a grey image converts to four times its decoded size).
const MAX_RGBA: u64 = 512 << 20;

/// Pictures that failed to decode, remembered so they aren't retried every frame: at most this
/// many, holding at most this many encoded bytes (oldest forgotten first). Larger failed inputs
/// aren't kept at all.
const BAD_MAX: usize = 256;
const BAD_BUDGET: usize = 64 << 20;

struct Entry {
    _bytes: Arc<Vec<u8>>,
    pm: Arc<Pixmap>,
    stamp: u64,
}

#[derive(Default)]
struct Cache {
    map: HashMap<(usize, u32, u64), Entry>,
    bytes: usize,
    clock: u64,
    /// Failed sources by address; each holds its bytes so the address can't be reused while it
    /// is listed. `bad_order` is oldest first; `bad_bytes` their encoded size.
    bad: HashMap<usize, Arc<Vec<u8>>>,
    bad_order: VecDeque<usize>,
    bad_bytes: usize,
    /// GIFs by bytes address and adjustments (`None`: not animated); the bytes stay alive with them.
    /// Decoded animations by (bytes, adjustments), each with the clock of its last use.
    anims: HashMap<(usize, u64), (Arc<Vec<u8>>, Option<Arc<Anim>>, u64)>,
}

/// Longest side, total decoded size and frame count kept for one animated GIF; the largest canvas
/// decoded at all; and the decoded size of all cached GIFs together. Hostile GIFs (millions of
/// tiny frames, a huge canvas) stop at these instead of exhausting memory.
const GIF_SIDE: u32 = 1024;
const GIF_BUDGET: usize = 64 << 20;
const GIF_MAX_FRAMES: usize = 1000;
const GIF_MAX_CANVAS: u64 = 4096 * 4096;
const GIF_MAX_TOTAL_CANVAS_PIXELS: u64 = 256 * 1024 * 1024;
const GIF_CACHE_BUDGET: usize = 192 << 20;

/// The frames of an animated GIF and when each one ends, in seconds from the start of a loop.
struct Anim {
    frames: Vec<Arc<Pixmap>>,
    ends: Vec<f64>,
}

impl Anim {
    /// Decoded bytes held by the frames.
    fn bytes(&self) -> usize {
        self.frames.iter().map(|f| f.width() as usize * f.height() as usize * 4).sum()
    }

    /// The frame showing `t` seconds into the (looping) animation and the seconds until the next.
    fn at(&self, t: f64) -> (usize, f64) {
        let total = self.ends.last().copied().unwrap_or(0.0);
        if total <= 0.0 || !t.is_finite() {
            return (0, 1.0);
        }
        let local = t.max(0.0) % total;
        let i = self.ends.partition_point(|e| *e <= local).min(self.ends.len().saturating_sub(1));
        (i, (self.ends.get(i).copied().unwrap_or(total) - local).max(0.001))
    }
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(g.get_or_insert_with(Cache::default))
}

fn adjust_key(a: &PictureAdjust) -> u64 {
    use std::hash::Hasher;
    // A real hash: an FNV-style xor-multiply mix let (b, c) and (-b, -c) collide (the sign bits
    // cancel), so a picture brightened and contrasted showed the darkened copy's pixels.
    let mut hs = std::hash::DefaultHasher::new();
    let mut mix = |v: u64| hs.write_u64(v);
    mix(a.brightness.to_bits());
    mix(a.contrast.to_bits());
    mix(a.saturation.unwrap_or(1.0).to_bits());
    mix(a.grayscale as u64);
    mix(a.bilevel.map_or(u64::MAX, f64::to_bits));
    mix(a.alpha_inv as u64);
    mix(a.sharpen.to_bits());
    if let Some(c) = a.clear_color {
        mix(((c.r as u64) << 16) | ((c.g as u64) << 8) | c.b as u64 | 1 << 40);
    }
    if let Some((d, l)) = duotone_rgb(a) {
        for c in [d, l] {
            mix(((c.r as u64) << 24) | ((c.g as u64) << 16) | ((c.b as u64) << 8) | c.a as u64 | 1 << 41);
        }
    }
    hs.finish()
}

/// The duotone's dark and light colours. The painter resolves them to plain RGB first (see
/// `paint::resolved`); a colour that is still a scheme reference falls back to black and white.
fn duotone_rgb(a: &PictureAdjust) -> Option<(deckcraft_color::Rgba, deckcraft_color::Rgba)> {
    use deckcraft_model::style::ColorBase;
    let (d, l) = a.duotone.as_ref()?;
    let rgb = |c: &deckcraft_model::style::ColorRef, or: deckcraft_color::Rgba| match &c.base {
        ColorBase::Rgb { rgb } if c.mods.is_empty() => *rgb,
        _ => or,
    };
    Some((rgb(d, deckcraft_color::Rgba::BLACK), rgb(l, deckcraft_color::Rgba::WHITE)))
}

/// Decode encoded bytes (PNG, JPEG, GIF, WebP, BMP, TIFF; WMF and EMF drawn at
/// [`crate::metafile::DEFAULT_SIDE`]) to straight RGBA.
pub fn decode(bytes: &[u8]) -> Option<image::RgbaImage> {
    decode_at(bytes, crate::metafile::DEFAULT_SIDE)
}

/// [`decode`], with a metafile drawn with its longer side `side` px.
fn decode_at(bytes: &[u8], side: u32) -> Option<image::RgbaImage> {
    if bytes.len() > 512 << 20 {
        return None;
    }
    if crate::metafile::is_metafile(bytes) {
        return crate::metafile::rasterize(bytes, side);
    }
    decode_raster(bytes, MAX_RGBA)
}

impl Cache {
    /// Remember that `bytes` doesn't decode, within [`BAD_MAX`] and [`BAD_BUDGET`].
    fn mark_bad(&mut self, addr: usize, bytes: &Arc<Vec<u8>>) {
        if bytes.len() > BAD_BUDGET || self.bad.contains_key(&addr) {
            return;
        }
        while self.bad.len() >= BAD_MAX || self.bad_bytes + bytes.len() > BAD_BUDGET {
            let Some(old) = self.bad_order.pop_front() else { break };
            if let Some(b) = self.bad.remove(&old) {
                self.bad_bytes = self.bad_bytes.saturating_sub(b.len());
            }
        }
        self.bad.insert(addr, bytes.clone());
        self.bad_order.push_back(addr);
        self.bad_bytes += bytes.len();
    }

    fn is_bad(&self, addr: usize, bytes: &Arc<Vec<u8>>) -> bool {
        self.bad.get(&addr).is_some_and(|b| Arc::ptr_eq(b, bytes))
    }
}

/// Whether a `w` × `h` picture fits [`MAX_RGBA`] (`max`: the cap in bytes).
fn fits(w: u32, h: u32, max: u64) -> bool {
    w > 0 && h > 0 && (w as u64) * (h as u64) * 4 <= max
}

/// Decode a raster picture to RGBA, refusing (before decoding, by its header, and again before
/// the RGBA conversion) any whose RGBA would exceed `max` bytes.
fn decode_raster(bytes: &[u8], max: u64) -> Option<image::RgbaImage> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    if let Ok((w, h)) = reader.into_dimensions()
        && !fits(w, h, max)
    {
        return None;
    }
    let img = image::load_from_memory(bytes).ok()?;
    if !fits(img.width(), img.height(), max) {
        return None;
    }
    Some(img.to_rgba8())
}

/// A raster picture's own size in 96-dpi pixels (so 0.75 pt each), read from its header: its
/// pixel size, scaled by the resolution it records (PNG `pHYs`, JPEG JFIF density) when that
/// isn't 96 dpi, as PowerPoint sizes pictures. `None` for metafiles and anything unreadable.
pub fn natural_size(bytes: &[u8]) -> Option<(f64, f64)> {
    if crate::metafile::is_metafile(bytes) {
        return None;
    }
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let (dx, dy) = dpi(bytes).unwrap_or((96.0, 96.0));
    Some((w as f64 * 96.0 / dx, h as f64 * 96.0 / dy))
}

/// The resolution (whole dots per inch across and down) a PNG or JPEG records, when it records
/// one in a plausible range.
pub fn dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    // Whole dots per inch: PNGs store dots per metre, so 96 dpi reads as 95.9866.
    let ok = |d: f64| (d.is_finite() && (10.0..=10000.0).contains(&d)).then_some(d.round());
    let be32 = |b: &[u8], i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    let be16 = |b: &[u8], i: usize| b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut i = 8usize;
        for _ in 0..64 {
            let len = be32(bytes, i)? as usize;
            let kind = bytes.get(i + 4..i + 8)?;
            if kind == b"pHYs" {
                let (x, y, unit) = (be32(bytes, i + 8)?, be32(bytes, i + 12)?, *bytes.get(i + 16)?);
                return if unit == 1 { Some((ok(x as f64 * 0.0254)?, ok(y as f64 * 0.0254)?)) } else { None };
            }
            if kind == b"IDAT" || kind == b"IEND" {
                return None;
            }
            i = i.checked_add(12)?.checked_add(len)?;
        }
        return None;
    }
    if bytes.starts_with(&[0xFF, 0xD8]) && bytes.get(6..11) == Some(b"JFIF\0".as_slice()) {
        let unit = *bytes.get(13)?;
        let (x, y) = (be16(bytes, 14)? as f64, be16(bytes, 16)? as f64);
        return match unit {
            1 => Some((ok(x)?, ok(y)?)),
            2 => Some((ok(x * 2.54)?, ok(y * 2.54)?)),
            _ => None,
        };
    }
    None
}

/// A copy of a PNG with a `pHYs` chunk saying `dpi` dots per inch.
///
/// Anything that is not a PNG, or a resolution the reader would reject, is returned unchanged.
/// A file that already has `pHYs` is left alone.
pub(crate) fn stamp_png_dpi(png: &[u8], dpi: f64) -> Vec<u8> {
    let Some(ppm) = pixels_per_metre(dpi) else { return png.to_vec() };
    let Some(end) = ihdr_end(png) else { return png.to_vec() };
    if has_phys(png) {
        return png.to_vec();
    }
    let mut body = Vec::with_capacity(13);
    body.extend_from_slice(b"pHYs");
    body.extend_from_slice(&ppm.to_be_bytes());
    body.extend_from_slice(&ppm.to_be_bytes());
    body.push(1);
    let crc = png_crc(&body);
    let mut out = Vec::with_capacity(png.len() + 21);
    out.extend_from_slice(png.get(..end).unwrap_or(png));
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.to_be_bytes());
    if let Some(rest) = png.get(end..) {
        out.extend_from_slice(rest);
    }
    out
}

/// Dots per inch as pixels per metre, the unit a PNG `pHYs` chunk stores.
fn pixels_per_metre(dpi: f64) -> Option<u32> {
    if !dpi.is_finite() || !(1.0..=10_000.0).contains(&dpi) {
        return None;
    }
    let ppm = (dpi / 0.0254).round();
    if !ppm.is_finite() || !(1.0..=u32::MAX as f64).contains(&ppm) {
        return None;
    }
    Some(ppm as u32)
}

fn ihdr_end(png: &[u8]) -> Option<usize> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let len = be_u32(png, 8)? as usize;
    if png.get(12..16) != Some(b"IHDR".as_slice()) {
        return None;
    }
    let end = 8usize.checked_add(12)?.checked_add(len)?;
    (end <= png.len()).then_some(end)
}

fn has_phys(png: &[u8]) -> bool {
    let mut i = 8usize;
    for _ in 0..64 {
        let len = match be_u32(png, i) {
            Some(n) => n as usize,
            None => return false,
        };
        let Some(kind) = png.get(i + 4..i + 8) else { return false };
        if kind == b"pHYs" {
            return true;
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return false;
        }
        i = match i.checked_add(12).and_then(|n| n.checked_add(len)) {
            Some(n) if n <= png.len() => n,
            _ => return false,
        };
    }
    false
}

fn be_u32(bytes: &[u8], i: usize) -> Option<u32> {
    let s = bytes.get(i..i + 4)?;
    let s: [u8; 4] = s.try_into().ok()?;
    Some(u32::from_be_bytes(s))
}

fn png_crc(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(c & 1);
            c = (c >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !c
}

/// A premultiplied pixmap of the picture, downsampled so its longer side is near `want` px.
pub fn decode_for(bytes: &Arc<Vec<u8>>, want: f64, adj: &PictureAdjust) -> Option<Arc<Pixmap>> {
    let addr = Arc::as_ptr(bytes) as usize;
    if with(|c| c.is_bad(addr, bytes)) {
        return None;
    }
    // Power-of-two size buckets so zooming doesn't re-decode every frame.
    let bucket = (want.max(16.0).log2().ceil() as u32).min(15);
    let key = (addr, bucket, adjust_key(adj));
    if let Some(pm) = with(|c| {
        c.clock += 1;
        let clock = c.clock;
        c.map.get_mut(&key).map(|e| {
            e.stamp = clock;
            e.pm.clone()
        })
    }) {
        return Some(pm);
    }
    let target = 1u32 << bucket;
    let Some(mut img) = decode_at(bytes, target) else {
        with(|c| c.mark_bad(addr, bytes));
        return None;
    };
    let (w, h) = img.dimensions();
    if w.max(h) > target.max(16) && w.max(h) > 4096.min(target * 2) {
        let k = target as f64 / w.max(h) as f64;
        let (nw, nh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    apply_adjust(&mut img, adj);
    let (w, h) = img.dimensions();
    let pm = pixmap(&img)?;
    with(|c| {
        c.clock += 1;
        let n = w as usize * h as usize * 4;
        c.bytes += n;
        c.map.insert(key, Entry { _bytes: bytes.clone(), pm: pm.clone(), stamp: c.clock });
        while c.bytes > BUDGET && c.map.len() > 1 {
            let Some(oldest) = c.map.iter().min_by_key(|(_, e)| e.stamp).map(|(k, _)| *k) else { break };
            if let Some(e) = c.map.remove(&oldest) {
                c.bytes = c.bytes.saturating_sub(e.pm.width() as usize * e.pm.height() as usize * 4);
            }
        }
    });
    Some(pm)
}

/// Straight RGBA to a premultiplied pixmap.
fn pixmap(img: &image::RgbaImage) -> Option<Arc<Pixmap>> {
    let (w, h) = img.dimensions();
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return None;
    }
    let data: Vec<vello_cpu::color::PremulRgba8> = img
        .pixels()
        .map(|p| {
            let a = p[3] as u16;
            let m = |c: u8| ((c as u16 * a + 127) / 255) as u8;
            vello_cpu::color::PremulRgba8 { r: m(p[0]), g: m(p[1]), b: m(p[2]), a: p[3] }
        })
        .collect();
    Some(Arc::new(Pixmap::from_parts(data, w as u16, h as u16)))
}

/// Decode every frame of an animated GIF (`None` for a still or broken one).
fn decode_anim(bytes: &[u8], adj: &PictureAdjust) -> Option<Anim> {
    use image::{AnimationDecoder, ImageDecoder};
    let mut dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    dec.set_limits(image::Limits::default()).ok()?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 || (w as u64) * (h as u64) > GIF_MAX_CANVAS {
        return None;
    }
    let k = (GIF_SIDE as f64 / w.max(h) as f64).min(1.0);
    let (nw, nh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
    let size = nw as usize * nh as usize * 4;
    let canvas_pixels = (w as u64) * (h as u64);
    let mut total_canvas_pixels: u64 = 0;
    let (mut frames, mut ends, mut t) = (Vec::new(), Vec::new(), 0.0);
    let mut iter = dec.into_frames();
    loop {
        if frames.len() >= GIF_MAX_FRAMES || (frames.len() + 1) * size > GIF_BUDGET {
            log::info!("animated GIF: keeping its first {} frames", frames.len());
            break;
        }
        if total_canvas_pixels.saturating_add(canvas_pixels) > GIF_MAX_TOTAL_CANVAS_PIXELS {
            log::info!("animated GIF: canvas pixel limit reached");
            break;
        }
        let Some(f) = iter.next() else { break };
        let Ok(f) = f else { break };
        total_canvas_pixels = total_canvas_pixels.saturating_add(canvas_pixels);
        let (n, d) = f.delay().numer_denom_ms();
        let ms = if d == 0 { 0.0 } else { n as f64 / d as f64 };
        // Like browsers: frames of 10 ms or less show for 100 ms.
        t += if ms <= 10.0 { 0.1 } else { ms / 1000.0 };
        let mut img = f.into_buffer();
        if img.dimensions() != (nw, nh) {
            img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
        }
        apply_adjust(&mut img, adj);
        frames.push(pixmap(&img)?);
        ends.push(t);
    }
    (frames.len() > 1).then_some(Anim { frames, ends })
}

/// The decoded animation of GIF bytes, once per bytes and adjustments.
fn anim(bytes: &Arc<Vec<u8>>, adj: &PictureAdjust) -> Option<Arc<Anim>> {
    if !bytes.starts_with(b"GIF8") {
        return None;
    }
    let key = (Arc::as_ptr(bytes) as usize, adjust_key(adj));
    let hit = with(|c| {
        c.clock += 1;
        let clock = c.clock;
        c.anims.get_mut(&key).map(|e| {
            e.2 = clock;
            e.1.clone()
        })
    });
    if let Some(a) = hit {
        return a;
    }
    let a = decode_anim(bytes, adj).map(Arc::new);
    with(|c| {
        // Make room by dropping the least recently used animations, not every one of them, so a
        // slide whose animations overflow the cache does not decode all of them again each frame.
        let new = a.as_ref().map_or(0, |a| a.bytes());
        loop {
            let held: usize = c.anims.values().filter_map(|e| e.1.as_ref()).map(|a| a.bytes()).sum();
            if held + new <= GIF_CACHE_BUDGET && c.anims.len() < 64 {
                break;
            }
            let Some(oldest) = c.anims.iter().min_by_key(|(_, e)| e.2).map(|(k, _)| *k) else { break };
            c.anims.remove(&oldest);
        }
        c.clock += 1;
        let clock = c.clock;
        c.anims.insert(key, (bytes.clone(), a.clone(), clock));
    });
    a
}

/// For an animated GIF: the frame showing `t` seconds into its (looping) play and the seconds until
/// the next frame. `None` for anything else.
pub fn gif_frame(bytes: &Arc<Vec<u8>>, adj: &PictureAdjust, t: f64) -> Option<(usize, f64)> {
    anim(bytes, adj).map(|a| a.at(t))
}

/// The frame of an animated GIF at `t` seconds.
pub(crate) fn gif_pixmap(bytes: &Arc<Vec<u8>>, adj: &PictureAdjust, t: f64) -> Option<Arc<Pixmap>> {
    let a = anim(bytes, adj)?;
    a.frames.get(a.at(t).0).cloned()
}

fn apply_adjust(img: &mut image::RgbaImage, a: &PictureAdjust) {
    let sat = a.saturation.unwrap_or(1.0);
    if a.brightness == 0.0
        && a.contrast == 0.0
        && sat == 1.0
        && !a.grayscale
        && a.clear_color.is_none()
        && a.duotone.is_none()
        && a.bilevel.is_none()
        && !a.alpha_inv
    {
        return;
    }
    let duo = duotone_rgb(a);
    let b = a.brightness.clamp(-1.0, 1.0) * 255.0;
    let c = a.contrast.clamp(-1.0, 1.0);
    let cf = if c >= 0.0 { 1.0 / (1.0 - c * 0.99) } else { 1.0 + c };
    for p in img.pixels_mut() {
        if let Some(cc) = a.clear_color
            && (p[0] as i32 - cc.r as i32).abs() < 8
            && (p[1] as i32 - cc.g as i32).abs() < 8
            && (p[2] as i32 - cc.b as i32).abs() < 8
        {
            p[3] = 0;
            continue;
        }
        let mut rgb = [p[0] as f64, p[1] as f64, p[2] as f64];
        let y = luma(rgb);
        let s = if a.grayscale { 0.0 } else { sat };
        for v in &mut rgb {
            *v = y + (*v - y) * s;
            // Measured on PowerPoint's export of grey ramps: contrast scales about mid-grey by
            // `cf`, and brightness adds b · 255 scaled by (1 + cf) / 2 (half of it is applied
            // before the contrast stretch, half after): +40% bright, +40% contrast maps 0 to 54.
            *v = (*v - 127.5) * cf + 127.5 + b * (1.0 + cf) / 2.0;
        }
        // Duotone: the pixel's luminance picks a colour between the dark and the light one.
        if let Some((d, l)) = duo {
            let t = (luma(rgb) / 255.0).clamp(0.0, 1.0);
            rgb =
                [d.r as f64 + (l.r as f64 - d.r as f64) * t, d.g as f64 + (l.g as f64 - d.g as f64) * t, d.b as f64 + (l.b as f64 - d.b as f64) * t];
        }
        // Black and white: white where the luminance reaches the threshold.
        if let Some(t) = a.bilevel {
            let v = if luma(rgb) >= t * 255.0 { 255.0 } else { 0.0 };
            rgb = [v; 3];
        }
        for (i, v) in rgb.iter().enumerate() {
            if let Some(ch) = p.0.get_mut(i) {
                *ch = v.round().clamp(0.0, 255.0) as u8;
            }
        }
        if a.alpha_inv {
            p[3] = 255 - p[3];
        }
    }
}

/// Luminance as PowerPoint weighs it for grayscale, black-and-white and duotone pictures
/// (measured: Rec. 709 weights on the stored values; pure red greys to 54, green to 182).
fn luma(rgb: [f64; 3]) -> f64 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn grey_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::GrayImage::from_pixel(w, h, image::Luma([90]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).expect("encode");
        out.into_inner()
    }

    /// A two-frame GIF whose first pixel is `n`, so each one has its own bytes.
    fn gif(n: u8) -> Arc<Vec<u8>> {
        use image::codecs::gif::GifEncoder;
        let frame = |c: u8| image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba([n, c, 0, 255])));
        let mut out = Vec::new();
        GifEncoder::new(&mut out).encode_frames([frame(0), frame(255)]).expect("encode");
        Arc::new(out)
    }

    #[test]
    fn a_full_animation_cache_drops_the_oldest_entry_not_all_of_them() {
        let adj = PictureAdjust::default();
        let gifs: Vec<_> = (0..70).map(gif).collect();
        for g in &gifs {
            assert!(anim(g, &adj).is_some());
        }
        let (len, newest) = with(|c| (c.anims.len(), c.anims.contains_key(&(Arc::as_ptr(&gifs[69]) as usize, adjust_key(&adj)))));
        assert!(newest && len >= 32, "{len} cached");
    }

    #[test]
    fn rgba_conversion_is_refused_past_the_cap() {
        let png = grey_png(100, 50);
        let rgba = 100 * 50 * 4;
        assert!(decode_raster(&png, rgba - 1).is_none(), "grey decodes small, its RGBA does not fit");
        let img = decode_raster(&png, rgba).expect("fits exactly");
        assert_eq!(img.dimensions(), (100, 50));
        // A picture whose header claims a huge size is refused.
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        assert!(decode_raster(&huge, MAX_RGBA).is_none());
        assert!(!fits(60_000, 60_000, MAX_RGBA) && fits(8192, 8192, MAX_RGBA));
    }

    #[test]
    fn failed_sources_are_bounded_and_released() {
        let mut c = Cache::default();
        let srcs: Vec<Arc<Vec<u8>>> = (0..BAD_MAX + 50).map(|i| Arc::new(vec![i as u8; 1000])).collect();
        for s in &srcs {
            c.mark_bad(Arc::as_ptr(s) as usize, s);
        }
        assert_eq!(c.bad.len(), BAD_MAX);
        assert_eq!(c.bad_bytes, BAD_MAX * 1000);
        // The oldest were forgotten and their bytes released; the newest are still known.
        assert_eq!(Arc::strong_count(&srcs[0]), 1);
        assert!(!c.is_bad(Arc::as_ptr(&srcs[0]) as usize, &srcs[0]));
        let last = srcs.last().expect("source");
        assert!(c.is_bad(Arc::as_ptr(last) as usize, last));
        // The byte budget holds too, and an input larger than it isn't kept at all.
        let big = Arc::new(vec![0u8; BAD_BUDGET / 2 + 1]);
        c.mark_bad(Arc::as_ptr(&big) as usize, &big);
        let big2 = Arc::new(vec![1u8; BAD_BUDGET / 2 + 1]);
        c.mark_bad(Arc::as_ptr(&big2) as usize, &big2);
        assert!(c.bad_bytes <= BAD_BUDGET);
        assert_eq!(Arc::strong_count(&big), 1);
        let huge = Arc::new(vec![0u8; BAD_BUDGET + 1]);
        c.mark_bad(Arc::as_ptr(&huge) as usize, &huge);
        assert_eq!(Arc::strong_count(&huge), 1);
    }

    #[test]
    fn invalid_pictures_fail_and_valid_ones_still_decode_and_cache() {
        let bad = Arc::new(b"not a picture at all".to_vec());
        assert!(decode_for(&bad, 64.0, &PictureAdjust::default()).is_none());
        assert!(decode_for(&bad, 64.0, &PictureAdjust::default()).is_none());
        let good = Arc::new(grey_png(20, 10));
        let a = decode_for(&good, 64.0, &PictureAdjust::default()).expect("decodes");
        let b = decode_for(&good, 64.0, &PictureAdjust::default()).expect("cached");
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!((a.width(), a.height()), (20, 10));
    }

    #[test]
    fn duotone_maps_luminance_between_its_two_colours() {
        use deckcraft_color::Rgba;
        use deckcraft_model::style::ColorRef;
        let mut img = image::RgbaImage::from_fn(3, 1, |x, _| image::Rgba([[0, 0, 0, 255], [128, 128, 128, 200], [255, 255, 255, 255]][x as usize]));
        let adj =
            PictureAdjust { duotone: Some((ColorRef::rgb(Rgba::rgb(100, 40, 0)), ColorRef::rgb(Rgba::rgb(250, 240, 200)))), ..Default::default() };
        apply_adjust(&mut img, &adj);
        assert_eq!(img.get_pixel(0, 0).0, [100, 40, 0, 255], "black takes the dark colour");
        assert_eq!(img.get_pixel(2, 0).0, [250, 240, 200, 255], "white takes the light colour");
        let mid = img.get_pixel(1, 0).0;
        assert!((mid[0] as i32 - 175).abs() <= 1 && (mid[1] as i32 - 140).abs() <= 1 && (mid[2] as i32 - 100).abs() <= 1, "{mid:?}");
        assert_eq!(mid[3], 200, "alpha is kept");
        // Different duotones are cached separately.
        let other = PictureAdjust { duotone: Some((ColorRef::rgb(Rgba::rgb(0, 0, 0)), ColorRef::rgb(Rgba::rgb(255, 0, 0)))), ..Default::default() };
        assert_ne!(adjust_key(&adj), adjust_key(&other));
        assert_ne!(adjust_key(&adj), adjust_key(&PictureAdjust::default()));
    }

    #[test]
    fn brightness_and_contrast_map_greys_like_powerpoint() {
        // Grey levels PowerPoint's export gives for inputs 0, 32, 64, 96 (and 128, 160).
        let run = |b: f64, c: f64, input: &[u8]| -> Vec<u8> {
            let mut img = image::RgbaImage::from_fn(input.len() as u32, 1, |x, _| {
                let v = input[x as usize];
                image::Rgba([v, v, v, 255])
            });
            apply_adjust(&mut img, &PictureAdjust { brightness: b, contrast: c, ..Default::default() });
            img.pixels().map(|p| p[0]).collect()
        };
        let near = |got: Vec<u8>, want: &[u8]| got.iter().zip(want).all(|(g, w)| (*g as i32 - *w as i32).abs() <= 5);
        assert!(near(run(0.4, 0.4, &[0, 32, 64, 96]), &[54, 105, 157, 210]), "{:?}", run(0.4, 0.4, &[0, 32, 64, 96]));
        assert!(near(run(0.4, -0.4, &[0, 32, 64, 96]), &[133, 152, 171, 190]));
        assert!(near(run(-0.4, 0.4, &[128, 160, 192]), &[0, 45, 97]));
        assert!(near(run(0.0, -0.8, &[0, 128, 255]), &[102, 127, 152]));
    }

    #[test]
    fn grayscale_black_and_white_and_inverted_alpha_like_powerpoint() {
        let swatches = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255], [200, 100, 50], [60, 160, 110], [120, 120, 130]];
        let run = |adj: PictureAdjust| -> Vec<[u8; 4]> {
            let mut img = image::RgbaImage::from_fn(swatches.len() as u32, 1, |x, _| {
                let c = swatches[x as usize];
                image::Rgba([c[0], c[1], c[2], 200])
            });
            apply_adjust(&mut img, &adj);
            img.pixels().map(|p| p.0).collect()
        };
        // PowerPoint's export of these swatches in grayscale: 54, 182, 18, 117, 135, 120.
        let g = run(PictureAdjust { grayscale: true, ..Default::default() });
        for (got, want) in g.iter().zip([54u8, 182, 18, 117, 135, 120]) {
            assert!((got[0] as i32 - want as i32).abs() <= 1 && got[0] == got[1] && got[1] == got[2], "{got:?} vs {want}");
        }
        // Black and white at 50%: only green and the light green reach mid-grey.
        let bw: Vec<u8> = run(PictureAdjust { bilevel: Some(0.5), ..Default::default() }).iter().map(|p| p[0]).collect();
        assert_eq!(bw, [0, 255, 0, 0, 255, 0]);
        // Inverted alpha: 200 becomes 55.
        assert!(run(PictureAdjust { alpha_inv: true, ..Default::default() }).iter().all(|p| p[3] == 55));
    }

    #[test]
    fn opposite_brightness_and_contrast_are_cached_apart() {
        let a = PictureAdjust { brightness: 0.4, contrast: 0.4, ..Default::default() };
        let b = PictureAdjust { brightness: -0.4, contrast: -0.4, ..Default::default() };
        assert_ne!(adjust_key(&a), adjust_key(&b));
        let c = PictureAdjust { brightness: 0.4, contrast: -0.4, ..Default::default() };
        let d = PictureAdjust { brightness: -0.4, contrast: 0.4, ..Default::default() };
        assert_ne!(adjust_key(&c), adjust_key(&d));
        assert_eq!(adjust_key(&a), adjust_key(&a.clone()));
    }

    #[test]
    fn a_gif_of_many_tiny_frames_keeps_only_the_frame_cap() {
        use image::{Delay, Frame, Rgba, RgbaImage};
        let frames = (0..GIF_MAX_FRAMES + 200).map(|i| {
            let c = if i % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
            Frame::from_parts(RgbaImage::from_pixel(1, 1, Rgba(c)), 0, 0, Delay::from_numer_denom_ms(20, 1))
        });
        let mut out = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut out).encode_frames(frames).expect("gif");
        let a = decode_anim(&out, &PictureAdjust::default()).expect("animated");
        assert_eq!(a.frames.len(), GIF_MAX_FRAMES);
        assert_eq!(a.ends.len(), GIF_MAX_FRAMES);
    }
}
