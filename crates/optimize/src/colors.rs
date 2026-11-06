//! Print production ▸ Convert colors, for images: RGB images drawn on the chosen pages become
//! grayscale or CMYK images (8 bits, Flate), with the same simple device formulas as the page
//! colours (see `pdfcraft_edit::convert_colors`). Images in other colour spaces, with a decode
//! array, or in a codec this crate can't read are left as they are, and so are images inside
//! form XObjects and patterns.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ImageBudget, Object, Stream};

use crate::OptimizeError;
use crate::images::{components, decodable, page_resources, pixels};

/// Images with more pixels than this are left alone (memory).
const MAX_PIXELS: u64 = 80_000_000;

fn clamp(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// One RGB pixel (0–255) in grey or CMYK (0–255).
fn convert_pixel(rgb: &[u8], gray: bool, out: &mut Vec<u8>) {
    let [r, g, b] = [0, 1, 2].map(|i| f64::from(rgb.get(i).copied().unwrap_or(0)) / 255.0);
    if gray {
        out.push((clamp(0.3 * r + 0.59 * g + 0.11 * b) * 255.0).round() as u8);
    } else {
        let k = 1.0 - r.max(g).max(b);
        let cmyk = if k >= 1.0 { [0.0, 0.0, 0.0, 1.0] } else { [(1.0 - r - k) / (1.0 - k), (1.0 - g - k) / (1.0 - k), (1.0 - b - k) / (1.0 - k), k] };
        out.extend(cmyk.map(|v| (clamp(v) * 255.0).round() as u8));
    }
}

/// The converted image, or `None` to leave it.
fn convert_image(doc: &Document, s: &Stream, gray: bool, budget: &mut ImageBudget) -> Option<Stream> {
    if s.dict.get(b"Subtype").and_then(Object::as_name) != Some(b"Image".as_slice()) || matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true))) {
        return None;
    }
    if components(doc, s.dict.get(b"ColorSpace")?)? != 3 || !decodable(doc, s)? {
        return None;
    }
    let (w, h) = (u64::try_from(s.dict.int(b"Width")?).ok()?, u64::try_from(s.dict.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
        return None;
    }
    // Pixels are read 8 bits a sample; a Flate image with other depths is left alone.
    let dct = matches!(s.dict.get(b"Filter").and_then(Object::as_name), Some(b"DCTDecode" | b"DCT"));
    if !dct && s.dict.int(b"BitsPerComponent") != Some(8) {
        return None;
    }
    let px = pixels(doc, s, 3, budget)?;
    let mut out = Vec::with_capacity(px.len() / 3 * if gray { 1 } else { 4 });
    for p in px.as_chunks::<3>().0 {
        convert_pixel(p, gray, &mut out);
    }
    let mut d = s.dict.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"Decode", b"Length", b"DL"] {
        d.remove(k);
    }
    d.set(b"ColorSpace".to_vec(), Object::name(if gray { "DeviceGray" } else { "DeviceCMYK" }));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    Some(Stream::flate(d, &out))
}

/// Convert the RGB images drawn directly on `pages` (0-based) to grey (`gray`) or CMYK. Returns
/// how many images changed (an image shared by several pages counts once).
pub fn convert_page_images(doc: &mut Document, pages: &[usize], gray: bool) -> Result<usize, OptimizeError> {
    let all = pdfcraft_model::pages(doc);
    let mut seen = HashSet::new();
    let mut changed = 0_usize;
    // One decoding allowance for the whole conversion, as upstream's image passes have.
    let mut budget = ImageBudget::new(doc);
    for &p in pages {
        let Some(page) = all.get(p) else { continue };
        let Some(resources) = page_resources(doc, page.obj) else { continue };
        let Some(xobjects) = resources.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()) else { continue };
        for (_, v) in xobjects.iter() {
            let Some(r) = v.as_ref() else { continue };
            if !seen.insert(r) {
                continue;
            }
            let new = match &*doc.get(r) {
                Object::Stream(s) => convert_image(doc, s, gray, &mut budget),
                _ => None,
            };
            if let Some(new) = new {
                doc.set(r, Object::Stream(new));
                changed = changed.saturating_add(1);
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_convert_to_grey_and_cmyk() {
        let mut g = Vec::new();
        convert_pixel(&[255, 255, 255], true, &mut g);
        convert_pixel(&[255, 0, 0], true, &mut g);
        assert_eq!(g, [255, 77]);
        let mut c = Vec::new();
        convert_pixel(&[255, 0, 0], false, &mut c);
        convert_pixel(&[0, 0, 0], false, &mut c);
        assert_eq!(c, [0, 255, 255, 0, 0, 0, 0, 255]);
    }
}
