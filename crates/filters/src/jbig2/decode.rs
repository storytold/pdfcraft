//! JBIG2 decoding for `JBIG2Decode` (ISO 32000-2 §7.4.7), through `hayro-jbig2`.

use crate::FilterError;
use crate::ccitt::Rows;

/// The largest JBIG2 page [`decode_jbig2`] decodes, in pixels. The decoder decodes a page whole,
/// and takes pages of up to about this many (20,000 × 10,000 decodes).
pub const MAX_JBIG2_PIXELS: u64 = 1 << 28;

/// A `JBIG2Decode` stream's `data`, with what its `/JBIG2Globals` stream decodes to, decoded as the
/// filter gives it: `rows` rows of `columns` 1-bit samples, most significant bit first, each
/// starting on a new byte, with black as 0 (JBIG2 codes black as 1).
///
/// The JBIG2 page has to be the image's size, `columns` × `rows`: PdfCraft's renderer draws the
/// whole page into the image, so the rows of a page of another height wouldn't be the image's. The
/// decoder decodes the page whole, so it is checked before it is decoded, and has to fit in `max`
/// bytes as such rows and have at most [`MAX_JBIG2_PIXELS`]. Regions inside it are bounded by the
/// decoder alone: a region may claim more pixels than its page, and is decoded before it is cut to
/// the page. With no rows asked for, nothing is decoded.
///
/// Errors: `data` or `globals` that don't decode, or a page of another size
/// ([`FilterError::Corrupt`]); a page past `max` ([`FilterError::LimitExceeded`]) or
/// [`MAX_JBIG2_PIXELS`] ([`FilterError::Unsupported`]).
pub fn decode_jbig2(data: &[u8], globals: Option<&[u8]>, columns: u32, rows: u32, max: usize) -> Result<Vec<u8>, FilterError> {
    if rows == 0 {
        return Ok(Vec::new());
    }
    let corrupt = |detail: String| FilterError::Corrupt { filter: "JBIG2Decode", detail };
    let image = hayro_jbig2::Image::new_embedded(data, globals).map_err(|e| corrupt(e.to_string()))?;
    let (width, height) = (image.width(), image.height());
    if (width, height) != (columns, rows) {
        return Err(corrupt(format!("a page of {width} × {height} pixels for an image of {columns} × {rows}")));
    }
    if u64::from(width) * u64::from(height) > MAX_JBIG2_PIXELS {
        return Err(FilterError::Unsupported(format!("JBIG2 pages of {width} × {height} pixels")));
    }
    // u32 to usize: wasm32 and up have at least 32-bit usize.
    let (width, height) = (width as usize, height as usize);
    if width.div_ceil(8).checked_mul(height).is_none_or(|bytes| bytes > max) {
        return Err(FilterError::LimitExceeded(max));
    }
    let mut out = Rows::new(width, height, false);
    image.decode(&mut out).map_err(|e| corrupt(e.to_string()))?;
    Ok(out.out)
}

/// JBIG2 codes black as 1, which `JBIG2Decode` gives as 0.
impl hayro_jbig2::Decoder for Rows {
    fn push_pixel(&mut self, black: bool) {
        self.push(!black, 1);
    }
    fn push_pixel_chunk(&mut self, black: bool, chunks: u32) {
        self.push(!black, (chunks as usize).saturating_mul(8));
    }
    fn next_line(&mut self) {
        self.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_jbig2, encode_jbig2_generic};

    /// `rows` rows of `columns` samples of noise, the bits that pad each row to a byte 0.
    fn noise(columns: u32, rows: u32) -> Vec<u8> {
        let stride = (columns as usize).div_ceil(8);
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut data: Vec<u8> = (0..stride * rows as usize)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        let pad = (stride * 8 - columns as usize) as u32;
        data.chunks_mut(stride).for_each(|row| *row.last_mut().unwrap() &= 0xFFu8.checked_shl(pad).unwrap_or(0));
        data
    }

    /// The same stream with its page information (the first segment's data, after its 11-byte
    /// header: width, then height) claiming a page of `width` × `height`.
    fn page_of(data: &[u8], width: u32, height: u32) -> Vec<u8> {
        let mut data = data.to_vec();
        data[11..15].copy_from_slice(&width.to_be_bytes());
        data[15..19].copy_from_slice(&height.to_be_bytes());
        data
    }

    /// Either coding decodes to its samples, black as 0, the bits that pad each row 0: a generic
    /// region of noise, and symbols for letters repeated across the page. With no rows asked for,
    /// nothing is decoded.
    #[test]
    fn both_codings_decode_to_their_samples() {
        let (w, h) = (203u32, 41u32);
        let bits = noise(w, h);
        let generic = encode_jbig2_generic(&bits, w, h).unwrap();
        assert_eq!(decode_jbig2(&generic, None, w, h, usize::MAX).unwrap(), bits);
        assert_eq!(decode_jbig2(b"not JBIG2", None, w, 0, usize::MAX).unwrap(), b"", "no rows asked for");

        // Four lines of three letters, black on white.
        const LETTERS: [[&str; 7]; 3] = [
            ["#####", "#....", "####.", "#....", "#....", "#....", "#####"],
            [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
            ["#...#", "#...#", "#####", "#...#", "#...#", "#...#", "#...#"],
        ];
        let (w, h) = (400u32, 40u32);
        let stride = (w as usize).div_ceil(8);
        let mut marks = vec![0xFFu8; stride * h as usize];
        for (line, top) in [2usize, 12, 22, 32].into_iter().enumerate() {
            for (k, x) in (3..w as usize - 6).step_by(7).enumerate() {
                for (dy, row) in LETTERS[(k + line) % 3].iter().enumerate() {
                    for (dx, _) in row.bytes().enumerate().filter(|(_, c)| *c == b'#') {
                        marks[(top + dy) * stride + (x + dx) / 8] &= !(0x80 >> ((x + dx) % 8));
                    }
                }
            }
        }
        let symbols = encode_jbig2(&marks, w, h).unwrap();
        assert!(symbols.len() < encode_jbig2_generic(&marks, w, h).unwrap().len(), "symbol coded");
        assert_eq!(decode_jbig2(&symbols, None, w, h, usize::MAX).unwrap(), marks);
    }

    /// The page has to be the image's size, and is bounded before it is decoded, being decoded
    /// whole: within `max` as rows of one bit, and within [`MAX_JBIG2_PIXELS`]. Globals are given
    /// to the decoder: ones it skips decode, ones that aren't JBIG2 are an error, as is damaged
    /// data, which never panics.
    #[test]
    fn what_it_cannot_decode_is_an_error() {
        let (w, h) = (203u32, 41u32);
        let data = encode_jbig2_generic(&noise(w, h), w, h).unwrap();
        for (columns, rows) in [(w - 1, h), (w + 1, h), (w, h - 1), (w, h + 1)] {
            let e = decode_jbig2(&data, None, columns, rows, usize::MAX).unwrap_err();
            assert!(matches!(e, FilterError::Corrupt { .. }), "{columns} × {rows}: {e}");
        }
        let page = 26 * h as usize;
        assert!(matches!(decode_jbig2(&data, None, w, h, page - 1), Err(FilterError::LimitExceeded(m)) if m == page - 1), "a byte short");
        assert_eq!(decode_jbig2(&data, None, w, h, page).unwrap().len(), page, "the page within max");
        // 16,384 × 16,384 is 2^28 pixels, and fails only later; a row more is past the cap.
        let large = |rows| decode_jbig2(&page_of(&data, 16_384, rows), None, 16_384, rows, usize::MAX);
        assert!(matches!(large(16_385), Err(FilterError::Unsupported(_))), "{:?}", large(16_385));
        assert!(!matches!(large(16_384), Err(FilterError::Unsupported(_))), "{:?}", large(16_384));

        // An extension segment (ISO/IEC 14492 7.4.14), which a decoder skips: its 11-byte header
        // gives the length of the data after it.
        let skipped = [0, 0, 0, 0, 0x3E, 0, 0, 0, 0, 0, 4, 0, 0, 0, 1];
        let generic = decode_jbig2(&data, None, w, h, usize::MAX).unwrap();
        assert_eq!(decode_jbig2(&data, Some(&skipped), w, h, usize::MAX).unwrap(), generic);
        assert!(matches!(decode_jbig2(&data, Some(b"not JBIG2"), w, h, usize::MAX), Err(FilterError::Corrupt { .. })));
        for end in 0..data.len() {
            let _ = decode_jbig2(&data[..end], None, w, h, usize::MAX);
        }
    }
}
