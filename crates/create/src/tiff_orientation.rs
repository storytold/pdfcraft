//! Normalize a TIFF directory's pixels to top-left orientation before embedding in PDF.

use super::{Embedded, MAX_DECODED_BYTES};
use pdfcraft_cos::Object;

pub(super) fn orient(page: &mut Embedded, orientation: u32) -> Result<(), &'static str> {
    if !(2..=8).contains(&orientation) {
        return Ok(());
    }
    let bits = match (page.dict.name(b"ColorSpace"), page.dict.int(b"BitsPerComponent")) {
        (Some(b"DeviceGray"), Some(1)) => 1,
        (Some(b"DeviceGray"), Some(8)) => 8,
        (Some(b"DeviceRGB"), Some(8)) => 24,
        (Some(b"DeviceCMYK"), Some(8)) => 32,
        _ => return Err("this TIFF pixel layout cannot be oriented"),
    };
    // Bound the transient pixel buffers too: the source remains alive while its replacement
    // is built, alongside any alpha plane. Earlier TIFF directories have their own total cap.
    let room = |page: &Embedded| {
        MAX_DECODED_BYTES.saturating_sub(page.data.capacity()).saturating_sub(page.smask.as_ref().map_or(0, |(_, alpha)| alpha.capacity()))
    };
    page.data = samples(&page.data, page.px, bits, orientation, room(page))?;
    let alpha_room = room(page);
    if let Some((_, alpha)) = page.smask.as_mut() {
        *alpha = samples(alpha, page.px, 8, orientation, alpha_room)?;
    }
    if orientation >= 5 {
        page.px = (page.px.1, page.px.0);
        page.dpi = (page.dpi.1, page.dpi.0);
    }
    if let Some((dict, _)) = page.smask.as_mut() {
        dict.set(b"Width".to_vec(), Object::Int(i64::from(page.px.0)));
        dict.set(b"Height".to_vec(), Object::Int(i64::from(page.px.1)));
    }
    Ok(())
}

fn samples(data: &[u8], dimensions: (u32, u32), bits: usize, orientation: u32, room: usize) -> Result<Vec<u8>, &'static str> {
    let bad = "the TIFF's oriented pixel dimensions are invalid or too large";
    let (w, h) = (usize::try_from(dimensions.0).map_err(|_| bad)?, usize::try_from(dimensions.1).map_err(|_| bad)?);
    if w == 0 || h == 0 {
        return Err(bad);
    }
    let stride = |width: usize| width.checked_mul(bits).and_then(|n| n.checked_add(7)).map(|n| n / 8).ok_or(bad);
    let source_stride = stride(w)?;
    if source_stride.checked_mul(h) != Some(data.len()) {
        return Err("the TIFF's decoded pixels do not match its dimensions");
    }
    let (nw, nh) = if orientation >= 5 { (h, w) } else { (w, h) };
    let output_stride = stride(nw)?;
    let len = output_stride.checked_mul(nh).ok_or(bad)?;
    if len > room {
        return Err("orienting this TIFF would exceed 2048 MiB of decoded pixel buffers");
    }
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| "not enough memory to orient this TIFF")?;
    out.resize(len, 0);
    for (y, row) in out.chunks_exact_mut(output_stride).enumerate() {
        for x in 0..nw {
            // x/y are bounded by the validated output dimensions; reflected coordinates are
            // within the nonzero source dimensions for each of the eight TIFF orientations.
            let (sx, sy) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => (x, y),
            };
            let start = sy.checked_mul(source_stride).and_then(|n| n.checked_add(sx.checked_mul(bits)? / 8)).ok_or(bad)?;
            if bits == 1 {
                let value = data.get(start).ok_or(bad)? >> (7 - sx % 8) & 1;
                *row.get_mut(x / 8).ok_or(bad)? |= value << (7 - x % 8);
            } else {
                let channels = bits / 8;
                let source = data.get(start..start.checked_add(channels).ok_or(bad)?).ok_or(bad)?;
                let target = x.checked_mul(channels).ok_or(bad)?;
                row.get_mut(target..target.checked_add(channels).ok_or(bad)?).ok_or(bad)?.copy_from_slice(source);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_layouts_and_transient_limits_fail_before_allocating() {
        for (bytes, dimensions, bits, orientation, room) in [
            (&[][..], (0, 2), 8, 6, 100),
            (&[1][..], (3, 2), 8, 6, 100),
            (&[1][..], (u32::MAX, u32::MAX), 32, 6, usize::MAX),
            (&[1, 2, 3, 4, 5, 6][..], (3, 2), 8, 6, 5),
            // Transposing a 9x1 one-bit row needs nine destination bytes, not two.
            (&[0xff, 0xff][..], (9, 1), 1, 6, 8),
        ] {
            assert!(samples(bytes, dimensions, bits, orientation, room).is_err());
        }
        assert_eq!(samples(&[0xff, 0xff], (9, 1), 1, 6, 9).unwrap(), vec![0x80; 9]);
    }
}
