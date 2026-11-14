//! Photos from phones are stored sideways with an EXIF Orientation tag. PDF image XObjects have
//! no such tag, so a JPEG embedded as it is would show up turned. Every JPEG embedded by this
//! crate (pages made from images, stamps, added images, replacements) goes through here first.

use std::borrow::Cow;

/// Quality of a re-encoded JPEG (the camera's own is rarely higher).
const QUALITY: u8 = 92;
/// Larger photos are embedded as they are: turning them would take hundreds of MB.
const MAX_PIXELS: u64 = 64 << 20;

#[cfg(test)]
thread_local! {
    /// How many JPEGs [`turned`] handed to the decoder on this thread.
    static DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reads the EXIF Orientation tag (1..=8) from APP1 Exif metadata in a JPEG, defaulting to 1.
pub fn jpeg_exif_orientation(bytes: &[u8]) -> u16 {
    let mut i = 2usize;
    while let Some(&byte) = bytes.get(i) {
        if byte != 0xFF {
            i += 1;
            continue;
        }
        let Some(&marker) = bytes.get(i.saturating_add(1)) else { break };
        match marker {
            0xFF => i += 1,
            0x00 | 0x01 | 0xD0..=0xD7 => i += 2,
            0xDA | 0xD9 => break,
            _ => {
                let Some(&[hi, lo]) = bytes.get(i.saturating_add(2)..i.saturating_add(4)) else { break };
                let len = usize::from(u16::from_be_bytes([hi, lo]));
                if len < 2 {
                    break;
                }
                let Some(seg) = bytes.get(i + 4..i + 2 + len) else { break };
                if marker == 0xE1
                    && let Some(tiff) = seg.strip_prefix(b"Exif\0\0")
                {
                    let Some(hdr_order) = tiff.get(0..2) else { return 1 };
                    let order = match hdr_order {
                        b"II" => craft_tiff::ByteOrder::Little,
                        b"MM" => craft_tiff::ByteOrder::Big,
                        _ => return 1,
                    };
                    let Some(off) = order.read_u32(tiff, 4) else { return 1 };
                    if let Ok((ifd, _)) = craft_tiff::parse_ifd_at(tiff, u64::from(off), order, 0, false, &craft_tiff::ParseOptions::default())
                        && let Some(o) = ifd.u16(craft_tiff::tags::ORIENTATION)
                    {
                        return o;
                    }
                    return 1;
                }
                i += 2 + len;
            }
        }
    }
    1
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
    if !bytes.starts_with(&[0xFF, 0xD8]) || !decodable_metadata(bytes) {
        return None;
    }
    let orientation = jpeg_exif_orientation(bytes);
    if !matches!(orientation, 2..=8) {
        return None;
    }
    #[cfg(test)]
    DECODES.with(|c| c.set(c.get().saturating_add(1)));
    let opts = craft_image_codecs::DecodeOptions {
        keep_orientation: false,
        limits: craft_image_codecs::Limits { max_pixels: MAX_PIXELS, ..Default::default() },
        ..Default::default()
    };
    let mut img = craft_image_codecs::decode_as_with(craft_image_codecs::Format::Jpeg, bytes, &opts).ok()?;
    if let Some((mut dx, mut dy)) = jfif_density_dpi(bytes) {
        if matches!(orientation, 5..=8) {
            (dx, dy) = (dy, dx);
        }
        img.meta.dpi = Some((dx, dy));
    }
    let enc_opts = craft_image_codecs::EncodeOptions { jpeg_quality: QUALITY, embed_metadata: true, ..Default::default() };
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Jpeg, &enc_opts).ok()
}

/// Most Extended XMP chunks a JPEG handed to the decoder may carry (64 MB of metadata). The
/// decoder re-sorts and rescans every chunk so far after each marker, quadratic in their number.
const MAX_EXTENDED_XMP_CHUNKS: usize = 1024;

/// Whether JPEG `bytes` may be handed to a decoder (`image`'s or `zune-jpeg`'s, which `image` uses),
/// headers included: not when it carries more than [`MAX_EXTENDED_XMP_CHUNKS`]. Every path that
/// decodes a JPEG from a file checks this first: image import here, and image export.
pub(crate) fn decodable_metadata(bytes: &[u8]) -> bool {
    extended_xmp_chunks(bytes) <= MAX_EXTENDED_XMP_CHUNKS
}

/// How many Extended XMP chunks (APP1 segments) come before the image data.
fn extended_xmp_chunks(bytes: &[u8]) -> usize {
    let mut chunks = 0usize;
    each_extended_xmp(bytes, |_| chunks += 1);
    chunks
}

/// JPEG `bytes` without their Extended XMP chunks, which nothing here reads: what is left decodes
/// in time linear in its size however many chunks the file carried.
pub(crate) fn without_extended_xmp(bytes: &[u8]) -> Vec<u8> {
    // Only the bytes that are kept: a CMYK JPEG can be mostly Extended XMP, and reserving the
    // whole file for the little that remains aborts the export under a tight address-space limit.
    let mut removed = 0usize;
    each_extended_xmp(bytes, |r| removed = removed.saturating_add(r.len()));
    let mut out = Vec::with_capacity(bytes.len().saturating_sub(removed));
    let mut at = 0;
    each_extended_xmp(bytes, |r| {
        out.extend_from_slice(bytes.get(at..r.start).unwrap_or_default());
        at = r.end;
    });
    out.extend_from_slice(bytes.get(at..).unwrap_or_default());
    out
}

/// Calls `f` with the byte range of each Extended XMP chunk (APP1 segment) before the image data.
fn each_extended_xmp(bytes: &[u8], mut f: impl FnMut(std::ops::Range<usize>)) {
    const PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
    let mut i = 2usize;
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
                let end = i.saturating_add(2).saturating_add(len.max(2));
                // The prefix must start the APP1 segment's own payload: a zero-length header
                // followed by prefix bytes is not a chunk.
                if marker == 0xE1 && bytes.get(i.saturating_add(4)..end.min(bytes.len())).is_some_and(|seg| seg.starts_with(PREFIX)) {
                    f(i..end.min(bytes.len()));
                }
                i = end;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DensityUnit {
    Inches,
    Centimeters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelDensity {
    density: (u16, u16),
    unit: DensityUnit,
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
                1 => DensityUnit::Inches,
                2 => DensityUnit::Centimeters,
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

fn jfif_density_dpi(bytes: &[u8]) -> Option<(f32, f32)> {
    let d = jfif_density(bytes)?;
    let scale = match d.unit {
        DensityUnit::Inches => 1.0,
        DensityUnit::Centimeters => 2.54,
    };
    Some((f32::from(d.density.0) * scale, f32::from(d.density.1) * scale))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tests::phone_jpeg;

    #[test]
    fn orientation_is_applied_and_plain_images_are_left_alone() {
        let photo = phone_jpeg(6, 144);
        let turned = upright(&photo);
        assert!(matches!(turned, Cow::Owned(_)));
        let decoded = craft_image_codecs::decode(&turned).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (20, 60));
        assert_eq!(jfif_density(&turned), Some(PixelDensity { density: (72, 144), unit: DensityUnit::Inches }));
        let stored = phone_jpeg(1, 72);
        assert!(matches!(upright(&stored), Cow::Borrowed(_)));
        assert!(matches!(upright(b"\x89PNG\r\n\x1a\n"), Cow::Borrowed(_)));
        assert!(matches!(upright(b"\xff\xd8 not a jpeg"), Cow::Borrowed(_)));
    }

    /// `jpeg` with `n` empty chunks of one never-complete Extended XMP packet after its SOI.
    pub(crate) fn with_extended_xmp_chunks(jpeg: &[u8], n: usize) -> Vec<u8> {
        let mut seg = b"http://ns.adobe.com/xmp/extension/\0".to_vec();
        seg.extend_from_slice(&[b'0'; 32]); // the packet's GUID
        seg.extend_from_slice(&1000u32.to_be_bytes()); // its full length
        seg.extend_from_slice(&0u32.to_be_bytes()); // this chunk's offset
        let mut chunk = vec![0xFF, 0xE1];
        chunk.extend_from_slice(&u16::try_from(seg.len() + 2).unwrap().to_be_bytes());
        chunk.extend_from_slice(&seg);
        [&jpeg[..2], b"\0".as_slice(), &chunk.repeat(n), &jpeg[2..]].concat()
    }

    #[test]
    fn a_jpeg_with_thousands_of_extended_xmp_chunks_is_not_decoded() {
        // Empty chunks of one never-complete extended packet: the decoder re-sorts and rescans every
        // chunk so far after each marker, quadratic in their number.
        let photo = phone_jpeg(6, 144);
        let with = |n: usize| with_extended_xmp_chunks(&photo, n);
        DECODES.with(|c| c.set(0));
        assert!(matches!(upright(&with(4)), Cow::Owned(_)), "a few chunks are an ordinary photo");
        assert!(matches!(upright(&with(5_000)), Cow::Borrowed(_)));
        assert_eq!(DECODES.with(std::cell::Cell::get), 1, "only the ordinary photo reached the decoder");
    }
}
