//! Binarization of a page raster into ink and paper (ClearScan stage 1).
//!
//! Two thresholds, both clean-room from their papers' mathematics:
//!
//! - **Otsu** (N. Otsu, *A Threshold Selection Method from Gray-Level Histograms*, 1979):
//!   the global threshold that maximises the between-class variance of the two halves of the
//!   histogram. Fast, and right when the page is evenly lit. Falls down under the lighting
//!   gradient, shadow or bleed-through of a real scan, where a single global cut is either too
//!   dark for the dim half or too light for the bright half.
//! - **Sauvola** (J. Sauvola & M. Pietikäinen, *Adaptive Document Image Binarization*, 2000):
//!   a local threshold per pixel, `T = m · (1 + k · (s / R − 1))` over the mean `m` and standard
//!   deviation `s` of a window around it, with `R` the range of the normalized standard
//!   deviation (128 for 8-bit greys) and `k` biasing towards ink (0.34 keeps faint strokes on
//!   clean paper without turning paper grain into ink). Flat regions fall to paper whatever
//!   their absolute level, so a lighting gradient across the sheet does not matter.
//!
//! The window statistics come from sliding column accumulators: the sum and sum of squares of
//! each window column are updated in O(1) as the window moves down a row, and the horizontal
//! pass folds them into the window sum in O(1) as it moves right. One pass, O(width) working
//! memory, integer-exact. Windows reach past the borders by repeating the edge rows and
//! columns, so a border pixel's window is full and its statistics stay stable.

use crate::grey::{GreyImage, MAX_PIXELS};

/// A binarized page: `width` × `height` flags, one per pixel, row-major, top-left origin.
/// `1` is ink (the darker-than-threshold side), `0` is paper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    width: u32,
    height: u32,
    ink: Vec<u8>,
}

impl Bitmap {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The ink flags, row-major, one byte per pixel (0 or 1).
    pub fn ink(&self) -> &[u8] {
        &self.ink
    }

    /// Whether `(x, y)` is ink. Outside the image there is only paper.
    pub fn is_ink(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return false;
        }
        let index = y as usize * self.width as usize + x as usize;
        self.ink.get(index).copied().unwrap_or(0) == 1
    }

    /// How many pixels are ink.
    pub fn ink_count(&self) -> u64 {
        self.ink.iter().map(|&f| u64::from(f)).sum()
    }

    /// A bitmap from ink flags. `ink.len()` must be `width` × `height`, and the canvas must
    /// fit [`MAX_PIXELS`]. Any non-zero byte is ink. `None` when the buffer does not match
    /// the canvas or the canvas is over the cap — callers keep the allocation they already made.
    pub fn from_ink(width: u32, height: u32, mut ink: Vec<u8>) -> Option<Bitmap> {
        let pixels = u64::from(width).checked_mul(u64::from(height))?;
        if pixels != ink.len() as u64 || pixels > MAX_PIXELS {
            return None;
        }
        for flag in &mut ink {
            *flag = u8::from(*flag != 0);
        }
        Some(Bitmap { width, height, ink })
    }
}

/// Which threshold a page is binarized with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Binarization {
    /// Sauvola on anything page-sized, Otsu on tiny images (below [`SAUVOLA_MIN_SIDE`] pixels
    /// on a side, where a 41-pixel window would be the whole image anyway and Otsu is cheaper).
    #[default]
    Auto,
    Sauvola,
    Otsu,
}

/// Below this many pixels on the shorter side, [`Binarization::Auto`] takes the Otsu path.
pub const SAUVOLA_MIN_SIDE: u32 = 48;

/// Sauvola's tuning, as named in the paper. `window` is forced odd and clamped to the image;
/// `k` is kept in the paper's working range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SauvolaParams {
    pub window: u32,
    pub k: f32,
}

impl Default for SauvolaParams {
    fn default() -> Self {
        // A 41-pixel window at 300 DPI spans about a character and a half: wide enough to hold
        // local statistics with text in them, narrow enough to follow a gradient.
        SauvolaParams { window: 41, k: 0.34 }
    }
}

/// Otsu's threshold for a grey image: the cut between two histogram halves that maximises the
/// between-class variance. Returns the threshold a pixel must be **strictly below** to be ink,
/// or `None` when the image holds a single grey level (nothing to separate).
pub(crate) fn otsu_threshold(grey: &GreyImage) -> Option<u8> {
    let mut histogram = [0_u64; 256];
    for &g in grey.data() {
        histogram[g as usize] += 1;
    }
    let total = u64::from(grey.width()) * u64::from(grey.height());
    if total == 0 {
        return None;
    }
    // The cut between grey levels `t` and `t + 1` splits the histogram into ink ≤ t and
    // paper > t; walk the levels keeping running class sums, and maximise σ²_between =
    // ω_ink · ω_paper · (μ_ink − μ_paper)².
    let mut ink_count = 0_u64;
    let mut ink_sum = 0_u64;
    let total_sum: u64 = histogram.iter().enumerate().map(|(level, &n)| n * level as u64).sum();
    let mut best_variance = 0.0_f64;
    let mut best_cut: Option<u8> = None;
    for (level, &count) in histogram.iter().enumerate().take(255) {
        ink_count += count;
        ink_sum += count * level as u64;
        if ink_count == 0 || ink_count == total {
            continue;
        }
        let paper_count = total - ink_count;
        let paper_sum = total_sum - ink_sum;
        let ink_mean = ink_sum as f64 / ink_count as f64;
        let paper_mean = paper_sum as f64 / paper_count as f64;
        let variance = (ink_count as f64) * (paper_count as f64) * (ink_mean - paper_mean).powi(2);
        if variance > best_variance {
            best_variance = variance;
            // `level` is the last grey on the ink side; a pixel is ink when it is ≤ level,
            // i.e. strictly below `level + 1`.
            best_cut = Some(level as u8 + 1);
        }
    }
    best_cut
}

/// Sauvola's threshold map, materialized per pixel while the window sums slide over the image
/// (the threshold is a function of the window statistics, so it is applied where they are).
fn sauvola_binarize(grey: &GreyImage, params: &SauvolaParams) -> Bitmap {
    let (width, height) = (grey.width(), grey.height());
    let mut ink = vec![0_u8; (width as usize) * (height as usize)];
    if width == 0 || height == 0 {
        return Bitmap { width, height, ink };
    }
    let side = params.window.clamp(1, width.min(height));
    // Odd sides keep the window centred on its pixel.
    let side = side | 1;
    let radius = (side as i64 - 1) / 2;
    // R is the dynamic range of the standard deviation of an 8-bit image, as the paper defines
    // it for normalized greys; k comes straight from the parameters.
    let r = 128.0_f32;
    let k = params.k;
    // Column accumulators over the window's current rows: sum and sum of squares of the grey
    // levels of each column x for rows [row - radius, row + radius] (edges repeated).
    let mut col_sum = vec![0_u64; width as usize];
    let mut col_sqsum = vec![0_u64; width as usize];
    let add_row = |y: i64, col_sum: &mut Vec<u64>, col_sqsum: &mut Vec<u64>, sign: i64| {
        let clamped = y.clamp(0, height as i64 - 1) as u32;
        let row = clamped as usize * width as usize;
        for x in 0..width as usize {
            let g = grey.data().get(row + x).copied().unwrap_or(u8::MAX) as u64;
            if sign > 0 {
                col_sum[x] += g;
                col_sqsum[x] += g * g;
            } else {
                col_sum[x] = col_sum[x].saturating_sub(g);
                col_sqsum[x] = col_sqsum[x].saturating_sub(g * g);
            }
        }
    };
    // Prime the window with rows [0, radius].
    for y in -radius..=radius {
        add_row(y, &mut col_sum, &mut col_sqsum, 1);
    }
    let area = u64::from(side) * u64::from(side);
    for y in 0..height as i64 {
        if y > 0 {
            // The window moved down a row: drop the top row, take in the next one.
            add_row(y - radius - 1, &mut col_sum, &mut col_sqsum, -1);
            add_row(y + radius, &mut col_sum, &mut col_sqsum, 1);
        }
        // Horizontal pass: window sums over columns [x - radius, x + radius] of the column
        // accumulators, edges repeated, slid in O(1) per pixel.
        let mut row_sum = 0_u64;
        let mut row_sqsum = 0_u64;
        for x in -radius..=radius {
            let cx = x.clamp(0, width as i64 - 1) as usize;
            row_sum += col_sum[cx];
            row_sqsum += col_sqsum[cx];
        }
        for x in 0..width as i64 {
            let mean = row_sum as f64 / area as f64;
            let mean_square = row_sqsum as f64 / area as f64;
            let variance = (mean_square - mean * mean).max(0.0);
            let deviation = variance.sqrt() as f32;
            // T = m · (1 + k·(s/R − 1)); ink is darker than its neighbourhood's threshold.
            let threshold = mean as f32 * (1.0 + k * (deviation / r - 1.0));
            let g = grey.get(x, y) as f32;
            let index = y as usize * width as usize + x as usize;
            if let Some(flag) = ink.get_mut(index) {
                *flag = u8::from(g < threshold);
            }
            // Slide the window one column right.
            let out = (x - radius).clamp(0, width as i64 - 1) as usize;
            let inx = (x + radius + 1).clamp(0, width as i64 - 1) as usize;
            row_sum = row_sum.saturating_sub(col_sum[out]).saturating_add(col_sum[inx]);
            row_sqsum = row_sqsum.saturating_sub(col_sqsum[out]).saturating_add(col_sqsum[inx]);
        }
    }
    Bitmap { width, height, ink }
}

/// Binarize `grey` into ink and paper. [`Binarization::Auto`] picks Sauvola for page-sized
/// images (the scan artifacts it exists for) and Otsu for tiny ones; a uniform image has no
/// ink either way. [`Binarization::Otsu`] is the fast global fallback for evenly lit pages.
pub fn binarize(grey: &GreyImage, mode: Binarization, params: &SauvolaParams) -> Bitmap {
    let effective = match mode {
        Binarization::Auto => {
            if grey.width().min(grey.height()) < SAUVOLA_MIN_SIDE {
                Binarization::Otsu
            } else {
                Binarization::Sauvola
            }
        }
        mode => mode,
    };
    match effective {
        Binarization::Otsu => {
            let mut ink = vec![0_u8; grey.data().len()];
            if let Some(threshold) = otsu_threshold(grey) {
                for (flag, &g) in ink.iter_mut().zip(grey.data()) {
                    *flag = u8::from(g < threshold);
                }
            }
            Bitmap { width: grey.width(), height: grey.height(), ink }
        }
        Binarization::Sauvola => sauvola_binarize(grey, params),
        Binarization::Auto => sauvola_binarize(grey, params),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` × `height` grey image from a closure.
    fn image(width: u32, height: u32, value: impl Fn(u32, u32) -> u8) -> GreyImage {
        let mut data = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                data.push(value(x, y));
            }
        }
        GreyImage::new(width, height, data).unwrap()
    }

    /// Ink drawn as an axis-aligned box.
    fn draw(grey: &mut GreyImage, x0: u32, y0: u32, w: u32, h: u32, value: u8) {
        for y in y0..y0.saturating_add(h).min(grey.height()) {
            for x in x0..x0.saturating_add(w).min(grey.width()) {
                let index = (y * grey.width() + x) as usize;
                grey.data_mut()[index] = value;
            }
        }
    }

    #[test]
    fn otsu_separates_a_well_lit_page() {
        let mut grey = image(80, 60, |_, _| 220);
        draw(&mut grey, 10, 10, 8, 12, 60);
        draw(&mut grey, 40, 30, 8, 12, 60);
        let binary = binarize(&grey, Binarization::Otsu, &SauvolaParams::default());
        assert_eq!(binary.ink_count(), 8 * 12 * 2, "exactly the two strokes are ink");
        assert!(binary.is_ink(14, 15) && binary.is_ink(44, 35));
        assert!(!binary.is_ink(0, 0) && !binary.is_ink(79, 59));
    }

    /// The case Otsu cannot win: a lighting gradient halves the page. Sauvola reads both
    /// halves with the same 90-level contrast between ink and paper.
    #[test]
    fn sauvola_reads_text_across_a_lighting_gradient() {
        let width = 200_u32;
        let height = 60_u32;
        let level = |x: u32| 90_u32 + (x * 140) / width; // 90 → 229, left dark
        let mut grey = image(width, height, |x, _| level(x) as u8);
        // Strokes 90 grey levels darker than the local paper.
        for (x0, y0) in [(20_u32, 20_u32), (150, 20)] {
            for y in y0..y0 + 16 {
                for x in x0..x0 + 12 {
                    grey.data_mut()[(y * width + x) as usize] = (level(x) - 90) as u8;
                }
            }
        }
        let sauvola = binarize(&grey, Binarization::Sauvola, &SauvolaParams::default());
        let left = stroke_ink(&sauvola, 20, 20, 12, 16);
        let right = stroke_ink(&sauvola, 150, 20, 12, 16);
        assert!(left >= 12 * 16 * 19 / 20, "the dark half keeps its stroke: {left}");
        assert!(right >= 12 * 16 * 19 / 20, "the bright half keeps its stroke: {right}");
        // Both strokes found, and paper stayed paper: no more than 2% stray ink besides them.
        assert!(sauvola.ink_count() <= 2 * 12 * 16 + 240, "paper stays paper: {}", sauvola.ink_count());
        // Otsu really does lose by it: the single cut either drowns a stroke in paper or
        // floods the dark half's background into ink.
        let otsu = binarize(&grey, Binarization::Otsu, &SauvolaParams::default());
        let otsu_left = stroke_ink(&otsu, 20, 20, 12, 16);
        let otsu_right = stroke_ink(&otsu, 150, 20, 12, 16);
        let lost_a_stroke = otsu_left < 12 * 16 / 2 || otsu_right < 12 * 16 / 2;
        let flooded_the_paper = otsu.ink_count() > 8 * 12 * 16;
        assert!(lost_a_stroke || flooded_the_paper, "gradient vs global cut: {} and {} of 192, {} ink", otsu_left, otsu_right, otsu.ink_count());
    }

    /// Ink pixels inside a box (the box is small enough that this is exact for a clean cut).
    fn stroke_ink(binary: &Bitmap, x0: u32, y0: u32, w: u32, h: u32) -> u64 {
        (y0..y0 + h).map(|y| (x0..x0 + w).filter(|&x| binary.is_ink(x as i64, y as i64)).count() as u64).sum()
    }

    #[test]
    fn a_blank_page_has_no_ink() {
        // A smooth gradient is blank to a local threshold, and Otsu cannot be: a global cut
        // through a ramp splits it, which is why Sauvola is the default.
        let grey = image(100, 100, |x, _| (150 + x / 10) as u8);
        for mode in [Binarization::Auto, Binarization::Sauvola] {
            assert_eq!(binarize(&grey, mode, &SauvolaParams::default()).ink_count(), 0, "{mode:?}");
        }
        assert!(binarize(&grey, Binarization::Otsu, &SauvolaParams::default()).ink_count() > 0, "Otsu splits the ramp");
    }

    #[test]
    fn a_single_level_page_has_no_ink_and_no_division_by_zero() {
        let grey = image(64, 64, |_, _| 137);
        for mode in [Binarization::Auto, Binarization::Otsu] {
            assert_eq!(binarize(&grey, mode, &SauvolaParams::default()).ink_count(), 0, "{mode:?}");
        }
    }

    #[test]
    fn tiny_images_take_the_otsu_path_and_still_work() {
        let mut grey = image(8, 6, |_, _| 200);
        draw(&mut grey, 3, 2, 2, 2, 40);
        let auto = binarize(&grey, Binarization::Auto, &SauvolaParams::default());
        let otsu = binarize(&grey, Binarization::Otsu, &SauvolaParams::default());
        assert_eq!(auto, otsu, "Auto agrees with Otsu below SAUVOLA_MIN_SIDE");
        assert_eq!(otsu.ink_count(), 4);
    }

    /// A window larger than the image is clamped; the whole image is each pixel's window, and
    /// a stroke darker than the global mean still comes out as ink.
    #[test]
    fn a_window_larger_than_the_image_is_clamped() {
        let mut grey = image(20, 12, |_, _| 210);
        draw(&mut grey, 8, 4, 4, 4, 30);
        let params = SauvolaParams { window: 99, k: 0.34 };
        let binary = binarize(&grey, Binarization::Sauvola, &params);
        let stroke = stroke_ink(&binary, 8, 4, 4, 4);
        assert!(stroke >= 4 * 4 * 3 / 4, "the stroke survives: {stroke}");
        assert_eq!(binary.ink_count(), stroke, "nothing else is ink");
    }

    #[test]
    fn a_one_pixel_image_is_handled() {
        for value in [0_u8, 128, 255] {
            let grey = GreyImage::new(1, 1, vec![value]).unwrap();
            for mode in [Binarization::Auto, Binarization::Sauvola, Binarization::Otsu] {
                let binary = binarize(&grey, mode, &SauvolaParams::default());
                assert_eq!((binary.width(), binary.height()), (1, 1));
            }
        }
    }

    /// Border pixels get full windows through repeated edges, so a stroke touching the border
    /// is read as well as one in the middle.
    #[test]
    fn strokes_touching_the_border_are_read() {
        let mut grey = image(120, 80, |_, _| 230);
        draw(&mut grey, 0, 0, 10, 12, 70);
        draw(&mut grey, 110, 68, 10, 12, 70);
        let binary = binarize(&grey, Binarization::Sauvola, &SauvolaParams::default());
        let corners = [(0_u32, 0_u32, 10, 12), (110, 68, 10, 12)];
        let found = corners.iter().map(|&(x, y, w, h)| stroke_ink(&binary, x, y, w, h)).collect::<Vec<_>>();
        for (n, &ink) in found.iter().enumerate() {
            assert!(ink >= 10 * 12 * 9 / 10, "border stroke {n}: {ink}");
        }
    }
}
