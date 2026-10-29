//! Greyscale conversion for page rasters (ClearScan stage 1).
//!
//! The preprocessing and recognition pipeline works on one luma channel: Sauvola's local means
//! and the projection profiles of [`crate::deskew`] are defined on greys. Input rasters arrive
//! the same way [`crate::Ocr::recognize`] takes them: a flat byte buffer of `width` × `height`
//! pixels with 1 (grey), 3 (RGB) or 4 (RGBA) bytes per pixel, row-major from the top-left.
//!
//! Luma uses the integer Rec. 601 weights (the same split ocrs' own image preparation uses),
//! so a pixel's darkness agrees with what the recognition model will see.

/// At most this many pixels are preprocessed in one go: a 600 DPI scan of A4 (34.8 MP) or
/// Letter (33.7 MP) fits, with room for the deskew rotation to grow the canvas; anything
/// finer than that is rejected while the buffers stay bounded (grey + binary + one rotated
/// copy ≈ 3 × 42 MB).
pub const MAX_PIXELS: u64 = 41_943_040;

/// A greyscale image: `width` × `height` bytes, row-major, top-left origin, 0 = black.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreyImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl GreyImage {
    /// The empty (0×0) image, for failing soft when a buffer cannot match its canvas.
    pub(crate) fn empty() -> GreyImage {
        GreyImage { width: 0, height: 0, data: Vec::new() }
    }

    /// A grey image from a checked buffer. `data.len()` must be `width` × `height`.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<GreyImage> {
        if u64::from(width) * u64::from(height) != data.len() as u64 {
            return None;
        }
        Some(GreyImage { width, height, data })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The grey values, mutable. Crate-internal: tests draw and degrade synthetic scans
    /// with it; code outside the tests builds images through [`to_grey`].
    #[cfg(test)]
    pub(crate) fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The grey value at `(x, y)`, or white outside the image (edges repeat when a filter
    /// window reaches past the border, and out-of-range reads fail soft).
    pub fn get(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return u8::MAX;
        }
        let index = y as usize * self.width as usize + x as usize;
        self.data.get(index).copied().unwrap_or(u8::MAX)
    }
}

/// Convert a flat pixel buffer to greyscale. `channels` is 1 (grey), 3 (RGB) or 4 (RGBA).
pub fn to_grey(pixels: &[u8], width: u32, height: u32, channels: u32) -> Result<GreyImage, String> {
    if width == 0 || height == 0 || pixels.is_empty() {
        return Err("the image is empty".into());
    }
    if !matches!(channels, 1 | 3 | 4) {
        return Err(format!("expected 1, 3 or 4 bytes per pixel, got {channels}"));
    }
    let pixel_count = u64::from(width) * u64::from(height);
    if pixel_count > MAX_PIXELS {
        return Err(format!("the image has {pixel_count} pixels; at most {MAX_PIXELS} are preprocessed"));
    }
    let expected = pixel_count.saturating_mul(u64::from(channels)) as usize;
    if pixels.len() != expected {
        return Err(format!("expected {expected} bytes for {width}×{height} at {channels} bytes per pixel, got {}", pixels.len()));
    }
    let luma = |p: &[u8]| -> u8 {
        // Rec. 601 integer luma, the JPEG/ITU rounding: (77r + 150g + 29b + 128) >> 8.
        let [r, g, b] = [u16::from(p[0]), u16::from(p[1]), u16::from(p[2])];
        ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8
    };
    let data = match channels {
        1 => pixels.to_vec(),
        3 => pixels.as_chunks::<3>().0.iter().map(|p| luma(p)).collect(),
        _ => pixels.as_chunks::<4>().0.iter().map(|p| luma(p)).collect(),
    };
    Ok(GreyImage { width, height, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_three_and_four_channels_agree() {
        let rgb = [10_u8, 200, 100, 40, 40, 240];
        let three = to_grey(&rgb, 2, 1, 3).unwrap();
        let four = to_grey(&[10, 200, 100, 255, 40, 40, 240, 255], 2, 1, 4).unwrap();
        assert_eq!(three, four);
        // Rec. 601: (77·10 + 150·200 + 29·100 + 128) >> 8 = 33798 >> 8 = 132.
        assert_eq!(three.data()[0], 132);
        assert_eq!(three.data()[1], ((77_u32 * 40 + 150 * 40 + 29 * 240 + 128) >> 8) as u8);
    }

    #[test]
    fn a_grey_input_is_taken_as_is() {
        let grey = to_grey(&[0, 255, 128], 3, 1, 1).unwrap();
        assert_eq!((grey.width(), grey.height(), grey.data()), (3, 1, &[0, 255, 128][..]));
    }

    #[test]
    fn out_of_range_reads_are_white() {
        let grey = to_grey(&[7; 4], 2, 2, 1).unwrap();
        assert_eq!(grey.get(-1, 0), 255);
        assert_eq!(grey.get(2, 2), 255);
        assert_eq!(grey.get(1, 1), 7);
    }

    #[test]
    fn hostile_shapes_are_errors_not_panics() {
        assert_eq!(to_grey(&[], 0, 0, 1).unwrap_err(), "the image is empty");
        assert!(to_grey(&[1, 2], 2, 1, 2).unwrap_err().contains("got 2"));
        assert!(to_grey(&[1, 2, 3], 2, 1, 3).unwrap_err().contains("got 3"));
        // 6000 × 8193 (49.2 MP) is over the 40 MP cap.
        assert!(to_grey(&[0; 4], 6000, 8193, 1).unwrap_err().contains("at most"));
    }

    #[test]
    fn mismatched_buffer_lengths_name_the_numbers() {
        let err = to_grey(&[1, 2, 3, 4, 5, 6, 7], 2, 2, 1).unwrap_err();
        assert!(err.contains("expected 4") && err.contains("got 7"), "{err}");
    }

    #[test]
    fn new_rejects_a_buffer_that_does_not_match_its_shape() {
        assert!(GreyImage::new(2, 2, vec![1, 2, 3]).is_none());
        assert!(GreyImage::new(2, 2, vec![1, 2, 3, 4]).is_some());
    }
}
