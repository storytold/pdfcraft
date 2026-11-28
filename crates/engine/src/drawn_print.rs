//! The drawn Windows print routes (#8529): PdfCraft draws every sheet of the print-ready PDF
//! itself and sends it through GDI, as pixels at the printer's resolution
//! ([`Route::Image`]) or as vector calls ([`Route::Vector`]), for systems whose own PDF renderer
//! prints blurred. A sheet the vector route can't draw (transparency, blend modes, soft masks,
//! patterns, overprint) is drawn as pixels.

use std::sync::Arc;

use pdfcraft_print::spool::{self, DrawError, DrawnSheet, Job, Route, SheetPainter, rgb};
use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderConfig, RenderRequest, Tile, device_pixels};

use crate::print_raster::over_white;

/// The finest resolution a sheet is drawn at as pixels.
pub const MAX_DRAWN_DPI: u32 = 600;
/// Rows of device pixels rendered at once.
const BAND_ROWS: u32 = 512;
/// The widest band the renderer draws at once.
const BAND_COLUMNS: u32 = 4096;

/// Sends the print-ready `pdf` for `job` along `route`, told `(sent, total)` as it goes. A drawn
/// route prints to `output_file` instead of the device when given (drivers that write files, and
/// tests); the standard route always prints to the device.
pub fn submit(pdf: &[u8], job: &Job, route: Route, output_file: Option<&str>, progress: &mut dyn FnMut(usize, usize)) -> Result<String, String> {
    let sent = match route {
        Route::Standard => spool::submit_with_progress(pdf, job, progress),
        Route::Image => spool::submit_drawn(pdf, job, &mut ImagePainter::new(job.grayscale), output_file, progress),
        Route::Vector => {
            let mut painter = VectorPainter { pictures: None, image: ImagePainter::new(job.grayscale), as_image: Default::default() };
            let sent = spool::submit_drawn(pdf, job, &mut painter, output_file, progress);
            sent.map(|s| match painter.as_image.len() {
                0 => s,
                n => format!("{s}; {n} sheet(s) the shapes route could not draw were drawn as pixels"),
            })
        }
    };
    sent.map_err(|e| e.to_string())
}

fn draw_error(e: impl std::fmt::Display) -> DrawError {
    DrawError::Spool(e.to_string())
}

/// Draws sheets as pixels at the printer's resolution, up to [`MAX_DRAWN_DPI`].
struct ImagePainter {
    renderer: Option<PageRenderer>,
    grayscale: bool,
}

impl ImagePainter {
    fn new(grayscale: bool) -> Self {
        Self { renderer: None, grayscale }
    }
}

impl SheetPainter for ImagePainter {
    fn open(&mut self, pdf: &[u8]) -> Result<(), DrawError> {
        // The print-ready PDF holds what prints, comments flattened into it.
        let config = RenderConfig { for_print: true, ..RenderConfig::default() };
        self.renderer = Some(PageRenderer::new(Arc::new(pdf.to_vec()), config));
        Ok(())
    }

    fn paint(&mut self, sheet: &DrawnSheet, document: &mut rgb::Document) -> Result<(), DrawError> {
        let renderer = self.renderer.as_mut().ok_or_else(|| draw_error("the print-ready PDF was not opened"))?;
        let page = sheet.page;
        let dpi = page.dpi.0.min(page.dpi.1).clamp(72, MAX_DRAWN_DPI);
        // At the device's own resolution bands are copied pixel for pixel; otherwise GDI scales them.
        let exact = page.dpi == (dpi, dpi);
        let scale = dpi as f32 / 72.0;
        let (width, height) = (device_pixels(page.paper.0 as f32, scale), device_pixels(page.paper.1 as f32, scale));
        // Device pixels count from the printable area's corner.
        let corner = ((page.printable.0 * f64::from(dpi) / 72.0).round() as i32, (page.printable.1 * f64::from(dpi) / 72.0).round() as i32);
        let mut rgb_band = Vec::new();
        for y in (0..height).step_by(BAND_ROWS as usize) {
            for x in (0..width).step_by(BAND_COLUMNS as usize) {
                let tile = Tile { x, y, w: BAND_COLUMNS.min(width - x), h: BAND_ROWS.min(height - y) };
                let r = renderer.render(RenderRequest { page: sheet.index, scale, tile: Some(tile), tag: PRINT_TAG, ..Default::default() });
                if let Some(e) = r.error {
                    return Err(draw_error(format!("sheet {}: {e}", sheet.index + 1)));
                }
                let mut rgba = r.rgba.to_vec();
                over_white(&mut rgba, self.grayscale);
                rgb_band.clear();
                rgb_band.extend(rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]));
                let image = rgb::RgbImage { width: r.width, height: r.height, data: &rgb_band };
                if exact {
                    let at = (
                        i32::try_from(x).unwrap_or(i32::MAX).saturating_sub(corner.0),
                        i32::try_from(y).unwrap_or(i32::MAX).saturating_sub(corner.1),
                    );
                    document.draw_pixels(&image, at).map_err(draw_error)?;
                } else {
                    let s = f64::from(scale);
                    let rect = (f64::from(x) / s, f64::from(y) / s, f64::from(r.width) / s, f64::from(r.height) / s);
                    document.draw_image(&image, rect).map_err(draw_error)?;
                }
            }
        }
        Ok(())
    }
}

/// Draws sheets as vector calls, and those that need transparency as pixels.
struct VectorPainter {
    pictures: Option<craft_pdf::emf::Pictures>,
    image: ImagePainter,
    /// Sheets drawn as pixels, so later copies of them skip the vector attempt.
    as_image: std::collections::BTreeSet<usize>,
}

impl SheetPainter for VectorPainter {
    fn open(&mut self, pdf: &[u8]) -> Result<(), DrawError> {
        self.pictures = Some(craft_pdf::emf::Pictures::open(pdf, craft_pdf::emf::Limits::default()).map_err(draw_error)?);
        self.image.open(pdf)
    }

    fn paint(&mut self, sheet: &DrawnSheet, document: &mut rgb::Document) -> Result<(), DrawError> {
        let pictures = self.pictures.as_ref().ok_or_else(|| draw_error("the print-ready PDF was not opened"))?;
        if self.as_image.contains(&sheet.index) {
            return self.image.paint(sheet, document);
        }
        match pictures.page(sheet.index, "PdfCraft", "sheet") {
            Ok(emf) => document.play_metafile(&emf.bytes, (0.0, 0.0, sheet.page.paper.0, sheet.page.paper.1)).map_err(draw_error),
            Err(craft_pdf::emf::Error::NeedsRaster(_) | craft_pdf::emf::Error::OutputLimit) => {
                self.as_image.insert(sheet.index);
                self.image.paint(sheet, document)
            }
            Err(e) => Err(draw_error(format!("sheet {}: {e}", sheet.index + 1))),
        }
    }
}

// The routes print only on Windows, through its in-box PDF printer.
#[cfg(test)]
#[cfg(windows)]
mod tests;
