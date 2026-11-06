//! Print as image (Acrobat's Advanced Print Setup ▸ Print As Image): every page
//! sent to the printer as a picture at a chosen resolution, for printers that skip text,
//! shading or transparency. The pages are rendered the way the viewer shows them (reading
//! colours off, the print's own colours) with only what the print's content choice keeps, made
//! into an image-only document, and that document is imposed on the paper like any other. The
//! print is no longer searchable text.

use pdfcraft_cos::{SaveOptions, write_incremental};
use pdfcraft_create::ImageResolution;
use pdfcraft_print::{Content, Raster};
use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderRequest};

use crate::export::encode_png;
use crate::{Document, guard};

/// `doc`'s pages `pages` (0-based, each once, in the order given) as an image-only PDF, one
/// page per entry, the size of the source page as displayed, drawing what `content` prints.
/// Pages too large for the renderer at `raster.dpi` render at the largest resolution it allows.
pub fn rasterize(doc: &Document, pages: &[usize], raster: &Raster, content: Content) -> Result<Vec<u8>, String> {
    let dpi = raster.dpi.clamp(72.0, 600.0);
    let src = doc.export_source();
    let mut config = src.config.clone();
    // The copy below holds only what prints, so the viewer's own comment hiding plays no part.
    config.hide_comments = false;
    config.hidden_comment_types = std::sync::Arc::new(Vec::new());
    let printed = guard(|| -> Result<Vec<u8>, String> {
        let mut cos = pdfcraft_cos::Document::open_with_password(src.bytes.clone(), config.password.as_deref()).map_err(|e| e.to_string())?;
        pdfcraft_print::keep_printed(&mut cos, content);
        write_incremental(&cos, &SaveOptions::default()).map_err(|e| e.to_string())
    })??;
    let mut renderer = PageRenderer::new(std::sync::Arc::new(printed), config);
    let mut images: Vec<(String, Vec<u8>)> = Vec::with_capacity(pages.len());
    let mut used: Option<f64> = None;
    for &page in pages {
        let (w, h) = src.sizes.get(page).copied().ok_or_else(|| format!("page {} does not exist", page + 1))?;
        let scale = pdfcraft_render::effective_scale(w, h, (dpi / 72.0) as f32);
        let page_dpi = f64::from(scale) * 72.0;
        // Every page of the image document shares one resolution, so its pages keep the
        // source's sizes; the lowest a page needed is used for all.
        used = Some(used.map_or(page_dpi, |u: f64| u.min(page_dpi)));
        let r = renderer.render(RenderRequest { page, scale, tag: PRINT_TAG, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        let mut rgba = r.rgba.to_vec();
        over_white(&mut rgba, raster.grayscale);
        images.push((format!("page-{}.png", page + 1), encode_png(r.width, r.height, &rgba)?));
    }
    let dpi = used.unwrap_or(dpi);
    let doc = pdfcraft_create::from_images_with_resolution(&images, ImageResolution::Dpi(dpi)).map_err(|e| e.to_string())?;
    pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())
}

/// Premultiplied RGBA composed over white paper (opaque), in grey when asked (Rec. 601 luma).
fn over_white(rgba: &mut [u8], grayscale: bool) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let gap = 255 - u32::from(px[3]);
        let (r, g, b) = ((u32::from(px[0]) + gap).min(255), (u32::from(px[1]) + gap).min(255), (u32::from(px[2]) + gap).min(255));
        let (r, g, b) = if grayscale {
            let y = ((299 * r + 587 * g + 114 * b) / 1000).min(255);
            (y, y, y)
        } else {
            (r, g, b)
        };
        px[0] = r as u8;
        px[1] = g as u8;
        px[2] = b as u8;
        px[3] = 255;
    }
}
