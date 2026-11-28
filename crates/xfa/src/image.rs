//! Pictures in templates and data: JPEG (embedded as is), PNG, GIF, BMP and TIFF (decoded to
//! 8-bit samples with a soft mask for transparency). Sizes are read from the headers without
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
    Bmp,
    Tiff,
}

/// What the bytes are, by their signature (the declared content type is often wrong).
pub fn kind(data: &[u8]) -> Option<Kind> {
    if data.starts_with(&[0xFF, 0xD8]) {
        Some(Kind::Jpeg)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if is_bmp(data) {
        Some(Kind::Bmp)
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Some(Kind::Tiff)
    } else {
        None
    }
}

/// "BM" alone is two letters of text: the info header that follows must have one of the sizes
/// the BMP versions define.
fn is_bmp(data: &[u8]) -> bool {
    data.starts_with(b"BM")
        && data.get(14..18).is_some_and(|b| matches!(u32::from_le_bytes([b[0], b[1], b[2], b[3]]), 12 | 16 | 40 | 52 | 56 | 64 | 108 | 124))
}

/// `data` with a BMP's pixel-data offset (`bfOffBits`) corrected when it points into the file or
/// info header or at its end with masks or a colour table still to come, as some writers leave it at 0: the rows then start after the headers, the colour
/// masks and the colour table. The `image` decoder seeks to the declared offset unchecked, so
/// such a picture would be read from the wrong place, shifted and wrapped. Anything else,
/// including a header too short to measure, is returned as it is.
pub fn bmp_with_pixel_offset(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    match bmp_pixel_offset_repair(data) {
        Some(offset) => {
            let mut fixed = data.to_vec();
            if let Some(field) = fixed.get_mut(10..14) {
                field.copy_from_slice(&offset.to_le_bytes());
            }
            std::borrow::Cow::Owned(fixed)
        }
        None => std::borrow::Cow::Borrowed(data),
    }
}

/// Where a BMP's rows start when its declared offset lies inside its headers, else `None`.
fn bmp_pixel_offset_repair(data: &[u8]) -> Option<u32> {
    let le16 = |at: usize| data.get(at..at + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
    let le32 = |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if !is_bmp(data) {
        return None;
    }
    let (declared, info_size) = (le32(10)?, le32(14)?);
    let headers_end = info_size.checked_add(14)?;
    // At the end of the headers is right only when no masks or colour table follow.
    if declared > headers_end {
        return None;
    }
    let (bpp, masks, used, entry) = if info_size == 12 {
        // BITMAPCOREHEADER: 16-bit fields, no colour count, 3-byte table entries.
        (le16(24)?, 0, 0, 3)
    } else if info_size < 40 {
        // OS/2's 16-byte header ends before the colour count; leave it to the decoder.
        return None;
    } else {
        // BI_BITFIELDS and BI_ALPHABITFIELDS put their masks after a 40-byte header; the larger
        // headers hold them inside.
        let masks = match (info_size, le32(30)?) {
            (40, 3) => 12,
            (40, 6) => 16,
            _ => 0,
        };
        (le16(28)?, masks, le32(46)?, 4)
    };
    let entries = match (used, bpp) {
        (0, 1..=8) => 1 << bpp,
        (0, _) => 0,
        (n, _) => n,
    };
    let offset = entries.checked_mul(entry)?.checked_add(masks)?.checked_add(headers_end)?;
    (offset != declared && usize::try_from(offset).ok()? <= data.len()).then_some(offset)
}

/// The pixel size, from the header alone; `None` when it can't be read or the image is bigger
/// than can be decoded (so layout gives such a picture its box, not a page-high strip).
pub fn size(data: &[u8]) -> Option<(u32, u32)> {
    header_size(data).filter(|&(w, h)| fits(w, h))
}

/// Lowest and highest resolution (in dots per inch) taken from an image header; anything outside
/// this range is treated as unknown so callers fall back to 72 dpi.
const MIN_DPI: f64 = 1.0;
const MAX_DPI: f64 = 10_000.0;

/// Resolution `(dpi_x, dpi_y)` read from the image header, without decoding:
/// - JPEG: JFIF `APP0` segment with density unit `1` (dots per inch) or `2` (dots per centimetre).
/// - PNG: `pHYs` chunk before `IDAT` with unit `1` (pixels per metre).
/// - TIFF: first IFD `XResolution` (282), `YResolution` (283) and `ResolutionUnit` (296; defaults
///   to `2` = inch, or `3` = centimetre).
/// - BMP: `BITMAPINFOHEADER` (40+ bytes) `biXPelsPerMeter` and `biYPelsPerMeter`.
///
/// Returns `None` for GIF, missing or unitless density headers, or zero, non-finite or
/// out-of-range values; callers assume 72 dpi.
pub fn resolution(data: &[u8]) -> Option<(f64, f64)> {
    let (dx, dy) = match kind(data)? {
        Kind::Jpeg => jpeg_resolution(data)?,
        Kind::Png => png_resolution(data)?,
        Kind::Tiff => tiff_resolution(data)?,
        Kind::Bmp => bmp_resolution(data)?,
        Kind::Gif => return None,
    };
    let ok = |v: f64| v.is_finite() && (MIN_DPI..=MAX_DPI).contains(&v);
    (ok(dx) && ok(dy)).then_some((dx, dy))
}

/// The picture's own size in points (`aspect="actual"`): its pixel size at its header
/// resolution, or at 72 dpi when it records none; `None` when the pixel size can't be read or is
/// zero. Drawing and measuring both use it, so a flowed picture takes the room it is drawn in.
pub fn actual_size(data: &[u8]) -> Option<(f64, f64)> {
    let (iw, ih) = size(data).filter(|&(w, h)| w > 0 && h > 0)?;
    let (dx, dy) = resolution(data).unwrap_or((72.0, 72.0));
    Some((f64::from(iw) * 72.0 / dx, f64::from(ih) * 72.0 / dy))
}

fn jpeg_resolution(data: &[u8]) -> Option<(f64, f64)> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    let mut guard = 0usize;
    while i + 4 <= data.len() && guard < 10_000 {
        guard += 1;
        if *data.get(i)? != 0xFF {
            i += 1;
            continue;
        }
        let marker = *data.get(i + 1)?;
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        if (0xD0..=0xD8).contains(&marker) || marker == 0x01 || marker == 0x00 {
            i = i.checked_add(2)?;
            continue;
        }
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        if len < 2 {
            return None;
        }
        if marker == 0xE0 && len >= 14 && data.get(i + 4..i + 9) == Some(b"JFIF\0") {
            let units = *data.get(i + 11)?;
            let xd = u16::from_be_bytes([*data.get(i + 12)?, *data.get(i + 13)?]);
            let yd = u16::from_be_bytes([*data.get(i + 14)?, *data.get(i + 15)?]);
            if xd > 0 && yd > 0 {
                match units {
                    1 => return Some((f64::from(xd), f64::from(yd))),
                    2 => return Some((f64::from(xd) * 2.54, f64::from(yd) * 2.54)),
                    _ => {}
                }
            }
        }
        i = i.checked_add(2)?.checked_add(len)?;
    }
    None
}

fn png_resolution(data: &[u8]) -> Option<(f64, f64)> {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut i = 8usize;
    let mut guard = 0usize;
    while i + 12 <= data.len() && guard < 1_024 {
        guard += 1;
        let &[l0, l1, l2, l3] = data.get(i..i + 4)? else {
            return None;
        };
        let len = usize::try_from(u32::from_be_bytes([l0, l1, l2, l3])).ok()?;
        let kind = data.get(i + 4..i + 8)?;
        let body_start = i.checked_add(8)?;
        let body_end = body_start.checked_add(len)?;
        let next = body_end.checked_add(4)?;
        if next > data.len() {
            return None;
        }
        match kind {
            b"pHYs" if len == 9 => {
                let &[x0, x1, x2, x3, y0, y1, y2, y3, unit] = data.get(body_start..body_end)? else {
                    return None;
                };
                if unit == 1 {
                    let px = u32::from_be_bytes([x0, x1, x2, x3]);
                    let py = u32::from_be_bytes([y0, y1, y2, y3]);
                    if px > 0 && py > 0 {
                        // Pixels per metre to dots per inch (1 in = 0.0254 m exact).
                        return Some((f64::from(px) * 0.0254, f64::from(py) * 0.0254));
                    }
                }
                return None;
            }
            b"IDAT" | b"IEND" => return None,
            _ => {}
        }
        i = next;
    }
    None
}

/// Most IFD entries scanned when reading TIFF resolution tags.
const MAX_TIFF_IFD_ENTRIES: usize = 4_096;

fn tiff_resolution(data: &[u8]) -> Option<(f64, f64)> {
    let le = if data.starts_with(b"II*\0") {
        true
    } else if data.starts_with(b"MM\0*") {
        false
    } else {
        return None;
    };
    let read_u16 = |at: usize| -> Option<u16> {
        let &[b0, b1] = data.get(at..at.checked_add(2)?)? else {
            return None;
        };
        Some(if le { u16::from_le_bytes([b0, b1]) } else { u16::from_be_bytes([b0, b1]) })
    };
    let read_u32 = |at: usize| -> Option<u32> {
        let &[b0, b1, b2, b3] = data.get(at..at.checked_add(4)?)? else {
            return None;
        };
        Some(if le { u32::from_le_bytes([b0, b1, b2, b3]) } else { u32::from_be_bytes([b0, b1, b2, b3]) })
    };
    let read_rational = |val_offset: usize| -> Option<f64> {
        let off = usize::try_from(read_u32(val_offset)?).ok()?;
        let num = read_u32(off)?;
        let den = read_u32(off.checked_add(4)?)?;
        (den > 0).then(|| f64::from(num) / f64::from(den))
    };

    let ifd = usize::try_from(read_u32(4)?).ok()?;
    let count = usize::from(read_u16(ifd)?);
    let entries_start = ifd.checked_add(2)?;
    let mut x_res = None;
    let mut y_res = None;
    let mut unit = None;
    let mut turned = false;
    for i in 0..count.min(MAX_TIFF_IFD_ENTRIES) {
        let entry = entries_start.checked_add(i.checked_mul(12)?)?;
        let tag = read_u16(entry)?;
        let ty = read_u16(entry.checked_add(2)?)?;
        let num_values = read_u32(entry.checked_add(4)?)?;
        let val_field = entry.checked_add(8)?;
        if val_field.checked_add(4)? > data.len() {
            return None;
        }
        match tag {
            274 if ty == 3 && num_values == 1 => {
                turned = (5..=8).contains(&read_u16(val_field)?);
            }
            282 => {
                if ty != 5 || num_values != 1 {
                    return None;
                }
                x_res = Some(read_rational(val_field)?);
            }
            283 => {
                if ty != 5 || num_values != 1 {
                    return None;
                }
                y_res = Some(read_rational(val_field)?);
            }
            296 => {
                if ty != 3 || num_values != 1 {
                    return None;
                }
                unit = Some(read_u16(val_field)?);
            }
            _ => {}
        }
    }
    let (x, y) = (x_res?, y_res?);
    let scale = match unit.unwrap_or(2) {
        2 => 1.0,
        3 => 2.54,
        _ => return None,
    };
    let (dx, dy) = if turned { (y, x) } else { (x, y) };
    Some((dx * scale, dy * scale))
}

fn bmp_resolution(data: &[u8]) -> Option<(f64, f64)> {
    if !is_bmp(data) {
        return None;
    }
    let &[s0, s1, s2, s3] = data.get(14..18)? else {
        return None;
    };
    if u32::from_le_bytes([s0, s1, s2, s3]) < 40 {
        return None;
    }
    let &[x0, x1, x2, x3, y0, y1, y2, y3] = data.get(38..46)? else {
        return None;
    };
    let xp = i32::from_le_bytes([x0, x1, x2, x3]);
    let yp = i32::from_le_bytes([y0, y1, y2, y3]);
    if xp <= 0 || yp <= 0 {
        return None;
    }
    // Pixels per metre to dots per inch (1 in = 0.0254 m exact).
    Some((f64::from(xp) * 0.0254, f64::from(yp) * 0.0254))
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
        // The decoders read only the headers (TIFF: the first IFD) until asked for pixels.
        Kind::Bmp => craft_image_codecs::probe_size(&bmp_with_pixel_offset(data)).ok().filter(|&(w, h)| w > 0 && h > 0),
        Kind::Tiff => {
            let info = craft_image_codecs::tiff_info(data).ok()?;
            let page = info.default_page().and_then(|i| info.pages.get(i))?;
            // Shown turned a quarter: the size viewers show is the stored one swapped.
            let turned = (5..=8).contains(&craft_image_codecs::tiff_orientation(data));
            let (w, h) = if turned { (page.height, page.width) } else { (page.width, page.height) };
            (w > 0 && h > 0).then_some((w, h))
        }
    }
}

fn decode_limits() -> craft_image_codecs::Limits {
    craft_image_codecs::Limits { max_width: MAX_SIDE, max_height: MAX_SIDE, max_pixels: MAX_PIXELS, max_alloc: MAX_PIXELS * 4 }
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

/// Decode a PNG, GIF (its first frame), BMP or TIFF (its first page). JPEGs are embedded as they
/// are and are not decoded here. The error is the warning to show.
pub fn decode(data: &[u8]) -> Result<Decoded, String> {
    let bad = |m: String| format!("an image could not be decoded and is left out: {m}");
    let (w, h) = header_size(data).ok_or_else(|| bad("unreadable header".into()))?;
    if !fits(w, h) {
        return Err(too_big());
    }
    match kind(data) {
        Some(Kind::Png) => {
            let opts = craft_image_codecs::DecodeOptions { keep_orientation: true, limits: decode_limits() };
            let img = craft_image_codecs::decode_as_with(craft_image_codecs::Format::Png, data, &opts).map_err(|e| match e {
                craft_image_codecs::CodecError::LimitExceeded(_) => too_big(),
                other => bad(other.to_string()),
            })?;
            let (fw, fh) = (img.width(), img.height());
            if !fits(fw, fh) {
                return Err(too_big());
            }
            let (channels, color_space) = match img.layout() {
                craft_image_codecs::ChannelLayout::Gray => (1, "DeviceGray"),
                craft_image_codecs::ChannelLayout::GrayA => (2, "DeviceGray"),
                craft_image_codecs::ChannelLayout::Rgb => (3, "DeviceRGB"),
                craft_image_codecs::ChannelLayout::Rgba => (4, "DeviceRGB"),
                _ => return Err(bad("unexpected PNG colour layout".into())),
            };
            let samples: std::borrow::Cow<'_, [u8]> = if let Some(u8s) = img.as_u8() {
                std::borrow::Cow::Borrowed(u8s)
            } else if let Some(u16s) = img.to_u16_samples() {
                std::borrow::Cow::Owned(u16s.into_iter().map(|v| (v >> 8) as u8).collect())
            } else {
                return Err(bad("expected 8-bit or 16-bit samples".into()));
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
            Ok(Decoded { width: fw, height: fh, color_space, samples: colour, alpha })
        }
        Some(k @ (Kind::Gif | Kind::Bmp | Kind::Tiff)) => {
            let format = match k {
                Kind::Gif => craft_image_codecs::Format::Gif,
                Kind::Bmp => craft_image_codecs::Format::Bmp,
                _ => craft_image_codecs::Format::Tiff,
            };
            let data = if k == Kind::Bmp { bmp_with_pixel_offset(data) } else { std::borrow::Cow::Borrowed(data) };
            let opts = craft_image_codecs::DecodeOptions { keep_orientation: false, limits: decode_limits() };
            let img = craft_image_codecs::decode_as_with(format, &data, &opts).map_err(|e| match e {
                craft_image_codecs::CodecError::LimitExceeded(_) => too_big(),
                other => bad(other.to_string()),
            })?;
            let raw = img.to_rgba8();
            let (width, height) = (img.width(), img.height());
            if !fits(width, height) {
                return Err(too_big());
            }
            let px = raw.as_chunks::<4>().0;
            let gray = px.iter().all(|p| p[0] == p[1] && p[1] == p[2]);
            let samples: Vec<u8> = if gray { px.iter().map(|p| p[0]).collect() } else { px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect() };
            let alpha = px.iter().any(|p| p[3] != 255).then(|| px.iter().map(|p| p[3]).collect());
            Ok(Decoded { width, height, color_space: if gray { "DeviceGray" } else { "DeviceRGB" }, samples, alpha })
        }
        Some(Kind::Jpeg) | None => Err(bad("not a PNG, GIF, BMP or TIFF".into())),
    }
}
