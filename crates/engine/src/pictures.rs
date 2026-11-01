//! Pictures placed on pages: added images, and the pictures of watermarks and backgrounds.

use std::sync::Arc;

use image::ImageDecoder;

use crate::EditError;

/// A picture for a watermark or background (Acrobat: Source ▸ File): an image, or `page`
/// (0-based) of a PDF.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkFile {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub page: usize,
}

/// Embed an image that is placed on a page, turned the way its EXIF Orientation says.
pub(crate) fn embed_image(
    doc: &mut pdfcraft_cos::Document,
    name: &str,
    bytes: &[u8],
) -> Result<(pdfcraft_cos::ObjRef, (f64, f64)), pdfcraft_create::CreateError> {
    pdfcraft_create::image_xobject(doc, name, bytes)
}

/// The largest box of `w`:`h` shape inside `rect`, centred in it: a stamp's picture keeps its
/// shape in a box of any other shape. A box that is already that shape (to within rounding) is
/// kept exactly, so callers that size it themselves get their box back. An empty or non-finite
/// box, or a picture with no size, is returned as it is, for the annotation's own check to refuse.
pub(crate) fn fit_picture(rect: [f64; 4], w: f64, h: f64) -> [f64; 4] {
    if !rect.iter().all(|v| v.is_finite()) {
        return rect;
    }
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    let (bw, bh) = (rect[2] - rect[0], rect[3] - rect[1]);
    let usable = |v: f64| v.is_finite() && v > 0.0;
    if !(usable(bw) && usable(bh) && usable(w) && usable(h)) {
        return rect;
    }
    let k = (bw / w).min(bh / h);
    let (fw, fh) = (w * k, h * k);
    if (fw - bw).abs() <= bw * 1e-9 && (fh - bh).abs() <= bh * 1e-9 {
        return rect;
    }
    let (cx, cy) = (rect[0] + bw / 2.0, rect[1] + bh / 2.0);
    [cx - fw / 2.0, cy - fh / 2.0, cx + fw / 2.0, cy + fh / 2.0]
}

/// Bring a mark's picture into `doc`: a PDF page as a form XObject, or an image.
pub(crate) fn mark_source(doc: &mut pdfcraft_cos::Document, f: &MarkFile) -> Result<pdfcraft_edit::MarkSource, EditError> {
    let head = &f.bytes[..f.bytes.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        let src = pdfcraft_cos::Document::open(f.bytes.clone()).map_err(|e| EditError::Source(format!("{}: {e}", f.name)))?;
        let (xobject, size) = pdfcraft_organize::page_as_form(doc, &src, f.page)?;
        Ok(pdfcraft_edit::MarkSource { xobject, size, image: false })
    } else {
        let (xobject, size) = embed_image(doc, &f.name, &f.bytes)?;
        Ok(pdfcraft_edit::MarkSource { xobject, size, image: true })
    }
}

/// The dimensions of an image from its bytes (JPEG, PNG, TIFF), without decoding the full raster.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let format = image::guess_format(bytes).ok()?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64_000_000 * 4);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let (mut w, mut h) = image::ImageDecoder::dimensions(&decoder);
    if w == 0 || h == 0 {
        return None;
    }
    if format == image::ImageFormat::Jpeg
        && u64::from(w) * u64::from(h) <= (64 << 20)
        && let Ok(orientation) = decoder.orientation()
        && matches!(
            orientation,
            image::metadata::Orientation::Rotate90
                | image::metadata::Orientation::Rotate270
                | image::metadata::Orientation::Rotate90FlipH
                | image::metadata::Orientation::Rotate270FlipH
        )
    {
        std::mem::swap(&mut w, &mut h);
    }
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone_jpeg(w: u32, h: u32, orientation: u8) -> Vec<u8> {
        use image::codecs::jpeg::JpegEncoder;
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new(&mut jpeg);
        encoder.encode_image(&image::RgbImage::from_pixel(w, h, image::Rgb([20, 20, 20]))).expect("valid jpeg");
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
    fn jpeg_exif_orientation_swaps_dimensions_for_quarter_turns() {
        let turned_6 = phone_jpeg(60, 20, 6);
        assert_eq!(image_dimensions(&turned_6), Some((20, 60)));

        let turned_8 = phone_jpeg(60, 20, 8);
        assert_eq!(image_dimensions(&turned_8), Some((20, 60)));

        let upright_1 = phone_jpeg(60, 20, 1);
        assert_eq!(image_dimensions(&upright_1), Some((60, 20)));
    }

    #[test]
    fn invalid_inputs_return_none() {
        assert_eq!(image_dimensions(&[]), None);
        assert_eq!(image_dimensions(b"not an image"), None);
    }
}
