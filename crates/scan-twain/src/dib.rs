//! Convert a Windows DIB (Device-Independent Bitmap, as returned by TWAIN `DAT_IMAGENATIVEXFER`)
//! into a standalone BMP file with accurate physical DPI metadata.

/// Convert raw DIB bytes (starting with `BITMAPINFOHEADER`) into a valid `.bmp` file,
/// patching `biXPelsPerMeter` and `biYPelsPerMeter` when the driver left them unset or
/// when an explicit DPI from `TW_IMAGEINFO` is available.
pub fn dib_to_bmp(dib: &[u8], dpi_x: f64, dpi_y: f64) -> Result<Vec<u8>, String> {
    if dib.len() < 40 {
        return Err("scanner returned a truncated bitmap header".into());
    }
    let header_size = u32::from_le_bytes([dib[0], dib[1], dib[2], dib[3]]) as usize;
    if header_size < 40 || header_size > dib.len() {
        return Err(format!("unsupported DIB header size ({header_size} bytes)"));
    }
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    let clr_used = u32::from_le_bytes([dib[32], dib[33], dib[34], dib[35]]) as usize;

    let palette_entries = if clr_used > 0 {
        clr_used
    } else if bit_count > 0 && bit_count <= 8 {
        1usize << (bit_count as usize)
    } else {
        0
    };
    // BI_BITFIELDS (3) adds 3 DWORD masks (12 bytes); BI_ALPHABITFIELDS (6) adds 4 DWORD masks (16 bytes).
    let masks_bytes = if header_size == 40 {
        match compression {
            3 => 12usize,
            6 => 16usize,
            _ => 0usize,
        }
    } else {
        0usize
    };
    let palette_bytes = palette_entries.checked_mul(4).ok_or_else(|| "DIB palette size overflow".to_string())?;
    let pixel_offset_in_dib =
        header_size.checked_add(palette_bytes).and_then(|v| v.checked_add(masks_bytes)).ok_or_else(|| "DIB header offset overflow".to_string())?;
    if pixel_offset_in_dib > dib.len() {
        return Err("scanner returned a DIB shorter than its header and palette".into());
    }

    let file_size =
        14usize.checked_add(dib.len()).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| "scanned image exceeds BMP size limit".to_string())?;
    let bf_off_bits =
        14usize.checked_add(pixel_offset_in_dib).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| "BMP pixel offset overflow".to_string())?;

    let mut out = Vec::with_capacity(14 + dib.len());
    // BITMAPFILEHEADER (14 bytes)
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&bf_off_bits.to_le_bytes());
    out.extend_from_slice(dib);

    // Patch biXPelsPerMeter (offset 14+24) and biYPelsPerMeter (offset 14+28) if valid DPI is given
    // and the DIB left them 0.
    let existing_x = i32::from_le_bytes([out[38], out[39], out[40], out[41]]);
    let existing_y = i32::from_le_bytes([out[42], out[43], out[44], out[45]]);
    if existing_x <= 0 && dpi_x.is_finite() && dpi_x > 0.0 {
        let ppm_x = (dpi_x / 0.0254).round() as i32;
        out[38..42].copy_from_slice(&ppm_x.to_le_bytes());
    }
    if existing_y <= 0 && dpi_y.is_finite() && dpi_y > 0.0 {
        let ppm_y = (dpi_y / 0.0254).round() as i32;
        out[42..46].copy_from_slice(&ppm_y.to_le_bytes());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dib_24bit(w: i32, h: i32) -> Vec<u8> {
        let stride = ((w * 3 + 3) / 4) * 4;
        let img_bytes = (stride * h.abs()) as usize;
        let mut dib = vec![0u8; 40 + img_bytes];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&w.to_le_bytes());
        dib[8..12].copy_from_slice(&h.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        dib[20..24].copy_from_slice(&(img_bytes as u32).to_le_bytes());
        dib
    }

    #[test]
    fn dib_to_bmp_prepends_file_header_and_sets_600_dpi() {
        let dib = sample_dib_24bit(4, 2);
        let bmp = dib_to_bmp(&dib, 600.0, 600.0).expect("valid BMP");
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(u32::from_le_bytes([bmp[2], bmp[3], bmp[4], bmp[5]]) as usize, bmp.len());
        assert_eq!(u32::from_le_bytes([bmp[10], bmp[11], bmp[12], bmp[13]]), 54);
        let ppm_x = i32::from_le_bytes([bmp[38], bmp[39], bmp[40], bmp[41]]);
        assert_eq!(ppm_x, 23622); // 600 / 0.0254 = 23622.047
    }

    #[test]
    fn dib_to_bmp_rejects_truncated_header() {
        assert!(dib_to_bmp(&[0u8; 16], 300.0, 300.0).is_err());
    }
}
