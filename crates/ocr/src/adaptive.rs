//! Adaptive (MRC) compression of scans (Acrobat parity `ocr.adaptive-compression`).
//!
//! Heuristic dual-guard Otsu scan binarization converting near-bilevel 8-bit text scans
//! (grayscale or RGB) into 1-bit monochrome images compressed with CCITT Group 4 or Flate.
//!
//! Prepress and content-loss guards:
//! - Chroma guard: Rejects colored content, stamps, and highlighters.
//! - Bimodal histogram guard: Requires clear dark ink and bright paper peaks; rejects flat or unimodal distributions.
//! - Content-loss guard: Protects faint pencil/pen strokes from bleaching out to background.
//! - Prepress fail-safe: Strictly preserves images with soft masks (/SMask, /SMaskInData),
//!   transparency masks (/Mask), or prepress production links (/OPI).

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::OcrError;

/// Maximum allowable chroma difference for dark/ink content (lum < 225).
const CHROMA_TOLERANCE: u8 = 15;

/// Absolute chroma ceiling for bright pixels (lum >= 225): colored highlighters or paper stains
/// must never be treated as paper background.
const BRIGHT_CHROMA_LIMIT: u8 = 30;

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

/// Attempts to binarize an 8-bit image (1 channel grayscale or 3 channels RGB) into 1-bit samples
/// using Heuristic Dual-Guard (Chroma Guard + Bimodal Histogram Guard) and Otsu thresholding.
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

    for row in samples.chunks(row_len).take(h) {
        let row_pixels = row.get(..w.checked_mul(n)?)?;
        for chunk in row_pixels.chunks_exact(n) {
            let lum = if n == 1 {
                chunk[0]
            } else {
                let (r, g, b) = (chunk[0], chunk[1], chunk[2]);
                let lum = ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8;
                let max_c = r.max(g).max(b);
                let min_c = r.min(g).min(b);
                let chroma = max_c - min_c;
                if chroma > BRIGHT_CHROMA_LIMIT || (lum < 225 && chroma > CHROMA_TOLERANCE) {
                    // Color detail detected! Reject to preserve color appearance.
                    return None;
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
                    // Early exit if midtones exceed 0.5% (mid / total > 0.005)
                    if (mid as u64).saturating_mul(200) > total as u64 {
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

    // Scope: Pristine near-bilevel scans.
    // Must be strictly two-tone (at least 98% in dark ink or paper-bright extremes),
    // with virtually no midtones (at most 0.5%), containing actual dark text content
    // (at least 0.5%), and white paper background (at least 50%).
    if extreme_ratio < 0.98 || midtone_ratio > 0.005 || dark_ratio < 0.005 || bright_ratio < 0.50 {
        return None;
    }

    // Otsu thresholding & Content-Loss Guard
    let raw = compute_otsu(&hist, total)?;
    let threshold = raw.clamp(100, 180);

    // Guard against bleaching faint text strokes (lum in [threshold..215)).
    let bleached_ink: u32 = hist.get(usize::from(threshold)..215).map_or(0, |slice| slice.iter().sum());
    if (bleached_ink as f64 / total as f64) > 0.0005 {
        return None;
    }

    // Pack 1-bit (MSB first, 1 = white, 0 = black)
    let mut out = vec![0u8; out_len];
    for (y, row) in samples.chunks(row_len).take(h).enumerate() {
        let row_pixels = row.get(..w.checked_mul(n)?)?;
        let out_row = out.get_mut(y * stride..(y + 1) * stride)?;
        for (x, chunk) in row_pixels.chunks_exact(n).enumerate() {
            let lum =
                if n == 1 { chunk[0] } else { ((u32::from(chunk[0]) * 299 + u32::from(chunk[1]) * 587 + u32::from(chunk[2]) * 114) / 1000) as u8 };
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
    for pair in arr.chunks_exact(2) {
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
    fn content_loss_guard_protects_faint_strokes() {
        let (w, h) = (200usize, 200usize);
        let total = w * h;
        let mut samples = vec![245u8; total];
        for s in samples.iter_mut().take(2000) {
            *s = 25;
        }
        // Add 0.1% faint pencil strokes at lum 190
        for s in samples.iter_mut().skip(2000).take(40) {
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
}
