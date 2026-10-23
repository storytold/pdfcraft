//! The resolution an encoded image says it has: BMP pels per metre, PNG `pHYs`, JPEG JFIF density (and the EXIF
//! resolution when JFIF has none), TIFF `XResolution`/`YResolution` with `ResolutionUnit`.
//!
//! Used to size a page made from an image (`pixels / dpi * 72`) the same way for every format. The same rules are
//! applied by `craft-scan` to the pages a scanner delivers, so a scan and the PDF made from it agree.
//!
//! Rules, the same for every container:
//! * the unit must be a real one (BMP/PNG: per metre; JFIF: 1 = dots per inch, 2 = dots per cm; TIFF: 2 = inch,
//!   3 = centimetre; 0 in JFIF and 1 in TIFF mean "aspect ratio only" and give nothing);
//! * both axes must be present and inside [`MIN_DPI`]..=[`MAX_DPI`] (zero, negative, NaN and absurd values give
//!   nothing: a file claiming 1 dpi or 1e9 dpi is a broken header, not a scanner);
//! * the two axes may differ (a 200 x 100 dpi fax mode is real), and are reported as they are.
//!
//! `None` means "the file says nothing usable"; the caller then falls back to 72 dpi (one point per pixel).

/// Smallest density taken as real (below it, a header is treated as broken).
pub const MIN_DPI: f64 = 10.0;
/// Largest density taken as real (flatbeds go to 9600 optical).
pub const MAX_DPI: f64 = 9600.0;

/// Horizontal and vertical dots per inch.
pub type Density = (f64, f64);

/// The density an encoded image carries, or `None` when it has none that can be used.
pub fn image_density(bytes: &[u8]) -> Option<Density> {
    let found = if bytes.starts_with(b"BM") {
        bmp_density(bytes)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png_density(bytes)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg_density(bytes)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        tiff_density(bytes)
    } else {
        None
    };
    found.and_then(sane)
}

/// The density of a TIFF page from its decoded tags: `unit` is `ResolutionUnit` (2 inch, 3 centimetre, anything else
/// none) and `x`/`y` the rational resolutions already divided out.
pub fn tiff_tag_density(unit: u32, x: Option<f64>, y: Option<f64>) -> Option<Density> {
    let scale = match unit {
        2 => 1.0,
        3 => 2.54,
        _ => return None,
    };
    sane((x? * scale, y? * scale))
}

/// The density when both axes lie within `MIN_DPI..=MAX_DPI` and are finite; otherwise `None`.
fn sane((x, y): Density) -> Option<Density> {
    let ok = |v: f64| v.is_finite() && (MIN_DPI..=MAX_DPI).contains(&v);
    (ok(x) && ok(y)).then_some((x, y))
}

/// The big-endian `u16` at byte `at`, or `None` when it runs past the end.
fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

/// The big-endian `u32` at byte `at`, or `None` when it runs past the end.
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Convert pixels per metre to dots per inch; `None` when either axis is zero.
fn per_metre((x, y): (u32, u32)) -> Option<Density> {
    (x != 0 && y != 0).then(|| (f64::from(x) * 0.0254, f64::from(y) * 0.0254))
}

/// `biXPelsPerMeter`/`biYPelsPerMeter` of a BITMAPINFOHEADER (the OS/2 core header has none).
fn bmp_density(bytes: &[u8]) -> Option<Density> {
    let header = u32::from_le_bytes(bytes.get(14..18)?.try_into().ok()?);
    if header < 40 {
        return None;
    }
    let x = i32::from_le_bytes(bytes.get(38..42)?.try_into().ok()?);
    let y = i32::from_le_bytes(bytes.get(42..46)?.try_into().ok()?);
    per_metre((u32::try_from(x).ok()?, u32::try_from(y).ok()?))
}

/// `pHYs` (pixels per unit x and y, unit 1 = metre; unit 0 is the aspect ratio only). It must precede `IDAT`.
fn png_density(bytes: &[u8]) -> Option<Density> {
    let mut at = 8usize;
    while let Some(len) = be32(bytes, at) {
        let kind = bytes.get(at + 4..at + 8)?;
        let body = at.checked_add(8)?;
        match kind {
            b"pHYs" if len == 9 => {
                if *bytes.get(body + 8)? != 1 {
                    return None;
                }
                return per_metre((be32(bytes, body)?, be32(bytes, body + 4)?));
            }
            b"IDAT" | b"IEND" => return None,
            _ => {}
        }
        at = body.checked_add(usize::try_from(len).ok()?)?.checked_add(4)?;
    }
    None
}

/// JFIF (`APP0`) density, else the EXIF resolution of `APP1`.
fn jpeg_density(bytes: &[u8]) -> Option<Density> {
    let mut exif = None;
    let mut at = 2usize;
    while let Some(&byte) = bytes.get(at) {
        if byte != 0xff {
            at += 1;
            continue;
        }
        at += 1;
        while bytes.get(at) == Some(&0xff) {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let len = usize::from(be16(bytes, at)?);
        let seg = bytes.get(at + 2..at.checked_add(len)?)?;
        match marker {
            0xe0 if seg.starts_with(b"JFIF\0") && seg.len() >= 12 => {
                let (x, y) = (f64::from(be16(seg, 8)?), f64::from(be16(seg, 10)?));
                let found = match seg[7] {
                    1 => Some((x, y)),
                    2 => Some((x * 2.54, y * 2.54)),
                    _ => None,
                };
                if let Some(found) = found.and_then(sane) {
                    return Some(found);
                }
            }
            0xe1 if exif.is_none() && seg.starts_with(b"Exif\0\0") => exif = tiff_density(&seg[6..]).and_then(sane),
            _ => {}
        }
        at += len;
    }
    exif
}

/// `XResolution`, `YResolution` and `ResolutionUnit` of the first IFD of a TIFF (or of an EXIF block, which is one).
fn tiff_density(t: &[u8]) -> Option<Density> {
    let little = match t.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = t.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = t.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    // The unit defaults to inch when the tag is missing (TIFF 6.0).
    let (mut x, mut y, mut unit) = (None, None, 2u16);
    for index in 0..count {
        let entry = ifd.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
        let (tag, kind) = (u16_at(entry)?, u16_at(entry + 2)?);
        match tag {
            0x011a | 0x011b if kind == 5 && u32_at(entry + 4)? == 1 => {
                let at = usize::try_from(u32_at(entry + 8)?).ok()?;
                let (num, den) = (u32_at(at)?, u32_at(at + 4)?);
                let value = (den != 0).then(|| f64::from(num) / f64::from(den));
                if tag == 0x011a {
                    x = value;
                } else {
                    y = value;
                }
            }
            0x0128 if kind == 3 => unit = u16_at(entry + 8)?,
            _ => {}
        }
    }
    let scale = match unit {
        2 => 1.0,
        3 => 2.54,
        _ => return None,
    };
    Some((x? * scale, y? * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 62-byte bitmap with a 2x2 pixel size, the given pixels-per-metre and info-header size.
    fn bmp(x: i32, y: i32, header: u32) -> Vec<u8> {
        let mut b = vec![0u8; 62];
        b[..2].copy_from_slice(b"BM");
        b[14..18].copy_from_slice(&header.to_le_bytes());
        b[18..22].copy_from_slice(&2i32.to_le_bytes());
        b[22..26].copy_from_slice(&2i32.to_le_bytes());
        b[38..42].copy_from_slice(&x.to_le_bytes());
        b[42..46].copy_from_slice(&y.to_le_bytes());
        b
    }

    /// A minimal PNG signature and header, with a `pHYs` chunk of `unit`, `x` and `y` when `phys` is set.
    fn png(unit: u8, x: u32, y: u32, phys: bool) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(&13u32.to_be_bytes());
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&[0; 13]);
        b.extend_from_slice(&[0; 4]);
        if phys {
            b.extend_from_slice(&9u32.to_be_bytes());
            b.extend_from_slice(b"pHYs");
            b.extend_from_slice(&x.to_be_bytes());
            b.extend_from_slice(&y.to_be_bytes());
            b.push(unit);
            b.extend_from_slice(&[0; 4]);
        }
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(b"IDAT");
        b.extend_from_slice(&[0; 4]);
        b
    }

    /// `jfif_unit` 255 leaves JFIF out.
    fn jpeg(jfif_unit: u8, x: u16, y: u16, exif: Option<Vec<u8>>) -> Vec<u8> {
        let mut b = vec![0xff, 0xd8];
        if let Some(t) = exif {
            b.extend_from_slice(&[0xff, 0xe1]);
            b.extend_from_slice(&u16::try_from(t.len() + 8).unwrap().to_be_bytes());
            b.extend_from_slice(b"Exif\0\0");
            b.extend_from_slice(&t);
        }
        if jfif_unit != 255 {
            b.extend_from_slice(&[0xff, 0xe0, 0, 16]);
            b.extend_from_slice(b"JFIF\0\x01\x01");
            b.push(jfif_unit);
            b.extend_from_slice(&x.to_be_bytes());
            b.extend_from_slice(&y.to_be_bytes());
            b.extend_from_slice(&[0, 0]);
        }
        b.extend_from_slice(&[0xff, 0xda, 0, 2]);
        b
    }

    /// A TIFF with two rational resolutions `x` and `y` (numerator, denominator) and, when given, a resolution unit tag,
    /// in little- or big-endian byte order.
    fn tiff(little: bool, unit: Option<u16>, x: (u32, u32), y: (u32, u32)) -> Vec<u8> {
        let w16 = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let w32 = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let entries = 2 + usize::from(unit.is_some());
        let data = 8 + 2 + entries * 12 + 4;
        let mut b = if little { b"II".to_vec() } else { b"MM".to_vec() };
        b.extend_from_slice(&w16(42));
        b.extend_from_slice(&w32(8));
        b.extend_from_slice(&w16(entries as u16));
        for (tag, at) in [(0x011au16, data), (0x011b, data + 8)] {
            b.extend_from_slice(&w16(tag));
            b.extend_from_slice(&w16(5));
            b.extend_from_slice(&w32(1));
            b.extend_from_slice(&w32(at as u32));
        }
        if let Some(u) = unit {
            b.extend_from_slice(&w16(0x0128));
            b.extend_from_slice(&w16(3));
            b.extend_from_slice(&w32(1));
            b.extend_from_slice(&w16(u));
            b.extend_from_slice(&[0, 0]);
        }
        b.extend_from_slice(&w32(0));
        for (n, d) in [x, y] {
            b.extend_from_slice(&w32(n));
            b.extend_from_slice(&w32(d));
        }
        b
    }

    /// Assert that `found` holds a density within 0.05 of `x` by `y`.
    fn near(found: Option<Density>, x: f64, y: f64) {
        let (fx, fy) = found.expect("a density");
        assert!((fx - x).abs() < 0.05 && (fy - y).abs() < 0.05, "{fx} x {fy}, wanted {x} x {y}");
    }

    #[test]
    fn bmp_pels_per_metre_are_dots_per_inch() {
        near(image_density(&bmp(5906, 5906, 40)), 150.01, 150.01);
        near(image_density(&bmp(3937, 7874, 40)), 100.0, 200.0);
        near(image_density(&bmp(5906, 5906, 124)), 150.01, 150.01);
    }

    #[test]
    fn bmp_without_a_usable_density_gives_none() {
        assert_eq!(image_density(&bmp(0, 0, 40)), None);
        assert_eq!(image_density(&bmp(5906, 0, 40)), None);
        assert_eq!(image_density(&bmp(-5906, -5906, 40)), None);
        assert_eq!(image_density(&bmp(5906, 5906, 12)), None, "the OS/2 core header has no density");
        assert_eq!(image_density(&bmp(1, 1, 40)), None, "0.0254 dpi is a broken header");
        assert_eq!(image_density(&bmp(i32::MAX, i32::MAX, 40)), None, "54 million dpi is absurd");
        assert_eq!(image_density(&bmp(5906, 5906, 40)[..30]), None, "truncated");
    }

    #[test]
    fn png_phys_in_metres_only() {
        near(image_density(&png(1, 5906, 5906, true)), 150.01, 150.01);
        near(image_density(&png(1, 3937, 11811, true)), 100.0, 300.0);
        assert_eq!(image_density(&png(0, 1, 1, true)), None, "unit 0 is an aspect ratio");
        assert_eq!(image_density(&png(1, 0, 5906, true)), None);
        assert_eq!(image_density(&png(1, u32::MAX, u32::MAX, true)), None);
        assert_eq!(image_density(&png(1, 5906, 5906, false)), None);
    }

    #[test]
    fn jfif_units() {
        near(image_density(&jpeg(1, 150, 150, None)), 150.0, 150.0);
        near(image_density(&jpeg(1, 200, 100, None)), 200.0, 100.0);
        near(image_density(&jpeg(2, 118, 118, None)), 299.72, 299.72);
        assert_eq!(image_density(&jpeg(0, 1, 1, None)), None, "units 0 is the aspect ratio");
        assert_eq!(image_density(&jpeg(1, 0, 0, None)), None);
        assert_eq!(image_density(&jpeg(1, 150, 0, None)), None);
        assert_eq!(image_density(&jpeg(1, 1, 1, None)), None, "1 dpi is absurd");
        assert_eq!(image_density(&jpeg(1, 60000, 60000, None)), None);
        assert_eq!(image_density(&jpeg(255, 0, 0, None)), None, "no JFIF, no EXIF");
    }

    #[test]
    fn exif_density_is_used_when_jfif_has_none() {
        let exif = tiff(true, Some(2), (300, 1), (300, 1));
        near(image_density(&jpeg(255, 0, 0, Some(exif.clone()))), 300.0, 300.0);
        near(image_density(&jpeg(0, 1, 1, Some(exif.clone()))), 300.0, 300.0);
        near(image_density(&jpeg(1, 150, 150, Some(exif))), 150.0, 150.0);
    }

    #[test]
    fn tiff_units_and_byte_orders() {
        for little in [true, false] {
            near(image_density(&tiff(little, Some(2), (150, 1), (150, 1))), 150.0, 150.0);
            near(image_density(&tiff(little, None, (150, 1), (150, 1))), 150.0, 150.0);
            near(image_density(&tiff(little, Some(3), (118, 1), (59, 1))), 299.72, 149.86);
            near(image_density(&tiff(little, Some(2), (3000, 20), (150_000, 1000))), 150.0, 150.0);
            assert_eq!(image_density(&tiff(little, Some(1), (150, 1), (150, 1))), None, "unit none");
            assert_eq!(image_density(&tiff(little, Some(2), (0, 1), (150, 1))), None);
            assert_eq!(image_density(&tiff(little, Some(2), (150, 0), (150, 1))), None);
            assert_eq!(image_density(&tiff(little, Some(2), (1, 1), (1, 1))), None);
            assert_eq!(image_density(&tiff(little, Some(2), (u32::MAX, 1), (150, 1))), None);
        }
    }

    #[test]
    fn unknown_and_short_input_gives_none() {
        assert_eq!(image_density(b""), None);
        assert_eq!(image_density(b"GIF89a"), None);
        assert_eq!(image_density(&[0xff, 0xd8]), None);
        assert_eq!(image_density(b"II*\0"), None);
    }
}
