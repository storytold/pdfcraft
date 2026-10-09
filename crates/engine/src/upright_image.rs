//! Photos from phones are stored sideways with an EXIF Orientation tag. PDF image XObjects have
//! no such tag, so a JPEG embedded as it is would show up turned. Images placed into a page
//! (stamps, added images, replacements) go through here first.

use std::borrow::Cow;
use std::io::Cursor;

use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

/// Quality of a re-encoded JPEG (the camera's own is rarely higher).
const QUALITY: u8 = 92;
/// Larger photos are embedded as they are: turning them would take hundreds of MB.
const MAX_PIXELS: u64 = 64 << 20;

/// A JPEG whose EXIF Orientation is not "as stored" has its pixels turned (and re-encoded);
/// everything else, and any JPEG this can't read, is returned unchanged.
pub(crate) fn upright(bytes: &[u8]) -> Cow<'_, [u8]> {
    match turned(bytes) {
        Some(out) => Cow::Owned(out),
        None => Cow::Borrowed(bytes),
    }
}

fn turned(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg).into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let (w, h) = decoder.dimensions();
    if orientation == Orientation::NoTransforms || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    let mut out = Vec::new();
    {
        let mut encoder = JpegEncoder::new_with_quality(&mut out, QUALITY);
        // Keep the resolution, which sets the image's natural size on the page.
        if let Some(mut density) = jfif_density(bytes) {
            if matches!(orientation, Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH) {
                density.density = (density.density.1, density.density.0);
            }
            encoder.set_pixel_density(density);
        }
        match &image {
            DynamicImage::ImageLuma8(gray) => encoder.encode_image(gray).ok()?,
            _ => encoder.encode_image(&image.to_rgb8()).ok()?,
        }
    }
    Some(out)
}

/// The resolution in a JFIF header, if the file has one in inches or centimetres.
fn jfif_density(bytes: &[u8]) -> Option<PixelDensity> {
    let mut i = 2usize;
    while let Some(&[0xFF, marker, hi, lo]) = bytes.get(i..i.saturating_add(4)) {
        let len = usize::from(u16::from_be_bytes([hi, lo]));
        let seg = bytes.get(i + 4..i + 2 + len)?;
        if marker == 0xE0 && seg.starts_with(b"JFIF\0") {
            let &[unit, x0, x1, y0, y1] = seg.get(7..12)? else { return None };
            let unit = match unit {
                1 => PixelDensityUnit::Inches,
                2 => PixelDensityUnit::Centimeters,
                _ => return None,
            };
            return Some(PixelDensity { density: (u16::from_be_bytes([x0, x1]), u16::from_be_bytes([y0, y1])), unit });
        }
        if marker == 0xDA {
            return None;
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, MarkFile};

    /// A 60 x 20 JPEG at `dpi` x 72 dpi, tagged with EXIF `orientation` (6: shown turned 90
    /// degrees clockwise).
    fn phone_jpeg(orientation: u8, dpi: u16) -> Vec<u8> {
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new(&mut jpeg);
        encoder.set_pixel_density(PixelDensity { density: (dpi, 72), unit: PixelDensityUnit::Inches });
        encoder.encode_image(&image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20]))).unwrap();
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xff, 0xe1]);
        out.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&exif);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn orientation_is_applied_and_plain_images_are_left_alone() {
        let photo = phone_jpeg(6, 144);
        let turned = upright(&photo);
        assert!(matches!(turned, Cow::Owned(_)));
        let decoded = image::load_from_memory(&turned).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (20, 60));
        assert_eq!(jfif_density(&turned), Some(PixelDensity { density: (72, 144), unit: PixelDensityUnit::Inches }));
        let stored = phone_jpeg(1, 72);
        assert!(matches!(upright(&stored), Cow::Borrowed(_)));
        assert!(matches!(upright(b"\x89PNG\r\n\x1a\n"), Cow::Borrowed(_)));
        assert!(matches!(upright(b"\xff\xd8 not a jpeg"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_phone_photo_used_as_a_custom_stamp_is_embedded_upright() {
        let mut session = crate::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        // A click places the stamp at its natural size: 30 x 20 pt as stored, 20 x 30 pt upright.
        // Named "Signature" so `signature_image` finds its picture.
        let file = MarkFile { name: "photo.jpg".into(), bytes: std::sync::Arc::new(phone_jpeg(6, 144)), page: 0 };
        let stamp = Edit::AddCustomStamp { page: 0, rect: [100.0, 100.0, 100.0, 100.0], name: "Signature".into(), file, author: "Ada".into() };
        session.apply(id, stamp).unwrap();
        let cos = &session.get(id).unwrap().editor.as_ref().unwrap().cos;
        let picture = pdfcraft_annot::signature_image(cos, 0, 0).unwrap().unwrap();
        let obj = cos.get(picture);
        let pdfcraft_cos::Object::Stream(s) = &*obj else { panic!() };
        assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height")), (Some(20), Some(60)));
        let rect = pdfcraft_annot::summaries(cos)[0].rect;
        assert!(((rect[2] - rect[0]) - 20.0).abs() < 0.01 && ((rect[3] - rect[1]) - 30.0).abs() < 0.01, "{rect:?}");
    }
}
