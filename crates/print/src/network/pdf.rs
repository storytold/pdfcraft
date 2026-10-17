//! PdfCraft's [`DocumentConverter`]: a print-ready PDF becomes PWG raster, for a printer that lists
//! raster but not PDF.
//!
//! The registry decides *whether* to convert (the printer does not take the document's format) and
//! *at what resolution* (the job's own, else the printer's listed one nearest 300 dpi); this is the
//! *how*, with PdfCraft's own renderer and craft-print's neutral [`pwg`] encoder. It makes nothing
//! but PWG raster, and starts from nothing but PDF; any other format the printer does not list is
//! [`ErrorKind::UnsupportedFormat`] from the registry.

use std::sync::Arc;

use craft_print::{ConvertContext, Document, DocumentConverter, DocumentFormat, Duplex, ErrorKind, OwnedDocument, PrintError, pwg};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, effective_scale};

fn failed(why: impl Into<String>) -> PrintError {
    PrintError::new(ErrorKind::Failed, why)
}

/// PDF to PWG raster. Stateless: register one with [`super::register_pdf_converter`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PdfToRaster;

impl DocumentConverter for PdfToRaster {
    fn targets(&self, from: DocumentFormat) -> Vec<DocumentFormat> {
        if from == DocumentFormat::Pdf { vec![DocumentFormat::PwgRaster] } else { Vec::new() }
    }

    fn convert(&self, document: Document<'_>, target: DocumentFormat, context: &ConvertContext) -> Result<OwnedDocument, PrintError> {
        if document.format != DocumentFormat::Pdf || target != DocumentFormat::PwgRaster {
            return Err(PrintError::new(
                ErrorKind::UnsupportedFormat,
                format!("this converter makes PWG raster from PDF, not {} from {}", target.mime(), document.format.mime()),
            ));
        }
        let bytes = Arc::new(document.bytes.to_vec());
        let sizes = page_sizes(&bytes)?;
        let (dx, dy) = (context.resolution.x_dpi.max(1), context.resolution.y_dpi.max(1));
        let gray = !context.colour;
        let mut renderer = PageRenderer::new(Arc::clone(&bytes), RenderConfig::default());
        if renderer.page_count() == 0 {
            return Err(failed("the document has no pages that can be rendered"));
        }
        let mut pages = Vec::with_capacity(sizes.len());
        for (index, &(w, h)) in sizes.iter().enumerate() {
            if context.cancel.is_cancelled() {
                return Err(PrintError::new(ErrorKind::Cancelled, "the print job was cancelled while the document was rendered"));
            }
            pages.push(render_page(&mut renderer, index, (w, h), (dx, dy), gray)?);
        }
        let duplex = match context.duplex {
            None | Some(Duplex::Off) => pwg::Duplex::Off,
            Some(Duplex::LongEdge) => pwg::Duplex::LongEdge,
            Some(Duplex::ShortEdge) => pwg::Duplex::ShortEdge,
        };
        let raster = pwg::encode(&pages, &pwg::PwgOptions { dpi: (dx, dy), duplex }).map_err(|e| failed(e.to_string()))?;
        Ok(OwnedDocument::new(DocumentFormat::PwgRaster, raster))
    }
}

/// The displayed size of each page in points.
fn page_sizes(bytes: &Arc<Vec<u8>>) -> Result<Vec<(f64, f64)>, PrintError> {
    let doc = pdfcraft_cos::Document::open(Arc::clone(bytes)).map_err(|e| failed(format!("the document could not be read: {e}")))?;
    let sizes: Vec<(f64, f64)> = pdfcraft_model::pages(&doc).iter().map(|p| p.display_size(&doc)).collect();
    if sizes.is_empty() { Err(failed("the document has no pages")) } else { Ok(sizes) }
}

/// One page as a raster page at `dpi` (the renderer takes one scale, so the finer axis is rendered
/// and the other is sampled down).
fn render_page(
    renderer: &mut PageRenderer,
    index: usize,
    (w, h): (f64, f64),
    (dx, dy): (u32, u32),
    gray: bool,
) -> Result<pwg::RasterPage, PrintError> {
    let number = index + 1;
    let top = dx.max(dy);
    let scale = (f64::from(top) / 72.0) as f32;
    // The renderer shrinks what it is asked for beyond its own limits; a raster header that says
    // otherwise would print at the wrong size, so a page that does not fit is refused.
    let (wf, hf) = (w as f32, h as f32);
    if !(wf.is_finite() && hf.is_finite()) || effective_scale(wf, hf, scale) < scale * 0.999 {
        return Err(failed(format!("page {number} is too large to render at {top} dpi: lower the resolution")));
    }
    let out = renderer.render(RenderRequest { page: index, kind: RequestKind::Pixels, tile: None, scale, preview: false, tag: 0 });
    if let Some(e) = out.error {
        return Err(failed(format!("page {number} could not be rendered: {e}")));
    }
    let bpp = if gray { 1 } else { 3 };
    let flat = flatten(&out.rgba, gray);
    let (rw, rh) = (out.width, out.height);
    if rw == 0 || rh == 0 || flat.len() != rw as usize * rh as usize * bpp {
        return Err(failed(format!("page {number} was rendered with an unexpected size")));
    }
    if dx == dy {
        return Ok(pwg::RasterPage { width: rw, height: rh, gray, pixels: flat });
    }
    let target = |points: f64, dpi: u32| ((points * f64::from(dpi) / 72.0).ceil() as u32).max(1);
    let (tw, th) = (target(w, dx), target(h, dy));
    Ok(pwg::RasterPage { width: tw, height: th, gray, pixels: resample(&flat, bpp, (rw, rh), (tw, th)) })
}

/// Nearest-neighbour sampling of an interleaved 8-bit picture.
fn resample(pixels: &[u8], bpp: usize, (sw, sh): (u32, u32), (tw, th): (u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity(tw as usize * th as usize * bpp);
    for y in 0..th {
        let sy = (u64::from(y) * u64::from(sh) / u64::from(th)).min(u64::from(sh) - 1) as usize;
        for x in 0..tw {
            let sx = (u64::from(x) * u64::from(sw) / u64::from(tw)).min(u64::from(sw) - 1) as usize;
            let at = (sy * sw as usize + sx) * bpp;
            out.extend_from_slice(pixels.get(at..at + bpp).unwrap_or(&[]));
        }
    }
    out
}

/// Premultiplied RGBA over white, as 8-bit grey (luma) or RGB.
fn flatten(rgba: &[u8], gray: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * if gray { 1 } else { 3 });
    let (pixels, _) = rgba.as_chunks::<4>();
    for px in pixels {
        let a = u16::from(px[3]);
        let over = |c: u8| (u16::from(c) + (255 - a)).min(255) as u8;
        let (r, g, b) = (over(px[0]), over(px[1]), over(px[2]));
        if gray {
            out.push(((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8);
        } else {
            out.extend_from_slice(&[r, g, b]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattening_composites_over_white() {
        assert_eq!(flatten(&[0, 0, 0, 255, 0, 0, 0, 0], false), [0, 0, 0, 255, 255, 255]);
        assert_eq!(flatten(&[255, 255, 255, 255], true), [255]);
        assert_eq!(flatten(&[1, 2, 3], true), Vec::<u8>::new());
    }

    #[test]
    fn resampling_keeps_rows_and_columns_in_place() {
        // 2x2 grey picture to 4x1: the columns double, the rows are sampled down.
        assert_eq!(resample(&[1, 2, 3, 4], 1, (2, 2), (4, 1)), [1, 1, 2, 2]);
        assert_eq!(resample(&[1, 2, 3, 4], 1, (2, 2), (1, 2)), [1, 3]);
    }

    #[test]
    fn it_converts_pdf_and_nothing_else() {
        let c = PdfToRaster;
        assert_eq!(c.targets(DocumentFormat::Pdf), [DocumentFormat::PwgRaster]);
        assert!(c.targets(DocumentFormat::Jpeg).is_empty());
        let ctx = ConvertContext::new(craft_print::Resolution::square(72), true);
        let e = c.convert(Document::new(DocumentFormat::Jpeg, &[1]), DocumentFormat::PwgRaster, &ctx).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::UnsupportedFormat);
        // Bytes that are not a PDF fail cleanly, as a failure and not a panic.
        let e = c.convert(Document::new(DocumentFormat::Pdf, b"not a pdf"), DocumentFormat::PwgRaster, &ctx).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Failed);
    }
}
