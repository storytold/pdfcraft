//! Bounded scan geometry. Corrections fit inside the page, never crop away an edge.
use image::{Rgb, RgbImage, imageops};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Correction {
    /// Clockwise quarter-turn, applied first.
    pub rotation: u16,
    /// Clockwise fine rotation, in degrees (-12..12).
    pub deskew: f64,
}

impl Correction {
    pub fn changed(self) -> bool {
        self.rotation != 0 || self.deskew.abs() >= 0.05
    }
    pub fn scale(self, width: f64, height: f64) -> f64 {
        let (s, c) = self.deskew.to_radians().sin_cos();
        (width / (width * c.abs() + height * s.abs())).min(height / (width * s.abs() + height * c.abs()))
    }
    /// Corrected pixel coordinates back to the original rendered page (edge coordinates).
    pub fn to_source(self, x: f64, y: f64, width: f64, height: f64) -> [f64; 2] {
        let (w, h) = if self.rotation % 180 == 90 { (height, width) } else { (width, height) };
        let k = self.scale(w, h);
        let (s, c) = self.deskew.to_radians().sin_cos();
        let (dx, dy) = ((x - w / 2.0) / k, (y - h / 2.0) / k);
        let (x, y) = (c * dx + s * dy + w / 2.0, -s * dx + c * dy + h / 2.0);
        match self.rotation {
            90 => [y, height - x],
            180 => [width - x, height - y],
            270 => [width - y, x],
            _ => [x, y],
        }
    }
    pub fn apply(self, image: &RgbImage) -> RgbImage {
        let turned = match self.rotation {
            90 => imageops::rotate90(image),
            180 => imageops::rotate180(image),
            270 => imageops::rotate270(image),
            _ => image.clone(),
        };
        if self.deskew.abs() < 0.05 {
            return turned;
        }
        let (w, h) = turned.dimensions();
        let inverse = Self { rotation: 0, ..self };
        RgbImage::from_fn(w, h, |x, y| {
            let [sx, sy] = inverse.to_source(x as f64 + 0.5, y as f64 + 0.5, w as f64, h as f64);
            let (sx, sy) = (sx - 0.5, sy - 0.5);
            let (x0, y0) = (sx.floor() as i64, sy.floor() as i64);
            let (fx, fy) = (sx - sx.floor(), sy - sy.floor());
            let sample = |dx: i64, dy: i64| {
                if x0 + dx >= 0 && y0 + dy >= 0 {
                    turned.get_pixel_checked((x0 + dx) as u32, (y0 + dy) as u32).copied().unwrap_or(Rgb([255; 3]))
                } else {
                    Rgb([255; 3])
                }
            };
            let p = [sample(0, 0), sample(1, 0), sample(0, 1), sample(1, 1)];
            Rgb(std::array::from_fn(|c| {
                ((p[0][c] as f64 * (1.0 - fx) + p[1][c] as f64 * fx) * (1.0 - fy) + (p[2][c] as f64 * (1.0 - fx) + p[3][c] as f64 * fx) * fy).round()
                    as u8
            }))
        })
    }
}

/// Projection-profile deskew, at bounded resolution. Blank/photo-like pages and weak peaks
/// are left alone. Search both directions and refine the strongest horizontal text baseline.
pub fn deskew(image: &RgbImage) -> f64 {
    let ratio = 1000.0 / image.width().max(image.height()).max(1) as f64;
    let small = if ratio < 1.0 {
        imageops::resize(
            image,
            (image.width() as f64 * ratio).round().max(1.0) as u32,
            (image.height() as f64 * ratio).round().max(1.0) as u32,
            imageops::FilterType::Triangle,
        )
    } else {
        image.clone()
    };
    let (w, h) = small.dimensions();
    // Remove edge-connected dark scanner frames from the ANALYSIS mask, including
    // their slanted portions after correction. Cropping/rotation retain all pixels.
    // The mask is bounded by the 1000-pixel analysis size, as is the flood-fill stack.
    let (mx, my) = (w / 50, h / 50);
    let mut ink: Vec<bool> = small.pixels().map(|p| p.0.iter().copied().map(u32::from).sum::<u32>() < 420).collect();
    let mut stack = Vec::new();
    for (x, y, _) in small.enumerate_pixels() {
        if (x < mx || x >= w - mx || y < my || y >= h - my)
            && let Some(dark) = ink.get_mut((y * w + x) as usize)
            && *dark
        {
            *dark = false;
            stack.push((x, y));
        }
    }
    while let Some((x, y)) = stack.pop() {
        for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
            for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                if let Some(dark) = ink.get_mut((ny * w + nx) as usize)
                    && *dark
                {
                    *dark = false;
                    stack.push((nx, ny));
                }
            }
        }
    }
    let points: Vec<(f64, f64)> = ink
        .iter()
        .enumerate()
        .filter(|(_, dark)| **dark)
        .map(|(i, _)| ((i as u32 % w) as f64 - w as f64 / 2.0, (i as u32 / w) as f64 - h as f64 / 2.0))
        .collect();
    let fraction = points.len() as f64 / (w as f64 * h as f64).max(1.0);
    if points.len() < 100 || !(0.002..0.35).contains(&fraction) {
        return 0.0;
    }
    let stride = points.len().div_ceil(100_000).max(1);
    let bins = (w + h + 4) as usize;
    let score = |degrees: f64| {
        let (s, c) = degrees.to_radians().sin_cos();
        let mut rows = vec![0u32; bins];
        for &(x, y) in points.iter().step_by(stride) {
            let bin = (s * x + c * y + bins as f64 / 2.0).round() as usize;
            if let Some(n) = rows.get_mut(bin) {
                *n += 1;
            }
        }
        rows.iter().map(|&n| (n as f64).powi(2)).sum::<f64>()
    };
    let baseline = score(0.0);
    let (mut best, mut value) = (0.0, baseline);
    for n in -48..=48 {
        let a = n as f64 / 4.0;
        let v = score(a);
        if v > value {
            best = a;
            value = v;
        }
    }
    if best.abs() >= 12.0 || value < baseline * 1.15 {
        return 0.0;
    }
    let coarse = best;
    for n in -5..=5 {
        let a = coarse + n as f64 * 0.05;
        let v = score(a);
        if v > value {
            best = a;
            value = v;
        }
    }
    if best.abs() < 0.15 { 0.0 } else { best }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blank_and_solid_images_are_not_deskewed() {
        for color in [0, 255] {
            assert_eq!(deskew(&RgbImage::from_pixel(600, 800, Rgb([color; 3]))), 0.0);
        }
    }
    #[test]
    fn deskew_finds_both_signs_and_keeps_corners() {
        let image = RgbImage::from_fn(600, 800, |x, y| {
            if (80..520).contains(&x) && (80..720).contains(&y) && y % 40 < 4 && x % 30 < 22 { Rgb([0; 3]) } else { Rgb([255; 3]) }
        });
        for angle in [-5.0, 3.0] {
            let mut tilted = Correction { rotation: 0, deskew: angle }.apply(&image);
            assert!((deskew(&tilted) + angle).abs() < 0.2, "{}", deskew(&tilted));
            for (x, y, pixel) in tilted.enumerate_pixels_mut() {
                if x < 5 || y < 5 || x >= 595 || y >= 795 {
                    *pixel = Rgb([25; 3]);
                }
            }
            assert!((deskew(&tilted) + angle).abs() < 0.2, "scanner frame: {}", deskew(&tilted));
            let corrected = Correction { rotation: 0, deskew: -angle }.apply(&tilted);
            assert!(deskew(&corrected).abs() < 0.2, "rotated scanner frame: {}", deskew(&corrected));
        }
        assert_eq!(Correction { rotation: 90, deskew: 0.0 }.to_source(0.0, 0.0, 600.0, 800.0), [0.0, 800.0]);
    }
}
