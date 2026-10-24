//! CCITT Group 4 (ITU-T T.6) encoding, for `CCITTFaxDecode` with `/K -1` (ISO 32000-2 §7.4.6).
//! The coding is the `fax` crate's; this module checks what it is given.

use fax::encoder::Encoder;
use fax::{Color, VecWriter};

use crate::FilterError;

/// The widest row [`encode_g4`] encodes. The encoder counts the positions in a row in 16 bits
/// and adds two of them, so a row stays within half that range.
pub const MAX_G4_COLUMNS: u32 = 32_767;

/// `rows` rows of `columns` 1-bit samples, most significant bit first, each row starting on a new
/// byte (as a PDF image's samples are packed), encoded as CCITT Group 4 with an end-of-block mark.
///
/// A 0 bit is coded black and a 1 bit white, so the stream decodes to the same bits with
/// `/BlackIs1 false`, the default: an image keeps its colour space and decode array. Bytes after
/// the last row are ignored, as are the bits that pad a row to a whole byte.
///
/// Errors: no rows or columns, or rows wider than [`MAX_G4_COLUMNS`] ([`FilterError::Unsupported`]);
/// fewer bytes than the rows need ([`FilterError::Corrupt`]).
pub fn encode_g4(data: &[u8], columns: u32, rows: u32) -> Result<Vec<u8>, FilterError> {
    let width = match u16::try_from(columns) {
        Ok(w) if w > 0 && columns <= MAX_G4_COLUMNS => w,
        _ => return Err(FilterError::Unsupported(format!("CCITT Group 4 encoding of rows {columns} samples wide"))),
    };
    if rows == 0 {
        return Err(FilterError::Unsupported("CCITT Group 4 encoding of an image without rows".into()));
    }
    let stride = usize::from(width).div_ceil(8);
    let samples = usize::try_from(rows).ok().and_then(|r| r.checked_mul(stride)).and_then(|n| data.get(..n)).ok_or_else(|| FilterError::Corrupt {
        filter: "CCITTFaxDecode",
        detail: format!("{} bytes for {rows} rows of {columns} samples", data.len()),
    })?;
    let mut encoder = Encoder::new(VecWriter::new());
    for row in samples.chunks_exact(stride) {
        let pels = (0..usize::from(width)).map(|x| {
            let white = row.get(x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
            if white { Color::White } else { Color::Black }
        });
        // `VecWriter` writes to memory: its error type has no values.
        if let Err(never) = encoder.encode_line(pels, width) {
            match never {}
        }
    }
    match encoder.finish() {
        Ok(writer) => Ok(writer.finish()),
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows decoded by an independent decoder (`hayro-ccitt`), as 1 for white and 0 for black.
    pub(crate) fn decode(data: &[u8], columns: u32, rows: u32) -> Vec<Vec<bool>> {
        struct Rows(Vec<Vec<bool>>, Vec<bool>, usize);
        impl hayro_ccitt::Decoder for Rows {
            fn push_pixel(&mut self, white: bool) {
                self.1.push(white);
            }
            fn push_pixel_chunk(&mut self, white: bool, chunks: u32) {
                self.1.extend(std::iter::repeat_n(white, chunks as usize * 8));
            }
            fn next_line(&mut self) {
                let mut row = std::mem::take(&mut self.1);
                row.truncate(self.2);
                self.0.push(row);
            }
        }
        let settings = hayro_ccitt::DecodeSettings {
            columns,
            rows,
            end_of_block: true,
            end_of_line: false,
            rows_are_byte_aligned: false,
            encoding: hayro_ccitt::EncodingMode::Group4,
            invert_black: false,
        };
        let mut out = Rows(Vec::new(), Vec::new(), columns as usize);
        hayro_ccitt::decode(data, &mut out, &mut hayro_ccitt::DecoderContext::new(settings)).unwrap();
        out.0
    }

    /// The rows of packed samples `data`, as 1 for a 1 bit.
    pub(crate) fn unpack(data: &[u8], columns: u32, rows: u32) -> Vec<Vec<bool>> {
        let stride = (columns as usize).div_ceil(8);
        (0..rows as usize).map(|y| (0..columns as usize).map(|x| data[y * stride + x / 8] & (0x80 >> (x % 8)) != 0).collect()).collect()
    }

    #[test]
    fn a_page_of_text_like_marks_round_trips_and_shrinks() {
        let (w, h) = (1700u32, 220u32);
        let stride = (w as usize).div_ceil(8);
        let mut data = vec![0xFF; stride * h as usize];
        // Short black strokes in lines, like glyphs on a white page.
        for y in (20..h as usize).step_by(40) {
            for dy in 0..12 {
                for x in (100..1600).step_by(9) {
                    if (x / 9 + dy) % 5 != 0 {
                        data[(y + dy) * stride + x / 8] &= !(0x80 >> (x % 8));
                    }
                }
            }
        }
        let g4 = encode_g4(&data, w, h).unwrap();
        assert_eq!(decode(&g4, w, h), unpack(&data, w, h));
        assert!(g4.len() * 4 < data.len(), "{} bytes from {}", g4.len(), data.len());
    }

    #[test]
    fn padding_bits_and_trailing_bytes_are_ignored() {
        // 3 columns: the last 5 bits of each byte pad the row; set them to anything.
        let data = [0b1010_1111, 0b0101_0000, 0b1110_1010, 0xAB, 0xCD];
        let g4 = encode_g4(&data, 3, 3).unwrap();
        assert_eq!(decode(&g4, 3, 3), [[true, false, true], [false, true, false], [true, true, true]]);
    }

    #[test]
    fn what_it_cannot_encode_is_an_error() {
        assert!(matches!(encode_g4(&[0; 8], 0, 1), Err(FilterError::Unsupported(_))));
        assert!(matches!(encode_g4(&[0; 8], 8, 0), Err(FilterError::Unsupported(_))));
        assert!(matches!(encode_g4(&vec![0; 4097], MAX_G4_COLUMNS + 1, 1), Err(FilterError::Unsupported(_))));
        assert!(matches!(encode_g4(&[0; 3], 16, 2), Err(FilterError::Corrupt { .. })), "two rows of 16 need 4 bytes");
        assert!(matches!(encode_g4(&[], 1, u32::MAX), Err(FilterError::Corrupt { .. })));
        let widest = vec![0x55; (MAX_G4_COLUMNS as usize).div_ceil(8) * 2];
        let g4 = encode_g4(&widest, MAX_G4_COLUMNS, 2).unwrap();
        assert_eq!(decode(&g4, MAX_G4_COLUMNS, 2), unpack(&widest, MAX_G4_COLUMNS, 2));
    }

    /// Rows of `columns` samples built from `runs`: alternating white and black runs one longer
    /// than those values (so never empty, even when shrinking makes them all 0), repeated until the
    /// rows are full; or, with `noise`, the bytes of `runs` as they are.
    fn bitmap(columns: u32, rows: u32, runs: &[u8], noise: bool) -> Vec<u8> {
        let stride = (columns as usize).div_ceil(8);
        let mut data = vec![0u8; stride * rows as usize];
        if runs.is_empty() {
            return data;
        }
        if noise {
            data.iter_mut().zip(runs.iter().cycle()).for_each(|(d, r)| *d = *r);
            return data;
        }
        let (mut white, mut left, mut next) = (true, 1 + usize::from(runs[0]), 1);
        for y in 0..rows as usize {
            for x in 0..columns as usize {
                if left == 0 {
                    white = !white;
                    left = 1 + usize::from(runs[next % runs.len()]);
                    next += 1;
                }
                if white {
                    data[y * stride + x / 8] |= 0x80 >> (x % 8);
                }
                left -= 1;
            }
        }
        data
    }

    proptest::proptest! {
        /// Any bitmap, of noise or of runs, decodes to its own bits through an independent decoder.
        #[test]
        fn bitmaps_round_trip(
            columns in 1u32..400,
            rows in 1u32..24,
            runs in proptest::collection::vec(0u8..=255, 1..48),
            noise in proptest::bool::ANY,
        ) {
            let data = bitmap(columns, rows, &runs, noise);
            let g4 = encode_g4(&data, columns, rows).unwrap();
            proptest::prop_assert_eq!(decode(&g4, columns, rows), unpack(&data, columns, rows));
        }

        /// Whatever it is given, it returns.
        #[test]
        fn any_input_returns(data in proptest::collection::vec(0u8..=255, 0..600), columns in 0u32..40_000, rows in 0u32..80) {
            let _ = encode_g4(&data, columns, rows);
        }
    }
}
