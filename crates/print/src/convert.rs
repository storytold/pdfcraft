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

use std::collections::{HashSet, VecDeque};

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
    (d0 + v * (d1 - d0)).clamp(0.0, 1.0)
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

/// Largest CMYK JPEG, in pixels, that conversion decodes. Larger images are left unchanged.
const MAX_CMYK_JPEG_PIXELS: usize = 1 << 26;

/// Decodes raw JPEG bytes of a 4-component CMYK image into raw CMYK samples.
fn decode_cmyk_jpeg(data: &[u8], w: usize, h: usize, decode_parms_color_transform: Option<i64>) -> Option<Vec<u8>> {
    if w.checked_mul(h)? > MAX_CMYK_JPEG_PIXELS {
        return None;
    }
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

/// Upper bound on Form-dictionary visits per document. With a global visited set, each
/// reachable Form is enqueued at most once, so this is a hard cap on work. When exceeded,
/// return `true` conservatively so the document-wide skip fires.
const MAX_FORM_VISITS: usize = 10_000;

/// Whether any page, or any Form XObject reachable from a page, defines
/// `/DefaultCMYK` or `/DefaultRGB` in its `/Resources /ColorSpace` dictionary.
fn pages_have_default_colour_spaces(doc: &Document) -> bool {
    let mut visited: HashSet<ObjRef> = HashSet::new();
    let mut work = 0usize;
    for p in pdfcraft_model::pages(doc) {
        if dict_reaches_default(doc, &p.dict, &mut visited, &mut work) {
            return true;
        }
    }
    false
}

/// Breadth-first walk from `start` over Form XObjects, looking for a scope that
/// declares `/DefaultCMYK` or `/DefaultRGB`.
fn dict_reaches_default(doc: &Document, start: &Dict, visited: &mut HashSet<ObjRef>, work: &mut usize) -> bool {
    let mut queue: VecDeque<Dict> = VecDeque::new();
    queue.push_back(start.clone());
    while let Some(dict) = queue.pop_front() {
        *work += 1;
        if *work > MAX_FORM_VISITS {
            return true;
        }
        let Some(res_obj) = dict.get(b"Resources").map(|r| doc.resolve(r)) else { continue };
        let Some(res) = res_obj.as_dict() else { continue };
        if let Some(cs_obj) = res.get(b"ColorSpace").map(|cs| doc.resolve(cs))
            && let Some(cs_dict) = cs_obj.as_dict()
            && (cs_dict.contains(b"DefaultCMYK") || cs_dict.contains(b"DefaultRGB"))
        {
            return true;
        }
        let Some(xobj_obj) = res.get(b"XObject").map(|x| doc.resolve(x)) else { continue };
        let Some(xobj) = xobj_obj.as_dict() else { continue };
        for (_, val) in xobj.iter() {
            let Object::Ref(r) = val else { continue };
            if !visited.insert(*r) {
                continue;
            }
            if let Object::Stream(s) = &*doc.get(*r)
                && s.dict.name(b"Subtype") == Some(b"Form")
            {
                queue.push_back(s.dict.clone());
            }
        }
    }
    false
}

/// Converts eligible DeviceCMYK raster images in `doc` to DeviceRGB.
pub fn convert_colors(doc: &mut Document, options: ConvertColorsOptions) -> Result<ConvertColorsReport, PrintError> {
    if !options.to_rgb {
        return Ok(ConvertColorsReport::default());
    }

    if options.preserve_icc && (has_cmyk_output_intents(doc) || pages_have_default_colour_spaces(doc)) {
        return Ok(ConvertColorsReport::default());
    }

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
        let colour_key_mask = matches!(d.get(b"Mask").map(|m| doc.resolve(m)).as_deref(), Some(Object::Array(_)));
        if d.contains(b"SMask") || d.contains(b"SMaskInData") || d.contains(b"OPI") || colour_key_mask {
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

    #[test]
    fn form_local_default_colour_space_skips_conversion() {
        let mut doc = Document::new_empty();

        // A form with a form-local /DefaultCMYK in its resources.
        let mut form_res = Dict::new();
        let mut form_cs = Dict::new();
        form_cs.set(b"DefaultCMYK".to_vec(), Object::name("DeviceCMYK"));
        form_res.set(b"ColorSpace".to_vec(), Object::Dict(form_cs));
        let mut form_dict = Dict::new();
        form_dict.set(b"Subtype".to_vec(), Object::name("Form"));
        form_dict.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]));
        form_dict.set(b"Resources".to_vec(), Object::Dict(form_res));
        let form = doc.add(Object::Stream(Stream::from_raw(form_dict, vec![])));

        // Direct test: empty page resources return false, while the form's resources return true.
        assert!(!dict_reaches_default(&doc, &Dict::new(), &mut HashSet::new(), &mut 0));
        let Object::Stream(s) = &*doc.get(form) else { panic!() };
        assert!(dict_reaches_default(&doc, &s.dict, &mut HashSet::new(), &mut 0));

        // Page referencing the form via /XObject also detects the form-local default.
        let mut xobj = Dict::new();
        xobj.set(b"Fm0".to_vec(), Object::Ref(form));
        let mut page_res = Dict::new();
        page_res.set(b"XObject".to_vec(), Object::Dict(xobj));
        let mut page_dict = Dict::new();
        page_dict.set(b"Resources".to_vec(), Object::Dict(page_res));
        assert!(dict_reaches_default(&doc, &page_dict, &mut HashSet::new(), &mut 0));
    }

    #[test]
    fn form_traversal_work_is_bounded_on_adversarial_graphs() {
        // Deterministic pseudo-random generator; no external crate.
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for case in 0..64 {
            let mut doc = Document::new_empty();
            const FORMS: usize = 60;
            let mut form_refs: Vec<ObjRef> = Vec::new();
            for _ in 0..FORMS {
                let mut xobjects = Dict::new();
                let entries = (next() % 9) as usize;
                for k in 0..entries {
                    let name = format!("E{k}").into_bytes();
                    let target = if form_refs.is_empty() || next() % 3 == 0 {
                        // Non-Form XObject: skipped by the subtype filter.
                        let mut d = Dict::new();
                        d.set(b"Type".to_vec(), Object::name("XObject"));
                        doc.add(Object::Stream(Stream::from_raw(d, vec![])))
                    } else {
                        // Earlier Form, possibly shared many times or cyclic.
                        form_refs[(next() as usize) % form_refs.len()]
                    };
                    xobjects.set(name, Object::Ref(target));
                }
                let mut res = Dict::new();
                res.set(b"XObject".to_vec(), Object::Dict(xobjects));
                let mut form = Dict::new();
                form.set(b"Subtype".to_vec(), Object::name("Form"));
                form.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]));
                form.set(b"Resources".to_vec(), Object::Dict(res));
                form_refs.push(doc.add(Object::Stream(Stream::from_raw(form, vec![]))));
            }

            let mut page_res = Dict::new();
            let mut xobjects = Dict::new();
            xobjects.set(b"F".to_vec(), Object::Ref(*form_refs.last().unwrap()));
            page_res.set(b"XObject".to_vec(), Object::Dict(xobjects));
            let mut page = Dict::new();
            page.set(b"Resources".to_vec(), Object::Dict(page_res));
            let page_ref = doc.add(Object::Dict(page));
            let Object::Dict(pd) = &*doc.get(page_ref) else { panic!() };

            let mut visited: HashSet<ObjRef> = HashSet::new();
            let mut work = 0usize;
            let start = std::time::Instant::now();
            let _ = dict_reaches_default(&doc, pd, &mut visited, &mut work);
            let elapsed = start.elapsed();

            assert!(elapsed.as_millis() < 500, "case {case}: {elapsed:?}");
            assert!(work <= MAX_FORM_VISITS + 1, "case {case}: work={work}");
        }
    }
}
