//! Halftone (dithered) 1-bit scans: a faint grey page re-sampled as random or patterned dots,
//! which reads as a grey texture with broken, ghostly text. The dots carry tones only when many
//! are averaged, and thin strokes are as small as the dots, so averaging at full size blurs the
//! text. [`restore`] instead smooths the dots a little (a Gaussian) and keeps the result at half
//! the size, where the tones are firm and the strokes stay sharp, then lifts the paper to white and
//! darkens the ink against the page's own background. A second stage, [`sharpen_text`], brings the
//! page back to its full size with bolder, smoother strokes, which read far better than the thin,
//! ragged ones the dots leave.

/// How a restored halftone page looks, each 0–100 with 50 the default: how heavy the strokes are
/// (`weight`), how dark the ink gets against the paper (`contrast`), and how much of the paper's
/// speckle is cleared to white (`cleanup`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tuning {
    pub weight: u8,
    pub contrast: u8,
    pub cleanup: u8,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { weight: 50, contrast: 50, cleanup: 50 }
    }
}

impl Tuning {
    /// The stroke thickening, the paper level above which it is white, and the ink curve.
    fn values(self) -> (f32, f32, f32) {
        let share = |v: u8| f32::from(v.min(100)) / 100.0;
        (0.7 * share(self.weight), 0.96 - 0.16 * share(self.cleanup), 1.2 + 2.0 * share(self.contrast))
    }
}

/// The window's reach on each side of [`is_halftone`]'s test: 5 × 5 pixels, a few dots.
const R: usize = 2;
/// A pixel is "mid-tone" when its window holds between these shares (percent) of white pixels.
const MID: (u32, u32) = (6, 94);
/// A page is a halftone when at least this share (percent) of it is mid-tone.
const SHARE: usize = 35;
/// The Gaussian's σ, in source pixels, and how many taps it reaches each side.
const SIGMA: f32 = 1.4;
const TAPS: isize = 4;
/// Relative to the local paper, a tone below `LO` is black and above `HI` white, with a gentle
/// curve (`GAMMA`) between.
const LO: f32 = 0.84;
const HI: f32 = 0.96;
const GAMMA: f32 = 1.3;
/// The paper's tone is the mean of the half-size pixels around that are not ink: within this
/// reach, and not darker than `INK` of the plain mean there.
const PAPER_REACH: usize = 20;
const INK: f32 = 0.75;
/// The second stage: the strokes are thickened by this share of a 3 x 3 minimum, smoothed by a
/// Gaussian of `SMOOTH_SIGMA`, then the paper is cleared and the ink darkened, as far as the
/// [`Tuning`] says.
/// Tones lighter than `THIN_BELOW` are not thickened, and ones `THIN_RANGE` darker fully are.
const THIN_BELOW: f32 = 220.0;
const THIN_RANGE: f32 = 60.0;
const SMOOTH_SIGMA: f32 = 0.8;
/// Pages with more pixels than this are left as they are (a poster at 300 dpi).
pub const MAX_PIXELS: u64 = 40_000_000;

/// The number of white pixels (255) in the 5 × 5 window around each pixel, edges repeated.
fn white_counts(px: &[u8], w: usize, h: usize) -> Vec<u8> {
    let at = |x: isize, max: usize| x.clamp(0, max as isize - 1) as usize;
    let mut across = vec![0u8; px.len()];
    for (y, row) in px.chunks_exact(w).enumerate() {
        for x in 0..w {
            let n = (-(R as isize)..=R as isize).filter(|d| row.get(at(x as isize + d, w)) == Some(&255)).count();
            if let Some(o) = across.get_mut(y * w + x) {
                *o = n as u8;
            }
        }
    }
    let mut out = vec![0u8; px.len()];
    for y in 0..h {
        for x in 0..w {
            let n: u32 = (-(R as isize)..=R as isize).map(|d| u32::from(across.get(at(y as isize + d, h) * w + x).copied().unwrap_or(0))).sum();
            if let Some(o) = out.get_mut(y * w + x) {
                *o = n as u8; // at most 25
            }
        }
    }
    out
}

/// Whether `px` (0 black, 255 white, `w` × `h`) is a halftone: most of it is neither solid paper
/// nor solid ink. Clean black-on-white text is only mid-tone along its edges.
pub fn is_halftone(px: &[u8], w: u32, h: u32) -> bool {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || w.checked_mul(h) != Some(px.len()) {
        return false;
    }
    let side = 2 * R + 1;
    let area = (side * side) as u32;
    let mid = white_counts(px, w, h)
        .iter()
        .filter(|&&n| {
            let share = u32::from(n) * 100 / area;
            (MID.0..=MID.1).contains(&share)
        })
        .count();
    mid * 100 >= px.len() * SHARE
}

fn transpose(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; v.len()];
    for y in 0..h {
        for x in 0..w {
            if let (Some(o), Some(i)) = (out.get_mut(x * h + y), v.get(y * w + x)) {
                *o = *i;
            }
        }
    }
    out
}

/// A running mean over `2 * reach + 1` samples of each row (edges repeated).
fn box_rows(v: &[f32], w: usize, reach: usize) -> Vec<f32> {
    let mut out = vec![0.0; v.len()];
    let n = (2 * reach + 1) as f32;
    let r = reach as isize;
    for (src, dst) in v.chunks_exact(w).zip(out.chunks_exact_mut(w)) {
        let at = |i: isize| src.get(i.clamp(0, w as isize - 1) as usize).copied().unwrap_or(0.0);
        let mut sum: f32 = (-r..=r).map(at).sum();
        for (x, o) in dst.iter_mut().enumerate() {
            *o = sum / n;
            sum += at(x as isize + r + 1) - at(x as isize - r);
        }
    }
    out
}

/// `v` (`w` × `h`) smoothed by a box of `2 * reach + 1` each way, `times` over (about a Gaussian).
fn box_blur(mut v: Vec<f32>, w: usize, h: usize, reach: usize, times: usize) -> Vec<f32> {
    for _ in 0..times {
        v = transpose(&box_rows(&v, w, reach), w, h);
        v = transpose(&box_rows(&v, h, reach), h, w);
    }
    v
}

/// The Gaussian kernel of `sigma` reaching `taps` each side, normalised.
fn kernel(sigma: f32, taps: isize) -> Vec<f32> {
    let k: Vec<f32> = (-taps..=taps).map(|d| (-((d * d) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.into_iter().map(|v| v / sum).collect()
}

/// `v` (`w` wide) convolved along its rows with `k`, edges repeated.
fn convolve_rows(v: &[f32], w: usize, k: &[f32]) -> Vec<f32> {
    let taps = (k.len() / 2) as isize;
    let mut out = vec![0.0; v.len()];
    for (src, dst) in v.chunks_exact(w).zip(out.chunks_exact_mut(w)) {
        for (x, o) in dst.iter_mut().enumerate() {
            *o = k
                .iter()
                .enumerate()
                .map(|(i, c)| c * src.get((x as isize + i as isize - taps).clamp(0, w as isize - 1) as usize).copied().unwrap_or(0.0))
                .sum();
        }
    }
    out
}

/// The first stage: a halftone scan (`px`: 0 black, 255 white, `w` × `h`) as its tones at half the
/// size (rounded up), 8-bit grey, as `(width, height, samples)`. `None` if the buffer doesn't match
/// the size or the page is past [`MAX_PIXELS`].
fn half_size_tones(px: &[u8], w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let (wu, hu) = (w as usize, h as usize);
    if wu == 0 || hu == 0 || wu.checked_mul(hu)? != px.len() || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    // Smooth the dots: a Gaussian along each axis.
    let k = kernel(SIGMA, TAPS);
    let src: Vec<f32> = px.iter().map(|&v| f32::from(v)).collect();
    let rows = convolve_rows(&src, wu, &k);
    drop(src);
    let cols = convolve_rows(&transpose(&rows, wu, hu), hu, &k);
    drop(rows);
    let smooth = transpose(&cols, hu, wu);
    drop(cols);
    // Half size: each pixel the mean of its 2 × 2 block (edges repeated).
    let (ow, oh) = (wu.div_ceil(2), hu.div_ceil(2));
    let at = |x: usize, y: usize| smooth.get(y.min(hu - 1) * wu + x.min(wu - 1)).copied().unwrap_or(0.0);
    let mut half = Vec::with_capacity(ow * oh);
    for y in 0..oh {
        for x in 0..ow {
            half.push((at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1)) / 4.0);
        }
    }
    drop(smooth);
    // The paper's own tone: the mean around, leaving out what is darker (ink), so it is the same
    // whatever the density of the dots.
    let mean = box_blur(half.clone(), ow, oh, PAPER_REACH, 2);
    let is_paper: Vec<f32> = half.iter().zip(&mean).map(|(v, m)| f32::from(u8::from(*v >= m * INK))).collect();
    let weighted: Vec<f32> = half.iter().zip(&is_paper).map(|(v, p)| v * p).collect();
    let (weighted, share) = (box_blur(weighted, ow, oh, PAPER_REACH, 2), box_blur(is_paper, ow, oh, PAPER_REACH, 2));
    let paper: Vec<f32> = weighted.iter().zip(&share).zip(&mean).map(|((w, s), m)| if *s > 0.05 { w / s } else { *m }).collect();
    let out = half
        .iter()
        .zip(&paper)
        .map(|(&v, &p)| {
            let t = ((v / p.max(1.0)).clamp(0.0, 1.0) - LO) / (HI - LO);
            (t.clamp(0.0, 1.0).powf(GAMMA) * 255.0).round() as u8
        })
        .collect();
    Some((u32::try_from(ow).ok()?, u32::try_from(oh).ok()?, out))
}

/// The value of `v` (`w` × `h`) at `(x, y)`, edges repeated.
fn sample(v: &[f32], w: usize, h: usize, x: isize, y: isize) -> f32 {
    v.get(y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize).copied().unwrap_or(0.0)
}

/// The Catmull-Rom weights for a point `t` (0–1) of the way between the middle two of four samples.
fn cubic(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [(-t3 + 2.0 * t2 - t) / 2.0, (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0, (-3.0 * t3 + 4.0 * t2 + t) / 2.0, (t3 - t2) / 2.0]
}

/// `v` (`w` × `h`) doubled in size, smoothly (bicubic), cut to `ow` × `oh`.
fn upscale(v: &[f32], w: usize, h: usize, ow: usize, oh: usize) -> Vec<f32> {
    // An output pixel sits at (x + 0.5) / 2 - 0.5 in the source: a quarter before a source pixel's
    // centre when x is even and a quarter after it when odd, so there are two fixed sets of weights.
    let weights = [cubic(0.75), cubic(0.25)];
    let mut out = Vec::with_capacity(ow * oh);
    for y in 0..oh {
        let y0 = (y / 2) as isize - if y % 2 == 0 { 2 } else { 1 };
        let wy = &weights[y % 2];
        for x in 0..ow {
            let x0 = (x / 2) as isize - if x % 2 == 0 { 2 } else { 1 };
            let wx = &weights[x % 2];
            let mut sum = 0.0;
            for (j, wj) in wy.iter().enumerate() {
                for (i, wi) in wx.iter().enumerate() {
                    sum += wj * wi * sample(v, w, h, x0 + i as isize, y0 + j as isize);
                }
            }
            out.push(sum);
        }
    }
    out
}

/// The darkest of the 3 samples around each one of each row.
fn min_rows(v: &[f32], w: usize) -> Vec<f32> {
    let mut out = vec![0.0; v.len()];
    for (src, dst) in v.chunks_exact(w).zip(out.chunks_exact_mut(w)) {
        for (x, o) in dst.iter_mut().enumerate() {
            let (a, b) = (x.saturating_sub(1), (x + 2).min(w));
            *o = src.get(a..b).map_or(0.0, |s| s.iter().copied().fold(f32::MAX, f32::min));
        }
    }
    out
}

/// The second stage: half-size tones (`ow` × `oh`) back to `w` × `h`, smooth, with bolder strokes
/// on clear paper.
fn sharpen_text(tones: &[u8], ow: usize, oh: usize, w: usize, h: usize, tuning: Tuning) -> Vec<u8> {
    let (thicken, clear, darken) = tuning.values();
    let small: Vec<f32> = tones.iter().map(|&v| f32::from(v)).collect();
    let big = upscale(&small, ow, oh, w, h);
    drop(small);
    let thin = transpose(&min_rows(&transpose(&min_rows(&big, w), w, h), h), h, w);
    // Only ink is thickened: the paper's own speckle stays as it is, for the clearing below.
    let bold: Vec<f32> = big.iter().zip(&thin).map(|(a, m)| a + thicken * ((THIN_BELOW - a) / THIN_RANGE).clamp(0.0, 1.0) * (m - a)).collect();
    drop((big, thin));
    let k = kernel(SMOOTH_SIGMA, 3);
    let rows = convolve_rows(&bold, w, &k);
    drop(bold);
    let smooth = transpose(&convolve_rows(&transpose(&rows, w, h), h, &k), h, w);
    smooth.iter().map(|v| ((v / (255.0 * clear)).clamp(0.0, 1.0).powf(darken) * 255.0).round() as u8).collect()
}

/// A halftone scan (`px`: 0 black, 255 white, `w` × `h`) restored: the clean grey picture it was
/// made from, the same size, 8-bit, with smooth, bold text. `None` if the buffer doesn't match
/// the size or the page is past [`MAX_PIXELS`].
pub fn restore(px: &[u8], w: u32, h: u32, tuning: Tuning) -> Option<Vec<u8>> {
    let (ow, oh, tones) = half_size_tones(px, w, h)?;
    Some(sharpen_text(&tones, ow as usize, oh as usize, w as usize, h as usize, tuning))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey of `black` tenths ink, dithered as a regular pattern.
    fn dithered(w: usize, h: usize, black: usize) -> Vec<u8> {
        (0..w * h).map(|i| if ((i % w) * 7 + (i / w) * 3) % 10 < black { 0 } else { 255 }).collect()
    }

    #[test]
    fn dots_are_a_halftone_and_clean_text_is_not() {
        assert!(is_halftone(&dithered(60, 60, 2), 60, 60), "20 % ink, evenly dotted");
        let mut text = vec![255u8; 60 * 60];
        for y in 10..50 {
            for x in 20..23 {
                text[y * 60 + x] = 0; // one thick vertical stroke
            }
        }
        assert!(!is_halftone(&text, 60, 60));
        assert!(!is_halftone(&[255; 3600], 60, 60), "blank paper");
        assert!(!is_halftone(&[255; 5], 60, 60), "a buffer of the wrong size");
    }

    #[test]
    fn restore_whitens_the_paper_and_keeps_a_dense_stroke_dark_at_full_size() {
        // 100 x 80: 10 % scattered dots as paper, a 4-wide stroke of 60 % dots as ink.
        let (w, h) = (100usize, 80usize);
        let px: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let ink = (48..52).contains(&x) && (20..60).contains(&y);
                let n = (x * 37 + y * 101 + x * y * 13) % 10;
                if n < if ink { 6 } else { 1 } { 0 } else { 255 }
            })
            .collect();
        let out = restore(&px, w as u32, h as u32, Tuning::default()).unwrap();
        assert_eq!(out.len(), w * h);
        assert!(out[10 * w + 10] >= 235, "paper: {}", out[10 * w + 10]);
        assert!(out[40 * w + 50] <= 110, "ink: {}", out[40 * w + 50]);
        let t = Tuning::default();
        assert!(restore(&px, 7, 7, t).is_none() && restore(&[], 0, 0, t).is_none(), "wrong sizes are refused");
        assert_eq!(restore(&[255; 9], 3, 3, t).unwrap(), vec![255; 9], "odd, tiny sizes work");
        // The sliders move the ink: heavier and darker strokes, lighter ones.
        let ink = |t: Tuning| restore(&px, w as u32, h as u32, t).unwrap().iter().map(|v| u32::from(255 - v)).sum::<u32>();
        let low = Tuning { weight: 0, contrast: 0, cleanup: 50 };
        let high = Tuning { weight: 100, contrast: 100, cleanup: 50 };
        assert!(ink(high) > ink(low), "{} vs {}", ink(high), ink(low));
        assert_eq!(Tuning::default().values(), (0.35, 0.88, 2.2));
    }

    #[test]
    fn a_smooth_upscale_keeps_a_flat_tone_flat() {
        let up = upscale(&[100.0; 12], 4, 3, 7, 5);
        assert!(up.iter().all(|v| (v - 100.0).abs() < 0.01), "{up:?}");
    }
}
