use crate::bit_reader::{BitWriter, bit_mask};
use crate::filter::FilterResult;
use crate::object::stream::{ImageColorSpace, ImageData, ImageDecodeParams};
use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;
use hayro_jpeg2000::{ColorSpace, DecodeSettings};

impl ImageColorSpace {
    fn num_components(&self) -> u8 {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Unknown(num) => *num,
        }
    }
}

pub(crate) fn decode(data: &[u8], params: &ImageDecodeParams) -> Option<FilterResult<'static>> {
    use crate::object::stream::ImageColorSpace;

    let settings = DecodeSettings {
        resolve_palette_indices: false,
        strict: false,
        target_resolution: params.target_dimension,
    };

    let image = hayro_jpeg2000::Image::new(data, &settings).ok()?;

    let width = image.width();
    let height = image.height();
    let bpc = params.bpc.unwrap_or(image.original_bit_depth());
    let cs = match image.color_space() {
        ColorSpace::Gray => ImageColorSpace::Gray,
        ColorSpace::RGB => ImageColorSpace::Rgb,
        ColorSpace::CMYK => ImageColorSpace::Cmyk,
        ColorSpace::Unknown { num_channels } => ImageColorSpace::Unknown(*num_channels),
        ColorSpace::Icc {
            num_channels: num_components,
            ..
        } => match num_components {
            1 => ImageColorSpace::Gray,
            3 => ImageColorSpace::Rgb,
            4 => ImageColorSpace::Cmyk,
            _ => return None,
        },
    };
    let has_alpha = image.has_alpha();
    let bitmap = image.decode().ok()?;

    let (mut data, mut alpha) = if !has_alpha {
        (bitmap, None)
    } else {
        // Extract the alpha channel.
        let total_channels = cs.num_components() + 1;
        let mut color_channels = Vec::with_capacity(
            (bitmap.len() / total_channels as usize) * cs.num_components() as usize,
        );
        let mut alpha_channel = Vec::with_capacity(bitmap.len() / total_channels as usize);

        for sample in bitmap.chunks_exact(total_channels as usize) {
            let (alpha, color) = sample.split_last()?;
            alpha_channel.push(*alpha);
            color_channels.extend_from_slice(color);
        }

        (color_channels, Some(alpha_channel))
    };

    // The decoded image is always 8-bit, so if necessary we have to rescale
    // ourselves.
    if bpc != 8 {
        data = scale(&data, bpc, cs.num_components(), width, height)?;
        alpha = alpha.and_then(|alpha| scale(&alpha, bpc, cs.num_components(), width, height));
    }

    Some(FilterResult {
        data: Cow::Owned(data),
        image_data: Some(ImageData {
            alpha,
            color_space: Some(cs),
            bits_per_component: bpc,
            width,
            height,
        }),
    })
}

fn scale(
    data: &[u8],
    bit_per_component: u8,
    num_components: u8,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    if bit_per_component == 0
        || bit_per_component > 32
        || num_components == 0
        || width == 0
        || height == 0
    {
        return None;
    }

    // PdfCraft patch: normalize all supported bit widths without an overflowing
    // shift or floating-point endpoint rounding, including no_std builds.
    let max_sample = u64::from(bit_mask(bit_per_component));

    let bits_per_row = (width as usize)
        .checked_mul(num_components as usize)?
        .checked_mul(bit_per_component as usize)?;
    let input_len = bits_per_row.div_ceil(8).checked_mul(height as usize)?;
    let mut input = vec![0; input_len];
    let mut writer = BitWriter::new(&mut input, bit_per_component)?;
    let components_per_row = (num_components as usize).checked_mul(width as usize)?;

    for bytes in data.chunks_exact(components_per_row) {
        for byte in bytes {
            // The numerator is at most 255 * u32::MAX + 127, which fits in
            // u64. The rounded quotient is <= max_sample, so the cast is exact.
            let scaled = ((u64::from(*byte) * max_sample + 127) / 255) as u32;
            writer.write(scaled)?;
        }

        writer.align();
    }

    let final_pos = writer.cur_pos();
    input.truncate(final_pos);

    Some(input)
}

#[cfg(test)]
mod tests {
    use super::scale;
    use crate::bit_reader::{BitReader, bit_mask};
    use alloc::vec::Vec;

    #[test]
    fn jpx_scale_accepts_32_bit_samples() {
        assert_eq!(
            scale(&[0, 255], 32, 1, 2, 1).unwrap(),
            [0, 0, 0, 0, 255, 255, 255, 255]
        );
    }

    #[test]
    fn jpx_scale_keeps_supported_bit_depths() {
        for bpc in 1_u8..=32 {
            let bytes = scale(&[0, 255], bpc, 1, 2, 1).unwrap();
            assert_eq!(bytes.len(), (2 * usize::from(bpc)).div_ceil(8));
            let mut reader = BitReader::new(&bytes);
            assert_eq!(reader.read(bpc), Some(0));
            assert_eq!(reader.read(bpc), Some(bit_mask(bpc)));
        }
    }

    #[test]
    fn jpx_scale_rejects_out_of_range_bit_depths() {
        for bpc in [0, 33, 255] {
            assert!(scale(&[255], bpc, 1, 1, 1).is_none());
        }
    }

    #[test]
    fn jpx_scale_rejects_zero_dimensions_and_components() {
        for (components, width, height) in [(0, 1, 1), (1, 0, 1), (1, 1, 0)] {
            assert!(scale(&[255], 32, components, width, height).is_none());
        }
    }

    #[test]
    fn jpx_scale_preserves_one_eight_and_sixteen_bit_samples() {
        let samples: Vec<u8> = (0..=255).collect();
        for bpc in [1, 8, 16] {
            let bytes = scale(&samples, bpc, 1, 256, 1).unwrap();
            let mut reader = BitReader::new(&bytes);
            for byte in &samples {
                let expected = match bpc {
                    1 => u32::from(*byte >= 128),
                    8 => u32::from(*byte),
                    16 => u32::from(*byte) * 257,
                    _ => unreachable!(),
                };
                assert_eq!(reader.read(bpc), Some(expected), "{bpc}-bit sample {byte}");
            }
        }
    }
}
