//! Resolution normalisation before recognition: how large a page's letters really are, and what
//! to do when they are too small to read.
//!
//! A 72 dpi scan's lowercase letters are about four pixels tall; the detector shrinks the page
//! further before it looks, and the recogniser guesses at what is left. Correcting the declared
//! resolution is not enough, because no declaration adds pixels: on the exemplar that opened
//! this work the scanner's own "correct image resolution" moved word accuracy from 0.851 to
//! 0.881, while a genuine 3× enlargement reached 0.970. Enlarging is therefore our own stage,
//! ahead of recognition.
//!
//! The size of the letters is not taken on trust: [`estimate_effective_dpi`] measures the words
//! a recognition pass found against the height nominal body text has at the page's declared
//! resolution, and [`choose`] turns that measurement into a [`ResolutionPolicy`].
//! [`upscale_lanczos`] is the enlargement itself — a separable Lanczos-3 kernel, because
//! bilinear enlargement is a low-pass filter that softens the strokes it is meant to make
//! readable. Everything is plain Rust on the RGBA raster: no process, no new dependency.

use crate::{OcrImage, Word};

/// How a page's resolution is handled before recognition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResolutionPolicy {
    /// Read the page as rendered.
    #[default]
    Native,
    /// Enlarge the raster by `factor` before recognition, with [`upscale_lanczos`].
    UpscaleLanczos { factor: u8 },
}

/// Body text these estimates assume: a page of letter- or business-sized prose is set at about
/// this size, so its letters are what the page's true resolution shows through.
const NOMINAL_TEXT_PT: f64 = 12.0;

/// Below this estimate a page is enlarged: at 150 dpi nominal body text's x-height is about
/// 13 pixels, and the recognisers read shorter letters badly (see the module docs).
const LEGIBLE_DPI: f32 = 150.0;

/// The factor [`choose`] enlarges a low-resolution page by: the measurement that opened this
/// work reached 0.970 with three times the pixels.
const UPSCALE_FACTOR: u8 = 3;

/// The resolution `words` were read at, in dots per inch, from how tall their letters are.
///
/// Each word's box spans the ink of its own text — the x-height for lowercase letters, the cap
/// height for capitals, down to the descender for "gap" (crate::ink_extent) — so its height
/// over that share is the word's em in pixels, and the median over the words shrugs off a stray
/// box or a heading. The words must be the boxes a recognition pass found in the raster rendered
/// at `declared_dpi`: the em pixels are compared with the em nominal body text
/// ([`NOMINAL_TEXT_PT`]) would have at that resolution, and the ratio says how much finer the
/// page really is than it claims. (Both halves are compared as ems, so comparing x-heights, as
/// the name says, comes to the same dpi.)
///
/// A page with no measurable word (a blank page, boxes without a positive height) comes back
/// with its declared resolution, or with 300 when the declared value cannot be one at all (NaN,
/// infinite, zero or negative — the same default [`crate::preprocess`] takes).
pub fn estimate_effective_dpi(words: &[Word], declared_dpi: f32) -> f32 {
    let declared = f64::from(usable_dpi(declared_dpi));
    let Some(em_px) = median_px_per_em(words) else { return declared as f32 };
    // The em nominal body text has at the declared resolution, against the em these letters
    // show: the ratio says how many times finer the page really is than it claims. (The two
    // declared-dpi factors cancel — the estimate rests on the pixels alone — but the comparison
    // against the declared resolution is the method, and its value is what an unmeasurable page
    // comes back as.)
    let expected_em_px = NOMINAL_TEXT_PT * declared / 72.0;
    (declared * (em_px / expected_em_px)) as f32
}

/// The policy a page's words call for: [`ResolutionPolicy::UpscaleLanczos`] at
/// [`UPSCALE_FACTOR`] when they measure below [`LEGIBLE_DPI`], [`ResolutionPolicy::Native`]
/// otherwise. A page with no words to measure is judged on its declared resolution alone.
pub fn choose(words: &[Word], declared_dpi: f32) -> ResolutionPolicy {
    if estimate_effective_dpi(words, declared_dpi) < LEGIBLE_DPI {
        ResolutionPolicy::UpscaleLanczos { factor: UPSCALE_FACTOR }
    } else {
        ResolutionPolicy::Native
    }
}

/// Enlarge `image` by a whole `factor` with a separable Lanczos-3 kernel, one pass across and
/// one down: the six-tap weights are worked out once per destination column and row, and each
/// pass is then a weighted sum along one axis.
///
/// The kernel keeps the detail bilinear enlargement smooths away — three times the pixels read
/// as a finer scan, not a softer one — and its weights sum to one, so the page keeps its tone.
/// A factor under 2 hands the page back as it was, and so does a result past the raster caps
/// ([`crate::MAX_SIDE`], [`crate::MAX_PIXELS`]): the step is a no-op rather than an error, which
/// is why [`crate::preprocess`] refuses those sizes before calling it.
pub fn upscale_lanczos(image: &OcrImage, factor: u8) -> OcrImage {
    let factor = u32::from(factor);
    if factor < 2 {
        return image.clone();
    }
    let (w, h) = (image.width, image.height);
    let Some((nw, nh)) = capped_size(w, h, factor) else { return image.clone() };
    let (nw_px, w_px) = (nw as usize, w as usize);
    let columns: Vec<([f64; TAPS], i64)> = (0..nw_px).map(|x| taps_for(x, factor)).collect();
    let mut across = vec![0u8; nw_px * h as usize * 4];
    for (src_row, dst_row) in image.rgba.chunks_exact(w_px * 4).zip(across.chunks_exact_mut(nw_px * 4)) {
        for (x, px) in dst_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (weights, first) = &columns[x];
            for (ch, out) in px.iter_mut().enumerate() {
                *out = tap_sum(weights, *first, w_px as i64 - 1, |sx| src_row.get(sx as usize * 4 + ch).copied().unwrap_or(255));
            }
        }
    }
    let rows: Vec<([f64; TAPS], i64)> = (0..nh as usize).map(|y| taps_for(y, factor)).collect();
    let mut out = vec![255u8; nw_px * nh as usize * 4];
    for (y, dst_row) in out.chunks_exact_mut(nw_px * 4).enumerate() {
        let (weights, first) = &rows[y];
        for (x, px) in dst_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            for (ch, dst) in px.iter_mut().enumerate() {
                *dst = tap_sum(weights, *first, h as i64 - 1, |sy| across.get((sy as usize * nw_px + x) * 4 + ch).copied().unwrap_or(255));
            }
        }
    }
    OcrImage::new(nw, nh, out).unwrap_or_else(|_| image.clone())
}

/// The page's declared resolution, or 300 for a value that cannot be one (NaN, infinite, zero
/// or negative) — the default [`crate::preprocess`] takes for the same reason.
fn usable_dpi(declared_dpi: f32) -> f32 {
    if declared_dpi.is_finite() && declared_dpi >= 1.0 { declared_dpi } else { 300.0 }
}

/// The median em size in pixels of the words' letters, or `None` when no word carries one: a box
/// without a positive, finite height, or text whose ink spans no height at all.
fn median_px_per_em(words: &[Word]) -> Option<f64> {
    let mut ems: Vec<f64> = Vec::with_capacity(words.len());
    for word in words {
        let height = f64::from(word.rect[3]) - f64::from(word.rect[1]);
        let (top, bottom) = crate::ink_extent(&word.text);
        let share = top - bottom;
        if height.is_finite() && height > 0.0 && share > 0.0 {
            ems.push(height / share);
        }
    }
    ems.sort_by(f64::total_cmp);
    ems.get(ems.len() / 2).copied()
}

/// Lanczos-3's support: the kernel reaches three source pixels each way, so six taps.
const TAPS: usize = 6;

/// `w` x `h` enlarged by `factor`, or `None` past the raster caps the rest of the crate
/// enforces: the sides are checked against [`crate::MAX_SIDE`] and the area against
/// [`crate::MAX_PIXELS`] before any buffer is allocated (a 10 000-pixel side times 255 is two
/// and a half million pixels along one side alone).
fn capped_size(w: u32, h: u32, factor: u32) -> Option<(u32, u32)> {
    let (nw, nh) = (w.checked_mul(factor)?, h.checked_mul(factor)?);
    if nw > crate::MAX_SIDE || nh > crate::MAX_SIDE {
        return None;
    }
    let pixels = (nw as usize).checked_mul(nh as usize)?;
    if pixels > crate::MAX_PIXELS as usize { None } else { Some((nw, nh)) }
}

/// The Lanczos-3 taps for one destination coordinate along an axis enlarged by `factor`.
///
/// The destination pixel's centre maps back to `(dst + 0.5) / factor − 0.5` in the source; the
/// six source pixels around it are taken with [`lanczos3`] weights, and the weights are
/// normalised to sum to one. The outer two are near zero, and at the borders — where the taps
/// clamp to the edge sample — the normalisation is what keeps the edge from darkening.
fn taps_for(dst: usize, factor: u32) -> ([f64; TAPS], i64) {
    let centre = (dst as f64 + 0.5) / f64::from(factor) - 0.5;
    let first = centre.floor() as i64 - (TAPS as i64 / 2 - 1);
    let mut weights = [0.0f64; TAPS];
    let mut total = 0.0;
    for (k, weight) in weights.iter_mut().enumerate() {
        *weight = lanczos3(centre - (first + k as i64) as f64);
        total += *weight;
    }
    if total.abs() > 1e-9 {
        for weight in &mut weights {
            *weight /= total;
        }
    } else {
        // Unreachable for a whole factor of 2 or more (the inner taps always carry weight), but
        // a kernel that summed to nothing would read as black: take the nearest sample alone.
        weights = [0.0; TAPS];
        if let Some(weight) = weights.get_mut((centre.round() as i64 - first).clamp(0, TAPS as i64 - 1) as usize) {
            *weight = 1.0;
        }
    }
    (weights, first)
}

/// The Lanczos-3 kernel at `t`: `sinc(t)·sinc(t/3)` inside its three-pixel support, zero outside.
fn lanczos3(t: f64) -> f64 {
    const SUPPORT: f64 = 3.0;
    if t.abs() >= SUPPORT {
        return 0.0;
    }
    let x = std::f64::consts::PI * t;
    sinc(x) * sinc(x / SUPPORT)
}

/// The normalised sinc, `sin(t)/t` (callers pass `πt`), 1 at zero.
fn sinc(t: f64) -> f64 {
    if t.abs() < 1e-9 { 1.0 } else { t.sin() / t }
}

/// One blended 8-bit sample: `weights` applied to the samples at `first`, `first + 1`, …
/// clamped to `last` at both ends, rounded and kept inside the byte range.
fn tap_sum(weights: &[f64; TAPS], first: i64, last: i64, sample: impl Fn(i64) -> u8) -> u8 {
    let mut sum = 0.0;
    for (k, weight) in weights.iter().enumerate() {
        sum += weight * f64::from(sample((first + k as i64).clamp(0, last)));
    }
    sum.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution;

    fn white(w: u32, h: u32) -> OcrImage {
        OcrImage::new(w, h, vec![255; (w * h * 4) as usize]).unwrap()
    }

    fn fill(img: &mut OcrImage, x0: u32, y0: u32, w: u32, h: u32) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let o = ((y * img.width + x) * 4) as usize;
                if let Some(p) = img.rgba.get_mut(o..o + 3) {
                    p.copy_from_slice(&[0, 0, 0]);
                }
            }
        }
    }

    /// A page that looks like text: rows of `glyph_px`-tall block marks on white, whose x-height
    /// is therefore `glyph_px`. 612 x 792 is a letter page; at 72 dpi it is 612 x 792 pixels.
    fn synthetic_text_page(width: u32, height: u32, glyph_px: u32) -> OcrImage {
        let mut img = white(width, height);
        let glyph_px = glyph_px.max(1);
        let margin = glyph_px * 4;
        let mut y = margin;
        while y + glyph_px < height {
            let mut x = margin;
            while x + glyph_px < width {
                fill(&mut img, x, y, glyph_px, glyph_px);
                x += glyph_px * 2;
            }
            y += glyph_px * 2;
        }
        img
    }

    /// The words a recogniser reports for [`synthetic_text_page`]: `glyph_px` is the page's
    /// x-height, so each box is as tall as its own text's ink. The three kinds of text are
    /// lowercase without ascenders (0.523 em), a capital (0.718 em) and a descender (0.210 em
    /// below the baseline); no text mixes them, so each box's share is known here.
    fn words_of(glyph_px: u32, texts: &[&str]) -> Vec<Word> {
        let em = glyph_px as f32 / 0.523;
        texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let share = if text.chars().any(|c| "gjpqy".contains(c)) {
                    0.523 + 0.210
                } else if text.chars().any(char::is_uppercase) {
                    0.718
                } else {
                    0.523
                };
                let left = 16.0 + i as f32 * 40.0;
                Word::new(*text, [left, 16.0, left + 24.0, 16.0 + (em * share).round()], "ocrs")
            })
            .collect()
    }

    /// A page's worth of words at a `glyph_px` x-height, mixed as a line of prose is.
    fn ocr_words_fixture(glyph_px: u32) -> Vec<Word> {
        words_of(glyph_px, &["sun", "Sun", "gap", "one", "an", "sea", "Sun"])
    }

    /// A 72 dpi letter page whose letters are four pixels tall reads badly; the page is enlarged
    /// three times so the recogniser sees letters it can read.
    #[test]
    fn low_resolution_page_is_upscaled_before_recognition() {
        let image = synthetic_text_page(612, 792, 4);
        let words = ocr_words_fixture(4);
        let policy = resolution::choose(&words, 72.0);
        assert!(matches!(policy, ResolutionPolicy::UpscaleLanczos { factor: 3 }));
        let enlarged = resolution::upscale_lanczos(&image, 3);
        assert_eq!(enlarged.width(), 1836);
        assert_eq!(enlarged.height(), 2376);
    }

    /// A page whose letters are already the size nominal text has at its resolution is read as
    /// rendered: nothing is enlarged, so nothing is softened and no time is spent.
    #[test]
    fn a_normal_resolution_page_stays_native() {
        // 12 pt lowercase letters at 300 dpi are 12 x 0.523 x 300 / 72, about 26 pixels tall.
        let words = ocr_words_fixture(26);
        let estimate = estimate_effective_dpi(&words, 300.0);
        assert!(estimate >= LEGIBLE_DPI, "readable letters measure above the bar: {estimate}");
        assert!((estimate - 300.0).abs() < 60.0, "and close to the resolution they were read at: {estimate}");
        assert_eq!(resolution::choose(&words, 300.0), ResolutionPolicy::Native);
    }

    /// The estimate follows the letters, not the boxes they arrived in: capitals and descenders
    /// make taller boxes for the same text size, and measure the same resolution.
    #[test]
    fn capital_and_descender_words_measure_like_lowercase() {
        let lowercase = estimate_effective_dpi(&words_of(8, &["sun", "one"]), 200.0);
        let tall = estimate_effective_dpi(&words_of(8, &["Sun", "gap"]), 200.0);
        assert!((tall - lowercase).abs() < 2.0, "lowercase {lowercase}, capital and descender {tall}");
    }

    /// A page with nothing to measure is judged on what it claims, and a claim that cannot be a
    /// resolution is the same 300 the rest of the pipeline defaults to.
    #[test]
    fn an_unmeasurable_page_keeps_its_declared_resolution() {
        assert_eq!(estimate_effective_dpi(&[], 250.0), 250.0);
        assert_eq!(estimate_effective_dpi(&[], f32::NAN), 300.0);
        assert_eq!(estimate_effective_dpi(&[], 0.0), 300.0);
        assert_eq!(resolution::choose(&[], 300.0), ResolutionPolicy::Native);
        // A declaration below the bar is still a page too small to read, with nothing to measure.
        assert!(matches!(resolution::choose(&[], 72.0), ResolutionPolicy::UpscaleLanczos { .. }));
        // A box with no height is no measurement either.
        let flat = vec![Word::new("sun", [10.0, 10.0, 40.0, 10.0], "ocrs")];
        assert_eq!(estimate_effective_dpi(&flat, 80.0), 80.0);
    }

    /// Enlarging triples the raster, keeps the page's tone (the kernel's weights sum to one) and
    /// interpolates the strokes rather than replicating whole source pixels.
    #[test]
    fn upscaling_triples_the_raster_keeps_tone_and_smooths_edges() {
        let image = synthetic_text_page(120, 160, 4);
        let enlarged = upscale_lanczos(&image, 3);
        assert_eq!((enlarged.width(), enlarged.height()), (360, 480));
        let mean = |img: &OcrImage| {
            let sum: u64 = img.rgba.chunks_exact(4).map(|p| u64::from(p[0])).sum();
            sum as f64 / f64::from(img.width * img.height)
        };
        assert!((mean(&enlarged) - mean(&image)).abs() < 1.0, "the page keeps its tone: {} then {}", mean(&image), mean(&enlarged));
        // A block-replicating enlargement only ever produces paper and ink: Lanczos leaves
        // intermediate greys along every stroke.
        assert!(enlarged.rgba.chunks_exact(4).any(|p| (10..245).contains(&p[0])), "the strokes are interpolated, not replicated");
        // A factor under two has nothing to enlarge, and a request past the raster caps hands
        // the page back untouched rather than allocating the buffer.
        assert_eq!(upscale_lanczos(&image, 1).rgba, image.rgba);
        assert_eq!(upscale_lanczos(&image, 0).rgba, image.rgba);
        let large = OcrImage::new(3000, 3000, vec![255; 3000 * 3000 * 4]).unwrap();
        assert_eq!(upscale_lanczos(&large, 3).width(), 3000, "past MAX_PIXELS the page is unchanged");
    }
}
