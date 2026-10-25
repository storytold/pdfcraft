use crate::object::Dict;
use crate::object::dict::keys::{
    BLACK_IS_1, COLUMNS, ENCODED_BYTE_ALIGN, END_OF_BLOCK, END_OF_LINE, K, ROWS,
};
use crate::object::stream::{FilterResult, ImageColorSpace, ImageData, ImageDecodeParams};
use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::iter;
use hayro_ccitt::{DecodeSettings, Decoder, DecoderContext, EncodingMode};

pub(crate) fn decode(
    data: &[u8],
    params: &Dict<'_>,
    image_params: &ImageDecodeParams,
    limit: usize,
) -> Option<FilterResult<'static>> {
    let k = params.get::<i32>(K).unwrap_or(0);

    // PdfCraft patch: `/Rows 0` means the height is not predetermined (ISO 32000-2 Table 11, and
    // the default), not zero rows. Scanners write it on Group 4 pages, which rendered blank.
    // PdfCraft patch (19): fewer `/Rows` than the image's `/Height` doesn't cut the image short: a
    // plotter driver wrote `/Rows` equal to `/Columns`, and the rows after it were drawn black
    // (#3308). The decoder still stops where the data ends.
    let rows = params
        .get::<u32>(ROWS)
        .filter(|&rows| rows != 0)
        .map_or(image_params.height, |rows| rows.max(image_params.height));
    // PdfCraft patch (hayro-interpret 54): nor are more rows decoded than the image's `/Height`:
    // rows past it are never drawn, and a 1-pixel-wide image with `/Rows 1073741824` took a byte
    // a row in its packed output, 1 GiB.
    let rows = if image_params.height == 0 {
        rows
    } else {
        rows.min(image_params.height)
    };
    let end_of_block = params.get::<bool>(END_OF_BLOCK).unwrap_or(true);

    let settings = DecodeSettings {
        columns: params.get::<usize>(COLUMNS).unwrap_or(1728) as u32,
        rows,
        end_of_block,
        end_of_line: params.get::<bool>(END_OF_LINE).unwrap_or(false),
        rows_are_byte_aligned: params.get::<bool>(ENCODED_BYTE_ALIGN).unwrap_or(false),
        encoding: if k < 0 {
            EncodingMode::Group4
        } else if k == 0 {
            EncodingMode::Group3_1D
        } else {
            EncodingMode::Group3_2D { k: k as u32 }
        },
        invert_black: params.get::<bool>(BLACK_IS_1).unwrap_or(false),
    };

    // PdfCraft patch: refuse absurd sizes before the decoder allocates for them.
    if !super::ccitt_size_ok(settings.columns, settings.rows) {
        return None;
    }

    // Whenever possible (if we don't have an indexed color space), we convert
    // the data as 8-bit instead of 1-bit, so that it can be easier converted
    // into an RGBA8 image.
    // PdfCraft patch (hayro-interpret 54): an image over 2^28 pixels stays 1-bit: hayro-interpret
    // reduces it from its packed samples, and a byte per pixel took 311 MB for a 21600 × 14400
    // scan (#4071).
    let packed = image_params.is_indexed
        || u64::from(settings.columns) * u64::from(settings.rows) > super::MAX_FULL_IMAGE_PIXELS;

    // PdfCraft patch (hayro-interpret 54): refuse a packed image whose samples (a row is
    // `/Columns` bits, rounded up to a byte) would pass the 512 MiB hayro-interpret reduces from
    // at most: a 1 × 2^30 image is 1 GiB packed. A drawn image is not checked against the decode
    // limit, which comes from `/Width`: hayro-interpret draws a `/Columns` that differs from it,
    // and a byte-a-pixel image is at most 2^28 pixels. A stream of unknown height (not drawn as
    // an image) stops taking pixels after as many rows of its output as the decode limit holds:
    // with `/Rows` absent a Group 3 decoder runs until the data ends (a Group 4 one decodes
    // nothing), and an explicit `/Rows` can ask for more than the caller's limit.
    const MAX_PACKED_BYTES: u64 = 512 << 20;
    let row_bytes = u64::from(settings.columns.div_ceil(8));
    if packed {
        let samples = row_bytes.checked_mul(u64::from(settings.rows))?;
        if samples > MAX_PACKED_BYTES {
            warn!("packed CCITT image of {samples} bytes is past 512 MiB");
            return None;
        }
    }
    let max_rows = if image_params.height == 0 {
        let output_row_bytes = if packed {
            row_bytes
        } else {
            u64::from(settings.columns)
        };
        let limit = u64::try_from(limit).unwrap_or(u64::MAX);
        let rows_in_limit =
            u32::try_from(limit / output_row_bytes.max(1)).unwrap_or(u32::MAX);
        if settings.rows == 0 {
            rows_in_limit
        } else {
            settings.rows.min(rows_in_limit)
        }
    } else {
        settings.rows
    };

    let (decoded, bpc) = if packed {
        struct BitPackDecoder {
            output: Vec<u8>,
            decoded_rows: u32,
            max_rows: u32,
            buffer: u8,
            bit_count: u8,
        }

        impl BitPackDecoder {
            fn push_bit(&mut self, white: bool) {
                let bit = if white { 1 } else { 0 };
                self.buffer = (self.buffer << 1) | bit;
                self.bit_count += 1;

                if self.bit_count == 8 {
                    self.output.push(self.buffer);
                    self.buffer = 0;
                    self.bit_count = 0;
                }
            }

            fn flush(&mut self) {
                if self.bit_count > 0 {
                    let padded = self.buffer << (8 - self.bit_count);
                    self.output.push(padded);
                    self.buffer = 0;
                    self.bit_count = 0;
                }
            }
        }

        impl Decoder for BitPackDecoder {
            fn push_pixel(&mut self, white: bool) {
                if self.decoded_rows < self.max_rows {
                    self.push_bit(white);
                }
            }

            fn push_pixel_chunk(&mut self, white: bool, chunk_count: u32) {
                if self.decoded_rows >= self.max_rows {
                    return;
                }
                let byte = if white { 0xFF } else { 0x00 };
                self.output
                    .extend(iter::repeat_n(byte, chunk_count as usize));
            }

            fn next_line(&mut self) {
                self.decoded_rows = self.decoded_rows.saturating_add(1);
                self.flush();
            }
        }

        let mut decoder = BitPackDecoder {
            output: Vec::new(),
            decoded_rows: 0,
            max_rows,
            buffer: 0,
            bit_count: 0,
        };
        let mut context = DecoderContext::new(settings);
        let result = hayro_ccitt::decode(data, &mut decoder, &mut context);

        // If we decoded at least one row, let's be lenient and return what we got.
        // See also 0001763.pdf.
        if result.is_err() && decoder.decoded_rows == 0 {
            return None;
        }

        (decoder.output, 1)
    } else {
        struct Luma8Decoder {
            output: Vec<u8>,
            decoded_rows: u32,
            max_rows: u32,
        }

        impl Decoder for Luma8Decoder {
            fn push_pixel(&mut self, white: bool) {
                if self.decoded_rows < self.max_rows {
                    self.output.push(if white { 0xFF } else { 0x00 });
                }
            }

            fn push_pixel_chunk(&mut self, white: bool, chunk_count: u32) {
                if self.decoded_rows >= self.max_rows {
                    return;
                }
                let byte = if white { 0xFF } else { 0x00 };
                self.output
                    .extend(iter::repeat_n(byte, chunk_count as usize * 8));
            }

            fn next_line(&mut self) {
                self.decoded_rows = self.decoded_rows.saturating_add(1);
            }
        }

        let mut decoder = Luma8Decoder {
            output: Vec::new(),
            decoded_rows: 0,
            max_rows,
        };
        let mut context = DecoderContext::new(settings);
        let result = hayro_ccitt::decode(data, &mut decoder, &mut context);

        if result.is_err() && decoder.decoded_rows == 0 {
            return None;
        }

        (decoder.output, 8)
    };

    Some(FilterResult {
        data: Cow::Owned(decoded),
        image_data: Some(ImageData {
            alpha: None,
            color_space: Some(ImageColorSpace::Gray),
            bits_per_component: bpc,
            width: settings.columns,
            height: image_params.height,
        }),
    })
}
