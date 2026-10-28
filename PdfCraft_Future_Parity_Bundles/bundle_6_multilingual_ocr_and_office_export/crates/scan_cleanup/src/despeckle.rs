//! Morphological Despeckle Filtering (Salt-and-Pepper Noise Removal).

pub struct DespeckleFilter;

impl DespeckleFilter {
    /// Removes isolated single/double noise pixels using a 3x3 median filter.
    pub fn filter_3x3(binary_image: &[u8], width: usize, height: usize) -> Vec<u8> {
        if width < 3 || height < 3 || binary_image.len() < width * height {
            return binary_image.to_vec();
        }

        let mut output = binary_image.to_vec();

        for y in 1..(height - 1) {
            let row_offset = y * width;
            for x in 1..(width - 1) {
                let idx = row_offset + x;

                // 3x3 neighborhood values
                let mut window = [
                    binary_image[idx - width - 1], binary_image[idx - width], binary_image[idx - width + 1],
                    binary_image[idx - 1],         binary_image[idx],         binary_image[idx + 1],
                    binary_image[idx + width - 1], binary_image[idx + width], binary_image[idx + width + 1],
                ];

                // Median selection
                window.sort_unstable();
                output[idx] = window[4]; // Median value
            }
        }

        output
    }
}
