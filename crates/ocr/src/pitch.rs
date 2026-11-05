//! How tall the text on a page is, measured cheaply from its pixels, to decide whether detecting
//! it in tiles is worth the cost.
//!
//! What decides whether tiling helps is the height of the text once the page is shrunk to the
//! detector's input, not how full the page is. Lines of text repeat at a regular distance, the
//! line pitch, so the ink summed per pixel row rises and falls with that period. The page is
//! shrunk to a grey copy of at most [`WORK`] pixels a side and cut into a [`GRID`] × [`GRID`]
//! grid of blocks. In each block the ink is summed per row, the slow trend (shading, pictures) is
//! taken out with a moving average, and the autocorrelation of what remains is taken. The
//! autocorrelations of the blocks are averaged: columns of text whose lines do not line up still
//! agree on the period, and blocks of blank paper are left out. The period is the first strong
//! peak of the average. The same is done with the ink summed per pixel column, for pages that lie
//! sideways, and the axis with the stronger peak wins.
//!
//! The constants were chosen on MDPBench (de/es/fr, 480 pages; see the PR that added this file):
//! tiling a page whose lines repeat every fewer than [`SMALL_PITCH`] detector pixels, or whose
//! lines show no regular spacing at all, scored better than tiling every page, at about half the
//! cost.

/// The largest side of the grey copy the pitch is measured on, in pixels.
const WORK: u32 = 1600;
/// The page is cut into `GRID` × `GRID` blocks.
const GRID: u32 = 6;
/// The longest line pitch looked for, in detector pixels.
const MAX_PITCH: f64 = 30.0;
/// A peak of the averaged, normalised autocorrelation weaker than this is noise.
const PEAK_FLOOR: f64 = 0.05;
/// The first peak at least this share of the strongest one is the pitch; the later ones are its
/// multiples (every second or third line).
const PEAK_SHARE: f64 = 0.6;
/// Lines closer together than this, in detector pixels, are small print.
pub(crate) const SMALL_PITCH: f64 = 9.0;

/// What the pixels of a page say about its lines of text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Lines {
    /// Nothing to measure: blank or uniform paper, a page too narrow to show a pitch, or pixels
    /// that do not form the image they claim to.
    Blank,
    /// Ink without a regular line spacing: pictures, blur, a skewed or curved photo, a few
    /// scattered lines.
    Unclear,
    /// Lines repeat every so many pixels of the detector's input.
    Pitch(f64),
}

impl Lines {
    /// Whether the page is worth detecting in tiles: its lines are close together once shrunk to
    /// the detector's input, or the pixels cannot tell. A blank page is not.
    pub fn wants_tiles(self) -> bool {
        match self {
            Lines::Blank => false,
            Lines::Unclear => true,
            Lines::Pitch(p) => p < SMALL_PITCH,
        }
    }
}

/// The lines of a `width` × `height` image of `channels` bytes per pixel (grey, grey and alpha,
/// RGB or RGBA), for a detector that reads `input` = (width, height) pixels. Reads every pixel
/// once; the rest works on at most [`WORK`] × [`WORK`] values.
pub(crate) fn lines(pixels: &[u8], width: u32, height: u32, channels: usize, input: (u32, u32)) -> Lines {
    let size = (width as usize).checked_mul(height as usize).and_then(|a| a.checked_mul(channels));
    if !(1..=4).contains(&channels) || size != Some(pixels.len()) || pixels.is_empty() || input.0 == 0 || input.1 == 0 {
        return Lines::Blank;
    }
    let Some(grey) = Grey::shrink(pixels, width, height, channels) else { return Lines::Blank };
    // The longest pitch looked for, in pixels of the grey copy, along each axis.
    let reach = |len: u32, input: u32| ((MAX_PITCH * f64::from(len) / f64::from(input)) as usize).max(4);
    let down = grey.curve(Axis::Down, reach(grey.height, input.1));
    let across = grey.curve(Axis::Across, reach(grey.width, input.0));
    if !down.inked && !across.inked {
        return Lines::Blank;
    }
    let to_input = |lag: usize, len: u32, input: u32| lag as f64 * f64::from(grey.step) * f64::from(input) / f64::from(len);
    match (peak(&down.correlation), peak(&across.correlation)) {
        (None, None) => Lines::Unclear,
        (Some((lag, d)), Some((_, a))) if d >= a => Lines::Pitch(to_input(lag, height, input.1)),
        (Some((lag, _)), None) => Lines::Pitch(to_input(lag, height, input.1)),
        (_, Some((lag, _))) => Lines::Pitch(to_input(lag, width, input.0)),
    }
}

/// A shrunk copy of a page as ink: 0 for white, 255 for black.
struct Grey {
    ink: Vec<f32>,
    width: u32,
    height: u32,
    /// Page pixels per pixel of the copy, along each axis.
    step: u32,
}

#[derive(Clone, Copy)]
enum Axis {
    /// Ink summed per row: lines of text lying the normal way.
    Down,
    /// Ink summed per column: lines lying sideways.
    Across,
}

/// The averaged autocorrelation of one axis, and whether any block had ink.
struct Curve {
    correlation: Vec<f64>,
    inked: bool,
}

impl Grey {
    /// Box-average the page down by a whole factor until neither side exceeds [`WORK`]. The
    /// caller has checked that `pixels` holds `width` × `height` pixels of `channels` bytes.
    fn shrink(pixels: &[u8], width: u32, height: u32, channels: usize) -> Option<Grey> {
        let step = width.max(height).div_ceil(WORK).max(1);
        let (w, h) = (width / step, height / step);
        if w == 0 || h == 0 {
            return None;
        }
        // At most WORK × WORK values: `step` is chosen so that each side fits.
        let mut sum = vec![0f32; (w as usize).checked_mul(h as usize)?];
        let row_bytes = (width as usize).checked_mul(channels)?;
        let (step_px, w_px) = (step as usize, w as usize);
        for (y, row) in pixels.chunks_exact(row_bytes).take(h as usize * step_px).enumerate() {
            let start = (y / step_px).checked_mul(w_px)?;
            let Some(out) = sum.get_mut(start..start.checked_add(w_px)?) else { continue };
            for (x, px) in row.chunks_exact(channels).take(w_px * step_px).enumerate() {
                if let Some(o) = out.get_mut(x / step_px) {
                    *o += luma(px);
                }
            }
        }
        let area = (step as f32) * (step as f32);
        let ink = sum.into_iter().map(|s| 255.0 - s / area).collect();
        Some(Grey { ink, width: w, height: h, step })
    }

    /// The ink of the copy at `(x, y)`, 0 outside it.
    fn at(&self, x: usize, y: usize) -> f32 {
        if x >= self.width as usize {
            return 0.0;
        }
        y.checked_mul(self.width as usize).and_then(|i| i.checked_add(x)).and_then(|i| self.ink.get(i)).copied().unwrap_or(0.0)
    }

    /// The autocorrelation of the ink profile along `axis`, for lags `0..=reach`, averaged over
    /// the blocks that have ink. Empty when no block has, or the blocks are too short to show
    /// `reach`.
    fn curve(&self, axis: Axis, reach: usize) -> Curve {
        let (along, across) = match axis {
            Axis::Down => (self.height as usize, self.width as usize),
            Axis::Across => (self.width as usize, self.height as usize),
        };
        let (block, side) = (along / GRID as usize, across / GRID as usize);
        let mut total = vec![0f64; reach + 1];
        let (mut blocks, mut inked) = (0usize, false);
        if side == 0 || block < 2 * reach + 2 {
            return Curve { correlation: Vec::new(), inked: false };
        }
        let mut profile = vec![0f64; block];
        for i in 0..GRID as usize {
            for j in 0..GRID as usize {
                // The block's ink per row (or column), averaged across the block.
                for (k, p) in profile.iter_mut().enumerate() {
                    let a = i * block + k;
                    let sum: f32 = (j * side..(j + 1) * side)
                        .map(|b| match axis {
                            Axis::Down => self.at(b, a),
                            Axis::Across => self.at(a, b),
                        })
                        .sum();
                    *p = f64::from(sum) / side as f64;
                }
                let detrended = detrend(&profile, reach);
                let energy: f64 = detrended.iter().map(|d| d * d).sum();
                // Blank paper: next to no variation along the block.
                if energy <= 1e-3 * block as f64 {
                    continue;
                }
                inked = true;
                blocks += 1;
                for (lag, t) in total.iter_mut().enumerate() {
                    let c: f64 = detrended.iter().zip(detrended.iter().skip(lag)).map(|(a, b)| a * b).sum();
                    *t += c / energy;
                }
            }
        }
        if blocks == 0 {
            return Curve { correlation: Vec::new(), inked };
        }
        total.iter_mut().for_each(|t| *t /= blocks as f64);
        Curve { correlation: total, inked }
    }
}

/// The grey value of a pixel of 1 to 4 bytes (grey, grey and alpha, RGB, RGBA), 0 to 255.
fn luma(px: &[u8]) -> f32 {
    match *px {
        [r, g, b, ..] => 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b),
        [v, ..] => f32::from(v),
        [] => 255.0,
    }
}

/// `profile` minus its moving average over `window` samples, the ends held at the edge values.
fn detrend(profile: &[f64], window: usize) -> Vec<f64> {
    let n = profile.len();
    let window = window.max(1);
    let half = window / 2;
    (0..n)
        .map(|i| {
            let sum: f64 = (0..window).filter_map(|j| profile.get((i + j).saturating_sub(half).min(n.saturating_sub(1)))).sum();
            profile.get(i).copied().unwrap_or(0.0) - sum / window as f64
        })
        .collect()
}

/// The line pitch in a normalised autocorrelation, as `(lag, strength)`: past the first zero
/// crossing, the first local maximum at least [`PEAK_SHARE`] of the strongest one, counting only
/// maxima above [`PEAK_FLOOR`]. `None` without such a peak.
fn peak(correlation: &[f64]) -> Option<(usize, f64)> {
    let zero = correlation.iter().position(|&c| c < 0.0)?;
    let peaks: Vec<(usize, f64)> = correlation
        .windows(3)
        .enumerate()
        .filter_map(|(i, w)| match *w {
            [a, b, c] if i + 1 >= zero.max(2) && b >= a && b >= c && b >= PEAK_FLOOR => Some((i + 1, b)),
            _ => None,
        })
        .collect();
    let strongest = peaks.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    peaks.into_iter().find(|p| p.1 >= PEAK_SHARE * strongest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The detector of the ocrs models reads 600 × 800 pixels.
    const INPUT: (u32, u32) = (600, 800);
    /// A4 at 300 dpi.
    const A4: (u32, u32) = (2480, 3508);

    /// A white grey page with lines of "words" (dark bars) every `pitch` pixels, 40 % of the
    /// pitch tall, within 200-pixel margins; `sideways` turns the lines a quarter turn.
    fn print(pitch: u32, sideways: bool) -> Vec<u8> {
        let (w, h) = A4;
        let mut px = vec![255u8; (w * h) as usize];
        for y in 200..h - 200 {
            for x in 200..w - 200 {
                let (along, line) = if sideways { (y, x) } else { (x, y) };
                // Words of 4 × 15 pixels with a space of 15 between them.
                if line % pitch < pitch * 2 / 5 && (along / 15) % 5 != 4 {
                    px[(y * w + x) as usize] = 0;
                }
            }
        }
        px
    }

    fn pitch_of(lines: Lines) -> f64 {
        match lines {
            Lines::Pitch(p) => p,
            other => panic!("no pitch: {other:?}"),
        }
    }

    #[test]
    fn a_blank_page_is_not_tiled() {
        let (w, h) = A4;
        let lines = lines(&vec![255u8; (w * h) as usize], w, h, 1, INPUT);
        assert_eq!(lines, Lines::Blank);
        assert!(!lines.wants_tiles());
    }

    #[test]
    fn large_print_is_not_tiled() {
        // Lines every 100 pixels of an A4 page at 300 dpi (8 mm, a 20-point font) repeat every
        // 100 × 800 / 3508 = 22.8 detector pixels.
        let (w, h) = A4;
        let lines = lines(&print(100, false), w, h, 1, INPUT);
        let p = pitch_of(lines);
        assert!((21.0..24.0).contains(&p), "{p}");
        assert!(!lines.wants_tiles());
    }

    #[test]
    fn small_print_is_tiled() {
        // Lines every 30 pixels (2.5 mm, a 7-point font): 6.8 detector pixels.
        let (w, h) = A4;
        let lines = lines(&print(30, false), w, h, 1, INPUT);
        let p = pitch_of(lines);
        assert!((6.0..7.5).contains(&p), "{p}");
        assert!(lines.wants_tiles());
        // The same page in RGB and RGBA.
        for channels in [3, 4] {
            let px: Vec<u8> = print(30, false).into_iter().flat_map(|v| std::iter::repeat_n(v, channels)).collect();
            assert_eq!(super::lines(&px, w, h, channels, INPUT), lines, "{channels} channels");
        }
    }

    #[test]
    fn sideways_small_print_is_tiled() {
        // Lines every 30 pixels across a 2480-pixel width: 30 × 600 / 2480 = 7.3 detector pixels.
        let (w, h) = A4;
        let lines = lines(&print(30, true), w, h, 1, INPUT);
        let p = pitch_of(lines);
        assert!((6.5..8.0).contains(&p), "{p}");
        assert!(lines.wants_tiles());
    }

    #[test]
    fn ink_without_lines_is_tiled() {
        // Noise (a picture, a blurred photo): the pixels cannot tell how tall the text is.
        let (w, h) = A4;
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let px: Vec<u8> = (0..w * h)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                if seed & 1 == 0 { 0 } else { 255 }
            })
            .collect();
        let lines = lines(&px, w, h, 1, INPUT);
        assert_eq!(lines, Lines::Unclear);
        assert!(lines.wants_tiles());
    }

    #[test]
    fn odd_inputs_give_an_answer_without_panicking() {
        assert_eq!(lines(&[], 0, 0, 1, INPUT), Lines::Blank);
        assert_eq!(lines(&[0], 1, 1, 1, INPUT), Lines::Blank);
        assert_eq!(lines(&[0, 0, 0], 1, 1, 3, INPUT), Lines::Blank);
        // Fewer or more bytes than the size claims, no channels, too many, a detector of no size.
        assert_eq!(lines(&[0; 7], 2, 2, 2, INPUT), Lines::Blank);
        assert_eq!(lines(&[0; 4], 2, 2, 0, INPUT), Lines::Blank);
        assert_eq!(lines(&[0; 20], 2, 2, 5, INPUT), Lines::Blank);
        assert_eq!(lines(&[0; 4], u32::MAX, u32::MAX, 4, INPUT), Lines::Blank);
        assert_eq!(lines(&print(30, false), A4.0, A4.1, 1, (0, 0)), Lines::Blank);
        // A sliver: one side shrinks to nothing, or too short to show a pitch.
        assert_eq!(lines(&[0; 5000], 1, 5000, 1, INPUT), Lines::Blank);
        let tall = (0..3 * 40_000u32).map(|i| if (i / 3) % 30 < 12 { 0 } else { 255 }).collect::<Vec<u8>>();
        let _ = lines(&tall, 3, 40_000, 1, INPUT);
        // A black page: ink everywhere, but nothing that varies.
        assert_eq!(lines(&vec![0u8; 2000 * 2000], 2000, 2000, 1, INPUT), Lines::Blank);
    }

    #[test]
    fn the_first_strong_peak_is_the_pitch() {
        // Peaks at 5 (0.5), 10 (0.8), 15 (0.3): 5 is past the share of the strongest, so it is
        // the pitch and 10 its double.
        let mut c = vec![0.0; 18];
        c[0] = 1.0;
        c[2] = -0.2;
        c[5] = 0.5;
        c[10] = 0.8;
        c[15] = 0.3;
        assert_eq!(peak(&c), Some((5, 0.5)));
        c[5] = 0.4;
        assert_eq!(peak(&c), Some((10, 0.8)));
        // No zero crossing, or nothing above the floor: no pitch.
        assert_eq!(peak(&[1.0, 0.5, 0.4, 0.5, 0.3]), None);
        assert_eq!(peak(&[1.0, -0.1, 0.01, 0.0, 0.02, 0.0]), None);
        assert_eq!(peak(&[]), None);
    }
}
