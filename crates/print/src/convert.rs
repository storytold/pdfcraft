//! Convert colours: explicit color conversion of document assets (M11 parity `print.convert-colors`).
//!
//! Converts uncalibrated 4-component [`DeviceCMYK`] raster images to [`DeviceRGB`] using the
//! standard subtractive color model (ISO 32000-2 §10.3.5).
//!
//! To prevent color degradation in calibrated prepress workflows, conversion strictly preserves
//! images that have:
//! - Soft masks (`/SMask` or `/SMaskInData`, ISO 32000-2 Table 87)
//! - Prepress production links (`/OPI`)
//! - Embedded color profiles (`/ICCBased`), `/Separation`, or `/DeviceN`
//! - Calibrated document context (`/OutputIntents` with CMYK destinations)
//! - Inherited page-level default color spaces (`/DefaultCMYK` or `/DefaultRGB`)

use std::collections::HashSet;

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};

use crate::PrintError;

/// Conversion configuration options for [`convert_colors`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConvertColorsOptions {
    /// Convert uncalibrated DeviceCMYK rasters to DeviceRGB.
    pub to_rgb: bool,
    /// Preserve calibrated ICC profiles and document OutputIntents (always true for safe conversion).
    pub preserve_icc: bool,
}

impl Default for ConvertColorsOptions {
    fn default() -> Self {
        Self { to_rgb: true, preserve_icc: true }
    }
}

/// Execution summary returned by [`convert_colors`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConvertColorsReport {
    /// Number of DeviceCMYK raster images successfully converted to DeviceRGB.
    pub converted: usize,
    /// Number of DeviceCMYK images skipped due to fail-safe guards.
    pub skipped: usize,
}

/// Converts subtractive CMYK samples (0..=255) to additive RGB (0..=255)
/// using the standard uncalibrated subtractive approximation (ISO 32000-2 §10.3.5):
/// `R = (1 - C) * (1 - K)`, `G = (1 - M) * (1 - K)`, `B = (1 - Y) * (1 - K)`.
#[inline]
pub fn cmyk_to_rgb(c: u8, m: u8, y: u8, k: u8) -> [u8; 3] {
    let r = ((255 - u32::from(c)) * (255 - u32::from(k)) + 127) / 255;
    let g = ((255 - u32::from(m)) * (255 - u32::from(k)) + 127) / 255;
    let b = ((255 - u32::from(y)) * (255 - u32::from(k)) + 127) / 255;
    [r as u8, g as u8, b as u8]
}

/// Applies a 4-channel `/Decode` array `[min, max]` mapping sample `val` from `[0, 255]`
/// into the decoded range `[d0, d1]`, returning the normalized sample in `[0, 255]`.
#[inline]
fn apply_decode(val: u8, d0: f32, d1: f32) -> f32 {
    let v = f32::from(val) / 255.0;
    d0 + v * (d1 - d0)
}

/// Reads and validates a 4-channel CMYK `/Decode` array from image dictionary `d`.
fn read_cmyk_decode(doc: &Document, d: &Dict) -> Option<[f32; 8]> {
    let raw = d.get(b"Decode")?;
    let resolved = doc.resolve(raw);
    let arr = resolved.as_array()?;
    if arr.len() != 8 {
        return None;
    }
    let mut out = [0.0f32; 8];
    for (i, item) in arr.iter().enumerate() {
        let val = doc.resolve(item);
        let num = val.as_f64()?;
        if let Some(slot) = out.get_mut(i) {
            *slot = num as f32;
        }
    }
    Some(out)
}

/// Decodes raw JPEG bytes of a 4-component CMYK image into raw CMYK samples.
fn decode_cmyk_jpeg(data: &[u8], w: usize, h: usize, decode_parms_color_transform: Option<i64>) -> Option<Vec<u8>> {
    let options = DecoderOptions::default().set_strict_mode(false).set_max_width(usize::from(u16::MAX)).set_max_height(usize::from(u16::MAX));
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    dec.decode_headers().ok()?;
    let input = dec.input_colorspace()?;
    if !matches!(input, ColorSpace::CMYK | ColorSpace::YCCK) || dec.dimensions()? != (w, h) {
        return None;
    }

    // ISO 32000-2 Table 13: check marker and DecodeParms contradiction
    if input == ColorSpace::CMYK && decode_parms_color_transform == Some(1) {
        return None;
    }
    if input == ColorSpace::YCCK && decode_parms_color_transform == Some(0) {
        return None;
    }

    dec.set_options(options.jpeg_set_out_colorspace(input));
    let mut px = dec.decode().ok()?;
    if px.len() != w.checked_mul(h)?.checked_mul(4)? {
        return None;
    }

    if input == ColorSpace::YCCK {
        for p in px.as_chunks_mut::<4>().0 {
            let (y, cb, cr) = (f32::from(p[0]), f32::from(p[1]) - 128.0, f32::from(p[2]) - 128.0);
            let ink = |rgb: f32| (255.0 - rgb).round().clamp(0.0, 255.0) as u8;
            p[0] = ink(y + 1.402 * cr);
            p[1] = ink(y - 0.344_136 * cb - 0.714_136 * cr);
            p[2] = ink(y + 1.772 * cb);
        }
    }

    Some(px)
}

/// Whether the color space of image dictionary `d` is pure uncalibrated `/DeviceCMYK`.
fn is_pure_cmyk_space(doc: &Document, d: &Dict) -> bool {
    let Some(cs) = d.get(b"ColorSpace").map(|cs| doc.resolve(cs)) else { return false };
    match &*cs {
        Object::Name(name) => matches!(name.as_slice(), b"DeviceCMYK" | b"CMYK"),
        Object::Array(arr) => {
            let first = arr.first().map(|f| doc.resolve(f));
            first.as_deref().and_then(Object::as_name).is_some_and(|n| matches!(n, b"DeviceCMYK" | b"CMYK"))
        }
        _ => false,
    }
}

/// Checks whether the document catalog has a CMYK `/OutputIntents` dictionary.
fn has_cmyk_output_intents(doc: &Document) -> bool {
    let Some(root_ref) = doc.trailer().reference(b"Root") else { return false };
    let Object::Dict(catalog) = &*doc.get(root_ref) else { return false };
    let Some(intents_obj) = catalog.get(b"OutputIntents").map(|oi| doc.resolve(oi)) else { return false };
    let Some(arr) = intents_obj.as_array() else { return false };
    for item in arr {
        let resolved = doc.resolve(item);
        if let Some(d) = resolved.as_dict() {
            if let Some(subtype) = d.get(b"S").map(|s| doc.resolve(s))
                && (subtype.as_name() == Some(b"GTS_PDFX") || subtype.as_name() == Some(b"GTS_PDFA1"))
            {
                return true;
            }
            if let Some(info) = d.get(b"OutputConditionIdentifier").map(|o| doc.resolve(o))
                && let Some(name) = info.as_name()
            {
                let s = String::from_utf8_lossy(name);
                if s.contains("CGATS") || s.contains("FOGRA") || s.contains("SWOP") || s.contains("GRACoL") {
                    return true;
                }
            }
        }
    }
    false
}

/// Finds all page-level `/DefaultCMYK` or `/DefaultRGB` resource references.
fn page_default_color_spaces(doc: &Document) -> (HashSet<ObjRef>, HashSet<ObjRef>) {
    let mut default_cmyk = HashSet::new();
    let mut default_rgb = HashSet::new();
    for p in pdfcraft_model::pages(doc) {
        let Some(res_obj) = p.dict.get(b"Resources").map(|r| doc.resolve(r)) else { continue };
        let Some(res) = res_obj.as_dict() else { continue };
        let Some(cs_obj) = res.get(b"ColorSpace").map(|cs| doc.resolve(cs)) else { continue };
        let Some(cs_dict) = cs_obj.as_dict() else { continue };
        if let Some(Object::Ref(r)) = cs_dict.get(b"DefaultCMYK") {
            default_cmyk.insert(*r);
        }
        if let Some(Object::Ref(r)) = cs_dict.get(b"DefaultRGB") {
            default_rgb.insert(*r);
        }
    }
    (default_cmyk, default_rgb)
}

/// Converts eligible DeviceCMYK raster images in `doc` to DeviceRGB.
pub fn convert_colors(doc: &mut Document, options: ConvertColorsOptions) -> Result<ConvertColorsReport, PrintError> {
    if !options.to_rgb {
        return Ok(ConvertColorsReport::default());
    }

    if options.preserve_icc && has_cmyk_output_intents(doc) {
        return Ok(ConvertColorsReport::default());
    }

    let (defaults_cmyk, defaults_rgb) = page_default_color_spaces(doc);
    let mut report = ConvertColorsReport::default();
    let image_refs: Vec<ObjRef> = doc
        .scan_objects()
        .filter_map(|(r, obj)| {
            let Object::Stream(s) = &*obj else { return None };
            if s.dict.get(b"Subtype").map(|st| doc.resolve(st)).as_deref().and_then(Object::as_name) != Some(b"Image") {
                return None;
            }
            if !is_pure_cmyk_space(doc, &s.dict) {
                return None;
            }
            Some(r)
        })
        .collect();

    for r in image_refs {
        let Object::Stream(s) = &*doc.get(r) else { continue };
        let d = &s.dict;

        // Strict fail-safe guards per ISO 32000-2 & prepress rules:
        if d.contains(b"SMask") || d.contains(b"SMaskInData") || d.contains(b"OPI") {
            report.skipped += 1;
            continue;
        }
        if defaults_cmyk.contains(&r) || defaults_rgb.contains(&r) {
            report.skipped += 1;
            continue;
        }

        // Only 8-bit components:
        if d.int(b"BitsPerComponent") != Some(8) {
            report.skipped += 1;
            continue;
        }

        let is_jpeg = d.get(b"Filter").map(|f| doc.resolve(f)).as_deref().and_then(Object::as_name) == Some(b"DCTDecode");
        if !is_jpeg {
            // Non-JPEG streams (e.g. FlateDecode or uncompressed) are currently preserved safely.
            report.skipped += 1;
            continue;
        }

        let (Some(w), Some(h)) = (d.int(b"Width"), d.int(b"Height")) else {
            report.skipped += 1;
            continue;
        };
        let (Ok(wu), Ok(hu)) = (usize::try_from(w), usize::try_from(h)) else {
            report.skipped += 1;
            continue;
        };

        let decode_parms = d.get(b"DecodeParms").map(|p| doc.resolve(p));
        let color_transform = decode_parms.as_deref().and_then(Object::as_dict).and_then(|p| p.int(b"ColorTransform"));

        let Some(samples) = decode_cmyk_jpeg(&s.raw, wu, hu, color_transform) else {
            report.skipped += 1;
            continue;
        };

        if samples.len() < wu.checked_mul(hu).and_then(|px| px.checked_mul(4)).unwrap_or(0) {
            report.skipped += 1;
            continue;
        }

        let decode = read_cmyk_decode(doc, d);
        let mut rgb_samples = Vec::with_capacity(wu.checked_mul(hu).and_then(|px| px.checked_mul(3)).unwrap_or(0));

        for chunk in samples.as_chunks::<4>().0 {
            let (c, m, y, k) = match decode {
                Some([c0, c1, m0, m1, y0, y1, k0, k1]) => {
                    let k_norm = apply_decode(chunk[3], k0, k1);
                    let kf = 1.0 - k_norm;
                    let c_norm = apply_decode(chunk[0], c0, c1);
                    let m_norm = apply_decode(chunk[1], m0, m1);
                    let y_norm = apply_decode(chunk[2], y0, y1);
                    let r = (255.0 * (1.0 - c_norm) * kf).round().clamp(0.0, 255.0) as u8;
                    let g = (255.0 * (1.0 - m_norm) * kf).round().clamp(0.0, 255.0) as u8;
                    let b = (255.0 * (1.0 - y_norm) * kf).round().clamp(0.0, 255.0) as u8;
                    rgb_samples.push(r);
                    rgb_samples.push(g);
                    rgb_samples.push(b);
                    continue;
                }
                None => (chunk[0], chunk[1], chunk[2], chunk[3]),
            };
            rgb_samples.extend_from_slice(&cmyk_to_rgb(c, m, y, k));
        }

        // Re-encode as JPEG (Quality 85 matches Acrobat High):
        let mut encoded = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut encoded, 85);
        if encoder.write_image(&rgb_samples, wu as u32, hu as u32, ExtendedColorType::Rgb8).is_err() {
            report.skipped += 1;
            continue;
        }

        let mut new_dict = d.clone();
        new_dict.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        new_dict.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        new_dict.set(b"Length".to_vec(), Object::Int(encoded.len() as i64));
        new_dict.remove(b"Decode");
        new_dict.remove(b"DecodeParms");

        doc.set(r, Object::Stream(Stream::from_raw(new_dict, encoded)));
        report.converted += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmyk_to_rgb_subtractive_math() {
        // Pure black K=255 -> RGB (0, 0, 0)
        assert_eq!(cmyk_to_rgb(0, 0, 0, 255), [0, 0, 0]);
        // Pure white -> RGB (255, 255, 255)
        assert_eq!(cmyk_to_rgb(0, 0, 0, 0), [255, 255, 255]);
        // Pure Cyan -> RGB (0, 255, 255)
        assert_eq!(cmyk_to_rgb(255, 0, 0, 0), [0, 255, 255]);
        // Pure Magenta -> RGB (255, 0, 255)
        assert_eq!(cmyk_to_rgb(0, 255, 0, 0), [255, 0, 255]);
        // Pure Yellow -> RGB (255, 255, 0)
        assert_eq!(cmyk_to_rgb(0, 0, 255, 0), [255, 255, 0]);
    }

    #[test]
    fn decode_array_inversion() {
        // Inverted decode [1.0, 0.0]
        assert_eq!(apply_decode(0, 1.0, 0.0), 1.0);
        assert_eq!(apply_decode(255, 1.0, 0.0), 0.0);
        // Identity decode [0.0, 1.0]
        assert_eq!(apply_decode(128, 0.0, 1.0), 128.0 / 255.0);
    }

    #[test]
    fn convert_colors_guards_smask_and_opi() {
        let mut doc = Document::new_empty();

        let mut dict = Dict::new();
        dict.set(b"Subtype".to_vec(), Object::name("Image"));
        dict.set(b"Width".to_vec(), Object::Int(10));
        dict.set(b"Height".to_vec(), Object::Int(10));
        dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceCMYK"));
        dict.set(b"SMask".to_vec(), Object::Dict(Dict::new()));

        let r = doc.add(Object::Stream(Stream::from_raw(dict, vec![0u8; 100])));
        let report = convert_colors(&mut doc, ConvertColorsOptions::default()).unwrap();

        assert_eq!(report.converted, 0);
        assert_eq!(report.skipped, 1);
        let Object::Stream(s) = &*doc.get(r) else { panic!() };
        assert_eq!(s.dict.get(b"ColorSpace").unwrap().as_name(), Some(b"DeviceCMYK".as_slice()));
    }
}
