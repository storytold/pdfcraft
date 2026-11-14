//! Printing from the browser (docs/gaps.md gap 4). A web page can't reach a spooler, so the
//! print-ready PDF is drawn sheet by sheet with PdfCraft's own renderer, one sheet a frame (the
//! web build has no threads), and the pictures go to the browser's print dialog in a hidden
//! iframe, one sheet per printed page. That prints the same in every browser, with or without a
//! PDF viewer of its own, and matches the preview. Copies, two-sided printing and colour are
//! chosen in the browser's dialog.
//!
//! The planning, the print page and the job are plain Rust and tested on every target; only
//! handing the pictures to the browser (`blob_url`, `print_in_iframe`) is wasm-only.

// Outside the browser only the tests drive the job; the app itself prints through the spooler.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use pdfcraft_engine::export::{ExportSource, Exporter};

/// The printer the web build lists: the browser's print dialog, which reaches the printers the
/// operating system knows. An id, never shown; the dialog shows [`browser_printer_label`].
pub(crate) const BROWSER_PRINTER: &str = "\u{1}browser";

/// The browser printer's name in the UI language.
pub(crate) fn browser_printer_label() -> &'static str {
    tl!("Browser's print dialog")
}

/// The most sheets a browser print takes: each is a picture held by the page until the dialog
/// closes. Longer jobs are saved as the print-ready PDF and printed from a PDF viewer.
pub(crate) const MAX_SHEETS: usize = 1000;
/// The most pixels one sheet is drawn with (the Windows spooler's limit too): about A0 at 200 dpi.
pub(crate) const MAX_SHEET_PIXELS: f64 = 64.0e6;
/// The most PNG bytes one job may hold before it stops, so a long job of busy pages can't exhaust
/// the tab's memory.
pub(crate) const MAX_JOB_BYTES: u64 = 1 << 30;
/// The lowest resolution a sheet is drawn at, whatever its size.
const MIN_DPI: f64 = 72.0;

/// The resolution each sheet (`(width, height)` in points) is drawn at: 300 dpi for up to 100
/// sheets, 200 up to 300 and 150 up to [`MAX_SHEETS`], lowered for a sheet that would exceed
/// [`MAX_SHEET_PIXELS`]. Refused for no sheets, too many, or a sheet without a usable size.
pub(crate) fn plan_dpi(sheets: &[(f64, f64)]) -> Result<Vec<f64>, String> {
    let base: f64 = match sheets.len() {
        0 => return Err("there are no sheets to print".into()),
        1..=100 => 300.0,
        101..=300 => 200.0,
        n if n <= MAX_SHEETS => 150.0,
        n => {
            return Err(format!(
                "{n} sheets are too many to print from the browser (at most {MAX_SHEETS}); save the print-ready PDF and print it from a PDF viewer"
            ));
        }
    };
    sheets
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| {
            if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 {
                return Err(format!("sheet {} has no usable size", i + 1));
            }
            let square_inches = (w / 72.0) * (h / 72.0);
            let cap = (MAX_SHEET_PIXELS / square_inches).sqrt();
            let dpi = if cap.is_finite() { base.min(cap) } else { base };
            Ok(dpi.max(MIN_DPI))
        })
        .collect()
}

/// A length in points for CSS, rounded to 1/1000 pt.
fn pt(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    format!("{r}pt")
}

/// `"` `&` `<` `>` escaped for an HTML attribute value.
fn attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Whether two sizes in points are the same within half a point.
fn same_size(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() <= 0.5 && (a.1 - b.1).abs() <= 0.5
}

/// The page the browser prints: each sheet's picture (`urls`, in order) filling a printed page
/// with no margin. The paper is the first sheet's size (the default `@page`). A sheet of that
/// size turned sideways (a landscape sheet among portrait ones) is turned 90° counter-clockwise
/// onto it, as a printer driver does, because browsers print every page at the default size
/// whatever named pages ask for (Chromium does). Any other size gets a named `@page` of its own,
/// which browsers that support it honour. Only the last sheet has no page break after it, so no
/// blank page follows.
pub(crate) fn print_document_html(sheets: &[(f64, f64)], urls: &[String]) -> String {
    let paper = sheets.first().copied().unwrap_or((612.0, 792.0));
    let (pw, ph) = (pt(paper.0), pt(paper.1));
    let mut css = format!(
        "html,body{{margin:0;padding:0;background:#fff}}@page{{size:{pw} {ph};margin:0}}\
         div{{position:relative;overflow:hidden;margin:0;padding:0;width:{pw};height:{ph};break-after:page;page-break-after:always}}\
         div:last-child{{break-after:auto;page-break-after:auto}}img{{display:block;position:absolute;left:0;top:0;margin:0;padding:0;border:0}}\
         .turned img{{width:{ph};height:{pw};transform-origin:0 0;transform:translate(0,{ph}) rotate(-90deg)}}"
    );
    // Sizes other than the paper's, each a named page.
    let mut others: Vec<(f64, f64)> = Vec::new();
    let mut html_sheets = String::new();
    for (&size, url) in sheets.iter().zip(urls) {
        let src = attr(url);
        if same_size(size, paper) {
            html_sheets.push_str(&format!("<div><img alt=\"\" style=\"width:{pw};height:{ph}\" src=\"{src}\"></div>"));
        } else if same_size((size.1, size.0), paper) {
            html_sheets.push_str(&format!("<div class=\"turned\"><img alt=\"\" src=\"{src}\"></div>"));
        } else {
            let k = match others.iter().position(|o| same_size(*o, size)) {
                Some(k) => k,
                None => {
                    let (w, h) = (pt(size.0), pt(size.1));
                    css.push_str(&format!("@page s{k}{{size:{w} {h};margin:0}}div.s{k}{{page:s{k};width:{w};height:{h}}}", k = others.len()));
                    others.push(size);
                    others.len() - 1
                }
            };
            let (w, h) = (pt(size.0), pt(size.1));
            html_sheets.push_str(&format!("<div class=\"s{k}\"><img alt=\"\" style=\"width:{w};height:{h}\" src=\"{src}\"></div>"));
        }
    }
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>PdfCraft</title><style>{css}</style></head><body>{html_sheets}</body></html>")
}

/// A browser print being prepared: the print-ready PDF's sheets drawn one [`WebPrintJob::step`]
/// at a time and handed to a sink (on the web, [`blob_url`]), which answers with the picture's URL.
pub(crate) struct WebPrintJob {
    exporter: Exporter,
    /// The print-ready PDF, kept for the download fallback when the browser won't print.
    pub pdf: std::sync::Arc<Vec<u8>>,
    pub name: String,
    sheets: Vec<(f64, f64)>,
    dpi: Vec<f64>,
    urls: Vec<String>,
    bytes: u64,
    /// [`MAX_JOB_BYTES`]; lower in tests.
    max_bytes: u64,
}

impl WebPrintJob {
    /// A job for `pdf`, whose pages are `sheets` (in points, from `print::layout`).
    pub fn new(pdf: Vec<u8>, sheets: Vec<(f64, f64)>, name: String) -> Result<Self, String> {
        let dpi = plan_dpi(&sheets)?;
        let pdf = std::sync::Arc::new(pdf);
        let source = ExportSource {
            bytes: pdf.clone(),
            config: Default::default(),
            pages: sheets.len(),
            sizes: sheets.iter().map(|&(w, h)| (w as f32, h as f32)).collect(),
        };
        Ok(Self { exporter: Exporter::from_source(source), pdf, name, sheets, dpi, urls: Vec::new(), bytes: 0, max_bytes: MAX_JOB_BYTES })
    }

    pub fn total(&self) -> usize {
        self.sheets.len()
    }

    pub fn done(&self) -> usize {
        self.urls.len()
    }

    pub fn finished(&self) -> bool {
        self.urls.len() >= self.sheets.len()
    }

    /// Draw the next sheet and give its PNG to `sink`. Errors end the job.
    pub fn step(&mut self, sink: &mut dyn FnMut(Vec<u8>) -> Result<String, String>) -> Result<(), String> {
        let i = self.urls.len();
        let Some(&dpi) = self.dpi.get(i) else { return Ok(()) };
        let png = self.exporter.png(i, dpi)?;
        self.bytes = self.bytes.saturating_add(png.len() as u64);
        if self.bytes > self.max_bytes {
            return Err(format!(
                "the sheets need more than {} MB as pictures, too much to print from the browser; save the print-ready PDF and print it from a PDF viewer",
                self.max_bytes >> 20
            ));
        }
        let url = sink(png)?;
        self.urls.push(url);
        Ok(())
    }

    /// The page to print, once [`WebPrintJob::finished`].
    pub fn html(&self) -> String {
        print_document_html(&self.sheets, &self.urls)
    }

    /// The pictures' URLs, now owned by the caller (who revokes them).
    pub fn take_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.urls)
    }
}

impl Drop for WebPrintJob {
    /// A job dropped before printing (cancelled, failed, replaced) lets go of its pictures.
    fn drop(&mut self) {
        revoke(&std::mem::take(&mut self.urls));
    }
}

/// Let go of pictures handed out by [`blob_url`].
fn revoke(urls: &[String]) {
    #[cfg(target_arch = "wasm32")]
    for url in urls {
        let _ = web_sys::Url::revoke_object_url(url);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = urls;
}

/// A JavaScript exception as text: an error's `message` (read as a property, since an error from
/// the print frame isn't an `instanceof Error` here), a thrown string, else its debug form.
#[cfg(target_arch = "wasm32")]
fn js_error(e: wasm_bindgen::JsValue) -> String {
    if let Some(s) = e.as_string() {
        return s;
    }
    js_sys::Reflect::get(&e, &wasm_bindgen::JsValue::from_str("message"))
        .ok()
        .and_then(|m| m.as_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("{e:?}"))
}

/// A PNG as a `blob:` URL. The bytes move out of wasm memory into the browser's.
#[cfg(target_arch = "wasm32")]
pub(crate) fn blob_url(png: Vec<u8>) -> Result<String, String> {
    let err = js_error;
    let array = js_sys::Uint8Array::from(png.as_slice());
    drop(png);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("image/png");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&js_sys::Array::of1(&array), &opts).map_err(err)?;
    web_sys::Url::create_object_url_with_blob(&blob).map_err(err)
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// The iframe of the last browser print and its pictures, removed when the browser says the
    /// dialog closed, or at the latest when the next print starts.
    static LAST_FRAME: std::cell::RefCell<Option<(web_sys::HtmlIFrameElement, Vec<String>)>> = const { std::cell::RefCell::new(None) };
}

/// Remove the last print's iframe and let go of its pictures.
#[cfg(target_arch = "wasm32")]
fn clear_last_frame() {
    if let Some((frame, urls)) = LAST_FRAME.with(|f| f.borrow_mut().take()) {
        frame.remove();
        revoke(&urls);
    }
}

/// Open the browser's print dialog on `html` in a hidden iframe once its pictures (`urls`) are
/// decoded. What goes wrong after this returns (a picture that won't decode, a browser that
/// refuses to print from a frame) is pushed to `failed`, for the app to report.
#[cfg(target_arch = "wasm32")]
pub(crate) fn print_in_iframe(
    html: &str,
    urls: Vec<String>,
    failed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ctx: Option<egui::Context>,
) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let err = js_error;
    clear_last_frame();
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let body = document.body().ok_or("no document body")?;
    let frame: web_sys::HtmlIFrameElement = document.create_element("iframe").map_err(err)?.dyn_into().map_err(|_| "not an iframe")?;
    // Not `display: none` or hidden: some browsers print nothing from a frame that isn't laid out.
    frame.set_attribute("style", "position:fixed;right:0;bottom:0;width:0;height:0;border:0;opacity:0;pointer-events:none").map_err(err)?;
    frame.set_attribute("aria-hidden", "true").map_err(err)?;
    frame.set_attribute("tabindex", "-1").map_err(err)?;
    let on_load = {
        let frame = frame.clone();
        wasm_bindgen::closure::Closure::once_into_js(move || {
            wasm_bindgen_futures::spawn_local(async move {
                let report = |e: String| {
                    if let Ok(mut q) = failed.lock() {
                        q.push(e);
                    }
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                };
                let (Some(window), Some(doc)) = (frame.content_window(), frame.content_document()) else {
                    report("the print page didn't load".into());
                    return;
                };
                let images = doc.images();
                for i in 0..images.length() {
                    let Some(img) = images.item(i).and_then(|e| e.dyn_into::<web_sys::HtmlImageElement>().ok()) else { continue };
                    if let Err(e) = wasm_bindgen_futures::JsFuture::from(img.decode()).await {
                        report(format!("sheet {} couldn't be prepared for printing: {}", i + 1, err(e)));
                        return;
                    }
                }
                // Clean up when the dialog closes; browsers that don't say so are cleaned up by
                // the next print.
                let after = wasm_bindgen::closure::Closure::once_into_js(clear_last_frame);
                let _ = window.add_event_listener_with_callback("afterprint", after.unchecked_ref());
                if let Err(e) = window.print() {
                    report(format!("the browser wouldn't open its print dialog: {}", err(e)));
                }
            });
        })
    };
    frame.add_event_listener_with_callback("load", on_load.unchecked_ref()).map_err(err)?;
    frame.set_srcdoc(html);
    body.append_child(&frame).map_err(err)?;
    LAST_FRAME.with(|f| *f.borrow_mut() = Some((frame, urls)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: (f64, f64) = (612.0, 792.0);

    #[test]
    fn resolution_falls_with_the_sheet_count_and_long_jobs_are_refused() {
        assert_eq!(plan_dpi(&[LETTER]).unwrap(), vec![300.0]);
        assert_eq!(plan_dpi(&vec![LETTER; 100]).unwrap()[99], 300.0);
        assert_eq!(plan_dpi(&vec![LETTER; 101]).unwrap()[0], 200.0);
        assert_eq!(plan_dpi(&vec![LETTER; 300]).unwrap()[0], 200.0);
        assert_eq!(plan_dpi(&vec![LETTER; 301]).unwrap()[0], 150.0);
        assert_eq!(plan_dpi(&vec![LETTER; MAX_SHEETS]).unwrap().len(), MAX_SHEETS);
        let e = plan_dpi(&vec![LETTER; MAX_SHEETS + 1]).unwrap_err();
        assert!(e.contains("save the print-ready PDF"), "{e}");
        assert!(plan_dpi(&[]).is_err());
    }

    #[test]
    fn large_sheets_are_drawn_within_the_pixel_cap_and_bad_sizes_are_refused() {
        // ARCH E, 36 × 48 inches: 300 dpi would be 155 megapixels.
        let dpi = plan_dpi(&[(36.0 * 72.0, 48.0 * 72.0)]).unwrap()[0];
        assert!(dpi < 300.0 && dpi > 150.0, "{dpi}");
        let pixels = (36.0 * dpi) * (48.0 * dpi);
        assert!(pixels <= MAX_SHEET_PIXELS * 1.0001, "{pixels}");
        // A huge sheet still gets a usable floor rather than nothing.
        assert_eq!(plan_dpi(&[(14_400.0, 14_400.0)]).unwrap()[0], MIN_DPI);
        for bad in [(f64::NAN, 792.0), (612.0, f64::INFINITY), (0.0, 792.0), (-612.0, 792.0)] {
            assert!(plan_dpi(&[LETTER, bad]).unwrap_err().contains("sheet 2"));
        }
    }

    #[test]
    fn the_print_page_turns_sideways_sheets_names_other_sizes_and_ends_without_a_blank_page() {
        const A4: (f64, f64) = (595.28, 841.89);
        let sheets = [LETTER, (792.0, 612.0), A4, (612.2, 791.8), A4];
        let urls: Vec<String> = (0..5).map(|i| format!("blob:https://example.test/{i}")).collect();
        let html = print_document_html(&sheets, &urls);
        // The paper is the first sheet's.
        assert!(html.contains("@page{size:612pt 792pt;margin:0}"), "{html}");
        assert_eq!(html.matches("<div").count(), 5);
        // Letter landscape is turned onto Letter portrait, not given a page of its own.
        assert!(html.contains("<div class=\"turned\"><img alt=\"\" src=\"blob:https://example.test/1\"></div>"), "{html}");
        assert!(html.contains(".turned img{width:792pt;height:612pt;transform-origin:0 0;transform:translate(0,792pt) rotate(-90deg)}"), "{html}");
        // A4 gets one named page, shared by both A4 sheets; a size within half a point is the paper.
        assert_eq!(html.matches("@page s0{size:595.28pt 841.89pt;margin:0}").count(), 1);
        assert!(!html.contains("@page s1"), "{html}");
        assert_eq!(html.matches("<div class=\"s0\">").count(), 2);
        assert!(html.contains("<div><img alt=\"\" style=\"width:612pt;height:792pt\" src=\"blob:https://example.test/3\"></div>"));
        assert!(html.contains("div:last-child{break-after:auto"));
        let quoted = print_document_html(&[LETTER], &["blob:x\"><script>".to_string()]);
        assert!(!quoted.contains("<script>"), "{quoted}");
    }

    /// A PDF of blank pages, one per size in points.
    fn blank_pdf(sizes: &[(f64, f64)]) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        let n = sizes.len();
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
        for (w, h) in sizes {
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] >>"));
        }
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for o in offsets {
            out.push_str(&format!("{o:010} 00000 n \n"));
        }
        out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1));
        out.into_bytes()
    }

    /// A PNG's width and height, from its header.
    fn png_size(png: &[u8]) -> (u32, u32) {
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        (w, h)
    }

    #[test]
    fn a_job_draws_every_sheet_at_its_planned_resolution_then_builds_the_page() {
        let sheets = vec![LETTER, (792.0, 612.0), (297.64, 420.94)];
        let mut job = WebPrintJob::new(blank_pdf(&sheets), sheets.clone(), "doc.pdf".into()).unwrap();
        assert_eq!((job.done(), job.total(), job.finished()), (0, 3, false));
        let mut pngs = Vec::new();
        while !job.finished() {
            job.step(&mut |png| {
                pngs.push(png);
                Ok(format!("blob:test/{}", pngs.len()))
            })
            .unwrap();
        }
        assert_eq!(pngs.len(), 3);
        assert_eq!(png_size(&pngs[0]), (2550, 3300), "Letter at 300 dpi");
        assert_eq!(png_size(&pngs[1]), (3300, 2550), "landscape Letter at 300 dpi");
        let (w, h) = png_size(&pngs[2]);
        assert!((w as i64 - 1240).abs() <= 1 && (h as i64 - 1754).abs() <= 1, "A6 at 300 dpi: {w}×{h}");
        // A finished job does nothing more.
        job.step(&mut |_| panic!("no sheet left")).unwrap();
        let html = job.html();
        assert_eq!(html.matches("<img ").count(), 3);
        assert!(html.contains("blob:test/3"));
        assert_eq!(job.take_urls().len(), 3);
    }

    #[test]
    fn a_sink_error_or_a_page_that_isnt_there_stops_the_job() {
        let mut job = WebPrintJob::new(blank_pdf(&[LETTER]), vec![LETTER], "doc.pdf".into()).unwrap();
        assert_eq!(job.step(&mut |_| Err("out of memory".into())).unwrap_err(), "out of memory");
        assert_eq!(job.done(), 0);
        // The layout promised two sheets but the PDF has one: an error, not a panic.
        let mut short = WebPrintJob::new(blank_pdf(&[LETTER]), vec![LETTER, LETTER], "doc.pdf".into()).unwrap();
        short.step(&mut |_| Ok("blob:1".into())).unwrap();
        assert!(short.step(&mut |_| Ok("blob:2".into())).is_err());
        // A job whose pictures outgrow the byte cap stops instead of filling the tab's memory.
        let mut big = WebPrintJob::new(blank_pdf(&[LETTER, LETTER]), vec![LETTER, LETTER], "doc.pdf".into()).unwrap();
        big.max_bytes = 1;
        let e = big.step(&mut |_| Ok("blob:1".into())).unwrap_err();
        assert!(e.contains("save the print-ready PDF"), "{e}");
        assert_eq!(big.done(), 0);
        // Garbage bytes fail at the first sheet.
        let mut garbage = WebPrintJob::new(b"not a pdf".to_vec(), vec![LETTER], "x.pdf".into()).unwrap();
        assert!(garbage.step(&mut |_| Ok("blob:1".into())).is_err());
    }
}
