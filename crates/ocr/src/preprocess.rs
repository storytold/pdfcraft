//! Preprocessing: a pure function between the page raster and recognition, in fixed order —
//! (a) DPI normalization, then the four opt-in steps (b) orientation, (c) deskew, (d) denoise,
//! (e) binarization. Normalization exists for those steps (their windows and thresholds are
//! calibrated for 300 dpi), so it runs only when at least one of them is on: with every step
//! off the raster is handed to the engines exactly as it was rendered. (Enlarging a render
//! on its own changes what the models read: a 72 dpi scan of "due: €250" read as "ue: ?250"
//! after a 4× bilinear enlargement.) Everything is plain Rust on the RGBA raster: no
//! Leptonica, no process, no new dependencies.
//!
//! Every geometric step records its inverse; the result carries one exact
//! [`InverseTransform`] (an affine map, composed in f64), and callers map every word box
//! through it back to the *original* raster coordinates before placing the words.
//!
//! The heuristics are deliberately conservative and panic-free: an image too small to judge,
//! a blank page, or a score without a clear margin is a no-op, never a guess.

use crate::{MAX_PIXELS, MAX_SIDE, OcrError, OcrImage};

/// The four opt-in steps. All off by default: a scan is passed through as rendered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreprocessOptions {
    /// Detect pages scanned at 90°/180°/270° and turn them upright.
    pub auto_rotate: bool,
    /// Straighten skewed text lines.
    pub deskew: bool,
    /// 3×3 median filter (salt-and-pepper noise).
    pub denoise: bool,
    /// Sauvola binarization (window 8, k 0.34).
    pub binarize: bool,
}

impl PreprocessOptions {
    /// Whether any step is on (and so the raster is normalized to 300 dpi first).
    pub fn any(&self) -> bool {
        self.auto_rotate || self.deskew || self.denoise || self.binarize
    }
}

/// A preprocessed raster: the image to recognize, the exact way back to the input raster, and
/// the resolution the image now stands for.
#[derive(Debug)]
pub struct Preprocessed {
    pub image: OcrImage,
    /// Maps pixel coordinates in [`Preprocessed::image`] back to the input raster's.
    pub inverse_transform: InverseTransform,
    /// The image's resolution in dots per inch: 300 after normalization (only when a step is
    /// on), or the source's own
    /// when it already rendered at 300 or more.
    pub dpi: f32,
}

/// Preprocess `image` (rendered at `source_dpi`) per `options`, in the fixed order. The image
/// is consumed: the common path (every step off, or a no-op) hands the raster back without
/// copying it, and a step that rewrites pixels replaces it.
pub fn preprocess(image: OcrImage, source_dpi: f32, options: &PreprocessOptions) -> Result<Preprocessed, OcrError> {
    let mut current = image;
    let mut inverse = InverseTransform::identity();
    // (a) DPI normalization: for the steps below, a scan under 300 dpi is enlarged by 300/src so
    // they see letter-sized strokes. With every step off nothing is resampled. A broken dpi
    // (zero, negative, NaN) is left alone.
    let mut dpi = if source_dpi.is_finite() && source_dpi >= 1.0 { source_dpi } else { 300.0 };
    if options.any() && dpi < 300.0 {
        let scale = f64::from(300.0 / dpi);
        current = resize_bilinear(&current, scale)?;
        inverse = compose(&inverse, &InverseTransform::scale(1.0 / scale));
        dpi = 300.0;
    }
    // (b) Orientation: turn 90/180/270 scans upright (opt-in, skipped for small images).
    if options.auto_rotate {
        let (w, h) = (current.width, current.height);
        let turns = detect_orientation(&current);
        if turns != 0 {
            current = rotate_quarter_turns(&current, turns);
            inverse = compose(&inverse, &inverse_quarter_turns(turns, w, h));
        }
    }
    // (c) Deskew: straighten skewed lines (opt-in; no-op under 64 px, blank, or without a
    // clear margin or below 0.1°).
    if options.deskew
        && let Some(angle) = estimate_skew(&current)
    {
        let (straightened, back) = deskew_rotate(&current, angle);
        current = straightened;
        inverse = compose(&inverse, &back);
    }
    // (d) Denoise: 3×3 median with edge clamping (opt-in; dimensions never change).
    if options.denoise {
        current = median3(&current);
    }
    // (e) Binarize: Sauvola (window 8, k 0.34) via an integral image (opt-in); dark ink stays
    // dark and paper stays light.
    if options.binarize {
        current = sauvola(&current);
    }
    Ok(Preprocessed { image: current, inverse_transform: inverse, dpi })
}

/// An affine map `(x, y) → (a·x + c·y + e, b·x + d·y + f)`, composed in f64 so mapping a
/// word box back is exact up to float rounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InverseTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl InverseTransform {
    pub fn identity() -> InverseTransform {
        InverseTransform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }

    fn scale(s: f64) -> InverseTransform {
        InverseTransform { a: s, b: 0.0, c: 0.0, d: s, e: 0.0, f: 0.0 }
    }

    /// `compose(outer, inner)` maps `p` to `outer(inner(p))`: the accumulated inverse of the
    /// earlier steps is the outer map (their spaces come later in the round trip), so each
    /// step calls `compose(&inverse, &this_step_inverse)`. The scale step, for instance,
    /// happens first, so its inverse is applied last: `scale⁻¹(rotate⁻¹(p))`.
    fn compose_(outer: &InverseTransform, inner: &InverseTransform) -> InverseTransform {
        InverseTransform {
            a: outer.a * inner.a + outer.c * inner.b,
            b: outer.b * inner.a + outer.d * inner.b,
            c: outer.a * inner.c + outer.c * inner.d,
            d: outer.b * inner.c + outer.d * inner.d,
            e: outer.a * inner.e + outer.c * inner.f + outer.e,
            f: outer.b * inner.e + outer.d * inner.f + outer.f,
        }
    }

    /// Map a point from the processed image back to the input raster.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// Map a word box `[left, top, right, bottom]` back to the input raster: the four corners
    /// go through [`Self::apply`] and their bounding box is rounded outward to whole pixels.
    /// A box with no finite corners comes back unchanged (garbage in, garbage out — never a
    /// panic).
    pub fn map_rect(&self, rect: [f32; 4]) -> [f32; 4] {
        let corners = [(rect[0], rect[1]), (rect[2], rect[1]), (rect[0], rect[3]), (rect[2], rect[3])];
        let mut lo = (f64::INFINITY, f64::INFINITY);
        let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut finite = false;
        for (x, y) in corners {
            if !(x.is_finite() && y.is_finite()) {
                continue;
            }
            let (mx, my) = self.apply(f64::from(x), f64::from(y));
            if mx.is_finite() && my.is_finite() {
                finite = true;
                lo = (lo.0.min(mx), lo.1.min(my));
                hi = (hi.0.max(mx), hi.1.max(my));
            }
        }
        if !finite {
            return rect;
        }
        [lo.0.floor() as f32, lo.1.floor() as f32, hi.0.ceil() as f32, hi.1.ceil() as f32]
    }
}

/// The step inverses compose in pipeline order: each new step's inverse is applied *after*
/// the inverses accumulated so far.
fn compose(outer: &InverseTransform, inner: &InverseTransform) -> InverseTransform {
    InverseTransform::compose_(outer, inner)
}

/// The size `width` x `height` becomes under `scale`, refused (before any buffer exists) when
/// a side is under one pixel or over [`MAX_SIDE`], or the area is over [`MAX_PIXELS`]: two
/// sides under the side cap can still ask for hundreds of megabytes, which `OcrImage::new`
/// would only refuse after they were allocated.
fn scaled_size(width: u32, height: u32, scale: f64) -> Result<(u32, u32), OcrError> {
    let new_w = (f64::from(width) * scale).ceil();
    let new_h = (f64::from(height) * scale).ceil();
    if !(1.0..=f64::from(MAX_SIDE)).contains(&new_w) || !(1.0..=f64::from(MAX_SIDE)).contains(&new_h) {
        return Err(OcrError::ImageTooLarge);
    }
    let (nw, nh) = (new_w as u32, new_h as u32);
    if (nw as usize).checked_mul(nh as usize).is_none_or(|pixels| pixels > MAX_PIXELS as usize) {
        return Err(OcrError::ImageTooLarge);
    }
    Ok((nw, nh))
}

/// Bilinear resize by `scale`, clamped at the edges; the result stays inside [`scaled_size`]'s
/// limits (the side cap and the area cap).
fn resize_bilinear(image: &OcrImage, scale: f64) -> Result<OcrImage, OcrError> {
    let (nw, nh) = scaled_size(image.width, image.height, scale)?;
    let mut out = vec![255u8; (nw as usize).checked_mul(nh as usize).and_then(|p| p.checked_mul(4)).ok_or(OcrError::ImageTooLarge)?];
    for (y, row) in out.chunks_exact_mut(4 * nw as usize).enumerate() {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            // The source position of the destination pixel's centre, and the four
            // neighbours bilinear interpolation blends (positions clamp at the edges).
            let sx = (x as f64 + 0.5) / scale - 0.5;
            let sy = (y as f64 + 0.5) / scale - 0.5;
            let (x0, y0) = (sx.floor().max(0.0), sy.floor().max(0.0));
            let (fx, fy) = (sx - x0, sy - y0);
            for (ch, o) in px.iter_mut().enumerate() {
                let blend = |dx: f64, dy: f64| f64::from(image.byte(x0 + dx, y0 + dy, ch));
                let v = blend(0.0, 0.0) * (1.0 - fx) * (1.0 - fy)
                    + blend(1.0, 0.0) * fx * (1.0 - fy)
                    + blend(0.0, 1.0) * (1.0 - fx) * fy
                    + blend(1.0, 1.0) * fx * fy;
                *o = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    OcrImage::new(nw, nh, out)
}

/// Grayscale (Rec.601 luma) of every pixel.
fn grayscale(image: &OcrImage) -> Vec<u8> {
    image.rgba.as_chunks::<4>().0.iter().map(|p| ((299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000).min(255) as u8).collect()
}

/// How the page must be turned (quarter turns, clockwise) to read upright.
type QuarterTurns = u32;

/// Detect the correction for pages scanned sideways or upside down. Method (documented,
/// panic-free, no OCR): ink is everything darker than the midpoint between the darkest and
/// lightest pixel; for each candidate correction (0/90/180/270 clockwise) the ink's row
/// profile is taken and scored by how sharply it bands (Σ Δ², the projection-profile score:
/// upright text lines band sharply, turned text does not). The best-banding axis wins when it
/// beats the other axis by a factor of 1.3; within the winning axis the line bands'
/// bottom-heaviness (real text carries more ink below each line's middle — baselines and
/// descenders) picks the quarter turn, with a 4%-of-ink margin. Anything less clear is a
/// no-op (`0`), and images under 32 px are never judged.
fn detect_orientation(image: &OcrImage) -> QuarterTurns {
    if image.min_side() < 32 {
        return 0;
    }
    let gray = grayscale(image);
    let (Some(lo), Some(hi)) = (gray.iter().min(), gray.iter().max()) else { return 0 };
    let t = (*lo as u32 + *hi as u32) / 2;
    let ink: Vec<bool> = gray.iter().map(|&g| (g as u32) < t).collect();
    let (w, h) = (image.width as usize, image.height as usize);
    let total: u64 = ink.iter().filter(|i| **i).count() as u64;
    if total == 0 {
        return 0;
    }
    // The ink grid rotated by k quarter turns (clockwise), as row profiles.
    let mut scores = [0.0f64; 4];
    let mut asyms = [0.0f64; 4];
    for k in 0u32..4 {
        let k = k as usize;
        let profile = rotated_row_profile(&ink, w, h, k as u32);
        scores[k] = profile.windows(2).map(|p| (p[1] as f64 - p[0] as f64).powi(2)).sum();
        asyms[k] = band_bottom_heaviness(&profile);
    }
    // 0 and 180 share one profile shape (reversed), as do 90 and 270: pick the axis first.
    let (vertical_pair, horizontal_pair) = ((scores[0], scores[2]), (scores[1], scores[3]));
    let row_axis = vertical_pair.0.max(vertical_pair.1);
    let column_axis = horizontal_pair.0.max(horizontal_pair.1);
    if row_axis >= column_axis {
        if row_axis < column_axis * 1.3 {
            return 0; // the axes are too close to call
        }
        if asyms[0] > 0.15 * total as f64 {
            0
        } else if asyms[2] > 0.15 * total as f64 {
            2
        } else {
            0 // the bands are not clearly bottom-heavy either way: no-op
        }
    } else {
        if column_axis < row_axis * 1.3 {
            return 0;
        }
        if asyms[1] > 0.15 * total as f64 {
            1
        } else if asyms[3] > 0.15 * total as f64 {
            3
        } else {
            0
        }
    }
}

/// The row profile of the ink grid after `k` quarter turns clockwise.
fn rotated_row_profile(ink: &[bool], w: usize, h: usize, k: u32) -> Vec<u64> {
    // The rotated image is `w` pixels tall for odd k and `h` for even k; its row index is
    // always the new y of the source pixel.
    let len = if k.is_multiple_of(2) { h } else { w };
    let mut profile = vec![0u64; len];
    for (y, row) in ink.chunks_exact(w).enumerate() {
        for (x, on) in row.iter().enumerate() {
            if !on {
                continue;
            }
            // Where does (x, y) land after k quarter turns clockwise?
            let ny = match k {
                1 => x,
                2 => h - 1 - y,
                3 => w - 1 - x,
                _ => y,
            };
            if let Some(p) = profile.get_mut(ny) {
                *p += 1;
            }
        }
    }
    profile
}

/// Sum over text-line bands of (ink in the band's lower half − ink in its upper half):
/// positive when the bands carry more ink near their bottoms, as real text does.
fn band_bottom_heaviness(profile: &[u64]) -> f64 {
    let mut asym = 0.0;
    let mut start: Option<usize> = None;
    for (i, &p) in profile.iter().chain(std::iter::once(&0)).enumerate() {
        match (start, p > 0) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                let mid = (s + i) / 2;
                let lower: u64 = profile.get(s..i).map_or(0, |b| b[mid.saturating_sub(s)..].iter().sum());
                let upper: u64 = profile.get(s..i).map_or(0, |b| b[..mid.saturating_sub(s)].iter().sum());
                asym += lower as f64 - upper as f64; // signed: a top-heavy band scores below zero
                start = None;
            }
            _ => {}
        }
    }
    asym
}

/// Rotate the raster `k` quarter turns clockwise (an exact pixel permutation).
fn rotate_quarter_turns(image: &OcrImage, k: QuarterTurns) -> OcrImage {
    let (w, h) = (image.width as usize, image.height as usize);
    let (nw, nh) = if k.is_multiple_of(2) { (image.width, image.height) } else { (image.height, image.width) };
    let mut out = vec![255u8; nw as usize * nh as usize * 4];
    for (y, row) in image.rgba.chunks_exact(w * 4).enumerate() {
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            let (nx, ny) = match k {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (y, w - 1 - x),
                _ => (x, y),
            };
            if let Some(dst) = out.get_mut((ny * nw as usize + nx) * 4..(ny * nw as usize + nx) * 4 + 4) {
                dst.copy_from_slice(px);
            }
        }
    }
    OcrImage::new(nw, nh, out).unwrap_or_else(|_| image.clone())
}

/// The inverse of a `k`-quarter-turn correction: where each corrected pixel came from.
fn inverse_quarter_turns(k: QuarterTurns, old_w: u32, old_h: u32) -> InverseTransform {
    let (w, h) = (f64::from(old_w), f64::from(old_h));
    match k {
        // Forward (content moves): (x, y) ↦ (h−1−y, x) for one turn clockwise.
        1 => InverseTransform { a: 0.0, b: -1.0, c: 1.0, d: 0.0, e: 0.0, f: h - 1.0 },
        2 => InverseTransform { a: -1.0, b: 0.0, c: 0.0, d: -1.0, e: w - 1.0, f: h - 1.0 },
        3 => InverseTransform { a: 0.0, b: 1.0, c: -1.0, d: 0.0, e: w - 1.0, f: 0.0 },
        _ => InverseTransform::identity(),
    }
}

/// Estimate the skew of the text lines, in degrees (−5…5, positive: lines run downhill to the
/// right). Method (documented, panic-free): for each candidate angle the ink is sheared into a
/// row profile (`r = y − (x − cx)·tan φ`); the projection-profile score (Σ profile²) peaks
/// where the shear realigns the lines. `None` — leave the page alone — when the image is under
/// 64 px on a side, blank, the winner is under 0.1°, or the winner does not clearly beat
/// leaving the page as is (score < 1.02 × the 0° score).
fn estimate_skew(image: &OcrImage) -> Option<f64> {
    if image.min_side() < 64 {
        return None;
    }
    let gray = grayscale(image);
    let (Some(lo), Some(hi)) = (gray.iter().min(), gray.iter().max()) else { return None };
    let t = (*lo as u32 + *hi as u32) / 2;
    let ink: Vec<bool> = gray.iter().map(|&g| (g as u32) < t).collect();
    if !ink.iter().any(|i| *i) {
        return None; // a blank page has no skew
    }
    let (w, h) = (image.width, image.height);
    let cx = f64::from(w) / 2.0;
    let max_shift = (f64::from(w) / 2.0 * 5f64.to_radians().tan()).ceil() as i64 + 2;
    let score = |phi_deg: f64| -> f64 {
        let slope = phi_deg.to_radians().tan();
        let mut profile = vec![0u64; (h as i64 + 2 * max_shift + 1) as usize];
        for (y, row) in ink.chunks_exact(w as usize).enumerate() {
            for (x, on) in row.iter().enumerate() {
                if !on {
                    continue;
                }
                let r = y as f64 - (x as f64 - cx) * slope;
                let idx = r.round() as i64 + max_shift;
                if let Some(p) = profile.get_mut(idx as usize) {
                    *p += 1;
                }
            }
        }
        profile.iter().map(|p| (*p as f64).powi(2)).sum()
    };
    let mut best = (0.0f64, score(0.0));
    let mut phi = -5.0;
    while phi <= 5.0 + 1e-9 {
        let s = score(phi);
        if s > best.1 {
            best = (phi, s);
        }
        phi += 0.25;
    }
    let (angle, best_score) = best;
    if angle.abs() < 0.1 || best_score < 1.02 * score(0.0) {
        return None; // straight enough, or no clear winner: no-op
    }
    Some(angle)
}

/// Rotate the content by `−angle` about the centre (the deskew correction), nearest-neighbour
/// with white fill; also returns the exact dest→src map (the correction's inverse).
fn deskew_rotate(image: &OcrImage, angle_deg: f64) -> (OcrImage, InverseTransform) {
    let phi = angle_deg.to_radians();
    let (cos, sin) = (phi.cos(), phi.sin());
    let (w, h) = (image.width, image.height);
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let back = InverseTransform { a: cos, b: sin, c: -sin, d: cos, e: cx - cos * cx + sin * cy, f: cy - sin * cx - cos * cy };
    let mut out = vec![255u8; w as usize * h as usize * 4];
    for (y, row) in out.chunks_exact_mut(w as usize * 4).enumerate() {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (sx, sy) = back.apply(x as f64, y as f64);
            let (xi, yi) = (sx.round() as i64, sy.round() as i64);
            if xi < 0 || yi < 0 || xi >= w as i64 || yi >= h as i64 {
                continue; // outside the scan: white fill
            }
            if let Some(src) = image.rgba.get((yi as usize * w as usize + xi as usize) * 4..(yi as usize * w as usize + xi as usize) * 4 + 4) {
                px.copy_from_slice(src);
            }
        }
    }
    (OcrImage::new(w, h, out).unwrap_or_else(|_| image.clone()), back)
}

/// 3×3 median filter on each colour channel, edges clamped (border pixels replicate);
/// dimensions and alpha are unchanged.
fn median3(image: &OcrImage) -> OcrImage {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut out = image.rgba.clone();
    for (i, dst) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        let mut rgba = [255u8; 3];
        for (ch, v) in rgba.iter_mut().enumerate() {
            let mut window = [0u8; 9];
            for (n, (dx, dy)) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].iter().enumerate() {
                let nx = ((x as i64) + dx).clamp(0, w as i64 - 1) as usize;
                let ny = ((y as i64) + dy).clamp(0, h as i64 - 1) as usize;
                window[n] = image.byte(nx as f64, ny as f64, ch);
            }
            window.sort_unstable();
            *v = window[4];
        }
        let alpha = dst[3];
        dst.copy_from_slice(&[rgba[0], rgba[1], rgba[2], alpha]);
    }
    OcrImage::new(image.width, image.height, out).unwrap_or_else(|_| image.clone())
}

/// The Sauvola ink mask: local thresholding with the documented window — 8 pixels wide, the
/// 8 pixels from −4 to +3 (the end is exclusive, so `x0..x1` is exactly `x−4..=x+3`), k 0.34 —
/// through an integral image. A pixel is ink when its grey is at or under the local
/// threshold.
fn sauvola_mask(image: &OcrImage) -> Vec<bool> {
    let (w, h) = (image.width as usize, image.height as usize);
    let gray = grayscale(image);
    // Integral sums and square sums, (w+1)×(h+1), so any window sum is four lookups.
    let stride = w + 1;
    let mut sums = vec![0u64; stride * (h + 1)];
    let mut squares = vec![0u64; stride * (h + 1)];
    for (y, row) in gray.chunks_exact(w).enumerate() {
        let (mut row_sum, mut row_sq) = (0u64, 0u64);
        for (x, &g) in row.iter().enumerate() {
            row_sum += u64::from(g);
            row_sq += u64::from(g) * u64::from(g);
            let above = sums.get(y * stride + x + 1).copied().unwrap_or(0);
            let above_sq = squares.get(y * stride + x + 1).copied().unwrap_or(0);
            if let (Some(s), Some(sq)) = (sums.get_mut((y + 1) * stride + x + 1), squares.get_mut((y + 1) * stride + x + 1)) {
                *s = above + row_sum;
                *sq = above_sq + row_sq;
            }
        }
    }
    let (k, r) = (0.34f64, 128.0f64);
    let half = 4usize;
    let mut ink = vec![false; w * h];
    for (y, row) in ink.chunks_exact_mut(w).enumerate() {
        for (x, dst) in row.iter_mut().enumerate() {
            let (x0, y0) = (x.saturating_sub(half), y.saturating_sub(half));
            // Exclusive end: the window covers −4..=+3, 8 pixels on a side (an end of
            // `x + half + 1` would make it 9 and disagree with the documented window).
            let (x1, y1) = ((x + half).min(w), (y + half).min(h));
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let sum = rect_sum(&sums, stride, x0, y0, x1, y1);
            let sq = rect_sum(&squares, stride, x0, y0, x1, y1);
            let mean = sum / n;
            let var = (sq / n - mean * mean).max(0.0);
            let threshold = mean * (1.0 + k * (var.sqrt() / r - 1.0));
            *dst = matches!(gray.get(y * w + x), Some(&g) if f64::from(g) <= threshold);
        }
    }
    ink
}

/// Sauvola binarization through [`sauvola_mask`], followed by a one-pixel dilation of the ink:
/// ink goes to black, paper to white, alpha preserved.
fn sauvola(image: &OcrImage) -> OcrImage {
    let (w, h) = (image.width as usize, image.height as usize);
    // One dilation keeps anti-aliased strokes whole: a thresholded edge pixel that flaked off
    // rejoins its stroke (slightly bold beats broken, for reading).
    let ink = dilate1(&sauvola_mask(image), w, h);
    let mut out = image.rgba.clone();
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let v = if ink.get(i).copied().unwrap_or(false) { 0u8 } else { 255u8 };
        if let Some(dst) = px.get_mut(..3) {
            dst.copy_from_slice(&[v, v, v]);
        }
    }
    OcrImage::new(image.width, image.height, out).unwrap_or_else(|_| image.clone())
}

/// Grow the ink mask by one pixel (8-neighbourhood, edges clamped by the bounds checks).
fn dilate1(ink: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut out = ink.to_vec();
    for y in 0..h {
        for x in 0..w {
            if !ink.get(y * w + x).copied().unwrap_or(false) {
                continue;
            }
            for dy in [-1isize, 0, 1] {
                for dx in [-1isize, 0, 1] {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    if let Some(o) = out.get_mut(ny as usize * w + nx as usize) {
                        *o = true;
                    }
                }
            }
        }
    }
    out
}

fn rect_sum(table: &[u64], stride: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
    let a = table.get(y1 * stride + x1).copied().unwrap_or(0);
    let b = table.get(y0 * stride + x1).copied().unwrap_or(0);
    let c = table.get(y1 * stride + x0).copied().unwrap_or(0);
    let d = table.get(y0 * stride + x0).copied().unwrap_or(0);
    (a + d - b - c) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sides under MAX_SIDE whose area passes MAX_PIXELS (9500 × 9500, 90 MP) are refused
    /// when the size is worked out, before the 360 MB buffer is allocated rather than after.
    #[test]
    fn an_upscale_over_the_area_cap_is_refused_before_allocating() {
        assert!(matches!(scaled_size(1000, 1000, 9.5), Err(OcrError::ImageTooLarge)));
        assert_eq!(scaled_size(1000, 1000, 2.0).unwrap(), (2000, 2000), "under the caps it still scales");
        assert_eq!(scaled_size(8000, 8000, 1.0).unwrap(), (8000, 8000), "exactly 64 MP is allowed");
        assert!(matches!(scaled_size(8001, 8000, 1.0), Err(OcrError::ImageTooLarge)), "one row over is not");
        assert!(matches!(scaled_size(1000, 1000, 10.5), Err(OcrError::ImageTooLarge)), "a side over MAX_SIDE");
        assert!(matches!(scaled_size(1000, 1000, 0.0), Err(OcrError::ImageTooLarge)), "a side under one pixel");
        assert!(matches!(scaled_size(1000, 1000, f64::NAN), Err(OcrError::ImageTooLarge)));
        let small = OcrImage::new(100, 100, vec![255; 100 * 100 * 4]).unwrap();
        assert!(matches!(resize_bilinear(&small, 100.5), Err(OcrError::ImageTooLarge)));
        assert_eq!(resize_bilinear(&small, 2.0).unwrap().width, 200);
    }

    fn white(w: u32, h: u32) -> OcrImage {
        OcrImage::new(w, h, vec![255; (w * h * 4) as usize]).unwrap()
    }

    /// Ink bounding box of a raster (None when blank).
    fn ink_bbox(image: &OcrImage) -> Option<[f32; 4]> {
        let gray = grayscale(image);
        let (Some(lo), Some(hi)) = (gray.iter().min(), gray.iter().max()) else { return None };
        let t = (*lo as u32 + *hi as u32) / 2;
        let (mut l, mut tp, mut r, mut b) = (u32::MAX, u32::MAX, 0u32, 0u32);
        let mut any = false;
        for (i, &g) in gray.iter().enumerate() {
            if (g as u32) >= t {
                continue;
            }
            any = true;
            let (x, y) = ((i as u32) % image.width, (i as u32) / image.width);
            l = l.min(x);
            r = r.max(x);
            tp = tp.min(y);
            b = b.max(y);
        }
        any.then_some([l as f32, tp as f32, r as f32, b as f32])
    }

    /// A horizontal line of three bottom-heavy block glyphs (L shapes) on white — the fixture
    /// for orientation detection (240 × 64 at 300 dpi).
    fn l_line() -> OcrImage {
        let (w, h) = (240u32, 64u32);
        let mut img = white(w, h);
        for gx in [20u32, 100, 180] {
            fill(&mut img, gx, 12, 12, 40);
            fill(&mut img, gx, 40, 36, 12);
        }
        img
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

    /// The same fixture pre-rotated by k quarter turns clockwise, built independently of the
    /// crate's rotation (so the test cannot pass by a shared bug).
    fn rotated(k: u32) -> OcrImage {
        let src = l_line();
        let (w, h) = (src.width as usize, src.height as usize);
        let (nw, nh) = if k.is_multiple_of(2) { (src.width, src.height) } else { (src.height, src.width) };
        let mut out = white(nw, nh);
        for (y, row) in src.rgba.chunks_exact(w * 4).enumerate() {
            for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
                if px[0] != 0 {
                    continue;
                }
                let (nx, ny) = match k {
                    1 => (h - 1 - y, x),
                    2 => (w - 1 - x, h - 1 - y),
                    3 => (y, w - 1 - x),
                    _ => (x, y),
                };
                fill(&mut out, nx as u32, ny as u32, 1, 1);
            }
        }
        out
    }

    /// A ruled fixture sheared by `degrees` (lines running downhill to the right for positive
    /// angles): four 8-px bars across a 300 × 120 page.
    fn sheared(degrees: f64) -> OcrImage {
        let (w, h) = (300u32, 120u32);
        let mut img = white(w, h);
        let slope = degrees.to_radians().tan();
        for (base, height) in [(20u32, 8u32), (45, 8), (70, 8), (95, 8)] {
            for x in 20..280u32 {
                let y = (base as f64 + (x as f64 - 150.0) * slope).round().clamp(0.0, f64::from(h - height - 1)) as u32;
                fill(&mut img, x, y, 1, height);
            }
        }
        img
    }

    /// With the four opt-in flags off and a dpi at or above 300, the raster passes through
    /// untouched and the inverse transform is exact.
    #[test]
    fn flags_off_and_adequate_dpi_is_an_identity_pass_through() {
        let src = l_line();
        let p = preprocess(src.clone(), 300.0, &PreprocessOptions::default()).unwrap();
        assert_eq!(p.image.rgba, src.rgba);
        assert_eq!(p.dpi, 300.0);
        assert_eq!(p.inverse_transform, InverseTransform::identity());
        assert_eq!(p.inverse_transform.map_rect([3.0, 7.0, 41.0, 19.0]), [3.0, 7.0, 41.0, 19.0]);
    }

    /// With a step on, a scan below 300 dpi is enlarged by 300/src first, and its boxes map
    /// back to the original raster.
    #[test]
    fn low_dpi_scans_are_upscaled_for_the_steps_and_boxes_map_back() {
        let src = white(100, 50);
        let on = PreprocessOptions { denoise: true, ..Default::default() };
        let p = preprocess(src.clone(), 150.0, &on).unwrap();
        assert_eq!((p.image.width, p.image.height), (200, 100));
        assert_eq!(p.dpi, 300.0);
        assert_eq!(p.inverse_transform.map_rect([0.0, 0.0, 200.0, 100.0]), [0.0, 0.0, 100.0, 50.0]);
        assert_eq!(p.inverse_transform.apply(2.0, 4.0), (1.0, 2.0));
        // 300 and 600 dpi are not resampled; a broken dpi is left alone.
        assert_eq!(preprocess(src.clone(), 600.0, &on).unwrap().image.width, 100);
        assert_eq!(preprocess(src, f32::NAN, &on).unwrap().image.width, 100);
    }

    /// With every step off a low-dpi render is read exactly as rendered: not resampled, with
    /// the identity as its way back. (Reading a 72 dpi scan through a 4x enlargement changed
    /// what the models read: "due: \€250" came back as "ue: ?250".)
    #[test]
    fn with_every_step_off_a_low_dpi_render_is_not_resampled() {
        let src = l_line();
        let p = preprocess(src.clone(), 72.0, &PreprocessOptions::default()).unwrap();
        assert_eq!(p.image.rgba, src.rgba);
        assert_eq!((p.image.width, p.image.height), (src.width, src.height));
        assert_eq!(p.dpi, 72.0, "the render's own resolution");
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// The upscale refuses to break the side cap.
    #[test]
    fn upscaling_respects_the_side_cap() {
        let src = white(9999, 10);
        let r = preprocess(src, 72.0, &PreprocessOptions { denoise: true, ..Default::default() });
        assert!(matches!(r, Err(OcrError::ImageTooLarge)), "upscale past the cap: {r:?}");
    }

    /// An upright page is left alone by the orientation detector.
    #[test]
    fn upright_pages_are_not_rotated() {
        let p = preprocess(l_line(), 300.0, &PreprocessOptions { auto_rotate: true, ..Default::default() }).unwrap();
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// Pages scanned at 90/180/270 are detected, turned upright, and their word boxes map back
    /// onto the scanned raster's ink.
    #[test]
    fn quarter_turns_are_detected_corrected_and_mapped_back() {
        for k in 1..=3 {
            let scanned = rotated(k);
            let p = preprocess(scanned.clone(), 300.0, &PreprocessOptions { auto_rotate: true, ..Default::default() }).unwrap();
            let corrected = ink_bbox(&p.image).unwrap_or_else(|| panic!("k={k}: the corrected page still has ink"));
            // The corrected reading is a horizontal line again: wider than tall.
            assert!(corrected[2] - corrected[0] > corrected[3] - corrected[1], "k={k}: {corrected:?}");
            // And the box maps back onto the scanned raster's own ink box.
            let back = p.inverse_transform.map_rect(corrected);
            let original = ink_bbox(&scanned).unwrap();
            for i in 0..4 {
                assert!((back[i] - original[i]).abs() <= 2.0, "k={k}: {back:?} vs {original:?}");
            }
        }
    }

    /// Images under 32 px on a side are never judged; the step is a no-op.
    #[test]
    fn tiny_images_skip_orientation() {
        let src = white(31, 31);
        let p = preprocess(src, 300.0, &PreprocessOptions { auto_rotate: true, ..Default::default() }).unwrap();
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// Blank pages are inconclusive: every step leaves them alone.
    #[test]
    fn blank_pages_are_a_noop() {
        let src = white(200, 200);
        let p = preprocess(src, 300.0, &PreprocessOptions { auto_rotate: true, deskew: true, denoise: true, binarize: true }).unwrap();
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// The skew estimator sees a 3° shear (either direction) and leaves a straight page alone.
    #[test]
    fn skew_is_estimated_and_straight_pages_are_left_alone() {
        for degrees in [3.0, -3.0] {
            let angle = estimate_skew(&sheared(degrees)).unwrap_or_else(|| panic!("{degrees}° should be found"));
            assert!((angle - degrees).abs() <= 0.5, "{degrees}°: got {angle}");
        }
        assert!(estimate_skew(&sheared(0.0)).is_none(), "a straight page is a no-op");
        assert!(estimate_skew(&l_line()).is_none(), "glyph lines without a shear are a no-op");
    }

    /// Deskew straightens ±3° lines (the vertical span tightens) and the centre maps back to
    /// itself.
    #[test]
    fn deskew_corrects_plus_and_minus_three_degrees() {
        for degrees in [3.0, -3.0] {
            let src = sheared(degrees);
            let before = ink_bbox(&src).unwrap();
            let p = preprocess(src.clone(), 300.0, &PreprocessOptions { deskew: true, ..Default::default() }).unwrap();
            let after = ink_bbox(&p.image).unwrap();
            assert!(after[3] - after[1] < before[3] - before[1], "{degrees}°: {before:?} → {after:?}");
            // The exact inverse maps the corrected centre back to the scanned centre.
            let (cx, cy) = (f64::from(src.width) / 2.0, f64::from(src.height) / 2.0);
            let (bx, by) = p.inverse_transform.apply(cx, cy);
            assert!((bx - cx).abs() < 1.0 && (by - cy).abs() < 1.0, "{degrees}°: ({bx}, {by})");
        }
    }

    /// Deskew no-ops on small images (under 64 px) — the detector is never consulted.
    #[test]
    fn small_images_skip_deskew() {
        let src = white(63, 63);
        let p = preprocess(src, 300.0, &PreprocessOptions { deskew: true, ..Default::default() }).unwrap();
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// The median filter removes isolated specks, keeps solid strokes, and never changes the
    /// dimensions.
    #[test]
    fn denoise_removes_specks_and_keeps_dimensions() {
        let mut src = white(32, 32);
        fill(&mut src, 15, 15, 1, 1); // salt
        fill(&mut src, 4, 4, 5, 5); // a solid stroke
        let p = preprocess(src, 300.0, &PreprocessOptions { denoise: true, ..Default::default() }).unwrap();
        assert_eq!((p.image.width, p.image.height), (32, 32));
        assert_eq!(p.image.byte(15.0, 15.0, 0), 255, "an isolated speck is gone");
        assert_eq!(p.image.byte(6.0, 6.0, 0), 0, "the inside of a solid block stays");
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// Sauvola keeps the polarity (dark ink stays dark, paper stays light) and the metadata.
    /// The fixture is a thin grey stroke: its windows mix ink and paper, so the local
    /// threshold separates them (a perfectly flat mid-tone area has no contrast and whitens,
    /// which is Sauvola working as designed).
    #[test]
    fn binarize_keeps_polarity_and_metadata() {
        let (w, h) = (64u32, 64u32);
        let mut src = white(w, h);
        for y in 26..30 {
            for x in 8..56 {
                let o = ((y * w + x) * 4) as usize;
                if let Some(p) = src.rgba.get_mut(o..o + 3) {
                    p.copy_from_slice(&[70, 70, 70]); // a grey scan's ink, far below paper
                }
            }
        }
        let p = preprocess(src, 300.0, &PreprocessOptions { binarize: true, ..Default::default() }).unwrap();
        assert_eq!((p.image.width, p.image.height), (w, h));
        assert_eq!(p.dpi, 300.0);
        assert_eq!(p.image.byte(30.0, 28.0, 0), 0, "ink stays dark");
        assert_eq!(p.image.byte(2.0, 2.0, 0), 255, "paper stays light");
        for px in p.image.rgba.as_chunks::<4>().0.iter() {
            assert!((px[0] == 0 || px[0] == 255) && px[0] == px[1] && px[1] == px[2], "{px:?}");
        }
    }

    /// The inverse transform composes: an upscaled, turned, deskewed page maps corrected
    /// points back inside the original raster.
    #[test]
    fn inverse_transform_maps_points_back_through_every_step() {
        let scanned = rotated(1); // a page photographed on its side
        let p = preprocess(scanned.clone(), 150.0, &PreprocessOptions { auto_rotate: true, deskew: true, ..Default::default() }).unwrap();
        let (w, h) = (f64::from(p.image.width), f64::from(p.image.height));
        for (x, y) in [(0.0, 0.0), (w - 1.0, 0.0), (0.0, h - 1.0), (w - 1.0, h - 1.0), (w / 2.0, h / 2.0)] {
            let (bx, by) = p.inverse_transform.apply(x, y);
            assert!((-2.0..=f64::from(scanned.width) + 2.0).contains(&bx), "x {x} → {bx}");
            assert!((-2.0..=f64::from(scanned.height) + 2.0).contains(&by), "y {y} → {by}");
        }
        // The scale step, alone, is exact.
        assert_eq!(InverseTransform::scale(0.5).apply(4.0, 10.0), (2.0, 5.0));
    }

    /// A realistic letter page (one line of type on white, at 300 dpi) reads as upright: the
    /// row axis bands decisively sharper than the column axis, and the near-balanced
    /// bottom-heaviness of real mixed-case text is below the 15% flip bar, so the page is left
    /// alone rather than risk flipping a good scan upside down.
    #[test]
    fn realistic_letter_pages_are_left_upright() {
        let (w, h) = (1200u32, 1650u32);
        let mut img = white(w, h);
        let mut x = 120u32;
        let base = 300u32;
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };
        while x < 1000 {
            let word = rnd() % 5 + 2;
            for _ in 0..word {
                let kind = rnd() % 4;
                let wl = rnd() % 8 + 3;
                match kind {
                    0 => fill(&mut img, x, base - 20, wl, 20), // x-height
                    1 => fill(&mut img, x, base - 34, wl, 34), // ascender
                    2 => fill(&mut img, x, base - 20, wl, 28), // descender
                    _ => {
                        fill(&mut img, x, base - 20, 2, 20);
                        fill(&mut img, x + 3, base - 14, 2, 14); // stems
                    }
                }
                x += wl + 2;
            }
            x += 12; // space
        }
        assert_eq!(detect_orientation(&img), 0);
        let p = preprocess(img, 300.0, &PreprocessOptions { auto_rotate: true, ..Default::default() }).unwrap();
        assert_eq!(p.inverse_transform, InverseTransform::identity());
    }

    /// Hostile inputs are errors or no-ops, never panics: 1×1 rasters, all-black pages, NaN
    /// boxes through the inverse map.
    #[test]
    fn hostile_inputs_never_panic() {
        let tiny = white(1, 1);
        let all = PreprocessOptions { auto_rotate: true, deskew: true, denoise: true, binarize: true };
        let p = preprocess(tiny, 300.0, &all).unwrap();
        assert_eq!((p.image.width, p.image.height), (1, 1));
        // All-black: no paper, nothing to judge.
        let mut black = white(80, 80);
        for px in black.rgba.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[0, 0, 0, 255]);
        }
        let p = preprocess(black, 300.0, &all).unwrap();
        assert_eq!(p.inverse_transform, InverseTransform::identity(), "all-black is inconclusive");
        // NaN boxes never poison downstream math: the finite corners bound the mapped box,
        // and a box with no finite corners comes back unchanged.
        let mapped = p.inverse_transform.map_rect([f32::NAN, 0.0, 5.0, 5.0]);
        assert!(mapped.iter().all(|v| v.is_finite()), "{mapped:?}");
        let unchanged = InverseTransform::identity().map_rect([f32::NAN; 4]);
        assert!(unchanged.iter().all(|v| v.is_nan()), "{unchanged:?}");
    }

    /// The one-pixel dilation is load-bearing, pinned with a fixture that fragments without
    /// it: a thin anti-aliased-looking stroke on a grey scan (alternating dark and light
    /// pixels) — the raw Sauvola mask drops the light runs (the local window sits on a grey
    /// page, so the threshold falls below them), while the binarized image keeps the stroke
    /// whole.
    #[test]
    fn binarize_without_the_dilation_the_stroke_fragments_and_with_it_stays_whole() {
        let (w, h) = (64u32, 64u32);
        let mut src = white(w, h);
        let set = |img: &mut OcrImage, x: u32, y: u32, v: u8| {
            let o = ((y * w + x) * 4) as usize;
            if let Some(p) = img.rgba.get_mut(o..o + 3) {
                p.copy_from_slice(&[v, v, v]);
            }
        };
        for px in src.rgba.as_chunks_mut::<4>().0.iter_mut() {
            px[..3].copy_from_slice(&[210, 210, 210]); // a grey scan's paper
        }
        // The stroke: one pixel tall, alternating dark (reads as ink) and light (an
        // anti-aliased shoulder the threshold alone drops) in runs of two.
        for x in 8..56u32 {
            let v = if (x / 2) % 2 == 0 { 110 } else { 170 };
            set(&mut src, x, 32, v);
        }
        let mask = sauvola_mask(&src);
        let at = |x: u32, y: u32| mask[y as usize * w as usize + x as usize];
        let dropped: Vec<u32> = (12..52).filter(|&x| !at(x, 32)).collect();
        assert!(!dropped.is_empty(), "without the dilation the stroke fragments: these stroke pixels fall under the threshold at {dropped:?}");
        let p = preprocess(src, 300.0, &PreprocessOptions { binarize: true, ..Default::default() }).unwrap();
        // With the dilation the stroke is whole: every position on it has ink within its
        // 8-neighbourhood.
        for x in 12..=51u32 {
            let ink_near = (-1i32..=1).any(|dy| {
                (-1i32..=1).any(|dx| {
                    let (nx, ny) = (x as i32 + dx, 32 + dy);
                    p.image.byte(f64::from(nx), f64::from(ny), 0) == 0
                })
            });
            assert!(ink_near, "the stroke is broken at x={x}");
        }
    }

    /// The deskew inverse, checked at OFF-CENTRE points (the centre maps to itself under any
    /// rotation, so it cannot catch a wrong sign): a point on a straightened line maps back
    /// onto the sheared source line, uphill and downhill.
    #[test]
    fn deskew_inverse_maps_off_centre_points_back_onto_the_source_line() {
        for degrees in [3.0, -3.0] {
            let src = sheared(degrees);
            let p = preprocess(src, 300.0, &PreprocessOptions { deskew: true, ..Default::default() }).unwrap();
            let slope = degrees.to_radians().tan();
            // The corrected first line sits at y = 20 (the fixture's base); the same ink was
            // at 20 + (x − 150)·slope in the source. Well off centre on purpose. The
            // rotation moves x a little too (it mixes in y: at most sin(5°)·40 ≈ 3.5 px
            // here), so the line is checked at the mapped x — a wrong sign still misses it
            // by ±(x−150)·slope, 6+ px at these offsets.
            for x in [30.0f64, 150.0, 270.0] {
                let (sx, sy) = p.inverse_transform.apply(x, 20.0);
                assert!((sx - x).abs() < 4.0, "{degrees}° x={x}: mapped to ({sx}, {sy})");
                let expected = 20.0 + (sx - 150.0) * slope;
                assert!((sy - expected).abs() < 2.0, "{degrees}° x={x}: ({sx}, {sy}), the source line is at {expected}");
            }
        }
    }
}
