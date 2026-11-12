//! Deskew detection and rectification (ClearScan stage 1).
//!
//! A scan lies a degree or two off the feed rollers. Text that is not level smears every
//! later stage: connected components fuse across lines, baselines drift, projection profiles
//! blur. This module measures the tilt and turns the page rectangular first.
//!
//! **Detection** is projection-profile variance, the classic measure for printed pages: when
//! the image is summed along a direction, text lines collapse into sharp peaks if that
//! direction is along the lines, and smear flat if it is not. The variance (equivalently, the
//! sum of squares) of the binned projection is maximal over the true text direction, so the
//! angle is found by scoring a coarse grid over ±15° (1° steps), refining ±1° around the best
//! (0.1° steps) and taking the parabolic vertex of the peak for sub-step precision. Ink
//! weight is a pixel's darkness (255 − grey), kept only where Otsu's threshold on the
//! downscaled copy calls it ink, so the paper of an unevenly lit scan weighs nothing; no
//! full-resolution binarization pass runs first.
//!
//! **Rectification** rotates the greyscale about the image centre by the opposite angle with
//! bilinear sampling, on a canvas grown to keep every corner (rotation never crops), the
//! outside filled with paper white. The [`Affine`] that maps deskewed pixels back to original
//! pixels is kept: everything found in the deskewed frame (bounding boxes, components) maps
//! back onto the scanned raster through it.
//!
//! All coordinates are pixel space, x right, y down. A positive `angle_radians` means the text
//! descends to the right; rectification rotates by `-angle_radians`.

use crate::binarize::otsu_threshold;
use crate::grey::{GreyImage, MAX_PIXELS};

/// The largest angle [`estimate_skew`] looks for, in degrees (the far end of a bad feed).
pub const DEFAULT_MAX_SKEW_DEGREES: f64 = 15.0;
/// The long side of the downscaled copy detection works on: enough resolution for 0.1°
/// precision on a scan, a million-odd pixels of work whatever the page.
const DETECTION_MAX_SIDE: u32 = 1024;
/// Darkness below which a pixel cannot be ink for detection (weights below this only add
/// noise, whatever the threshold said).
const INK_WEIGHT_FLOOR: f64 = 8.0;

/// An affine map `x' = a·x + c·y + e`, `y' = b·x + d·y + f` on pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// Where `(x, y)` maps to.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// The inverse map, or `None` if this one is degenerate (not invertible).
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        // [a c e; b d f]⁻¹ with the translation unrotated first.
        let (ia, ic) = (self.d / det, -self.c / det);
        let (ib, id) = (-self.b / det, self.a / det);
        Some(Affine { a: ia, b: ib, c: ic, d: id, e: -(ia * self.e + ic * self.f), f: -(ib * self.e + id * self.f) })
    }

    fn rotation_about(angle: f64, from_center: (f64, f64), to_center: (f64, f64)) -> Affine {
        // p' = C_to + R(angle)·(p − C_from), R as the standard matrix [[cos, −sin],[sin, cos]].
        let (sin, cos) = angle.sin_cos();
        Affine {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: to_center.0 - (cos * from_center.0 - sin * from_center.1),
            f: to_center.1 - (sin * from_center.0 + cos * from_center.1),
        }
    }
}

/// One ink-weighted pixel of the downscaled detection copy: `(x, y, weight)`.
pub(crate) struct InkPixel(f64, f64, f64);

/// How much the detection copy is downscaled by: the long side fits
/// [`DETECTION_MAX_SIDE`], at least 1:1.
fn detection_scale(width: u32, height: u32) -> u32 {
    ((width.max(height) as f64) / f64::from(DETECTION_MAX_SIDE)).ceil().max(1.0) as u32
}

/// The box-averaged greyscale of `grey` downscaled until its long side fits
/// [`DETECTION_MAX_SIDE`], with the scale that was used.
fn downscaled(grey: &GreyImage) -> (GreyImage, u32) {
    let (width, height) = (grey.width(), grey.height());
    let scale = detection_scale(width, height);
    let (down_w, down_h) = (width.div_ceil(scale), height.div_ceil(scale));
    let mut data = vec![0_u8; (down_w * down_h) as usize];
    for dy in 0..down_h {
        for dx in 0..down_w {
            let (x0, y0) = (dx * scale, dy * scale);
            let (x1, y1) = ((x0 + scale).min(width), (y0 + scale).min(height));
            let mut sum = 0_u64;
            let mut count = 0_u64;
            for y in y0..y1 {
                let row = y as usize * width as usize;
                for x in x0..x1 {
                    sum += u64::from(grey.data().get(row + x as usize).copied().unwrap_or(u8::MAX));
                    count += 1;
                }
            }
            // `count` is zero only for an empty box, which cannot be reached (the grid
            // spans the image); the checked division fails soft to paper white regardless.
            let average = sum.checked_div(count).unwrap_or(u64::from(u8::MAX)).min(u64::from(u8::MAX));
            let average = u8::try_from(average).unwrap_or(u8::MAX);
            if let Some(slot) = data.get_mut(dy as usize * down_w as usize + dx as usize) {
                *slot = average;
            }
        }
    }
    // The buffer was just sized to the canvas, so this cannot fail; fail soft as empty.
    let down = GreyImage::new(down_w, down_h, data).unwrap_or_else(GreyImage::empty);
    (down, scale)
}

/// The ink pixels of the downscaled copy: those Otsu calls ink, weighted by darkness. A page
/// without a threshold (single grey level) or with nothing dark enough has no ink, and reads
/// as level.
fn ink_pixels(grey: &GreyImage) -> Vec<InkPixel> {
    let (down, _) = downscaled(grey);
    let Some(threshold) = otsu_threshold(&down) else {
        return Vec::new();
    };
    let mut ink = Vec::new();
    for dy in 0..down.height() {
        for dx in 0..down.width() {
            let grey_level = down.get(dx as i64, dy as i64);
            let weight = f64::from(u8::MAX - grey_level);
            if u32::from(grey_level) < u32::from(threshold) && weight >= INK_WEIGHT_FLOOR {
                ink.push(InkPixel(f64::from(dx), f64::from(dy), weight));
            }
        }
    }
    ink
}

/// The spread of `r(x, y) = −x·sin a + y·cos a` over the downscaled image, for sizing bins.
fn projection_span(a: f64, width: u32, height: u32) -> (f64, f64) {
    let (sin, cos) = a.sin_cos();
    let r = |x: f64, y: f64| -x * sin + y * cos;
    let corners = [
        r(0.0, 0.0),
        r(f64::from(width.saturating_sub(1)), 0.0),
        r(0.0, f64::from(height.saturating_sub(1))),
        r(f64::from(width.saturating_sub(1)), f64::from(height.saturating_sub(1))),
    ];
    let min = corners.iter().copied().fold(f64::INFINITY, f64::min);
    let max = corners.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (min, max)
}

/// The projection-profile score of angle `a`: ink weight binned along the axis perpendicular
/// to direction `(cos a, sin a)`, summed squared. Sharper text lines, higher score.
fn projection_score(ink: &[InkPixel], a: f64, width: u32, height: u32) -> f64 {
    let (min_r, max_r) = projection_span(a, width, height);
    let span = (max_r - min_r).max(0.0);
    if span.is_finite() && span <= f64::EPSILON {
        return 0.0;
    }
    // One bin per unit of r; a bin's width is 1 by construction and the count is bounded by
    // the span, itself bounded by the (downscaled) image diagonal.
    let bins = (span + 2.0) as usize;
    let mut counts = vec![0.0_f64; bins];
    let (sin, cos) = a.sin_cos();
    for InkPixel(x, y, weight) in ink {
        let r = -x * sin + y * cos - min_r;
        if !r.is_finite() {
            continue;
        }
        let bin = r.floor() as i64;
        if let Ok(index) = usize::try_from(bin)
            && let Some(count) = counts.get_mut(index)
        {
            *count += weight;
        }
    }
    counts.iter().map(|c| c * c).sum()
}

/// Score every angle in `from`..=`to` by `step` and return the best.
fn best_angle(ink: &[InkPixel], width: u32, height: u32, from: f64, to: f64, step: f64) -> (f64, f64) {
    let mut best = (from, projection_score(ink, from, width, height));
    let mut angle = from + step;
    while angle <= to + step / 2.0 {
        let score = projection_score(ink, angle, width, height);
        if score > best.1 {
            best = (angle, score);
        }
        angle += step;
    }
    best
}

/// The parabolic vertex through `(−step, y₋), (0, y₀), (step, y₊)`, offset from the centre.
fn parabolic_vertex(y_minus: f64, y_zero: f64, y_plus: f64) -> f64 {
    let denominator = y_minus - 2.0 * y_zero + y_plus;
    if !denominator.is_finite() || denominator.abs() < 1e-12 {
        return 0.0;
    }
    let delta = 0.5 * (y_minus - y_plus) / denominator;
    if delta.is_finite() { delta.clamp(-1.0, 1.0) } else { 0.0 }
}

/// Estimate the page's skew: the angle, in radians, by which its text lines descend to the
/// right (negative: they rise). `max_angle_degrees` bounds the search (1°…45°). A page with
/// no ink reads as level.
pub fn estimate_skew(grey: &GreyImage, max_angle_degrees: f64) -> f64 {
    let max = max_angle_degrees.clamp(1.0, 45.0).to_radians();
    let ink = ink_pixels(grey);
    if ink.is_empty() {
        return 0.0;
    }
    let scale = detection_scale(grey.width(), grey.height());
    let (down_w, down_h) = (grey.width().div_ceil(scale), grey.height().div_ceil(scale));
    // Coarse: a degree at a time across the whole range. Fine: a tenth of a degree around
    // the peak, then the parabolic vertex for what lies between the grid points.
    let (coarse, score) = best_angle(&ink, down_w, down_h, -max, max, 1.0_f64.to_radians());
    // A page whose projections score nothing everywhere (a single pixel, say) has no
    // direction to find: it reads as level rather than tilting to the range's first entry.
    if score <= f64::EPSILON {
        return 0.0;
    }
    let fine_step = 0.1_f64.to_radians();
    let (fine, _) = best_angle(&ink, down_w, down_h, coarse - 1.0_f64.to_radians(), coarse + 1.0_f64.to_radians(), fine_step);
    let score = |a: f64| projection_score(&ink, a, down_w, down_h);
    let (y_minus, y_zero, y_plus) = (score(fine - fine_step), score(fine), score(fine + fine_step));
    (fine + parabolic_vertex(y_minus, y_zero, y_plus) * fine_step).clamp(-max, max)
}

/// The size a canvas needs to hold `grey` rotated by `angle` about its centre, with a margin
/// for sampling at the edges, or `None` if the rotated page would exceed [`MAX_PIXELS`].
fn rotated_size(width: u32, height: u32, angle: f64) -> Option<(u32, u32)> {
    let (sin, cos) = angle.sin_cos();
    let corners = [(0.0, 0.0), (f64::from(width), 0.0), (0.0, f64::from(height)), (f64::from(width), f64::from(height))];
    let rotated: Vec<(f64, f64)> = corners.iter().map(|&(x, y)| (x * cos - y * sin, x * sin + y * cos)).collect();
    let min_x = rotated.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let max_x = rotated.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let min_y = rotated.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_y = rotated.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let span_x = (max_x - min_x).ceil().max(0.0);
    let span_y = (max_y - min_y).ceil().max(0.0);
    if !(span_x.is_finite() && span_y.is_finite()) {
        return None;
    }
    // Two pixels of margin keep bilinear samples at the new edges off the very last source
    // pixel, and round the size up to whole pixels.
    let out_w = span_x as u64 + 2;
    let out_h = span_y as u64 + 2;
    if out_w.saturating_mul(out_h) > MAX_PIXELS || out_w > u32::MAX as u64 || out_h > u32::MAX as u64 {
        return None;
    }
    Some((out_w as u32, out_h as u32))
}

/// Bilinear sample of `grey` at continuous `(x, y)`, paper white outside the image.
fn sample_bilinear(grey: &GreyImage, x: f64, y: f64) -> u8 {
    if !x.is_finite() || !y.is_finite() {
        return u8::MAX;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let at = |px: i64, py: i64| f64::from(grey.get(px, py));
    let top = at(x0 as i64, y0 as i64) * (1.0 - fx) + at(x0 as i64 + 1, y0 as i64) * fx;
    let bottom = at(x0 as i64, y0 as i64 + 1) * (1.0 - fx) + at(x0 as i64 + 1, y0 as i64 + 1) * fx;
    let value = top * (1.0 - fy) + bottom * fy;
    // Interpolation can leave the range by rounding; clamp back into a byte.
    value.round().clamp(0.0, f64::from(u8::MAX)) as u8
}

/// A deskewed page: the rotated greyscale, the angle it was levelled by, and the affine that
/// maps deskewed pixel coordinates back onto the original raster.
#[derive(Clone, Debug)]
pub struct Deskewed {
    pub grey: GreyImage,
    pub angle_radians: f64,
    pub to_original: Affine,
}

/// Rotate `grey` level by `angle_radians` (the opposite of the tilt: see [`estimate_skew`]).
///
/// The canvas grows to keep every corner of the page, the outside is paper white, and the
/// mapping back to the original raster is returned with the result. Fails, without
/// allocating, when the rotated page would not fit [`MAX_PIXELS`].
pub fn rectify(grey: &GreyImage, angle_radians: f64) -> Result<Deskewed, String> {
    let (width, height) = (grey.width(), grey.height());
    if width == 0 || height == 0 {
        return Err("the image is empty".into());
    }
    let rotate_by = -angle_radians;
    let (out_w, out_h) = rotated_size(width, height, rotate_by)
        .ok_or_else(|| format!("rotating the page by {angle_radians:.3} radians would not fit {MAX_PIXELS} pixels"))?;
    // Deskewed pixel p maps back to the original raster through the inverse of the rotation
    // the content was given: p_original = C_in + R(angle)·(p − C_out).
    let from_center = ((f64::from(width) - 1.0) / 2.0, (f64::from(height) - 1.0) / 2.0);
    let to_center = ((f64::from(out_w) - 1.0) / 2.0, (f64::from(out_h) - 1.0) / 2.0);
    let to_original = Affine::rotation_about(angle_radians, to_center, from_center);
    let mut data = vec![0_u8; out_w as usize * out_h as usize];
    for y in 0..out_h {
        for x in 0..out_w {
            let (sx, sy) = to_original.apply(f64::from(x), f64::from(y));
            let index = y as usize * out_w as usize + x as usize;
            if let Some(slot) = data.get_mut(index) {
                *slot = sample_bilinear(grey, sx, sy);
            }
        }
    }
    let rotated = GreyImage::new(out_w, out_h, data).ok_or("the rotated image does not fit its canvas")?;
    Ok(Deskewed { grey: rotated, angle_radians, to_original })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of text-like ink: rows of word blocks with x-height and line gaps, tilted by
    /// `angle`. Word blocks, not glyph strokes: for the projection profile, what matters is
    /// ink concentrated in line bands with clean gaps between them.
    fn text_page(width: u32, height: u32, angle: f64) -> GreyImage {
        let mut data = vec![248_u8; (width * height) as usize];
        let (sin, cos) = angle.sin_cos();
        let put = |data: &mut [u8], x: i64, y: i64| {
            if x >= 0 && y >= 0 && (x as u64) < u64::from(width) && (y as u64) < u64::from(height) {
                let index = y as usize * width as usize + x as usize;
                data[index] = 40;
            }
        };
        // Six lines of seven words; every third word is an ascender, every fifth a descender.
        // The words sit on baselines tilted by `angle`: word k's origin is as far along its
        // line as its page-x, which is what makes a tilted page read as tilted.
        for line in 0..6_i64 {
            let line_y = 30 + line * 34;
            for word in 0..7_i64 {
                let word_x = 24 + word * 50;
                let x_height = 10;
                let (rise, sink) = match word % 15 {
                    2 => (4, 0), // an ascender
                    4 => (0, 4), // a descender
                    _ => (0, 0),
                };
                let along_line = word_x as f64 * angle.tan();
                for along in 0..38_i64 {
                    for down in -rise..(x_height + sink) {
                        let t = along as f64;
                        let u = down as f64;
                        let base_x = word_x as f64;
                        let base_y = line_y as f64 + along_line;
                        let x = base_x + t * cos - u * sin;
                        let y = base_y + t * sin + u * cos;
                        put(&mut data, x.round() as i64, y.round() as i64);
                    }
                }
            }
        }
        GreyImage::new(width, height, data).unwrap()
    }

    #[test]
    fn a_level_page_reads_as_level() {
        let page = text_page(360, 260, 0.0);
        let angle = estimate_skew(&page, DEFAULT_MAX_SKEW_DEGREES);
        assert!(angle.abs() < 0.002, "{angle}");
    }

    #[test]
    fn small_tilts_are_measured() {
        for tilt in [-6.5_f64, -1.0, 1.4, 3.2, 9.7] {
            let page = text_page(360, 260, tilt.to_radians());
            let found = estimate_skew(&page, DEFAULT_MAX_SKEW_DEGREES).to_degrees();
            assert!((found - tilt).abs() <= 0.12, "tilt {tilt}° read as {found}°");
        }
    }

    #[test]
    fn a_page_without_ink_is_level() {
        let data = vec![250_u8; 300 * 200];
        let grey = GreyImage::new(300, 200, data).unwrap();
        assert_eq!(estimate_skew(&grey, DEFAULT_MAX_SKEW_DEGREES), 0.0);
    }

    #[test]
    fn a_tiny_faint_page_is_level() {
        // Below the ink-weight floor there is nothing to project.
        let data = vec![250_u8; 3 * 2];
        let grey = GreyImage::new(3, 2, data).unwrap();
        assert_eq!(estimate_skew(&grey, DEFAULT_MAX_SKEW_DEGREES), 0.0);
    }

    #[test]
    fn a_single_pixel_is_level() {
        let grey = GreyImage::new(1, 1, vec![0]).unwrap();
        assert_eq!(estimate_skew(&grey, DEFAULT_MAX_SKEW_DEGREES), 0.0);
    }

    /// Rectification turns a tilted page into a level one, read back by estimation.
    #[test]
    fn rectification_levels_the_text() {
        let tilt = 3.2_f64.to_radians();
        let page = text_page(360, 260, tilt);
        let found = estimate_skew(&page, DEFAULT_MAX_SKEW_DEGREES);
        let deskewed = rectify(&page, found).unwrap();
        let again = estimate_skew(&deskewed.grey, DEFAULT_MAX_SKEW_DEGREES);
        assert!(again.abs() < 0.05, "still tilted by {again}");
    }

    #[test]
    fn the_canvas_grows_to_keep_the_page() {
        let page = text_page(200, 100, 0.0);
        let level = rectify(&page, 0.0).unwrap();
        assert_eq!((level.grey.width(), level.grey.height()), (202, 102), "the two margin pixels");
        let tilted = rectify(&page, 10.0_f64.to_radians()).unwrap();
        // The rotated 200×100 page spans 200·cos10° + 100·sin10° ≈ 214.3 wide, 133.2 tall.
        assert!(tilted.grey.width() >= 216 && tilted.grey.height() >= 134, "{:?}×{:?}", tilted.grey.width(), tilted.grey.height());
    }

    /// Rotation about the centre with bilinear sampling keeps ink mass: the ink in the deskewed
    /// page matches the ink in the original within interpolation spread.
    #[test]
    fn rotation_keeps_the_ink() {
        let page = text_page(300, 220, 0.0);
        let ink_before: f64 = page.data().iter().map(|&g| f64::from(u8::MAX - g)).sum();
        let deskewed = rectify(&page, 5.0_f64.to_radians()).unwrap();
        let ink_after: f64 = deskewed.grey.data().iter().map(|&g| f64::from(u8::MAX - g)).sum();
        assert!(ink_after >= ink_before * 0.95, "ink lost: {ink_before} → {ink_after}");
        assert!(ink_after <= ink_before * 1.05, "ink invented: {ink_before} → {ink_after}");
    }

    /// The affine maps deskewed pixels back onto the original raster: the deskewed centre is
    /// the original centre, and the composition with its inverse is the identity.
    #[test]
    fn the_transform_maps_back_onto_the_original() {
        let page = text_page(251, 181, 0.0);
        let deskewed = rectify(&page, 7.0_f64.to_radians()).unwrap();
        let (cx, cy) = deskewed.to_original.apply(f64::from(deskewed.grey.width() - 1) / 2.0, f64::from(deskewed.grey.height() - 1) / 2.0);
        let expected = ((f64::from(page.width()) - 1.0) / 2.0, (f64::from(page.height()) - 1.0) / 2.0);
        assert!((cx - expected.0).abs() < 1.0 && (cy - expected.1).abs() < 1.0, "centre went to ({cx}, {cy})");
        let inverse = deskewed.to_original.inverse().unwrap();
        for (x, y) in [(0.0_f64, 0.0), (100.0, 42.0), (250.0, 180.0)] {
            let (ux, uy) = deskewed.to_original.apply(x, y);
            let (rx, ry) = inverse.apply(ux, uy);
            assert!((rx - x).abs() < 1e-9 && (ry - y).abs() < 1e-9, "({x}, {y}) → ({ux}, {uy}) → ({rx}, {ry})");
        }
    }

    #[test]
    fn the_inverse_of_a_degenerate_map_is_none() {
        let zero = Affine { a: 0.0, b: 0.0, c: 0.0, d: 0.0, e: 0.0, f: 0.0 };
        assert_eq!(zero.inverse(), None);
        assert_eq!(Affine::IDENTITY.inverse(), Some(Affine::IDENTITY));
    }

    /// A single dark mark, rotated and mapped back, lands on itself.
    #[test]
    fn a_mark_maps_back_onto_itself() {
        let mut data = vec![255_u8; 61 * 61];
        data[30 * 61 + 30] = 0;
        let grey = GreyImage::new(61, 61, data).unwrap();
        let deskewed = rectify(&grey, 0.5_f64.to_radians()).unwrap();
        // Find the darkest deskewed pixel and map it home.
        let mut best = (u8::MAX, 0_usize);
        for (index, &g) in deskewed.grey.data().iter().enumerate() {
            if g < best.0 {
                best = (g, index);
            }
        }
        let (dx, dy) = ((best.1 % deskewed.grey.width() as usize) as f64, (best.1 / deskewed.grey.width() as usize) as f64);
        let (ox, oy) = deskewed.to_original.apply(dx, dy);
        assert!((ox - 30.0).abs() <= 1.0 && (oy - 30.0).abs() <= 1.0, "mark went to ({ox}, {oy})");
    }

    #[test]
    fn an_empty_image_is_an_error_not_a_crash() {
        let grey = GreyImage::new(0, 0, Vec::new()).unwrap();
        assert_eq!(rectify(&grey, 0.1).unwrap_err(), "the image is empty");
    }
}
