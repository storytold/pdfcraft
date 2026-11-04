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

/// The samples of JPEG 2000 `data` if its header says it is `w` × `h` with no alpha channel
/// (`/SMaskInData`). `None` for data that doesn't decode, another size, or alpha; the decoder
/// panicking is `None` too.
pub fn decode_jpx(data: &[u8], w: usize, h: usize) -> Option<JpxSamples> {
    catch_unwind(AssertUnwindSafe(|| {
        let img = Image::new(data, &DecodeSettings::default()).ok()?;
        // The header has to give the size the caller checked, so nothing larger is allocated.
        if (usize::try_from(img.width()).ok()?, usize::try_from(img.height()).ok()?) != (w, h) || img.has_alpha() {
            return None;
        }
        let space = img.color_space();
        let components = usize::from(space.num_channels());
        let device = match space {
            ColorSpace::Gray => Some("DeviceGray"),
            ColorSpace::RGB => Some("DeviceRGB"),
            ColorSpace::CMYK => Some("DeviceCMYK"),
            _ => None,
        };
        let data = img.decode().ok()?;
        (components > 0 && data.len() == w.checked_mul(h)?.checked_mul(components)?).then_some(JpxSamples { data, components, device })
    }))
    .ok()?
}
