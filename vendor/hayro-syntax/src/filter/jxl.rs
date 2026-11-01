//! PdfCraft patch (29): `JXLDecode`, JPEG XL image data (ISO/IEC 18181) in an image XObject.
//!
//! ISO 32000-2 defines no JPEG XL filter yet (the PDF Association has announced one), so this
//! reads the name `JXLDecode`, the placeholder other implementations use. Only the first frame
//! is drawn (a PDF image is static), as 8-bit grey or RGB with an optional alpha channel;
//! CMYK, spot colours and other layouts are refused (the image is then not drawn).

use crate::filter::FilterResult;
use crate::object::stream::{ImageColorSpace, ImageData, ImageDecodeParams};
use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;
use jxl_oxide::{JxlImage, PixelFormat};

/// Most pixels an image may have (the decoded raster is 3 or 4 bytes for each).
const MAX_PIXELS: u64 = 1 << 28;

pub(crate) fn decode(data: &[u8], image_params: &ImageDecodeParams) -> Option<FilterResult<'static>> {
    let image = JxlImage::builder().read(std::io::Cursor::new(data)).ok()?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    // A declared size that disagrees with the codestream is not trusted (as for the other codecs).
    if (image_params.width != 0 && image_params.width != width) || (image_params.height != 0 && image_params.height != height) {
        return None;
    }
    let (color_space, has_alpha) = match image.pixel_format() {
        PixelFormat::Gray => (ImageColorSpace::Gray, false),
        PixelFormat::Graya => (ImageColorSpace::Gray, true),
        PixelFormat::Rgb => (ImageColorSpace::Rgb, false),
        PixelFormat::Rgba => (ImageColorSpace::Rgb, true),
        _ => return None,
    };
    let render = image.render_frame(0).ok()?;
    let mut stream = render.stream();
    let channels = stream.channels() as usize;
    let pixels = (width as usize).checked_mul(height as usize)?;
    let mut buf = vec![0u8; pixels.checked_mul(channels)?];
    if stream.write_to_buffer::<u8>(&mut buf) != buf.len() {
        return None;
    }
    let colors = channels - usize::from(has_alpha);
    let (data, alpha) = if has_alpha {
        let mut color: Vec<u8> = Vec::with_capacity(pixels.checked_mul(colors)?);
        let mut alpha: Vec<u8> = Vec::with_capacity(pixels);
        for px in buf.chunks_exact(channels) {
            let (a, c) = px.split_last()?;
            alpha.push(*a);
            color.extend_from_slice(c);
        }
        (color, Some(alpha))
    } else {
        (buf, None)
    };
    Some(FilterResult {
        data: Cow::Owned(data),
        image_data: Some(ImageData { alpha, color_space: Some(color_space), bits_per_component: 8, width, height }),
    })
}
