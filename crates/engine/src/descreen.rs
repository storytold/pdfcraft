//! Halftone (dithered) 1-bit scans: a faint grey page re-sampled as random or patterned dots,
//! which reads as a grey texture with broken, ghostly text. The dots carry tones only when many
//! are averaged, and thin strokes are as small as the dots, so averaging at full size blurs the
//! text. [`restore`] instead smooths the dots a little (a Gaussian) and keeps the result at half
//! the size, where the tones are firm and the strokes stay sharp, then lifts the paper to white and
//! darkens the ink against the page's own background. A second stage, [`sharpen_text`], brings the
//! page back to its full size with bolder, smoother strokes, which read far better than the thin,
//! ragged ones the dots leave.
//!
//! Only a page that is such a texture all over is restored ([`is_halftone`]). Crisp black and
//! white text, however small or dense, is none; nor is a page of crisp text beside a dithered
//! photograph, or of solid writing on speckled paper: restoring those would blur ink that was
//! sharp. The test is for fine dither (dots a pixel or two across, as error diffusion and
//! dispersed ordered dither make); a coarse clustered-dot screen is not recognised.

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

/// [`is_halftone`] looks at the page in blocks of this many pixels a side.
const BLOCK: usize = 16;
/// A block is dither when its lone pixels (ones unlike all four neighbours, of either colour) are
/// at least this share (percent) of it, and at least half of its rarer colour. Dither puts its
/// dots apart, so nearly all of them are lone; the strokes of text, even a pixel wide, are not.
const LONE: u64 = 3;
/// A page is a halftone when at least this share (percent) of its blocks are dither …
const COVER: u64 = 50;
/// … and no more than this share (percent) of its black is crisp strokes: black in a straight run
/// at least `LONG` pixels long that is at most `THICK` across the other way (wider is a blot or a
/// scanner's dark border, not a stroke). A dithered page has next to none, while sharp text, ruled
/// lines and handwriting are mostly that.
const CRISP: u64 = 15;
const LONG: usize = 6;
const THICK: usize = 12;
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
/// Pages with more pixels than this are left as they are (a poster at 300 dpi). A restore holds
/// three buffers of floats the size of the page at its peak, about 13 bytes a pixel with the page
/// itself: 520 MB at this cap. A browser build has a 32-bit address space and cannot recover from
/// an allocation that fails, so its cap is lower (about 210 MB).
pub const MAX_PIXELS: u64 = if cfg!(target_arch = "wasm32") { 16_000_000 } else { 40_000_000 };

/// What [`is_halftone`] goes by: the share (percent) of the page's blocks that are dither, and the
/// share (percent) of its black that is crisp strokes. `None` for a page too small to have a
/// block, a buffer that doesn't match the size, or a page past [`MAX_PIXELS`].
fn texture(px: &[u8], w: u32, h: u32) -> Option<(u64, u64)> {
    let (w, h) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let (across, down) = (w / BLOCK, h / BLOCK);
    if across == 0 || down == 0 || w.checked_mul(h)? != px.len() || px.len() as u64 > MAX_PIXELS {
        return None;
    }
    let black = |v: u8| v < 128;
    let rows: Vec<&[u8]> = px.chunks_exact(w).collect();
    // The length of the vertical run of black each black pixel lies in (255 for longer ones):
    // counted downwards, then the run's last count taken back up it.
    let mut tall = vec![0u8; px.len()];
    for (y, row) in rows.iter().enumerate() {
        for (x, &v) in row.iter().enumerate() {
            if black(v) {
                let above = if y > 0 { tall.get((y - 1) * w + x).copied().unwrap_or(0) } else { 0 };
                if let Some(t) = tall.get_mut(y * w + x) {
                    *t = above.saturating_add(1);
                }
            }
        }
    }
    for y in (0..h.saturating_sub(1)).rev() {
        for x in 0..w {
            let below = tall.get((y + 1) * w + x).copied().unwrap_or(0);
            if let Some(t) = tall.get_mut(y * w + x)
                && *t > 0
                && below > 0
            {
                *t = below;
            }
        }
    }
    let (mut dither, mut crisp, mut ink) = (0u64, 0u64, 0u64);
    // The black and the lone pixels of each block of the row of blocks being read.
    let mut blocks = vec![(0u32, 0u32); across];
    for (y, row) in rows.iter().enumerate() {
        let (above, below) = (y.checked_sub(1).and_then(|y| rows.get(y)), rows.get(y + 1));
        let mut x = 0;
        while let Some(&first) = row.get(x) {
            // One horizontal run of a colour: `x..end`.
            let is_black = black(first);
            let end = row.get(x..).and_then(|rest| rest.iter().position(|&v| black(v) != is_black)).map_or(w, |n| x + n);
            let wide = end - x;
            if is_black {
                ink += wide as u64;
                for i in x..end {
                    let high = usize::from(tall.get(y * w + i).copied().unwrap_or(0));
                    crisp += u64::from(wide.max(high) >= LONG && wide.min(high) <= THICK);
                    if let Some(b) = blocks.get_mut(i / BLOCK) {
                        b.0 += 1;
                    }
                }
            }
            // A lone pixel has the other colour on all four sides (so it is not at an edge).
            let other = |row: Option<&&[u8]>| row.and_then(|r| r.get(x)).is_some_and(|&v| black(v) != is_black);
            if wide == 1
                && x > 0
                && end < w
                && other(above)
                && other(below)
                && let Some(b) = blocks.get_mut(x / BLOCK)
            {
                b.1 += 1;
            }
            x = end;
        }
        // The last row of a row of blocks: count its dither blocks and start the next.
        if y % BLOCK == BLOCK - 1 && y / BLOCK < down {
            let area = (BLOCK * BLOCK) as u64;
            for (dark, lone) in blocks.iter_mut().map(std::mem::take) {
                let (dark, lone) = (u64::from(dark), u64::from(lone));
                let rarer = dark.min(area.saturating_sub(dark));
                dither += u64::from(lone * 100 >= LONE * area && lone * 2 >= rarer);
            }
        }
    }
    let all = (across as u64).checked_mul(down as u64)?;
    Some((dither * 100 / all, crisp * 100 / ink.max(1)))
}

/// Whether `px` (0 black, 255 white, `w` × `h`) is a halftone: a page that is dither texture over
/// most of itself, with next to no crisp strokes in it. Sharp text is not, at any size or
/// density, and neither is a page that mixes sharp ink with a dithered picture or speckled paper,
/// which a restore would blur. A page past [`MAX_PIXELS`] is not looked at.
pub fn is_halftone(px: &[u8], w: u32, h: u32) -> bool {
    texture(px, w, h).is_some_and(|(dither, crisp)| dither >= COVER && crisp <= CRISP)
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
    // Each buffer is dropped as soon as the next is made of it: two are alive at a time.
    let rows = convolve_rows(&src, wu, &k);
    drop(src);
    let turned = transpose(&rows, wu, hu);
    drop(rows);
    let cols = convolve_rows(&turned, hu, &k);
    drop(turned);
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

/// The darkest of the 3 samples above, at and below each one (`v` is `w` wide).
fn min_cols(v: &[f32], w: usize) -> Vec<f32> {
    let rows: Vec<&[f32]> = v.chunks_exact(w).collect();
    let mut out = vec![0.0; v.len()];
    for (y, dst) in out.chunks_exact_mut(w).enumerate() {
        let around = rows.get(y.saturating_sub(1)..(y + 2).min(rows.len())).unwrap_or(&[]);
        for (x, o) in dst.iter_mut().enumerate() {
            *o = around.iter().filter_map(|r| r.get(x)).copied().fold(f32::MAX, f32::min);
        }
    }
    out
}

/// The second stage: half-size tones (`ow` × `oh`) back to `w` × `h`, smooth, with bolder strokes
/// on clear paper.
fn sharpen_text(tones: &[u8], ow: usize, oh: usize, w: usize, h: usize, tuning: Tuning) -> Vec<u8> {
    let (thicken, clear, darken) = tuning.values();
    let small: Vec<f32> = tones.iter().map(|&v| f32::from(v)).collect();
    // No more than three buffers of the page's size are alive at a time: each is dropped, or
    // written over, as soon as the next is made of it.
    let mut bold = upscale(&small, ow, oh, w, h);
    drop(small);
    let across = min_rows(&bold, w);
    let thin = min_cols(&across, w);
    drop(across);
    // Only ink is thickened: the paper's own speckle stays as it is, for the clearing below.
    for (a, m) in bold.iter_mut().zip(&thin) {
        *a += thicken * ((THIN_BELOW - *a) / THIN_RANGE).clamp(0.0, 1.0) * (m - *a);
    }
    drop(thin);
    let k = kernel(SMOOTH_SIGMA, 3);
    let rows = convolve_rows(&bold, w, &k);
    drop(bold);
    let turned = transpose(&rows, w, h);
    drop(rows);
    let cols = convolve_rows(&turned, h, &k);
    drop(turned);
    let smooth = transpose(&cols, h, w);
    drop(cols);
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

    /// A page as greys (0 black to 1 white).
    struct Page {
        w: usize,
        h: usize,
        grey: Vec<f32>,
    }

    impl Page {
        fn new(w: usize, h: usize, paper: f32) -> Self {
            Page { w, h, grey: vec![paper; w * h] }
        }

        fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, v: f32) {
            for yy in y..(y + h).min(self.h) {
                for xx in x..(x + w).min(self.w) {
                    self.grey[yy * self.w + xx] = v;
                }
            }
        }

        /// Lines of capital letters in a 5 x 7 pixel font, each pixel `scale` across, filling the
        /// page inside `margin`: dense text with one-pixel strokes and diagonals at `scale` 1.
        fn text(&mut self, scale: usize, margin: usize, ink: f32) {
            const GLYPHS: [[u8; 7]; 10] = [
                [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11], // A
                [0x1f, 0x10, 0x1e, 0x10, 0x10, 0x10, 0x1f], // E
                [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11], // K
                [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11], // M
                [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11], // N
                [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e], // O
                [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e], // S
                [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // T
                [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11], // X
                [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f], // Z
            ];
            let (cw, ch) = (6 * scale, 9 * scale);
            let (cols, rows) = ((self.w - 2 * margin) / cw, (self.h - 2 * margin) / ch);
            for r in 0..rows {
                for c in 0..cols {
                    // A space every sixth letter.
                    if (r * cols + c) % 6 == 5 {
                        continue;
                    }
                    let glyph = GLYPHS[(r * 7 + c * 3) % GLYPHS.len()];
                    for (gy, bits) in glyph.iter().enumerate() {
                        for gx in (0..5).filter(|gx| bits & (0x10 >> gx) != 0) {
                            self.fill(margin + c * cw + gx * scale, margin + r * ch + gy * scale, scale, scale, ink);
                        }
                    }
                }
            }
        }

        /// The page as crisp black and white.
        fn crisp(&self) -> Vec<u8> {
            self.grey.iter().map(|&v| if v < 0.5 { 0 } else { 255 }).collect()
        }

        /// The page dithered to black and white by error diffusion (Floyd and Steinberg), as a
        /// scanner or a fax machine makes a halftone of greys.
        fn dithered(&self) -> Vec<u8> {
            let mut v = self.grey.clone();
            let mut out = vec![0u8; v.len()];
            for y in 0..self.h {
                for x in 0..self.w {
                    let i = y * self.w + x;
                    let white = v[i] >= 0.5;
                    out[i] = if white { 255 } else { 0 };
                    let e = v[i] - f32::from(u8::from(white));
                    for (dx, dy, share) in [(1isize, 0usize, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)] {
                        let (nx, ny) = (x as isize + dx, y + dy);
                        if nx >= 0 && (nx as usize) < self.w && ny < self.h {
                            v[ny * self.w + nx as usize] += e * share / 16.0;
                        }
                    }
                }
            }
            out
        }
    }

    fn halftone(px: &[u8], page: &Page) -> bool {
        is_halftone(px, page.w as u32, page.h as u32)
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
        assert!(!is_halftone(&[0; 3600], 60, 60), "all black");
        assert!(!is_halftone(&[255; 5], 60, 60), "a buffer of the wrong size");
        assert!(!is_halftone(&dithered(15, 40, 2), 15, 40), "too small to judge");
        assert_eq!(texture(&dithered(64, 64, 2), 64, 64), Some((100, 0)), "all dither, no strokes");
    }

    /// A faint page scanned to dots is a halftone, text and paper alike: at any strength of the
    /// paper's grey and the ink's, and whether the dots are diffused or patterned.
    #[test]
    fn a_faint_page_dithered_to_dots_is_a_halftone() {
        for (paper, ink) in [(0.8, 0.45), (0.9, 0.6), (0.72, 0.3)] {
            let mut page = Page::new(320, 400, paper);
            page.text(3, 24, ink);
            let px = page.dithered();
            assert!(halftone(&px, &page), "paper {paper}, ink {ink}: {:?}", texture(&px, 320, 400));
        }
        // A dispersed ordered dither (a 4 x 4 Bayer matrix) of grey paper.
        const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let px: Vec<u8> = (0..160 * 160).map(|i| if 12 > BAYER[(i / 160) % 4][(i % 160) % 4] { 255 } else { 0 }).collect();
        assert!(is_halftone(&px, 160, 160), "{:?}", texture(&px, 160, 160));
    }

    /// #6812 review: crisp black and white text is no halftone, however small or dense. At fax
    /// resolutions most of such a page is "neither solid paper nor solid ink", which is what the
    /// first test went by, and the text was blurred into blobs.
    #[test]
    fn crisp_text_is_no_halftone_at_any_size_or_density() {
        // One pixel a font pixel is about 72 dpi for 10 pt text, then 144, 216 and 288 dpi.
        for scale in 1..=4 {
            // With margins, and cropped to the text.
            for margin in [36 * scale, 0] {
                let mut page = Page::new(300 * scale, 380 * scale, 1.0);
                page.text(scale, margin, 0.0);
                let px = page.crisp();
                let got = texture(&px, page.w as u32, page.h as u32);
                assert!(!halftone(&px, &page), "scale {scale}, margin {margin}: {got:?}");
                assert!(got.is_some_and(|(dither, _)| dither <= 5), "hardly a block of it is dither: {got:?}");
            }
        }
    }

    /// #6812 review, a newspaper page: crisp text beside a large dithered photograph. The photo is
    /// a halftone and the page is not: restoring it would smear the text.
    #[test]
    fn crisp_text_beside_a_dithered_photograph_is_no_halftone() {
        for photo_rows in [150, 330, 440] {
            let mut page = Page::new(400, 520, 1.0);
            page.text(3, 12, 0.0);
            // A photograph: smooth greys from dark to light.
            for y in 20..20 + photo_rows {
                for x in 20..380 {
                    page.grey[y * 400 + x] = 0.15 + 0.7 * (x as f32 / 400.0) * (y as f32 / 520.0 + 0.4).min(1.0);
                }
            }
            // The text stays crisp: only the photograph is dithered.
            let dots = page.dithered();
            let mut px = page.crisp();
            for y in 20..20 + photo_rows {
                px[y * 400 + 20..y * 400 + 380].copy_from_slice(&dots[y * 400 + 20..y * 400 + 380]);
            }
            assert!(!halftone(&px, &page), "{photo_rows} rows of photo: {:?}", texture(&px, 400, 520));
        }
    }

    /// #6812 review, a record card: solid handwriting and ruled lines on paper whose grain the
    /// scanner turned into fine speckle. The speckle is dots, but the ink is crisp.
    #[test]
    fn solid_writing_on_speckled_paper_is_no_halftone() {
        for speckle in [8u32, 15, 25] {
            let (w, h) = (480usize, 360usize);
            let mut page = Page::new(w, h, 1.0);
            // Ruled lines three pixels thick, and writing as slanted strokes four pixels thick.
            for y in (40..h - 20).step_by(48) {
                page.fill(20, y, w - 40, 3, 0.0);
                for stroke in 0..22 {
                    for t in 0..30 {
                        page.fill(30 + stroke * 19 + t / 3, y - 34 + t, 4, 1, 0.0);
                    }
                }
            }
            let mut px = page.crisp();
            // Speckle: a share (percent) of the pixels, from a fixed pseudo-random sequence.
            let mut state = 0x2545_f491_4f6c_dd1du64;
            for v in px.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                if (state % 100) < u64::from(speckle) {
                    *v = 0;
                }
            }
            assert!(!halftone(&px, &page), "{speckle} % speckle: {:?}", texture(&px, w as u32, h as u32));
        }
    }

    /// A scanner's dark border is a blot, not strokes: it does not stop a dithered page beside it
    /// being a halftone.
    #[test]
    fn a_dark_border_beside_a_dithered_page_is_still_a_halftone() {
        let mut page = Page::new(320, 400, 0.8);
        page.text(3, 30, 0.45);
        let mut px = page.dithered();
        for y in 0..400 {
            px[y * 320..y * 320 + 14].fill(0);
        }
        assert!(halftone(&px, &page), "{:?}", texture(&px, 320, 400));
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
