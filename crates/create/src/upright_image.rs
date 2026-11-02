//! Photos from phones are stored sideways with an EXIF Orientation tag. PDF image XObjects have
//! no such tag, so a JPEG embedded as it is would show up turned. Every JPEG embedded by this
//! crate (pages made from images, stamps, added images, replacements) goes through here first.

use std::borrow::Cow;
use std::io::Cursor;

use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

/// Quality of a re-encoded JPEG (the camera's own is rarely higher).
const QUALITY: u8 = 92;
/// Larger photos are embedded as they are: turning them would take hundreds of MB.
const MAX_PIXELS: u64 = 64 << 20;

#[cfg(test)]
thread_local! {
    /// How many JPEGs [`turned`] handed to the decoder on this thread.
    static DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A JPEG whose EXIF Orientation is not "as stored" has its pixels turned (and re-encoded);
/// everything else, and any JPEG this can't read, is returned unchanged.
pub(crate) fn upright(bytes: &[u8]) -> Cow<'_, [u8]> {
    match turned(bytes) {
        Some(out) => Cow::Owned(out),
        None => Cow::Borrowed(bytes),
    }
}

fn turned(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(&[0xFF, 0xD8]) || extended_xmp_chunks(bytes) > MAX_EXTENDED_XMP_CHUNKS {
        return None;
    }
    #[cfg(test)]
    DECODES.with(|c| c.set(c.get().saturating_add(1)));
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

/// Most Extended XMP chunks a JPEG handed to the decoder may carry (64 MB of metadata). The
/// decoder re-sorts and rescans every chunk so far after each marker, quadratic in their number.
const MAX_EXTENDED_XMP_CHUNKS: usize = 1024;

/// How many Extended XMP chunks (APP1 segments) come before the image data.
fn extended_xmp_chunks(bytes: &[u8]) -> usize {
    const PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
    let (mut i, mut chunks) = (2usize, 0usize);
    while let Some(&byte) = bytes.get(i) {
        // Stray bytes between segments, which the decoder skips too.
        if byte != 0xFF {
            i += 1;
            continue;
        }
        let Some(&marker) = bytes.get(i.saturating_add(1)) else { break };
        match marker {
            // Fill bytes before a marker.
            0xFF => i += 1,
            // Markers without a segment.
            0x00 | 0x01 | 0xD0..=0xD7 => i += 2,
            // The image data starts (or ends): no headers after it.
            0xDA | 0xD9 => break,
            _ => {
                let Some(&[hi, lo]) = bytes.get(i.saturating_add(2)..i.saturating_add(4)) else { break };
                let len = usize::from(u16::from_be_bytes([hi, lo]));
                if marker == 0xE1 && bytes.get(i.saturating_add(4)..).is_some_and(|seg| seg.starts_with(PREFIX)) {
                    chunks += 1;
                }
                i = i.saturating_add(2).saturating_add(len.max(2));
            }
        }
    }
    chunks
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
    use crate::tests::phone_jpeg;

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
    fn a_jpeg_with_thousands_of_extended_xmp_chunks_is_not_decoded() {
        // Empty chunks of one never-complete extended packet: the decoder re-sorts and rescans every
        // chunk so far after each marker, quadratic in their number.
        let chunk = |n: usize| {
            let mut seg = b"http://ns.adobe.com/xmp/extension/\0".to_vec();
            seg.extend_from_slice(&[b'0'; 32]); // the packet's GUID
            seg.extend_from_slice(&1000u32.to_be_bytes()); // its full length
            seg.extend_from_slice(&0u32.to_be_bytes()); // this chunk's offset
            let mut out = vec![0xFF, 0xE1];
            out.extend_from_slice(&u16::try_from(seg.len() + 2).unwrap().to_be_bytes());
            out.extend_from_slice(&seg);
            out.repeat(n)
        };
        let photo = phone_jpeg(6, 144);
        let with = |n: usize| [&photo[..2], b"\0".as_slice(), &chunk(n), &photo[2..]].concat();
        DECODES.with(|c| c.set(0));
        assert!(matches!(upright(&with(4)), Cow::Owned(_)), "a few chunks are an ordinary photo");
        assert!(matches!(upright(&with(5_000)), Cow::Borrowed(_)));
        assert_eq!(DECODES.with(std::cell::Cell::get), 1, "only the ordinary photo reached the decoder");
    }
}
