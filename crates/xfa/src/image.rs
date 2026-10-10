//! Pictures in templates and data: JPEG (embedded as is), PNG and GIF (decoded to 8-bit
//! samples with a soft mask for transparency). Sizes are read from the headers without
//! decoding, so layout stays cheap; decoding happens once, when the PDF is written.

/// Most pixels an image may have once decoded: decoding keeps up to 8 bytes per pixel alive
/// (the decoder's buffer, the colour samples and the coverage), so this is 160 MB at most.
pub const MAX_PIXELS: u64 = 20_000_000;
/// Widest or tallest an image may be (PNG headers can claim billions).
const MAX_SIDE: u32 = 65_535;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Gif,
}

/// What the bytes are, by their signature (the declared content type is often wrong).
pub fn kind(data: &[u8]) -> Option<Kind> {
    if data.starts_with(&[0xFF, 0xD8]) {
        Some(Kind::Jpeg)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else {
        None
    }
}

/// The pixel size, from the header alone; `None` when it can't be read or the image is bigger
/// than can be decoded (so layout gives such a picture its box, not a page-high strip).
pub fn size(data: &[u8]) -> Option<(u32, u32)> {
    header_size(data).filter(|&(w, h)| fits(w, h))
}

fn fits(w: u32, h: u32) -> bool {
    w <= MAX_SIDE && h <= MAX_SIDE && u64::from(w) * u64::from(h) <= MAX_PIXELS
}

/// What the header claims, unchecked.
fn header_size(data: &[u8]) -> Option<(u32, u32)> {
    match kind(data)? {
        Kind::Jpeg => crate::pdf::jpeg_size(data),
        Kind::Png => {
            // The IHDR chunk is first: length, "IHDR", width, height (big-endian).
            if data.get(12..16)? != b"IHDR" {
                return None;
            }
            let be = |at: usize| data.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            let (w, h) = (be(16)?, be(20)?);
            (w > 0 && h > 0).then_some((w, h))
        }
        Kind::Gif => {
            let le = |at: usize| data.get(at..at + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
            let (w, h) = (le(6)?, le(8)?);
            (w > 0 && h > 0).then_some((w, h))
        }
    }
}

/// Samples ready for an image XObject.
#[derive(Debug)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    /// `DeviceGray` or `DeviceRGB`.
    pub color_space: &'static str,
    /// 8-bit samples, row by row.
    pub samples: Vec<u8>,
    /// 8-bit coverage per pixel when any pixel is not opaque.
    pub alpha: Option<Vec<u8>>,
}

fn too_big() -> String {
    format!("an image has more than {} million pixels and is left out", MAX_PIXELS / 1_000_000)
}

/// Decode a PNG or GIF (its first frame). JPEGs are embedded as they are and are not decoded
/// here. The error is the warning to show.
pub fn decode(data: &[u8]) -> Result<Decoded, String> {
    let bad = |m: String| format!("an image could not be decoded and is left out: {m}");
    let (w, h) = header_size(data).ok_or_else(|| bad("unreadable header".into()))?;
    if !fits(w, h) {
        return Err(too_big());
    }
    match kind(data) {
        Some(Kind::Png) => {
            let mut dec = png::Decoder::new(std::io::Cursor::new(data));
            dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
            let mut reader = dec.read_info().map_err(|e| bad(e.to_string()))?;
            // The header's size is what the buffer is sized by: checked above.
            let (fw, fh) = (reader.info().width, reader.info().height);
            if !fits(fw, fh) {
                return Err(too_big());
            }
            let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| bad("image too large".into()))?];
            let frame = reader.next_frame(&mut buf).map_err(|e| bad(e.to_string()))?;
            let samples = buf.get(..frame.buffer_size()).ok_or_else(|| bad("short image data".into()))?;
            let (channels, color_space) = match frame.color_type {
                png::ColorType::Grayscale => (1, "DeviceGray"),
                png::ColorType::GrayscaleAlpha => (2, "DeviceGray"),
                png::ColorType::Rgb => (3, "DeviceRGB"),
                png::ColorType::Rgba => (4, "DeviceRGB"),
                png::ColorType::Indexed => return Err(bad("unexpected palette output".into())),
            };
            let has_alpha = channels == 2 || channels == 4;
            let colour_n = channels - usize::from(has_alpha);
            let mut colour = Vec::with_capacity(samples.len() / channels * colour_n);
            let mut alpha = Vec::with_capacity(if has_alpha { samples.len() / channels } else { 0 });
            for px in samples.chunks_exact(channels) {
                colour.extend_from_slice(px.get(..colour_n).unwrap_or_default());
                if has_alpha && let Some(&a) = px.get(colour_n) {
                    alpha.push(a);
                }
            }
            let alpha = (has_alpha && alpha.iter().any(|a| *a != 255)).then_some(alpha);
            Ok(Decoded { width: frame.width, height: frame.height, color_space, samples: colour, alpha })
        }
        Some(Kind::Gif) => {
            // The frame may be larger than the logical screen the header gives: the decoder is
            // told the limits, so it refuses before allocating.
            let mut reader = image::ImageReader::new(std::io::Cursor::new(data));
            reader.set_format(image::ImageFormat::Gif);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_SIDE);
            limits.max_image_height = Some(MAX_SIDE);
            limits.max_alloc = Some(MAX_PIXELS * 4);
            reader.limits(limits);
            let img = reader.decode().map_err(|e| if matches!(e, image::ImageError::Limits(_)) { too_big() } else { bad(e.to_string()) })?;
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            if !fits(width, height) {
                return Err(too_big());
            }
            let px = rgba.as_raw().as_chunks::<4>().0;
            let gray = px.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
            let samples: Vec<u8> = if gray { px.iter().map(|p| p[0]).collect() } else { px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect() };
            let alpha = px.iter().any(|p| p[3] != 255).then(|| px.iter().map(|p| p[3]).collect());
            Ok(Decoded { width, height, color_space: if gray { "DeviceGray" } else { "DeviceRGB" }, samples, alpha })
        }
        Some(Kind::Jpeg) | None => Err(bad("not a PNG or GIF".into())),
    }
}
