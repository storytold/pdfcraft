//! Pictures placed on pages: added images, and the pictures of watermarks and backgrounds.

use std::sync::Arc;

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
