//! Structural similarity (SSIM) of two renderings of a page: how alike they look, from 1 for
//! the same picture down toward 0 (or below, for an inverted one).
//!
//! The measure is Wang, Bovik, Sheikh and Simoncelli's (IEEE Transactions on Image Processing
//! 13(4), 2004) on BT.601 luma: an 11 × 11 Gaussian window (sigma 1.5), K1 = 0.01, K2 = 0.03
//! for 8-bit samples, averaged over every window wholly inside the image. An image narrower or
//! shorter than the window is scored as one window with equal weights.

use std::collections::VecDeque;

const WINDOW: usize = 11;
const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);

/// The SSIM of RGBA image `b` against RGBA image `a`, both `(pixels, width, height)`, scored on
/// luma (alpha is ignored). `None` when the sizes differ, either image is empty, or a buffer is
/// shorter than its size says.
pub fn structural_similarity(a: (&[u8], u32, u32), b: (&[u8], u32, u32)) -> Option<f64> {
    let ((pa, w, h), (pb, wb, hb)) = (a, b);
    if (w, h) != (wb, hb) {
        return None;
    }
    let (w, h) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let len = w.checked_mul(h)?.checked_mul(4)?;
    if len == 0 || pa.len() < len || pb.len() < len {
        return None;
    }
    let (la, lb) = (luma(pa.get(..len)?), luma(pb.get(..len)?));
    Some(if w < WINDOW || h < WINDOW { global_ssim(&la, &lb) } else { windowed_ssim(&la, &lb, w) })
}

/// BT.601 luma of each RGBA pixel.
fn luma(rgba: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgba.as_chunks::<4>();
    // 0 to 255 by construction.
    pixels.iter().map(|&[r, g, b, _]| (0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)).round() as u8).collect()
}

/// The normalised 11-tap Gaussian, sigma 1.5.
fn kernel() -> [f64; WINDOW] {
    let mut k = [0.0; WINDOW];
    for (i, v) in k.iter_mut().enumerate() {
        let d = i as f64 - 5.0;
        *v = (-(d * d) / (2.0 * 1.5 * 1.5)).exp();
    }
    let sum: f64 = k.iter().sum();
    k.map(|v| v / sum)
}

/// Windowed means of x, y, x², y² and xy.
type Moments = [f64; 5];

fn index_of([mx, my, xx, yy, xy]: Moments) -> f64 {
    let (vx, vy, cxy) = (xx - mx * mx, yy - my * my, xy - mx * my);
    ((2.0 * mx * my + C1) * (2.0 * cxy + C2)) / ((mx * mx + my * my + C1) * (vx + vy + C2))
}

fn add_weighted(sum: &mut Moments, k: f64, (p, q): (f64, f64)) {
    for (s, v) in sum.iter_mut().zip([p, q, p * p, q * q, p * q]) {
        *s += k * v;
    }
}

/// The horizontal moments of every window along one row.
fn row_moments(k: &[f64; WINDOW], a: &[u8], b: &[u8]) -> Vec<Moments> {
    a.windows(WINDOW)
        .zip(b.windows(WINDOW))
        .map(|(wa, wb)| {
            let mut m = [0.0; 5];
            for ((kt, p), q) in k.iter().zip(wa).zip(wb) {
                add_weighted(&mut m, *kt, (f64::from(*p), f64::from(*q)));
            }
            m
        })
        .collect()
}

/// The mean index over every full window. The Gaussian is separable, so rows are filtered once
/// and the last 11 are kept: memory is 11 rows of moments, not the image.
fn windowed_ssim(a: &[u8], b: &[u8], w: usize) -> f64 {
    let k = kernel();
    let mut lines = a.chunks_exact(w).zip(b.chunks_exact(w)).map(|(ra, rb)| row_moments(&k, ra, rb));
    let mut rows: VecDeque<Vec<Moments>> = lines.by_ref().take(WINDOW).collect();
    let (mut total, mut count) = (0.0, 0usize);
    loop {
        let mut column = vec![[0.0; 5]; w.saturating_sub(WINDOW - 1)];
        for (row, kt) in rows.iter().zip(&k) {
            for (sum, m) in column.iter_mut().zip(row) {
                for (s, v) in sum.iter_mut().zip(m) {
                    *s += kt * v;
                }
            }
        }
        total += column.iter().map(|m| index_of(*m)).sum::<f64>();
        count += column.len();
        let Some(next) = lines.next() else { break };
        rows.pop_front();
        rows.push_back(next);
    }
    total / count.max(1) as f64
}

/// One equal-weight window over the whole image.
fn global_ssim(a: &[u8], b: &[u8]) -> f64 {
    let mut m = [0.0; 5];
    for (p, q) in a.iter().zip(b) {
        add_weighted(&mut m, 1.0, (f64::from(*p), f64::from(*q)));
    }
    let n = a.len().max(1) as f64;
    index_of(m.map(|v| v / n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` × `h` RGBA picture with detail everywhere, each pixel moved by `noise(i)`.
    fn picture(w: u32, h: u32, noise: impl Fn(usize) -> i16) -> Vec<u8> {
        (0..(w * h) as usize)
            .flat_map(|i| {
                let (x, y) = (i % w as usize, i / w as usize);
                let v = ((x * 7 + y * 13) % 200) as i16 + 28 + noise(i);
                let v = v.clamp(0, 255) as u8;
                [v, v, v, 255]
            })
            .collect()
    }

    #[test]
    fn identical_pictures_score_highest_and_more_damage_scores_lower() {
        for (w, h) in [(40, 30), (5, 7)] {
            let a = picture(w, h, |_| 0);
            let same = structural_similarity((&a, w, h), (&a, w, h)).unwrap_or(0.0);
            let flip = |amp: i16| move |i: usize| if i.is_multiple_of(2) { amp } else { -amp };
            let light = picture(w, h, flip(4));
            let heavy = picture(w, h, flip(40));
            let light = structural_similarity((&a, w, h), (&light, w, h)).unwrap_or(1.0);
            let heavy = structural_similarity((&a, w, h), (&heavy, w, h)).unwrap_or(1.0);
            assert!(same > light && light > heavy, "{w}x{h}: {same} {light} {heavy}");
        }
    }

    #[test]
    fn pictures_that_cannot_be_compared_have_no_score() {
        let a = picture(20, 20, |_| 0);
        assert_eq!(structural_similarity((&a, 20, 20), (&a, 20, 19)), None, "sizes differ");
        assert_eq!(structural_similarity((&a, 20, 20), (&a[..40], 20, 20)), None, "buffer short");
        assert_eq!(structural_similarity((&[], 0, 0), (&[], 0, 0)), None, "empty");
        assert_eq!(structural_similarity((&a, u32::MAX, u32::MAX), (&a, u32::MAX, u32::MAX)), None, "absurd size");
    }
}
