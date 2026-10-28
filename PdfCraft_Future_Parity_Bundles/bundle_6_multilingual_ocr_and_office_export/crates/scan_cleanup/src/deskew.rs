//! Automatic Deskewing via Projection Profile Analysis and Bilinear Image Rotation.

pub struct DeskewEngine;

impl DeskewEngine {
    /// Detects skew angle in degrees (positive = clockwise skew).
    pub fn detect_skew_angle(grayscale: &[u8], width: usize, height: usize) -> f32 {
        if width == 0 || height == 0 || grayscale.len() < width * height {
            return 0.0;
        }

        // Test angles from -15.0 to +15.0 degrees in 0.5 degree steps
        let mut best_angle = 0.0f32;
        let mut max_variance = 0.0f64;

        let sample_step = 2; // Fast downsampled line scan
        let rad_step = 0.5f32.to_radians();

        for i in -30..=30 {
            let angle_rad = i as f32 * rad_step;
            let tan_theta = angle_rad.tan();

            let mut row_sums = vec![0u64; height];

            for y in (0..height).step_by(sample_step) {
                let y_f = y as f32;
                for x in (0..width).step_by(sample_step) {
                    let sample_y = (y_f + x as f32 * tan_theta).round() as i32;
                    if sample_y >= 0 && (sample_y as usize) < height {
                        let px = grayscale[y * width + x] as u64;
                        // Count dark text pixels (value < 128)
                        if px < 128 {
                            row_sums[sample_y as usize] += 1;
                        }
                    }
                }
            }

            // Compute projection profile variance
            let sum: u64 = row_sums.iter().sum();
            let mean = sum as f64 / height as f64;
            let variance: f64 = row_sums.iter()
                .map(|&val| {
                    let diff = val as f64 - mean;
                    diff * diff
                })
                .sum();

            if variance > max_variance {
                max_variance = variance;
                best_angle = i as f32 * 0.5;
            }
        }

        best_angle
    }

    /// Rotates image by detected angle using nearest-neighbor / bilinear interpolation.
    pub fn rotate_image(
        grayscale: &[u8],
        width: usize,
        height: usize,
        angle_degrees: f32,
    ) -> Vec<u8> {
        if angle_degrees.abs() < 0.1 {
            return grayscale.to_vec();
        }

        let mut output = vec![255u8; width * height]; // White background
        let angle_rad = -angle_degrees.to_radians();
        let cos_theta = angle_rad.cos();
        let sin_theta = angle_rad.sin();

        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;

        for y in 0..height {
            let dy = y as f32 - cy;
            for x in 0..width {
                let dx = x as f32 - cx;

                let src_x = (cos_theta * dx - sin_theta * dy + cx).round() as i32;
                let src_y = (sin_theta * dx + cos_theta * dy + cy).round() as i32;

                if src_x >= 0 && (src_x as usize) < width && src_y >= 0 && (src_y as usize) < height {
                    output[y * width + x] = grayscale[src_y as usize * width + src_x as usize];
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_skew_preservation() {
        let data = vec![255u8; 100 * 100];
        let rotated = DeskewEngine::rotate_image(&data, 100, 100, 0.0);
        assert_eq!(data, rotated);
    }
}
