//! Thermal label printers on Windows. Their drivers (Zebra's ZDesigner, measured) dither every grey
//! they are given, and Windows' PDF path resamples images even when a page is already a
//! black-and-white picture at the printer's resolution, so barcodes and seals came out as dot
//! patterns. Acrobat sends such printers pixels through GDI. So does this: every sheet of the
//! print-ready PDF is rendered at the printer's resolution, made black and white (grey areas
//! dithered), and sent pixel for pixel with the user's own driver settings (stock, darkness,
//! cutter), once every sheet is known to be that stock.

/// Premultiplied RGBA over white paper, `width` pixels a row → RGB where every pixel is black or
/// white. A pixel in a flat grey area (it and its four neighbours all mid-grey: a fill, a logo, a
/// photo) is dithered with a fixed 8 × 8 pattern, so the area keeps its shade. Any other pixel is
/// ink when its luma is below half: anti-aliased edges of text, barcodes and QR codes fall to one
/// side or the other instead of reaching the driver as grey for it to dither.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn black_and_white(rgba: &[u8], width: usize) -> Vec<u8> {
    const BAYER: [[u8; 8]; 8] = [
        [0, 32, 8, 40, 2, 34, 10, 42],
        [48, 16, 56, 24, 50, 18, 58, 26],
        [12, 44, 4, 36, 14, 46, 6, 38],
        [60, 28, 52, 20, 62, 30, 54, 22],
        [3, 35, 11, 43, 1, 33, 9, 41],
        [51, 19, 59, 27, 49, 17, 57, 25],
        [15, 47, 7, 39, 13, 45, 5, 37],
        [63, 31, 55, 23, 61, 29, 53, 21],
    ];
    let pixels = rgba.as_chunks::<4>().0;
    let luma: Vec<u8> = pixels
        .iter()
        .map(|px| {
            let gap = 255 - u32::from(px[3]);
            let (r, g, b) = ((u32::from(px[0]) + gap).min(255), (u32::from(px[1]) + gap).min(255), (u32::from(px[2]) + gap).min(255));
            ((299 * r + 587 * g + 114 * b) / 1000) as u8
        })
        .collect();
    let width = width.max(1);
    let mid = |i: usize| luma.get(i).is_some_and(|l| (32..=223).contains(l));
    let mut out = Vec::with_capacity(luma.len() * 3);
    for (i, &l) in luma.iter().enumerate() {
        let (x, y) = (i % width, i / width);
        let flat = mid(i) && x > 0 && x + 1 < width && i >= width && mid(i - 1) && mid(i + 1) && mid(i - width) && mid(i + width);
        let threshold = if flat { BAYER[y % 8][x % 8] as u32 * 4 + 2 } else { 128 };
        let v = if u32::from(l) < threshold { 0 } else { 255 };
        out.extend_from_slice(&[v, v, v]);
    }
    out
}

/// A label job's sheets must be the stock in the user's settings: pictures go to the driver pixel
/// for pixel, so a sheet larger than the stock would be cut off and a smaller one would waste
/// the rest of the label. Sides may differ by 5 pt, as drivers round metric stock; a sheet fed
/// the other way round is refused too, since the pictures are not turned.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn sheet_fits_stock(printer: &str, sheet: (f64, f64), stock: (f64, f64)) -> Result<(), String> {
    let same = (sheet.0 - stock.0).abs() <= 5.0 && (sheet.1 - stock.1).abs() <= 5.0;
    if same {
        return Ok(());
    }
    let inches = |(w, h): (f64, f64)| format!("{:.2} × {:.2} in", w / 72.0, h / 72.0);
    Err(format!(
        "{printer} is set to {} labels in its Printing Preferences, and this print's sheets are {}; nothing was sent. Choose the label's paper size, or set the label size in the printer's Printing Preferences",
        inches(stock),
        inches(sheet)
    ))
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
    let to_points = |pixels: u32| f64::from(pixels) * 72.0 / f64::from(dpi);
    let mut pictures: Vec<(u32, u32, Vec<u8>)> = Vec::with_capacity(sheets);
    let mut bytes = 0usize;
    for sheet in 0..sheets {
        let r = renderer.render(RenderRequest { page: sheet, scale, tag: PRINT_TAG, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("sheet {}: {e}", sheet + 1));
        }
        sheet_fits_stock(printer, (to_points(r.width), to_points(r.height)), page.paper)?;
        let rgb = black_and_white(&r.rgba, r.width as usize);
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
