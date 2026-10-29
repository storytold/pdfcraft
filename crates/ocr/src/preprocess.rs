//! The composed preprocessing pipeline (ClearScan stage 1).
//!
//! [`preprocess`] takes a page raster as it comes off the renderer (the same flat buffer
//! [`crate::Ocr::recognize`] takes), and returns a [`PreprocessedPage`]: a deskewed greyscale,
//! its Sauvola binarization, and the affine that maps the deskewed frame back onto the
//! original raster. The order is deliberate:
//!
//! 1. **Convert** to greyscale ([`crate::grey`]).
//! 2. **Estimate the skew** on a downscaled, Otsu-gated darkness map of the original (the
//!    paper weighs nothing; no page binarization runs yet).
//! 3. **Rectify once**, rotating the full-resolution greyscale bilinearly about the centre.
//!    Rotating the greyscale rather than the bitmap keeps strokes smooth for the binarizer.
//! 4. **Binarize once**, on the level page, where window statistics are not skewed across
//!    text lines.
//!
//! Everything found afterwards in the deskewed frame — recognition boxes, connected
//! components, glyph bitmaps — maps back onto the scanned raster through `to_original`, and
//! the original maps forward through `from_original`.

use crate::OcrError;
use crate::binarize::{Binarization, Bitmap, SauvolaParams, binarize};
use crate::deskew::{Affine, DEFAULT_MAX_SKEW_DEGREES, estimate_skew, rectify};
use crate::grey::{GreyImage, to_grey};

/// The knobs of [`preprocess`]. The defaults fit a 300 DPI text scan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreprocessedOptions {
    /// The largest skew looked for, in degrees (1…45).
    pub max_skew_degrees: f64,
    /// Which threshold separates ink from paper.
    pub binarization: Binarization,
    /// Sauvola's window and bias, when it is used.
    pub sauvola: SauvolaParams,
}

impl Default for PreprocessedOptions {
    fn default() -> Self {
        PreprocessedOptions { max_skew_degrees: DEFAULT_MAX_SKEW_DEGREES, binarization: Binarization::Auto, sauvola: SauvolaParams::default() }
    }
}

/// A page ready for glyph work: level, binarized, and traceable back to the scan.
#[derive(Clone, Debug)]
pub struct PreprocessedPage {
    /// The deskewed greyscale. Recognition runs on this, so its boxes share this frame.
    pub grey: GreyImage,
    /// The binarization of `grey`: 1 is ink.
    pub binary: Bitmap,
    /// The tilt that was found and removed, in radians (positive: the text descended to the
    /// right).
    pub angle_radians: f64,
    /// Deskewed pixel coordinates → original raster coordinates.
    pub to_original: Affine,
    /// Original raster pixel coordinates → deskewed pixel coordinates.
    pub from_original: Affine,
}

/// Preprocess a page raster: `pixels` holds `width` × `height` pixels of `channels` (1, 3 or
/// 4) bytes each, row-major from the top-left.
pub fn preprocess(pixels: &[u8], width: u32, height: u32, channels: u32, options: &PreprocessedOptions) -> Result<PreprocessedPage, OcrError> {
    let grey = to_grey(pixels, width, height, channels).map_err(OcrError::Preprocess)?;
    let angle = estimate_skew(&grey, options.max_skew_degrees);
    let deskewed = rectify(&grey, angle).map_err(OcrError::Preprocess)?;
    let binary = binarize(&deskewed.grey, options.binarization, &options.sauvola);
    let to_original = deskewed.to_original;
    // A rotation is always invertible; the identity fallback only covers a degenerate map
    // that cannot arise, and fails soft as ever here.
    let from_original = to_original.inverse().unwrap_or(Affine::IDENTITY);
    Ok(PreprocessedPage { grey: deskewed.grey, binary, angle_radians: angle, to_original, from_original })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tilted, unevenly lit page of text: strokes on a gradient, tilted by `angle`.
    fn degraded_scan(width: u32, height: u32, angle: f64) -> Vec<u8> {
        let mut data = vec![0_u8; (width * height) as usize];
        let (sin, cos) = angle.sin_cos();
        let put = |data: &mut [u8], x: i64, y: i64, value: u8| {
            if x >= 0 && y >= 0 && (x as u64) < u64::from(width) && (y as u64) < u64::from(height) {
                let index = y as usize * width as usize + x as usize;
                data[index] = value;
            }
        };
        for line in 0..5_i64 {
            let line_y = 28 + line * 40;
            for word in 0..7_i64 {
                let word_x = 20 + word * 48;
                for stroke in 0..4_i64 {
                    let base_x = (word_x + stroke * 8) as f64;
                    // The stroke's origin sits as far along the tilted baseline as its page-x.
                    let base_y = line_y as f64 + base_x * angle.tan();
                    for along in 0..3_i64 {
                        for down in 0..16_i64 {
                            let t = along as f64;
                            let u = down as f64;
                            let x = base_x + t * cos - u * sin;
                            let y = base_y + t * sin + u * cos;
                            // A gradient across the sheet, ink 70 levels under the local paper.
                            let paper = 120.0 + x / f64::from(width) * 110.0;
                            let ink = (paper - 70.0).round().clamp(0.0, 255.0) as u8;
                            put(&mut data, x.round() as i64, y.round() as i64, ink);
                        }
                    }
                }
            }
        }
        // Fill the paper with the gradient the strokes were drawn against.
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                if data[index] == 0 {
                    let paper = (120.0 + f64::from(x) / f64::from(width) * 110.0).round().clamp(0.0, 255.0) as u8;
                    data[index] = paper;
                }
            }
        }
        data
    }

    #[test]
    fn the_pipeline_levels_binarizes_and_maps_back() {
        let (width, height) = (400_u32, 240_u32);
        let pixels = degraded_scan(width, height, 2.6_f64.to_radians());
        let page = preprocess(&pixels, width, height, 1, &PreprocessedOptions::default()).unwrap();
        // The degraded page (gradient, rounding) estimates within a fifth of a degree; a
        // clean page reads within 0.12° (see the deskew tests).
        assert!((page.angle_radians.to_degrees() - 2.6).abs() <= 0.2, "angle {}", page.angle_radians.to_degrees());
        assert!(page.binary.ink_count() > 0, "the text is ink");
        // The text is level now: its ink rows are tight, the profile variance across rows is
        // high (compare the same measure on the tilted input's binarization).
        let rows = row_sharpness(&page.binary);
        let tilted =
            crate::binarize::binarize(&crate::grey::to_grey(&pixels, width, height, 1).unwrap(), Binarization::Auto, &SauvolaParams::default());
        let before = row_sharpness(&tilted);
        assert!(rows > before * 1.5, "rows sharpened: {rows} vs {before}");
        // The transforms compose to the identity on a deskewed point.
        let (x, y) = page.to_original.apply(150.0, 90.0);
        let (bx, by) = page.from_original.apply(x, y);
        assert!((bx - 150.0).abs() < 1e-9 && (by - 90.0).abs() < 1e-9);
    }

    /// The sum of squares of per-row ink counts: sharp text lines score high.
    fn row_sharpness(binary: &Bitmap) -> f64 {
        (0..binary.height())
            .map(|y| {
                let ink = (0..binary.width()).filter(|&x| binary.is_ink(x as i64, y as i64)).count() as f64;
                ink * ink
            })
            .sum()
    }

    #[test]
    fn rgba_input_is_accepted_and_matches_the_grey_input() {
        let (width, height) = (200_u32, 120_u32);
        let grey = degraded_scan(width, height, 1.5_f64.to_radians());
        let mut rgba = Vec::with_capacity(grey.len() * 4);
        for g in &grey {
            rgba.extend_from_slice(&[*g, *g, *g, 255]);
        }
        let a = preprocess(&grey, width, height, 1, &PreprocessedOptions::default()).unwrap();
        let b = preprocess(&rgba, width, height, 4, &PreprocessedOptions::default()).unwrap();
        assert_eq!(a.binary, b.binary);
        assert_eq!(a.angle_radians, b.angle_radians);
    }

    #[test]
    fn a_blank_page_comes_through_untouched_and_empty() {
        let pixels = vec![250_u8; 100 * 60];
        let page = preprocess(&pixels, 100, 60, 1, &PreprocessedOptions::default()).unwrap();
        assert_eq!(page.angle_radians, 0.0);
        assert_eq!(page.binary.ink_count(), 0);
        assert_eq!((page.grey.width(), page.grey.height()), (102, 62), "only the margin");
    }

    #[test]
    fn hostile_inputs_are_errors_not_panics() {
        let options = PreprocessedOptions::default();
        let error = |r: Result<PreprocessedPage, OcrError>| r.unwrap_err().to_string();
        assert!(error(preprocess(&[], 0, 0, 1, &options)).contains("the image is empty"));
        assert!(error(preprocess(&[1, 2], 2, 1, 2, &options)).contains("got 2"));
        assert!(error(preprocess(&[0; 8], 6000, 8193, 1, &options)).contains("at most"));
    }

    #[test]
    fn a_single_pixel_is_a_page() {
        let page = preprocess(&[128], 1, 1, 1, &PreprocessedOptions::default()).unwrap();
        // On the margin canvas the lone grey pixel is the only dark class, so Otsu marks
        // just it; nothing else is invented.
        assert!(page.binary.ink_count() <= 1, "{} ink pixels", page.binary.ink_count());
        assert_eq!(page.to_original.apply(1.0, 1.0), (0.0, 0.0));
    }
}
