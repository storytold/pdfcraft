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
        // Checked before decoding: a codestream may declare up to 16384 components and every one
        // is decoded, though a palette box (`pclr`) gives the output its own count, so both are
        // checked (an allocation failure aborts, past `catch_unwind`).
        if !matches!(components, 1 | 3 | 4) || !matches!(codestream_components(data)?, 1 | 3 | 4) {
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

/// The component count (`Csiz`) of the codestream in JPEG 2000 `data`: a raw codestream, or the
/// first top-level `jp2c` box of a JP2 file (ISO 15444-1 Annex I). `None` when there is none.
fn codestream_components(data: &[u8]) -> Option<u16> {
    let mut rest = data;
    if !rest.starts_with(&[0xFF, 0x4F]) {
        loop {
            let len = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?);
            let (head, len) = match len {
                0 => (8, rest.len()),
                1 => (16, usize::try_from(u64::from_be_bytes(rest.get(8..16)?.try_into().ok()?)).ok()?),
                n => (8, usize::try_from(n).ok()?),
            };
            if rest.get(4..8)? == b"jp2c" {
                rest = rest.get(head..len.min(rest.len()))?;
                break;
            }
            // Each box moves on by at least its header, so the walk ends.
            rest = rest.get(len.max(head)..)?;
        }
    }
    // SOC, then SIZ: its marker, Lsiz, Rsiz and eight 32-bit sizes before Csiz (ISO 15444-1 A.5.1).
    if rest.get(..4)? != [0xFF, 0x4F, 0xFF, 0x51] {
        return None;
    }
    Some(u16::from_be_bytes(rest.get(40..42)?.try_into().ok()?))
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

    /// A palette of three columns over a codestream of five components is refused too: the
    /// palette sets the output's count, but all five components would be decoded.
    #[test]
    fn a_palette_over_more_than_four_components_is_refused() {
        assert!(super::decode_jpx(include_bytes!("../tests/data/palette-five-components.jp2"), 16, 16).is_none());
        assert_eq!(super::codestream_components(include_bytes!("../tests/data/palette-five-components.jp2")), Some(5));
        assert_eq!(super::codestream_components(b"\0\0\0\x07jp2c"), None, "a box shorter than its header");
    }
}
