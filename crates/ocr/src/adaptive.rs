//! Adaptive (MRC) compression of scans (Acrobat parity `ocr.adaptive-compression`).
//!
//! Heuristic dual-guard Otsu scan binarization converting near-bilevel 8-bit text scans
//! (grayscale or RGB) into 1-bit monochrome images compressed with CCITT Group 4 or Flate.
//!
//! Prepress and content-loss guards:
//! - Chroma guard: Rejects colored content, stamps, and highlighters.
//! - Bimodal histogram guard: Requires clear dark ink and bright paper peaks; rejects flat or unimodal distributions.
//! - Scale-invariant smooth-grey flux guard: Rejects localized smooth grey regions (emblems, binder clips, seals, shaded banners) whose local mean luminance shifts by >40 levels under binarization, regardless of scan DPI.
//! - Content-loss guard: Protects faint pencil/pen strokes in `[threshold..215)` from bleaching out to background.
//! - Prepress fail-safe: Strictly preserves images with soft masks (/SMask, /SMaskInData),
//!   transparency masks (/Mask), or prepress production links (/OPI).

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::OcrError;

/// Maximum allowable chroma difference for dark/ink content (lum < 225) without counting as fringe.
const CHROMA_TOLERANCE: u8 = 15;

/// Absolute chroma ceiling for bright pixels (lum >= 225) without counting as fringe.
const BRIGHT_CHROMA_LIMIT: u8 = 30;

/// Any single pixel above this chroma difference is treated as intentional color (red stamp,
/// blue signature, highlighter) and immediately aborts binarization.
const VIVID_CHROMA_LIMIT: u8 = 80;

/// Maximum fraction of pixels allowed to carry mild JPEG chroma-subsampling fringe (0.8%).
const MAX_CHROMA_FRINGE_RATIO: f64 = 0.008;

/// Maximum fraction of midtone pixels (lum 66..=214) from anti-aliased text edges and JPEG ringing (6.0%).
const MAX_MIDTONE_RATIO: f64 = 0.06;

/// Minimum fraction of dark ink + bright paper pixels (94.0%).
const MIN_EXTREME_RATIO: f64 = 0.94;

/// Minimum fraction of dark ink pixels (0.5%).
const MIN_DARK_RATIO: f64 = 0.005;

/// Minimum fraction of bright paper background pixels (50.0%).
const MIN_BRIGHT_RATIO: f64 = 0.50;

/// Maximum fraction of pixels in `[threshold..215)` that may be bleached to white (2.0%).
const MAX_CONTENT_LOSS_RATIO: f64 = 0.02;

/// Minimum scale-invariant tile dimension in pixels (matches ~8x8 blocks at 72 DPI for ~100 DPI scans).
const MIN_TILE_SIZE: usize = 8;

/// Maximum scale-invariant tile dimension in pixels.
const MAX_TILE_SIZE: usize = 64;

/// Lower mean luminance bound for identifying continuous smooth grey tiles.
const SMOOTH_GREY_MIN_MEAN: f64 = 60.0;

/// Upper mean luminance bound for identifying continuous smooth grey tiles.
const SMOOTH_GREY_MAX_MEAN: f64 = 195.0;

/// Maximum local luminance shift `|mu_after - mu_before|` before a smooth grey tile counts as distorted.
// Why: matches oracle detection of greyscale regions that collapse to solid black or wash out under thresholding.
const MAX_TILE_FLUX_SHIFT: f64 = 40.0;

/// Maximum white paper fraction (`>= 215`) allowed for a tile to qualify as continuous grey tone.
const MAX_TILE_PAPER_RATIO: f64 = 0.20;

/// Minimum midtone fraction (`66..=214`) required so bold dark-ink strokes (`<= 65`) are not mistaken for grey shading.
const MIN_TILE_MIDTONE_RATIO: f64 = 0.30;

/// Maximum average adjacent-pixel gradient allowed for a smooth grey tile (separates smooth emblems from halftoned screens).
const MAX_TILE_SMOOTH_GRADIENT: f64 = 20.0;

/// Maximum number of 8-connected distorted smooth-grey tiles tolerated before rejecting binarization.
const MAX_CLUSTERED_SMOOTH_GREY_TILES: usize = 3;

/// Options for adaptive scan compression.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveCompressOptions {
    /// Enable binarization of near-bilevel scans into 1-bit monochrome images.
    pub binarize_scans: bool,
    /// Maximum number of pixels to process in a single image (default 80_000_000).
    pub max_pixels: u64,
}

impl Default for AdaptiveCompressOptions {
    fn default() -> Self {
        Self { binarize_scans: true, max_pixels: 80_000_000 }
    }
}

/// Execution summary returned by [`adaptive_compress_scans`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdaptiveCompressReport {
    /// Number of near-bilevel scan images successfully binarized to 1-bit monochrome.
    pub compressed: usize,
    /// Number of scan candidates skipped due to guards (color, photos, or prepress).
    pub skipped: usize,
}

/// Computes Otsu's between-class variance threshold over a 256-bin luminance histogram,
/// returning `None` when the distribution is degenerate (single-level or empty).
pub fn compute_otsu(hist: &[u32; 256], total: usize) -> Option<u8> {
    if total == 0 {
        return None;
    }
    let sum: f64 = hist.iter().enumerate().map(|(i, &c)| (i as f64) * (c as f64)).sum();
    let mut wb = 0.0;
    let mut sb = 0.0;
    let mut max_variance = 0.0;
    let mut best_t = None;
    let total_f = total as f64;

    for (t, &count) in hist.iter().enumerate() {
        wb += count as f64;
        if wb == 0.0 {
            continue;
        }
        let wf = total_f - wb;
        if wf == 0.0 {
            break;
        }
        sb += (t as f64) * (count as f64);
        let mb = sb / wb;
        let mf = (sum - sb) / wf;
        let variance = wb * wf * (mb - mf) * (mb - mf);
        if variance > max_variance {
            max_variance = variance;
            best_t = Some(t as u8);
        }
    }

    best_t
}

#[inline]
fn rgb_lum(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8
}

#[inline]
fn sample_lum(row: &[u8], x: usize, n: usize) -> Option<u8> {
    if n == 1 {
        row.get(x).copied()
    } else {
        let base = x.checked_mul(3)?;
        let r = *row.get(base)?;
        let g = *row.get(base + 1)?;
        let b = *row.get(base + 2)?;
        Some(rgb_lum(r, g, b))
    }
}

/// Detects localized smooth grey regions (emblems, seals, binder clips, shaded banners) whose local
/// block luminance collapses by >40 levels under binarization across a scale-invariant tile grid.
fn has_smooth_grey_flux_collapse(samples: &[u8], row_len: usize, w: usize, h: usize, n: usize, threshold: u8) -> bool {
    // Scale tile dimension to image resolution (~2.8 mm / ~8x8 px at 72 DPI) so 100-DPI small objects
    // are not diluted inside oversized fixed tiles.
    let tile_size = (w.min(h) / 75).clamp(MIN_TILE_SIZE, MAX_TILE_SIZE);
    let th = h / tile_size;
    let tw = w / tile_size;
    if th == 0 || tw == 0 {
        return false;
    }
    let Some(grid_len) = th.checked_mul(tw) else { return false };
    let Some(row_stride_px) = w.checked_mul(n) else { return false };

    let area = (tile_size * tile_size) as f64;
    let grad_h_denom = (tile_size * tile_size.saturating_sub(1).max(1)) as f64;
    let grad_v_denom = (tile_size.saturating_sub(1).max(1) * tile_size) as f64;

    let mut smooth_bad = vec![false; grid_len];
    let mut prev_row = [0u8; MAX_TILE_SIZE];

    for ty in 0..th {
        let y0 = ty * tile_size;
        for tx in 0..tw {
            let x0 = tx * tile_size;
            let mut sum_lum = 0u32;
            let mut bright_after_t = 0u32;
            let mut paper_count = 0u32;
            let mut mid_count = 0u32;
            let mut sum_gx = 0u32;
            let mut sum_gy = 0u32;

            for dy in 0..tile_size {
                let Some(y_off) = (y0 + dy).checked_mul(row_len) else { return false };
                let Some(row) = samples.get(y_off..).and_then(|s| s.get(..row_stride_px)) else {
                    return false;
                };
                let mut prev_x = 0u8;
                for dx in 0..tile_size {
                    let Some(lum) = sample_lum(row, x0 + dx, n) else { return false };
                    sum_lum += u32::from(lum);
                    if lum >= threshold {
                        bright_after_t += 1;
                    }
                    if lum >= 215 {
                        paper_count += 1;
                    } else if lum >= 66 {
                        mid_count += 1;
                    }
                    if dx > 0 {
                        sum_gx += u32::from(lum.abs_diff(prev_x));
                    }
                    if dy > 0
                        && let Some(&up) = prev_row.get(dx)
                    {
                        sum_gy += u32::from(lum.abs_diff(up));
                    }
                    prev_x = lum;
                    if let Some(slot) = prev_row.get_mut(dx) {
                        *slot = lum;
                    }
                }
            }

            let mu_b = f64::from(sum_lum) / area;
            let mu_a = (f64::from(bright_after_t) * 255.0) / area;
            let bright_frac = f64::from(paper_count) / area;
            let mid_frac = f64::from(mid_count) / area;
            let gx = f64::from(sum_gx) / grad_h_denom;
            let gy = f64::from(sum_gy) / grad_v_denom;
            let grad = 0.5 * (gx + gy);

            if (SMOOTH_GREY_MIN_MEAN..=SMOOTH_GREY_MAX_MEAN).contains(&mu_b)
                && (mu_a - mu_b).abs() > MAX_TILE_FLUX_SHIFT
                && bright_frac < MAX_TILE_PAPER_RATIO
                && mid_frac >= MIN_TILE_MIDTONE_RATIO
                && grad < MAX_TILE_SMOOTH_GRADIENT
                && let Some(cell) = smooth_bad.get_mut(ty * tw + tx)
            {
                *cell = true;
            }
        }
    }

    let mut clustered = 0usize;
    for ty in 0..th {
        for tx in 0..tw {
            if smooth_bad.get(ty * tw + tx) != Some(&true) {
                continue;
            }
            let mut has_neighbor = false;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if dy == 0 && dx == 0 {
                        continue;
                    }
                    if let (Some(ny), Some(nx)) = (ty.checked_add_signed(dy), tx.checked_add_signed(dx))
                        && ny < th
                        && nx < tw
                        && smooth_bad.get(ny * tw + nx) == Some(&true)
                    {
                        has_neighbor = true;
                        break;
                    }
                }
                if has_neighbor {
                    break;
                }
            }
            if has_neighbor {
                clustered += 1;
                if clustered > MAX_CLUSTERED_SMOOTH_GREY_TILES {
                    return true;
                }
            }
        }
    }

    false
}

/// Attempts to binarize an 8-bit image (1 channel grayscale or 3 channels RGB) into 1-bit samples
/// using Heuristic Dual-Guard (Chroma Guard + Bimodal Histogram Guard), scale-invariant smooth-grey
/// flux guard, and Otsu thresholding.
pub fn try_binarize_scan(samples: &[u8], row_len: usize, w: usize, h: usize, n: usize) -> Option<Vec<u8>> {
    let total = w.checked_mul(h)?;
    if total == 0 || (n != 1 && n != 3) {
        return None;
    }

    let stride = w.div_ceil(8);
    let out_len = stride.checked_mul(h)?;
    if samples.len() < row_len.checked_mul(h)? {
        return None;
    }

    // Pass 1: Chroma & Bimodal Histogram Guard
    let mut hist = [0u32; 256];
    let mut dark = 0usize;
    let mut bright = 0usize;
    let mut mid = 0usize;
    let mut chroma_fringe = 0usize;

    for row in samples.chunks(row_len).take(h) {
        let row_pixels = row.get(..w.checked_mul(n)?)?;
        for chunk in row_pixels.chunks_exact(n) {
            let lum = if n == 1 {
                chunk[0]
            } else {
                let (r, g, b) = (chunk[0], chunk[1], chunk[2]);
                let lum = rgb_lum(r, g, b);
                let max_c = r.max(g).max(b);
                let min_c = r.min(g).min(b);
                let chroma = max_c - min_c;
                if chroma > VIVID_CHROMA_LIMIT {
                    // Vivid color detail (stamp, blue ink, photo): immediately reject.
                    return None;
                }
                if chroma > BRIGHT_CHROMA_LIMIT || (lum < 225 && chroma > CHROMA_TOLERANCE) {
                    chroma_fringe += 1;
                    // Early exit if mild chroma fringe exceeds 0.8% (chroma_fringe / total > 0.008).
                    if (chroma_fringe as u64).saturating_mul(125) > total as u64 {
                        return None;
                    }
                }
                lum
            };

            if let Some(entry) = hist.get_mut(usize::from(lum)) {
                *entry += 1;
            }
            match lum {
                0..=65 => dark += 1,
                215..=255 => bright += 1,
                66..=214 => {
                    mid += 1;
                    // Early exit if midtones exceed 6.0% (mid / total > 0.06)
                    if (mid as u64).saturating_mul(100) > (total as u64).saturating_mul(6) {
                        return None;
                    }
                }
            }
        }
    }

    let extreme_ratio = (dark + bright) as f64 / total as f64;
    let midtone_ratio = mid as f64 / total as f64;
    let dark_ratio = dark as f64 / total as f64;
    let bright_ratio = bright as f64 / total as f64;
    let chroma_fringe_ratio = chroma_fringe as f64 / total as f64;

    // Scope: Near-bilevel scans (allowing anti-aliased text edges and mild JPEG chroma ringing).
    if chroma_fringe_ratio > MAX_CHROMA_FRINGE_RATIO
        || extreme_ratio < MIN_EXTREME_RATIO
        || midtone_ratio > MAX_MIDTONE_RATIO
        || dark_ratio < MIN_DARK_RATIO
        || bright_ratio < MIN_BRIGHT_RATIO
    {
        return None;
    }

    // Otsu thresholding & Content-Loss Guard
    let raw = compute_otsu(&hist, total)?;
    let threshold = raw.clamp(100, 180);

    // Guard against bleaching faint text strokes (lum in [threshold..215)).
    let bleached_ink: u32 = hist.get(usize::from(threshold)..215).map_or(0, |slice| slice.iter().sum());
    if (bleached_ink as f64 / total as f64) > MAX_CONTENT_LOSS_RATIO {
        return None;
    }

    // Why: localized smooth greyscale emblems, clips, and shaded banners can stay below 6% global midtones
    // yet collapse by >40 levels locally; a scale-invariant smooth-grey flux check catches them across DPIs.
    if has_smooth_grey_flux_collapse(samples, row_len, w, h, n, threshold) {
        return None;
    }

    // Pack 1-bit (MSB first, 1 = white, 0 = black)
    let mut out = vec![0u8; out_len];
    for (y, row) in samples.chunks(row_len).take(h).enumerate() {
        let row_pixels = row.get(..w.checked_mul(n)?)?;
        let out_row = out.get_mut(y * stride..(y + 1) * stride)?;
        for (x, chunk) in row_pixels.chunks_exact(n).enumerate() {
            let lum = if n == 1 { chunk[0] } else { rgb_lum(chunk[0], chunk[1], chunk[2]) };
            if lum >= threshold {
                *out_row.get_mut(x / 8)? |= 0x80 >> (x % 8);
            }
        }
    }

    Some(out)
}

/// Whether image dictionary `d` carries a non-default `/Decode` array for an `n`-component
/// DeviceGray (`n = 1`) or DeviceRGB (`n = 3`) image.
fn has_non_default_decode(doc: &Document, d: &Dict, n: usize) -> bool {
    let Some(raw) = d.get(b"Decode") else { return false };
    let resolved = doc.resolve(raw);
    let Some(arr) = resolved.as_array() else { return true };
    if arr.len() != n * 2 {
        return true;
    }
    for pair in arr.as_chunks::<2>().0 {
        let Some(d0) = doc.resolve(&pair[0]).as_f64() else { return true };
        let Some(d1) = doc.resolve(&pair[1]).as_f64() else { return true };
        if (d0 - 0.0).abs() > 1e-6 || (d1 - 1.0).abs() > 1e-6 {
            return true;
        }
    }
    false
}

/// Checks if an image stream dictionary is a potential scan candidate for adaptive binarization.
fn scan_candidate_shape(doc: &Document, d: &Dict) -> Option<(usize, usize, usize)> {
    if d.name(b"Subtype") != Some(b"Image") {
        return None;
    }
    // Fail-safe prepress guards: reject soft masks, OPI dictionaries, or explicit masks
    if d.contains(b"SMask") || d.contains(b"SMaskInData") || d.contains(b"OPI") || d.contains(b"Mask") {
        return None;
    }
    let bpc = d.int(b"BitsPerComponent").unwrap_or(8);
    if bpc != 8 {
        return None;
    }
    let cs = d.get(b"ColorSpace")?;
    let resolved = doc.resolve(cs);
    let cs_name = match &*resolved {
        Object::Name(n) => n.as_slice(),
        _ => return None,
    };
    let n = match cs_name {
        b"DeviceGray" => 1,
        b"DeviceRGB" => 3,
        _ => return None,
    };
    if has_non_default_decode(doc, d, n) {
        return None;
    }
    let w = usize::try_from(d.int(b"Width")?).ok()?;
    let h = usize::try_from(d.int(b"Height")?).ok()?;
    Some((w, h, n))
}

/// Largest OCR JPEG, in pixels, that the decoder will open.
const MAX_OCR_JPEG_PIXELS: usize = 1 << 26;

/// Decodes JPEG data into uncompressed pixel samples.
fn decode_jpeg_samples(data: &[u8], w: usize, h: usize, n: usize) -> Option<Vec<u8>> {
    if w.checked_mul(h)? > MAX_OCR_JPEG_PIXELS {
        return None;
    }
    let options = zune_jpeg::zune_core::options::DecoderOptions::default()
        .set_strict_mode(false)
        .set_max_width(usize::from(u16::MAX))
        .set_max_height(usize::from(u16::MAX));
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(zune_jpeg::zune_core::bytestream::ZCursor::new(data), options);
    dec.decode_headers().ok()?;
    if dec.dimensions()? != (w, h) {
        return None;
    }
    let colorspace = match n {
        1 => zune_jpeg::zune_core::colorspace::ColorSpace::Luma,
        3 => zune_jpeg::zune_core::colorspace::ColorSpace::RGB,
        _ => return None,
    };
    dec.set_options(options.jpeg_set_out_colorspace(colorspace));
    let px = dec.decode().ok()?;
    let expected = w.checked_mul(h)?.checked_mul(n)?;
    if px.len() == expected { Some(px) } else { None }
}

/// Scans all image objects in `doc` and applies adaptive binarization to near-bilevel scans.
pub fn adaptive_compress_scans(doc: &mut Document, options: AdaptiveCompressOptions) -> Result<AdaptiveCompressReport, OcrError> {
    if !options.binarize_scans {
        return Ok(AdaptiveCompressReport::default());
    }

    let mut report = AdaptiveCompressReport::default();
    let soft_mask_refs: HashSet<u32> = doc
        .scan_objects()
        .filter_map(|(_, obj)| match &*obj {
            Object::Stream(s) => s.dict.reference(b"SMask").map(|r| r.num),
            _ => None,
        })
        .collect();

    let candidates: Vec<ObjRef> = doc
        .scan_objects()
        .filter_map(|(r, obj)| match &*obj {
            Object::Stream(s) if !soft_mask_refs.contains(&r.num) => scan_candidate_shape(doc, &s.dict).map(|_| r),
            _ => None,
        })
        .collect();

    let mut seen = HashSet::new();

    for r in candidates {
        if !seen.insert(r.num) {
            continue;
        }

        let Object::Stream(s) = &*doc.get(r) else {
            continue;
        };

        let Some((w, h, n)) = scan_candidate_shape(doc, &s.dict) else {
            continue;
        };

        match (w as u64).checked_mul(h as u64) {
            Some(p) if p <= options.max_pixels => {}
            _ => {
                report.skipped += 1;
                continue;
            }
        };

        let row_len = match w.checked_mul(n) {
            Some(rl) => rl,
            None => {
                report.skipped += 1;
                continue;
            }
        };

        let is_jpeg = doc.resolve(s.dict.get(b"Filter").unwrap_or(&Object::Null)).as_name() == Some(b"DCTDecode");
        let samples = if is_jpeg {
            decode_jpeg_samples(&s.raw, w, h, n)
        } else {
            pdfcraft_cos::ImageBudget::new(doc).decode(doc, s, pdfcraft_cos::MAX_DECODED).ok()
        };

        let Some(samples) = samples else {
            report.skipped += 1;
            continue;
        };

        let Some(packed) = try_binarize_scan(&samples, row_len, w, h, n) else {
            report.skipped += 1;
            continue;
        };

        let g4_opt = pdfcraft_filters::encode_g4(&packed, w as u32, h as u32).ok();
        let flate = pdfcraft_filters::encode_flate(&packed);

        let mut new_dict = s.dict.clone();
        new_dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        new_dict.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        new_dict.remove(b"Decode");
        new_dict.remove(b"DecodeParms");

        let final_data = if let Some(g4) = g4_opt {
            if g4.len() < flate.len() {
                let mut parms = Dict::new();
                parms.set(b"K".to_vec(), Object::Int(-1));
                parms.set(b"Columns".to_vec(), Object::Int(w as i64));
                parms.set(b"Rows".to_vec(), Object::Int(h as i64));
                new_dict.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
                new_dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
                g4
            } else {
                new_dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
                flate
            }
        } else {
            new_dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
            flate
        };

        new_dict.set(b"Length".to_vec(), Object::Int(final_data.len() as i64));
        doc.set(r, Object::Stream(Stream::from_raw(new_dict, final_data)));
        report.compressed += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_otsu_direct_unit_tests() {
        // Degenerate single-level distributions must return None
        let mut degenerate_dark = [0u32; 256];
        degenerate_dark[25] = 1000;
        assert_eq!(compute_otsu(&degenerate_dark, 1000), None);

        let mut degenerate_bright = [0u32; 256];
        degenerate_bright[245] = 1000;
        assert_eq!(compute_otsu(&degenerate_bright, 1000), None);

        // Classic bimodal distribution (500 ink pixels at 20, 500 paper pixels at 220)
        let mut bimodal = [0u32; 256];
        bimodal[20] = 500;
        bimodal[220] = 500;
        let t = compute_otsu(&bimodal, 1000);
        assert!(t.is_some());
        let threshold = t.unwrap();
        assert!((20..220).contains(&threshold));
    }

    #[test]
    fn binarize_refuses_degenerate_histogram() {
        let (w, h) = (100usize, 100usize);
        let samples = vec![25u8; w * h];
        assert_eq!(try_binarize_scan(&samples, w, w, h, 1), None);
    }

    #[test]
    fn binarize_converts_pristine_bilevel_scan() {
        let (w, h) = (100usize, 100usize);
        let mut samples = vec![245u8; w * h];
        for s in samples.iter_mut().take(500) {
            *s = 25;
        }
        let res = try_binarize_scan(&samples, w, w, h, 1);
        assert!(res.is_some());
        let packed = res.unwrap();
        assert_eq!(packed.len(), (w.div_ceil(8)) * h);
    }

    #[test]
    fn chroma_guard_rejects_color_content() {
        let (w, h) = (100usize, 100usize);
        // 95% white background, 5% red stamp (r=240, g=20, b=20)
        let mut samples = vec![245u8; w * h * 3];
        for chunk in samples.as_chunks_mut::<3>().0.iter_mut().take(500) {
            chunk[0] = 240;
            chunk[1] = 20;
            chunk[2] = 20;
        }
        assert_eq!(try_binarize_scan(&samples, w * 3, w, h, 3), None);
    }

    #[test]
    fn chroma_guard_allows_mild_jpeg_fringe_but_rejects_vivid_pixel() {
        // Why: legal stamps and wet-ink signatures carry high chroma (>80) and must never be flattened to 1-bit black.
        let (w, h) = (100usize, 100usize);
        // 95% white background, 4.6% black ink, 0.4% mild chroma fringe (chroma=25 <= 80, <= 0.8%) -> passes
        let mut samples = vec![245u8; w * h * 3];
        for chunk in samples.as_chunks_mut::<3>().0.iter_mut().take(460) {
            chunk[0] = 25;
            chunk[1] = 25;
            chunk[2] = 25;
        }
        for chunk in samples.as_chunks_mut::<3>().0.iter_mut().skip(460).take(40) {
            chunk[0] = 45;
            chunk[1] = 20;
            chunk[2] = 20;
        }
        assert!(try_binarize_scan(&samples, w * 3, w, h, 3).is_some());

        // Mild chroma (chroma = 35 <= 80) exceeding 0.8% (1.2%) must also reject.
        let mut over_fringe = samples.clone();
        for chunk in over_fringe.as_chunks_mut::<3>().0.iter_mut().skip(460).take(120) {
            chunk[0] = 55;
            chunk[1] = 20;
            chunk[2] = 20;
        }
        assert_eq!(try_binarize_scan(&over_fringe, w * 3, w, h, 3), None);

        // Even a single vivid pixel (chroma = 90 > 80) must reject the whole image.
        samples[0] = 150;
        samples[1] = 60;
        samples[2] = 60;
        assert_eq!(try_binarize_scan(&samples, w * 3, w, h, 3), None);
    }

    #[test]
    fn binarizes_anti_aliased_scan_with_mild_jpeg_fringe() {
        // Why: colour scanners saving B&W text as 4:2:0 JPEG introduce <=0.8% mild edge chroma (<=80) that should still compress to 1-bit.
        let (w, h) = (100usize, 100usize);
        let mut samples = vec![245u8; w * h * 3];
        // 5.0% dark ink spread across every other row so no single tile fills with smooth midtones
        for y in (0..h).step_by(4) {
            for x in 10..30 {
                let idx = (y * w + x) * 3;
                samples[idx] = 25;
                samples[idx + 1] = 25;
                samples[idx + 2] = 25;
            }
        }
        // 0.4% mild JPEG fringe (chroma = 40 <= 80, <= 0.8%) along row 2
        for x in 10..50 {
            let idx = (2 * w + x) * 3;
            samples[idx] = 50;
            samples[idx + 1] = 10;
            samples[idx + 2] = 10;
        }
        // 2.5% dark anti-aliased edge (lum = 90 < T) adjacent to ink rows
        for y in (1..h).step_by(4) {
            for x in 10..20 {
                let idx = (y * w + x) * 3;
                samples[idx] = 90;
                samples[idx + 1] = 90;
                samples[idx + 2] = 90;
            }
        }
        // 1.0% light anti-aliased halo (lum = 190 >= T, <= 2.0% content-loss guard)
        for y in (3..h).step_by(8) {
            for x in 10..18 {
                let idx = (y * w + x) * 3;
                samples[idx] = 190;
                samples[idx + 1] = 190;
                samples[idx + 2] = 190;
            }
        }
        assert!(try_binarize_scan(&samples, w * 3, w, h, 3).is_some());
    }

    #[test]
    fn refuses_gray_graphic_above_midtone_budget() {
        let (w, h) = (100usize, 100usize);
        let mut samples = vec![245u8; w * h];
        for s in samples.iter_mut().take(500) {
            *s = 25;
        }
        // 10% midtone shading (lum = 120) exceeds the 6% midtone budget.
        for s in samples.iter_mut().skip(500).take(1000) {
            *s = 120;
        }
        assert_eq!(try_binarize_scan(&samples, w, w, h, 1), None);
    }

    #[test]
    fn content_loss_guard_protects_faint_strokes() {
        // Why: light pencil notes and faded strokes in [T..215) bleach to white paper if bleached flux is unbounded.
        let (w, h) = (200usize, 200usize);
        let total = w * h;
        let mut samples = vec![245u8; total];
        for s in samples.iter_mut().take(2000) {
            *s = 25;
        }
        // Add 2.5% faint pencil strokes at lum 190 (passes <= 6% midtone guard, triggers > 2% content-loss guard)
        for s in samples.iter_mut().skip(2000).take(1000) {
            *s = 190;
        }
        assert_eq!(try_binarize_scan(&samples, w, w, h, 1), None);
    }

    #[test]
    fn scan_candidate_guards_smask_and_opi() {
        let doc = Document::new_empty();

        let mut gray_dict = Dict::new();
        gray_dict.set(b"Subtype".to_vec(), Object::name("Image"));
        gray_dict.set(b"Width".to_vec(), Object::Int(10));
        gray_dict.set(b"Height".to_vec(), Object::Int(10));
        gray_dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        gray_dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        assert!(scan_candidate_shape(&doc, &gray_dict).is_some());

        let mut smask_dict = gray_dict.clone();
        smask_dict.set(b"SMask".to_vec(), Object::Dict(Dict::new()));
        assert!(scan_candidate_shape(&doc, &smask_dict).is_none());

        let mut opi_dict = gray_dict.clone();
        opi_dict.set(b"OPI".to_vec(), Object::Dict(Dict::new()));
        assert!(scan_candidate_shape(&doc, &opi_dict).is_none());
    }

    #[test]
    fn adaptive_compress_scans_end_to_end() {
        let mut doc = Document::new_empty();

        let (w, h) = (100usize, 100usize);
        let mut samples = vec![245u8; w * h];
        for s in samples.iter_mut().take(500) {
            *s = 25;
        }

        let mut dict = Dict::new();
        dict.set(b"Type".to_vec(), Object::name("XObject"));
        dict.set(b"Subtype".to_vec(), Object::name("Image"));
        dict.set(b"Width".to_vec(), Object::Int(w as i64));
        dict.set(b"Height".to_vec(), Object::Int(h as i64));
        dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));

        let stream = Stream::from_raw(dict, samples);
        let r = doc.add(Object::Stream(stream));

        let report = adaptive_compress_scans(&mut doc, AdaptiveCompressOptions::default()).unwrap();
        assert_eq!(report.compressed, 1);
        assert_eq!(report.skipped, 0);

        let Object::Stream(s) = &*doc.get(r) else { panic!() };
        assert_eq!(s.dict.int(b"BitsPerComponent"), Some(1));
        assert_eq!(s.dict.name(b"ColorSpace"), Some(b"DeviceGray".as_slice()));
    }

    #[test]
    fn low_dpi_scan_with_binder_clip_or_small_seal_is_rejected() {
        // Why: 100-DPI scans shrink binder clips and seals to ~11px tiles; scale-invariant tiles B = min(W,H)/75 catch local grey distortion that fixed 64px tiles dilute.
        let (w, h) = (848, 1280);
        let mut samples = vec![245u8; w * h];
        // ~3% crisp black text rows across the page
        for y in (200..1100).step_by(10) {
            for x in 100..360 {
                samples[y * w + x] = 25;
            }
        }
        // Small 36x36 smooth grey metallic clip / seal (~0.12% of page area, below fixed 64x64 threshold)
        for y in 44..80 {
            for x in 220..256 {
                samples[y * w + x] = 120 + ((x + y) % 5) as u8;
            }
        }
        assert!(try_binarize_scan(&samples, w, w, h, 1).is_none());
    }

    #[test]
    fn high_dpi_letterhead_with_small_greyscale_emblem_is_rejected() {
        // Why: a <1% area greyscale coat-of-arms on a 300-DPI page passes global midtone limits (<6%) but suffers >40-level local luminance collapse.
        let (w, h) = (1200, 1600);
        let mut samples = vec![245u8; w * h];
        // ~3% crisp dark text spread across the page
        for y in (300..1450).step_by(10) {
            for x in 150..650 {
                samples[y * w + x] = 25;
            }
        }
        // Small 110x110 smooth greyscale coat-of-arms (~0.63% of page area) at top-left
        for y in 32..142 {
            for x in 32..142 {
                samples[y * w + x] = 115 + ((x + y) % 7) as u8;
            }
        }
        assert!(try_binarize_scan(&samples, w, w, h, 1).is_none());
    }

    #[test]
    fn shaded_table_header_row_is_rejected() {
        // Why: office forms and BOQ tables often shade header rows in mid-grey (Y~110..180); binarizing crushes the row to solid black and hides black column titles.
        let (w, h) = (800, 1000);
        let mut samples = vec![245u8; w * h];
        // ~4% crisp dark table body text
        for y in (220..900).step_by(8) {
            for x in 80..480 {
                samples[y * w + x] = 25;
            }
        }
        // Mid-grey shaded header strip (lum = 120, 30px tall, <2.5% page area) with black column labels
        for y in 120..150 {
            for x in 80..720 {
                samples[y * w + x] = if (y - 120) % 12 < 3 && (x % 90) < 8 { 20 } else { 120 };
            }
        }
        assert!(try_binarize_scan(&samples, w, w, h, 1).is_none());
    }

    #[test]
    fn white_lettering_inside_grey_banner_is_rejected() {
        // Why: inverted white text inside a mid-grey header strip disappears when the surrounding grey background falls below Otsu threshold T.
        let (w, h) = (800, 1000);
        let mut samples = vec![245u8; w * h];
        // ~4% crisp dark document body
        for y in (260..900).step_by(8) {
            for x in 100..520 {
                samples[y * w + x] = 20;
            }
        }
        // Smooth mid-grey header banner (90x400, lum = 125) carrying small inverted white lettering (lum = 245)
        for y in 40..130 {
            for x in 100..500 {
                samples[y * w + x] = if (y % 24) < 3 && (x % 32) == 0 { 245 } else { 125 };
            }
        }
        assert!(try_binarize_scan(&samples, w, w, h, 1).is_none());
    }

    #[test]
    fn dense_small_font_contract_with_high_edge_halo_is_binarized() {
        // Why: dense legal/contract pages accumulate 4–5.5% dark-side anti-aliased edge pixels globally while preserving local block luminance (|mu' - mu| << 40).
        let (w, h) = (1000, 1000);
        let mut samples = vec![245u8; w * h];
        // Dense horizontal 2px dark text rows + 1px dark anti-aliased edge (lum = 95) -> 4.8% global darkened midtones,
        // yet every tile still has >=25% white paper and smooth-grey distortion remains 0.
        for y in (60..940).step_by(18) {
            for x in 80..920 {
                samples[y * w + x] = 20;
                samples[(y + 1) * w + x] = 20;
                samples[(y + 2) * w + x] = 95;
            }
        }
        assert!(try_binarize_scan(&samples, w, w, h, 1).is_some());
    }
}
