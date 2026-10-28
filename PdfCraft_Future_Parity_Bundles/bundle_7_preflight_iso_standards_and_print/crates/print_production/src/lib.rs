//! Pre-press Print Inspection, CMYK Separation Previews, and Total Area Coverage (TAC).

pub struct CmykSeparationPlates {
    pub width: usize,
    pub height: usize,
    pub cyan_plate: Vec<u8>,
    pub magenta_plate: Vec<u8>,
    pub yellow_plate: Vec<u8>,
    pub black_plate: Vec<u8>,
}

pub struct PrintProductionEngine;

impl PrintProductionEngine {
    /// Extracts individual CMYK separation plates from 4-component CMYK pixel buffer.
    pub fn extract_plates(cmyk_pixels: &[u8], width: usize, height: usize) -> CmykSeparationPlates {
        let pixel_count = width * height;
        let mut cyan = Vec::with_capacity(pixel_count);
        let mut magenta = Vec::with_capacity(pixel_count);
        let mut yellow = Vec::with_capacity(pixel_count);
        let mut black = Vec::with_capacity(pixel_count);

        for chunk in cmyk_pixels.chunks_exact(4) {
            cyan.push(chunk[0]);
            magenta.push(chunk[1]);
            yellow.push(chunk[2]);
            black.push(chunk[3]);
        }

        CmykSeparationPlates {
            width,
            height,
            cyan_plate: cyan,
            magenta_plate: magenta,
            yellow_plate: yellow,
            black_plate: black,
        }
    }

    /// Computes Total Area Coverage (TAC) in percent for each pixel ($C\% + M\% + Y\% + K\%$).
    /// Returns mask of pixels exceeding the specified max limit (e.g. 300%).
    pub fn calculate_tac_violations(
        cmyk_pixels: &[u8],
        max_tac_percent: f32,
    ) -> Vec<bool> {
        let mut violations = Vec::with_capacity(cmyk_pixels.len() / 4);

        for chunk in cmyk_pixels.chunks_exact(4) {
            let c = (chunk[0] as f32 / 255.0) * 100.0;
            let m = (chunk[1] as f32 / 255.0) * 100.0;
            let y = (chunk[2] as f32 / 255.0) * 100.0;
            let k = (chunk[3] as f32 / 255.0) * 100.0;

            let total_tac = c + m + y + k;
            violations.push(total_tac > max_tac_percent);
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tac_calculation() {
        // Pixel 1: C=100%, M=100%, Y=100%, K=50% -> 350% TAC (violates 300%)
        // Pixel 2: C=50%, M=50%, Y=50%, K=50% -> 200% TAC (passes)
        let pixels = vec![
            255, 255, 255, 128,
            128, 128, 128, 128,
        ];

        let violations = PrintProductionEngine::calculate_tac_violations(&pixels, 300.0);
        assert_eq!(violations, vec![true, false]);
    }
}
