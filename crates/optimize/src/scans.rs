//! Recognize Text's "Searchable Image" output: the scan images on recognised pages, resampled
//! down to the chosen resolution in place.
//!
//! Unlike the optimizer's image pass, this keeps every image's kind: the same object, its colour
//! space, its decode array, its masks, and its compression family. A JPEG stays a JPEG; a
//! black-and-white (1-bit) scan or image mask stays 1-bit and lossless (averaged, then
//! thresholded at half, then Flate). Soft masks and stencil masks are left as they are: both are
//! mapped onto the image's unit square whatever their size (ISO 32000-2 §8.9.6.3, §11.6.5.3),
//! so they still line up. What can't be handled faithfully is left alone and reported.
//!
//! The image keeps its placement on the page, so anything placed over it in page space (the
//! invisible OCR text) still lines up.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object, Stream};

use crate::OptimizeError;
use crate::images::{MAX_PIXELS, components, effective_resolutions, is_dct, jpeg, pixels, resample};

/// An image is resampled only when it is more than this much finer than the target: a 310 ppi
/// scan isn't worth recompressing for 300.
const MARGIN: f64 = 1.05;

/// JPEG quality of resampled JPEG scans (between the optimizer's "High" and "Maximum": the text
/// in a scan has to stay crisp).
const JPEG_QUALITY: u8 = 85;

/// What [`downsample_page_images`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanImages {
    /// Images the pages draw (soft and stencil masks not counted).
    pub images: usize,
    /// Images resampled down to the target.
    pub resampled: usize,
    /// Encoded bytes of the resampled images, before and after.
    pub bytes_before: usize,
    pub bytes_after: usize,
    /// Images above the target that were left as they are, each with why.
    pub skipped: Vec<String>,
    /// Why no image of the document was looked at, when the whole document was refused (a
    /// signed one): one reason for the document, not one per image, so it is not in `skipped`.
    pub refused: Option<String>,
}

/// Resample the images that `pages` draw, where their effective resolution is above `target_ppi`,
/// down to it, each in place (the same object). The resolution is the smallest over every use in
/// the document, so an image also drawn larger on another page is never made too coarse there.
/// A non-finite or non-positive target changes nothing.
pub fn downsample_page_images(doc: &mut Document, pages: &[ObjRef], target_ppi: f64) -> Result<ScanImages, OptimizeError> {
    let mut report = ScanImages::default();
    if !target_ppi.is_finite() || target_ppi <= 0.0 || pages.is_empty() {
        return Ok(report);
    }
    let all = pdfcraft_annot::page_refs(doc).map_err(|_| OptimizeError::NoPages)?;
    let here = effective_resolutions(doc, pages);
    let everywhere = effective_resolutions(doc, &all);
    // Masks belong to their image and are kept as they are.
    let masks: HashSet<ObjRef> = here
        .keys()
        .filter_map(|r| match &*doc.get(*r) {
            Object::Stream(s) => Some([s.dict.reference(b"SMask"), s.dict.reference(b"Mask")]),
            _ => None,
        })
        .flatten()
        .flatten()
        .collect();
    let mut refs: Vec<ObjRef> = here.keys().copied().filter(|r| !masks.contains(r)).collect();
    refs.sort_by_key(|r| (r.num, r.generation));
    for r in refs {
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        report.images += 1;
        let ppi = everywhere.get(&r).copied().unwrap_or(f64::INFINITY);
        let name = describe(r, s);
        if !ppi.is_finite() {
            report.skipped.push(format!("{name}: where the document draws it couldn't all be measured"));
            continue;
        }
        if ppi <= target_ppi * MARGIN {
            continue;
        }
        match resampled(doc, s, ppi, target_ppi) {
            Ok(new) if new.raw.len() < s.raw.len() => {
                report.resampled += 1;
                report.bytes_before = report.bytes_before.saturating_add(s.raw.len());
                report.bytes_after = report.bytes_after.saturating_add(new.raw.len());
                doc.set(r, Object::Stream(new));
            }
            Ok(_) => report.skipped.push(format!("{name}: resampled, it was not smaller")),
            Err(why) => report.skipped.push(format!("{name}: {why}")),
        }
    }
    Ok(report)
}

/// "image 12 0 (2480 × 3508 px)", for the report.
fn describe(r: ObjRef, s: &Stream) -> String {
    match (s.dict.int(b"Width"), s.dict.int(b"Height")) {
        (Some(w), Some(h)) => format!("image {} {} ({w} × {h} px)", r.num, r.generation),
        _ => format!("image {} {}", r.num, r.generation),
    }
}

/// The filter names of `s`, for a reason it can't be decoded.
fn filter_names(s: &Stream) -> String {
    let names: Vec<String> = match s.dict.get(b"Filter") {
        Some(Object::Name(n)) => vec![String::from_utf8_lossy(n).into_owned()],
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).map(|n| String::from_utf8_lossy(n).into_owned()).collect(),
        _ => Vec::new(),
    };
    if names.is_empty() { "its compression".into() } else { names.join(" + ") }
}

/// `s` resampled from `ppi` to `target` (both finite, `ppi` above `target`), or why not.
fn resampled(doc: &Document, s: &Stream, ppi: f64, target: f64) -> Result<Stream, String> {
    let d = &s.dict;
    let (Some(w), Some(h)) = (d.int(b"Width").and_then(|v| u32::try_from(v).ok()), d.int(b"Height").and_then(|v| u32::try_from(v).ok())) else {
        return Err("its size is missing or invalid".into());
    };
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("it is empty or too large to resample".into());
    }
    if d.contains(b"F") {
        return Err("its data is in an external file".into());
    }
    let k = target / ppi;
    let scaled = |v: u32| {
        let n = (f64::from(v) * k).round();
        // `k` is below 1 and positive, so this is between 1 and `v`.
        if n >= 1.0 { n.min(f64::from(v)) as u32 } else { 1 }
    };
    let (nw, nh) = (scaled(w), scaled(h));
    if (nw, nh) == (w, h) {
        return Err("it is already at the target size".into());
    }
    let image_mask = matches!(d.get(b"ImageMask").map(|m| doc.resolve(m)).as_deref(), Some(Object::Bool(true)));
    // A colour-key mask names exact sample values, which resampling blends into others.
    if !image_mask && d.get(b"Mask").is_some_and(|m| matches!(&*doc.resolve(m), Object::Array(_))) {
        return Err("it has a colour-key mask, which resampled colours would no longer match".into());
    }
    // Premultiplied soft masks must keep the image's size (§11.6.5.3).
    if let Some(m) = d.reference(b"SMask")
        && let Object::Stream(m) = &*doc.get(m)
        && m.dict.contains(b"Matte")
    {
        return Err("its soft mask is premultiplied (/Matte), which needs the image's own size".into());
    }
    let bpc = d.int(b"BitsPerComponent");
    let n = if image_mask { Some(1) } else { d.get(b"ColorSpace").and_then(|cs| components(doc, cs)) };
    let Some(n) = n else {
        return Err("its colour space isn't grey or RGB (CMYK, indexed and others are left alone)".into());
    };
    let Some(dct) = is_dct(s) else {
        return Err(format!("{} isn't decoded here yet", filter_names(s)));
    };
    let mut dict = d.clone();
    for key in [&b"Filter"[..], b"DecodeParms", b"Length", b"DL"] {
        dict.remove(key);
    }
    dict.set(b"Width".to_vec(), Object::Int(i64::from(nw)));
    dict.set(b"Height".to_vec(), Object::Int(i64::from(nh)));
    match (bpc, image_mask) {
        // Black and white: stays 1-bit and lossless.
        (Some(1) | None, true) | (Some(1), false) if n == 1 && !dct => {
            let gray = bilevel_samples(s, w, h).ok_or("its samples couldn't be decoded")?;
            let small = resample(&gray, 1, w, h, nw, nh).ok_or("it couldn't be resampled")?;
            Ok(Stream::flate(dict, &pack_bits(&small, nw, nh).ok_or("it couldn't be resampled")?))
        }
        (Some(8), false) => {
            // The samples as stored: a decode array is linear, so it applies to resampled
            // samples as it did to the originals, and stays.
            let px = pixels(doc, s, n).ok_or("its samples couldn't be decoded (or its JPEG colour transform is ambiguous)")?;
            let small = resample(&px, n, w, h, nw, nh).ok_or("it couldn't be resampled")?;
            if dct {
                let data = jpeg(&small, n, nw, nh, JPEG_QUALITY).ok_or("it couldn't be encoded as JPEG")?;
                dict.set(b"Filter".to_vec(), Object::name("DCTDecode"));
                Ok(Stream::from_raw(dict, data))
            } else {
                Ok(Stream::flate(dict, &small))
            }
        }
        (_, true) => Err("its image mask isn't stored as plain 1-bit samples".into()),
        (Some(1), false) => Err("it is a 1-bit colour image".into()),
        (Some(b), false) => Err(format!("it has {b} bits per component (1 and 8 are resampled)")),
        (None, false) => Err("it has no bits per component".into()),
    }
}

/// The 1-bit samples of `s` (rows padded to a byte), unpacked to one byte each: 0 or 255.
fn bilevel_samples(s: &Stream, w: u32, h: u32) -> Option<Vec<u8>> {
    let row = usize::try_from(w).ok()?.div_ceil(8);
    let len = row.checked_mul(usize::try_from(h).ok()?)?;
    let data = s.decoded().ok()?;
    let data = data.get(..len)?;
    let w = usize::try_from(w).ok()?;
    let mut out = Vec::with_capacity(w.checked_mul(usize::try_from(h).ok()?)?);
    for r in data.chunks(row) {
        out.extend((0..w).map(|x| if r.get(x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0) { 255 } else { 0 }));
    }
    Some(out)
}

/// One-byte-per-pixel grey `px` (`w` × `h`) thresholded at half and packed to 1 bit, rows padded
/// to a byte.
fn pack_bits(px: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let w = usize::try_from(w).ok()?;
    let row = w.div_ceil(8);
    let mut out = vec![0u8; row.checked_mul(usize::try_from(h).ok()?)?];
    if w == 0 {
        return Some(out);
    }
    for (src, dst) in px.chunks(w).zip(out.chunks_mut(row)) {
        for (x, v) in src.iter().enumerate() {
            if *v >= 128
                && let Some(b) = dst.get_mut(x / 8)
            {
                *b |= 0x80 >> (x % 8);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_unpack_and_pack_back_to_the_same_rows() {
        // 10 pixels wide: two bytes per row, the last six bits padding.
        let rows = [0b1010_0000u8, 0b1100_0000, 0b0000_0001, 0b0100_0000];
        let s = Stream::from_raw(pdfcraft_cos::Dict::new(), rows.to_vec());
        let px = bilevel_samples(&s, 10, 2).unwrap();
        assert_eq!(&px[..10], &[255, 0, 255, 0, 0, 0, 0, 0, 255, 255]);
        assert_eq!(&px[10..], &[0, 0, 0, 0, 0, 0, 0, 255, 0, 255]);
        assert_eq!(pack_bits(&px, 10, 2).unwrap(), rows);
        // Short data is refused, not read past.
        assert!(bilevel_samples(&s, 10, 3).is_none());
    }
}
