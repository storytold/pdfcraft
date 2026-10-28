//! Otsu Global and Sauvola Adaptive Binarization for Background Whitening.

pub struct BinarizationEngine;

impl BinarizationEngine {
    /// Computes optimal global threshold using Otsu's discriminant method.
    pub fn otsu_threshold(grayscale: &[u8]) -> u8 {
        if grayscale.is_empty() {
            return 128;
        }

        let mut histogram = [0u64; 256];
        for &val in grayscale {
            histogram[val as usize] += 1;
        }

        let total_pixels = grayscale.len() as f64;
        let mut sum_all = 0.0f64;
        for (i, &count) in histogram.iter().enumerate() {
            sum_all += i as f64 * count as f64;
        }

        let mut sum_b = 0.0f64;
        let mut w_b = 0.0f64;
        let mut max_var_between = 0.0f64;
        let mut optimal_threshold = 128u8;

        for (t, &count) in histogram.iter().enumerate() {
            w_b += count as f64;
            if w_b == 0.0 {
                continue;
            }
            let w_f = total_pixels - w_b;
            if w_f == 0.0 {
                break;
            }

            sum_b += t as f64 * count as f64;
            let m_b = sum_b / w_b;
            let m_f = (sum_all - sum_b) / w_f;

            let var_between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
            if var_between > max_var_between {
                max_var_between = var_between;
                optimal_threshold = t as u8;
            }
        }

        optimal_threshold
    }

    /// Applies Otsu binarization to clean up page background (0 = black text, 255 = clean white background).
    pub fn binarize_otsu(grayscale: &[u8]) -> Vec<u8> {
        let threshold = Self::otsu_threshold(grayscale);
        grayscale.iter()
            .map(|&val| if val <= threshold { 0 } else { 255 })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otsu_thresholding() {
        // Half dark pixels, half light pixels
        let mut data = vec![30u8; 500];
        data.extend_from_slice(&vec![220u8; 500]);
        let t = BinarizationEngine::otsu_threshold(&data);
        assert!(t >= 30 && t <= 220);

        let bin = BinarizationEngine::binarize_otsu(&data);
        assert_eq!(bin[0], 0);
        assert_eq!(bin[999], 255);
    }
}
