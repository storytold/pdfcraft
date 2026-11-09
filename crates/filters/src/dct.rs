//! JPEG (`DCTDecode`) samples as a page reads them, for code that changes pixels and stores them
//! again (redaction). The caller pays for the decode first (`pdfcraft_cos::ImageBudget::pay_jpeg`).

use std::panic::{AssertUnwindSafe, catch_unwind};

use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};

/// The 8-bit samples of JPEG `data` if its header says it is `w` × `h` with `components` colour
/// components: grey, RGB (YCbCr converted, as a PDF reader converts it), or CMYK as stored, not
/// inverted the way a decoder converting to RGB inverts it (a page undoes Adobe's inversion with
/// `/Decode`), with YCCK converted to CMYK. `None` for a JPEG that doesn't decode, or decodes to
/// another size or component count; the decoder panicking is `None` too.
pub fn decode_dct(data: &[u8], w: usize, h: usize, components: usize) -> Option<Vec<u8>> {
    // The header has to give the size the caller checked, so nothing larger is allocated.
    let options = DecoderOptions::default().set_strict_mode(false).set_max_width(w).set_max_height(h);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    catch_unwind(AssertUnwindSafe(|| dec.decode_headers())).ok()?.ok()?;
    let input = dec.input_colorspace()?;
    if dec.dimensions()? != (w, h) || input.num_components() != components {
        return None;
    }
    let out = match input {
        ColorSpace::Luma => ColorSpace::Luma,
        ColorSpace::YCbCr | ColorSpace::RGB => ColorSpace::RGB,
        ColorSpace::CMYK | ColorSpace::YCCK => input,
        _ => return None,
    };
    dec.set_options(options.jpeg_set_out_colorspace(out));
    let mut px = catch_unwind(AssertUnwindSafe(|| dec.decode())).ok()?.ok()?;
    if px.len() != w.checked_mul(h)?.checked_mul(components)? {
        return None;
    }
    if input == ColorSpace::YCCK {
        for p in px.as_chunks_mut::<4>().0 {
            let (y, cb, cr) = (f32::from(p[0]), f32::from(p[1]) - 128.0, f32::from(p[2]) - 128.0);
            let ink = |rgb: f32| (255.0 - rgb).round().clamp(0.0, 255.0) as u8;
            p[0] = ink(y + 1.402 * cr);
            p[1] = ink(y - 0.344_136 * cb - 0.714_136 * cr);
            p[2] = ink(y + 1.772 * cb);
        }
    }
    Some(px)
}
