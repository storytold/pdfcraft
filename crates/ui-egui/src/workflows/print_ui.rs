//! The Print dialog (Acrobat's File ▸ Print, execution plan M10.5): printer, copies, grayscale;
//! pages to print (all, current, range with labels, the selected pages; odd/even, reverse); page sizing & handling
//! (Size, Poster, Multiple, Booklet); orientation; comments & forms; and a live preview of the
//! sheets. Printing sends the print-ready PDF to the system spooler; "Save as PDF" writes it.

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

mod body;
mod draft;

pub(crate) use body::body;
pub use draft::{Handling, PrintDraft, Which};

/// The preview pane, and the paper inside it (12 pt inset on each side).
const PREVIEW_W: f32 = 320.0;
const PREVIEW_H: f32 = 380.0;
const PREVIEW_INSET: f32 = 12.0;
pub(crate) const PREVIEW_BOX: (f32, f32) = (PREVIEW_W - 2.0 * PREVIEW_INSET, PREVIEW_H - 2.0 * PREVIEW_INSET);
/// Label printing reads the printed area of at most this many pages (each is a quick render on
/// the UI thread); further pages print whole.
pub(crate) const LABEL_CROP_PAGES: usize = 50;

/// Lists the printers off the UI thread (the desktop app sets [`PdfCraftApp::printer_source`]):
/// on Windows each driver's capabilities are read for its two-sided support, and CUPS' driverless discovery can wait on
/// the network. Without a source the dialog reads them as it opens, so tests stay deterministic.
pub type PrinterSource = std::sync::Arc<dyn Fn() -> Vec<spool::Printer> + Send + Sync>;

/// Where a printer's driver options come from: the spooler's (`spool::printer_options_in`, in the
/// UI language) unless a test stands in. Called off the GUI thread.
pub type OptionsSource = std::sync::Arc<dyn Fn(&str) -> Vec<spool::PrinterOption> + Send + Sync>;

/// The system spooler's printers, as the desktop app's [`PrinterSource`].
pub fn system_printers() -> PrinterSource {
    std::sync::Arc::new(spool::printers)
}

/// The Print dialog's work in the background: the printer list being read, and the jobs being
/// spooled, oldest first.
#[derive(Default)]
pub(crate) struct PrintTasks {
    #[cfg(not(target_arch = "wasm32"))]
    printers: Option<std::sync::mpsc::Receiver<Vec<spool::Printer>>>,
    #[cfg(not(target_arch = "wasm32"))]
    jobs: Vec<PrintJob>,
    /// The progress card is showing a print job (it is shared with other long jobs).
    #[cfg(not(target_arch = "wasm32"))]
    showing_progress: bool,
    /// The driver options being read, per printer (`read_options_in_background`).
    #[cfg(not(target_arch = "wasm32"))]
    options: Vec<(String, std::sync::mpsc::Receiver<Vec<spool::PrinterOption>>)>,
    /// The browser print being drawn, a sheet a frame (`web_print`).
    #[cfg(target_arch = "wasm32")]
    web_job: Option<crate::web_print::WebPrintJob>,
    /// What went wrong after the browser print page was handed over, with the print-ready PDF
    /// to download instead.
    #[cfg(target_arch = "wasm32")]
    web_failed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// The last browser print's PDF and name, for that fallback.
    #[cfg(target_arch = "wasm32")]
    web_last: Option<(String, std::sync::Arc<Vec<u8>>)>,
}

/// The printers to offer when no [`PrinterSource`] is set: the spooler's, or in a browser its
/// print dialog.
fn listed_printers() -> Vec<spool::Printer> {
    #[cfg(target_arch = "wasm32")]
    {
        vec![spool::Printer { name: crate::web_print::BROWSER_PRINTER.to_string(), default: true, two_sided: None, description: String::new() }]
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        spool::printers()
    }
}

/// The first list is still being read and the user hasn't chosen yet (#160): no printer is
/// selected only because none is known yet, so the dialog says it is looking rather than
/// offering Save as PDF, and its button waits. Printing then would have saved a PDF.
fn still_looking(d: &PrintDraft) -> bool {
    d.printers_loading && d.printer.is_none() && !d.printer_picked
}

/// A printer's name as the dialog shows it.
fn printer_label(name: &str) -> String {
    if name == crate::web_print::BROWSER_PRINTER { crate::web_print::browser_printer_label().to_string() } else { name.to_string() }
}

/// A job being spooled: its printer, the sheets sent so far of how many, and its worker's news.
#[cfg(not(target_arch = "wasm32"))]
struct PrintJob {
    printer: String,
    sheets: Option<(usize, usize)>,
    news: std::sync::mpsc::Receiver<JobNews>,
}

#[cfg(not(target_arch = "wasm32"))]
enum JobNews {
    /// `(sent, sheets)`: the spooler has this many of the job's sheets.
    Sent(usize, usize),
    /// The spooler's answer.
    Done(Result<String, String>),
}

/// The printer the dialog preselects: the system default, else the first.
fn default_printer(printers: &[spool::Printer]) -> Option<String> {
    printers.iter().find(|p| p.default).or(printers.first()).map(|p| p.name.clone())
}

/// Whether `printer` can still be chosen from `printers`: Save as PDF (`None`) always can.
fn offered(printer: Option<&str>, printers: &[spool::Printer]) -> bool {
    printer.is_none_or(|name| printers.iter().any(|p| p.name == name))
}

/// Asks for a frame when a worker thread ends, also when it panics: [`PdfCraftApp::poll_print`]
/// then sees the answer, or the disconnected channel of a worker that died, without waiting
/// for some other event to draw (#2108 review). Declared before the worker's sender, so the
/// sender is dropped first and the frame finds the channel closed.
#[cfg(not(target_arch = "wasm32"))]
struct WakeOnExit(Option<egui::Context>);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for WakeOnExit {
    fn drop(&mut self) {
        if let Some(ctx) = &self.0 {
            ctx.request_repaint();
        }
    }
}

impl PdfCraftApp {
    pub fn open_print(&mut self) {
        let Some((i, _)) = self.active_ids() else { return };
        let (printers, printers_loading) = match self.printer_source.clone() {
            // The list from the last time the dialog opened, until the fresh one arrives.
            #[cfg(not(target_arch = "wasm32"))]
            Some(source) => {
                self.list_printers_in_background(source);
                (self.print_draft.printers.clone(), true)
            }
            _ => (listed_printers(), false),
        };
        let default = default_printer(&printers);
        let current = self.views[i].current;
        let selected: Vec<usize> = self.views[i].selected.iter().copied().collect();
        let keep = std::mem::take(&mut self.print_draft);
        // The printer picked last time, Save as PDF included, while it is still listed (#3537).
        let picked = keep.printer_picked && offered(keep.printer.as_deref(), &printers);
        let printer = if picked { keep.printer.clone() } else { default };
        // Picked pages are what Print is for (Acrobat's "Selected pages"); without any, a choice
        // left over from another document falls back to the whole document.
        let which = match (selected.is_empty(), keep.which) {
            (false, _) => Which::Selected,
            (true, Which::Selected) => Which::All,
            (true, other) => other,
        };
        self.print_draft = PrintDraft {
            crops: Vec::new(),
            crop_pages: None,
            printers,
            printers_loading,
            printer_picked: picked,
            units: self.units,
            printer,
            current_page: current,
            sheet: 0,
            selected,
            which,
            ..keep
        };
        self.dialog = Some(crate::Dialog::Print);
    }

    /// Label printing: find each printed page's printed area (a quick low-resolution render, as
    /// Fit Visible does) and which way its text runs. Only the pages being printed are read, at
    /// most [`LABEL_CROP_PAGES`] of them (the rest print whole); read again when they change.
    pub(crate) fn read_label_crops(&mut self, id: pdfcraft_engine::DocId) {
        let Some(doc) = self.session.get(id) else { return };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let mut pages = self.print_draft.settings(doc.info.pages.len(), &labels).map(|s| s.pages).unwrap_or_default();
        pages.sort_unstable();
        pages.dedup();
        pages.truncate(LABEL_CROP_PAGES);
        if self.print_draft.crop_pages.as_ref() == Some(&pages) {
            return;
        }
        let up = self.session.print_text_up(id, &pages);
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        self.print_draft.crops = doc
            .info
            .pages
            .iter()
            .enumerate()
            .map(|(page, p)| {
                let (w, h) = (f64::from(p.width), f64::from(p.height));
                if pages.binary_search(&page).is_err() || w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let out = r.render(pdfcraft_render::RenderRequest {
                    page,
                    kind: pdfcraft_render::RequestKind::Pixels,
                    tile: None,
                    scale: (1000.0 / w.max(h) as f32).clamp(0.1, 4.0),
                    tag: 0,
                    preview: false,
                });
                let [u0, v0, u1, v1] = crate::zoom_snap::ink_bounds(out.width as usize, out.height as usize, &out.rgba)?;
                // Image rows run down from the top; display space runs up. Half a millimetre of slack.
                let pad = 1.5;
                let rect = [f64::from(u0) * w - pad, (1.0 - f64::from(v1)) * h - pad, f64::from(u1) * w + pad, (1.0 - f64::from(v0)) * h + pad];
                Some(print::Crop { rect, text_up: up.get(page).copied().unwrap_or(false) })
            })
            .collect();
        self.print_draft.crop_pages = Some(pages);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn list_printers_in_background(&mut self, source: PrinterSource) {
        // A list already being read is as fresh as a new one would be.
        if self.print_tasks.printers.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = WakeOnExit(self.ctx.clone());
        std::thread::spawn(move || {
            let _wake = wake;
            let tx = tx;
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(source());
        });
        self.print_tasks.printers = Some(rx);
    }

    /// Read `printer`'s driver options off the GUI thread (#1818); `poll_print` picks them up.
    /// A read already under way for the same printer is left to finish.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_options_in_background(&mut self, printer: String) {
        if self.print_tasks.options.iter().any(|(p, _)| *p == printer) {
            return;
        }
        let source: OptionsSource = self.options_source.clone().unwrap_or_else(|| {
            // The driver's own names in the UI language, as it is when Properties… is clicked.
            let lang = crate::i18n::current().code();
            std::sync::Arc::new(move |p: &str| spool::printer_options_in(p, lang))
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = WakeOnExit(self.ctx.clone());
        let name = printer.clone();
        let spawned = std::thread::Builder::new().name("printer options".into()).spawn(move || {
            let _wake = wake;
            let tx = tx;
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(source(&name));
        });
        match spawned {
            Ok(_) => self.print_tasks.options.push((printer, rx)),
            // No thread to read them on: the printer offers nothing this time.
            Err(_) => self.receive_options(printer, Vec::new()),
        }
    }

    /// The options read for `printer` arrived, or the read died. A read that a later click
    /// superseded (another printer's is awaited) is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    fn receive_options(&mut self, printer: String, options: Vec<spool::PrinterOption>) {
        let d = &mut self.print_draft;
        if d.driver_options_loading.as_deref() == Some(printer.as_str()) {
            d.driver_options_loading = None;
            d.driver_options = Some((printer, options));
        }
    }

    /// Pick up a printer list, driver options or a spooled job that finished (each frame).
    pub(crate) fn poll_print(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.poll_web_print();
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(printer) = self.print_draft.read_options_for.take() {
                self.read_options_in_background(printer);
            }
            let mut arrived = Vec::new();
            self.print_tasks.options.retain(|(printer, rx)| match rx.try_recv() {
                Ok(options) => {
                    arrived.push((printer.clone(), options));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                // A read that died offers nothing; the panel says so rather than wait.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    arrived.push((printer.clone(), Vec::new()));
                    false
                }
            });
            for (printer, options) in arrived {
                self.receive_options(printer, options);
            }
            if let Some(rx) = &self.print_tasks.printers {
                let printers = match rx.try_recv() {
                    Ok(p) => Some(p),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    // A source that died lists nothing new; the dialog keeps what it has.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(self.print_draft.printers.clone()),
                };
                if let Some(printers) = printers {
                    self.print_tasks.printers = None;
                    let d = &mut self.print_draft;
                    // A picked printer that is no longer listed falls back to the default too.
                    if !d.printer_picked || !offered(d.printer.as_deref(), &printers) {
                        d.printer_picked = false;
                        d.printer = default_printer(&printers);
                    }
                    d.printers = printers;
                    d.printers_loading = false;
                }
            }
            let mut finished = Vec::new();
            self.print_tasks.jobs.retain_mut(|job| {
                loop {
                    match job.news.try_recv() {
                        Ok(JobNews::Sent(sent, sheets)) => job.sheets = Some((sent, sheets)),
                        Ok(JobNews::Done(r)) => {
                            finished.push((job.printer.clone(), r));
                            break false;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break true,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished.push((job.printer.clone(), Err("the print job stopped unexpectedly".into())));
                            break false;
                        }
                    }
                }
            });
            self.show_print_progress();
            for (printer, result) in finished {
                self.report_print_job(&printer, result);
            }
        }
    }

    /// The oldest job being spooled on the progress card, counting its sheets as they reach the
    /// spooler, while no other long job holds the card.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_print_progress(&mut self) {
        let Some(job) = self.print_tasks.jobs.first() else {
            if std::mem::take(&mut self.print_tasks.showing_progress) {
                self.progress_notice = None;
            }
            return;
        };
        if self.progress_notice.is_some() && !self.print_tasks.showing_progress {
            return;
        }
        let printer = job.printer.as_str();
        let (label, fraction) = match job.sheets {
            Some((sent, sheets)) if sheets > 0 => {
                // The sheet on its way: one past those sent, until the last is.
                let sheet = sent.saturating_add(1).min(sheets).to_string();
                let label = crate::i18n::fmt(
                    tl!("Sending sheet {s} of {n} to {printer}…"),
                    &[("s", &sheet), ("n", &sheets.to_string()), ("printer", printer)],
                );
                (label, sent as f32 / sheets as f32)
            }
            _ => (crate::i18n::fmt(tl!("Sending to {printer}…"), &[("printer", printer)]), 0.0),
        };
        self.progress_notice = Some(crate::widgets::ProgressNotice { label, fraction, cancellable: false });
        self.print_tasks.showing_progress = true;
        // The bar's sweep keeps moving between sheets.
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn report_print_job(&mut self, printer: &str, result: Result<String, String>) {
        match result {
            Ok(msg) if msg.is_empty() => self.notify(crate::i18n::fmt(tl!("Sent to {printer}"), &[("printer", printer)])),
            Ok(msg) => self.notify(crate::i18n::fmt(tl!("Sent to {printer}: {msg}"), &[("printer", printer), ("msg", &msg)])),
            Err(e) => self.notify_error(e),
        }
    }

    /// Print (or save) with the dialog's settings. Returns `true` on success. Save as PDF without
    /// a preset path returns `true` once the save picker is showing; the file is written on a
    /// later frame, when the user has chosen where.
    pub fn print_now(&mut self) -> bool {
        // What's typed in a form field is part of what's printed (#166).
        if !self.commit_form_typing() {
            return false;
        }
        let Some((_, id)) = self.active_ids() else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let settings = match self.print_draft.settings(doc.info.pages.len(), &labels) {
            Ok(s) => s,
            Err(e) => {
                self.notify_tr(&e);
                return false;
            }
        };
        let name = doc.name.clone();
        let bytes = match self.session.print_pdf(id, &settings) {
            Ok(b) => b,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        match self.print_draft.printer.clone() {
            #[cfg(target_arch = "wasm32")]
            Some(printer) if printer == crate::web_print::BROWSER_PRINTER => {
                let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
                match print::layout(&sizes, &settings) {
                    Ok(sheets) => self.print_in_browser(name, bytes, sheets.iter().map(|s| s.size).collect()),
                    Err(e) => {
                        self.notify_error(e);
                        false
                    }
                }
            }
            Some(printer) => self.send_to_printer(printer, bytes, self.print_draft.job(&name)),
            None => self.save_print_pdf(&name, bytes),
        }
    }

    /// Spool a job off the UI thread: on Windows every sheet is rendered at the printer's
    /// resolution first, which can take minutes for a long document. The dialog closes at once,
    /// the progress card counts the sheets as they reach the spooler, and the spooler's answer
    /// arrives as a notice.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let repaint = || {
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            };
            // The receiver may be gone (the app quit): nothing to report to then.
            let result = spool::submit_with_progress(&bytes, &job, &mut |sent, sheets| {
                let _ = tx.send(JobNews::Sent(sent, sheets));
                repaint();
            });
            let _ = tx.send(JobNews::Done(result.map_err(|e| e.to_string())));
            repaint();
        });
        self.print_tasks.jobs.push(PrintJob { printer, sheets: None, news: rx });
        self.show_print_progress();
        true
    }

    /// A browser has no spooler: `submit` answers at once.
    #[cfg(target_arch = "wasm32")]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> bool {
        let result = spool::submit(&bytes, &job).map_err(|e| e.to_string());
        let ok = result.is_ok();
        self.report_print_job(&printer, result);
        ok
    }

    /// Print through the browser's print dialog: start drawing `sheets` (the pages of the
    /// print-ready `bytes`), a sheet a frame in [`PdfCraftApp::poll_print`]. A print still being
    /// prepared is replaced. The dialog closes at once.
    #[cfg(target_arch = "wasm32")]
    fn print_in_browser(&mut self, name: String, bytes: Vec<u8>, sheets: Vec<(f64, f64)>) -> bool {
        match crate::web_print::WebPrintJob::new(bytes, sheets, name) {
            Ok(job) => {
                let n = job.total().to_string();
                self.print_tasks.web_job = Some(job);
                self.notify_fmt("Preparing {n} sheets for printing…", &[("n", &n)]);
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint();
                }
                true
            }
            Err(e) => {
                self.notify_error(e);
                false
            }
        }
    }

    /// Draw the next sheet of the browser print, and hand the finished page to the browser.
    #[cfg(target_arch = "wasm32")]
    fn poll_web_print(&mut self) {
        let failed: Vec<String> = self.print_tasks.web_failed.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        if let Some(e) = failed.into_iter().next() {
            self.web_print_failed(&e);
        }
        let Some(job) = self.print_tasks.web_job.as_mut() else { return };
        if !job.finished() {
            if let Err(e) = job.step(&mut crate::web_print::blob_url) {
                self.print_tasks.web_job = None;
                self.notify_error(e);
                return;
            }
            let (done, total) = (job.done().to_string(), job.total().to_string());
            self.notify_fmt("Preparing sheet {done} of {total} for printing…", &[("done", &done), ("total", &total)]);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint();
            }
            return;
        }
        let Some(mut job) = self.print_tasks.web_job.take() else { return };
        let html = job.html();
        let urls = job.take_urls();
        self.print_tasks.web_last = Some((job.name.clone(), job.pdf.clone()));
        match crate::web_print::print_in_iframe(&html, urls, self.print_tasks.web_failed.clone(), self.ctx.clone()) {
            Ok(()) => self.notify_tr("Opening the browser's print dialog…"),
            Err(e) => self.web_print_failed(&e),
        }
    }

    /// The browser wouldn't print (a sandboxed frame, a picture it couldn't decode): offer the
    /// print-ready PDF as a download instead, so the work isn't lost.
    #[cfg(target_arch = "wasm32")]
    fn web_print_failed(&mut self, error: &str) {
        let Some((name, pdf)) = self.print_tasks.web_last.take() else {
            self.notify_error(error);
            return;
        };
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        match crate::editing::download(&file, &pdf) {
            Ok(()) => self.notify_fmt("Couldn't print from the browser ({e}); downloaded {name} to print instead", &[("e", error), ("name", &file)]),
            Err(_) => self.notify_error(error),
        }
    }

    /// Print ▸ Save as PDF on the desktop: write to `save_override` (tests and automation), or
    /// ask where and write on a later frame. Returns `true` once written, or once the save picker
    /// is showing.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let write = move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
            Ok(()) => {
                app.notify_fmt("Saved the print-ready PDF to {path}", &[("path", &path.display().to_string())]);
                true
            }
            Err(e) => {
                app.notify_fmt("Could not save: {e}", &[("e", &e.to_string())]);
                false
            }
        };
        match self.save_override.clone() {
            Some(p) => write(self, p.into()),
            None => {
                let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
                let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{stem} (print).pdf"));
                self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, path| {
                    write(app, path);
                })
            }
        }
    }

    /// Print ▸ Save as PDF in a browser: download it, as Save does (#170). There is no folder to
    /// choose and no file system to write to, so `save_override` doesn't apply.
    #[cfg(target_arch = "wasm32")]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        match crate::editing::download(&file, &bytes) {
            Ok(()) => {
                self.notify_fmt("Downloaded {name}", &[("name", &file)]);
                true
            }
            Err(e) => {
                self.notify_fmt("Couldn't download {name}: {e}", &[("name", &file), ("e", &e)]);
                false
            }
        }
    }
}

/// The longest side, in pixels, of one page's print-preview raster (also kept within the GPU's
/// `max_texture_side`): an Actual-size print of a large page would otherwise ask for a raster
/// bigger than a texture can be.
pub(crate) const PREVIEW_MAX_SIDE: f32 = 4096.0;
/// The most bytes of print-preview rasters kept at once. The current sheet comes first, then the
/// next and the previous one; a neighbour that doesn't fit isn't rendered ahead.
pub(crate) const PREVIEW_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

/// Pages on the current sheet and its neighbours, each with the device pixels per point that
/// fill the preview pane (so the picture is not a stretched thumbnail), nearest sheet first and
/// within [`PREVIEW_MAX_SIDE`] (and `max_side`, the GPU's texture limit) and [`PREVIEW_BYTES`].
pub(crate) fn preview_rasters(d: &PrintDraft, sizes: &[(f64, f64)], labels: &[String], ppp: f32, max_side: f32) -> Vec<(usize, f32)> {
    let Ok(settings) = d.settings(sizes.len(), labels) else { return Vec::new() };
    let Ok(sheets) = print::layout(sizes, &settings) else { return Vec::new() };
    if sheets.is_empty() {
        return Vec::new();
    }
    let ppp = if ppp.is_finite() { ppp.max(1.0) } else { 1.0 };
    let side = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
    let i = d.sheet.min(sheets.len() - 1);
    // Nearest first: the sheet on screen, then the next one, then the previous one.
    let order = [Some(i), i.checked_add(1).filter(|n| *n < sheets.len()), i.checked_sub(1)];
    let mut out: Vec<(usize, f32, f64)> = Vec::new();
    let mut bytes = 0.0f64;
    for sheet in order.into_iter().flatten().filter_map(|s| sheets.get(s)) {
        let (sw, sh) = (sheet.size.0 as f32, sheet.size.1 as f32);
        if !(sw.is_finite() && sh.is_finite()) || sw < 1.0 || sh < 1.0 {
            continue;
        }
        let k = (PREVIEW_BOX.0 / sw).min(PREVIEW_BOX.1 / sh);
        for pl in &sheet.placed {
            let [a, b, _, _, _, _] = pl.matrix.0;
            let placed = (a.hypot(b) as f32) * k * ppp;
            let Some(&(pw, ph)) = sizes.get(pl.page) else { continue };
            let long_pt = pw.max(ph) as f32;
            if !placed.is_finite() || placed < 0.05 || !long_pt.is_finite() || long_pt < 1.0 {
                continue;
            }
            let scale = placed.min(64.0).min(side / long_pt);
            let cost = (pw * f64::from(scale)).ceil() * (ph * f64::from(scale)).ceil() * 4.0;
            if let Some(slot) = out.iter_mut().find(|(page, _, _)| *page == pl.page) {
                if scale > slot.1 && bytes - slot.2 + cost <= PREVIEW_BYTES {
                    bytes += cost - slot.2;
                    (slot.1, slot.2) = (scale, cost);
                }
            } else if out.len() < 48 && (out.is_empty() || bytes + cost <= PREVIEW_BYTES) {
                bytes += cost;
                out.push((pl.page, scale, cost));
            }
        }
    }
    out.into_iter().map(|(page, scale, _)| (page, scale)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_page_preview_uses_positions_and_selection_guards() {
        for labels in [["i", "ii", "iii", "1", "2"], ["1", "1", "1", "1", "1"], ["1", "2", "3", "4", "5"]] {
            let labels: Vec<String> = labels.into_iter().map(str::to_owned).collect();
            for current_page in 0..5 {
                let draft = PrintDraft { which: Which::Current, current_page, ..Default::default() };
                let rasters = preview_rasters(&draft, &[(200.0, 300.0); 5], &labels, 1.0, 8192.0);
                assert_eq!(rasters.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![current_page]);
            }
        }
        let labels: Vec<String> = ["i", "ii", "iii", "1", "2"].into_iter().map(str::to_owned).collect();
        let mut draft = PrintDraft { which: Which::Selected, selected: vec![0, 3], ..Default::default() };
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0, 3]);
        draft.which = Which::All;
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0, 1, 2, 3, 4]);
        draft.which = Which::Current;
        draft.reverse = true;
        draft.subset = Subset::Odd;
        assert_eq!(draft.settings(5, &labels).unwrap().pages, vec![0]);
        draft.subset = Subset::Even;
        assert!(draft.settings(5, &labels).is_err());
        draft.subset = Subset::All;
        for (count, current) in [(0, 0), (5, 5), (5, usize::MAX)] {
            draft.current_page = current;
            assert!(draft.settings(count, &labels).is_err());
        }
    }

    #[test]
    fn composed_sheets_keep_exact_scaling_and_the_dialogs_duplex_choice() {
        let draft = PrintDraft { duplex: Some(spool::Duplex::LongEdge), ..PrintDraft::default() };
        let job = draft.job("report.pdf");
        assert_eq!(job.scaling, spool::Scaling::Exact);
        assert_eq!(job.duplex, Some(spool::Duplex::LongEdge));
        let off = PrintDraft { duplex: Some(spool::Duplex::Off), ..PrintDraft::default() };
        assert_eq!(off.job("report.pdf").duplex, Some(spool::Duplex::Off));
    }

    /// Untouched, Two-sided sends nothing, so the duplex set in the printer's Properties applies
    /// (#2422); cut and stack still asks for single-sided sheets.
    #[test]
    fn two_sided_follows_the_printer_until_chosen() {
        assert_eq!(PrintDraft::default().duplex, None);
        assert_eq!(PrintDraft::default().job("report.pdf").duplex, None);
        let mut cut = PrintDraft { handling: Handling::Multiple, order: PageOrder::CutStack, ..PrintDraft::default() };
        assert_eq!(cut.job("report.pdf").duplex, Some(spool::Duplex::Off));
        assert!(cut.settings(4, &[]).is_ok(), "the printer's default doesn't block cut and stack");
        cut.duplex = Some(spool::Duplex::ShortEdge);
        assert!(cut.settings(4, &[]).is_err());
    }

    /// A job being spooled holds the progress card, counting its sheets as the spooler reports
    /// them; the card goes when the job ends and its answer arrives as a notice. A card another
    /// long job is showing is left alone.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_progress_card_counts_the_sheets_of_a_print_job() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.print_tasks.jobs.push(PrintJob { printer: "Office".into(), sheets: None, news: rx });
        app.poll_print();
        assert_eq!(app.progress_notice.as_ref().map(|p| p.label.as_str()), Some("Sending to Office…"));

        tx.send(JobNews::Sent(0, 3)).unwrap();
        tx.send(JobNews::Sent(1, 3)).unwrap();
        app.poll_print();
        let shown = app.progress_notice.clone().unwrap();
        assert_eq!(shown.label, "Sending sheet 2 of 3 to Office…");
        assert!((shown.fraction - 1.0 / 3.0).abs() < 1e-4 && !shown.cancellable, "{shown:?}");

        tx.send(JobNews::Sent(3, 3)).unwrap();
        tx.send(JobNews::Done(Ok(String::new()))).unwrap();
        app.poll_print();
        assert!(app.progress_notice.is_none() && app.print_tasks.jobs.is_empty());
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Sent to Office"));

        // Another job's card stays; a worker that vanished is reported, not left spinning.
        let other = crate::widgets::ProgressNotice { label: "Optimizing…".into(), fraction: 0.5, cancellable: true };
        app.progress_notice = Some(other.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        app.print_tasks.jobs.push(PrintJob { printer: "Office".into(), sheets: None, news: rx });
        tx.send(JobNews::Sent(1, 2)).unwrap();
        app.poll_print();
        assert_eq!(app.progress_notice, Some(other));
        drop(tx);
        app.poll_print();
        assert!(app.print_tasks.jobs.is_empty() && app.progress_notice.is_some());
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("the print job stopped unexpectedly"));
    }

    /// Issue #1308: Current page is the page on screen, even when a later page is labelled with
    /// its number. A typed range still reads labels first.
    #[test]
    fn current_page_is_the_viewed_page_not_a_label() {
        let labels: Vec<String> = ["i", "ii", "iii", "1", "2"].map(String::from).to_vec();
        for (current, want) in [(0, vec![0]), (1, vec![1]), (3, vec![3])] {
            let d = PrintDraft { which: Which::Current, current_page: current, ..PrintDraft::default() };
            assert_eq!(d.settings(5, &labels).map(|s| s.pages), Ok(want), "viewing page {}", current + 1);
        }
        let typed = PrintDraft { which: Which::Range, range: "1".into(), ..PrintDraft::default() };
        assert_eq!(typed.settings(5, &labels).map(|s| s.pages), Ok(vec![3]));
    }

    #[test]
    fn print_preview_rasters_stay_within_the_texture_limit() {
        // Actual size on a 14,400 pt (200 in) page would ask for a raster far past any texture.
        let d = PrintDraft { size: SizeMode::Actual, ..PrintDraft::default() };
        let sizes = vec![(14_400.0, 14_400.0); 2];
        let labels = vec!["1".into(), "2".into()];
        for max_side in [2048.0, 8192.0, 16384.0, f32::NAN, 0.0] {
            for (page, scale) in preview_rasters(&d, &sizes, &labels, 4.0, max_side) {
                let long = 14_400.0 * scale;
                let cap = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
                assert!(long <= cap + 1.0, "page {page} at {max_side}: {long} px");
            }
        }
    }

    #[test]
    fn print_preview_rasters_keep_the_current_sheet_within_the_byte_budget() {
        // Many large pages: the current sheet always gets its raster; neighbours only while the
        // budget lasts, and the total stays within it.
        let d = PrintDraft { sheet: 5, ..PrintDraft::default() };
        let sizes = vec![(2_000.0, 2_000.0); 12];
        let labels: Vec<String> = (1..=12).map(|n| n.to_string()).collect();
        let rasters = preview_rasters(&d, &sizes, &labels, 4.0, 16384.0);
        assert_eq!(rasters.first().map(|r| r.0), Some(5), "the sheet on screen comes first: {rasters:?}");
        let total: f64 = rasters.iter().map(|(_, s)| (2_000.0 * f64::from(*s)).ceil().powi(2) * 4.0).sum();
        assert!(total <= PREVIEW_BYTES || rasters.len() == 1, "{total} bytes for {rasters:?}");
    }

    #[test]
    fn print_preview_is_sharper_than_a_thumbnail_on_a_high_dpi_screen() {
        let d = PrintDraft::default();
        let sizes = vec![(612.0, 792.0); 3];
        let labels = vec!["1".into(), "2".into(), "3".into()];
        let rasters = preview_rasters(&d, &sizes, &labels, 2.0, 8192.0);
        let scale = rasters.iter().find(|(page, _)| *page == 0).map(|(_, s)| *s).unwrap();
        // A 132 pt thumbnail at 2× is about 0.43 px/pt. The preview pane needs roughly twice that.
        assert!(scale > 0.7, "letter page in the preview at 2 px/pt: {scale}");
        assert!(rasters.iter().any(|(page, _)| *page == 1), "the next sheet is ready");
        let mut later = d.clone();
        later.sheet = 2;
        let rasters = preview_rasters(&later, &sizes, &labels, 2.0, 8192.0);
        assert!(rasters.iter().any(|(page, _)| *page == 2));
    }
}
