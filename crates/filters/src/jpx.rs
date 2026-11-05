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
        if !matches!(components, 1 | 3 | 4) || !components_fit(data) {
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

/// Whether every codestream the decoder may decode in JPEG 2000 `data` has one, three or four
/// components (`Csiz`, ISO 15444-1 A.5.1): a raw codestream, or each top-level `jp2c` box of a JP2
/// file. The boxes are read by the decoder's rules: it reads them until one it can't and decodes
/// the last `jp2c`, so all of those are checked.
fn components_fit(data: &[u8]) -> bool {
    // SOC, then SIZ: its marker, Lsiz, Rsiz and eight 32-bit sizes before Csiz.
    let fits = |c: &[u8]| c.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) && matches!(c.get(40..42), Some([0, 1 | 3 | 4]));
    if data.starts_with(&[0xFF, 0x4F]) {
        return fits(data);
    }
    let (mut rest, mut any) = (data, false);
    while !rest.is_empty() {
        // Bytes left that aren't a box are refused, not skipped: the decoder's own arithmetic may
        // still read one there (its XLBox wraps on a 32-bit target).
        let Some((kind, body, next)) = jp2_box(rest) else { return false };
        if kind == b"jp2c" {
            if !fits(body) {
                return false;
            }
            any = true;
        }
        rest = next;
    }
    any
}

/// The first box of `data`: its type, its contents and what follows it. `None` for one too short,
/// with a length below its header's, or past the end. Each box is at least 8
/// bytes, so a walk ends.
fn jp2_box(data: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let l_box = u32::from_be_bytes(data.get(..4)?.try_into().ok()?);
    let kind = data.get(4..8)?;
    let (start, len): (usize, usize) = match l_box {
        0 => (8, data.len().checked_sub(8)?),
        1 => (16, usize::try_from(u64::from_be_bytes(data.get(8..16)?.try_into().ok()?).checked_sub(16)?).ok()?),
        n => (8, usize::try_from(n.checked_sub(8)?).ok()?),
    };
    let end = start.checked_add(len)?;
    Some((kind, data.get(start..end)?, data.get(end..)?))
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
    }

    /// Every `jp2c` box is checked, since the decoder decodes the last: one that fits before one
    /// that doesn't is refused. A box the decoder can't read ends the walk, as it ends the decoder's.
    #[test]
    fn every_codestream_the_decoder_reads_is_checked() {
        let five = include_bytes!("../tests/data/palette-five-components.jp2");
        let at = five.windows(4).position(|w| w == b"jp2c").and_then(|t| t.checked_sub(4)).unwrap();
        let mut three = vec![0, 0, 0, 50, b'j', b'p', b'2', b'c', 0xFF, 0x4F, 0xFF, 0x51];
        three.resize(48, 0);
        three.extend([0, 3]);
        let mut both = five[..at].to_vec();
        both.extend(&three);
        assert!(super::components_fit(&both), "one codestream of three components fits");
        both.extend(&five[at..]);
        assert!(!super::components_fit(&both), "a later one of five doesn't");
        assert!(!super::components_fit(&[&[0, 0, 0, 5][..], b"jp2c", &three[8..]].concat()), "a length below the header's");
        assert!(!super::components_fit(&three[..40]), "a box past the end");
        assert!(!super::components_fit(&[&three[..], &[0, 0, 0, 1]].concat()), "bytes after the last box");
    }
}
