//! Print as image (Acrobat's Advanced Print Setup ▸ Print As Image): every page
//! sent to the printer as a picture at a chosen resolution, for printers that skip text,
//! shading or transparency. The pages are rendered the way the viewer shows them (reading
//! colours off, the print's own colours) with only what the print's content choice keeps, made
//! into an image-only document, and that document is imposed on the paper like any other. The
//! print is no longer searchable text.
//!
//! [`pictures`] renders only some pages, the same way, for the pages that must not reach the
//! Windows print component as they are (Type 3 text, #2684); the others stay vector.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Object, SaveOptions, Stream, write_incremental};
use pdfcraft_create::ImageResolution;
use pdfcraft_print::{Content, Raster};
use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderRequest};

use crate::export::{ExportSource, encode_png};
use crate::{Document, guard};

/// `doc`'s pages `pages` (0-based, each once, in the order given) as an image-only PDF, one
/// page per entry, the size of the source page as displayed, drawing what `content` prints.
/// Pages too large for the renderer at `raster.dpi` render at the largest resolution it allows.
pub fn rasterize(doc: &Document, pages: &[usize], raster: &Raster, content: Content) -> Result<Vec<u8>, String> {
    let dpi = raster.dpi.clamp(72.0, 600.0);
    let mut src = doc.export_source();
    src.bytes = printed(&src, content)?;
    let mut renderer = renderer(&src);
    let mut images: Vec<(String, Vec<u8>)> = Vec::with_capacity(pages.len());
    let mut used: Option<f64> = None;
    for &page in pages {
        let picture = render(&mut renderer, &src, page, dpi, raster.grayscale)?;
        // Every page of the image document shares one resolution, so its pages keep the
        // source's sizes; the lowest a page needed is used for all.
        used = Some(used.map_or(picture.dpi, |u: f64| u.min(picture.dpi)));
        images.push((format!("page-{}.png", page.saturating_add(1)), encode_png(picture.width, picture.height, &picture.rgba)?));
    }
    let dpi = used.unwrap_or(dpi);
    let doc = pdfcraft_create::from_images_with_resolution(&images, ImageResolution::Dpi(dpi)).map_err(|e| e.to_string())?;
    pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())
}

/// Pages `pages` (0-based) of `printed`, the print copy already reduced to what prints
/// (`pdfcraft_print::keep_printed`), rendered as [`rasterize`] renders them, each as an image
/// XObject of the page as displayed, keyed by page, for `pdfcraft_print::impose_with_pictures`.
/// Each picture is compressed as soon as it is drawn, so only one page's pixels are held at once.
pub fn pictures(doc: &Document, printed: &pdfcraft_cos::Document, pages: &[usize], raster: &Raster) -> Result<HashMap<usize, Stream>, String> {
    if pages.is_empty() {
        return Ok(HashMap::new());
    }
    let dpi = raster.dpi.clamp(72.0, 600.0);
    let mut src = doc.export_source();
    src.bytes = Arc::new(pdfcraft_cos::write_full(printed, &SaveOptions::default()).map_err(|e| e.to_string())?);
    let mut renderer = renderer(&src);
    let mut out = HashMap::with_capacity(pages.len());
    for &page in pages {
        if out.contains_key(&page) {
            continue;
        }
        let picture = render(&mut renderer, &src, page, dpi, raster.grayscale)?;
        // Opaque after `over_white`: the alpha goes, and grey pictures keep one channel.
        let channels = if raster.grayscale { 1 } else { 3 };
        let samples: Vec<u8> = picture.rgba.as_chunks::<4>().0.iter().flat_map(|px| px.iter().take(channels).copied()).collect();
        let mut image = Dict::new();
        image.set(b"Type".to_vec(), Object::name("XObject"));
        image.set(b"Subtype".to_vec(), Object::name("Image"));
        image.set(b"Width".to_vec(), Object::Int(i64::from(picture.width)));
        image.set(b"Height".to_vec(), Object::Int(i64::from(picture.height)));
        image.set(b"ColorSpace".to_vec(), Object::name(if raster.grayscale { "DeviceGray" } else { "DeviceRGB" }));
        image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        out.insert(page, Stream::flate(image, &samples));
    }
    Ok(out)
}

/// A page rendered for printing: opaque RGBA (over white, grey when asked) and the resolution
/// it was drawn at.
struct Picture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    dpi: f64,
}

/// `src`'s bytes reduced to what prints under `content` (`pdfcraft_print::keep_printed`).
fn printed(src: &ExportSource, content: Content) -> Result<Arc<Vec<u8>>, String> {
    let bytes = guard(|| -> Result<Vec<u8>, String> {
        let mut cos = pdfcraft_cos::Document::open_with_password(src.bytes.clone(), src.config.password.as_deref()).map_err(|e| e.to_string())?;
        pdfcraft_print::keep_printed(&mut cos, content);
        write_incremental(&cos, &SaveOptions::default()).map_err(|e| e.to_string())
    })??;
    Ok(Arc::new(bytes))
}

fn renderer(src: &ExportSource) -> PageRenderer {
    let mut config = src.config.clone();
    // The bytes hold only what prints, so the viewer's own comment hiding plays no part.
    config.hide_comments = false;
    PageRenderer::new(src.bytes.clone(), config)
}

/// Page `page` at `dpi`, or at the largest resolution the renderer allows for a page too large.
fn render(renderer: &mut PageRenderer, src: &ExportSource, page: usize, dpi: f64, grayscale: bool) -> Result<Picture, String> {
    let number = page.saturating_add(1);
    let (w, h) = src.sizes.get(page).copied().ok_or_else(|| format!("page {number} does not exist"))?;
    let scale = pdfcraft_render::effective_scale(w, h, (dpi / 72.0) as f32);
    let r = renderer.render(RenderRequest { page, scale, tag: PRINT_TAG, ..Default::default() });
    if let Some(e) = r.error {
        return Err(format!("page {number}: {e}"));
    }
    let pixels = (r.width as usize).checked_mul(r.height as usize).and_then(|n| n.checked_mul(4));
    if r.width == 0 || r.height == 0 || pixels != Some(r.rgba.len()) {
        return Err(format!("page {number} could not be drawn for printing"));
    }
    let mut rgba = r.rgba.to_vec();
    over_white(&mut rgba, grayscale);
    Ok(Picture { width: r.width, height: r.height, rgba, dpi: f64::from(scale) * 72.0 })
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
