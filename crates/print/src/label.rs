//! Thermal label printers on Windows. Their drivers (Zebra's ZDesigner, measured) dither every grey
//! they are given, and Windows' PDF path resamples images even when a page is already a
//! black-and-white picture at the printer's resolution, so barcodes and seals came out as dot
//! patterns. Acrobat sends such printers pixels through GDI. So does this: every sheet of the
//! print-ready PDF is rendered at the printer's resolution, made black and white, and sent pixel
//! for pixel with the user's own driver settings (stock, darkness, cutter).

/// Premultiplied RGBA over white paper → RGB where every pixel is black or white: luma below half
/// is ink. Anti-aliased edges fall to one side or the other instead of reaching the driver as
/// grey for it to dither.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn black_and_white(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for px in rgba.as_chunks::<4>().0 {
        let gap = 255 - u32::from(px[3]);
        let (r, g, b) = ((u32::from(px[0]) + gap).min(255), (u32::from(px[1]) + gap).min(255), (u32::from(px[2]) + gap).min(255));
        let ink = (299 * r + 587 * g + 114 * b) / 1000 < 128;
        let v = if ink { 0 } else { 255 };
        out.extend_from_slice(&[v, v, v]);
    }
    out
}

/// The most bytes of black-and-white pictures one label job holds (a 4 × 6 in label at 300 dpi
/// is under 7 MB).
#[cfg(windows)]
const MAX_PICTURE_BYTES: usize = 256 * 1024 * 1024;

/// `job`'s printer when it is a label printer (the job's, else the default).
#[cfg(windows)]
pub(crate) fn label_printer(job: &crate::spool::Job) -> Option<String> {
    let name = job.printer.clone().or_else(|| craft_print::printers().into_iter().find(|p| p.default).map(|p| p.name))?;
    craft_print::is_label_printer(&name).then_some(name)
}

/// Prints the print-ready `pdf` on the label printer `printer` as black-and-white pictures at its
/// resolution, `progress(rendered, sheets)` as each sheet is ready.
#[cfg(windows)]
pub(crate) fn submit(pdf: &[u8], job: &crate::spool::Job, printer: &str, progress: &mut dyn FnMut(usize, usize)) -> Result<String, String> {
    use craft_print::rgb;
    use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderConfig, RenderRequest};
    let settings = rgb::user_settings(printer).map_err(|e| e.to_string())?;
    let page = rgb::page(printer, Some(&settings)).map_err(|e| e.to_string())?;
    let dpi = page.dpi.0.min(page.dpi.1).max(1);
    let mut renderer = PageRenderer::new(std::sync::Arc::new(pdf.to_vec()), RenderConfig::default());
    let sheets = renderer.page_count();
    if sheets == 0 || sheets > rgb::MAX_PAGES {
        return Err(format!("the print has {sheets} sheets; a label printer job takes 1 to {}", rgb::MAX_PAGES));
    }
    let scale = dpi as f32 / 72.0;
    let mut pictures: Vec<(u32, u32, Vec<u8>)> = Vec::with_capacity(sheets);
    let mut bytes = 0usize;
    for sheet in 0..sheets {
        let r = renderer.render(RenderRequest { page: sheet, scale, tag: PRINT_TAG, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("sheet {}: {e}", sheet + 1));
        }
        let rgb = black_and_white(&r.rgba);
        bytes = bytes.saturating_add(rgb.len());
        if bytes > MAX_PICTURE_BYTES {
            return Err("the print is too large to send to a label printer as pictures".into());
        }
        pictures.push((r.width, r.height, rgb));
        progress(sheet + 1, sheets);
    }
    let images: Vec<rgb::RgbImage<'_>> = pictures.iter().map(|(w, h, data)| rgb::RgbImage { width: *w, height: *h, data }).collect();
    // Each picture covers exactly its own device pixels, so the transport copies it as it is.
    let per_point = f64::from(dpi) / 72.0;
    let pages: Vec<rgb::PagePicture<'_, '_>> =
        images.iter().map(|image| (image, (0.0, 0.0, f64::from(image.width) / per_point, f64::from(image.height) / per_point))).collect();
    let rgb_job = rgb::Job {
        printer,
        settings: Some(&settings),
        title: &job.title,
        output_file: None,
        copies: job.copies.max(1),
        color_policy: rgb::ColorPolicy::Unmanaged,
        primitives: &[],
    };
    rgb::print_pages(&rgb_job, &pages).map_err(|e| e.to_string())?;
    Ok(format!("sent {sheets} sheet(s) to the label printer in black and white at {dpi} dpi"))
}
