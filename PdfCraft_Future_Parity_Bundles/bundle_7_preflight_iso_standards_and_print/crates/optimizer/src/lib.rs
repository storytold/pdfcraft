//! PDF Optimizer: Image downsampling, stream recompression, and metadata scrubbing.

#[derive(Debug, Clone)]
pub struct OptimizationSettings {
    pub downsample_images_above_dpi: f32, // e.g. 225 DPI
    pub target_image_dpi: f32,            // e.g. 150 DPI
    pub recompress_flate: bool,
    pub discard_private_metadata: bool,
    pub discard_unused_fonts: bool,
}

impl Default for OptimizationSettings {
    fn default() -> Self {
        Self {
            downsample_images_above_dpi: 225.0,
            target_image_dpi: 150.0,
            recompress_flate: true,
            discard_private_metadata: true,
            discard_unused_fonts: true,
        }
    }
}

pub struct ImageDownsampler;

impl ImageDownsampler {
    /// Downsamples a 2D sample grid by a factor using area averaging.
    pub fn downsample_pixels(
        samples: &[u8],
        src_w: usize,
        src_h: usize,
        channels: usize,
        factor: usize,
    ) -> (Vec<u8>, usize, usize) {
        if factor <= 1 || src_w == 0 || src_h == 0 {
            return (samples.to_vec(), src_w, src_h);
        }

        let dst_w = (src_w / factor).max(1);
        let dst_h = (src_h / factor).max(1);
        let mut out = vec![0u8; dst_w * dst_h * channels];

        let block_area = (factor * factor) as u32;

        for dy in 0..dst_h {
            let sy_start = dy * factor;
            let sy_end = (sy_start + factor).min(src_h);

            for dx in 0..dst_w {
                let sx_start = dx * factor;
                let sx_end = (sx_start + factor).min(src_w);

                let dst_idx = (dy * dst_w + dx) * channels;

                for c in 0..channels {
                    let mut sum = 0u32;
                    let mut count = 0u32;

                    for sy in sy_start..sy_end {
                        let row_offset = sy * src_w * channels;
                        for sx in sx_start..sx_end {
                            let src_idx = row_offset + sx * channels + c;
                            if let Some(&val) = samples.get(src_idx) {
                                sum += val as u32;
                                count += 1;
                            }
                        }
                    }

                    out[dst_idx + c] = if count > 0 { (sum / count) as u8 } else { 0 };
                }
            }
        }

        (out, dst_w, dst_h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_downsampling() {
        // 4x4 image with 1 channel, downsampled by factor 2 -> 2x2
        let input = vec![
            100, 100, 200, 200,
            100, 100, 200, 200,
            0,   0,   50,  50,
            0,   0,   50,  50,
        ];
        let (out, dw, dh) = ImageDownsampler::downsample_pixels(&input, 4, 4, 1, 2);
        assert_eq!((dw, dh), (2, 2));
        assert_eq!(out, vec![100, 200, 0, 50]);
    }
}
