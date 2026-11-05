//! JPEG 2000 (`JPXDecode`) samples as a page reads them, for code that changes pixels and stores
//! them again (redaction). The caller pays for the decode first (`pdfcraft_cos::ImageBudget`).

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro_jpeg2000::{ColorSpace, DecodeSettings, Image};

/// A decoded JPEG 2000 image: 8-bit samples, interleaved, palette indices resolved.
pub struct JpxSamples {
    pub data: Vec<u8>,
    pub components: usize,
    /// The PDF device colour space the file states (`DeviceGray`, `DeviceRGB`, `DeviceCMYK`), or
    /// `None` for an ICC profile or a space PDF has no device name for.
    pub device: Option<&'static str>,
}

/// The samples of JPEG 2000 `data` if its header says it is `w` × `h` with one, three or four
/// components and no alpha channel (`/SMaskInData`). `None` for data that doesn't decode, another
/// size or component count, or alpha; the decoder panicking is `None` too.
pub fn decode_jpx(data: &[u8], w: usize, h: usize) -> Option<JpxSamples> {
    catch_unwind(AssertUnwindSafe(|| {
        let img = Image::new(data, &DecodeSettings::default()).ok()?;
        // The header has to give the size the caller checked, so nothing larger is allocated.
        if (usize::try_from(img.width()).ok()?, usize::try_from(img.height()).ok()?) != (w, h) || img.has_alpha() {
            return None;
        }
        let space = img.color_space();
        let components = usize::from(space.num_channels());
        // Checked before decoding: a codestream may declare up to 255 components, and the output
        // buffer is allocated for all of them (an allocation failure aborts, past `catch_unwind`).
        if !matches!(components, 1 | 3 | 4) {
            return None;
        }
        let device = match space {
            ColorSpace::Gray => Some("DeviceGray"),
            ColorSpace::RGB => Some("DeviceRGB"),
            ColorSpace::CMYK => Some("DeviceCMYK"),
            _ => None,
        };
        let data = img.decode().ok()?;
        (data.len() == w.checked_mul(h)?.checked_mul(components)?).then_some(JpxSamples { data, components, device })
    }))
    .ok()?
}

#[cfg(test)]
mod tests {
    /// An image of five components (a JP2 file whose colour box names no space the decoder knows,
    /// so the decoder takes the codestream's count) is refused before its samples are allocated:
    /// only one, three and four have a device colour space.
    #[test]
    fn an_image_of_more_than_four_components_is_refused() {
        assert!(super::decode_jpx(include_bytes!("../tests/data/five-components.jp2"), 16, 16).is_none());
    }
}
