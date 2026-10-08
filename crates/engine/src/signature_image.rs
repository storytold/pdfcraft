//! Local image signatures, shared by Fill & Sign and its automation tool.

use std::io::{Cursor, Read};
use std::sync::Arc;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use crate::{Edit, MarkFile};

/// Saved signatures live in settings, so both source and normalized PNG are bounded.
pub const MAX_SIGNATURE_IMAGE_BYTES: usize = 4 << 20;
const MAX_PIXELS: u64 = 4 << 20;

#[derive(Debug, thiserror::Error)]
pub enum SignatureImageError {
    #[error("Choose a PNG or JPEG image.")]
    Format,
    #[error("Choose an image smaller than 4 MiB and 4 megapixels (at most 4096 pixels per side).")]
    Size,
    #[error("Couldn't read the signature image: {0}")]
    Read(#[from] std::io::Error),
    #[error("Couldn't decode the signature image: {0}")]
    Decode(#[from] image::ImageError),
}

/// A validated image, normalized to PNG without source paths or metadata. Alpha is preserved.
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureImage {
    bytes: Arc<Vec<u8>>,
    rgba: Arc<Vec<u8>>,
    size: [usize; 2],
}

impl SignatureImage {
    /// Read at most the file limit plus one byte, even if the file grows while being read.
    pub fn read(reader: impl Read) -> Result<Self, SignatureImageError> {
        let mut bytes = Vec::new();
        reader.take(MAX_SIGNATURE_IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SignatureImageError> {
        if bytes.len() > MAX_SIGNATURE_IMAGE_BYTES {
            return Err(SignatureImageError::Size);
        }
        let format = image::guess_format(bytes).map_err(|_| SignatureImageError::Format)?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
            return Err(SignatureImageError::Format);
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(32 << 20);
        reader.limits(limits);
        let mut decoder = reader.into_decoder()?;
        let (w, h) = decoder.dimensions();
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Err(SignatureImageError::Size);
        }
        let orientation = decoder.orientation()?;
        let mut image = DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        let rgba = image.into_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(rgba.clone()).write_to(&mut png, ImageFormat::Png)?;
        let bytes = png.into_inner();
        if bytes.len() > MAX_SIGNATURE_IMAGE_BYTES {
            return Err(SignatureImageError::Size);
        }
        Ok(Self { bytes: Arc::new(bytes), rgba: Arc::new(rgba.into_raw()), size })
    }

    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn size(&self) -> [usize; 2] {
        self.size
    }

    /// Left edge at `at`, vertically centered, with the same 150 pt width cap as typed names.
    pub fn rect(&self, at: [f64; 2], initials: bool) -> Option<[f64; 4]> {
        if !at.iter().all(|v| v.is_finite()) {
            return None;
        }
        let [w, h] = self.size;
        let scale = (150.0 / w as f64).min(if initials { 24.0 } else { 32.0 } / h as f64);
        let (width, height) = (w as f64 * scale, h as f64 * scale);
        Some([at[0], at[1] - height / 2.0, at[0] + width, at[1] + height / 2.0])
    }

    pub fn edit(&self, page: usize, at: [f64; 2], initials: bool, author: &str) -> Option<Edit> {
        let rect = self.rect(at, initials)?;
        let label = if initials { "Add initials" } else { "Add signature" };
        Some(Edit::Batch {
            label: label.into(),
            edits: vec![Edit::AddCustomStamp {
                page,
                rect,
                name: if initials { "Initials" } else { "Signature" }.into(),
                file: MarkFile { name: "signature.png".into(), bytes: self.bytes.clone(), page: 0 },
                author: author.into(),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(w, h, image::Rgba([40, 60, 80, 128]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn image_signatures_preserve_alpha_and_fit_the_signature_box() {
        let image = SignatureImage::from_bytes(&png(400, 40)).unwrap();
        assert_eq!(image.size(), [400, 40]);
        assert_eq!(&image.rgba()[..4], &[40, 60, 80, 128]);
        let Edit::Batch { edits, .. } = image.edit(0, [20.0, 100.0], false, "Ada").unwrap() else { panic!() };
        let Edit::AddCustomStamp { rect, .. } = &edits[0] else { panic!() };
        assert_eq!(*rect, [20.0, 92.5, 170.0, 107.5]);
        assert!(image.edit(0, [f64::NAN, 0.0], false, "").is_none());
        assert_eq!(SignatureImage::from_bytes(image.bytes()).unwrap(), image);
    }

    #[test]
    fn invalid_and_oversized_signature_images_are_rejected() {
        assert!(SignatureImage::from_bytes(b"not an image").is_err());
        assert!(SignatureImage::from_bytes(b"\x89PNG\r\n\x1a\ntruncated").is_err());
        assert!(SignatureImage::from_bytes(&vec![0; MAX_SIGNATURE_IMAGE_BYTES + 1]).is_err());
        assert!(SignatureImage::from_bytes(&png(4097, 1)).is_err());
        assert!(SignatureImage::from_bytes(&png(4096, 1025)).is_err());
        assert!(SignatureImage::read(std::io::repeat(0)).is_err());
        assert!(matches!(SignatureImage::from_bytes(b"GIF89a\x02\0\x02\0"), Err(SignatureImageError::Format)));
    }

    #[test]
    fn jpeg_signatures_are_normalized_to_png() {
        let mut jpeg = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20])).write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
        let image = SignatureImage::from_bytes(&jpeg.into_inner()).unwrap();
        assert_eq!(image.size(), [60, 20]);
        assert!(image.bytes().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(image.rgba().chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn jpeg_signature_orientation_is_applied_before_preview_and_embedding() {
        let mut jpeg = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20])).write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
        let jpeg = jpeg.into_inner();
        // Synthetic EXIF IFD with Orientation=6 (90 degrees clockwise).
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut rotated = jpeg[..2].to_vec();
        rotated.extend_from_slice(&[0xff, 0xe1]);
        rotated.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        rotated.extend_from_slice(exif);
        rotated.extend_from_slice(&jpeg[2..]);
        let image = SignatureImage::from_bytes(&rotated).unwrap();
        assert_eq!(image.size(), [20, 60]);
        assert_eq!(SignatureImage::from_bytes(image.bytes()).unwrap().size(), [20, 60]);
        assert!(!image.bytes().windows(4).any(|w| w == b"Exif"));
    }

    #[test]
    fn very_wide_image_signatures_keep_the_requested_rectangle() {
        let image = SignatureImage::from_bytes(&png(3000, 10)).unwrap();
        let mut session = crate::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        session.apply(id, image.edit(0, [20.0, 100.0], false, "Ada").unwrap()).unwrap();
        let a = &session.get(id).unwrap().info.annotations[0];
        assert_eq!(a.rect, [20.0, 99.75, 170.0, 100.25]);
    }
}
